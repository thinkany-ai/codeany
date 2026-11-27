//! The workspace menu at the top of the sidebar, as Termany has it: the
//! workspaces (one on another device carries its name), then New workspace,
//! Import from another device, and Settings. Creating, editing, importing
//! and deleting happen inside the menu.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::i18n::{t, tf};
use crate::style::AppButtonStyle as _;
use crate::style::layout;
use crate::style::size::{ICON, TEXT};
use crate::threads::Workspace;

/// Icons to pick from, like Termany's picker (a short list).
const EMOJIS: &[&str] = &[
    "⭐", "🔥", "🚀", "💻", "🖥️", "🏠", "🏢", "🧪", "🛠️", "📦", "📚", "🎨", "🌱", "🌊", "⚡", "🍀", "🐱", "🐦", "🦊",
    "🐼", "🍎", "☕", "🎯", "🧠",
];

/// Id prefix of the one workspace an older CodeAny (without workspaces)
/// offers: all of that device's conversations.
const LEGACY: &str = "legacy:";

/// Why a device's workspaces could not be listed, in plain words; the
/// details follow in brackets.
fn friendly_error(device: &str, raw: &str) -> String {
    let lower = raw.to_lowercase();
    let reason = if lower.contains("not sharing") || lower.contains("502") {
        tf("{name} is not sharing: turn on Share this machine in Settings → Devices there.", &[("name", device.to_string())])
    } else if lower.contains("offline") || lower.contains("cannot reach") || lower.contains("timed out") || lower.contains("did not answer") {
        tf("Cannot reach {name}: check that CodeAny is open there and the two devices are connected.", &[("name", device.to_string())])
    } else if lower.contains("not connected") || lower.contains("not paired") {
        tf("{name} no longer accepts this device: connect them again in Settings → Devices.", &[("name", device.to_string())])
    } else {
        return tf("Cannot list the workspaces on {name}: {err}", &[("name", device.to_string()), ("err", raw.to_string())]);
    };
    format!("{reason} ({raw})")
}

pub enum WorkspaceMenuEvent {
    Activate(String),
    Create { title: String, icon: Option<String>, device: Option<(String, String)> },
    Edit { id: String, title: String, icon: Option<String> },
    Delete(String),
    /// Mount a workspace that lives on another device (its device set).
    Mount(Workspace),
    OpenSettings,
}

/// Another device that can hold or share workspaces.
#[derive(Clone)]
pub struct MenuDevice {
    pub id: String,
    pub name: String,
    pub online: bool,
}

enum Mode {
    List,
    /// New (`id` None) or edit a workspace.
    Form { id: Option<String>, icon: Option<String>, device: Option<String> },
    Import { device: Option<String>, list: Option<Result<Vec<Workspace>, String>> },
    ConfirmDelete(String),
}

pub struct WorkspaceMenu {
    workspaces: Vec<Workspace>,
    active: Option<String>,
    devices: Vec<MenuDevice>,
    mode: Mode,
    name: Entity<InputState>,
    _subscription: Subscription,
    _load: Option<Task<()>>,
}

impl EventEmitter<WorkspaceMenuEvent> for WorkspaceMenu {}

