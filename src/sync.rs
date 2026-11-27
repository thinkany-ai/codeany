//! Threads across devices.
//!
//! A thread lives on the device its agent runs on, like a Termany workspace:
//! that device's database holds the record everyone else mirrors. Each
//! CodeAny that shares its agents also serves its threads, as the `codeany`
//! service next to `agl` (through the relay, or a paired device's direct
//! link):
//!
//! - `threads.list`: a summary of every thread that runs here;
//! - `threads.get {id}`: one thread in full;
//! - `threads.put {thread}`: a thread another device started or continued
//!   here, written back;
//! - `threads.delete {id}`;
//! - `projects.logo {path}`: a project's logo, as the sidebar shows it.
//!
//! The other devices poll the list, fetch what changed, and show those
//! threads under the device's name in their sidebar. Continuing one there
//! runs its agent here (as before) and puts the result back, so both
//! devices see the same conversation.

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::Duration,
};

use agl::protocol::WireMsg;
use anyhow::{Result, anyhow};
use futures::{SinkExt as _, StreamExt as _, channel::mpsc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use crate::{agl_bridge, threads::{Thread, Workspace}};

pub const SERVICE: &str = "codeany";
const POLL: Duration = Duration::from_secs(6);
/// Unsettled threads of another device are mirrored; settled ones only
/// when already here.
const MAX_MIRRORED: usize = 60;

/// What changed about a thread, cheaply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub updated_at: u64,
    pub items: usize,
    pub title: String,
    pub settled: bool,
    pub agent: String,
    /// How big the last item is: it grows in place while a turn streams.
    #[serde(default)]
    pub last: usize,
}

impl Fingerprint {
    pub fn of(thread: &Thread) -> Self {
        Self {
            updated_at: thread.updated_at,
            items: thread.items.len(),
            title: thread.title.clone(),
            settled: thread.settled,
            agent: thread.agent.clone(),
            last: thread.items.last().map(crate::threads::item_weight).unwrap_or(0),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub id: u64,
    #[serde(flatten)]
    pub fp: Fingerprint,
}

/// What the agent panel applies to its store.
pub enum Incoming {
    /// Another device wrote a thread that runs here.
    Put(Thread),
    /// Another device deleted a thread that runs here.
    Delete(u64),
    /// A thread that runs on another device, as that device has it now.
    Mirror(Thread),
    /// Write back the threads kept here for `machine` that it did not list.
    Resend { machine: String, listed: HashSet<u64> },
    /// Another device made a workspace that lives here.
    PutWorkspace(Workspace),
    /// The workspaces that live on `machine`, as it has them now.
    Workspaces { machine: String, list: Vec<Workspace> },
    /// A thread `machine` no longer has.
    Gone { machine: String, id: u64 },
}

struct State {
    /// Threads that run here, as last saved.
    local: HashMap<u64, Thread>,
    /// Workspaces that live here, as last saved.
    workspaces: Vec<Workspace>,
    /// Threads that run on another device: (device, id) → what that device
    /// has, as far as we know; a change from it is pushed there.
    known: HashMap<(String, u64), Fingerprint>,
    /// Changed here, not yet written back to their device.
    dirty: HashMap<(String, u64), Thread>,
    /// Ids each device listed last time, to notice deletions.
    listed: HashMap<String, HashSet<u64>>,
    /// Ids deleted here that their device should drop.
    deletes: Vec<(String, u64)>,
    /// Devices whose missing threads the agent panel should write back.
    resend: Vec<String>,
    inbox: Option<mpsc::UnboundedSender<Incoming>>,
    url: Option<String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        local: HashMap::new(),
        workspaces: Vec::new(),
        known: HashMap::new(),
        dirty: HashMap::new(),
        listed: HashMap::new(),
        deletes: Vec::new(),
        resend: Vec::new(),
        inbox: None,
        url: None,
    })
});
static START: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Start syncing; the agent panel applies what arrives. Call once.
pub fn start(threads: &[Thread]) -> mpsc::UnboundedReceiver<Incoming> {
    let (tx, rx) = mpsc::unbounded();
    {
        let mut state = STATE.lock().unwrap();
        state.inbox = Some(tx);
        for thread in threads {
            match &thread.machine {
                None => {
                    state.local.insert(thread.id, thread.clone());
                }
                Some(machine) => {
                    state.known.insert((machine.clone(), thread.id), Fingerprint::of(thread));
                }
            }
        }
    }
    agl_bridge::runtime().spawn(async {
        loop {
            poll().await;
            tokio::time::sleep(POLL).await;
        }
    });
    rx
}

