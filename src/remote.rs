//! The user's other machines: through a nex relay account, and directly to
//! paired devices (see [`crate::peer`]).
//!
//! This CodeAny is one nex device (`~/.codeany/nex.json`, mode 0600). While
//! signed in it stays online on the relay, so the account's other devices
//! see it and can pair with it end to end; with sharing on it also exposes
//! its agl daemon (`agl`) and its threads (`codeany`), and then the
//! conversation runs on a laptop while the agents run here.
//!
//! [`connect`] picks the way to a device: a paired device's LAN / tailnet
//! address when it answers, else the relay — end-to-end encrypted for a
//! paired device, plain otherwise.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use futures::{Sink, SinkExt as _, Stream, StreamExt as _};
use nex::{
    ExposedService, Host, NexClient, Target,
    proto::{DeviceCredential, DeviceInfo, PairingCode},
};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_util::sync::CancellationToken;

use crate::{agl_bridge, peer};
use crate::i18n::{t, tf};

/// The nex service CodeAny exposes its agl daemon under.
pub const SERVICE: &str = "agl";
/// Present while a device shares its agents, even when it only lets paired
/// devices in (and so exposes no plain `agl`).
const HOST_MARKER: &str = "codeany.host";
const APP: &str = "codeany";
/// The relay Termany signs in to; the default for a new account here.
pub const DEFAULT_RELAY: &str = "https://api.nex.surf";

/// A WebSocket to a service, however it is carried.
pub trait WsTransport:
    Stream<Item = Result<Message, tungstenite::Error>> + Sink<Message, Error = tungstenite::Error> + Send + Unpin
{
}
impl<T> WsTransport for T where
    T: Stream<Item = Result<Message, tungstenite::Error>> + Sink<Message, Error = tungstenite::Error> + Send + Unpin
{
}
pub type Ws = Box<dyn WsTransport>;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Account {
    server: String,
    credential: DeviceCredential,
    /// Share this machine's agents (superseded by the paired-devices file,
    /// which also covers paired devices while signed out).
    #[serde(default)]
    host: bool,
    /// The account's sign-in (an email), when it came from Termany.
    #[serde(default)]
    account: Option<String>,
}

/// What Settings → Devices shows about this device's account.
#[derive(Clone, Debug)]
pub struct Status {
    pub server: String,
    pub device_id: String,
    pub account: Option<String>,
    pub host: HostStatus,
}

/// This device's connection to the relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostStatus {
    Off,
    /// Online through the relay, or reconnecting to it.
    On,
    Failed(String),
}

struct State {
    account: Option<Account>,
    host: HostStatus,
    host_cancel: Option<CancellationToken>,
}

static STATE: Mutex<State> = Mutex::new(State {
    account: None,
    host: HostStatus::Off,
    host_cancel: None,
});

fn path() -> PathBuf {
    crate::paths::home().join("nex.json")
}

fn save(account: &Account) -> Result<()> {
    peer::write_secret(&path(), account)
}

/// Read the saved account, go online, and listen for paired devices. Call once.
pub fn init() {
    let account = std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str::<Account>(&text).ok());
    if peer::share().is_none() && account.as_ref().is_some_and(|a| a.host) {
        peer::set_share(true);
    }
    let signed_in = account.is_some();
    STATE.lock().unwrap().account = account;
    peer::start();
    if signed_in {
        start_host();
    }
    // How long the relay takes to each device, now and then.
    agl_bridge::runtime().spawn(async {
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            measure_relay().await;
            tokio::time::sleep(Duration::from_secs(45)).await;
        }
    });
}

/// This device's link, when signed in.
pub fn status() -> Option<Status> {
    let state = STATE.lock().unwrap();
    let account = state.account.as_ref()?;
    Some(Status {
        server: account.server.clone(),
        device_id: account.credential.device_id.clone(),
        account: account.account.clone(),
        host: state.host.clone(),
    })
}

/// The relay and this device's id, when signed in.
pub fn account() -> Option<(String, String)> {
    let state = STATE.lock().unwrap();
    state
        .account
        .as_ref()
        .map(|a| (a.server.clone(), a.credential.device_id.clone()))
}

