//! Color themes: built-in palettes, user themes from `<config dir>/themes/*.toml`,
//! and downgrading to the 256-color palette for terminals without truecolor.
use std::path::Path;

use ratatui::style::{Color, Modifier, Style};

pub const DEFAULT_THEME: &str = "tokyo-night";

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Stable key stored in the config (`tokyo-night`).
    pub id: String,
    /// Display name (`Tokyo Night`).
    pub name: String,
    pub light: bool,
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub accent2: Color,
    pub border: Color,
    pub sel_bg: Color,
    /// Text drawn on top of an accent-colored bar.
    pub bar_fg: Color,
    pub green: Color,
    pub red: Color,
    pub yellow: Color,
    pub cyan: Color,
}

fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

struct Palette {
    id: &'static str,
    name: &'static str,
    light: bool,
    // bg, fg, dim, accent, accent2, border, sel_bg, bar_fg, green, red, yellow, cyan
    c: [u32; 12],
}

const PALETTES: &[Palette] = &[
    Palette {
        id: "tokyo-night",
        name: "Tokyo Night",
        light: false,
        c: [
            0x1a1b26, 0xc0caf5, 0x565f89, 0x7aa2f7, 0xbb9af7, 0x3b4261, 0x292e42, 0x1a1b26,
            0x9ece6a, 0xf7768e, 0xe0af68, 0x7dcfff,
        ],
    },
    Palette {
        id: "dracula",
        name: "Dracula",
        light: false,
        c: [
            0x282a36, 0xf8f8f2, 0x6272a4, 0xbd93f9, 0xff79c6, 0x44475a, 0x44475a, 0x282a36,
            0x50fa7b, 0xff5555, 0xf1fa8c, 0x8be9fd,
        ],
    },
    Palette {
        id: "nord",
        name: "Nord",
        light: false,
        c: [
            0x2e3440, 0xd8dee9, 0x616e88, 0x88c0d0, 0x81a1c1, 0x4c566a, 0x3b4252, 0x2e3440,
            0xa3be8c, 0xbf616a, 0xebcb8b, 0x8fbcbb,
        ],
    },
    Palette {
        id: "gruvbox-dark",
        name: "Gruvbox Dark",
        light: false,
        c: [
            0x282828, 0xebdbb2, 0x928374, 0xfabd2f, 0xfe8019, 0x665c54, 0x3c3836, 0x282828,
            0xb8bb26, 0xfb4934, 0xfabd2f, 0x8ec07c,
        ],
    },
    Palette {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        light: false,
        c: [
            0x1e1e2e, 0xcdd6f4, 0x6c7086, 0x89b4fa, 0xcba6f7, 0x45475a, 0x313244, 0x1e1e2e,
            0xa6e3a1, 0xf38ba8, 0xf9e2af, 0x94e2d5,
        ],
    },
    Palette {
        id: "one-dark",
        name: "One Dark",
        light: false,
        c: [
            0x282c34, 0xabb2bf, 0x5c6370, 0x61afef, 0xc678dd, 0x4b5263, 0x2c313c, 0x282c34,
            0x98c379, 0xe06c75, 0xe5c07b, 0x56b6c2,
        ],
    },
    Palette {
        id: "solarized-dark",
        name: "Solarized Dark",
        light: false,
        c: [
            0x002b36, 0x93a1a1, 0x586e75, 0x268bd2, 0x2aa198, 0x586e75, 0x073642, 0x002b36,
            0x859900, 0xdc322f, 0xb58900, 0x2aa198,
        ],
    },
    Palette {
        id: "solarized-light",
        name: "Solarized Light",
        light: true,
        c: [
            0xfdf6e3, 0x586e75, 0x93a1a1, 0x268bd2, 0x6c71c4, 0x93a1a1, 0xeee8d5, 0xfdf6e3,
            0x859900, 0xdc322f, 0xb58900, 0x2aa198,
        ],
    },
    Palette {
        id: "catppuccin-latte",
        name: "Catppuccin Latte",
        light: true,
        c: [
            0xeff1f5, 0x4c4f69, 0x8c8fa1, 0x1e66f5, 0x8839ef, 0xacb0be, 0xdce0e8, 0xeff1f5,
            0x40a02b, 0xd20f39, 0xdf8e1d, 0x179299,
        ],
    },
    Palette {
        id: "gruvbox-light",
        name: "Gruvbox Light",
        light: true,
        c: [
            0xfbf1c7, 0x3c3836, 0x928374, 0xb57614, 0xaf3a03, 0xbdae93, 0xebdbb2, 0xfbf1c7,
            0x79740e, 0x9d0006, 0xb57614, 0x427b58,
        ],
    },
];