/// After every save: serve the new local threads, and queue changes to
/// other devices' threads for writing back.
pub fn saved(threads: &[Thread]) {
    let mut state = STATE.lock().unwrap();
    // Copy only the local threads that changed since the last save.
    let ids: HashSet<u64> = threads.iter().filter(|t| t.machine.is_none()).map(|t| t.id).collect();
    state.local.retain(|id, _| ids.contains(id));
    for thread in threads.iter().filter(|t| t.machine.is_none()) {
        let fp = Fingerprint::of(thread);
        if state.local.get(&thread.id).map(Fingerprint::of) != Some(fp) {
            state.local.insert(thread.id, thread.clone());
        }
    }
    // A thread nobody wrote in yet is not worth showing elsewhere.
    for thread in threads.iter().filter(|t| !t.items.is_empty()) {
        let Some(machine) = &thread.machine else { continue };
        let key = (machine.clone(), thread.id);
        let fp = Fingerprint::of(thread);
        if state.known.get(&key) != Some(&fp) {
            state.known.insert(key.clone(), fp);
            state.dirty.insert(key, thread.clone());
        }
    }
}

/// After every save: the workspaces that live here, for other devices.
pub fn saved_workspaces(workspaces: &[Workspace]) {
    STATE.lock().unwrap().workspaces = workspaces.iter().filter(|w| w.device.is_none()).cloned().collect();
}

/// Make a workspace on the device it lives on (it keeps the record).
pub async fn put_workspace(machine: String, mut workspace: Workspace) -> anyhow::Result<()> {
    workspace.device = None;
    workspace.device_name = None;
    agl_bridge::backend_for(Some(&machine), SERVICE)
        .call("workspaces.put", json!({ "workspace": workspace }))
        .await
        .map(|_| ())
}

/// The workspaces that live on `machine`, to mount one here.
pub async fn device_workspaces(machine: String) -> anyhow::Result<Vec<Workspace>> {
    let list = agl_bridge::backend_for(Some(&machine), SERVICE).call("workspaces.list", json!({})).await?;
    Ok(serde_json::from_value(list)?)
}

/// Queue threads kept here for writing back to their device.
pub fn resend(threads: &[Thread]) {
    let mut state = STATE.lock().unwrap();
    for thread in threads {
        if let Some(machine) = &thread.machine {
            state.dirty.insert((machine.clone(), thread.id), thread.clone());
        }
    }
}

/// Deleted here: delete it on its device too.
pub fn deleted(machine: &str, id: u64) {
    let mut state = STATE.lock().unwrap();
    let key = (machine.to_string(), id);
    state.known.remove(&key);
    state.dirty.remove(&key);
    state.deletes.push(key);
}

fn send(incoming: Incoming) {
    if let Some(tx) = &STATE.lock().unwrap().inbox {
        let _ = tx.unbounded_send(incoming);
    }
}

// ---- Serving ----------------------------------------------------------------

/// The local URL of this device's thread service, started on first use.
pub async fn server_url() -> Result<String> {
    let _guard = START.lock().await;
    if let Some(url) = STATE.lock().unwrap().url.clone() {
        return Ok(url);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let url = format!("ws://127.0.0.1:{port}/sync?token={token}");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let token = token.clone();
            tokio::spawn(async move {
                let check = |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                             resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    let ok = req.uri().query().is_some_and(|q| q.split('&').any(|kv| kv == format!("token={token}")));
                    if ok {
                        Ok(resp)
                    } else {
                        let mut refused = tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(None);
                        *refused.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
                        Err(refused)
                    }
                };
                if let Ok(ws) = tokio_tungstenite::accept_hdr_async(socket, check).await {
                    serve(ws).await;
                }
            });
        }
    });
    STATE.lock().unwrap().url = Some(url.clone());
    Ok(url)
}

