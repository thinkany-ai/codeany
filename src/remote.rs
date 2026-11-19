//! The user's other machines, through a nex relay.
//!
//! This CodeAny is one nex device (`~/.codeany/nex.json`, mode 0600). It can
//! reach the agl of any device in the same account that exposes an `agl`
//! service, and, with hosting on, expose its own: then the conversation runs
//! on a laptop while the agents run here, as with T3 Connect.

use std::{path::PathBuf, sync::Mutex};

use anyhow::{Result, anyhow};
use nex::{
    ExposedService, Host, NexClient,
    proto::{DeviceCredential, DeviceInfo, PairingCode},
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::agl_bridge;

/// The nex service name CodeAny exposes its agl daemon under.
pub const SERVICE: &str = "agl";
const APP: &str = "codeany";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Account {
    server: String,
    credential: DeviceCredential,
    /// Expose this machine's agents to the account's other devices.
    #[serde(default)]
    host: bool,
}

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
    use std::io::Write as _;
    let file = path();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(&file)?.write_all(serde_json::to_string_pretty(account)?.as_bytes())?;
    Ok(())
}

/// Read the saved account and start hosting if it was on. Call once.
pub fn init() {
    let account = std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str::<Account>(&text).ok());
    let host = account.as_ref().is_some_and(|a| a.host);
    STATE.lock().unwrap().account = account;
    if host {
        start_host();
    }
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
        .ok_or("this CodeAny is not linked to a nex relay (Settings → Remote)")?;
    NexClient::new(&account.server, account.credential.clone()).map_err(|e| format!("{e:#}"))
}

/// A nex error as the person can act on it.
pub fn describe_error(err: &anyhow::Error) -> String {
    if let Some(api) = err.downcast_ref::<nex::ApiError>() {
        return match api.status {
            401 => "this device's nex credential was revoked; link it again in Settings → Remote".into(),
            404 => "the machine is offline".into(),
            409 => "two devices share that name".into(),
            _ => format!("{api}"),
        };
    }
    if let Some(tungstenite::Error::Http(resp)) = err.downcast_ref::<tungstenite::Error>() {
        return match resp.status().as_u16() {
            404 => "the machine is offline".into(),
            502 => "the machine's CodeAny is not sharing its agents".into(),
            code => format!("the relay refused the stream ({code})"),
        };
    }
    format!("{err:#}")
}

use tokio_tungstenite::tungstenite;

fn device_name() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "codeany".into())
}

fn link(server: String, credential: DeviceCredential) -> Result<()> {
    stop_host();
    let account = Account {
        server,
        credential,
        host: false,
    };
    save(&account)?;
    STATE.lock().unwrap().account = Some(account);
    Ok(())
}

/// Join an account with a pairing code from one of its devices.
pub async fn join(server: String, code: String) -> Result<()> {
    let credential = agl_bridge::spawn({
        let server = server.clone();
        async move { NexClient::claim(&server, code.trim(), &device_name(), Some(APP)).await }
    })
    .await?;
    link(server, credential)
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
    link(server, credential)
}

/// Forget the account here, revoking this device on the relay first.
pub async fn sign_out() -> Result<()> {
    stop_host();
    if let Ok(client) = client() {
        let id = client.device_id().to_string();
        let _ = agl_bridge::spawn(async move { client.revoke(&id).await }).await;
    }
    STATE.lock().unwrap().account = None;
    let _ = std::fs::remove_file(path());
    Ok(())
}

pub async fn devices() -> Result<Vec<DeviceInfo>> {
    let client = client().map_err(|e| anyhow!(e))?;
    agl_bridge::spawn(async move { client.devices().await }).await
}

pub async fn pairing_code() -> Result<PairingCode> {
    let client = client().map_err(|e| anyhow!(e))?;
    agl_bridge::spawn(async move { client.create_pairing_code().await }).await
}

pub async fn revoke(device_id: String) -> Result<()> {
    let client = client().map_err(|e| anyhow!(e))?;
    agl_bridge::spawn(async move { client.revoke(&device_id).await }).await
}

/// Whether a device can run conversations for this one.
pub fn is_agent_host(device: &DeviceInfo) -> bool {
    !device.is_self && device.services.iter().any(|s| s.name == SERVICE)
}

pub fn host_status() -> HostStatus {
    STATE.lock().unwrap().host.clone()
}

/// Turn sharing this machine's agents on or off, and remember it.
pub fn set_hosting(on: bool) -> Result<()> {
    {
        let mut state = STATE.lock().unwrap();
        let account = state.account.as_mut().ok_or_else(|| anyhow!("link a nex relay first"))?;
        account.host = on;
        save(account)?;
    }
    if on {
        start_host();
    } else {
        stop_host();
    }
    Ok(())
}

fn stop_host() {
    let mut state = STATE.lock().unwrap();
    if let Some(cancel) = state.host_cancel.take() {
        cancel.cancel();
    }
    state.host = HostStatus::Off;
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
            let endpoint = agl_bridge::local_endpoint().await?;
            let service = ExposedService::ws(SERVICE, &endpoint.url())?;
            Host::new(client, vec![service, ExposedService::echo()])
                .run(cancel.clone())
                .await
        }
        .await;
        if let Err(err) = result {
            let message = format!("{err:#}");
            eprintln!("codeany: sharing agents stopped: {message}");
            let mut state = STATE.lock().unwrap();
            if !cancel.is_cancelled() {
                state.host = HostStatus::Failed(message);
                state.host_cancel = None;
            }
        }
    });
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
            match rt.block_on(join(server.clone(), code.clone())) {
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
            if account().is_none() {
                eprintln!("codeany: not linked; run `codeany link <relay url> <pairing code>` first");
                return 1;
            }
            if let Err(err) = set_hosting(true) {
                eprintln!("codeany: {err:#}");
                return 1;
            }
            let (server, id) = account().unwrap_or_default();
            println!("codeany: sharing this machine's agents through {server} as {id}; Ctrl-C stops");
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