pub fn client() -> Result<NexClient, String> {
    let state = STATE.lock().unwrap();
    let account = state
        .account
        .as_ref()
        .ok_or_else(|| t("this CodeAny is not signed in (Settings → Devices)").to_string())?;
    NexClient::new(&account.server, account.credential.clone()).map_err(|e| format!("{e:#}"))
}

/// A nex error as the person can act on it.
pub fn describe_error(err: &anyhow::Error) -> String {
    if let Some(api) = err.downcast_ref::<nex::ApiError>() {
        return match api.status {
            401 => t("this device was signed out of the account; sign in again in Settings → Devices").into(),
            404 => t("the machine is offline").into(),
            409 => t("two devices share that name").into(),
            _ => format!("{api}"),
        };
    }
    if let Some(tungstenite::Error::Http(resp)) = err.downcast_ref::<tungstenite::Error>() {
        return match resp.status().as_u16() {
            404 => t("the machine is offline").into(),
            502 => t("the machine's CodeAny is not sharing its agents, or only accepts end-to-end encrypted devices").into(),
            code => tf("the relay refused the stream ({code})", &[("code", code.to_string())]),
        };
    }
    format!("{err:#}")
}

/// The computer's own name.
pub fn device_name() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    crate::command("hostname")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "codeany".into())
}

/// This device's name as the others see it.
pub fn this_name() -> String {
    if account().is_some() {
        if let Some(me) = DEVICES.lock().unwrap().iter().find(|d| d.is_self) {
            return me.name.clone();
        }
    }
    peer::saved_name().unwrap_or_else(device_name)
}

/// A device's name, for messages.
pub fn name_of(id: &str) -> String {
    if let Some(device) = DEVICES.lock().unwrap().iter().find(|d| d.id == id) {
        return device.name.clone();
    }
    peer::peers()
        .into_iter()
        .find(|p| p.is(id))
        .map(|p| p.display_name())
        .unwrap_or_else(|| id.to_string())
}

fn link(server: String, credential: DeviceCredential, account: Option<String>, host: bool) -> Result<()> {
    stop_host();
    let account = Account {
        server: server.trim().trim_end_matches('/').to_string(),
        credential,
        host,
        account,
    };
    save(&account)?;
    STATE.lock().unwrap().account = Some(account);
    if host && peer::share().is_none() {
        peer::set_share(true);
    }
    start_host();
    Ok(())
}

/// Join an account with a pairing code from one of its devices.
/// `host`: share this machine's agents right away.
pub async fn join(server: String, code: String, host: bool) -> Result<()> {
    let credential = agl_bridge::spawn({
        let server = server.clone();
        async move { NexClient::claim(&server, code.trim(), &device_name(), Some(APP)).await }
    })
    .await?;
    link(server, credential, None, host)
}

#[derive(Deserialize)]
struct TermanyConfig {
    server: String,
    credential: DeviceCredential,
    #[serde(default)]
    account: Option<String>,
}

