//! The interface's language. English is the source: every string in the code
//! is English and is its own key, so a missing translation shows the English.
//!
//! - `t("Settings")` gives the text in the current language.
//! - `tf("Generated {n} images", &[("n", n.to_string())])` fills `{name}`
//!   placeholders after translating, so each language orders its own words.
//!
//! The choice is "System" (follow macOS) or one language, saved as the
//! `language` setting. Dictionaries live in `dict_*.rs`, one per part of the
//! app, each with a table per language.

use std::{
    collections::HashMap,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

mod dict_agent;
mod dict_app;
mod dict_misc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    En,
    ZhCn,
    ZhTw,
    Ja,
    Ko,
}

impl Language {
    /// Every shipped language, in the order the picker lists them.
    pub const ALL: [Language; 5] = [Language::En, Language::ZhCn, Language::ZhTw, Language::Ja, Language::Ko];

    /// The BCP-47 tag saved in settings.
    pub fn tag(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::ZhCn => "zh-CN",
            Language::ZhTw => "zh-TW",
            Language::Ja => "ja",
            Language::Ko => "ko",
        }
    }

    /// The language's own name, so someone stuck in the wrong language can
    /// still find theirs.
    pub fn label(self) -> &'static str {
        match self {
            Language::En => "English",
            Language::ZhCn => "简体中文",
            Language::ZhTw => "繁體中文",
            Language::Ja => "日本語",
            Language::Ko => "한국어",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|l| *l == self).unwrap_or(0)
    }

    fn from_tag(tag: &str) -> Option<Language> {
        Self::ALL.iter().copied().find(|l| l.tag().eq_ignore_ascii_case(tag))
    }

    /// The shipped language closest to any BCP-47 tag: an exact match, then
    /// the base language. Chinese goes by script, not region, so Hong Kong,
    /// Macau and `Hant` read Traditional.
    pub fn matching(tag: &str) -> Option<Language> {
        let lower = tag.trim().replace('_', "-").to_ascii_lowercase();
        if lower.is_empty() {
            return None;
        }
        if lower.starts_with("zh") {
            let traditional = lower.contains("hant") || lower.contains("-tw") || lower.contains("-hk") || lower.contains("-mo");
            return Some(if traditional { Language::ZhTw } else { Language::ZhCn });
        }
        if let Some(exact) = Self::from_tag(&lower) {
            return Some(exact);
        }
        let base = lower.split('-').next().unwrap_or_default();
        Self::ALL.iter().copied().find(|l| l.tag().split('-').next() == Some(base))
    }

    fn table(self) -> &'static HashMap<&'static str, &'static str> {
        static TABLES: OnceLock<Vec<HashMap<&'static str, &'static str>>> = OnceLock::new();
        let tables = TABLES.get_or_init(|| {
            Language::ALL
                .iter()
                .map(|language| {
                    let parts: [&[(&str, &str)]; 3] = match language {
                        Language::En => [&[], &[], &[]],
                        Language::ZhCn => [dict_app::ZH_CN, dict_agent::ZH_CN, dict_misc::ZH_CN],
                        Language::ZhTw => [dict_app::ZH_TW, dict_agent::ZH_TW, dict_misc::ZH_TW],
                        Language::Ja => [dict_app::JA, dict_agent::JA, dict_misc::JA],
                        Language::Ko => [dict_app::KO, dict_agent::KO, dict_misc::KO],
                    };
                    parts.into_iter().flatten().copied().collect()
                })
                .collect()
        });
        &tables[self.index()]
    }
}

/// The language in use, as an index into `Language::ALL`.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

pub fn current() -> Language {
    Language::ALL[CURRENT.load(Ordering::Relaxed).min(Language::ALL.len() - 1)]
}

/// Use `preference` (`None` follows the system) from now on. Callers then
/// rebuild the menus and redraw the windows.
pub fn apply(preference: Option<Language>) {
    let language = preference.unwrap_or_else(system_language);
    CURRENT.store(language.index(), Ordering::Relaxed);
}

/// The saved preference: `None` is "System".
pub fn preference_from_setting(value: Option<String>) -> Option<Language> {
    value.as_deref().and_then(Language::from_tag)
}

pub fn preference_to_setting(preference: Option<Language>) -> Option<String> {
    preference.map(|l| l.tag().to_string())
}

/// The first of the user's preferred languages that ships, else English.
pub fn system_language() -> Language {
    static SYSTEM: OnceLock<Language> = OnceLock::new();
    *SYSTEM.get_or_init(|| {
        system_tags().iter().find_map(|tag| Language::matching(tag)).unwrap_or(Language::En)
    })
}

/// The user's languages, most preferred first: macOS's list, then `LANG`.
fn system_tags() -> Vec<String> {
    let mut tags = Vec::new();
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("defaults").args(["read", "-g", "AppleLanguages"]).output() {
        // A plist array: `(\n    "zh-Hans-CN",\n    "en-CN"\n)`.
        tags.extend(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().trim_matches(|c| c == '"' || c == ',' || c == '(' || c == ')').to_string())
                .filter(|l| !l.is_empty()),
        );
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(var) {
            let tag = value.split('.').next().unwrap_or_default().to_string();
            if !tag.is_empty() && tag != "C" && tag != "POSIX" {
                tags.push(tag);
            }
        }
    }
    tags
}

/// `text` (English) in the current language.
pub fn t(text: &'static str) -> &'static str {
    match current() {
        Language::En => text,
        language => language.table().get(text).copied().unwrap_or(text),
    }
}

/// `text` translated, then with each `{name}` replaced by its value.
pub fn tf(text: &'static str, args: &[(&str, String)]) -> String {
    let mut out = t(text).to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_system_tags() {
        assert_eq!(Language::matching("zh-Hans-CN"), Some(Language::ZhCn));
        assert_eq!(Language::matching("zh-Hant-TW"), Some(Language::ZhTw));
        assert_eq!(Language::matching("zh-HK"), Some(Language::ZhTw));
        assert_eq!(Language::matching("en-CN"), Some(Language::En));
        assert_eq!(Language::matching("ja_JP"), Some(Language::Ja));
        assert_eq!(Language::matching("fr-CA"), None);
    }

    /// Without touching the app-wide language, which other tests read.
    #[test]
    fn falls_back_to_english_and_fills_placeholders() {
        assert!(Language::ZhCn.table().get("A string nobody translated").is_none());
        assert_eq!(tf("A string nobody translated {n}", &[("n", "3".into())]), "A string nobody translated 3");
        assert_eq!(Language::ZhCn.table().get("Settings…").copied().map(|s| !s.is_empty()), Some(true));
    }

    /// Every translation keeps the source's placeholders, so `tf` can fill them.
    #[test]
    fn translations_keep_placeholders() {
        let placeholders = |s: &str| {
            let mut found: Vec<String> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(n, _)| n.to_string())).collect();
            found.sort();
            found
        };
        for language in Language::ALL {
            for (source, translated) in language.table() {
                assert_eq!(placeholders(source), placeholders(translated), "{:?}: {source:?} → {translated:?}", language);
            }
        }
    }
}
