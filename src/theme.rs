//! Colour themes. One is active at a time (a thread-local, since all rendering
//! happens on the main thread); pick it in the app with `^t`, or set `"theme"`
//! in the settings file.
//!
//! Extra themes can be added straight in the settings file:
//!
//! ```json
//! "themes": [
//!   {
//!     "name": "My Theme",
//!     "accent": "#ff8800", "green": "#a6e3a1", "red": "#f38ba8",
//!     "yellow": "#f9e2af", "blue": "#89b4fa", "text": "#cdd6f4",
//!     "subtle": "#7f849c", "border": "#45475a", "sel_bg": "#313244",
//!     "bg": "#1e1e2e", "on_accent": "#1e1e2e"
//!   }
//! ]
//! ```
//!
//! `on_accent` (text drawn on a colour fill) defaults to `bg`; every other slot
//! is required. The name is slugified for the `"theme"` key (`"My Theme"` →
//! `"my-theme"`); a custom theme whose slug matches a built-in replaces it.

use std::cell::Cell;
use std::sync::OnceLock;

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

/// The colour slots the UI paints with. Every theme fills all of them.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Primary highlight (selected tab, section headings, cursor bar).
    pub accent: Color,
    pub green: Color,
    pub red: Color,
    pub yellow: Color,
    pub blue: Color,
    /// Body text.
    pub text: Color,
    /// De-emphasised text (hints, metadata).
    pub subtle: Color,
    /// Idle pane borders.
    pub border: Color,
    /// Selected-row background.
    pub sel_bg: Color,
    /// Window background.
    pub bg: Color,
    /// Text drawn on top of an accent/colour fill (the mode pill).
    pub on_accent: Color,
}