fn termany_config() -> Option<TermanyConfig> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let text = std::fs::read_to_string(home.join(".termany").join("nex.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// The account Termany on this machine is signed in to: relay and email.
pub fn termany_account() -> Option<(String, Option<String>)> {
    termany_config().map(|c| (c.server, c.account))
}

/// Join the account Termany on this machine is signed in to: Termany's own
/// device asks the relay for a pairing code, which this CodeAny claims as a
/// device of its own. Termany's credential is only read, never kept.
pub async fn join_termany(host: bool) -> Result<()> {
    let config = termany_config().ok_or_else(|| anyhow!("Termany is not signed in on this machine"))?;
    let server = config.server.clone();
    let credential = agl_bridge::spawn({
        let server = server.clone();
        async move {
            let termany = NexClient::new(&server, config.credential)?;
            let code = termany.create_pairing_code().await?;
            NexClient::claim(&server, &code.code, &device_name(), Some(APP)).await
        }
    })
    .await?;
    link(server, credential, config.account, host)
}

/// Rename a device: this one everywhere (on the relay too), any other one on
/// this machine only. An empty name restores another device's own.
pub async fn rename(device_id: String, name: String) -> Result<()> {
    let name = name.trim().to_string();
    let is_self = account().is_some_and(|(_, id)| id == device_id) || device_id == SELF_ID;
    if !is_self {
        peer::rename(&device_id, &name);
        refresh_cache_names();
        return Ok(());
    }
    if name.is_empty() {
        return Ok(());
    }
    peer::set_saved_name(&name);
    if let Ok(client) = client() {
        let id = client.device_id().to_string();
        let relay_name = name.clone();
        agl_bridge::spawn(async move { client.rename(&id, &relay_name).await }).await?;
    }
    if let Some(me) = DEVICES.lock().unwrap().iter_mut().find(|d| d.is_self) {
        me.name = name;
    }
    Ok(())
}

/// Create a new account on a relay (needs its admin key), with this
/// machine as its first device.
pub async fn create_account(server: String, admin_key: String) -> Result<()> {
    let credential = agl_bridge::spawn({
        let server = server.clone();
        async move {
            let name = device_name();
            let key = Some(admin_key.trim()).filter(|k| !k.is_empty());
            NexClient::bootstrap(&server, &name, &name, Some(APP), key).await
        }
    })
    .await?;
    link(server, credential, None, true)
}

/// Forget the account here, revoking this device on the relay first.
/// Paired devices stay paired and keep working directly.
pub async fn sign_out() -> Result<()> {
    stop_host();
    if let Ok(client) = client() {
        let id = client.device_id().to_string();
        let _ = agl_bridge::spawn(async move { client.revoke(&id).await }).await;
    }
    STATE.lock().unwrap().account = None;
    let _ = std::fs::remove_file(path());
    DEVICES.lock().unwrap().retain(|d| !d.via_account || d.peer.is_some());
    Ok(())
}

pub async fn pairing_code() -> Result<PairingCode> {
    let client = client().map_err(|e| anyhow!(e))?;
    agl_bridge::spawn(async move { client.create_pairing_code().await }).await
}

/// Remove a device from the account.
pub async fn revoke(device_id: String) -> Result<()> {
    let client = client().map_err(|e| anyhow!(e))?;
    agl_bridge::spawn(async move { client.revoke(&device_id).await }).await
}

// ---- Sharing ------------------------------------------------------------

/// Whether this machine lets its other devices run agents and read threads.
pub fn hosting() -> bool {
    peer::share().unwrap_or(false)
}

pub fn host_status() -> HostStatus {
    STATE.lock().unwrap().host.clone()
}

/// Turn sharing this machine's agents on or off, and remember it.
pub fn set_hosting(on: bool) -> Result<()> {
    peer::set_share(on);
    {
        let mut state = STATE.lock().unwrap();
        if let Some(account) = state.account.as_mut() {
            account.host = on;
            save(account)?;
        }
    }
    restart_host();
    Ok(())
}

/// Bring what the relay sees in line with the settings: sharing, strict,
/// the paired-device port.
pub fn restart_host() {
    if account().is_some() {
        start_host();
    }
}

fn stop_host() {
    let mut state = STATE.lock().unwrap();
    if let Some(cancel) = state.host_cancel.take() {
        cancel.cancel();
    }
    state.host = HostStatus::Off;
}

/// The relay service that tells the account's devices which paired key
/// this one holds, so a device paired by code and seen through the account
/// shows as one.
fn identity_service() -> String {
    format!("codeany.id.{}", peer::peer_id(&peer::public_key()).trim_start_matches("peer:"))
}

async fn services() -> Result<Vec<ExposedService>> {
    let mut out = vec![
        ExposedService::echo(),
        ExposedService { name: identity_service(), target: Target::Echo },
    ];
    if let Some(port) = peer::port() {
        out.push(ExposedService::tcp(peer::E2E_SERVICE, &format!("127.0.0.1:{port}"))?);
    }
    if hosting() {
        out.push(ExposedService { name: HOST_MARKER.into(), target: Target::Echo });
        if !peer::strict() {
            let endpoint = agl_bridge::local_endpoint().await?;
            out.push(ExposedService::ws(SERVICE, &endpoint.url())?);
            out.push(ExposedService::ws(crate::sync::SERVICE, &crate::sync::server_url().await?)?);
        }
    }
    Ok(out)
}

fn start_host() {
    stop_host();
    let client = match client() {
        Ok(client) => client,
        Err(err) => {
            STATE.lock().unwrap().host = HostStatus::Failed(err);
            return;
        }
    };
    let cancel = CancellationToken::new();
    {
        let mut state = STATE.lock().unwrap();
        state.host = HostStatus::On;
        state.host_cancel = Some(cancel.clone());
    }
    agl_bridge::runtime().spawn(async move {
        let result = async {
            let services = services().await?;
            Host::new(client, services).run(cancel.clone()).await
        }
        .await;
        if let Err(err) = result {
            let message = format!("{err:#}");
            eprintln!("codeany: the relay connection stopped: {message}");
            let mut state = STATE.lock().unwrap();
            if !cancel.is_cancelled() {
                state.host = HostStatus::Failed(message);
                state.host_cancel = None;
            }
        }
    });
}

// ---- Devices --------------------------------------------------------------

/// The id this device has in [`devices`] while signed out.
pub const SELF_ID: &str = "self";

/// One of the user's devices, however it is reached.
#[derive(Clone, Debug)]
pub struct Device {
    /// The nex device id, or `peer:…` for a device paired only.
    pub id: String,
    pub name: String,
    pub app: Option<String>,
    pub is_self: bool,
    /// In the account (it may also be paired).
    pub via_account: bool,
    /// Online on the relay.
    pub account_online: bool,
    /// Shares its agents.
    pub shares: bool,
    /// Paired, or waiting for it to verify us.
    pub peer: Option<peer::Peer>,
    /// Shares, but only with paired devices, and we are not one.
    pub requires_pairing: bool,
    pub direct: Option<peer::DirectStatus>,
    /// Round trip through the relay, like a ping.
    pub relay_rtt: Option<u32>,
    pub last_seen_at: Option<i64>,
}

impl Device {
    pub fn paired(&self) -> bool {
        self.peer.as_ref().is_some_and(|p| !p.pending)
    }

    pub fn pending(&self) -> bool {
        self.peer.as_ref().is_some_and(|p| p.pending)
    }

    pub fn direct_ok(&self) -> bool {
        self.paired() && self.direct.as_ref().is_some_and(|d| d.rtt_ms.is_some())
    }

    /// Reachable some way right now.
    pub fn online(&self) -> bool {
        self.is_self || self.account_online || self.direct_ok()
    }

    /// Can run conversations for this one now.
    pub fn usable(&self) -> bool {
        !self.is_self
            && self.shares
            && !self.requires_pairing
            && ((self.account_online && (self.paired() || self.via_account)) || self.direct_ok())
    }
}

static DEVICES: Mutex<Vec<Device>> = Mutex::new(Vec::new());
static RELAY_RTT: LazyLock<Mutex<HashMap<String, (u32, Instant)>>> = LazyLock::new(Default::default);

/// The devices as last listed.
pub fn cached_devices() -> Vec<Device> {
    DEVICES.lock().unwrap().clone()
}

fn refresh_cache_names() {
    let peers = peer::peers();
    for device in DEVICES.lock().unwrap().iter_mut().filter(|d| !d.is_self) {
        if let Some(p) = peers.iter().find(|p| p.is(&device.id)) {
            device.name = p.display_name();
        } else if let Some(alias) = peer::alias(&device.id) {
            device.name = alias;
        }
    }
}

fn has_service(info: &DeviceInfo, name: &str) -> bool {
    info.services.iter().any(|s| s.name == name)
}

/// Every device once: the account's, and the paired ones, merged.
pub async fn devices() -> Result<Vec<Device>> {
    let relay = match client() {
        Ok(client) => Some(agl_bridge::spawn(async move { client.devices().await }).await),
        Err(_) => None,
    };
    let peers = peer::peers();
    let rtts = RELAY_RTT.lock().unwrap().clone();
    let mut out = Vec::new();
    let mut matched = std::collections::HashSet::new();
    match relay {
        Some(Ok(list)) => {
            for info in list {
                let peer = peers
                    .iter()
                    .find(|p| {
                        p.nex_device_id.as_deref() == Some(info.id.as_str())
                            || has_service(&info, &format!("codeany.id.{}", p.id().trim_start_matches("peer:")))
                    })
                    .cloned();
                if let Some(p) = &peer {
                    matched.insert(p.public_key.clone());
                    if !info.is_self {
                        peer::link_nex(&p.public_key, &info.id);
                    }
                }
                let paired = peer.as_ref().is_some_and(|p| !p.pending);
                let shares = has_service(&info, SERVICE) || has_service(&info, HOST_MARKER);
                let name = if info.is_self {
                    info.name.clone()
                } else {
                    peer.as_ref()
                        .and_then(|p| p.alias.clone())
                        .or_else(|| peer::alias(&info.id))
                        .unwrap_or_else(|| info.name.clone())
                };
                out.push(Device {
                    requires_pairing: info.online && shares && !has_service(&info, SERVICE) && !paired && !info.is_self,
                    direct: peer.as_ref().and_then(|p| peer::direct_status(&p.public_key)),
                    relay_rtt: rtts.get(&info.id).filter(|(_, at)| at.elapsed() < Duration::from_secs(180)).map(|(ms, _)| *ms),
                    id: info.id.clone(),
                    name,
                    app: info.app.clone(),
                    is_self: info.is_self,
                    via_account: true,
                    account_online: info.online,
                    shares: if info.is_self { hosting() } else { shares },
                    peer,
                    last_seen_at: info.last_seen_at,
                });
            }
        }
        Some(Err(err)) if peers.is_empty() => return Err(err),
        _ => {}
    }
    if !out.iter().any(|d| d.is_self) {
        out.push(Device {
            id: account().map(|(_, id)| id).unwrap_or_else(|| SELF_ID.into()),
            name: this_name(),
            app: Some(APP.into()),
            is_self: true,
            via_account: false,
            account_online: false,
            shares: hosting(),
            peer: None,
            requires_pairing: false,
            direct: None,
            relay_rtt: None,
            last_seen_at: None,
        });
    }
    for p in peers.into_iter().filter(|p| !matched.contains(&p.public_key)) {
        let direct = peer::direct_status(&p.public_key);
        out.push(Device {
            id: p.id(),
            name: p.display_name(),
            app: Some(APP.into()),
            is_self: false,
            via_account: false,
            account_online: false,
            shares: direct.as_ref().is_some_and(|d| d.share),
            requires_pairing: false,
            direct,
            relay_rtt: None,
            last_seen_at: None,
            peer: Some(p),
        });
    }
    *DEVICES.lock().unwrap() = out.clone();
    Ok(out)
}

/// Round trip through the relay to every online device, like a ping.
async fn measure_relay() {
    let Ok(client) = client() else { return };
    let Ok(list) = client.devices().await else { return };
    let probes = list
        .into_iter()
        .filter(|d| !d.is_self && d.online && has_service(d, "echo"))
        .map(|d| {
            let client = client.clone();
            async move {
                let probe = async {
                    let mut ws = client.connect(&d.id, "echo").await?;
                    let started = Instant::now();
                    ws.send(Message::Text("ping".into())).await?;
                    while let Some(msg) = ws.next().await {
                        if matches!(msg?, Message::Text(_)) {
                            let _ = ws.close(None).await;
                            return anyhow::Ok(started.elapsed().as_millis() as u32);
                        }
                    }
                    Err(anyhow!("closed"))
                };
                match tokio::time::timeout(Duration::from_secs(8), probe).await {
                    Ok(Ok(ms)) => Some((d.id, ms.max(1))),
                    _ => None,
                }
            }
        });
    let results = futures::future::join_all(probes).await;
    let mut rtts = RELAY_RTT.lock().unwrap();
    for (id, ms) in results.into_iter().flatten() {
        rtts.insert(id, (ms, Instant::now()));
    }
}

/// Measure every way to every device now.
pub async fn remeasure() -> Result<()> {
    agl_bridge::spawn(async {
        tokio::join!(peer::remeasure(), measure_relay());
        Ok(())
    })
    .await
}

/// A WebSocket to `service` on `device`: directly or end-to-end encrypted
/// through the relay when paired, plainly through the relay otherwise.
pub async fn connect(device: &str, service: &str) -> Result<Ws> {
    if let Some(p) = peer::find(device) {
        let (ws, _) = peer::connect(&p, service).await?;
        return Ok(ws);
    }
    if peer::is_peer_id(device) {
        anyhow::bail!(t("that device is no longer connected to this one"));
    }
    let client = client().map_err(|e| anyhow!(e))?;
    let ws = client.connect(device, service).await.map_err(|e| {
        let requires = DEVICES.lock().unwrap().iter().any(|d| d.id == device && d.requires_pairing);
        if requires {
            anyhow!(t("that machine only accepts end-to-end encrypted devices; turn it on in Settings → Devices"))
        } else {
            anyhow!(describe_error(&e))
        }
    })?;
    Ok(Box::new(ws))
}

/// `codeany serve`: share this machine's agents without opening a window,
/// for a machine that sits in a closet. `codeany link <server> <code>` links
/// it first.
pub fn serve_cli(args: &[String]) -> i32 {
    let rt = agl_bridge::runtime();
    match args.first().map(String::as_str) {
        Some("link") => {
            let (Some(server), Some(code)) = (args.get(1), args.get(2)) else {
                eprintln!("usage: codeany link <relay url> <pairing code>");
                return 2;
            };
            STATE.lock().unwrap().account = None;
            match rt.block_on(join(server.clone(), code.clone(), false)) {
                Ok(()) => {
                    let (_, id) = account().unwrap_or_default();
                    println!("linked as {} ({id})", device_name());
                    0
                }
                Err(err) => {
                    eprintln!("codeany: {}", describe_error(&err));
                    1
                }
            }
        }
        Some("serve") => {
            init();
            if account().is_none() && peer::peers().is_empty() {
                eprintln!("codeany: not linked; run `codeany link <relay url> <pairing code>` first");
                return 1;
            }
            if let Err(err) = set_hosting(true) {
                eprintln!("codeany: {err:#}");
                return 1;
            }
            match account() {
                Some((server, id)) => println!("codeany: sharing this machine's agents through {server} as {id}; Ctrl-C stops"),
                None => println!("codeany: sharing this machine's agents with paired devices; Ctrl-C stops"),
            }
            rt.block_on(async {
                loop {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                            if let HostStatus::Failed(message) = host_status() {
                                eprintln!("codeany: {message}");
                                return;
                            }
                        }
                    }
                }
            });
            stop_host();
            agl_bridge::shutdown();
            0
        }
        _ => 2,
    }
}

