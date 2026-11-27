//! What every main window shares: the threads and settings (`Store`), the
//! agl sessions driving turns, the messages waiting to be sent, and the
//! account's other devices. One per app; each window's `AgentPanel` is a
//! view of it, showing a workspace of its own.

use std::{
    cell::{Ref, RefCell, RefMut},
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
};

use agl::protocol::{AgentEvent, AgentInfo};
use futures::StreamExt as _;
use gpui_kit::*;

use crate::{
    agl_bridge::{self, SessionOptions},
    git,
    i18n::{t, tf},
    remote,
    threads::{ChatItem, Store, Thread, UserMessage, delete_thread_files},
};

/// How often the account's other machines are looked up while signed in.
const HOSTS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// A thread's live agl session, for its current agent.
pub(crate) struct Session {
    pub id: String,
    pub agent: String,
    /// The nex device the session runs on; `None` is this machine.
    pub machine: Option<String>,
    pub features: Vec<String>,
    _events: Task<()>,
}

pub(crate) enum PromptState {
    Waiting,
    Sending,
    Accepted { queued: bool },
    Failed(String),
}

pub(crate) struct PendingPrompt {
    pub turn_id: String,
    pub message: UserMessage,
    pub state: PromptState,
}

impl PendingPrompt {
    pub fn new(message: UserMessage) -> Self {
        Self { turn_id: uuid::Uuid::new_v4().to_string(), message, state: PromptState::Waiting }
    }

    pub fn is_submitted(&self) -> bool {
        matches!(self.state, PromptState::Sending | PromptState::Accepted { .. })
    }
}

/// What a window shows, for the others to know.
#[derive(Clone)]
pub(crate) struct View {
    pub window: AnyWindowHandle,
    pub panel: WeakEntity<crate::agent_panel::AgentPanel>,
    pub workspace: Option<String>,
    pub active: Option<u64>,
    /// The order windows opened in, to open them again in it.
    order: u64,
}

impl View {
    pub fn new(window: AnyWindowHandle, panel: WeakEntity<crate::agent_panel::AgentPanel>, workspace: Option<String>) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let order = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { window, panel, workspace, active: None, order }
    }
}

/// Set as CodeAny quits: the windows closing then are the ones to reopen.
static QUITTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn quitting() {
    QUITTING.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) struct Shared {
    pub store: Store,
    pub sessions: HashMap<u64, Session>,
    pub running: HashSet<u64>,
    /// Only the first pending prompt can be submitted to agl. The others
    /// wait here so they can be removed or explicitly steered.
    pub pending_prompts: HashMap<u64, VecDeque<PendingPrompt>>,
    /// Threads that finished a turn while no window showed them, until opened.
    pub unread: HashSet<u64>,
    /// A transient note on the "Working…" line: retries, stalls.
    pub notes: HashMap<u64, String>,
    /// The account's other machines that share their agents.
    pub hosts: Vec<remote::Device>,
    /// Detected agents, by machine.
    pub agents_cache: HashMap<Option<String>, Result<Vec<AgentInfo>, String>>,
    detecting: HashSet<Option<String>>,
    /// Each window's agent panel and what it shows.
    pub views: HashMap<EntityId, View>,
}

impl Shared {
    pub fn thread(&self, id: u64) -> Option<&Thread> {
        self.store.threads.iter().find(|t| t.id == id)
    }

    pub fn thread_mut(&mut self, id: u64) -> Option<&mut Thread> {
        self.store.threads.iter_mut().find(|t| t.id == id)
    }

    /// The thread's live agl session for its current agent, if open.
    pub fn live_agl_session(&self, id: u64) -> Option<&Session> {
        let thread = self.thread(id)?;
        self.sessions.get(&id).filter(|s| s.agent == thread.agent && s.machine == thread.machine)
    }

    pub fn can_steer(&self, id: u64) -> bool {
        self.running.contains(&id) && self.live_agl_session(id).is_some_and(|s| s.features.iter().any(|f| f == "steer"))
    }

    pub fn is_busy(&self, id: u64) -> bool {
        self.running.contains(&id)
            || self.pending_prompts.get(&id).and_then(VecDeque::front).is_some_and(PendingPrompt::is_submitted)
    }

    /// Whether some window shows thread `id`.
    pub fn is_shown(&self, id: u64) -> bool {
        self.views.values().any(|v| v.active == Some(id))
    }

