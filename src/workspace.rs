//! The main window: sidebar, conversation or editor, and the surfaces panel.

use crate::style::AppButtonStyle as _;

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, kbd::Kbd, Selectable as _,
    button::{Button, ButtonVariants as _},
    input::{Editor, EditorState, InputEvent},
    notification::{Notification, NotificationType},
    resizable::{h_resizable, resizable_panel},
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
    search_palette::{self, Badge, SearchPalette, SearchPaletteEvent, Target},
    file_tree::{FileTree, FileTreeEvent},
    git::{self, RepoSnapshot},
    surface_host::{SurfaceHost, SurfaceHostEvent},
};
use crate::style::chrome_button;
use crate::style::layout;
use crate::style::size::{TEXT, ICON};
use crate::i18n::{t, tf};

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
    /// The active thread's worktree, shown in the file tree, surfaces and git
    /// status in place of `root`.
    work_dir: Option<PathBuf>,
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
        return Err(tf("File is too large to open ({size} MB)", &[("size", (meta.len() / 1024 / 1024).to_string())]).into());
    }
    let bytes = std::fs::read(path).map_err(|e| SharedString::from(e.to_string()))?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err(t("Binary file is not displayed").into());
    }
    String::from_utf8(bytes).map_err(|_| t("File is not valid UTF-8").into())
}

impl Workspace {
    /// The main window, showing the agent.
    pub fn new(root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let agent_panel = cx.new(|cx| AgentPanel::new(window, cx));
        // Launched without a folder (from Finder or the Dock): pick up where
        // the last run left off. The very first run opens nothing.
        let root = root.or_else(|| agent_panel.read(cx).last_project());
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
        let surfaces = cx.new(SurfaceHost::new);

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
            #[cfg(target_os = "macos")]
            subscriptions.push(
                cx.observe_global::<crate::updater::AvailableUpdate>(|_, cx| cx.notify()),
            );
            subscriptions.push(cx.subscribe_in(&agent_panel, window, Self::on_agent_event));
            subscriptions.push(cx.observe_in(&agent_panel, window, |this, panel, window, cx| {
                let work_dir = panel.read(cx).work_dir();
                if work_dir != this.work_dir {
                    this.set_work_dir(work_dir, window, cx);
                }
                cx.notify();
            }));
        }

        let mut this = Self {
            root: None,
            work_dir: None,
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
        self.file_tree.update(cx, |tree, cx| tree.set_root(dir.clone(), cx));
        self.surfaces.update(cx, |surfaces, cx| surfaces.set_project(dir, window, cx));
    }

