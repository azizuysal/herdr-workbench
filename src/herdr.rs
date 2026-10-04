use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug)]
pub enum HerdrError {
    Io(io::Error),
    Failed {
        argv: Vec<OsString>,
        status: Option<i32>,
        stderr: String,
    },
    InvalidJson {
        context: String,
        source: serde_json::Error,
    },
    InvalidResponse(String),
    Api {
        code: String,
        message: String,
    },
    MissingSocket,
    EmptyResponse,
}

impl fmt::Display for HerdrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Failed {
                argv,
                status,
                stderr,
            } => write!(
                formatter,
                "Herdr command {:?} failed with status {:?}: {}",
                argv,
                status,
                stderr.trim()
            ),
            Self::InvalidJson { context, source } => {
                write!(formatter, "invalid Herdr JSON from {context}: {source}")
            }
            Self::InvalidResponse(message) => {
                write!(formatter, "invalid Herdr response: {message}")
            }
            Self::Api { code, message } => write!(formatter, "Herdr API {code}: {message}"),
            Self::MissingSocket => write!(
                formatter,
                "HERDR_SOCKET_PATH is unavailable; run this command inside Herdr"
            ),
            Self::EmptyResponse => write!(formatter, "Herdr API returned an empty response"),
        }
    }
}

impl std::error::Error for HerdrError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidJson { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for HerdrError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub trait CommandRunner: Send + Sync {
    fn output(&self, program: &OsStr, args: &[OsString]) -> io::Result<Output>;
}

#[derive(Debug, Default)]
pub struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn output(&self, program: &OsStr, args: &[OsString]) -> io::Result<Output> {
        Command::new(program).args(args).output()
    }
}

pub trait HerdrClient: Send + Sync {
    fn command(&self, args: &[OsString]) -> Result<Value, HerdrError>;
    fn request(&self, method: &str, params: Value) -> Result<Value, HerdrError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompanionEvent {
    SidebarBecameOnlyPane,
    Directory { pane_id: String, cwd: PathBuf },
    Error(String),
}

pub struct CompanionMonitor {
    receiver: mpsc::Receiver<CompanionEvent>,
    stopped: Arc<AtomicBool>,
}

impl CompanionMonitor {
    pub fn from_env(
        workspace_id: &str,
        tab_id: &str,
        pane_id: &str,
        source_pane: Option<String>,
    ) -> Result<Self, HerdrError> {
        let socket_path = std::env::var_os("HERDR_SOCKET_PATH")
            .filter(|value| !value.is_empty())
            .ok_or(HerdrError::MissingSocket)?;
        Self::start(&socket_path, workspace_id, tab_id, pane_id, source_pane)
    }

    pub fn try_recv(&self) -> Result<CompanionEvent, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }

    #[cfg(test)]
    pub fn inactive() -> Self {
        let (_sender, receiver) = mpsc::channel();
        Self {
            receiver,
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(unix)]
    fn start(
        socket_path: &OsStr,
        workspace_id: &str,
        tab_id: &str,
        pane_id: &str,
        source_pane: Option<String>,
    ) -> Result<Self, HerdrError> {
        let reader = connect_layout_subscription(socket_path)?;
        let (sender, receiver) = mpsc::channel();
        let socket_path = socket_path.to_owned();
        let workspace_id = workspace_id.to_owned();
        let tab_id = tab_id.to_owned();
        let pane_id = pane_id.to_owned();
        let stopped = Arc::new(AtomicBool::new(false));
        let task_stopped = Arc::clone(&stopped);
        thread::spawn(move || {
            let mut follower = CompanionFollower {
                workspace_id,
                tab_id,
                sidebar_id: pane_id,
                source_pane,
            };
            monitor_companion(reader, &socket_path, &mut follower, &sender, &task_stopped);
        });
        Ok(Self { receiver, stopped })
    }

    #[cfg(not(unix))]
    fn start(
        _socket_path: &OsStr,
        _workspace_id: &str,
        _tab_id: &str,
        _pane_id: &str,
        _source_pane: Option<String>,
    ) -> Result<Self, HerdrError> {
        Err(HerdrError::MissingSocket)
    }
}

impl Drop for CompanionMonitor {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

struct CompanionFollower {
    workspace_id: String,
    tab_id: String,
    sidebar_id: String,
    source_pane: Option<String>,
}

impl CompanionFollower {
    fn directory(
        &mut self,
        panes: &[PaneInfo],
        focused: Option<&str>,
    ) -> Result<Option<CompanionEvent>, HerdrError> {
        let eligible = |pane: &&PaneInfo| {
            pane.workspace_id == self.workspace_id
                && pane.tab_id == self.tab_id
                && pane.pane_id != self.sidebar_id
        };
        let pane = panes
            .iter()
            .filter(eligible)
            .find(|pane| pane.focused)
            .or_else(|| {
                panes
                    .iter()
                    .filter(eligible)
                    .find(|pane| Some(pane.pane_id.as_str()) == focused)
            })
            .or_else(|| {
                panes
                    .iter()
                    .filter(eligible)
                    .find(|pane| Some(&pane.pane_id) == self.source_pane.as_ref())
            })
            .or_else(|| panes.iter().find(eligible));
        let Some(pane) = pane else {
            return Ok(None);
        };
        self.source_pane = Some(pane.pane_id.clone());
        let cwd = pane
            .cwd
            .as_deref()
            .or(pane.foreground_cwd.as_deref())
            .filter(|cwd| !cwd.is_empty())
            .ok_or_else(|| {
                HerdrError::InvalidResponse(format!(
                    "pane {} has no current directory",
                    pane.pane_id
                ))
            })?;
        Ok(Some(CompanionEvent::Directory {
            pane_id: pane.pane_id.clone(),
            cwd: PathBuf::from(cwd),
        }))
    }
}

pub struct LiveHerdr<R: CommandRunner = ProcessRunner> {
    program: OsString,
    socket_path: Option<OsString>,
    runner: R,
}

impl LiveHerdr<ProcessRunner> {
    pub fn from_env() -> Self {
        Self {
            program: std::env::var_os("HERDR_BIN_PATH")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "herdr".into()),
            socket_path: std::env::var_os("HERDR_SOCKET_PATH").filter(|value| !value.is_empty()),
            runner: ProcessRunner,
        }
    }
}

impl<R: CommandRunner> LiveHerdr<R> {
    pub fn with_runner(
        program: impl Into<OsString>,
        socket_path: Option<OsString>,
        runner: R,
    ) -> Self {
        Self {
            program: program.into(),
            socket_path,
            runner,
        }
    }