    /// The window showing workspace `id`, other than panel `except`'s.
    pub fn view_of(&self, workspace: &str, except: EntityId) -> Option<View> {
        self.views
            .iter()
            .find(|(panel, v)| **panel != except && v.workspace.as_deref() == Some(workspace))
            .map(|(_, v)| v.clone())
    }

    /// Whether `machine` can run a conversation now: its connection is up,
    /// or the device list says it is reachable.
    pub fn machine_online(&self, machine: &str) -> bool {
        agl_bridge::is_connected(machine) || self.hosts.iter().any(|h| h.id == machine && h.usable())
    }

    /// A machine's name: as the relay lists it, else as it was when a
    /// thread started there.
    pub fn machine_name(&self, id: &str, fallback: Option<&str>) -> String {
        self.hosts
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.name.clone())
            .or_else(|| fallback.map(str::to_string))
            .or_else(|| Some(remote::name_of(id)).filter(|name| name != id))
            .unwrap_or_else(|| "another machine".into())
    }

    /// Forget the panels of windows that closed (an editor window can keep
    /// its main window's panel alive).
    pub fn prune(&mut self, cx: &App) {
        let alive: Vec<WindowId> = cx.windows().iter().map(|w| w.window_id()).collect();
        let before = self.views.len();
        self.views.retain(|_, v| alive.contains(&v.window.window_id()));
        if self.views.len() != before {
            self.save_windows();
        }
    }

    /// Remember which workspaces the windows show, to open them next time.
    pub fn save_windows(&mut self) {
        if QUITTING.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let mut views: Vec<&View> = self.views.values().collect();
        views.sort_by_key(|v| v.order);
        let mut windows: Vec<String> = Vec::new();
        for id in views.into_iter().filter_map(|v| v.workspace.clone()) {
            if !windows.contains(&id) {
                windows.push(id);
            }
        }
        if windows != self.store.windows {
            self.store.windows = windows;
            self.store.save();
        }
    }

    /// The workspaces the windows of the last run showed, still here.
    pub fn windows_to_restore(&self) -> Vec<String> {
        self.store.windows.iter().filter(|id| self.store.workspaces.iter().any(|w| &w.id == *id)).cloned().collect()
    }

    /// A workspace no window shows, the one used last first.
    pub fn free_workspace(&self) -> Option<String> {
        let free = |id: &String| !self.views.values().any(|v| v.workspace.as_ref() == Some(id));
        self.store
            .active_workspace
            .clone()
            .filter(free)
            .or_else(|| self.store.workspaces.iter().map(|w| w.id.clone()).find(free))
    }

    fn take_pending_prompt(&mut self, id: u64, turn_id: &str) -> Option<PendingPrompt> {
        let prompts = self.pending_prompts.get_mut(&id)?;
        prompts.front().filter(|prompt| prompt.turn_id == turn_id)?;
        let pending = prompts.pop_front();
        if prompts.is_empty() {
            self.pending_prompts.remove(&id);
        }
        pending
    }

    pub fn pending_prompt_mut(&mut self, id: u64, turn_id: &str) -> Option<&mut PendingPrompt> {
        self.pending_prompts.get_mut(&id)?.front_mut().filter(|p| p.turn_id == turn_id)
    }
}

pub enum HubEvent {
    /// A turn ended.
    TurnFinished,
    /// Thread `id` is gone.
    ThreadRemoved(u64),
    /// Thread `id` grew (a reply streams in, another device wrote it).
    ThreadChanged(u64),
    /// The agents of a machine were detected.
    AgentsDetected(Option<String>),
    /// The agents turned on in Settings changed.
    AgentsChanged,
    /// A workspace was deleted.
    WorkspacesChanged,
}

impl Hub {
    /// Tell every window.
    pub fn announce(&mut self, event: HubEvent, cx: &mut Context<Self>) {
        cx.emit(event);
        cx.notify();
    }
}

pub struct Hub {
    shared: Rc<RefCell<Shared>>,
}

impl EventEmitter<HubEvent> for Hub {}

struct GlobalHub(Entity<Hub>);
impl Global for GlobalHub {}

/// The app's hub, made (and the sync started) on first use.
pub fn hub(cx: &mut App) -> Entity<Hub> {
    if let Some(hub) = cx.try_global::<GlobalHub>() {
        return hub.0.clone();
    }
    let hub = cx.new(Hub::new);
    cx.set_global(GlobalHub(hub.clone()));
    hub
}

