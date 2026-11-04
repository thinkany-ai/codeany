//! Agent logos, embedded in the binary (from douchat's icon set).

use std::{collections::HashMap, sync::Arc, sync::OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon};
use gpui_kit::*;

const LOGOS: &[(&str, &[u8], ImageFormat)] = &[
    ("claude", include_bytes!("../assets/agents/claudecode.svg"), ImageFormat::Svg),
    ("codex", include_bytes!("../assets/agents/codex.svg"), ImageFormat::Svg),
    ("cursor", include_bytes!("../assets/agents/cursor.svg"), ImageFormat::Svg),
    ("droid", include_bytes!("../assets/agents/droid.svg"), ImageFormat::Svg),
    ("gemini", include_bytes!("../assets/agents/gemini.svg"), ImageFormat::Svg),
    ("grok", include_bytes!("../assets/agents/grok.svg"), ImageFormat::Svg),
    ("hermes", include_bytes!("../assets/agents/hermes.webp"), ImageFormat::Webp),
    ("kilocode", include_bytes!("../assets/agents/kilocode.svg"), ImageFormat::Svg),
    ("kimi", include_bytes!("../assets/agents/kimi.svg"), ImageFormat::Svg),
    ("omp", include_bytes!("../assets/agents/omp.svg"), ImageFormat::Svg),
    ("openclaw", include_bytes!("../assets/agents/openclaw.svg"), ImageFormat::Svg),
    ("opencode", include_bytes!("../assets/agents/opencode.svg"), ImageFormat::Svg),
];

/// White marks that need a dark tile to be seen, as in douchat.
const ON_DARK: &[&str] = &["cursor", "droid", "grok", "kilocode", "opencode"];

fn logo(agent: &str) -> Option<Arc<Image>> {
    static IMAGES: OnceLock<HashMap<&'static str, Arc<Image>>> = OnceLock::new();
    IMAGES
        .get_or_init(|| {
            LOGOS
                .iter()
                .map(|(id, bytes, format)| (*id, Arc::new(Image::from_bytes(*format, bytes.to_vec()))))
                .collect()
        })
        .get(agent)
        .cloned()
}

/// An agent's logo at `size`, or a generic bot for agents without one.
pub fn agent_icon(agent: &str, size: Pixels, cx: &App) -> AnyElement {
    let Some(image) = logo(agent) else {
        return Icon::new(IconName::Bot)
            .size(size)
            .flex_none()
            .text_color(cx.theme().muted_foreground)
            .into_any_element();
    };
    if ON_DARK.contains(&agent) {
        div()
            .size(size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(size * 0.22)
            .bg(rgb(0x242528))
            .child(img(image).size(size * 0.72))
            .into_any_element()
    } else {
        div()
            .size(size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(img(image).size(size).object_fit(ObjectFit::Contain))
            .into_any_element()
    }
}

/// An agent's brand color, for naming it in text.
pub fn agent_color(agent: &str) -> Option<Hsla> {
    let hex = match agent {
        "claude" => 0xd97757,
        "codex" => 0x5b6bff,
        "gemini" => 0x3186ff,
        "kimi" => 0x1783ff,
        "openclaw" => 0xff4d4d,
        _ => return None,
    };
    Some(rgb(hex).into())
}
