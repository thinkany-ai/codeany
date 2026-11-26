//! Agent mode: a list of threads and a conversation with a coding agent.

use crate::style::AppButtonStyle as _;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
};

use agl::protocol::{AgentEvent, AgentInfo};
use futures::StreamExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{InputEvent, Textarea, TextareaState},
    menu::{ContextMenuExt as _, DropdownMenu as _},
    popover::Popover,
    Disableable as _,
    shimmer::ShimmerText,
    spinner::Spinner,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{
    agent_icons::{agent_color, agent_icon},
    agl_bridge::{self, SessionOptions},
    chat_tabs::TabSet,
    git,
    i18n::{t, tf},
    model_picker::{ModelPicker, ModelPickerEvent, model_display_name},
    project_avatar::{ProjectAvatar, avatar_size},
    project_filter::{ProjectFilter, ProjectFilterEvent},
    workspace_switcher::{MenuDevice, WorkspaceMenu, WorkspaceMenuEvent},
    remote,
    remote_folders::{RemoteFolders, RemoteFoldersEvent},
    threads::{
        ACCESS_LEVELS, ChatItem, Store, Thread, ToolStatus, access_name, agent_name,
        UserMessage, clock_time, delete_thread_files, display_title, is_image_path, relative_time,
        Worktree, save_pasted_image,
    },
};
use crate::style::{
    layout,
    size::{TEXT, ICON},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarSection {
    Projects,
    Recent,
    Archived,
}

/// How often the account's other machines are looked up while signed in.
const HOSTS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
/// Threads listed per project before "Show more".
const THREADS_PER_PROJECT: usize = 8;

/// An error line for display. The errors CodeAny writes itself are stored in
/// English and shown in the current language; agents' own errors pass through.
fn display_error(text: &str) -> String {
    match text {
        "Interrupted" => t("Interrupted").into(),
        "The turn failed" => t("The turn failed").into(),
        other => other.to_string(),
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// What a tab in the header shows.
pub struct ChatTab {
    /// The thread it shows; `None` for a new thread.
    pub id: Option<u64>,
    pub title: String,
    /// The thread's agent, for its icon; `None` for a new thread.
    pub agent: Option<String>,
    pub running: bool,
}

pub enum AgentPanelEvent {
    OpenFile(PathBuf),
    SwitchProject(Option<PathBuf>),
    OpenFolder,
    TurnFinished,
    OpenSettings,
}

/// A thread's live agl session, for its current agent.
struct Session {
    id: String,
    agent: String,
    /// The nex device the session runs on; `None` is this machine.
    machine: Option<String>,
    features: Vec<String>,
    _events: Task<()>,
}

enum PromptState {
    Waiting,
    Sending,
    Accepted { queued: bool },
    Failed(String),
}

struct PendingPrompt {
    turn_id: String,
    message: UserMessage,
    state: PromptState,
}

impl PendingPrompt {
    fn is_submitted(&self) -> bool {
        matches!(self.state, PromptState::Sending | PromptState::Accepted { .. })
    }
}

pub struct AgentPanel {
    store: Store,
    project: Option<PathBuf>,
    active: Option<u64>,
    /// The agents of `agents_machine`: the open thread's machine, or the
    /// next new thread's.
    agents: Option<Result<Vec<AgentInfo>, String>>,
    agents_machine: Option<String>,
    /// Detected agents, by machine.
    agents_cache: HashMap<Option<String>, Result<Vec<AgentInfo>, String>>,
    /// The machine the next new thread runs on; `None` is this one.
    machine: Option<String>,
    /// The account's other machines that share their agents.
    hosts: Vec<remote::Device>,
    folders: Entity<RemoteFolders>,
    folders_open: bool,
    /// The workspace menu at the top of the sidebar, and whether it is open.
    workspace_menu: Entity<WorkspaceMenu>,
    workspace_menu_open: bool,
    /// The Projects section's filter menu, and whether it is open.
    project_filter: Entity<ProjectFilter>,
    project_filter_open: bool,
    /// The one project the sidebar shows, picked in that menu; `None` shows all.
    shown_project: Option<PathBuf>,
    /// The agent, model, effort and access level for the next new thread.
    agent: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    access: String,
    /// Whether a new thread on a git project gets a worktree of its own.
    use_worktree: bool,
    picker: Entity<ModelPicker>,
    picker_open: bool,
    /// Threads of closed tabs, most recent last, for reopening.
    closed_tabs: Vec<u64>,
    projects_expanded: bool,
    recent_expanded: bool,
    /// Archived threads are listed at the foot of the sidebar, folded.
    archived_expanded: bool,
    sort_repositories_by_name: bool,
    navigation_history: Vec<u64>,
    navigation_index: usize,
    all_projects: bool,
    expanded_projects: HashSet<PathBuf>,
    /// Projects showing all their threads, not only the latest few.
    long_projects: HashSet<Option<PathBuf>>,
    sessions: HashMap<u64, Session>,
    running: HashSet<u64>,
    /// Only the first pending prompt can be submitted to agl. The others
    /// wait here so they can be removed or explicitly steered.
    pending_prompts: HashMap<u64, VecDeque<PendingPrompt>>,
    /// Threads that finished a turn while not shown, until opened.
    unread: HashSet<u64>,
    /// A transient note on the "Working…" line: retries, stalls.
    notes: HashMap<u64, String>,
    expanded: HashSet<(u64, usize)>,
    /// Tool groups the user opened (true) or closed (false), by first item.
    groups: HashMap<(u64, usize), bool>,
    /// "Worked for …" folds the user opened, by the turn's first work item.
    worked: HashSet<(u64, usize)>,
    /// The answer last copied, to show a check.
    copied: Option<(u64, usize)>,
    composer: Entity<TextareaState>,
    /// Images pasted into the composer, sent with the next message.
    attachments: Vec<PathBuf>,
    /// The open project's git branch, from the workspace's polling.
    branch: Option<String>,
    /// The transcript, virtualized: one row per turn (and one for the
    /// "Working…" line), laid out only while in view. It follows the tail
    /// as a reply grows; scrolling up stops that, so a running turn cannot
    /// drag the reader back down, and scrolling back to the end resumes it.
    chat_list: ListState,
    /// The thread the rows are of, its turns, and a cheap signature of each
    /// row, to tell the list which rows to measure again.
    chat_thread: Option<u64>,
    chat_turns: Vec<Turn>,
    chat_sigs: Vec<u64>,
    /// Whether the transcript keeps to its end, as the list last reported.
    follow: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AgentPanelEvent> for AgentPanel {}

impl AgentPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 12)
                .submit_on_enter(true)
                .placeholder(t("Ask anything, or describe a change to make"))
        });
        let picker = cx.new(|cx| ModelPicker::new(window, cx));
        let folders = cx.new(|_| RemoteFolders::new());
        let project_filter = cx.new(|cx| ProjectFilter::new(window, cx));
        let workspace_menu = cx.new(|cx| WorkspaceMenu::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&workspace_menu, window, Self::on_workspace_menu),
            cx.subscribe(&project_filter, |this, _, event: &ProjectFilterEvent, cx| {
                match event {
                    ProjectFilterEvent::Pick(project) => {
                        this.shown_project = project.clone();
                        if let Some(project) = project {
                            this.expanded_projects.insert(project.clone());
                        }
                        this.project_filter_open = false;
                    }
                    ProjectFilterEvent::SortByName(by_name) => this.sort_repositories_by_name = *by_name,
                }
                cx.notify();
            }),
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
            cx.subscribe(&folders, |this, _, event: &RemoteFoldersEvent, cx| match event {
                RemoteFoldersEvent::Pick(dir) => {
                    if let Some(machine) = this.machine.clone() {
                        this.store.remote_dirs.insert(machine, dir.clone());
                        this.store.save();
                    }
                    this.folders_open = false;
                    cx.notify();
                }
            }),
            // Model names and efforts arrive as the picker loads them.
            cx.observe(&picker, |_, _, cx| cx.notify()),
        ];

        // Detecting agents starts agl and probes every CLI's version.
        cx.on_next_frame(window, |this, _, cx| {
            let machine = this.machine.clone();
            this.agents_machine = machine.clone();
            this.picker.update(cx, |picker, cx| picker.set_machine(machine.clone(), cx));
            this.detect_agents(machine, cx);
        });

        // The account's other machines come online and go.
        cx.spawn(async move |this, cx| {
            loop {
                // The account's devices and the paired ones.
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

        let mut store = Store::load();
        // Everything from before workspaces lands in one.
        store.ensure_workspaces(&remote::this_name());
        store.machine = store.active_workspace().and_then(|w| w.device.clone());
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
        // Tabs of threads deleted or settled since are gone.
        let live: HashSet<u64> = store.threads.iter().filter(|t| !t.settled).map(|t| t.id).collect();
        for set in &mut store.tabs {
            set.retain_threads(|id| live.contains(&id));
        }
        crate::style::set_typography(store.typography.clone(), cx);
        if let Some(theme) = store.theme.clone() {
            crate::style::set_choice(theme, window, cx);
        }
        // Rows well above and below the view stay laid out, so a fast
        // scroll rarely shows a row being measured.
        let chat_list = ListState::new(0, ListAlignment::Top, px(1200.));
        chat_list.set_follow_mode(FollowMode::Tail);
        {
            let panel = cx.weak_entity();
            chat_list.set_scroll_handler(move |event, _, cx| {
                let following = event.is_following_tail;
                panel
                    .update(cx, |this, cx| {
                        if this.follow != following {
                            this.follow = following;
                            cx.notify();
                        }
                    })
                    .ok();
            });
        }
        Self {
            machine: store.machine.clone(),
            agents_machine: store.machine.clone(),
            agents_cache: HashMap::new(),
            hosts: Vec::new(),
            folders,
            folders_open: false,
            workspace_menu,
            workspace_menu_open: false,
            project_filter,
            project_filter_open: false,
            shown_project: None,
            agent: store.agent.clone(),
            model: store.model.clone(),
            effort: store.effort.clone(),
            access: store.access.clone(),
            use_worktree: false,
            picker,
            picker_open: false,
            closed_tabs: Vec::new(),
            unread: HashSet::new(),
            projects_expanded: true,
            recent_expanded: true,
            archived_expanded: false,
            sort_repositories_by_name: false,
            navigation_history: Vec::new(),
            navigation_index: 0,
            all_projects: false,
            expanded_projects: HashSet::new(),
            long_projects: HashSet::new(),
            store,
            project: None,
            active: None,
            agents: None,
            sessions: HashMap::new(),
            running: HashSet::new(),
            pending_prompts: HashMap::new(),
            notes: HashMap::new(),
            expanded: HashSet::new(),
            groups: HashMap::new(),
            worked: HashSet::new(),
            copied: None,
            composer,
            attachments: Vec::new(),
            branch: None,
            chat_list,
            chat_thread: None,
            chat_turns: Vec::new(),
            chat_sigs: Vec::new(),
            follow: true,
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
            // A folder opened here joins the workspace shown, when it is this device's.
            let active = self.store.active_workspace.clone();
            if let Some(ws) = self.store.workspaces.iter_mut().find(|w| Some(&w.id) == active.as_ref() && w.device.is_none()) {
                if !ws.projects.contains(project) {
                    ws.projects.insert(0, project.clone());
                }
            }
        }
        if self.store.last_project != project || project.is_some() {
            self.store.last_project = project.clone();
            self.store.save();
        }
        if self.project == project {
            return;
        }
        self.project = project;
        self.branch = None;
        // Each project has its own tabs; show the one last active there.
        self.active = self.tab_set_mut().current();
        self.sync_machine(cx);
        cx.notify();
    }

    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }

    fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        self.composer_focus_handle(cx).focus(window, cx);
    }

    /// A new thread opens in a tab of its own.
    pub fn new_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_tab(window, cx);
    }

    // ---- Tabs ---------------------------------------------------------------

    /// The open project's tabs in the workspace shown, if any yet.
    fn tab_set(&self) -> Option<&TabSet> {
        let workspace = &self.store.active_workspace;
        self.store.tabs.iter().find(|s| s.project == self.project && &s.workspace == workspace)
    }

    /// The open project's tabs in the workspace shown, created on first use.
    fn tab_set_mut(&mut self) -> &mut TabSet {
        let workspace = self.store.active_workspace.clone();
        let ix = match self.store.tabs.iter().position(|s| s.project == self.project && s.workspace == workspace) {
            Some(ix) => ix,
            None => match self.store.tabs.iter().position(|s| s.project == self.project && s.workspace.is_none()) {
                // Tabs from before workspaces join the first one to open them.
                Some(ix) => {
                    self.store.tabs[ix].workspace = workspace;
                    ix
                }
                None => {
                    self.store.tabs.push(TabSet::new(self.project.clone(), workspace));
                    self.store.tabs.len() - 1
                }
            },
        };
        &mut self.store.tabs[ix]
    }

    /// Whether any tab shows a thread; a lone new tab cannot be closed.
    pub fn has_open_thread(&self) -> bool {
        self.tab_set().is_some_and(|s| s.tabs.iter().any(Option::is_some))
    }

    /// The open project's tabs, and the active one.
    pub fn tab_items(&self) -> (Vec<ChatTab>, usize) {
        let (tabs, active) = match self.tab_set() {
            Some(set) => (set.tabs.clone(), set.active),
            None => (vec![self.active], 0),
        };
        let items = tabs
            .into_iter()
            .map(|id| {
                let thread = id.and_then(|id| self.store.threads.iter().find(|t| t.id == id));
                ChatTab {
                    id,
                    title: thread.map(|t| display_title(&t.title)).unwrap_or_else(|| t("New thread").into()),
                    agent: thread.map(|t| t.agent.clone()),
                    running: id.is_some_and(|id| self.is_busy(id)),
                }
            })
            .collect();
        (items, active)
    }

    /// Give a thread a title of the user's own.
    pub fn rename_thread(&mut self, id: u64, title: &str, cx: &mut Context<Self>) {
        let title: String = title.trim().chars().take(120).collect();
        if title.is_empty() {
            return;
        }
        if let Some(thread) = self.thread_mut(id) {
            thread.title = title;
        }
        self.store.save();
        cx.notify();
    }

    /// Close every tab but `ix`; the threads stay in the sidebar.
    pub fn close_other_tabs(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let closed = self.tab_set_mut().close_others(ix);
        self.closed_tabs.extend(closed);
        self.show_active_tab(window, cx);
    }

    /// Close the tabs after `ix`.
    pub fn close_tabs_to_right(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let closed = self.tab_set_mut().close_right(ix);
        self.closed_tabs.extend(closed);
        self.show_active_tab(window, cx);
    }

    /// Show what the active tab holds.
    fn show_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab_set_mut().current() {
            Some(id) => self.select_thread(id, window, cx),
            None => {
                self.active = None;
                self.sync_machine(cx);
                self.focus_composer(window, cx);
            }
        }
        self.store.save();
        cx.notify();
    }

    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_set_mut().open_new();
        self.show_active_tab(window, cx);
    }

    /// Close tab `ix`, or the active one. The thread stays in the sidebar.
    pub fn close_tab(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let set = self.tab_set_mut();
        let ix = ix.unwrap_or(set.active);
        if let Some(id) = set.close(ix) {
            self.closed_tabs.push(id);
        }
        self.show_active_tab(window, cx);
    }

    /// Reopen the tab closed last whose thread still exists.
    pub fn reopen_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(id) = self.closed_tabs.pop() {
            if self.store.threads.iter().any(|t| t.id == id) {
                self.tab_set_mut().open(id);
                self.show_active_tab(window, cx);
                return;
            }
        }
    }

    pub fn select_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_set_mut().select(ix);
        self.show_active_tab(window, cx);
    }

    pub fn select_last_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let set = self.tab_set_mut();
        let last = set.tabs.len().saturating_sub(1);
        set.select(last);
        self.show_active_tab(window, cx);
    }

    pub fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_set_mut().cycle(delta);
        self.show_active_tab(window, cx);
    }

    /// Open `id` in a tab of its own (⌘-click, middle-click), or switch to
    /// the tab already showing it.
    fn open_in_new_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.store.threads.iter().find(|t| t.id == id) else {
            return;
        };
        // Another project's thread gets its tab when `select_thread` moves there.
        if thread.machine.is_some() || thread.project == self.project {
            self.tab_set_mut().open(id);
        }
        self.select_thread(id, window, cx);
        self.store.save();
    }

    /// Whether a tab of its project shows `id`.
    fn is_open_in_tab(&self, thread: &Thread) -> bool {
        let project = if thread.machine.is_some() { &self.project } else { &thread.project };
        let workspace = self.store.workspace_of(thread).map(|w| w.id.clone());
        self.store
            .tabs
            .iter()
            .find(|s| &s.project == project && s.workspace == workspace)
            .is_some_and(|s| s.tabs.contains(&Some(thread.id)))
    }

    /// Close the tabs showing `id`, in every project, and show whatever
    /// takes the active tab's place.
    fn drop_thread_tabs(&mut self, id: u64, cx: &mut Context<Self>) {
        for set in &mut self.store.tabs {
            set.retain_threads(|t| t != id);
        }
        if self.active == Some(id) {
            self.active = self.tab_set_mut().current();
            self.sync_machine(cx);
        }
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

    /// The project open when CodeAny last ran, if its folder still exists.
    pub fn last_project(&self) -> Option<PathBuf> {
        self.store.last_project.clone().filter(|p| p.is_dir())
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
        let remote = thread.machine.is_some();
        // The sidebar shows the workspace the thread is in.
        let home = self.store.workspace_of(thread).map(|w| w.id.clone());
        let switched = home.is_some() && home != self.store.active_workspace;
        if switched {
            self.store.active_workspace = home;
            self.shown_project = None;
            self.machine = thread.machine.clone();
            self.store.machine = self.machine.clone();
            self.store.save();
        }
        self.active = Some(id);
        self.unread.remove(&id);
        // A folder on another machine is not one to open here.
        if !remote && project != self.project {
            cx.emit(AgentPanelEvent::SwitchProject(project.clone()));
            self.project = project;
            // Another project's thread gets a tab there, beside its others.
            self.tab_set_mut().open(id);
        } else if switched {
            // So does another workspace's.
            self.tab_set_mut().open(id);
        } else {
            // Like following a link: the active tab shows it.
            self.tab_set_mut().navigate(id);
        }
        self.sync_machine(cx);
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

    /// Go to the end of the transcript and follow it again. The list keeps
    /// to the end itself while rows below lay out (markdown grows a row
    /// after its first frame).
    fn pin_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.follow = true;
        self.chat_list.set_follow_mode(FollowMode::Tail);
        cx.notify();
    }

    /// Keep to the end of the transcript if the reader is following it.
    fn follow_to_bottom(&mut self) {
        if self.follow {
            self.chat_list.scroll_to_end();
        }
    }

    /// Measure every transcript row again (the theme or text size changed).
    pub fn remeasure_chat(&self) {
        self.chat_list.remeasure();
    }

    fn delete_thread(&mut self, id: u64, cx: &mut Context<Self>) {
        self.remove_thread(id, true, cx);
    }

    /// Delete after asking: unlike archiving, it cannot be undone.
    fn confirm_delete(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.store.threads.iter().find(|t| t.id == id) else { return };
        let message = tf("Delete “{title}”?", &[("title", thread.title.clone())]);
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(t("The conversation is removed for good. To hide it but keep it, archive it instead.")),
            &[PromptButton::ok(t("Delete")), PromptButton::cancel(t("Cancel"))],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| this.delete_thread(id, cx)).ok();
            }
        })
        .detach();
    }

    /// Drop a thread; `propagate` also deletes it on the device it runs on.
    fn remove_thread(&mut self, id: u64, propagate: bool, cx: &mut Context<Self>) {
        if let Some(session) = self.sessions.remove(&id) {
            agl_bridge::backend(session.machine.as_deref()).close(session.id);
        }
        self.running.remove(&id);
        if let Some(prompts) = self.pending_prompts.remove(&id) {
            for prompt in prompts {
                Self::delete_pasted_images(&prompt.message.images);
            }
        }
        if let Some(thread) = self.store.threads.iter().find(|t| t.id == id) {
            if let Some(machine) = thread.machine.as_deref().filter(|_| propagate) {
                crate::sync::deleted(machine, id);
            }
            delete_thread_files(thread);
            // Another machine's worktree is that machine's to remove.
            if let Some(worktree) = thread.worktree.clone().filter(|_| thread.machine.is_none()) {
                cx.background_executor()
                    .spawn(async move {
                        if let Err(err) = git::remove_worktree(&worktree.repo, &worktree.path, &worktree.branch) {
                            eprintln!("codeany: kept worktree {} ({}): {err}", worktree.path.display(), worktree.branch);
                        }
                    })
                    .detach();
            }
        }
        self.store.threads.retain(|t| t.id != id);
        let current = self.navigation_history.get(self.navigation_index).copied();
        self.navigation_history.retain(|visited| *visited != id);
        self.navigation_index = current.and_then(|current| self.navigation_history.iter().position(|visited| *visited == current))
            .unwrap_or_else(|| self.navigation_history.len().saturating_sub(1));
        self.drop_thread_tabs(id, cx);
        self.store.save();
        cx.notify();
    }

    /// The thread's live agl session for its current agent, if open.
    fn live_agl_session(&self, id: u64) -> Option<&Session> {
        let thread = self.store.threads.iter().find(|t| t.id == id)?;
        self.sessions
            .get(&id)
            .filter(|s| s.agent == thread.agent && s.machine == thread.machine)
    }

    /// Open the thread's agl session and subscribe to it; the session id.
    fn open_session(&mut self, id: u64, cx: &mut Context<Self>) -> Task<Result<String, String>> {
        let Some(thread) = self.store.threads.iter().find(|t| t.id == id) else {
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
                self.store.agent_args.get(&thread.agent).cloned().unwrap_or_default()
            } else {
                Vec::new()
            },
        };
        // A worktree is checked out on first use, and again if its folder went away.
        let checkout = thread.worktree.clone().filter(|w| thread.machine.is_none() && !w.path.exists());
        cx.spawn(async move |this, cx| {
            if let Some(worktree) = checkout {
                cx.background_executor()
                    .spawn(async move { git::add_worktree(&worktree.repo, &worktree.path, &worktree.branch) })
                    .await
                    .map_err(|err| format!("Cannot create the worktree: {err}"))?;
                // The workspace shows the worktree now that it exists.
                this.update(cx, |_, cx| cx.notify()).ok();
            }
            if let Some(dir) = remote_dir {
                if let Ok(reply) = dir.await {
                    opts.cwd = reply["path"].as_str().map(PathBuf::from);
                }
            }
            let meta = agl_bridge::backend(machine.as_deref())
                .open_session(name, opts)
                .await
                .map_err(|err| format!("{err:#}"))?;
            this.update(cx, |this, cx| {
                if !this.store.threads.iter().any(|thread| thread.id == id) {
                    agl_bridge::backend(machine.as_deref()).close(meta.id);
                    return Err("thread is gone".into());
                }
                let mut events = agl_bridge::backend(machine.as_deref()).subscribe(meta.id.clone());
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
                this.sessions.insert(
                    id,
                    Session {
                        id: meta.id.clone(),
                        agent,
                        machine,
                        features: meta.features,
                        _events: task,
                    },
                );
                Ok(meta.id)
            })
            .map_err(|_| "the window closed".to_string())?
        })
    }

    /// Take the images in a paste; `true` when there were any, so the
    /// textarea leaves the rest of the paste alone.
    fn paste(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        // The agent on another machine cannot open files from this one.
        if self.context_machine().is_some() {
            return false;
        }
        let mut pasted = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    if let Some(path) = save_pasted_image(image) {
                        self.attachments.push(path);
                        pasted = true;
                    }
                }
                // Image files copied in Finder are attached where they are.
                ClipboardEntry::ExternalPaths(paths) => {
                    for path in paths.paths().iter().filter(|p| is_image_path(p)) {
                        if !self.attachments.contains(path) {
                            self.attachments.push(path.clone());
                        }
                        pasted = true;
                    }
                }
                ClipboardEntry::String(_) => {}
            }
        }
        if pasted {
            cx.notify();
        }
        pasted
    }

    /// The open project's current branch, as the workspace last saw it.
    pub fn set_branch(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        if self.branch != branch {
            self.branch = branch;
            cx.notify();
        }
    }

    /// Pick image files to attach.
    fn attach_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t("Attach").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            this.update(cx, |this, cx| {
                for path in paths.into_iter().filter(|p| is_image_path(p)) {
                    if !this.attachments.contains(&path) {
                        this.attachments.push(path);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove_attachment(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.attachments.len() {
            let path = self.attachments.remove(ix);
            Self::delete_pasted_images(&[path]);
            cx.notify();
        }
    }

    fn delete_pasted_images(images: &[PathBuf]) {
        let dir = crate::paths::images();
        for path in images {
            if path.starts_with(&dir) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }

        let id = match self.active {
            Some(id) => id,
            None => {
                let Some(agent) = self.agent.clone() else {
                    return;
                };
                let mut thread = match self.machine.clone() {
                    Some(machine) => {
                        let mut thread = Thread::new(
                            self.store.remote_dirs.get(&machine).cloned(),
                            agent,
                            self.access.clone(),
                        );
                        thread.machine_name = Some(self.machine_name(&machine, None));
                        thread.machine = Some(machine);
                        thread
                    }
                    None => Thread::new(self.project.clone(), agent, self.access.clone()),
                };
                thread.workspace = self.store.active_workspace.clone();
                thread.model = self.model.clone();
                thread.effort = self.effort.clone();
                thread.title = if text.is_empty() {
                    "Image".to_string()
                } else {
                    text.lines().next().unwrap_or("").chars().take(60).collect()
                };
                if self.use_worktree && thread.machine.is_none() {
                    thread.worktree = thread
                        .project
                        .as_deref()
                        .and_then(|project| Worktree::plan(project, thread.id, &thread.title));
                }
                let id = thread.id;
                self.store.threads.insert(0, thread);
                self.active = Some(id);
                self.tab_set_mut().navigate(id);
                id
            }
        };

        self.pending_prompts.entry(id).or_default().push_back(PendingPrompt {
            turn_id: uuid::Uuid::new_v4().to_string(),
            message: UserMessage {
                text,
                images: std::mem::take(&mut self.attachments),
            },
            state: PromptState::Waiting,
        });
        self.dispatch_prompt(id, false, cx);
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.follow = true;
        self.chat_list.set_follow_mode(FollowMode::Tail);
        self.store.save();
        cx.notify();
    }

    fn can_steer(&self, id: u64) -> bool {
        self.running.contains(&id)
            && self.live_agl_session(id).is_some_and(|s| s.features.iter().any(|f| f == "steer"))
    }

    fn is_busy(&self, id: u64) -> bool {
        self.running.contains(&id)
            || self.pending_prompts.get(&id).and_then(VecDeque::front).is_some_and(PendingPrompt::is_submitted)
    }

    fn dispatch_prompt(&mut self, id: u64, steer: bool, cx: &mut Context<Self>) {
        let running = self.running.contains(&id);
        if running && !(steer && self.can_steer(id)) {
            return;
        }
        let Some(pending) = self.pending_prompts.get_mut(&id).and_then(VecDeque::front_mut) else {
            return;
        };
        if !matches!(pending.state, PromptState::Waiting) {
            return;
        }
        let Some(thread) = self.store.threads.iter().find(|thread| thread.id == id) else {
            return;
        };
        let turn_id = pending.turn_id.clone();
        let context = if running {
            String::new()
        } else {
            thread.handoff_context().unwrap_or_default()
        };
        let prompt = context + &pending.message.prompt();
        let machine = thread.machine.clone();
        pending.state = PromptState::Sending;
        let session = self.live_agl_session(id).map(|s| s.id.clone());
        let open = session.map_or_else(|| self.open_session(id, cx), |session| Task::ready(Ok(session)));
        let mode = if running { "steer" } else { "queue" };
        cx.spawn(async move |this, cx| {
            let result = match open.await {
                Ok(session_id) => agl_bridge::backend(machine.as_deref())
                    .prompt(&session_id, &turn_id, prompt, mode)
                    .await
                    .map_err(|err| format!("{err:#}")),
                Err(err) => Err(err),
            };
            if let Err(err) = result {
                this.update(cx, |this, cx| {
                    if let Some(pending) = this.pending_prompt_mut(id, &turn_id)
                        && matches!(pending.state, PromptState::Sending)
                    {
                        pending.state = PromptState::Failed(err);
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    fn pending_prompt_mut(&mut self, id: u64, turn_id: &str) -> Option<&mut PendingPrompt> {
        self.pending_prompts.get_mut(&id)?.front_mut().filter(|p| p.turn_id == turn_id)
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

    fn remove_pending_prompt(&mut self, id: u64, turn_id: &str, cx: &mut Context<Self>) {
        let Some(prompts) = self.pending_prompts.get_mut(&id) else { return };
        let Some(index) = prompts.iter().position(|p| p.turn_id == turn_id && !p.is_submitted()) else { return };
        if let Some(prompt) = prompts.remove(index) {
            Self::delete_pasted_images(&prompt.message.images);
        }
        if prompts.is_empty() {
            self.pending_prompts.remove(&id);
        }
        self.dispatch_prompt(id, false, cx);
        cx.notify();
    }

    fn retry_pending_prompt(&mut self, id: u64, turn_id: &str, cx: &mut Context<Self>) {
        if let Some(prompt) = self.pending_prompt_mut(id, turn_id)
            && matches!(prompt.state, PromptState::Failed(_))
        {
            prompt.state = PromptState::Waiting;
            self.dispatch_prompt(id, false, cx);
            cx.notify();
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active else { return };
        if let Some(session) = self.sessions.get(&id) {
            agl_bridge::backend(session.machine.as_deref()).abort(&session.id);
        }
        cx.notify();
    }

    fn answer(&mut self, thread_id: u64, request_id: String, decision: &str, cx: &mut Context<Self>) {
        if let Some(session) = self.sessions.get(&thread_id) {
            agl_bridge::backend(session.machine.as_deref()).approve(session.id.clone(), request_id, decision.to_string());
        }
        cx.notify();
    }

    // ---- Machines ---------------------------------------------------------

    /// The machine the composer talks to: the open thread's, or the next
    /// new thread's.
    fn context_machine(&self) -> Option<String> {
        match self.active_thread() {
            Some(thread) => thread.machine.clone(),
            None => self.machine.clone(),
        }
    }

    /// Whether `machine` can run a conversation now: its connection is up,
    /// or the device list says it is reachable.
    fn machine_online(&self, machine: &str) -> bool {
        agl_bridge::is_connected(machine)
            || self.hosts.iter().any(|h| h.id == machine && h.usable())
    }

    /// A machine's name: as the relay lists it, else as it was when a
    /// thread started there.
    fn machine_name(&self, id: &str, fallback: Option<&str>) -> String {
        self.hosts
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.name.clone())
            .or_else(|| fallback.map(str::to_string))
            .or_else(|| Some(remote::name_of(id)).filter(|name| name != id))
            .unwrap_or_else(|| "another machine".into())
    }

    fn set_hosts(&mut self, hosts: Vec<remote::Device>, cx: &mut Context<Self>) {
        let hosts: Vec<_> = hosts.into_iter().filter(|d| !d.is_self && d.shares).collect();
        let ids = |list: &[remote::Device]| {
            list.iter().map(|h| (h.id.clone(), h.name.clone(), h.usable())).collect::<Vec<_>>()
        };
        if ids(&hosts) != ids(&self.hosts) {
            // A machine that came back may list its agents now.
            for host in hosts.iter().filter(|h| h.usable()) {
                if matches!(self.agents_cache.get(&Some(host.id.clone())), Some(Err(_))) {
                    self.agents_cache.remove(&Some(host.id.clone()));
                    if self.agents_machine.as_deref() == Some(host.id.as_str()) {
                        self.agents = None;
                        self.detect_agents(Some(host.id.clone()), cx);
                    }
                }
            }
            self.hosts = hosts;
            cx.notify();
        }
    }

    /// Look the account's machines up now, e.g. right after signing in.
    pub fn refresh_hosts(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let hosts = remote::devices().await.unwrap_or_default();
            this.update(cx, |this, cx| this.set_hosts(hosts, cx)).ok();
        })
        .detach();
    }

    /// Run the next new thread on `machine`; `None` is this one.
    fn set_machine(&mut self, machine: Option<String>, cx: &mut Context<Self>) {
        // Another device has other projects.
        if machine != self.machine {
            self.shown_project = None;
        }
        self.machine = machine.clone();
        self.store.machine = machine;
        self.store.save();
        self.folders_open = false;
        self.sync_machine(cx);
        cx.notify();
    }

    /// Show the agents of the machine the composer talks to.
    fn sync_machine(&mut self, cx: &mut Context<Self>) {
        let machine = self.context_machine();
        if self.agents_machine == machine {
            return;
        }
        self.agents_machine = machine.clone();
        self.picker.update(cx, |picker, cx| picker.set_machine(machine.clone(), cx));
        match self.agents_cache.get(&machine) {
            Some(Ok(_)) => self.show_agents(cx),
            _ => {
                self.agents = None;
                self.detect_agents(machine, cx);
            }
        }
        cx.notify();
    }

    /// Whether an agent is offered for threads (Settings → Agents).
    pub fn agent_enabled(&self, agent: &str) -> bool {
        self.store.enabled_agents.iter().any(|a| a == agent)
    }

    pub fn set_agent_enabled(&mut self, agent: &str, enabled: bool, cx: &mut Context<Self>) {
        self.store.enabled_agents.retain(|a| a != agent);
        if enabled {
            self.store.enabled_agents.push(agent.to_string());
        }
        self.store.save();
        self.show_agents(cx);
    }

    /// This machine's agents, once detected (Settings → Agents).
    pub fn local_agents(&self) -> Option<&[AgentInfo]> {
        match self.agents_cache.get(&None) {
            Some(Ok(agents)) => Some(agents),
            _ => None,
        }
    }

    /// Look for this machine's agents again, after their settings changed.
    pub fn redetect_local_agents(&mut self, cx: &mut Context<Self>) {
        self.detect_agents(None, cx);
    }

    /// The arguments appended to an agent's launch arguments here.
    pub fn agent_args(&self, agent: &str) -> Vec<String> {
        self.store.agent_args.get(agent).cloned().unwrap_or_default()
    }

    pub fn set_agent_args(&mut self, agent: &str, args: Vec<String>, cx: &mut Context<Self>) {
        if args.is_empty() {
            self.store.agent_args.remove(agent);
        } else {
            self.store.agent_args.insert(agent.to_string(), args);
        }
        self.store.save();
        cx.notify();
    }

    fn detect_agents(&mut self, machine: Option<String>, cx: &mut Context<Self>) {
        let detect = agl_bridge::backend(machine.as_deref()).detect();
        cx.spawn(async move |this, cx| {
            let agents = detect.await.map_err(|err| format!("{err:#}"));
            this.update(cx, |this, cx| {
                this.agents_cache.insert(machine.clone(), agents);
                if this.agents_machine == machine {
                    this.show_agents(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Hand `agents_machine`'s detected agents to the composer and picker.
    fn show_agents(&mut self, cx: &mut Context<Self>) {
        let machine = self.agents_machine.clone();
        let agents = self.agents_cache.get(&machine).cloned();
        if let Some(Ok(agents)) = &agents {
            let enabled = &self.store.enabled_agents;
            let usable = |a: &AgentInfo| a.available && enabled.contains(&a.id);
            // The next new thread's agent must be one installed and enabled there.
            if machine == self.machine && self.agent.as_deref().is_none_or(|id| !agents.iter().any(|a| a.id == id && usable(a))) {
                self.agent = agents.iter().find(|a| usable(a)).map(|a| a.id.clone());
                self.model = None;
            }
            // Starred models of agents turned off stay starred, out of sight.
            let favorites: Vec<(String, String)> =
                self.store.favorites.iter().filter(|(a, _)| enabled.contains(a)).cloned().collect();
            let agent = match self.active_thread() {
                Some(thread) => Some(thread.agent.clone()),
                None => self.agent.clone(),
            };
            // The picker offers the enabled agents, and the open thread's
            // own even if it has been turned off since.
            let offered: Vec<AgentInfo> = agents
                .iter()
                .filter(|a| usable(a) || agent.as_deref() == Some(a.id.as_str()))
                .cloned()
                .collect();
            self.picker.update(cx, |picker, cx| {
                picker.set_agents(&offered, cx);
                picker.set_favorites(favorites, cx);
                // Load the agent's models for its label and efforts.
                if let Some(agent) = agent {
                    picker.ensure_models(&agent, cx);
                }
            });
        }
        self.agents = agents;
        cx.notify();
    }

    /// A thread another device wrote, deleted or has newer.
    fn apply_incoming(&mut self, incoming: crate::sync::Incoming, cx: &mut Context<Self>) {
        use crate::sync::Incoming;
        match incoming {
            Incoming::Put(thread) | Incoming::Mirror(thread) => {
                let id = thread.id;
                // A turn under way here owns the thread until it ends.
                if self.is_busy(id) {
                    return;
                }
                match self.store.threads.iter_mut().find(|t| t.id == id) {
                    // Another thread that happens to share the id: leave it.
                    Some(existing) if existing.machine != thread.machine => return,
                    Some(existing) => {
                        let grew = thread.items.len() > existing.items.len();
                        *existing = thread;
                        if grew && self.active != Some(id) {
                            self.unread.insert(id);
                        }
                    }
                    None => self.store.threads.push(thread),
                }
                // A device seen for the first time gets a workspace to show it in.
                self.store.ensure_workspaces(&remote::this_name());
                if self.active == Some(id) {
                    self.follow_to_bottom();
                }
                self.store.save();
                cx.notify();
            }
            Incoming::PutWorkspace(ws) => {
                match self.store.workspaces.iter_mut().find(|w| w.id == ws.id) {
                    Some(existing) => *existing = ws,
                    None => self.store.workspaces.push(ws),
                }
                self.store.save();
                cx.notify();
            }
            Incoming::Workspaces { machine, list } => {
                let mut changed = false;
                for ws in self.store.workspaces.iter_mut().filter(|w| w.device.as_deref() == Some(machine.as_str())) {
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
                    self.store.save();
                    cx.notify();
                }
            }
            Incoming::Delete(id) => {
                if self.store.threads.iter().any(|t| t.id == id && t.machine.is_none()) {
                    self.remove_thread(id, false, cx);
                }
            }
            Incoming::Resend { machine, listed } => {
                let missing: Vec<Thread> = self
                    .store
                    .threads
                    .iter()
                    .filter(|t| t.machine.as_deref() == Some(machine.as_str()) && !listed.contains(&t.id))
                    .cloned()
                    .collect();
                crate::sync::resend(&missing);
            }
            Incoming::Gone { machine, id } => {
                if self.store.threads.iter().any(|t| t.id == id && t.machine.as_deref() == Some(machine.as_str())) {
                    self.remove_thread(id, false, cx);
                }
            }
        }
    }

    fn apply_event(&mut self, id: u64, event: AgentEvent, cx: &mut Context<Self>) {
        let mut sent = None;
        let mut idle = false;
        match &event {
            AgentEvent::Status { status } => {
                // starting, running and waiting (on an approval) are all busy.
                idle = matches!(status.as_str(), "idle" | "suspended" | "closed");
                if idle {
                    if self.running.remove(&id) && !self.is_busy(id) && self.active != Some(id) {
                        self.unread.insert(id);
                    }
                    if status == "closed" {
                        self.sessions.remove(&id);
                    }
                } else {
                    self.running.insert(id);
                }
            }
            AgentEvent::UserMessage { turn_id, .. } | AgentEvent::Queued { turn_id, .. } => {
                if let Some(pending) = self.pending_prompt_mut(id, turn_id) {
                    pending.state = PromptState::Accepted {
                        queued: matches!(event, AgentEvent::Queued { .. }),
                    };
                }
            }
            AgentEvent::TurnStarted { turn_id } => {
                self.running.insert(id);
                sent = self.take_pending_prompt(id, turn_id);
            }
            AgentEvent::Steered { turn_id, .. } => {
                sent = self.take_pending_prompt(id, turn_id);
            }
            AgentEvent::TurnEnded { turn_id, error, .. } => {
                if let Some(pending) = self.pending_prompt_mut(id, turn_id) {
                    // The agent could not launch this turn. Keep the message
                    // and its attachments available for retry.
                    pending.state = PromptState::Failed(
                        error.clone().unwrap_or_else(|| t("The turn did not start").into()),
                    );
                    cx.notify();
                    return;
                }
            }
            AgentEvent::Retrying { message } => {
                self.notes.insert(id, tf("Retrying… {message}", &[("message", message.clone())]));
            }
            AgentEvent::Heartbeat { stalled: true, idle_ms, .. } => {
                self.notes
                    .insert(id, tf("No output for {n}s", &[("n", (idle_ms / 1000).to_string())]));
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
        let sent_message = sent.is_some();
        if let Some(pending) = sent {
            thread.items.push(ChatItem::User(pending.message));
        }
        thread.apply(event);
        if sent_message || turn_ended {
            self.store.save();
        }
        if turn_ended {
            self.notes.remove(&id);
            cx.emit(AgentPanelEvent::TurnFinished);
        }
        if idle {
            self.dispatch_prompt(id, false, cx);
        }
        if self.active == Some(id) {
            self.follow_to_bottom();
        }
        cx.notify();
    }

    fn on_picker_event(
        &mut self,
        _: &Entity<ModelPicker>,
        event: &ModelPickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ModelPickerEvent::Select { agent, model, effort, done } => {
                self.choose_model(agent.clone(), model.clone(), cx);
                if let Some(effort) = effort {
                    self.set_effort(effort.clone(), cx);
                }
                // Picking a model leaves the picker open for its effort;
                // picking the effort (or a model without any) closes it.
                if *done {
                    self.picker_open = false;
                    self.focus_composer(window, cx);
                }
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
            self.store.threads.iter().any(|t| t.id == *id && (!t.items.is_empty() || self.pending_prompts.contains_key(id)))
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
                if let Some(session) = self.sessions.get(&id) {
                    agl_bridge::backend(session.machine.as_deref()).configure(&session.id, None, Some(model.clone().unwrap_or_else(|| "default".into())), None);
                }
            }
            self.follow_to_bottom();
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
            if let Some(session) = self.sessions.get(&id) {
                agl_bridge::backend(session.machine.as_deref()).configure(&session.id, None, None, effort);
            }
        }
        self.store.save();
        cx.notify();
    }

    fn set_picker_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = open;
        if open {
            let (agent, model, effort) = match self.active_thread() {
                Some(thread) => (thread.agent.clone(), thread.model.clone(), thread.effort.clone()),
                None => (self.agent.clone().unwrap_or_default(), self.model.clone(), self.effort.clone()),
            };
            self.picker
                .update(cx, |picker, cx| picker.open(agent, model, effort, window, cx));
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
            if let Some(session) = self.sessions.get(&id) {
                agl_bridge::backend(session.machine.as_deref()).configure(&session.id, Some(access.to_string()), None, None);
            }
        }
        self.store.save();
        cx.notify();
    }

    /// Copy and the time, under a finished turn's answer.
    fn render_turn_actions(&self, thread_id: u64, items: &[ChatItem], turn: &Turn, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let answer = turn.visible.iter().rev().find_map(|&ix| match &items[ix] {
            ChatItem::Assistant(text) => Some((ix, without_control_lines(text).into_owned())),
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
            .text_size(TEXT)
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
        at_top: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let blocks = group_items(items, ixs);
        let is_step_block = |block: &Block| match block {
            Block::Group(_) => true,
            Block::Gallery(_) => false,
            Block::Single(ix) => is_step(&items[*ix]),
        };
        let mut previous_step: Option<bool> = None;
        let mut out = Vec::new();
        for block in blocks {
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
                Block::Gallery(ixs) => self.render_gallery(items, &ixs, cx),
                Block::Group(ixs) => {
                    let key = (thread_id, ixs[0]);
                    // Folded until opened, even while the turn runs: a long
                    // turn reads as its messages with one line per run of tools.
                    let open = self.groups.get(&key).copied().unwrap_or(false);
                    self.render_group(thread_id, items, ixs, open, cx)
                }
            };
            out.push(div().mt(gap).child(element).into_any_element());
        }
        out
    }

    /// Images generated in a row: one heading, the pictures side by side,
    /// wrapping when the panel is narrow. A click opens one full size.
    fn render_gallery(&self, items: &[ChatItem], ixs: &[usize], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let images: Vec<PathBuf> = ixs.iter().flat_map(|&ix| generated_images(&items[ix])).collect();
        let label = match images.len() {
            1 => t("Generated image").to_string(),
            n => tf("Generated {n} images", &[("n", n.to_string())]),
        };
        // Two fit side by side in a typical panel; more get smaller.
        let side = px(if images.len() <= 2 { 320. } else { 220. });
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .h(layout::CONTROL)
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_2)
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::Image).size(ICON))
                    .child(label),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(layout::SPACE_2)
                    .children(images.into_iter().enumerate().map(|(n, path)| {
                        let open = path.clone();
                        div()
                            .id(("gallery-image", ixs[0] * 64 + n))
                            .size(side)
                            .flex_none()
                            .rounded(layout::RADIUS)
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .child(img(path).size_full().object_fit(ObjectFit::Contain))
                            .on_click(move |_, _, cx| cx.open_with_system(&open))
                    })),
            )
            .into_any_element()
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
                                .child(tf("· {n} failed", &[("n", failed.to_string())])),
                        )
                    })
                    .when(running, |this| this.child(Spinner::new().with_size(crate::style::size::icon_pixels(cx))))
                    .child(
                        Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                            .size(ICON)
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
        if settled {
            self.drop_thread_tabs(id, cx);
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

    /// The interface language: `None` follows the system.
    pub fn language_preference(&self) -> Option<crate::i18n::Language> {
        crate::i18n::preference_from_setting(self.store.language.clone())
    }

    pub fn set_language(&mut self, preference: Option<crate::i18n::Language>, cx: &mut Context<Self>) {
        self.store.language = crate::i18n::preference_to_setting(preference);
        self.store.save();
        crate::i18n::apply(preference);
        crate::set_app_menus(cx);
        cx.refresh_windows();
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: String, window: &mut Window, cx: &mut Context<Self>) {
        self.store.theme = Some(theme.clone());
        self.store.save();
        crate::style::set_choice(theme, window, cx);
        cx.notify();
    }

    pub fn set_typography(&mut self, value: crate::style::Typography, cx: &mut Context<Self>) {
        self.store.typography = value.clone();
        self.store.save();
        crate::style::set_typography(value, cx);
    }

    fn new_thread_in_project(&mut self, project: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        // Another device's project is a folder there: the new thread works
        // in it on that device; this window's project stays as it is.
        if let Some(machine) = self.machine.clone() {
            if let Some(dir) = &project {
                let active = self.store.active_workspace.clone();
                if let Some(ws) = self.store.workspaces.iter_mut().find(|w| Some(&w.id) == active.as_ref()) {
                    if !ws.projects.contains(dir) {
                        ws.projects.insert(0, dir.clone());
                    }
                }
            }
            match project {
                Some(dir) => self.store.remote_dirs.insert(machine, dir),
                None => self.store.remote_dirs.remove(&machine),
            };
            self.store.save();
            self.new_thread(window, cx);
            return;
        }
        if project != self.project {
            cx.emit(AgentPanelEvent::SwitchProject(project.clone()));
            self.project = project;
            self.branch = None;
        }
        self.new_thread(window, cx);
    }

    /// A conversation with its current agent and trailing activity.
    /// Where a thread stands, for its dot in the sidebar.
    fn thread_status(&self, thread: &Thread) -> ThreadStatus {
        let running = self.running.contains(&thread.id);
        // Only what happened since the last turn ended counts.
        let last_turn = thread
            .items
            .iter()
            .rposition(|item| matches!(item, ChatItem::TurnEnd { .. }) )
            .map_or(0, |ix| ix + 1);
        let recent = &thread.items[last_turn.min(thread.items.len())..];
        if running && recent.iter().any(|item| matches!(item, ChatItem::Approval { decision: None, .. })) {
            return ThreadStatus::NeedsYou;
        }
        if running {
            return ThreadStatus::Working;
        }
        // The last thing said was an error, with no turn ended cleanly since.
        let send_failed = self.pending_prompts.get(&thread.id)
            .and_then(VecDeque::front)
            .is_some_and(|prompt| matches!(prompt.state, PromptState::Failed(_)));
        let turn_failed = thread
            .items
            .iter()
            .rev()
            .find(|item| !matches!(item, ChatItem::Plan(_) | ChatItem::TurnEnd { .. }))
            .is_some_and(|item| matches!(item, ChatItem::Error(_)));
        if send_failed || turn_failed {
            ThreadStatus::Failed
        } else if self.unread.contains(&thread.id) {
            ThreadStatus::Done
        } else {
            ThreadStatus::Idle
        }
    }

    fn render_thread_row(&self, thread: &Thread, indent: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = thread.id;
        let active = self.active == Some(id);
        let running = self.running.contains(&id);
        let settled = thread.settled;
        let open = self.is_open_in_tab(thread);
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
            .when(indent, |this| this.pr(layout::SPACE_3))
            // The status dot sits under the project's avatar, so the
            // thread itself reads as indented beneath the project.
            .child(
                div()
                    .w(avatar_size(cx))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(render_status_dot(self.thread_status(thread), id, cx)),
            )
            .child(
                div()
                    .flex_none()
                    // Closer to the dot than the row's gap, so the two read as a pair.
                    .ml(px(-4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(agent_icon(&thread.agent, crate::style::size::icon_pixels(cx), cx)),
            )
            .child(
                div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap()
                    .text_color(if active || open { theme.foreground } else { theme.foreground.opacity(0.8) })
                    // Threads open in a tab stand out from the rest.
                    .when(open, |this| this.font_weight(FontWeight::MEDIUM))
                    .child(display_title(&thread.title)),
            )
            // The sidebar shows one device at a time; only a thread from
            // another one (in a tab, say) names its device.
            .when_some(thread.machine.as_deref().filter(|m| self.machine.as_deref() != Some(*m)), |this, machine| {
                let name = self.machine_name(machine, thread.machine_name.as_deref());
                this.child(
                    div().flex_none().max_w(px(90.)).overflow_hidden().text_ellipsis().whitespace_nowrap()
                        .flex().items_center().gap(px(3.))
                        .text_size(TEXT).text_color(theme.muted_foreground)
                        .child(Icon::new(IconName::Monitor).size(ICON))
                        .child(name),
                )
            })
            .child(if running {
                render_working(cx)
            } else {
                div().flex_none().min_w(px(30.)).text_right().text_size(TEXT)
                    .text_color(theme.muted_foreground).child(relative_time(thread.updated_at))
                    .into_any_element()
            })
            // A click shows the thread in the active tab; ⌘-click or a
            // middle-click opens it in a new one, as links do in a browser.
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.modifiers().platform {
                    this.open_in_new_tab(id, window, cx);
                } else {
                    this.select_thread(id, window, cx);
                }
            }))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| {
                this.open_in_new_tab(id, window, cx);
            }))
            .context_menu({
                let panel = cx.entity().downgrade();
                move |menu, _, _| {
                    let (new_tab, settle, delete) = (panel.clone(), panel.clone(), panel.clone());
                    menu.item(crate::style::menu_item(t("Open in New Tab")).on_click(move |_, window, cx| {
                        new_tab.update(cx, |this, cx| this.open_in_new_tab(id, window, cx)).ok();
                    }))
                    .separator()
                    .item(
                        crate::style::menu_item(t(if settled { "Unarchive" } else { "Archive" })).on_click(move |_, _, cx| {
                            settle.update(cx, |this, cx| this.set_settled(id, !settled, cx)).ok();
                        }),
                    )
                    .separator()
                    .item(crate::style::menu_item(t("Delete…")).on_click(move |_, window, cx| {
                        delete.update(cx, |this, cx| this.confirm_delete(id, window, cx)).ok();
                    }))
                }
            })
            .into_any_element()
    }

    fn toggle_section(&mut self, section: SidebarSection, cx: &mut Context<Self>) {
        let expanded = match section {
            SidebarSection::Projects => &mut self.projects_expanded,
            SidebarSection::Recent => &mut self.recent_expanded,
            SidebarSection::Archived => &mut self.archived_expanded,
        };
        *expanded = !*expanded;
        cx.notify();
    }

    fn render_section_header(&self, section: SidebarSection, action: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (title, expanded) = match section {
            SidebarSection::Projects => ("Projects", self.projects_expanded),
            SidebarSection::Recent => ("Chats", self.recent_expanded),
            SidebarSection::Archived => ("Archived", self.archived_expanded),
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
                    .child(t(title))
                    .when(!expanded, |this| this.child(Icon::new(IconName::ChevronRight).size(ICON)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx))),
            )
            .when(section == SidebarSection::Projects, |this| {
                let filtering = self.shown_project.is_some();
                let filter = self.project_filter.clone();
                this.child(
                    Popover::new("project-filter")
                        .anchor(Anchor::TopRight)
                        .open(self.project_filter_open)
                        .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                            this.project_filter_open = *open;
                            if *open {
                                let projects = this.device_projects();
                                let (shown, by_name) = (this.shown_project.clone(), this.sort_repositories_by_name);
                                this.project_filter.update(cx, |filter, cx| filter.open(projects, shown, by_name, window, cx));
                            }
                            cx.notify();
                        }))
                        .p_0()
                        .trigger(
                            crate::style::chrome_button("project-filter-button")
                                .child(Icon::new(IconName::ListFilter).size(ICON))
                                // Showing one project: say so.
                                .text_color(if filtering { theme.primary } else { theme.muted_foreground })
                                .accessibility_label(t("Filter projects")),
                        )
                        .content(move |_, _, _| filter.clone()),
                )
            })
            .child(action)
            .into_any_element()
    }

    /// The projects of the device the sidebar shows, most recently active
    /// first: this one's opened folders, another's last picked folder, and
    /// the folders their threads work in.
    fn device_projects(&self) -> Vec<PathBuf> {
        let mut projects: Vec<PathBuf> = self.store.active_workspace().map(|w| w.projects.clone()).unwrap_or_default();
        let mut latest: HashMap<PathBuf, u64> = HashMap::new();
        for thread in self.store.threads.iter().filter(|t| !t.settled && self.in_active_workspace(t)) {
            if let Some(project) = &thread.project {
                let at = latest.entry(project.clone()).or_default();
                *at = (*at).max(thread.updated_at);
                if !projects.contains(project) {
                    projects.push(project.clone());
                }
            }
        }
        projects.sort_by_key(|p| std::cmp::Reverse(latest.get(p).copied().unwrap_or(0)));
        projects
    }

    /// Whether `thread` is in the workspace the window shows.
    fn in_active_workspace(&self, thread: &Thread) -> bool {
        let active = self.store.active_workspace.as_ref();
        active.is_some() && self.store.workspace_of(thread).map(|w| &w.id) == active
    }

    /// The other devices a workspace can live on, for the workspace menu.
    fn menu_devices(&self) -> Vec<MenuDevice> {
        let mut devices: Vec<MenuDevice> = self
            .hosts
            .iter()
            .map(|h| MenuDevice { id: h.id.clone(), name: h.name.clone(), online: self.machine_online(&h.id) })
            .collect();
        for w in &self.store.workspaces {
            if let Some(id) = &w.device {
                if !devices.iter().any(|d| &d.id == id) {
                    let name = self.machine_name(id, w.device_name.as_deref());
                    devices.push(MenuDevice { id: id.clone(), name, online: self.machine_online(id) });
                }
            }
        }
        devices
    }

    /// Show workspace `id`: its projects and chats, its device for new threads.
    /// Run new threads on `device`: show its workspace, made if it has none.
    fn switch_device(&mut self, device: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.store.active_workspace().filter(|w| w.device == device).map(|w| w.id.clone());
        let id = current.or_else(|| self.store.workspaces.iter().find(|w| w.device == device).map(|w| w.id.clone()));
        let id = match id {
            Some(id) => id,
            None => {
                let Some(machine) = device.clone() else { return };
                let name = self.machine_name(&machine, None);
                let ws = crate::threads::Workspace::new(name.clone(), Some("💻".into()), Some(machine.clone()), Some(name));
                let id = ws.id.clone();
                // One on another device is that device's record.
                let put = crate::sync::put_workspace(machine, ws.clone());
                cx.background_executor()
                    .spawn(async move {
                        if let Err(err) = put.await {
                            eprintln!("codeany: creating the workspace on its device failed: {err:#}");
                        }
                    })
                    .detach();
                self.store.workspaces.push(ws);
                id
            }
        };
        self.activate_workspace(id, window, cx);
    }

    fn activate_workspace(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.store.workspaces.iter().find(|w| w.id == id).cloned() else { return };
        self.store.active_workspace = Some(id);
        self.shown_project = None;
        self.set_machine(ws.device.clone(), cx);
        // Each workspace keeps its own tabs: show the one last active there.
        self.active = self.tab_set_mut().current();
        if self.active_thread().is_some_and(|t| !self.in_active_workspace(t)) {
            self.new_thread(window, cx);
        }
        self.sync_machine(cx);
        self.pin_to_bottom(cx);
        self.store.save();
        cx.notify();
    }

    fn on_workspace_menu(
        &mut self,
        _: &Entity<WorkspaceMenu>,
        event: &WorkspaceMenuEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            WorkspaceMenuEvent::Activate(id) => {
                self.workspace_menu_open = false;
                self.activate_workspace(id.clone(), window, cx);
            }
            WorkspaceMenuEvent::Create { title, icon, device } => {
                let (device, device_name) = match device {
                    Some((id, name)) => (Some(id.clone()), Some(name.clone())),
                    None => (None, None),
                };
                let ws = crate::threads::Workspace::new(title.clone(), icon.clone(), device.clone(), device_name);
                // One on another device is that device's record.
                if let Some(machine) = device {
                    let put = crate::sync::put_workspace(machine, ws.clone());
                    cx.background_executor()
                        .spawn(async move {
                            if let Err(err) = put.await {
                                eprintln!("codeany: creating the workspace on its device failed: {err:#}");
                            }
                        })
                        .detach();
                }
                let id = ws.id.clone();
                self.store.workspaces.push(ws);
                self.workspace_menu_open = false;
                self.activate_workspace(id, window, cx);
            }
            WorkspaceMenuEvent::Edit { id, title, icon } => {
                if let Some(ws) = self.store.workspaces.iter_mut().find(|w| &w.id == id) {
                    ws.title = title.clone();
                    ws.icon = icon.clone();
                    if let Some(machine) = ws.device.clone() {
                        let put = crate::sync::put_workspace(machine, ws.clone());
                        cx.background_executor().spawn(async move { put.await.ok() }).detach();
                    }
                }
                self.store.save();
                cx.notify();
            }
            WorkspaceMenuEvent::Delete(id) => {
                if !crate::threads::Workspace::deletable(&self.store.workspaces, id) {
                    return;
                }
                let Some(ws) = self.store.workspaces.iter().find(|w| &w.id == id).cloned() else { return };
                if ws.device.is_none() {
                    // Its threads and projects go to the first workspace left here.
                    for thread in self.store.threads.iter_mut().filter(|t| t.workspace.as_ref() == Some(id)) {
                        thread.workspace = None;
                    }
                    let projects = ws.projects.clone();
                    if let Some(first) = self.store.workspaces.iter_mut().find(|w| w.device.is_none() && &w.id != id) {
                        for p in projects {
                            if !first.projects.contains(&p) {
                                first.projects.push(p);
                            }
                        }
                    }
                }
                self.store.workspaces.retain(|w| &w.id != id);
                self.store.ensure_workspaces(&remote::this_name());
                if self.store.active_workspace.as_ref() == Some(id) {
                    if let Some(first) = self.store.workspaces.first().map(|w| w.id.clone()) {
                        self.activate_workspace(first, window, cx);
                    }
                }
                self.store.save();
                cx.notify();
            }
            WorkspaceMenuEvent::Mount(ws) => {
                let id = ws.id.clone();
                if !self.store.workspaces.iter().any(|w| w.id == id) {
                    self.store.workspaces.push(ws.clone());
                }
                self.workspace_menu_open = false;
                self.activate_workspace(id, window, cx);
            }
            WorkspaceMenuEvent::OpenSettings => {
                self.workspace_menu_open = false;
                cx.emit(AgentPanelEvent::OpenSettings);
                cx.notify();
            }
        }
    }

    /// The workspace menu's button: the workspace shown, its device when it
    /// is another one.
    pub fn render_workspace_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.store.active_workspace().cloned();
        let title = ws.as_ref().map(|w| w.title.clone()).unwrap_or_else(remote::this_name);
        let icon = ws.as_ref().and_then(|w| w.icon.clone());
        let menu = self.workspace_menu.clone();
        Popover::new("workspace-menu")
            .anchor(Anchor::TopLeft)
            .open(self.workspace_menu_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.workspace_menu_open = *open;
                if *open {
                    let (workspaces, active, devices) =
                        (this.store.workspaces.clone(), this.store.active_workspace.clone(), this.menu_devices());
                    this.workspace_menu.update(cx, |menu, cx| menu.open(workspaces, active, devices, cx));
                }
                cx.notify();
            }))
            .p_0()
            .trigger(
                Button::new("workspace-switcher")
                    .ghost()
                    .w_full()
                    .h(layout::CONTROL)
                    .px(layout::SPACE_2)
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(layout::SPACE_2)
                            .text_size(TEXT)
                            .child(crate::workspace_switcher::avatar(icon.as_deref(), &title, px(22.), cx))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(div().flex_1())
                            .child(Icon::new(IconName::ChevronDown).size(ICON).text_color(theme.muted_foreground)),
                    ),
            )
            .content(move |_, _, _| menu.clone())
            .into_any_element()
    }

    pub fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();

        // Threads, as one list per project.
        enum Entry<'a> {
            Thread(&'a Thread),
        }
        impl Entry<'_> {
            fn updated(&self) -> u64 {
                match self {
                    Entry::Thread(t) => t.updated_at,
                }
            }
        }
        // Ordered, so equal timestamps never reshuffle from one frame to the next.
        let mut groups: std::collections::BTreeMap<Option<PathBuf>, Vec<Entry>> = Default::default();
        // One workspace at a time, picked at the top: its projects and chats.
        for thread in self.store.threads.iter().filter(|t| !t.settled && self.in_active_workspace(t)) {
            groups.entry(thread.project.clone()).or_default().push(Entry::Thread(thread));
        }
        for entries in groups.values_mut() {
            entries.sort_by(|a, b| {
                let key = |e: &Entry| match e {
                    Entry::Thread(t) => t.id.to_string(),
                };
                b.updated().cmp(&a.updated()).then_with(|| key(a).cmp(&key(b)))
            });
        }
        let icon_button = |id: &'static str, icon: IconName| {
            crate::style::chrome_button(id)
                .child(Icon::new(icon).size(ICON))
                .text_color(theme.muted_foreground)
        };

        // ---- Projects, most recently active first; the open one leads.
        // The workspace's own, and the folders its threads work in.
        let mut projects: Vec<PathBuf> = self.store.active_workspace().map(|w| w.projects.clone()).unwrap_or_default();
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
        // By last activity, so opening a project never moves it. The path
        // breaks ties: without it the list reorders on every hover.
        projects.sort_by(|a, b| {
            let rank = |p: &PathBuf| std::cmp::Reverse(activity(p).max(u64::from(self.store.projects.contains(p))));
            if self.sort_repositories_by_name {
                a.file_name().cmp(&b.file_name()).then_with(|| a.cmp(b))
            } else {
                rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
            }
        });
        // One project picked in the filter menu: only it, opened.
        let filtered = self.shown_project.clone();
        if let Some(shown) = &filtered {
            projects.retain(|p| p == shown);
            if projects.is_empty() {
                projects.push(shown.clone());
            }
        }
        let hidden_projects = if self.all_projects || filtered.is_some() {
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
            .flex_col()
            ;

        // The entries of one group, capped unless asked for all.
        let push_entries = |this: &Self,
                                list: Stateful<Div>,
                                key: Option<PathBuf>,
                                entries: &[Entry],
                                indent: bool,
                                cx: &mut Context<Self>|
         -> Stateful<Div> {
            let all = this.long_projects.contains(&key);
            let shown = if all { entries.len() } else { entries.len().min(if indent { 3 } else { THREADS_PER_PROJECT }) };
            let mut list = list.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(entries[..shown].iter().map(|entry| match entry {
                        Entry::Thread(t) => this.render_thread_row(t, indent, cx),
                    })),
            );
            let more = entries.len() - shown;
            if more > 0 {
                list = list.child(
                    div()
                        .id(SharedString::from(format!("more-{key:?}")))
                        .h(layout::CONTROL)
                        .pl(layout::SIDEBAR_ROW_INSET + avatar_size(cx) + layout::SPACE_2)
                        .flex()
                        .items_center()
                        .text_size(TEXT)
                        .text_color(theme.muted_foreground)
                        .cursor_pointer()
                        .hover(|this| this.text_color(theme.foreground))
                        .child(tf("More ({n})", &[("n", more.to_string())]))
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
                icon_button("add-project", IconName::FolderPlus)
                    .on_click(cx.listener(|this, _, window, cx| match this.machine.clone() {
                        None => cx.emit(AgentPanelEvent::OpenFolder),
                        // A folder on that device: pick it for a new thread.
                        Some(machine) => {
                            this.new_thread(window, cx);
                            let dir = this.store.remote_dirs.get(&machine).cloned();
                            this.folders.update(cx, |folders, cx| folders.open(machine, dir, cx));
                            this.folders_open = true;
                            cx.notify();
                        }
                    }))
                    .app_tooltip(t("Open Folder (⌘O)"))
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
                            .child(t("Open a folder to start a project")),
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
                    let expanded = filtered.is_some() || self.expanded_projects.contains(project);
                    let current = self.project.as_ref() == Some(project);
                    let live = entries.iter().any(|e| match e {
                        Entry::Thread(t) => self.running.contains(&t.id),
                    });
                    let count = entries.len();
                    let path = project.clone();
                    let new_thread_path = path.clone();
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("project-{}", project.display())))
                            .group("project")
                            .mt(layout::SPACE_1)
                            .h(layout::SIDEBAR_NAV_ROW)
                            .flex_none()
                            .pl(layout::SIDEBAR_ROW_INSET)
                            .pr(layout::SPACE_2)
                            .flex()
                            .items_center()
                            .gap(layout::SPACE_2)
                            .rounded(layout::RADIUS)
                            .cursor_pointer()
                            .text_size(TEXT)
                            .when(current && self.active.is_none(), |this| this.bg(theme.list_active))
                            .hover(|this| this.bg(theme.list_hover))
                            .child(ProjectAvatar {
                                project: project.clone(),
                                name: name.clone(),
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_color(theme.foreground)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(name),
                            )
                            .when(live, |this| {
                                this.child(div().flex_none().size(px(6.)).rounded_full().bg(theme.success))
                            })
                            .child(
                                div()
                                    .relative()
                                    .size(layout::CONTROL)
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(count > 0 && !expanded, |this| {
                                        this.child(
                                            div().id("project-thread-count")
                                                .text_color(theme.muted_foreground)
                                                .group_hover("project", |this| this.invisible())
                                                .child(count.to_string()),
                                        )
                                    })
                                    .child(
                                        div().id("project-new-thread")
                                            .absolute().inset_0()
                                            .invisible()
                                            .group_hover("project", |this| this.visible())
                                            .child(
                                                icon_button("new-project-thread", IconName::SquarePen)
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.new_thread_in_project(Some(new_thread_path.clone()), window, cx);
                                                    }))
                                                    .app_tooltip(t("New Thread")),
                                            ),
                                    ),
                            )
                            .child(
                                Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                    .size(ICON)
                                    .flex_none()
                                    .text_color(theme.muted_foreground.opacity(0.7)),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.expanded_projects.insert(path.clone()) {
                                    this.expanded_projects.remove(&path);
                                }
                                cx.notify();
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
                            .child(tf("More projects ({n})", &[("n", hidden_projects.to_string())]))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.all_projects = true;
                                cx.notify();
                            })),
                    );
                }
            }
        }

        // ---- Conversations without a repository (not while one project is shown)
        if filtered.is_none() && groups.get(&None).is_some_and(|entries| !entries.is_empty()) {
            list = list.child(self.render_section_header(
                SidebarSection::Recent,
                icon_button("new-chat", IconName::SquarePen)
                    .on_click(cx.listener(|this, _, window, cx| this.new_thread_in_project(None, window, cx)))
                    .app_tooltip(t("New Chat"))
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
                            .child(t("No chats yet")),
                    );
                }
                list = push_entries(self, list, None, entries, false, cx);
            }
        }

        // ---- Archived threads: out of the lists above, kept until deleted.
        let mut archived: Vec<Entry> =
            self.store
                .threads
                .iter()
                .filter(|t| t.settled && self.in_active_workspace(t))
                .filter(|t| filtered.is_none() || t.project == filtered)
                .map(Entry::Thread)
                .collect();
        if !archived.is_empty() {
            archived.sort_by(|a, b| b.updated().cmp(&a.updated()));
            let count = div()
                .text_size(TEXT)
                .text_color(theme.muted_foreground.opacity(0.7))
                .child(archived.len().to_string())
                .into_any_element();
            list = list.child(self.render_section_header(SidebarSection::Archived, count, cx));
            if self.archived_expanded {
                list = push_entries(self, list, Some(PathBuf::from("archived:")), &archived, false, cx);
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
        let Some(id) = self
            .active_thread()
            .filter(|t| !t.items.is_empty() || self.pending_prompts.contains_key(&t.id))
            .map(|t| t.id)
        else {
            return self.render_empty(cx);
        };
        self.sync_chat_rows(id);

        // Only the rows in view (and a margin) are laid out; the rest keep
        // the height they were last measured at.
        let rows = list(
            self.chat_list.clone(),
            // Each row is laid out on its own, so `mx_auto` inside has no
            // parent to center against: a full-width row centers it.
            cx.processor(|this: &mut Self, ix: usize, _window, cx| {
                div().w_full().flex().justify_center().child(this.render_chat_row(ix, cx)).into_any_element()
            }),
        )
        .size_full();

        div()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(theme.background)
            .child(div().id("messages").flex_1().min_h_0().min_w_0().child(rows))
            .when(!self.follow, |this| {
                // Back to the newest message, and following it again.
                this.child(
                    div().w_full().flex_none().flex().justify_center().h(px(0.)).child(
                        div()
                            .id("jump-to-latest")
                            .mt(px(-44.))
                            .h(px(30.))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap(layout::SPACE_1)
                            .rounded_full()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.popover)
                            .shadow_sm()
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground)
                            .cursor_pointer()
                            .hover(|this| this.text_color(theme.foreground))
                            .child(Icon::new(IconName::ArrowDown).size(ICON))
                            .child(t("Latest"))
                            .on_click(cx.listener(|this, _, _, cx| this.pin_to_bottom(cx))),
                    ),
                )
            })
            .child(
                div()
                    .w_full()
                    .max_w(layout::CONTENT_MAX)
                    .mx_auto()
                    .px(layout::SPACE_6)
                    .pb(layout::SPACE_4)
                    .flex_none()
                    .child(self.render_composer(self.active_thread(), cx)),
            )
            .into_any_element()
    }

    /// Bring the transcript's rows up to date with thread `id`: one per
    /// turn, then the "Working…" line while it runs, then a bottom margin.
    /// Rows whose signature changed are measured again; the others keep
    /// their height, so a growing reply touches one row.
    fn sync_chat_rows(&mut self, id: u64) {
        let Some((turns, sigs)) = self.chat_rows(id) else { return };
        if self.chat_thread != Some(id) {
            self.chat_thread = Some(id);
            self.chat_list.reset(sigs.len());
            if self.follow {
                self.chat_list.set_follow_mode(FollowMode::Tail);
            }
        } else {
            let old = self.chat_sigs.len();
            let shared = old.min(sigs.len());
            for ix in (0..shared).filter(|&ix| self.chat_sigs[ix] != sigs[ix]) {
                self.chat_list.splice(ix..ix + 1, 1);
            }
            if old != sigs.len() {
                self.chat_list.splice(shared..old, sigs.len() - shared);
            }
        }
        self.chat_turns = turns;
        self.chat_sigs = sigs;
    }

    fn chat_rows(&self, id: u64) -> Option<(Vec<Turn>, Vec<u64>)> {
        use std::hash::{Hash as _, Hasher as _};
        let thread = self.store.threads.iter().find(|t| t.id == id)?;
        let running = self.running.contains(&id);
        let turns = split_turns(&thread.items);
        let last = turns.len().saturating_sub(1);
        let mut sigs: Vec<u64> = turns
            .iter()
            .enumerate()
            .map(|(n, turn)| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                let live = n == last && running && !turn.finished;
                (n, turn.user, live, turn.finished, turn.ms).hash(&mut h);
                for &ix in turn.user.iter().chain(&turn.work).chain(&turn.visible) {
                    let item = &thread.items[ix];
                    (ix, std::mem::discriminant(item), crate::threads::item_weight(item)).hash(&mut h);
                    let key = (id, ix);
                    (
                        self.expanded.contains(&key),
                        self.groups.get(&key),
                        self.worked.contains(&key),
                        self.copied == Some(key),
                    )
                        .hash(&mut h);
                }
                h.finish()
            })
            .collect();
        if running {
            sigs.push(u64::MAX - 1);
        }
        sigs.push(u64::MAX);
        Some((turns, sigs))
    }

    /// One row of the transcript: a turn, the "Working…" line, or the
    /// bottom margin.
    fn render_chat_row(&self, n: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut row = div()
            .w_full()
            .min_w_0()
            .max_w(layout::CONTENT_MAX)
            .mx_auto()
            .px(layout::SPACE_6)
            .flex()
            .flex_col();
        let Some(thread) = self.active_thread() else { return row.into_any_element() };
        let running = self.running.contains(&thread.id);
        let Some(turn) = self.chat_turns.get(n) else {
            if running && n == self.chat_turns.len() {
                return row
                    .child(
                        div()
                            .mt(px(16.))
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground)
                            .child(
                                ShimmerText::new(
                                    self.notes
                                        .get(&thread.id)
                                        .cloned()
                                        .unwrap_or_else(|| t("Working…").into()),
                                )
                                .id("working"),
                            ),
                    )
                    .into_any_element();
            }
            return row.h(layout::SPACE_6).into_any_element();
        };
        if n == 0 {
            row = row.pt(layout::SPACE_6);
        }
        let mut first = n == 0;
        if let Some(user) = turn.user {
            row = row.child(
                div()
                    .when(!first, |this| this.mt(px(40.)))
                    .child(self.render_item(thread.id, user, &thread.items[user], cx)),
            );
            first = false;
        }
        let live = n + 1 == self.chat_turns.len() && running && !turn.finished;
        let has_work = turn.work.iter().any(|&ix| !is_hidden(&thread.items[ix]));
        if live || !has_work {
            // In progress: everything in order, the newest steps open.
            let mut ixs: Vec<usize> = turn.work.iter().chain(&turn.visible).copied().collect();
            ixs.sort_unstable();
            row = row.children(self.render_blocks(thread.id, &thread.items, &ixs, first, cx));
        } else {
            // Done: the work folds under "Worked for …"; the answer stays.
            let key = (thread.id, turn.work[0]);
            let open = self.worked.contains(&key);
            row = row.child(
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
                row = row.children(self.render_blocks(thread.id, &thread.items, &turn.work, false, cx));
            }
            row = row.children(self.render_blocks(thread.id, &thread.items, &turn.visible, false, cx));
        }
        if turn.finished && !live {
            row = row.child(self.render_turn_actions(thread.id, &thread.items, turn, cx));
        }
        row.into_any_element()
    }

    /// A new thread: one heading and the composer; the project and the rest
    /// are picked in the bar under it.
    fn render_empty(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
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
                    .text_size(rems(2.25))
                    .line_height(rems(2.75))
                    .font_weight(FontWeight::MEDIUM)
                    .text_center()
                    .child(t("What should we work on?")),
            )
            .child(
                div()
                    .mt(px(28.))
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
            ChatItem::User(message) => div()
                .flex()
                .flex_col()
                .items_end()
                .gap(layout::SPACE_2)
                .when(!message.images.is_empty(), |this| {
                    this.child(
                        div()
                            .max_w(relative(0.82))
                            .flex()
                            .flex_wrap()
                            .justify_end()
                            .gap(layout::SPACE_2)
                            .children(message.images.iter().enumerate().map(|(n, path)| {
                                let open_path = path.clone();
                                div()
                                    .id(("user-image", ix * 64 + n))
                                    .rounded(layout::RADIUS)
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(theme.border)
                                    .cursor_pointer()
                                    .child(img(path.clone()).max_w(px(240.)).max_h(px(180.)))
                                    .on_click(move |_, _, cx| cx.open_with_system(&open_path))
                            })),
                    )
                })
                .when(!message.text.is_empty(), |this| {
                    this.child(
                        div()
                            .max_w(relative(0.82))
                            .px(layout::SPACE_4)
                            .py(layout::SPACE_3)
                            .rounded(layout::BUBBLE_RADIUS)
                            .bg(theme.secondary)
                            .text_size(TEXT)
                            .line_height(relative(1.6))
                            .child(message.text.clone()),
                    )
                })
                .into_any_element(),
            // Saved before errors like this were caught when the turn ended.
            ChatItem::Assistant(text) if crate::threads::provider_error(text).is_some() => {
                let message = crate::threads::provider_error(text).unwrap_or_default();
                self.render_item(thread_id, ix, &ChatItem::Error(message), cx)
            }
            ChatItem::Assistant(text) => {
                let text = without_control_lines(text);
                if text.trim().is_empty() {
                    return div().into_any_element();
                }
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(TEXT)
                    .line_height(relative(1.6))
                    .child(crate::style::markdown(("msg", ix), text.into_owned(), cx).selectable(true))
                    .into_any_element()
            }
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
                            .child(div().flex_none().child(t("Thought")))
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
                let on = self.store.threads.iter().find(|t| t.id == thread_id);
                // Files on another machine are not ones to open here.
                let remote = on.is_some_and(|t| t.machine.is_some());
                let base = match on {
                    Some(t) if remote => t.project.clone(),
                    _ => self.project.clone(),
                };
                let (icon, text) = tool_line(title, detail.as_deref(), running, base.as_deref());
                let has_output = output.as_ref().is_some_and(|o| !o.trim().is_empty())
                    || code.is_some()
                    || !images.is_empty();
                let path = path.clone().filter(|_| !remote);
                let generated = title == "Generate image";
                let images = if generated { generated_images(item) } else { images.clone() };
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
                            .when(running, |this| this.child(Spinner::new().with_size(crate::style::size::icon_pixels(cx))))
                            .when(*status == ToolStatus::Failed, |this| {
                                // A command that exits non-zero is usually an
                                // agent looking around (`git status` outside a
                                // repo, a glob with no match): worth a note, not
                                // an alarm. Other tools failing is.
                                let command = title == "Run";
                                this.child(
                                    div()
                                        .flex_none()
                                        .text_size(TEXT)
                                        .text_color(if command { theme.muted_foreground } else { theme.danger })
                                        .child(if command { t("non-zero exit") } else { t("failed") }),
                                )
                            }),
                    )
                    .when(expanded && has_output, |this| {
                        this.child(
                            div()
                                .id(("tool-output", ix))
                                .my_1()
                                .max_h(px(260.))
                                .overflow_y_scroll()
                                .px_3()
                                .py_2()
                                .rounded(layout::RADIUS)
                                // Detail, not content: a faint panel, flush with the line above.
                                .bg(theme.secondary.opacity(0.45))
                                .border_1()
                                .border_color(theme.border.opacity(0.5))
                                .font_family(theme.mono_font_family.clone())
                                .text_size(TEXT)
                                .line_height(relative(1.5))
                                .text_color(theme.foreground.opacity(0.75))
                                .when_some(code.clone(), |this, code| {
                                    this.child(div().text_color(theme.foreground).child(code))
                                        .when(output.is_some(), |this| {
                                            this.child(div().my_2().h(px(1.)).bg(theme.border))
                                        })
                                })
                                // Output ends with a newline; drawn as is, it leaves a blank line.
                                .child(output.as_deref().unwrap_or_default().trim_end().to_string()),
                        )
                    })
                    .when((expanded || generated) && !images.is_empty(), |this| {
                        this.child(
                            div()
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
                        "deny" => (t("Denied"), theme.danger),
                        _ => (t("Allowed"), theme.success),
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
                                    tf("Allow {title}?", &[("title", title.clone())])
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
                                    .text_size(TEXT)
                                    .child(detail),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .gap(layout::SPACE_2)
                                .child(answer_button("approve", t("Allow"), "allow", true))
                                .when(options.iter().any(|o| o == "allow_session"), |this| {
                                    this.child(answer_button(
                                        "approve-session",
                                        t("Allow for session"),
                                        "allow_session",
                                        false,
                                    ))
                                })
                                .child(answer_button("deny", t("Deny"), "deny", false)),
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
                        .text_size(TEXT)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(IconName::ListTodo).size(ICON))
                        .child(tf(
                            "Plan · {done}/{total}",
                            &[
                                ("done", entries.iter().filter(|e| e.status == "completed").count().to_string()),
                                ("total", entries.len().to_string()),
                            ],
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
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(IconName::ArrowLeftRight).size(ICON))
                            .child(t(if from_agent == to_agent { "Model switch" } else { "Context handoff" }))
                            .child(div().w(px(2.)))
                            .child(agent_icon(from_agent, crate::style::size::icon_pixels(cx), cx))
                            .child(div().text_color(theme.foreground).child(from_label.clone()))
                            .child(Icon::new(IconName::ArrowRight).size(ICON))
                            .child(agent_icon(to_agent, crate::style::size::icon_pixels(cx), cx))
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
                .w_full()
                .min_w_0()
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
                // Long errors wrap inside the box rather than run past it.
                .child(div().flex_1().min_w_0().overflow_hidden().child(display_error(text)))
                .into_any_element(),
        }
    }

    /// Where the agent runs: this machine or another of the account's. A
    /// thread stays on the machine it started on.
    /// A setting of the composer's context bar that cannot change here.
    fn context_label(
        &self,
        id: &'static str,
        icon: IconName,
        label: impl Into<SharedString>,
        _tooltip: Option<String>,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        div()
            .id(id)
            .h(layout::CONTROL)
            .px_2()
            .flex()
            .items_center()
            .gap(layout::SPACE_2)
            .text_size(TEXT)
            .text_color(theme.muted_foreground)
            .child(Icon::new(icon).size(ICON))
            .child(label.into())
            .into_any_element()
    }

    /// A setting of the composer's context bar, as a menu button.
    fn context_button(id: &'static str, icon: IconName, label: impl Into<SharedString>, cx: &App) -> Button {
        Button::new(id)
            .ghost()
            .small()
            .h(layout::CONTROL)
            .app_icon(icon)
            .child(div().text_size(TEXT).child(label.into()))
            .text_size(TEXT)
            .text_color(cx.theme().muted_foreground)
    }

    /// The machine the agent runs on: fixed for a thread, a choice for a new one.
    fn render_machine_picker(&self, thread: Option<&Thread>, cx: &mut Context<Self>) -> AnyElement {
        if let Some(thread) = thread {
            let Some(machine) = thread.machine.as_deref() else {
                return self.context_label("machine", IconName::Laptop, remote::this_name(), None, cx);
            };
            let name = self.machine_name(machine, thread.machine_name.as_deref());
            let online = self.machine_online(machine);
            let label = if online { name } else { tf("{name} · offline", &[("name", name.clone())]) };
            return self.context_label("machine", IconName::Monitor, label, None, cx);
        }
        let current = self.machine.clone();
        let (icon, label) = match &current {
            Some(id) => (IconName::Monitor, self.machine_name(id, None)),
            None => (IconName::Laptop, remote::this_name()),
        };
        // The workspace decides the device: pick another workspace to run
        // somewhere else.
        if !self.store.workspaces.is_empty() {
            let label = match &current {
                Some(id) if !self.machine_online(id) => tf("{name} · offline", &[("name", label.clone())]),
                _ => label,
            };
            // Another device is another workspace: picking one switches to it.
            let mut devices: Vec<(Option<String>, String, bool)> = vec![(None, remote::this_name(), true)];
            for w in self.store.workspaces.iter().filter(|w| w.device.is_some()) {
                let id = w.device.clone().unwrap();
                if !devices.iter().any(|(d, _, _)| d.as_deref() == Some(id.as_str())) {
                    devices.push((Some(id.clone()), self.machine_name(&id, w.device_name.as_deref()), self.machine_online(&id)));
                }
            }
            for h in &self.hosts {
                if !devices.iter().any(|(d, _, _)| d.as_deref() == Some(h.id.as_str())) {
                    devices.push((Some(h.id.clone()), h.name.clone(), h.usable()));
                }
            }
            if devices.len() < 2 {
                let tip = t("Connect your other devices in Settings → Devices to run agents there").to_string();
                return self.context_label("machine", icon, label, Some(tip), cx);
            }
            let entity = cx.entity().downgrade();
            return Self::context_button("machine-picker", icon, label, cx)
                .dropdown_menu(move |mut menu, _, _| {
                    for (ix, (id, name, online)) in devices.iter().enumerate() {
                        if ix == 1 {
                            menu = menu.separator();
                        }
                        let entity = entity.clone();
                        let pick = id.clone();
                        let label = if id.is_none() {
                            tf("This machine ({name})", &[("name", name.clone())])
                        } else if *online {
                            name.clone().into()
                        } else {
                            tf("{name} (offline)", &[("name", name.clone())])
                        };
                        menu = menu.item(
                            crate::style::menu_item(label)
                                .checked(current == *id)
                                .on_click(move |_, window, cx| {
                                    let pick = pick.clone();
                                    entity.update(cx, |this, cx| this.switch_device(pick, window, cx)).ok();
                                }),
                        );
                    }
                    menu
                })
                .into_any_element();
        }
        if self.hosts.is_empty() && current.is_none() {
            let tip = t("Connect your other devices in Settings → Devices to run agents there").to_string();
            return self.context_label("machine", icon, label, Some(tip), cx);
        }
        let hosts: Vec<(String, String, bool)> =
            self.hosts.iter().map(|h| (h.id.clone(), h.name.clone(), h.usable())).collect();
        let entity = cx.entity().downgrade();
        Self::context_button("machine-picker", icon, label, cx)
            .dropdown_menu(move |mut menu, _, _| {
                let local = entity.clone();
                menu = menu.item(
                    crate::style::menu_item(tf("This machine ({name})", &[("name", remote::this_name())]))
                        .checked(current.is_none())
                        .on_click(move |_, _, cx| {
                            local.update(cx, |this, cx| this.set_machine(None, cx)).ok();
                        }),
                );
                if !hosts.is_empty() {
                    menu = menu.separator();
                }
                for (id, name, online) in &hosts {
                    let entity = entity.clone();
                    let pick = id.clone();
                    let label = if *online { name.clone() } else { tf("{name} (offline)", &[("name", name.clone())]) };
                    menu = menu.item(
                        crate::style::menu_item(label)
                            .checked(current.as_deref() == Some(id.as_str()))
                            .disabled(!online)
                            .on_click(move |_, _, cx| {
                                let pick = pick.clone();
                                entity.update(cx, |this, cx| this.set_machine(Some(pick), cx)).ok();
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    /// The project on this machine: fixed for a thread, a choice for a new one.
    fn render_project_picker(&self, thread: Option<&Thread>, cx: &mut Context<Self>) -> AnyElement {
        let name = |p: &PathBuf| {
            p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
        };
        let current = match thread {
            Some(thread) => thread.project.clone(),
            None => self.project.clone(),
        };
        let label = current.as_ref().map(name).unwrap_or_else(|| t("No project").into());
        if thread.is_some() {
            let tip = current.as_ref().map(|p| p.display().to_string());
            return self.context_label("project", IconName::Folder, label, tip, cx);
        }
        let recent: Vec<PathBuf> = self.store.projects.iter().take(8).cloned().collect();
        let entity = cx.entity().downgrade();
        Self::context_button("project-picker", IconName::Folder, label, cx)
            .dropdown_menu(move |mut menu, _, _| {
                for project in &recent {
                    let entity = entity.clone();
                    let pick = project.clone();
                    menu = menu.item(
                        crate::style::menu_item(name(project))
                            .checked(current.as_ref() == Some(project))
                            .on_click(move |_, _, cx| {
                                let pick = pick.clone();
                                entity.update(cx, |_, cx| cx.emit(AgentPanelEvent::SwitchProject(Some(pick)))).ok();
                            }),
                    );
                }
                if !recent.is_empty() {
                    menu = menu.separator();
                }
                let open = entity.clone();
                menu = menu.item(crate::style::menu_item(t("Open Folder…")).on_click(move |_, _, cx| {
                    open.update(cx, |_, cx| cx.emit(AgentPanelEvent::OpenFolder)).ok();
                }));
                if current.is_some() {
                    let none = entity.clone();
                    menu = menu.item(crate::style::menu_item(t("No project")).on_click(move |_, _, cx| {
                        none.update(cx, |_, cx| cx.emit(AgentPanelEvent::SwitchProject(None))).ok();
                    }));
                }
                menu
            })
            .into_any_element()
    }

    /// Where the agent edits: the project folder itself, or a git worktree
    /// of it on a branch of the thread's own.
    fn render_workspace_picker(&self, thread: Option<&Thread>, cx: &mut Context<Self>) -> AnyElement {
        if let Some(thread) = thread {
            return match &thread.worktree {
                Some(worktree) => self.context_label(
                    "workspace",
                    IconName::FolderGit2,
                    t("Worktree"),
                    Some(worktree.path.display().to_string()),
                    cx,
                ),
                None => self.context_label("workspace", IconName::FolderGit2, t("Local"), None, cx),
            };
        }
        // Only a git repository can have worktrees.
        let git = self.branch.is_some();
        let worktree = self.use_worktree && git;
        let label = t(if worktree { "New worktree" } else { "Local" });
        let entity = cx.entity().downgrade();
        Self::context_button("workspace-picker", IconName::FolderGit2, label, cx)
            .dropdown_menu(move |menu, _, _| {
                let local = entity.clone();
                let new = entity.clone();
                menu.item(crate::style::menu_item(t("Local")).checked(!worktree).on_click(move |_, _, cx| {
                    local.update(cx, |this, cx| this.set_use_worktree(false, cx)).ok();
                }))
                .item(
                    crate::style::menu_item(t(if git { "New worktree" } else { "New worktree (needs git)" }))
                        .checked(worktree)
                        .disabled(!git)
                        .on_click(move |_, _, cx| {
                            new.update(cx, |this, cx| this.set_use_worktree(true, cx)).ok();
                        }),
                )
            })
            .into_any_element()
    }

    fn set_use_worktree(&mut self, on: bool, cx: &mut Context<Self>) {
        self.use_worktree = on;
        cx.notify();
    }

    /// The worktree folder the active thread works in, once it exists; the
    /// workspace shows it in place of the project.
    pub fn work_dir(&self) -> Option<PathBuf> {
        let thread = self.active_thread().filter(|t| t.machine.is_none())?;
        let dir = thread.worktree.as_ref()?.dir_for(thread.project.as_deref()?);
        dir.exists().then_some(dir)
    }

    /// The folder on another machine a new thread works in.
    fn render_folder_picker(&self, thread: Option<&Thread>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let (machine, dir) = match thread {
            Some(thread) => (thread.machine.clone()?, thread.project.clone()),
            None => {
                let machine = self.machine.clone()?;
                let dir = self.store.remote_dirs.get(&machine).cloned();
                (machine, dir)
            }
        };
        let label = dir
            .as_ref()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| t("No project").into());
        let full = dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| t("No project").into());
        if thread.is_some() {
            return Some(
                div()
                    .id("remote-folder")
                    .px_2()
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_2)
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::Folder).size(ICON))
                    .child(label)
                    .into_any_element(),
            );
        }
        let folders = self.folders.clone();
        Some(
            Popover::new("remote-folder-picker")
                .anchor(Anchor::BottomLeft)
                .open(self.folders_open)
                .on_open_change(cx.listener(move |this, open: &bool, _, cx| {
                    this.folders_open = *open;
                    if *open {
                        let dir = this.store.remote_dirs.get(&machine).cloned();
                        let machine = machine.clone();
                        this.folders.update(cx, |folders, cx| folders.open(machine, dir, cx));
                    }
                    cx.notify();
                }))
                .p_0()
                .trigger(
                    Button::new("remote-folder-button")
                        .ghost()
                        .small()
                        .h(layout::CONTROL)
                        .app_icon(IconName::Folder)
                        .child(div().text_size(TEXT).child(label))
                        .text_size(TEXT)
                        .text_color(theme.muted_foreground)
                        .app_tooltip(full),
                )
                .content(move |_, _, _| folders.clone())
                .into_any_element(),
        )
    }

    fn render_composer(&self, thread: Option<&Thread>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let running = thread.is_some_and(|t| self.running.contains(&t.id));
        let will_queue = thread.is_some_and(|t| self.is_busy(t.id) || self.pending_prompts.contains_key(&t.id));
        let has_text = !self.composer.read(cx).value().trim().is_empty()
            || !self.attachments.is_empty();
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
                .child(t("Detecting agents…"))
                .into_any_element(),
            (Some(Err(err)), _) => div()
                .px_2()
                .text_size(TEXT)
                .text_color(theme.danger)
                .child(tf("Agents unavailable: {error}", &[("error", err.to_string())]))
                .into_any_element(),
            (Some(Ok(_)), None) => div()
                .px_2()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(t("No agent installed"))
                .into_any_element(),
            (Some(Ok(_)), Some(agent)) => {
                let label = self.model_label(&agent, model.as_deref(), cx);
                // The effort rides along, where the model offers a choice.
                let effort_label = (!self.model_efforts(&agent, model.as_deref(), cx).is_empty())
                    .then(|| crate::model_picker::effort_label(effort.as_deref()));
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
                                    .child(agent_icon(&agent, crate::style::size::icon_pixels(cx), cx))
                                    .child(label)
                                    .when_some(effort_label, |this, effort| {
                                        this.child(div().text_color(theme.muted_foreground).child(effort))
                                    })
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

        // The context bar under the box: where the agent runs, and how.
        let remote = match thread {
            Some(thread) => thread.machine.is_some(),
            None => self.machine.is_some(),
        };
        let machine_picker = self.render_machine_picker(thread, cx);
        let project_picker = if remote {
            self.render_folder_picker(thread, cx)
        } else {
            Some(self.render_project_picker(thread, cx))
        };
        let local_project = !remote && thread.map_or(self.project.is_some(), |t| t.project.is_some());
        let workspace_picker = local_project.then(|| self.render_workspace_picker(thread, cx));
        // A worktree thread shows its own branch, even before it is checked out.
        let branch = thread
            .and_then(|t| t.worktree.as_ref())
            .map(|w| w.branch.clone())
            .or_else(|| self.branch.clone())
            .filter(|_| local_project);
        let branch = branch.map(|branch| {
            self.context_label("branch", IconName::GitBranch, branch, Some(t("Current branch").into()), cx)
        });
        let can_attach = !remote;

        let access = self.access.clone();
        let access_picker = Button::new("access-picker")
            .ghost()
            .small().h(layout::CONTROL)
            .app_icon(match access.as_str() {
                    "edit" => IconName::FilePen,
                    "ask" => IconName::ShieldQuestionMark,
                    "read_only" => IconName::Lock,
                    _ => IconName::LockOpen,
            })
            .child(div().text_size(TEXT).child(access_name(&access)))
            .text_size(TEXT)
            .text_color(theme.muted_foreground)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (id, name) in ACCESS_LEVELS {
                    let entity = entity.clone();
                    menu = menu.item(crate::style::menu_item(t(name)).checked(access == id).on_click(
                        move |_, _, cx| {
                            entity.update(cx, |this, cx| this.set_access(id, cx)).ok();
                        },
                    ));
                }
                menu
            });

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
                        .child(
                            Icon::new(if will_queue { IconName::ListEnd } else { IconName::ArrowUp })
                                .size(ICON)
                                .text_color(theme.primary_foreground),
                        )
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

        let input_box = div()
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
            .when(!self.attachments.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(layout::SPACE_2)
                        .children(self.attachments.iter().enumerate().map(|(ix, path)| {
                            let open_path = path.clone();
                            div()
                                .id(("attachment", ix))
                                .group("attachment")
                                .relative()
                                .size(px(64.))
                                .rounded(layout::RADIUS)
                                .overflow_hidden()
                                .border_1()
                                .border_color(theme.border)
                                .cursor_pointer()
                                .child(img(path.clone()).size_full().object_fit(ObjectFit::Cover))
                                .on_click(move |_, _, cx| cx.open_with_system(&open_path))
                                .child(
                                    div()
                                        .id(("remove-attachment", ix))
                                        .absolute()
                                        .top(px(3.))
                                        .right(px(3.))
                                        .size(rems(1.5))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .bg(theme.foreground.opacity(0.7))
                                        .invisible()
                                        .group_hover("attachment", |this| this.visible())
                                        .child(
                                            Icon::new(IconName::X)
                                                .size(ICON)
                                                .text_color(theme.background),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.remove_attachment(ix, cx)
                                        })),
                                )
                        })),
                )
            })
            .child(
                div()
                    .text_size(TEXT)
                    // The textarea pads its text on every side and offers no
                    // way to turn that off; step out by as much, so the text
                    // sits the box's own padding from its edges, like the
                    // attachments and buttons do.
                    .mx(-gpui_kit::component::Size::default().input_px())
                    .mt(-gpui_kit::component::Size::default().input_py())
                    .child({
                        let panel = cx.entity().downgrade();
                        Textarea::new(&self.composer).text_size(TEXT)
                            .appearance(false)
                            .bordered(false)
                            .on_paste(move |item, _, cx| {
                                panel.update(cx, |this, cx| this.paste(item, cx)).unwrap_or(false)
                            })
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_1)
                    // Icon buttons have room around their glyphs; pull them
                    // left by that, so the glyph lines up with the text.
                    // (Not down: the round send button on the right is drawn
                    // to its edge and already keeps the box's padding.)
                    .ml(px(-5.))
                    .when(can_attach, |this| {
                        this.child(
                            crate::style::chrome_button("attach")
                                .app_icon(IconName::Paperclip)
                                .text_color(theme.muted_foreground)
                                .on_click(cx.listener(|this, _, window, cx| this.attach_images(window, cx)))
                                .app_tooltip(t("Attach images")),
                        )
                    })
                    .child(div().flex_1())
                    .when(will_queue && can_send, |this| {
                        this.child(
                            div()
                                .mr_2()
                                .text_size(TEXT)
                                .text_color(theme.muted_foreground)
                                .child(t("Enter to queue")),
                        )
                    })
                    .child(agent_picker)
                    .child(div().w(layout::SPACE_1))
                    .child(action),
            );

        let divider = || div().w(px(1.)).h(px(14.)).mx_1().bg(theme.border);
        let context_bar = div()
            .px(layout::SPACE_2)
            .pt(layout::SPACE_2)
            .flex()
            .items_center()
            .flex_wrap()
            .gap(layout::SPACE_1)
            .child(machine_picker)
            .when_some(project_picker, |this, picker| this.child(divider()).child(picker))
            .when_some(workspace_picker, |this, picker| this.child(picker))
            .child(div().flex_1())
            .when_some(branch, |this, branch| this.child(branch))
            .child(access_picker);

        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .when_some(thread.and_then(|t| self.render_pending_prompts(t.id, cx)), |this, queue| this.child(queue))
            .child(input_box)
            .child(context_bar)
            .into_any_element()
    }

    fn render_pending_prompts(&self, id: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let prompts = self.pending_prompts.get(&id)?;
        let theme = cx.theme().clone();
        let can_steer = self.can_steer(id);
        Some(
            div()
                .id("pending-prompts")
                .w_full()
                .max_h(px(160.))
                .overflow_x_hidden()
                .overflow_y_scroll()
                .mb(layout::SPACE_2)
                .rounded(layout::RADIUS)
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .children(prompts.iter().enumerate().map(|(index, prompt)| {
                    let turn_id = prompt.turn_id.clone();
                    let text = prompt.message.text.lines().next().unwrap_or("");
                    let status = match &prompt.state {
                        PromptState::Waiting | PromptState::Accepted { queued: true } => t("Queued"),
                        PromptState::Sending | PromptState::Accepted { queued: false } => t("Sending…"),
                        PromptState::Failed(_) => t("Send failed"),
                    };
                    let row = div()
                        .id(SharedString::from(turn_id.clone()))
                        .flex()
                        .items_center()
                        .gap(layout::SPACE_2)
                        .px(layout::SPACE_3)
                        .py(layout::SPACE_1)
                        .when(index > 0, |row| row.border_t_1().border_color(theme.border))
                        .text_size(TEXT)
                        .children(prompt.message.images.iter().take(3).map(|path| {
                            img(path.clone())
                                .size(px(24.))
                                .flex_none()
                                .rounded(layout::RADIUS)
                                .object_fit(ObjectFit::Cover)
                        }))
                        .when(prompt.message.images.len() > 3, |row| {
                            row.child(div().flex_none().child(format!("+{}", prompt.message.images.len() - 3)))
                        })
                        .child(
                            div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap()
                                .child(if text.is_empty() { t("Image attachment").into() } else { text.to_owned() }),
                        )
                        .child(div().flex_none().text_color(theme.muted_foreground).child(status));
                    let row = match &prompt.state {
                        PromptState::Failed(error) => row.child(
                            crate::style::chrome_button("retry-prompt")
                                .app_icon(IconName::RefreshCw)
                                .accessibility_label("Retry queued message")
                                .on_click(cx.listener({
                                    let turn_id = turn_id.clone();
                                    move |this, _, _, cx| this.retry_pending_prompt(id, &turn_id, cx)
                                }))
                                .app_tooltip(tf("Retry send · {error}", &[("error", error.to_string())])),
                        ),
                        PromptState::Waiting if index == 0 && can_steer => row.child(
                            Button::new("steer-prompt")
                                .ghost()
                                .small()
                                .h(layout::CONTROL)
                                .app_label(t("Steer"))
                                .on_click(cx.listener({
                                    let turn_id = turn_id.clone();
                                    move |this, _, _, cx| {
                                        if this.pending_prompt_mut(id, &turn_id).is_some() {
                                            this.dispatch_prompt(id, true, cx);
                                        }
                                    }
                                }))
                                .app_tooltip(t("Send into the current turn")),
                        ),
                        _ => row,
                    };
                    row.when(!prompt.is_submitted(), |row| {
                        row.child(
                            crate::style::chrome_button("remove-prompt")
                                .app_icon(IconName::X)
                                .accessibility_label("Remove queued message")
                                .on_click(cx.listener(move |this, _, _, cx| this.remove_pending_prompt(id, &turn_id, cx)))
                                .app_tooltip(t("Remove queued message")),
                        )
                    })
                }))
                .into_any_element(),
        )
    }
}