// ---- "Sign in with termany.sh" ------------------------------------------
//
// The same OAuth-style loopback sign-in Termany uses: termany.sh's
// /connect/termany page only sends its link code back to a loopback callback
// (`http://127.0.0.1:PORT/api/nex/login/callback`), so CodeAny serves that
// callback itself for the few minutes a sign-in takes, then trades code +
// state for a device credential in the user's account at POST
// /api/nex/exchange. The code is useless without the state, which never
// leaves this process.

const CALLBACK_PATH: &str = "/api/nex/login/callback";
const LOGIN_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// termany.sh, or `TERMANY_SITE_URL` for a local site.
fn site_url() -> String {
    std::env::var("TERMANY_SITE_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| "https://termany.sh".into())
        .trim_end_matches('/')
        .to_string()
}

/// Where a browser sign-in stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Login {
    Idle,
    /// Waiting for the browser to come back.
    Pending,
    Failed(String),
}

static LOGIN: Mutex<(Login, Option<tokio::task::JoinHandle<()>>)> = Mutex::new((Login::Idle, None));

pub fn login_status() -> Login {
    LOGIN.lock().unwrap().0.clone()
}

fn set_login(state: Login) {
    LOGIN.lock().unwrap().0 = state;
}

/// Give up on a browser sign-in: its link stops working.
pub fn cancel_login() {
    let mut login = LOGIN.lock().unwrap();
    if let Some(task) = login.1.take() {
        task.abort();
    }
    login.0 = Login::Idle;
}

