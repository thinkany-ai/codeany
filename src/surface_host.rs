//! The right-hand panel that hosts surface extensions, and the "Open a
//! surface" picker.

use std::{path::PathBuf, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{
    git,
    surfaces::{self, Extension, Host, Request, Surface, SurfaceContext},
};
use crate::style::{
    chrome_button,
    size::{ICON, ICON_LG, ICON_SM, TEXT, TEXT_LG, TEXT_SM, TEXT_XS},
};

pub enum SurfaceHostEvent {
    OpenFile(PathBuf),
}

struct OpenSurface {
    extension: Rc<dyn Extension>,
    surface: Box<dyn Surface>,
}

pub struct SurfaceHost {
    extensions: Vec<Rc<dyn Extension>>,
    open: Vec<OpenSurface>,
    active: Option<usize>,
    visible: bool,
    ctx: SurfaceContext,
}

impl EventEmitter<SurfaceHostEvent> for SurfaceHost {}

impl SurfaceHost {
    pub fn new() -> Self {
        Self {
            extensions: surfaces::builtin(),
            open: Vec::new(),
            active: None,
            visible: false,
            ctx: SurfaceContext::default(),
        }
    }

    pub fn has_open(&self) -> bool {
        !self.open.is_empty()
    }

    /// Whether the panel takes room in the window.
    pub fn is_visible(&self) -> bool {
        self.visible && !self.open.is_empty()
    }

    pub fn set_project(&mut self, project: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let git_root = project.as_deref().and_then(git::toplevel);
        self.ctx = SurfaceContext { project, git_root };
        let ctx = self.ctx.clone();
        for open in &self.open {
            open.surface.set_context(&ctx, window, cx);
        }
        cx.notify();
    }

    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for open in &self.open {
            open.surface.refresh(window, cx);
        }
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.open.iter().position(|o| o.extension.manifest().id == id)
    }

    /// Show a surface, opening it if needed. Returns whether it is shown.
    pub fn open(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(ix) = self.index_of(id) {
            self.visible = true;
            self.activate(ix, window, cx);
            return true;
        }
        let Some(extension) = self.extensions.iter().find(|e| e.manifest().id == id).cloned() else {
            return false;
        };
        if extension.unavailable(&self.ctx).is_some() {
            return false;
        }
        let weak = cx.entity().downgrade();
        let host = Host::new(move |request, window, cx| {
            weak.update(cx, |this, cx| this.handle(request, window, cx)).ok();
        });
        let surface = extension.build(&self.ctx, host, window, cx);
        self.open.push(OpenSurface { extension, surface });
        self.visible = true;
        self.activate(self.open.len() - 1, window, cx);
        true
    }

    /// For ⌘J and the like: show the surface, or hide the panel if it is
    /// already what the panel shows.
    pub fn toggle(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let showing = self.is_visible() && self.active.and_then(|ix| self.open.get(ix)).is_some_and(|o| o.extension.manifest().id == id);
        if showing {
            self.set_visible(false, window, cx);
        } else {
            self.open(id, window, cx);
        }
    }

    pub fn set_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = visible;
        if let Some(open) = self.active.and_then(|ix| self.open.get(ix)) {
            open.surface.set_active(visible, window, cx);
        }
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = self.active.filter(|&p| p != ix).and_then(|p| self.open.get(p)) {
            previous.surface.set_active(false, window, cx);
        }
        self.active = Some(ix);
        if let Some(open) = self.open.get(ix) {
            open.surface.set_active(true, window, cx);
        }
        cx.notify();
    }

    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.open.len() {
            return;
        }
        let closed = self.open.remove(ix);
        closed.surface.set_active(false, window, cx);
        self.active = match self.active {
            _ if self.open.is_empty() => None,
            Some(active) if active > ix => Some(active - 1),
            Some(active) if active == ix => Some(ix.min(self.open.len() - 1)),
            other => other,
        };
        if let Some(ix) = self.active {
            self.activate(ix, window, cx);
        }
        cx.notify();
    }

    fn handle(&mut self, request: Request, window: &mut Window, cx: &mut Context<Self>) {
        match request {
            Request::OpenFile(path) => cx.emit(SurfaceHostEvent::OpenFile(path)),
            Request::OpenUrl(url) => {
                if self.open("browser", window, cx) {
                    if let Some(ix) = self.index_of("browser") {
                        self.open[ix].surface.open_url(&url, window, cx);
                    }
                }
            }
        }
    }

    /// The "Open a surface" dialog.
    pub fn show_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
            return;
        }
        let items: Vec<PickerItem> = self
            .extensions
            .iter()
            .map(|e| {
                let m = e.manifest();
                PickerItem {
                    id: m.id,
                    name: m.name,
                    icon: m.icon.clone(),
                    key: m.key,
                    unavailable: e.unavailable(&self.ctx),
                    open: self.index_of(m.id).is_some(),
                }
            })
            .collect();
        // Hide a native browser view while the dialog is up.
        if let Some(open) = self.active.and_then(|ix| self.open.get(ix)) {
            open.surface.set_active(false, window, cx);
        }
        let weak = cx.entity().downgrade();
        let picker = cx.new(|cx| SurfacePicker {
            items,
            focus: cx.focus_handle(),
            on_pick: Rc::new(move |id, window, cx| {
                window.close_dialog(cx);
                weak.update(cx, |this, cx| {
                    this.open(id, window, cx);
                })
                .ok();
            }),
        });
        let content = picker.clone();
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            dialog
                .w(px(420.))
                .p_0()
                .close_button(false)
                .on_close(move |_, window, cx| {
                    // Bring back the surface the dialog covered.
                    weak.update(cx, |this, cx| {
                        let visible = this.visible;
                        if let Some(open) = this.active.and_then(|ix| this.open.get(ix)) {
                            open.surface.set_active(visible, window, cx);
                        }
                    })
                    .ok();
                })
                .child(content.clone())
        });
        window.defer(cx, move |window, cx| {
            let focus = picker.read(cx).focus.clone();
            focus.focus(window, cx);
        });
    }
}

