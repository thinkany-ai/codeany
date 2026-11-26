//! The app's shortcuts, each of which can be rebound in Settings → Shortcuts.
//!
//! Every shortcut has default keystrokes (in GPUI syntax, e.g. `cmd-shift-t`).
//! A rebound one has a single keystroke instead, saved in the database under
//! `keybindings` (shortcut id → keystroke). Applying rebuilds the keymap from
//! the bindings that were there before (components, terminal, model menu)
//! plus these, so a changed default stops working at once.

use std::{collections::HashMap, rc::Rc};

use gpui_kit::*;

use crate::{db::Db, paths};

const SETTING: &str = "keybindings";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Tabs,
    Workspace,
    General,
    Terminal,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::Tabs, Group::Workspace, Group::General, Group::Terminal];

    pub fn label(self) -> &'static str {
        match self {
            Group::Tabs => "Tabs",
            Group::Workspace => "Workspace",
            Group::General => "General",
            Group::Terminal => "Terminal",
        }
    }
}

pub struct Shortcut {
    pub id: &'static str,
    pub label: &'static str,
    pub group: Group,
    context: Option<&'static str>,
    defaults: &'static [&'static str],
    action: fn() -> Box<dyn Action>,
}

macro_rules! shortcut {
    ($id:literal, $label:literal, $group:ident, $context:expr, [$($key:literal),*], $action:expr) => {
        Shortcut {
            id: $id,
            label: $label,
            group: Group::$group,
            context: $context,
            defaults: &[$($key),*],
            action: || Box::new($action) as Box<dyn Action>,
        }
    };
}

const WS: Option<&str> = Some("Workspace");
const TERM: Option<&str> = Some("Terminal");

pub static SHORTCUTS: &[Shortcut] = &[
    shortcut!("new-tab", "New tab", Tabs, WS, ["cmd-t"], crate::NewTab),
    shortcut!("close-tab", "Close tab", Tabs, WS, ["cmd-w"], crate::CloseTab),
    shortcut!("reopen-tab", "Reopen closed tab", Tabs, WS, ["cmd-shift-t"], crate::ReopenTab),
    shortcut!("next-tab", "Next tab", Tabs, WS, ["ctrl-tab", "cmd-shift-]", "cmd-alt-right"], crate::NextTab),
    shortcut!("prev-tab", "Previous tab", Tabs, WS, ["ctrl-shift-tab", "cmd-shift-[", "cmd-alt-left"], crate::PrevTab),
    shortcut!("tab-1", "Go to tab 1", Tabs, WS, ["cmd-1"], crate::SelectTab1),
    shortcut!("tab-2", "Go to tab 2", Tabs, WS, ["cmd-2"], crate::SelectTab2),
    shortcut!("tab-3", "Go to tab 3", Tabs, WS, ["cmd-3"], crate::SelectTab3),
    shortcut!("tab-4", "Go to tab 4", Tabs, WS, ["cmd-4"], crate::SelectTab4),
    shortcut!("tab-5", "Go to tab 5", Tabs, WS, ["cmd-5"], crate::SelectTab5),
    shortcut!("tab-6", "Go to tab 6", Tabs, WS, ["cmd-6"], crate::SelectTab6),
    shortcut!("tab-7", "Go to tab 7", Tabs, WS, ["cmd-7"], crate::SelectTab7),
    shortcut!("tab-8", "Go to tab 8", Tabs, WS, ["cmd-8"], crate::SelectTab8),
    shortcut!("tab-last", "Go to last tab", Tabs, WS, ["cmd-9"], crate::SelectLastTab),
    shortcut!("new-thread", "New thread", Workspace, WS, ["cmd-n"], crate::NewThread),
    shortcut!("search", "Search", Workspace, WS, ["cmd-p"], crate::Search),
    shortcut!("open-editor", "Switch Agent / Editor", Workspace, WS, ["cmd-e"], crate::OpenEditor),
    shortcut!("surfaces", "Surfaces", Workspace, WS, ["cmd-alt-t"], crate::OpenSurface),
    shortcut!("toggle-sidebar", "Toggle sidebar", Workspace, WS, ["cmd-b"], crate::ToggleSidebar),
    shortcut!("toggle-terminal", "Toggle terminal", Workspace, WS, ["cmd-j", "ctrl-`"], crate::ToggleTerminal),
    shortcut!("toggle-git", "Toggle git changes", Workspace, WS, ["cmd-shift-g"], crate::ToggleGitPanel),
    shortcut!("open-folder", "Open folder", General, WS, ["cmd-o"], crate::OpenFolder),
    shortcut!("save", "Save file", General, WS, ["cmd-s"], crate::Save),
    shortcut!("settings", "Settings", General, WS, ["cmd-,"], crate::OpenSettings),
    shortcut!("quit", "Quit CodeAny", General, None, ["cmd-q"], crate::Quit),
    shortcut!("terminal-copy", "Copy", Terminal, TERM, ["cmd-c"], crate::terminal::Copy),
    shortcut!("terminal-paste", "Paste", Terminal, TERM, ["cmd-v"], crate::terminal::Paste),
    shortcut!("terminal-clear", "Clear", Terminal, TERM, ["cmd-k"], crate::terminal::Clear),
];