/// Start "Sign in with termany.sh": listen on a loopback port and return
/// the page to open in the browser. [`login_status`] follows it.
pub async fn start_login(host: bool) -> Result<String> {
    cancel_login();
    let state = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let name = device_name();
    let (url, listener) = agl_bridge::spawn({
        let (state, name) = (state.clone(), name.clone());
        async move {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
            let port = listener.local_addr()?.port();
            let mut url = url::Url::parse(&format!("{}/connect/termany", site_url()))?;
            url.query_pairs_mut()
                .append_pair("callback", &format!("http://127.0.0.1:{port}{CALLBACK_PATH}"))
                .append_pair("state", &state)
                .append_pair("device", &name);
            anyhow::Ok((url.to_string(), listener))
        }
    })
    .await?;
    let task = agl_bridge::runtime().spawn(async move {
        let deadline = tokio::time::Instant::now() + LOGIN_TTL;
        loop {
            let accepted = tokio::time::timeout_at(deadline, listener.accept()).await;
            let Ok(Ok((socket, _))) = accepted else {
                set_login(Login::Failed(t("sign-in timed out; start again").into()));
                break;
            };
            if serve_callback(socket, &state, &name, host).await {
                break;
            }
        }
    });
    let mut login = LOGIN.lock().unwrap();
    login.0 = Login::Pending;
    login.1 = Some(task);
    Ok(url)
}

