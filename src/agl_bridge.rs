//! Coding agents through agl's daemon protocol.
//!
//! CodeAny runs an agl daemon inside its own process, on a tokio runtime, and
//! talks to it over the daemon's local WebSocket API: the same protocol it
//! speaks to the daemon of another of the user's machines through a nex
//! stream (see [`crate::remote`]). One [`Backend`] is one such connection,
//! kept up across drops; [`backend`] hands out the one for a machine, `None`
//! being this one.
//!
//! GPUI runs its own executor, so every call here returns a future that runs
//! on the tokio runtime and can be awaited from any executor.

use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use agl::protocol::{AgentEvent, AgentInfo, EventEnvelope, WireMsg};
pub use agl::session::SessionMeta;
use anyhow::{Context as _, Result, anyhow, bail};
use futures::{
    SinkExt as _, StreamExt as _,
    channel::{mpsc, oneshot},
};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

/// How long a call waits for a dropped connection to come back.
const CONNECT_WAIT: Duration = Duration::from_secs(20);
/// Listing agents probes every CLI; listing models may start one.
const CALL_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_BACKOFF: Duration = Duration::from_secs(15);

/// One model an agent can run, as agl lists it.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub efforts: Vec<String>,
    #[serde(default)]
    pub default: bool,
}

pub struct SessionOptions {
    pub agent: String,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub access: String,
    pub resume: Option<String>,
    /// Appended to the agent's launch arguments (Settings → Agents); only
    /// for this machine's agl, which alone accepts them.
    pub extra_args: Vec<String>,
}

/// The executable chosen for an agent on this machine, if any. It is kept
/// as agl's `custom_agents` entry of the same id: the built-in definition
/// (arguments, environment, protocol) with only its command replaced. An
/// entry that only adds environment keeps the agent's own command name.
pub fn agent_command(agent: &str) -> Option<String> {
    let cfg = agl::config::Config::load().ok()?;
    let entry = cfg.custom_agents.iter().find(|c| c.id == agent)?;
    let own = builtin_agent(agent).is_some_and(|d| d.commands.contains(&entry.command));
    (!own).then(|| entry.command.clone())
}

/// Variables set for an agent on this machine, beyond its built-in ones.
pub fn agent_env(agent: &str) -> Vec<(String, String)> {
    let Ok(cfg) = agl::config::Config::load() else { return Vec::new() };
    let Some(entry) = cfg.custom_agents.iter().find(|c| c.id == agent) else { return Vec::new() };
    let builtin = builtin_agent(agent).map(|d| d.env).unwrap_or_default();
    let mut env: Vec<(String, String)> = entry
        .env
        .iter()
        .filter(|(k, v)| !builtin.iter().any(|(bk, bv)| bk == *k && bv == *v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    env.sort();
    env
}

/// Whether an agent's executable can be chosen here: agl's custom agents
/// speak ACP, Claude's or Codex's protocol, not the one-shot ones.
pub fn agent_command_editable(agent: &str) -> bool {
    use agl::agents::Protocol;
    builtin_agent(agent).is_none_or(|d| matches!(d.protocol, Protocol::Claude | Protocol::Codex | Protocol::Acp))
}

fn builtin_agent(agent: &str) -> Option<agl::agents::AgentDef> {
    agl::agents::registry(&agl::config::Config::default()).into_iter().find(|d| d.id == agent)
}

/// Choose an agent's executable (`None` or blank: look it up on the PATH)
/// and the variables its processes start with, on this machine; agl then
/// re-reads its configuration. Both live in agl's `custom_agents` entry.
pub fn set_agent_launch(
    agent: String,
    command: Option<String>,
    env: Vec<(String, String)>,
) -> impl Future<Output = Result<()>> + Send + 'static {
    spawn(async move {
        use agl::agents::Protocol;
        let command = command
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .map(|c| match c.strip_prefix("~/") {
                Some(rest) => std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest).to_string_lossy().into_owned(),
                None => c,
            });
        let mut cfg = agl::config::Config::load()?;
        cfg.custom_agents.retain(|c| c.id != agent);
        if command.is_some() || !env.is_empty() {
            let def = builtin_agent(&agent).ok_or_else(|| anyhow!("unknown agent {agent}"))?;
            let protocol = match def.protocol {
                Protocol::Claude => "claude",
                Protocol::Codex => "codex",
                Protocol::Acp => "acp",
                Protocol::Cursor | Protocol::Pi => bail!("this agent's executable and environment cannot be changed"),
            };
            // No executable chosen: the agent's own command name, the first
            // one found on the PATH, as agl would pick.
            let command = command
                .or_else(|| def.commands.iter().find(|c| agl::shellenv::which(c).is_some()).cloned())
                .or_else(|| def.commands.first().cloned())
                .ok_or_else(|| anyhow!("{agent} has no command"))?;
            let mut merged: std::collections::HashMap<String, String> = def.env.into_iter().collect();
            merged.extend(env);
            cfg.custom_agents.push(agl::config::CustomAgent {
                id: def.id,
                name: Some(def.name),
                command,
                args: def.args,
                protocol: protocol.into(),
                env: merged,
                // agl keeps a built-in agent's own icon when none is named.
                icon: None,
            });
        }
        cfg.save()?;
        backend(None).call("env.reload", json!({})).await?;
        Ok(())
    })
}

