//! The settings window: a modal with sections on the left, like Termany's.

use std::sync::{Arc, OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{
    agent_icons::agent_icon,
    agent_panel::AgentPanel,
    paths, style,
    themes::{self, Palette, THEMES},
    threads::{ACCESS_LEVELS, access_name, agent_name},
};
use crate::style::size::{HEADING, ICON, TEXT, TEXT_SM, TEXT_XS};

/// The dialog's size; the content scrolls inside.
pub const WIDTH: f32 = 1040.;
pub const HEIGHT: f32 = 720.;
const NAV_WIDTH: f32 = 220.;
const CARD_WIDTH: f32 = 188.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Appearance,
    Agents,
    Shortcuts,
    About,
}

impl Section {
    const ALL: [Section; 5] = [
        Section::General,
        Section::Appearance,
        Section::Agents,
        Section::Shortcuts,
        Section::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Agents => "Agents",
            Section::Shortcuts => "Shortcuts",
            Section::About => "About",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::General => IconName::Settings,
            Section::Appearance => IconName::Palette,
            Section::Agents => IconName::Bot,
            Section::Shortcuts => IconName::Keyboard,
            Section::About => IconName::Info,
        }
    }
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("⌘N", "New thread"),
    ("⌘E", "Switch Agent / Editor"),
    ("⌘O", "Open folder"),
    ("⌘,", "Settings"),
    ("⌘B", "Toggle sidebar"),
    ("⌘J / ⌃`", "Toggle terminal"),
    ("⌘⇧G", "Toggle git changes"),
    ("⌘S", "Save file"),
    ("⌘W", "Close tab"),
    ("Enter", "Send message"),
    ("⇧Enter", "New line in message"),
    ("⌘1 … ⌘9", "Pick a model in the model menu"),
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
}

impl Settings {
    pub fn new(agent_panel: &Entity<AgentPanel>) -> Self {
        Self {
            section: Section::Appearance,
            agent_panel: agent_panel.downgrade(),
        }
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .w(px(NAV_WIDTH))
            .flex_none()
            .h_full()
            .px_4()
            .py_6()
            .flex()
            .flex_col()
            .gap_1()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_2()
                    .pb_3()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child("Settings"),
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
                    .child(section.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.section = section;
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
            .child(title)
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
                    .child(div().text_size(TEXT).child(label))
                    .when_some(hint, |this, hint| {
                        this.child(
                            div()
                                .text_size(TEXT_SM)
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
            .label(current_agent.as_deref().map(agent_name).unwrap_or_else(|| "None".into()))
            .dropdown_caret(true)
            .dropdown_menu({
                let weak = weak.clone();
                move |mut menu, _, _| {
                    for agent in agents.iter().filter(|a| a.available) {
                        let (weak, id) = (weak.clone(), agent.id.clone());
                        menu = menu.item(
                            PopupMenuItem::new(agent.name.clone())
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
            .label(access_name(&access))
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (id, name) in ACCESS_LEVELS {
                    let weak = weak.clone();
                    menu = menu.item(PopupMenuItem::new(name).checked(access == id).on_click(
                        move |_, _, cx| {
                            weak.update(cx, |panel, cx| panel.set_default_access(id, cx)).ok();
                        },
                    ));
                }
                menu
            });
        let data_dir = paths::home();
        div()
            .child(Self::heading("General", cx))
            .child(Self::row(
                "Default agent",
                Some("New threads start with this agent; open threads keep theirs.".into()),
                agent_menu.into_any_element(),
                cx,
            ))
            .child(Self::row(
                "Default access",
                Some("What agents may do without asking.".into()),
                access_menu.into_any_element(),
                cx,
            ))
            .child(Self::row(
                "Data",
                Some(data_dir.display().to_string()),
                Button::new("reveal-data")
                    .outline()
                    .small()
                    .label("Show in Finder")
                    .on_click(move |_, _, cx| cx.reveal_path(&paths::database()))
                    .into_any_element(),
                cx,
            ))
            .into_any_element()
    }

    /// A small picture of a theme: sidebar strip, accent dot and text lines.
    fn theme_preview(p: &Palette) -> impl IntoElement {
        let color = |text: &str| -> Hsla {
            let (r, g, b, a) = themes::parse_color(text).unwrap_or_default();
            Rgba { r, g, b, a }.into()
        };
        let line = |w: f32, c: Hsla| div().w(px(w)).h(px(5.)).rounded_full().bg(c);
        div()
            .size_full()
            .flex()
            .bg(color(p.bg))
            .child(div().w(relative(0.3)).h_full().bg(color(p.sidebar.unwrap_or(p.bg2))))
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
                        .rounded_xl()
                        .overflow_hidden()
                        .border_2()
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
                            this.child(Icon::new(IconName::Check).size_3p5().text_color(theme.primary))
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
            .child(div().flex_1().h_full().bg(rgb(0xf7f7f8)))
            .child(div().flex_1().h_full().bg(rgb(0x18181b)))
            .into_any_element();

        div()
            .child(Self::heading("Theme", cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(20.))
                    .gap_y(px(22.))
                    .child(card("system", "System", system_preview, current == "system"))
                    .children(THEMES.iter().map(|p| {
                        card(p.id, p.name, Self::theme_preview(p).into_any_element(), current == p.id)
                    })),
            )
            .into_any_element()
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
                        .child("Detecting agents…"),
                )
            }
            Some(mut agents) => {
                agents.sort_by_key(|a| !a.available);
                for agent in agents {
                    let detail = match (&agent.version, &agent.path) {
                        _ if !agent.available => "Not installed".to_string(),
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
                                            .text_size(TEXT_SM)
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
                                                .text_size(TEXT_XS)
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
                    .child(div().flex_1().text_size(TEXT).child(*action))
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(2.))
                            .rounded_md()
                            .bg(theme.secondary)
                            .text_size(TEXT_SM)
                            .text_color(theme.muted_foreground)
                            .child(*keys),
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
            .child(div().mt_2().text_size(HEADING).font_weight(FontWeight::SEMIBOLD).child("CodeAny"))
            .child(
                div()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
            )
            .child(
                div()
                    .mt_4()
                    .max_w(px(420.))
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .text_center()
                    .child("A lightweight coding-agent app with an editor, built on GPUI and agl."),
            )
            .child(
                div()
                    .mt_2()
                    .text_size(TEXT_SM)
                    .text_color(theme.muted_foreground)
                    .child(format!("Data: {}", paths::home().display())),
            )
            .into_any_element()
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Shrink with small windows rather than running off the bottom.
        let max_height = window.viewport_size().height - px(48.);
        let content = match self.section {
            Section::General => self.render_general(cx),
            Section::Appearance => self.render_appearance(cx),
            Section::Agents => self.render_agents(cx),
            Section::Shortcuts => self.render_shortcuts(cx),
            Section::About => self.render_about(cx),
        };
        div()
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .max_h(max_height)
            .flex()
            .rounded_xl()
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
                                .icon(Icon::new(IconName::X).size(ICON))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                    ),
            )
    }
}
