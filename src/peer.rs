//! Paired devices: end-to-end encrypted links between this CodeAny and the
//! user's other machines, with keys only those machines ever hold.
//!
//! Each CodeAny has an X25519 identity (`~/.codeany/nex-identity.json`, 0600).
//! Two of them pair the way Termany's devices do:
//!
//! ```text
//!   device A (connects)                       device B (is connected to)
//!   "Connect a device" → paste B's            shows its authorization code
//!   authorization code → shows a
//!   connection code, waits ..............→   "Verify access" → type that code
//!   "Connected to B"                    ←.... B now trusts A's key
//! ```
//!
//! The authorization code carries B's public key, its LAN / Tailscale
//! addresses and a secret that only lets a device *ask* to connect. The
//! connection code is derived from both public keys, so a request swapped on
//! the way shows a different code than the one B computes.
//!
//! Every link is a [Noise](https://noiseprotocol.org) session over TCP:
//! `IK` between paired devices (both static keys authenticated, ephemeral
//! keys for forward secrecy), `IKpsk2` for a pairing request (the secret from
//! the authorization code as PSK). The same listener serves LAN / tailnet
//! connections and, through the `codeany.e2e` relay service, connections the
//! nex relay carries — it then only ever sees ciphertext.
//!
//! After the handshake the client sends one JSON line (`{"service":"agl"}`,
//! `{"ping":true}`, …), the host answers one, and a service request then
//! carries a WebSocket both ways.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use futures::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, DuplexStream},
    net::{TcpListener, TcpStream},
};
use tokio_tungstenite::tungstenite::Message;

use crate::agl_bridge;
use crate::i18n::{t, tf};

/// Prefix of a shareable public key (base64url, 32 bytes), as in nex.
pub const PUBLIC_KEY_PREFIX: &str = "nexpk1.";
const INVITE_PREFIX: &str = "codeany1.";
/// Where CodeAny listens for paired devices unless told otherwise; the dev
/// build (`~/.codeany-dev`) uses the next port so both can run.
pub const DEFAULT_PORT: u16 = 47430;
/// The relay service that carries this protocol for paired devices.
pub const E2E_SERVICE: &str = "codeany.e2e";

const PATTERN_PAIRED: &str = "Noise_IK_25519_ChaChaPoly_SHA256";
const PATTERN_REQUEST: &str = "Noise_IKpsk2_25519_ChaChaPoly_SHA256";
const PATTERN_INVITE: &str = "Noise_NN_25519_ChaChaPoly_SHA256";
const MODE_PAIRED: u8 = b'K';
const MODE_REQUEST: u8 = b'Q';
/// Asks for the authorization code; only over the relay (loopback here).
const MODE_INVITE: u8 = b'I';

const MAX_FRAME: usize = 65_535;
const CHUNK: usize = 16 * 1024;
const MAX_LINE: usize = 64 * 1024;
const MAX_REQUESTS: usize = 20;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
const DIAL_TIMEOUT: Duration = Duration::from_millis(1500);
/// A device that did not answer directly is not tried again for this long
/// before a connection falls back to the relay.
const DIRECT_RETRY: Duration = Duration::from_secs(30);
/// No 0/O, 1/I: the code is read off one screen and typed into another.
const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

// ---------------------------------------------------------------------------
// Identity and saved state
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    /// `nexpk1.…`: safe to show and share.
    public_key: String,
    /// base64url raw X25519 private key.
    private_key: String,
}