/// What the connection test asks for, and the answer it wants back.
const TEST_PROMPT: &str = "Connection test. Reply with exactly CODEANY_OK and nothing else. Do not use any tools.";
const TEST_ANSWER: &str = "CODEANY_OK";

/// Run one short turn with an agent on this machine, the way a thread
/// would (its executable and `extra_args` included), and time it. This
/// uses the agent's own account, so it may use a little of its quota.
pub fn test_agent(agent: String, extra_args: Vec<String>) -> impl Future<Output = Result<Duration>> + Send + 'static {
    spawn(async move {
        let started = Instant::now();
        let dir = std::env::temp_dir().join("codeany-agent-test");
        std::fs::create_dir_all(&dir)?;
        let local = backend(None);
        let meta = local
            .open_session(
                format!("codeany:test:{agent}:{}", uuid::Uuid::new_v4()),
                SessionOptions {
                    agent,
                    cwd: Some(dir),
                    model: None,
                    effort: None,
                    access: "read_only".into(),
                    resume: None,
                    extra_args,
                },
            )
            .await?;
        let mut events = local.subscribe(meta.id.clone());
        let turn = uuid::Uuid::new_v4().to_string();
        let sent = local.prompt(&meta.id, &turn, TEST_PROMPT.into(), "queue").await;
        let answer = match sent {
            Err(err) => Err(err),
            Ok(()) => tokio::time::timeout(Duration::from_secs(60), async {
                let mut text = String::new();
                while let Some(event) = events.next().await {
                    match event {
                        AgentEvent::TextDelta { text: delta, .. } => text.push_str(&delta),
                        AgentEvent::TurnEnded { outcome, error, .. } => {
                            return match outcome.as_str() {
                                "completed" => Ok(text),
                                _ => Err(anyhow!("{}", error.unwrap_or(outcome))),
                            };
                        }
                        AgentEvent::Error { message } => return Err(anyhow!("{message}")),
                        _ => {}
                    }
                }
                Err(anyhow!("the agent stopped before answering"))
            })
            .await
            .unwrap_or_else(|_| Err(anyhow!("no answer within 60 seconds"))),
        };
        local.close(meta.id.clone());
        let text = answer?;
        if !text.contains(TEST_ANSWER) {
            let shown: String = text.trim().chars().take(200).collect();
            bail!("unexpected answer: {shown}");
        }
        Ok(started.elapsed())
    })
}

