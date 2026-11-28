//! The main window: sidebar, conversation or editor, and the surfaces panel.

use crate::style::AppButtonStyle as _;

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, kbd::Kbd,
    popover::Popover,
    button::{Button, ButtonVariants as _},
    input::{Editor, EditorState, Input, InputEvent, InputState},
    menu::ContextMenuExt as _,
    notification::{Notification, NotificationType},
    resizable::{HandleEdge, resize_handle},
    window_paddings,
    WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::assets::IconName;

use crate::{
    CloseTab, NewTab, NewThread, NextTab, OpenFolder, PrevTab, ReopenTab, SelectLastTab,
    SelectTab1, SelectTab2, SelectTab3, SelectTab4, SelectTab5, SelectTab6, SelectTab7, SelectTab8, OpenSettings, OpenSurface, RefreshProject, Save, Search,
    OpenEditor, ToggleGitPanel, ToggleSidebar, ToggleTerminal,
    agent_panel::{AgentPanel, AgentPanelEvent},
    search_palette::{self, SearchPalette, SearchPaletteEvent, Target},
    file_tree::{FileTree, FileTreeEvent},
    git::{self, RepoSnapshot},
    surface_host::{SurfaceHost, SurfaceHostEvent},
};
use crate::style::chrome_button;
use crate::style::layout;
use crate::style::size::{TEXT, ICON, TAB_ACTION_ICON};
use crate::i18n::{t, tf};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const SIDEBAR_DEFAULT: Pixels = px(260.);
const SIDEBAR_MIN: Pixels = px(220.);
const SIDEBAR_MAX: Pixels = px(600.);
const CONVERSATION_MIN: Pixels = px(420.);
const SURFACES_DEFAULT: Pixels = px(360.);
const SURFACES_MIN: Pixels = px(300.);
const SURFACES_MAX: Pixels = px(720.);

pub fn minimum_window_size() -> Size<Pixels> {
    size(SIDEBAR_MIN + CONVERSATION_MIN + SURFACES_MIN, px(0.))
}

/// Open a main window on folder `root` and workspace `workspace` (each
/// picked by the window when `None`), offset from the window in front.
pub fn open_main_window(root: Option<PathBuf>, workspace: Option<String>, cx: &mut App) {
    let mut bounds = WindowBounds::centered(size(px(1280.), px(820.)), cx);
    if let Some(front) = cx.active_window().and_then(|w| w.update(cx, |_, window, _| window.bounds()).ok()) {
        let mut moved = front;
        moved.origin = moved.origin + point(px(28.), px(28.));
        bounds = WindowBounds::Windowed(moved);
    }
    let options = WindowOptions {
        window_bounds: Some(bounds),
        window_min_size: Some(minimum_window_size()),
        titlebar: Some(TitlebarOptions {
            title: Some("CodeAny".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.), px(13.))),
        }),
        window_decorations: window_decorations(),
        ..Default::default()
    };
    if let Err(err) = gpui_kit::open_window(options, cx, move |window, cx| {
        crate::style::apply(window, cx);
        cx.new(|cx| Workspace::new(root, workspace, window, cx))
    }) {
        eprintln!("codeany: cannot open a window: {err:#}");
    }
}

/// ⌘⇧N: a window on a workspace no window shows yet; with none left, the
/// window showing the one used last comes to the front.
pub fn open_new_window(cx: &mut App) {
    let hub = crate::hub::hub(cx);
    let shared = hub.read(cx).shared();
    shared.borrow_mut().prune(cx);
    let free = shared.borrow().free_workspace();
    match free {
        Some(id) => open_main_window(None, Some(id), cx),
        None => {
            let last = {
                let s = shared.borrow();
                s.store.active_workspace.as_deref().and_then(|id| s.view_of(id, EntityId::from(u64::MAX)))
            };
            if let Some(view) = last {
                view.window.update(cx, |_, window, _| window.activate_window()).ok();
            }
        }
    }
}

enum TabContent {
    Editor(Entity<EditorState>),
    Message(SharedString),
}

struct EditorTab {
    /// The device the file is on; `None` is this one.
    machine: Option<String>,
    path: PathBuf,
    content: TabContent,
    dirty: bool,
    modified_at: Option<SystemTime>,
    _subscription: Option<Subscription>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Agent,
    Editor,
}

#[derive(Clone, Copy)]
enum PanelDivider {
    Sidebar,
    Surfaces,
}

impl Render for PanelDivider {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// Room for the macOS traffic lights in the transparent title bar; on Linux
/// the window buttons drawn by [`Workspace::render_window_buttons`] sit there.
const TRAFFIC_LIGHTS_WIDTH: f32 = 86.;
/// How far a press in the title bar travels before it drags the window, so
/// a click on one of its buttons stays a click.
const WINDOW_DRAG_SLOP: Pixels = px(4.);

/// Linux draws no title bar for a window like this one, so the window draws
/// its own frame: the shadow and resize edges come with the root, the
/// buttons and the dragging from [`Workspace`].
fn window_decorations() -> Option<WindowDecorations> {
    cfg!(target_os = "linux").then_some(WindowDecorations::Client)
}

/// Whether this window draws its own title bar buttons: on Linux when it
/// draws its own frame, on Windows unless full screen, where the system
/// answers for no part of the window.
fn draws_window_buttons(window: &Window) -> bool {
    if cfg!(target_os = "windows") {
        return !window.is_fullscreen();
    }
    cfg!(target_os = "linux") && matches!(window.window_decorations(), Decorations::Client { .. })
}

pub struct Workspace {
    root: Option<PathBuf>,
    /// The chat tab whose title is being typed: its thread and the input.
    renaming_tab: Option<(u64, Entity<InputState>)>,
    _rename_subscription: Option<Subscription>,
    /// The active thread's worktree, shown in the file tree, surfaces and git
    /// status in place of `root`.
    work_dir: Option<PathBuf>,
    /// The other device the conversation shown runs on, and its folder
    /// there: the surfaces show that in place of anything on this one.
    remote: Option<(String, Option<PathBuf>)>,
    /// The main window shows the agent; an editor window, opened from it, the editor.
    mode: Mode,
    /// From the main window: its editor window, while open.
    editor_window: Option<(AnyWindowHandle, WeakEntity<Workspace>)>,
    /// From an editor window: the main window that opened it.
    main_window: Option<(AnyWindowHandle, WeakEntity<Workspace>)>,
    agent_panel: Entity<AgentPanel>,
    /// The panel's conversation and thread list, embedded cached.
    chat_view: Entity<crate::panel_views::ChatView>,
    sidebar_view: Entity<crate::panel_views::SidebarView>,
    settings: Entity<crate::settings::Settings>,
    file_tree: Entity<FileTree>,
    surfaces: Entity<SurfaceHost>,
    tabs: Vec<EditorTab>,
    active_tab: Option<usize>,
    git: RepoSnapshot,
    overview_open: bool,
    overview_git: Option<((Option<String>, PathBuf), Option<RepoSnapshot>)>,
    overview_poll: Option<Task<()>>,
    show_sidebar: bool,
    sidebar_width: Pixels,
    surfaces_width: Pixels,
    focus_handle: FocusHandle,
    /// Where a press in the title bar landed, until it drags the window or lets go.
    window_drag: Option<Point<Pixels>>,
    /// The open search palette's subscription.
    _palette: Option<Subscription>,
    _poll_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn language_for(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match name.as_str() {
        "cargo.lock" => return "toml",
        "makefile" | "gnumakefile" => return "make",
        ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => return "bash",
        _ => {}
    }
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "go" => "go",
        "py" | "pyi" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" | "mdx" => "markdown",
        "sh" | "bash" | "zsh" => "bash",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "css" | "scss" => "css",
        "html" | "htm" => "html",
        "java" => "java",
        "diff" | "patch" => "diff",
        _ => "text",
    }
}

fn modified_at(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Read a file for display, or explain why it cannot be shown.
fn load_text(path: &Path) -> Result<String, SharedString> {
    let meta = std::fs::metadata(path).map_err(|e| SharedString::from(e.to_string()))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(too_large(meta.len()));
    }
    let bytes = std::fs::read(path).map_err(|e| SharedString::from(e.to_string()))?;
    text_of(bytes)
}

fn too_large(bytes: u64) -> SharedString {
    tf("File is too large to open ({size} MB)", &[("size", (bytes / 1024 / 1024).to_string())]).into()
}

fn text_of(bytes: Vec<u8>) -> Result<String, SharedString> {
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err(t("Binary file is not displayed").into());
    }
    String::from_utf8(bytes).map_err(|_| t("File is not valid UTF-8").into())
}

/// [`load_text`] for a file on another device, through its agl. Blocks.
fn load_remote_text(machine: &str, path: &Path) -> Result<String, SharedString> {
    use base64::Engine as _;
    // Its size first: a big file is not worth the trip.
    let dir = path.parent().unwrap_or(Path::new("/"));
    let name = path.to_string_lossy();
    let size = crate::exec::run(Some(machine), dir, &["wc", "-c", &name], &[]).map_err(SharedString::from)?;
    if !size.ok() {
        return Err(size.stderr.trim().to_string().into());
    }
    let size: u64 = size.stdout.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
    if size > MAX_FILE_BYTES {
        return Err(too_large(size));
    }
    let file = crate::exec::call(machine, "fs.read", serde_json::json!({ "path": path })).map_err(SharedString::from)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(file["content_base64"].as_str().unwrap_or_default())
        .map_err(|err| SharedString::from(err.to_string()))?;
    text_of(bytes)
}

