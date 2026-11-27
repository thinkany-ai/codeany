//! The settings window: a modal with sections on the left, like Termany's.

use crate::style::AppButtonStyle as _;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    Disableable as _,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    menu::DropdownMenu as _,
    switch::Switch,
    window_paddings,
};
use nex::proto::PairingCode;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{
    agent_icons::agent_icon,
    keymap,
    agent_panel::AgentPanel,
    peer, remote, style,
    themes::{self, Palette, THEMES},
};
use crate::style::size::{TEXT, ICON};
use crate::i18n::{t, tf};

/// The dialog's size; the content scrolls inside.
pub const WIDTH: f32 = 1040.;
pub const HEIGHT: f32 = 720.;
const FRAME_BORDER: Pixels = px(1.);
const NAV_WIDTH: f32 = 220.;
const CONTENT_PADDING: Pixels = px(28.);
const CARD_PADDING: Rems = rems(1.);
const REPO: &str = "https://github.com/thinkany-ai/codeany";
/// About-section destinations, like Termany's.
const ABOUT_LINKS: &[(&str, &str)] = &[
    ("Website", "https://codeany.ai?utm_source=codeany_app&utm_medium=settings_about"),
    ("Source code", REPO),
    ("Feedback", "https://github.com/thinkany-ai/codeany/issues"),
];
const THEME_CARD_MIN_WIDTH: Pixels = px(188.);
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

/// The agent settings being edited.
struct AgentEditor {
    id: String,
    path: Entity<InputState>,
    args: Entity<TextareaState>,
    /// `NAME=value` per line, set on the agent's processes.
    env: Entity<TextareaState>,
    /// The executable when the editor opened, to tell whether it changed.
    original_command: String,
    /// Whether that executable was chosen here, not found on the PATH.
    custom_command: bool,
    original_env: Vec<(String, String)>,
    icon: IconChoice,
    error: Option<String>,
    saving: bool,
}

enum IconChoice {
    Keep,
    Picked(PathBuf),
    /// Back to the agent's own logo.
    Reset,
}

#[derive(Clone)]
enum AgentTest {
    Running,
    Passed(std::time::Duration),
    Failed(String),
}

/// Whether devices can join through a termany.sh account. Off until the
/// cloud side ships: devices connect with an authorization code only. A
/// machine already signed in still shows the account and can sign out.
const ACCOUNTS: bool = false;

