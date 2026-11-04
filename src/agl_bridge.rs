//! Local coding agents through the embedded `agl` session manager.
//!
//! agl is async (tokio) while GPUI runs its own executor, so a tokio runtime
//! lives on background threads and owns the [`Manager`]. The UI calls the
//! cheap, non-blocking methods here and receives each session's events over a
//! `futures` channel.

use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

use agl::{
    agents,
    config::Config,
    protocol::{AgentEvent, AgentInfo},
    session::{ConfigureParams, CreateParams, Manager, SessionMeta},
};
use futures::channel::mpsc;
use tokio::sync::broadcast::error::RecvError;

const GC_INTERVAL: Duration = Duration::from_secs(30);

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
}

pub struct Agl {
    rt: tokio::runtime::Runtime,
    mgr: Arc<Manager>,
}

static AGL: OnceLock<Result<Agl, String>> = OnceLock::new();

/// Keep dscode's sessions, history and config in `~/.dscode/agl`, apart
/// from the user's own `~/.agl`. Must run before any other thread starts.
pub fn set_home() {
    if std::env::var_os("AGL_HOME").is_some() {
        return;
    }
    let dir = crate::paths::agl_home();
    // Sessions and history from before `~/.dscode` move along.
    if !dir.exists() {
        if let Some(old) = crate::paths::legacy().map(|l| l.join("agl")).filter(|p| p.is_dir()) {
            let _ = std::fs::create_dir_all(crate::paths::home());
            let _ = std::fs::rename(old, &dir);
        }
    }
    // SAFETY: called at the top of `main`, before any thread exists.
    unsafe { std::env::set_var("AGL_HOME", dir) };
}

/// The shared agent manager, started on first use.
pub fn get() -> Result<&'static Agl, String> {
    AGL.get_or_init(|| Agl::start().map_err(|err| format!("{err:#}")))
        .as_ref()
        .map_err(Clone::clone)
}

impl Agl {
    fn start() -> anyhow::Result<Agl> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("agl")
            .enable_all()
            .build()?;
        let mgr = Manager::new(Config::load()?);
        {
            let _guard = rt.enter();
            // Saved sessions come back suspended and resume on their next prompt.
            mgr.load();
            let gc = mgr.clone();
            rt.spawn(async move {
                loop {
                    tokio::time::sleep(GC_INTERVAL).await;
                    gc.gc(false).await;
                }
            });
        }
        Ok(Agl { rt, mgr })
    }

    /// Installed agents and what agl can do with each. Blocks.
    pub fn detect(&self) -> Vec<AgentInfo> {
        let cfg = self.mgr.cfg();
        self.rt.block_on(agents::detect(&cfg))
    }

    /// Models an agent offers, without starting a conversation. Blocks.
    pub fn list_models(&self, agent: &str) -> anyhow::Result<Vec<ModelInfo>> {
        let cfg = self.mgr.cfg();
        let value = self.rt.block_on(agents::list_models(&cfg, agent))?;
        Ok(value["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| serde_json::from_value(m.clone()).ok())
            .collect())
    }

    /// Open the session called `name`, creating it on first use. `resume`
    /// attaches a new session to an existing native conversation.
    pub fn open_session(&self, name: String, opts: SessionOptions) -> anyhow::Result<SessionMeta> {
        let _guard = self.rt.enter();
        let meta = self.mgr.create(CreateParams {
            agent: opts.agent,
            cwd: opts.cwd,
            model: opts.model.clone(),
            effort: opts.effort.clone(),
            access: Some(opts.access.clone()),
            name: Some(name),
            resume: opts.resume,
            ..Default::default()
        })?;
        // A reused session keeps its old settings; bring them up to date.
        if meta.access != opts.access || meta.model != opts.model || meta.effort != opts.effort {
            return self.configure(&meta.id, Some(opts.access), opts.model, opts.effort);
        }
        Ok(meta)
    }

    /// Every event of one session, from now on.
    pub fn subscribe(&self, session_id: String) -> mpsc::UnboundedReceiver<AgentEvent> {
        let mut bus = self.mgr.bus.subscribe();
        let (tx, rx) = mpsc::unbounded();
        self.rt.spawn(async move {
            loop {
                match bus.recv().await {
                    Ok(envelope) if envelope.session_id == session_id => {
                        if tx.unbounded_send(envelope.event).is_err() {
                            break;
                        }
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        });
        rx
    }

    /// mode: `queue`, or `steer` to inject into the running turn.
    pub fn prompt(&self, session_id: &str, text: String, mode: &str) -> anyhow::Result<()> {
        let _guard = self.rt.enter();
        self.mgr.prompt(session_id, text, Some(mode), None).map(|_| ())
    }

    pub fn abort(&self, session_id: &str) -> anyhow::Result<()> {
        self.mgr.abort(session_id)
    }

    pub fn approve(&self, session_id: String, request_id: String, decision: String) {
        let mgr = self.mgr.clone();
        self.rt.spawn(async move {
            if let Err(err) = mgr.approve(&session_id, &request_id, &decision).await {
                eprintln!("agl: approve failed: {err:#}");
            }
        });
    }

    /// Change access, model or effort; agl keeps the conversation.
    pub fn configure(
        &self,
        session_id: &str,
        access: Option<String>,
        model: Option<String>,
        effort: Option<String>,
    ) -> anyhow::Result<SessionMeta> {
        self.mgr.configure(
            session_id,
            ConfigureParams {
                access,
                model,
                effort,
                ..Default::default()
            },
        )
    }

    pub fn close(&self, session_id: String) {
        let mgr = self.mgr.clone();
        self.rt.spawn(async move {
            let _ = mgr.close(&session_id).await;
        });
    }

    /// Stop every agent process. Called when the app quits.
    pub fn shutdown(&self) {
        let mgr = self.mgr.clone();
        let _ = self.rt.block_on(async move {
            tokio::time::timeout(Duration::from_secs(5), mgr.shutdown_all()).await
        });
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use futures::{StreamExt as _, executor::block_on};

    /// `DSCODE_LIVE_AGENT=codex cargo test live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_two_turns_with_approval() {
        let dir = std::env::temp_dir().join("dscode-agl-live");
        std::fs::create_dir_all(dir.join("work")).unwrap();
        std::fs::write(dir.join("work/a.txt"), "hello").unwrap();
        // SAFETY: the only test that runs agl.
        unsafe { std::env::set_var("AGL_HOME", dir.join("home")) };
        let agent = std::env::var("DSCODE_LIVE_AGENT").unwrap_or_else(|_| "codex".into());

        let agl = get().unwrap();
        let meta = agl
            .open_session(
                format!("live-{agent}"),
                SessionOptions {
                    agent,
                    cwd: Some(dir.join("work")),
                    model: None,
                    effort: None,
                    access: "ask".into(),
                    resume: None,
                },
            )
            .unwrap();
        let mut events = agl.subscribe(meta.id.clone());
        for prompt in [
            "Run `ls` with your shell tool, then reply with exactly: DONE",
            "What file did you see? Reply with just the file name.",
        ] {
            agl.prompt(&meta.id, prompt.into(), "queue").unwrap();
            block_on(async {
                while let Some(event) = events.next().await {
                    match &event {
                        AgentEvent::TextDelta { .. } | AgentEvent::Reasoning { .. } | AgentEvent::Heartbeat { .. } => {}
                        other => println!("{other:?}"),
                    }
                    match event {
                        AgentEvent::TextDelta { text } => print!("{text}"),
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
        agl.shutdown();
    }
}