impl Identity {
    fn private(&self) -> Vec<u8> {
        B64.decode(&self.private_key).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetKind {
    Lan,
    Tailscale,
    Custom,
}

impl NetKind {
    pub fn label(self) -> &'static str {
        match self {
            NetKind::Lan => "LAN",
            NetKind::Tailscale => "Tailscale",
            NetKind::Custom => "Direct",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub kind: NetKind,
}

/// A device paired with this one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub public_key: String,
    /// Its own name, as it last told us.
    pub name: String,
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    /// Its id on the nex relay, when it was signed in.
    #[serde(default)]
    pub nex_device_id: Option<String>,
    #[serde(default)]
    pub added_at: u64,
    /// We asked it to connect and it has not verified us yet: not trusted
    /// here until it has.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    /// What the user renamed it to here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

impl Peer {
    /// The id it has in device lists and in threads that run on it.
    pub fn id(&self) -> String {
        peer_id(&self.public_key)
    }

    pub fn display_name(&self) -> String {
        self.alias.clone().unwrap_or_else(|| self.name.clone())
    }

    /// Whether `device` (a peer id or nex device id) names this peer.
    pub fn is(&self, device: &str) -> bool {
        self.id() == device || self.nex_device_id.as_deref() == Some(device)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeersFile {
    #[serde(default)]
    peers: Vec<Peer>,
    /// Only paired devices may use this machine, even through the account.
    #[serde(default)]
    strict: bool,
    /// Before codes expired, one secret served every authorization code.
    /// Kept so old files still read; no longer honoured.
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    pair_secret: Option<String>,
    /// Names given here to account devices (relay id → name).
    #[serde(default)]
    aliases: HashMap<String, String>,
    /// This device's name while signed out.
    #[serde(default)]
    device_name: Option<String>,
    /// Let other devices run agents and read threads here.
    #[serde(default)]
    share: Option<bool>,
}

/// A device asking to connect; its connection code is only on its screen.
#[derive(Clone, Debug)]
pub struct Request {
    pub name: String,
    pub public_key: String,
    invite: Invite,
}

/// How a paired device answers directly right now.
#[derive(Clone, Debug)]
pub struct DirectStatus {
    /// Round trip once connected, like a ping.
    pub rtt_ms: Option<u32>,
    /// The address that answered.
    pub endpoint: Option<Endpoint>,
    /// Why no address answers, when none does.
    pub note: Option<String>,
    /// Whether it shares its agents, as it says.
    pub share: bool,
    pub at: Instant,
}

struct State {
    file: PeersFile,
    requests: Vec<Request>,
    /// The port the listener got.
    port: Option<u16>,
    tailscale_name: Option<String>,
    direct: HashMap<String, DirectStatus>,
    /// The secret of the last authorization code copied, and when it stops
    /// working. Kept in memory only: a restart voids it too.
    offer: Option<(Vec<u8>, Instant)>,
    /// Whether that code was shown as a QR code: then the first device to
    /// ask with it is connected at once, and the code is used up.
    offer_scanned: bool,
    /// Devices a QR code connected, not yet announced.
    scanned_in: Vec<String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        file: read_file(&peers_path()),
        requests: Vec::new(),
        port: None,
        tailscale_name: None,
        direct: HashMap::new(),
        offer: None,
        offer_scanned: false,
        scanned_in: Vec::new(),
    })
});

static IDENTITY: LazyLock<Identity> = LazyLock::new(load_identity);

fn identity_path() -> PathBuf {
    crate::paths::home().join("nex-identity.json")
}

fn peers_path() -> PathBuf {
    crate::paths::home().join("nex-peers.json")
}

fn read_file(path: &std::path::Path) -> PeersFile {
    let mut file: PeersFile = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    file.peers.retain(|p| parse_public_key(&p.public_key).is_ok());
    if file.peers.is_empty() {
        file.strict = false;
    }
    file
}

/// Write `value` readable by this user only, atomically.
pub(crate) fn write_secret(path: &std::path::Path, value: &impl Serialize) -> Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(&tmp)?.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn load_identity() -> Identity {
    let path = identity_path();
    if let Some(saved) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Identity>(&text).ok())
        .filter(|id| parse_public_key(&id.public_key).is_ok() && id.private().len() == 32)
    {
        return saved;
    }
    let keypair = snow::Builder::new(PATTERN_PAIRED.parse().expect("noise pattern"))
        .generate_keypair()
        .expect("generate an X25519 key pair");
    let identity = Identity {
        public_key: format!("{PUBLIC_KEY_PREFIX}{}", B64.encode(&keypair.public)),
        private_key: B64.encode(&keypair.private),
    };
    if let Err(err) = write_secret(&path, &identity) {
        eprintln!("codeany: cannot save the device identity: {err:#}");
    }
    identity
}

fn with_file<T>(f: impl FnOnce(&mut PeersFile) -> T) -> T {
    let mut state = STATE.lock().unwrap();
    let out = f(&mut state.file);
    if let Err(err) = write_secret(&peers_path(), &state.file) {
        eprintln!("codeany: cannot save paired devices: {err:#}");
    }
    out
}

// ---------------------------------------------------------------------------
// Keys, codes, ids
// ---------------------------------------------------------------------------

/// The raw 32 bytes of a `nexpk1.…` key.
pub fn parse_public_key(key: &str) -> Result<Vec<u8>> {
    let body = key
        .trim()
        .strip_prefix(PUBLIC_KEY_PREFIX)
        .ok_or_else(|| anyhow!("a public key starts with '{PUBLIC_KEY_PREFIX}'"))?;
    let raw = B64.decode(body).map_err(|_| anyhow!("not a valid public key"))?;
    if raw.len() != 32 {
        bail!("not a valid public key");
    }
    Ok(raw)
}

fn key_text(raw: &[u8]) -> String {
    format!("{PUBLIC_KEY_PREFIX}{}", B64.encode(raw))
}

pub fn public_key() -> String {
    IDENTITY.public_key.clone()
}

fn code_chars(digest: &[u8], n: usize) -> String {
    digest[..n].iter().map(|b| CODE_ALPHABET[(*b % 32) as usize] as char).collect()
}

/// A short, human-comparable fingerprint of a public key: `ABCD-EFGH-…`.
// Not shown in Settings any more; kept for checking a key by eye.
#[allow(dead_code)]
pub fn fingerprint(public_key: &str) -> String {
    let raw = parse_public_key(public_key).unwrap_or_default();
    let digest = Sha256::digest(&raw);
    let chars = code_chars(&digest, 20);
    chars.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

/// The connection code two devices compute from both their keys.
pub fn pairing_code(a: &str, b: &str) -> String {
    let mut keys = [a.trim().to_string(), b.trim().to_string()];
    keys.sort();
    let digest = Sha256::digest(format!("codeany-pairing/v1\n{}\n{}", keys[0], keys[1]).as_bytes());
    let code = code_chars(&digest, 8);
    format!("{}-{}", &code[..4], &code[4..])
}

/// A typed code in canonical form: case, spaces and dashes do not matter.
pub fn normalize_code(code: &str) -> String {
    let raw: String = code
        .to_uppercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| match c {
            '0' => 'O',
            '1' => 'I',
            c => c,
        })
        .collect();
    if raw.len() == 8 { format!("{}-{}", &raw[..4], &raw[4..]) } else { raw }
}

/// The stable id a paired device has in device lists and threads.
pub fn peer_id(public_key: &str) -> String {
    let digest = Sha256::digest(public_key.trim().as_bytes());
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("peer:{hex}")
}

pub fn is_peer_id(id: &str) -> bool {
    id.starts_with("peer:")
}

fn pairing_psk(secret: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    hkdf::Hkdf::<Sha256>::new(None, secret)
        .expand(b"codeany-pairing/v1", &mut out)
        .expect("32 bytes is a valid HKDF length");
    out
}

/// How long a copied authorization code lets devices ask to connect.
pub const INVITE_TTL: Duration = Duration::from_secs(10 * 60);

/// The secret of the authorization code still in force, if any.
fn pair_secret() -> Option<Vec<u8>> {
    let mut state = STATE.lock().unwrap();
    match &state.offer {
        Some((secret, until)) if Instant::now() < *until => Some(secret.clone()),
        Some(_) => {
            state.offer = None;
            None
        }
        None => None,
    }
}

/// How long the last copied authorization code still works.
pub fn invite_expires_in() -> Option<Duration> {
    pair_secret()?;
    let until = STATE.lock().unwrap().offer.as_ref().map(|(_, until)| *until)?;
    Some(until.saturating_duration_since(Instant::now()))
}

// ---------------------------------------------------------------------------
// Authorization codes
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Invite {
    public_key: String,
    name: String,
    endpoints: Vec<Endpoint>,
    nex_device_id: Option<String>,
    pair_secret: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
struct InviteBody {
    v: u32,
    #[serde(default)]
    name: String,
    pk: String,
    #[serde(default)]
    eps: Vec<(String, u16, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ps: Option<String>,
}

/// The text another CodeAny pastes to connect to this one.
/// A fresh authorization code, good for `INVITE_TTL`. It replaces the last
/// one: only the newest code lets a device ask to connect.
pub fn new_invite() -> String {
    offer_invite(false)
}

/// A fresh authorization code to show as a QR code. Only someone in front of
/// this screen can read it, so the first device to ask with it is connected
/// without typing a connection code; it then stops working.
pub fn new_scan_invite() -> String {
    offer_invite(true)
}

fn offer_invite(scanned: bool) -> String {
    let secret: Vec<u8> = uuid::Uuid::new_v4().as_bytes().to_vec();
    {
        let mut state = STATE.lock().unwrap();
        state.offer = Some((secret.clone(), Instant::now() + INVITE_TTL));
        state.offer_scanned = scanned;
    }
    invite_with(Some(secret))
}

/// The names of devices a QR code connected since the last call.
pub fn take_scanned_in() -> Vec<String> {
    std::mem::take(&mut STATE.lock().unwrap().scanned_in)
}

/// This device as other devices should know it (name, key, addresses),
/// without a secret: it cannot be used to ask to connect.
pub fn invite() -> String {
    invite_with(None)
}

fn invite_with(secret: Option<Vec<u8>>) -> String {
    let body = InviteBody {
        v: 1,
        name: crate::remote::this_name(),
        pk: public_key(),
        eps: local_endpoints()
            .into_iter()
            .map(|e| (e.host, e.port, kind_text(e.kind).into()))
            .collect(),
        dev: crate::remote::account().map(|(_, id)| id),
        ps: secret.map(|s| B64.encode(s)),
    };
    format!("{INVITE_PREFIX}{}", B64.encode(serde_json::to_vec(&body).unwrap_or_default()))
}

fn kind_text(kind: NetKind) -> &'static str {
    match kind {
        NetKind::Lan => "lan",
        NetKind::Tailscale => "tailscale",
        NetKind::Custom => "custom",
    }
}

fn parse_invite(raw: &str) -> Result<Invite> {
    let text: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if text.starts_with("termany1.") {
        bail!(t("that is a Termany authorization code; paste the one from CodeAny on the other machine"));
    }
    let body = text
        .strip_prefix(INVITE_PREFIX)
        .ok_or_else(|| anyhow!(t("that is not a CodeAny authorization code")))?;
    let bytes = B64.decode(body).map_err(|_| anyhow!(t("this authorization code is damaged; copy it again")))?;
    let body: InviteBody =
        serde_json::from_slice(&bytes).map_err(|_| anyhow!(t("this authorization code is damaged; copy it again")))?;
    if body.v != 1 {
        bail!(t("this authorization code is from a newer CodeAny; update first"));
    }
    let public_key = key_text(&parse_public_key(&body.pk).map_err(|_| anyhow!(t("this authorization code is damaged")))?);
    let endpoints = body
        .eps
        .into_iter()
        .take(16)
        .filter(|(host, port, _)| {
            *port > 0
                && !host.is_empty()
                && host.len() <= 253
                && host.chars().all(|c| c.is_ascii_alphanumeric() || ".:_-".contains(c))
        })
        .map(|(host, port, kind)| Endpoint {
            host,
            port,
            kind: match kind.as_str() {
                "lan" => NetKind::Lan,
                "tailscale" => NetKind::Tailscale,
                _ => NetKind::Custom,
            },
        })
        .collect();
    let name = body.name.trim().chars().filter(|c| !c.is_control()).take(100).collect::<String>();
    Ok(Invite {
        public_key,
        name: if name.is_empty() { "device".into() } else { name },
        endpoints,
        nex_device_id: body
            .dev
            .filter(|d| !d.is_empty() && d.len() <= 100 && d.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))),
        pair_secret: body.ps.and_then(|s| B64.decode(s).ok()).filter(|s| s.len() >= 16 && s.len() <= 64),
    })
}

