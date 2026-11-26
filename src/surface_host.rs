//! The right-hand panel that hosts surface extensions. With nothing open it
//! shows a launcher; opened tabs can add surfaces from their dropdown.

use crate::style::AppButtonStyle as _;

use std::{path::PathBuf, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, menu::DropdownMenu as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{
    git,
    i18n::t,
    surfaces::{self, Extension, Host, Request, Surface, SurfaceContext},
};
use crate::style::{
    chrome_button, layout,
    size::{ICON, TAB_ACTION_ICON, TEXT},
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
    /// Listing the surfaces to open instead of showing the active one.
    launcher: bool,
    launcher_focus: FocusHandle,
    ctx: SurfaceContext,
}

impl EventEmitter<SurfaceHostEvent> for SurfaceHost {}

impl SurfaceHost {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            extensions: surfaces::builtin(),
            open: Vec::new(),
            active: None,
            visible: false,
            launcher: false,
            launcher_focus: cx.focus_handle(),
            ctx: SurfaceContext::default(),
        }
    }

    /// Whether the panel takes room in the window.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    fn showing_launcher(&self) -> bool {
        self.launcher || self.active.is_none()
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

    /// Show or hide the panel; shown with nothing open, it lists the surfaces.
    pub fn set_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = visible;
        let launcher = self.showing_launcher();
        if let Some(open) = self.active.and_then(|ix| self.open.get(ix)) {
            open.surface.set_active(visible && !launcher, window, cx);
        }
        if visible && launcher {
            self.launcher_focus.focus(window, cx);
        }
        cx.notify();
    }

    /// ⌥⌘T: list the surfaces in the panel; again hides the panel.
    pub fn show_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible && self.showing_launcher() {
            self.set_visible(false, window, cx);
            return;
        }
        self.launcher = true;
        self.set_visible(true, window, cx);
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.launcher = false;
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
        if let Some(ix) = self.active.filter(|_| !self.launcher) {
            self.activate(ix, window, cx);
        } else if self.active.is_none() {
            self.launcher_focus.focus(window, cx);
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

    fn add_surface_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let host = cx.entity().downgrade();
        chrome_button("add-surface")
            .child(Icon::new(IconName::Plus).size(TAB_ACTION_ICON))
            .accessibility_label(t("Open a surface"))
            .dropdown_menu({
                let host = host.clone();
                move |menu, _, cx| {
                    let Some(entity) = host.upgrade() else { return menu };
                    let this = entity.read(cx);
                    this.extensions.iter().fold(menu, |menu, extension| {
                        let manifest = extension.manifest();
                        let id = manifest.id;
                        let disabled = this.index_of(id).is_none() && extension.unavailable(&this.ctx).is_some();
                        let host = host.clone();
                        menu.item(
                            crate::style::menu_item_with_icon(t(manifest.name), manifest.icon.clone())
                                .disabled(disabled)
                                .on_click(move |_, window, cx| {
                                    let _ = host.update(cx, |this, cx| this.open(id, window, cx));
                                }),
                        )
                    })
                }
            })
            .on_open_change(move |open, window, cx| {
                let _ = host.update(cx, |this, cx| {
                    // Native browser content otherwise covers the dropdown.
                    if let Some(tab) = this.active.and_then(|ix| this.open.get(ix))
                        && tab.extension.manifest().id == "browser"
                    {
                        tab.surface.set_active(this.visible && !this.showing_launcher() && !*open, window, cx);
                    }
                });
            })
    }

    /// The surfaces to open, each with its key; unavailable ones dimmed.
    fn render_launcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let items: Vec<_> = self
            .extensions
            .iter()
            .map(|e| (e.manifest(), e.unavailable(&self.ctx)))
            .collect();
        let keys: Vec<(char, &'static str)> = items
            .iter()
            .filter(|(_, unavailable)| unavailable.is_none())
            .map(|(m, _)| (m.key, m.id))
            .collect();
        div()
            .id("surface-launcher")
            .size_full()
            .track_focus(&self.launcher_focus)
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let keys_down = &event.keystroke;
                if keys_down.modifiers.platform || keys_down.modifiers.control || keys_down.modifiers.alt {
                    return;
                }
                let mut chars = keys_down.key.chars();
                let (Some(ch), None) = (chars.next(), chars.next()) else { return };
                if let Some((_, id)) = keys.iter().find(|(key, _)| key.eq_ignore_ascii_case(&ch)) {
                    cx.stop_propagation();
                    this.open(id, window, cx);
                }
            }))
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .px_3()
                    .pb_2()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(t("Open a surface")),
            )
            .children(items.into_iter().map(|(m, unavailable)| {
                let enabled = unavailable.is_none();
                let open = self.index_of(m.id).is_some();
                let id = m.id;
                div()
                    .id(m.id)
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open(id, window, cx);
                            }))
                    })
                    .child(Icon::new(m.icon.clone()).size(ICON))
                    .child(div().flex_1().child(t(m.name)))
                    .when_some(unavailable, |this, reason| {
                        this.child(div().text_size(TEXT).text_color(theme.muted_foreground).child(t(reason)))
                    })
                    .when(open, |this| this.child(div().size(px(6.)).rounded_full().bg(theme.primary)))
                    .child(
                        div()
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(theme.secondary)
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground)
                            .child(m.key.to_string()),
                    )
            }))
            .into_any_element()
    }
}

impl Render for SurfaceHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let launcher = self.showing_launcher();
        let content = if launcher {
            Some(self.render_launcher(cx))
        } else {
            self.active.and_then(|ix| self.open.get(ix)).map(|o| o.surface.view().into_any_element())
        };
        let tabs = self.open.iter().enumerate().map(|(ix, open)| {
            let m = open.extension.manifest();
            let active = !launcher && self.active == Some(ix);
            div()
                .id(("surface-tab", ix))
                .group("surface-tab")
                .flex_none()
                .h(layout::CONTROL)
                .pl_2()
                .pr_1()
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded_md()
                .text_size(TEXT)
                .whitespace_nowrap()
                .cursor_pointer()
                .when(active, |this| this.bg(theme.list_active).text_color(theme.foreground))
                .when(!active, |this| {
                    this.text_color(theme.muted_foreground)
                        .hover(|this| this.bg(theme.list_hover))
                })
                .child(Icon::new(m.icon.clone()).size(ICON))
                .child(t(m.name))
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
                        .child(Icon::new(IconName::X).size(TAB_ACTION_ICON))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close(ix, window, cx);
                        })),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.activate(ix, window, cx)))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .border_l_1()
            .border_color(theme.border)
            .child(
                div()
                    .h(layout::HEADER)
                    .pt(layout::HEADER_TOP_INSET)
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("surface-tabs")
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .overflow_x_scroll()
                                    .children(tabs),
                            )
                            .when(!self.open.is_empty(), |this| this.child(self.add_surface_menu(cx))),
                    )
                    .child(
                        chrome_button("hide-surfaces")
                            .app_icon(IconName::PanelRight)
                            .on_click(cx.listener(|this, _, window, cx| this.set_visible(false, window, cx)))
                            .app_tooltip(t("Hide panel")),
                    ),
            )
            .child(div().flex_1().min_h_0().when_some(content, |this, view| this.child(view)))
    }
}
