//! Agent mode: a list of threads and a conversation with a coding agent.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use agl::protocol::{AgentEvent, AgentInfo};
use futures::StreamExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{InputEvent, Textarea, TextareaState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
    popover::Popover,
    Disableable as _,
    shimmer::ShimmerText,
    spinner::Spinner,
    text::TextView,
    tooltip::Tooltip,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{
    external::{self, ExternalSession},
    agent_icons::{agent_color, agent_icon},
    agl_bridge::{self, SessionOptions},
    model_picker::{ModelPicker, ModelPickerEvent, model_display_name},
    threads::{
        ACCESS_LEVELS, ChatItem, Store, Thread, ToolStatus, access_name, agent_name,
        clock_time, delete_thread_images, relative_time,
    },
};
use crate::style::{
    layout,
    size::{HEADING, ICON, ICON_LG, ICON_SM, TEXT, TEXT_LG, TEXT_SM},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarSection {
    Projects,
    Recent,
}

const EXTERNAL_SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(8);
/// Threads listed per project before "Show more".
const THREADS_PER_PROJECT: usize = 8;

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

pub enum AgentPanelEvent {
    OpenFile(PathBuf),
    SwitchProject(Option<PathBuf>),
    OpenFolder,
    TurnFinished,
}

/// A thread's live agl session, for its current agent.
struct Session {
    id: String,
    agent: String,
    features: Vec<String>,
    _events: Task<()>,
}

pub struct AgentPanel {
    store: Store,
    project: Option<PathBuf>,
    active: Option<u64>,
    agents: Option<Result<Vec<AgentInfo>, String>>,
    /// The agent, model, effort and access level for the next new thread.
    agent: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    access: String,
    picker: Entity<ModelPicker>,
    picker_open: bool,
    projects_expanded: bool,
    recent_expanded: bool,
    sort_repositories_by_name: bool,
    navigation_history: Vec<u64>,
    navigation_index: usize,
    all_projects: bool,
    expanded_projects: HashSet<PathBuf>,
    /// Projects showing all their threads, not only the latest few.
    long_projects: HashSet<Option<PathBuf>>,
    /// Claude Code and Codex sessions started in terminals.
    external: Vec<ExternalSession>,
    sessions: HashMap<u64, Session>,
    running: HashSet<u64>,
    /// A transient note on the "Working…" line: retries, stalls.
    notes: HashMap<u64, String>,
    expanded: HashSet<(u64, usize)>,
    /// Tool groups the user opened (true) or closed (false), by first item.
    groups: HashMap<(u64, usize), bool>,
    /// "Worked for …" folds the user opened, by the turn's first work item.
    worked: HashSet<(u64, usize)>,
    /// The answer last copied, to show a check.
    copied: Option<(u64, usize)>,
    /// Keeps a just-opened thread at the bottom while markdown lays out.
    _pin_bottom: Option<Task<()>>,
    composer: Entity<TextareaState>,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AgentPanelEvent> for AgentPanel {}

impl AgentPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 12)
                .submit_on_enter(true)
                .placeholder("Ask anything, or describe a change to make")
        });
        let picker = cx.new(|cx| ModelPicker::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                        // Re-render so the send button follows whether there is text.
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    }
                },
            ),
            cx.subscribe_in(&picker, window, Self::on_picker_event),
            // Model names and efforts arrive as the picker loads them.
            cx.observe(&picker, |_, _, cx| cx.notify()),
        ];

        // Detecting agents starts agl and probes every CLI's version.
        cx.spawn(async move |this, cx| {
            let agents = cx
                .background_executor()
                .spawn(async { agl_bridge::get().map(|agl| agl.detect()) })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(agents) = &agents {
                    let installed = |id: &str| agents.iter().any(|a| a.id == id && a.available);
                    if this.agent.as_deref().is_none_or(|id| !installed(id)) {
                        this.agent = agents.iter().find(|a| a.available).map(|a| a.id.clone());
                        this.model = None;
                    }
                    let favorites = this.store.favorites.clone();
                    let agent = this.agent.clone();
                    this.picker.update(cx, |picker, cx| {
                        picker.set_agents(agents, cx);
                        picker.set_favorites(favorites, cx);
                        // Load the default agent's models for its label and efforts.
                        if let Some(agent) = agent {
                            picker.ensure_models(&agent, cx);
                        }
                    });
                }
                this.agents = Some(agents);
                cx.notify();
            })
            .ok();
        })
        .detach();

        // Terminal sessions come and go; look again every few seconds.
        cx.spawn(async move |this, cx| {
            loop {
                let sessions = cx.background_executor().spawn(async { external::scan() }).await;
                let alive = this.update(cx, |this, cx| {
                    if this.external != sessions {
                        this.external = sessions;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(EXTERNAL_SCAN_INTERVAL).await;
            }
        })
        .detach();

        let store = Store::load();
        if let Some(theme) = store.theme.clone() {
            crate::style::set_choice(theme, window, cx);
        }
        Self {
            agent: store.agent.clone(),
            model: store.model.clone(),
            effort: store.effort.clone(),
            access: store.access.clone(),
            picker,
            picker_open: false,
            projects_expanded: true,
            recent_expanded: true,
            sort_repositories_by_name: false,
            navigation_history: Vec::new(),
            navigation_index: 0,
            all_projects: false,
            expanded_projects: HashSet::new(),
            long_projects: HashSet::new(),
            external: Vec::new(),
            store,
            project: None,
            active: None,
            agents: None,
            sessions: HashMap::new(),
            running: HashSet::new(),
            notes: HashMap::new(),
            expanded: HashSet::new(),
            groups: HashMap::new(),
            worked: HashSet::new(),
            copied: None,
            _pin_bottom: None,
            composer,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_project(&mut self, project: Option<PathBuf>, cx: &mut Context<Self>) {
        if let Some(project) = &project {
            // Start with the current repository's conversations visible.
            if self.project.is_none() && self.expanded_projects.is_empty() {
                self.expanded_projects.insert(project.clone());
            }
            self.store.projects.retain(|p| p != project);
            self.store.projects.insert(0, project.clone());
            self.store.save();
        }
        if self.project == project {
            return;
        }
        self.project = project;
        // Keep the open thread only if it belongs to the new project.
        if self.active_thread().is_some_and(|t| t.project != self.project) {
            self.active = None;
        }
        cx.notify();
    }

    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }

    fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        self.composer_focus_handle(cx).focus(window, cx);
    }

    pub fn new_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active = None;
        self.focus_composer(window, cx);
        cx.notify();
    }

    pub fn active_title(&self) -> Option<String> {
        self.active_thread().map(|t| t.title.clone())
    }

    fn active_thread(&self) -> Option<&Thread> {
        let id = self.active?;
        self.store.threads.iter().find(|t| t.id == id)
    }

    fn thread_mut(&mut self, id: u64) -> Option<&mut Thread> {
        self.store.threads.iter_mut().find(|t| t.id == id)
    }

    fn agent_display_name(&self, id: &str) -> String {
        match &self.agents {
            Some(Ok(agents)) => agents
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.name.clone())
                .unwrap_or_else(|| agent_name(id)),
            _ => agent_name(id),
        }
    }

    /// Every thread, for the search palette.
    pub fn threads(&self) -> &[Thread] {
        &self.store.threads
    }

    /// Projects opened, most recent first.
    pub fn recent_projects(&self) -> &[PathBuf] {
        &self.store.projects
    }

    pub fn select_thread(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.store.threads.iter().find(|t| t.id == id) else {
            return;
        };
        if self.navigation_history.get(self.navigation_index) != Some(&id) {
            if !self.navigation_history.is_empty() {
                self.navigation_history.truncate(self.navigation_index + 1);
            }
            self.navigation_history.push(id);
            self.navigation_index = self.navigation_history.len() - 1;
        }
        let project = thread.project.clone();
        let follow = thread.synced.zip(thread.external.clone());
        self.active = Some(id);
        if project != self.project {
            cx.emit(AgentPanelEvent::SwitchProject(project.clone()));
            self.project = project;
        }
        // Still following a terminal session: pick up what happened there.
        if let Some((synced, path)) = follow {
            if let Some(session) = self.external.iter().find(|e| e.path == path).cloned() {
                if external::modified(&path) > synced {
                    self.import_transcript(id, session, cx);
                }
            }
        }
        self.pin_to_bottom(cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    pub fn can_navigate(&self, forward: bool) -> bool {
        if forward {
            self.navigation_index + 1 < self.navigation_history.len()
        } else {
            self.navigation_index > 0
        }
    }

    pub fn navigate(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_navigate(forward) { return; }
        if forward { self.navigation_index += 1; } else { self.navigation_index -= 1; }
        let id = self.navigation_history[self.navigation_index];
        self.select_thread(id, window, cx);
    }

    /// Scroll to the end, and keep there for a moment: markdown lays out
    /// asynchronously and grows the transcript after the first frame.
    fn pin_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.scroll.scroll_to_bottom();
        self._pin_bottom = Some(cx.spawn(async move |this, cx| {
            for _ in 0..15 {
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                if this.update(cx, |this, _| this.scroll.scroll_to_bottom()).is_err() {
                    break;
                }
            }
        }));
    }

    /// The dscode thread a terminal session was imported into.
    fn imported_thread(&self, session: &ExternalSession) -> Option<u64> {
        self.store
            .threads
            .iter()
            .find(|t| t.native_ids.get(&session.agent) == Some(&session.id))
            .map(|t| t.id)
    }

    /// The terminal session a thread continues, while it runs in a terminal.
    fn live_session(&self, thread: &Thread) -> Option<&ExternalSession> {
        let id = thread.native_ids.get(&thread.agent)?;
        self.external
            .iter()
            .find(|e| e.live.is_some() && e.agent == thread.agent && &e.id == id)
    }

    /// Open a terminal session as a thread: import its transcript once, then
    /// resume it through agl when the user replies.
    fn open_external(&mut self, session: ExternalSession, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.imported_thread(&session) {
            self.select_thread(id, window, cx);
            return;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let project = (Some(&session.cwd) != home.as_ref()).then(|| session.cwd.clone());
        let mut thread = Thread::new(project, session.agent.clone(), self.access.clone());
        thread.title = session.title.clone();
        thread.native_ids.insert(session.agent.clone(), session.id.clone());
        thread.external = Some(session.path.clone());
        thread.updated_at = session.updated;
        let id = thread.id;
        self.store.threads.insert(0, thread);
        self.import_transcript(id, session, cx);
        self.select_thread(id, window, cx);
    }

    /// Read a terminal transcript in the background into the thread.
    fn import_transcript(&mut self, id: u64, session: ExternalSession, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let path = session.path.clone();
            let (items, modified) = cx
                .background_executor()
                .spawn(async move { (external::transcript(&session), external::modified(&session.path)) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(thread) = this.thread_mut(id) {
                    let agent = thread.agent.clone();
                    thread.items = items;
                    thread.seen.insert(agent, thread.items.len());
                    thread.external = Some(path);
                    thread.synced = Some(modified);
                }
                this.store.save();
                this.pin_to_bottom(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn delete_thread(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(session) = self.sessions.remove(&id) {
            if let Ok(agl) = agl_bridge::get() {
                agl.close(session.id);
            }
        }
        self.running.remove(&id);
        if let Some(thread) = self.store.threads.iter().find(|t| t.id == id) {
            delete_thread_images(thread);
        }
        self.store.threads.retain(|t| t.id != id);
        let current = self.navigation_history.get(self.navigation_index).copied();
        self.navigation_history.retain(|visited| *visited != id);
        self.navigation_index = current.and_then(|current| self.navigation_history.iter().position(|visited| *visited == current))
            .unwrap_or_else(|| self.navigation_history.len().saturating_sub(1));
        if self.active == Some(id) {
            self.active = None;
        }
        self.store.save();
        cx.notify();
    }

    /// The thread's agl session, opened and subscribed on first use.
    fn ensure_session(&mut self, id: u64, cx: &mut Context<Self>) -> Result<String, String> {
        let thread = self
            .store
            .threads
            .iter()
            .find(|t| t.id == id)
            .ok_or("thread is gone")?;
        if let Some(session) = self.sessions.get(&id) {
            if session.agent == thread.agent {
                return Ok(session.id.clone());
            }
        }
        let agl = agl_bridge::get()?;
        let meta = agl
            .open_session(
                thread.session_name(),
                SessionOptions {
                    agent: thread.agent.clone(),
                    cwd: thread.project.clone(),
                    model: thread.model.clone(),
                    effort: thread.effort.clone(),
                    access: thread.access.clone(),
                    resume: thread.native_ids.get(&thread.agent).cloned(),
                },
            )
            .map_err(|err| format!("{err:#}"))?;
        let agent = thread.agent.clone();

        let mut events = agl.subscribe(meta.id.clone());
        let task = cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this
                    .update(cx, |this, cx| this.apply_event(id, event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.sessions.insert(
            id,
            Session {
                id: meta.id.clone(),
                agent,
                features: meta.features,
                _events: task,
            },
        );
        Ok(meta.id)
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }

        let id = match self.active {
            Some(id) => id,
            None => {
                let Some(agent) = self.agent.clone() else {
                    return;
                };
                let mut thread = Thread::new(self.project.clone(), agent, self.access.clone());
                thread.model = self.model.clone();
                thread.effort = self.effort.clone();
                thread.title = text.lines().next().unwrap_or("").chars().take(60).collect();
                let id = thread.id;
                self.store.threads.insert(0, thread);
                self.active = Some(id);
                id
            }
        };

        let running = self.running.contains(&id);
        // An agent new to the thread first reads what it missed.
        let context = self
            .store
            .threads
            .iter()
            .find(|t| t.id == id)
            .filter(|_| !running)
            .and_then(Thread::handoff_context)
            .unwrap_or_default();
        if let Some(thread) = self.thread_mut(id) {
            thread.items.push(ChatItem::User(text.clone()));
            // Continued here: the terminal's later writes are another branch.
            thread.synced = None;
        }
        let result = self.ensure_session(id, cx).and_then(|session_id| {
            // While a turn runs, steer it where the agent supports that; else queue.
            let steer = running && self.sessions[&id].features.iter().any(|f| f == "steer");
            let agl = agl_bridge::get()?;
            agl.prompt(&session_id, context + &text, if steer { "steer" } else { "queue" })
                .map_err(|err| format!("{err:#}"))
        });
        if let Err(err) = result {
            if let Some(thread) = self.thread_mut(id) {
                thread.items.push(ChatItem::Error(err));
            }
        }

        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.scroll.scroll_to_bottom();
        self.store.save();
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active else { return };
        if let (Some(session), Ok(agl)) = (self.sessions.get(&id), agl_bridge::get()) {
            let _ = agl.abort(&session.id);
        }
        cx.notify();
    }

    fn answer(&mut self, thread_id: u64, request_id: String, decision: &str, cx: &mut Context<Self>) {
        if let (Some(session), Ok(agl)) = (self.sessions.get(&thread_id), agl_bridge::get()) {
            agl.approve(session.id.clone(), request_id, decision.to_string());
        }
        cx.notify();
    }

    fn apply_event(&mut self, id: u64, event: AgentEvent, cx: &mut Context<Self>) {
        match &event {
            AgentEvent::Status { status } => {
                // starting, running and waiting (on an approval) are all busy.
                if matches!(status.as_str(), "idle" | "suspended" | "closed") {
                    self.running.remove(&id);
                } else {
                    self.running.insert(id);
                }
            }
            AgentEvent::TurnStarted { .. } => {
                self.running.insert(id);
            }
            AgentEvent::Retrying { message } => {
                self.notes.insert(id, format!("Retrying… {message}"));
            }
            AgentEvent::Heartbeat { stalled: true, idle_ms, .. } => {
                self.notes
                    .insert(id, format!("No output for {}s", idle_ms / 1000));
            }
            AgentEvent::TextDelta { .. } | AgentEvent::ToolStart { .. } => {
                self.notes.remove(&id);
            }
            _ => {}
        }
        let turn_ended = matches!(event, AgentEvent::TurnEnded { .. });

        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        thread.apply(event);
        if turn_ended {
            self.notes.remove(&id);
            self.store.save();
            cx.emit(AgentPanelEvent::TurnFinished);
        }
        if self.active == Some(id) && self.is_near_bottom() {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn is_near_bottom(&self) -> bool {
        let max = self.scroll.max_offset().y;
        -self.scroll.offset().y >= max - px(120.)
    }

    fn on_picker_event(
        &mut self,
        _: &Entity<ModelPicker>,
        event: &ModelPickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ModelPickerEvent::Select { agent, model } => {
                self.choose_model(agent.clone(), model.clone(), cx);
                self.picker_open = false;
                self.focus_composer(window, cx);
            }
            ModelPickerEvent::ToggleFavorite { agent, model } => {
                let key = (agent.clone(), model.clone());
                if let Some(ix) = self.store.favorites.iter().position(|f| *f == key) {
                    self.store.favorites.remove(ix);
                } else {
                    self.store.favorites.push(key);
                }
                let favorites = self.store.favorites.clone();
                self.picker
                    .update(cx, |picker, cx| picker.set_favorites(favorites, cx));
                self.store.save();
            }
        }
        cx.notify();
    }

    /// A model's display name, e.g. "Claude Sonnet 5.5"; `None` is the agent's default.
    fn model_label(&self, agent: &str, model: Option<&str>, cx: &App) -> String {
        let models = self.picker.read(cx).models(agent).unwrap_or_default();
        let info = match model {
            Some(id) => models.iter().find(|m| m.id == id),
            None => models.iter().find(|m| m.default).or_else(|| models.first()),
        };
        match (info, model) {
            (Some(info), _) => model_display_name(agent, info),
            (None, Some(id)) => id.to_string(),
            (None, None) => self.agent_display_name(agent),
        }
    }

    /// The efforts the agent's model accepts.
    fn model_efforts(&self, agent: &str, model: Option<&str>, cx: &App) -> Vec<String> {
        let models = self.picker.read(cx).models(agent).unwrap_or_default();
        match model {
            Some(id) => models.iter().find(|m| m.id == id),
            None => models.iter().find(|m| m.default).or_else(|| models.first()),
        }
        .map(|m| m.efforts.clone())
        .unwrap_or_default()
    }

    /// Pick the agent and model: for the open thread (a handoff when the
    /// agent changes), or for the next new one.
    fn choose_model(&mut self, agent: String, model: Option<String>, cx: &mut Context<Self>) {
        // The default model is the one with no explicit choice.
        let model = model.filter(|m| m != "default");
        let active = self.active.filter(|id| {
            self.store.threads.iter().any(|t| t.id == *id && !t.items.is_empty())
        });
        if let Some(id) = active {
            let Some(thread) = self.store.threads.iter().find(|t| t.id == id) else {
                return;
            };
            let from_label = self.model_label(&thread.agent, thread.model.as_deref(), cx);
            let to_label = self.model_label(&agent, model.as_deref(), cx);
            let same_agent = thread.agent == agent;
            if let Some(thread) = self.thread_mut(id) {
                thread.switch(agent.clone(), model.clone(), from_label, to_label);
            }
            // Same agent: change the model in place; agl keeps the conversation.
            if same_agent {
                if let (Some(session), Ok(agl)) = (self.sessions.get(&id), agl_bridge::get()) {
                    let _ = agl.configure(&session.id, None, Some(model.clone().unwrap_or_else(|| "default".into())), None);
                }
            }
            self.scroll.scroll_to_bottom();
        }
        self.agent = Some(agent.clone());
        self.model = model.clone();
        self.store.agent = Some(agent.clone());
        self.store.model = model;
        self.picker.update(cx, |picker, cx| picker.ensure_models(&agent, cx));
        self.store.save();
        cx.notify();
    }

    fn set_effort(&mut self, effort: Option<String>, cx: &mut Context<Self>) {
        self.effort = effort.clone();
        self.store.effort = effort.clone();
        if let Some(id) = self.active {
            if let Some(thread) = self.thread_mut(id) {
                thread.effort = effort.clone();
            }
            if let (Some(session), Ok(agl)) = (self.sessions.get(&id), agl_bridge::get()) {
                let _ = agl.configure(&session.id, None, None, effort);
            }
        }
        self.store.save();
        cx.notify();
    }

    fn set_picker_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = open;
        if open {
            let (agent, model) = match self.active_thread() {
                Some(thread) => (thread.agent.clone(), thread.model.clone()),
                None => (self.agent.clone().unwrap_or_default(), self.model.clone()),
            };
            self.picker
                .update(cx, |picker, cx| picker.open(agent, model, window, cx));
        } else {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    fn set_access(&mut self, access: &str, cx: &mut Context<Self>) {
        self.access = access.to_string();
        self.store.access = access.to_string();
        // The open thread changes too; agl keeps the conversation.
        if let Some(id) = self.active {
            if let Some(thread) = self.thread_mut(id) {
                thread.access = access.to_string();
            }
            if let (Some(session), Ok(agl)) = (self.sessions.get(&id), agl_bridge::get()) {
                let _ = agl.configure(&session.id, Some(access.to_string()), None, None);
            }
        }
        self.store.save();
        cx.notify();
    }

    /// Copy and the time, under a finished turn's answer.
    fn render_turn_actions(&self, thread_id: u64, items: &[ChatItem], turn: &Turn, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let answer = turn.visible.iter().rev().find_map(|&ix| match &items[ix] {
            ChatItem::Assistant(text) => Some((ix, text.clone())),
            _ => None,
        });
        let at = turn.work.iter().chain(&turn.visible).find_map(|&ix| match items[ix] {
            ChatItem::TurnEnd { at, .. } => at,
            _ => None,
        });
        let Some((ix, text)) = answer else {
            return div().into_any_element();
        };
        let copied = self.copied == Some((thread_id, ix));
        div()
            .mt(layout::SPACE_1)
            .flex()
            .items_center()
            .gap(layout::SPACE_2)
            .text_size(TEXT_SM)
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .id(("copy-answer", ix))
                    .size(layout::CONTROL)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|this| this.bg(theme.list_hover).text_color(theme.foreground))
                    .child(Icon::new(if copied { IconName::Check } else { IconName::Copy }).size(ICON))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                        this.copied = Some((thread_id, ix));
                        cx.notify();
                    })),
            )
            .when_some(at, |this, at| this.child(clock_time(at)))
            .into_any_element()
    }

    fn toggle_group(&mut self, key: (u64, usize), open: bool, cx: &mut Context<Self>) {
        self.groups.insert(key, !open);
        cx.notify();
    }

    /// Items laid out with tool runs grouped. `open_groups` opens groups by
    /// default (inside a fold); `live` opens only the newest one.
    fn render_blocks(
        &self,
        thread_id: u64,
        items: &[ChatItem],
        ixs: &[usize],
        open_groups: bool,
        at_top: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let blocks = group_items(items, ixs);
        let last = blocks.len().saturating_sub(1);
        let is_step_block = |block: &Block| match block {
            Block::Group(_) => true,
            Block::Single(ix) => is_step(&items[*ix]),
        };
        let mut previous_step: Option<bool> = None;
        let mut out = Vec::new();
        for (n, block) in blocks.into_iter().enumerate() {
            let step = is_step_block(&block);
            let gap = match previous_step {
                None if at_top => px(0.),
                None => layout::SPACE_3,
                Some(prev) if prev && step => px(2.),
                Some(_) => layout::SPACE_4,
            };
            previous_step = Some(step);
            let element = match block {
                Block::Single(ix) => self.render_item(thread_id, ix, &items[ix], cx),
                Block::Group(ixs) => {
                    let key = (thread_id, ixs[0]);
                    let default_open = open_groups || n == last;
                    let open = self.groups.get(&key).copied().unwrap_or(default_open);
                    self.render_group(thread_id, items, ixs, open, cx)
                }
            };
            out.push(div().mt(gap).child(element).into_any_element());
        }
        out
    }

    /// A run of tool calls folded into one line, Codex-style.
    fn render_group(
        &self,
        thread_id: u64,
        items: &[ChatItem],
        ixs: Vec<usize>,
        open: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let (summary, failed, icon) = summarize_group(items, &ixs);
        let running = ixs.iter().any(|&ix| {
            matches!(&items[ix], ChatItem::Tool { status: ToolStatus::Running, .. })
        });
        let key = (thread_id, ixs[0]);
        let children: Vec<AnyElement> = ixs
            .iter()
            .filter(|&&ix| !is_hidden(&items[ix]))
            .map(|&ix| self.render_item(thread_id, ix, &items[ix], cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(("tool-group", ixs[0]))
                    .h(layout::CONTROL)
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_2)
                    .text_size(TEXT)
                    .text_color(theme.foreground)
                    .cursor_pointer()
                    .child(Icon::new(icon).size(ICON).text_color(theme.muted_foreground))
                    .child(summary)
                    .when(failed > 0, |this| {
                        this.child(
                            div()
                                .text_color(theme.danger)
                                .child(format!("· {failed} failed")),
                        )
                    })
                    .when(running, |this| this.child(Spinner::new().with_size(ICON)))
                    .child(
                        Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                            .size(ICON_SM)
                            .text_color(theme.muted_foreground.opacity(0.6)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(key, open, cx))),
            )
            .when(open, |this| {
                this.child(div().flex().flex_col().gap(px(2.)).children(children))
            })
            .into_any_element()
    }

    fn toggle_expanded(&mut self, key: (u64, usize), cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        cx.notify();
    }

    // ---- Sidebar ------------------------------------------------------------

    fn set_settled(&mut self, id: u64, settled: bool, cx: &mut Context<Self>) {
        if let Some(thread) = self.thread_mut(id) {
            thread.settled = settled;
        }
        if settled && self.active == Some(id) {
            self.active = None;
        }
        self.store.save();
        cx.notify();
    }

    // ---- Settings -------------------------------------------------------------

    /// Installed and known agents, once detected.
    pub fn agents(&self) -> Option<&[AgentInfo]> {
        match &self.agents {
            Some(Ok(agents)) => Some(agents),
            _ => None,
        }
    }

    pub fn default_agent(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    pub fn default_access(&self) -> &str {
        &self.access
    }

    /// The agent new threads start with; open threads keep theirs.
    pub fn set_default_agent(&mut self, agent: String, cx: &mut Context<Self>) {
        self.agent = Some(agent.clone());
        self.model = None;
        self.store.agent = Some(agent.clone());
        self.store.model = None;
        self.picker.update(cx, |picker, cx| picker.ensure_models(&agent, cx));
        self.store.save();
        cx.notify();
    }

    pub fn set_default_access(&mut self, access: &str, cx: &mut Context<Self>) {
        self.access = access.to_string();
        self.store.access = access.to_string();
        self.store.save();
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: String, window: &mut Window, cx: &mut Context<Self>) {
        self.store.theme = Some(theme.clone());
        self.store.save();
        crate::style::set_choice(theme, window, cx);
        cx.notify();
    }

    /// Enter a project: switch to it, show its threads, start a new one there.
    fn open_project(&mut self, project: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = &project {
            if !self.expanded_projects.insert(path.clone()) {
                self.expanded_projects.remove(path);
            }
        }
        if project != self.project {
            cx.emit(AgentPanelEvent::SwitchProject(project.clone()));
            self.project = project;
        }
        self.new_thread(window, cx);
    }

    /// A repository conversation: status, title, and a quiet trailing timestamp.
    fn render_thread_row(&self, thread: &Thread, indent: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = thread.id;
        let active = self.active == Some(id);
        let running = self.running.contains(&id);
        let settled = thread.settled;
        div()
            .id((if indent { "project-thread" } else { "thread" }, id))
            .group("thread")
            .relative()
            .h(layout::SIDEBAR_ROW)
            .flex_none()
            .px(layout::SIDEBAR_ROW_INSET)
            .flex()
            .items_center()
            .gap(layout::SPACE_2)
            .rounded(layout::RADIUS)
            .cursor_pointer()
            .text_size(TEXT)
            .when(active, |this| this.bg(theme.list_active))
            .when(!active, |this| this.hover(|this| this.bg(theme.list_hover)))
            .child(self.render_entry_status(running, self.live_session(thread).is_some(), cx))
            .child(
                div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap()
                    .text_color(if active { theme.foreground } else { theme.foreground.opacity(0.8) })
                    .child(thread.title.clone()),
            )
            .child(
                div().flex_none().min_w(px(30.)).text_right().text_size(TEXT_SM)
                    .text_color(theme.muted_foreground).child(relative_time(thread.updated_at)),
            )
            .tooltip({
                let agents = thread.agents_used().iter().map(|id| agent_name(id)).collect::<Vec<_>>().join(", ");
                let label = format!("{} · {} · {}", thread.title, agents, relative_time(thread.updated_at));
                move |window, cx| Tooltip::new(label.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| this.select_thread(id, window, cx)))
            .context_menu({
                let panel = cx.entity().downgrade();
                move |menu, _, _| {
                    let (settle, delete) = (panel.clone(), panel.clone());
                    menu.item(
                        PopupMenuItem::new(if settled { "Reopen" } else { "Settle" }).on_click(move |_, _, cx| {
                            settle.update(cx, |this, cx| this.set_settled(id, !settled, cx)).ok();
                        }),
                    )
                    .separator()
                    .item(PopupMenuItem::new("Delete").on_click(move |_, _, cx| {
                        delete.update(cx, |this, cx| this.delete_thread(id, cx)).ok();
                    }))
                }
            })
            .into_any_element()
    }

    fn toggle_section(&mut self, section: SidebarSection, cx: &mut Context<Self>) {
        let expanded = match section {
            SidebarSection::Projects => &mut self.projects_expanded,
            SidebarSection::Recent => &mut self.recent_expanded,
        };
        *expanded = !*expanded;
        cx.notify();
    }

    fn render_section_header(&self, section: SidebarSection, action: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (title, expanded) = match section {
            SidebarSection::Projects => ("Repositories", self.projects_expanded),
            SidebarSection::Recent => ("Chats", self.recent_expanded),
        };
        div()
            .mt(layout::SPACE_4)
            .mb(layout::SPACE_2)
            .h(layout::SIDEBAR_ROW)
            .flex_none()
            .px(layout::SIDEBAR_ROW_INSET)
            .flex()
            .items_center()
            .child(
                div().id(title).flex_1().h_full().flex().items_center().gap(layout::SPACE_1)
                    .text_size(TEXT).text_color(theme.muted_foreground).cursor_pointer()
                    .child(title)
                    .when(!expanded, |this| this.child(Icon::new(IconName::ChevronRight).size(ICON_SM)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx))),
            )
            .when(section == SidebarSection::Projects, |this| {
                let panel = cx.entity().downgrade();
                let by_name = self.sort_repositories_by_name;
                this.child(
                    crate::style::chrome_button("repository-sort")
                        .child(Icon::new(IconName::ListFilter).size(ICON_LG))
                        .text_color(theme.muted_foreground)
                        .tooltip("Sort repositories")
                        .dropdown_menu(move |mut menu, _, _| {
                            for (label, value) in [("Recently active", false), ("Name", true)] {
                                let panel = panel.clone();
                                menu = menu.item(PopupMenuItem::new(label).checked(by_name == value)
                                    .on_click(move |_, _, cx| {
                                        panel.update(cx, |this, cx| { this.sort_repositories_by_name = value; cx.notify(); }).ok();
                                    }));
                            }
                            menu
                        }),
                )
            })
            .child(action)
            .into_any_element()
    }

    /// A terminal session not yet opened in dscode.
    fn render_external_row(&self, session: &ExternalSession, _indent: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let open = session.clone();
        div()
            .id(SharedString::from(format!("external-{}", session.id)))
            .h(layout::SIDEBAR_ROW).flex_none().px(layout::SIDEBAR_ROW_INSET)
            .flex().items_center().gap(layout::SPACE_2).rounded(layout::RADIUS)
            .cursor_pointer().text_size(TEXT).text_color(theme.foreground.opacity(0.8))
            .hover(|this| this.bg(theme.list_hover))
            .child(self.render_entry_status(session.live.as_deref() == Some("busy"), session.live.is_some(), cx))
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(session.title.clone()))
            .child(div().flex_none().min_w(px(30.)).text_right().text_size(TEXT_SM)
                .text_color(theme.muted_foreground).child(relative_time(session.updated)))
            .tooltip({
                let label = format!("{} · {} · {}", session.title, agent_name(&session.agent), session.cwd.display());
                move |window, cx| Tooltip::new(label.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| this.open_external(open.clone(), window, cx)))
            .into_any_element()
    }

    fn render_entry_status(&self, running: bool, live: bool, cx: &App) -> AnyElement {
        let theme = cx.theme();
        div().size(ICON_LG).flex_none().flex().items_center().justify_center()
            .child(if running {
                Spinner::new().with_size(ICON_SM).color(theme.muted_foreground).into_any_element()
            } else {
                div().size(px(6.)).rounded_full()
                    .bg(if live { theme.primary } else { theme.muted_foreground.opacity(0.35) }).into_any_element()
            })
            .into_any_element()
    }

    pub fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let home = std::env::var_os("HOME").map(PathBuf::from);

        // Threads and not-yet-opened terminal sessions, as one list per project.
        enum Entry<'a> {
            Thread(&'a Thread),
            External(&'a ExternalSession),
        }
        impl Entry<'_> {
            fn updated(&self) -> u64 {
                match self {
                    Entry::Thread(t) => t.updated_at,
                    Entry::External(e) => e.updated,
                }
            }
        }
        // Ordered, so equal timestamps never reshuffle from one frame to the next.
        let mut groups: std::collections::BTreeMap<Option<PathBuf>, Vec<Entry>> = Default::default();
        for thread in self.store.threads.iter().filter(|t| !t.settled) {
            groups.entry(thread.project.clone()).or_default().push(Entry::Thread(thread));
        }
        for session in &self.external {
            if self.imported_thread(session).is_some() {
                continue;
            }
            let project = (Some(&session.cwd) != home.as_ref()).then(|| session.cwd.clone());
            groups.entry(project).or_default().push(Entry::External(session));
        }
        for entries in groups.values_mut() {
            entries.sort_by(|a, b| {
                let key = |e: &Entry| match e {
                    Entry::Thread(t) => t.id.to_string(),
                    Entry::External(e) => e.id.clone(),
                };
                b.updated().cmp(&a.updated()).then_with(|| key(a).cmp(&key(b)))
            });
        }
        let icon_button = |id: &'static str, icon: IconName, tooltip: &'static str| {
            crate::style::chrome_button(id)
                .child(Icon::new(icon).size(ICON_LG))
                .text_color(theme.muted_foreground)
                .tooltip(tooltip)
        };

        // ---- Projects, most recently active first; the open one leads.
        let mut projects: Vec<PathBuf> = self.store.projects.clone();
        for project in groups.keys().flatten() {
            if !projects.contains(project) {
                projects.push(project.clone());
            }
        }
        let activity = |p: &PathBuf| {
            groups
                .get(&Some(p.clone()))
                .and_then(|entries| entries.first())
                .map(Entry::updated)
                .unwrap_or(0)
        };
        // The open project first, then by last activity. Many sessions share
        // a timestamp (the CLI touches them together), so the path breaks
        // ties: without it the list reorders on every hover.
        projects.sort_by(|a, b| {
            let rank = |p: &PathBuf| {
                (
                    self.project.as_ref() != Some(p),
                    std::cmp::Reverse(activity(p).max(u64::from(self.store.projects.contains(p)))),
                )
            };
            if self.sort_repositories_by_name {
                a.file_name().cmp(&b.file_name()).then_with(|| a.cmp(b))
            } else {
                rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
            }
        });
        let hidden_projects = if self.all_projects {
            0
        } else {
            projects.len().saturating_sub(5)
        };
        projects.truncate(projects.len() - hidden_projects);

        let mut list = div()
            .id("thread-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(layout::SIDEBAR_INSET)
            .pb(layout::SPACE_2)
            .flex()
            .flex_col();

        // The entries of one group, capped unless asked for all.
        let push_entries = |this: &Self,
                                list: Stateful<Div>,
                                key: Option<PathBuf>,
                                entries: &[Entry],
                                indent: bool,
                                cx: &mut Context<Self>|
         -> Stateful<Div> {
            let mut list = list;
            let all = this.long_projects.contains(&key);
            let shown = if all { entries.len() } else { entries.len().min(if indent { 3 } else { THREADS_PER_PROJECT }) };
            for entry in &entries[..shown] {
                list = list.child(match entry {
                    Entry::Thread(t) => this.render_thread_row(t, indent, cx),
                    Entry::External(e) => this.render_external_row(e, indent, cx),
                });
            }
            let more = entries.len() - shown;
            if more > 0 {
                list = list.child(
                    div()
                        .id(SharedString::from(format!("more-{key:?}")))
                        .h(layout::CONTROL)
                        .pl(if indent { layout::SIDEBAR_CHAT_INSET } else { layout::SIDEBAR_ROW_INSET })
                        .flex()
                        .items_center()
                        .text_size(TEXT)
                        .text_color(theme.muted_foreground)
                        .cursor_pointer()
                        .hover(|this| this.text_color(theme.foreground))
                        .child(format!("More ({more})"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.long_projects.insert(key.clone());
                            cx.notify();
                        })),
                );
            }
            list
        };

        {
            list = list.child(self.render_section_header(
                SidebarSection::Projects,
                icon_button("add-project", IconName::FolderPlus, "Open Folder (⌘O)")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(AgentPanelEvent::OpenFolder)))
                    .into_any_element(),
                cx,
            ));
            if self.projects_expanded {
                if projects.is_empty() {
                    list = list.child(
                        div()
                            .px(layout::SIDEBAR_ROW_INSET)
                            .py_1()
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground.opacity(0.7))
                            .child("Open a folder to start a project"),
                    );
                }
                for project in &projects {
                    let key = Some(project.clone());
                    let entries: &[Entry] = groups.get(&key).map(Vec::as_slice).unwrap_or(&[]);
                    let name: SharedString = project
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| project.display().to_string())
                        .into();
                    let expanded = self.expanded_projects.contains(project);
                    let current = self.project.as_ref() == Some(project);
                    let busy = entries.iter().any(|e| match e {
                        Entry::Thread(t) => self.running.contains(&t.id),
                        Entry::External(e) => e.live.as_deref() == Some("busy"),
                    });
                    let path = project.clone();
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("project-{}", project.display())))
                            .mt(layout::SPACE_2)
                            .h(layout::SIDEBAR_NAV_ROW)
                            .flex_none()
                            .pl(layout::SIDEBAR_ROW_INSET)
                            .pr_0()
                            .flex()
                            .items_center()
                            .gap(layout::SPACE_2)
                            .rounded(layout::RADIUS)
                            .cursor_pointer()
                            .text_size(TEXT)
                            .when(current && self.active.is_none(), |this| this.bg(theme.list_active))
                            .hover(|this| this.bg(theme.list_hover))
                            .child(
                                Icon::new(if expanded { IconName::FolderOpen } else { IconName::Folder })
                                    .size(ICON_LG)
                                    .flex_none()
                                    .text_color(theme.muted_foreground),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(name),
                            )
                            .when(busy, |this| {
                                this.child(div().size(px(6.)).rounded_full().bg(theme.primary))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_project(Some(path.clone()), window, cx)
                            })),
                    );
                    if expanded {
                        list = push_entries(self, list, key, entries, true, cx);
                    }
                }
                if hidden_projects > 0 {
                    list = list.child(
                        div()
                            .id("show-more-projects")
                            .h(layout::SIDEBAR_ROW)
                            .flex_none()
                            .px(layout::SIDEBAR_ROW_INSET)
                            .flex()
                            .items_center()
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground)
                            .cursor_pointer()
                            .hover(|this| this.text_color(theme.foreground))
                            .child(format!("More repositories ({hidden_projects})"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.all_projects = true;
                                cx.notify();
                            })),
                    );
                }
            }
        }

        // ---- Conversations without a repository
        if groups.get(&None).is_some_and(|entries| !entries.is_empty()) {
            list = list.child(self.render_section_header(
                SidebarSection::Recent,
                icon_button("new-chat", IconName::SquarePen, "New Chat")
                    .on_click(cx.listener(|this, _, window, cx| this.open_project(None, window, cx)))
                    .into_any_element(),
                cx,
            ));
            if self.recent_expanded {
                let entries = groups.get(&None).map(Vec::as_slice).unwrap_or(&[]);
                if entries.is_empty() {
                    list = list.child(
                        div()
                            .px(layout::SIDEBAR_ROW_INSET)
                            .py_1()
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground.opacity(0.7))
                            .child("No chats yet"),
                    );
                }
                list = push_entries(self, list, None, entries, false, cx);
            }
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(list)
            .into_any_element()
    }

    // ---- Conversation -------------------------------------------------------

    pub fn render_chat(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let thread = self.active_thread().cloned();

        let Some(thread) = thread.filter(|t| !t.items.is_empty()) else {
            return self.render_empty(cx);
        };

        let running = self.running.contains(&thread.id);
        let mut messages = div()
            .w_full()
            .min_w_0()
            .max_w(layout::CONTENT_MAX)
            .mx_auto()
            .px(layout::SPACE_6)
            .pt(layout::SPACE_6)
            .pb(layout::SPACE_6)
            .flex()
            .flex_col();
        let turns = split_turns(&thread.items);
        let last_turn = turns.len().saturating_sub(1);
        let mut first = true;
        for (n, turn) in turns.iter().enumerate() {
            if let Some(user) = turn.user {
                messages = messages.child(
                    div()
                        .when(!first, |this| this.mt(px(40.)))
                        .child(self.render_item(thread.id, user, &thread.items[user], cx)),
                );
                first = false;
            }
            let live = n == last_turn && running;
            let has_work = turn.work.iter().any(|&ix| !is_hidden(&thread.items[ix]));
            if live || !has_work {
                // In progress: everything in order, the newest steps open.
                let mut ixs: Vec<usize> = turn.work.iter().chain(&turn.visible).copied().collect();
                ixs.sort_unstable();
                messages = messages.children(self.render_blocks(thread.id, &thread.items, &ixs, live, first, cx));
            } else {
                // Done: the work folds under "Worked for …"; the answer stays.
                let key = (thread.id, turn.work[0]);
                let open = self.worked.contains(&key);
                messages = messages.child(
                    div()
                        .id(("worked", turn.work[0]))
                        .when(!first, |this| this.mt(px(16.)))
                        .pb(px(6.))
                        .flex()
                        .items_center()
                        .gap(layout::SPACE_2)
                        .border_b_1()
                        .border_color(theme.border)
                        .text_size(TEXT)
                        .text_color(theme.muted_foreground)
                        .cursor_pointer()
                        .hover(|this| this.text_color(theme.foreground))
                        .child(worked_for(turn.ms))
                        .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(ICON))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.worked.remove(&key) {
                                this.worked.insert(key);
                            }
                            cx.notify();
                        })),
                );
                if open {
                    messages = messages.children(self.render_blocks(thread.id, &thread.items, &turn.work, true, false, cx));
                }
                messages = messages.children(self.render_blocks(thread.id, &thread.items, &turn.visible, false, false, cx));
            }
            if turn.finished && !live {
                messages = messages.child(self.render_turn_actions(thread.id, &thread.items, turn, cx));
            }
            first = false;
        }
        if running {
            messages = messages.child(
                div()
                    .mt(px(16.))
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(
                        ShimmerText::new(
                            self.notes
                                .get(&thread.id)
                                .cloned()
                                .unwrap_or_else(|| "Working…".into()),
                        )
                        .id("working"),
                    ),
            );
        }

        div()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(theme.background)
            .child(
                div()
                    .id("messages")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_x_hidden()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(messages),
            )
            .child(
                div()
                    .w_full()
                    .max_w(layout::CONTENT_MAX)
                    .mx_auto()
                    .px(layout::SPACE_6)
                    .pb(layout::SPACE_4)
                    .flex_none()
                    .when_some(self.live_session(&thread).cloned(), |this, session| {
                        let busy = session.live.as_deref() == Some("busy");
                        this.child(
                            div()
                                .mb_2()
                                .px_3()
                                .py(px(7.))
                                .flex()
                                .items_center()
                                .gap(layout::SPACE_2)
                                .rounded(layout::RADIUS)
                                .bg(theme.secondary)
                                .text_size(TEXT_SM)
                                .text_color(theme.muted_foreground)
                                .child(Icon::new(IconName::SquareTerminal).size(ICON).flex_none())
                                .child(div().flex_1().child(format!(
                                    "{} is open in a terminal{}. Replies here continue it in dscode; the terminal won't see them.",
                                    agent_name(&session.agent),
                                    if busy { " and working" } else { "" }
                                ))),
                        )
                    })
                    .child(self.render_composer(Some(&thread), cx)),
            )
            .into_any_element()
    }

    fn render_empty(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let project_name: SharedString = self
            .project
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Choose a project".into())
            .into();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px_6()
            .pb(px(64.))
            .bg(theme.background)
            .child(
                div()
                    .text_size(HEADING)
                    .font_weight(FontWeight::MEDIUM)
                    .child("What should we work on?"),
            )
            .child(
                div()
                    .id("empty-project")
                    .mt_3()
                    .mb(px(28.))
                    .px(layout::ROW_INSET)
                    .py(px(4.))
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_2)
                    .rounded_full()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .cursor_pointer()
                    .hover(|this| this.bg(theme.list_hover).text_color(theme.foreground))
                    .child(Icon::new(IconName::Folder).size(ICON))
                    .child(project_name)
                    .child(Icon::new(IconName::ChevronDown).size(ICON_SM))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(AgentPanelEvent::OpenFolder))),
            )
            .child(
                div()
                    .w_full()
                    .max_w(layout::CONTENT_MAX)
                    .child(self.render_composer(None, cx)),
            )
            .into_any_element()
    }

    fn render_item(&self, thread_id: u64, ix: usize, item: &ChatItem, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let key = (thread_id, ix);
        let expanded = self.expanded.contains(&key);
        match item {
            ChatItem::User(text) => div()
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(relative(0.82))
                        .px(layout::SPACE_4)
                        .py(layout::SPACE_3)
                        .rounded(layout::BUBBLE_RADIUS)
                        .bg(theme.secondary)
                        .text_size(TEXT_LG)
                        .line_height(relative(1.6))
                        .child(text.clone()),
                )
                .into_any_element(),
            ChatItem::Assistant(text) => div()
                .flex()
                .justify_start()
                .child(
                    div()
                        .max_w(relative(0.92))
                        .px(layout::SPACE_4)
                        .py(layout::SPACE_3)
                        .rounded(layout::BUBBLE_RADIUS)
                        .bg(theme.muted)
                        .text_size(TEXT_LG)
                        .line_height(relative(1.6))
                        .child(TextView::markdown(("msg", ix), text.clone()).selectable(true)),
                )
                .into_any_element(),
            ChatItem::Thinking(text) => {
                let preview: String = text.lines().next().unwrap_or("").chars().take(120).collect();
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(("thinking", ix))
                            .h(layout::CONTROL)
                            .flex()
                            .items_center()
                            .gap(layout::SPACE_2)
                            .rounded_md()
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground)
                            .cursor_pointer()
                            .hover(|this| this.text_color(theme.foreground))
                            .child(Icon::new(IconName::Brain).size(ICON).flex_none())
                            .child(div().flex_none().child("Thought"))
                            .when(!expanded, |this| {
                                this.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_color(theme.muted_foreground.opacity(0.7))
                                        .child(preview),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(key, cx))),
                    )
                    .when(expanded, |this| {
                        this.child(
                            div()
                                .ml(px(7.))
                                .my_1()
                                .pl(layout::SPACE_4)
                                .border_l_1()
                                .border_color(theme.border)
                                .text_size(TEXT)
                                .line_height(relative(1.55))
                                .text_color(theme.muted_foreground)
                                .child(text.clone()),
                        )
                    })
                    .into_any_element()
            }
            ChatItem::Tool {
                title,
                detail,
                path,
                output,
                status,
                code,
                images,
                ..
            } => {
                let running = *status == ToolStatus::Running;
                let (icon, text) = tool_line(title, detail.as_deref(), running, self.project.as_deref());
                let has_output = output.as_ref().is_some_and(|o| !o.trim().is_empty())
                    || code.is_some()
                    || !images.is_empty();
                let path = path.clone();
                let generated = title == "Generate image";
                let images = images.clone();
                let shots = images.len();
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(("tool", ix))
                            .min_w_0()
                            .h(layout::CONTROL)
                            .flex()
                            .items_center()
                            .gap(layout::SPACE_2)
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground)
                            .when(has_output || path.is_some(), |this| {
                                this.cursor_pointer().hover(|this| this.text_color(theme.foreground))
                            })
                            .when(has_output, |this| {
                                this.on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(key, cx)))
                            })
                            .when(!has_output && path.is_some(), |this| {
                                let path = path.clone().unwrap();
                                this.on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(AgentPanelEvent::OpenFile(path.clone()))
                                }))
                            })
                            .child(Icon::new(icon).size(ICON).flex_none())
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(text),
                            )
                            .when(shots > 0 && !expanded && !generated, |this| {
                                this.child(
                                    Icon::new(IconName::Image)
                                        .size(ICON)
                                        .flex_none()
                                        .text_color(theme.muted_foreground.opacity(0.7)),
                                )
                            })
                            .when(running, |this| this.child(Spinner::new().with_size(ICON)))
                            .when(*status == ToolStatus::Failed, |this| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .text_size(TEXT_SM)
                                        .text_color(theme.danger)
                                        .child("failed"),
                                )
                            }),
                    )
                    .when(expanded && has_output, |this| {
                        this.child(
                            div()
                                .id(("tool-output", ix))
                                .ml(px(24.))
                                .my_1()
                                .max_h(px(260.))
                                .overflow_y_scroll()
                                .px_3()
                                .py_2()
                                .rounded(layout::RADIUS)
                                .bg(theme.secondary)
                                .font_family(theme.mono_font_family.clone())
                                .text_size(TEXT_SM)
                                .line_height(relative(1.5))
                                .text_color(theme.foreground.opacity(0.75))
                                .when_some(code.clone(), |this, code| {
                                    this.child(div().text_color(theme.foreground).child(code))
                                        .when(output.is_some(), |this| {
                                            this.child(div().my_2().h(px(1.)).bg(theme.border))
                                        })
                                })
                                .child(output.clone().unwrap_or_default()),
                        )
                    })
                    .when((expanded || generated) && !images.is_empty(), |this| {
                        this.child(
                            div()
                                .ml(px(24.))
                                .my_1()
                                .flex()
                                .flex_wrap()
                                .gap(layout::SPACE_2)
                                .children(images.into_iter().enumerate().map(|(n, path)| {
                                    let open_path = path.clone();
                                    div()
                                        .id(("tool-image", ix * 64 + n))
                                        .max_w(px(360.))
                                        .rounded(layout::RADIUS)
                                        .overflow_hidden()
                                        .border_1()
                                        .border_color(theme.border)
                                        .cursor_pointer()
                                        .child(
                                            img(path)
                                                .max_w(px(360.))
                                                .max_h(px(if generated { 360. } else { 220. })),
                                        )
                                        .on_click(move |_, _, cx| cx.open_with_system(&open_path))
                                })),
                        )
                    })
                    .into_any_element()
            }
            ChatItem::TurnEnd { .. } => div().into_any_element(),
            ChatItem::Approval {
                request_id,
                title,
                detail,
                options,
                decision,
            } => match decision {
                Some(decision) => {
                    // Only a refusal is worth a line; an approval shows in the tool itself.
                    let (label, color) = match decision.as_str() {
                        "deny" => ("Denied", theme.danger),
                        _ => ("Allowed", theme.success),
                    };
                    div()
                        .h(layout::CONTROL)
                        .flex()
                        .items_center()
                        .gap(layout::SPACE_2)
                        .text_size(TEXT)
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(IconName::ShieldCheck).size(ICON).text_color(color))
                        .child(div().flex_none().child(label))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(title.clone()),
                        )
                        .into_any_element()
                }
                None => {
                    let answer_button = |id: &'static str, label: &'static str, decision: &'static str, primary: bool| {
                        let request_id = request_id.clone();
                        div()
                            .id((id, ix))
                            .h(layout::CONTROL)
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(layout::RADIUS)
                            .text_size(TEXT)
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .map(|this| {
                                if primary {
                                    this.bg(theme.primary)
                                        .text_color(theme.primary_foreground)
                                        .hover(|this| this.bg(theme.primary_hover))
                                } else {
                                    this.bg(theme.secondary).hover(|this| this.bg(theme.secondary_hover))
                                }
                            })
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.answer(thread_id, request_id.clone(), decision, cx)
                            }))
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap(layout::SPACE_2)
                        .p(layout::SPACE_3)
                        .rounded_xl()
                        .border_1()
                        .border_color(theme.warning.opacity(0.35))
                        .bg(theme.warning.opacity(0.06))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(layout::SPACE_2)
                                .text_size(TEXT)
                                .font_weight(FontWeight::MEDIUM)
                                .child(Icon::new(IconName::ShieldQuestionMark).size(ICON).text_color(theme.warning))
                                .child(if title.ends_with('?') {
                                    title.clone()
                                } else {
                                    format!("Allow {title}?")
                                }),
                        )
                        .when_some(detail.clone(), |this, detail| {
                            this.child(
                                div()
                                    .px(layout::ROW_INSET)
                                    .py(px(6.))
                                    .rounded(layout::RADIUS)
                                    .bg(theme.secondary)
                                    .font_family(theme.mono_font_family.clone())
                                    .text_size(TEXT_SM)
                                    .child(detail),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .gap(layout::SPACE_2)
                                .child(answer_button("approve", "Allow", "allow", true))
                                .when(options.iter().any(|o| o == "allow_session"), |this| {
                                    this.child(answer_button(
                                        "approve-session",
                                        "Allow for session",
                                        "allow_session",
                                        false,
                                    ))
                                })
                                .child(answer_button("deny", "Deny", "deny", false)),
                        )
                        .into_any_element()
                }
            },
            ChatItem::Plan(entries) => div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .px(layout::SPACE_4)
                .py(px(10.))
                .rounded_xl()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .mb_1()
                        .flex()
                        .items_center()
                        .gap(layout::SPACE_2)
                        .text_size(TEXT_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(IconName::ListTodo).size(ICON))
                        .child(format!(
                            "Plan · {}/{}",
                            entries.iter().filter(|e| e.status == "completed").count(),
                            entries.len()
                        )),
                )
                .children(entries.iter().map(|entry| {
                    let done = entry.status == "completed";
                    let active = entry.status == "in_progress";
                    div()
                        .flex()
                        .items_start()
                        .gap(layout::SPACE_2)
                        .text_size(TEXT)
                        .line_height(relative(1.5))
                        .child(
                            Icon::new(if done {
                                IconName::CircleCheck
                            } else if active {
                                IconName::CircleDot
                            } else {
                                IconName::Circle
                            })
                            .size(ICON)
                            .mt(px(3.))
                            .flex_none()
                            .text_color(if done {
                                theme.success
                            } else if active {
                                theme.primary
                            } else {
                                theme.muted_foreground
                            }),
                        )
                        .child(
                            div()
                                .when(done, |this| this.line_through().text_color(theme.muted_foreground))
                                .child(entry.content.clone()),
                        )
                }))
                .into_any_element(),
            ChatItem::Handoff {
                from_agent,
                from_label,
                to_agent,
                to_label,
            } => {
                let line = || div().flex_1().h(px(1.)).bg(theme.border);
                div()
                    .my_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(line())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(layout::SPACE_2)
                            .text_size(TEXT_SM)
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(IconName::ArrowLeftRight).size(ICON))
                            .child(if from_agent == to_agent { "Model switch" } else { "Context handoff" })
                            .child(div().w(px(2.)))
                            .child(agent_icon(from_agent, ICON, cx))
                            .child(div().text_color(theme.foreground).child(from_label.clone()))
                            .child(Icon::new(IconName::ArrowRight).size(ICON_SM))
                            .child(agent_icon(to_agent, ICON, cx))
                            .child(
                                div()
                                    .text_color(agent_color(to_agent).unwrap_or(theme.foreground))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(to_label.clone()),
                            ),
                    )
                    .child(line())
                    .into_any_element()
            }
            ChatItem::Error(text) => div()
                .flex()
                .gap(layout::SPACE_2)
                .px_3()
                .py(px(9.))
                .rounded(layout::RADIUS)
                .border_1()
                .border_color(theme.danger.opacity(0.25))
                .bg(theme.danger.opacity(0.08))
                .text_size(TEXT)
                .text_color(theme.danger)
                .child(Icon::new(IconName::CircleAlert).size(ICON).mt(px(2.)).flex_none())
                .child(div().flex_1().child(text.clone()))
                .into_any_element(),
        }
    }

    fn render_composer(&self, thread: Option<&Thread>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let running = thread.is_some_and(|t| self.running.contains(&t.id));
        let has_text = !self.composer.read(cx).value().trim().is_empty();
        let can_send = has_text && (thread.is_some() || self.agent.is_some());
        let entity = cx.entity().downgrade();

        // Agent and model, for the open thread or the next new one.
        let (agent, model, effort) = match thread {
            Some(thread) => (Some(thread.agent.clone()), thread.model.clone(), thread.effort.clone()),
            None => (self.agent.clone(), self.model.clone(), self.effort.clone()),
        };
        let agent_picker = match (&self.agents, agent) {
            (None, _) => div()
                .px_2()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child("Detecting agents…")
                .into_any_element(),
            (Some(Err(err)), _) => div()
                .px_2()
                .text_size(TEXT)
                .text_color(theme.danger)
                .child(format!("Agents unavailable: {err}"))
                .into_any_element(),
            (Some(Ok(_)), None) => div()
                .px_2()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child("No agent installed")
                .into_any_element(),
            (Some(Ok(_)), Some(agent)) => {
                let label = self.model_label(&agent, model.as_deref(), cx);
                let picker = self.picker.clone();
                Popover::new("model-picker")
                    .anchor(Anchor::BottomLeft)
                    .open(self.picker_open && !running)
                    .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                        this.set_picker_open(*open, window, cx)
                    }))
                    .p_0()
                    .trigger(
                        Button::new("agent-picker")
                            .ghost()
                            .small().h(layout::CONTROL)
                            .disabled(running)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(layout::SPACE_2)
                                    .text_size(TEXT)
                                    .child(agent_icon(&agent, ICON_LG, cx))
                                    .child(label)
                                    .child(
                                        Icon::new(IconName::ChevronDown)
                                            .size(ICON)
                                            .text_color(theme.muted_foreground),
                                    ),
                            ),
                    )
                    .content(move |_, _, _| picker.clone())
                    .into_any_element()
            }
        };

        // Reasoning effort, where the model offers a choice.
        let efforts = match (&self.agent, thread) {
            (_, Some(thread)) => self.model_efforts(&thread.agent, thread.model.as_deref(), cx),
            (Some(agent), None) => self.model_efforts(agent, self.model.as_deref(), cx),
            _ => Vec::new(),
        };
        let effort_picker = (!efforts.is_empty()).then(|| {
            let current = effort.clone();
            let entity = entity.clone();
            Button::new("effort-picker")
                .ghost()
                .small().h(layout::CONTROL)
                .icon(Icon::new(IconName::Brain).size(ICON))
                .child(div().text_size(TEXT).child(capitalize(current.as_deref().unwrap_or("Auto"))))
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    let options = std::iter::once(None).chain(efforts.iter().cloned().map(Some));
                    for option in options {
                        let entity = entity.clone();
                        let label = capitalize(option.as_deref().unwrap_or("Auto"));
                        menu = menu.item(
                            PopupMenuItem::new(label)
                                .checked(current == option)
                                .on_click(move |_, _, cx| {
                                    let option = option.clone();
                                    entity.update(cx, |this, cx| this.set_effort(option, cx)).ok();
                                }),
                        );
                    }
                    menu
                })
        });

        let access = self.access.clone();
        let access_picker = Button::new("access-picker")
            .ghost()
            .small().h(layout::CONTROL)
            .icon(
                Icon::new(match access.as_str() {
                    "edit" => IconName::FilePen,
                    "ask" => IconName::ShieldQuestionMark,
                    "read_only" => IconName::Lock,
                    _ => IconName::LockOpen,
                })
                .size(ICON),
            )
            .child(div().text_size(TEXT).child(access_name(&access)))
            .text_size(TEXT)
            .text_color(theme.muted_foreground)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (id, name) in ACCESS_LEVELS {
                    let entity = entity.clone();
                    menu = menu.item(PopupMenuItem::new(name).checked(access == id).on_click(
                        move |_, _, cx| {
                            entity.update(cx, |this, cx| this.set_access(id, cx)).ok();
                        },
                    ));
                }
                menu
            });

        // While running, a typed message steers the turn; an empty box stops it.
        let action = div()
            .id("composer-action")
            .size(layout::CONTROL)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .map(|this| {
                if can_send {
                    this.bg(theme.primary)
                        .cursor_pointer()
                        .hover(|this| this.bg(theme.primary_hover))
                        .child(Icon::new(IconName::ArrowUp).size(ICON).text_color(theme.primary_foreground))
                        .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                } else if running {
                    this.bg(theme.foreground)
                        .cursor_pointer()
                        .child(div().size(px(10.)).rounded(px(2.)).bg(theme.background))
                        .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                } else {
                    this.bg(theme.secondary)
                        .child(Icon::new(IconName::ArrowUp).size(ICON).text_color(theme.muted_foreground))
                }
            });

        div()
            .w_full()
            .min_w_0()
            .rounded(layout::BUBBLE_RADIUS)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_sm()
            .p(layout::SPACE_3)
            .flex()
            .flex_col()
            .gap(layout::SPACE_2)
            .child(
                div()
                    .text_size(TEXT_LG)
                    .child(Textarea::new(&self.composer).appearance(false).bordered(false)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(layout::SPACE_1)
                    .ml(px(-4.))
                    .child(agent_picker)
                    .when_some(effort_picker, |this, picker| {
                        this.child(div().w(px(1.)).h(px(14.)).mx_1().bg(theme.border))
                            .child(picker)
                    })
                    .child(div().w(px(1.)).h(px(14.)).mx_1().bg(theme.border))
                    .child(access_picker)
                    .child(div().flex_1())
                    .when(running && has_text, |this| {
                        this.child(
                            div()
                                .mr_2()
                                .text_size(TEXT_SM)
                                .text_color(theme.muted_foreground)
                                .child("Enter to steer"),
                        )
                    })
                    .child(action),
            )
            .into_any_element()
    }
}

