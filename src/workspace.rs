//! The main window: sidebar, conversation or editor, and the surfaces panel.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, kbd::Kbd, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Editor, EditorState, InputEvent},
    notification::Notification,
    menu::{DropdownMenu as _, PopupMenuItem},
    Side,
    resizable::{h_resizable, resizable_panel},
    WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::assets::IconName;

use crate::{
    CloseTab, NewThread, OpenFolder, OpenSettings, OpenSurface, RefreshProject, Save, Search,
    OpenEditor, ToggleGitPanel, ToggleSidebar, ToggleTerminal,
    agent_panel::{AgentPanel, AgentPanelEvent},
    search_palette::{self, Badge, SearchPalette, SearchPaletteEvent, Target},
    file_tree::{FileTree, FileTreeEvent},
    git::{self, RepoSnapshot},
    surface_host::{SurfaceHost, SurfaceHostEvent},
};
use crate::style::chrome_button;
use crate::style::layout;
use crate::style::size::{HEADING, ICON, ICON_LG, ICON_SM, TEXT, TEXT_SM};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_secs(3);

enum TabContent {
    Editor(Entity<EditorState>),
    Message(SharedString),
}

struct EditorTab {
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

/// Room for the macOS traffic lights in the transparent title bar.
const TRAFFIC_LIGHTS_WIDTH: f32 = 86.;

pub struct Workspace {
    root: Option<PathBuf>,
    /// The main window shows the agent; an editor window, opened from it, the editor.
    mode: Mode,
    /// From the main window: its editor window, while open.
    editor_window: Option<(AnyWindowHandle, WeakEntity<Workspace>)>,
    /// From an editor window: the main window that opened it.
    main_window: Option<(AnyWindowHandle, WeakEntity<Workspace>)>,
    agent_panel: Entity<AgentPanel>,
    settings: Entity<crate::settings::Settings>,
    file_tree: Entity<FileTree>,
    surfaces: Entity<SurfaceHost>,
    tabs: Vec<EditorTab>,
    active_tab: Option<usize>,
    git: RepoSnapshot,
    show_sidebar: bool,
    focus_handle: FocusHandle,
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
        return Err(format!("File is too large to open ({} MB)", meta.len() / 1024 / 1024).into());
    }
    let bytes = std::fs::read(path).map_err(|e| SharedString::from(e.to_string()))?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err("Binary file is not displayed".into());
    }
    String::from_utf8(bytes).map_err(|_| "File is not valid UTF-8".into())
}

impl Workspace {
    /// The main window, showing the agent.
    pub fn new(root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let agent_panel = cx.new(|cx| AgentPanel::new(window, cx));
        let this = Self::build(root, Mode::Agent, agent_panel, None, window, cx);
        let handle = this.agent_panel.read(cx).composer_focus_handle(cx);
        handle.focus(window, cx);
        this
    }

    /// An editor window for `root`. It shares the main window's agent panel
    /// (for settings and the search palette) without showing or steering it.
    fn new_editor(
        root: Option<PathBuf>,
        main_window: (AnyWindowHandle, WeakEntity<Workspace>),
        agent_panel: Entity<AgentPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let this = Self::build(root, Mode::Editor, agent_panel, Some(main_window), window, cx);
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
        let surfaces = cx.new(|_| SurfaceHost::new());

        let mut subscriptions = vec![
            cx.subscribe_in(
                &file_tree,
                window,
                |this, _, event: &FileTreeEvent, window, cx| match event {
                    FileTreeEvent::OpenFile(path) => this.open_file(path.clone(), window, cx),
                },
            ),
            cx.subscribe_in(&surfaces, window, |this, _, event: &SurfaceHostEvent, window, cx| match event {
                SurfaceHostEvent::OpenFile(path) => this.show_file(path.clone(), window, cx),
            }),
            cx.observe(&surfaces, |_, _, cx| cx.notify()),
            cx.observe_window_appearance(window, |_, window, cx| crate::style::apply(window, cx)),
        ];
        if mode == Mode::Agent {
            subscriptions.push(cx.subscribe_in(&agent_panel, window, Self::on_agent_event));
            subscriptions.push(cx.observe(&agent_panel, |_, _, cx| cx.notify()));
        }

        let mut this = Self {
            root: None,
            mode,
            editor_window: None,
            main_window,
            settings: cx.new(|_| crate::settings::Settings::new(&agent_panel)),
            agent_panel,
            file_tree,
            surfaces,
            tabs: Vec::new(),
            active_tab: None,
            git: RepoSnapshot::default(),
            show_sidebar: true,
            focus_handle: cx.focus_handle(),
            _palette: None,
            _poll_task: None,
            _subscriptions: subscriptions,
        };
        if root.is_some() {
            this.set_root(root, window, cx);
        }
        this
    }

