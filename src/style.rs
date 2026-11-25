//! The app's look: a calm palette for light and dark that follows the
//! system, or one of the built-in themes in [`crate::themes`].

use std::sync::RwLock;

use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable, Theme, ThemeMode, button::{Button, ButtonVariants as _}, menu::PopupMenuItem,
    text::{TextView, TextViewStyle}, tooltip::Tooltip,
};
use anyhow::Result;
use gpui_kit::*;

use crate::themes::{self, Palette, parse_color};

/// Text and action icons follow the window's typography setting.
pub mod size {
    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::{App, Pixels, Rems, rems};

    pub const TEXT: Rems = rems(1.);

    /// 16 px icons with the default 14 px text.
    pub const ICON: Rems = rems(8. / 7.);

    pub fn icon_pixels(cx: &App) -> Pixels {
        ICON.to_pixels(cx.theme().font_size)
    }
}

/// Shared geometry for the window, navigation and transcript.
/// Keep spacing on a 4 px grid; icon sizes live in `size` above.
pub mod layout {
    use gpui_kit::{Pixels, px};

    pub const SPACE_1: Pixels = px(4.);
    pub const SPACE_2: Pixels = px(8.);
    pub const SPACE_3: Pixels = px(12.);
    pub const SPACE_4: Pixels = px(16.);
    pub const SPACE_6: Pixels = px(24.);
    pub const CONTROL: Pixels = px(28.);
    pub const HEADER: Pixels = px(40.);
    pub const RADIUS: Pixels = px(8.);
    pub const BUBBLE_RADIUS: Pixels = px(12.);
    pub const SIDEBAR_INSET: Pixels = SPACE_2;
    pub const ROW_INSET: Pixels = SPACE_2;
    pub const SIDEBAR_ROW: Pixels = px(28.);
    pub const SIDEBAR_ROW_INSET: Pixels = SPACE_2;
    pub const SIDEBAR_NAV_ROW: Pixels = px(32.);
    pub const CONTENT_MAX: Pixels = px(720.);
}

/// A borderless icon button in the window chrome; they all share one size.
pub fn chrome_button(id: &'static str) -> Button {
    Button::new(id)
        .ghost()
        .size(layout::CONTROL)
        .w(layout::CONTROL)
        .h(layout::CONTROL)
        .p_0()
}

/// The fixed-width font for code and the terminal.
pub const MONO_FONT: &str = "Menlo";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Typography {
    pub family: Option<String>,
    pub size: u8,
}

impl Default for Typography {
    fn default() -> Self {
        Self { family: None, size: 14 }
    }
}

impl Global for Typography {}

impl Typography {
    pub const SIZES: std::ops::RangeInclusive<u8> = 11..=20;

    fn apply(&self, theme: &mut Theme) {
        let family = self.family.as_deref().unwrap_or(".SystemUIFont");
        theme.font_family = family.to_owned().into();
        // Code and the terminal need fixed-width glyphs, whatever the UI font.
        theme.mono_font_family = MONO_FONT.into();
        theme.font_size = px(self.size.clamp(*Self::SIZES.start(), *Self::SIZES.end()) as f32);
        theme.mono_font_size = theme.font_size;
    }
}

pub fn typography(cx: &App) -> Typography {
    cx.try_global::<Typography>().cloned().unwrap_or_default()
}

pub fn set_typography(value: Typography, cx: &mut App) {
    cx.set_global(value.clone());
    Theme::update(cx, |theme| value.apply(theme));
}

/// App labels and icons use the same scale across button sizes.
pub trait AppButtonStyle {
    fn app_label(self, label: impl Into<SharedString>) -> Self;
    fn app_icon(self, icon: impl Into<Icon>) -> Self;
    fn app_tooltip(self, label: impl Into<SharedString>) -> AppTooltipButton;
}

impl AppButtonStyle for Button {
    fn app_label(self, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        self.accessibility_label(label.clone()).child(
            div().min_w_0().truncate().text_size(size::TEXT).child(label),
        )
    }

    fn app_icon(self, icon: impl Into<Icon>) -> Self {
        // Button::icon applies its own size to the supplied icon.
        self.child(icon.into().size(size::ICON))
    }

    fn app_tooltip(self, label: impl Into<SharedString>) -> AppTooltipButton {
        AppTooltipButton { button: self, label: label.into() }
    }
}