/// Write a file on another device, through its agl. Blocks.
fn save_remote_text(machine: &str, path: &Path, text: &str) -> Result<(), String> {
    use base64::Engine as _;
    let content = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    crate::exec::call(machine, "fs.write", serde_json::json!({ "path": path, "content_base64": content })).map(|_| ())
}

impl Workspace {
    /// The main window, showing the agent.
    pub fn new(root: Option<PathBuf>, workspace: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let agent_panel = cx.new(|cx| AgentPanel::new(workspace, window, cx));
        // Launched without a folder (from Finder or the Dock): pick up where
        // the last run left off, in the workspace shown. The very first run
        // opens nothing.
        let root = root.or_else(|| agent_panel.read(cx).default_root());
        let this = Self::build(root, Mode::Agent, agent_panel, None, window, cx);
        let handle = this.agent_panel.read(cx).composer_focus_handle(cx);
        handle.focus(window, cx);
        this
    }

    /// An editor window for `root`. It shares the main window's agent panel
    /// (for settings and the search palette) without showing or steering it.
    fn new_editor(
        machine: Option<String>,
        root: Option<PathBuf>,
        main_window: (AnyWindowHandle, WeakEntity<Workspace>),
        agent_panel: Entity<AgentPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(None, Mode::Editor, agent_panel, Some(main_window), window, cx);
        if root.is_some() {
            this.set_location(machine, root, window, cx);
        }
        this.focus_handle.focus(window, cx);
        this
    }