/// Run agl as part of CodeAny: under CodeAny's name in messages, with its
/// state in `~/.codeany/agl` (apart from the user's own `~/.agl`), and with
/// agents and computer use started through CodeAny's own executable, so
/// macOS attributes them to CodeAny. Must run before any other thread
/// starts and before anything else uses agl.
pub fn set_home() {
    let brand = agl::brand::Brand {
        cli_name: "codeany".into(),
        display_name: "CodeAny".into(),
        home: Some(crate::paths::agent_home()),
        service_label: "ai.codeany.daemon".into(),
        systemd_unit: "codeany.service".into(),
        // Updated with the app, not through agl's releases.
        releases: None,
        app_launcher: std::env::current_exe().ok(),
        product: Some(agl::brand::Product {
            name: "CodeAny".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }),
        ..agl::brand::Brand::default()
    };
    if let Err(err) = agl::brand::set(brand) {
        eprintln!("codeany: {err:#}");
    }
    // A `CODEANY_HOME` set for this run says where its data lives, whatever
    // `AGL_HOME` came along: one started from a CodeAny terminal or agent
    // (`make dev` is) inherits that app's, and would try to serve its home.
    if std::env::var_os("AGL_HOME").is_some() && std::env::var_os("CODEANY_HOME").is_none() {
        return;
    }
    let dir = crate::paths::agent_home();
    // SAFETY: called at the top of `main`, before any thread exists.
    unsafe { std::env::set_var("AGL_HOME", dir) };
}

pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("codeany-agents")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Run `fut` on the tokio runtime; await the result from any executor.
pub fn spawn<T: Send + 'static>(
    fut: impl Future<Output = Result<T>> + Send + 'static,
) -> impl Future<Output = Result<T>> + Send + 'static {
    let (tx, rx) = oneshot::channel();
    runtime().spawn(async move {
        let _ = tx.send(fut.await);
    });
    async move { rx.await.map_err(|_| anyhow!("the agent runtime stopped"))? }
}

// ---------------------------------------------------------------------------
// This machine's daemon
// ---------------------------------------------------------------------------

/// Where this machine's agl daemon listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
}

impl Endpoint {
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/ws?token={}", self.port, self.token)
    }
}

/// The daemon this process runs, once started.
static OWN: tokio::sync::Mutex<Option<agl::daemon::EmbeddedDaemon>> = tokio::sync::Mutex::const_new(None);
static START: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// The daemon described by `$AGL_HOME/daemon.json`, if its process lives.
fn running_daemon() -> Option<(u32, Endpoint)> {
    let text = std::fs::read_to_string(agl::config::daemon_file()).ok()?;
    let info: Value = serde_json::from_str(&text).ok()?;
    let pid = info["pid"].as_u64()? as u32;
    let endpoint = Endpoint {
        port: info["port"].as_u64()? as u16,
        token: info["token"].as_str()?.to_string(),
    };
    pid_alive(pid).then_some((pid, endpoint))
}

/// Whether an agent service answers at `endpoint`. Something listening on
/// its port is not enough: another program may have taken the port since.
async fn answers(endpoint: &Endpoint) -> bool {
    let connect = tokio_tungstenite::connect_async(endpoint.url());
    matches!(tokio::time::timeout(Duration::from_secs(3), connect).await, Ok(Ok(_)))
}

/// This machine's daemon: the one already running for CodeAny's agent
/// home (another CodeAny on the same data), or one hosted in this process.
pub async fn local_endpoint() -> Result<Endpoint> {
    let _guard = START.lock().await;
    let mut own = OWN.lock().await;
    if let Some(d) = own.as_ref().filter(|d| d.is_running()) {
        return Ok(Endpoint { port: d.port, token: d.token.clone() });
    }
    // `CODEANY_AGL_PORT` pins the port (`make dev` uses 18022); a daemon
    // already running on another port is then not reused.
    let port = std::env::var("CODEANY_AGL_PORT").ok().and_then(|p| p.parse::<u16>().ok());
    if let Some((_, endpoint)) = running_daemon().filter(|(_, e)| port.is_none_or(|p| p == e.port)) {
        if answers(&endpoint).await {
            return Ok(endpoint);
        }
        // Left by a daemon that did not get to clean up: its pid may be
        // another process by now (and Windows cannot be asked here at all),
        // its port another program's. A daemon writes the file once it
        // listens, so nothing is starting.
        let _ = std::fs::remove_file(agl::config::daemon_file());
    }
    let cfg = agl::config::Config::load()?;
    let opts = agl::daemon::DaemonOptions {
        port: Some(port.unwrap_or(0)),
        parent_pid: None,
        token: Some(uuid::Uuid::new_v4().simple().to_string()),
    };
    let d = agl::daemon::spawn_embedded(cfg, opts)
        .await
        .context("the agent service failed to start")?;
    let endpoint = Endpoint { port: d.port, token: d.token.clone() };
    *own = Some(d);
    Ok(endpoint)
}

