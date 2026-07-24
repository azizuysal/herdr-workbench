use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const MIN_WIDTH: u16 = 24;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfig {
    #[serde(default = "default_width")]
    pub width: u16,
    #[serde(default = "default_true")]
    pub show_ignored: bool,
    #[serde(default)]
    pub show_git_directory: bool,
    #[serde(default)]
    pub icons: IconMode,
    #[serde(default)]
    pub follow_symlinks: bool,
    #[serde(default = "default_true")]
    pub restore_visible: bool,
    #[serde(default)]
    pub show_empty_git_groups: bool,
    #[serde(default)]
    pub open: OpenConfig,
}
pub type Config = PluginConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IconMode {
    #[default]
    NerdFont,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenConfig {
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub placement: OpenPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenPlacement {
    #[default]
    Tab,
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub path: PathBuf,
    pub config: PluginConfig,
    pub modified: Option<SystemTime>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    Invalid {
        path: PathBuf,
        message: String,
    },
    InvalidEditor(String),
    NonUtf8Editor,
    NonUtf8Path,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "cannot read config {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "invalid config {}: {source}", path.display())
            }
            Self::Invalid { path, message } => {
                write!(f, "invalid config {}: {message}", path.display())
            }
            Self::InvalidEditor(message) => write!(f, "invalid $EDITOR: {message}"),
            Self::NonUtf8Editor => write!(f, "$EDITOR must contain valid UTF-8"),
            Self::NonUtf8Path => write!(
                f,
                "the selected file path cannot be passed to the UTF-8 argv template"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            width: default_width(),
            show_ignored: true,
            show_git_directory: false,
            icons: IconMode::NerdFont,
            follow_symlinks: false,
            restore_visible: true,
            show_empty_git_groups: false,
            open: OpenConfig::default(),
        }
    }
}

impl Default for OpenConfig {
    fn default() -> Self {
        Self {
            command: Vec::new(),
            placement: OpenPlacement::Tab,
        }
    }
}

impl PluginConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Ok(load(path)?.config)
    }
    pub fn load_with_metadata(path: &Path) -> Result<LoadedConfig, ConfigError> {
        load(path)
    }
    pub fn validate(&self, path: &Path) -> Result<(), ConfigError> {
        if self.width < MIN_WIDTH {
            return Err(invalid(
                path,
                format!("width must be at least {MIN_WIDTH} columns"),
            ));
        }
        if self.open.command.is_empty() {
            return Ok(());
        }
        if self.open.command[0].trim().is_empty() {
            return Err(invalid(path, "open.command[0] must name an executable"));
        }
        if self
            .open
            .command
            .iter()
            .any(|argument| argument.contains('\0'))
        {
            return Err(invalid(path, "open.command must not contain NUL bytes"));
        }
        let placeholders = self
            .open
            .command
            .iter()
            .filter(|argument| argument.as_str() == "{path}")
            .count();
        if placeholders != 1
            || self
                .open
                .command
                .iter()
                .any(|argument| argument.contains("{path}") && argument != "{path}")
        {
            return Err(invalid(
                path,
                "open.command must contain exactly one argument equal to {path}",
            ));
        }
        Ok(())
    }

    pub fn open_argv(&self, path: &Path) -> Result<Option<Vec<String>>, ConfigError> {
        if self.open.command.is_empty() {
            return Ok(None);
        }
        let selected = path.to_str().ok_or(ConfigError::NonUtf8Path)?;
        Ok(Some(
            self.open
                .command
                .iter()
                .map(|argument| {
                    if argument == "{path}" {
                        selected.to_owned()
                    } else {
                        argument.clone()
                    }
                })
                .collect(),
        ))
    }

    pub fn open_argv_with_editor(
        &self,
        path: &Path,
        editor: Option<&OsStr>,
    ) -> Result<Option<Vec<String>>, ConfigError> {
        if let Some(argv) = self.open_argv(path)? {
            return Ok(Some(argv));
        }
        let Some(editor) = editor.filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let editor = editor.to_str().ok_or(ConfigError::NonUtf8Editor)?;
        if editor.trim().is_empty() {
            return Ok(None);
        }
        let mut argv = shlex::split(editor).ok_or_else(|| {
            ConfigError::InvalidEditor(
                "the command has unmatched quotes; use config.toml for a complex command"
                    .to_string(),
            )
        })?;
        if argv.is_empty() || argv[0].trim().is_empty() {
            return Err(ConfigError::InvalidEditor(
                "the command must name an executable".to_string(),
            ));
        }
        let selected = path.to_str().ok_or(ConfigError::NonUtf8Path)?;
        argv.push(selected.to_owned());
        Ok(Some(argv))
    }
}

impl LoadedConfig {
    pub fn has_changed(&self) -> Result<bool, ConfigError> {
        let current = fs::metadata(&self.path)
            .map_err(|source| ConfigError::Io {
                path: self.path.clone(),
                source,
            })?
            .modified()
            .ok();
        Ok(current != self.modified)
    }
}

