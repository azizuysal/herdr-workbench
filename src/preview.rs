use std::{
    env, fmt, io,
    path::{Path, PathBuf},
    time::Duration,
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};
use unicode_width::UnicodeWidthChar;

use crate::{
    PLUGIN_ID, PREVIEW_ENTRYPOINT,
    file_tree::{FileTree, Preview, sanitize_terminal},
    git::{GitStatusProvider, SourceControlGroup},
    herdr::{HerdrClient, LiveHerdr},
    highlight::{HighlightLine, HighlightRole, HighlightedText, label},
    media::{self, VisualPreview},
    render::sanitize_multiline,
    theme::{self, Palette},
    workspace::WorkspaceRoot,
};

const ROOT_ENV: &str = "HERDR_WORKBENCH_PREVIEW_ROOT";
const KIND_ENV: &str = "HERDR_WORKBENCH_PREVIEW_KIND";
const PATH_ENV: &str = "HERDR_WORKBENCH_PREVIEW_PATH";
const LINE_ENV: &str = "HERDR_WORKBENCH_PREVIEW_LINE";
const GROUP_ENV: &str = "HERDR_WORKBENCH_PREVIEW_GROUP";
const OID_ENV: &str = "HERDR_WORKBENCH_PREVIEW_OID";
const REPOSITORY_ENV: &str = "HERDR_WORKBENCH_PREVIEW_REPOSITORY";
const MAX_VISUAL_ZOOM: u8 = 5;
const VISUAL_PAN_STEP: i64 = 8;
const VISUAL_WHEEL_STEP: i64 = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewRequest {
    File {
        path: PathBuf,
        line: Option<usize>,
    },
    Source {
        repository: PathBuf,
        path: PathBuf,
        group: SourceControlGroup,
    },
    Commit {
        repository: PathBuf,
        oid: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewError(String);

impl PreviewError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PreviewError {}

pub trait PreviewOpener: Send + Sync {
    fn open(&self, workspace: &WorkspaceRoot, request: &PreviewRequest)
    -> Result<(), PreviewError>;
}

pub struct HerdrPreviewOpener<C = LiveHerdr> {
    client: C,
}

impl<C> HerdrPreviewOpener<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

impl<C: HerdrClient> PreviewOpener for HerdrPreviewOpener<C> {
    fn open(
        &self,
        workspace: &WorkspaceRoot,
        request: &PreviewRequest,
    ) -> Result<(), PreviewError> {
        let mut request_env = std::collections::BTreeMap::from([(
            ROOT_ENV.to_string(),
            encode_path(workspace.path()),
        )]);
        match request {
            PreviewRequest::File { path, line } => {
                request_env.insert(KIND_ENV.to_string(), "file".to_string());
                request_env.insert(PATH_ENV.to_string(), encode_path(path));
                if let Some(line) = line {
                    request_env.insert(LINE_ENV.to_string(), line.to_string());
                }
            }
            PreviewRequest::Source {
                repository,
                path,
                group,
            } => {
                request_env.insert(KIND_ENV.to_string(), "source".to_string());
                request_env.insert(REPOSITORY_ENV.to_string(), encode_path(repository));
                request_env.insert(PATH_ENV.to_string(), encode_path(path));
                request_env.insert(GROUP_ENV.to_string(), encode_group(*group).to_string());
            }
            PreviewRequest::Commit { repository, oid } => {
                request_env.insert(KIND_ENV.to_string(), "commit".to_string());
                request_env.insert(REPOSITORY_ENV.to_string(), encode_path(repository));
                request_env.insert(OID_ENV.to_string(), oid.clone());
            }
        }
        self.client
            .request(
                "plugin.pane.open",
                serde_json::json!({
                    "plugin_id": PLUGIN_ID,
                    "entrypoint": PREVIEW_ENTRYPOINT,
                    "placement": "popup",
                    "focus": true,
                    "env": request_env,
                }),
            )
            .map_err(|error| PreviewError::new(error.to_string()))?;
        Ok(())
    }
}

impl PreviewRequest {
    fn path(&self) -> Option<&Path> {
        match self {
            Self::File { path, .. } | Self::Source { path, .. } => Some(path),
            Self::Commit { .. } => None,
        }
    }
}

#[derive(Debug)]
struct PreviewInvocation {
    workspace: WorkspaceRoot,
    request: PreviewRequest,
}

impl PreviewInvocation {
    fn from_env() -> Result<Self, PreviewError> {
        let root = decode_path(&required_env(ROOT_ENV)?)?;
        let request = match required_env(KIND_ENV)?.as_str() {
            "file" => PreviewRequest::File {
                path: decode_path(&required_env(PATH_ENV)?)?,
                line: env::var(LINE_ENV)
                    .ok()
                    .map(|line| {
                        line.parse::<usize>()
                            .ok()
                            .filter(|line| *line > 0)
                            .ok_or_else(|| PreviewError::new("invalid preview line"))
                    })
                    .transpose()?,
            },
            "source" => PreviewRequest::Source {
                repository: decode_path(&required_env(REPOSITORY_ENV)?)?,
                path: decode_path(&required_env(PATH_ENV)?)?,
                group: decode_group(&required_env(GROUP_ENV)?)?,
            },
            "commit" => PreviewRequest::Commit {
                repository: decode_path(&required_env(REPOSITORY_ENV)?)?,
                oid: required_env(OID_ENV)?,
            },
            kind => return Err(PreviewError::new(format!("invalid preview kind {kind:?}"))),
        };
        let workspace = WorkspaceRoot::from_directory(&root)
            .map_err(|error| PreviewError::new(error.to_string()))?;
        Ok(Self { workspace, request })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PreviewContent {
    Text(HighlightedText),
    Visual(VisualPreview),
}

impl PreviewContent {
    pub fn plain_text(&self) -> String {
        match self {
            Self::Text(text) => text.plain_text(),
            Self::Visual(visual) => format!(
                "{}×{} visual preview",
                visual.source_width, visual.source_height
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewDocument {
    pub title: String,
    pub content: PreviewContent,
    pub initial_line: usize,
    pub is_error: bool,
    pub numbered_line_range: Option<std::ops::Range<usize>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TextPosition {
    line: usize,
    column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TextSelection {
    anchor: TextPosition,
    cursor: TextPosition,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct VisualViewport {
    zoom: u8,
    pan_x: i64,
    pan_y: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PreviewRenderState {
    selection: Option<TextSelection>,
    visual_viewport: VisualViewport,
    show_line_numbers: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SelectionRenderState {
    vertical: usize,
    line_number_gutter_width: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WrappedTextLine {
    source_line: usize,
    start_column: usize,
    end_column: usize,
    first_visual_line: bool,
}

impl VisualViewport {
    fn zoom_in(&mut self) {
        self.zoom = self.zoom.saturating_add(1).min(MAX_VISUAL_ZOOM);
    }

    fn zoom_out(&mut self) {
        self.zoom = self.zoom.saturating_sub(1);
        if self.zoom == 0 {
            self.pan_x = 0;
            self.pan_y = 0;
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn pan(&mut self, horizontal: i64, vertical: i64) {
        if self.zoom == 0 {
            return;
        }
        self.pan_x = self.pan_x.saturating_add(horizontal);
        self.pan_y = self.pan_y.saturating_add(vertical);
    }
}

impl TextSelection {
    fn ordered(self) -> (TextPosition, TextPosition) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }
}

impl PreviewDocument {
    fn error(title: String, error: impl fmt::Display) -> Self {
        Self {
            title,
            content: plain_content(&error.to_string(), HighlightRole::Red),
            initial_line: 0,
            is_error: true,
            numbered_line_range: None,
        }
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let invocation = PreviewInvocation::from_env()?;
    let title = request_title(&invocation.request);
    let document =
        load_document(&invocation).unwrap_or_else(|error| PreviewDocument::error(title, error));
    let visual_path = if matches!(&document.content, PreviewContent::Visual(_)) {
        invocation
            .request
            .path()
            .map(|path| invocation.workspace.resolve_path(path))
            .transpose()
            .map_err(|error| PreviewError::new(error.to_string()))?
    } else {
        None
    };
    terminal_loop(&document, visual_path.as_deref())
}

fn load_document(invocation: &PreviewInvocation) -> Result<PreviewDocument, PreviewError> {
    let request = &invocation.request;
    let title = request_title(request);
    match request {
        PreviewRequest::File { path, line } => {
            let resolved = invocation
                .workspace
                .resolve_path(path)
                .map_err(|error| PreviewError::new(error.to_string()))?;
            if let Some(visual) = media::load(&resolved).map_err(PreviewError::new)? {
                return Ok(PreviewDocument {
                    title,
                    content: PreviewContent::Visual(visual),
                    initial_line: 0,
                    is_error: false,
                    numbered_line_range: None,
                });
            }
            let tree = FileTree::new(invocation.workspace.clone(), true, false);
            let (content, numbered_line_range) = match tree
                .preview(path)
                .map_err(|error| PreviewError::new(error.to_string()))?
            {
                Preview::Text {
                    source, truncated, ..
                } => {
                    let line_count = source.lines().count();
                    (
                        PreviewContent::Text(
                            HighlightedText::source(path, &source, false, truncated)
                                .map_err(PreviewError::new)?,
                        ),
                        Some(0..line_count),
                    )
                }
                Preview::Binary { bytes, truncated } => (
                    plain_content(
                        &format!(
                            "Binary file: {bytes} bytes{}",
                            if truncated {
                                " (preview limit reached)"
                            } else {
                                ""
                            }
                        ),
                        HighlightRole::Muted,
                    ),
                    None,
                ),
            };
            Ok(PreviewDocument {
                title,
                content,
                initial_line: line.map_or(0, |line| line.saturating_sub(4)),
                is_error: false,
                numbered_line_range,
            })
        }
        PreviewRequest::Source {
            repository,
            path,
            group,
        } => {
            let repository_workspace = resolve_repository(&invocation.workspace, repository)?;
            invocation
                .workspace
                .resolve_path(path)
                .map_err(|error| PreviewError::new(error.to_string()))?;
            let repository_path = path_in_repository(path, repository)?;
            let provider = GitStatusProvider::new(repository_workspace.path());
            let snapshot = provider
                .refresh()
                .map_err(|error| PreviewError::new(error.to_string()))?;
            let groups = snapshot.groups();
            let entry = groups
                .get(group)
                .and_then(|entries| entries.iter().find(|entry| entry.path == repository_path))
                .ok_or_else(|| {
                    PreviewError::new(format!(
                        "{} is no longer present in {}",
                        path.display(),
                        group_label(*group)
                    ))
                })?;
            let (content, numbered_line_range) = if *group == SourceControlGroup::Untracked {
                let resolved = invocation
                    .workspace
                    .resolve_path(path)
                    .map_err(|error| PreviewError::new(error.to_string()))?;
                if let Some(visual) = media::load(&resolved).map_err(PreviewError::new)? {
                    (PreviewContent::Visual(visual), None)
                } else {
                    let tree = FileTree::new(invocation.workspace.clone(), true, false);
                    match tree
                        .preview(path)
                        .map_err(|error| PreviewError::new(error.to_string()))?
                    {
                        Preview::Text {
                            source, truncated, ..
                        } => {
                            let line_count = source.lines().count();
                            (
                                PreviewContent::Text(
                                    HighlightedText::source(path, &source, false, truncated)
                                        .map_err(PreviewError::new)?
                                        .prepend(label(
                                            format!(
                                                "untracked: {}",
                                                sanitize_terminal(&path.display().to_string())
                                            ),
                                            HighlightRole::Green,
                                        )),
                                ),
                                Some(1..line_count.saturating_add(1)),
                            )
                        }
                        Preview::Binary { bytes, truncated } => (
                            plain_content(
                                &format!(
                                    "untracked binary: {} · {bytes} bytes{}",
                                    sanitize_terminal(&path.display().to_string()),
                                    if truncated {
                                        " · preview truncated"
                                    } else {
                                        ""
                                    }
                                ),
                                HighlightRole::Muted,
                            ),
                            None,
                        ),
                    }
                }
            } else {
                let diff = provider
                    .preview(entry, *group)
                    .map_err(|error| PreviewError::new(error.to_string()))?;
                (
                    PreviewContent::Text(
                        HighlightedText::diff(path, &diff).map_err(PreviewError::new)?,
                    ),
                    None,
                )
            };
            Ok(PreviewDocument {
                title,
                content,
                initial_line: 0,
                is_error: false,
                numbered_line_range,
            })
        }
        PreviewRequest::Commit { repository, oid } => {
            let repository_workspace = resolve_repository(&invocation.workspace, repository)?;
            let provider = GitStatusProvider::new(repository_workspace.path());
            let preview = provider
                .commit_preview(oid)
                .map_err(|error| PreviewError::new(error.to_string()))?;
            Ok(PreviewDocument {
                title,
                content: PreviewContent::Text(
                    HighlightedText::diff(Path::new("commit.diff"), &preview)
                        .map_err(PreviewError::new)?,
                ),
                initial_line: 0,
                is_error: false,
                numbered_line_range: None,
            })
        }
    }
}

fn request_title(request: &PreviewRequest) -> String {
    match request {
        PreviewRequest::File { path, .. } => sanitize_terminal(&path.display().to_string()),
        PreviewRequest::Source { path, group, .. } => {
            format!(
                "{} · {}",
                group_label(*group),
                sanitize_terminal(&path.display().to_string())
            )
        }
        PreviewRequest::Commit { repository, oid } => {
            let commit = format!("Commit {}", oid.get(..oid.len().min(12)).unwrap_or(oid));
            if repository.as_os_str().is_empty() {
                commit
            } else {
                format!(
                    "{commit} · {}",
                    sanitize_terminal(&repository.display().to_string())
                )
            }
        }
    }
}

fn resolve_repository(
    workspace: &WorkspaceRoot,
    repository: &Path,
) -> Result<WorkspaceRoot, PreviewError> {
    let path = workspace
        .resolve_path(repository)
        .map_err(|error| PreviewError::new(error.to_string()))?;
    let repository = WorkspaceRoot::from_directory(&path)
        .map_err(|error| PreviewError::new(error.to_string()))?;
    if repository.is_git_worktree {
        Ok(repository)
    } else {
        Err(PreviewError::new(
            "Source Control preview repository must be a Git worktree root",
        ))
    }
}

fn path_in_repository(path: &Path, repository: &Path) -> Result<PathBuf, PreviewError> {
    if repository.as_os_str().is_empty() {
        return Ok(path.to_owned());
    }
    path.strip_prefix(repository)
        .map(Path::to_owned)
        .map_err(|_| {
            PreviewError::new(format!(
                "{} is outside repository {}",
                path.display(),
                repository.display()
            ))
        })
}

fn plain_content(text: &str, role: HighlightRole) -> PreviewContent {
    PreviewContent::Text(HighlightedText::plain(&sanitize_multiline(text), role))
}

fn group_label(group: SourceControlGroup) -> &'static str {
    match group {
        SourceControlGroup::MergeChanges => "Merge Changes",
        SourceControlGroup::StagedChanges => "Staged Changes",
        SourceControlGroup::Changes => "Changes",
        SourceControlGroup::Untracked => "Untracked",
    }
}

fn encode_group(group: SourceControlGroup) -> &'static str {
    match group {
        SourceControlGroup::MergeChanges => "merge",
        SourceControlGroup::StagedChanges => "staged",
        SourceControlGroup::Changes => "changes",
        SourceControlGroup::Untracked => "untracked",
    }
}

fn decode_group(value: &str) -> Result<SourceControlGroup, PreviewError> {
    match value {
        "merge" => Ok(SourceControlGroup::MergeChanges),
        "staged" => Ok(SourceControlGroup::StagedChanges),
        "changes" => Ok(SourceControlGroup::Changes),
        "untracked" => Ok(SourceControlGroup::Untracked),
        _ => Err(PreviewError::new(format!(
            "invalid Source Control preview group {value:?}"
        ))),
    }
}

fn required_env(key: &str) -> Result<String, PreviewError> {
    env::var(key).map_err(|_| PreviewError::new(format!("missing {key}")))
}

fn encode_path(path: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let bytes = path.as_os_str().to_string_lossy().as_bytes();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn decode_path(value: &str) -> Result<PathBuf, PreviewError> {
    if !value.len().is_multiple_of(2) {
        return Err(PreviewError::new("invalid encoded preview path"));
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| PreviewError::new("invalid encoded preview path"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|_| PreviewError::new("invalid encoded preview path"))
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        ) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

fn terminal_loop(
    document: &PreviewDocument,
    visual_path: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut document = document.clone();
    let _guard = TerminalGuard::enter()?;
    let (theme_path, mut resolved_theme, _) = theme::load_from_env();
    if let Some(appearance) = theme::query_terminal_appearance()
        && let Some(path) = theme_path
    {
        resolved_theme =
            theme::resolve_config(&path, Some(appearance)).map_err(PreviewError::new)?;
    }
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let mut vertical = 0_usize;
    let mut initial_line = Some(document.initial_line);
    let mut selection = None;
    let mut visual_viewport = VisualViewport::default();
    let mut help_visible = false;
    let mut show_line_numbers = false;
    let mut dirty = true;

    loop {
        let mut content_width = 1_usize;
        let mut content_height = 1_usize;
        if dirty {
            terminal.draw(|frame| {
                let area = frame.area();
                content_width = usize::from(area.width);
                content_height = usize::from(area.height).max(1);
                if let Some(source_line) = initial_line.take() {
                    vertical = wrapped_row_for_source_line(
                        &document,
                        content_width,
                        show_line_numbers,
                        source_line,
                    );
                }
                vertical = vertical.min(max_vertical_scroll(
                    &document,
                    content_height,
                    content_width,
                    show_line_numbers,
                ));
                if help_visible {
                    render_preview_help(
                        frame.buffer_mut(),
                        area,
                        &resolved_theme.palette,
                        matches!(&document.content, PreviewContent::Visual(_)),
                        document.numbered_line_range.is_some(),
                    );
                } else {
                    render_preview_with_selection(
                        frame.buffer_mut(),
                        area,
                        &document,
                        vertical,
                        &resolved_theme.palette,
                        PreviewRenderState {
                            selection,
                            visual_viewport,
                            show_line_numbers,
                        },
                    );
                }
            })?;
            dirty = false;
        } else {
            let area = terminal.size()?;
            content_width = usize::from(area.width);
            content_height = usize::from(area.height).max(1);
        }
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => {
                    if help_visible {
                        match key.code {
                            KeyCode::Char('?') | KeyCode::Esc => {
                                help_visible = false;
                                dirty = true;
                            }
                            KeyCode::Char('q') | KeyCode::Char(' ') => return Ok(()),
                            _ => {}
                        }
                        continue;
                    }
                    if key.code == KeyCode::Char('?') {
                        help_visible = true;
                        dirty = true;
                        continue;
                    }
                    if select_all_requested(key) {
                        selection = match &document.content {
                            PreviewContent::Text(text) => select_all(text),
                            PreviewContent::Visual(_) => None,
                        };
                        dirty = true;
                        continue;
                    }
                    if copy_requested(key)
                        && let PreviewContent::Text(text) = &document.content
                        && let Some(selected) =
                            selection.and_then(|selection| selected_text(text, selection))
                    {
                        if let Err(error) = crate::clipboard::copy_text(&selected) {
                            document = PreviewDocument::error(document.title.clone(), error);
                            selection = None;
                            vertical = 0;
                        }
                        dirty = true;
                        continue;
                    }
                    if key.code == KeyCode::Esc && selection.take().is_some() {
                        dirty = true;
                        continue;
                    }
                    if key.code == KeyCode::Char('o')
                        && matches!(&document.content, PreviewContent::Visual(_))
                    {
                        let result = visual_path
                            .ok_or_else(|| {
                                PreviewError::new(
                                    "the visual preview path is unavailable for system preview",
                                )
                            })
                            .and_then(|path| {
                                crate::system_preview::open(path)
                                    .map_err(|error| PreviewError::new(error.to_string()))
                            });
                        if let Err(error) = result {
                            document = PreviewDocument::error(document.title.clone(), error);
                            vertical = 0;
                            visual_viewport.reset();
                        }
                        dirty = true;
                        continue;
                    }
                    if handle_line_number_toggle(key, &document, &mut show_line_numbers) {
                        dirty = true;
                        continue;
                    }
                    let vertical_limit = max_vertical_scroll(
                        &document,
                        content_height,
                        content_width,
                        show_line_numbers,
                    );
                    if handle_preview_key(
                        key,
                        &mut document,
                        content_height,
                        vertical_limit,
                        &mut vertical,
                        &mut visual_viewport,
                    ) {
                        return Ok(());
                    }
                    dirty = true;
                }
                Event::Mouse(mouse) => {
                    let area = terminal.size()?.into();
                    if handle_selection_mouse(
                        mouse,
                        &document,
                        area,
                        vertical,
                        show_line_numbers,
                        &mut selection,
                    ) {
                        dirty = true;
                        continue;
                    }
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            if visual_viewport.zoom > 0
                                && matches!(&document.content, PreviewContent::Visual(_))
                            {
                                visual_viewport.pan(0, -VISUAL_WHEEL_STEP);
                            } else if !move_pdf_page(&mut document, -1) {
                                vertical = vertical.saturating_sub(3);
                            }
                            dirty = true;
                        }
                        MouseEventKind::ScrollDown => {
                            if visual_viewport.zoom > 0
                                && matches!(&document.content, PreviewContent::Visual(_))
                            {
                                visual_viewport.pan(0, VISUAL_WHEEL_STEP);
                            } else if !move_pdf_page(&mut document, 1) {
                                vertical = vertical.saturating_add(3).min(max_vertical_scroll(
                                    &document,
                                    content_height,
                                    content_width,
                                    show_line_numbers,
                                ));
                            }
                            dirty = true;
                        }
                        _ => {}
                    }
                }
                Event::Resize(_, _) => dirty = true,
                Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
            }
        }
    }
}

fn select_all_requested(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('a')
        && key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
}

fn copy_requested(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('y')
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        || key.code == KeyCode::Char('c')
            && key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
}

fn handle_preview_key(
    key: KeyEvent,
    document: &mut PreviewDocument,
    content_height: usize,
    vertical_limit: usize,
    vertical: &mut usize,
    visual_viewport: &mut VisualViewport,
) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
        || matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(' ')
        )
    {
        return true;
    }
    if matches!(&document.content, PreviewContent::Visual(_)) {
        match key.code {
            KeyCode::Char('+') | KeyCode::Char('=') => visual_viewport.zoom_in(),
            KeyCode::Char('-') => visual_viewport.zoom_out(),
            KeyCode::Char('0') => visual_viewport.reset(),
            KeyCode::Left | KeyCode::Char('h') => {
                visual_viewport.pan(-VISUAL_PAN_STEP, 0);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                visual_viewport.pan(VISUAL_PAN_STEP, 0);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                visual_viewport.pan(0, -VISUAL_PAN_STEP);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                visual_viewport.pan(0, VISUAL_PAN_STEP);
            }
            KeyCode::PageUp => {
                move_pdf_page(document, -1);
            }
            KeyCode::PageDown => {
                move_pdf_page(document, 1);
            }
            KeyCode::Home => {
                move_pdf_to_boundary(document, false);
            }
            KeyCode::End => {
                move_pdf_to_boundary(document, true);
            }
            _ => {}
        }
        return false;
    }
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => *vertical = vertical.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            *vertical = vertical.saturating_add(1).min(vertical_limit);
        }
        KeyCode::PageUp => *vertical = vertical.saturating_sub(content_height),
        KeyCode::PageDown => {
            *vertical = vertical.saturating_add(content_height).min(vertical_limit);
        }
        KeyCode::Home => *vertical = 0,
        KeyCode::End => *vertical = vertical_limit,
        _ => {}
    }
    false
}

fn handle_line_number_toggle(
    key: KeyEvent,
    document: &PreviewDocument,
    show_line_numbers: &mut bool,
) -> bool {
    if key.code != KeyCode::Char('n')
        || key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        || document.numbered_line_range.is_none()
    {
        return false;
    }
    *show_line_numbers = !*show_line_numbers;
    true
}

fn move_pdf_page(document: &mut PreviewDocument, delta: isize) -> bool {
    let PreviewContent::Visual(visual) = &mut document.content else {
        return false;
    };
    let (Some(page_count), Some(page_index)) = (visual.page_count, visual.page_index) else {
        return false;
    };
    let target = page_index
        .saturating_add_signed(delta)
        .min(page_count.saturating_sub(1));
    if target != page_index
        && let Err(error) = visual.show_pdf_page(target)
    {
        *document = PreviewDocument::error(document.title.clone(), error);
    }
    true
}

fn move_pdf_to_boundary(document: &mut PreviewDocument, end: bool) -> bool {
    let PreviewContent::Visual(visual) = &mut document.content else {
        return false;
    };
    let (Some(page_count), Some(page_index)) = (visual.page_count, visual.page_index) else {
        return false;
    };
    let target = if end { page_count.saturating_sub(1) } else { 0 };
    if target != page_index
        && let Err(error) = visual.show_pdf_page(target)
    {
        *document = PreviewDocument::error(document.title.clone(), error);
    }
    true
}

pub fn render_preview(
    buffer: &mut Buffer,
    area: Rect,
    document: &PreviewDocument,
    vertical: usize,
    _horizontal: usize,
    palette: &Palette,
) {
    render_preview_with_selection(
        buffer,
        area,
        document,
        vertical,
        palette,
        PreviewRenderState::default(),
    );
}

fn render_preview_help(
    buffer: &mut Buffer,
    area: Rect,
    palette: &Palette,
    is_visual: bool,
    line_numbers_available: bool,
) {
    Block::default()
        .style(Style::default().bg(palette.panel_bg))
        .render(area, buffer);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let key = Style::default()
        .fg(palette.accent)
        .bg(palette.panel_bg)
        .add_modifier(Modifier::BOLD);
    let action = Style::default().fg(palette.text).bg(palette.panel_bg);
    let muted = Style::default().fg(palette.overlay0).bg(palette.panel_bg);
    let shortcut = |keys: &'static str, description: &'static str| {
        Line::from(vec![
            Span::styled(format!("{keys:<19}"), key),
            Span::styled(description, action),
        ])
    };
    let mut lines = vec![Line::from(Span::styled(
        "Preview shortcuts",
        key.add_modifier(Modifier::UNDERLINED),
    ))];
    if is_visual {
        lines.extend([
            shortcut("+ / -", "zoom in / out"),
            shortcut("0", "fit to window"),
            shortcut("Arrows or h/j/k/l", "pan when zoomed"),
            shortcut("Mouse wheel", "pan / PDF page"),
            shortcut("PgUp/PgDn", "previous / next PDF page"),
            shortcut("Home/End", "first / last PDF page"),
            shortcut("o", native_visual_action()),
            shortcut("Esc", "close preview"),
            shortcut("Space or q", "close preview"),
            shortcut("?", "close this help"),
        ]);
    } else {
        lines.extend([
            shortcut("Mouse drag", "select text"),
            shortcut("Cmd+A / Ctrl+A", "select all text"),
            shortcut("Cmd+C / Ctrl+C", "copy selected text"),
            shortcut("y", "copy selected text"),
        ]);
        if line_numbers_available {
            lines.push(shortcut("n", "toggle line numbers"));
        }
        lines.extend([
            shortcut("Up/Down or j/k", "scroll vertically"),
            shortcut("PgUp/PgDn", "scroll by page"),
            shortcut("Home/End", "first / last line"),
            shortcut("Mouse wheel", "scroll vertically"),
            shortcut("Esc", "clear selection / close"),
            shortcut("Space or q", "close preview"),
            shortcut("?", "close this help"),
            Line::from(Span::styled("Cmd keys require terminal forwarding", muted)),
        ]);
    }
    Paragraph::new(lines)
        .style(Style::default().bg(palette.panel_bg))
        .render(area, buffer);
}

#[cfg(target_os = "macos")]
fn native_visual_action() -> &'static str {
    "open in Quick Look"
}

#[cfg(not(target_os = "macos"))]
fn native_visual_action() -> &'static str {
    "open in system viewer"
}

fn render_preview_with_selection(
    buffer: &mut Buffer,
    area: Rect,
    document: &PreviewDocument,
    vertical: usize,
    palette: &Palette,
    state: PreviewRenderState,
) {
    Block::default()
        .style(Style::default().bg(palette.panel_bg))
        .render(area, buffer);
    if area.width == 0 || area.height == 0 {
        return;
    }
    match &document.content {
        PreviewContent::Text(text) => {
            let gutter_width = effective_line_number_gutter_width(
                document,
                state.show_line_numbers,
                usize::from(area.width),
            );
            let wrapped_lines = wrapped_text_layout(
                text,
                usize::from(area.width).saturating_sub(gutter_width).max(1),
            );
            let lines = wrapped_lines
                .iter()
                .map(|wrapped| {
                    let mut spans = Vec::new();
                    if let Some(prefix) = line_number_prefix(
                        document,
                        wrapped.source_line,
                        state.show_line_numbers,
                        wrapped.first_visual_line,
                        gutter_width,
                    ) {
                        spans.push(Span::styled(
                            prefix,
                            Style::default().fg(palette.overlay0).bg(palette.panel_bg),
                        ));
                    }
                    spans.extend(highlighted_spans(
                        &text.lines[wrapped.source_line],
                        wrapped.start_column,
                        wrapped.end_column,
                        palette,
                    ));
                    Line::from(spans)
                })
                .collect::<Vec<_>>();
            Paragraph::new(lines)
                .style(
                    Style::default()
                        .fg(if document.is_error {
                            palette.red
                        } else {
                            palette.text
                        })
                        .bg(palette.panel_bg),
                )
                .scroll((u16::try_from(vertical).unwrap_or(u16::MAX), 0))
                .render(area, buffer);
            if let Some(selection) = state.selection {
                render_selection(
                    buffer,
                    area,
                    text,
                    &wrapped_lines,
                    selection,
                    palette.surface1,
                    SelectionRenderState {
                        vertical,
                        line_number_gutter_width: gutter_width,
                    },
                );
            }
        }
        PreviewContent::Visual(visual) => {
            render_visual(buffer, area, visual, palette, state.visual_viewport);
        }
    }
}

fn handle_selection_mouse(
    mouse: MouseEvent,
    document: &PreviewDocument,
    area: Rect,
    vertical: usize,
    show_line_numbers: bool,
    selection: &mut Option<TextSelection>,
) -> bool {
    let PreviewContent::Text(text) = &document.content else {
        return false;
    };
    let gutter_width =
        effective_line_number_gutter_width(document, show_line_numbers, usize::from(area.width));
    let wrapped_lines = wrapped_text_layout(
        text,
        usize::from(area.width).saturating_sub(gutter_width).max(1),
    );
    let selection_cell = || {
        if mouse.column < area.x
            || mouse.column >= area.x.saturating_add(area.width)
            || mouse.row < area.y
            || mouse.row >= area.y.saturating_add(area.height)
        {
            return None;
        }
        let wrapped = wrapped_lines.get(vertical + usize::from(mouse.row - area.y))?;
        let column = usize::from(mouse.column - area.x).saturating_sub(gutter_width);
        text_cell_boundaries(
            text,
            wrapped.source_line,
            wrapped.start_column,
            wrapped.end_column,
            column,
        )
    };
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let Some((start, end)) = selection_cell() else {
                return false;
            };
            *selection = Some(TextSelection {
                anchor: start,
                cursor: end,
            });
            true
        }
        MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
            let Some(current) = *selection else {
                return false;
            };
            let Some((start, end)) = selection_cell() else {
                return false;
            };
            let cursor = if start < current.anchor { start } else { end };
            *selection = Some(TextSelection {
                anchor: current.anchor,
                cursor,
            });
            true
        }
        _ => false,
    }
}

fn text_cell_boundaries(
    text: &HighlightedText,
    line: usize,
    start_column: usize,
    end_column: usize,
    display_column: usize,
) -> Option<(TextPosition, TextPosition)> {
    let line_text = text.lines.get(line)?.plain_text();
    let characters = line_text.chars().collect::<Vec<_>>();
    let mut display = 0_usize;
    for (index, character) in characters
        .iter()
        .enumerate()
        .take(end_column)
        .skip(start_column)
    {
        let width = UnicodeWidthChar::width(*character).unwrap_or(0);
        if width > 0 && display_column < display.saturating_add(width) {
            let mut end = index + 1;
            while end < end_column.min(characters.len())
                && UnicodeWidthChar::width(characters[end]).unwrap_or(0) == 0
            {
                end += 1;
            }
            return Some((
                TextPosition {
                    line,
                    column: index,
                },
                TextPosition { line, column: end },
            ));
        }
        display = display.saturating_add(width);
    }
    let end = TextPosition {
        line,
        column: end_column.min(characters.len()),
    };
    Some((end, end))
}

fn select_all(text: &HighlightedText) -> Option<TextSelection> {
    let last_line = text.lines.len().checked_sub(1)?;
    let end_column = text.lines[last_line].plain_text().chars().count();
    let selection = TextSelection {
        anchor: TextPosition { line: 0, column: 0 },
        cursor: TextPosition {
            line: last_line,
            column: end_column,
        },
    };
    selected_text(text, selection).map(|_| selection)
}

fn selected_text(text: &HighlightedText, selection: TextSelection) -> Option<String> {
    let (start, end) = selection.ordered();
    if start == end || start.line >= text.lines.len() || end.line >= text.lines.len() {
        return None;
    }
    let mut selected = String::new();
    for line_index in start.line..=end.line {
        if line_index > start.line {
            selected.push('\n');
        }
        let line = text.lines[line_index].plain_text();
        let length = line.chars().count();
        let from = if line_index == start.line {
            start.column.min(length)
        } else {
            0
        };
        let to = if line_index == end.line {
            end.column.min(length)
        } else {
            length
        };
        selected.extend(line.chars().skip(from).take(to.saturating_sub(from)));
    }
    Some(selected)
}

fn render_selection(
    buffer: &mut Buffer,
    area: Rect,
    text: &HighlightedText,
    wrapped_lines: &[WrappedTextLine],
    selection: TextSelection,
    background: Color,
    state: SelectionRenderState,
) {
    let (start, end) = selection.ordered();
    for viewport_row in 0..usize::from(area.height) {
        let Some(wrapped) = wrapped_lines.get(state.vertical + viewport_row) else {
            break;
        };
        let line_index = wrapped.source_line;
        if line_index < start.line || line_index > end.line {
            continue;
        }
        let Some(line) = text.lines.get(line_index) else {
            break;
        };
        let line = line.plain_text();
        let length = line.chars().count();
        let selection_from = if line_index == start.line {
            start.column.min(length)
        } else {
            0
        };
        let selection_to = if line_index == end.line {
            end.column.min(length)
        } else {
            length
        };
        let from = selection_from.max(wrapped.start_column);
        let to = selection_to.min(wrapped.end_column);
        if from >= to {
            continue;
        }
        let display_start = state
            .line_number_gutter_width
            .saturating_add(display_width_between(&line, wrapped.start_column, from));
        let display_end = state
            .line_number_gutter_width
            .saturating_add(display_width_between(&line, wrapped.start_column, to));
        for display_column in display_start..display_end.min(usize::from(area.width)) {
            let x = area.x + u16::try_from(display_column).unwrap_or(u16::MAX);
            let y = area.y + u16::try_from(viewport_row).unwrap_or(u16::MAX);
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_bg(background);
            }
        }
    }
}

fn display_width_between(text: &str, start: usize, end: usize) -> usize {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|character| UnicodeWidthChar::width(character).unwrap_or(0))
        .sum()
}

fn max_vertical_scroll(
    document: &PreviewDocument,
    content_height: usize,
    content_width: usize,
    show_line_numbers: bool,
) -> usize {
    match &document.content {
        PreviewContent::Text(text) => wrapped_text_layout(
            text,
            wrapped_content_width(document, content_width, show_line_numbers),
        )
        .len()
        .max(1)
        .saturating_sub(content_height),
        PreviewContent::Visual(_) => 0,
    }
}

fn line_number_gutter_width(document: &PreviewDocument, show_line_numbers: bool) -> usize {
    if !show_line_numbers {
        return 0;
    }
    document
        .numbered_line_range
        .as_ref()
        .map(|range| {
            range
                .end
                .saturating_sub(range.start)
                .max(1)
                .to_string()
                .len()
                .max(4)
                + 2
        })
        .unwrap_or(0)
}

fn effective_line_number_gutter_width(
    document: &PreviewDocument,
    show_line_numbers: bool,
    content_width: usize,
) -> usize {
    line_number_gutter_width(document, show_line_numbers).min(content_width.saturating_sub(1))
}

fn wrapped_content_width(
    document: &PreviewDocument,
    content_width: usize,
    show_line_numbers: bool,
) -> usize {
    content_width
        .saturating_sub(effective_line_number_gutter_width(
            document,
            show_line_numbers,
            content_width,
        ))
        .max(1)
}

fn line_number_prefix(
    document: &PreviewDocument,
    line_index: usize,
    show_line_numbers: bool,
    first_visual_line: bool,
    gutter_width: usize,
) -> Option<String> {
    let range = document.numbered_line_range.as_ref()?;
    if !show_line_numbers || gutter_width == 0 {
        return None;
    }
    let number_width = gutter_width.saturating_sub(2);
    let number = (first_visual_line && range.contains(&line_index))
        .then(|| line_index.saturating_sub(range.start).saturating_add(1))
        .map(|number| number.to_string())
        .unwrap_or_default();
    if gutter_width < 3 || number.len() > number_width {
        return Some(" ".repeat(gutter_width));
    }
    Some(format!("{number:>number_width$}  "))
}

fn wrapped_row_for_source_line(
    document: &PreviewDocument,
    content_width: usize,
    show_line_numbers: bool,
    source_line: usize,
) -> usize {
    let PreviewContent::Text(text) = &document.content else {
        return 0;
    };
    wrapped_text_layout(
        text,
        wrapped_content_width(document, content_width, show_line_numbers),
    )
    .iter()
    .position(|line| line.source_line >= source_line)
    .unwrap_or(0)
}

fn wrapped_text_layout(text: &HighlightedText, content_width: usize) -> Vec<WrappedTextLine> {
    let content_width = content_width.max(1);
    let mut wrapped = Vec::new();
    for (source_line, line) in text.lines.iter().enumerate() {
        let plain = line.plain_text();
        let characters = plain.chars().collect::<Vec<_>>();
        if characters.is_empty() {
            wrapped.push(WrappedTextLine {
                source_line,
                start_column: 0,
                end_column: 0,
                first_visual_line: true,
            });
            continue;
        }
        let mut start_column = 0;
        while start_column < characters.len() {
            let mut end_column = start_column;
            let mut line_width = 0_usize;
            let mut last_whitespace = None;
            while end_column < characters.len() {
                let character = characters[end_column];
                let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
                if line_width > 0 && line_width.saturating_add(character_width) > content_width {
                    break;
                }
                line_width = line_width.saturating_add(character_width);
                end_column += 1;
                if character.is_whitespace() {
                    last_whitespace = Some(end_column);
                }
            }
            if end_column < characters.len()
                && let Some(whitespace_end) = last_whitespace
                && whitespace_end > start_column
                && characters[start_column..whitespace_end]
                    .iter()
                    .any(|character| !character.is_whitespace())
            {
                end_column = whitespace_end;
            }
            wrapped.push(WrappedTextLine {
                source_line,
                start_column,
                end_column,
                first_visual_line: start_column == 0,
            });
            start_column = end_column;
        }
    }
    wrapped
}

fn highlighted_spans(
    line: &HighlightLine,
    start_column: usize,
    end_column: usize,
    palette: &Palette,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut span_start = 0_usize;
    for span in &line.spans {
        let span_length = span.text.chars().count();
        let span_end = span_start.saturating_add(span_length);
        let from = start_column.max(span_start);
        let to = end_column.min(span_end);
        if from < to {
            let text = span
                .text
                .chars()
                .skip(from - span_start)
                .take(to - from)
                .collect::<String>();
            let mut style = Style::default()
                .fg(role_color(span.role, palette))
                .bg(palette.panel_bg);
            if span.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if span.italic {
                style = style.add_modifier(Modifier::ITALIC);
            }
            spans.push(Span::styled(text, style));
        }
        span_start = span_end;
    }
    spans
}

fn role_color(role: HighlightRole, palette: &Palette) -> Color {
    match role {
        HighlightRole::Text => palette.text,
        HighlightRole::Muted => palette.overlay0,
        HighlightRole::Accent => palette.accent,
        HighlightRole::Green => palette.green,
        HighlightRole::Yellow => palette.yellow,
        HighlightRole::Red => palette.red,
        HighlightRole::Blue => palette.blue,
        HighlightRole::Teal => palette.teal,
        HighlightRole::Peach => palette.peach,
    }
}

fn render_visual(
    buffer: &mut Buffer,
    area: Rect,
    visual: &VisualPreview,
    palette: &Palette,
    viewport: VisualViewport,
) {
    let target_width = u32::from(area.width);
    let target_height = u32::from(area.height).saturating_mul(2);
    if target_width == 0 || target_height == 0 {
        return;
    }
    let fitted = image::imageops::thumbnail(&visual.pixels, target_width, target_height);
    let zoom_factor = 1_u32 << viewport.zoom;
    let resized = if viewport.zoom == 0 {
        fitted
    } else {
        image::imageops::thumbnail(
            &visual.pixels,
            fitted
                .width()
                .saturating_mul(zoom_factor)
                .min(visual.pixels.width()),
            fitted
                .height()
                .saturating_mul(zoom_factor)
                .min(visual.pixels.height()),
        )
    };
    let x_offset = visual_draw_offset(target_width, resized.width(), viewport.pan_x);
    let y_offset = visual_draw_offset(target_height, resized.height(), viewport.pan_y);

    for cell_y in 0..area.height {
        for cell_x in 0..area.width {
            let pixel_x = i64::from(cell_x) - x_offset;
            let top_y = i64::from(cell_y) * 2 - y_offset;
            let bottom_y = top_y + 1;
            let top = visual_pixel(&resized, pixel_x, top_y, palette.panel_bg);
            let bottom = visual_pixel(&resized, pixel_x, bottom_y, palette.panel_bg);
            let cell = &mut buffer[(area.x + cell_x, area.y + cell_y)];
            match (top, bottom) {
                (None, None) => {}
                (Some(top), None) => {
                    cell.set_symbol("▀");
                    cell.set_style(Style::default().fg(top).bg(palette.panel_bg));
                }
                (None, Some(bottom)) => {
                    cell.set_symbol("▄");
                    cell.set_style(Style::default().fg(bottom).bg(palette.panel_bg));
                }
                (Some(top), Some(bottom)) => {
                    cell.set_symbol("▀");
                    cell.set_style(Style::default().fg(top).bg(bottom));
                }
            }
        }
    }
}

fn visual_draw_offset(viewport_extent: u32, image_extent: u32, pan: i64) -> i64 {
    if image_extent <= viewport_extent {
        return i64::from(viewport_extent.saturating_sub(image_extent) / 2);
    }
    let max_origin = i64::from(image_extent - viewport_extent);
    let origin = (max_origin / 2).saturating_add(pan).clamp(0, max_origin);
    -origin
}

fn visual_pixel(
    image: &image::RgbaImage,
    x: i64,
    y: i64,
    panel_background: Color,
) -> Option<Color> {
    let x = u32::try_from(x).ok()?;
    let y = u32::try_from(y).ok()?;
    let pixel = image.get_pixel_checked(x, y)?.0;
    match pixel[3] {
        0..=15 => None,
        255 => Some(Color::Rgb(pixel[0], pixel[1], pixel[2])),
        alpha => {
            let Color::Rgb(background_red, background_green, background_blue) = panel_background
            else {
                return Some(Color::Rgb(pixel[0], pixel[1], pixel[2]));
            };
            let blend = |foreground: u8, background: u8| {
                let alpha = u16::from(alpha);
                ((u16::from(foreground) * alpha
                    + u16::from(background) * (u16::from(u8::MAX) - alpha))
                    / u16::from(u8::MAX)) as u8
            };
            Some(Color::Rgb(
                blend(pixel[0], background_red),
                blend(pixel[1], background_green),
                blend(pixel[2], background_blue),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::FakeHerdr;

    #[test]
    fn opener_uses_a_declared_popup_and_hex_encoded_paths() {
        let directory = tempfile::tempdir().expect("temporary workspace");
        let workspace = WorkspaceRoot::resolve(directory.path()).expect("workspace");
        let opener = HerdrPreviewOpener::new(FakeHerdr::new([Ok(serde_json::json!({}))]));
        let request = PreviewRequest::File {
            path: PathBuf::from("folder/file name.rs"),
            line: Some(42),
        };

        opener.open(&workspace, &request).expect("open popup");

        let calls = opener.client.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "plugin.pane.open");
        assert_eq!(calls[0].1["plugin_id"], PLUGIN_ID);
        assert_eq!(calls[0].1["entrypoint"], PREVIEW_ENTRYPOINT);
        assert_eq!(calls[0].1["placement"], "popup");
        assert_eq!(calls[0].1["focus"], true);
        assert!(calls[0].1.get("cwd").is_none());
        assert_eq!(calls[0].1["env"][KIND_ENV], "file");
        assert_eq!(calls[0].1["env"][LINE_ENV], "42");
        assert_eq!(
            decode_path(calls[0].1["env"][PATH_ENV].as_str().unwrap()).unwrap(),
            Path::new("folder/file name.rs")
        );
    }

    #[test]
    fn source_and_commit_openers_pass_repository_payloads() {
        let directory = tempfile::tempdir().expect("temporary workspace");
        let workspace = WorkspaceRoot::resolve(directory.path()).expect("workspace");
        let opener = HerdrPreviewOpener::new(FakeHerdr::new([
            Ok(serde_json::json!({})),
            Ok(serde_json::json!({})),
        ]));
        let oid = "a".repeat(40);

        opener
            .open(
                &workspace,
                &PreviewRequest::Source {
                    repository: PathBuf::from("nested-repository"),
                    path: PathBuf::from("nested-repository/notes.txt"),
                    group: SourceControlGroup::Changes,
                },
            )
            .expect("open source popup");

        opener
            .open(
                &workspace,
                &PreviewRequest::Commit {
                    repository: PathBuf::from("nested-repository"),
                    oid: oid.clone(),
                },
            )
            .expect("open commit popup");

        let calls = opener.client.calls();
        assert_eq!(calls[0].1["env"][KIND_ENV], "source");
        assert_eq!(calls[0].1["env"][GROUP_ENV], "changes");
        assert_eq!(
            decode_path(calls[0].1["env"][REPOSITORY_ENV].as_str().unwrap()).unwrap(),
            Path::new("nested-repository")
        );
        assert_eq!(calls[1].1["env"][KIND_ENV], "commit");
        assert_eq!(calls[1].1["env"][OID_ENV], oid);
        assert_eq!(
            decode_path(calls[1].1["env"][REPOSITORY_ENV].as_str().unwrap()).unwrap(),
            Path::new("nested-repository")
        );
        assert!(calls[1].1["env"].get(PATH_ENV).is_none());
    }

    #[test]
    fn commit_document_loads_local_metadata_stat_and_patch() {
        let directory = tempfile::tempdir().expect("temporary workspace");
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(directory.path())
                .output()
                .expect("run git");
            assert!(output.status.success(), "git {args:?}");
            output.stdout
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(directory.path().join("notes.txt"), "committed\n").expect("fixture");
        git(&["add", "notes.txt"]);
        git(&["commit", "-qm", "preview history"]);
        let oid = String::from_utf8(git(&["rev-parse", "HEAD"]))
            .expect("UTF-8 oid")
            .trim()
            .to_string();
        let invocation = PreviewInvocation {
            workspace: WorkspaceRoot::resolve(directory.path()).expect("workspace"),
            request: PreviewRequest::Commit {
                repository: PathBuf::new(),
                oid: oid.clone(),
            },
        };

        let document = load_document(&invocation).expect("commit preview");
        let text = document.content.plain_text();

        assert_eq!(document.title, format!("Commit {}", &oid[..12]));
        assert!(text.contains("preview history"));
        assert!(text.contains("notes.txt | 1 +"));
        assert!(text.contains("+committed"));
        assert_eq!(document.numbered_line_range, None);
    }

    #[test]
    fn source_and_commit_previews_use_the_requested_nested_repository() {
        let workspace_directory = tempfile::tempdir().expect("temporary workspace");
        let create_repository = |name: &str, content: &str, subject: &str| {
            let repository = workspace_directory.path().join(name);
            std::fs::create_dir(&repository).expect("create repository");
            let git = |args: &[&str]| {
                let output = std::process::Command::new("git")
                    .args(args)
                    .current_dir(&repository)
                    .output()
                    .expect("run git");
                assert!(output.status.success(), "git {args:?}");
                output.stdout
            };
            git(&["init", "-q"]);
            git(&["config", "user.email", "test@example.invalid"]);
            git(&["config", "user.name", "Test"]);
            std::fs::write(repository.join("same.txt"), "base\n").expect("fixture");
            git(&["add", "same.txt"]);
            git(&["commit", "-qm", subject]);
            std::fs::write(repository.join("same.txt"), content).expect("fixture");
            repository
        };
        let repository_a = create_repository("repository-a", "staged in a\n", "commit in a");
        let repository_b = create_repository("repository-b", "worktree in b\n", "commit in b");
        let git_a = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&repository_a)
                .output()
                .expect("run git");
            assert!(output.status.success(), "git {args:?}");
            output.stdout
        };
        git_a(&["add", "same.txt"]);
        let oid = String::from_utf8(git_a(&["rev-parse", "HEAD"]))
            .expect("UTF-8 oid")
            .trim()
            .to_string();
        let workspace =
            WorkspaceRoot::from_directory(workspace_directory.path()).expect("workspace");

        let staged = load_document(&PreviewInvocation {
            workspace: workspace.clone(),
            request: PreviewRequest::Source {
                repository: PathBuf::from("repository-a"),
                path: PathBuf::from("repository-a/same.txt"),
                group: SourceControlGroup::StagedChanges,
            },
        })
        .expect("staged preview");
        let worktree = load_document(&PreviewInvocation {
            workspace: workspace.clone(),
            request: PreviewRequest::Source {
                repository: PathBuf::from("repository-b"),
                path: PathBuf::from("repository-b/same.txt"),
                group: SourceControlGroup::Changes,
            },
        })
        .expect("worktree preview");
        let commit = load_document(&PreviewInvocation {
            workspace,
            request: PreviewRequest::Commit {
                repository: PathBuf::from("repository-a"),
                oid: oid.clone(),
            },
        })
        .expect("commit preview");

        assert!(staged.content.plain_text().contains("+staged in a"));
        assert!(!staged.content.plain_text().contains("worktree in b"));
        assert!(worktree.content.plain_text().contains("+worktree in b"));
        assert!(!worktree.content.plain_text().contains("staged in a"));
        assert!(commit.content.plain_text().contains("commit in a"));
        assert!(!commit.content.plain_text().contains("commit in b"));
        assert_eq!(
            commit.title,
            format!("Commit {} · repository-a", &oid[..12])
        );
        let _ = repository_b;
    }

    #[test]
    fn repository_preview_rejects_paths_outside_its_exact_worktree_root() {
        let workspace_directory = tempfile::tempdir().expect("temporary workspace");
        let repository = workspace_directory.path().join("repository");
        std::fs::create_dir(&repository).expect("create repository");
        let output = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repository)
            .output()
            .expect("run git");
        assert!(output.status.success());
        let workspace =
            WorkspaceRoot::from_directory(workspace_directory.path()).expect("workspace");

        assert!(resolve_repository(&workspace, Path::new("repository")).is_ok());
        assert!(resolve_repository(&workspace, Path::new("repository/.git")).is_err());
        assert!(resolve_repository(&workspace, Path::new("../repository")).is_err());
        assert!(resolve_repository(&workspace, Path::new("/tmp/repository")).is_err());
        assert!(path_in_repository(Path::new("other/file.txt"), Path::new("repository")).is_err());
        for path in [
            PathBuf::from("../outside.txt"),
            PathBuf::from("/tmp/outside.txt"),
            PathBuf::from("other/file.txt"),
        ] {
            assert!(
                load_document(&PreviewInvocation {
                    workspace: workspace.clone(),
                    request: PreviewRequest::Source {
                        repository: PathBuf::from("repository"),
                        path,
                        group: SourceControlGroup::Changes,
                    },
                })
                .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn repository_preview_rejects_a_symlink_escape() {
        use std::os::unix::fs::symlink;

        let workspace_directory = tempfile::tempdir().expect("temporary workspace");
        let outside_directory = tempfile::tempdir().expect("outside workspace");
        symlink(
            outside_directory.path(),
            workspace_directory.path().join("escape"),
        )
        .expect("symlink");
        let workspace =
            WorkspaceRoot::from_directory(workspace_directory.path()).expect("workspace");

        assert!(resolve_repository(&workspace, Path::new("escape")).is_err());
    }

    #[test]
    fn file_document_is_root_bound_and_starts_near_a_search_match() {
        let directory = tempfile::tempdir().expect("temporary workspace");
        std::fs::write(
            directory.path().join("notes.txt"),
            (1..=20)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .expect("fixture");
        let invocation = PreviewInvocation {
            workspace: WorkspaceRoot::resolve(directory.path()).expect("workspace"),
            request: PreviewRequest::File {
                path: PathBuf::from("notes.txt"),
                line: Some(12),
            },
        };

        let document = load_document(&invocation).expect("preview");

        assert!(document.content.plain_text().contains("line 12"));
        assert!(!document.content.plain_text().contains("  12  line 12"));
        assert_eq!(document.initial_line, 8);
        assert_eq!(document.numbered_line_range, Some(0..20));
        let escaping = PreviewInvocation {
            workspace: invocation.workspace,
            request: PreviewRequest::File {
                path: PathBuf::from("../outside.txt"),
                line: None,
            },
        };
        assert!(load_document(&escaping).is_err());
    }

    #[test]
    fn popup_renderer_is_content_only_and_sanitizes_content() {
        let area = Rect::new(0, 0, 32, 5);
        let mut buffer = Buffer::empty(area);
        let document = PreviewDocument {
            title: "src/main.rs".to_string(),
            content: plain_content("first\nsecond\u{1b}[31m", HighlightRole::Text),
            initial_line: 0,
            is_error: false,
            numbered_line_range: None,
        };

        render_preview(&mut buffer, area, &document, 0, 0, &Palette::catppuccin());

        let rendered = (0..area.height)
            .map(|row| {
                (0..area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains("Preview"));
        assert!(!rendered.contains("src/main.rs"));
        assert!(!rendered.contains("Esc close"));
        assert!(rendered.contains("second�[31m"));
        assert!(!rendered.contains("Explorer"));
    }

    #[test]
    fn arrows_and_page_keys_scroll_the_preview_viewport() {
        let mut document = PreviewDocument {
            title: "long.txt".to_string(),
            content: plain_content(
                &(1..=100)
                    .map(|line| format!("line {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                HighlightRole::Text,
            ),
            initial_line: 0,
            is_error: false,
            numbered_line_range: None,
        };
        let mut vertical = 0;
        let mut visual_viewport = VisualViewport::default();
        let vertical_limit = max_vertical_scroll(&document, 10, 40, false);
        let mut key = |code, vertical: &mut usize| {
            handle_preview_key(
                crossterm::event::KeyEvent::new(code, KeyModifiers::NONE),
                &mut document,
                10,
                vertical_limit,
                vertical,
                &mut visual_viewport,
            )
        };

        assert!(!key(KeyCode::Down, &mut vertical));
        assert_eq!(vertical, 1);
        assert!(!key(KeyCode::PageDown, &mut vertical));
        assert_eq!(vertical, 11);
        assert!(!key(KeyCode::PageUp, &mut vertical));
        assert_eq!(vertical, 1);
        assert!(!key(KeyCode::End, &mut vertical));
        assert_eq!(vertical, 90);
        assert!(!key(KeyCode::Home, &mut vertical));
        assert_eq!(vertical, 0);
        assert!(!key(KeyCode::Up, &mut vertical));
        assert_eq!(vertical, 0);
        assert!(key(KeyCode::Esc, &mut vertical));
        assert!(key(KeyCode::Char(' '), &mut vertical));
    }

    #[test]
    fn long_lines_soft_wrap_without_changing_select_all_text() {
        let source = format!("{}complete", "x".repeat(24));
        let highlighted = HighlightedText::plain(&source, HighlightRole::Text);
        let selection = select_all(&highlighted).expect("non-empty selection");
        let document = PreviewDocument {
            title: "long.txt".to_string(),
            content: PreviewContent::Text(highlighted),
            initial_line: 0,
            is_error: false,
            numbered_line_range: Some(0..1),
        };
        let area = Rect::new(0, 0, 10, 4);
        let mut buffer = Buffer::empty(area);

        render_preview(&mut buffer, area, &document, 0, 0, &Palette::catppuccin());

        let rendered = (0..area.height)
            .map(|row| {
                (0..area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered[0], "xxxxxxxxxx");
        assert_eq!(rendered[1], "xxxxxxxxxx");
        assert_eq!(rendered[2], "xxxxcomple");
        assert_eq!(rendered[3].trim_end(), "te");
        let PreviewContent::Text(text) = &document.content else {
            panic!("text preview");
        };
        assert_eq!(selected_text(text, selection), Some(source));
        assert_eq!(max_vertical_scroll(&document, 2, 10, false), 2);
    }

    #[test]
    fn prose_wraps_at_whitespace_and_numbers_only_the_first_visual_row() {
        let document = PreviewDocument {
            title: "notes.txt".to_string(),
            content: plain_content("alpha beta", HighlightRole::Text),
            initial_line: 0,
            is_error: false,
            numbered_line_range: Some(0..1),
        };
        let area = Rect::new(0, 0, 13, 2);
        let palette = Palette::catppuccin();
        let mut buffer = Buffer::empty(area);

        render_preview_with_selection(
            &mut buffer,
            area,
            &document,
            0,
            &palette,
            PreviewRenderState {
                selection: None,
                visual_viewport: VisualViewport::default(),
                show_line_numbers: true,
            },
        );

        assert_eq!(buffer[(3, 0)].symbol(), "1");
        assert_eq!(buffer[(3, 1)].symbol(), " ");
        assert_eq!(buffer[(6, 0)].symbol(), "a");
        assert_eq!(buffer[(6, 1)].symbol(), "b");
        assert_eq!(
            wrapped_text_layout(
                match &document.content {
                    PreviewContent::Text(text) => text,
                    PreviewContent::Visual(_) => panic!("text preview"),
                },
                7,
            ),
            vec![
                WrappedTextLine {
                    source_line: 0,
                    start_column: 0,
                    end_column: 6,
                    first_visual_line: true,
                },
                WrappedTextLine {
                    source_line: 0,
                    start_column: 6,
                    end_column: 10,
                    first_visual_line: false,
                },
            ]
        );
    }

    #[test]
    fn mouse_drag_selects_visible_multiline_text_for_copy() {
        let document = PreviewDocument {
            title: "notes.txt".to_string(),
            content: plain_content("alpha\nbeta", HighlightRole::Text),
            initial_line: 0,
            is_error: false,
            numbered_line_range: Some(0..2),
        };
        let mut selection = None;
        let area = Rect::new(0, 0, 16, 2);

        assert!(handle_selection_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 7,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            &document,
            area,
            0,
            true,
            &mut selection,
        ));
        assert!(handle_selection_mouse(
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 8,
                row: 1,
                modifiers: KeyModifiers::NONE,
            },
            &document,
            area,
            0,
            true,
            &mut selection,
        ));

        let PreviewContent::Text(text) = &document.content else {
            panic!("text preview");
        };
        assert_eq!(
            selection.and_then(|selection| selected_text(text, selection)),
            Some("lpha\nbet".to_string())
        );
    }

    #[test]
    fn mouse_drag_selection_maps_wrapped_rows_back_to_source_text() {
        let document = PreviewDocument {
            title: "notes.txt".to_string(),
            content: plain_content("abcdefghij", HighlightRole::Text),
            initial_line: 0,
            is_error: false,
            numbered_line_range: None,
        };
        let mut selection = None;
        let area = Rect::new(0, 0, 4, 3);

        assert!(handle_selection_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 1,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            &document,
            area,
            0,
            false,
            &mut selection,
        ));
        assert!(handle_selection_mouse(
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 1,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            &document,
            area,
            0,
            false,
            &mut selection,
        ));

        let PreviewContent::Text(text) = &document.content else {
            panic!("text preview");
        };
        assert_eq!(
            selection.and_then(|selection| selected_text(text, selection)),
            Some("bcdefghij".to_string())
        );
    }

    #[test]
    fn line_numbers_default_off_toggle_without_changing_copy_or_selection() {
        let highlighted =
            HighlightedText::source(Path::new("main.rs"), "fn main() {}", false, false)
                .expect("highlight");
        let selection = select_all(&highlighted).expect("non-empty selection");
        assert_eq!(
            selected_text(&highlighted, selection),
            Some("fn main() {}".to_string())
        );
        let document = PreviewDocument {
            title: "main.rs".to_string(),
            content: PreviewContent::Text(highlighted),
            initial_line: 0,
            is_error: false,
            numbered_line_range: Some(0..1),
        };
        let area = Rect::new(0, 0, 20, 1);
        let palette = Palette::catppuccin();
        let mut default_buffer = Buffer::empty(area);

        render_preview(&mut default_buffer, area, &document, 0, 0, &palette);
        assert_eq!(default_buffer[(0, 0)].symbol(), "f");

        let mut show_line_numbers = false;
        assert!(handle_line_number_toggle(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &document,
            &mut show_line_numbers,
        ));
        assert!(show_line_numbers);

        let mut numbered_buffer = Buffer::empty(area);
        render_preview_with_selection(
            &mut numbered_buffer,
            area,
            &document,
            0,
            &palette,
            PreviewRenderState {
                selection: Some(selection),
                visual_viewport: VisualViewport::default(),
                show_line_numbers,
            },
        );

        assert_eq!(numbered_buffer[(3, 0)].symbol(), "1");
        assert_eq!(numbered_buffer[(6, 0)].symbol(), "f");
        assert_ne!(numbered_buffer[(3, 0)].bg, palette.surface1);
        assert_eq!(numbered_buffer[(6, 0)].bg, palette.surface1);
        assert_ne!(numbered_buffer[(6, 0)].fg, palette.surface1);

        assert!(handle_line_number_toggle(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &document,
            &mut show_line_numbers,
        ));
        assert!(!show_line_numbers);
    }

    #[test]
    fn preview_copy_and_select_all_accept_command_control_and_vim_keys() {
        assert!(select_all_requested(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::SUPER
        )));
        assert!(select_all_requested(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL
        )));
        assert!(copy_requested(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::SUPER
        )));
        assert!(copy_requested(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(copy_requested(KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE
        )));
        assert!(!copy_requested(KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::SUPER
        )));
    }

    #[test]
    fn preview_help_renders_one_action_per_line() {
        let area = Rect::new(0, 0, 60, 15);
        let mut buffer = Buffer::empty(area);

        render_preview_help(&mut buffer, area, &Palette::catppuccin(), false, true);

        let rendered = (0..area.height)
            .map(|row| {
                (0..area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered[0].trim(), "Preview shortcuts");
        assert!(rendered[1].contains("Mouse drag"));
        assert!(rendered[2].contains("Cmd+A / Ctrl+A"));
        assert!(rendered[3].contains("Cmd+C / Ctrl+C"));
        assert!(rendered[4].trim_start().starts_with('y'));
        assert!(rendered[5].contains("toggle line numbers"));
        assert!(rendered[12].trim_start().starts_with('?'));
        assert!(rendered[13].contains("Cmd keys require terminal forwarding"));
    }

    #[test]
    fn visual_help_exposes_zoom_pan_pdf_and_native_preview_controls() {
        let area = Rect::new(0, 0, 60, 11);
        let mut buffer = Buffer::empty(area);

        render_preview_help(&mut buffer, area, &Palette::catppuccin(), true, false);

        let rendered = (0..area.height)
            .map(|row| {
                (0..area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(rendered[1].contains("+ / -"));
        assert!(rendered[2].contains("fit to window"));
        assert!(rendered[3].contains("pan when zoomed"));
        assert!(rendered[5].contains("PDF page"));
        assert!(rendered[7].trim_start().starts_with('o'));
        assert!(rendered[7].contains(native_visual_action()));
        assert!(rendered[10].trim_start().starts_with('?'));
    }

    #[test]
    fn visual_viewport_zoom_pan_and_fit_are_bounded() {
        let mut viewport = VisualViewport::default();

        viewport.pan(8, 8);
        assert_eq!(viewport, VisualViewport::default());
        for _ in 0..10 {
            viewport.zoom_in();
        }
        assert_eq!(viewport.zoom, MAX_VISUAL_ZOOM);
        viewport.pan(VISUAL_PAN_STEP, -VISUAL_PAN_STEP);
        assert_eq!(
            (viewport.pan_x, viewport.pan_y),
            (VISUAL_PAN_STEP, -VISUAL_PAN_STEP)
        );
        viewport.zoom_out();
        assert_eq!(viewport.zoom, MAX_VISUAL_ZOOM - 1);
        viewport.reset();
        assert_eq!(viewport, VisualViewport::default());

        assert_eq!(visual_draw_offset(10, 6, 100), 2);
        assert_eq!(visual_draw_offset(10, 30, 0), -10);
        assert_eq!(visual_draw_offset(10, 30, -100), 0);
        assert_eq!(visual_draw_offset(10, 30, 100), -20);
    }

    #[test]
    fn visual_keys_zoom_pan_and_reset_without_changing_text_scroll() {
        let pixels = image::RgbaImage::new(100, 100);
        let mut document = PreviewDocument {
            title: "design.png".to_string(),
            content: PreviewContent::Visual(VisualPreview::raster(pixels, 100, 100)),
            initial_line: 0,
            is_error: false,
            numbered_line_range: None,
        };
        let mut vertical = 0;
        let mut viewport = VisualViewport::default();

        assert!(!handle_preview_key(
            KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE),
            &mut document,
            10,
            0,
            &mut vertical,
            &mut viewport,
        ));
        assert_eq!(viewport.zoom, 1);
        assert!(!handle_preview_key(
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &mut document,
            10,
            0,
            &mut vertical,
            &mut viewport,
        ));
        assert!(!handle_preview_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut document,
            10,
            0,
            &mut vertical,
            &mut viewport,
        ));
        assert_eq!(
            (viewport.pan_x, viewport.pan_y),
            (VISUAL_PAN_STEP, VISUAL_PAN_STEP)
        );
        assert_eq!(vertical, 0);
        assert!(!handle_preview_key(
            KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE),
            &mut document,
            10,
            0,
            &mut vertical,
            &mut viewport,
        ));
        assert_eq!(viewport, VisualViewport::default());
    }

    #[test]
    fn visual_preview_uses_half_blocks_with_truecolor_pixels() {
        let mut pixels = image::RgbaImage::new(1, 2);
        pixels.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        pixels.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        let document = PreviewDocument {
            title: "sample.png".to_string(),
            content: PreviewContent::Visual(VisualPreview::raster(pixels, 1, 2)),
            initial_line: 0,
            is_error: false,
            numbered_line_range: None,
        };
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);

        render_preview(&mut buffer, area, &document, 0, 0, &Palette::catppuccin());

        assert_eq!(buffer[(0, 0)].symbol(), "▀");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(255, 0, 0));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(0, 0, 255));
    }

    #[test]
    fn git_diff_preview_keeps_diff_and_language_semantics() {
        let highlighted = HighlightedText::diff(
            Path::new("src/main.rs"),
            "@@ -1 +1 @@\n-pub fn old() -> u32 { 1 }\n+pub fn new() -> u32 { 2 }",
        )
        .expect("highlight");
        let document = PreviewDocument {
            title: "Changes · src/main.rs".to_string(),
            content: PreviewContent::Text(highlighted),
            initial_line: 0,
            is_error: false,
            numbered_line_range: None,
        };
        let area = Rect::new(0, 0, 40, 3);
        let mut buffer = Buffer::empty(area);
        let palette = Palette::catppuccin();

        render_preview(&mut buffer, area, &document, 0, 0, &palette);

        assert_eq!(buffer[(0, 1)].symbol(), "-");
        assert_eq!(buffer[(0, 1)].fg, palette.red);
        assert_eq!(buffer[(0, 2)].symbol(), "+");
        assert_eq!(buffer[(0, 2)].fg, palette.green);
        assert!((1..area.width).any(|column| buffer[(column, 2)].fg == palette.blue));
        assert!((1..area.width).any(|column| buffer[(column, 2)].fg == palette.peach));
    }
}