#[derive(IntoElement)]
pub struct AppTooltipButton {
    button: Button,
    label: SharedString,
}

impl Selectable for AppTooltipButton {
    fn selected(mut self, selected: bool) -> Self {
        self.button = self.button.selected(selected);
        self
    }

    fn is_selected(&self) -> bool {
        self.button.is_selected()
    }

    fn open(mut self, open: bool) -> Self {
        self.button = self.button.open(open);
        self
    }

    fn is_open(&self) -> bool {
        self.button.is_open()
    }
}

impl RenderOnce for AppTooltipButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        with_tooltip(self.button.accessibility_label(self.label.clone()), self.label)
    }
}

pub fn with_tooltip(content: impl IntoElement, label: impl Into<SharedString>) -> impl IntoElement {
    let label = label.into();
    div()
        .id(SharedString::from(format!("button-tooltip:{label}")))
        .flex_none()
        .child(content)
        .tooltip(move |window, cx| Tooltip::new(label.clone()).text_size(size::TEXT).build(window, cx))
}

pub fn menu_item(label: impl Into<SharedString>) -> PopupMenuItem {
    let label = label.into();
    PopupMenuItem::element(move |_, _| {
        div()
            .id("menu-item-label")
            .role(accesskit::Role::Label)
            .aria_label(label.clone())
            .flex_1()
            .text_size(size::TEXT)
            .child(label.clone())
    })
}

pub fn markdown(id: impl Into<ElementId>, text: impl Into<SharedString>, cx: &App) -> TextView {
    let font_size = cx.theme().font_size;
    let text_style = StyleRefinement::default().text_size(size::TEXT);
    TextView::markdown(id, text)
        .style(
            TextViewStyle::default()
                .heading_font_size(move |_, _| font_size)
                .code_block(text_style.clone())
                .table_cell(text_style.clone()),
        )
        .inline_code_style(text_style)
        .on_link_click(|url, _, _, cx| open_link(url, cx))
}

/// Open a link from an answer. Agents often link files by path
/// (`/Users/me/out/cat.png`, `~/notes.md`, `file:///…`, `src/main.rs:12`);
/// those open in their default app, as the system cannot open them as URLs.
pub fn open_link(url: &str, cx: &mut App) {
    match local_path(url) {
        Some(path) => cx.open_with_system(&path),
        None => cx.open_url(url),
    }
}

/// The existing local file or folder a link points to, if it is one.
fn local_path(url: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let raw = if let Some(rest) = url.strip_prefix("file://") {
        percent_decode(rest)
    } else if url.contains("://") || url.starts_with("mailto:") {
        return None;
    } else {
        percent_decode(url)
    };
    let path = match raw.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
        None => PathBuf::from(&raw),
    };
    if !path.is_absolute() {
        return None;
    }
    if path.exists() {
        return Some(path);
    }
    // `file.rs:12` or `file.rs:12:5`: the file, without the position.
    let text = path.to_string_lossy();
    let mut trimmed = text.as_ref();
    for _ in 0..2 {
        match trimmed.rsplit_once(':') {
            Some((head, tail)) if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) => trimmed = head,
            _ => break,
        }
    }
    let path = PathBuf::from(trimmed);
    path.exists().then_some(path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&text[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod link_tests {
    use super::{local_path, percent_decode};

    #[test]
    fn finds_local_files_behind_links() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let cargo = format!("{manifest}/Cargo.toml");
        assert_eq!(local_path(&cargo).unwrap().to_string_lossy(), cargo);
        assert_eq!(local_path(&format!("{cargo}:12:5")).unwrap().to_string_lossy(), cargo);
        assert_eq!(local_path(&format!("file://{}", cargo.replace(' ', "%20"))).unwrap().to_string_lossy(), cargo);
        assert!(local_path("https://example.com/a.png").is_none());
        assert!(local_path("/no/such/file.png").is_none());
        assert!(local_path("relative/file.md").is_none());
        assert_eq!(percent_decode("%E7%8C%AB.png"), "猫.png");
    }
}

/// Lucide's icons drawn with a lighter stroke, closer to the system's.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let data = gpui_kit::assets::AllAssets.load(path)?;
        if !path.ends_with(".svg") {
            return Ok(data);
        }
        Ok(data.map(|svg| match std::str::from_utf8(&svg) {
            Ok(text) if text.contains("stroke-width=\"2\"") => {
                text.replace("stroke-width=\"2\"", "stroke-width=\"1.75\"").into_bytes().into()
            }
            _ => svg,
        }))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::AllAssets.list(path)
    }
}