    fn set_root(&mut self, root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.root = root.clone();
        self.tabs.clear();
        self.active_tab = None;
        self.git = RepoSnapshot::default();
        self.file_tree
            .update(cx, |tree, cx| tree.set_root(root.clone(), cx));
        self.surfaces
            .update(cx, |surfaces, cx| surfaces.set_project(root.clone(), window, cx));
        if self.mode == Mode::Agent {
            self.agent_panel
                .update(cx, |panel, cx| panel.set_project(root.clone(), cx));
        }

        let app = match self.mode {
            Mode::Agent => "dscode",
            Mode::Editor => "dscode editor",
        };
        let title = root
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| format!("{} — {app}", n.to_string_lossy()))
            .unwrap_or_else(|| app.into());
        window.set_window_title(&title);

        self._poll_task = root.map(|_| {
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    let Ok(root) = this.read_with(cx, |this, _| this.root.clone()) else {
                        break;
                    };
                    let Some(root) = root else { break };
                    let snapshot = cx
                        .background_executor()
                        .spawn(async move { git::snapshot(&root) })
                        .await;
                    let alive = this.update_in(cx, |this, window, cx| {
                        this.apply_snapshot(snapshot.unwrap_or_default(), window, cx);
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
    fn apply_snapshot(&mut self, snapshot: RepoSnapshot, window: &mut Window, cx: &mut Context<Self>) {
        let changed = snapshot != self.git;
        self.git = snapshot.clone();
        self.file_tree.update(cx, |tree, cx| {
            tree.set_git(snapshot, cx);
            tree.refresh(cx);
        });
        if changed {
            self.surfaces.update(cx, |surfaces, cx| surfaces.refresh(window, cx));
        }
        self.reload_changed_tabs(window, cx);
        cx.notify();
    }

    fn reload_changed_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for tab in &mut self.tabs {
            if tab.dirty {
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

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|tab| tab.path == path) {
            self.activate_tab(ix, window, cx);
            return;
        }

        let (content, subscription) = match load_text(&path) {
            Ok(text) => {
                let language = language_for(&path);
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
            Err(message) => (TabContent::Message(message), None),
        };

        let ix = self.active_tab.map(|ix| ix + 1).unwrap_or(self.tabs.len());
        self.tabs.insert(
            ix,
            EditorTab {
                modified_at: modified_at(&path),
                path,
                content,
                dirty: false,
                _subscription: subscription,
            },
        );
        self.activate_tab(ix, window, cx);
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
            prompt: Some("Open".into()),
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
        match std::fs::write(&tab.path, text.as_bytes()) {
            Ok(()) => {
                tab.dirty = false;
                tab.modified_at = modified_at(&tab.path);
                self.surfaces.update(cx, |surfaces, cx| surfaces.refresh(window, cx));
            }
            Err(err) => {
                let message = format!("Failed to save {}: {err}", tab.path.display());
                window.push_notification(Notification::error(message), cx);
            }
        }
        cx.notify();
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
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
        self.surfaces.update(cx, |surfaces, cx| surfaces.show_picker(window, cx));
    }

    fn refresh_project(&mut self, _: &RefreshProject, window: &mut Window, cx: &mut Context<Self>) {
        self.file_tree.update(cx, |tree, cx| tree.refresh(cx));
        self.surfaces.update(cx, |surfaces, cx| surfaces.refresh(window, cx));
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
                let status = self.git.statuses.get(&tab.path).copied();
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
                            .size(px(20.))
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
                                            .xsmall()
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
                    .text_size(TEXT_SM)
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
                .child(div().text_color(theme.muted_foreground).child(label))
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
                    .text_size(HEADING)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child("dscode"),
            )
            .when(self.root.is_none(), |this| {
                this.child(
                    Button::new("open-folder")
                        .primary()
                        .label("Open Folder…")
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
                Some(format!("Ln {}, Col {}", position.line + 1, position.character + 1))
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
            .text_size(TEXT_SM)
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
                                .child(Icon::new(IconName::GitBranch).size(ICON_SM))
                                .child(branch),
                        )
                    })
                    .when(!self.git.statuses.is_empty(), |this| {
                        this.child(format!("{} changed", self.git.statuses.len()))
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
                let path = match (&self.root, path.is_relative()) {
                    (Some(root), true) => root.join(path),
                    _ => path.clone(),
                };
                self.show_file(path, window, cx);
            }
            AgentPanelEvent::SwitchProject(project) => {
                if *project != self.root {
                    self.set_root(project.clone(), window, cx);
                }
            }
            AgentPanelEvent::OpenFolder => self.open_folder(&OpenFolder, window, cx),
            AgentPanelEvent::TurnFinished => {
                self.file_tree.update(cx, |tree, cx| tree.refresh(cx));
                self.surfaces.update(cx, |surfaces, cx| surfaces.refresh(window, cx));
            }
        }
    }

    /// Open a file in the editor: here in an editor window, or in the main
    /// window's editor window.
    fn show_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Editor => self.open_file(path, window, cx),
            Mode::Agent => {
                if let Some((handle, editor)) = self.ensure_editor_window(window, cx) {
                    let _ = handle.update(cx, |_, window, cx| {
                        editor.update(cx, |editor, cx| editor.open_file(path, window, cx))
                    });
                }
            }
        }
    }

    /// Bring up this project's editor window, opening it when there is none.
    fn ensure_editor_window(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
        let root = self.root.clone();
        if let Some((handle, editor)) = &self.editor_window
            && let Some(editor) = editor.upgrade()
        {
            let handle = *handle;
            let root = root.clone();
            let shown = handle.update(cx, |_, window, cx| {
                window.activate_window();
                editor.update(cx, |editor, cx| {
                    // Follow the main window's project, unless that would drop unsaved edits.
                    if editor.root != root && !editor.tabs.iter().any(|tab| tab.dirty) {
                        editor.set_root(root, window, cx);
                    }
                })
            });
            if shown.is_ok() {
                return Some((handle, editor));
            }
        }

        let main = (window.window_handle(), cx.entity().downgrade());
        let agent_panel = self.agent_panel.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1280.), px(820.)), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("dscode editor".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(13.))),
            }),
            ..Default::default()
        };
        let (handle, editor) = gpui_kit::open_window(options, cx, move |window, cx| {
            crate::style::apply(window, cx);
            cx.new(|cx| Workspace::new_editor(root, main, agent_panel, window, cx))
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
        let command = |label: &str, icon: IconName, action: Box<dyn Action>| search_palette::Item {
            icon,
            label: label.into(),
            detail: None,
            badge: Badge::Keys(Kbd::binding_for_action(action.as_ref(), Some("Workspace"), window)),
            target: Target::Action(action),
            updated_at: 0,
        };
        let commands = vec![
            command("New Thread", IconName::SquarePen, Box::new(NewThread)),
            command("Open Folder…", IconName::FolderOpen, Box::new(OpenFolder)),
            command("Open Editor Window", IconName::Code, Box::new(OpenEditor)),
            command("Toggle Sidebar", IconName::PanelLeft, Box::new(ToggleSidebar)),
            command("Toggle Terminal", IconName::SquareTerminal, Box::new(ToggleTerminal)),
            command("Toggle Git Changes", IconName::GitCompare, Box::new(ToggleGitPanel)),
            command("Open a Surface…", IconName::PanelRight, Box::new(OpenSurface)),
            command("Save", IconName::Save, Box::new(Save)),
            command("Close Tab", IconName::X, Box::new(CloseTab)),
            command("Refresh", IconName::RefreshCw, Box::new(RefreshProject)),
            command("Settings…", IconName::Settings, Box::new(OpenSettings)),
        ];
        let panel = self.agent_panel.read(cx);
        let others = panel
            .threads()
            .iter()
            .map(|t| search_palette::thread_item(t.id, &t.title, t.project.as_ref(), t.updated_at))
            .chain(panel.recent_projects().iter().map(search_palette::project_item))
            .collect();

        let palette = cx.new(|cx| SearchPalette::new(commands, others, window, cx));
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
                .child(content.clone())
        });
        // The dialog takes focus as it opens; hand it to the search box after.
        window.defer(cx, move |window, cx| palette.update(cx, |palette, cx| palette.focus(window, cx)));
    }

    fn on_palette_event(
        &mut self,
        _: &Entity<SearchPalette>,
        event: &SearchPaletteEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SearchPaletteEvent::Pick(target) = event;
        // Close first: a command may open a dialog of its own.
        window.close_dialog(cx);
        self._palette = None;
        match target.clone() {
            Target::Action(action) => {
                self.focus_handle.focus(window, cx);
                window.dispatch_action(action, cx);
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
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
    }

    /// The main window's way into its editor window.
    fn render_editor_button(&self, id: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        chrome_button(id)
            .child(Icon::new(IconName::SquareCode).size(ICON_LG))
            .tooltip("Open Editor Window (⌘E)")
            .on_click(cx.listener(|this, _, window, cx| this.open_editor(&OpenEditor, window, cx)))
    }

    fn render_mode_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let workspace = cx.entity().downgrade();
        Button::new("workspace-mode")
            .ghost()
            .small()
            .h(layout::SIDEBAR_NAV_ROW)
            .px(layout::SPACE_2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_2)
                    .text_size(TEXT)
                    .font_weight(FontWeight::MEDIUM)
                    .child(if mode == Mode::Agent { "dscode" } else { "Editor" })
                    .child(Icon::new(IconName::ChevronDown).size(ICON_SM).text_color(cx.theme().muted_foreground)),
            )
            .dropdown_menu(move |menu, _, _| {
                let agent = workspace.clone();
                let editor = workspace.clone();
                let entry = |title: &'static str, description: &'static str| {
                    PopupMenuItem::element(move |_, cx| {
                        div()
                            .py(layout::SPACE_2)
                            .flex()
                            .flex_col()
                            .gap(layout::SPACE_1)
                            .child(div().text_size(crate::style::size::TEXT_LG).child(title))
                            .child(div().text_size(TEXT_SM).text_color(cx.theme().muted_foreground).child(description))
                    })
                };
                menu.min_w(px(236.)).check_side(Side::Right)
                    .item(entry("dscode", "Chat and work with agents")
                        .checked(mode == Mode::Agent)
                        .on_click(move |_, _, cx| {
                            if mode == Mode::Editor {
                                agent.update(cx, |this, cx| this.with_main_window(cx, |_, _, _| {})).ok();
                            }
                        }))
                    .item(entry("Editor", "Browse and edit project files")
                        .checked(mode == Mode::Editor)
                        .on_click(move |_, window, cx| {
                            if mode == Mode::Agent {
                                editor.update(cx, |this, cx| this.open_editor(&OpenEditor, window, cx)).ok();
                            }
                        }))
            })
    }

    fn render_sidebar_nav(&self, id: &'static str, label: &'static str, icon: IconName, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = id == "nav-editor" && self.mode == Mode::Editor;
        div()
            .id(id)
            .h(layout::SIDEBAR_NAV_ROW)
            .flex_none()
            .px(layout::SIDEBAR_ROW_INSET)
            .flex().items_center().gap(layout::SPACE_2)
            .rounded(layout::RADIUS)
            .cursor_pointer().text_size(TEXT)
            .text_color(theme.foreground.opacity(0.85))
            .when(selected, |this| this.bg(theme.list_active).text_color(theme.foreground))
            .hover(|this| this.bg(theme.list_hover).text_color(theme.foreground))
            .child(Icon::new(icon).size(ICON_LG).flex_none().text_color(theme.muted_foreground))
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| match id {
                "nav-chat" => this.new_thread(&NewThread, window, cx),
                "nav-editor" => {
                    if this.mode == Mode::Agent { this.open_editor(&OpenEditor, window, cx); }
                }
                _ => this.open_settings(&OpenSettings, window, cx),
            }))
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = match self.mode {
            Mode::Agent => self.agent_panel.update(cx, |panel, cx| panel.render_sidebar(cx)),
            Mode::Editor => self.file_tree.clone().into_any_element(),
        };
        let can_back = self.mode == Mode::Agent && self.agent_panel.read(cx).can_navigate(false);
        let can_forward = self.mode == Mode::Agent && self.agent_panel.read(cx).can_navigate(true);
        div()
            .size_full().min_w_0().flex().flex_col().bg(theme.sidebar)
            .child(
                div()
                    .h(layout::HEADER).flex_none().flex().items_center()
                    .pl(px(TRAFFIC_LIGHTS_WIDTH)).pr(layout::SIDEBAR_INSET)
                    .child(
                        chrome_button("hide-sidebar")
                            .child(crate::icons::sidebar().size(ICON_LG))
                            .text_color(theme.muted_foreground)
                            .tooltip("Hide Sidebar (⌘B)")
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx))),
                    )
                    .child(
                        chrome_button("sidebar-search")
                            .child(crate::icons::search().size(ICON_LG))
                            .text_color(theme.muted_foreground)
                            .tooltip("Search (⌘P)")
                            .on_click(cx.listener(|this, _, window, cx| this.search(&Search, window, cx))),
                    )
                    .child(Self::drag_region("sidebar-drag"))
                    .child(
                        chrome_button("sidebar-back").disabled(!can_back)
                            .child(Icon::new(IconName::ArrowLeft).size(ICON_LG))
                            .text_color(theme.muted_foreground)
                            .tooltip("Previous conversation")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.agent_panel.update(cx, |panel, cx| panel.navigate(false, window, cx));
                            })),
                    )
                    .child(
                        chrome_button("sidebar-forward").disabled(!can_forward)
                            .child(Icon::new(IconName::ArrowRight).size(ICON_LG))
                            .text_color(theme.muted_foreground)
                            .tooltip("Next conversation")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.agent_panel.update(cx, |panel, cx| panel.navigate(true, window, cx));
                            })),
                    ),
            )
            .child(
                div().flex_none().px(layout::SIDEBAR_INSET).pt(layout::SPACE_3)
                    .flex().flex_col()
                    .child(self.render_sidebar_nav("nav-chat", "New Chat", IconName::Send, cx))
                    .child(self.render_sidebar_nav("nav-editor", "Editor", IconName::SquareTerminal, cx))
                    .child(self.render_sidebar_nav("nav-customize", "Customize", IconName::Blocks, cx)),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(content))
            .when(self.mode == Mode::Agent && self.root.is_none(), |this| {
                this.child(
                    div().mx(layout::SIDEBAR_INSET).mb(layout::SPACE_2).p(layout::SPACE_3)
                        .rounded(layout::BUBBLE_RADIUS).border_1().border_color(theme.border)
                        .bg(theme.background.opacity(0.55)).flex().flex_col().gap(layout::SPACE_2)
                        .child(div().text_size(TEXT).child("Your workspace"))
                        .child(div().text_size(TEXT_SM).text_color(theme.muted_foreground).child("Open a repository to keep chats and files together."))
                        .child(Button::new("setup-open-folder").small().w_full().label("Open Folder")
                            .on_click(cx.listener(|this, _, window, cx| this.open_folder(&OpenFolder, window, cx)))),
                )
            })
            .child(
                div().h(px(56.)).flex_none().px(layout::SIDEBAR_INSET)
                    .flex().items_center().gap(layout::SPACE_2)
                    .child(
                        div().size(px(28.)).flex_none().rounded_full().bg(theme.foreground.opacity(0.07))
                            .flex().items_center().justify_center()
                            .child(Icon::new(IconName::SquareCode).size(ICON_LG).text_color(theme.muted_foreground)),
                    )
                    .child(self.render_mode_picker(cx))
                    .child(div().flex_1())
                    .child(
                        chrome_button("open-settings")
                            .child(Icon::new(IconName::Settings).size(ICON_LG))
                            .text_color(theme.muted_foreground).tooltip("Settings (⌘,)")
                            .on_click(cx.listener(|this, _, window, cx| this.open_settings(&OpenSettings, window, cx))),
                    ),
            )
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let project: SharedString = self
            .root
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "No project".into())
            .into();
        let current: Option<String> = match self.mode {
            Mode::Agent => Some(
                self.agent_panel
                    .read(cx)
                    .active_title()
                    .unwrap_or_else(|| "New thread".into()),
            ),
            Mode::Editor => self
                .active_tab
                .and_then(|ix| self.tabs.get(ix))
                .map(|tab| self.relative_path(&tab.path)),
        };

        div()
            .h(layout::HEADER)
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .pr_2()
            .border_b_1()
            .border_color(theme.border)
            .map(|this| {
                if self.show_sidebar {
                    this.pl_4()
                } else {
                    this.pl(px(TRAFFIC_LIGHTS_WIDTH)).child(
                        chrome_button("show-sidebar")
                            .child(crate::icons::sidebar().size(ICON_LG))
                            .tooltip("Show Sidebar (⌘B)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_sidebar(&ToggleSidebar, window, cx)
                            })),
                    )
                }
            })
            .child(
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
            )
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
            .child(Self::drag_region("header-drag"))
            .when(!self.show_sidebar && self.mode == Mode::Agent, |this| {
                this.child(self.render_editor_button("header-editor", cx))
            })
            .child({
                let visible = self.surfaces.read(cx).is_visible();
                chrome_button("toggle-surfaces")
                    .child(Icon::new(IconName::PanelRight).size(ICON_LG))
                    .selected(visible)
                    .tooltip("Surfaces (⌘T)")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.surfaces.update(cx, |surfaces, cx| {
                            // Nothing open yet: offer the picker instead of an empty panel.
                            if visible {
                                surfaces.set_visible(false, window, cx);
                            } else if !surfaces.is_visible() && surfaces.has_open() {
                                surfaces.set_visible(true, window, cx);
                            } else {
                                surfaces.show_picker(window, cx);
                            }
                        })
                    }))
            })
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        let content = match self.mode {
            Mode::Agent => self
                .agent_panel
                .update(cx, |panel, cx| panel.render_chat(cx)),
            Mode::Editor => self.render_editor_area(cx),
        };

        let center = div()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().min_w_0().child(content));

        let main = h_resizable("workspace-h")
            .child(
                resizable_panel()
                    .size(px(260.))
                    .size_range(px(220.)..px(600.))
                    .fixed()
                    .visible(self.show_sidebar)
                    .child(self.render_sidebar(cx)),
            )
            .child(resizable_panel().child(center))
            .child(
                resizable_panel()
                    .size(px(520.))
                    .size_range(px(280.)..px(1400.))
                    .fixed()
                    .visible(self.surfaces.read(cx).is_visible())
                    .child(self.surfaces.clone()),
            );

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
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::search))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(div().flex_1().min_h_0().child(main))
            .when(self.mode == Mode::Editor, |this| {
                this.child(self.render_status_bar(cx))
            })
    }
}