impl Palette {
    fn theme(&self) -> Theme {
        let [
            bg,
            fg,
            dim,
            accent,
            accent2,
            border,
            sel_bg,
            bar_fg,
            green,
            red,
            yellow,
            cyan,
        ] = self.c.map(rgb);
        Theme {
            id: self.id.into(),
            name: self.name.into(),
            light: self.light,
            bg,
            fg,
            dim,
            accent,
            accent2,
            border,
            sel_bg,
            bar_fg,
            green,
            red,
            yellow,
            cyan,
        }
    }
}

impl Theme {
    /// Uses the terminal's own background and 16 ANSI colors, so it follows the
    /// user's terminal theme.
    pub fn terminal() -> Self {
        Self {
            id: "terminal".into(),
            name: "Terminal colors".into(),
            light: false,
            bg: Color::Reset,
            fg: Color::Reset,
            dim: Color::DarkGray,
            accent: Color::Cyan,
            accent2: Color::Magenta,
            border: Color::DarkGray,
            sel_bg: Color::DarkGray,
            bar_fg: Color::Black,
            green: Color::Green,
            red: Color::Red,
            yellow: Color::Yellow,
            cyan: Color::Cyan,
        }
    }

    /// All built-in themes, default first.
    pub fn builtin() -> Vec<Theme> {
        let mut v: Vec<Theme> = PALETTES.iter().map(Palette::theme).collect();
        v.push(Theme::terminal());
        v
    }