/// A workspace's icon: its emoji, else the title's initial on a tile.
pub fn avatar(icon: Option<&str>, title: &str, size: Pixels, cx: &App) -> AnyElement {
    let theme = cx.theme();
    match icon.filter(|i| !i.is_empty()) {
        Some(emoji) => div()
            .size(size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .text_size(size * 0.78)
            .child(emoji.to_string())
            .into_any_element(),
        None => div()
            .size(size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(size * 0.25)
            .bg(theme.secondary)
            .text_size(size * 0.55)
            .font_weight(FontWeight::SEMIBOLD)
            .child(title.trim().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into()))
            .into_any_element(),
    }
}

impl WorkspaceMenu {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t("Workspace name")));
        let _subscription = cx.subscribe(&name, |this: &mut Self, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(cx);
            }
        });
        Self {
            workspaces: Vec::new(),
            active: None,
            devices: Vec::new(),
            mode: Mode::List,
            name,
            _subscription,
            _load: None,
        }
    }

    /// What to show as the menu opens.
    pub fn open(&mut self, workspaces: Vec<Workspace>, active: Option<String>, devices: Vec<MenuDevice>, cx: &mut Context<Self>) {
        self.workspaces = workspaces;
        self.active = active;
        self.devices = devices;
        self.mode = Mode::List;
        cx.notify();
    }

    fn start_form(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let existing = id.as_ref().and_then(|id| self.workspaces.iter().find(|w| &w.id == id)).cloned();
        let (title, icon) = match &existing {
            Some(w) => (w.title.clone(), w.icon.clone()),
            None => (String::new(), Some(EMOJIS[self.workspaces.len() % EMOJIS.len()].to_string())),
        };
        self.mode = Mode::Form { id, icon, device: None };
        self.name.update(cx, |input, cx| {
            input.set_value(title, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Mode::Form { id, icon, device } = &self.mode else { return };
        let title = self.name.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        let event = match id {
            Some(id) => WorkspaceMenuEvent::Edit { id: id.clone(), title, icon: icon.clone() },
            None => WorkspaceMenuEvent::Create {
                title,
                icon: icon.clone(),
                device: device
                    .as_ref()
                    .and_then(|d| self.devices.iter().find(|m| &m.id == d))
                    .map(|d| (d.id.clone(), d.name.clone())),
            },
        };
        self.mode = Mode::List;
        cx.emit(event);
    }

    fn start_import(&mut self, cx: &mut Context<Self>) {
        let first = self.devices.iter().find(|d| d.online).map(|d| d.id.clone());
        self.mode = Mode::Import { device: None, list: None };
        if let Some(device) = first {
            self.load_device(device, cx);
        }
        cx.notify();
    }

    fn load_device(&mut self, device: String, cx: &mut Context<Self>) {
        self.mode = Mode::Import { device: Some(device.clone()), list: None };
        let load = crate::sync::device_workspaces(device.clone());
        let name = self.devices.iter().find(|d| d.id == device).map(|d| d.name.clone()).unwrap_or_else(|| device.clone());
        self._load = Some(cx.spawn(async move |this, cx| {
            let list = match load.await {
                Ok(list) => Ok(list),
                Err(err) => {
                    let text = format!("{err:#}");
                    if text.contains("unknown method") {
                        // A CodeAny from before workspaces: everything there
                        // is one workspace, named after the device.
                        let mut legacy = Workspace::new(name.clone(), Some("💻".into()), None, None);
                        legacy.id = format!("{LEGACY}{device}");
                        Ok(vec![legacy])
                    } else {
                        Err(friendly_error(&name, &text))
                    }
                }
            };
            this.update(cx, |this, cx| {
                if let Mode::Import { device: Some(shown), list: slot } = &mut this.mode {
                    if *shown == device {
                        *slot = Some(list);
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_list(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let mut list = div().flex().flex_col().gap(px(2.));
        for (ix, w) in self.workspaces.iter().enumerate() {
            let deletable = Workspace::deletable(&self.workspaces, &w.id);
            let active = self.active.as_ref() == Some(&w.id);
            let device = w.device.as_ref().map(|id| {
                let found = self.devices.iter().find(|d| &d.id == id);
                let name = found.map(|d| d.name.clone()).or(w.device_name.clone()).unwrap_or_else(|| id.clone());
                (name, found.is_some_and(|d| d.online))
            });
            let (pick, edit, delete) = (w.id.clone(), w.id.clone(), w.id.clone());
            list = list.child(
                div()
                    .id(("workspace", ix))
                    .group("workspace-row")
                    .h(px(36.))
                    .px(layout::SPACE_2)
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_2)
                    .rounded(layout::RADIUS)
                    .cursor_pointer()
                    .text_size(TEXT)
                    .hover(|this| this.bg(theme.list_hover))
                    .child(avatar(w.icon.as_deref(), &w.title, px(18.), cx))
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(w.title.clone()))
                    .when_some(device, |this, (name, online)| {
                        this.child(
                            div()
                                .flex_none()
                                .px(px(6.))
                                .rounded(px(4.))
                                .bg(theme.secondary)
                                .text_size(TEXT * 0.85)
                                .text_color(theme.muted_foreground)
                                .child(name),
                        )
                        .when(!online, |this| {
                            this.child(div().flex_none().text_size(TEXT * 0.85).text_color(theme.muted_foreground).child(t("offline")))
                        })
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .invisible()
                            .group_hover("workspace-row", |this| this.visible())
                            .child(
                                Button::new(("edit-workspace", ix))
                                    .ghost()
                                    .xsmall()
                                    .app_icon(IconName::Pencil)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.start_form(Some(edit.clone()), window, cx);
                                    })),
                            )
                            .when(deletable, |this| {
                                this.child(
                                    Button::new(("delete-workspace", ix))
                                        .ghost()
                                        .xsmall()
                                        .app_icon(IconName::Trash)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.mode = Mode::ConfirmDelete(delete.clone());
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .size(ICON)
                            .when(active, |this| this.child(Icon::new(IconName::Check).size(ICON).text_color(theme.primary))),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(WorkspaceMenuEvent::Activate(pick.clone())))),
            );
        }
        let action = |id: &'static str, icon: IconName, label: SharedString| {
            div()
                .id(id)
                .h(px(34.))
                .px(layout::SPACE_2)
                .flex()
                .items_center()
                .gap(layout::SPACE_2)
                .rounded(layout::RADIUS)
                .cursor_pointer()
                .text_size(TEXT)
                .hover(|this| this.bg(theme.list_hover))
                .child(div().size(px(20.)).flex().items_center().justify_center().child(Icon::new(icon).size(ICON).text_color(theme.muted_foreground)))
                .child(label)
        };
        list.child(div().my_1().h(px(1.)).bg(theme.border))
            .child(
                action("new-workspace", IconName::Plus, t("New workspace").into())
                    .on_click(cx.listener(|this, _, window, cx| this.start_form(None, window, cx))),
            )
            .when(!self.devices.is_empty(), |this| {
                this.child(
                    action("import-workspace", IconName::Plus, t("Import from another device").into())
                        .on_click(cx.listener(|this, _, _, cx| this.start_import(cx))),
                )
            })
            .child(
                action("workspace-settings", IconName::Settings, t("Settings").into())
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(WorkspaceMenuEvent::OpenSettings))),
            )
    }

    fn render_form(&self, id: Option<&String>, icon: Option<&String>, device: Option<&String>, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let title = self.name.read(cx).value().to_string();
        let creating = id.is_none();
        let mut grid = div().flex().flex_wrap().gap(px(2.));
        for (ix, emoji) in EMOJIS.iter().enumerate() {
            let selected = icon.map(String::as_str) == Some(*emoji);
            grid = grid.child(
                div()
                    .id(("emoji", ix))
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(layout::RADIUS)
                    .cursor_pointer()
                    .text_size(px(18.))
                    .when(selected, |this| this.bg(theme.list_active))
                    .hover(|this| this.bg(theme.list_hover))
                    .child(emoji.to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Mode::Form { icon, .. } = &mut this.mode {
                            *icon = Some(emoji.to_string());
                        }
                        cx.notify();
                    })),
            );
        }
        let mut form = div()
            .flex()
            .flex_col()
            .gap(layout::SPACE_2)
            .child(
                div()
                    .text_size(TEXT)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if creating { t("New workspace") } else { t("Edit workspace") }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_2)
                    .child(avatar(icon.map(String::as_str), &title, px(28.), cx))
                    .child(div().flex_1().child(Input::new(&self.name).small().text_size(TEXT))),
            )
            .child(grid);
        // Where a new one lives: this device, or another that is online.
        if creating && !self.devices.is_empty() {
            let row = |id: ElementId, label: String, selected: bool, enabled: bool| {
                div()
                    .id(id)
                    .h(px(30.))
                    .px(layout::SPACE_2)
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_2)
                    .rounded(layout::RADIUS)
                    .text_size(TEXT)
                    .when(enabled, |this| this.cursor_pointer().hover(|this| this.bg(theme.list_hover)))
                    .when(!enabled, |this| this.text_color(theme.muted_foreground))
                    .child(div().flex_1().child(label))
                    .when(selected, |this| this.child(Icon::new(IconName::Check).size(ICON).text_color(theme.primary)))
            };
            form = form
                .child(div().text_size(TEXT).text_color(theme.muted_foreground).child(t("Device")))
                .child(
                    row("device-self".into(), tf("This machine ({name})", &[("name", crate::remote::this_name())]), device.is_none(), true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Mode::Form { device, .. } = &mut this.mode {
                                *device = None;
                            }
                            cx.notify();
                        })),
                );
            for (ix, d) in self.devices.iter().enumerate() {
                let label = if d.online { d.name.clone() } else { format!("{} ({})", d.name, t("offline")) };
                let pick = d.id.clone();
                let online = d.online;
                form = form.child(
                    row(("device", ix).into(), label, device == Some(&d.id), online).on_click(cx.listener(move |this, _, _, cx| {
                        if !online {
                            return;
                        }
                        if let Mode::Form { device, .. } = &mut this.mode {
                            *device = Some(pick.clone());
                        }
                        cx.notify();
                    })),
                );
            }
        }
        form.child(
            div()
                .flex()
                .justify_end()
                .gap(layout::SPACE_2)
                .child(
                    Button::new("cancel-workspace-form")
                        .ghost()
                        .small()
                        .app_label(t("Cancel"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.mode = Mode::List;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("submit-workspace-form")
                        .primary()
                        .small()
                        .app_label(if creating { t("Create") } else { t("Save") })
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                ),
        )
    }

    fn render_import(&self, device: Option<&String>, list: Option<&Result<Vec<Workspace>, String>>, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().pb_1().text_size(TEXT).font_weight(FontWeight::SEMIBOLD).child(t("Import from another device")));
        let online: Vec<&MenuDevice> = self.devices.iter().filter(|d| d.online).collect();
        if online.is_empty() {
            body = body.child(
                div().text_size(TEXT).text_color(theme.muted_foreground).child(t("No other device is online and sharing.")),
            );
        }
        // The devices, as tabs of a row.
        if online.len() > 1 {
            let mut tabs = div().flex().flex_wrap().gap_1().pb_1();
            for (ix, d) in online.iter().enumerate() {
                let pick = d.id.clone();
                tabs = tabs.child(
                    Button::new(("import-device", ix))
                        .small()
                        .when(device == Some(&d.id), |b| b.primary())
                        .when(device != Some(&d.id), |b| b.ghost())
                        .app_label(d.name.clone())
                        .on_click(cx.listener(move |this, _, _, cx| this.load_device(pick.clone(), cx))),
                );
            }
            body = body.child(tabs);
        }
        let muted = |text: String| div().px(layout::SPACE_2).py_1().text_size(TEXT).text_color(theme.muted_foreground).child(text);
        match list {
            _ if device.is_none() => {}
            None => body = body.child(muted(t("Loading…").into())),
            Some(Err(err)) => body = body.child(div().px(layout::SPACE_2).text_size(TEXT).text_color(theme.danger).child(err.clone())),
            Some(Ok(list)) if list.is_empty() => body = body.child(muted(t("That device has no workspaces yet.").into())),
            Some(Ok(list)) => {
                let device_id = device.cloned().unwrap_or_default();
                let device_name = self.devices.iter().find(|d| d.id == device_id).map(|d| d.name.clone());
                for (ix, w) in list.iter().enumerate() {
                    let mounted = self.workspaces.iter().any(|m| {
                        m.id == w.id || (w.id.starts_with(LEGACY) && m.device.as_deref() == Some(device_id.as_str()))
                    });
                    let mut mount = w.clone();
                    mount.device = Some(device_id.clone());
                    mount.device_name = device_name.clone();
                    body = body.child(
                        div()
                            .id(("import-workspace-row", ix))
                            .h(px(34.))
                            .px(layout::SPACE_2)
                            .flex()
                            .items_center()
                            .gap(layout::SPACE_2)
                            .rounded(layout::RADIUS)
                            .text_size(TEXT)
                            .when(!mounted, |this| this.cursor_pointer().hover(|this| this.bg(theme.list_hover)))
                            .when(mounted, |this| this.text_color(theme.muted_foreground))
                            .child(avatar(w.icon.as_deref(), &w.title, px(18.), cx))
                            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(w.title.clone()))
                            .when(mounted, |this| this.child(div().text_size(TEXT * 0.85).child(t("Added"))))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !mounted {
                                    this.mode = Mode::List;
                                    cx.emit(WorkspaceMenuEvent::Mount(mount.clone()));
                                }
                            })),
                    );
                }
            }
        }
        body.child(
            div().pt_1().flex().justify_end().child(
                Button::new("close-import")
                    .ghost()
                    .small()
                    .app_label(t("Back"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.mode = Mode::List;
                        cx.notify();
                    })),
            ),
        )
    }

    fn render_confirm(&self, id: &str, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let Some(w) = self.workspaces.iter().find(|w| w.id == id) else { return div() };
        let question = match &w.device {
            Some(_) => tf(
                "Remove {name} from this window? It stays on {device}, with its conversations.",
                &[("name", w.title.clone()), ("device", w.device_name.clone().unwrap_or_default())],
            ),
            None => tf(
                "Delete {name}? Its projects and conversations move to the first workspace.",
                &[("name", w.title.clone())],
            ),
        };
        let delete = w.id.clone();
        div()
            .flex()
            .flex_col()
            .gap(layout::SPACE_3)
            .p_1()
            .child(div().text_size(TEXT).text_color(theme.foreground).child(question))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(layout::SPACE_2)
                    .child(
                        Button::new("cancel-delete-workspace")
                            .ghost()
                            .small()
                            .app_label(t("Cancel"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.mode = Mode::List;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("confirm-delete-workspace")
                            .danger()
                            .small()
                            .app_label(if w.device.is_some() { t("Remove") } else { t("Delete") })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.mode = Mode::List;
                                // The list shown loses it too, so the last one left stays.
                                this.workspaces.retain(|w| w.id != delete);
                                cx.emit(WorkspaceMenuEvent::Delete(delete.clone()));
                                cx.notify();
                            })),
                    ),
            )
    }
}

impl Render for WorkspaceMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.mode {
            Mode::List => self.render_list(cx),
            Mode::Form { id, icon, device } => {
                let (id, icon, device) = (id.clone(), icon.clone(), device.clone());
                self.render_form(id.as_ref(), icon.as_ref(), device.as_ref(), cx)
            }
            Mode::Import { device, list } => {
                let device = device.clone();
                let list = list.as_ref().map(|l| l.clone());
                self.render_import(device.as_ref(), list.as_ref(), cx)
            }
            Mode::ConfirmDelete(id) => {
                let id = id.clone();
                self.render_confirm(&id, cx)
            }
        };
        div().w(px(300.)).p(layout::SPACE_2).child(body)
    }
}