// ---------------------------------------------------------------------------
// Reading the state
// ---------------------------------------------------------------------------

pub fn peers() -> Vec<Peer> {
    STATE.lock().unwrap().file.peers.clone()
}

/// The paired (verified) device `device` names: a peer id or nex id.
pub fn find(device: &str) -> Option<Peer> {
    STATE.lock().unwrap().file.peers.iter().find(|p| !p.pending && p.is(device)).cloned()
}

/// Remember which account device a code-connected one is, so the two show
/// as one and its connections stay end-to-end encrypted.
pub fn link_nex(public_key: &str, nex_id: &str) {
    let known = STATE
        .lock()
        .unwrap()
        .file
        .peers
        .iter()
        .any(|p| p.public_key == public_key && p.nex_device_id.as_deref() != Some(nex_id));
    if known {
        with_file(|file| {
            for p in file.peers.iter_mut().filter(|p| p.public_key == public_key) {
                p.nex_device_id = Some(nex_id.to_string());
            }
        });
    }
}

pub fn requests() -> Vec<Request> {
    STATE.lock().unwrap().requests.clone()
}

pub fn strict() -> bool {
    STATE.lock().unwrap().file.strict
}

pub fn set_strict(strict: bool) {
    with_file(|file| file.strict = strict && file.peers.iter().any(|p| !p.pending));
    crate::remote::restart_host();
}

pub fn share() -> Option<bool> {
    STATE.lock().unwrap().file.share
}

pub fn set_share(on: bool) {
    with_file(|file| file.share = Some(on));
}

pub fn saved_name() -> Option<String> {
    STATE.lock().unwrap().file.device_name.clone()
}

pub fn set_saved_name(name: &str) {
    let name = clean_name(name);
    with_file(|file| file.device_name = (!name.is_empty()).then_some(name));
}

pub fn alias(nex_id: &str) -> Option<String> {
    STATE.lock().unwrap().file.aliases.get(nex_id).cloned()
}

/// Name another device here only; an empty name restores its own.
pub fn rename(device: &str, name: &str) {
    let name = clean_name(name);
    with_file(|file| {
        let mut found = false;
        for peer in file.peers.iter_mut().filter(|p| p.is(device)) {
            peer.alias = (!name.is_empty()).then(|| name.clone());
            found = true;
        }
        if !is_peer_id(device) || !found {
            if name.is_empty() {
                file.aliases.remove(device);
            } else {
                file.aliases.insert(device.to_string(), name.clone());
            }
        }
    });
}

fn clean_name(name: &str) -> String {
    name.trim().chars().filter(|c| !c.is_control()).take(100).collect()
}

pub fn direct_status(public_key: &str) -> Option<DirectStatus> {
    STATE.lock().unwrap().direct.get(public_key).cloned()
}

fn set_direct(public_key: &str, status: DirectStatus) {
    STATE.lock().unwrap().direct.insert(public_key.to_string(), status);
}

pub fn port() -> Option<u16> {
    STATE.lock().unwrap().port
}

// ---------------------------------------------------------------------------
// Network addresses
// ---------------------------------------------------------------------------

fn classify(ip: IpAddr) -> Option<NetKind> {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            if a == 100 && (64..128).contains(&b) {
                Some(NetKind::Tailscale)
            } else if a == 10 || (a == 172 && (16..32).contains(&b)) || (a == 192 && b == 168) {
                Some(NetKind::Lan)
            } else {
                None
            }
        }
        // The listener is IPv4; v6 tailnet addresses are covered by the
        // MagicDNS name.
        IpAddr::V6(_) => None,
    }
}

/// Where other devices can reach this one directly.
pub fn local_endpoints() -> Vec<Endpoint> {
    let Some(port) = port() else { return Vec::new() };
    let mut out = Vec::new();
    if let Some(name) = STATE.lock().unwrap().tailscale_name.clone() {
        out.push(Endpoint { host: name, port, kind: NetKind::Tailscale });
    }
    for (_, ip) in local_ip_address::list_afinet_netifas().unwrap_or_default() {
        if let Some(kind) = classify(ip) {
            let host = ip.to_string();
            if !out.iter().any(|e: &Endpoint| e.host == host) {
                out.push(Endpoint { host, port, kind });
            }
        }
    }
    out
}

fn lookup_tailscale_name() -> Option<String> {
    for bin in [
        "tailscale",
        "/usr/local/bin/tailscale",
        "/opt/homebrew/bin/tailscale",
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    ] {
        let Ok(out) = crate::command(bin)
            .args(["status", "--json", "--self", "--peers=false"])
            .stderr(std::process::Stdio::null())
            .output()
        else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let json: Value = serde_json::from_slice(&out.stdout).ok()?;
        let name = json["Self"]["DNSName"].as_str()?.trim_end_matches('.').to_string();
        return (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))).then_some(name);
    }
    None
}

// ---------------------------------------------------------------------------
// Noise sessions
// ---------------------------------------------------------------------------

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> std::io::Result<()> {
    w.write_u16(data.len() as u16).await?;
    w.write_all(data).await?;
    w.flush().await
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let n = r.read_u16().await? as usize;
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

fn prologue(mode: u8) -> Vec<u8> {
    let mut p = b"codeany-peer/v1 ".to_vec();
    p.push(mode);
    p
}

/// Run an established session's transport: plaintext on the returned
/// stream, ciphertext frames on `io`.
fn spawn_transport<S>(io: S, transport: snow::StatelessTransportState) -> DuplexStream
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (app, inner) = tokio::io::duplex(256 * 1024);
    let transport = Arc::new(transport);
    let (mut net_rd, mut net_wr) = tokio::io::split(io);
    let (mut in_rd, mut in_wr) = tokio::io::split(inner);
    let t = transport.clone();
    tokio::spawn(async move {
        let mut nonce = 0u64;
        let mut out = vec![0u8; MAX_FRAME];
        while let Ok(frame) = read_frame(&mut net_rd).await {
            let Ok(n) = t.read_message(nonce, &frame, &mut out) else { break };
            nonce += 1;
            if in_wr.write_all(&out[..n]).await.is_err() {
                break;
            }
        }
        let _ = in_wr.shutdown().await;
    });
    tokio::spawn(async move {
        let mut nonce = 0u64;
        let mut buf = vec![0u8; CHUNK];
        let mut out = vec![0u8; MAX_FRAME];
        loop {
            let n = match in_rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let Ok(m) = transport.write_message(nonce, &buf[..n], &mut out) else { break };
            nonce += 1;
            if write_frame(&mut net_wr, &out[..m]).await.is_err() {
                break;
            }
        }
        let _ = net_wr.shutdown().await;
    });
    app
}

/// The client half of a handshake; the plaintext stream.
async fn initiate<S>(io: S, mode: u8, remote: Option<&[u8]>, psk: Option<[u8; 32]>) -> Result<DuplexStream>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    initiate_as(io, mode, &IDENTITY.private(), remote, psk).await
}

async fn initiate_as<S>(
    mut io: S,
    mode: u8,
    private: &[u8],
    remote: Option<&[u8]>,
    psk: Option<[u8; 32]>,
) -> Result<DuplexStream>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let pattern = match mode {
        MODE_PAIRED => PATTERN_PAIRED,
        MODE_REQUEST => PATTERN_REQUEST,
        _ => PATTERN_INVITE,
    };
    let prologue = prologue(mode);
    let mut builder = snow::Builder::new(pattern.parse()?).prologue(&prologue);
    if mode != MODE_INVITE {
        let remote = remote.ok_or_else(|| anyhow!("no key for the other device"))?;
        builder = builder.local_private_key(private).remote_public_key(remote);
    }
    if let Some(psk) = &psk {
        builder = builder.psk(2, psk);
    }
    let mut hs = builder.build_initiator()?;
    let mut buf = vec![0u8; MAX_FRAME];
    io.write_u8(mode).await?;
    let n = hs.write_message(&[], &mut buf)?;
    write_frame(&mut io, &buf[..n]).await?;
    let reply = read_frame(&mut io)
        .await
        .map_err(|_| anyhow!(t("the device closed the connection during the handshake")))?;
    hs.read_message(&reply, &mut buf)
        .map_err(|_| anyhow!(t("the device is not the one paired (its key does not match)")))?;
    Ok(spawn_transport(io, hs.into_stateless_transport_mode()?))
}

