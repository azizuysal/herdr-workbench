use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::config::{self, Config};
use crate::file_tree::sanitize_terminal;
use crate::herdr::{HerdrClient, HerdrError, InvocationContext, PaneInfo, command_args, pane_list};
use crate::state::{self, DockSide, PersistedState, StateError};
use crate::workspace::{WorkspaceError, WorkspaceRoot};
use crate::{PLUGIN_ID, SIDEBAR_ENTRYPOINT};

const ACTION_LOCK_FILE: &str = "controller.lock";
const LOCK_WAIT: Duration = Duration::from_secs(5);
const STALE_LOCK_AGE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Show,
    Hide,
    Toggle,
    Focus,
    ToggleDock,
}

struct SidebarIdentity {
    root: PathBuf,
    title: String,
}

#[derive(Debug)]
pub enum ControllerError {
    Herdr(HerdrError),
    State(StateError),
    Config(config::ConfigError),
    Workspace(WorkspaceError),
    MissingContext(&'static str),
    InvalidContext(String),
    InvalidResponse(String),
    LockTimeout(PathBuf),
    LockIo {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Herdr(error) => write!(formatter, "{error}"),
            Self::State(error) => write!(formatter, "{error}"),
            Self::Config(error) => write!(formatter, "{error}"),
            Self::Workspace(error) => write!(formatter, "{error}"),
            Self::MissingContext(field) => {
                write!(formatter, "Herdr invocation context is missing {field}")
            }
            Self::InvalidContext(message) => write!(formatter, "invalid Herdr context: {message}"),
            Self::InvalidResponse(message) => {
                write!(formatter, "invalid Herdr response: {message}")
            }
            Self::LockTimeout(path) => write!(
                formatter,
                "another sidebar action did not finish within {} seconds; lock: {}",
                LOCK_WAIT.as_secs(),
                path.display()
            ),
            Self::LockIo { path, source } => {
                write!(formatter, "controller lock {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ControllerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Herdr(error) => Some(error),
            Self::State(error) => Some(error),
            Self::Config(error) => Some(error),
            Self::Workspace(error) => Some(error),
            Self::LockIo { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<HerdrError> for ControllerError {
    fn from(error: HerdrError) -> Self {
        Self::Herdr(error)
    }
}

impl From<StateError> for ControllerError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

impl From<config::ConfigError> for ControllerError {
    fn from(error: config::ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<WorkspaceError> for ControllerError {
    fn from(error: WorkspaceError) -> Self {
        Self::Workspace(error)
    }
}

pub struct Controller<H> {
    herdr: H,
    context: InvocationContext,
    state_path: PathBuf,
    config: Config,
}

impl<H: HerdrClient> Controller<H> {
    pub fn from_env(herdr: H) -> Result<Self, ControllerError> {
        let context = InvocationContext::from_env()?;
        let state_path = state::state_path_from_env()?;
        let config = config::load_from_env()?.config;
        Ok(Self::new(herdr, context, state_path, config))
    }

    pub fn new(herdr: H, context: InvocationContext, state_path: PathBuf, config: Config) -> Self {
        Self {
            herdr,
            context,
            state_path,
            config,
        }
    }

    pub fn execute(&self, action: Action) -> Result<(), ControllerError> {
        let mut action_lock = ActionLock::acquire(lock_path(&self.state_path))?;
        let workspace_id = required_id(
            self.context.workspace_id.as_deref(),
            "workspace_id",
            "workspace",
        )?;
        let tab_id = required_id(self.context.tab_id.as_deref(), "tab_id", "tab")?;
        let mut persisted = PersistedState::load(&self.state_path)?;
        let panes = self.list_panes(workspace_id)?;
        let identity =
            SidebarIdentity::new(self.sidebar_root(&persisted, workspace_id, tab_id, &panes)?)?;
        let owned = owned_sidebar(&persisted, workspace_id, tab_id, &panes, &identity.title);

        match action {
            Action::Show | Action::Focus => {
                self.show(
                    &mut persisted,
                    workspace_id,
                    tab_id,
                    &panes,
                    owned,
                    &identity,
                )?;
            }
            Action::Hide => {
                self.hide(
                    &mut persisted,
                    workspace_id,
                    tab_id,
                    &panes,
                    owned,
                    &mut action_lock,
                )?;
            }
            Action::Toggle if owned.is_some() => {
                self.hide(
                    &mut persisted,
                    workspace_id,
                    tab_id,
                    &panes,
                    owned,
                    &mut action_lock,
                )?;
            }
            Action::Toggle => {
                self.show(
                    &mut persisted,
                    workspace_id,
                    tab_id,
                    &panes,
                    None,
                    &identity,
                )?;
            }
            Action::ToggleDock => {
                let sidebar = owned.ok_or_else(|| {
                    ControllerError::InvalidResponse(format!(
                        "tab {tab_id} has no registered sidebar pane to dock"
                    ))
                })?;
                self.toggle_dock(&mut persisted, workspace_id, tab_id, sidebar)?;
            }
        }
        persisted.save_atomic(&self.state_path)?;
        Ok(())
    }

    pub fn restore(&self) -> Result<(), ControllerError> {
        let _lock = ActionLock::acquire(lock_path(&self.state_path))?;
        let mut persisted = PersistedState::load(&self.state_path)?;
        if !self.config.restore_visible {
            return Ok(());
        }
        let panes = self.list_all_panes()?;
        let targets: Vec<(String, String)> = persisted
            .workspaces
            .iter()
            .flat_map(|(workspace, workspace_state)| {
                workspace_state
                    .tabs
                    .iter()
                    .filter(|(_, sidebar)| sidebar.visible)
                    .map(move |(tab, _)| (workspace.clone(), tab.clone()))
            })
            .collect();

        for (workspace_id, tab_id) in targets {
            let tab_panes: Vec<PaneInfo> = panes
                .iter()
                .filter(|pane| pane.workspace_id == workspace_id && pane.tab_id == tab_id)
                .cloned()
                .collect();
            if tab_panes.is_empty() {
                return Err(ControllerError::InvalidResponse(format!(
                    "cannot restore sidebar for missing tab {tab_id}"
                )));
            }
            let identity = SidebarIdentity::new(self.sidebar_root(
                &persisted,
                &workspace_id,
                &tab_id,
                &tab_panes,
            )?)?;
            let owned = owned_sidebar(
                &persisted,
                &workspace_id,
                &tab_id,
                &tab_panes,
                &identity.title,
            );
            if owned.is_some() {
                continue;
            }
            self.show(
                &mut persisted,
                &workspace_id,
                &tab_id,
                &tab_panes,
                None,
                &identity,
            )?;
        }
        persisted.save_atomic(&self.state_path)?;
        Ok(())
    }

    fn show(
        &self,
        persisted: &mut PersistedState,
        workspace_id: &str,
        tab_id: &str,
        panes: &[PaneInfo],
        owned: Option<&PaneInfo>,
        identity: &SidebarIdentity,
    ) -> Result<(), ControllerError> {
        let title = identity.title.as_str();
        if let Some(sidebar) = owned {
            self.focus_pane(&sidebar.pane_id)?;
            let state = persisted.tab_mut(workspace_id, tab_id);
            state.visible = true;
            state.pane_id = Some(sidebar.pane_id.clone());
            return Ok(());
        }

        if panes
            .iter()
            .any(|pane| pane.tab_id == tab_id && has_sidebar_title(pane, title))
        {
            return Err(ControllerError::InvalidResponse(format!(
                "tab {tab_id} already contains an unregistered sidebar pane titled {title:?}; refusing to guess ownership or create a duplicate"
            )));
        }
        let focused = panes
            .iter()
            .find(|pane| pane.focused && pane.tab_id == tab_id)
            .or_else(|| {
                self.context.focused_pane_id.as_deref().and_then(|id| {
                    panes
                        .iter()
                        .find(|pane| pane.pane_id == id && pane.tab_id == tab_id)
                })
            });
        let target = focused
            .or_else(|| panes.iter().find(|pane| pane.tab_id == tab_id))
            .ok_or_else(|| {
                ControllerError::InvalidResponse(format!("tab {tab_id} has no target pane"))
            })?;
        validate_id(&target.pane_id, "pane")?;
        let layout = self.layout(&target.pane_id)?;
        let saved = persisted.tab(workspace_id, tab_id).cloned();
        let dock_side = saved
            .as_ref()
            .map(|state| state.dock_side)
            .unwrap_or_default();
        let edge = edge_pane(&layout, tab_id, dock_side).unwrap_or_else(|| target.pane_id.clone());
        validate_id(&edge, "pane")?;
        let width = saved
            .as_ref()
            .map(|state| state.width)
            .unwrap_or(self.config.width);
        let mut args = command_args(&[
            "plugin",
            "pane",
            "open",
            "--plugin",
            PLUGIN_ID,
            "--entrypoint",
            SIDEBAR_ENTRYPOINT,
            "--placement",
            "split",
            "--target-pane",
            &edge,
            "--direction",
            "right",
        ]);
        args.push("--env".into());
        args.push(format!("HERDR_WORKBENCH_WIDTH={width}").into());
        if let Some(editor) = editor_environment_argument(std::env::var_os("EDITOR").as_deref()) {
            args.push("--env".into());
            args.push(editor);
        }
        args.push("--focus".into());
        let response = self.herdr.command(&args)?;
        let pane_id = opened_pane_id(&response)?;
        validate_id(&pane_id, "pane")?;
        self.command(&["pane", "rename", &pane_id, title])?;

        if dock_side == DockSide::Left && pane_id != edge {
            self.command(&[
                "pane",
                "swap",
                "--source-pane",
                &pane_id,
                "--target-pane",
                &edge,
            ])?;
        }
        let post_layout = self.layout(&pane_id)?;
        self.resize_to_width(&pane_id, width, dock_side, &post_layout)?;

        let state = persisted.tab_mut(workspace_id, tab_id);
        state.visible = true;
        state.pane_id = Some(pane_id);
        state.previous_pane_id = focused.map(|pane| pane.pane_id.clone());
        state.workspace_cwd = Some(identity.root.to_string_lossy().into_owned());
        state.tab_label.clone_from(&self.context.tab_label);
        state.width = width;
        Ok(())
    }

    fn toggle_dock(
        &self,
        persisted: &mut PersistedState,
        workspace_id: &str,
        tab_id: &str,
        sidebar: &PaneInfo,
    ) -> Result<(), ControllerError> {
        let layout = self.layout(&sidebar.pane_id)?;
        let saved_side = persisted
            .tab(workspace_id, tab_id)
            .map(|state| state.dock_side)
            .unwrap_or_default();
        let current = pane_dock_side(&layout, &sidebar.pane_id).unwrap_or(saved_side);
        let desired = current.opposite();
        let target =
            edge_pane_excluding(&layout, tab_id, desired, &sidebar.pane_id).ok_or_else(|| {
                ControllerError::InvalidResponse(
                    "cannot dock the sidebar without another pane in the tab".to_string(),
                )
            })?;
        validate_id(&target, "pane")?;
        let width = pane_width(&layout, &sidebar.pane_id)
            .or_else(|| persisted.tab(workspace_id, tab_id).map(|state| state.width))
            .unwrap_or(self.config.width);
        let response = self.command(&[
            "pane",
            "swap",
            "--source-pane",
            &sidebar.pane_id,
            "--target-pane",
            &target,
        ])?;
        if response
            .pointer("/result/swap/changed")
            .and_then(Value::as_bool)
            != Some(true)
        {
            let reason = response
                .pointer("/result/swap/reason")
                .and_then(Value::as_str)
                .unwrap_or("unchanged");
            return Err(ControllerError::InvalidResponse(format!(
                "Herdr did not move the sidebar to the {desired} edge: {reason}"
            )));
        }
        let moved_layout = self.layout(&sidebar.pane_id)?;
        if let Err(error) = self.resize_to_width(&sidebar.pane_id, width, desired, &moved_layout) {
            let _ = self.command(&[
                "pane",
                "swap",
                "--source-pane",
                &sidebar.pane_id,
                "--target-pane",
                &target,
            ]);
            return Err(error);
        }
        let state = persisted.tab_mut(workspace_id, tab_id);
        state.dock_side = desired;
        state.width = width;
        state.pane_id = Some(sidebar.pane_id.clone());
        state.visible = true;
        Ok(())
    }

    fn hide(
        &self,
        persisted: &mut PersistedState,
        workspace_id: &str,
        tab_id: &str,
        _panes: &[PaneInfo],
        owned: Option<&PaneInfo>,
        action_lock: &mut ActionLock,
    ) -> Result<(), ControllerError> {
        let saved = persisted.tab(workspace_id, tab_id).cloned();
        if let Some(sidebar) = owned
            && let Ok(layout) = self.layout(&sidebar.pane_id)
            && let Some(width) = pane_width(&layout, &sidebar.pane_id)
        {
            persisted.tab_mut(workspace_id, tab_id).width = width;
        }
        let state = persisted.tab_mut(workspace_id, tab_id);
        state.visible = false;
        state.pane_id = None;
        if owned.is_some() {
            persisted.save_atomic(&self.state_path)?;
        }
        if let Some(sidebar) = owned {
            let self_closing = std::env::var("HERDR_PANE_ID").ok().as_deref()
                == Some(sidebar.pane_id.as_str())
                && std::env::var("HERDR_PLUGIN_ENTRYPOINT_ID").ok().as_deref()
                    == Some(SIDEBAR_ENTRYPOINT);
            if self_closing {
                action_lock.release()?;
            }
            if let Err(error) = self.command(&["pane", "close", &sidebar.pane_id]) {
                if self_closing {
                    *action_lock = ActionLock::acquire(lock_path(&self.state_path))?;
                }
                if let Some(saved) = saved.clone() {
                    *persisted.tab_mut(workspace_id, tab_id) = saved;
                    persisted.save_atomic(&self.state_path)?;
                }
                return Err(error);
            }
        }
        let previous = saved.and_then(|state| state.previous_pane_id);
        if let Some(previous) = previous.filter(|id| valid_id(id)) {
            let _ = self.focus_pane(&previous);
        }
        Ok(())
    }

    fn list_panes(&self, workspace_id: &str) -> Result<Vec<PaneInfo>, ControllerError> {
        validate_id(workspace_id, "workspace")?;
        let value = self.command(&["pane", "list", "--workspace", workspace_id])?;
        Ok(pane_list(&value)?)
    }

    fn list_all_panes(&self) -> Result<Vec<PaneInfo>, ControllerError> {
        let value = self.command(&["pane", "list"])?;
        Ok(pane_list(&value)?)
    }

    fn sidebar_root(
        &self,
        persisted: &PersistedState,
        workspace_id: &str,
        tab_id: &str,
        panes: &[PaneInfo],
    ) -> Result<PathBuf, ControllerError> {
        if let Some(saved) = persisted
            .tab(workspace_id, tab_id)
            .and_then(|state| state.workspace_cwd.as_deref())
        {
            let path = PathBuf::from(saved);
            if !path.exists() {
                return Ok(path);
            }
            return Ok(WorkspaceRoot::resolve(&path)?.path().to_owned());
        }
        let cwd = self
            .context
            .focused_pane_cwd
            .as_deref()
            .or(self.context.workspace_cwd.as_deref())
            .map(PathBuf::from)
            .or_else(|| {
                panes
                    .iter()
                    .find(|pane| pane.focused && pane.tab_id == tab_id)
                    .and_then(|pane| pane.foreground_cwd.as_ref().or(pane.cwd.as_ref()))
                    .map(PathBuf::from)
            })
            .or_else(|| {
                panes
                    .iter()
                    .find(|pane| pane.tab_id == tab_id)
                    .and_then(|pane| pane.foreground_cwd.as_ref().or(pane.cwd.as_ref()))
                    .map(PathBuf::from)
            })
            .ok_or(ControllerError::MissingContext("workspace_cwd"))?;
        Ok(WorkspaceRoot::resolve(&cwd)?.path().to_owned())
    }

    fn layout(&self, pane_id: &str) -> Result<Value, ControllerError> {
        self.command(&["pane", "layout", "--pane", pane_id])
    }

    fn resize_to_width(
        &self,
        pane_id: &str,
        requested: u16,
        dock_side: DockSide,
        layout: &Value,
    ) -> Result<(), ControllerError> {
        let area_width = layout_area_width(layout).unwrap_or(requested.max(1));
        let maximum = ((u32::from(area_width) * 45) / 100).max(1) as u16;
        let desired = requested.min(maximum);
        let mut current_layout = layout.clone();
        for _ in 0..4 {
            let current = pane_width(&current_layout, pane_id).unwrap_or(desired);
            if current == desired || area_width == 0 {
                return Ok(());
            }
            let direction = match (dock_side, current < desired) {
                (DockSide::Left, true) | (DockSide::Right, false) => "right",
                (DockSide::Left, false) | (DockSide::Right, true) => "left",
            };
            let amount = (f64::from(current.abs_diff(desired)) / f64::from(area_width))
                .min(0.5)
                .to_string();
            current_layout = self.command(&[
                "pane",
                "resize",
                "--pane",
                pane_id,
                "--direction",
                direction,
                "--amount",
                &amount,
            ])?;
        }
        Ok(())
    }

    fn command(&self, parts: &[&str]) -> Result<Value, ControllerError> {
        Ok(self.herdr.command(&command_args(parts))?)
    }

    fn focus_pane(&self, pane_id: &str) -> Result<(), ControllerError> {
        validate_id(pane_id, "pane")?;
        self.command(&["pane", "zoom", pane_id, "--on"])?;
        self.command(&["pane", "zoom", pane_id, "--off"])?;
        Ok(())
    }
}

pub fn register_sidebar_instance() -> Result<(), ControllerError> {
    let identity = sidebar_process_identity_from_env()?;
    let state_path = state::state_path_from_env()?;
    let _lock = ActionLock::acquire(lock_path(&state_path))?;
    let mut persisted = PersistedState::load(&state_path)?;
    let tab = persisted.tab_mut(identity.workspace_id, identity.tab_id);
    tab.visible = true;
    tab.pane_id = Some(identity.pane_id);
    if let Ok(width) = std::env::var("HERDR_WORKBENCH_WIDTH")
        && let Ok(width) = width.parse::<u16>()
    {
        tab.width = width;
    }
    persisted.save_atomic(&state_path)?;
    Ok(())
}

pub fn unregister_sidebar_instance() -> Result<(), ControllerError> {
    let identity = sidebar_process_identity_from_env()?;
    let state_path = state::state_path_from_env()?;
    let _lock = ActionLock::acquire(lock_path(&state_path))?;
    let mut persisted = PersistedState::load(&state_path)?;
    let tab = persisted.tab_mut(identity.workspace_id, identity.tab_id);
    tab.visible = false;
    tab.pane_id = None;
    persisted.save_atomic(&state_path)?;
    Ok(())
}

struct SidebarProcessIdentity {
    workspace_id: String,
    tab_id: String,
    pane_id: String,
}

fn sidebar_process_identity_from_env() -> Result<SidebarProcessIdentity, ControllerError> {
    validate_sidebar_process_context(
        std::env::var("HERDR_ENV").ok().as_deref(),
        std::env::var_os("HERDR_SOCKET_PATH").as_deref(),
        std::env::var("HERDR_PLUGIN_ID").ok().as_deref(),
        std::env::var("HERDR_PLUGIN_ENTRYPOINT_ID").ok().as_deref(),
    )?;
    let workspace_id = std::env::var("HERDR_WORKSPACE_ID")
        .map_err(|_| ControllerError::MissingContext("HERDR_WORKSPACE_ID"))?;
    let tab_id = std::env::var("HERDR_TAB_ID")
        .map_err(|_| ControllerError::MissingContext("HERDR_TAB_ID"))?;
    let pane_id = std::env::var("HERDR_PANE_ID")
        .map_err(|_| ControllerError::MissingContext("HERDR_PANE_ID"))?;
    validate_id(&workspace_id, "workspace")?;
    validate_id(&tab_id, "tab")?;
    validate_id(&pane_id, "pane")?;
    Ok(SidebarProcessIdentity {
        workspace_id,
        tab_id,
        pane_id,
    })
}

fn validate_sidebar_process_context(
    herdr_env: Option<&str>,
    socket_path: Option<&OsStr>,
    plugin_id: Option<&str>,
    entrypoint_id: Option<&str>,
) -> Result<(), ControllerError> {
    if herdr_env != Some("1") || socket_path.is_none_or(OsStr::is_empty) {
        return Err(ControllerError::InvalidContext(
            "sidebar must run as a Herdr-managed plugin pane".to_string(),
        ));
    }
    if plugin_id != Some(PLUGIN_ID) || entrypoint_id != Some(SIDEBAR_ENTRYPOINT) {
        return Err(ControllerError::InvalidContext(
            "sidebar process lacks matching plugin ownership context".to_string(),
        ));
    }
    Ok(())
}

fn owned_sidebar<'a>(
    persisted: &PersistedState,
    workspace_id: &str,
    tab_id: &str,
    panes: &'a [PaneInfo],
    title: &str,
) -> Option<&'a PaneInfo> {
    let pane_id = persisted.tab(workspace_id, tab_id)?.pane_id.as_deref()?;
    panes.iter().find(|pane| {
        pane.pane_id == pane_id
            && pane.workspace_id == workspace_id
            && pane.tab_id == tab_id
            && has_sidebar_title(pane, title)
    })
}

fn has_sidebar_title(pane: &PaneInfo, title: &str) -> bool {
    pane.label.as_deref() == Some(title)
}

fn sidebar_title(root: &Path) -> Result<String, ControllerError> {
    let name = root.file_name().unwrap_or(root.as_os_str());
    let title = sanitize_terminal(&name.to_string_lossy().to_uppercase());
    if title.is_empty() {
        return Err(ControllerError::InvalidContext(
            "workspace root has no displayable folder name".to_string(),
        ));
    }
    Ok(title)
}

impl SidebarIdentity {
    fn new(root: PathBuf) -> Result<Self, ControllerError> {
        let title = sidebar_title(&root)?;
        Ok(Self { root, title })
    }
}

fn opened_pane_id(response: &Value) -> Result<String, ControllerError> {
    response
        .pointer("/result/plugin_pane/pane/pane_id")
        .or_else(|| response.pointer("/result/pane/pane_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ControllerError::InvalidResponse(
                "plugin pane open did not return result.plugin_pane.pane.pane_id".to_string(),
            )
        })
}

fn editor_environment_argument(editor: Option<&OsStr>) -> Option<OsString> {
    let editor = editor.filter(|value| !value.is_empty())?;
    let mut argument = OsString::from("EDITOR=");
    argument.push(editor);
    Some(argument)
}

fn edge_pane(layout: &Value, tab_id: &str, side: DockSide) -> Option<String> {
    edge_pane_filtered(layout, tab_id, side, None)
}

fn edge_pane_excluding(
    layout: &Value,
    tab_id: &str,
    side: DockSide,
    excluded: &str,
) -> Option<String> {
    edge_pane_filtered(layout, tab_id, side, Some(excluded))
}

fn edge_pane_filtered(
    layout: &Value,
    tab_id: &str,
    side: DockSide,
    excluded: Option<&str>,
) -> Option<String> {
    if layout
        .pointer("/result/layout/tab_id")
        .and_then(Value::as_str)
        != Some(tab_id)
    {
        return None;
    }
    layout
        .pointer("/result/layout/panes")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|pane| {
            let pane_id = pane.get("pane_id")?.as_str()?;
            if excluded == Some(pane_id) {
                return None;
            }
            Some((
                pane.pointer("/rect/x")?.as_u64()?,
                pane.pointer("/rect/y")?.as_u64()?,
                pane.pointer("/rect/width")?.as_u64()?,
                pane_id.to_owned(),
            ))
        })
        .min_by_key(|(x, y, width, _)| match side {
            DockSide::Left => (*x, *y),
            DockSide::Right => (u64::MAX.saturating_sub(x.saturating_add(*width)), *y),
        })
        .map(|(_, _, _, pane_id)| pane_id)
}

fn pane_dock_side(layout: &Value, pane_id: &str) -> Option<DockSide> {
    let area = layout.pointer("/result/layout/area")?;
    let area_left = area.get("x")?.as_u64()?;
    let area_right = area_left.saturating_add(area.get("width")?.as_u64()?);
    let pane = layout
        .pointer("/result/layout/panes")
        .and_then(Value::as_array)?
        .iter()
        .find(|pane| pane.get("pane_id").and_then(Value::as_str) == Some(pane_id))?;
    let pane_left = pane.pointer("/rect/x")?.as_u64()?;
    let pane_right = pane_left.saturating_add(pane.pointer("/rect/width")?.as_u64()?);
    match (pane_left == area_left, pane_right == area_right) {
        (true, false) => Some(DockSide::Left),
        (false, true) => Some(DockSide::Right),
        _ => None,
    }
}

fn pane_width(layout: &Value, pane_id: &str) -> Option<u16> {
    layout
        .pointer("/result/layout/panes")
        .or_else(|| layout.pointer("/result/resize/layout/panes"))
        .or_else(|| layout.get("panes"))
        .and_then(Value::as_array)?
        .iter()
        .find(|pane| pane.get("pane_id").and_then(Value::as_str) == Some(pane_id))?
        .pointer("/rect/width")
        .and_then(Value::as_u64)
        .and_then(|width| u16::try_from(width).ok())
}

fn layout_area_width(layout: &Value) -> Option<u16> {
    layout
        .pointer("/result/layout/area/width")
        .or_else(|| layout.pointer("/result/resize/layout/area/width"))
        .or_else(|| layout.pointer("/area/width"))
        .and_then(Value::as_u64)
        .and_then(|width| u16::try_from(width).ok())
}

fn required_id<'a>(
    value: Option<&'a str>,
    field: &'static str,
    kind: &str,
) -> Result<&'a str, ControllerError> {
    let value = value.ok_or(ControllerError::MissingContext(field))?;
    validate_id(value, kind)?;
    Ok(value)
}

fn validate_id(id: &str, kind: &str) -> Result<(), ControllerError> {
    if valid_id(id) {
        return Ok(());
    }
    Err(ControllerError::InvalidContext(format!(
        "unsafe {kind} id {id:?}"
    )))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.'))
}

fn lock_path(state_path: &Path) -> PathBuf {
    state_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(ACTION_LOCK_FILE)
}

struct ActionLock {
    path: PathBuf,
    held: bool,
}

impl ActionLock {
    fn acquire(path: PathBuf) -> Result<Self, ControllerError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ControllerError::LockIo {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let started = std::time::Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id()).map_err(|source| {
                        ControllerError::LockIo {
                            path: path.clone(),
                            source,
                        }
                    })?;
                    return Ok(Self { path, held: true });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                        .is_some_and(|age| age >= STALE_LOCK_AGE);
                    if stale {
                        fs::remove_file(&path).map_err(|source| ControllerError::LockIo {
                            path: path.clone(),
                            source,
                        })?;
                        continue;
                    }
                    if started.elapsed() >= LOCK_WAIT {
                        return Err(ControllerError::LockTimeout(path));
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(source) => {
                    return Err(ControllerError::LockIo {
                        path: path.clone(),
                        source,
                    });
                }
            }
        }
    }