/// The biggest file sent along with a message to another device.
const MAX_UPLOAD: u64 = 25 * 1024 * 1024;

/// Copy attachments to `machine`; the paths of the copies there.
async fn upload_attachments(machine: &str, paths: &[std::path::PathBuf]) -> Result<Vec<std::path::PathBuf>, String> {
    use base64::Engine as _;
    let mut uploaded = Vec::with_capacity(paths.len());
    for path in paths {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let too_big = std::fs::metadata(path).map(|m| m.len() > MAX_UPLOAD).unwrap_or(false);
        if too_big {
            return Err(tf("{name} is too large to send to another device (25 MB at most)", &[("name", name)]));
        }
        let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        let reply = agl_bridge::backend_for(Some(machine), crate::sync::SERVICE)
            .call("files.put", serde_json::json!({ "name": name, "data": data }))
            .await
            .map_err(|err| format!("{}: {err:#}", t("Could not send the attachment to the other device")))?;
        let remote = reply["path"].as_str().ok_or_else(|| t("Could not send the attachment to the other device").to_string())?;
        uploaded.push(std::path::PathBuf::from(remote));
    }
    Ok(uploaded)
}

pub fn delete_pasted_images(images: &[std::path::PathBuf]) {
    let dir = crate::paths::images();
    for path in images {
        if path.starts_with(&dir) {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Hub {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut store = Store::load();
        // Everything from before workspaces lands in one.
        store.ensure_workspaces(&remote::this_name());
        store.machine = store.active_workspace().and_then(|w| w.device.clone());
        // Tabs of threads deleted or settled since are gone.
        let live: HashSet<u64> = store.threads.iter().filter(|t| !t.settled).map(|t| t.id).collect();
        for set in &mut store.tabs {
            set.retain_threads(|id| live.contains(&id));
        }
        // And tabs that strayed into another workspace's row go too.
        store.drop_foreign_tabs();

        // Threads written here by other devices, and theirs to mirror.
        let mut inbox = crate::sync::start(&store.threads);
        cx.spawn(async move |this, cx| {
            while let Some(incoming) = inbox.next().await {
                if this.update(cx, |this, cx| this.apply_incoming(incoming, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // The account's other machines come online and go.
        cx.spawn(async move |this, cx| {
            loop {
                let hosts = remote::devices().await.ok();
                let alive = this.update(cx, |this, cx| {
                    if let Some(hosts) = hosts {
                        this.set_hosts(hosts, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(HOSTS_INTERVAL).await;
            }
        })
        .detach();

        Self {
            shared: Rc::new(RefCell::new(Shared {
                store,
                sessions: HashMap::new(),
                running: HashSet::new(),
                pending_prompts: HashMap::new(),
                unread: HashSet::new(),
                notes: HashMap::new(),
                hosts: Vec::new(),
                agents_cache: HashMap::new(),
                detecting: HashSet::new(),
                views: HashMap::new(),
            })),
        }
    }

    /// The state the windows read; borrow it briefly, and never across a
    /// call that may change it.
    pub(crate) fn shared(&self) -> Rc<RefCell<Shared>> {
        self.shared.clone()
    }

    fn s(&self) -> Ref<'_, Shared> {
        self.shared.borrow()
    }

    fn sm(&self) -> RefMut<'_, Shared> {
        self.shared.borrow_mut()
    }

    // ---- Machines -----------------------------------------------------------

    fn set_hosts(&mut self, hosts: Vec<remote::Device>, cx: &mut Context<Self>) {
        let hosts: Vec<_> = hosts.into_iter().filter(|d| !d.is_self && d.shares).collect();
        let ids = |list: &[remote::Device]| list.iter().map(|h| (h.id.clone(), h.name.clone(), h.usable())).collect::<Vec<_>>();
        if ids(&hosts) == ids(&self.s().hosts) {
            return;
        }
        // A machine that came back may list its agents now.
        let retry: Vec<String> = {
            let s = self.s();
            hosts
                .iter()
                .filter(|h| h.usable() && matches!(s.agents_cache.get(&Some(h.id.clone())), Some(Err(_))))
                .map(|h| h.id.clone())
                .collect()
        };
        self.sm().hosts = hosts;
        for machine in retry {
            self.sm().agents_cache.remove(&Some(machine.clone()));
            self.detect_agents(Some(machine), cx);
        }
        cx.notify();
    }

    /// Look the account's machines up now, e.g. right after signing in.
    pub fn refresh_hosts(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let hosts = remote::devices().await.unwrap_or_default();
            this.update(cx, |this, cx| this.set_hosts(hosts, cx)).ok();
        })
        .detach();
    }

    /// Detect `machine`'s agents (again); `AgentsDetected` tells when done.
    pub fn detect_agents(&mut self, machine: Option<String>, cx: &mut Context<Self>) {
        if !self.sm().detecting.insert(machine.clone()) {
            return;
        }
        let detect = agl_bridge::backend(machine.as_deref()).detect();
        cx.spawn(async move |this, cx| {
            let agents = detect.await.map_err(|err| format!("{err:#}"));
            this.update(cx, |this, cx| {
                {
                    let mut s = this.sm();
                    s.detecting.remove(&machine);
                    s.agents_cache.insert(machine.clone(), agents);
                }
                cx.emit(HubEvent::AgentsDetected(machine));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- Turns --------------------------------------------------------------

    /// Queue `message` on thread `id` and send it when the agent can take it.
    pub fn enqueue(&mut self, id: u64, message: UserMessage, cx: &mut Context<Self>) {
        self.sm().pending_prompts.entry(id).or_default().push_back(PendingPrompt::new(message));
        self.dispatch_prompt(id, false, cx);
        self.s().store.save();
    }

    /// Open the thread's agl session and subscribe to it; the session id.
    fn open_session(&mut self, id: u64, cx: &mut Context<Self>) -> Task<Result<String, String>> {
        let s = self.s();
        let Some(thread) = s.thread(id) else {
            return Task::ready(Err("thread is gone".into()));
        };
        let machine = thread.machine.clone();
        let agent = thread.agent.clone();
        let name = thread.session_name();
        // Another device's thread with no project works in a folder of its own
        // there, as one here does; a device too old to make one uses its home.
        let remote_dir = (thread.machine.is_some() && thread.project.is_none()).then(|| {
            let backend = agl_bridge::backend_for(thread.machine.as_deref(), "codeany");
            async move { backend.call("threads.dir", serde_json::json!({ "id": id })).await }
        });
        let mut opts = SessionOptions {
            agent: thread.agent.clone(),
            cwd: thread.cwd(),
            model: thread.model.clone(),
            effort: thread.effort.clone(),
            access: thread.access.clone(),
            resume: thread.native_ids.get(&thread.agent).cloned(),
            // Only this machine's agents take the arguments set here.
            extra_args: if thread.machine.is_none() {
                s.store.agent_args.get(&thread.agent).cloned().unwrap_or_default()
            } else {
                Vec::new()
            },
        };
        let worktree = thread.worktree.clone();
        let branch = thread.start_branch.clone();
        let project = thread.project.clone();
        drop(s);
        cx.spawn(async move |this, cx| {
            if let Some(project) = project.filter(|_| worktree.is_some() || branch.is_some()) {
                if let Some(machine) = machine.as_deref() {
                    let checkout = agl_bridge::backend_for(Some(machine), crate::sync::SERVICE)
                        .call("git.prepare", serde_json::json!({
                            "id": id, "path": project, "worktree": worktree, "branch": branch,
                        }))
                        .await
                        .map_err(|err| format!("{err:#}"))?;
                    let cwd = checkout["cwd"].as_str().ok_or("The device did not return a checkout path")?;
                    opts.cwd = Some(cwd.into());
                } else {
                    let worktree = worktree.clone();
                    let branch = branch.clone();
                    let project = project.clone();
                    cx.background_executor()
                        .spawn(async move {
                            if let Some(worktree) = worktree {
                                worktree.prepare(branch.as_deref())?;
                            } else if let Some(branch) = branch {
                                git::switch_branch(&project, &branch)?;
                            }
                            Ok::<_, String>(())
                        })
                        .await?;
                }
                let exists = this.update(cx, |this, cx| {
                    let mut state = this.sm();
                    let Some(thread) = state.thread_mut(id) else { return false };
                    if machine.is_some()
                        && let Some(worktree) = &mut thread.worktree
                        && worktree.checkout_dir != opts.cwd
                    {
                        worktree.checkout_dir.clone_from(&opts.cwd);
                        state.store.save();
                    }
                    cx.notify();
                    true
                }).map_err(|_| "the app is closing".to_string())?;
                if !exists {
                    // Deletion can arrive while checkout is still in flight.
                    if let Some(worktree) = worktree {
                        if let Some(machine) = machine.as_deref() {
                            crate::sync::deleted(machine, id, Some((project, worktree)));
                        } else {
                            cx.background_executor().spawn(async move { worktree.cleanup() }).detach();
                        }
                    }
                    return Err("thread is gone".into());
                }
            }
            if let Some(dir) = remote_dir {
                if let Ok(reply) = dir.await {
                    opts.cwd = reply["path"].as_str().map(std::path::PathBuf::from);
                }
            }
            let meta = agl_bridge::backend(machine.as_deref())
                .open_session(name, opts)
                .await
                .map_err(|err| format!("{err:#}"))?;
            this.update(cx, |this, cx| {
                if this.s().thread(id).is_none() {
                    agl_bridge::backend(machine.as_deref()).close(meta.id);
                    return Err("thread is gone".into());
                }
                let mut events = agl_bridge::backend(machine.as_deref()).subscribe(meta.id.clone());
                let task = cx.spawn(async move |this, cx| {
                    while let Some(event) = events.next().await {
                        if this.update(cx, |this, cx| this.apply_event(id, event, cx)).is_err() {
                            break;
                        }
                    }
                });
                this.sm().sessions.insert(
                    id,
                    Session { id: meta.id.clone(), agent, machine, features: meta.features, _events: task },
                );
                Ok(meta.id)
            })
            .map_err(|_| "the app is closing".to_string())?
        })
    }

    pub fn dispatch_prompt(&mut self, id: u64, steer: bool, cx: &mut Context<Self>) {
        let mut s = self.sm();
        let running = s.running.contains(&id);
        if running && !(steer && s.can_steer(id)) {
            return;
        }
        let context = match s.thread(id) {
            Some(thread) if !running => thread.handoff_context().unwrap_or_default(),
            Some(_) => String::new(),
            None => return,
        };
        let machine = s.thread(id).and_then(|t| t.machine.clone());
        let session = s.live_agl_session(id).map(|s| s.id.clone());
        // A new session here learns about the other devices and the command
        // that reaches them.
        let briefing = if machine.is_none() && session.is_none() && !running {
            let names: Vec<String> = s.hosts.iter().map(|h| h.name.clone()).collect();
            crate::device_cli::briefing(id, &names).unwrap_or_default()
        } else {
            String::new()
        };
        let Some(pending) = s.pending_prompts.get_mut(&id).and_then(VecDeque::front_mut) else {
            return;
        };
        if !matches!(pending.state, PromptState::Waiting) {
            return;
        }
        let turn_id = pending.turn_id.clone();
        let mut message = pending.message.clone();
        let prefix = briefing + &context;
        pending.state = PromptState::Sending;
        drop(s);
        let open = session.map_or_else(|| self.open_session(id, cx), |session| Task::ready(Ok(session)));
        let mode = if running { "steer" } else { "queue" };
        cx.spawn(async move |this, cx| {
            // An agent on another device cannot open files from this one:
            // copy them over first and point it at the copies.
            let uploaded = match machine.as_deref() {
                Some(machine) if !message.images.is_empty() => {
                    upload_attachments(machine, &message.images).await.map(|paths| message.images = paths)
                }
                _ => Ok(()),
            };
            let result = match (uploaded, open.await) {
                (Err(err), _) | (_, Err(err)) => Err(err),
                (Ok(()), Ok(session_id)) => agl_bridge::backend(machine.as_deref())
                    .prompt(&session_id, &turn_id, prefix + &message.prompt(), mode)
                    .await
                    .map_err(|err| format!("{err:#}")),
            };
            if let Err(err) = result {
                this.update(cx, |this, cx| {
                    if let Some(pending) = this.sm().pending_prompt_mut(id, &turn_id)
                        && matches!(pending.state, PromptState::Sending)
                    {
                        pending.state = PromptState::Failed(err);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    pub fn remove_pending_prompt(&mut self, id: u64, turn_id: &str, cx: &mut Context<Self>) {
        {
            let mut s = self.sm();
            let Some(prompts) = s.pending_prompts.get_mut(&id) else { return };
            let Some(index) = prompts.iter().position(|p| p.turn_id == turn_id && !p.is_submitted()) else { return };
            if let Some(prompt) = prompts.remove(index) {
                delete_pasted_images(&prompt.message.images);
            }
            if prompts.is_empty() {
                s.pending_prompts.remove(&id);
            }
        }
        self.dispatch_prompt(id, false, cx);
        cx.notify();
    }

    pub fn retry_pending_prompt(&mut self, id: u64, turn_id: &str, cx: &mut Context<Self>) {
        let retry = match self.sm().pending_prompt_mut(id, turn_id) {
            Some(prompt) if matches!(prompt.state, PromptState::Failed(_)) => {
                prompt.state = PromptState::Waiting;
                true
            }
            _ => false,
        };
        if retry {
            self.dispatch_prompt(id, false, cx);
            cx.notify();
        }
    }

    pub fn stop(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(session) = self.s().sessions.get(&id) {
            agl_bridge::backend(session.machine.as_deref()).abort(&session.id);
        }
        cx.notify();
    }

    pub fn answer(&mut self, id: u64, request_id: String, decision: &str, cx: &mut Context<Self>) {
        if let Some(session) = self.s().sessions.get(&id) {
            agl_bridge::backend(session.machine.as_deref()).approve(session.id.clone(), request_id, decision.to_string());
        }
        cx.notify();
    }

    /// Drop a thread; `propagate` also deletes it on the device it runs on.
    pub fn remove_thread(&mut self, id: u64, propagate: bool, cx: &mut Context<Self>) {
        let mut s = self.sm();
        if let Some(session) = s.sessions.remove(&id) {
            agl_bridge::backend(session.machine.as_deref()).close(session.id);
        }
        s.running.remove(&id);
        if let Some(prompts) = s.pending_prompts.remove(&id) {
            for prompt in prompts {
                delete_pasted_images(&prompt.message.images);
            }
        }
        if let Some(thread) = s.thread(id) {
            if let Some(machine) = thread.machine.as_deref().filter(|_| propagate) {
                crate::sync::deleted(machine, id, thread.project.clone().zip(thread.worktree.clone()));
            }
            delete_thread_files(thread);
            // Another machine's worktree is that machine's to remove.
            if let Some(worktree) = thread.worktree.clone().filter(|_| thread.machine.is_none()) {
                cx.background_executor()
                    .spawn(async move { worktree.cleanup() })
                    .detach();
            }
        }
        s.store.threads.retain(|t| t.id != id);
        for set in &mut s.store.tabs {
            set.retain_threads(|t| t != id);
        }
        s.unread.remove(&id);
        s.store.save();
        drop(s);
        cx.emit(HubEvent::ThreadRemoved(id));
        cx.notify();
    }

    // ---- What arrives -------------------------------------------------------

    /// A thread another device wrote, deleted or has newer.
    fn apply_incoming(&mut self, incoming: crate::sync::Incoming, cx: &mut Context<Self>) {
        use crate::sync::Incoming;
        match incoming {
            Incoming::Put(thread) | Incoming::Mirror(thread) => {
                let id = thread.id;
                let mut s = self.sm();
                // A turn under way here owns the thread until it ends.
                if s.is_busy(id) {
                    return;
                }
                let shown = s.is_shown(id);
                match s.thread_mut(id) {
                    // Another thread that happens to share the id: leave it.
                    Some(existing) if existing.machine != thread.machine => return,
                    Some(existing) => {
                        let grew = thread.items.len() > existing.items.len();
                        *existing = thread;
                        if grew && !shown {
                            s.unread.insert(id);
                        }
                    }
                    None => s.store.threads.push(thread),
                }
                // A device seen for the first time gets a workspace to show it in.
                s.store.ensure_workspaces(&remote::this_name());
                s.store.save();
                drop(s);
                cx.emit(HubEvent::ThreadChanged(id));
                cx.notify();
            }
            Incoming::PutWorkspace(ws) => {
                let mut s = self.sm();
                match s.store.workspaces.iter_mut().find(|w| w.id == ws.id) {
                    Some(existing) => *existing = ws,
                    None => s.store.workspaces.push(ws),
                }
                s.store.save();
                drop(s);
                cx.notify();
            }
            Incoming::Workspaces { machine, list } => {
                let mut s = self.sm();
                let mut changed = false;
                for ws in s.store.workspaces.iter_mut().filter(|w| w.device.as_deref() == Some(machine.as_str())) {
                    if let Some(theirs) = list.iter().find(|r| r.id == ws.id) {
                        if (&ws.title, &ws.icon, &ws.projects) != (&theirs.title, &theirs.icon, &theirs.projects) {
                            ws.title = theirs.title.clone();
                            ws.icon = theirs.icon.clone();
                            ws.projects = theirs.projects.clone();
                            changed = true;
                        }
                    }
                }
                if changed {
                    s.store.save();
                    drop(s);
                    cx.notify();
                }
            }
            Incoming::Delete(id) => {
                let here = self.s().store.threads.iter().any(|t| t.id == id && t.machine.is_none());
                if here {
                    self.remove_thread(id, false, cx);
                }
            }
            Incoming::Resend { machine, listed } => {
                let missing: Vec<Thread> = self
                    .s()
                    .store
                    .threads
                    .iter()
                    .filter(|t| t.machine.as_deref() == Some(machine.as_str()) && !listed.contains(&t.id))
                    .cloned()
                    .collect();
                crate::sync::resend(&missing);
            }
            Incoming::Gone { machine, id } => {
                let theirs = self.s().store.threads.iter().any(|t| t.id == id && t.machine.as_deref() == Some(machine.as_str()));
                if theirs {
                    self.remove_thread(id, false, cx);
                }
            }
        }
    }

    fn apply_event(&mut self, id: u64, event: AgentEvent, cx: &mut Context<Self>) {
        let mut s = self.sm();
        let mut sent = None;
        let mut idle = false;
        match &event {
            AgentEvent::Status { status } => {
                // starting, running and waiting (on an approval) are all busy.
                idle = matches!(status.as_str(), "idle" | "suspended" | "closed");
                if idle {
                    if s.running.remove(&id) && !s.is_busy(id) && !s.is_shown(id) {
                        s.unread.insert(id);
                    }
                    if status == "closed" {
                        s.sessions.remove(&id);
                    }
                } else {
                    s.running.insert(id);
                }
            }
            AgentEvent::UserMessage { turn_id, .. } | AgentEvent::Queued { turn_id, .. } => {
                let queued = matches!(event, AgentEvent::Queued { .. });
                if let Some(pending) = s.pending_prompt_mut(id, turn_id) {
                    pending.state = PromptState::Accepted { queued };
                }
            }
            AgentEvent::TurnStarted { turn_id } => {
                s.running.insert(id);
                sent = s.take_pending_prompt(id, turn_id);
            }
            AgentEvent::Steered { turn_id, .. } => {
                sent = s.take_pending_prompt(id, turn_id);
            }
            AgentEvent::TurnEnded { turn_id, error, .. } => {
                if let Some(pending) = s.pending_prompt_mut(id, turn_id) {
                    // The agent could not launch this turn. Keep the message
                    // and its attachments available for retry.
                    pending.state = PromptState::Failed(error.clone().unwrap_or_else(|| t("The turn did not start").into()));
                    drop(s);
                    cx.notify();
                    return;
                }
            }
            AgentEvent::Retrying { message } => {
                s.notes.insert(id, tf("Retrying… {message}", &[("message", message.clone())]));
            }
            AgentEvent::Heartbeat { stalled: true, idle_ms, .. } => {
                s.notes.insert(id, tf("No output for {n}s", &[("n", (idle_ms / 1000).to_string())]));
            }
            AgentEvent::TextDelta { .. } | AgentEvent::ToolStart { .. } => {
                s.notes.remove(&id);
            }
            _ => {}
        }
        let turn_ended = matches!(event, AgentEvent::TurnEnded { .. });

        let Some(thread) = s.thread_mut(id) else {
            return;
        };
        let sent_message = sent.is_some();
        if let Some(pending) = sent {
            if thread.worktree.is_none() {
                thread.start_branch = None;
            }
            thread.items.push(ChatItem::User(pending.message));
        }
        thread.apply(event);
        if sent_message || turn_ended {
            s.store.save();
        }
        if turn_ended {
            s.notes.remove(&id);
        }
        drop(s);
        if turn_ended {
            cx.emit(HubEvent::TurnFinished);
        }
        if idle {
            self.dispatch_prompt(id, false, cx);
        }
        cx.emit(HubEvent::ThreadChanged(id));
        cx.notify();
    }
}