/// Start this machine's daemon in the background.
pub fn start_local() {
    let _ = backend(None);
}

/// Stop the daemon this process runs, and every agent process it owns.
/// Leaves a daemon another CodeAny runs alone. Blocks for up to 5 seconds.
pub fn shutdown() {
    runtime().block_on(async {
        let Some(d) = OWN.lock().await.take() else { return };
        let _ = tokio::time::timeout(Duration::from_secs(5), d.shutdown()).await;
    });
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Connected,
    /// The last attempt failed; retrying.
    Failed(String),
}

struct Sub {
    senders: Vec<mpsc::UnboundedSender<AgentEvent>>,
    /// The newest event seen, to fill in what a dropped connection missed.
    last_seq: u64,
}

type Pending = HashMap<String, tokio::sync::oneshot::Sender<Result<Value>>>;

struct Shared {
    machine: Option<String>,
    /// The nex service on `machine`: `agl`, or CodeAny's own `codeany`.
    service: &'static str,
    out: Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>,
    pending: Mutex<Pending>,
    subs: Mutex<HashMap<String, Sub>>,
    next: AtomicU64,
    state: watch::Sender<ConnState>,
}

/// One agl daemon, local or on another machine, over one WebSocket that is
/// reopened whenever it drops.
pub struct Backend {
    shared: Arc<Shared>,
}

type BackendKey = (Option<String>, &'static str);
static BACKENDS: Mutex<Option<HashMap<BackendKey, Arc<Backend>>>> = Mutex::new(None);

/// The backend for `machine` (a nex device id, or a paired device's
/// `peer:` id), `None` for this machine.
pub fn backend(machine: Option<&str>) -> Arc<Backend> {
    backend_for(machine, "agl")
}

/// Whether this window talks to `machine`'s agents right now. Only looks:
/// it never opens a connection.
pub fn is_connected(machine: &str) -> bool {
    BACKENDS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|all| all.get(&(Some(machine.to_string()), "agl")))
        .is_some_and(|b| b.state() == ConnState::Connected)
}

/// A connection to `service` on `machine`, kept up like [`backend`]'s.
pub fn backend_for(machine: Option<&str>, service: &'static str) -> Arc<Backend> {
    let key = (machine.map(str::to_string), service);
    let mut all = BACKENDS.lock().unwrap();
    all.get_or_insert_with(HashMap::new)
        .entry(key.clone())
        .or_insert_with(|| Arc::new(Backend::start(key.0, service)))
        .clone()
}

async fn open(machine: &Option<String>, service: &str) -> Result<crate::remote::Ws> {
    match machine {
        None => {
            let endpoint = local_endpoint().await?;
            let (ws, _) = tokio_tungstenite::connect_async(endpoint.url())
                .await
                .context("connect to the local agent service")?;
            Ok(Box::new(ws))
        }
        Some(device) => crate::remote::connect(device, service).await,
    }
}

impl Backend {
    fn start(machine: Option<String>, service: &'static str) -> Backend {
        let shared = Arc::new(Shared {
            machine,
            service,
            out: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            subs: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            state: watch::Sender::new(ConnState::Connecting),
        });
        runtime().spawn(Shared::run(shared.clone()));
        Backend { shared }
    }

    pub fn state(&self) -> ConnState {
        self.shared.state.borrow().clone()
    }