/// A named theme in the picker.
#[derive(Debug, Clone)]
pub struct ThemeEntry {
    /// Stable id used in the settings file.
    pub slug: String,
    /// Name shown in the picker.
    pub name: String,
    pub palette: Palette,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

const CATPPUCCIN_MOCHA: Palette = Palette {
    accent: rgb(0xcba6f7),
    green: rgb(0xa6e3a1),
    red: rgb(0xf38ba8),
    yellow: rgb(0xf9e2af),
    blue: rgb(0x89b4fa),
    text: rgb(0xcdd6f4),
    subtle: rgb(0x7f849c),
    border: rgb(0x45475a),
    sel_bg: rgb(0x313244),
    bg: rgb(0x1e1e2e),
    on_accent: rgb(0x1e1e2e),
};

const CATPPUCCIN_LATTE: Palette = Palette {
    accent: rgb(0x8839ef),
    green: rgb(0x40a02b),
    red: rgb(0xd20f39),
    yellow: rgb(0xdf8e1d),
    blue: rgb(0x1e66f5),
    text: rgb(0x4c4f69),
    subtle: rgb(0x8c8fa1),
    border: rgb(0xbcc0cc),
    sel_bg: rgb(0xccd0da),
    bg: rgb(0xeff1f5),
    on_accent: rgb(0xeff1f5),
};

const TOKYO_NIGHT: Palette = Palette {
    accent: rgb(0xbb9af7),
    green: rgb(0x9ece6a),
    red: rgb(0xf7768e),
    yellow: rgb(0xe0af68),
    blue: rgb(0x7aa2f7),
    text: rgb(0xc0caf5),
    subtle: rgb(0x565f89),
    border: rgb(0x414868),
    sel_bg: rgb(0x292e42),
    bg: rgb(0x1a1b26),
    on_accent: rgb(0x1a1b26),
};

const DRACULA: Palette = Palette {
    accent: rgb(0xbd93f9),
    green: rgb(0x50fa7b),
    red: rgb(0xff5555),
    yellow: rgb(0xf1fa8c),
    blue: rgb(0x8be9fd),
    text: rgb(0xf8f8f2),
    subtle: rgb(0x6272a4),
    border: rgb(0x44475a),
    sel_bg: rgb(0x44475a),
    bg: rgb(0x282a36),
    on_accent: rgb(0x282a36),
};

const NORD: Palette = Palette {
    accent: rgb(0x88c0d0),
    green: rgb(0xa3be8c),
    red: rgb(0xbf616a),
    yellow: rgb(0xebcb8b),
    blue: rgb(0x81a1c1),
    text: rgb(0xd8dee9),
    subtle: rgb(0x6c7a96),
    border: rgb(0x434c5e),
    sel_bg: rgb(0x3b4252),
    bg: rgb(0x2e3440),
    on_accent: rgb(0x2e3440),
};

const GRUVBOX_DARK: Palette = Palette {
    accent: rgb(0xfe8019),
    green: rgb(0xb8bb26),
    red: rgb(0xfb4934),
    yellow: rgb(0xfabd2f),
    blue: rgb(0x83a598),
    text: rgb(0xebdbb2),
    subtle: rgb(0x928374),
    border: rgb(0x504945),
    sel_bg: rgb(0x3c3836),
    bg: rgb(0x282828),
    on_accent: rgb(0x282828),
};

const GRUVBOX_LIGHT: Palette = Palette {
    accent: rgb(0xaf3a03),
    green: rgb(0x79740e),
    red: rgb(0x9d0006),
    yellow: rgb(0xb57614),
    blue: rgb(0x076678),
    text: rgb(0x3c3836),
    subtle: rgb(0x7c6f64),
    border: rgb(0xd5c4a1),
    sel_bg: rgb(0xebdbb2),
    bg: rgb(0xfbf1c7),
    on_accent: rgb(0xfbf1c7),
};

const ONE_DARK: Palette = Palette {
    accent: rgb(0xc678dd),
    green: rgb(0x98c379),
    red: rgb(0xe06c75),
    yellow: rgb(0xe5c07b),
    blue: rgb(0x61afef),
    text: rgb(0xabb2bf),
    subtle: rgb(0x5c6370),
    border: rgb(0x3e4451),
    sel_bg: rgb(0x2c313a),
    bg: rgb(0x282c34),
    on_accent: rgb(0x282c34),
};

const ROSE_PINE: Palette = Palette {
    accent: rgb(0xc4a7e7),
    green: rgb(0x9ccfd8),
    red: rgb(0xeb6f92),
    yellow: rgb(0xf6c177),
    blue: rgb(0x31748f),
    text: rgb(0xe0def4),
    subtle: rgb(0x908caa),
    border: rgb(0x26233a),
    sel_bg: rgb(0x1f1d2e),
    bg: rgb(0x191724),
    on_accent: rgb(0x191724),
};

const SOLARIZED_DARK: Palette = Palette {
    accent: rgb(0x268bd2),
    green: rgb(0x859900),
    red: rgb(0xdc322f),
    yellow: rgb(0xb58900),
    blue: rgb(0x268bd2),
    text: rgb(0x93a1a1),
    subtle: rgb(0x586e75),
    border: rgb(0x073642),
    sel_bg: rgb(0x073642),
    bg: rgb(0x002b36),
    on_accent: rgb(0x002b36),
};

const SOLARIZED_LIGHT: Palette = Palette {
    accent: rgb(0x268bd2),
    green: rgb(0x859900),
    red: rgb(0xdc322f),
    yellow: rgb(0xb58900),
    blue: rgb(0x268bd2),
    text: rgb(0x657b83),
    subtle: rgb(0x93a1a1),
    border: rgb(0xeee8d5),
    sel_bg: rgb(0xeee8d5),
    bg: rgb(0xfdf6e3),
    on_accent: rgb(0xfdf6e3),
};

const CATPPUCCIN_FRAPPE: Palette = Palette {
    accent: rgb(0xca9ee6),
    green: rgb(0xa6d189),
    red: rgb(0xe78284),
    yellow: rgb(0xe5c890),
    blue: rgb(0x8caaee),
    text: rgb(0xc6d0f5),
    subtle: rgb(0x838ba7),
    border: rgb(0x51576d),
    sel_bg: rgb(0x414559),
    bg: rgb(0x303446),
    on_accent: rgb(0x303446),
};

const CATPPUCCIN_MACCHIATO: Palette = Palette {
    accent: rgb(0xc6a0f6),
    green: rgb(0xa6da95),
    red: rgb(0xed8796),
    yellow: rgb(0xeed49f),
    blue: rgb(0x8aadf4),
    text: rgb(0xcad3f5),
    subtle: rgb(0x8087a2),
    border: rgb(0x494d64),
    sel_bg: rgb(0x363a4f),
    bg: rgb(0x24273a),
    on_accent: rgb(0x24273a),
};

const KANAGAWA: Palette = Palette {
    accent: rgb(0x957fb8),
    green: rgb(0x98bb6c),
    red: rgb(0xe46876),
    yellow: rgb(0xe6c384),
    blue: rgb(0x7fb4ca),
    text: rgb(0xdcd7ba),
    subtle: rgb(0x727169),
    border: rgb(0x54546d),
    sel_bg: rgb(0x223249),
    bg: rgb(0x1f1f28),
    on_accent: rgb(0x1f1f28),
};

const ROSE_PINE_MOON: Palette = Palette {
    accent: rgb(0xc4a7e7),
    green: rgb(0x9ccfd8),
    red: rgb(0xeb6f92),
    yellow: rgb(0xf6c177),
    blue: rgb(0x3e8fb0),
    text: rgb(0xe0def4),
    subtle: rgb(0x908caa),
    border: rgb(0x44415a),
    sel_bg: rgb(0x2a283e),
    bg: rgb(0x232136),
    on_accent: rgb(0x232136),
};

const ROSE_PINE_DAWN: Palette = Palette {
    accent: rgb(0x907aa9),
    green: rgb(0x56949f),
    red: rgb(0xb4637a),
    yellow: rgb(0xea9d34),
    blue: rgb(0x286983),
    text: rgb(0x575279),
    subtle: rgb(0x9893a5),
    border: rgb(0xdfdad9),
    sel_bg: rgb(0xf4ede8),
    bg: rgb(0xfaf4ed),
    on_accent: rgb(0xfaf4ed),
};

const GRUVBOX_MATERIAL: Palette = Palette {
    accent: rgb(0xe78a4e),
    green: rgb(0xa9b665),
    red: rgb(0xea6962),
    yellow: rgb(0xd8a657),
    blue: rgb(0x7daea3),
    text: rgb(0xd4be98),
    subtle: rgb(0x928374),
    border: rgb(0x45403d),
    sel_bg: rgb(0x3a3735),
    bg: rgb(0x282828),
    on_accent: rgb(0x282828),
};

const EVERFOREST_DARK: Palette = Palette {
    accent: rgb(0x83c092),
    green: rgb(0xa7c080),
    red: rgb(0xe67e80),
    yellow: rgb(0xdbbc7f),
    blue: rgb(0x7fbbb3),
    text: rgb(0xd3c6aa),
    subtle: rgb(0x859289),
    border: rgb(0x4f585e),
    sel_bg: rgb(0x3d484d),
    bg: rgb(0x2d353b),
    on_accent: rgb(0x2d353b),
};

const EVERFOREST_LIGHT: Palette = Palette {
    accent: rgb(0x35a77c),
    green: rgb(0x8da101),
    red: rgb(0xf85552),
    yellow: rgb(0xdfa000),
    blue: rgb(0x3a94c5),
    text: rgb(0x5c6a72),
    subtle: rgb(0x939f91),
    border: rgb(0xe0dcc7),
    sel_bg: rgb(0xefebd4),
    bg: rgb(0xfdf6e3),
    on_accent: rgb(0xfdf6e3),
};

const MONOKAI: Palette = Palette {
    accent: rgb(0xae81ff),
    green: rgb(0xa6e22e),
    red: rgb(0xf92672),
    yellow: rgb(0xe6db74),
    blue: rgb(0x66d9ef),
    text: rgb(0xf8f8f2),
    subtle: rgb(0x75715e),
    border: rgb(0x49483e),
    sel_bg: rgb(0x3e3d32),
    bg: rgb(0x272822),
    on_accent: rgb(0x272822),
};

const AYU_DARK: Palette = Palette {
    accent: rgb(0xff8f40),
    green: rgb(0xaad94c),
    red: rgb(0xf07178),
    yellow: rgb(0xe6b450),
    blue: rgb(0x59c2ff),
    text: rgb(0xbfbdb6),
    subtle: rgb(0x565b66),
    border: rgb(0x131721),
    sel_bg: rgb(0x1b2733),
    bg: rgb(0x0d1017),
    on_accent: rgb(0x0d1017),
};

const AYU_MIRAGE: Palette = Palette {
    accent: rgb(0xffad66),
    green: rgb(0xd5ff80),
    red: rgb(0xf28779),
    yellow: rgb(0xffd173),
    blue: rgb(0x73d0ff),
    text: rgb(0xcccac2),
    subtle: rgb(0x707a8c),
    border: rgb(0x171b24),
    sel_bg: rgb(0x2d3640),
    bg: rgb(0x1f2430),
    on_accent: rgb(0x1f2430),
};

const GITHUB_DARK: Palette = Palette {
    accent: rgb(0x58a6ff),
    green: rgb(0x3fb950),
    red: rgb(0xf85149),
    yellow: rgb(0xd29922),
    blue: rgb(0x58a6ff),
    text: rgb(0xc9d1d9),
    subtle: rgb(0x8b949e),
    border: rgb(0x30363d),
    sel_bg: rgb(0x21262d),
    bg: rgb(0x0d1117),
    on_accent: rgb(0x0d1117),
};

const GITHUB_LIGHT: Palette = Palette {
    accent: rgb(0x0969da),
    green: rgb(0x1a7f37),
    red: rgb(0xcf222e),
    yellow: rgb(0x9a6700),
    blue: rgb(0x0969da),
    text: rgb(0x1f2328),
    subtle: rgb(0x656d76),
    border: rgb(0xd0d7de),
    sel_bg: rgb(0xeaeef2),
    bg: rgb(0xffffff),
    on_accent: rgb(0xffffff),
};

const ZENBURN: Palette = Palette {
    accent: rgb(0xdfaf8f),
    green: rgb(0x7f9f7f),
    red: rgb(0xcc9393),
    yellow: rgb(0xf0dfaf),
    blue: rgb(0x8cd0d3),
    text: rgb(0xdcdccc),
    subtle: rgb(0x9f9f8f),
    border: rgb(0x5f5f5f),
    sel_bg: rgb(0x4f4f4f),
    bg: rgb(0x3f3f3f),
    on_accent: rgb(0x3f3f3f),
};

/// Built-ins, in picker order. The first is the default.
const BUILTINS: &[(&str, &str, Palette)] = &[
    ("catppuccin-mocha", "Catppuccin Mocha", CATPPUCCIN_MOCHA),
    ("catppuccin-macchiato", "Catppuccin Macchiato", CATPPUCCIN_MACCHIATO),
    ("catppuccin-frappe", "Catppuccin Frappé", CATPPUCCIN_FRAPPE),
    ("catppuccin-latte", "Catppuccin Latte", CATPPUCCIN_LATTE),
    ("tokyo-night", "Tokyo Night", TOKYO_NIGHT),
    ("kanagawa", "Kanagawa", KANAGAWA),
    ("dracula", "Dracula", DRACULA),
    ("nord", "Nord", NORD),
    ("rose-pine", "Rosé Pine", ROSE_PINE),
    ("rose-pine-moon", "Rosé Pine Moon", ROSE_PINE_MOON),
    ("rose-pine-dawn", "Rosé Pine Dawn", ROSE_PINE_DAWN),
    ("gruvbox-dark", "Gruvbox Dark", GRUVBOX_DARK),
    ("gruvbox-material", "Gruvbox Material", GRUVBOX_MATERIAL),
    ("gruvbox-light", "Gruvbox Light", GRUVBOX_LIGHT),
    ("everforest-dark", "Everforest Dark", EVERFOREST_DARK),
    ("everforest-light", "Everforest Light", EVERFOREST_LIGHT),
    ("one-dark", "One Dark", ONE_DARK),
    ("monokai", "Monokai", MONOKAI),
    ("ayu-dark", "Ayu Dark", AYU_DARK),
    ("ayu-mirage", "Ayu Mirage", AYU_MIRAGE),
    ("github-dark", "GitHub Dark", GITHUB_DARK),
    ("github-light", "GitHub Light", GITHUB_LIGHT),
    ("zenburn", "Zenburn", ZENBURN),
    ("solarized-dark", "Solarized Dark", SOLARIZED_DARK),
    ("solarized-light", "Solarized Light", SOLARIZED_LIGHT),
];

fn builtin_entries() -> Vec<ThemeEntry> {
    BUILTINS
        .iter()
        .map(|(slug, name, palette)| ThemeEntry {
            slug: (*slug).to_string(),
            name: (*name).to_string(),
            palette: *palette,
        })
        .collect()
}

// ---- custom themes from the settings file --------------------------------

/// A theme as written in `config.json` — hex strings, validated on load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeSpec {
    pub name: String,
    #[serde(default)]
    pub slug: Option<String>,
    pub accent: String,
    pub green: String,
    pub red: String,
    pub yellow: String,
    pub blue: String,
    pub text: String,
    pub subtle: String,
    pub border: String,
    pub sel_bg: String,
    pub bg: String,
    /// Optional — defaults to `bg`.
    #[serde(default)]
    pub on_accent: Option<String>,
}

