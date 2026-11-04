//! A lazily loaded file tree: a directory is read only when it is expanded.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::assets::IconName;

use crate::git::{FileStatus, RepoSnapshot};
use crate::style::size::{TEXT, TEXT_SM};

const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 12.;
const HIDDEN_NAMES: &[&str] = &[".git", ".DS_Store"];

#[derive(Clone, Debug)]
struct DirEntry {
    path: PathBuf,
    name: SharedString,
    is_dir: bool,
}

#[derive(Clone)]
struct Row {
    entry: DirEntry,
    depth: usize,
}

pub enum FileTreeEvent {
    OpenFile(PathBuf),
}

pub struct FileTree {
    root: Option<PathBuf>,
    children: HashMap<PathBuf, Vec<DirEntry>>,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    selected: Option<PathBuf>,
    git: RepoSnapshot,
    scroll_handle: UniformListScrollHandle,
    focus_handle: FocusHandle,
}

impl EventEmitter<FileTreeEvent> for FileTree {}

impl Focusable for FileTree {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn read_dir(dir: &Path) -> Vec<DirEntry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<DirEntry> = read
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if HIDDEN_NAMES.contains(&name.as_str()) {
                return None;
            }
            let path = entry.path();
            // Follow symlinks so a linked directory can be expanded.
            let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
            Some(DirEntry {
                path,
                name: name.into(),
                is_dir,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

impl FileTree {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            root: None,
            children: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            git: RepoSnapshot::default(),
            scroll_handle: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.root = root;
        self.children.clear();
        self.expanded.clear();
        self.selected = None;
        if let Some(root) = self.root.clone() {
            self.children.insert(root.clone(), read_dir(&root));
            self.expanded.insert(root);
        }
        self.rebuild_rows();
        cx.notify();
    }

    pub fn set_git(&mut self, git: RepoSnapshot, cx: &mut Context<Self>) {
        if self.git != git {
            self.git = git;
            cx.notify();
        }
    }

    /// Re-read every loaded directory, picking up files created or removed
    /// outside the editor.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let dirs: Vec<PathBuf> = self.children.keys().cloned().collect();
        for dir in dirs {
            if !dir.is_dir() {
                self.children.remove(&dir);
                self.expanded.remove(&dir);
                changed = true;
                continue;
            }
            let fresh = read_dir(&dir);
            let old = &self.children[&dir];
            let same = old.len() == fresh.len()
                && old
                    .iter()
                    .zip(&fresh)
                    .all(|(a, b)| a.path == b.path && a.is_dir == b.is_dir);
            if !same {
                self.children.insert(dir, fresh);
                changed = true;
            }
        }
        if changed {
            self.rebuild_rows();
            cx.notify();
        }
    }

    pub fn select_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        // Expand every ancestor so the file is visible.
        let mut dir = path.parent();
        while let Some(d) = dir {
            if !d.starts_with(&root) {
                break;
            }
            if !self.children.contains_key(d) {
                self.children.insert(d.to_path_buf(), read_dir(d));
            }
            self.expanded.insert(d.to_path_buf());
            dir = d.parent();
        }
        self.selected = Some(path.to_path_buf());
        self.rebuild_rows();
        if let Some(ix) = self.rows.iter().position(|row| row.entry.path == path) {
            self.scroll_handle.scroll_to_item(ix, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        if let Some(root) = self.root.clone() {
            self.push_rows(&root, 0);
        }
    }

    fn push_rows(&mut self, dir: &Path, depth: usize) {
        let Some(entries) = self.children.get(dir).cloned() else {
            return;
        };
        for entry in entries {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            let path = entry.path.clone();
            self.rows.push(Row { entry, depth });
            if expanded {
                self.push_rows(&path, depth + 1);
            }
        }
    }

    fn toggle_dir(&mut self, path: &Path) {
        if self.expanded.remove(path) {
            return;
        }
        if !self.children.contains_key(path) {
            self.children.insert(path.to_path_buf(), read_dir(path));
        }
        self.expanded.insert(path.to_path_buf());
    }

    fn on_click_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        self.selected = Some(row.entry.path.clone());
        if row.entry.is_dir {
            self.toggle_dir(&row.entry.path);
            self.rebuild_rows();
        } else {
            cx.emit(FileTreeEvent::OpenFile(row.entry.path));
        }
        cx.notify();
    }

    fn status_color(status: FileStatus, cx: &App) -> Hsla {
        let theme = cx.theme();
        match status {
            FileStatus::Modified | FileStatus::Renamed => theme.warning,
            FileStatus::Added | FileStatus::Untracked => theme.success,
            FileStatus::Deleted | FileStatus::Conflicted => theme.danger,
        }
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        range
            .filter_map(|ix| self.rows.get(ix).cloned().map(|row| (ix, row)))
            .map(|(ix, row)| {
                let entry = &row.entry;
                let selected = self.selected.as_deref() == Some(entry.path.as_path());
                let status = if entry.is_dir {
                    self.git.dir_has_changes(&entry.path)
                } else {
                    self.git.statuses.get(&entry.path).copied()
                };
                let label_color = status
                    .map(|s| Self::status_color(s, cx))
                    .unwrap_or(theme.foreground);

                let icon = if entry.is_dir {
                    if self.expanded.contains(&entry.path) {
                        IconName::FolderOpen
                    } else {
                        IconName::Folder
                    }
                } else {
                    IconName::File
                };
                let chevron = entry.is_dir.then(|| {
                    if self.expanded.contains(&entry.path) {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    }
                });

                div()
                    .id(ix)
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl(px(8. + row.depth as f32 * INDENT))
                    .pr_2()
                    .text_size(TEXT)
                    .cursor_pointer()
                    .when(selected, |this| this.bg(theme.list_active))
                    .when(!selected, |this| this.hover(|this| this.bg(theme.list_hover)))
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .when_some(chevron, |this, chevron| {
                                this.child(
                                    Icon::new(chevron)
                                        .xsmall()
                                        .text_color(theme.muted_foreground),
                                )
                            }),
                    )
                    .child(
                        Icon::new(icon)
                            .small()
                            .flex_none()
                            .text_color(if entry.is_dir {
                                theme.muted_foreground
                            } else {
                                theme.muted_foreground.opacity(0.8)
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(label_color)
                            .child(entry.name.clone()),
                    )
                    .when_some(status.filter(|_| !entry.is_dir), |this, status| {
                        this.child(
                            div()
                                .flex_none()
                                .text_size(TEXT_SM)
                                .text_color(Self::status_color(status, cx))
                                .child(status.letter()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.on_click_row(ix, cx)))
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for FileTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let title: SharedString = self
            .root
            .as_ref()
            .and_then(|root| root.file_name())
            .map(|name| name.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "NO FOLDER".into())
            .into();

        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px_3()
                    .text_size(TEXT_SM)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(title),
            )
            .child(
                uniform_list(
                    "file-tree",
                    self.rows.len(),
                    cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll_handle)
                .flex_1()
                .pb_4(),
            )
    }
}