    fn command_output(&self, args: &[OsString]) -> Result<Output, HerdrError> {
        let output = self.runner.output(&self.program, args)?;
        if output.status.success() {
            return Ok(output);
        }
        Err(HerdrError::Failed {
            argv: std::iter::once(self.program.clone())
                .chain(args.iter().cloned())
                .collect(),
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

impl<R: CommandRunner> HerdrClient for LiveHerdr<R> {
    fn command(&self, args: &[OsString]) -> Result<Value, HerdrError> {
        let output = self.command_output(args)?;
        serde_json::from_slice(&output.stdout).map_err(|source| HerdrError::InvalidJson {
            context: format!("{:?}", args),
            source,
        })
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, HerdrError> {
        let socket_path = self
            .socket_path
            .as_deref()
            .ok_or(HerdrError::MissingSocket)?;
        socket_request(socket_path, method, params)
    }
}

#[cfg(unix)]
fn socket_request(socket_path: &OsStr, method: &str, params: Value) -> Result<Value, HerdrError> {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(std::path::Path::new(socket_path))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let request = serde_json::json!({
        "id": format!("herdr-workbench:{}", std::process::id()),
        "method": method,
        "params": params,
    });
    serde_json::to_writer(&mut stream, &request).map_err(|source| HerdrError::InvalidJson {
        context: "socket request encoding".to_string(),
        source,
    })?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    if line.is_empty() {
        return Err(HerdrError::EmptyResponse);
    }
    let response: ApiEnvelope =
        serde_json::from_str(&line).map_err(|source| HerdrError::InvalidJson {
            context: method.to_string(),
            source,
        })?;
    if let Some(error) = response.error {
        return Err(HerdrError::Api {
            code: error.code,
            message: error.message,
        });
    }
    response.result.ok_or(HerdrError::EmptyResponse)
}

#[cfg(unix)]
fn connect_layout_subscription(
    socket_path: &OsStr,
) -> Result<BufReader<std::os::unix::net::UnixStream>, HerdrError> {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(std::path::Path::new(socket_path))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let request = serde_json::json!({
        "id": format!("herdr-workbench-layouts:{}", std::process::id()),
        "method": "events.subscribe",
        "params": {
            "subscriptions": [{"type": "layout.updated"}, {"type": "pane.updated"}, {"type": "pane.focused"}]
        },
    });
    serde_json::to_writer(&mut stream, &request).map_err(|source| HerdrError::InvalidJson {
        context: "layout subscription encoding".to_string(),
        source,
    })?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.is_empty() {
        return Err(HerdrError::EmptyResponse);
    }
    let response: ApiEnvelope =
        serde_json::from_str(&line).map_err(|source| HerdrError::InvalidJson {
            context: "events.subscribe".to_string(),
            source,
        })?;
    if let Some(error) = response.error {
        return Err(HerdrError::Api {
            code: error.code,
            message: error.message,
        });
    }
    if response.result.is_none() {
        return Err(HerdrError::EmptyResponse);
    }
    reader
        .get_mut()
        .set_read_timeout(Some(Duration::from_millis(500)))?;
    Ok(reader)
}

#[cfg(unix)]
fn monitor_companion(
    mut reader: BufReader<std::os::unix::net::UnixStream>,
    socket_path: &OsStr,
    follower: &mut CompanionFollower,
    sender: &mpsc::Sender<CompanionEvent>,
    stopped: &AtomicBool,
) {
    let mut line = String::new();
    let mut refresh = true;
    let mut focused = None;
    let mut last_refresh = Instant::now();
    while !stopped.load(Ordering::Relaxed) {
        if refresh || last_refresh.elapsed() >= Duration::from_millis(500) {
            let result = socket_request(
                socket_path,
                "pane.list",
                serde_json::json!({"workspace_id": follower.workspace_id}),
            )
            .and_then(|value| pane_list(&value))
            .and_then(|panes| follower.directory(&panes, focused.as_deref()));
            let event = match result {
                Ok(event) => event,
                Err(error) => Some(CompanionEvent::Error(error.to_string())),
            };
            if let Some(event) = event
                && sender.send(event).is_err()
            {
                return;
            }
            focused = None;
            refresh = false;
            last_refresh = Instant::now();
        }
        match reader.read_line(&mut line) {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Ok(0) | Err(_) => loop {
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                if sender
                    .send(CompanionEvent::Error(
                        "Herdr connection lost; reconnecting".to_string(),
                    ))
                    .is_err()
                {
                    return;
                }
                thread::sleep(Duration::from_millis(250));
                let Ok(next_reader) = connect_layout_subscription(socket_path) else {
                    continue;
                };
                let Ok(layout) = socket_request(
                    socket_path,
                    "pane.layout",
                    serde_json::json!({"pane_id": follower.sidebar_id}),
                ) else {
                    continue;
                };
                match layout_has_companion(
                    &layout,
                    &follower.workspace_id,
                    &follower.tab_id,
                    &follower.sidebar_id,
                ) {
                    Ok(false) => {
                        let _ = sender.send(CompanionEvent::SidebarBecameOnlyPane);
                        return;
                    }
                    Ok(true) => {
                        reader = next_reader;
                        line.clear();
                        refresh = true;
                        break;
                    }
                    Err(_) => continue,
                }
            },
            Ok(_) => {
                let event = match serde_json::from_str::<Value>(&line) {
                    Ok(event) => event,
                    Err(error) => {
                        line.clear();
                        if sender
                            .send(CompanionEvent::Error(format!(
                                "invalid Herdr event: {error}"
                            )))
                            .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                };
                line.clear();
                if layout_event_leaves_sidebar_alone(
                    &event,
                    &follower.workspace_id,
                    &follower.tab_id,
                    &follower.sidebar_id,
                ) {
                    let _ = sender.send(CompanionEvent::SidebarBecameOnlyPane);
                    return;
                }
                if event.get("event").and_then(Value::as_str) == Some("pane_focused")
                    && event.pointer("/data/workspace_id").and_then(Value::as_str)
                        == Some(&follower.workspace_id)
                {
                    focused = event
                        .pointer("/data/pane_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    refresh = true;
                } else if event
                    .pointer("/data/pane/workspace_id")
                    .and_then(Value::as_str)
                    == Some(&follower.workspace_id)
                    && event.pointer("/data/pane/tab_id").and_then(Value::as_str)
                        == Some(&follower.tab_id)
                    && event.pointer("/data/pane/pane_id").and_then(Value::as_str)
                        != Some(&follower.sidebar_id)
                {
                    refresh = true;
                }
            }
        }
    }
}

pub fn layout_has_companion(
    value: &Value,
    workspace_id: &str,
    tab_id: &str,
    pane_id: &str,
) -> Result<bool, HerdrError> {
    let layout = value
        .get("layout")
        .or_else(|| value.pointer("/result/layout"))
        .ok_or_else(|| HerdrError::InvalidResponse("pane.layout omitted layout".to_string()))?;
    if layout.get("workspace_id").and_then(Value::as_str) != Some(workspace_id)
        || layout.get("tab_id").and_then(Value::as_str) != Some(tab_id)
    {
        return Err(HerdrError::InvalidResponse(
            "pane.layout returned a different workspace or tab".to_string(),
        ));
    }
    let panes = layout
        .get("panes")
        .and_then(Value::as_array)
        .ok_or_else(|| HerdrError::InvalidResponse("pane.layout omitted panes".to_string()))?;
    if !panes
        .iter()
        .any(|pane| pane.get("pane_id").and_then(Value::as_str) == Some(pane_id))
    {
        return Err(HerdrError::InvalidResponse(
            "pane.layout does not contain the sidebar pane".to_string(),
        ));
    }
    Ok(panes.iter().any(|pane| {
        pane.get("pane_id")
            .and_then(Value::as_str)
            .is_some_and(|candidate| candidate != pane_id)
    }))
}

fn layout_event_leaves_sidebar_alone(
    value: &Value,
    workspace_id: &str,
    tab_id: &str,
    pane_id: &str,
) -> bool {
    if value.get("event").and_then(Value::as_str) != Some("layout_updated") {
        return false;
    }
    let Some(layout) = value.pointer("/data/layout") else {
        return false;
    };
    if layout.get("workspace_id").and_then(Value::as_str) != Some(workspace_id)
        || layout.get("tab_id").and_then(Value::as_str) != Some(tab_id)
    {
        return false;
    }
    let Some(panes) = layout.get("panes").and_then(Value::as_array) else {
        return false;
    };
    panes.len() == 1 && panes[0].get("pane_id").and_then(Value::as_str) == Some(pane_id)
}

#[cfg(not(unix))]
fn socket_request(
    _socket_path: &OsStr,
    _method: &str,
    _params: Value,
) -> Result<Value, HerdrError> {
    Err(HerdrError::MissingSocket)
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope {
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    code: String,
    message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvocationContext {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub workspace_label: Option<String>,
    #[serde(default)]
    pub workspace_cwd: Option<String>,
    #[serde(default)]
    pub tab_id: Option<String>,
    #[serde(default)]
    pub tab_label: Option<String>,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    #[serde(default)]
    pub focused_pane_cwd: Option<String>,
    #[serde(default)]
    pub invocation_source: Option<String>,
}

impl InvocationContext {
    pub fn from_env() -> Result<Self, HerdrError> {
        match std::env::var("HERDR_PLUGIN_CONTEXT_JSON") {
            Ok(json) if !json.trim().is_empty() => {
                serde_json::from_str(&json).map_err(|source| HerdrError::InvalidJson {
                    context: "HERDR_PLUGIN_CONTEXT_JSON".to_string(),
                    source,
                })
            }
            _ => Ok(Self {
                workspace_id: std::env::var("HERDR_WORKSPACE_ID").ok(),
                tab_id: std::env::var("HERDR_TAB_ID").ok(),
                focused_pane_id: std::env::var("HERDR_PANE_ID").ok(),
                ..Self::default()
            }),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PaneInfo {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

pub fn pane_list(value: &Value) -> Result<Vec<PaneInfo>, HerdrError> {
    let panes = value
        .get("panes")
        .or_else(|| value.pointer("/result/panes"))
        .cloned()
        .ok_or_else(|| HerdrError::InvalidResponse("pane.list omitted panes".to_string()))?;
    serde_json::from_value(panes).map_err(|source| HerdrError::InvalidJson {
        context: "pane list".to_string(),
        source,
    })
}

pub fn command_args(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

pub struct FakeHerdr {
    responses: Mutex<VecDeque<Result<Value, HerdrError>>>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl FakeHerdr {
    pub fn new(responses: impl IntoIterator<Item = Result<Value, HerdrError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("fake calls lock").clone()
    }

    fn next(&self) -> Result<Value, HerdrError> {
        self.responses
            .lock()
            .expect("fake responses lock")
            .pop_front()
            .unwrap_or_else(|| Ok(serde_json::json!({"result": {}})))
    }
}

impl HerdrClient for FakeHerdr {
    fn command(&self, args: &[OsString]) -> Result<Value, HerdrError> {
        self.calls.lock().expect("fake calls lock").push((
            "command".to_string(),
            Value::Array(
                args.iter()
                    .map(|arg| Value::String(arg.to_string_lossy().into_owned()))
                    .collect(),
            ),
        ));
        self.next()
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, HerdrError> {
        self.calls
            .lock()
            .expect("fake calls lock")
            .push((method.to_string(), params));
        self.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(
        pane_id: &str,
        workspace_id: &str,
        tab_id: &str,
        focused: bool,
        cwd: Option<&str>,
        foreground_cwd: Option<&str>,
    ) -> PaneInfo {
        PaneInfo {
            pane_id: pane_id.to_string(),
            workspace_id: workspace_id.to_string(),
            tab_id: tab_id.to_string(),
            focused,
            cwd: cwd.map(str::to_string),
            foreground_cwd: foreground_cwd.map(str::to_string),
            label: None,
        }
    }

    fn follower(source_pane: Option<&str>) -> CompanionFollower {
        CompanionFollower {
            workspace_id: "w1".to_string(),
            tab_id: "w1:t1".to_string(),
            sidebar_id: "w1:sidebar".to_string(),
            source_pane: source_pane.map(str::to_string),
        }
    }

    fn directory(event: Option<CompanionEvent>) -> (String, PathBuf) {
        match event.expect("directory event") {
            CompanionEvent::Directory { pane_id, cwd } => (pane_id, cwd),
            event => panic!("expected directory event, got {event:?}"),
        }
    }

    #[test]
    fn companion_follower_switches_to_the_focused_terminal_in_its_tab() {
        let mut follower = follower(Some("w1:p1"));
        let panes = vec![
            pane("w1:p1", "w1", "w1:t1", false, Some("/first"), None),
            pane("w1:p2", "w1", "w1:t1", true, Some("/second"), None),
        ];

        assert_eq!(
            directory(follower.directory(&panes, Some("w1:p2")).unwrap()),
            ("w1:p2".to_string(), PathBuf::from("/second"))
        );
        assert_eq!(follower.source_pane.as_deref(), Some("w1:p2"));
    }

    #[test]
    fn companion_follower_keeps_its_last_terminal_for_sidebar_and_other_tabs() {
        let mut follower = follower(Some("w1:p1"));
        let panes = vec![
            pane("w1:p1", "w1", "w1:t1", false, Some("/project"), None),
            pane("w1:sidebar", "w1", "w1:t1", true, Some("/plugin"), None),
            pane("w1:p3", "w1", "w1:t2", true, Some("/other-tab"), None),
            pane("w2:p1", "w2", "w2:t1", true, Some("/other-workspace"), None),
        ];

        assert_eq!(
            directory(follower.directory(&panes, Some("w1:sidebar")).unwrap()),
            ("w1:p1".to_string(), PathBuf::from("/project"))
        );
    }

    #[test]
    fn companion_follower_uses_the_initial_source_before_an_arbitrary_terminal() {
        let mut follower = follower(Some("w1:p2"));
        let panes = vec![
            pane("w1:p1", "w1", "w1:t1", false, Some("/first"), None),
            pane("w1:p2", "w1", "w1:t1", false, Some("/preferred"), None),
        ];

        assert_eq!(
            directory(follower.directory(&panes, None).unwrap()),
            ("w1:p2".to_string(), PathBuf::from("/preferred"))
        );
    }

    #[test]
    fn companion_follower_prefers_pane_cwd_over_foreground_cwd() {
        let mut follower = follower(None);
        let panes = vec![pane(
            "w1:p1",
            "w1",
            "w1:t1",
            true,
            Some("/shell-cwd"),
            Some("/foreground-cwd"),
        )];

        assert_eq!(
            directory(follower.directory(&panes, None).unwrap()),
            ("w1:p1".to_string(), PathBuf::from("/shell-cwd"))
        );
    }

    #[test]
    fn companion_follower_rejects_a_selected_pane_without_a_directory() {
        let mut follower = follower(None);
        let panes = vec![pane("w1:p1", "w1", "w1:t1", true, None, None)];

        let error = follower
            .directory(&panes, None)
            .expect_err("missing cwd must fail");
        assert!(
            matches!(error, HerdrError::InvalidResponse(message) if message == "pane w1:p1 has no current directory")
        );
    }

    #[test]
    fn pane_list_decodes_public_fields() {
        let value = serde_json::json!({
            "result": {"panes": [{
                "pane_id": "w1:p2",
                "workspace_id": "w1",
                "tab_id": "w1:t1",
                "focused": true,
                "foreground_cwd": "/tmp/project",
                "label": "Herdr Workbench"
            }]}
        });
        let panes = pane_list(&value).expect("pane list");
        assert_eq!(panes[0].pane_id, "w1:p2");
        assert!(panes[0].focused);
    }

    #[test]
    fn fake_records_argv_without_a_shell() {
        let fake = FakeHerdr::new([Ok(serde_json::json!({"result": {}}))]);
        fake.command(&command_args(&["pane", "list"]))
            .expect("command");
        assert_eq!(
            fake.calls(),
            vec![("command".to_string(), serde_json::json!(["pane", "list"]))]
        );
    }

    #[test]
    fn layout_requires_a_non_sidebar_companion() {
        let alone = serde_json::json!({
            "type": "pane_layout",
            "layout": {
                "workspace_id": "w1",
                "tab_id": "w1:t1",
                "panes": [{"pane_id": "w1:p2"}]
            }
        });
        assert!(!layout_has_companion(&alone, "w1", "w1:t1", "w1:p2").unwrap());

        let accompanied = serde_json::json!({
            "type": "pane_layout",
            "layout": {
                "workspace_id": "w1",
                "tab_id": "w1:t1",
                "panes": [{"pane_id": "w1:p1"}, {"pane_id": "w1:p2"}]
            }
        });
        assert!(layout_has_companion(&accompanied, "w1", "w1:t1", "w1:p2").unwrap());
        assert!(layout_has_companion(&accompanied, "w2", "w1:t1", "w1:p2").is_err());
        assert!(layout_has_companion(&accompanied, "w1", "w1:t1", "w1:p3").is_err());
    }

    #[test]
    fn only_the_matching_tab_layout_can_close_the_sidebar() {
        let event = serde_json::json!({
            "event": "layout_updated",
            "data": {
                "type": "layout_updated",
                "layout": {
                    "workspace_id": "w1",
                    "tab_id": "w1:t1",
                    "panes": [{"pane_id": "w1:p2"}]
                }
            }
        });
        assert!(layout_event_leaves_sidebar_alone(
            &event, "w1", "w1:t1", "w1:p2"
        ));
        assert!(!layout_event_leaves_sidebar_alone(
            &event, "w1", "w1:t2", "w1:p2"
        ));
        assert!(!layout_event_leaves_sidebar_alone(
            &event, "w1", "w1:t1", "w1:p1"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn companion_monitor_consumes_live_layout_subscription_events() {
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir().expect("socket directory");
        let socket_path = directory.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake Herdr socket");
        let (release_sender, release_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept subscription");
            let mut request = String::new();
            BufReader::new(stream.try_clone().expect("clone stream"))
                .read_line(&mut request)
                .expect("read subscription");
            let request: Value = serde_json::from_str(&request).expect("subscription JSON");
            assert_eq!(request["method"], "events.subscribe");
            assert_eq!(
                request["params"]["subscriptions"],
                serde_json::json!([
                    {"type": "layout.updated"},
                    {"type": "pane.updated"},
                    {"type": "pane.focused"}
                ])
            );
            writeln!(
                stream,
                "{}",
                serde_json::json!({
                    "id": request["id"],
                    "result": {"type": "events_subscribed"}
                })
            )
            .expect("subscription response");
            stream.flush().expect("flush subscription response");

            let (mut list_stream, _) = listener.accept().expect("accept initial pane list");
            let mut list_request = String::new();
            BufReader::new(list_stream.try_clone().expect("clone list stream"))
                .read_line(&mut list_request)
                .expect("read pane list");
            let list_request: Value = serde_json::from_str(&list_request).expect("pane list JSON");
            assert_eq!(list_request["method"], "pane.list");
            assert_eq!(
                list_request["params"],
                serde_json::json!({"workspace_id": "w1"})
            );
            writeln!(
                list_stream,
                "{}",
                serde_json::json!({
                    "id": list_request["id"],
                    "result": {"panes": [{
                        "pane_id": "w1:p1",
                        "workspace_id": "w1",
                        "tab_id": "w1:t1",
                        "focused": true,
                        "cwd": "/project"
                    }]}
                })
            )
            .expect("pane list response");
            list_stream.flush().expect("flush pane list response");
            writeln!(
                stream,
                "{}",
                serde_json::json!({
                    "event": "layout_updated",
                    "data": {
                        "type": "layout_updated",
                        "layout": {
                            "workspace_id": "w1",
                            "tab_id": "w1:t1",
                            "panes": [{"pane_id": "w1:p2"}]
                        }
                    }
                })
            )
            .expect("layout event");
            stream.flush().expect("flush layout event");
            release_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("keep fake socket open");
        });

        let monitor = CompanionMonitor::start(
            socket_path.as_os_str(),
            "w1",
            "w1:t1",
            "w1:p2",
            Some("w1:p1".to_string()),
        )
        .unwrap();
        assert_eq!(
            monitor
                .receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("initial directory"),
            CompanionEvent::Directory {
                pane_id: "w1:p1".to_string(),
                cwd: PathBuf::from("/project")
            }
        );
        assert_eq!(
            monitor
                .receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("companion event"),
            CompanionEvent::SidebarBecameOnlyPane
        );
        release_sender.send(()).expect("release fake server");
        server.join().expect("fake server");
    }

    #[cfg(unix)]
    #[test]
    fn companion_monitor_polls_for_a_cwd_change_without_an_event() {
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir().expect("socket directory");
        let socket_path = directory.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake Herdr socket");
        let (release_sender, release_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut subscription, _) = listener.accept().expect("accept subscription");
            let mut subscription_request = String::new();
            BufReader::new(subscription.try_clone().expect("clone subscription"))
                .read_line(&mut subscription_request)
                .expect("read subscription");
            let subscription_request: Value =
                serde_json::from_str(&subscription_request).expect("subscription JSON");
            writeln!(
                subscription,
                "{}",
                serde_json::json!({"id": subscription_request["id"], "result": {"type": "events_subscribed"}})
            )
            .expect("subscription response");
            subscription.flush().expect("flush subscription response");

            for cwd in ["/before", "/after"] {
                let (mut list_stream, _) = listener.accept().expect("accept pane list");
                let mut request = String::new();
                BufReader::new(list_stream.try_clone().expect("clone list stream"))
                    .read_line(&mut request)
                    .expect("read pane list");
                let request: Value = serde_json::from_str(&request).expect("pane list JSON");
                assert_eq!(request["method"], "pane.list");
                writeln!(
                    list_stream,
                    "{}",
                    serde_json::json!({
                        "id": request["id"],
                        "result": {"panes": [{
                            "pane_id": "w1:p1",
                            "workspace_id": "w1",
                            "tab_id": "w1:t1",
                            "focused": true,
                            "cwd": cwd
                        }]}
                    })
                )
                .expect("pane list response");
                list_stream.flush().expect("flush pane list response");
            }
            release_receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("release fake server");
        });

        let monitor = CompanionMonitor::start(
            socket_path.as_os_str(),
            "w1",
            "w1:t1",
            "w1:sidebar",
            Some("w1:p1".to_string()),
        )
        .unwrap();
        assert_eq!(
            monitor
                .receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("initial directory"),
            CompanionEvent::Directory {
                pane_id: "w1:p1".to_string(),
                cwd: PathBuf::from("/before")
            }
        );
        assert_eq!(
            monitor
                .receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("polled directory"),
            CompanionEvent::Directory {
                pane_id: "w1:p1".to_string(),
                cwd: PathBuf::from("/after")
            }
        );
        drop(monitor);
        release_sender.send(()).expect("release fake server");
        server.join().expect("fake server");
    }

    #[cfg(unix)]
    #[test]
    fn companion_monitor_uses_the_authoritative_list_after_a_focus_event() {
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir().expect("socket directory");
        let socket_path = directory.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake Herdr socket");
        let (release_sender, release_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut subscription, _) = listener.accept().expect("accept subscription");
            let mut subscription_request = String::new();
            BufReader::new(subscription.try_clone().expect("clone subscription"))
                .read_line(&mut subscription_request)
                .expect("read subscription");
            let subscription_request: Value =
                serde_json::from_str(&subscription_request).expect("subscription JSON");
            writeln!(
                subscription,
                "{}",
                serde_json::json!({"id": subscription_request["id"], "result": {"type": "events_subscribed"}})
            )
            .expect("subscription response");
            subscription.flush().expect("flush subscription response");

            let (mut initial_list, _) = listener.accept().expect("accept initial pane list");
            let mut initial_request = String::new();
            BufReader::new(initial_list.try_clone().expect("clone initial list"))
                .read_line(&mut initial_request)
                .expect("read initial pane list");
            let initial_request: Value =
                serde_json::from_str(&initial_request).expect("initial list JSON");
            writeln!(
                initial_list,
                "{}",
                serde_json::json!({
                    "id": initial_request["id"],
                    "result": {"panes": [
                        {"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "focused": false, "cwd": "/source"},
                        {"pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1", "focused": false, "cwd": "/other"}
                    ]}
                })
            )
            .expect("initial pane list response");
            initial_list
                .flush()
                .expect("flush initial pane list response");

            writeln!(
                subscription,
                "{}",
                serde_json::json!({
                    "event": "pane_focused",
                    "data": {"type": "pane_focused", "workspace_id": "w1", "pane_id": "w1:p2"}
                })
            )
            .expect("focus event");
            subscription.flush().expect("flush focus event");

            let (mut focused_list, _) = listener.accept().expect("accept focused pane list");
            let mut focused_request = String::new();
            BufReader::new(focused_list.try_clone().expect("clone focused list"))
                .read_line(&mut focused_request)
                .expect("read focused pane list");
            let focused_request: Value =
                serde_json::from_str(&focused_request).expect("focused list JSON");
            assert_eq!(focused_request["method"], "pane.list");
            writeln!(
                focused_list,
                "{}",
                serde_json::json!({
                    "id": focused_request["id"],
                    "result": {"panes": [
                        {"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "focused": true, "cwd": "/authoritative"},
                        {"pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1", "focused": false, "cwd": "/event-only"}
                    ]}
                })
            )
            .expect("focused pane list response");
            focused_list
                .flush()
                .expect("flush focused pane list response");
            release_receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("release fake server");
        });

        let monitor = CompanionMonitor::start(
            socket_path.as_os_str(),
            "w1",
            "w1:t1",
            "w1:sidebar",
            Some("w1:p1".to_string()),
        )
        .unwrap();
        assert_eq!(
            monitor
                .receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("initial source"),
            CompanionEvent::Directory {
                pane_id: "w1:p1".to_string(),
                cwd: PathBuf::from("/source")
            }
        );
        assert_eq!(
            monitor
                .receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("authoritative focused pane"),
            CompanionEvent::Directory {
                pane_id: "w1:p1".to_string(),
                cwd: PathBuf::from("/authoritative")
            }
        );
        drop(monitor);
        release_sender.send(()).expect("release fake server");
        server.join().expect("fake server");
    }
}