/// How the transcript lays out: single items, or runs of tool calls.
#[derive(Debug, PartialEq)]
enum Block {
    Single(usize),
    Group(Vec<usize>),
}

/// Approved permission requests and turn markers leave no visible trace.
fn is_hidden(item: &ChatItem) -> bool {
    matches!(item, ChatItem::Approval { decision: Some(d), .. } if d != "deny")
        || matches!(item, ChatItem::TurnEnd { .. })
}

fn is_step(item: &ChatItem) -> bool {
    matches!(item, ChatItem::Tool { .. } | ChatItem::Thinking(_)) || is_hidden(item)
}

/// Fold runs of two or more tool calls (with thinking and settled approvals
/// in between) into groups; hidden items outside a group are dropped.
fn group_items(items: &[ChatItem], ixs: &[usize]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let flush = |run: &mut Vec<usize>, blocks: &mut Vec<Block>| {
        let tools = run
            .iter()
            .filter(|&&ix| matches!(items[ix], ChatItem::Tool { .. }))
            .count();
        if tools >= 2 {
            blocks.push(Block::Group(std::mem::take(run)));
        } else {
            blocks.extend(
                run.drain(..)
                    .filter(|&ix| !is_hidden(&items[ix]))
                    .map(Block::Single),
            );
        }
    };
    for &ix in ixs {
        if is_step(&items[ix]) {
            run.push(ix);
        } else {
            flush(&mut run, &mut blocks);
            blocks.push(Block::Single(ix));
        }
    }
    flush(&mut run, &mut blocks);
    blocks
}