/// The chosen theme: `"system"` or a [`themes::THEMES`] id.
pub struct ThemeChoice(pub String);

impl Global for ThemeChoice {}

/// The terminal's ANSI colors under the current theme, when it sets them.
static ANSI: RwLock<Option<[(u8, u8, u8); 16]>> = RwLock::new(None);

/// ANSI color `index` (0..16) as the current theme defines it.
pub fn ansi(index: usize) -> Option<(u8, u8, u8)> {
    ANSI.read().ok()?.as_ref()?.get(index).copied()
}

pub fn choice(cx: &App) -> String {
    cx.try_global::<ThemeChoice>()
        .map(|c| c.0.clone())
        .unwrap_or_else(|| "system".into())
}

/// Switch themes and repaint.
pub fn set_choice(id: String, window: &mut Window, cx: &mut App) {
    cx.set_global(ThemeChoice(id));
    apply(window, cx);
}

fn color(text: &str) -> Hsla {
    let (r, g, b, a) = parse_color(text).unwrap_or((0., 0., 0., 1.));
    Rgba { r, g, b, a }.into()
}

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

fn hexa(value: u32) -> Hsla {
    rgba(value).into()
}

/// The dim layer behind modal dialogs, strong enough to set them apart.
fn overlay(dark: bool) -> Hsla {
    if dark { hexa(0x00000099) } else { hexa(0x00000059) }
}

/// Apply the chosen theme on top of the component library's defaults.
pub fn apply(window: &mut Window, cx: &mut App) {
    if let Some(palette) = themes::find(&choice(cx)) {
        apply_palette(palette, window, cx);
        return;
    }
    *ANSI.write().unwrap() = None;
    Theme::sync_system_appearance(Some(window), cx);
    let dark = Theme::global(cx).is_dark();
    let typography = typography(cx);

    Theme::update(cx, |theme| {
        theme.overlay = overlay(dark);
        typography.apply(theme);
        theme.radius = px(8.);
        theme.radius_lg = px(12.);

        if dark {
            theme.background = hex(0x18181b);
            theme.foreground = hex(0xececee);
            theme.sidebar = hex(0x121214);
            theme.sidebar_foreground = hex(0xececee);
            theme.sidebar_border = hex(0x232327);
            theme.border = hex(0x2a2a2f);
            theme.input = hex(0x2f2f35);
            theme.muted = hex(0x202024);
            theme.muted_foreground = hex(0x8b8b94);
            theme.secondary = hex(0x242428);
            theme.secondary_hover = hex(0x2c2c31);
            theme.secondary_active = hex(0x323238);
            theme.popover = hex(0x1f1f23);
            theme.list_hover = hexa(0xffffff0c);
            theme.list_active = hexa(0xffffff14);
            theme.list_active_border = hexa(0x00000000);
            theme.accent = hexa(0xffffff10);
            theme.primary = hex(0x7c83ff);
            theme.primary_hover = hex(0x8c92ff);
            theme.primary_active = hex(0x6c73f0);
            theme.primary_foreground = hex(0xffffff);
            theme.ring = hexa(0x7c83ff99);
            theme.selection = hexa(0x7c83ff40);
            theme.caret = hex(0xc7c9ff);
            theme.link = hex(0x9ea4ff);
            theme.title_bar = hex(0x18181b);
            theme.title_bar_border = hex(0x232327);
            theme.status_bar = hex(0x121214);
            theme.tab_bar = hex(0x121214);
            theme.tab = hex(0x121214);
            theme.tab_active = hex(0x18181b);
            theme.success = hex(0x4ade80);
            theme.warning = hex(0xfbbf24);
            theme.danger = hex(0xf87171);
            theme.scrollbar_thumb = hexa(0xffffff26);
            theme.scrollbar_thumb_hover = hexa(0xffffff40);
        } else {
            theme.background = hex(0xffffff);
            theme.foreground = hex(0x1c1c1f);
            theme.sidebar = hex(0xebebee);
            theme.sidebar_foreground = hex(0x1c1c1f);
            theme.sidebar_border = hex(0xebebee);
            theme.border = hex(0xe7e7ea);
            theme.input = hex(0xdedee3);
            theme.muted = hex(0xf4f4f5);
            theme.muted_foreground = hex(0x86868f);
            theme.secondary = hex(0xf2f2f4);
            theme.secondary_hover = hex(0xeaeaed);
            theme.secondary_active = hex(0xe2e2e6);
            theme.popover = hex(0xffffff);
            theme.list_hover = hexa(0x0000000a);
            theme.list_active = hexa(0x00000010);
            theme.list_active_border = hexa(0x00000000);
            theme.accent = hexa(0x0000000a);
            theme.primary = hex(0x6370f0);
            theme.primary_hover = hex(0x5562e8);
            theme.primary_active = hex(0x4955d8);
            theme.primary_foreground = hex(0xffffff);
            theme.ring = hexa(0x6370f080);
            theme.selection = hexa(0x6370f033);
            theme.caret = hex(0x4955d8);
            theme.link = hex(0x4f5bd5);
            theme.title_bar = hex(0xffffff);
            theme.title_bar_border = hex(0xebebee);
            theme.status_bar = hex(0xf7f7f8);
            theme.tab_bar = hex(0xf7f7f8);
            theme.tab = hex(0xf7f7f8);
            theme.tab_active = hex(0xffffff);
            theme.success = hex(0x16a34a);
            theme.warning = hex(0xd97706);
            theme.danger = hex(0xdc2626);
            theme.scrollbar_thumb = hexa(0x00000026);
            theme.scrollbar_thumb_hover = hexa(0x00000040);
        }
    });
}