/// Shortcuts that belong to one control and cannot be changed.
const FIXED_SHORTCUTS: &[(&str, &str)] = &[
    ("↩", "Send message"),
    ("⇧↩", "New line in message"),
    ("⌘1 … ⌘9", "Pick a model"),
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
    /// A device that just connected by scanning the QR code.
    scanned_in: Option<String>,
    /// A code that adds a device to the account.
    pairing: Option<PairingCode>,
    code_input: Option<Entity<InputState>>,
    name_input: Option<Entity<InputState>>,
    /// The authorization code pasted to connect a device.
    auth_input: Option<Entity<InputState>>,
    /// The connection code typed to verify one.
    verify_input: Option<Entity<InputState>>,
    /// Enter in each input does what its button does.
    _input_subscriptions: Vec<Subscription>,
    /// The device being renamed.
    renaming: Option<String>,
    /// An action waiting for a second click.
    confirm: Option<Confirm>,
    /// Connecting or verifying a device.
    link: Option<Link>,
    /// The authorization code was just copied.
    copied: bool,
    /// The connection code was just copied.
    code_copied: bool,
    /// The agent whose settings are open, with its inputs.
    agent_editor: Option<AgentEditor>,
    /// Connection tests, by agent: running, or how the last one went.
    agent_tests: HashMap<String, AgentTest>,
    /// Measuring every way to every device.
    measuring: bool,
    _devices_poll: Option<Task<()>>,
    _link_poll: Option<Task<()>>,
    /// Re-render About when an update turns up.
    _update_watch: Option<Subscription>,
    /// The shortcut waiting for its new keystroke.
    recording: Option<&'static str>,
    _recorder: Option<Subscription>,
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
            scanned_in: None,
            pairing: None,
            code_input: None,
            name_input: None,
            auth_input: None,
            verify_input: None,
            _input_subscriptions: Vec::new(),
            agent_editor: None,
            agent_tests: HashMap::new(),
            renaming: None,
            confirm: None,
            link: None,
            copied: false,
            code_copied: false,
            measuring: false,
            _devices_poll: None,
            _link_poll: None,
            _update_watch: None,
            recording: None,
            _recorder: None,
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
                        this.stop_recording(cx);
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
        let language = panel.read(cx).language_preference();
        let language_menu = Button::new("language")
            .outline()
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
        let language = Self::row(
            "Language",
            Some(t("Follows the system unless you pick one.").into()),
            language_menu.into_any_element(),
            cx,
        );
        div()
            .child(Self::heading("General", cx))
            .child(Self::card(language, cx))
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

    fn render_appearance(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        // Match the dialog's width clamp when choosing how many previews fit.
        let inset = window_paddings(window);
        let dialog_width = px(WIDTH).min(
            window.viewport_size().width - inset.left - inset.right - theme.spacing_tokens().lg * 2.,
        );
        let grid_width = dialog_width
            - px(NAV_WIDTH)
            - (CONTENT_PADDING + CARD_PADDING.to_pixels(window.rem_size()) + FRAME_BORDER * 2.) * 2.;
        let column_gap = px(20.);
        let columns =
            (((grid_width + column_gap) / (THEME_CARD_MIN_WIDTH + column_gap)) as u16).clamp(1, 3);
        let card_radius = theme.radius_lg;
        let preview_radius = (card_radius - CARD_BORDER).max(px(0.));
        // The theme of the workspace this window shows.
        let current = self.agent_panel.upgrade().map(|p| p.read(cx).theme_choice()).unwrap_or_else(|| style::choice(cx));
        let weak = self.agent_panel.clone();
        let card = |id: &'static str, name: &'static str, preview: AnyElement, selected: bool| {
            let weak = weak.clone();
            div()
                .id(id)
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

        let grid = div()
            .py_4()
            .grid()
            .grid_cols(columns)
            .gap_x(column_gap)
            .gap_y(px(22.))
            .child(card("system", t("System"), system_preview, current == "system"))
            .children(THEMES.iter().map(|p| {
                card(
                    p.id,
                    p.name,
                    Self::theme_preview(p, preview_radius).into_any_element(),
                    current == p.id,
                )
            }));
        div()
            .child(Self::heading("Appearance", cx))
            .child(self.render_typography(cx))
            .child(Self::subheading("Theme", cx))
            .child(
                div()
                    .mb_2()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(t("For the workspace this window shows; each keeps its own.")),
            )
            .child(Self::card(grid, cx))
            .into_any_element()
    }

    fn render_typography(&self, cx: &App) -> impl IntoElement {
        let typography = style::typography(cx);
        let fonts_panel = self.agent_panel.clone();
        let sizes_panel = self.agent_panel.clone();
        let font_picker = Button::new("appearance-font")
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

        let rows = div()
            .child(Self::row("Font", None, font_picker.into_any_element(), cx))
            .child(Self::row("Text size", None, size_picker.into_any_element(), cx));
        div().child(Self::subheading("Typography", cx).mt_0()).child(Self::card(rows, cx))
    }

    fn open_agent_editor(&mut self, agent: &str, detected: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        // The executable in use: the one chosen here, else the one found.
        let custom = crate::agl_bridge::agent_command(agent);
        let custom_command = custom.is_some();
        let command = custom.or_else(|| detected.clone()).unwrap_or_default();
        let env = crate::agl_bridge::agent_env(agent);
        let env_text = env_text(&env);
        let args = self
            .agent_panel
            .upgrade()
            .map(|panel| panel.read(cx).agent_args(agent).join("\n"))
            .unwrap_or_default();
        let placeholder = "/path/to/agent".to_string();
        let path = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder(placeholder);
            input.set_value(command.clone(), window, cx);
            input
        });
        let args_input = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx).auto_grow(3, 8).placeholder("--flag\nvalue");
            input.set_value(args.clone(), window, cx);
            input
        });
        let env_input = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx).auto_grow(3, 8).placeholder("ANTHROPIC_BASE_URL=https://…\nMY_API_KEY");
            input.set_value(env_text, window, cx);
            input
        });
        self.agent_editor = Some(AgentEditor {
            id: agent.to_string(),
            path,
            args: args_input,
            env: env_input,
            original_command: command,
            custom_command,
            original_env: env,
            icon: IconChoice::Keep,
            error: None,
            saving: false,
        });
        cx.notify();
    }

    fn pick_agent_icon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("Choose picture").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().find(|p| crate::threads::is_image_path(p)) else { return };
            this.update(cx, |this, cx| {
                if let Some(editor) = &mut this.agent_editor {
                    editor.icon = IconChoice::Picked(path);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Save the open agent's icon, arguments and executable; agents are
    /// then detected again, so the list shows the executable in use.
    fn save_agent(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.agent_editor else { return };
        let id = editor.id.clone();
        let command = editor.path.read(cx).value().trim().to_string();
        let args: Vec<String> = editor
            .args
            .read(cx)
            .value()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        let env = match parse_env(&editor.env.read(cx).value()) {
            Ok(env) => env,
            Err(err) => {
                editor.error = Some(err);
                cx.notify();
                return;
            }
        };
        let icon = std::mem::replace(&mut editor.icon, IconChoice::Keep);
        let icon_result = match &icon {
            IconChoice::Keep => Ok(()),
            IconChoice::Picked(path) => crate::agent_icons::set_custom_icon(&id, Some(path)),
            IconChoice::Reset => crate::agent_icons::set_custom_icon(&id, None),
        };
        if let Err(err) = icon_result {
            editor.error = Some(tf("Cannot use this picture: {err}", &[("err", err.to_string())]));
            cx.notify();
            return;
        }
        if let Some(panel) = self.agent_panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_agent_args(&id, args, cx));
        }
        let command_changed = command != editor.original_command;
        let env_changed = env != editor.original_env;
        if !command_changed && !env_changed {
            self.agent_editor = None;
            self.agent_tests.remove(&id);
            cx.refresh_windows();
            cx.notify();
            return;
        }
        editor.saving = true;
        editor.error = None;
        // An executable found on the PATH and left alone stays looked up.
        let command = (command_changed || editor.custom_command).then_some(command);
        let save = crate::agl_bridge::set_agent_launch(id.clone(), command, env);
        cx.spawn(async move |this, cx| {
            let result = save.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.agent_editor = None;
                        this.agent_tests.remove(&id);
                        if let Some(panel) = this.agent_panel.upgrade() {
                            panel.update(cx, |panel, cx| panel.redetect_local_agents(cx));
                        }
                    }
                    Err(err) => {
                        if let Some(editor) = &mut this.agent_editor {
                            editor.saving = false;
                            editor.error = Some(format!("{err:#}"));
                        }
                    }
                }
                cx.refresh_windows();
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Send the agent one short prompt, the way a thread would, and time it.
    fn test_agent(&mut self, agent: String, cx: &mut Context<Self>) {
        // The arguments being edited, saved or not; the executable must be
        // saved first (agl reads it from its own configuration).
        let args = match &self.agent_editor {
            Some(editor) if editor.id == agent => editor
                .args
                .read(cx)
                .value()
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            _ => self.agent_panel.upgrade().map(|p| p.read(cx).agent_args(&agent)).unwrap_or_default(),
        };
        self.agent_tests.insert(agent.clone(), AgentTest::Running);
        let test = crate::agl_bridge::test_agent(agent.clone(), args);
        cx.spawn(async move |this, cx| {
            let result = test.await;
            this.update(cx, |this, cx| {
                this.agent_tests.insert(
                    agent,
                    match result {
                        Ok(took) => AgentTest::Passed(took),
                        Err(err) => AgentTest::Failed(format!("{err:#}")),
                    },
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _ = window;
        let theme = cx.theme().clone();
        let agents = self
            .agent_panel
            .upgrade()
            .and_then(|panel| panel.read(cx).local_agents().map(|a| a.to_vec()));
        let intro = div()
            .mb_2()
            .text_size(TEXT)
            .text_color(theme.muted_foreground)
            .child(t("Each agent you turn on is offered when you start or switch a thread. Its executable, arguments and icon are under Edit, with a connection test."));
        let Some(mut agents) = agents else {
            return div()
                .child(Self::heading("Agents", cx))
                .child(intro)
                .child(div().text_size(TEXT).text_color(theme.muted_foreground).child(t("Detecting agents…")))
                .into_any_element();
        };
        agents.sort_by_key(|a| !a.available);
        let mut rows = div();
        for (ix, agent) in agents.into_iter().enumerate() {
            let id = agent.id.clone();
            let editing = self.agent_editor.as_ref().is_some_and(|e| e.id == id);
            let enabled = self.agent_panel.upgrade().is_some_and(|p| p.read(cx).agent_enabled(&id));
            // The version says it runs; the executable is under Edit.
            let detail = match &agent.version {
                _ if !agent.available => t("Not installed").to_string(),
                Some(version) => version.clone(),
                None => t("Version unknown").to_string(),
            };
            let detected = agent.path.clone();
            let mut row = div()
                .py_3()
                .flex()
                .flex_col()
                .gap_2()
                .border_b_1()
                .border_color(theme.border.opacity(0.6))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().when(!agent.available, |this| this.opacity(0.55)).child(agent_icon(&id, px(32.), cx)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(div().text_size(TEXT).font_weight(FontWeight::MEDIUM).child(agent.name.clone()))
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
                        .child(
                            // Only enabled agents are offered when starting or
                            // switching a thread.
                            Switch::new(("enable-agent", ix))
                                .checked(enabled)
                                .disabled(!agent.available)
                                .on_click(cx.listener({
                                    let id = id.clone();
                                    move |this, on: &bool, _, cx| {
                                        if let Some(panel) = this.agent_panel.upgrade() {
                                            panel.update(cx, |panel, cx| panel.set_agent_enabled(&id, *on, cx));
                                        }
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(
                            Button::new(("edit-agent", ix))
                                .outline()
                                .app_label(t("Edit"))
                                .disabled(editing)
                                .on_click(cx.listener({
                                    let id = id.clone();
                                    move |this, _, window, cx| this.open_agent_editor(&id, detected.clone(), window, cx)
                                })),
                        ),
                );
            if editing {
                row = row.child(self.render_agent_editor(&id, cx));
            }
            rows = rows.child(row);
        }
        div()
            .child(Self::heading("Agents", cx))
            .child(intro)
            .child(Self::card(rows, cx))
            .into_any_element()
    }

    fn render_agent_editor(&self, agent: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(editor) = &self.agent_editor else { return div().into_any_element() };
        let editable = crate::agl_bridge::agent_command_editable(agent);
        let installed = self
            .agent_panel
            .upgrade()
            .and_then(|p| p.read(cx).local_agents().map(|a| a.iter().any(|a| a.id == agent && a.available)))
            .unwrap_or(false);
        let test = self.agent_tests.get(agent).cloned();
        let testing = matches!(test, Some(AgentTest::Running));
        let env_changed = parse_env(&editor.env.read(cx).value()).ok().as_ref() != Some(&editor.original_env);
        // The test runs what agl has saved.
        let path_changed = editor.path.read(cx).value().trim() != editor.original_command.trim() || env_changed;
        let test_line: AnyElement = match test {
            _ if path_changed => div()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(t(if env_changed { "Save to test with the new environment." } else { "Save to test the new executable." }))
                .into_any_element(),
            Some(AgentTest::Passed(took)) => div()
                .flex()
                .items_center()
                .gap_1()
                .text_size(TEXT)
                .text_color(theme.success)
                .child(Icon::new(IconName::CircleCheck).size(ICON).flex_none())
                .child(tf("Connected · {s}s", &[("s", format!("{:.1}", took.as_secs_f32()))]))
                .into_any_element(),
            Some(AgentTest::Failed(err)) => div()
                .flex()
                .items_start()
                .gap_1()
                .text_size(TEXT)
                .text_color(theme.danger)
                .child(Icon::new(IconName::CircleAlert).size(ICON).mt(px(2.)).flex_none())
                .child(div().min_w_0().child(err))
                .into_any_element(),
            _ => div().into_any_element(),
        };
        let label = |text: &'static str| div().w(px(120.)).flex_none().pt(px(6.)).text_size(TEXT).child(t(text));
        let preview: AnyElement = match &editor.icon {
            IconChoice::Picked(path) => div()
                .size(px(40.))
                .rounded(px(9.))
                .overflow_hidden()
                .child(img(path.clone()).size_full().object_fit(ObjectFit::Cover))
                .into_any_element(),
            IconChoice::Reset => div().child(crate::agent_icons::builtin_icon(agent, px(40.), cx)).into_any_element(),
            IconChoice::Keep => agent_icon(agent, px(40.), cx),
        };
        let has_custom = match &editor.icon {
            IconChoice::Picked(_) => true,
            IconChoice::Reset => false,
            IconChoice::Keep => crate::agent_icons::custom_icon(agent).is_some(),
        };
        div()
            .ml(px(44.))
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_start()
                    .child(label("Icon"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(preview)
                            .child(
                                Button::new("agent-icon-pick")
                                    .outline()
                                    .app_label(t("Choose picture"))
                                    .on_click(cx.listener(|this, _, window, cx| this.pick_agent_icon(window, cx))),
                            )
                            .when(has_custom, |this| {
                                this.child(
                                    Button::new("agent-icon-reset")
                                        .ghost()
                                        .app_label(t("Use its own"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(editor) = &mut this.agent_editor {
                                                editor.icon = IconChoice::Reset;
                                            }
                                            cx.notify();
                                        })),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .child(label("Executable"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(Input::new(&editor.path).text_size(TEXT).disabled(!editable))
                            .child(div().text_size(TEXT).text_color(theme.muted_foreground).child(if editable {
                                t("Leave empty to find it on the PATH.")
                            } else {
                                t("This agent's executable cannot be changed here.")
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .child(label("Arguments"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme.input)
                                    .child(Textarea::new(&editor.args).text_size(TEXT).appearance(false).bordered(false)),
                            )
                            .child(
                                div()
                                    .text_size(TEXT)
                                    .text_color(theme.muted_foreground)
                                    .child(t("One per line, added after the agent's own arguments. New threads use them.")),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .child(label("Environment"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme.input)
                                    .when(!editable, |this| this.opacity(0.5))
                                    .child(
                                        Textarea::new(&editor.env)
                                            .text_size(TEXT)
                                            .appearance(false)
                                            .bordered(false)
                                            .disabled(!editable),
                                    ),
                            )
                            .child(div().text_size(TEXT).text_color(theme.muted_foreground).child(if editable {
                                t("One per line: NAME=value sets a value, stored in plain text on this machine; NAME alone passes that variable from your shell, read each time the agent starts. New threads use them.")
                            } else {
                                t("This agent's environment cannot be changed here.")
                            })),
                    ),
            )
            .when_some(editor.error.clone(), |this, err| this.child(Self::error_banner(err, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        // Tests what is saved: the executable and arguments a
                        // new thread would run with.
                        Button::new("agent-test")
                            .outline()
                            .app_label(if testing { t("Testing…") } else { t("Test connection") })
                            .disabled(testing || !installed || path_changed)
                            .on_click(cx.listener({
                                let id = agent.to_string();
                                move |this, _, _, cx| this.test_agent(id.clone(), cx)
                            })),
                    )
                    .child(div().flex_1().min_w_0().child(test_line))
                    .child(
                        Button::new("agent-cancel")
                            .ghost()
                            .app_label(t("Cancel"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.agent_editor = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("agent-save")
                            .primary()
                            .app_label(if editor.saving { t("Saving…") } else { t("Save") })
                            .disabled(editor.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save_agent(cx))),
                    ),
            )
            .into_any_element()
    }

    /// Take the next keystroke, anywhere in the app, as the shortcut's
    /// new keys; Esc cancels. Nothing else sees keystrokes meanwhile.
    fn start_recording(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if self.recording == Some(id) {
            self.stop_recording(cx);
            return;
        }
        self.recording = Some(id);
        let weak = cx.entity().downgrade();
        self._recorder = Some(cx.intercept_keystrokes(move |event, _, cx| {
            let Some(this) = weak.upgrade() else { return };
            let Some(id) = this.read(cx).recording else { return };
            cx.stop_propagation();
            let keystroke = &event.keystroke;
            let cancel = keystroke.key == "escape" && !keystroke.modifiers.modified();
            let keys = keymap::from_keystroke(keystroke);
            if !cancel && keys.is_none() {
                return; // Modifiers only, or a plain key: keep waiting.
            }
            // Not dropped here: this is that subscription's own callback.
            this.update(cx, |this, cx| {
                this.recording = None;
                cx.notify();
            });
            let weak = weak.clone();
            cx.defer(move |cx| {
                weak.update(cx, |this, _| this._recorder = None).ok();
            });
            if let Some(keys) = keys {
                keymap::set(id, Some(keys), cx);
            }
        }));
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording.take().is_some() {
            self._recorder = None;
            cx.notify();
        }
    }

    /// A shortcut's keys as a chip, like a key cap.
    fn key_chip(text: String, cx: &App) -> Div {
        let theme = cx.theme();
        div()
            .h(px(26.))
            .px_2()
            .flex()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme.border.opacity(0.0))
            .bg(theme.secondary)
            .text_size(TEXT)
            .text_color(theme.muted_foreground)
            .whitespace_nowrap()
            .child(text)
    }

    fn render_shortcuts(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let conflicts = keymap::conflicts(cx);
        let mut page = div().child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .child(Self::heading("Shortcuts", cx))
                .when(keymap::any_custom(cx), |this| {
                    this.child(
                        Button::new("reset-shortcuts")
                            .ghost()
                            .small()
                            .app_label(t("Reset all"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stop_recording(cx);
                                keymap::reset_all(cx);
                                cx.notify();
                            })),
                    )
                }),
        );
        page = page.child(
            div()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(t("Click a shortcut, then press the new keys. Esc cancels.")),
        );
        for group in keymap::Group::ALL {
            let mut rows = div();
            for shortcut in keymap::SHORTCUTS.iter().filter(|s| s.group == group) {
                let id = shortcut.id;
                let recording = self.recording == Some(id);
                let conflict = conflicts.get(id).copied();
                let keys = keymap::keys(id, cx).iter().map(|k| keymap::display(k)).collect::<Vec<_>>().join(" / ");
                let chip = Self::key_chip(if recording { t("Press keys…").to_string() } else { keys }, cx)
                    .id(id)
                    .cursor_pointer()
                    .hover(|this| this.bg(theme.list_hover))
                    .when(recording, |this| this.border_color(theme.primary).text_color(theme.primary))
                    .when(conflict.is_some() && !recording, |this| this.text_color(theme.danger))
                    .on_click(cx.listener(move |this, _, _, cx| this.start_recording(id, cx)));
                let control = div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(keymap::is_custom(id, cx) && !recording, |this| {
                        this.child(
                            Button::new(("reset-shortcut", keymap::SHORTCUTS.iter().position(|s| s.id == id).unwrap_or(0)))
                                .ghost()
                                .small()
                                .app_label(t("Reset"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.stop_recording(cx);
                                    keymap::set(id, None, cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(chip)
                    .into_any_element();
                let hint = conflict.map(|other| tf("Also used by “{name}”", &[("name", t(other).to_string())]));
                rows = rows.child(Self::row(shortcut.label, hint, control, cx));
            }
            page = page.child(Self::subheading(group.label(), cx)).child(Self::card(rows, cx));
        }
        let mut fixed = div();
        for (keys, label) in FIXED_SHORTCUTS {
            fixed = fixed.child(Self::row(label, None, Self::key_chip(t(keys).to_string(), cx).into_any_element(), cx));
        }
        page.child(Self::subheading("Composer and model menu", cx))
            .child(Self::card(fixed, cx))
            .into_any_element()
    }

    fn render_about(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        // The updater runs in the released app only (Sparkle in the bundle).
        let update = cx
            .try_global::<crate::updater::AvailableUpdate>()
            .map(|update| update.0.clone());
        if update.is_some() && self._update_watch.is_none() {
            self._update_watch = Some(cx.observe_global::<crate::updater::AvailableUpdate>(|_, cx| cx.notify()));
        }
        let update_row: AnyElement = match update {
            Some(Some(version)) => Button::new("install-update")
                .primary()
                .small()
                .app_label(tf("Update to v{version}", &[("version", version)]))
                .on_click(|_, _, _| crate::updater::check())
                .into_any_element(),
            Some(None) => Button::new("check-updates")
                .outline()
                .small()
                .app_label(t("Check for Updates"))
                .on_click(|_, _, _| crate::updater::check())
                .into_any_element(),
            None => div()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(t("Automatic updates are available in the released app."))
                .into_any_element(),
        };
        let links = ABOUT_LINKS.iter().enumerate().map(|(ix, (label, url))| {
            let url = *url;
            div()
                .id(("about-link", ix))
                .px(px(14.))
                .py(px(11.))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .text_size(TEXT)
                .cursor_pointer()
                .when(ix + 1 < ABOUT_LINKS.len(), |this| this.border_b_1().border_color(theme.border))
                .hover(|this| this.bg(theme.list_hover))
                .child(t(label))
                .child(Icon::new(IconName::ExternalLink).size(ICON).text_color(theme.muted_foreground))
                .on_click(move |_, _, cx| cx.open_url(url))
        });
        div()
            .child(Self::heading("About", cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .child(img(logo()).size(px(52.)).rounded(px(12.)).flex_none())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(18.)).font_weight(FontWeight::BOLD).child("CodeAny"))
                            .child(
                                div()
                                    .text_size(TEXT)
                                    .text_color(theme.muted_foreground)
                                    .child(tf("Version {version}", &[("version", env!("CARGO_PKG_VERSION").to_string())])),
                            ),
                    ),
            )
            // A row, so the button keeps its own width.
            .child(div().mt(px(10.)).mb(px(14.)).flex().child(update_row))
            .child(
                div()
                    .mb(px(18.))
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(t("A local-first coding agent.")),
            )
            .child(
                // The same card as the other pages.
                div()
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_xl()
                    .bg(theme.secondary.opacity(0.35))
                    .children(links),
            )
            .into_any_element()
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Shrink with small windows rather than running off the bottom.
        let max_height = window.viewport_size().height - px(48.);
        // Each input submits on Enter, like clicking its button. (The dialog
        // would otherwise take Enter as "OK" and close the whole settings.)
        let on_enter = |input: &Entity<InputState>,
                        window: &mut Window,
                        cx: &mut Context<Self>,
                        submit: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            cx.subscribe_in(input, window, move |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    submit(this, window, cx);
                }
            })
        };
        if self.code_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("XXXX-XXXX"));
            let sub = on_enter(&input, window, cx, |this, window, cx| this.join_with_code(window, cx));
            self._input_subscriptions.push(sub);
            self.code_input = Some(input);
        }
        if self.name_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("Device name")));
            let sub = on_enter(&input, window, cx, |this, _, cx| this.save_rename(cx));
            self._input_subscriptions.push(sub);
            self.name_input = Some(input);
        }
        if self.auth_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("codeany1.…"));
            let sub = on_enter(&input, window, cx, |this, _, cx| this.submit_connect(cx));
            self._input_subscriptions.push(sub);
            self.auth_input = Some(input);
        }
        if self.verify_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("XXXX-XXXX"));
            let sub = on_enter(&input, window, cx, |this, window, cx| this.submit_verify(window, cx));
            self._input_subscriptions.push(sub);
            self.verify_input = Some(input);
        }
        let content = match self.section {
            Section::General => self.render_general(cx),
            Section::Appearance => self.render_appearance(window, cx),
            Section::Agents => self.render_agents(window, cx),
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
            // Its own text color: the window's root, which hosts dialogs, is
            // drawn before the window puts its workspace's theme in place, so
            // what it passes down may be another window's theme.
            .text_color(theme.foreground)
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
                            .px(CONTENT_PADDING)
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

    /// Copy a fresh authorization code, good for ten minutes; the row counts
    /// down meanwhile.
    fn copy_invite(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(peer::new_invite()));
        self.copied = true;
        self.watch_invite(cx);
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

    /// A fresh authorization code as a QR code, in a dialog over Settings,
    /// for a phone to scan.
    fn show_invite_qr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.scanned_in = None;
        let Some(image) = qr_image(&peer::new_scan_invite()) else {
            self.device_error = Some(t("The authorization code is too long for a QR code.").into());
            cx.notify();
            return;
        };
        self.watch_invite(cx);
        cx.notify();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            dialog
                .w(px(360.))
                .title(t("Scan to connect"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_3()
                        .child(
                            // Always dark on white: scanners need the contrast,
                            // in a dark theme too.
                            div()
                                .p_3()
                                .rounded_lg()
                                .bg(gpui_kit::white())
                                .child(img(image.clone()).size(px(240.)).object_fit(ObjectFit::Contain)),
                        )
                        .child(
                            div()
                                .text_size(TEXT)
                                .text_color(theme.muted_foreground)
                                .text_center()
                                .child(t("Scan it with the device that should connect: it connects right away. The code works once, for 10 minutes, and replaces the code copied before.")),
                        ),
                )
        });
        // The phone that scans it is connected without a connection code:
        // close the dialog and say so.
        cx.spawn_in(window, async move |this, cx| {
            while peer::invite_expires_in().is_some() {
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                if let Some(name) = peer::take_scanned_in().pop() {
                    this.update_in(cx, |this, window, cx| {
                        window.close_dialog(cx);
                        this.scanned_in = Some(name);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            }
        })
        .detach();
    }

    /// Keep the code's time left fresh in the hint until it runs out.
    fn watch_invite(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            while peer::invite_expires_in().is_some() {
                cx.background_executor().timer(std::time::Duration::from_secs(15)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
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
        let intro = if ACCOUNTS {
            t("Use your other machines from this one: run agents there, and see their \
               threads here. Sign in to the same account on each, or connect them with an \
               authorization code; CodeAny picks the fastest way, directly over the same \
               Wi-Fi or Tailscale network when it can.")
        } else {
            t("Use your other machines from this one: run agents there, and see their \
               threads here. Connect them with an authorization code; CodeAny connects \
               directly over the same Wi-Fi or Tailscale network.")
        };
        if self._devices_poll.is_none() {
            self.watch_devices(cx);
        }
        let theme = cx.theme().clone();
        let mut page = div().child(Self::heading("Devices", cx)).child(
            div()
                .mb_4()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(intro),
        );
        let error = self.device_error.clone().or_else(|| match remote::login_status() {
            remote::Login::Failed(err) => Some(err),
            _ => None,
        });
        if let Some(error) = error {
            page = page.child(Self::error_banner(error, cx));
        }
        if let Some(name) = &self.scanned_in {
            page = page.child(
                div()
                    .my_2()
                    .p_3()
                    .rounded_lg()
                    .bg(theme.success.opacity(0.1))
                    .text_size(TEXT)
                    .text_color(theme.success)
                    .child(tf("{name} connected by scanning the QR code.", &[("name", name.clone())])),
            );
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

    /// Related rows grouped in a rounded card. Rows draw a rule under
    /// themselves; the card hides the last one against its own border.
    fn card(content: impl IntoElement, cx: &App) -> Div {
        let theme = cx.theme();
        div()
            .mt_2()
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary.opacity(0.35))
            .px(CARD_PADDING)
            .overflow_hidden()
            .child(div().mb(px(-1.)).child(content))
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

        // This machine: its name and whether it is shared. Access: the ways
        // another machine connects, from either end.
        let mut card = div();
        let mut access = div();
        let mut notes = div();

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
        let account_text: Option<String> = match &status {
            Some(status) => Some(tf(
                "Signed in · {account}",
                &[("account", status.account.clone().unwrap_or_else(|| status.server.clone()))],
            )),
            None if !ACCOUNTS => None,
            None if pending_login => Some(t("Waiting for you to approve in the browser…").into()),
            None => Some(t("Not signed in").into()),
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
                        .app_label(t("Cancel"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("confirm-sign-out")
                        .primary()
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
                .app_label(t("Sign out"))
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.confirm = Some(Confirm::SignOut);
                    cx.notify();
                }))
                .into_any_element(),
            // Without accounts there is nothing to sign in to.
            (None, _) if !ACCOUNTS => div().into_any_element(),
            (None, _) => div()
                .flex()
                .items_center()
                .gap_2()
                .when(pending_login, |this| {
                    this.child(
                        Button::new("cancel-sign-in")
                            .ghost()
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
                        .when_some(account_text, |this, text| {
                            this.child(div().text_size(TEXT).text_color(theme.muted_foreground).child(text))
                        }),
                )
                .child(actions),
        );
        if let Some(remote::Status { host: remote::HostStatus::Failed(err), .. }) = &status {
            notes = notes.child(Self::error_banner(tf("Not reachable through the account: {err}", &[("err", err.to_string())]), cx));
        }

        // Signed out: other ways into an account.
        if ACCOUNTS && status.is_none() {
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
                        this.child(div().w(px(140.)).child(Input::new(&input).text_size(TEXT)))
                    })
                    .child(
                        Button::new("join-code")
                            .outline()
                            .app_label(t("Join"))
                            .disabled(busy || pending_login)
                            .on_click(cx.listener(|this, _, window, cx| this.join_with_code(window, cx))),
                    )
                    .into_any_element(),
                cx,
            ));
        }

        // The authorization code another device pastes to connect.
        // A fresh code on every copy, good for ten minutes.
        let hint = match peer::invite_expires_in() {
            Some(left) => tf(
                "Copied. Paste it into “Connect a device” on your other machine within {minutes} min.",
                &[("minutes", left.as_secs().div_ceil(60).max(1).to_string())],
            ),
            None => t("Copy a code and paste it into “Connect a device” on your other machine; it works for 10 minutes.").to_string(),
        };
        access = access.child(Self::row(
            "Authorization code",
            Some(hint),
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("invite-qr")
                        .outline()
                        .app_icon(crate::icons::qr_code())
                        .app_label(t("QR code"))
                        .on_click(cx.listener(|this, _, window, cx| this.show_invite_qr(window, cx))),
                )
                .child(
                    Button::new("copy-invite")
                        .outline()
                        .app_icon(if self.copied { IconName::Check } else { IconName::Copy })
                        .app_label(if self.copied { t("Copied") } else { t("Copy") })
                        .on_click(cx.listener(|this, _, _, cx| this.copy_invite(cx))),
                )
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
        if ACCOUNTS && status.is_some() && verified > 0 {
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

        if ACCOUNTS && status.is_some() {
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
                            .app_icon(IconName::Copy)
                            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
                    })
                    .into_any_element(),
                None => Button::new("pairing-code")
                    .outline()
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
        access = access
            .child(Self::row(
                "Connect a device",
                Some(t("Paste the authorization code from another machine to use it from here.").into()),
                Button::new("connect-device")
                    .outline()
                    .app_label(t("Connect"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_connect(window, cx)))
                    .into_any_element(),
                cx,
            ))
            .child(Self::row(
                "Verify access",
                Some(t("When another machine connects to this one, type the connection code it shows.").into()),
                Button::new("verify-access")
                    .outline()
                    .app_label(t("Verify"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_verify(window, cx)))
                    .into_any_element(),
                cx,
            ));
        div()
            .child(Self::subheading("This device", cx))
            .child(Self::card(card, cx))
            .child(notes)
            .child(Self::subheading("Access", cx))
            .child(Self::card(access, cx))
            .when(self.link.is_some(), |this| this.child(div().mt_2().child(self.render_link(window, cx))))
            .into_any_element()
    }

    fn rename_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let input = self.name_input.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .when_some(input, |this, input| this.child(div().w(px(200.)).child(Input::new(&input).text_size(TEXT))))
            .child(
                Button::new("save-name")
                    .primary()
                    .app_label(t("Save"))
                    .on_click(cx.listener(|this, _, _, cx| this.save_rename(cx))),
            )
            .child(
                Button::new("cancel-name")
                    .ghost()
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
                        .app_icon(IconName::RefreshCw)
                        .app_label(if measuring { t("Measuring…") } else { t("Refresh") })
                        .disabled(measuring)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_now(cx))),
                )
        );
        let _ = window;

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
                        .child(if ACCOUNTS {
                            t("No other devices yet. Sign in to the same account on another machine, or \
                               click Connect a device and paste its authorization code.")
                        } else {
                            t("No other devices yet. Copy the authorization code on another machine, then use Connect a device here.")
                        })
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
        card.child(Self::card(list, cx)).into_any_element()
    }

    fn render_link(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let close_label = match &self.link {
            Some(Link::Connect { connected: true, .. }) | Some(Link::Verify { done: Some(_), .. }) => t("Done"),
            _ => t("Cancel"),
        };
        let close = Button::new("close-link")
            .ghost()
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
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .text_size(px(28.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .font_family("Menlo")
                                            .child(offer.code.clone()),
                                    )
                                    .child({
                                        let code = offer.code.clone();
                                        let copied = self.code_copied;
                                        Button::new("copy-connection-code")
                                            .outline()
                                            .small()
                                            .app_icon(if copied { IconName::Check } else { IconName::Copy })
                                            .app_label(if copied { t("Copied") } else { t("Copy") })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                                                this.code_copied = true;
                                                cx.notify();
                                                cx.spawn(async move |this, cx| {
                                                    cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                                                    this.update(cx, |this, cx| {
                                                        this.code_copied = false;
                                                        cx.notify();
                                                    })
                                                    .ok();
                                                })
                                                .detach();
                                            }))
                                    }),
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
                                        this.child(div().flex_1().child(Input::new(&input).text_size(TEXT)))
                                    })
                                    .child(
                                        Button::new("submit-connect")
                                            .primary()
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
                                        this.child(div().w(px(180.)).child(Input::new(&input).text_size(TEXT)))
                                    })
                                    .child(
                                        Button::new("submit-verify")
                                            .primary()
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

        // How it is doing. The app is named only when it is not CodeAny (a
        // Termany in the same account); how it is reached shows in the
        // encryption badge and the round trips.
        let mut detail: Vec<String> = Vec::new();
        match device.app.as_deref() {
            None | Some("codeany") => {}
            Some("termany") => detail.push("Termany".into()),
            Some(other) => detail.push(other.into()),
        }
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
                            .app_icon(IconName::Trash)
                            .disabled(busy)
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
                    .app_label(t("Cancel"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.confirm = None;
                        cx.notify();
                    })),
            )
            .child(
                Button::new(("confirm-ok", ix))
                    .primary()
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

/// What a bare `NAME` line is stored as.
fn shell_ref(name: &str) -> String {
    format!("${{{name}}}")
}

/// The editor's text for saved variables: a bare name for one taken from
/// the shell.
fn env_text(env: &[(String, String)]) -> String {
    env.iter()
        .map(|(k, v)| if *v == shell_ref(k) { k.clone() } else { format!("{k}={v}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `NAME=value` or `NAME` lines (blank lines and `#` comments skipped),
/// sorted by name.
fn parse_env(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut env = std::collections::BTreeMap::new();
    for (ix, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let valid = |name: &str| {
            name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        match line.split_once('=') {
            Some((name, value)) if valid(name.trim()) => {
                let value = value.trim();
                // Quotes as a shell would write them.
                let value = value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                    .unwrap_or(value);
                env.insert(name.trim().to_string(), value.to_string());
            }
            // A bare name: its value from the user's shell, at each launch
            // (agl expands `${NAME}`).
            None if valid(line) => {
                env.insert(line.to_string(), shell_ref(line));
            }
            _ => return Err(tf("Environment line {n}: expected NAME=value.", &[("n", (ix + 1).to_string())])),
        }
    }
    Ok(env.into_iter().collect())
}

#[cfg(test)]
mod env_tests {
    #[test]
    fn parses_env_lines() {
        let env = super::parse_env("# key\nB=2\nexport A=\"x y\"\n\nC='z'\nD=a=b").unwrap();
        assert_eq!(
            env,
            vec![
                ("A".into(), "x y".into()),
                ("B".into(), "2".into()),
                ("C".into(), "z".into()),
                ("D".into(), "a=b".into())
            ]
        );
        assert!(super::parse_env("1BAD=x").is_err());
        assert_eq!(super::parse_env("MY_KEY").unwrap(), vec![("MY_KEY".into(), "${MY_KEY}".into())]);
        let saved = super::parse_env("MY_KEY\nB=2").unwrap();
        assert_eq!(super::env_text(&saved), "B=2\nMY_KEY");
        assert!(super::parse_env("1BAD").is_err());
        assert_eq!(super::parse_env("EMPTY=").unwrap(), vec![("EMPTY".into(), String::new())]);
    }
}

/// `text` as a QR code image (dark modules on white, with a quiet zone).
fn qr_image(text: &str) -> Option<Arc<Image>> {
    use qrcode::{EcLevel, QrCode, render::svg};
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L).ok()?;
    let svg = code
        .render::<svg::Color>()
        .min_dimensions(480, 480)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Some(Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())))
}

#[cfg(test)]
mod qr_tests {
    #[test]
    fn a_long_authorization_code_fits_a_qr_code() {
        // Several endpoints make the code long; it must still render.
        let code = format!("codeany1.{}", "A".repeat(1500));
        assert!(super::qr_image(&code).is_some());
    }
}