    fn colors_mut(&mut self) -> [(&'static str, &mut Color); 12] {
        [
            ("bg", &mut self.bg),
            ("fg", &mut self.fg),
            ("dim", &mut self.dim),
            ("accent", &mut self.accent),
            ("accent2", &mut self.accent2),
            ("border", &mut self.border),
            ("sel_bg", &mut self.sel_bg),
            ("bar_fg", &mut self.bar_fg),
            ("green", &mut self.green),
            ("red", &mut self.red),
            ("yellow", &mut self.yellow),
            ("cyan", &mut self.cyan),
        ]
    }

    /// Parse a user theme. `base` (an existing theme id) supplies any missing colors;
    /// without it the first built-in theme is the base. Unknown keys are ignored and
    /// invalid colors are reported.
    pub fn from_toml(raw: &str, fallback_id: &str, builtin: &[Theme]) -> Result<Theme, String> {
        let table: toml::Table = raw.parse().map_err(|e| format!("invalid TOML: {e}"))?;
        let base_id = table
            .get("base")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_THEME);
        let mut theme = builtin
            .iter()
            .find(|t| t.id == base_id)
            .or(builtin.first())
            .cloned()
            .ok_or("no base theme")?;
        let id = slug(
            table
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(fallback_id),
        );
        if id.is_empty() {
            return Err("theme needs a non-empty id or file name".into());
        }
        theme.name = table
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.clone());
        theme.id = id;
        if let Some(light) = table.get("light").and_then(|v| v.as_bool()) {
            theme.light = light;
        }
        for (key, slot) in theme.colors_mut() {
            if let Some(v) = table.get(key) {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("{key}: expected a \"#rrggbb\" string"))?;
                *slot = parse_color(s).ok_or_else(|| format!("{key}: invalid color {s:?}"))?;
            }
        }
        Ok(theme)
    }

    /// Convert every color to the nearest 256-color palette entry.
    pub fn to_256(mut self) -> Self {
        for (_, c) in self.colors_mut() {
            *c = nearest_256(*c);
        }
        self
    }

    // ---- styles -----------------------------------------------------------------

    pub fn base(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg)
    }
    pub fn dim_style(&self) -> Style {
        Style::default().fg(self.dim).bg(self.bg)
    }
    pub fn accent_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .bg(self.bg)
            .add_modifier(Modifier::BOLD)
    }
    pub fn border_style(&self, focused: bool) -> Style {
        if focused {
            Style::default()
                .fg(self.accent)
                .bg(self.bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.border).bg(self.bg)
        }
    }
    pub fn title_style(&self, focused: bool) -> Style {
        Style::default()
            .fg(if focused { self.accent } else { self.fg })
            .bg(self.bg)
            .add_modifier(Modifier::BOLD)
    }
    pub fn selected(&self) -> Style {
        Style::default()
            .fg(self.fg)
            .bg(self.sel_bg)
            .add_modifier(Modifier::BOLD)
    }
    pub fn bar(&self) -> Style {
        Style::default()
            .fg(self.bar_fg)
            .bg(self.accent)
            .add_modifier(Modifier::BOLD)
    }
    /// Subtle strip used for the status and hint rows.
    pub fn strip(&self) -> Style {
        Style::default().fg(self.dim).bg(self.sel_bg)
    }
    pub fn key_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .bg(self.sel_bg)
            .add_modifier(Modifier::BOLD)
    }
    pub fn fg_on_strip(&self) -> Style {
        Style::default().fg(self.fg).bg(self.sel_bg)
    }
    pub fn color(&self, c: Color) -> Style {
        Style::default().fg(c).bg(self.bg)
    }
    pub fn bold_color(&self, c: Color) -> Style {
        self.color(c).add_modifier(Modifier::BOLD)
    }
    pub fn error_style(&self) -> Style {
        self.color(self.red)
    }
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

pub fn parse_color(s: &str) -> Option<Color> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(rgb)
}