/// One finished turn: the work before the agent's final message, folded
/// under "Worked for …", and what stays in view.
struct Turn {
    /// The user's message, if the turn has one.
    user: Option<usize>,
    work: Vec<usize>,
    /// Final message, errors, plans, pending approvals, handoffs.
    visible: Vec<usize>,
    ms: Option<u64>,
    finished: bool,
}

/// An image the agent made: the answer itself, so it stays in view.
fn is_generated_image(item: &ChatItem) -> bool {
    matches!(item, ChatItem::Tool { title, images, .. } if title == "Generate image" && !images.is_empty())
}

/// Split a transcript into turns at each user message.
fn split_turns(items: &[ChatItem]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut body: Vec<usize> = Vec::new();
    let mut user: Option<usize> = None;
    let close = |user: Option<usize>, body: &mut Vec<usize>, turns: &mut Vec<Turn>| {
        if user.is_none() && body.is_empty() {
            return;
        }
        let ms = body.iter().find_map(|&ix| match items[ix] {
            ChatItem::TurnEnd { ms, .. } => Some(ms),
            _ => None,
        });
        let finished = ms.is_some();
        // The final message is the last thing the agent said after its last tool.
        let last_tool = body.iter().rposition(|&ix| matches!(items[ix], ChatItem::Tool { .. }));
        let final_message = body
            .iter()
            .rposition(|&ix| matches!(items[ix], ChatItem::Assistant(_)))
            .filter(|&pos| last_tool.is_none_or(|tool| pos > tool));
        let (mut work, mut visible) = (Vec::new(), Vec::new());
        for (pos, &ix) in body.iter().enumerate() {
            let stays = Some(pos) == final_message
                || matches!(
                    items[ix],
                    ChatItem::Error(_) | ChatItem::Plan(_) | ChatItem::Handoff { .. }
                )
                || matches!(items[ix], ChatItem::Approval { decision: None, .. })
                || is_generated_image(&items[ix]);
            if stays { visible.push(ix) } else { work.push(ix) }
        }
        turns.push(Turn {
            user,
            work,
            visible,
            ms: ms.flatten(),
            finished,
        });
        body.clear();
    };
    for (ix, item) in items.iter().enumerate() {
        if matches!(item, ChatItem::User(_)) {
            close(user, &mut body, &mut turns);
            user = Some(ix);
        } else {
            body.push(ix);
        }
    }
    close(user, &mut body, &mut turns);
    turns
}