impl ThemeSpec {
    /// Validate the hex strings and turn this into a usable theme.
    pub fn build(&self) -> Result<ThemeEntry, String> {
        let name = self.name.trim().to_string();
        if name.is_empty() {
            return Err("name is empty".into());
        }
        let hex = |slot: &str, v: &str| parse_hex(v).map_err(|e| format!("{slot} {e}"));
        let bg = hex("bg", &self.bg)?;
        let palette = Palette {
            accent: hex("accent", &self.accent)?,
            green: hex("green", &self.green)?,
            red: hex("red", &self.red)?,
            yellow: hex("yellow", &self.yellow)?,
            blue: hex("blue", &self.blue)?,
            text: hex("text", &self.text)?,
            subtle: hex("subtle", &self.subtle)?,
            border: hex("border", &self.border)?,
            sel_bg: hex("sel_bg", &self.sel_bg)?,
            bg,
            on_accent: match &self.on_accent {
                Some(v) => hex("on_accent", v)?,
                None => bg,
            },
        };
        let slug = self
            .slug
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| slugify(&name));
        Ok(ThemeEntry {
            slug,
            name,
            palette,
        })
    }
}

fn parse_hex(s: &str) -> Result<Color, String> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("\"{s}\" is not a #rrggbb colour"));
    }
    let n = u32::from_str_radix(h, 16).unwrap();
    Ok(rgb(n))
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    out
}

