//! The settings window: a modal with sections on the left, like Termany's.

use crate::style::AppButtonStyle as _;

use std::sync::{Arc, OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    Disableable as _,
    input::{Input, InputState},
    menu::DropdownMenu as _,
    switch::Switch,
};
use nex::proto::PairingCode;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{
    agent_icons::agent_icon,
    agent_panel::AgentPanel,
    paths, peer, remote, style,
    themes::{self, Palette, THEMES},
    threads::{ACCESS_LEVELS, access_name, agent_name},
};
use crate::style::size::{TEXT, ICON};
use crate::i18n::{t, tf};

/// The dialog's size; the content scrolls inside.
pub const WIDTH: f32 = 1040.;
pub const HEIGHT: f32 = 720.;
const FRAME_BORDER: Pixels = px(1.);
const NAV_WIDTH: f32 = 220.;
const CARD_WIDTH: f32 = 188.;
const CARD_BORDER: Pixels = px(2.);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Appearance,
    Agents,
    Devices,
    Shortcuts,
    About,
}

impl Section {
    const ALL: [Section; 6] = [
        Section::General,
        Section::Appearance,
        Section::Agents,
        Section::Devices,
        Section::Shortcuts,
        Section::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Agents => "Agents",
            Section::Devices => "Devices",
            Section::Shortcuts => "Shortcuts",
            Section::About => "About",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::General => IconName::Settings,
            Section::Appearance => IconName::Palette,
            Section::Agents => IconName::Bot,
            Section::Devices => IconName::Monitor,
            Section::Shortcuts => IconName::Keyboard,
            Section::About => IconName::Info,
        }
    }
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("⌘T / ⌘N", "New tab"),
    ("⌘W", "Close tab"),
    ("⌘⇧T", "Reopen closed tab"),
    ("⌃Tab / ⌘⇧]", "Next tab"),
    ("⌃⇧Tab / ⌘⇧[", "Previous tab"),
    ("⌘1 … ⌘8 / ⌘9", "Go to tab / last tab"),
    ("⌥⌘T", "Surfaces"),
    ("⌘E", "Switch Agent / Editor"),
    ("⌘O", "Open folder"),
    ("⌘,", "Settings"),
    ("⌘B", "Toggle sidebar"),
    ("⌘J / ⌃`", "Toggle terminal"),
    ("⌘⇧G", "Toggle git changes"),
    ("⌘S", "Save file"),
    ("Enter", "Send message"),
    ("⇧Enter", "New line in message"),
    ("⌘1 … ⌘9 (in the model menu)", "Pick a model"),
    ("⌘C / ⌘V / ⌘K", "Terminal copy / paste / clear"),
];

fn logo() -> Arc<Image> {
    static LOGO: OnceLock<Arc<Image>> = OnceLock::new();
    LOGO.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            include_bytes!("../assets/codeany.png").to_vec(),
        ))
    })
    .clone()
}

pub struct Settings {
    section: Section,
    agent_panel: WeakEntity<AgentPanel>,
    /// Every device, while Devices is shown.
    devices: Option<Result<Vec<remote::Device>, String>>,
    /// A sign-in, sign-out or other device action is under way.
    busy: bool,
    /// The last device action's failure.
    device_error: Option<String>,
    /// A code that adds a device to the account.
    pairing: Option<PairingCode>,
    code_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    /// The authorization code pasted to connect a device.
    auth_input: Option<Entity<InputState>>,
    /// The connection code typed to verify one.
    verify_input: Option<Entity<InputState>>,
    /// The device being renamed.
    renaming: Option<String>,
    /// An action waiting for a second click.
    confirm: Option<Confirm>,
    /// Connecting or verifying a device.
    link: Option<Link>,
    /// The authorization code was just copied.
    copied: bool,
    /// Measuring every way to every device.
    measuring: bool,
    _devices_poll: Option<Task<()>>,
    _link_poll: Option<Task<()>>,
}

#[derive(Clone, PartialEq, Eq)]
enum Confirm {
    SignOut,
    /// Unpair the device with this public key.
    Unpair(String),
    /// Remove a device from the account.
    Revoke(String),
}

enum Link {
    /// On the device that connects: paste the other's authorization code,
    /// then show the connection code until it is verified there.
    Connect {
        contacting: bool,
        offer: Option<peer::Offer>,
        connected: bool,
        error: Option<String>,
    },
    /// On the device being connected to: type the code the other shows.
    Verify { done: Option<String>, error: Option<String> },
}

impl Settings {
    pub fn new(agent_panel: &Entity<AgentPanel>) -> Self {
        Self {
            section: Section::Appearance,
            agent_panel: agent_panel.downgrade(),
            devices: None,
            busy: false,
            device_error: None,
            pairing: None,
            code_input: None,
            name_input: None,
            auth_input: None,
            verify_input: None,
            renaming: None,
            confirm: None,
            link: None,
            copied: false,
            measuring: false,
            _devices_poll: None,
            _link_poll: None,
        }
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let inner_radius = (theme.radius_lg - FRAME_BORDER).max(px(0.));
        div()
            .w(px(NAV_WIDTH))
            .flex_none()
            .h_full()
            .px_4()
            .py_6()
            .flex()
            .flex_col()
            .gap_1()
            // Overflow clips to a rectangle, so the sidebar paints its own
            // left corners inside the dialog border.
            .rounded_tl(inner_radius)
            .rounded_bl(inner_radius)
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_2()
                    .pb_3()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(t("Settings")),
            )
            .children(Section::ALL.into_iter().map(|section| {
                let selected = self.section == section;
                div()
                    .id(section.label())
                    .h(px(36.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .rounded_lg()
                    .text_size(TEXT)
                    .cursor_pointer()
                    .when(selected, |this| this.bg(theme.list_active))
                    .when(!selected, |this| this.hover(|this| this.bg(theme.list_hover)))
                    .child(
                        Icon::new(section.icon())
                            .size(ICON)
                            .text_color(if selected { theme.primary } else { theme.muted_foreground }),
                    )
                    .child(t(section.label()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.section = section;
                        if section == Section::Devices {
                            this.watch_devices(cx);
                        }
                        cx.notify();
                    }))
            }))
    }

