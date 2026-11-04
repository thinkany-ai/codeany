//! The right-hand panel: every working tree change against `HEAD`.

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    diff::{Diff, DiffFile, DiffFileStatus, DiffMode, DiffState},
};
use gpui_kit::*;
use gpui_kit::assets::IconName;

use crate::git;
use crate::style::size::{TEXT, TEXT_SM};

pub struct GitPanel {
    root: Option<PathBuf>,
    is_repo: bool,
    patch: String,
    error: Option<SharedString>,
    diff: Entity<DiffState>,
    mode: DiffMode,
    _refresh_task: Option<Task<()>>,
}

impl GitPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            root: None,
            is_repo: false,
            patch: String::new(),
            error: None,
            diff: cx.new(|cx| DiffState::new(Vec::new(), cx)),
            mode: DiffMode::Unified,
            _refresh_task: None,
        }
    }

    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.root = root;
        self.patch.clear();
        self.diff
            .update(cx, |diff, cx| diff.set_files(Vec::new(), cx));
        self.refresh(cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.is_repo = false;
            cx.notify();
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { git::working_tree_diff(&root) });
        self._refresh_task = Some(cx.spawn(async move |this, cx| {
            let patch = task.await;
            this.update(cx, |this, cx| this.apply_patch(patch, cx)).ok();
        }));
    }

    fn apply_patch(&mut self, patch: Option<String>, cx: &mut Context<Self>) {
        self.is_repo = patch.is_some();
        let patch = patch.unwrap_or_default();
        if patch == self.patch {
            cx.notify();
            return;
        }
        match DiffFile::parse(&patch) {
            Ok(files) => {
                self.error = None;
                self.diff.update(cx, |diff, cx| diff.set_files(files, cx));
            }
            Err(err) => self.error = Some(format!("Failed to parse diff: {err}").into()),
        }
        self.patch = patch;
        cx.notify();
    }

    fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = match self.mode {
            DiffMode::Unified => DiffMode::Split,
            _ => DiffMode::Unified,
        };
        let mode = self.mode;
        self.diff.update(cx, |diff, cx| diff.set_mode(mode, cx));
        cx.notify();
    }

    fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let files = self.diff.read(cx).files().to_vec();
        div()
            .id("git-file-list")
            .flex_none()
            .max_h(px(180.))
            .overflow_y_scroll()
            .border_b_1()
            .border_color(theme.border)
            .py_1()
            .children(files.into_iter().enumerate().map(|(ix, file)| {
                let path = file.path().clone();
                let (letter, color) = match file.status() {
                    DiffFileStatus::Added => ("A", theme.success),
                    DiffFileStatus::Deleted => ("D", theme.danger),
                    DiffFileStatus::Renamed => ("R", theme.warning),
                    _ => ("M", theme.warning),
                };
                let name: SharedString = path
                    .rsplit('/')
                    .next()
                    .unwrap_or(path.as_ref())
                    .to_string()
                    .into();
                let diff = self.diff.clone();
                div()
                    .id(ix)
                    .h(px(22.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(TEXT)
                    .cursor_pointer()
                    .hover(|this| this.bg(theme.list_hover))
                    .child(div().w(px(10.)).text_size(TEXT_SM).text_color(color).child(letter))
                    .child(div().flex_none().child(name))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(TEXT_SM)
                            .text_color(theme.muted_foreground)
                            .child(path.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_SM)
                            .flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_color(theme.success)
                                    .child(format!("+{}", file.additions())),
                            )
                            .child(
                                div()
                                    .text_color(theme.danger)
                                    .child(format!("-{}", file.deletions())),
                            ),
                    )
                    .on_click(move |_, _, cx| {
                        diff.update(cx, |diff, cx| diff.scroll_to_file(&path, cx));
                    })
            }))
    }
}

impl Render for GitPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let file_count = self.diff.read(cx).files().len();

        let header = div()
            .h(px(32.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .pl_3()
            .pr_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(TEXT_SM)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(format!("CHANGES ({file_count})")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        Button::new("diff-mode")
                            .ghost()
                            .xsmall()
                            .icon(match self.mode {
                                DiffMode::Unified => IconName::Columns2,
                                _ => IconName::Rows2,
                            })
                            .tooltip(match self.mode {
                                DiffMode::Unified => "Side by side",
                                _ => "Unified",
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_mode(cx))),
                    )
                    .child(
                        Button::new("diff-refresh")
                            .ghost()
                            .xsmall()
                            .icon(IconName::RefreshCw)
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            );

        let body = if self.root.is_none() {
            placeholder("No folder opened", &theme).into_any_element()
        } else if !self.is_repo {
            placeholder("Not a git repository", &theme).into_any_element()
        } else if let Some(error) = self.error.clone() {
            placeholder(error, &theme).into_any_element()
        } else if file_count == 0 {
            placeholder("No changes", &theme).into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.render_file_list(cx))
                .child(
                    div().flex_1().min_h_0().child(
                        Diff::new(&self.diff)
                            .line_number(true)
                            .syntax_highlight(true)
                            .size_full(),
                    ),
                )
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .child(header)
            .child(body)
    }
}

fn placeholder(text: impl Into<SharedString>, theme: &component::Theme) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(TEXT)
        .text_color(theme.muted_foreground)
        .child(text.into())
}