/// How the transcript lays out: single items, or runs of tool calls.
#[derive(Debug, PartialEq)]
enum Block {
    Single(usize),
    Group(Vec<usize>),
    /// Images the agent generated one after another, shown side by side.
    Gallery(Vec<usize>),
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
        if is_generated_image(&items[ix]) {
            flush(&mut run, &mut blocks);
            match blocks.last_mut() {
                Some(Block::Gallery(gallery)) => gallery.push(ix),
                _ => blocks.push(Block::Gallery(vec![ix])),
            }
        } else if is_step(&items[ix]) {
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
#[derive(Clone)]
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
    !generated_images(item).is_empty()
}

/// The images a "Generate image" call made. One saved before oversized
/// images fell back to their file has none, but its output names the file.
fn generated_images(item: &ChatItem) -> Vec<PathBuf> {
    let ChatItem::Tool { title, images, output, .. } = item else {
        return Vec::new();
    };
    if title != "Generate image" {
        return Vec::new();
    }
    if !images.is_empty() {
        return images.clone();
    }
    let saved = PathBuf::from(output.as_deref().unwrap_or_default().trim());
    if saved.is_absolute() && is_image_path(&saved) && saved.is_file() {
        vec![saved]
    } else {
        Vec::new()
    }
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
    let Some(ms) = ms else { return t("Worked").into() };
    let secs = (ms + 500) / 1000;
    match secs {
        0 => tf("Worked for {s}s", &[("s", "1".into())]),
        1..60 => tf("Worked for {s}s", &[("s", secs.to_string())]),
        60..3600 => tf("Worked for {m}m {s}s", &[("m", (secs / 60).to_string()), ("s", (secs % 60).to_string())]),
        _ => tf("Worked for {h}h {m}m", &[("h", (secs / 3600).to_string()), ("m", (secs % 3600 / 60).to_string())]),
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
    // Each verb has its own key with and without the detail, so every
    // language places the detail where it reads naturally.
    let with = |(alone, with_detail): (&'static str, &'static str), rest: &str| {
        if rest.is_empty() { t(alone).to_string() } else { tf(with_detail, &[("detail", rest.to_string())]) }
    };
    let pick = |running_pair, done_pair| if running { running_pair } else { done_pair };
    match title {
        "Run" => (IconName::SquareTerminal, with(pick(("Running", "Running {detail}"), ("Ran", "Ran {detail}")), &short)),
        "Edit" => (IconName::FilePen, with(pick(("Editing", "Editing {detail}"), ("Edited", "Edited {detail}")), &short)),
        "Read" => (IconName::FileText, with(pick(("Reading", "Reading {detail}"), ("Read", "Read {detail}")), &short)),
        "Search" => (
            IconName::Search,
            with(pick(("Searching for", "Searching for {detail}"), ("Searched for", "Searched for {detail}")), &short),
        ),
        "Web" if detail.starts_with("http") => {
            (IconName::Globe, with(pick(("Fetching", "Fetching {detail}"), ("Fetched", "Fetched {detail}")), detail))
        }
        "Web" => (
            IconName::Globe,
            with(
                pick(
                    ("Searching the web for", "Searching the web for {detail}"),
                    ("Searched the web for", "Searched the web for {detail}"),
                ),
                detail,
            ),
        ),
        "Computer" if detail.is_empty() => (IconName::Monitor, t("Used the computer").into()),
        "Browser" if detail.is_empty() => (IconName::Globe, t("Used the browser").into()),
        "Computer" => (IconName::Monitor, detail.to_string()),
        "Browser" => (IconName::Globe, detail.to_string()),
        "Update todos" => (IconName::ListTodo, t("Updated todos").into()),
        "Generate image" => (IconName::Image, t(if running { "Generating image" } else { "Generated image" }).into()),
        "View image" => (IconName::Image, with(pick(("Viewing", "Viewing {detail}"), ("Viewed", "Viewed {detail}")), &short)),
        // A tool we do not know by name: its own name, untranslated.
        other if detail.is_empty() => (IconName::Wrench, other.to_string()),
        other => (IconName::Wrench, format!("{other} {detail}")),
    }
}

/// "Read files, ran commands, searched the web", the failures, and an icon.
fn summarize_group(items: &[ChatItem], ixs: &[usize]) -> (String, usize, IconName) {
    // Each kind of tool, in order of first use, with how many times it ran.
    let mut kinds: Vec<(&str, usize)> = Vec::new();
    let mut failed = 0;
    for &ix in ixs {
        if let ChatItem::Tool { title, status, .. } = &items[ix] {
            // Commands exiting non-zero are routine; see `render_item`.
            if *status == ToolStatus::Failed && title != "Run" {
                failed += 1;
            }
            match kinds.iter_mut().find(|(kind, _)| *kind == title.as_str()) {
                Some((_, count)) => *count += 1,
                None => kinds.push((title.as_str(), 1)),
            }
        }
    }
    let counted = |n: usize, one: &'static str, many: &'static str| {
        if n == 1 { t(one).to_string() } else { tf(many, &[("n", n.to_string())]) }
    };
    let mut parts: Vec<String> = Vec::new();
    for &(kind, n) in &kinds {
        let part = match kind {
            "Run" => counted(n, "ran 1 command", "ran {n} commands"),
            "Edit" => counted(n, "edited 1 file", "edited {n} files"),
            "Read" => counted(n, "read 1 file", "read {n} files"),
            "Search" => counted(n, "searched 1 time", "searched {n} times"),
            "Generate image" => counted(n, "generated 1 image", "generated {n} images"),
            "View image" => counted(n, "viewed 1 image", "viewed {n} images"),
            "Web" => t("searched the web").to_string(),
            "Computer" => t("used the computer").to_string(),
            "Browser" => t("used the browser").to_string(),
            "Update todos" => t("updated todos").to_string(),
            other => other.to_string(),
        };
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    let kinds: Vec<&str> = kinds.iter().map(|(kind, _)| *kind).collect();
    let icon = match kinds.first().copied() {
        Some("Computer") => IconName::Monitor,
        Some("Browser") | Some("Web") => IconName::Globe,
        Some("Edit") => IconName::FilePen,
        Some("Run") => IconName::SquareTerminal,
        _ => IconName::Search,
    };
    (capitalize(&parts.join(t(", "))), failed, icon)
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
        assert_eq!(summary, "Used the computer, ran 1 command");
        // A command exiting non-zero is not counted as a failure.
        assert_eq!(failed, 0);
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

/// Spinner plus "Working", in place of a running thread's timestamp.
/// Where a thread stands, as its sidebar dot shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ThreadStatus {
    /// The agent is working: green, pulsing.
    Working,
    /// The agent waits on an approval: amber.
    NeedsYou,
    /// The last turn ended in an error: red.
    Failed,
    /// A turn finished while the thread was not shown: blue.
    Done,
    /// Nothing new: grey.
    Idle,
}


fn render_status_dot(status: ThreadStatus, id: u64, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let color = match status {
        ThreadStatus::Working => theme.success,
        ThreadStatus::NeedsYou => theme.warning,
        ThreadStatus::Failed => theme.danger,
        ThreadStatus::Done => theme.primary,
        ThreadStatus::Idle => theme.muted_foreground.opacity(0.35),
    };
    let dot = div()
        .id(("thread-status", id))
        .size(px(7.))
        .rounded_full()
        .bg(color);
    if status == ThreadStatus::Working {
        dot.with_animation(
            ("thread-status-pulse", id),
            Animation::new(std::time::Duration::from_millis(1200)).repeat().with_easing(pulsating_between(0.35, 1.)),
            |dot, delta| dot.opacity(delta),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    }
}

/// An answer without the control lines some agents print as text, such as
/// `[MODE_UPDATE] yolo` when their approval mode changes.
fn without_control_lines(text: &str) -> std::borrow::Cow<'_, str> {
    const MARKERS: [&str; 1] = ["[MODE_UPDATE]"];
    let is_control = |line: &str| MARKERS.iter().any(|m| line.trim_start().starts_with(m));
    if !text.lines().any(is_control) {
        return text.into();
    }
    text.lines().filter(|line| !is_control(line)).collect::<Vec<_>>().join("\n").into()
}

fn render_working(cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(layout::SPACE_1)
        .text_size(TEXT)
        .text_color(theme.primary)
        .child(Spinner::new().with_size(crate::style::size::icon_pixels(cx)).color(theme.primary))
        .child(t("Working"))
        .into_any_element()
}