    fn heading(title: &'static str, cx: &App) -> impl IntoElement {
        div()
            .mb_4()
            .text_size(TEXT)
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().muted_foreground)
            .child(t(title))
    }

    /// A labelled setting: name and hint on the left, the control on the right.
    fn row(label: &'static str, hint: Option<String>, control: AnyElement, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .py_3()
            .flex()
            .items_center()
            .gap_4()
            .border_b_1()
            .border_color(theme.border.opacity(0.6))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(TEXT).child(t(label)))
                    .when_some(hint, |this, hint| {
                        this.child(
                            div()
                                .text_size(TEXT)
                                .text_color(theme.muted_foreground)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(hint),
                        )
                    }),
            )
            .child(control)
    }

    fn render_general(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(panel) = self.agent_panel.upgrade() else {
            return div().into_any_element();
        };
        let (agents, current_agent, access) = {
            let panel = panel.read(cx);
            (
                panel.agents().map(|a| a.to_vec()).unwrap_or_default(),
                panel.default_agent().map(String::from),
                panel.default_access().to_string(),
            )
        };
        let weak = panel.downgrade();
        let agent_menu = Button::new("default-agent")
            .outline()
            .small()
            .app_label(current_agent.as_deref().map(agent_name).unwrap_or_else(|| t("None").into()))
            .dropdown_caret(true)
            .dropdown_menu({
                let weak = weak.clone();
                move |mut menu, _, _| {
                    for agent in agents.iter().filter(|a| a.available) {
                        let (weak, id) = (weak.clone(), agent.id.clone());
                        menu = menu.item(
                            crate::style::menu_item(agent.name.clone())
                                .checked(current_agent.as_deref() == Some(agent.id.as_str()))
                                .on_click(move |_, _, cx| {
                                    let id = id.clone();
                                    weak.update(cx, |panel, cx| panel.set_default_agent(id, cx)).ok();
                                }),
                        );
                    }
                    menu
                }
            });
        let access_menu = Button::new("default-access")
            .outline()
            .small()
            .app_label(access_name(&access))
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (id, name) in ACCESS_LEVELS {
                    let weak = weak.clone();
                    menu = menu.item(crate::style::menu_item(crate::i18n::t(name)).checked(access == id).on_click(
                        move |_, _, cx| {
                            weak.update(cx, |panel, cx| panel.set_default_access(id, cx)).ok();
                        },
                    ));
                }
                menu
            });
        let language = panel.read(cx).language_preference();
        let language_menu = Button::new("language")
            .outline()
            .small()
            .app_label(match language {
                Some(language) => language.label().to_string(),
                None => format!("{} ({})", crate::i18n::t("System"), crate::i18n::system_language().label()),
            })
            .dropdown_caret(true)
            .dropdown_menu({
                let weak = panel.downgrade();
                move |mut menu, _, _| {
                    let system = weak.clone();
                    menu = menu
                        .item(
                            crate::style::menu_item(format!(
                                "{} ({})",
                                crate::i18n::t("System"),
                                crate::i18n::system_language().label()
                            ))
                            .checked(language.is_none())
                            .on_click(move |_, _, cx| {
                                system.update(cx, |panel, cx| panel.set_language(None, cx)).ok();
                            }),
                        )
                        .separator();
                    for choice in crate::i18n::Language::ALL {
                        let weak = weak.clone();
                        menu = menu.item(
                            crate::style::menu_item(choice.label())
                                .checked(language == Some(choice))
                                .on_click(move |_, _, cx| {
                                    weak.update(cx, |panel, cx| panel.set_language(Some(choice), cx)).ok();
                                }),
                        );
                    }
                    menu
                }
            });
        let data_dir = paths::home();
        div()
            .child(Self::heading("General", cx))
            .child(Self::row(
                "Language",
                Some(t("Follows the system unless you pick one.").into()),
                language_menu.into_any_element(),
                cx,
            ))
            .child(Self::row(
                "Default agent",
                Some(t("New threads start with this agent; open threads keep theirs.").into()),
                agent_menu.into_any_element(),
                cx,
            ))
            .child(Self::row(
                "Default access",
                Some(t("What agents may do without asking.").into()),
                access_menu.into_any_element(),
                cx,
            ))
            .child(Self::row(
                "Data",
                Some(data_dir.display().to_string()),
                Button::new("reveal-data")
                    .outline()
                    .small()
                    .app_label(t("Show in Finder"))
                    .on_click(move |_, _, cx| cx.reveal_path(&paths::database()))
                    .into_any_element(),
                cx,
            ))
            .into_any_element()
    }

    /// A small picture of a theme: sidebar strip, accent dot and text lines.
    fn theme_preview(p: &Palette, radius: Pixels) -> impl IntoElement {
        let color = |text: &str| -> Hsla {
            let (r, g, b, a) = themes::parse_color(text).unwrap_or_default();
            Rgba { r, g, b, a }.into()
        };
        let line = |w: f32, c: Hsla| div().w(px(w)).h(px(5.)).rounded_full().bg(c);
        div()
            .size_full()
            .flex()
            .rounded(radius)
            .bg(color(p.bg))
            .child(
                div()
                    .w(relative(0.3))
                    .h_full()
                    .rounded_tl(radius)
                    .rounded_bl(radius)
                    .bg(color(p.sidebar.unwrap_or(p.bg2))),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .child(div().size(px(7.)).rounded_full().bg(color(p.accent)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(line(72., color(p.fg)))
                            .child(line(56., color(p.fg)))
                            .child(line(40., color(p.fg_dim))),
                    ),
            )
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let card_radius = theme.radius_lg;
        let preview_radius = (card_radius - CARD_BORDER).max(px(0.));
        let current = style::choice(cx);
        let weak = self.agent_panel.clone();
        let card = |id: &'static str, name: &'static str, preview: AnyElement, selected: bool| {
            let weak = weak.clone();
            div()
                .id(id)
                .w(px(CARD_WIDTH))
                .flex()
                .flex_col()
                .gap_2()
                .cursor_pointer()
                .child(
                    div()
                        .h(px(108.))
                        .rounded(card_radius)
                        .overflow_hidden()
                        .border(CARD_BORDER)
                        .border_color(if selected { theme.primary } else { theme.border })
                        .child(preview),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(TEXT)
                        .child(name)
                        .when(selected, |this| {
                            this.child(Icon::new(IconName::Check).size(ICON).text_color(theme.primary))
                        }),
                )
                .on_click(move |_, window, cx| {
                    let id = id.to_string();
                    weak.update(cx, |panel, cx| panel.set_theme(id, window, cx)).ok();
                })
        };

        // "System" shows both halves of CodeAny's own light and dark look.
        let system_preview = div()
            .size_full()
            .flex()
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .rounded_tl(preview_radius)
                    .rounded_bl(preview_radius)
                    .bg(rgb(0xf7f7f8)),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .rounded_tr(preview_radius)
                    .rounded_br(preview_radius)
                    .bg(rgb(0x18181b)),
            )
            .into_any_element();

        div()
            .child(self.render_typography(cx))
            .child(Self::heading("Theme", cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(20.))
                    .gap_y(px(22.))
                    .child(card("system", t("System"), system_preview, current == "system"))
                    .children(THEMES.iter().map(|p| {
                        card(p.id, p.name, Self::theme_preview(p, preview_radius).into_any_element(), current == p.id)
                    })),
            )
            .into_any_element()
    }

    fn render_typography(&self, cx: &App) -> impl IntoElement {
        let typography = style::typography(cx);
        let fonts_panel = self.agent_panel.clone();
        let sizes_panel = self.agent_panel.clone();
        let font_picker = Button::new("appearance-font")
            .small()
            .w(px(200.))
            .app_label(typography.family.as_deref().unwrap_or(t("System")))
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, cx| {
                let selected = style::typography(cx).family;
                let families = cx.text_system().all_font_names().into_iter()
                    .filter(|name| !name.is_empty() && !name.starts_with('.'))
                    .map(Some);
                menu = menu.min_w(px(220.)).max_h(px(360.)).scrollable(true);
                for family in std::iter::once(None).chain(families) {
                    let panel = fonts_panel.clone();
                    let label = family.as_deref().unwrap_or(t("System")).to_owned();
                    menu = menu.item(style::menu_item(label).checked(family == selected)
                        .on_click(move |_, _, cx| {
                            let mut typography = style::typography(cx);
                            typography.family = family.clone();
                            panel.update(cx, |panel, cx| panel.set_typography(typography, cx)).ok();
                        }));
                }
                menu
            });
        let size_picker = Button::new("appearance-text-size")
            .small()
            .w(px(96.))
            .app_label(format!("{} px", typography.size))
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, cx| {
                let selected = style::typography(cx).size;
                for size in style::Typography::SIZES {
                    let panel = sizes_panel.clone();
                    menu = menu.item(style::menu_item(format!("{size} px")).checked(size == selected)
                        .on_click(move |_, _, cx| {
                            let mut typography = style::typography(cx);
                            typography.size = size;
                            panel.update(cx, |panel, cx| panel.set_typography(typography, cx)).ok();
                        }));
                }
                menu
            });

        div()
            .mb_6()
            .child(Self::heading("Typography", cx))
            .child(Self::row("Font", None, font_picker.into_any_element(), cx))
            .child(Self::row("Text size", None, size_picker.into_any_element(), cx))
    }

    fn render_agents(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let agents = self
            .agent_panel
            .upgrade()
            .and_then(|panel| panel.read(cx).agents().map(|a| a.to_vec()));
        let mut list = div().flex().flex_col();
        match agents {
            None => {
                list = list.child(
                    div()
                        .text_size(TEXT)
                        .text_color(theme.muted_foreground)
                        .child(t("Detecting agents…")),
                )
            }
            Some(mut agents) => {
                agents.sort_by_key(|a| !a.available);
                for agent in agents {
                    let detail = match (&agent.version, &agent.path) {
                        _ if !agent.available => t("Not installed").to_string(),
                        (Some(version), Some(path)) => format!("{version} · {path}"),
                        (_, Some(path)) => path.clone(),
                        _ => String::new(),
                    };
                    list = list.child(
                        div()
                            .py_3()
                            .flex()
                            .items_center()
                            .gap_3()
                            .border_b_1()
                            .border_color(theme.border.opacity(0.6))
                            .when(!agent.available, |this| this.opacity(0.55))
                            .child(agent_icon(&agent.id, px(28.), cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(div().text_size(TEXT).child(agent.name.clone()))
                                    .child(
                                        div()
                                            .text_size(TEXT)
                                            .text_color(theme.muted_foreground)
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(detail),
                                    ),
                            )
                            .when(agent.available, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .children(agent.features.iter().take(4).map(|f| {
                                            div()
                                                .px(px(6.))
                                                .py(px(1.))
                                                .rounded_md()
                                                .bg(theme.secondary)
                                                .text_size(TEXT)
                                                .text_color(theme.muted_foreground)
                                                .child(f.clone())
                                        })),
                                )
                            }),
                    );
                }
            }
        }
        div()
            .child(Self::heading("Agents", cx))
            .child(list)
            .into_any_element()
    }

    fn render_shortcuts(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        div()
            .child(Self::heading("Shortcuts", cx))
            .children(SHORTCUTS.iter().map(|(keys, action)| {
                div()
                    .py(px(10.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border.opacity(0.6))
                    .child(div().flex_1().text_size(TEXT).child(t(*action)))
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(2.))
                            .rounded_md()
                            .bg(theme.secondary)
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground)
                            .child(t(*keys)),
                    )
            }))
            .into_any_element()
    }

    fn render_about(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        div()
            .pt_10()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(img(logo()).size(px(96.)))
            .child(div().mt_2().text_size(TEXT).font_weight(FontWeight::SEMIBOLD).child("CodeAny"))
            .child(
                div()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(tf("Version {version}", &[("version", env!("CARGO_PKG_VERSION").to_string())])),
            )
            .child(
                div()
                    .mt_4()
                    .max_w(px(420.))
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .text_center()
                    .child(t("A lightweight coding-agent app with an editor.")),
            )
            .child(
                div()
                    .mt_2()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(tf("Data: {path}", &[("path", paths::home().display().to_string())])),
            )
            .into_any_element()
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Shrink with small windows rather than running off the bottom.
        let max_height = window.viewport_size().height - px(48.);
        if self.code_input.is_none() {
            self.code_input = Some(cx.new(|cx| InputState::new(window, cx).placeholder("XXXX-XXXX")));
        }
        if self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| InputState::new(window, cx).placeholder(t("Device name"))));
        }
        if self.auth_input.is_none() {
            self.auth_input = Some(cx.new(|cx| InputState::new(window, cx).placeholder("codeany1.…")));
        }
        if self.verify_input.is_none() {
            self.verify_input = Some(cx.new(|cx| InputState::new(window, cx).placeholder("XXXX-XXXX")));
        }
        let content = match self.section {
            Section::General => self.render_general(cx),
            Section::Appearance => self.render_appearance(cx),
            Section::Agents => self.render_agents(cx),
            Section::Devices => self.render_devices(window, cx),
            Section::Shortcuts => self.render_shortcuts(cx),
            Section::About => self.render_about(cx),
        };
        div()
            .w_full()
            .min_w_0()
            .h(px(HEIGHT))
            .max_h(max_height)
            .flex()
            // The dialog around it is bare, so this is the only frame.
            .rounded(theme.radius_lg)
            .border(FRAME_BORDER)
            .border_color(theme.border)
            .overflow_hidden()
            .bg(theme.background)
            .child(self.render_nav(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        div()
                            .id("settings-content")
                            .size_full()
                            .overflow_y_scroll()
                            .px(px(28.))
                            .py_6()
                            .child(content),
                    )
                    .child(
                        div().absolute().top(px(14.)).right(px(14.)).child(
                            Button::new("close-settings")
                                .ghost()
                                .small()
                                .app_icon(IconName::X)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                    ),
            )
    }
}

