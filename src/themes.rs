//! Built-in color themes, ported from Termany's (~/code/termany/apps/web/src/themes).
//! Colors stay as the CSS strings they were authored in; [`parse_color`]
//! reads `#rrggbb` and `rgba(r, g, b, a)`.

pub struct Palette {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    /// Main background.
    pub bg: &'static str,
    /// Raised surfaces: sidebar, tab strip.
    pub bg2: &'static str,
    /// Hover and active surfaces.
    pub bg3: &'static str,
    pub border: &'static str,
    pub fg: &'static str,
    pub fg_dim: &'static str,
    pub accent: &'static str,
    pub accent_soft: &'static str,
    /// The sidebar's own background, when it differs from `bg2`.
    pub sidebar: Option<&'static str>,
    /// The terminal's 16 ANSI colors, when the theme sets them.
    pub ansi: Option<[&'static str; 16]>,
    pub cursor: Option<&'static str>,
}

pub const THEMES: &[Palette] = &[
    Palette {
        id: "codex",
        name: "Codex",
        dark: false,
        bg: "#ffffff",
        bg2: "#f5f5f7",
        bg3: "#ececed",
        border: "#e5e5e7",
        fg: "#1a1a1a",
        fg_dim: "#5c5c62",
        accent: "#0a84ff",
        accent_soft: "rgba(10, 132, 255, 0.14)",
        sidebar: Some("#f5f5f7"),
        ansi: None,
        cursor: Some("#0a84ff"),
    },
    Palette {
        id: "default-dark",
        name: "Default Dark",
        dark: true,
        bg: "#12151b",
        bg2: "#181d24",
        bg3: "#1f2630",
        border: "#262e39",
        fg: "#c8cdd4",
        fg_dim: "#7d8794",
        accent: "#5bb8cc",
        accent_soft: "rgba(91, 184, 204, 0.16)",
        sidebar: Some("#0e1015"),
        ansi: None,
        cursor: Some("#5bb8cc"),
    },
    Palette {
        id: "solarized-dark",
        name: "Solarized Dark",
        dark: true,
        bg: "#002b36",
        bg2: "#073642",
        bg3: "#0a4250",
        border: "#0e4b59",
        fg: "#93a1a1",
        fg_dim: "#586e75",
        accent: "#268bd2",
        accent_soft: "rgba(38, 139, 210, 0.18)",
        sidebar: Some("#00222b"),
        ansi: None,
        cursor: Some("#268bd2"),
    },
    Palette {
        id: "daylight",
        name: "Daylight",
        dark: false,
        bg: "#ffffff",
        bg2: "#eef1f6",
        bg3: "#e2e7ef",
        border: "#e1e6ec",
        fg: "#2b2f36",
        fg_dim: "#9aa3af",
        accent: "#3b82f6",
        accent_soft: "rgba(59, 130, 246, 0.12)",
        sidebar: Some("#e2e7f0"),
        ansi: None,
        cursor: Some("#2b2f36"),
    },
    Palette {
        id: "meadow",
        name: "Meadow",
        dark: false,
        bg: "#ffffff",
        bg2: "#f2f4f2",
        bg3: "#e6ebe6",
        border: "#e2e7e2",
        fg: "#2f3a33",
        fg_dim: "#97a39b",
        accent: "#3f9d5a",
        accent_soft: "rgba(63, 157, 90, 0.14)",
        sidebar: Some("#e6ebe6"),
        ansi: None,
        cursor: Some("#3f9d5a"),
    },
    Palette {
        id: "charcoal",
        name: "Charcoal",
        dark: true,
        bg: "#2a2826",
        bg2: "#2a2826",
        bg3: "#3a3834",
        border: "#363430",
        fg: "#d8d4ca",
        fg_dim: "#8d887e",
        accent: "#d8a657",
        accent_soft: "rgba(216, 166, 87, 0.14)",
        sidebar: Some("#222120"),
        ansi: None,
        cursor: Some("#e8dcc0"),
    },
    Palette {
        id: "gruvbox",
        name: "Gruvbox Dark",
        dark: true,
        bg: "#282828",
        bg2: "#32302f",
        bg3: "#504945",
        border: "#504945",
        fg: "#ebdbb2",
        fg_dim: "#928374",
        accent: "#fe8019",
        accent_soft: "rgba(254, 128, 25, 0.15)",
        sidebar: Some("#1d2021"),
        ansi: Some(["#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984", "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2"]),
        cursor: Some("#ebdbb2"),
    },
    Palette {
        id: "gruvbox-light",
        name: "Gruvbox Light",
        dark: false,
        bg: "#fbf1c7",
        bg2: "#ebdbb2",
        bg3: "#d5c4a1",
        border: "#d5c4a1",
        fg: "#3c3836",
        fg_dim: "#7c6f64",
        accent: "#d65d0e",
        accent_soft: "rgba(214, 93, 14, 0.12)",
        sidebar: None,
        ansi: Some(["#fbf1c7", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#7c6f64", "#928374", "#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71", "#427b58", "#3c3836"]),
        cursor: Some("#3c3836"),
    },
    Palette {
        id: "rose-pine",
        name: "Rosé Pine",
        dark: true,
        bg: "#191724",
        bg2: "#1f1d2e",
        bg3: "#26233a",
        border: "#403d52",
        fg: "#e0def4",
        fg_dim: "#908caa",
        accent: "#ebbcba",
        accent_soft: "rgba(235, 188, 186, 0.14)",
        sidebar: None,
        ansi: Some(["#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4", "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4"]),
        cursor: Some("#524f67"),
    },
    Palette {
        id: "rose-pine-dawn",
        name: "Rosé Pine Dawn",
        dark: false,
        bg: "#faf4ed",
        bg2: "#fffaf3",
        bg3: "#f2e9e1",
        border: "#dfdad9",
        fg: "#575279",
        fg_dim: "#797593",
        accent: "#d7827e",
        accent_soft: "rgba(215, 130, 126, 0.12)",
        sidebar: None,
        ansi: Some(["#f2e9e1", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279", "#9893a5", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279"]),
        cursor: Some("#cecacd"),
    },
    Palette {
        id: "aurora",
        name: "Aurora",
        dark: true,
        bg: "#0d1022",
        bg2: "#171b3a",
        bg3: "#232952",
        border: "#2c3263",
        fg: "#dbe2ff",
        fg_dim: "#7d86b8",
        accent: "#37e6c3",
        accent_soft: "rgba(55, 230, 195, 0.16)",
        sidebar: Some("#12102b"),
        ansi: Some(["#232952", "#ff6b8f", "#3ddc97", "#ffd166", "#5aa2ff", "#b78cff", "#45e3dd", "#aeb6dd", "#3a4373", "#ff8fab", "#6ff0b4", "#ffe08a", "#82bbff", "#cfaaff", "#74f0ea", "#eef1ff"]),
        cursor: Some("#37e6c3"),
    },
    Palette {
        id: "horizon",
        name: "Horizon",
        dark: false,
        bg: "#fffaf4",
        bg2: "#ffe9d9",
        bg3: "#ffd9c2",
        border: "#f0cfb8",
        fg: "#46425e",
        fg_dim: "#8d8699",
        accent: "#f96f5d",
        accent_soft: "rgba(249, 111, 93, 0.14)",
        sidebar: Some("#ffdfc9"),
        ansi: Some(["#ece4da", "#d6455d", "#2f9e6e", "#c07f00", "#3a74d9", "#a75ac8", "#0f9ba8", "#6f6a85", "#a49d92", "#e4576f", "#37b980", "#d9950a", "#5b8ce4", "#bc74da", "#16b4c2", "#46425e"]),
        cursor: Some("#f96f5d"),
    },
    Palette {
        id: "phosphor",
        name: "Phosphor",
        dark: true,
        bg: "#010402",
        bg2: "#04120a",
        bg3: "#0a2416",
        border: "#0f3520",
        fg: "#3dff8c",
        fg_dim: "#1e9e5a",
        accent: "#3dff8c",
        accent_soft: "rgba(61, 255, 140, 0.18)",
        sidebar: None,
        ansi: Some(["#0a2416", "#ff6d5a", "#3dff8c", "#d8ff5e", "#4aa8ff", "#b591ff", "#35e0c8", "#b8ffd6", "#14663c", "#ff8a75", "#7dffb4", "#eaff9c", "#7cc0ff", "#cbb0ff", "#78f0dd", "#eafff2"]),
        cursor: Some("#3dff8c"),
    },
];

pub fn find(id: &str) -> Option<&'static Palette> {
    THEMES.iter().find(|t| t.id == id)
}

/// `#rrggbb`, `#rgb` or `rgba(r, g, b, a)` as (r, g, b, a) in 0..=1.
pub fn parse_color(text: &str) -> Option<(f32, f32, f32, f32)> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('#') {
        let hex = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 | 8 => hex.to_string(),
            _ => return None,
        };
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|b| b as f32 / 255.);
        let alpha = if hex.len() == 8 { byte(6)? } else { 1. };
        return Some((byte(0)?, byte(2)?, byte(4)?, alpha));
    }
    let inner = text.strip_prefix("rgba(").or_else(|| text.strip_prefix("rgb("))?.strip_suffix(')')?;
    let parts: Vec<f32> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    match parts[..] {
        [r, g, b] => Some((r / 255., g / 255., b / 255., 1.)),
        [r, g, b, a] => Some((r / 255., g / 255., b / 255., a)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_color_parses() {
        for t in THEMES {
            for color in [t.bg, t.bg2, t.bg3, t.border, t.fg, t.fg_dim, t.accent, t.accent_soft] {
                assert!(parse_color(color).is_some(), "{}: {color}", t.id);
            }
            for color in t.ansi.iter().flatten().chain(t.sidebar.iter()).chain(t.cursor.iter()) {
                assert!(parse_color(color).is_some(), "{}: {color}", t.id);
            }
        }
        assert_eq!(parse_color("rgba(215, 130, 126, 0.12)").unwrap().3, 0.12);
    }
}