/// Nearest xterm-256 color (6x6x6 cube or the gray ramp) by squared RGB distance.
pub fn nearest_256(c: Color) -> Color {
    let Color::Rgb(r, g, b) = c else { return c };
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let dist = |a: (u8, u8, u8)| -> i32 {
        let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
        d(a.0, r) + d(a.1, g) + d(a.2, b)
    };
    let snap = |v: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| (**l as i32 - v as i32).abs())
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (snap(r), snap(g), snap(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let avg = (r as i32 + g as i32 + b as i32) / 3;
    let gi_idx = ((avg - 8 + 5) / 10).clamp(0, 23);
    let gv = (8 + 10 * gi_idx) as u8;
    if dist((gv, gv, gv)) < dist(cube) {
        Color::Indexed(232 + gi_idx as u8)
    } else {
        Color::Indexed(16 + (36 * ri + 6 * gi + bi) as u8)
    }
}

pub fn detect_truecolor() -> bool {
    std::env::var("COLORTERM")
        .map(|v| matches!(v.to_lowercase().as_str(), "truecolor" | "24bit"))
        .unwrap_or(false)
}

/// Built-in themes plus any `*.toml` in `dir`. A user theme with the id of a
/// built-in replaces it. Returns the themes and a list of files that failed to load.
pub fn load_all(dir: Option<&Path>) -> (Vec<Theme>, Vec<String>) {
    let mut themes = Theme::builtin();
    let mut problems = Vec::new();
    let Some(entries) = dir.and_then(|d| std::fs::read_dir(d).ok()) else {
        return (themes, problems);
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    let builtin = themes.clone();
    for path in files {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let result = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|raw| Theme::from_toml(&raw, &stem, &builtin));
        match result {
            Ok(t) => match themes.iter().position(|x| x.id == t.id) {
                Some(i) => themes[i] = t,
                None => themes.push(t),
            },
            Err(e) => problems.push(format!(
                "{}: {e}",
                path.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            )),
        }
    }
    (themes, problems)
}

#[cfg(test)]
impl Theme {
    pub fn builtin_first() -> Theme {
        Theme::builtin().remove(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_unique_and_the_default_exists() {
        let all = Theme::builtin();
        let mut ids: Vec<_> = all.iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        assert_eq!(all[0].id, DEFAULT_THEME);
        assert!(
            all.iter().any(|t| t.light)
                && all.iter().any(|t| !t.light)
                && all.iter().any(|t| t.id == "terminal")
        );
        for t in &all {
            if t.id != "terminal" {
                assert_ne!(t.fg, t.bg, "{} has unreadable text", t.id);
            }
        }
    }

    #[test]
    fn color_parsing() {
        assert_eq!(parse_color("#1a2b3c"), Some(Color::Rgb(0x1a, 0x2b, 0x3c)));
        assert_eq!(parse_color("FFFFFF"), Some(Color::Rgb(255, 255, 255)));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#gggggg"), None);
        assert_eq!(parse_color("red"), None);
    }

    #[test]
    fn downgrade_to_256_colors() {
        assert_eq!(nearest_256(Color::Rgb(0, 0, 0)), Color::Indexed(16));
        assert_eq!(nearest_256(Color::Rgb(255, 255, 255)), Color::Indexed(231));
        assert_eq!(nearest_256(Color::Rgb(255, 0, 0)), Color::Indexed(196));
        assert_eq!(nearest_256(Color::Rgb(128, 128, 128)), Color::Indexed(244));
        assert_eq!(nearest_256(Color::Cyan), Color::Cyan);
        let t = Theme::builtin()[0].clone().to_256();
        assert!(matches!(t.bg, Color::Indexed(_)) && matches!(t.accent, Color::Indexed(_)));
    }

    #[test]
    fn user_theme_overrides_a_base_and_reports_errors() {
        let base = Theme::builtin();
        let t = Theme::from_toml(
            "name = \"Mine\"\nbase = \"nord\"\naccent = \"#ff0000\"",
            "my theme",
            &base,
        )
        .unwrap();
        assert_eq!(
            (t.id.as_str(), t.name.as_str(), t.accent),
            ("my-theme", "Mine", Color::Rgb(255, 0, 0))
        );
        assert_eq!(t.bg, base.iter().find(|x| x.id == "nord").unwrap().bg);
        assert!(
            Theme::from_toml("accent = \"nope\"", "x", &base)
                .unwrap_err()
                .contains("accent")
        );
        assert!(Theme::from_toml("accent = 5", "x", &base).is_err());
        assert!(Theme::from_toml("= broken", "x", &base).is_err());
        assert!(Theme::from_toml("", "!!!", &base).is_err());
    }

    #[test]
    fn load_all_merges_user_files_and_collects_problems() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("mine.toml"), "accent = \"#00ff00\"").unwrap();
        std::fs::write(
            d.path().join("nord.toml"),
            "name = \"My Nord\"\nbase = \"nord\"",
        )
        .unwrap();
        std::fs::write(d.path().join("bad.toml"), "bg = \"zzz\"").unwrap();
        std::fs::write(d.path().join("ignored.txt"), "x").unwrap();
        let (themes, problems) = load_all(Some(d.path()));
        assert_eq!(themes.len(), Theme::builtin().len() + 1);
        assert_eq!(
            themes.iter().find(|t| t.id == "nord").unwrap().name,
            "My Nord"
        );
        assert!(themes.iter().any(|t| t.id == "mine"));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("bad.toml"));
        assert_eq!(load_all(None).0.len(), Theme::builtin().len());
        assert_eq!(
            load_all(Some(Path::new("/definitely/missing"))).0.len(),
            Theme::builtin().len()
        );
    }
}
