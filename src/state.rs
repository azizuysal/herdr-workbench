use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const STATE_FILE_NAME: &str = "state.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedState {
    #[serde(default = "state_version")]
    pub version: u8,
    #[serde(default)]
    pub workspaces: BTreeMap<String, WorkspaceState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceState {
    #[serde(default)]
    pub tabs: BTreeMap<String, SidebarState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SidebarState {
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub dock_side: DockSide,
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub previous_pane_id: Option<String>,
    #[serde(default)]
    pub workspace_cwd: Option<String>,
    #[serde(default)]
    pub tab_label: Option<String>,
    #[serde(default = "default_width")]
    pub width: u16,
    #[serde(default)]
    pub view: SidebarView,
    #[serde(default)]
    pub expanded: BTreeSet<String>,
    #[serde(default)]
    pub git_view_mode: GitViewMode,
    #[serde(default)]
    pub git_tree_expanded: BTreeSet<String>,
    #[serde(default)]
    pub git_tree_initialized: bool,
    #[serde(default)]
    pub git_collapsed_groups: BTreeSet<String>,
    #[serde(default)]
    pub selection: Option<String>,
    #[serde(default)]
    pub scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DockSide {
    #[default]
    Left,
    Right,
}

impl DockSide {
    pub const fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

impl std::fmt::Display for DockSide {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Left => "left",
            Self::Right => "right",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarView {
    #[default]
    Explorer,
    SourceControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum GitViewMode {
    #[default]
    Flat,
    Tree,
}

impl GitViewMode {
    pub const fn opposite(self) -> Self {
        match self {
            Self::Flat => Self::Tree,
            Self::Tree => Self::Flat,
        }
    }
}

#[derive(Debug)]
pub enum StateError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Encode(serde_json::Error),
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
    UnsupportedVersion {
        path: PathBuf,
        version: u8,
    },
    MissingStateDirectory,
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "state file {}: {source}", path.display()),
            Self::Encode(source) => write!(f, "cannot encode sidebar state: {source}"),
            Self::Corrupt { path, source } => write!(
                f,
                "state file {} is corrupt: {source}; reset it explicitly to continue",
                path.display()
            ),
            Self::UnsupportedVersion { path, version } => write!(
                f,
                "state file {} uses unsupported version {version}; reset it explicitly to continue",
                path.display()
            ),
            Self::MissingStateDirectory => write!(f, "HERDR_PLUGIN_STATE_DIR is not set"),
        }
    }
}
impl std::error::Error for StateError {}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            visible: false,
            dock_side: DockSide::Left,
            pane_id: None,
            previous_pane_id: None,
            workspace_cwd: None,
            tab_label: None,
            width: default_width(),
            view: SidebarView::Explorer,
            expanded: BTreeSet::new(),
            git_view_mode: GitViewMode::Flat,
            git_tree_expanded: BTreeSet::new(),
            git_tree_initialized: false,
            git_collapsed_groups: BTreeSet::new(),
            selection: None,
            scroll: 0,
        }
    }
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: state_version(),
            workspaces: BTreeMap::new(),
        }
    }
}

impl PersistedState {
    pub fn load(path: &Path) -> Result<Self, StateError> {
        load(path)
    }
    pub fn save_atomic(&self, path: &Path) -> Result<(), StateError> {
        save(path, self)
    }
    pub fn tab_mut(
        &mut self,
        workspace: impl Into<String>,
        tab: impl Into<String>,
    ) -> &mut SidebarState {
        self.workspaces
            .entry(workspace.into())
            .or_default()
            .tabs
            .entry(tab.into())
            .or_default()
    }
    pub fn tab(&self, workspace: &str, tab: &str) -> Option<&SidebarState> {
        self.workspaces.get(workspace)?.tabs.get(tab)
    }
}

pub fn state_path_from_env() -> Result<PathBuf, StateError> {
    let directory =
        env::var_os("HERDR_PLUGIN_STATE_DIR").ok_or(StateError::MissingStateDirectory)?;
    Ok(PathBuf::from(directory).join(STATE_FILE_NAME))
}
pub fn load_from_env() -> Result<PersistedState, StateError> {
    load(&state_path_from_env()?)
}
pub fn save_to_env(state: &PersistedState) -> Result<(), StateError> {
    save(&state_path_from_env()?, state)
}

pub fn load(path: &Path) -> Result<PersistedState, StateError> {
    if !path.exists() {
        return Ok(PersistedState::default());
    }
    let bytes = fs::read(path).map_err(|source| StateError::Io {
        path: path.to_owned(),
        source,
    })?;
    let state: PersistedState =
        serde_json::from_slice(&bytes).map_err(|source| StateError::Corrupt {
            path: path.to_owned(),
            source,
        })?;
    if state.version != state_version() {
        return Err(StateError::UnsupportedVersion {
            path: path.to_owned(),
            version: state.version,
        });
    }
    Ok(state)
}

/// This is deliberately explicit: callers must show recovery UI before invoking it.
pub fn reset_corrupt_state(path: &Path) -> Result<PersistedState, StateError> {
    let state = PersistedState::default();
    save(path, &state)?;
    Ok(state)
}

pub fn save(path: &Path, state: &PersistedState) -> Result<(), StateError> {
    let parent = path.parent().ok_or_else(|| StateError::Io {
        path: path.to_owned(),
        source: std::io::Error::other("state path has no parent"),
    })?;
    fs::create_dir_all(parent).map_err(|source| StateError::Io {
        path: parent.to_owned(),
        source,
    })?;
    let bytes = serde_json::to_vec_pretty(state).map_err(StateError::Encode)?;
    let temporary = parent.join(format!(".{}.{}.tmp", STATE_FILE_NAME, std::process::id()));
    let mut file = File::create(&temporary).map_err(|source| StateError::Io {
        path: temporary.clone(),
        source,
    })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| StateError::Io {
            path: temporary.clone(),
            source,
        })?;
    fs::rename(&temporary, path).map_err(|source| StateError::Io {
        path: path.to_owned(),
        source,
    })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| StateError::Io {
            path: parent.to_owned(),
            source,
        })?;
    Ok(())
}

fn state_version() -> u8 {
    1
}
fn default_width() -> u16 {
    32
}