    fn set_root(&mut self, root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.root = root.clone();
        self.work_dir = None;
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
            Mode::Agent => "CodeAny",
            Mode::Editor => t("CodeAny Editor"),
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
                    let Ok(root) = this.read_with(cx, |this, _| this.shown_dir()) else {
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
        if self.mode == Mode::Agent {
            let branch = snapshot.branch.clone();
            self.agent_panel.update(cx, |panel, cx| panel.set_branch(branch, cx));
        }
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
        match std::fs::write(&tab.path, text.as_bytes()) {
            Ok(()) => {
                tab.dirty = false;
                tab.modified_at = modified_at(&tab.path);
                self.surfaces.update(cx, |surfaces, cx| surfaces.refresh(window, cx));
            }
            Err(err) => {
                let message = tf("Failed to save {path}: {err}", &[("path", tab.path.display().to_string()), ("err", err.to_string())]);
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
                let path = match (&self.shown_dir(), path.is_relative()) {
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
                title: Some(t("CodeAny Editor").into()),
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
                // The settings view draws its own background, border and
                // corners; the dialog's would peek out around them.
                .bg(gpui_kit::transparent_black())
                .border_0()
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

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = match self.mode {
            Mode::Agent => self.agent_panel.update(cx, |panel, cx| panel.render_sidebar(cx)),
            Mode::Editor => self.file_tree.clone().into_any_element(),
        };
        let can_back = self.mode == Mode::Agent && self.agent_panel.read(cx).can_navigate(false);
        let can_forward = self.mode == Mode::Agent && self.agent_panel.read(cx).can_navigate(true);
        let sidebar = div()
            .size_full().min_w_0().flex().flex_col().bg(theme.sidebar)
            .child(
                div()
                    .h(layout::HEADER).flex_none().flex().items_center()
                    .pl(px(TRAFFIC_LIGHTS_WIDTH)).pr(layout::SIDEBAR_INSET)
                    .child(
                        chrome_button("hide-sidebar")
                            .child(crate::icons::sidebar().size(ICON))
                            .text_color(theme.muted_foreground)
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx)))
                            .app_tooltip(t("Hide Sidebar (⌘B)")),
                    )
                    .child(
                        chrome_button("sidebar-search")
                            .child(crate::icons::search().size(ICON))
                            .text_color(theme.muted_foreground)
                            .on_click(cx.listener(|this, _, window, cx| this.search(&Search, window, cx)))
                            .app_tooltip(t("Search (⌘P)")),
                    )
                    .child(Self::drag_region("sidebar-drag"))
                    .child(
                        chrome_button("sidebar-back").disabled(!can_back)
                            .child(Icon::new(IconName::ArrowLeft).size(ICON))
                            .text_color(theme.muted_foreground)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.agent_panel.update(cx, |panel, cx| panel.navigate(false, window, cx));
                            }))
                            .app_tooltip(t("Previous conversation")),
                    )
                    .child(
                        chrome_button("sidebar-forward").disabled(!can_forward)
                            .child(Icon::new(IconName::ArrowRight).size(ICON))
                            .text_color(theme.muted_foreground)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.agent_panel.update(cx, |panel, cx| panel.navigate(true, window, cx));
                            }))
                            .app_tooltip(t("Next conversation")),
                    ),
            )
            .child(
                div().flex_none().px(layout::SIDEBAR_INSET).pt(layout::SPACE_3)
                    .flex().flex_col()
                    .child(self.render_new_chat_button(cx)),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(content));

        sidebar.child(self.render_sidebar_footer(cx))
    }

    /// The sidebar's foot: settings, and, when one is ready, the update on
    /// the right.
    fn render_sidebar_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mode = self.mode;
        let icon_button = |id: &'static str, icon: IconName| {
            chrome_button(id)
                .child(Icon::new(icon).size(ICON))
                .text_color(theme.muted_foreground)
        };
        #[cfg(target_os = "macos")]
        let update = cx
            .try_global::<crate::updater::AvailableUpdate>()
            .and_then(|update| update.0.clone())
            .filter(|_| mode == Mode::Agent);
        #[cfg(not(target_os = "macos"))]
        let update: Option<String> = None;
        div()
            .h(px(52.))
            .flex_none()
            .px(layout::SIDEBAR_INSET)
            .flex()
            .items_center()
            .gap(layout::SPACE_1)
            .child(
                icon_button("open-settings", IconName::Settings)
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(&OpenSettings, window, cx)))
                    .app_tooltip(t("Settings (⌘,)")),
            )
            .child(div().flex_1())
            .when_some(update, |this, version| {
                this.child(
                    div()
                        .id("install-update")
                        .size(px(32.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .border_1()
                        .border_color(theme.foreground.opacity(0.7))
                        .text_color(theme.foreground)
                        .cursor_pointer()
                        .hover(|this| this.bg(theme.list_hover))
                        .child(Icon::new(IconName::ArrowUp).size(ICON))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tf("Update to {version}", &[("version", version.to_string())]))
                                .text_size(TEXT)
                                .build(window, cx)
                        })
                        .on_click(|_, _, _| {
                            #[cfg(target_os = "macos")]
                            crate::updater::check();
                        }),
                )
            })
    }

    /// The agent window's tabs, one per open thread, drawn like a browser's:
    /// the active tab takes the page's color and flows into it, with curved
    /// feet; the others sit on the darker strip, split by thin rules.
    fn render_chat_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        const RADIUS: Pixels = px(8.);
        // × and + share a size and colour.
        const TAB_ICON: Pixels = px(14.);
        let theme = cx.theme().clone();
        let (tabs, active) = self.agent_panel.read(cx).tab_items();
        let count = tabs.len();
        let closable = count > 1 || self.agent_panel.read(cx).has_open_thread();
        let icon_size = crate::style::size::icon_pixels(cx);
        // A concave corner joining the active tab to the page.
        let foot = |left: bool| {
            div()
                .absolute()
                // Down to the strip's bottom row: the curve ends the rule,
                // instead of the rule showing under the curve.
                .bottom_0()
                .when(left, |this| this.left(-RADIUS))
                .when(!left, |this| this.right(-RADIUS))
                .size(RADIUS)
                .bg(theme.background)
                .child(div().size_full().bg(theme.tab_bar).map(|this| {
                    if left { this.rounded_br(RADIUS) } else { this.rounded_bl(RADIUS) }
                }))
        };
        div()
            .id("chat-tabs")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_end()
            .pl(RADIUS)
            .children(tabs.into_iter().enumerate().map(|(ix, tab)| {
                let selected = ix == active;
                // Rules split two inactive tabs; none touches the active one.
                let rule = ix + 1 < count && !selected && ix + 1 != active;
                let shortcut = match ix {
                    0..=7 => format!(" (⌘{})", ix + 1),
                    _ => String::new(),
                };
                let tip = format!("{}{shortcut}", tab.title);
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
                        None => Icon::new(IconName::MessageCirclePlus).size(ICON).into_any_element(),
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
                    .h(px(32.))
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
                            // Reaches the strip's bottom row, over its rule.
                            this.bg(theme.background)
                                .text_color(theme.foreground)
                                .child(foot(true))
                                .child(foot(false))
                        } else {
                            this.text_color(theme.muted_foreground)
                                .hover(|this| this.bg(theme.list_hover).text_color(theme.foreground))
                        }
                    })
                    .child(div().flex_none().flex().items_center().child(icon))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(tab.title),
                    )
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
                                .child(Icon::new(IconName::X).size(TAB_ICON))
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
                                .top(px(9.))
                                .h(px(14.))
                                .w(px(1.))
                                .bg(theme.border)
                                .group_hover("chat-tab", |this| this.invisible()),
                        )
                    })
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| {
                        this.agent_panel.update(cx, |panel, cx| panel.close_tab(Some(ix), window, cx));
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.agent_panel.update(cx, |panel, cx| panel.select_tab(ix, window, cx));
                    }))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).text_size(TEXT).build(window, cx)
                    })
            }))
            .child(
                div().h_full().flex_none().flex().items_center().pl_1().child(
                    chrome_button("new-chat-tab")
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(IconName::Plus).size(TAB_ICON))
                        .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)))
                        .app_tooltip(t("New Tab (⌘T)")),
                ),
            )
            // The strip past the tabs drags the window.
            .child(Self::drag_region("chat-tabs-drag"))
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let project: SharedString = self
            .root
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
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
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .pr_2()
            // Tabs sit on a darker strip, as in a browser.
            // A rule under the strip keeps scrolled content apart from it;
            // the active tab covers it, joining the page.
            .when(self.mode == Mode::Agent, |this| {
                // Room above the tabs, so buttons centre on the tab row, not the strip.
                this.relative().pt(px(8.)).bg(theme.tab_bar).child(
                    // Drawn before the tabs, so the active one paints over it.
                    div().absolute().left_0().right_0().bottom_0().h(px(1.)).bg(theme.border),
                )
            })
            .map(|this| {
                if self.show_sidebar {
                    this.when(self.mode == Mode::Editor, |this| this.pl_4())
                } else {
                    this.pl(px(TRAFFIC_LIGHTS_WIDTH)).child(
                        chrome_button("show-sidebar")
                            .child(crate::icons::sidebar().size(ICON))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_sidebar(&ToggleSidebar, window, cx)
                            }))
                            .app_tooltip(t("Show Sidebar (⌘B)")),
                    )
                }
            })
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
            .when(self.mode != Mode::Agent, |this| this.child(Self::drag_region("header-drag")))
            .when(!self.show_sidebar && self.mode == Mode::Agent, |this| {
                this.child(self.render_editor_button("header-editor", cx))
            })
            .child({
                let visible = self.surfaces.read(cx).is_visible();
                chrome_button("toggle-surfaces")
                    .child(Icon::new(IconName::PanelRight).size(ICON))
                    .selected(visible)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        // Shown with nothing open, the panel lists the surfaces.
                        this.surfaces.update(cx, |surfaces, cx| surfaces.set_visible(!visible, window, cx))
                    }))
                    .app_tooltip(t("Surfaces (⌥⌘T)"))
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
