//! Semantic palette resolution for Herdr-managed terminal panes.
//!
//! Built-in RGB values mirror Herdr's semantic palettes. Their upstream color
//! sources are pinned in `THIRD_PARTY_NOTICES.md`; this module does not expose
//! a separate theme picker.

use ratatui::style::Color;
use std::{
    env,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime},
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
        palette!(accent:rgb!(122,162,247),panel_bg:rgb!(26,27,38),surface0:rgb!(36,40,59),surface1:rgb!(65,72,104),surface_dim:rgb!(26,27,38),overlay0:rgb!(86,95,137),overlay1:rgb!(105,113,150),text:rgb!(192,202,245),subtext0:rgb!(169,177,214),green:rgb!(158,206,106),yellow:rgb!(224,175,104),red:rgb!(247,118,142),blue:rgb!(122,162,247),teal:rgb!(125,207,255),peach:rgb!(255,158,100))
    }
    pub fn tokyo_night_day() -> Self {
        palette!(accent:rgb!(46,125,233),panel_bg:rgb!(225,226,231),surface0:rgb!(196,200,218),surface1:rgb!(168,174,203),surface_dim:rgb!(210,211,218),overlay0:rgb!(137,144,179),overlay1:rgb!(104,112,154),text:rgb!(55,96,191),subtext0:rgb!(97,114,176),green:rgb!(88,117,57),yellow:rgb!(140,108,62),red:rgb!(245,42,101),blue:rgb!(46,125,233),teal:rgb!(17,140,116),peach:rgb!(177,92,0))
    }
    pub fn dracula() -> Self {
        palette!(accent:rgb!(189,147,249),panel_bg:rgb!(40,42,54),surface0:rgb!(68,71,90),surface1:rgb!(98,114,164),surface_dim:rgb!(40,42,54),overlay0:rgb!(98,114,164),overlay1:rgb!(130,140,180),text:rgb!(248,248,242),subtext0:rgb!(210,210,220),green:rgb!(80,250,123),yellow:rgb!(241,250,140),red:rgb!(255,85,85),blue:rgb!(139,233,253),teal:rgb!(139,233,253),peach:rgb!(255,184,108))
    }
    pub fn nord() -> Self {
        palette!(accent:rgb!(136,192,208),panel_bg:rgb!(46,52,64),surface0:rgb!(59,66,82),surface1:rgb!(67,76,94),surface_dim:rgb!(46,52,64),overlay0:rgb!(76,86,106),overlay1:rgb!(100,110,130),text:rgb!(236,239,244),subtext0:rgb!(216,222,233),green:rgb!(163,190,140),yellow:rgb!(235,203,139),red:rgb!(191,97,106),blue:rgb!(129,161,193),teal:rgb!(143,188,187),peach:rgb!(208,135,112))
    }
    pub fn gruvbox() -> Self {
        palette!(accent:rgb!(215,153,33),panel_bg:rgb!(40,40,40),surface0:rgb!(60,56,54),surface1:rgb!(80,73,69),surface_dim:rgb!(40,40,40),overlay0:rgb!(146,131,116),overlay1:rgb!(168,153,132),text:rgb!(235,219,178),subtext0:rgb!(213,196,161),green:rgb!(184,187,38),yellow:rgb!(250,189,47),red:rgb!(251,73,52),blue:rgb!(131,165,152),teal:rgb!(142,192,124),peach:rgb!(254,128,25))
    }
    pub fn gruvbox_light() -> Self {
        palette!(accent:rgb!(7,102,120),panel_bg:rgb!(251,241,199),surface0:rgb!(235,219,178),surface1:rgb!(213,196,161),surface_dim:rgb!(242,229,188),overlay0:rgb!(146,131,116),overlay1:rgb!(124,111,100),text:rgb!(60,56,54),subtext0:rgb!(80,73,69),green:rgb!(121,116,14),yellow:rgb!(181,118,20),red:rgb!(157,0,6),blue:rgb!(7,102,120),teal:rgb!(66,123,88),peach:rgb!(175,58,3))
    }
    pub fn one_dark() -> Self {
        palette!(accent:rgb!(97,175,239),panel_bg:rgb!(40,44,52),surface0:rgb!(44,49,58),surface1:rgb!(62,68,81),surface_dim:rgb!(40,44,52),overlay0:rgb!(92,99,112),overlay1:rgb!(115,122,135),text:rgb!(171,178,191),subtext0:rgb!(150,156,168),green:rgb!(152,195,121),yellow:rgb!(229,192,123),red:rgb!(224,108,117),blue:rgb!(97,175,239),teal:rgb!(86,182,194),peach:rgb!(209,154,102))
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
        palette!(accent:rgb!(126,156,216),panel_bg:rgb!(31,31,40),surface0:rgb!(42,42,55),surface1:rgb!(54,54,70),surface_dim:rgb!(31,31,40),overlay0:rgb!(114,113,105),overlay1:rgb!(135,134,125),text:rgb!(220,215,186),subtext0:rgb!(200,195,170),green:rgb!(118,148,106),yellow:rgb!(192,163,110),red:rgb!(195,64,67),blue:rgb!(126,156,216),teal:rgb!(127,180,202),peach:rgb!(255,160,102))
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
        palette!(accent:rgb!(255,199,153),panel_bg:rgb!(26,26,26),surface0:rgb!(35,35,35),surface1:rgb!(40,40,40),surface_dim:rgb!(16,16,16),overlay0:rgb!(92,92,92),overlay1:rgb!(126,126,126),text:rgb!(255,255,255),subtext0:rgb!(160,160,160),green:rgb!(153,255,228),yellow:rgb!(255,199,153),red:rgb!(255,128,128),blue:rgb!(176,176,176),teal:rgb!(102,221,204),peach:rgb!(255,199,153))
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
        resolve_config(&path, None).unwrap_or_else(|error| ThemeResolution {
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

pub fn query_terminal_appearance() -> Option<Appearance> {
    const QUERY: &[u8] = b"\x1b]11;?\x1b\\";
    let mut stdout = io::stdout();
    stdout.write_all(QUERY).ok()?;
    stdout.flush().ok()?;

    let deadline = Instant::now() + Duration::from_millis(250);
    let mut response = Vec::with_capacity(128);
    while Instant::now() < deadline && response.len() < 512 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
        let mut poll = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll` receives one valid stack-allocated descriptor.
        if unsafe { libc::poll(&mut poll, 1, timeout) } <= 0 {
            break;
        }

        let mut chunk = [0_u8; 128];
        // SAFETY: `read` writes at most the capacity of a valid stack buffer.
        let length =
            unsafe { libc::read(libc::STDIN_FILENO, chunk.as_mut_ptr().cast(), chunk.len()) };
        if length <= 0 {
            break;
        }
        response.extend_from_slice(&chunk[..length as usize]);
        if let Some(appearance) = parse_terminal_appearance_response(&response) {
            return Some(appearance);
        }
    }
    None
}

fn parse_terminal_appearance_response(response: &[u8]) -> Option<Appearance> {
    const PREFIX: &[u8] = b"\x1b]11;";
    for start in response
        .windows(PREFIX.len())
        .enumerate()
        .filter_map(|(index, window)| (window == PREFIX).then_some(index + PREFIX.len()))
    {
        let tail = &response[start..];
        let bell = tail.iter().position(|byte| *byte == b'\x07');
        let string_terminator = tail.windows(2).position(|window| window == b"\x1b\\");
        let Some(end) = (match (bell, string_terminator) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(end), None) | (None, Some(end)) => Some(end),
            (None, None) => None,
        }) else {
            continue;
        };
        let Some(value) = std::str::from_utf8(&tail[..end]).ok() else {
            continue;
        };
        let Some((red, green, blue)) = parse_terminal_rgb(value) else {
            continue;
        };
        return Some(appearance_from_rgb(red, green, blue));
    }
    None
}

fn parse_terminal_rgb(value: &str) -> Option<(u8, u8, u8)> {
    if let Some(rgb) = value.strip_prefix("rgb:") {
        let mut parts = rgb.split('/');
        let color = (
            parse_hex_component(parts.next()?)?,
            parse_hex_component(parts.next()?)?,
            parse_hex_component(parts.next()?)?,
        );
        return parts.next().is_none().then_some(color);
    }
    let hex = value.strip_prefix('#')?;
    let digits = hex.len() / 3;
    if !matches!(digits, 1..=4) || hex.len() != digits * 3 {
        return None;
    }
    Some((
        parse_hex_component(&hex[..digits])?,
        parse_hex_component(&hex[digits..digits * 2])?,
        parse_hex_component(&hex[digits * 2..])?,
    ))
}

fn parse_hex_component(component: &str) -> Option<u8> {
    if component.is_empty()
        || component.len() > 4
        || !component
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    let value = u32::from_str_radix(component, 16).ok()?;
    let max = (1_u32 << (component.len() * 4)) - 1;
    Some(((value * 255 + max / 2) / max) as u8)
}

fn appearance_from_rgb(red: u8, green: u8, blue: u8) -> Appearance {
    let luminance = u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114;
    if luminance >= 128_000 {
        Appearance::Light
    } else {
        Appearance::Dark
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
    let (selected, fallback) = if auto_switch {
        match appearance.unwrap_or(Appearance::Dark) {
            Appearance::Dark => (dark_name.unwrap_or(&sibling_dark), "catppuccin"),
            Appearance::Light => (light_name.unwrap_or(&sibling_light), "catppuccin-latte"),
        }
    } else {
        (name, "catppuccin")
    };
    let (palette, diagnostic) = match Palette::named(selected) {
        Some(p) => (p, None),
        None => (
            Palette::named(fallback).expect("Herdr fallback theme is built in"),
            Some(format!(
                "Herdr theme '{selected}' is unavailable; using Herdr fallback '{fallback}'"
            )),
        ),
    };
    ThemeResolution {
        palette: overrides.apply(palette.with_terminal_background()),
        name: selected.to_string(),
        diagnostic,
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
    let custom = t
        .and_then(|v| v.get("custom"))
        .and_then(toml::Value::as_table);
    if let Some(custom) = custom {
        for (key, value) in custom {
            if let Some(raw) = value.as_str() {
                overrides
                    .colors
                    .insert(key.clone(), parse_color(raw).unwrap_or(Color::Cyan));
            }
        }
    }
    let legacy_accent = root
        .get("ui")
        .and_then(toml::Value::as_table)
        .and_then(|ui| ui.get("accent"))
        .and_then(toml::Value::as_str);
    if !custom.is_some_and(|custom| custom.contains_key("accent"))
        && let Some(accent) = legacy_accent.filter(|accent| *accent != "cyan")
    {
        overrides.colors.insert(
            "accent".to_string(),
            parse_color(accent).unwrap_or(Color::Cyan),
        );
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
    fn unavailable_auto_appearance_matches_herdrs_dark_default() {
        let resolution = resolve(
            "catppuccin",
            true,
            None,
            None,
            None,
            &ThemeOverrides::default(),
        );
        assert_eq!(resolution.name, "catppuccin");
        assert_eq!(
            resolution.palette,
            Palette::catppuccin().with_terminal_background()
        );
        assert_eq!(resolution.diagnostic, None);
    }

    #[test]
    fn unknown_theme_matches_herdrs_appearance_specific_fallback() {
        let resolution = resolve(
            "unknown",
            true,
            None,
            None,
            Some(Appearance::Light),
            &ThemeOverrides::default(),
        );
        assert_eq!(resolution.name, "unknown");
        assert_eq!(
            resolution.palette,
            Palette::catppuccin_latte().with_terminal_background()
        );
        assert!(
            resolution
                .diagnostic
                .as_deref()
                .is_some_and(|message| message.contains("catppuccin-latte"))
        );
    }

    #[test]
    fn config_applies_herdrs_legacy_ui_accent_precedence() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            r##"
[ui]
accent = "#010203"

[theme]
name = "catppuccin"
"##,
        )
        .unwrap();
        assert_eq!(
            resolve_config(&path, Some(Appearance::Dark))
                .unwrap()
                .palette
                .accent,
            Color::Rgb(1, 2, 3)
        );

        std::fs::write(
            &path,
            r##"
[ui]
accent = "#010203"

[theme]
name = "catppuccin"

[theme.custom]
accent = "#040506"
"##,
        )
        .unwrap();
        assert_eq!(
            resolve_config(&path, Some(Appearance::Dark))
                .unwrap()
                .palette
                .accent,
            Color::Rgb(4, 5, 6)
        );
    }

    #[test]
    fn parses_herdr_background_color_responses() {
        assert_eq!(
            parse_terminal_appearance_response(b"noise\x1b]11;rgb:ffff/ffff/ffff\x1b\\"),
            Some(Appearance::Light)
        );
        assert_eq!(
            parse_terminal_appearance_response(b"\x1b]11;#181825\x07"),
            Some(Appearance::Dark)
        );
        assert_eq!(
            parse_terminal_appearance_response(
                b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\\x1b]11;rgb:1818/1818/2525\x1b\\"
            ),
            Some(Appearance::Dark)
        );
    }

    #[test]
    fn rejects_incomplete_or_invalid_background_color_responses() {
        assert_eq!(
            parse_terminal_appearance_response(b"\x1b]11;rgb:ffff/ffff/ffff"),
            None
        );
        assert_eq!(
            parse_terminal_appearance_response(b"\x1b]11;rgb:nope/ffff/ffff\x1b\\"),
            None
        );
        assert_eq!(
            parse_terminal_appearance_response(b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
            None
        );
    }

    #[test]
    fn scales_terminal_rgb_components_like_herdr() {
        assert_eq!(parse_hex_component("f"), Some(255));
        assert_eq!(parse_hex_component("80"), Some(128));
        assert_eq!(parse_hex_component("800"), Some(128));
        assert_eq!(parse_hex_component("8000"), Some(128));
        assert_eq!(parse_terminal_rgb("#fff"), Some((255, 255, 255)));
        assert_eq!(parse_terminal_rgb("#808080"), Some((128, 128, 128)));
    }

    #[test]
    fn infers_appearance_at_herdrs_luminance_boundary() {
        assert_eq!(appearance_from_rgb(127, 127, 127), Appearance::Dark);
        assert_eq!(appearance_from_rgb(128, 128, 128), Appearance::Light);
    }

    #[test]
    fn built_in_semantic_colors_match_herdr() {
        assert_eq!(Palette::tokyo_night().overlay1, Color::Rgb(105, 113, 150));
        assert_eq!(
            Palette::tokyo_night_day().surface_dim,
            Color::Rgb(210, 211, 218)
        );
        assert_eq!(Palette::dracula().overlay1, Color::Rgb(130, 140, 180));
        assert_eq!(Palette::dracula().subtext0, Color::Rgb(210, 210, 220));
        assert_eq!(Palette::nord().overlay1, Color::Rgb(100, 110, 130));
        assert_eq!(Palette::one_dark().surface0, Color::Rgb(44, 49, 58));
        assert_eq!(Palette::one_dark().overlay1, Color::Rgb(115, 122, 135));
        assert_eq!(Palette::one_dark().subtext0, Color::Rgb(150, 156, 168));
        assert_eq!(Palette::kanagawa().overlay1, Color::Rgb(135, 134, 125));
        assert_eq!(Palette::vesper().panel_bg, Color::Rgb(26, 26, 26));
        assert_eq!(Palette::vesper().overlay0, Color::Rgb(92, 92, 92));
        assert_eq!(Palette::vesper().blue, Color::Rgb(176, 176, 176));
        assert_eq!(Palette::vesper().teal, Color::Rgb(102, 221, 204));
    }
}