/// Answer one request to the loopback port; true once the sign-in settled.
async fn serve_callback(mut socket: tokio::net::TcpStream, expected: &str, name: &str, host: bool) -> bool {
    let Some(target) = read_request_target(&mut socket).await else {
        return false;
    };
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        let _ = respond(&mut socket, 400, "Bad request").await;
        return false;
    };
    if url.path() != CALLBACK_PATH {
        let _ = respond(&mut socket, 404, "Not found").await;
        return false;
    }
    let params: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
    let state = params.get("state").map(String::as_str).unwrap_or("");
    if state != expected {
        // A sign-in this app never started, or someone else's: refuse it
        // and keep waiting for the real one.
        let _ = respond(&mut socket, 400, t("This sign-in link is unknown or expired.")).await;
        return false;
    }
    let code = params.get("code").map(String::as_str).unwrap_or("");
    let result = if code.is_empty() {
        Err(anyhow!(t("sign-in was not approved")))
    } else {
        exchange(code, state).await.and_then(|(server, credential, account)| {
            if STATE.lock().unwrap().account.is_some() {
                anyhow::bail!(t("already signed in; sign out first"));
            }
            link(server, credential, account, host)
        })
    };
    let page = match &result {
        Ok(()) => {
            set_login(Login::Idle);
            tf("Signed in as {name}. You can return to CodeAny.", &[("name", device_name_or(name))])
        }
        Err(err) => {
            set_login(Login::Failed(format!("{err:#}")));
            tf("Sign-in failed: {err}", &[("err", format!("{err:#}"))])
        }
    };
    let _ = respond(&mut socket, 200, &page).await;
    true
}