async fn serve(ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
    let (mut sink, mut stream) = ws.split();
    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else {
            if matches!(msg, Message::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(WireMsg::Request { id, method, params }) = serde_json::from_str::<WireMsg>(&text) else { continue };
        let handled = if method == "projects.logo" {
            // It walks the project's folder: off the socket's thread.
            tokio::task::spawn_blocking(move || project_logo(params))
                .await
                .unwrap_or_else(|err| Err(anyhow!("{err}")))
        } else {
            handle(&method, params)
        };
        let (result, error) = match handled {
            Ok(value) => (Some(value), None),
            Err(err) => (None, Some(format!("{err:#}"))),
        };
        let reply = WireMsg::Response { id, result, error }.to_json();
        if sink.send(Message::Text(reply.into())).await.is_err() {
            break;
        }
    }
}

/// `projects.logo {path}`: the logo the sidebar shows for a project here, a
/// PNG in base64 (`{png}`), or null when it has none.
fn project_logo(params: Value) -> Result<Value> {
    use base64::Engine as _;
    let path = params["path"].as_str().ok_or_else(|| anyhow!("path is required"))?;
    let path = std::path::Path::new(path);
    if !path.is_absolute() || !path.is_dir() {
        return Ok(Value::Null);
    }
    Ok(match crate::project_avatar::logo_png(path) {
        Some(png) => json!({ "png": base64::engine::general_purpose::STANDARD.encode(png) }),
        None => Value::Null,
    })
}

fn handle(method: &str, params: Value) -> Result<Value> {
    match method {
        "threads.list" => {
            let state = STATE.lock().unwrap();
            let list: Vec<Summary> = state
                .local
                .values()
                .filter(|t| !t.items.is_empty())
                .map(|t| Summary { id: t.id, fp: Fingerprint::of(t) })
                .collect();
            Ok(serde_json::to_value(list)?)
        }
        "threads.get" => {
            let id = params["id"].as_u64().ok_or_else(|| anyhow!("id is required"))?;
            let state = STATE.lock().unwrap();
            Ok(serde_json::to_value(state.local.get(&id))?)
        }
        "threads.put" => {
            let mut thread: Thread = serde_json::from_value(params["thread"].clone())?;
            // It runs here: from this device's side it is a local thread.
            thread.machine = None;
            thread.machine_name = None;
            STATE.lock().unwrap().local.insert(thread.id, thread.clone());
            send(Incoming::Put(thread));
            Ok(json!({ "ok": true }))
        }
        "threads.delete" => {
            let id = params["id"].as_u64().ok_or_else(|| anyhow!("id is required"))?;
            STATE.lock().unwrap().local.remove(&id);
            send(Incoming::Delete(id));
            Ok(json!({ "ok": true }))
        }
        "workspaces.list" => Ok(serde_json::to_value(&STATE.lock().unwrap().workspaces)?),
        "workspaces.put" => {
            let mut workspace: Workspace = serde_json::from_value(params["workspace"].clone())?;
            workspace.device = None;
            workspace.device_name = None;
            send(Incoming::PutWorkspace(workspace));
            Ok(json!({ "ok": true }))
        }
        "machine.info" => Ok(json!({ "name": crate::remote::this_name() })),
        // The folder of a thread with no project, as this device gives its own.
        "threads.dir" => {
            let id = params["id"].as_u64().ok_or_else(|| anyhow!("id is required"))?;
            let dir = crate::paths::thread_dir(id);
            std::fs::create_dir_all(&dir)?;
            Ok(json!({ "path": dir }))
        }
        other => Err(anyhow!("unknown method '{other}'")),
    }
}

// ---- Mirroring other devices ------------------------------------------------

async fn poll() {
    // Write back first, so a list never undoes a change made here.
    let (dirty, deletes) = {
        let mut state = STATE.lock().unwrap();
        (state.dirty.clone(), std::mem::take(&mut state.deletes))
    };
    let reachable: HashMap<String, String> = crate::remote::cached_devices()
        .into_iter()
        .filter(|d| !d.is_self && d.usable())
        .map(|d| (d.id.clone(), d.name.clone()))
        .collect();
    let mut failed_deletes = Vec::new();
    for (machine, id) in deletes {
        if !reachable.contains_key(&machine) {
            failed_deletes.push((machine, id));
            continue;
        }
        let call = agl_bridge::backend_for(Some(&machine), SERVICE).call("threads.delete", json!({ "id": id }));
        if call.await.is_err() {
            failed_deletes.push((machine, id));
        }
    }
    if !failed_deletes.is_empty() {
        STATE.lock().unwrap().deletes.extend(failed_deletes);
    }
    for ((machine, id), mut thread) in dirty {
        if !reachable.contains_key(&machine) {
            continue;
        }
        let fp = Fingerprint::of(&thread);
        thread.machine = None;
        thread.machine_name = None;
        let call = agl_bridge::backend_for(Some(&machine), SERVICE)
            .call("threads.put", json!({ "thread": thread }));
        if call.await.is_ok() {
            let mut state = STATE.lock().unwrap();
            if state.dirty.get(&(machine.clone(), id)).is_some_and(|t| Fingerprint::of(t) == fp) {
                state.dirty.remove(&(machine, id));
            }
        }
    }

    for (machine, name) in reachable {
        let backend = agl_bridge::backend_for(Some(&machine), SERVICE);
        // An older CodeAny there has no workspaces to list.
        if let Ok(list) = backend.call("workspaces.list", json!({})).await {
            if let Ok(list) = serde_json::from_value::<Vec<Workspace>>(list) {
                send(Incoming::Workspaces { machine: machine.clone(), list });
            }
        }
        let Ok(list) = backend.call("threads.list", json!({})).await else { continue };
        let Ok(list) = serde_json::from_value::<Vec<Summary>>(list) else { continue };
        let ids: HashSet<u64> = list.iter().map(|s| s.id).collect();
        let (wanted, gone) = {
            let mut state = STATE.lock().unwrap();
            let mut fresh: Vec<&Summary> = list
                .iter()
                .filter(|s| {
                    let key = (machine.clone(), s.id);
                    let here = state.known.get(&key);
                    // Ours not yet written back wins until it is.
                    !state.dirty.contains_key(&key)
                        && here != Some(&s.fp)
                        && (here.is_some() || !s.fp.settled)
                })
                .collect();
            fresh.sort_by_key(|s| std::cmp::Reverse(s.fp.updated_at));
            let unknown: Vec<u64> = fresh
                .iter()
                .filter(|s| !state.known.contains_key(&(machine.clone(), s.id)))
                .skip(MAX_MIRRORED)
                .map(|s| s.id)
                .collect();
            let wanted: Vec<u64> = fresh.iter().map(|s| s.id).filter(|id| !unknown.contains(id)).collect();
            let gone: Vec<u64> = match state.listed.insert(machine.clone(), ids.clone()) {
                // Listed before and not now: deleted there.
                Some(before) => before
                    .difference(&ids)
                    .copied()
                    .filter(|id| !state.dirty.contains_key(&(machine.clone(), *id)))
                    .collect(),
                // The first list since launch: a thread kept here that the
                // device lacks may never have reached it; write it back
                // rather than lose it.
                None => {
                    state.resend.push(machine.clone());
                    Vec::new()
                }
            };
            (wanted, gone)
        };
        let resend = std::mem::take(&mut STATE.lock().unwrap().resend);
        if resend.contains(&machine) {
            send(Incoming::Resend { machine: machine.clone(), listed: ids.clone() });
        }
        for id in gone {
            STATE.lock().unwrap().known.remove(&(machine.clone(), id));
            send(Incoming::Gone { machine: machine.clone(), id });
        }
        for id in wanted {
            let Ok(value) = backend.call("threads.get", json!({ "id": id })).await else { continue };
            let Ok(Some(mut thread)) = serde_json::from_value::<Option<Thread>>(value) else { continue };
            thread.machine = Some(machine.clone());
            thread.machine_name = Some(name.clone());
            STATE.lock().unwrap().known.insert((machine.clone(), id), Fingerprint::of(&thread));
            send(Incoming::Mirror(thread));
        }
    }
}