    /// One request; the reply's `result`.
    pub fn call(&self, method: &str, params: Value) -> impl Future<Output = Result<Value>> + Send + 'static {
        let shared = self.shared.clone();
        let method = method.to_string();
        spawn(async move { shared.call(&method, params).await })
    }

    /// Installed agents and what agl can do with each.
    pub fn detect(&self) -> impl Future<Output = Result<Vec<AgentInfo>>> + Send + 'static {
        let call = self.call("agents.list", json!({}));
        async move { Ok(serde_json::from_value(call.await?)?) }
    }

    /// Models an agent offers, without starting a conversation.
    pub fn list_models(&self, agent: &str) -> impl Future<Output = Result<Vec<ModelInfo>>> + Send + 'static {
        let call = self.call("agents.models", json!({ "agent": agent }));
        async move {
            let value = call.await?;
            Ok(value["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| serde_json::from_value(m.clone()).ok())
                .collect())
        }
    }

    /// The machine's home directory, where a remote folder picker starts.
    pub fn home(&self) -> impl Future<Output = Result<PathBuf>> + Send + 'static {
        let call = self.call("machine.info", json!({}));
        async move {
            let info = call.await?;
            info["home"]
                .as_str()
                .filter(|h| !h.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| anyhow!("CodeAny there is too old to report its home folder"))
        }
    }

    /// The folders in `dir`, by name, on the backend's machine.
    pub fn list_dirs(&self, dir: PathBuf) -> impl Future<Output = Result<Vec<String>>> + Send + 'static {
        let call = self.call(
            "exec",
            json!({ "argv": ["ls", "-1Ap", dir], "timeout_ms": 10_000 }),
        );
        async move {
            let out = call.await?;
            if out["exit_code"].as_i64() != Some(0) {
                bail!("{}", out["stderr"].as_str().unwrap_or("cannot list the folder").trim());
            }
            let mut dirs: Vec<String> = out["stdout"]
                .as_str()
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.strip_suffix('/'))
                .map(str::to_string)
                .collect();
            dirs.sort_by_key(|d| (d.starts_with('.'), d.to_lowercase()));
            Ok(dirs)
        }
    }

    /// Open the session called `name`, creating it on first use. `resume`
    /// attaches a new session to an existing native conversation.
    pub fn open_session(
        &self,
        name: String,
        opts: SessionOptions,
    ) -> impl Future<Output = Result<SessionMeta>> + Send + 'static {
        let shared = self.shared.clone();
        spawn(async move {
            let meta: SessionMeta = serde_json::from_value(
                shared
                    .call(
                        "session.create",
                        json!({
                            "agent": opts.agent,
                            "cwd": opts.cwd,
                            "model": opts.model,
                            "effort": opts.effort,
                            "access": opts.access,
                            "name": name,
                            "resume": opts.resume,
                            "extra_args": opts.extra_args,
                        }),
                    )
                    .await?,
            )?;
            // A reused session keeps its old settings; bring them up to date.
            if meta.access != opts.access || meta.model != opts.model || meta.effort != opts.effort {
                let value = shared
                    .call(
                        "session.configure",
                        json!({
                            "session_id": meta.id,
                            "access": opts.access,
                            "model": opts.model,
                            "effort": opts.effort,
                        }),
                    )
                    .await?;
                return Ok(serde_json::from_value(value)?);
            }
            Ok(meta)
        })
    }

    /// Every event of one session, from now on, across reconnects.
    pub fn subscribe(&self, session_id: String) -> mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = mpsc::unbounded();
        let first = {
            let mut subs = self.shared.subs.lock().unwrap();
            let sub = subs.entry(session_id.clone()).or_insert_with(|| Sub {
                senders: Vec::new(),
                last_seq: 0,
            });
            sub.senders.push(tx);
            sub.senders.len() == 1
        };
        if first {
            let shared = self.shared.clone();
            runtime().spawn(async move {
                let _ = shared.subscribe(&session_id, false).await;
            });
        }
        rx
    }

    /// mode: `queue`, or `steer` to inject into the running turn.
    pub fn prompt(
        &self,
        session_id: &str,
        turn_id: &str,
        text: String,
        mode: &str,
    ) -> impl Future<Output = Result<()>> + Send + 'static {
        let call = self.call(
            "session.prompt",
            json!({ "session_id": session_id, "turn_id": turn_id, "text": text, "mode": mode }),
        );
        async move { call.await.map(|_| ()) }
    }

    /// Fire and forget; failures are logged.
    fn send(&self, method: &'static str, params: Value) {
        let call = self.call(method, params);
        runtime().spawn(async move {
            if let Err(err) = call.await {
                eprintln!("codeany: {method} failed: {err:#}");
            }
        });
    }

    pub fn abort(&self, session_id: &str) {
        self.send("session.abort", json!({ "session_id": session_id }));
    }

    pub fn approve(&self, session_id: String, request_id: String, decision: String) {
        self.send(
            "session.approve",
            json!({ "session_id": session_id, "request_id": request_id, "decision": decision }),
        );
    }

    /// Change access, model or effort; agl keeps the conversation.
    pub fn configure(&self, session_id: &str, access: Option<String>, model: Option<String>, effort: Option<String>) {
        let mut params = json!({ "session_id": session_id });
        for (key, value) in [("access", access), ("model", model), ("effort", effort)] {
            if let Some(value) = value {
                params[key] = Value::String(value);
            }
        }
        self.send("session.configure", params);
    }

    pub fn close(&self, session_id: String) {
        self.shared.subs.lock().unwrap().remove(&session_id);
        self.send("session.close", json!({ "session_id": session_id }));
    }
}