fn device_name_or(name: &str) -> String {
    if name.is_empty() { t("this device").into() } else { name.to_string() }
}

#[derive(Deserialize)]
struct ExchangeBody {
    code: Option<i64>,
    message: Option<String>,
    data: Option<ExchangeData>,
}

#[derive(Deserialize)]
struct ExchangeData {
    server: Option<String>,
    account: Option<String>,
    credential: Option<DeviceCredential>,
}

async fn exchange(code: &str, state: &str) -> Result<(String, DeviceCredential, Option<String>)> {
    let resp = reqwest::Client::new()
        .post(format!("{}/api/nex/exchange", site_url()))
        .json(&serde_json::json!({ "code": code, "state": state }))
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| anyhow!("cannot reach termany.sh: {e}"))?;
    let status = resp.status();
    let body: Option<ExchangeBody> = resp.json().await.ok();
    let body = match body {
        Some(body) if status.is_success() && body.code == Some(0) => body,
        Some(ExchangeBody { message: Some(message), .. }) if !message.is_empty() => anyhow::bail!(message),
        _ => anyhow::bail!("termany.sh answered HTTP {}", status.as_u16()),
    };
    let data = body.data.ok_or_else(|| anyhow!("termany.sh returned no credential"))?;
    match (data.server, data.credential) {
        (Some(server), Some(credential))
            if !credential.device_id.is_empty() && !credential.device_token.is_empty() =>
        {
            Ok((server, credential, data.account.filter(|a| !a.is_empty())))
        }
        _ => anyhow::bail!("termany.sh returned an incomplete credential"),
    }
}

/// The request head's target (`/path?query`), for a GET.
async fn read_request_target(socket: &mut tokio::net::TcpStream) -> Option<String> {
    use tokio::io::AsyncReadExt as _;
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), socket.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let mut parts = head.lines().next()?.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    (method == "GET").then(|| target.to_string())
}

async fn respond(socket: &mut tokio::net::TcpStream, status: u16, message: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let escaped = message.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let body = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <title>CodeAny</title><body style=\"font:16px -apple-system,system-ui,sans-serif;padding:48px 24px;text-align:center;color:#333\">\
         <p>{escaped}</p></body>"
    );
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(body.as_bytes()).await?;
    socket.shutdown().await
}