// ---- registry + active theme --------------------------------------------

static REGISTRY: OnceLock<Vec<ThemeEntry>> = OnceLock::new();

thread_local! {
    static CURRENT: Cell<usize> = const { Cell::new(0) };
}

/// Build the theme list: built-ins plus any custom themes (a custom theme whose
/// slug matches a built-in replaces it). Call once, before the first render.
pub fn init(custom: Vec<ThemeEntry>) {
    let mut list = builtin_entries();
    for entry in custom {
        match list.iter_mut().find(|e| e.slug == entry.slug) {
            Some(slot) => *slot = entry,
            None => list.push(entry),
        }
    }
    let _ = REGISTRY.set(list);
}

/// Every theme, in picker order.
pub fn registry() -> &'static [ThemeEntry] {
    REGISTRY.get_or_init(builtin_entries)
}

fn clamp(i: usize) -> usize {
    i.min(registry().len().saturating_sub(1))
}

/// The active theme.
pub fn current() -> &'static ThemeEntry {
    &registry()[clamp(CURRENT.with(Cell::get))]
}

/// Index of the active theme in [`registry`].
pub fn current_index() -> usize {
    clamp(CURRENT.with(Cell::get))
}

/// Switch to the theme at `idx` in [`registry`].
pub fn set_index(idx: usize) {
    CURRENT.with(|c| c.set(clamp(idx)));
}