    fn build(
        root: Option<PathBuf>,
        mode: Mode,
        agent_panel: Entity<AgentPanel>,
        main_window: Option<(AnyWindowHandle, WeakEntity<Workspace>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let file_tree = cx.new(FileTree::new);
        let surfaces = cx.new(|cx| SurfaceHost::new(mode == Mode::Agent, cx));

        let mut subscriptions = vec![
            cx.subscribe_in(
                &file_tree,
                window,
                |this, _, event: &FileTreeEvent, window, cx| match event {
                    FileTreeEvent::OpenFile(path) => {
                        let machine = this.remote.as_ref().map(|(machine, _)| machine.clone());
                        this.open_file(machine, path.clone(), window, cx);
                    }
                },
            ),
            cx.subscribe_in(&surfaces, window, |this, _, event: &SurfaceHostEvent, window, cx| match event {
                SurfaceHostEvent::OpenFile { machine, path } => this.show_file(machine.clone(), path.clone(), window, cx),
            }),
            cx.observe(&surfaces, |_, _, cx| cx.notify()),
            // The system went light or dark: each window redraws in its theme.
            cx.observe_window_appearance(window, |_, _, cx| crate::style::changed(cx)),
        ];
        if mode == Mode::Agent {
            #[cfg(target_os = "macos")]
            subscriptions.push(
                cx.observe_global::<crate::updater::AvailableUpdate>(|_, cx| cx.notify()),
            );
            subscriptions.push(cx.subscribe_in(&agent_panel, window, Self::on_agent_event));
            subscriptions.push(cx.observe_in(&agent_panel, window, |this, panel, window, cx| {
                let (work_dir, remote) = {
                    let panel = panel.read(cx);
                    (panel.work_dir(), panel.remote_location())
                };
                if work_dir != this.work_dir {
                    this.set_work_dir(work_dir, window, cx);
                }
                if remote != this.remote {
                    this.remote = remote;
                    this.sync_surfaces(window, cx);
                }
                cx.notify();
            }));
        }

        let mut this = Self {
            root: None,
            work_dir: None,
            remote: None,
            renaming_tab: None,
            _rename_subscription: None,
            mode,
            editor_window: None,
            main_window,
            settings: cx.new(|_| crate::settings::Settings::new(&agent_panel)),
            chat_view: cx.new(|cx| crate::panel_views::ChatView::new(agent_panel.clone(), cx)),
            sidebar_view: cx.new(|cx| crate::panel_views::SidebarView::new(agent_panel.clone(), cx)),
            agent_panel,
            file_tree,
            surfaces,
            tabs: Vec::new(),
            active_tab: None,
            git: RepoSnapshot::default(),
            overview_open: false,
            overview_git: None,
            overview_poll: None,
            show_sidebar: true,
            sidebar_width: SIDEBAR_DEFAULT,
            surfaces_width: SURFACES_DEFAULT,
            focus_handle: cx.focus_handle(),
            window_drag: None,
            _palette: None,
            _poll_task: None,
            _subscriptions: subscriptions,
        };
        if root.is_some() {
            this.set_root(root, window, cx);
        }
        // A window that opens on another device's workspace.
        if mode == Mode::Agent {
            this.remote = this.agent_panel.read(cx).remote_location();
            if this.remote.is_some() {
                this.sync_surfaces(window, cx);
            }
        }
        this
    }

    /// Point the surfaces at what the window shows: the conversation's
    /// folder on its device, or this one's project.
    fn sync_surfaces(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (machine, dir) = self.conversation_location();
        let project = match self.mode {
            Mode::Agent => self.agent_panel.read(cx).conversation_project(),
            Mode::Editor => self.root.clone(),
        };
        self.surfaces.update(cx, |surfaces, cx| surfaces.set_location(machine, dir, project, window, cx));
    }

    fn conversation_location(&self) -> (Option<String>, Option<PathBuf>) {
        match &self.remote {
            Some((machine, dir)) => (Some(machine.clone()), dir.clone()),
            None => (None, self.shown_dir()),
        }
    }

    /// The folder the file tree, surfaces and git status show.
    fn shown_dir(&self) -> Option<PathBuf> {
        self.work_dir.clone().or_else(|| self.root.clone())
    }

    /// Show a thread's worktree in place of the project, or (`None`) the
    /// project again.
    fn set_work_dir(&mut self, work_dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.work_dir = work_dir;
        let dir = self.shown_dir();
        self.git = RepoSnapshot::default();
        self.file_tree.update(cx, |tree, cx| tree.set_root(dir, cx));
        self.sync_surfaces(window, cx);
    }

    fn set_root(&mut self, root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.set_location(None, root, window, cx);
    }

    fn set_location(&mut self, machine: Option<String>, root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Editor {
            self.remote = machine.as_ref().map(|id| (id.clone(), root.clone()));
        }
        self.root = root.clone();
        self.work_dir = None;
        self.tabs.clear();
        self.active_tab = None;
        self.git = RepoSnapshot::default();
        self.file_tree
            .update(cx, |tree, cx| tree.set_location(machine.clone(), root.clone(), cx));
        if self.mode == Mode::Agent {
            self.agent_panel
                .update(cx, |panel, cx| panel.set_project(root.clone(), cx));
        }
        self.sync_surfaces(window, cx);

        let app = match self.mode {
            Mode::Agent => "CodeAny",
            Mode::Editor => t("CodeAny Editor"),
        };
        let title = root
            .as_ref()
            .map(|r| format!("{} — {app}", crate::threads::project_label(r)))
            .unwrap_or_else(|| app.into());
        window.set_window_title(&title);

        self._poll_task = root.map(|_| {
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    let Ok(root) = this.read_with(cx, |this, _| this.shown_dir()) else {
                        break;
                    };
                    let Some(root) = root else { break };
                    // Only the editor shows the file tree; its folders are
                    // read here, off the UI thread, with the git status.
                    let dirs = this
                        .read_with(cx, |this, cx| {
                            (this.mode == Mode::Editor && machine.is_none()).then(|| this.file_tree.read(cx).loaded_dirs())
                        })
                        .ok()
                        .flatten();
                    let machine = machine.clone();
                    let (snapshot, listing) = cx
                        .background_executor()
                        .spawn(async move { (git::snapshot(machine.as_deref(), &root), dirs.map(FileTree::read_dirs)) })
                        .await;
                    let alive = this.update_in(cx, |this, window, cx| {
                        this.apply_snapshot(snapshot.unwrap_or_default(), listing, window, cx);
                    });
                    if alive.is_err() {
                        break;
                    }
                    cx.background_executor().timer(POLL_INTERVAL).await;
                }
            })
        });
        cx.notify();
    }

    /// Bring the tree, git panel and clean tabs up to date with the disk.
    fn apply_snapshot(
        &mut self,
        snapshot: RepoSnapshot,
        listing: Option<crate::file_tree::Listing>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = snapshot != self.git;
        self.git = snapshot.clone();
        self.file_tree.update(cx, |tree, cx| {
            tree.set_git(snapshot, cx);
            if let Some(listing) = listing {
                tree.apply_listing(listing, cx);
            }
        });
        if changed {
            self.surfaces.update(cx, |surfaces, cx| surfaces.refresh(window, cx));
        }
        self.reload_changed_tabs(window, cx);
        // Nothing new on disk: nothing to redraw.
        if changed {
            cx.notify();
        }
    }

    fn reload_changed_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for tab in &mut self.tabs {
            // A file on another device is read when it is opened.
            if tab.dirty || tab.machine.is_some() {
                continue;
            }
            let TabContent::Editor(editor) = &tab.content else {
                continue;
            };
            let current = modified_at(&tab.path);
            if current.is_none() || current == tab.modified_at {
                continue;
            }
            tab.modified_at = current;
            if let Ok(text) = load_text(&tab.path) {
                editor.update(cx, |editor, cx| {
                    let position = editor.cursor_position();
                    editor.set_value(text, window, cx);
                    editor.set_cursor_position(position, window, cx);
                });
            }
        }
    }

    /// An editor on `text`, and what marks its tab dirty when it changes.
    fn editor_content(path: &Path, text: String, window: &mut Window, cx: &mut Context<Self>) -> (TabContent, Option<Subscription>) {
        let language = language_for(path);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(true)
        });
        editor.update(cx, |editor, cx| editor.set_value(text, window, cx));
        let subscription = cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some(tab) = this.tabs.iter_mut().find(|tab| {
                    matches!(&tab.content, TabContent::Editor(e) if *e == editor)
                }) {
                    if !tab.dirty {
                        tab.dirty = true;
                        cx.notify();
                    }
                }
            }
        });
        (TabContent::Editor(editor), Some(subscription))
    }

    /// Open a file of `machine` (`None`: this one) in a tab.
    fn open_file(&mut self, machine: Option<String>, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|tab| tab.path == path && tab.machine == machine) {
            self.activate_tab(ix, window, cx);
            return;
        }

        let (content, subscription) = match &machine {
            None => match load_text(&path) {
                Ok(text) => Self::editor_content(&path, text, window, cx),
                Err(message) => (TabContent::Message(message), None),
            },
            // It arrives from its device below.
            Some(_) => (TabContent::Message(t("Loading…").into()), None),
        };

        let ix = self.active_tab.map(|ix| ix + 1).unwrap_or(self.tabs.len());
        self.tabs.insert(
            ix,
            EditorTab {
                modified_at: if machine.is_none() { modified_at(&path) } else { None },
                machine: machine.clone(),
                path: path.clone(),
                content,
                dirty: false,
                _subscription: subscription,
            },
        );
        self.activate_tab(ix, window, cx);

        let Some(machine) = machine else { return };
        cx.spawn_in(window, async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn({
                    let (machine, path) = (machine.clone(), path.clone());
                    async move { load_remote_text(&machine, &path) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                // Closed meanwhile.
                let Some(ix) = this.tabs.iter().position(|tab| tab.path == path && tab.machine.as_ref() == Some(&machine)) else {
                    return;
                };
                let (content, subscription) = match loaded {
                    Ok(text) => Self::editor_content(&path, text, window, cx),
                    Err(message) => (TabContent::Message(message), None),
                };
                this.tabs[ix].content = content;
                this.tabs[ix]._subscription = subscription;
                if this.active_tab == Some(ix) {
                    this.activate_tab(ix, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        self.active_tab = Some(ix);
        let path = tab.path.clone();
        if let TabContent::Editor(editor) = &tab.content {
            editor.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.file_tree
            .update(cx, |tree, cx| tree.select_path(&path, cx));
        cx.notify();
    }

    fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        self.active_tab = match self.active_tab {
            _ if self.tabs.is_empty() => None,
            Some(active) if active > ix => Some(active - 1),
            Some(active) if active == ix => Some(ix.min(self.tabs.len() - 1)),
            other => other,
        };
        if let Some(active) = self.active_tab {
            self.activate_tab(active, window, cx);
        } else {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    fn open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("Open").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.set_root(Some(path), window, cx))
                .ok();
        })
        .detach();
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab.and_then(|ix| self.tabs.get_mut(ix)) else {
            return;
        };
        let TabContent::Editor(editor) = &tab.content else {
            return;
        };
        let text = editor.read(cx).value();
        let (machine, path) = (tab.machine.clone(), tab.path.clone());
        let Some(machine) = machine else {
            let saved = std::fs::write(&path, text.as_bytes()).map_err(|err| err.to_string());
            self.saved(None, path, saved, window, cx);
            return;
        };
        // Its device writes it; the tab stays dirty until that is done.
        cx.spawn_in(window, async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn({
                    let (machine, path) = (machine.clone(), path.clone());
                    async move { save_remote_text(&machine, &path, &text) }
                })
                .await;
            this.update_in(cx, |this, window, cx| this.saved(Some(machine), path, saved, window, cx)).ok();
        })
        .detach();
    }

    /// A save ended: the tab is clean, or the window says why not.
    fn saved(&mut self, machine: Option<String>, path: PathBuf, result: Result<(), String>, window: &mut Window, cx: &mut Context<Self>) {
        match result {
            Ok(()) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.path == path && tab.machine == machine) {
                    tab.dirty = false;
                    if machine.is_none() {
                        tab.modified_at = modified_at(&path);
                    }
                }
                self.surfaces.update(cx, |surfaces, cx| surfaces.refresh(window, cx));
            }
            Err(err) => {
                let message = tf("Failed to save {path}: {err}", &[("path", path.display().to_string()), ("err", err)]);
                let message: SharedString = message.into();
                window.push_notification(
                    Notification::new()
                        .with_type(NotificationType::Error)
                        .content(move |_, _, _| div().text_size(TEXT).child(message.clone()).into_any_element()),
                    cx,
                );
            }
        }
        cx.notify();
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Agent {
            self.agent_panel.update(cx, |panel, cx| panel.close_tab(None, window, cx));
            return;
        }
        if let Some(ix) = self.active_tab {
            self.close_tab_at(ix, window, cx);
        }
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.show_sidebar = !self.show_sidebar;
        cx.notify();
    }

    fn toggle_git_panel(&mut self, _: &ToggleGitPanel, window: &mut Window, cx: &mut Context<Self>) {
        self.surfaces.update(cx, |surfaces, cx| surfaces.toggle("diff", window, cx));
    }

    fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.surfaces.update(cx, |surfaces, cx| surfaces.toggle("terminal", window, cx));
    }

    fn open_surface(&mut self, _: &OpenSurface, window: &mut Window, cx: &mut Context<Self>) {
        self.surfaces.update(cx, |surfaces, cx| surfaces.show_launcher(window, cx));
    }

    fn refresh_project(&mut self, _: &RefreshProject, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(root) = &self.root {
            cx.remove_asset::<crate::project_avatar::ProjectLogo>(root);
        }
        self.file_tree.update(cx, |tree, cx| tree.refresh(cx));
        self.surfaces.update(cx, |surfaces, cx| surfaces.refresh(window, cx));
        cx.notify();
    }

    fn relative_path(&self, path: &Path) -> String {
        self.root
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(path)
            .display()
            .to_string()
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .id("tab-bar")
            .h(px(34.))
            .flex_none()
            .flex()
            .overflow_x_scroll()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                let active = self.active_tab == Some(ix);
                let name: SharedString = tab
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .into();
                let status = self.git.statuses.get(&tab.path).copied().filter(|_| tab.machine.is_none());
                div()
                    .id(ix)
                    .group("tab")
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .h_full()
                    .pl_3()
                    .pr_1()
                    .text_size(TEXT)
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .when(active, |this| {
                        this.bg(theme.background).text_color(theme.foreground)
                    })
                    .when(!active, |this| this.text_color(theme.muted_foreground))
                    .child(
                        div()
                            .when_some(status, |this, status| {
                                this.text_color(match status {
                                    git::FileStatus::Untracked | git::FileStatus::Added => {
                                        theme.success
                                    }
                                    git::FileStatus::Deleted | git::FileStatus::Conflicted => {
                                        theme.danger
                                    }
                                    _ => theme.warning,
                                })
                            })
                            .child(name),
                    )
                    .child(
                        div()
                            .size(rems(1.5))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .hover(|this| this.bg(theme.list_hover))
                            .map(|this| {
                                if tab.dirty {
                                    this.child(div().size(px(8.)).rounded_full().bg(theme.foreground.opacity(0.7)))
                                } else {
                                    this.child(
                                        Icon::new(IconName::X)
                                            .size(ICON)
                                            .text_color(theme.muted_foreground),
                                    )
                                    .invisible()
                                    .when(active, |this| this.visible())
                                    .group_hover("tab", |this| this.visible())
                                }
                            })
                            .id(("close-tab", ix))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab_at(ix, window, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_tab(ix, window, cx)
                    }))
            }))
    }

    fn render_editor_area(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(tab) = self.active_tab.and_then(|ix| self.tabs.get(ix)) else {
            return self.render_welcome(cx).into_any_element();
        };

        let body = match &tab.content {
            TabContent::Editor(editor) => div()
                .flex_1()
                .min_h_0()
                .child(
                    Editor::new(editor)
                        .bordered(false)
                        .appearance(false)
                        .h_full(),
                )
                .into_any_element(),
            TabContent::Message(message) => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(message.clone())
                .into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .child(self.render_tab_bar(cx))
            .child(
                div()
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px_3()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(self.relative_path(&tab.path).replace('/', " › ")),
            )
            .child(body)
            .into_any_element()
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let shortcut = |keys: &'static str, label: &'static str| {
            div()
                .flex()
                .justify_between()
                .w(px(280.))
                .text_size(TEXT)
                .child(div().text_color(theme.muted_foreground).child(t(label)))
                .child(div().text_color(theme.foreground).child(keys))
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(theme.background)
            .child(
                div()
                    .text_size(TEXT)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child("CodeAny"),
            )
            .when(self.root.is_none(), |this| {
                this.child(
                    Button::new("open-folder")
                        .primary()
                        .app_label(t("Open Folder…"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_folder(&OpenFolder, window, cx)
                        })),
                )
            })
            .child(
                div()
                    .mt_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(shortcut("⌘O", "Open Folder"))
                    .child(shortcut("⌘S", "Save"))
                    .child(shortcut("⌘B", "Toggle Sidebar"))
                    .child(shortcut("⌘⇧G", "Toggle Git Changes"))
                    .child(shortcut("⌘J / ⌃`", "Toggle Terminal")),
            )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.active_tab.and_then(|ix| self.tabs.get(ix));
        let cursor = active.and_then(|tab| match &tab.content {
            TabContent::Editor(editor) => {
                let position = editor.read(cx).cursor_position();
                Some(tf("Ln {line}, Col {col}", &[("line", (position.line + 1).to_string()), ("col", (position.character + 1).to_string())]))
            }
            TabContent::Message(_) => None,
        });
        let language = active.map(|tab| language_for(&tab.path));

        div()
            .h(px(26.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px_2()
            .text_size(TEXT)
            .bg(theme.title_bar)
            .border_t_1()
            .border_color(theme.border)
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when_some(self.git.branch.clone(), |this, branch| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(Icon::new(IconName::GitBranch).size(ICON))
                                .child(branch),
                        )
                    })
                    .when(!self.git.statuses.is_empty(), |this| {
                        this.child(tf("{n} changed", &[("n", self.git.statuses.len().to_string())]))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when_some(cursor, |this, cursor| this.child(cursor))
                    .when_some(language, |this, language| this.child(language)),
            )
    }

}


impl Workspace {
    fn on_agent_event(
        &mut self,
        _: &Entity<AgentPanel>,
        event: &AgentPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AgentPanelEvent::OpenFile(path) => {
                // A file the conversation names is on the device it runs on.
                let (machine, root) = match self.remote.clone() {
                    Some((machine, dir)) => (Some(machine), dir),
                    None => (None, self.shown_dir()),
                };
                let path = match (&root, path.is_relative()) {
                    (Some(root), true) => root.join(path),
                    _ => path.clone(),
                };
                self.show_file(machine, path, window, cx);
            }
            AgentPanelEvent::SwitchProject(project) => {
                if *project != self.root {
                    self.set_root(project.clone(), window, cx);
                }
            }
            AgentPanelEvent::OpenProjectInEditor { machine, project } => {
                self.show_editor_window(machine.clone(), Some(project.clone()), true, window, cx);
            }
            AgentPanelEvent::OpenSettings => self.open_settings(&OpenSettings, window, cx),
            AgentPanelEvent::TurnFinished => {
                self.file_tree.update(cx, |tree, cx| tree.refresh(cx));
                self.surfaces.update(cx, |surfaces, cx| surfaces.refresh(window, cx));
            }
        }
    }

    /// Open a file in the editor: here in an editor window, or in the main
    /// window's editor window.
    fn show_file(&mut self, machine: Option<String>, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Editor => self.open_file(machine, path, window, cx),
            Mode::Agent => {
                if let Some((handle, editor)) = self.ensure_editor_window(window, cx) {
                    let _ = handle.update(cx, |_, window, cx| {
                        editor.update(cx, |editor, cx| editor.open_file(machine, path, window, cx))
                    });
                }
            }
        }
    }

    /// Bring up this project's editor window, opening it when there is none.
    fn ensure_editor_window(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
        self.show_editor_window(None, self.root.clone(), false, window, cx)
    }

    fn show_editor_window(
        &mut self,
        machine: Option<String>,
        root: Option<PathBuf>,
        exact_project: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
        if let Some((handle, editor)) = &self.editor_window
            && let Some(editor) = editor.upgrade()
        {
            let handle = *handle;
            let editor_state = editor.read(cx);
            let same_location = editor_state.conversation_location() == (machine.clone(), root.clone());
            let dirty = editor_state.tabs.iter().any(|tab| tab.dirty);
            if same_location || !dirty || !exact_project {
                let shown = handle.update(cx, |_, window, cx| {
                    window.activate_window();
                    if !same_location && !dirty {
                        editor.update(cx, |editor, cx| {
                            editor.set_location(machine.clone(), root.clone(), window, cx);
                        });
                    }
                });
                if shown.is_ok() {
                    return Some((handle, editor));
                }
            }
        }

        self.open_editor_window(machine, root, window, cx)
    }

    fn open_editor_window(
        &mut self,
        machine: Option<String>,
        root: Option<PathBuf>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
        let main = (window.window_handle(), cx.entity().downgrade());
        let agent_panel = self.agent_panel.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1280.), px(820.)), cx)),
            window_min_size: Some(minimum_window_size()),
            titlebar: Some(TitlebarOptions {
                title: Some(t("CodeAny Editor").into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(13.))),
            }),
            window_decorations: window_decorations(),
            ..Default::default()
        };
        let (handle, editor) = gpui_kit::open_window(options, cx, move |window, cx| {
            crate::style::apply(window, cx);
            cx.new(|cx| Workspace::new_editor(machine, root, main, agent_panel, window, cx))
        })
        .ok()?;
        self.editor_window = Some((handle, editor.downgrade()));
        Some((handle, editor))
    }

    /// ⌘E: from the main window, its editor window; from an editor window, back to the main one.
    fn open_editor(&mut self, _: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Agent => {
                self.ensure_editor_window(window, cx);
            }
            Mode::Editor => self.with_main_window(cx, |_, _, _| {}),
        }
    }

    /// Activate the main window and run `f` on it, from an editor window.
    fn with_main_window(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    ) {
        let Some((handle, main)) = self.main_window.clone() else { return };
        let Some(main) = main.upgrade() else { return };
        let _ = handle.update(cx, |_, window, cx| {
            window.activate_window();
            main.update(cx, |main, cx| f(main, window, cx));
        });
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        window.open_dialog(cx, move |dialog, window, _| {
            // Centre it vertically; the dialog snaps up itself when it overflows.
            let top = ((window.viewport_size().height - px(crate::settings::HEIGHT)) / 2.).max(px(24.));
            dialog
                .w(px(crate::settings::WIDTH))
                .margin_top(top)
                .p_0()
                // The settings view draws its own background, border and
                // corners; the dialog's would peek out around them.
                .bg(gpui_kit::transparent_black())
                .border_0()
                // Enter belongs to the input it is typed in; it must not
                // take the dialog's "OK" and close the settings.
                .on_ok(|_, _, _| false)
                .close_button(false)
                .child(settings.clone())
        });
    }

    /// Open the search palette, or close it when it is already open.
    fn search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
            return;
        }
        let command = |label: &'static str, icon: IconName, action: Box<dyn Action>| search_palette::Item {
            icon,
            label: t(label).into(),
            agent: None,
            detail: None,
            shortcut: Kbd::binding_for_action(action.as_ref(), Some("Workspace"), window),
            target: Target::Action(action),
            updated_at: 0,
        };
        let quick_action = |label, icon, target| search_palette::Item {
            label,
            icon,
            agent: None,
            detail: None,
            shortcut: None,
            target,
            updated_at: 0,
        };
        let panel = self.agent_panel.read(cx);
        let mut commands = Vec::new();
        if let Some(project) = panel.draft_project() {
            let mut current = command("New Thread", IconName::SquarePen, Box::new(NewThread));
            current.label = tf("New thread in {project}", &[("project", crate::threads::project_label(&project))]);
            commands.push(current);
        }
        commands.extend([
            quick_action(t("New thread in another project…").into(), IconName::SquarePen, Target::ChooseThreadProject),
            quick_action(t("New thread without a project").into(), IconName::MessageSquare, Target::NewThreadInProject(None)),
            quick_action(t("New project").into(), IconName::FolderPlus, Target::NewProject),
            command("Settings…", IconName::Settings, Box::new(OpenSettings)),
        ]);
        let quick_count = commands.len();
        commands.extend([
            quick_action(t("Change theme").into(), IconName::Palette, Target::ChangeTheme),
            command("Open Folder…", IconName::FolderOpen, Box::new(OpenFolder)),
            command("Open Editor Window", IconName::Code, Box::new(OpenEditor)),
            command("Toggle Sidebar", IconName::PanelLeft, Box::new(ToggleSidebar)),
            command("Toggle Terminal", IconName::SquareTerminal, Box::new(ToggleTerminal)),
            command("Toggle Git Changes", IconName::GitCompare, Box::new(ToggleGitPanel)),
            command("Open a Surface…", IconName::PanelRight, Box::new(OpenSurface)),
            command("Save", IconName::Save, Box::new(Save)),
            command("Close Tab", IconName::X, Box::new(CloseTab)),
            command("Refresh", IconName::RefreshCw, Box::new(RefreshProject)),
        ]);
        let threads = panel.threads();
        let local_device = crate::remote::this_name();
        let others = threads
            .iter()
            .map(|thread| {
                let device = match thread.machine.as_deref() {
                    Some(id) => panel.machine_name(id, thread.machine_name.as_deref()),
                    None => local_device.clone(),
                };
                let project = thread.project.as_ref().map(|path| crate::threads::project_label(path));
                let mode = project.as_ref().map(|_| {
                    t(if thread.worktree.is_some() { "Worktree" } else { "Local" }).to_string()
                });
                let mut context = vec![device, project.unwrap_or_else(|| t("Chats only").into())];
                context.extend(mode);
                context.extend(panel.thread_branch(thread));
                search_palette::thread_item(thread, context.join(" · "))
            })
            .chain(panel.recent_projects().iter().map(search_palette::project_item))
            .collect();
        drop(threads);

        let palette = cx.new(|cx| SearchPalette::new(commands, Some(quick_count), others, window, cx));
        self.show_search_palette(palette, window, cx);
    }

    fn show_search_palette(
        &mut self,
        palette: Entity<SearchPalette>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._palette = Some(cx.subscribe_in(&palette, window, Self::on_palette_event));
        // Where a full-height palette's top would sit when centred.
        let height = window.viewport_size().height;
        let top = (height - (height * 0.7).min(px(search_palette::MAX_HEIGHT))) / 2.;
        let content = palette.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .w(px(search_palette::WIDTH))
                .margin_top(top)
                .p_0()
                .close_button(false)
                .on_ok(|_, _, _| false)
                .child(content.clone())
        });
        // The dialog takes focus as it opens; hand it to the search box after.
        window.defer(cx, move |window, cx| palette.update(cx, |palette, cx| palette.focus(window, cx)));
    }

    fn on_palette_event(
        &mut self,
        palette: &Entity<SearchPalette>,
        event: &SearchPaletteEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Close first: an action may open a dialog of its own.
        window.close_dialog(cx);
        self._palette = None;
        let SearchPaletteEvent::Pick(target) = event else {
            if let Some(previous) = palette.read(cx).previous() {
                self.show_search_palette(previous, window, cx);
            }
            return;
        };
        match target.clone() {
            Target::Action(action) => {
                self.focus_handle.focus(window, cx);
                window.dispatch_action(action, cx);
            }
            Target::ChooseThreadProject => {
                let panel = self.agent_panel.read(cx);
                let current = panel.draft_project();
                let options = panel
                    .device_projects()
                    .into_iter()
                    .filter(|path| Some(path) != current.as_ref())
                    .map(|path| {
                        let mut item = search_palette::project_item(&path);
                        item.target = Target::NewThreadInProject(Some(path));
                        item
                    })
                    .collect();
                let new_project = search_palette::Item {
                    icon: IconName::FolderPlus,
                    agent: None,
                    label: t("New project").into(),
                    detail: None,
                    shortcut: None,
                    target: Target::NewProject,
                    updated_at: 0,
                };
                let picker = cx.new(|cx| {
                    SearchPalette::new(vec![new_project], None, options, window, cx)
                        .with_previous(palette.clone())
                });
                self.show_search_palette(picker, window, cx);
            }
            Target::NewThreadInProject(project) => match self.mode {
                Mode::Agent => self.agent_panel.update(cx, |panel, cx| {
                    panel.new_thread_in_project(project, window, cx);
                }),
                Mode::Editor => self.with_main_window(cx, move |main, window, cx| {
                    main.agent_panel.update(cx, |panel, cx| panel.new_thread_in_project(project, window, cx));
                }),
            },
            Target::NewProject => match self.mode {
                Mode::Agent => self.agent_panel.update(cx, |panel, cx| panel.open_project_dialog(None, window, cx)),
                Mode::Editor => self.with_main_window(cx, |main, window, cx| {
                    main.agent_panel.update(cx, |panel, cx| panel.open_project_dialog(None, window, cx));
                }),
            },
            Target::ChangeTheme => {
                self.settings.update(cx, |settings, cx| settings.show_appearance(cx));
                self.open_settings(&OpenSettings, window, cx);
            }
            Target::Thread(id) => match self.mode {
                Mode::Agent => self
                    .agent_panel
                    .update(cx, |panel, cx| panel.select_thread(id, window, cx)),
                Mode::Editor => self.with_main_window(cx, move |main, window, cx| {
                    main.agent_panel
                        .update(cx, |panel, cx| panel.select_thread(id, window, cx));
                }),
            },
            Target::Project(path) => {
                if self.root.as_ref() != Some(&path) {
                    self.set_root(Some(path), window, cx);
                }
            }
        }
        cx.notify();
    }

    /// ⌘T: a new chat tab; in an editor window, the surfaces as before.
    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Agent => self.agent_panel.update(cx, |panel, cx| panel.new_tab(window, cx)),
            Mode::Editor => self.open_surface(&OpenSurface, window, cx),
        }
    }

    /// Run `f` on the agent panel's tabs, in the main window only.
    fn with_tabs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut AgentPanel, &mut Window, &mut Context<AgentPanel>),
    ) {
        if self.mode == Mode::Agent {
            self.agent_panel.update(cx, |panel, cx| f(panel, window, cx));
        }
    }

    fn new_thread(&mut self, _: &NewThread, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Agent => self
                .agent_panel
                .update(cx, |panel, cx| panel.new_thread(window, cx)),
            Mode::Editor => self.with_main_window(cx, |main, window, cx| {
                main.agent_panel
                    .update(cx, |panel, cx| panel.new_thread(window, cx));
            }),
        }
        cx.notify();
    }

    /// An empty stretch of title bar that drags the window.
    fn drag_region(id: &'static str) -> impl IntoElement {
        div()
            .id(id)
            .flex_1()
            .h_full()
            // Windows moves the window itself, and maximizes it on a double
            // click, from wherever the window says its caption is; it keeps
            // a strip to hold however many tabs crowd the bar.
            .when(cfg!(target_os = "windows"), |this| {
                this.min_w(px(32.)).window_control_area(WindowControlArea::Drag)
            })
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    if cfg!(target_os = "linux") {
                        window.zoom_window();
                    } else {
                        window.titlebar_double_click();
                    }
                } else {
                    window.start_window_move();
                }
            })
    }

    /// Close, minimize and maximize, where macOS puts its traffic lights:
    /// Linux and Windows leave them to a window that draws its own title bar.
    fn render_window_buttons(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = window.is_window_active();
        let supported = window.window_controls();
        let windows = cfg!(target_os = "windows");
        let button = |id: &'static str, color: u32, area: WindowControlArea| {
            div()
                .id(id)
                .size(px(12.))
                .flex_none()
                .rounded_full()
                .cursor_pointer()
                .bg(if active { rgb(color).into() } else { theme.border })
                .hover(|this| this.opacity(0.75))
                // A press here is for the button, not a drag of the window.
                .occlude()
                // Windows presses these itself once told which is which.
                .when(windows, |this| this.window_control_area(area))
                .when(!windows, |this| {
                    this.on_click(move |_, window, _| match area {
                        WindowControlArea::Close => window.remove_window(),
                        WindowControlArea::Min => window.minimize_window(),
                        _ => window.zoom_window(),
                    })
                })
        };
        div()
            .id("window-buttons")
            .absolute()
            .top_0()
            .left_0()
            .h(layout::HEADER)
            .w(px(TRAFFIC_LIGHTS_WIDTH))
            .pt(layout::HEADER_TOP_INSET)
            .pl(px(14.))
            .flex()
            .items_center()
            .gap(layout::SPACE_2)
            // The room around the buttons drags the window, under them so
            // that each button answers for itself.
            .child(
                div()
                    .id("window-buttons-drag")
                    .absolute()
                    .inset_0()
                    .when(windows, |this| this.window_control_area(WindowControlArea::Drag))
                    .when(!windows, |this| {
                        this.on_mouse_down(MouseButton::Left, |event, window, _| {
                            if event.click_count == 2 {
                                window.zoom_window();
                            }
                        })
                    }),
            )
            .child(button("window-close", 0xff5f57, WindowControlArea::Close))
            .when(supported.minimize, |this| this.child(button("window-minimize", 0xfebc2e, WindowControlArea::Min)))
            .when(supported.maximize, |this| this.child(button("window-maximize", 0x28c840, WindowControlArea::Max)))
    }

    /// The main window's way into its editor window.
    fn render_editor_button(&self, id: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        chrome_button(id)
            .child(Icon::new(IconName::SquareCode).size(ICON))
            .on_click(cx.listener(|this, _, window, cx| this.open_editor(&OpenEditor, window, cx)))
            .app_tooltip(t("Open Editor Window (⌘E)"))
    }

    fn render_new_chat_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .id("nav-chat")
            .h(layout::SIDEBAR_NAV_ROW)
            .flex_none()
            .px(layout::SIDEBAR_ROW_INSET)
            .flex().items_center().gap(layout::SPACE_2)
            .rounded(layout::RADIUS)
            .cursor_pointer().text_size(TEXT)
            .text_color(theme.foreground.opacity(0.85))
            .hover(|this| this.bg(theme.list_hover).text_color(theme.foreground))
            .child(
                Icon::new(IconName::MessageCirclePlus)
                    .size(ICON)
                    .flex_none()
                    .text_color(theme.muted_foreground),
            )
            .child(t("New Chat"))
            .on_click(cx.listener(|this, _, window, cx| this.new_thread(&NewThread, window, cx)))
    }

    fn render_sidebar(&mut self, reserve_window_buttons: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = match self.mode {
            Mode::Agent => crate::panel_views::cached(self.sidebar_view.clone()).into_any_element(),
            Mode::Editor => self.file_tree.clone().into_any_element(),
        };
        // Beside the window buttons: the workspace menu, as in Termany. The
        // sidebar, back and forward buttons sit at the left of the tab strip.
        let header: AnyElement = match self.mode {
            Mode::Agent => self.agent_panel.update(cx, |panel, cx| panel.render_workspace_switcher(cx)),
            Mode::Editor => div()
                .flex()
                .items_center()
                .child(
                    chrome_button("hide-sidebar")
                        .child(crate::icons::sidebar().size(ICON))
                        .text_color(theme.muted_foreground)
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx)))
                        .app_tooltip(t("Hide Sidebar (⌘B)")),
                )
                .into_any_element(),
        };
        let sidebar = div()
            .size_full().min_w_0().flex().flex_col().bg(theme.sidebar)
            .child(
                div()
                    .h(layout::HEADER).flex_none().flex().items_center()
                    .pt(layout::HEADER_TOP_INSET)
                    .pl(if reserve_window_buttons { px(TRAFFIC_LIGHTS_WIDTH) } else { layout::SIDEBAR_INSET })
                    .pr(layout::SIDEBAR_INSET)
                    .child(div().flex_1().min_w_0().child(header))
                    .when(self.mode == Mode::Editor, |this| this.child(Self::drag_region("sidebar-drag"))),
            )
            .when(self.mode == Mode::Agent, |this| this.child(
                div()
                    .flex_none()
                    .px(layout::SIDEBAR_INSET)
                    .pt(layout::SPACE_3)
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_1)
                    .child(div().flex_1().min_w_0().child(self.render_new_chat_button(cx)))
                    .child(
                            chrome_button("sidebar-search")
                                .child(crate::icons::search().size(ICON))
                                .text_color(theme.muted_foreground)
                                .accessibility_label(t("Search"))
                                .on_click(cx.listener(|this, _, window, cx| this.search(&Search, window, cx)))
                                .app_tooltip(t("Search")),
                    ),
            ))
            .child(div().flex_1().min_h_0().min_w_0().child(content));

        sidebar.when(self.mode == Mode::Agent, |this| this.child(self.render_sidebar_footer(cx)))
    }

    /// Settings and the optional app update in the sidebar footer.
    fn render_sidebar_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        #[cfg(target_os = "macos")]
        let update = cx
            .try_global::<crate::updater::AvailableUpdate>()
            .and_then(|update| update.0.clone())
            .filter(|_| self.mode == Mode::Agent);
        #[cfg(not(target_os = "macos"))]
        let update: Option<String> = None;
        div()
            .h(px(52.))
            .flex_none()
            .px(layout::SIDEBAR_INSET)
            .flex()
            .items_center()
            .gap(layout::SPACE_2)
            .child(
                Button::new("open-settings")
                    .ghost()
                    .h(layout::CONTROL)
                    .px_2()
                    .gap_2()
                    .text_color(theme.muted_foreground)
                    .app_icon(IconName::Settings)
                    .app_label(t("Settings"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(&OpenSettings, window, cx)))
                    .app_tooltip(t("Settings (⌘,)")),
            )
            .child(div().flex_1())
            .when_some(update, |this, version| {
                // The settings icon's size, filled so a waiting update is noticed.
                let size = crate::style::size::icon_pixels(cx);
                this.child(
                    chrome_button("install-update")
                        .child(
                            div()
                                .size(size)
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(theme.foreground)
                                .child(Icon::new(IconName::Download).size(size * 0.62).text_color(theme.background)),
                        )
                        .on_click(|_, _, _| {
                            #[cfg(target_os = "macos")]
                            crate::updater::check();
                        })
                        .app_tooltip(crate::i18n::tf("Update to v{version}", &[("version", version)])),
                )
            })
    }

    /// The agent window's tabs, one per open thread, drawn like a browser's:
    /// the active tab takes the page's color and flows into it, with curved
    /// feet; the others sit on the darker strip, split by thin rules.
    /// Type a new title for a tab's thread, in the tab itself.
    fn start_tab_rename(&mut self, id: u64, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(title, window, cx);
            input
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        // Enter or leaving the field keeps the title.
        self._rename_subscription = Some(cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.finish_tab_rename(true, cx);
            }
        }));
        self.renaming_tab = Some((id, input));
        cx.notify();
    }

    fn finish_tab_rename(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some((id, input)) = self.renaming_tab.take() else { return };
        self._rename_subscription = None;
        if keep {
            let title = input.read(cx).value().to_string();
            self.agent_panel.update(cx, |panel, cx| panel.rename_thread(id, &title, cx));
        }
        cx.notify();
    }

    fn render_chat_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        const RADIUS: Pixels = px(8.);
        const RULE_HEIGHT: Pixels = px(14.);
        let theme = cx.theme().clone();
        let (tabs, active) = self.agent_panel.read(cx).tab_items();
        let count = tabs.len();
        let closable = count > 1 || self.agent_panel.read(cx).has_open_thread();
        let icon_size = crate::style::size::icon_pixels(cx);
        // Paint only the curve so the strip or adjacent hover shows through.
        let foot = |left: bool| {
            let background = theme.background;
            div()
                .absolute()
                .bottom_0()
                .size(RADIUS)
                .child(
                    canvas(|_, _, _| (), move |bounds, (), window, _| {
                        let bottom_right = bounds.bottom_right();
                        let (outer_x, inner_x) = if left {
                            (bounds.origin.x, bottom_right.x)
                        } else {
                            (bottom_right.x, bounds.origin.x)
                        };
                        let mut path = PathBuilder::fill();
                        path.move_to(point(outer_x, bottom_right.y));
                        path.arc_to(point(RADIUS, RADIUS), px(0.), false, !left, point(inner_x, bounds.origin.y));
                        path.line_to(point(inner_x, bottom_right.y));
                        path.close();
                        if let Ok(path) = path.build() {
                            window.paint_path(path, background);
                        }
                    })
                    .size_full(),
                )
        };
        div()
            .id("chat-tabs")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_end()
            // Leave a gap between the sidebar divider and the active tab's foot.
            .pl(RADIUS + layout::SPACE_2)
            .children(tabs.into_iter().enumerate().map(|(ix, tab)| {
                let selected = ix == active;
                // Rules split two inactive tabs; none touches the active one.
                let rule = ix + 1 < count && !selected && ix + 1 != active;
                let icon = if tab.running {
                    Icon::new(IconName::LoaderCircle)
                        .size(ICON)
                        .text_color(theme.primary)
                        .with_animation(
                            ("chat-tab-spin", ix),
                            Animation::new(std::time::Duration::from_secs(1)).repeat(),
                            |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
                        )
                        .into_any_element()
                } else {
                    match &tab.agent {
                        Some(agent) => crate::agent_icons::agent_icon(agent, icon_size, cx),
                        // Its glyph fills the box, so a size down matches the other icons.
                        None => Icon::new(IconName::MessageCirclePlus).size(crate::style::size::TAB_ACTION_ICON).into_any_element(),
                    }
                };
                div()
                    .id(("chat-tab", ix))
                    .group("chat-tab")
                    .relative()
                    // Full width until crowded, then all shrink alike.
                    .w(px(220.))
                    .flex_shrink(1.)
                    .min_w(px(56.))
                    .h_full()
                    .pl_3()
                    .pr_1p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_t(RADIUS)
                    .text_size(TEXT)
                    .cursor_pointer()
                    .map(|this| {
                        if selected {
                            this.bg(theme.background)
                                .text_color(theme.foreground)
                                .when(ix == 0, |this| this.child(foot(true).left(-RADIUS)))
                                .when(ix + 1 == count, |this| this.child(foot(false).right(-RADIUS)))
                        } else {
                            // Draw shared corners over the inactive tab's hover fill.
                            this.text_color(theme.muted_foreground)
                                .hover(|this| this.bg(theme.list_hover).text_color(theme.foreground))
                                .when(ix + 1 == active, |this| this.child(foot(true).right_0()))
                                .when(ix == active + 1, |this| this.child(foot(false).left_0()))
                        }
                    })
                    .child(div().flex_none().flex().items_center().child(icon))
                    .child(match (&self.renaming_tab, tab.id) {
                        (Some((renaming, input)), Some(id)) if *renaming == id => div()
                            .flex_1()
                            .min_w_0()
                            // Clicks in the field stay there: the tab would
                            // otherwise take them and move focus away.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            // Esc puts the old title back.
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                if event.keystroke.key == "escape" {
                                    cx.stop_propagation();
                                    this.finish_tab_rename(false, cx);
                                }
                            }))
                            .child(gpui_kit::component::Sizable::small(Input::new(input).text_size(TEXT)))
                            .into_any_element(),
                        _ => div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(tab.title.clone())
                            .into_any_element(),
                    })
                    .when(closable, |this| {
                        this.child(
                            div()
                                .id(("chat-tab-close", ix))
                                .size(px(18.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .text_color(theme.muted_foreground)
                                .hover(|this| this.bg(theme.foreground.opacity(0.1)).text_color(theme.foreground))
                                .child(Icon::new(IconName::X).size(TAB_ACTION_ICON))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.agent_panel.update(cx, |panel, cx| panel.close_tab(Some(ix), window, cx));
                                })),
                        )
                    })
                    .when(rule, |this| {
                        this.child(
                            div()
                                .absolute()
                                .right_0()
                                .top((layout::HEADER - layout::HEADER_TOP_INSET - RULE_HEIGHT) / 2.)
                                .h(RULE_HEIGHT)
                                .w(px(1.))
                                .bg(theme.border)
                                .group_hover("chat-tab", |this| this.invisible()),
                        )
                    })
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| {
                        this.agent_panel.update(cx, |panel, cx| panel.close_tab(Some(ix), window, cx));
                    }))
                    .on_click(cx.listener({
                        let (id, title) = (tab.id, tab.title.clone());
                        move |this, event: &ClickEvent, window, cx| {
                            this.agent_panel.update(cx, |panel, cx| panel.select_tab(ix, window, cx));
                            // A double click renames a thread's tab.
                            if event.click_count() == 2 && let Some(id) = id {
                                this.start_tab_rename(id, title.clone(), window, cx);
                            }
                        }
                    }))
                    .context_menu({
                        let workspace = cx.entity().downgrade();
                        let (id, title) = (tab.id, tab.title.clone());
                        move |menu, _, _| {
                            let (rename, close, others, right, reopen) =
                                (workspace.clone(), workspace.clone(), workspace.clone(), workspace.clone(), workspace.clone());
                            let title = title.clone();
                            menu.when_some(id, |menu, id| {
                                menu.item(crate::style::menu_item(t("Rename")).on_click(move |_, window, cx| {
                                    let title = title.clone();
                                    rename.update(cx, |this, cx| this.start_tab_rename(id, title, window, cx)).ok();
                                }))
                                .separator()
                            })
                            .item(crate::style::menu_item(t("Close Tab")).on_click(move |_, window, cx| {
                                close
                                    .update(cx, |this, cx| {
                                        this.agent_panel.update(cx, |panel, cx| panel.close_tab(Some(ix), window, cx))
                                    })
                                    .ok();
                            }))
                            .item(crate::style::menu_item(t("Close Other Tabs")).disabled(count < 2).on_click(move |_, window, cx| {
                                others
                                    .update(cx, |this, cx| {
                                        this.agent_panel.update(cx, |panel, cx| panel.close_other_tabs(ix, window, cx))
                                    })
                                    .ok();
                            }))
                            .item(crate::style::menu_item(t("Close Tabs to the Right")).disabled(ix + 1 >= count).on_click(move |_, window, cx| {
                                right
                                    .update(cx, |this, cx| {
                                        this.agent_panel.update(cx, |panel, cx| panel.close_tabs_to_right(ix, window, cx))
                                    })
                                    .ok();
                            }))
                            .separator()
                            .item(crate::style::menu_item(t("Reopen Closed Tab")).on_click(move |_, window, cx| {
                                reopen
                                    .update(cx, |this, cx| this.agent_panel.update(cx, |panel, cx| panel.reopen_tab(window, cx)))
                                    .ok();
                            }))
                        }
                    })
            }))
            .child(
                div().h_full().flex_none().flex().items_center().pl_1()
                    .child(
                        chrome_button("new-chat-tab")
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(IconName::Plus).size(TAB_ACTION_ICON))
                            .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)))
                            .app_tooltip(t("New Tab (⌘T)")),
                    ),
            )
            // The strip past the tabs drags the window.
            .child(Self::drag_region("chat-tabs-drag"))
    }

    fn set_overview_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.overview_open = open;
        self.overview_git = None;
        self.overview_poll = open.then(|| cx.spawn(async move |this, cx| {
            loop {
                let Ok((machine, dir)) = this.read_with(cx, |this, _| this.conversation_location()) else {
                    break;
                };
                if let Some(dir) = dir {
                    let target = (machine, dir);
                    let (machine, dir) = target.clone();
                    let snapshot = cx.background_executor()
                        .spawn(async move { git::snapshot(machine.as_deref(), &dir) })
                        .await;
                    if this.update(cx, |this, cx| {
                        let location = this.conversation_location();
                        if location.0 == target.0 && location.1.as_ref() == Some(&target.1) {
                            let result = Some((target, snapshot));
                            if this.overview_git != result {
                                this.overview_git = result;
                                cx.notify();
                            }
                        }
                    }).is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(POLL_INTERVAL).await;
            }
        }));
        cx.notify();
    }

    fn render_project_overview(&self, cx: &Context<Self>) -> impl IntoElement {
        let workspace = cx.entity().downgrade();
        let theme = cx.theme();
        Popover::new("project-overview")
            .anchor(Anchor::TopRight)
            .open(self.overview_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| this.set_overview_open(*open, cx)))
            .p_0()
            .trigger(
                chrome_button("project-overview-button")
                    .child(Icon::new(IconName::SquareMenu).size(ICON))
                    .text_color(if self.overview_open { theme.primary } else { theme.muted_foreground })
                    .accessibility_label(t("Project Overview")),
            )
            .content(move |_, _, cx| {
                workspace.read_with(cx, |this, cx| this.project_overview_content(workspace.clone(), cx))
                    .unwrap_or_else(|_| div().into_any_element())
            })
    }

    fn project_overview_content(&self, workspace: WeakEntity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let (machine, dir) = self.conversation_location();
        let device = machine.as_deref()
            .map(|id| self.agent_panel.read(cx).machine_name(id, None))
            .unwrap_or_else(crate::remote::this_name);
        let snapshot = self.overview_git.as_ref()
            .filter(|((id, path), _)| id == &machine && Some(path) == dir.as_ref());
        let project = self.agent_panel.read(cx).conversation_project();
        let project_name = project.as_deref().map(crate::threads::project_label)
            .unwrap_or_else(|| t("No project").into());
        let checkout = match (&dir, &project) {
            (Some(dir), Some(project)) if dir != project => t("Worktree"),
            (_, Some(_)) => t("Local"),
            _ => t("No project"),
        };
        let git = snapshot.and_then(|(_, git)| git.as_ref());
        let branch = match git {
            Some(git) => git.branch.as_deref().filter(|branch| *branch != "HEAD")
                .unwrap_or_else(|| t("Detached HEAD")),
            None if dir.is_some() && snapshot.is_none() => t("Loading…"),
            None => t("Terminal"),
        };
        let diff = git.map(|git| format!("{} ({})", t("Diff"), git.statuses.len()))
            .unwrap_or_else(|| t("Diff").into());
        let row = |id, icon, label: String, trailing| {
            div().id(id).h(layout::CONTROL).px(layout::SPACE_2).rounded(layout::RADIUS)
                .flex().items_center().gap(layout::SPACE_2).min_w_0()
                .child(Icon::new(icon).size(ICON).flex_none().text_color(theme.muted_foreground))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(label))
                .child(Icon::new(trailing).size(ICON).flex_none().text_color(theme.muted_foreground))
                .cursor_pointer().hover(|this| this.bg(theme.list_hover))
        };
        let open_surface = |surface| {
            let workspace = workspace.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                let _ = workspace.update(cx, |this, cx| {
                    if this.surfaces.update(cx, |host, cx| host.open(surface, window, cx)) {
                        this.set_overview_open(false, cx);
                    }
                });
            }
        };
        let devices_workspace = workspace.clone();
        let project_workspace = workspace.clone();
        div().w(px(320.)).p(layout::SPACE_2).flex().flex_col().gap(layout::SPACE_1).text_size(TEXT)
            .child(
                row("overview-device", IconName::Monitor, device, IconName::ChevronRight)
                    .on_click(move |_, window, cx| {
                        let _ = devices_workspace.update(cx, |this, cx| {
                            this.set_overview_open(false, cx);
                            this.settings.update(cx, |settings, cx| settings.show_devices(cx));
                            this.open_settings(&OpenSettings, window, cx);
                        });
                    }),
            )
            .when_some(project, |this, project| this.child(
                row("overview-project", IconName::Folder, project_name, IconName::ExternalLink)
                    .aria_label(t("Open in Editor"))
                    .on_click(move |_, window, cx| {
                        let _ = project_workspace.update(cx, |this, cx| {
                            this.set_overview_open(false, cx);
                            this.open_editor_window(machine.clone(), Some(project.clone()), window, cx);
                        });
                    }),
            ))
            .when(dir.is_some(), |this| this.child(
                row("overview-worktree", IconName::GitFork, checkout.into(), IconName::Files)
                    .on_click(open_surface("files")),
            ))
            .child(
                row("overview-branch", IconName::GitBranch, branch.into(), IconName::SquareTerminal)
                    .on_click(open_surface("terminal")),
            )
            .when(git.is_some(), |this| this.child(
                row("overview-diff", IconName::FileDiff, diff, IconName::ChevronRight)
                    .on_click(open_surface("diff")),
            ))
            .into_any_element()
    }

    fn render_header(&mut self, reserve_window_buttons: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let window_button_inset = if reserve_window_buttons { px(TRAFFIC_LIGHTS_WIDTH) } else { layout::SPACE_2 };
        let project: SharedString = self
            .root
            .as_ref()
            .map(|p| crate::threads::project_label(p))
            .unwrap_or_else(|| t("No project").into())
            .into();
        let current: Option<String> = match self.mode {
            // The agent window shows its tabs instead.
            Mode::Agent => None,
            Mode::Editor => self
                .active_tab
                .and_then(|ix| self.tabs.get(ix))
                .map(|tab| self.relative_path(&tab.path)),
        };

        div()
            .h(layout::HEADER)
            .pt(layout::HEADER_TOP_INSET)
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .pr_2()
            .when(self.mode == Mode::Agent, |this| this.bg(theme.tab_bar))
            .when(self.mode == Mode::Agent, |this| {
                let can_back = self.agent_panel.read(cx).can_navigate(false);
                let can_forward = self.agent_panel.read(cx).can_navigate(true);
                let shown = self.show_sidebar;
                // Left of the tabs, as in Termany: past the window buttons
                // when the sidebar is hidden.
                this.pl(if shown { layout::SPACE_2 } else { window_button_inset })
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .child(
                                chrome_button("toggle-sidebar")
                                    .child(crate::icons::sidebar().size(ICON))
                                    .text_color(theme.muted_foreground)
                                    .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx)))
                                    .app_tooltip(if shown { t("Hide Sidebar (⌘B)") } else { t("Show Sidebar (⌘B)") }),
                            )
                            .child(
                                chrome_button("conversation-back")
                                    .disabled(!can_back)
                                    .child(Icon::new(IconName::ChevronLeft).size(ICON))
                                    .text_color(theme.muted_foreground)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.agent_panel.update(cx, |panel, cx| panel.navigate(false, window, cx));
                                    }))
                                    .app_tooltip(t("Previous conversation")),
                            )
                            .child(
                                chrome_button("conversation-forward")
                                    .disabled(!can_forward)
                                    .child(Icon::new(IconName::ChevronRight).size(ICON))
                                    .text_color(theme.muted_foreground)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.agent_panel.update(cx, |panel, cx| panel.navigate(true, window, cx));
                                    }))
                                    .app_tooltip(t("Next conversation")),
                            ),
                    )
            })
            .when(self.mode == Mode::Editor, |this| this.map(|this| {
                if self.show_sidebar {
                    this.pl_4()
                } else {
                    this.pl(window_button_inset).child(
                        chrome_button("show-sidebar")
                            .child(crate::icons::sidebar().size(ICON))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_sidebar(&ToggleSidebar, window, cx)
                            }))
                            .app_tooltip(t("Show Sidebar (⌘B)")),
                    )
                }
            }))
            // The agent window's tabs say enough; the editor names its project.
            .when(self.mode == Mode::Editor, |this| this.child(
                div()
                    .id("breadcrumb-project")
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .cursor_pointer()
                    .hover(|this| this.text_color(theme.foreground))
                    .child(Icon::new(IconName::Folder).size(ICON))
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(project))
                    .on_click(cx.listener(|this, _, window, cx| this.open_folder(&OpenFolder, window, cx))),
            ))
            .when_some(current, |this, current| {
                this.child(div().text_size(TEXT).text_color(theme.muted_foreground).child("/"))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(TEXT)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(current),
                    )
            })
            .when(self.mode == Mode::Agent, |this| this.child(self.render_chat_tabs(cx)))
            .when(self.mode == Mode::Editor, |this| {
                this.child(Self::drag_region("header-drag"))
                    .child(
                        chrome_button("back-to-codeany")
                            .app_icon(IconName::Code)
                            .on_click(cx.listener(|this, _, window, cx| this.open_editor(&OpenEditor, window, cx)))
                            .app_tooltip(t("Return to CodeAny")),
                    )
            })
            .when(!self.show_sidebar && self.mode == Mode::Agent, |this| {
                this.child(self.render_editor_button("header-editor", cx))
            })
            .when(self.mode == Mode::Agent, |this| this.child(self.render_project_overview(cx)))
            .when(!self.surfaces.read(cx).is_visible(), |this| {
                this.child(
                    chrome_button("toggle-surfaces")
                        .app_icon(IconName::PanelRight)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.surfaces.update(cx, |surfaces, cx| surfaces.set_visible(true, window, cx))
                        }))
                        .app_tooltip(t("Surfaces (⌥⌘T)")),
                )
            })
    }

    /// Reserve the conversation's minimum width before applying the sidebar
    /// preferences. Temporarily narrower panels regain their width as room returns.
    fn side_widths(&self, width: Pixels, surfaces_visible: bool) -> (Pixels, Pixels) {
        let surfaces_min = if surfaces_visible { SURFACES_MIN } else { px(0.) };
        let sidebar = if self.show_sidebar {
            self.sidebar_width.min((width - CONVERSATION_MIN - surfaces_min).max(px(0.)))
        } else {
            px(0.)
        };
        let surfaces = if surfaces_visible {
            self.surfaces_width.min((width - sidebar - CONVERSATION_MIN).max(px(0.)))
        } else {
            px(0.)
        };
        (sidebar, surfaces)
    }

    fn resize_panel_at(
        &mut self,
        divider: PanelDivider,
        width: Pixels,
        pointer: Pixels,
        cx: &mut Context<Self>,
    ) {
        let (sidebar, surfaces) = self.side_widths(width, self.surfaces.read(cx).is_visible());
        let (preferred, next) = match divider {
            PanelDivider::Sidebar => {
                let max = (width - surfaces - CONVERSATION_MIN).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
                (&mut self.sidebar_width, pointer.clamp(SIDEBAR_MIN, max))
            }
            PanelDivider::Surfaces => {
                let max = (width - sidebar - CONVERSATION_MIN).clamp(SURFACES_MIN, SURFACES_MAX);
                (&mut self.surfaces_width, (width - pointer).clamp(SURFACES_MIN, max))
            }
        };
        if *preferred != next {
            *preferred = next;
            cx.notify();
        }
    }

    fn panel_divider(divider: PanelDivider, cx: &Context<Self>) -> impl IntoElement {
        let (id, edge) = match divider {
            PanelDivider::Sidebar => ("sidebar-divider", HandleEdge::Trailing),
            PanelDivider::Surfaces => ("surfaces-divider", HandleEdge::Leading),
        };
        let workspace = cx.entity().downgrade();
        resize_handle(id, Axis::Horizontal)
            .inside(edge)
            .on_drag(divider, move |divider, _, window, cx| {
                // GPUI starts the drag after the pointer has already moved.
                let inset = window_paddings(window);
                let width = window.viewport_size().width - inset.left - inset.right;
                let pointer = window.mouse_position().x - inset.left;
                let _ = workspace.update(cx, |this, cx| this.resize_panel_at(*divider, width, pointer, cx));
                cx.new(|_| *divider)
            })
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // This window's workspace's theme, before anything reads it.
        let choice = self.agent_panel.read(cx).theme_choice();
        crate::style::use_theme(&choice, window, cx);
        let theme = cx.theme().clone();

        let reserve_window_buttons = !cfg!(target_os = "macos") || !window.is_fullscreen();
        let content = match self.mode {
            Mode::Agent => crate::panel_views::cached(self.chat_view.clone()).into_any_element(),
            Mode::Editor => self.render_editor_area(cx),
        };

        let center = div()
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(self.render_header(reserve_window_buttons, cx))
            .child(div().flex_1().min_h_0().min_w_0().child(content));

        let surfaces_visible = self.surfaces.read(cx).is_visible();
        let inset = window_paddings(window);
        let width = window.viewport_size().width - inset.left - inset.right;
        let (sidebar_width, surfaces_width) = self.side_widths(width, surfaces_visible);
        let main = div()
            .id("workspace-panels")
            .size_full()
            .flex()
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<PanelDivider>, _, cx| {
                this.resize_panel_at(
                    *event.drag(cx),
                    event.bounds.size.width,
                    event.event.position.x - event.bounds.left(),
                    cx,
                );
            }))
            .when(self.show_sidebar, |main| {
                main.child(
                    div().relative().flex_none().w(sidebar_width).h_full()
                        .child(self.render_sidebar(reserve_window_buttons, cx))
                        .child(Self::panel_divider(PanelDivider::Sidebar, cx)),
                )
            })
            .child(center)
            .when(surfaces_visible, |main| {
                main.child(
                    div().relative().flex_none().w(surfaces_width).h_full()
                        .child(self.surfaces.clone())
                        .child(Self::panel_divider(PanelDivider::Surfaces, cx)),
                )
            });

        div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_git_panel))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::open_surface))
            .on_action(cx.listener(Self::refresh_project))
            .on_action(cx.listener(Self::open_editor))
            .on_action(cx.listener(Self::new_thread))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(|this, _: &ReopenTab, window, cx| {
                this.with_tabs(window, cx, |panel, window, cx| panel.reopen_tab(window, cx))
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                this.with_tabs(window, cx, |panel, window, cx| panel.cycle_tab(1, window, cx))
            }))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| {
                this.with_tabs(window, cx, |panel, window, cx| panel.cycle_tab(-1, window, cx))
            }))
            .on_action(cx.listener(|this, _: &SelectTab1, window, cx| this.with_tabs(window, cx, |p, w, cx| p.select_tab(0, w, cx))))
            .on_action(cx.listener(|this, _: &SelectTab2, window, cx| this.with_tabs(window, cx, |p, w, cx| p.select_tab(1, w, cx))))
            .on_action(cx.listener(|this, _: &SelectTab3, window, cx| this.with_tabs(window, cx, |p, w, cx| p.select_tab(2, w, cx))))
            .on_action(cx.listener(|this, _: &SelectTab4, window, cx| this.with_tabs(window, cx, |p, w, cx| p.select_tab(3, w, cx))))
            .on_action(cx.listener(|this, _: &SelectTab5, window, cx| this.with_tabs(window, cx, |p, w, cx| p.select_tab(4, w, cx))))
            .on_action(cx.listener(|this, _: &SelectTab6, window, cx| this.with_tabs(window, cx, |p, w, cx| p.select_tab(5, w, cx))))
            .on_action(cx.listener(|this, _: &SelectTab7, window, cx| this.with_tabs(window, cx, |p, w, cx| p.select_tab(6, w, cx))))
            .on_action(cx.listener(|this, _: &SelectTab8, window, cx| this.with_tabs(window, cx, |p, w, cx| p.select_tab(7, w, cx))))
            .on_action(cx.listener(|this, _: &SelectLastTab, window, cx| {
                this.with_tabs(window, cx, |panel, window, cx| panel.select_last_tab(window, cx))
            }))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::search))
            // Linux has no title bar of its own here: a press anywhere in
            // the header row drags the window once it has moved far enough.
            .when(cfg!(target_os = "linux"), |this| {
                this.on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, _| {
                    let top = event.position.y - window_paddings(window).top;
                    this.window_drag = (event.click_count == 1 && top < layout::HEADER).then_some(event.position);
                }))
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                    let Some(from) = this.window_drag else { return };
                    // Let go, or dragging something of its own, such as a panel divider.
                    if !event.dragging() || cx.has_active_drag() {
                        this.window_drag = None;
                        return;
                    }
                    let moved = event.position - from;
                    if moved.x.abs() > WINDOW_DRAG_SLOP || moved.y.abs() > WINDOW_DRAG_SLOP {
                        this.window_drag = None;
                        window.start_window_move();
                    }
                }))
            })
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(div().flex_1().min_h_0().child(main))
            .when(draws_window_buttons(window), |this| this.child(self.render_window_buttons(window, cx)))
            .when(self.mode == Mode::Editor, |this| {
                this.child(self.render_status_bar(cx))
            })
    }
}
