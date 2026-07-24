//! Semantic palette resolution for Herdr-managed terminal panes.
//!
//! Built-in RGB values below are selected from the canonical upstream palettes
//! pinned in `THIRD_PARTY_NOTICES.md`. They are mapped into Herdr's semantic
//! roles for compatibility, not exposed as a separate theme system.

use ratatui::style::Color;
use std::{
    env,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub panel_bg: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub surface_dim: Color,
    pub overlay0: Color,
    pub overlay1: Color,
    pub text: Color,
    pub subtext0: Color,
    pub accent: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub blue: Color,
    pub teal: Color,
    pub peach: Color,
}

macro_rules! palette { ($($n:ident:$v:expr),+ $(,)?) => { Palette { $($n: $v),+ } }; }
macro_rules! rgb {
    ($r:literal,$g:literal,$b:literal) => {
        Color::Rgb($r, $g, $b)
    };
}

impl Palette {
    pub fn terminal() -> Self {
        palette!(accent:Color::Blue,panel_bg:Color::Reset,surface0:Color::Reset,surface1:Color::DarkGray,surface_dim:Color::DarkGray,overlay0:Color::Gray,overlay1:Color::White,text:Color::Reset,subtext0:Color::Gray,green:Color::Green,yellow:Color::Yellow,red:Color::LightRed,blue:Color::Blue,teal:Color::Cyan,peach:Color::Yellow)
    }
    pub fn catppuccin() -> Self {
        palette!(accent:rgb!(137,180,250),panel_bg:rgb!(24,24,37),surface0:rgb!(49,50,68),surface1:rgb!(69,71,90),surface_dim:rgb!(30,30,46),overlay0:rgb!(108,112,134),overlay1:rgb!(127,132,156),text:rgb!(205,214,244),subtext0:rgb!(166,173,200),green:rgb!(166,227,161),yellow:rgb!(249,226,175),red:rgb!(243,139,168),blue:rgb!(137,180,250),teal:rgb!(148,226,213),peach:rgb!(250,179,135))
    }
    pub fn catppuccin_latte() -> Self {
        palette!(accent:rgb!(30,102,245),panel_bg:rgb!(239,241,245),surface0:rgb!(204,208,218),surface1:rgb!(188,192,204),surface_dim:rgb!(230,233,239),overlay0:rgb!(156,160,176),overlay1:rgb!(140,143,161),text:rgb!(76,79,105),subtext0:rgb!(108,111,133),green:rgb!(64,160,43),yellow:rgb!(223,142,29),red:rgb!(210,15,57),blue:rgb!(30,102,245),teal:rgb!(23,146,153),peach:rgb!(254,100,11))
    }
    pub fn tokyo_night() -> Self {
        palette!(accent:rgb!(122,162,247),panel_bg:rgb!(26,27,38),surface0:rgb!(36,40,59),surface1:rgb!(65,72,104),surface_dim:rgb!(26,27,38),overlay0:rgb!(86,95,137),overlay1:rgb!(115,122,162),text:rgb!(192,202,245),subtext0:rgb!(169,177,214),green:rgb!(158,206,106),yellow:rgb!(224,175,104),red:rgb!(247,118,142),blue:rgb!(122,162,247),teal:rgb!(125,207,255),peach:rgb!(255,158,100))
    }
    pub fn tokyo_night_day() -> Self {
        palette!(accent:rgb!(46,125,233),panel_bg:rgb!(225,226,231),surface0:rgb!(196,200,218),surface1:rgb!(168,174,203),surface_dim:rgb!(208,213,227),overlay0:rgb!(137,144,179),overlay1:rgb!(104,112,154),text:rgb!(55,96,191),subtext0:rgb!(97,114,176),green:rgb!(88,117,57),yellow:rgb!(140,108,62),red:rgb!(245,42,101),blue:rgb!(46,125,233),teal:rgb!(17,140,116),peach:rgb!(177,92,0))
    }
    pub fn dracula() -> Self {
        palette!(accent:rgb!(189,147,249),panel_bg:rgb!(40,42,54),surface0:rgb!(68,71,90),surface1:rgb!(98,114,164),surface_dim:rgb!(40,42,54),overlay0:rgb!(98,114,164),overlay1:rgb!(98,114,164),text:rgb!(248,248,242),subtext0:rgb!(98,114,164),green:rgb!(80,250,123),yellow:rgb!(241,250,140),red:rgb!(255,85,85),blue:rgb!(139,233,253),teal:rgb!(139,233,253),peach:rgb!(255,184,108))
    }
    pub fn nord() -> Self {
        palette!(accent:rgb!(136,192,208),panel_bg:rgb!(46,52,64),surface0:rgb!(59,66,82),surface1:rgb!(67,76,94),surface_dim:rgb!(46,52,64),overlay0:rgb!(76,86,106),overlay1:rgb!(76,86,106),text:rgb!(236,239,244),subtext0:rgb!(216,222,233),green:rgb!(163,190,140),yellow:rgb!(235,203,139),red:rgb!(191,97,106),blue:rgb!(129,161,193),teal:rgb!(143,188,187),peach:rgb!(208,135,112))
    }
    pub fn gruvbox() -> Self {
        palette!(accent:rgb!(215,153,33),panel_bg:rgb!(40,40,40),surface0:rgb!(60,56,54),surface1:rgb!(80,73,69),surface_dim:rgb!(40,40,40),overlay0:rgb!(146,131,116),overlay1:rgb!(168,153,132),text:rgb!(235,219,178),subtext0:rgb!(213,196,161),green:rgb!(184,187,38),yellow:rgb!(250,189,47),red:rgb!(251,73,52),blue:rgb!(131,165,152),teal:rgb!(142,192,124),peach:rgb!(254,128,25))
    }
    pub fn gruvbox_light() -> Self {
        palette!(accent:rgb!(7,102,120),panel_bg:rgb!(251,241,199),surface0:rgb!(235,219,178),surface1:rgb!(213,196,161),surface_dim:rgb!(242,229,188),overlay0:rgb!(146,131,116),overlay1:rgb!(124,111,100),text:rgb!(60,56,54),subtext0:rgb!(80,73,69),green:rgb!(121,116,14),yellow:rgb!(181,118,20),red:rgb!(157,0,6),blue:rgb!(7,102,120),teal:rgb!(66,123,88),peach:rgb!(175,58,3))
    }
    pub fn one_dark() -> Self {
        palette!(accent:rgb!(97,175,239),panel_bg:rgb!(40,44,52),surface0:rgb!(40,44,52),surface1:rgb!(62,68,81),surface_dim:rgb!(40,44,52),overlay0:rgb!(92,99,112),overlay1:rgb!(127,132,142),text:rgb!(171,178,191),subtext0:rgb!(127,132,142),green:rgb!(152,195,121),yellow:rgb!(229,192,123),red:rgb!(224,108,117),blue:rgb!(97,175,239),teal:rgb!(86,182,194),peach:rgb!(209,154,102))
    }
    pub fn one_light() -> Self {
        palette!(accent:rgb!(64,120,242),panel_bg:rgb!(250,250,250),surface0:rgb!(240,240,241),surface1:rgb!(229,229,230),surface_dim:rgb!(245,245,246),overlay0:rgb!(160,161,167),overlay1:rgb!(104,107,119),text:rgb!(56,58,66),subtext0:rgb!(104,107,119),green:rgb!(80,161,79),yellow:rgb!(193,132,1),red:rgb!(228,86,73),blue:rgb!(64,120,242),teal:rgb!(1,132,188),peach:rgb!(152,104,1))
    }
    pub fn solarized() -> Self {
        palette!(accent:rgb!(38,139,210),panel_bg:rgb!(0,43,54),surface0:rgb!(7,54,66),surface1:rgb!(88,110,117),surface_dim:rgb!(0,43,54),overlay0:rgb!(88,110,117),overlay1:rgb!(101,123,131),text:rgb!(147,161,161),subtext0:rgb!(131,148,150),green:rgb!(133,153,0),yellow:rgb!(181,137,0),red:rgb!(220,50,47),blue:rgb!(38,139,210),teal:rgb!(42,161,152),peach:rgb!(203,75,22))
    }
    pub fn solarized_light() -> Self {
        palette!(accent:rgb!(38,139,210),panel_bg:rgb!(253,246,227),surface0:rgb!(238,232,213),surface1:rgb!(147,161,161),surface_dim:rgb!(238,232,213),overlay0:rgb!(147,161,161),overlay1:rgb!(88,110,117),text:rgb!(101,123,131),subtext0:rgb!(131,148,150),green:rgb!(133,153,0),yellow:rgb!(181,137,0),red:rgb!(220,50,47),blue:rgb!(38,139,210),teal:rgb!(42,161,152),peach:rgb!(203,75,22))
    }
    pub fn kanagawa() -> Self {
        palette!(accent:rgb!(126,156,216),panel_bg:rgb!(31,31,40),surface0:rgb!(42,42,55),surface1:rgb!(54,54,70),surface_dim:rgb!(31,31,40),overlay0:rgb!(114,113,105),overlay1:rgb!(114,113,105),text:rgb!(220,215,186),subtext0:rgb!(200,195,170),green:rgb!(118,148,106),yellow:rgb!(192,163,110),red:rgb!(195,64,67),blue:rgb!(126,156,216),teal:rgb!(127,180,202),peach:rgb!(255,160,102))
    }
    pub fn kanagawa_lotus() -> Self {
        palette!(accent:rgb!(77,105,155),panel_bg:rgb!(242,236,188),surface0:rgb!(220,213,172),surface1:rgb!(201,203,209),surface_dim:rgb!(213,206,163),overlay0:rgb!(160,156,172),overlay1:rgb!(138,137,128),text:rgb!(84,84,100),subtext0:rgb!(67,67,108),green:rgb!(111,137,78),yellow:rgb!(119,113,63),red:rgb!(200,64,83),blue:rgb!(77,105,155),teal:rgb!(78,140,162),peach:rgb!(204,109,0))
    }
    pub fn rose_pine() -> Self {
        palette!(accent:rgb!(196,167,231),panel_bg:rgb!(25,23,36),surface0:rgb!(31,29,46),surface1:rgb!(38,35,58),surface_dim:rgb!(25,23,36),overlay0:rgb!(110,106,134),overlay1:rgb!(144,140,170),text:rgb!(224,222,244),subtext0:rgb!(200,197,220),green:rgb!(49,116,143),yellow:rgb!(246,193,119),red:rgb!(235,111,146),blue:rgb!(49,116,143),teal:rgb!(156,207,216),peach:rgb!(234,154,151))
    }
    pub fn rose_pine_dawn() -> Self {
        palette!(accent:rgb!(144,122,169),panel_bg:rgb!(250,244,237),surface0:rgb!(242,233,225),surface1:rgb!(255,250,243),surface_dim:rgb!(242,233,225),overlay0:rgb!(152,147,165),overlay1:rgb!(121,117,147),text:rgb!(70,66,97),subtext0:rgb!(121,117,147),green:rgb!(40,105,131),yellow:rgb!(234,157,52),red:rgb!(180,99,122),blue:rgb!(40,105,131),teal:rgb!(86,148,159),peach:rgb!(215,130,126))
    }
    pub fn vesper() -> Self {
        palette!(accent:rgb!(255,199,153),panel_bg:rgb!(16,16,16),surface0:rgb!(35,35,35),surface1:rgb!(40,40,40),surface_dim:rgb!(16,16,16),overlay0:rgb!(80,80,80),overlay1:rgb!(126,126,126),text:rgb!(255,255,255),subtext0:rgb!(160,160,160),green:rgb!(153,255,228),yellow:rgb!(255,199,153),red:rgb!(255,128,128),blue:rgb!(160,160,160),teal:rgb!(153,255,228),peach:rgb!(255,199,153))
    }
    pub fn named(name: &str) -> Option<Self> {
        Some(
            match name.to_ascii_lowercase().replace([' ', '_'], "-").as_str() {
                "terminal" => Self::terminal(),
                "catppuccin" | "catppuccin-mocha" => Self::catppuccin(),
                "catppuccin-latte" | "latte" | "light" => Self::catppuccin_latte(),
                "tokyo-night" | "tokyonight" => Self::tokyo_night(),
                "tokyo-night-day" | "tokyo-day" | "tokyonight-day" => Self::tokyo_night_day(),
                "dracula" => Self::dracula(),
                "nord" => Self::nord(),
                "gruvbox" | "gruvbox-dark" => Self::gruvbox(),
                "gruvbox-light" => Self::gruvbox_light(),
                "one-dark" | "onedark" => Self::one_dark(),
                "one-light" | "onelight" => Self::one_light(),
                "solarized" | "solarized-dark" => Self::solarized(),
                "solarized-light" => Self::solarized_light(),
                "kanagawa" => Self::kanagawa(),
                "kanagawa-lotus" | "lotus" => Self::kanagawa_lotus(),
                "rose-pine" | "rosepine" => Self::rose_pine(),
                "rose-pine-dawn" | "rosepine-dawn" | "dawn" => Self::rose_pine_dawn(),
                "vesper" => Self::vesper(),
                _ => return None,
            },
        )
    }