impl Shared {
    fn label(&self) -> String {
        match &self.machine {
            None => "the agent service".into(),
            Some(device) => format!("CodeAny on {}", crate::remote::name_of(device)),
        }
    }

    async fn run(self: Arc<Self>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            match open(&self.machine, self.service).await {
                Ok(ws) => {
                    backoff = Duration::from_secs(1);
                    self.serve(ws).await;
                    self.state.send_replace(ConnState::Connecting);
                }
                Err(err) => {
                    let message = format!("{err:#}");
                    // Threads are polled for every device; only agents are worth a log line.
                    if self.service == "agl" {
                        eprintln!("codeany: cannot reach {}: {message}", self.label());
                    }
                    self.state.send_replace(ConnState::Failed(message));
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    async fn serve(self: &Arc<Self>, ws: crate::remote::Ws) {
        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        *self.out.lock().unwrap() = Some(out_tx);
        self.state.send_replace(ConnState::Connected);

        // Sessions followed before the drop: subscribe again, catch up.
        let followed: Vec<String> = self.subs.lock().unwrap().keys().cloned().collect();
        for session_id in followed {
            let shared = self.clone();
            tokio::spawn(async move {
                let _ = shared.subscribe(&session_id, true).await;
            });
        }

        let writer = async {
            while let Some(text) = out_rx.recv().await {
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        };
        let reader = async {
            while let Some(Ok(msg)) = stream.next().await {
                match msg {
                    Message::Text(text) => self.handle(&text),
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        };
        tokio::select! {
            _ = writer => {}
            _ = reader => {}
        }
        *self.out.lock().unwrap() = None;
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(anyhow!("lost the connection to {}", self.label())));
        }
    }

    fn handle(&self, text: &str) {
        match serde_json::from_str::<WireMsg>(text) {
            Ok(WireMsg::Response { id, result, error }) => {
                if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(match error {
                        Some(e) => Err(anyhow!(e)),
                        None => Ok(result.unwrap_or(Value::Null)),
                    });
                }
            }
            Ok(WireMsg::Event(envelope)) => self.deliver(envelope),
            _ => {}
        }
    }

    fn deliver(&self, envelope: EventEnvelope) {
        let mut subs = self.subs.lock().unwrap();
        let Some(sub) = subs.get_mut(&envelope.session_id) else { return };
        if envelope.seq <= sub.last_seq {
            return;
        }
        sub.last_seq = envelope.seq;
        sub.senders
            .retain(|tx| tx.unbounded_send(envelope.event.clone()).is_ok());
        if sub.senders.is_empty() {
            subs.remove(&envelope.session_id);
        }
    }

    /// Follow a session's events. `catch_up` replays what happened since the
    /// last event seen; otherwise only newer events count.
    async fn subscribe(&self, session_id: &str, catch_up: bool) -> Result<()> {
        self.call("subscribe", json!({ "session_id": session_id })).await?;
        let after = match self.subs.lock().unwrap().get(session_id) {
            Some(sub) => sub.last_seq,
            None => return Ok(()),
        };
        let params = if catch_up && after > 0 {
            json!({ "session_id": session_id, "after_seq": after })
        } else {
            json!({ "session_id": session_id, "limit": 1 })
        };
        let missed: Vec<EventEnvelope> =
            serde_json::from_value(self.call("session.history", params).await?)?;
        if catch_up && after > 0 {
            for envelope in missed {
                self.deliver(envelope);
            }
        } else if let Some(newest) = missed.last() {
            let mut subs = self.subs.lock().unwrap();
            if let Some(sub) = subs.get_mut(session_id) {
                sub.last_seq = sub.last_seq.max(newest.seq);
            }
        }
        Ok(())
    }

    async fn connected(&self) -> Result<tokio::sync::mpsc::UnboundedSender<String>> {
        let mut state = self.state.subscribe();
        let deadline = tokio::time::Instant::now() + CONNECT_WAIT;
        loop {
            if let Some(out) = self.out.lock().unwrap().clone() {
                return Ok(out);
            }
            let failed = match &*state.borrow_and_update() {
                ConnState::Failed(message) => Some(message.clone()),
                _ => None,
            };
            if tokio::time::timeout_at(deadline, state.changed()).await.is_err() {
                return Err(match failed {
                    Some(message) => anyhow!("cannot reach {}: {message}", self.label()),
                    None => anyhow!("timed out connecting to {}", self.label()),
                });
            }
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let out = self.connected().await?;
        let id = self.next.fetch_add(1, Ordering::SeqCst).to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let msg = WireMsg::Request {
            id: id.clone(),
            method: method.into(),
            params,
        };
        if out.send(msg.to_json()).is_err() {
            self.pending.lock().unwrap().remove(&id);
            bail!("lost the connection to {}", self.label());
        }
        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(anyhow!("lost the connection to {}", self.label())),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("{} did not answer `{method}`", self.label()))
            }
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use futures::executor::block_on;

    /// `CODEANY_LIVE_AGENT=codex cargo test live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_two_turns_with_approval() {
        let dir = std::env::temp_dir().join("codeany-agl-live");
        std::fs::create_dir_all(dir.join("work")).unwrap();
        std::fs::write(dir.join("work/a.txt"), "hello").unwrap();
        // SAFETY: the only test that runs agl.
        unsafe { std::env::set_var("AGL_HOME", dir.join("home")) };
        let agent = std::env::var("CODEANY_LIVE_AGENT").unwrap_or_else(|_| "codex".into());

        let agl = backend(None);
        let meta = block_on(agl.open_session(
            format!("live-{agent}"),
            SessionOptions {
                agent,
                cwd: Some(dir.join("work")),
                model: None,
                effort: None,
                access: "ask".into(),
                resume: None,
                extra_args: Vec::new(),
            },
        ))
        .unwrap();
        let mut events = agl.subscribe(meta.id.clone());
        for prompt in [
            "Run `ls` with your shell tool, then reply with exactly: DONE",
            "What file did you see? Reply with just the file name.",
        ] {
            let turn_id = uuid::Uuid::new_v4().to_string();
            block_on(agl.prompt(&meta.id, &turn_id, prompt.into(), "queue")).unwrap();
            block_on(async {
                while let Some(event) = events.next().await {
                    match event {
                        AgentEvent::TextDelta { text, .. } => print!("{text}"),
                        AgentEvent::PermissionRequest { request_id, .. } => {
                            agl.approve(meta.id.clone(), request_id, "allow".into())
                        }
                        AgentEvent::TurnEnded { .. } => break,
                        _ => {}
                    }
                }
            });
            println!("\n---");
        }
        shutdown();
    }
}