    fn release(&mut self) -> Result<(), ControllerError> {
        if !self.held {
            return Ok(());
        }
        match fs::remove_file(&self.path) {
            Ok(()) => {
                self.held = false;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.held = false;
                Ok(())
            }
            Err(source) => Err(ControllerError::LockIo {
                path: self.path.clone(),
                source,
            }),
        }
    }
}

impl Drop for ActionLock {
    fn drop(&mut self) {
        if self.held {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::FakeHerdr;
    use tempfile::TempDir;

    const TEST_SIDEBAR_TITLE: &str = "PROJECT";

    fn context(temp: &TempDir) -> InvocationContext {
        let project = temp.path().join("project");
        fs::create_dir_all(&project).expect("project directory");
        InvocationContext {
            workspace_id: Some("w1".to_string()),
            workspace_cwd: Some(project.to_string_lossy().into_owned()),
            tab_id: Some("w1:t1".to_string()),
            focused_pane_id: Some("w1:p1".to_string()),
            ..InvocationContext::default()
        }
    }

    fn pane_list_response(sidebar: bool) -> Value {
        let mut panes = vec![serde_json::json!({
            "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1",
            "focused": !sidebar, "cwd": "/tmp/project"
        })];
        if sidebar {
            panes.push(serde_json::json!({
                "pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1",
                "focused": true, "cwd": "/tmp/project", "label": TEST_SIDEBAR_TITLE
            }));
        }
        serde_json::json!({"result": {"panes": panes}})
    }

    fn layout_response(sidebar: bool) -> Value {
        let panes = if sidebar {
            vec![
                serde_json::json!({"pane_id":"w1:p2","focused":true,"rect":{"x":0,"y":0,"width":32,"height":40}}),
                serde_json::json!({"pane_id":"w1:p1","focused":false,"rect":{"x":32,"y":0,"width":68,"height":40}}),
            ]
        } else {
            vec![
                serde_json::json!({"pane_id":"w1:p1","focused":true,"rect":{"x":0,"y":0,"width":100,"height":40}}),
            ]
        };
        serde_json::json!({"result":{"layout":{"tab_id":"w1:t1","area":{"x":0,"y":0,"width":100,"height":40},"panes":panes}}})
    }

    fn resize_response(width: u16) -> Value {
        serde_json::json!({
            "result": {
                "resize": {
                    "layout": {
                        "area": {"x":0,"y":0,"width":100,"height":40},
                        "panes": [
                            {"pane_id":"w1:p2","rect":{"x":0,"y":0,"width":width,"height":40}},
                            {"pane_id":"w1:p1","rect":{"x":width,"y":0,"width":100-width,"height":40}}
                        ]
                    }
                }
            }
        })
    }

    fn left_layout_response(sidebar_width: u16) -> Value {
        serde_json::json!({
            "result":{"layout":{
                "tab_id":"w1:t1",
                "area":{"x":0,"y":0,"width":100,"height":40},
                "panes":[
                    {"pane_id":"w1:p2","focused":true,"rect":{"x":0,"y":0,"width":sidebar_width,"height":40}},
                    {"pane_id":"w1:p1","focused":false,"rect":{"x":sidebar_width,"y":0,"width":100-sidebar_width,"height":40}}
                ]
            }}
        })
    }

    fn right_layout_response(sidebar_width: u16) -> Value {
        let content_width = 100 - sidebar_width;
        serde_json::json!({
            "result":{"layout":{
                "tab_id":"w1:t1",
                "area":{"x":0,"y":0,"width":100,"height":40},
                "panes":[
                    {"pane_id":"w1:p1","focused":false,"rect":{"x":0,"y":0,"width":content_width,"height":40}},
                    {"pane_id":"w1:p2","focused":true,"rect":{"x":content_width,"y":0,"width":sidebar_width,"height":40}}
                ]
            }}
        })
    }

    fn right_resize_response(sidebar_width: u16) -> Value {
        let content_width = 100 - sidebar_width;
        serde_json::json!({
            "result": {
                "resize": {
                    "layout": {
                        "area": {"x":0,"y":0,"width":100,"height":40},
                        "panes": [
                            {"pane_id":"w1:p1","rect":{"x":0,"y":0,"width":content_width,"height":40}},
                            {"pane_id":"w1:p2","rect":{"x":content_width,"y":0,"width":sidebar_width,"height":40}}
                        ]
                    }
                }
            }
        })
    }

    #[test]
    fn show_opens_swaps_and_records_exact_owned_pane() {
        let temp = TempDir::new().expect("temp");
        let state_path = temp.path().join("state.json");
        let fake = FakeHerdr::new([
            Ok(pane_list_response(false)),
            Ok(layout_response(false)),
            Ok(serde_json::json!({"result":{"plugin_pane":{"pane":{"pane_id":"w1:p2"}}}})),
            Ok(serde_json::json!({"result":{}})),
            Ok(serde_json::json!({"result":{}})),
            Ok(layout_response(true)),
        ]);
        let controller =
            Controller::new(fake, context(&temp), state_path.clone(), Config::default());
        controller.execute(Action::Show).expect("show");
        let state = PersistedState::load(&state_path).expect("state");
        assert_eq!(
            state
                .tab("w1", "w1:t1")
                .and_then(|tab| tab.pane_id.as_deref()),
            Some("w1:p2")
        );
        assert!(state.tab("w1", "w1:t1").expect("tab").visible);
        let calls = controller.herdr.calls();
        let open_args = calls[2]
            .1
            .as_array()
            .expect("argv")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert!(
            !open_args.contains(&"--cwd"),
            "the plugin binary must start from HERDR_PLUGIN_ROOT; workspace identity comes from context"
        );
        assert_eq!(
            calls[3].1,
            serde_json::json!(["pane", "rename", "w1:p2", TEST_SIDEBAR_TITLE])
        );
    }

    #[test]
    fn editor_environment_is_forwarded_without_shell_interpolation() {
        assert_eq!(
            editor_environment_argument(Some(OsStr::new("nvim --cmd 'set number'"))),
            Some(OsString::from("EDITOR=nvim --cmd 'set number'"))
        );
        assert_eq!(editor_environment_argument(None), None);
        assert_eq!(editor_environment_argument(Some(OsStr::new(""))), None);
    }

    #[test]
    fn show_restores_the_saved_right_dock_without_a_swap() {
        let temp = TempDir::new().expect("temp");
        let state_path = temp.path().join("state.json");
        let mut state = PersistedState::default();
        state.tab_mut("w1", "w1:t1").dock_side = DockSide::Right;
        state.save_atomic(&state_path).expect("save");
        let fake = FakeHerdr::new([
            Ok(pane_list_response(false)),
            Ok(layout_response(false)),
            Ok(serde_json::json!({"result":{"plugin_pane":{"pane":{"pane_id":"w1:p2"}}}})),
            Ok(serde_json::json!({"result":{}})),
            Ok(right_layout_response(32)),
        ]);
        let controller =
            Controller::new(fake, context(&temp), state_path.clone(), Config::default());

        controller.execute(Action::Show).expect("show on right");

        let state = PersistedState::load(&state_path).expect("state");
        assert_eq!(
            state.tab("w1", "w1:t1").expect("tab").dock_side,
            DockSide::Right
        );
        let calls = controller.herdr.calls();
        let open_args = calls[2]
            .1
            .as_array()
            .expect("argv")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(
            open_args
                .windows(2)
                .find(|pair| pair[0] == "--target-pane")
                .map(|pair| pair[1]),
            Some("w1:p1")
        );
        assert!(
            calls
                .iter()
                .all(|call| call.1.get(1).and_then(Value::as_str) != Some("swap"))
        );
    }

    #[test]
    fn dock_toggle_moves_to_each_edge_and_persists_the_preference() {
        let temp = TempDir::new().expect("temp");
        let state_path = temp.path().join("state.json");
        let mut state = PersistedState::default();
        let tab = state.tab_mut("w1", "w1:t1");
        tab.visible = true;
        tab.pane_id = Some("w1:p2".to_string());
        tab.dock_side = DockSide::Left;
        state.save_atomic(&state_path).expect("save");
        let fake = FakeHerdr::new([
            Ok(pane_list_response(true)),
            Ok(left_layout_response(32)),
            Ok(serde_json::json!({"result":{"swap":{"changed":true}}})),
            Ok(right_layout_response(68)),
            Ok(right_resize_response(32)),
        ]);
        let controller =
            Controller::new(fake, context(&temp), state_path.clone(), Config::default());

        controller
            .execute(Action::ToggleDock)
            .expect("dock on right");

        let state = PersistedState::load(&state_path).expect("right state");
        assert_eq!(
            state.tab("w1", "w1:t1").expect("tab").dock_side,
            DockSide::Right
        );
        let calls = controller.herdr.calls();
        assert_eq!(
            calls[2].1,
            serde_json::json!([
                "pane",
                "swap",
                "--source-pane",
                "w1:p2",
                "--target-pane",
                "w1:p1"
            ])
        );
        assert_eq!(calls[4].1[5], "right");

        let fake = FakeHerdr::new([
            Ok(pane_list_response(true)),
            Ok(right_layout_response(32)),
            Ok(serde_json::json!({"result":{"swap":{"changed":true}}})),
            Ok(left_layout_response(68)),
            Ok(resize_response(32)),
        ]);
        let controller =
            Controller::new(fake, context(&temp), state_path.clone(), Config::default());

        controller
            .execute(Action::ToggleDock)
            .expect("dock on left");

        let state = PersistedState::load(&state_path).expect("left state");
        assert_eq!(
            state.tab("w1", "w1:t1").expect("tab").dock_side,
            DockSide::Left
        );
        assert_eq!(controller.herdr.calls()[4].1[5], "left");
    }

    #[test]
    fn failed_dock_swap_does_not_change_the_saved_preference() {
        let temp = TempDir::new().expect("temp");
        let state_path = temp.path().join("state.json");
        let mut state = PersistedState::default();
        let tab = state.tab_mut("w1", "w1:t1");
        tab.visible = true;
        tab.pane_id = Some("w1:p2".to_string());
        state.save_atomic(&state_path).expect("save");
        let fake = FakeHerdr::new([
            Ok(pane_list_response(true)),
            Ok(left_layout_response(32)),
            Ok(serde_json::json!({
                "result":{"swap":{"changed":false,"reason":"not_found"}}
            })),
        ]);
        let controller =
            Controller::new(fake, context(&temp), state_path.clone(), Config::default());

        assert!(matches!(
            controller.execute(Action::ToggleDock),
            Err(ControllerError::InvalidResponse(message))
                if message.contains("not_found")
        ));
        assert_eq!(
            PersistedState::load(&state_path)
                .expect("state")
                .tab("w1", "w1:t1")
                .expect("tab")
                .dock_side,
            DockSide::Left
        );
    }

    #[test]
    fn repeated_show_focuses_registered_pane_without_opening() {
        let temp = TempDir::new().expect("temp");
        let state_path = temp.path().join("state.json");
        let mut state = PersistedState::default();
        let tab = state.tab_mut("w1", "w1:t1");
        tab.visible = true;
        tab.pane_id = Some("w1:p2".to_string());
        state.save_atomic(&state_path).expect("save");
        let fake = FakeHerdr::new([
            Ok(pane_list_response(true)),
            Ok(serde_json::json!({"result":{}})),
            Ok(serde_json::json!({"result":{}})),
        ]);
        let controller = Controller::new(fake, context(&temp), state_path, Config::default());
        controller.execute(Action::Show).expect("show");
        let calls = controller.herdr.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(
            calls[1].1,
            serde_json::json!(["pane", "zoom", "w1:p2", "--on"])
        );
        assert_eq!(
            calls[2].1,
            serde_json::json!(["pane", "zoom", "w1:p2", "--off"])
        );
    }

    #[test]
    fn unsafe_host_id_is_rejected_before_argv() {
        let temp = TempDir::new().expect("temp");
        let mut context = context(&temp);
        context.tab_id = Some("--help".to_string());
        let controller = Controller::new(
            FakeHerdr::new([]),
            context,
            temp.path().join("state.json"),
            Config::default(),
        );
        assert!(matches!(
            controller.execute(Action::Show),
            Err(ControllerError::InvalidContext(_))
        ));
    }

    #[test]
    fn sidebar_process_requires_exact_managed_plugin_context() {
        let socket = OsStr::new("/tmp/herdr.sock");
        assert!(
            validate_sidebar_process_context(
                Some("1"),
                Some(socket),
                Some(PLUGIN_ID),
                Some(SIDEBAR_ENTRYPOINT)
            )
            .is_ok()
        );
        for invalid in [
            validate_sidebar_process_context(
                None,
                Some(socket),
                Some(PLUGIN_ID),
                Some(SIDEBAR_ENTRYPOINT),
            ),
            validate_sidebar_process_context(
                Some("1"),
                None,
                Some(PLUGIN_ID),
                Some(SIDEBAR_ENTRYPOINT),
            ),
            validate_sidebar_process_context(
                Some("1"),
                Some(socket),
                Some("another-plugin"),
                Some(SIDEBAR_ENTRYPOINT),
            ),
            validate_sidebar_process_context(
                Some("1"),
                Some(socket),
                Some(PLUGIN_ID),
                Some("preview"),
            ),
        ] {
            assert!(matches!(invalid, Err(ControllerError::InvalidContext(_))));
        }
    }

    #[test]
    fn hide_persists_width_closes_owned_pane_and_returns_focus() {
        let temp = TempDir::new().expect("temp");
        let state_path = temp.path().join("state.json");
        let mut state = PersistedState::default();
        let tab = state.tab_mut("w1", "w1:t1");
        tab.visible = true;
        tab.pane_id = Some("w1:p2".to_string());
        tab.previous_pane_id = Some("w1:p1".to_string());
        state.save_atomic(&state_path).expect("save");
        let fake = FakeHerdr::new([
            Ok(pane_list_response(true)),
            Ok(layout_response(true)),
            Ok(serde_json::json!({"result":{}})),
            Ok(serde_json::json!({"result":{}})),
            Ok(serde_json::json!({"result":{}})),
        ]);
        let controller =
            Controller::new(fake, context(&temp), state_path.clone(), Config::default());
        controller.execute(Action::Hide).expect("hide");

        let state = PersistedState::load(&state_path).expect("state");
        let tab = state.tab("w1", "w1:t1").expect("tab");
        assert!(!tab.visible);
        assert_eq!(tab.pane_id, None);
        assert_eq!(tab.width, 32);
        let calls = controller.herdr.calls();
        assert_eq!(calls[2].1, serde_json::json!(["pane", "close", "w1:p2"]));
        assert_eq!(
            calls[3].1,
            serde_json::json!(["pane", "zoom", "w1:p1", "--on"])
        );
        assert_eq!(
            calls[4].1,
            serde_json::json!(["pane", "zoom", "w1:p1", "--off"])
        );
    }

    #[test]
    fn hide_closes_a_registered_sidebar_even_when_it_is_the_only_pane() {
        let temp = TempDir::new().expect("temp");
        let state_path = temp.path().join("state.json");
        let mut state = PersistedState::default();
        let tab = state.tab_mut("w1", "w1:t1");
        tab.visible = true;
        tab.pane_id = Some("w1:p2".to_string());
        state.save_atomic(&state_path).expect("save");
        let only_sidebar = serde_json::json!({
            "result": {"panes": [{
                "pane_id": "w1:p2",
                "workspace_id": "w1",
                "tab_id": "w1:t1",
                "focused": true,
                "cwd": temp.path(),
                "label": TEST_SIDEBAR_TITLE
            }]}
        });
        let only_sidebar_layout = serde_json::json!({
            "result": {"layout": {
                "tab_id": "w1:t1",
                "area": {"x": 0, "y": 0, "width": 100, "height": 40},
                "panes": [{
                    "pane_id": "w1:p2",
                    "focused": true,
                    "rect": {"x": 0, "y": 0, "width": 100, "height": 40}
                }]
            }}
        });
        let fake = FakeHerdr::new([
            Ok(only_sidebar),
            Ok(only_sidebar_layout),
            Ok(serde_json::json!({"result":{}})),
        ]);
        let controller =
            Controller::new(fake, context(&temp), state_path.clone(), Config::default());
        controller.execute(Action::Hide).expect("hide");

        let state = PersistedState::load(&state_path).expect("state");
        assert!(!state.tab("w1", "w1:t1").expect("tab").visible);
        let calls = controller.herdr.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[2].1, serde_json::json!(["pane", "close", "w1:p2"]));
    }

    #[test]
    fn toggle_hides_and_focus_without_owned_pane_behaves_like_show() {
        let temp = TempDir::new().expect("temp");
        let hidden_path = temp.path().join("hidden.json");
        let fake = FakeHerdr::new([
            Ok(pane_list_response(false)),
            Ok(layout_response(false)),
            Ok(serde_json::json!({"result":{"plugin_pane":{"pane":{"pane_id":"w1:p2"}}}})),
            Ok(serde_json::json!({"result":{}})),
            Ok(serde_json::json!({"result":{}})),
            Ok(layout_response(true)),
        ]);
        let controller =
            Controller::new(fake, context(&temp), hidden_path.clone(), Config::default());
        controller
            .execute(Action::Focus)
            .expect("focus behaves as show");
        assert!(
            PersistedState::load(&hidden_path)
                .unwrap()
                .tab("w1", "w1:t1")
                .unwrap()
                .visible
        );

        let toggle_path = temp.path().join("toggle.json");
        let mut state = PersistedState::default();
        let tab = state.tab_mut("w1", "w1:t1");
        tab.visible = true;
        tab.pane_id = Some("w1:p2".to_string());
        state.save_atomic(&toggle_path).unwrap();
        let fake = FakeHerdr::new([
            Ok(pane_list_response(true)),
            Ok(layout_response(true)),
            Ok(serde_json::json!({"result":{}})),
        ]);
        let controller =
            Controller::new(fake, context(&temp), toggle_path.clone(), Config::default());
        controller.execute(Action::Toggle).expect("toggle hide");
        assert!(
            !PersistedState::load(&toggle_path)
                .unwrap()
                .tab("w1", "w1:t1")
                .unwrap()
                .visible
        );
    }

    #[test]
    fn restore_is_idempotent_and_rejects_unregistered_duplicate_labels() {
        let temp = TempDir::new().expect("temp");
        let state_path = temp.path().join("state.json");
        let mut state = PersistedState::default();
        let tab = state.tab_mut("w1", "w1:t1");
        tab.visible = true;
        tab.pane_id = Some("w1:p2".to_string());
        state.save_atomic(&state_path).unwrap();
        let fake = FakeHerdr::new([Ok(pane_list_response(true))]);
        let controller =
            Controller::new(fake, context(&temp), state_path.clone(), Config::default());
        controller
            .restore()
            .expect("owned pane is already restored");
        assert_eq!(controller.herdr.calls().len(), 1);

        let mut stale = PersistedState::load(&state_path).unwrap();
        stale.tab_mut("w1", "w1:t1").pane_id = Some("w1:stale".to_string());
        stale.save_atomic(&state_path).unwrap();
        let fake = FakeHerdr::new([Ok(pane_list_response(true))]);
        let controller = Controller::new(fake, context(&temp), state_path, Config::default());
        assert!(matches!(
            controller.restore(),
            Err(ControllerError::InvalidResponse(message))
                if message.contains("refusing to guess ownership")
        ));
    }

    #[test]
    fn action_lock_serializes_concurrent_controllers() {
        let temp = TempDir::new().expect("temp");
        let path = temp.path().join(ACTION_LOCK_FILE);
        let first = ActionLock::acquire(path.clone()).expect("first lock");
        let (sender, receiver) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let second = ActionLock::acquire(path).expect("second lock");
            sender.send(()).unwrap();
            drop(second);
        });
        assert!(
            receiver.recv_timeout(Duration::from_millis(100)).is_err(),
            "second action must wait"
        );
        drop(first);
        receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("second action proceeds after first");
        waiter.join().unwrap();
    }

    #[test]
    fn failed_close_rolls_visible_state_back() {
        let temp = TempDir::new().expect("temp");
        let state_path = temp.path().join("state.json");
        let mut state = PersistedState::default();
        let tab = state.tab_mut("w1", "w1:t1");
        tab.visible = true;
        tab.pane_id = Some("w1:p2".to_string());
        state.save_atomic(&state_path).unwrap();
        let fake = FakeHerdr::new([
            Ok(pane_list_response(true)),
            Ok(layout_response(true)),
            Err(HerdrError::Api {
                code: "close_failed".to_string(),
                message: "fixture".to_string(),
            }),
        ]);
        let controller =
            Controller::new(fake, context(&temp), state_path.clone(), Config::default());
        assert!(controller.execute(Action::Hide).is_err());
        let state = PersistedState::load(&state_path).unwrap();
        let tab = state.tab("w1", "w1:t1").unwrap();
        assert!(tab.visible);
        assert_eq!(tab.pane_id.as_deref(), Some("w1:p2"));
    }

    #[test]
    fn resize_feedback_corrects_a_rounding_overshoot() {
        let temp = TempDir::new().expect("temp");
        let fake = FakeHerdr::new([Ok(resize_response(31)), Ok(resize_response(32))]);
        let controller = Controller::new(
            fake,
            context(&temp),
            temp.path().join("state.json"),
            Config::default(),
        );
        let initial = resize_response(33);
        controller
            .resize_to_width("w1:p2", 32, DockSide::Left, &initial)
            .expect("resize");
        let calls = controller.herdr.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1[5], "left");
        assert_eq!(calls[1].1[5], "right");
    }
}