/// One JSON line out, one JSON line back.
async fn exchange(stream: &mut DuplexStream, request: &Value) -> Result<Value> {
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    stream.write_all(&line).await?;
    let reply = read_line(stream).await?.ok_or_else(|| anyhow!(t("the device closed the connection")))?;
    let reply: Value = serde_json::from_str(&reply)?;
    if let Some(error) = reply["error"].as_str() {
        bail!("{error}");
    }
    Ok(reply)
}

/// One line, byte by byte, so nothing after it is consumed.
async fn read_line(stream: &mut DuplexStream) -> Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        if stream.read(&mut byte).await? == 0 {
            return Ok(None);
        }
        if byte[0] == b'\n' {
            return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
        }
        line.push(byte[0]);
        if line.len() > MAX_LINE {
            bail!("line too long");
        }
    }
}

async fn reply(stream: &mut DuplexStream, value: Value) -> Result<()> {
    let mut line = serde_json::to_vec(&value)?;
    line.push(b'\n');
    stream.write_all(&line).await?;
    stream.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// The listener (host side)
// ---------------------------------------------------------------------------

/// Start listening for paired devices and pairing requests. Call once.
pub fn start() {
    LazyLock::force(&IDENTITY);
    agl_bridge::runtime().spawn(async {
        let wanted = std::env::var("CODEANY_PEER_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or_else(|| {
                let dev = crate::paths::home()
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with("-dev"));
                DEFAULT_PORT + u16::from(dev)
            });
        let listener = match TcpListener::bind(("0.0.0.0", wanted)).await {
            Ok(l) => l,
            Err(err) => {
                eprintln!("codeany: port {wanted} for paired devices is taken ({err}); using another");
                match TcpListener::bind(("0.0.0.0", 0)).await {
                    Ok(l) => l,
                    Err(err) => {
                        eprintln!("codeany: cannot listen for paired devices: {err}");
                        return;
                    }
                }
            }
        };
        let port = listener.local_addr().map(|a| a.port()).ok();
        STATE.lock().unwrap().port = port;
        // A port that changed after the host went online needs a new e2e target.
        crate::remote::restart_host();
        tokio::task::spawn_blocking(|| {
            let name = lookup_tailscale_name();
            STATE.lock().unwrap().tailscale_name = name;
        });
        loop {
            match listener.accept().await {
                Ok((socket, from)) => {
                    tokio::spawn(async move {
                        if let Err(err) = serve(socket, from).await {
                            let text = format!("{err:#}");
                            if !text.contains("early eof") && !text.contains("closed") {
                                eprintln!("codeany: paired connection from {from}: {text}");
                            }
                        }
                    });
                }
                Err(err) => {
                    eprintln!("codeany: paired-device listener: {err}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });
    // Measure the ways to every device now and then; confirm pending ones.
    agl_bridge::runtime().spawn(async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        loop {
            remeasure().await;
            // A device that does not answer may just be starting: look
            // again soon rather than show it offline for long.
            let unanswered = peers().iter().any(|p| {
                p.pending || direct_status(&p.public_key).is_none_or(|s| s.rtt_ms.is_none())
            });
            tokio::time::sleep(Duration::from_secs(if unanswered { 10 } else { 45 })).await;
        }
    });
}

async fn serve(mut socket: TcpStream, from: SocketAddr) -> Result<()> {
    let _ = socket.set_nodelay(true);
    let accepted = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        let mode = socket.read_u8().await?;
        let (pattern, psk, secret) = match mode {
            MODE_PAIRED => (PATTERN_PAIRED, None, None),
            // Asking to connect needs the secret of a code still in force.
            MODE_REQUEST => match pair_secret() {
                Some(secret) => (PATTERN_REQUEST, Some(pairing_psk(&secret)), Some(secret)),
                None => bail!("no authorization code is in force"),
            },
            // The authorization code only goes to the relay's streams: those
            // reach this port from loopback; LAN neighbours do not.
            MODE_INVITE if from.ip().is_loopback() => (PATTERN_INVITE, None, None),
            _ => bail!("unexpected greeting"),
        };
        let private = IDENTITY.private();
        let prologue = prologue(mode);
        let mut builder = snow::Builder::new(pattern.parse()?).prologue(&prologue);
        if mode != MODE_INVITE {
            builder = builder.local_private_key(&private);
        }
        if let Some(psk) = &psk {
            builder = builder.psk(2, psk);
        }
        let mut hs = builder.build_responder()?;
        let mut buf = vec![0u8; MAX_FRAME];
        let first = read_frame(&mut socket).await?;
        hs.read_message(&first, &mut buf).context("handshake")?;
        let remote = hs.get_remote_static().map(key_text);
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(&mut socket, &buf[..n]).await?;
        anyhow::Ok((mode, remote, secret, hs.into_stateless_transport_mode()?))
    })
    .await
    .map_err(|_| anyhow!("handshake timed out"))??;
    let (mode, remote, secret, transport) = accepted;
    let mut stream = spawn_transport(socket, transport);
    let line = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_line(&mut stream))
        .await
        .map_err(|_| anyhow!("no request"))??
        .ok_or_else(|| anyhow!("closed before the request"))?;
    let request: Value = serde_json::from_str(&line)?;
    match mode {
        MODE_INVITE => reply(&mut stream, json!({ "ok": true, "invite": new_invite() })).await,
        MODE_REQUEST => {
            let remote = remote.ok_or_else(|| anyhow!("no key"))?;
            let answer = match accept_request(&remote, &request["pair"]) {
                Ok(()) => {
                    // A QR code's secret: connected at once, and used up.
                    if secret.is_some_and(|s| take_scanned_offer(&s)) {
                        trust_request(&remote);
                    }
                    json!({ "ok": true })
                }
                Err(err) => json!({ "error": format!("{err:#}") }),
            };
            reply(&mut stream, answer).await
        }
        _ => {
            let remote = remote.ok_or_else(|| anyhow!("no key"))?;
            let trusted = STATE
                .lock()
                .unwrap()
                .file
                .peers
                .iter()
                .any(|p| !p.pending && p.public_key == remote);
            if !trusted {
                return reply(&mut stream, json!({ "error": "this device is not connected to that one; connect them again" })).await;
            }
            serve_paired(stream, &remote, request).await
        }
    }
}

/// A request from a paired device.
async fn serve_paired(mut stream: DuplexStream, remote: &str, request: Value) -> Result<()> {
    // It may tell us how to reach it now; keep its addresses current.
    if let Some(hello) = request.get("hello").and_then(|h| h.as_str()) {
        if let Ok(invite) = parse_invite(hello) {
            if invite.public_key == remote {
                refresh_peer(&invite);
            }
        }
    }
    if request["check"].as_bool() == Some(true) || request["ping"].as_bool() == Some(true) {
        let share = crate::remote::hosting();
        return reply(
            &mut stream,
            json!({ "ok": true, "share": share, "name": crate::remote::this_name(), "invite": invite() }),
        )
        .await;
    }
    let Some(service) = request["service"].as_str() else {
        return reply(&mut stream, json!({ "error": "unknown request" })).await;
    };
    if !crate::remote::hosting() {
        return reply(&mut stream, json!({ "error": "that machine's CodeAny is not sharing its agents" })).await;
    }
    let target = match service {
        "agl" => agl_bridge::local_endpoint().await.map(|e| e.url()),
        "codeany" => crate::sync::server_url().await,
        crate::pty_host::SERVICE => crate::pty_host::server_url().await,
        other => Err(anyhow!("unknown service '{other}'")),
    };
    let target = match target {
        Ok(url) => url,
        Err(err) => return reply(&mut stream, json!({ "error": format!("{err:#}") })).await,
    };
    let (local, _) = match tokio_tungstenite::connect_async(target.as_str()).await {
        Ok(ws) => ws,
        Err(err) => return reply(&mut stream, json!({ "error": format!("local service unavailable: {err}") })).await,
    };
    reply(&mut stream, json!({ "ok": true })).await?;
    let ws = tokio_tungstenite::accept_async(stream).await?;
    nex::pipe::ws_ws(ws, local).await;
    Ok(())
}

/// A pairing request, authenticated by the secret in our authorization code.
fn accept_request(remote: &str, pair: &Value) -> Result<()> {
    let invite = pair["invite"].as_str().ok_or_else(|| anyhow!("the request is incomplete"))?;
    let invite = parse_invite(invite)?;
    if invite.public_key != remote {
        bail!("the request does not match its sender");
    }
    let mut state = STATE.lock().unwrap();
    state.requests.retain(|r| r.public_key != remote);
    if state.requests.len() >= MAX_REQUESTS {
        bail!(t("too many pending requests; try again in a few minutes"));
    }
    let name = pair["name"].as_str().map(clean_name).filter(|n| !n.is_empty()).unwrap_or_else(|| invite.name.clone());
    state.requests.push(Request { name, public_key: remote.to_string(), invite });
    Ok(())
}

fn refresh_peer(invite: &Invite) {
    let changed = {
        let state = STATE.lock().unwrap();
        state.file.peers.iter().any(|p| {
            p.public_key == invite.public_key
                && (p.name != invite.name
                    || (!invite.endpoints.is_empty() && p.endpoints != invite.endpoints)
                    || (invite.nex_device_id.is_some() && p.nex_device_id != invite.nex_device_id))
        })
    };
    if changed {
        with_file(|file| {
            for p in file.peers.iter_mut().filter(|p| p.public_key == invite.public_key) {
                p.name = invite.name.clone();
                if !invite.endpoints.is_empty() {
                    p.endpoints = invite.endpoints.clone();
                }
                if invite.nex_device_id.is_some() {
                    p.nex_device_id = invite.nex_device_id.clone();
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Dialing (client side)
// ---------------------------------------------------------------------------

/// Try every address at once and keep the first that gets all the way
/// through `talk` (the handshake and whatever follows), not merely the
/// first to accept a TCP connection: a proxy in TUN mode (Clash, Surge, …)
/// accepts connections to any private address at once, then never answers,
/// and would otherwise win the race against the address that works.
async fn dial_with<T, F, Fut>(endpoints: &[Endpoint], talk: F) -> Result<(T, Endpoint)>
where
    F: Fn(TcpStream) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    if endpoints.is_empty() {
        bail!(t("no address to reach the device at"));
    }
    let mut attempts: futures::stream::FuturesUnordered<_> = endpoints
        .iter()
        .cloned()
        .map(|e| {
            let talk = &talk;
            async move {
                let socket = tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect((e.host.as_str(), e.port)))
                    .await
                    .map_err(|_| anyhow!("timed out"))?
                    .map_err(|err| match err.kind() {
                        std::io::ErrorKind::ConnectionRefused => anyhow!("refused"),
                        std::io::ErrorKind::PermissionDenied => anyhow!("blocked"),
                        _ => anyhow!("{err}"),
                    })?;
                let _ = socket.set_nodelay(true);
                let found = talk(socket).await?;
                anyhow::Ok((found, e))
            }
        })
        .collect();
    let mut errors = Vec::new();
    while let Some(attempt) = attempts.next().await {
        match attempt {
            Ok(found) => return Ok(found),
            Err(err) => errors.push(err),
        }
    }
    // The device's own answer says more than a silent address does.
    let timeout = |e: &anyhow::Error| {
        let text = format!("{e:#}");
        text.contains("timed out") || text.contains(t("the device did not answer"))
    };
    let pos = errors.iter().position(|e| !timeout(e)).unwrap_or(0);
    Err(errors.swap_remove(pos))
}

/// A relay stream as plain bytes (the `codeany.e2e` service is tcp).
fn relay_bytes(ws: nex::WsStream) -> DuplexStream {
    let (app, inner) = tokio::io::duplex(256 * 1024);
    let (mut sink, mut stream) = ws.split();
    let (mut rd, mut wr) = tokio::io::split(inner);
    tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            let written = match msg {
                Message::Binary(bytes) => wr.write_all(&bytes).await,
                Message::Text(text) => wr.write_all(text.as_bytes()).await,
                Message::Close(_) => break,
                _ => Ok(()),
            };
            if written.is_err() {
                break;
            }
        }
        let _ = wr.shutdown().await;
    });
    tokio::spawn(async move {
        let mut buf = vec![0u8; CHUNK];
        loop {
            match rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if sink.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() {
                        return;
                    }
                }
            }
        }
        let _ = sink.send(Message::Close(None)).await;
    });
    app
}

