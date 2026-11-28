//! Agent logos, embedded in the binary (from douchat's icon set).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

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

/// An agent's logo: ours, else the one agl ships for it (agl 0.3.19 has a
/// mark for every built-in agent, so new agents get theirs with an update).
fn logo(agent: &str) -> Option<Arc<Image>> {
    static IMAGES: OnceLock<HashMap<&'static str, Arc<Image>>> = OnceLock::new();
    static AGL: OnceLock<Mutex<HashMap<String, Option<Arc<Image>>>>> = OnceLock::new();
    let ours = IMAGES.get_or_init(|| {
        LOGOS
            .iter()
            .map(|(id, bytes, format)| (*id, Arc::new(Image::from_bytes(*format, bytes.to_vec()))))
            .collect()
    });
    if let Some(image) = ours.get(agent) {
        return Some(image.clone());
    }
    AGL.get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(agent.to_string())
        .or_insert_with(|| {
            agl::agents::icons::builtin(agent)
                .map(|svg| Arc::new(Image::from_bytes(ImageFormat::Svg, svg.as_bytes().to_vec())))
        })
        .clone()
}

/// Where icons picked in Settings → Agents are kept.
fn custom_dir() -> PathBuf {
    crate::paths::home().join("agent-icons")
}

static CUSTOM: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();

/// The icon picked for an agent, if any: `<id>-<stamp>.<ext>`, a new name
/// each time, so a replaced picture is never served from the image cache.
pub fn custom_icon(agent: &str) -> Option<PathBuf> {
    let cache = CUSTOM.get_or_init(Default::default);
    if let Some(found) = cache.lock().unwrap().get(agent) {
        return found.clone();
    }
    let prefix = format!("{agent}-");
    let found = std::fs::read_dir(custom_dir()).ok().and_then(|entries| {
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                name.strip_prefix(&prefix).is_some_and(|rest| rest.split('.').next().is_some_and(|s| s.chars().all(|c| c.is_ascii_digit())))
            })
            .max()
    });
    cache.lock().unwrap().insert(agent.to_string(), found.clone());
    found
}

/// Use the picture at `source` as the agent's icon, or (`None`) its own again.
pub fn set_custom_icon(agent: &str, source: Option<&Path>) -> std::io::Result<()> {
    if let Some(old) = custom_icon(agent) {
        let _ = std::fs::remove_file(old);
    }
    let mut saved = None;
    if let Some(source) = source {
        let ext = source.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "png".into());
        std::fs::create_dir_all(custom_dir())?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let path = custom_dir().join(format!("{agent}-{stamp}.{ext}"));
        std::fs::copy(source, &path)?;
        saved = Some(path);
    }
    CUSTOM.get_or_init(Default::default).lock().unwrap().insert(agent.to_string(), saved);
    Ok(())
}

/// An agent's logo at `size`: the one picked in Settings, its own, or a
/// generic bot for agents without one.
pub fn agent_icon(agent: &str, size: Pixels, cx: &App) -> AnyElement {
    if let Some(path) = custom_icon(agent) {
        return div()
            .size(size)
            .flex_none()
            .rounded(size * 0.22)
            .overflow_hidden()
            .child(img(path).size_full().object_fit(ObjectFit::Cover))
            .into_any_element();
    }
    builtin_icon(agent, size, cx)
}

/// An agent's own logo, ignoring any picked in Settings.
pub fn builtin_icon(agent: &str, size: Pixels, cx: &App) -> AnyElement {
    let agent = agent.split_once("-profile-").map_or(agent, |(base, _)| base);
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