/// "Worked for 6m 21s".
fn worked_for(ms: Option<u64>) -> String {
    let Some(ms) = ms else { return "Worked".into() };
    let secs = (ms + 500) / 1000;
    match secs {
        0 => "Worked for 1s".into(),
        1..60 => format!("Worked for {secs}s"),
        60..3600 => format!("Worked for {}m {}s", secs / 60, secs % 60),
        _ => format!("Worked for {}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

/// A tool call as one line of text, the way Codex reads: "Ran cargo test".
fn tool_line(
    title: &str,
    detail: Option<&str>,
    running: bool,
    project: Option<&std::path::Path>,
) -> (IconName, String) {
    let detail = detail.map(|d| d.lines().next().unwrap_or("").trim()).unwrap_or("");
    // Paths inside the project read better relative to it.
    let short = project
        .and_then(|root| std::path::Path::new(detail).strip_prefix(root).ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| detail.to_string());
    let with = |verb: &str, rest: &str| {
        if rest.is_empty() { verb.to_string() } else { format!("{verb} {rest}") }
    };
    match title {
        "Run" => (IconName::SquareTerminal, with(if running { "Running" } else { "Ran" }, &short)),
        "Edit" => (IconName::FilePen, with(if running { "Editing" } else { "Edited" }, &short)),
        "Read" => (IconName::FileText, with(if running { "Reading" } else { "Read" }, &short)),
        "Search" => (IconName::Search, with(if running { "Searching for" } else { "Searched for" }, &short)),
        "Web" if detail.starts_with("http") => {
            (IconName::Globe, with(if running { "Fetching" } else { "Fetched" }, detail))
        }
        "Web" => (
            IconName::Globe,
            with(if running { "Searching the web for" } else { "Searched the web for" }, detail),
        ),
        "Computer" if detail.is_empty() => (IconName::Monitor, "Used the computer".into()),
        "Browser" if detail.is_empty() => (IconName::Globe, "Used the browser".into()),
        "Computer" => (IconName::Monitor, detail.to_string()),
        "Browser" => (IconName::Globe, detail.to_string()),
        "Update todos" => (IconName::ListTodo, "Updated todos".into()),
        "Generate image" => (IconName::Image, if running { "Generating image" } else { "Generated image" }.into()),
        "View image" => (IconName::Image, with(if running { "Viewing" } else { "Viewed" }, &short)),
        other => (IconName::Wrench, with(other, detail)),
    }
}

/// "Read files, ran commands, searched the web", the failures, and an icon.
fn summarize_group(items: &[ChatItem], ixs: &[usize]) -> (String, usize, IconName) {
    let mut kinds: Vec<&str> = Vec::new();
    let mut failed = 0;
    for &ix in ixs {
        if let ChatItem::Tool { title, status, .. } = &items[ix] {
            if *status == ToolStatus::Failed {
                failed += 1;
            }
            if !kinds.contains(&title.as_str()) {
                kinds.push(title.as_str());
            }
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for kind in &kinds {
        let part = match *kind {
            "Run" => "ran commands",
            "Edit" => "edited files",
            "Read" => "read files",
            "Search" => "searched files",
            "Web" => "searched the web",
            "Computer" => "used the computer",
            "Browser" => "used the browser",
            "Update todos" => "updated todos",
            "Generate image" => "generated images",
            "View image" => "viewed images",
            other => other,
        };
        if !parts.iter().any(|p| p == part) {
            parts.push(part.to_string());
        }
    }
    let icon = match kinds.first().copied() {
        Some("Computer") => IconName::Monitor,
        Some("Browser") | Some("Web") => IconName::Globe,
        Some("Edit") => IconName::FilePen,
        Some("Run") => IconName::SquareTerminal,
        _ => IconName::Search,
    };
    (capitalize(&parts.join(", ")), failed, icon)
}

#[cfg(test)]
mod group_tests {
    use super::{Block, group_items, split_turns, summarize_group, tool_line, worked_for};
    use crate::threads::{ChatItem, ToolStatus};

    fn tool(title: &str, status: ToolStatus) -> ChatItem {
        ChatItem::Tool {
            id: "c".into(),
            title: title.into(),
            detail: None,
            path: None,
            output: None,
            status,
            code: None,
            images: Vec::new(),
        }
    }

    fn allowed() -> ChatItem {
        ChatItem::Approval {
            request_id: "r".into(),
            title: "Allow Computer Use to use \"Music\"?".into(),
            detail: None,
            options: Vec::new(),
            decision: Some("allow_session".into()),
        }
    }

    #[test]
    fn approvals_disappear_and_tool_runs_fold() {
        let items = vec![
            ChatItem::User("play".into()),
            allowed(),
            tool("Computer", ToolStatus::Done),
            allowed(),
            tool("Run", ToolStatus::Failed),
            ChatItem::Assistant("done".into()),
            allowed(),
            tool("Run", ToolStatus::Done),
        ];
        let all: Vec<usize> = (0..items.len()).collect();
        let blocks = group_items(&items, &all);
        assert_eq!(blocks[0], Block::Single(0));
        let Block::Group(ixs) = &blocks[1] else { panic!("tools should fold") };
        assert_eq!(ixs, &vec![1, 2, 3, 4]);
        assert_eq!(blocks[2], Block::Single(5));
        // A lone tool stays a single line and its approval is dropped.
        assert_eq!(blocks[3], Block::Single(7));
        assert_eq!(blocks.len(), 4);

        let (summary, failed, _) = summarize_group(&items, ixs);
        assert_eq!(summary, "Used the computer, ran commands");
        assert_eq!(failed, 1);
    }

    #[test]
    fn finished_turns_fold_their_work_and_keep_the_answer() {
        let items = vec![
            ChatItem::User("q".into()),
            ChatItem::Assistant("I'll look.".into()),
            tool("Read", ToolStatus::Done),
            tool("Run", ToolStatus::Done),
            ChatItem::Assistant("Answer".into()),
            ChatItem::TurnEnd { ms: Some(381_000), at: None },
            ChatItem::User("next".into()),
            ChatItem::Assistant("Working on it".into()),
        ];
        let turns = split_turns(&items);
        assert_eq!(turns.len(), 2);
        assert_eq!((turns[0].user, &turns[0].work, &turns[0].visible), (Some(0), &vec![1, 2, 3, 5], &vec![4]));
        assert!(turns[0].finished);
        assert_eq!(worked_for(turns[0].ms), "Worked for 6m 21s");
        assert!(!turns[1].finished);
    }

    #[test]
    fn tool_lines_read_like_sentences() {
        let root = std::path::Path::new("/p");
        assert_eq!(tool_line("Run", Some("cargo test\nmore"), false, None).1, "Ran cargo test");
        assert_eq!(tool_line("Read", Some("/p/src/main.rs"), false, Some(root)).1, "Read src/main.rs");
        assert_eq!(tool_line("Web", Some("\"RouteAny\" router"), false, None).1, "Searched the web for \"RouteAny\" router");
        assert_eq!(tool_line("Computer", Some("Play the song"), false, None).1, "Play the song");
    }
}