/// Open the e2e service of `nex_id` through the relay.
async fn relay_stream(nex_id: &str) -> Result<DuplexStream> {
    let client = crate::remote::client().map_err(|e| anyhow!(e))?;
    let ws = client
        .connect(nex_id, E2E_SERVICE)
        .await
        .map_err(|e| anyhow!(crate::remote::describe_error(&e)))?;
    Ok(relay_bytes(ws))
}

/// How a connection to a paired device went.
#[derive(Clone, Debug)]
pub enum Via {
    Direct(Endpoint),
    Relay,
}

/// A paired session with `peer`, directly when it answers, else through
/// the relay (still end-to-end encrypted).
async fn open_paired(peer: &Peer, allow_relay: bool) -> Result<(DuplexStream, Via)> {
    let remote = parse_public_key(&peer.public_key)?;
    let recently_failed = direct_status(&peer.public_key)
        .is_some_and(|s| s.rtt_ms.is_none() && s.at.elapsed() < DIRECT_RETRY);
    let mut direct_error = None;
    if !(recently_failed && allow_relay && peer.nex_device_id.is_some()) {
        let handshake = |socket: TcpStream| {
            let remote = remote.clone();
            async move {
                tokio::time::timeout(HANDSHAKE_TIMEOUT, initiate(socket, MODE_PAIRED, Some(&remote), None))
                    .await
                    .map_err(|_| anyhow!("handshake timed out"))?
            }
        };
        match dial_with(&peer.endpoints, handshake).await {
            Ok((stream, endpoint)) => return Ok((stream, Via::Direct(endpoint))),
            Err(err) => direct_error = Some(err),
        }
    }
    match (&peer.nex_device_id, allow_relay && crate::remote::account().is_some()) {
        (Some(nex_id), true) => {
            let io = relay_stream(nex_id).await?;
            let stream = tokio::time::timeout(HANDSHAKE_TIMEOUT, initiate(io, MODE_PAIRED, Some(&remote), None))
                .await
                .map_err(|_| anyhow!("handshake timed out"))??;
            Ok((stream, Via::Relay))
        }
        _ => Err(anyhow!(tf(
            "cannot reach {name} directly ({reason}); both machines need the same Wi-Fi or Tailscale network, or the same account",
            &[
                ("name", peer.display_name()),
                ("reason", direct_error.map(|e| e.to_string()).unwrap_or_else(|| t("no address").into())),
            ],
        ))),
    }
}

/// A WebSocket to `service` on a paired device.
pub async fn connect(peer: &Peer, service: &str) -> Result<(crate::remote::Ws, Via)> {
    let (mut stream, via) = open_paired(peer, true).await?;
    let started = Instant::now();
    exchange(&mut stream, &json!({ "service": service, "hello": invite() })).await?;
    // It answered and serves us: that is fresher than the last measurement,
    // which may have failed while it was starting up.
    if let Via::Direct(endpoint) = &via {
        set_direct(
            &peer.public_key,
            DirectStatus {
                rtt_ms: Some((started.elapsed().as_millis() as u32).max(1)),
                endpoint: Some(endpoint.clone()),
                note: None,
                share: true,
                at: Instant::now(),
            },
        );
    }
    let (ws, _) = tokio_tungstenite::client_async(format!("ws://{}/{service}", peer.id().replace(':', "-")), stream)
        .await
        .context("open the stream")?;
    Ok((Box::new(ws), via))
}