struct Keymap {
    /// Every binding registered before this module's, kept across rebuilds.
    base: Vec<KeyBinding>,
    overrides: HashMap<String, String>,
}

impl Global for Keymap {}

/// Bind the shortcuts. Call after everything else has bound its keys.
pub fn init(cx: &mut App) {
    let base = cx.key_bindings().borrow().bindings().cloned().collect();
    let mut overrides: HashMap<String, String> =
        Db::open(&paths::database()).ok().and_then(|db| db.setting(SETTING)).unwrap_or_default();
    overrides.retain(|id, keys| SHORTCUTS.iter().any(|s| s.id == id) && parse(keys).is_some());
    cx.set_global(Keymap { base, overrides });
    apply(cx);
}

fn apply(cx: &mut App) {
    let mut bindings = cx.global::<Keymap>().base.clone();
    for shortcut in SHORTCUTS {
        let context = shortcut
            .context
            .and_then(|c| KeyBindingContextPredicate::parse(c).ok())
            .map(Rc::new);
        for keys in keys(shortcut.id, cx) {
            match KeyBinding::load(&keys, (shortcut.action)(), context.clone(), false, None, cx) {
                Ok(binding) => bindings.push(binding),
                Err(err) => eprintln!("shortcut {}: {keys}: {err}", shortcut.id),
            }
        }
    }
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

/// The keystrokes a shortcut is bound to now.
pub fn keys(id: &str, cx: &App) -> Vec<String> {
    if let Some(keys) = cx.global::<Keymap>().overrides.get(id) {
        return vec![keys.clone()];
    }
    SHORTCUTS
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.defaults.iter().map(|k| for_platform(id, k)).collect())
        .unwrap_or_default()
}

/// Defaults are written for macOS. Elsewhere Command becomes Control, and
/// the terminal's copy, paste and clear add Shift, as Ctrl-C and Ctrl-V
/// belong to the shell there.
fn for_platform(id: &str, keys: &str) -> String {
    if cfg!(target_os = "macos") {
        return keys.to_string();
    }
    let keys = keys.replace("cmd-", "ctrl-");
    if id.starts_with("terminal-") {
        return keys.replace("ctrl-", "ctrl-shift-");
    }
    keys
}

pub fn is_custom(id: &str, cx: &App) -> bool {
    cx.global::<Keymap>().overrides.contains_key(id)
}

pub fn any_custom(cx: &App) -> bool {
    !cx.global::<Keymap>().overrides.is_empty()
}

/// Bind a shortcut to `keys`, or back to its defaults with None.
pub fn set(id: &str, keys: Option<String>, cx: &mut App) {
    let defaults = SHORTCUTS.iter().find(|s| s.id == id).map(|s| s.defaults);
    let keymap = cx.global_mut::<Keymap>();
    match keys {
        Some(keys) if defaults != Some(&[keys.as_str()][..]) => {
            keymap.overrides.insert(id.to_string(), keys);
        }
        _ => {
            keymap.overrides.remove(id);
        }
    }
    save_and_apply(cx);
}

pub fn reset_all(cx: &mut App) {
    cx.global_mut::<Keymap>().overrides.clear();
    save_and_apply(cx);
}

fn save_and_apply(cx: &mut App) {
    if let Ok(db) = Db::open(&paths::database()) {
        db.set_setting(SETTING, &cx.global::<Keymap>().overrides);
    }
    apply(cx);
    // The menus show the shortcuts too.
    crate::set_app_menus(cx);
}