impl Render for SurfaceHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tabs = self.open.iter().enumerate().map(|(ix, open)| {
            let m = open.extension.manifest();
            let active = self.active == Some(ix);
            div()
                .id(("surface-tab", ix))
                .group("surface-tab")
                .h(px(28.))
                .pl_2()
                .pr_1()
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded_md()
                .text_size(TEXT)
                .cursor_pointer()
                .when(active, |this| this.bg(theme.list_active).text_color(theme.foreground))
                .when(!active, |this| {
                    this.text_color(theme.muted_foreground)
                        .hover(|this| this.bg(theme.list_hover))
                })
                .child(Icon::new(m.icon.clone()).size_3p5())
                .child(m.name)
                .child(
                    div()
                        .id(("close-surface", ix))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .invisible()
                        .when(active, |this| this.visible())
                        .group_hover("surface-tab", |this| this.visible())
                        .hover(|this| this.bg(theme.list_hover))
                        .child(Icon::new(IconName::X).size(ICON_SM))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close(ix, window, cx);
                        })),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.activate(ix, window, cx)))
        });
        let content = self.active.and_then(|ix| self.open.get(ix)).map(|o| o.surface.view());
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .border_l_1()
            .border_color(theme.border)
            .child(
                div()
                    .h(px(40.))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .id("surface-tabs")
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .overflow_x_scroll()
                            .children(tabs),
                    )
                    .child(
                        chrome_button("add-surface")
                            .icon(Icon::new(IconName::Plus).size(ICON))
                            .tooltip("Open a surface (⌘T)")
                            .on_click(cx.listener(|this, _, window, cx| this.show_picker(window, cx))),
                    )
                    .child(
                        chrome_button("hide-surfaces")
                            .icon(Icon::new(IconName::PanelRightClose).size(ICON_LG))
                            .tooltip("Hide panel")
                            .on_click(cx.listener(|this, _, window, cx| this.set_visible(false, window, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().when_some(content, |this, view| this.child(view)))
    }
}

struct PickerItem {
    id: &'static str,
    name: &'static str,
    icon: IconName,
    key: char,
    unavailable: Option<&'static str>,
    open: bool,
}

/// "Open a surface": every extension with its key; unavailable ones dimmed.
struct SurfacePicker {
    items: Vec<PickerItem>,
    focus: FocusHandle,
    on_pick: Rc<dyn Fn(&'static str, &mut Window, &mut App)>,
}

impl Render for SurfacePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let keys = &event.keystroke;
                if keys.modifiers.platform || keys.modifiers.control || keys.modifiers.alt {
                    return;
                }
                let Some(ch) = keys.key.chars().next().filter(|_| keys.key.chars().count() == 1) else {
                    return;
                };
                let pick = this
                    .items
                    .iter()
                    .find(|item| item.key.eq_ignore_ascii_case(&ch) && item.unavailable.is_none())
                    .map(|item| item.id);
                if let Some(id) = pick {
                    cx.stop_propagation();
                    (this.on_pick)(id, window, cx);
                }
            }))
            .px_4()
            .py_5()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .mb_3()
                    .text_size(TEXT_LG)
                    .font_weight(FontWeight::MEDIUM)
                    .text_center()
                    .child("Open a surface"),
            )
            .children(self.items.iter().map(|item| {
                let enabled = item.unavailable.is_none();
                let (id, on_pick) = (item.id, self.on_pick.clone());
                div()
                    .id(item.id)
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded_lg()
                    .text_size(TEXT)
                    .when(!enabled, |this| this.opacity(0.45))
                    .when(enabled, |this| {
                        this.cursor_pointer()
                            .hover(|this| this.bg(theme.list_hover))
                            .on_click(move |_, window, cx| on_pick(id, window, cx))
                    })
                    .child(Icon::new(item.icon.clone()).size(ICON_LG))
                    .child(div().flex_1().child(item.name))
                    .when_some(item.unavailable, |this, reason| {
                        this.child(
                            div()
                                .text_size(TEXT_SM)
                                .text_color(theme.muted_foreground)
                                .child(reason),
                        )
                    })
                    .when(item.open, |this| {
                        this.child(div().size(px(6.)).rounded_full().bg(theme.primary))
                    })
                    .child(
                        div()
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(theme.secondary)
                            .text_size(TEXT_XS)
                            .text_color(theme.muted_foreground)
                            .child(item.key.to_string()),
                    )
            }))
    }
}