/// Ask a paired device to answer: round trip and whether it shares agents.
async fn ping(peer: &Peer, allow_relay: bool) -> Result<(u32, bool, Via, Value)> {
    let (mut stream, via) = open_paired(peer, allow_relay).await?;
    let started = Instant::now();
    let answer = exchange(&mut stream, &json!({ "ping": true, "hello": invite() })).await?;
    let rtt = started.elapsed().as_millis() as u32;
    Ok((rtt.max(1), answer["share"].as_bool().unwrap_or(false), via, answer))
}

/// Measure the direct way to every paired device, and see whether the ones
/// we wait on have verified us.
pub async fn remeasure() {
    let all = peers();
    let checks = all.into_iter().map(|peer| async move {
        if peer.pending {
            let _ = check(&peer.public_key).await;
            return;
        }
        let direct_only = Peer { nex_device_id: None, ..peer.clone() };
        let status = match ping(&direct_only, false).await {
            Ok((rtt, share, via, answer)) => {
                learn(&peer, &answer);
                DirectStatus {
                    rtt_ms: Some(rtt),
                    endpoint: match via {
                        Via::Direct(e) => Some(e),
                        Via::Relay => None,
                    },
                    note: None,
                    share,
                    at: Instant::now(),
                }
            }
            Err(err) => {
                // Not reachable directly: through the relay it can still
                // tell us its new addresses and whether it shares.
                let mut share = direct_status(&peer.public_key).is_some_and(|s| s.share);
                if peer.nex_device_id.is_some() && crate::remote::account().is_some() {
                    if let Ok((_, s, _, answer)) = ping(&peer, true).await {
                        share = s;
                        learn(&peer, &answer);
                    }
                }
                DirectStatus { rtt_ms: None, endpoint: None, note: Some(format!("{err:#}")), share, at: Instant::now() }
            }
        };
        set_direct(&peer.public_key, status);
    });
    futures::future::join_all(checks).await;
}

fn learn(peer: &Peer, answer: &Value) {
    if let Some(invite) = answer["invite"].as_str().and_then(|i| parse_invite(i).ok()) {
        if invite.public_key == peer.public_key {
            refresh_peer(&invite);
        }
    }
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

/// Step 1's answer on the connecting device: show `code` until the other
/// side has typed it.
#[derive(Clone, Debug)]
pub struct Offer {
    pub name: String,
    pub public_key: String,
    pub code: String,
}

/// Start connecting to the device whose authorization code this is.
pub async fn pair_with_code(code: String) -> Result<Offer> {
    let invite = parse_invite(&code)?;
    pair(invite).await
}

/// Start connecting to one of the account's devices: its authorization code
/// comes through the relay. A code swapped on the way would show a
/// different connection code there, so the relay cannot slip in.
pub async fn pair_with_device(nex_id: String) -> Result<Offer> {
    let io = relay_stream(&nex_id).await?;
    let mut stream = tokio::time::timeout(HANDSHAKE_TIMEOUT, initiate(io, MODE_INVITE, None, None))
        .await
        .map_err(|_| anyhow!(t("the device did not answer")))??;
    let answer = exchange(&mut stream, &json!({ "invite": true })).await?;
    let invite = answer["invite"].as_str().ok_or_else(|| anyhow!(t("the device sent no authorization code")))?;
    let mut invite = parse_invite(invite)?;
    invite.nex_device_id.get_or_insert(nex_id);
    pair(invite).await
}

async fn pair(invite: Invite) -> Result<Offer> {
    if invite.public_key == public_key() {
        bail!(t("that is this device's own authorization code; paste the other device's"));
    }
    let secret = invite.pair_secret.clone().ok_or_else(|| anyhow!(t("this authorization code cannot be used to connect")))?;
    let psk = pairing_psk(&secret);
    let remote = parse_public_key(&invite.public_key)?;
    let request = json!({ "pair": { "name": crate::remote::this_name(), "invite": self::invite() } });
    let mut errors = Vec::new();
    let mut sent = false;
    let ask = |socket: TcpStream| send_request(socket, &remote, psk, &request);
    match dial_with(&invite.endpoints, ask).await {
        Ok(_) => sent = true,
        // Reached it, and it turned the request down: the code is no
        // longer in force (expired, or a newer one was copied).
        Err(err) if format!("{err:#}").contains(t("the device refused the authorization code; copy it again")) => {
            errors.push(tf(
                "{name} did not accept this authorization code: it lasts 10 minutes and only the newest one works. Copy a new one there. ({err})",
                &[("name", invite.name.clone()), ("err", format!("{err:#}"))],
            ))
        }
        // Nothing answered directly: no address works from here.
        Err(err) => errors.push(tf(
            "cannot reach {name} directly ({reason}); both machines need the same Wi-Fi or Tailscale network, or the same account",
            &[("name", invite.name.clone()), ("reason", format!("{err:#}"))],
        )),
    }
    if !sent {
        if let (Some(nex_id), Some(_)) = (&invite.nex_device_id, crate::remote::account()) {
            match relay_stream(nex_id).await {
                Ok(io) => match send_request(io, &remote, psk, &request).await {
                    Ok(()) => sent = true,
                    Err(err) => errors.push(format!("{err:#}")),
                },
                Err(err) => errors.push(format!("{err:#}")),
            }
        }
    }
    if !sent {
        bail!(
            "{}",
            errors.pop().unwrap_or_else(|| tf(
                "cannot reach {name}; both machines need the same Wi-Fi or Tailscale network, or the same account",
                &[("name", invite.name.clone())],
            ))
        );
    }
    let public_key = invite.public_key.clone();
    let name = invite.name.clone();
    with_file(|file| {
        let existing = file.peers.iter().find(|p| p.public_key == public_key).cloned();
        file.peers.retain(|p| p.public_key != public_key);
        file.peers.push(Peer {
            public_key: public_key.clone(),
            name: invite.name.clone(),
            endpoints: invite.endpoints.clone(),
            nex_device_id: invite.nex_device_id.clone(),
            added_at: existing.as_ref().map(|p| p.added_at).unwrap_or_else(crate::threads::now_secs),
            pending: true,
            alias: existing.and_then(|p| p.alias),
        });
    });
    Ok(Offer { code: pairing_code(&self::public_key(), &public_key), name, public_key })
}

async fn send_request<S>(io: S, remote: &[u8], psk: [u8; 32], request: &Value) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut stream = tokio::time::timeout(HANDSHAKE_TIMEOUT, initiate(io, MODE_REQUEST, Some(remote), Some(psk)))
        .await
        .map_err(|_| anyhow!(t("the device did not answer")))?
        .map_err(|_| anyhow!(t("the device refused the authorization code; copy it again")))?;
    exchange(&mut stream, request).await.map(|_| ())
}