/// For each shortcut sharing a keystroke with another in the same context,
/// the label of that other one.
pub fn conflicts(cx: &App) -> HashMap<&'static str, &'static str> {
    let mut owners: HashMap<(Option<&str>, String), Vec<&Shortcut>> = HashMap::new();
    for shortcut in SHORTCUTS {
        for keys in keys(shortcut.id, cx) {
            owners.entry((shortcut.context, keys)).or_default().push(shortcut);
        }
    }
    let mut out = HashMap::new();
    for list in owners.values().filter(|l| l.len() > 1) {
        for s in list {
            if let Some(other) = list.iter().find(|o| o.id != s.id) {
                out.insert(s.id, other.label);
            }
        }
    }
    out
}

/// The keystroke a recorder keeps, in binding syntax; None while only
/// modifiers are down, or for a plain key that typing needs.
pub fn from_keystroke(keystroke: &Keystroke) -> Option<String> {
    let key = keystroke.key.as_str();
    if key.is_empty() || matches!(key, "shift" | "control" | "alt" | "platform" | "cmd" | "function" | "fn") {
        return None;
    }
    let m = &keystroke.modifiers;
    let function_key = key.len() > 1 && key.starts_with('f') && key[1..].parse::<u8>().is_ok();
    if !(m.platform || m.control || m.alt || function_key) {
        return None;
    }
    let mut out = String::new();
    for (on, name) in [(m.control, "ctrl-"), (m.alt, "alt-"), (m.shift, "shift-"), (m.platform, "cmd-")] {
        if on {
            out.push_str(name);
        }
    }
    out.push_str(key);
    parse(&out).is_some().then_some(out)
}

/// Modifiers and key of a binding-syntax keystroke.
fn parse(keys: &str) -> Option<(Vec<&'static str>, &str)> {
    let mut rest = keys;
    let mut mods = Vec::new();
    loop {
        let Some((head, tail)) = rest.split_once('-') else { break };
        if tail.is_empty() {
            break; // `cmd--`: the key is "-".
        }
        let symbol = match head {
            "ctrl" | "control" => "⌃",
            "alt" | "option" => "⌥",
            "shift" => "⇧",
            "cmd" | "super" | "platform" => "⌘",
            "fn" => "fn",
            _ => return None,
        };
        mods.push(symbol);
        rest = tail;
    }
    (!rest.is_empty()).then_some((mods, rest))
}

/// `cmd-shift-t` as `⌘⇧T`.
pub fn display(keys: &str) -> String {
    let Some((mut mods, key)) = parse(keys) else { return keys.to_string() };
    let order = ["fn", "⌃", "⌥", "⇧", "⌘"];
    mods.sort_by_key(|m| order.iter().position(|o| o == m));
    if !cfg!(target_os = "macos") {
        // "Ctrl+Shift+T", the way Linux and Windows write it.
        let names: Vec<&str> = mods
            .iter()
            .map(|m| match *m {
                "⌃" => "Ctrl",
                "⌥" => "Alt",
                "⇧" => "Shift",
                "⌘" => "Super",
                other => other,
            })
            .chain(std::iter::once(key))
            .collect();
        return names
            .iter()
            .map(|n| if n.len() == 1 { n.to_uppercase() } else { capitalize(n) })
            .collect::<Vec<_>>()
            .join("+");
    }
    let key = match key {
        "tab" => "Tab".to_string(),
        "enter" => "↩".to_string(),
        "escape" => "Esc".to_string(),
        "space" => "Space".to_string(),
        "backspace" => "⌫".to_string(),
        "delete" => "⌦".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" => "PgUp".to_string(),
        "pagedown" => "PgDn".to_string(),
        other => other.to_uppercase(),
    };
    format!("{}{key}", mods.concat())
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{display, parse, SHORTCUTS};

    #[cfg(target_os = "macos")]
    #[test]
    fn displays_keystrokes() {
        assert_eq!(display("cmd-shift-t"), "⇧⌘T");
        assert_eq!(display("ctrl-`"), "⌃`");
        assert_eq!(display("cmd--"), "⌘-");
        assert_eq!(display("cmd-alt-right"), "⌥⌘→");
        assert!(parse("hyper-x").is_none());
    }

    #[test]
    fn ids_are_unique() {
        for (i, a) in SHORTCUTS.iter().enumerate() {
            assert!(SHORTCUTS[i + 1..].iter().all(|b| b.id != a.id), "{}", a.id);
        }
    }
}
