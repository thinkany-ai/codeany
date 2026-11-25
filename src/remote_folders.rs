//! Pick a folder on another machine, for a thread whose agent runs there.
//!
//! The folders come from that machine's agl (`machine.info` for its home,
//! `exec ls` for a folder's subfolders), through the same nex stream the
//! conversation uses.

use crate::style::AppButtonStyle as _;

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::agl_bridge;
use crate::i18n::t;
use crate::style::layout;
use crate::style::size::{ICON, TEXT};

pub enum RemoteFoldersEvent {
    Pick(PathBuf),
}

pub struct RemoteFolders {
    machine: Option<String>,
    dir: Option<PathBuf>,
    /// The folders in `dir`, while loading `None`.
    entries: Option<Result<Vec<String>, String>>,
    _load: Option<Task<()>>,
}

impl EventEmitter<RemoteFoldersEvent> for RemoteFolders {}

impl RemoteFolders {
    pub fn new() -> Self {
        Self { machine: None, dir: None, entries: None, _load: None }
    }

    /// Show `dir` on `machine`, or its home folder.
    pub fn open(&mut self, machine: String, dir: Option<PathBuf>, cx: &mut Context<Self>) {
        self.machine = Some(machine.clone());
        match dir {
            Some(dir) => self.go(dir, cx),
            None => {
                self.dir = None;
                self.entries = None;
                let home = agl_bridge::backend(Some(&machine)).home();
                self._load = Some(cx.spawn(async move |this, cx| {
                    let home = home.await;
                    this.update(cx, |this, cx| match home {
                        Ok(home) => this.go(home, cx),
                        Err(err) => {
                            this.entries = Some(Err(format!("{err:#}")));
                            cx.notify();
                        }
                    })
                    .ok();
                }));
            }
        }
        cx.notify();
    }

    fn go(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let Some(machine) = self.machine.clone() else { return };
        self.dir = Some(dir.clone());
        self.entries = None;
        let list = agl_bridge::backend(Some(&machine)).list_dirs(dir.clone());
        self._load = Some(cx.spawn(async move |this, cx| {
            let entries = list.await.map_err(|err| format!("{err:#}"));
            this.update(cx, |this, cx| {
                // Moved on meanwhile.
                if this.dir.as_ref() == Some(&dir) {
                    this.entries = Some(entries);
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for RemoteFolders {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let dir = self.dir.clone();
        let parent = dir.as_ref().and_then(|d| d.parent()).map(PathBuf::from);
        let row = |id: (&'static str, usize)| {
            div()
                .id(id)
                .h(px(28.))
                .px(layout::SPACE_2)
                .flex()
                .items_center()
                .gap(layout::SPACE_2)
                .rounded(layout::RADIUS)
                .cursor_pointer()
                .text_size(TEXT)
                .hover(|this| this.bg(theme.list_hover))
        };
        let list: AnyElement = match &self.entries {
            None => div()
                .p(layout::SPACE_2)
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(t("Loading…"))
                .into_any_element(),
            Some(Err(err)) => div()
                .p(layout::SPACE_2)
                .text_size(TEXT)
                .text_color(theme.danger)
                .child(err.clone())
                .into_any_element(),
            Some(Ok(entries)) if entries.is_empty() => div()
                .p(layout::SPACE_2)
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(t("No folders here"))
                .into_any_element(),
            Some(Ok(entries)) => div()
                .flex()
                .flex_col()
                .children(entries.iter().enumerate().map(|(ix, name)| {
                    let target = dir.clone().unwrap_or_default().join(name);
                    row(("remote-folder", ix))
                        .child(Icon::new(IconName::Folder).size(ICON).text_color(theme.muted_foreground))
                        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(name.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| this.go(target.clone(), cx)))
                }))
                .into_any_element(),
        };
        div()
            .w(px(360.))
            .p(layout::SPACE_2)
            .flex()
            .flex_col()
            .gap(layout::SPACE_2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_1)
                    .child(
                        Button::new("remote-folder-up")
                            .ghost()
                            .small()
                            .app_icon(IconName::ArrowUp)
                            .disabled(parent.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(parent) = parent.clone() {
                                    this.go(parent, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground)
                            .child(dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default()),
                    ),
            )
            .child(
                div()
                    .id("remote-folder-list")
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                Button::new("remote-folder-use")
                    .primary()
                    .small()
                    .w_full()
                    .app_label(t("Use this folder"))
                    .disabled(dir.is_none())
                    .when_some(dir, |this, dir| {
                        this.on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(RemoteFoldersEvent::Pick(dir.clone()))
                        }))
                    }),
            )
    }
}