/// Whether the device we asked has verified us; marks it connected if so.
pub async fn check(public_key: &str) -> Result<bool> {
    let Some(peer) = peers().into_iter().find(|p| p.public_key == public_key) else {
        return Ok(false);
    };
    if !peer.pending {
        return Ok(true);
    }
    let (mut stream, _) = match open_paired(&Peer { pending: false, ..peer.clone() }, true).await {
        Ok(found) => found,
        Err(_) => return Ok(false),
    };
    match exchange(&mut stream, &json!({ "check": true, "hello": invite() })).await {
        Ok(answer) => {
            with_file(|file| {
                for p in file.peers.iter_mut().filter(|p| p.public_key == public_key) {
                    p.pending = false;
                }
            });
            learn(&peer, &answer);
            crate::remote::restart_host();
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

/// Step 2, on the device being connected to: the code the other one shows.
pub fn verify(code: &str) -> Result<String> {
    let code = normalize_code(code);
    let mine = public_key();
    let request = {
        let state = STATE.lock().unwrap();
        if state.requests.is_empty() {
            bail!(t("no device is asking to connect; click Connect a device on the other one first"));
        }
        state.requests.iter().find(|r| pairing_code(&mine, &r.public_key) == code).cloned()
    };
    let request = request.ok_or_else(|| anyhow!(t("that code does not match; check it on the device that is connecting")))?;
    Ok(trust(request))
}

/// Whether `secret` is that of the QR code in force; it then stops working,
/// so one QR code connects one device.
fn take_scanned_offer(secret: &[u8]) -> bool {
    let mut state = STATE.lock().unwrap();
    let scanned = state.offer_scanned && state.offer.as_ref().is_some_and(|(s, _)| s == secret);
    if scanned {
        state.offer = None;
        state.offer_scanned = false;
    }
    scanned
}

/// Connect the device whose request just came in with a QR code's secret.
fn trust_request(public_key: &str) {
    let request = STATE.lock().unwrap().requests.iter().find(|r| r.public_key == public_key).cloned();
    if let Some(request) = request {
        let name = trust(request);
        STATE.lock().unwrap().scanned_in.push(name);
    }
}

/// Trust a device that asked to connect; its name.
fn trust(request: Request) -> String {
    let invite = request.invite.clone();
    with_file(|file| {
        let existing = file.peers.iter().find(|p| p.public_key == invite.public_key).cloned();
        file.peers.retain(|p| p.public_key != invite.public_key);
        file.peers.push(Peer {
            public_key: invite.public_key.clone(),
            name: request.name.clone(),
            endpoints: invite.endpoints.clone(),
            nex_device_id: invite.nex_device_id.clone(),
            added_at: crate::threads::now_secs(),
            pending: false,
            alias: existing.and_then(|p| p.alias),
        });
    });
    STATE.lock().unwrap().requests.retain(|r| r.public_key != request.public_key);
    crate::remote::restart_host();
    request.name
}

pub fn decline(public_key: &str) {
    STATE.lock().unwrap().requests.retain(|r| r.public_key != public_key);
}

/// Forget a paired device (or give up waiting on one).
pub fn remove(public_key: &str) {
    with_file(|file| {
        file.peers.retain(|p| p.public_key != public_key);
        if !file.peers.iter().any(|p| !p.pending) {
            file.strict = false;
        }
    });
    STATE.lock().unwrap().direct.remove(public_key);
    crate::remote::restart_host();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_symmetric_and_typable() {
        let a = format!("{PUBLIC_KEY_PREFIX}{}", B64.encode([1u8; 32]));
        let b = format!("{PUBLIC_KEY_PREFIX}{}", B64.encode([2u8; 32]));
        let code = pairing_code(&a, &b);
        assert_eq!(code, pairing_code(&b, &a));
        assert_eq!(code.len(), 9);
        assert_eq!(normalize_code(&code.to_lowercase().replace('-', " ")), code);
        assert_ne!(code, pairing_code(&a, &a));
        assert_eq!(fingerprint(&a).len(), 24);
        assert!(peer_id(&a).starts_with("peer:"));
    }

    #[test]
    fn a_scanned_code_connects_one_device_only() {
        let secret = |invite: &str| parse_invite(invite).unwrap().pair_secret.unwrap();
        // A copied code still needs the connection code.
        let copied = secret(&new_invite());
        assert!(!take_scanned_offer(&copied));
        // A QR code's secret works once, and only while it is the newest.
        let scanned = secret(&new_scan_invite());
        assert!(!take_scanned_offer(&copied));
        assert!(take_scanned_offer(&scanned));
        assert!(!take_scanned_offer(&scanned));
        assert!(pair_secret().is_none());
    }

    #[test]
    fn invites_round_trip() {
        let body = InviteBody {
            v: 1,
            name: "Studio".into(),
            pk: format!("{PUBLIC_KEY_PREFIX}{}", B64.encode([7u8; 32])),
            eps: vec![("192.168.1.4".into(), 47430, "lan".into()), ("bad host!".into(), 1, "lan".into())],
            dev: Some("dev_123".into()),
            ps: Some(B64.encode([9u8; 16])),
        };
        let text = format!("{INVITE_PREFIX}{}", B64.encode(serde_json::to_vec(&body).unwrap()));
        let invite = parse_invite(&format!("  {text}\n")).unwrap();
        assert_eq!(invite.name, "Studio");
        assert_eq!(invite.endpoints.len(), 1);
        assert_eq!(invite.endpoints[0].kind, NetKind::Lan);
        assert_eq!(invite.nex_device_id.as_deref(), Some("dev_123"));
        assert_eq!(invite.pair_secret.as_deref(), Some(&[9u8; 16][..]));
        assert!(parse_invite("termany1.abc").is_err());
        assert!(parse_invite("codeany1.!!!").is_err());
    }

    #[test]
    fn noise_sessions_carry_bytes_both_ways() {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let initiator = snow::Builder::new(PATTERN_PAIRED.parse().unwrap()).generate_keypair().unwrap();
            let responder = snow::Builder::new(PATTERN_PAIRED.parse().unwrap()).generate_keypair().unwrap();
            let (a, b) = tokio::io::duplex(1 << 20);
            let server = tokio::spawn({
                let private = responder.private.clone();
                async move {
                    let mut b = b;
                    let prologue = prologue(MODE_PAIRED);
                    let mut hs = snow::Builder::new(PATTERN_PAIRED.parse().unwrap())
                        .prologue(&prologue)
                        .local_private_key(&private)
                        .build_responder()
                        .unwrap();
                    assert_eq!(b.read_u8().await.unwrap(), MODE_PAIRED);
                    let mut buf = vec![0u8; MAX_FRAME];
                    let first = read_frame(&mut b).await.unwrap();
                    hs.read_message(&first, &mut buf).unwrap();
                    let remote = hs.get_remote_static().unwrap().to_vec();
                    let n = hs.write_message(&[], &mut buf).unwrap();
                    write_frame(&mut b, &buf[..n]).await.unwrap();
                    let mut stream = spawn_transport(b, hs.into_stateless_transport_mode().unwrap());
                    let line = read_line(&mut stream).await.unwrap().unwrap();
                    reply(&mut stream, json!({ "echo": line })).await.unwrap();
                    let mut big = vec![0u8; 200_000];
                    stream.read_exact(&mut big).await.unwrap();
                    assert!(big.iter().all(|b| *b == 5));
                    remote
                }
            });
            // `initiate` uses this process's identity; drive the client by hand
            // with the generated pair instead.
            let mut a = a;
            let prologue = prologue(MODE_PAIRED);
            let mut hs = snow::Builder::new(PATTERN_PAIRED.parse().unwrap())
                .prologue(&prologue)
                .local_private_key(&initiator.private)
                .remote_public_key(&responder.public)
                .build_initiator()
                .unwrap();
            let mut buf = vec![0u8; MAX_FRAME];
            a.write_u8(MODE_PAIRED).await.unwrap();
            let n = hs.write_message(&[], &mut buf).unwrap();
            write_frame(&mut a, &buf[..n]).await.unwrap();
            let msg = read_frame(&mut a).await.unwrap();
            hs.read_message(&msg, &mut buf).unwrap();
            let mut stream = spawn_transport(a, hs.into_stateless_transport_mode().unwrap());
            let answer = exchange(&mut stream, &json!({ "ping": true })).await.unwrap();
            assert_eq!(answer["echo"], "{\"ping\":true}");
            stream.write_all(&vec![5u8; 200_000]).await.unwrap();
            let remote = server.await.unwrap();
            assert_eq!(remote, initiator.public);
        });
    }

    /// The real listener, end to end, in a throwaway home:
    /// `cargo test peer::tests::listener -- --ignored --test-threads=1`
    #[test]
    #[ignore]
    fn listener_pairs_and_serves_threads() {
        let home = std::env::temp_dir().join(format!("codeany-peer-test-{}", std::process::id()));
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        // SAFETY: an ignored test run on its own, before any thread reads them.
        unsafe {
            std::env::set_var("CODEANY_HOME", &home);
            std::env::set_var("CODEANY_PEER_PORT", port.to_string());
        }
        let mut inbox = crate::sync::start(&[]);
        start();
        let rt = agl_bridge::runtime();
        rt.block_on(async {
            for _ in 0..50 {
                if self::port().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert_eq!(self::port(), Some(port));
            // No code copied yet: nobody may ask to connect.
            assert!(parse_invite(&invite()).unwrap().pair_secret.is_none());
            let host = parse_invite(&new_invite()).unwrap();
            let host_key = parse_public_key(&host.public_key).unwrap();

            // A device of our own making asks to connect.
            let me = snow::Builder::new(PATTERN_PAIRED.parse().unwrap()).generate_keypair().unwrap();
            let my_key = key_text(&me.public);
            let my_invite = format!(
                "{INVITE_PREFIX}{}",
                B64.encode(serde_json::to_vec(&InviteBody {
                    v: 1,
                    name: "Laptop".into(),
                    pk: my_key.clone(),
                    eps: vec![],
                    dev: None,
                    ps: Some(B64.encode([3u8; 16])),
                }).unwrap())
            );
            let dial = || TcpStream::connect(("127.0.0.1", port));
            // A wrong secret gets nowhere.
            let wrong = initiate_as(dial().await.unwrap(), MODE_REQUEST, &me.private, Some(&host_key), Some(pairing_psk(&[1u8; 16]))).await;
            if let Ok(mut s) = wrong {
                assert!(exchange(&mut s, &json!({ "pair": { "name": "x", "invite": my_invite } })).await.is_err());
            }
            let psk = pairing_psk(host.pair_secret.as_ref().unwrap());
            let mut s = initiate_as(dial().await.unwrap(), MODE_REQUEST, &me.private, Some(&host_key), Some(psk)).await.unwrap();
            exchange(&mut s, &json!({ "pair": { "name": "Laptop", "invite": my_invite } })).await.unwrap();
            assert_eq!(requests().len(), 1);

            // Not trusted until the code is typed here.
            let mut s = initiate_as(dial().await.unwrap(), MODE_PAIRED, &me.private, Some(&host_key), None).await.unwrap();
            assert!(exchange(&mut s, &json!({ "check": true })).await.is_err());
            assert!(verify("AAAA-AAAA").is_err());
            assert_eq!(verify(&pairing_code(&host.public_key, &my_key).to_lowercase()).unwrap(), "Laptop");
            assert!(requests().is_empty());
            assert!(find(&peer_id(&my_key)).is_some());

            let mut s = initiate_as(dial().await.unwrap(), MODE_PAIRED, &me.private, Some(&host_key), None).await.unwrap();
            let answer = exchange(&mut s, &json!({ "ping": true })).await.unwrap();
            assert_eq!(answer["ok"], true);

            // Sharing off: no services.
            set_share(false);
            let mut s = initiate_as(dial().await.unwrap(), MODE_PAIRED, &me.private, Some(&host_key), None).await.unwrap();
            assert!(exchange(&mut s, &json!({ "service": "codeany" })).await.is_err());

            // Sharing on: the thread service, over the encrypted link.
            set_share(true);
            let mut s = initiate_as(dial().await.unwrap(), MODE_PAIRED, &me.private, Some(&host_key), None).await.unwrap();
            exchange(&mut s, &json!({ "service": "codeany" })).await.unwrap();
            let (mut ws, _) = tokio_tungstenite::client_async("ws://peer/codeany", s).await.unwrap();
            let mut thread = crate::threads::Thread::new(Some("/tmp/x".into()), "claude".into(), "full".into());
            thread.machine = Some("someone".into());
            thread.items.push(crate::threads::ChatItem::Assistant("hello".into()));
            for (id, method, params) in [
                ("1", "threads.list", json!({})),
                ("2", "threads.put", json!({ "thread": thread })),
                ("3", "threads.list", json!({})),
            ] {
                let msg = agl::protocol::WireMsg::Request { id: id.into(), method: method.into(), params }.to_json();
                ws.send(Message::Text(msg.into())).await.unwrap();
                let Some(Ok(Message::Text(reply))) = ws.next().await else { panic!("no reply") };
                let reply: Value = serde_json::from_str(&reply).unwrap();
                assert_eq!(reply["id"], id);
                assert!(reply.get("error").is_none(), "{reply}");
                if id == "1" {
                    assert_eq!(reply["result"], json!([]));
                }
                if id == "3" {
                    assert_eq!(reply["result"][0]["id"], thread.id);
                }
            }
            match inbox.next().await {
                Some(crate::sync::Incoming::Put(put)) => {
                    assert_eq!(put.id, thread.id);
                    assert!(put.machine.is_none());
                }
                _ => panic!("expected the thread to arrive"),
            }

            let project = home.join("repo");
            std::fs::create_dir_all(&project).unwrap();
            for args in [
                &["init", "-b", "main"][..],
                &["-c", "user.email=test@example.com", "-c", "user.name=Test", "commit", "--allow-empty", "-m", "initial"],
                &["branch", "feature"],
            ] {
                let output = crate::command("git").args(args).current_dir(&project).output().unwrap();
                assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            }
            let git_call = |id: &str, method: &str, params: Value| {
                agl::protocol::WireMsg::Request { id: id.into(), method: method.into(), params }.to_json()
            };
            let msg = git_call("4", "git.project", json!({ "path": project }));
            ws.send(Message::Text(msg.into())).await.unwrap();
            let Some(Ok(Message::Text(reply))) = ws.next().await else { panic!("no Git project reply") };
            let reply: Value = serde_json::from_str(&reply).unwrap();
            let git: crate::git::ProjectGit = serde_json::from_value(reply["result"].clone()).unwrap();
            assert_eq!(git.branches, ["feature", "main"]);
            let id = 42;
            let worktree = crate::threads::Worktree::from_project(&git, id, "Remote checkout");
            for (request, method, params) in [
                ("5", "git.prepare", json!({
                    "id": id, "path": project, "branch": "feature", "worktree": worktree,
                })),
                ("6", "threads.delete", json!({ "id": id, "checkout": (&project, &worktree) })),
            ] {
                let msg = git_call(request, method, params);
                ws.send(Message::Text(msg.into())).await.unwrap();
                let Some(Ok(Message::Text(reply))) = ws.next().await else { panic!("no Git checkout reply") };
                let reply: Value = serde_json::from_str(&reply).unwrap();
                assert!(reply.get("error").is_none(), "{reply}");
                if request == "5" {
                    let cwd = std::path::Path::new(reply["result"]["cwd"].as_str().unwrap());
                    assert_eq!(cwd.canonicalize().unwrap(), worktree.path.canonicalize().unwrap());
                }
            }
            assert!(!worktree.path.exists());

            // A stranger's key is refused.
            let other = snow::Builder::new(PATTERN_PAIRED.parse().unwrap()).generate_keypair().unwrap();
            let mut s = initiate_as(dial().await.unwrap(), MODE_PAIRED, &other.private, Some(&host_key), None).await.unwrap();
            assert!(exchange(&mut s, &json!({ "ping": true })).await.is_err());

            // The authorization code is not handed out on the LAN side… but
            // loopback is the relay's way in, so it is here.
            let mut s = initiate_as(dial().await.unwrap(), MODE_INVITE, &[], None, None).await.unwrap();
            let answer = exchange(&mut s, &json!({ "invite": true })).await.unwrap();
            assert!(answer["invite"].as_str().unwrap().starts_with(INVITE_PREFIX));

            remove(&my_key);
            assert!(find(&peer_id(&my_key)).is_none());
        });
        let _ = std::fs::remove_dir_all(home);
    }

    /// A TUN-mode proxy accepts a connection to any address and then says
    /// nothing; the address that really answers must still win.
    #[test]
    fn a_silent_address_does_not_win_the_race() {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let real = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = |l: &TcpListener| Endpoint {
                host: "127.0.0.1".into(),
                port: l.local_addr().unwrap().port(),
                kind: NetKind::Lan,
            };
            let endpoints = vec![endpoint(&silent), endpoint(&real)];
            tokio::spawn(async move {
                let mut held = Vec::new();
                while let Ok((socket, _)) = silent.accept().await {
                    held.push(socket);
                }
            });
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = real.accept().await {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let _ = socket.write_all(b"!").await;
                }
            });
            let talk = |mut socket: TcpStream| async move {
                let mut byte = [0u8; 1];
                tokio::time::timeout(Duration::from_secs(2), socket.read_exact(&mut byte))
                    .await
                    .map_err(|_| anyhow!("timed out"))??;
                anyhow::Ok(byte[0])
            };
            let (answer, used) = dial_with(&endpoints, talk).await.unwrap();
            assert_eq!(answer, b'!');
            assert_eq!(used, endpoints[1]);
        });
    }
}