    pub fn with_terminal_background(mut self) -> Self {
        self.panel_bg = Color::Reset;
        self
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::catppuccin()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeOverrides {
    pub colors: std::collections::BTreeMap<String, Color>,
}
impl ThemeOverrides {
    pub fn apply(&self, mut p: Palette) -> Palette {
        for (key, color) in &self.colors {
            match key.as_str() {
                "panel_bg" => p.panel_bg = *color,
                "surface0" => p.surface0 = *color,
                "surface1" => p.surface1 = *color,
                "surface_dim" => p.surface_dim = *color,
                "overlay0" => p.overlay0 = *color,
                "overlay1" => p.overlay1 = *color,
                "text" => p.text = *color,
                "subtext0" => p.subtext0 = *color,
                "accent" => p.accent = *color,
                "green" => p.green = *color,
                "yellow" => p.yellow = *color,
                "red" => p.red = *color,
                "blue" => p.blue = *color,
                "teal" => p.teal = *color,
                "peach" => p.peach = *color,
                _ => {}
            }
        }
        p
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeResolution {
    pub palette: Palette,
    pub name: String,
    pub diagnostic: Option<String>,
}

pub fn load_from_env() -> (Option<PathBuf>, ThemeResolution, Option<SystemTime>) {
    let path = env::var_os("HERDR_CONFIG_PATH")
        .map(PathBuf::from)
        .or_else(resolve_herdr_config_path);
    let Some(path) = path else {
        return (
            None,
            ThemeResolution {
                palette: Palette::terminal(),
                name: "terminal".to_string(),
                diagnostic: Some(
                    "Herdr config path is unavailable; using terminal ANSI palette".to_string(),
                ),
            },
            None,
        );
    };
    let modified = path
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok();
    let theme = if path.exists() {
        resolve_config(&path, terminal_appearance()).unwrap_or_else(|error| ThemeResolution {
            palette: Palette::terminal(),
            name: "terminal".to_string(),
            diagnostic: Some(error),
        })
    } else {
        ThemeResolution {
            palette: Palette::catppuccin().with_terminal_background(),
            name: "catppuccin".to_string(),
            diagnostic: None,
        }
    };
    (Some(path), theme, modified)
}

fn resolve_herdr_config_path() -> Option<PathBuf> {
    let binary = env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let output = Command::new(binary).arg("--help").output().ok()?;
    output.stdout.split(|byte| *byte == b'\n').find_map(|line| {
        line.strip_prefix(b"Config:")
            .and_then(|path| std::str::from_utf8(path).ok())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    })
}

pub fn terminal_appearance() -> Option<Appearance> {
    if let Ok(value) = env::var("HERDR_WORKBENCH_APPEARANCE") {
        match value.to_ascii_lowercase().as_str() {
            "dark" => return Some(Appearance::Dark),
            "light" => return Some(Appearance::Light),
            _ => {}
        }
    }
    let background = env::var("COLORFGBG")
        .ok()?
        .rsplit(';')
        .next()?
        .parse::<u8>()
        .ok()?;
    Some(if background >= 7 {
        Appearance::Light
    } else {
        Appearance::Dark
    })
}

pub fn query_terminal_appearance() -> Option<Appearance> {
    const ENABLE: &[u8] = b"\x1b[?2031h";
    const QUERY: &[u8] = b"\x1b[?996n";
    const DISABLE: &[u8] = b"\x1b[?2031l";

    let mut stdout = io::stdout();
    stdout.write_all(ENABLE).ok()?;
    stdout.write_all(QUERY).ok()?;
    stdout.flush().ok()?;

    let mut poll = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    let mut response = [0_u8; 128];
    // SAFETY: `poll` receives one valid stack-allocated descriptor and `read`
    // writes at most the capacity of a valid stack-allocated byte buffer.
    let length = unsafe {
        if libc::poll(&mut poll, 1, 250) <= 0 {
            0
        } else {
            libc::read(
                libc::STDIN_FILENO,
                response.as_mut_ptr().cast(),
                response.len(),
            )
        }
    };
    let _ = stdout.write_all(DISABLE);
    let _ = stdout.flush();
    let response = (length > 0).then(|| &response[..length as usize])?;
    if response
        .windows(b"\x1b[?997;1n".len())
        .any(|window| window == b"\x1b[?997;1n")
    {
        Some(Appearance::Dark)
    } else if response
        .windows(b"\x1b[?997;2n".len())
        .any(|window| window == b"\x1b[?997;2n")
    {
        Some(Appearance::Light)
    } else {
        None
    }
}

pub fn resolve(
    name: &str,
    auto_switch: bool,
    dark_name: Option<&str>,
    light_name: Option<&str>,
    appearance: Option<Appearance>,
    overrides: &ThemeOverrides,
) -> ThemeResolution {
    let (sibling_dark, sibling_light) = sibling_names(name);
    let selected = if auto_switch {
        match appearance {
            Some(Appearance::Dark) => dark_name.unwrap_or(&sibling_dark),
            Some(Appearance::Light) => light_name.unwrap_or(&sibling_light),
            None => "terminal",
        }
    } else {
        name
    };
    let (palette, diagnostic) = match Palette::named(selected) {
        Some(p) => (p, None),
        None => (
            Palette::terminal(),
            Some(format!(
                "Herdr theme '{selected}' is unavailable; using terminal ANSI palette"
            )),
        ),
    };
    ThemeResolution {
        palette: overrides.apply(palette.with_terminal_background()),
        name: selected.to_string(),
        diagnostic: diagnostic.or_else(|| {
            if auto_switch && appearance.is_none() {
                Some("Herdr appearance was unavailable; using terminal ANSI palette".into())
            } else {
                None
            }
        }),
    }
}

/// Herdr v0.7.5's built-in dark/light sibling selection, including aliases.
fn sibling_names(name: &str) -> (String, String) {
    match name.to_ascii_lowercase().replace([' ', '_'], "-").as_str() {
        "catppuccin" | "catppuccin-mocha" | "catppuccin-latte" | "latte" | "light" => {
            ("catppuccin".into(), "catppuccin-latte".into())
        }
        "tokyo-night" | "tokyonight" | "tokyo-night-day" | "tokyo-day" | "tokyonight-day" => {
            ("tokyo-night".into(), "tokyo-night-day".into())
        }
        "gruvbox" | "gruvbox-dark" | "gruvbox-light" => ("gruvbox".into(), "gruvbox-light".into()),
        "one-dark" | "onedark" | "one-light" | "onelight" => {
            ("one-dark".into(), "one-light".into())
        }
        "solarized" | "solarized-dark" | "solarized-light" => {
            ("solarized".into(), "solarized-light".into())
        }
        "kanagawa" | "kanagawa-lotus" | "lotus" => ("kanagawa".into(), "kanagawa-lotus".into()),
        "rose-pine" | "rosepine" | "rose-pine-dawn" | "rosepine-dawn" | "dawn" => {
            ("rose-pine".into(), "rose-pine-dawn".into())
        }
        _ => (name.into(), name.into()),
    }
}

/// Read only the documented `[theme]` section from Herdr's resolved TOML config.
pub fn resolve_config(
    path: &Path,
    appearance: Option<Appearance>,
) -> Result<ThemeResolution, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read Herdr config {}: {e}", path.display()))?;
    let root: toml::Value = toml::from_str(&source)
        .map_err(|e| format!("invalid Herdr config {}: {e}", path.display()))?;
    let t = root.get("theme").and_then(toml::Value::as_table);
    let name = t
        .and_then(|v| v.get("name"))
        .and_then(toml::Value::as_str)
        .unwrap_or("catppuccin");
    let auto = t
        .and_then(|v| v.get("auto_switch"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    let dark = t
        .and_then(|v| v.get("dark_name"))
        .and_then(toml::Value::as_str);
    let light = t
        .and_then(|v| v.get("light_name"))
        .and_then(toml::Value::as_str);
    let mut overrides = ThemeOverrides::default();
    if let Some(custom) = t
        .and_then(|v| v.get("custom"))
        .and_then(toml::Value::as_table)
    {
        for (key, value) in custom {
            if let Some(raw) = value.as_str()
                && let Some(color) = parse_color(raw)
            {
                overrides.colors.insert(key.clone(), color);
            }
        }
    }
    Ok(resolve(name, auto, dark, light, appearance, &overrides))
}
fn parse_color(raw: &str) -> Option<Color> {
    let s = raw.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let hex = hex.as_bytes();
        if hex.len() == 3 {
            let nibble = |i| {
                std::str::from_utf8(&hex[i..i + 1])
                    .ok()
                    .and_then(|part| u8::from_str_radix(part, 16).ok())
                    .map(|value| value * 17)
            };
            return Some(Color::Rgb(nibble(0)?, nibble(1)?, nibble(2)?));
        }
        if hex.len() == 6 {
            let n = |i| u8::from_str_radix(std::str::from_utf8(&hex[i..i + 2]).ok()?, 16).ok();
            return Some(Color::Rgb(n(0)?, n(2)?, n(4)?));
        }
    }
    if let Some(parts) = s
        .strip_prefix("rgb(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let values: Vec<_> = parts.split(',').map(str::trim).collect();
        if values.len() == 3 {
            return Some(Color::Rgb(
                values[0].parse().ok()?,
                values[1].parse().ok()?,
                values[2].parse().ok()?,
            ));
        }
    }
    match s.to_ascii_lowercase().as_str() {
        "reset" | "default" | "none" | "transparent" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "magenta" | "purple" => Some(Color::Magenta),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtins_and_auto_switch_resolve() {
        assert!(Palette::named("vesper").is_some());
        let r = resolve(
            "catppuccin",
            true,
            Some("dracula"),
            Some("one-light"),
            Some(Appearance::Light),
            &ThemeOverrides::default(),
        );
        assert_eq!(r.name, "one-light");
    }
    #[test]
    fn custom_is_semantic() {
        let mut o = ThemeOverrides::default();
        o.colors.insert("accent".into(), Color::Red);
        assert_eq!(
            resolve("nord", false, None, None, None, &o).palette.accent,
            Color::Red
        );
    }

    #[test]
    fn resolved_canvas_inherits_terminal_background_unless_explicitly_overridden() {
        let inherited = resolve(
            "catppuccin",
            false,
            None,
            None,
            Some(Appearance::Dark),
            &ThemeOverrides::default(),
        );
        assert_eq!(inherited.palette.panel_bg, Color::Reset);

        let mut overrides = ThemeOverrides::default();
        overrides
            .colors
            .insert("panel_bg".into(), Color::Rgb(4, 5, 6));
        let explicit = resolve(
            "catppuccin",
            false,
            None,
            None,
            Some(Appearance::Dark),
            &overrides,
        );
        assert_eq!(explicit.palette.panel_bg, Color::Rgb(4, 5, 6));
    }

    #[test]
    fn unavailable_auto_appearance_uses_terminal_palette() {
        let resolution = resolve(
            "catppuccin",
            true,
            None,
            None,
            None,
            &ThemeOverrides::default(),
        );
        assert_eq!(resolution.name, "terminal");
        assert_eq!(resolution.palette, Palette::terminal());
        assert!(
            resolution
                .diagnostic
                .as_deref()
                .is_some_and(|message| message.contains("terminal ANSI"))
        );
    }
}