/// Switch by slug (from the settings file); unknown slugs are ignored.
pub fn set_slug(slug: Option<&str>) {
    if let Some(slug) = slug
        && let Some(i) = registry().iter().position(|e| e.slug == slug)
    {
        set_index(i);
    }
}

fn palette() -> Palette {
    current().palette
}

// Slot accessors — shorthand for `palette().<slot>`, used all over `ui`/`md`.
pub fn accent() -> Color {
    palette().accent
}
pub fn green() -> Color {
    palette().green
}
pub fn red() -> Color {
    palette().red
}
pub fn yellow() -> Color {
    palette().yellow
}
pub fn blue() -> Color {
    palette().blue
}
pub fn text() -> Color {
    palette().text
}
pub fn subtle() -> Color {
    palette().subtle
}
pub fn border() -> Color {
    palette().border
}
pub fn sel_bg() -> Color {
    palette().sel_bg
}
pub fn bg() -> Color {
    palette().bg
}
pub fn on_accent() -> Color {
    palette().on_accent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_forms() {
        assert_eq!(parse_hex("#1e1e2e").unwrap(), Color::Rgb(30, 30, 46));
        assert_eq!(parse_hex("1E1E2E").unwrap(), Color::Rgb(30, 30, 46));
        assert!(parse_hex("#fff").is_err());
        assert!(parse_hex("nope").is_err());
    }

    #[test]
    fn slugifies_names() {
        assert_eq!(slugify("My Theme"), "my-theme");
        assert_eq!(slugify("  Rosé Pine!! "), "ros-pine");
    }

    #[test]
    fn spec_builds_and_defaults_on_accent() {
        let spec = ThemeSpec {
            name: "Test".into(),
            slug: None,
            accent: "#ffffff".into(),
            green: "#00ff00".into(),
            red: "#ff0000".into(),
            yellow: "#ffff00".into(),
            blue: "#0000ff".into(),
            text: "#cccccc".into(),
            subtle: "#888888".into(),
            border: "#444444".into(),
            sel_bg: "#222222".into(),
            bg: "#111111".into(),
            on_accent: None,
        };
        let entry = spec.build().unwrap();
        assert_eq!(entry.slug, "test");
        assert_eq!(entry.palette.on_accent, entry.palette.bg);
    }
}