// ---- Devices -------------------------------------------------------------
//
// Two parts, like Termany's: this device (name, account, authorization code,
// what it shares) and every other device once, however it is reached:
// through the account, paired with an authorization code, or both (then end
// to end encrypted).

impl Settings {
    /// Keep the device list and sign-in state fresh while Devices is shown.
    fn watch_devices(&mut self, cx: &mut Context<Self>) {
        self.load_devices(cx);
        self._devices_poll = Some(cx.spawn(async move |this, cx| {
            let mut tick = 0u32;
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                tick += 1;
                // Rendering Devices starts this again; stop a while after,
                // unless a browser sign-in is still under way.
                let pending = remote::login_status() == remote::Login::Pending;
                let Ok(showing) = this.update(cx, |this, cx| {
                    // A browser sign-in finishing, a request arriving.
                    cx.notify();
                    if tick % 5 == 0 {
                        this.load_devices(cx);
                    }
                    let showing = this.section == Section::Devices && (pending || this.link.is_some() || tick < 300);
                    if !showing {
                        this._devices_poll = None;
                    }
                    showing
                }) else {
                    break;
                };
                if !showing {
                    break;
                }
            }
        }));
    }

    fn load_devices(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let devices = remote::devices().await.map_err(|e| remote::describe_error(&e));
            this.update(cx, |this, cx| {
                this.devices = Some(devices);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Run a device action, then refresh what depends on it.
    fn device_action(
        &mut self,
        action: impl std::future::Future<Output = anyhow::Result<()>> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        self.device_error = None;
        self.confirm = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = action.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                if let Err(err) = result {
                    this.device_error = Some(remote::describe_error(&err));
                }
                this.after_account_change(cx);
            })
            .ok();
        })
        .detach();
    }

    fn after_account_change(&mut self, cx: &mut Context<Self>) {
        self.load_devices(cx);
        if let Some(panel) = self.agent_panel.upgrade() {
            panel.update(cx, |panel, cx| panel.refresh_hosts(cx));
        }
        cx.notify();
    }

    fn sign_in_browser(&mut self, cx: &mut Context<Self>) {
        self.device_error = None;
        cx.spawn(async move |this, cx| {
            let started = remote::start_login(true).await;
            this.update(cx, |this, cx| {
                match started {
                    Ok(url) => cx.open_url(&url),
                    Err(err) => this.device_error = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        // Notice when the browser comes back.
        let was_signed_in = remote::account().is_some();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(700)).await;
                let pending = remote::login_status() == remote::Login::Pending;
                let signed_in = remote::account().is_some() && !was_signed_in;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
                if signed_in {
                    this.update(cx, |this, cx| this.after_account_change(cx)).ok();
                    break;
                }
                if !pending {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn join_with_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.code_input.clone() else { return };
        let code = input.read(cx).value().trim().to_string();
        if code.is_empty() {
            self.device_error = Some(t("Enter the code shown on your other device.").into());
            cx.notify();
            return;
        }
        input.update(cx, |input, cx| input.set_value("", window, cx));
        let server = remote::termany_account()
            .map(|(server, _)| server)
            .unwrap_or_else(|| remote::DEFAULT_RELAY.to_string());
        self.device_action(remote::join(server, code, true), cx);
    }

    fn create_pairing_code(&mut self, cx: &mut Context<Self>) {
        self.device_error = None;
        cx.spawn(async move |this, cx| {
            let code = remote::pairing_code().await;
            this.update(cx, |this, cx| {
                match code {
                    Ok(code) => this.pairing = Some(code),
                    Err(err) => this.device_error = Some(remote::describe_error(&err)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn start_rename(&mut self, id: String, current: String, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = Some(id);
        if let Some(input) = self.name_input.clone() {
            input.update(cx, |input, cx| {
                input.set_value(current, window, cx);
                input.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn save_rename(&mut self, cx: &mut Context<Self>) {
        let (Some(input), Some(id)) = (self.name_input.clone(), self.renaming.take()) else { return };
        let name = input.read(cx).value().trim().to_string();
        self.device_action(remote::rename(id, name), cx);
    }

    fn refresh_now(&mut self, cx: &mut Context<Self>) {
        self.measuring = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = remote::remeasure().await;
            this.update(cx, |this, cx| {
                this.measuring = false;
                if let Err(err) = result {
                    this.device_error = Some(format!("{err:#}"));
                }
                this.load_devices(cx);
            })
            .ok();
        })
        .detach();
    }

    fn copy_invite(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(peer::invite()));
        self.copied = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // ---- Connecting devices

    fn open_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_link(cx);
        self.link = Some(Link::Connect { contacting: false, offer: None, connected: false, error: None });
        if let Some(input) = self.auth_input.clone() {
            input.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn open_verify(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_link(cx);
        self.link = Some(Link::Verify { done: None, error: None });
        if let Some(input) = self.verify_input.clone() {
            input.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.focus(window, cx);
            });
        }
        cx.notify();
    }

    /// Close the connect / verify panel; giving up before the other side
    /// verified stops trusting it.
    fn close_link(&mut self, cx: &mut Context<Self>) {
        if let Some(Link::Connect { offer: Some(offer), connected: false, .. }) = &self.link {
            peer::remove(&offer.public_key);
        }
        self.link = None;
        self._link_poll = None;
        self.load_devices(cx);
        cx.notify();
    }

    fn submit_connect(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.auth_input.clone() else { return };
        let code = input.read(cx).value().trim().to_string();
        if code.is_empty() {
            if let Some(Link::Connect { error, .. }) = &mut self.link {
                *error = Some(t("Paste the authorization code from the other device.").into());
            }
            cx.notify();
            return;
        }
        self.start_pairing(agl_pair(peer::pair_with_code(code)), cx);
    }

    /// Turn on end-to-end encryption with one of the account's devices.
    fn connect_device(&mut self, nex_id: String, cx: &mut Context<Self>) {
        self.close_link(cx);
        self.start_pairing(agl_pair(peer::pair_with_device(nex_id)), cx);
    }

    fn start_pairing(
        &mut self,
        pairing: impl std::future::Future<Output = anyhow::Result<peer::Offer>> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.link = Some(Link::Connect { contacting: true, offer: None, connected: false, error: None });
        cx.notify();
        self._link_poll = Some(cx.spawn(async move |this, cx| {
            let offer = pairing.await;
            let public_key = match offer {
                Ok(offer) => {
                    let key = offer.public_key.clone();
                    let alive = this.update(cx, |this, cx| {
                        this.link = Some(Link::Connect { contacting: false, offer: Some(offer), connected: false, error: None });
                        this.load_devices(cx);
                    });
                    if alive.is_err() {
                        return;
                    }
                    key
                }
                Err(err) => {
                    this.update(cx, |this, cx| {
                        this.link = Some(Link::Connect {
                            contacting: false,
                            offer: None,
                            connected: false,
                            error: Some(format!("{err:#}")),
                        });
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            // Wait for the other device to verify the code.
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                let key = public_key.clone();
                let done = crate::agl_bridge::spawn(async move { peer::check(&key).await }).await.unwrap_or(false);
                let Ok(still_open) = this.update(cx, |this, cx| {
                    let Some(Link::Connect { offer: Some(offer), connected, .. }) = &mut this.link else { return false };
                    if offer.public_key != public_key {
                        return false;
                    }
                    if done {
                        *connected = true;
                        this.after_account_change(cx);
                    }
                    !done
                }) else {
                    break;
                };
                if !still_open {
                    break;
                }
            }
        }));
    }

    fn submit_verify(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.verify_input.clone() else { return };
        let code = input.read(cx).value().trim().to_string();
        let result = if code.is_empty() {
            Err(anyhow::anyhow!(t("Type the connection code the other device shows.")))
        } else {
            peer::verify(&code)
        };
        match result {
            Ok(name) => {
                input.update(cx, |input, cx| input.set_value("", window, cx));
                self.link = Some(Link::Verify { done: Some(name), error: None });
                self.after_account_change(cx);
            }
            Err(err) => {
                self.link = Some(Link::Verify { done: None, error: Some(format!("{err:#}")) });
            }
        }
        cx.notify();
    }

    // ---- Rendering

    fn render_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self._devices_poll.is_none() {
            self.watch_devices(cx);
        }
        let theme = cx.theme().clone();
        let mut page = div().child(Self::heading("Devices", cx)).child(
            div()
                .mb_4()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(t(
                    "Use your other machines from this one: run agents there, and see their \
                     threads here. Sign in to the same account on each, or connect them with an \
                     authorization code; CodeAny picks the fastest way, directly over the same \
                     Wi-Fi or Tailscale network when it can.",
                )),
        );
        let error = self.device_error.clone().or_else(|| match remote::login_status() {
            remote::Login::Failed(err) => Some(err),
            _ => None,
        });
        if let Some(error) = error {
            page = page.child(Self::error_banner(error, cx));
        }
        page.child(self.render_this_device(window, cx))
            .child(self.render_other_devices(window, cx))
            .into_any_element()
    }

    fn error_banner(error: String, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .my_2()
            .p_3()
            .rounded_lg()
            .bg(theme.danger.opacity(0.08))
            .text_size(TEXT)
            .text_color(theme.danger)
            .child(error)
    }

    fn subheading(title: &'static str, cx: &App) -> Div {
        div()
            .mt_6()
            .mb_1()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .text_size(TEXT)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .child(t(title)),
            )
    }

    fn status_dot(color: Hsla) -> impl IntoElement {
        div().size(px(8.)).flex_none().rounded_full().bg(color)
    }

    fn render_this_device(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let busy = self.busy;
        let status = remote::status();
        let pending_login = remote::login_status() == remote::Login::Pending;
        let me = self
            .devices
            .as_ref()
            .and_then(|d| d.as_ref().ok())
            .and_then(|d| d.iter().find(|d| d.is_self).cloned());
        let name = me.as_ref().map(|d| d.name.clone()).unwrap_or_else(remote::this_name);
        let self_id = me.as_ref().map(|d| d.id.clone()).unwrap_or_else(|| remote::SELF_ID.to_string());
        let online = status.as_ref().is_some_and(|s| s.host == remote::HostStatus::On);

        let mut card = div().child(Self::subheading("This device", cx));

        // Name and account, with the one action on the account.
        let name_cell: AnyElement = if self.renaming.as_deref() == Some(self_id.as_str()) {
            self.rename_editor(cx)
        } else {
            let id = self_id.clone();
            let current = name.clone();
            div()
                .id("self-name")
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .child(div().text_size(TEXT).font_weight(FontWeight::MEDIUM).child(name.clone()))
                .child(Icon::new(IconName::Pencil).size(ICON).text_color(theme.muted_foreground))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.start_rename(id.clone(), current.clone(), window, cx)
                }))
                .into_any_element()
        };
        let account_text = match &status {
            Some(status) => tf(
                "Signed in · {account}",
                &[("account", status.account.clone().unwrap_or_else(|| status.server.clone()))],
            ),
            None if pending_login => t("Waiting for you to approve in the browser…").into(),
            None => t("Not signed in").into(),
        };
        let actions: AnyElement = match (&status, &self.confirm) {
            (Some(_), Some(Confirm::SignOut)) => div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(TEXT)
                        .text_color(theme.muted_foreground)
                        .child(t("Sign out? Devices in the account can no longer reach this one.")),
                )
                .child(
                    Button::new("cancel-sign-out")
                        .ghost()
                        .small()
                        .app_label(t("Cancel"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("confirm-sign-out")
                        .primary()
                        .small()
                        .app_label(t("Sign out"))
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pairing = None;
                            this.device_action(remote::sign_out(), cx)
                        })),
                )
                .into_any_element(),
            (Some(_), _) => Button::new("sign-out")
                .outline()
                .small()
                .app_label(t("Sign out"))
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.confirm = Some(Confirm::SignOut);
                    cx.notify();
                }))
                .into_any_element(),
            (None, _) => div()
                .flex()
                .items_center()
                .gap_2()
                .when(pending_login, |this| {
                    this.child(
                        Button::new("cancel-sign-in")
                            .ghost()
                            .small()
                            .app_label(t("Cancel"))
                            .on_click(cx.listener(|_, _, _, cx| {
                                remote::cancel_login();
                                cx.notify();
                            })),
                    )
                })
                .child(
                    Button::new("sign-in")
                        .primary()
                        .small()
                        .app_label(if pending_login { t("Open the browser again") } else { t("Sign in with termany.sh") })
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.sign_in_browser(cx))),
                )
                .into_any_element(),
        };
        card = card.child(
            div()
                .py_3()
                .flex()
                .items_center()
                .gap_3()
                .border_b_1()
                .border_color(theme.border.opacity(0.6))
                .child(Self::status_dot(if online { theme.success } else { theme.muted_foreground.opacity(0.4) }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(name_cell)
                        .child(div().text_size(TEXT).text_color(theme.muted_foreground).child(account_text)),
                )
                .child(actions),
        );
        if let Some(remote::Status { host: remote::HostStatus::Failed(err), .. }) = &status {
            card = card.child(Self::error_banner(tf("Not reachable through the account: {err}", &[("err", err.to_string())]), cx));
        }

        // Signed out: other ways into an account.
        if status.is_none() {
            if let Some((server, account)) = remote::termany_account() {
                let hint = match account {
                    Some(account) => tf("Termany on this machine is signed in as {account}.", &[("account", account.to_string())]),
                    None => tf("Termany on this machine is linked to {server}.", &[("server", server.to_string())]),
                };
                card = card.child(Self::row(
                    "Use Termany's account",
                    Some(hint),
                    Button::new("join-termany")
                        .outline()
                        .small()
                        .app_label(t("Use it"))
                        .disabled(busy || pending_login)
                        .on_click(cx.listener(|this, _, _, cx| this.device_action(remote::join_termany(true), cx)))
                        .into_any_element(),
                    cx,
                ));
            }
            let code_input = self.code_input.clone();
            card = card.child(Self::row(
                "Join with an account code",
                Some(t("From “Add a device to the account” on a machine already signed in.").into()),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(code_input, |this, input| {
                        this.child(div().w(px(140.)).child(Input::new(&input).text_size(TEXT).small()))
                    })
                    .child(
                        Button::new("join-code")
                            .outline()
                            .small()
                            .app_label(t("Join"))
                            .disabled(busy || pending_login)
                            .on_click(cx.listener(|this, _, window, cx| this.join_with_code(window, cx))),
                    )
                    .into_any_element(),
                cx,
            ));
        }

        // The authorization code another device pastes to connect.
        let invite = peer::invite();
        let short = if invite.len() > 36 { format!("{}…{}", &invite[..24], &invite[invite.len() - 8..]) } else { invite };
        card = card.child(Self::row(
            "Authorization code",
            Some(tf(
                "Paste it into “Connect a device” on your other machine. {code} · {fingerprint}",
                &[("code", short), ("fingerprint", peer::fingerprint(&peer::public_key()).to_string())],
            )),
            Button::new("copy-invite")
                .outline()
                .small()
                .app_icon(if self.copied { IconName::Check } else { IconName::Copy })
                .app_label(if self.copied { t("Copied") } else { t("Copy") })
                .on_click(cx.listener(|this, _, _, cx| this.copy_invite(cx)))
                .into_any_element(),
            cx,
        ));

        let sharing = remote::hosting();
        card = card.child(Self::row(
            "Share this machine",
            Some(if sharing {
                t("Your other devices can run agents here and see this machine's threads while CodeAny is open.").into()
            } else {
                t("Your other devices cannot run agents here or see its threads.").into()
            }),
            Switch::new("share-agents")
                .checked(sharing)
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    if let Err(err) = remote::set_hosting(*on) {
                        this.device_error = Some(format!("{err:#}"));
                    }
                    // The relay lists the services a moment later.
                    let refresh = cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                        this.update(cx, |this, cx| this.after_account_change(cx)).ok();
                    });
                    refresh.detach();
                    cx.notify();
                }))
                .into_any_element(),
            cx,
        ));

        let verified = peer::peers().iter().filter(|p| !p.pending).count();
        if status.is_some() && verified > 0 {
            card = card.child(Self::row(
                "Only end-to-end encrypted devices",
                Some(t("Devices in the account that are not connected by code can no longer reach this one. Encrypted ones keep working, directly or through the cloud.").into()),
                Switch::new("strict")
                    .checked(peer::strict())
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        peer::set_strict(*on);
                        this.after_account_change(cx);
                    }))
                    .into_any_element(),
                cx,
            ));
        }

        if status.is_some() {
            let pairing: AnyElement = match &self.pairing {
                Some(code) => div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().px_2().py_1().rounded_md().bg(theme.secondary).text_size(TEXT).child(code.code.clone()))
                    .child({
                        let text = code.code.clone();
                        Button::new("copy-code")
                            .ghost()
                            .small()
                            .app_icon(IconName::Copy)
                            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
                    })
                    .into_any_element(),
                None => Button::new("pairing-code")
                    .outline()
                    .small()
                    .app_label(t("Get a code"))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.create_pairing_code(cx)))
                    .into_any_element(),
            };
            card = card.child(Self::row(
                "Add a device to the account",
                Some(t("A one-time code for “Join with an account code” on a machine that is not signed in.").into()),
                pairing,
                cx,
            ));
        }
        let _ = window;
        card.into_any_element()
    }

    fn rename_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let input = self.name_input.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .when_some(input, |this, input| this.child(div().w(px(200.)).child(Input::new(&input).text_size(TEXT).small())))
            .child(
                Button::new("save-name")
                    .primary()
                    .small()
                    .app_label(t("Save"))
                    .on_click(cx.listener(|this, _, _, cx| this.save_rename(cx))),
            )
            .child(
                Button::new("cancel-name")
                    .ghost()
                    .small()
                    .app_label(t("Cancel"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.renaming = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_other_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let measuring = self.measuring;
        let mut card = div().child(
            Self::subheading("Other devices", cx)
                .child(
                    Button::new("refresh-devices")
                        .ghost()
                        .small()
                        .app_icon(IconName::RefreshCw)
                        .app_label(if measuring { t("Measuring…") } else { t("Refresh") })
                        .disabled(measuring)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_now(cx))),
                )
                .child(
                    Button::new("verify-access")
                        .outline()
                        .small()
                        .app_label(t("Verify access"))
                        .on_click(cx.listener(|this, _, window, cx| this.open_verify(window, cx))),
                )
                .child(
                    Button::new("connect-device")
                        .primary()
                        .small()
                        .app_label(t("Connect a device"))
                        .on_click(cx.listener(|this, _, window, cx| this.open_connect(window, cx))),
                ),
        );

        if self.link.is_some() {
            card = card.child(self.render_link(window, cx));
        }

        // Devices asking to connect.
        for (ix, request) in peer::requests().into_iter().enumerate() {
            let key = request.public_key.clone();
            card = card.child(
                div()
                    .my_2()
                    .p_3()
                    .rounded_lg()
                    .bg(theme.primary.opacity(0.08))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(TEXT)
                            .child(tf("{name} asks to connect to this device", &[("name", request.name.clone())])),
                    )
                    .child(
                        Button::new(("decline-request", ix))
                            .ghost()
                            .small()
                            .app_label(t("Decline"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                peer::decline(&key);
                                cx.notify();
                                this.load_devices(cx);
                            })),
                    )
                    .child(
                        Button::new(("verify-request", ix))
                            .primary()
                            .small()
                            .app_label(t("Verify"))
                            .on_click(cx.listener(|this, _, window, cx| this.open_verify(window, cx))),
                    ),
            );
        }

        let list: AnyElement = match &self.devices {
            None => div().py_3().text_size(TEXT).text_color(theme.muted_foreground).child(t("Loading…")).into_any_element(),
            Some(Err(err)) => Self::error_banner(err.clone(), cx).into_any_element(),
            Some(Ok(devices)) => {
                let mut devices: Vec<remote::Device> = devices.iter().filter(|d| !d.is_self).cloned().collect();
                devices.sort_by(|a, b| {
                    (!a.online(), a.name.to_lowercase()).cmp(&(!b.online(), b.name.to_lowercase()))
                });
                if devices.is_empty() {
                    div()
                        .py_3()
                        .text_size(TEXT)
                        .text_color(theme.muted_foreground)
                        .child(t(
                            "No other devices yet. Sign in to the same account on another machine, or \
                             click Connect a device and paste its authorization code.",
                        ))
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .children(devices.into_iter().enumerate().map(|(ix, device)| self.render_device(ix, device, cx)))
                        .into_any_element()
                }
            }
        };
        card.child(list).into_any_element()
    }

    fn render_link(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let close_label = match &self.link {
            Some(Link::Connect { connected: true, .. }) | Some(Link::Verify { done: Some(_), .. }) => t("Done"),
            _ => t("Cancel"),
        };
        let close = Button::new("close-link")
            .ghost()
            .small()
            .app_label(close_label)
            .on_click(cx.listener(|this, _, _, cx| this.close_link(cx)));
        let mut panel = div()
            .my_2()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary.opacity(0.4))
            .flex()
            .flex_col()
            .gap_3();
        let muted = |text: String| div().text_size(TEXT).text_color(theme.muted_foreground).child(text);
        match &self.link {
            Some(Link::Connect { contacting, offer, connected, error }) => {
                panel = panel.child(div().text_size(TEXT).font_weight(FontWeight::SEMIBOLD).child(t("Connect a device")));
                match (offer, connected) {
                    (Some(offer), true) => {
                        panel = panel.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(TEXT)
                                .child(Icon::new(IconName::CircleCheck).size(ICON).text_color(theme.success))
                                .child(tf("Connected to {name}. Its threads show up in the sidebar when it shares them.", &[("name", offer.name.clone())])),
                        );
                    }
                    (Some(offer), false) => {
                        panel = panel
                            .child(muted(tf(
                                "On {name}, open Settings → Devices, click Verify access and type this connection code:",
                                &[("name", offer.name.clone())],
                            )))
                            .child(
                                div()
                                    .py_2()
                                    .text_size(px(28.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .font_family("Menlo")
                                    .child(offer.code.clone()),
                            )
                            .child(muted(tf("Waiting for {name} to verify…", &[("name", offer.name.clone())])));
                    }
                    (None, _) if *contacting => {
                        panel = panel.child(muted(t("Contacting the other device…").into()));
                    }
                    (None, _) => {
                        let input = self.auth_input.clone();
                        panel = panel
                            .child(muted(
                                t("Paste the other device's authorization code (Settings → Devices on that machine).").into(),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .when_some(input, |this, input| {
                                        this.child(div().flex_1().child(Input::new(&input).text_size(TEXT).small()))
                                    })
                                    .child(
                                        Button::new("submit-connect")
                                            .primary()
                                            .small()
                                            .app_label(t("Next"))
                                            .on_click(cx.listener(|this, _, _, cx| this.submit_connect(cx))),
                                    ),
                            );
                    }
                }
                if let Some(error) = error {
                    panel = panel.child(div().text_size(TEXT).text_color(theme.danger).child(error.clone()));
                }
            }
            Some(Link::Verify { done, error }) => {
                panel = panel.child(div().text_size(TEXT).font_weight(FontWeight::SEMIBOLD).child(t("Verify access")));
                match done {
                    Some(name) => {
                        panel = panel.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(TEXT)
                                .child(Icon::new(IconName::CircleCheck).size(ICON).text_color(theme.success))
                                .child(tf("{name} is connected to this device.", &[("name", name.clone())])),
                        );
                    }
                    None => {
                        let input = self.verify_input.clone();
                        panel = panel
                            .child(muted(t("Type the connection code shown on the device that is connecting.").into()))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .when_some(input, |this, input| {
                                        this.child(div().w(px(180.)).child(Input::new(&input).text_size(TEXT).small()))
                                    })
                                    .child(
                                        Button::new("submit-verify")
                                            .primary()
                                            .small()
                                            .app_label(t("Verify"))
                                            .on_click(cx.listener(|this, _, window, cx| this.submit_verify(window, cx))),
                                    ),
                            );
                    }
                }
                if let Some(error) = error {
                    panel = panel.child(div().text_size(TEXT).text_color(theme.danger).child(error.clone()));
                }
            }
            None => {}
        }
        panel.child(div().flex().justify_end().child(close)).into_any_element()
    }

    fn render_device(&self, ix: usize, device: remote::Device, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let busy = self.busy;
        let paired = device.paired();
        let pending = device.pending();

        let name_cell: AnyElement = if self.renaming.as_deref() == Some(device.id.as_str()) {
            self.rename_editor(cx)
        } else {
            let id = device.id.clone();
            let current = device.name.clone();
            div()
                .id(("device-name", ix))
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .child(div().text_size(TEXT).font_weight(FontWeight::MEDIUM).child(device.name.clone()))
                .child(Icon::new(IconName::Pencil).size(ICON).text_color(theme.muted_foreground.opacity(0.6)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.start_rename(id.clone(), current.clone(), window, cx)
                }))
                .into_any_element()
        };

        // What it is and how it is doing.
        let mut detail = Vec::new();
        if let Some(app) = device.app.as_deref() {
            detail.push(match app {
                "codeany" => "CodeAny".to_string(),
                "termany" => "Termany".to_string(),
                other => other.to_string(),
            });
        }
        detail.push(if device.via_account { t("In your account").into() } else { t("Connected by code").into() });
        if pending {
            detail.push(t("waiting for it to verify the connection code").into());
        } else if device.requires_pairing {
            detail.push(t("only accepts end-to-end encrypted devices").into());
        } else if !device.online() {
            detail.push(match device.last_seen_at {
                Some(_) if device.via_account => t("offline").into(),
                _ if !device.via_account => t("not reachable on this network").into(),
                _ => t("offline").into(),
            });
        } else if device.shares {
            detail.push(t("shares agents and threads").into());
        } else {
            detail.push(t("not sharing").into());
        }

        // Both ways to it, as round trips: LAN 3 ms · Cloud 180 ms.
        let mut routes = Vec::new();
        if paired && !pending {
            let direct = device.direct.as_ref();
            let network = direct
                .and_then(|d| d.endpoint.as_ref())
                .map(|e| e.kind.label())
                .unwrap_or(t("Direct"));
            routes.push((
                match direct.and_then(|d| d.rtt_ms) {
                    Some(ms) => format!("{network} {ms} ms"),
                    None => format!("{network} —"),
                },
                direct.is_some_and(|d| d.rtt_ms.is_some()),
            ));
        }
        if device.via_account && device.account_online {
            let direct_in_use = routes.first().is_some_and(|(_, used)| *used);
            routes.push((
                match device.relay_rtt {
                    Some(ms) => tf("Cloud {ms} ms", &[("ms", ms.to_string())]),
                    None => t("Cloud —").into(),
                },
                !direct_in_use,
            ));
        }
        let direct_note = device
            .direct
            .as_ref()
            .filter(|d| paired && d.rtt_ms.is_none())
            .and_then(|d| d.note.clone());

        // End-to-end encryption, as one switch.
        let e2e: Option<AnyElement> = if pending {
            None
        } else if paired {
            let key = device.peer.as_ref().map(|p| p.public_key.clone()).unwrap_or_default();
            Some(
                Button::new(("e2e-on", ix))
                    .ghost()
                    .small()
                    .app_icon(IconName::Lock)
                    .app_label(t("End-to-end encrypted"))
                    .disabled(!device.via_account)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm = Some(Confirm::Unpair(key.clone()));
                        cx.notify();
                    }))
                    .into_any_element(),
            )
        } else if device.via_account && (device.account_online || device.requires_pairing) && device.app.as_deref() == Some("codeany") {
            let id = device.id.clone();
            Some(
                Button::new(("e2e-off", ix))
                    .outline()
                    .small()
                    .app_icon(IconName::LockOpen)
                    .app_label(t("Turn on end-to-end encryption"))
                    .on_click(cx.listener(move |this, _, _, cx| this.connect_device(id.clone(), cx)))
                    .into_any_element(),
            )
        } else {
            None
        };

        let confirm = self.confirm.clone();
        let peer_key = device.peer.as_ref().map(|p| p.public_key.clone());
        let actions: AnyElement = match (&confirm, &peer_key) {
            (Some(Confirm::Unpair(key)), Some(mine)) if key == mine => {
                let key = key.clone();
                let question = if device.via_account {
                    tf("Turn off end-to-end encryption with {name}? It stays reachable through the account.", &[("name", device.name.clone())])
                } else {
                    tf("Remove {name}? It can no longer reach this device, nor this device it.", &[("name", device.name.clone())])
                };
                self.confirm_row(ix, question, cx.listener(move |this, _, _, cx| {
                    peer::remove(&key);
                    this.confirm = None;
                    this.after_account_change(cx);
                }), cx)
            }
            (Some(Confirm::Revoke(id)), _) if *id == device.id => {
                let id = id.clone();
                self.confirm_row(
                    ix,
                    tf("Remove {name} from the account? It has to sign in again to come back.", &[("name", device.name.clone())]),
                    cx.listener(move |this, _, _, cx| this.device_action(remote::revoke(id.clone()), cx)),
                    cx,
                )
            }
            _ => {
                let mut row = div().flex().items_center().gap_1();
                if let Some(key) = peer_key.clone().filter(|_| !device.via_account || pending) {
                    row = row.child(
                        Button::new(("remove-peer", ix))
                            .ghost()
                            .small()
                            .app_label(if pending { t("Cancel") } else { t("Remove") })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if pending {
                                    peer::remove(&key);
                                    this.after_account_change(cx);
                                } else {
                                    this.confirm = Some(Confirm::Unpair(key.clone()));
                                    cx.notify();
                                }
                            })),
                    );
                }
                if device.via_account {
                    let id = device.id.clone();
                    row = row.child(
                        Button::new(("revoke-device", ix))
                            .ghost()
                            .small()
                            .app_icon(IconName::Trash)
                            .disabled(busy)
                            .tooltip(t("Remove from the account"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm = Some(Confirm::Revoke(id.clone()));
                                cx.notify();
                            })),
                    );
                }
                row.into_any_element()
            }
        };

        let dot = if device.online() {
            theme.success
        } else if pending {
            theme.warning
        } else {
            theme.muted_foreground.opacity(0.4)
        };
        div()
            .py_3()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(theme.border.opacity(0.6))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(Self::status_dot(dot))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(name_cell)
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .text_size(TEXT)
                                    .text_color(theme.muted_foreground)
                                    .child(detail.join(" · "))
                                    .children(routes.into_iter().map(|(label, used)| {
                                        div()
                                            .px_1p5()
                                            .rounded_md()
                                            .when(used, |this| this.bg(theme.secondary).text_color(theme.foreground))
                                            .child(label)
                                    })),
                            )
                            .when_some(direct_note, |this, note| {
                                this.child(
                                    div()
                                        .text_size(TEXT)
                                        .text_color(theme.muted_foreground.opacity(0.8))
                                        .child(direct_hint(&note)),
                                )
                            }),
                    )
                    .when_some(e2e, |this, e2e| this.child(e2e))
                    .child(actions),
            )
            .into_any_element()
    }

    fn confirm_row(
        &self,
        ix: usize,
        question: String,
        on_confirm: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .max_w(px(420.))
            .child(div().text_size(TEXT).text_color(cx.theme().muted_foreground).child(question))
            .child(
                Button::new(("confirm-cancel", ix))
                    .ghost()
                    .small()
                    .app_label(t("Cancel"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.confirm = None;
                        cx.notify();
                    })),
            )
            .child(
                Button::new(("confirm-ok", ix))
                    .primary()
                    .small()
                    .app_label(t("Confirm"))
                    .on_click(on_confirm),
            )
            .into_any_element()
    }
}

/// Start a pairing on the agent runtime, where its sockets live.
fn agl_pair(
    pairing: impl std::future::Future<Output = anyhow::Result<peer::Offer>> + Send + 'static,
) -> impl std::future::Future<Output = anyhow::Result<peer::Offer>> + 'static {
    crate::agl_bridge::spawn(pairing)
}

/// Why a paired device does not answer directly, in plain words.
fn direct_hint(note: &str) -> String {
    let note_lower = note.to_lowercase();
    if note_lower.contains("blocked") || note_lower.contains("permission") {
        t("Not reachable on this network: on macOS allow CodeAny in System Settings → Privacy & Security → Local Network, on both machines.").into()
    } else if note_lower.contains("timed out") {
        t("No answer on this network: guest or office Wi-Fi may keep devices apart, or a VPN takes the LAN traffic. Tailscale works around both.").into()
    } else if note_lower.contains("refused") {
        t("Found it, but the connection was refused: its CodeAny may be closed, outdated, or blocked by a firewall.").into()
    } else if note_lower.contains("no address") {
        t("It has no LAN or Tailscale address to reach it at.").into()
    } else {
        tf("Not reachable directly: {note}", &[("note", note.to_string())])
    }
}