/// Map a Termany-style palette onto the component theme.
fn apply_palette(p: &Palette, window: &mut Window, cx: &mut App) {
    Theme::change(if p.dark { ThemeMode::Dark } else { ThemeMode::Light }, Some(window), cx);
    *ANSI.write().unwrap() = p.ansi.map(|ansi| {
        ansi.map(|c| {
            let (r, g, b, _) = parse_color(c).unwrap_or_default();
            ((r * 255.) as u8, (g * 255.) as u8, (b * 255.) as u8)
        })
    });

    let (bg, bg2, bg3) = (color(p.bg), color(p.bg2), color(p.bg3));
    let (fg, fg_dim, border) = (color(p.fg), color(p.fg_dim), color(p.border));
    let accent = color(p.accent);
    let sidebar = p.sidebar.map(color).unwrap_or(bg2);
    // Text on the accent: dark on light accents, white otherwise.
    let on_accent = if accent.l > 0.62 { hex(0x111111) } else { hex(0xffffff) };
    let typography = typography(cx);

    Theme::update(cx, |theme| {
        theme.overlay = overlay(p.dark);
        typography.apply(theme);
        theme.radius = px(8.);
        theme.radius_lg = px(12.);

        theme.background = bg;
        theme.foreground = fg;
        theme.sidebar = sidebar;
        theme.sidebar_foreground = fg;
        theme.sidebar_border = border;
        theme.border = border;
        theme.input = border;
        theme.muted = bg3;
        theme.muted_foreground = fg_dim;
        theme.secondary = bg3;
        theme.secondary_hover = fg.opacity(0.1);
        theme.secondary_active = fg.opacity(0.14);
        // Popovers float above the page: raised in dark themes, the page itself in light ones.
        theme.popover = if p.dark { bg2 } else { bg };
        theme.list_hover = fg.opacity(0.05);
        theme.list_active = fg.opacity(0.09);
        theme.list_active_border = gpui_kit::transparent_black();
        theme.accent = fg.opacity(0.07);
        theme.primary = accent;
        theme.primary_hover = accent.opacity(0.88);
        theme.primary_active = accent.opacity(0.78);
        theme.primary_foreground = on_accent;
        theme.ring = accent.opacity(0.6);
        theme.selection = color(p.accent_soft).opacity(0.6_f32.max(color(p.accent_soft).a));
        theme.caret = p.cursor.map(color).unwrap_or(accent);
        theme.link = accent;
        theme.title_bar = bg;
        theme.title_bar_border = border;
        theme.status_bar = sidebar;
        theme.tab_bar = sidebar;
        theme.tab = sidebar;
        theme.tab_active = bg;
        theme.scrollbar_thumb = fg.opacity(0.18);
        theme.scrollbar_thumb_hover = fg.opacity(0.3);
        if let Some(ansi) = p.ansi {
            theme.success = color(ansi[2]);
            theme.warning = color(ansi[3]);
            theme.danger = color(ansi[1]);
        }
    });
}