pub fn config_path_from_env() -> Result<PathBuf, ConfigError> {
    let directory = env::var_os("HERDR_PLUGIN_CONFIG_DIR").ok_or_else(|| {
        invalid(
            Path::new("HERDR_PLUGIN_CONFIG_DIR"),
            "environment variable is not set",
        )
    })?;
    Ok(PathBuf::from(directory).join(CONFIG_FILE_NAME))
}

pub fn load_from_env() -> Result<LoadedConfig, ConfigError> {
    load(&config_path_from_env()?)
}

pub fn load(path: &Path) -> Result<LoadedConfig, ConfigError> {
    if !path.exists() {
        return Ok(LoadedConfig {
            path: path.to_owned(),
            config: PluginConfig::default(),
            modified: None,
        });
    }
    let source = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_owned(),
        source,
    })?;
    let config: PluginConfig = toml::from_str(&source).map_err(|source| ConfigError::Parse {
        path: path.to_owned(),
        source,
    })?;
    config.validate(path)?;
    let modified = fs::metadata(path)
        .map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?
        .modified()
        .ok();
    Ok(LoadedConfig {
        path: path.to_owned(),
        config,
        modified,
    })
}

pub fn validate_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = std::fs::read_to_string("herdr-plugin.toml")?;
    let manifest: toml::Value = toml::from_str(&manifest)?;
    let root = manifest.as_table().ok_or("manifest root must be a table")?;
    for (key, expected) in [("id", "herdr-workbench"), ("min_herdr_version", "0.7.5")] {
        if root.get(key).and_then(toml::Value::as_str) != Some(expected) {
            return Err(format!("manifest {key} must equal {expected:?}").into());
        }
    }
    let package_version = env!("CARGO_PKG_VERSION");
    if root.get("version").and_then(toml::Value::as_str) != Some(package_version) {
        return Err(
            format!("manifest version must equal package version {package_version:?}").into(),
        );
    }
    let platforms = root
        .get("platforms")
        .and_then(toml::Value::as_array)
        .ok_or("manifest platforms must be an array")?;
    if platforms.len() != 2
        || !platforms
            .iter()
            .any(|value| value.as_str() == Some("linux"))
        || !platforms
            .iter()
            .any(|value| value.as_str() == Some("macos"))
    {
        return Err("manifest platforms must be exactly linux and macos".into());
    }
    let panes = root
        .get("panes")
        .and_then(toml::Value::as_array)
        .ok_or("manifest must have panes")?;
    let pane_contract = [("sidebar", "split"), ("preview", "popup")];
    if panes.len() != pane_contract.len()
        || pane_contract.iter().any(|(id, placement)| {
            !panes.iter().any(|pane| {
                pane.get("id").and_then(toml::Value::as_str) == Some(*id)
                    && pane.get("placement").and_then(toml::Value::as_str) == Some(*placement)
                    && argv(pane.get("command"))
            })
        })
    {
        return Err(
            "manifest must define sidebar split and centered preview popup panes with argv commands"
                .into(),
        );
    }
    let preview = panes
        .iter()
        .find(|pane| pane.get("id").and_then(toml::Value::as_str) == Some("preview"))
        .ok_or("manifest must define the preview popup")?;
    if preview.get("width").and_then(toml::Value::as_str) != Some("80%")
        || preview.get("height").and_then(toml::Value::as_str) != Some("80%")
    {
        return Err("preview popup must be centered at 80% width and height".into());
    }
    let startup = root
        .get("startup")
        .and_then(toml::Value::as_array)
        .ok_or("manifest must have startup hook")?;
    if startup.len() != 1 || !argv(startup[0].get("command")) {
        return Err("manifest must define one startup argv command".into());
    }
    let actions = root
        .get("actions")
        .and_then(toml::Value::as_array)
        .ok_or("manifest must have actions")?;
    let expected = ["show", "hide", "toggle", "focus"];
    if actions.len() != expected.len()
        || expected.iter().any(|id| {
            !actions.iter().any(|action| {
                action.get("id").and_then(toml::Value::as_str) == Some(*id)
                    && action
                        .get("contexts")
                        .and_then(toml::Value::as_array)
                        .is_some_and(|contexts| {
                            contexts.len() == 1 && contexts[0].as_str() == Some("workspace")
                        })
                    && argv(action.get("command"))
            })
        })
    {
        return Err(
            "manifest must define exactly show, hide, toggle, and focus workspace argv actions"
                .into(),
        );
    }
    Ok(())
}

fn argv(value: Option<&toml::Value>) -> bool {
    value
        .and_then(toml::Value::as_array)
        .is_some_and(|items| !items.is_empty() && items.iter().all(toml::Value::is_str))
}

fn default_width() -> u16 {
    32
}
fn default_true() -> bool {
    true
}
fn invalid(path: &Path, message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        path: path.to_owned(),
        message: message.into(),
    }
}
