use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use notify::{RecursiveMode, Watcher};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::clipboard::{ClipboardWriter, HerdrClipboard};
use crate::config::{self, Config, LoadedConfig};
use crate::controller::{
    Action, Controller, register_sidebar_instance, unregister_sidebar_instance,
};
use crate::decoration::{GitCoordinates, GitState};
use crate::file_manager::{FileManagerOpener, SystemFileManagerOpener};
use crate::file_tree::{FileTree, NodeKind};
use crate::git::{
    DirectoryStatus, GitEntry, GitError, GitSnapshot, GitStatusProvider, SourceControlGroup,
    StatusCode,
};
use crate::herdr::{
    CompanionEvent, CompanionMonitor, HerdrClient, InvocationContext, LiveHerdr,
    layout_has_companion,
};
use crate::icons::{EntryKind, IconMode};
use crate::preview::{HerdrPreviewOpener, PreviewOpener, PreviewRequest};
use crate::render::{HitTargets, RenderModel, RenderRow, SearchControl, SearchScope, View};
use crate::search::{
    SearchHandle, SearchMode, SearchProvider, SearchQuery, SearchResults, SearchUpdate,
    fuzzy_filename_matches,
};
use crate::state::{GitViewMode, PersistedState, SidebarState, SidebarView};
use crate::theme::{Appearance, ThemeResolution};
use crate::workspace::WorkspaceRoot;

const REFRESH_DEBOUNCE: Duration = Duration::from_millis(180);
const CONFIG_POLL: Duration = Duration::from_millis(500);
const BUSY_FRAME_INTERVAL: Duration = Duration::from_millis(125);

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    register_sidebar_instance()?;
    let context = InvocationContext::from_env()?;
    let loaded_config = config::load_from_env()?;
    let cwd = context
        .focused_pane_cwd
        .as_deref()
        .or(context.workspace_cwd.as_deref())
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let workspace = WorkspaceRoot::resolve(&cwd)?;
    let state_path = crate::state::state_path_from_env()?;
    let state = PersistedState::load(&state_path)?;
    let workspace_id = std::env::var("HERDR_WORKSPACE_ID")
        .or(context.workspace_id.clone().ok_or("missing workspace id"))?;
    let tab_id =
        std::env::var("HERDR_TAB_ID").or(context.tab_id.clone().ok_or("missing tab id"))?;
    let pane_id = std::env::var("HERDR_PANE_ID")?;
    let companion_monitor = CompanionMonitor::from_env(&workspace_id, &tab_id, &pane_id)?;
    let herdr = LiveHerdr::from_env();
    let layout = herdr.request("pane.layout", serde_json::json!({"pane_id": pane_id}))?;
    if !layout_has_companion(&layout, &workspace_id, &tab_id, &pane_id)? {
        unregister_sidebar_instance()?;
        return Ok(());
    }

    let mut application = SidebarApp::new(
        context,
        workspace,
        workspace_id,
        tab_id,
        state_path,
        state,
        loaded_config,
        companion_monitor,
        load_theme(),
    )?;
    loop {
        let outcome = terminal_loop(&mut application)?;
        application.persist(outcome != LoopOutcome::Exit)?;
        match outcome {
            LoopOutcome::Exit => break,
            LoopOutcome::Hide => {
                Controller::from_env(LiveHerdr::from_env())?.execute(Action::Hide)?;
                break;
            }
            LoopOutcome::ToggleDock => {
                match Controller::from_env(LiveHerdr::from_env())
                    .and_then(|controller| controller.execute(Action::ToggleDock))
                {
                    Ok(()) => match PersistedState::load(&application.state_path) {
                        Ok(state) => application.persisted = state,
                        Err(error) => {
                            application.error =
                                Some(format!("Cannot reload sidebar state: {error}"));
                        }
                    },
                    Err(error) => {
                        application.error = Some(format!("Cannot move sidebar: {error}"));
                    }
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopOutcome {
    Exit,
    Hide,
    ToggleDock,
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}

fn terminal_loop(application: &mut SidebarApp) -> Result<LoopOutcome, Box<dyn std::error::Error>> {
    let _guard = TerminalGuard::enter()?;
    if terminal_appearance().is_none()
        && let Some(appearance) = query_terminal_appearance()
    {
        application.apply_terminal_appearance(appearance);
    }
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let mut hits = HitTargets::default();
    let animation_origin = Instant::now();

    loop {
        application.poll_background();
        if matches!(
            application.companion_monitor.try_recv(),
            Ok(CompanionEvent::SidebarBecameOnlyPane)
        ) {
            return Ok(LoopOutcome::Exit);
        }
        let mut model = application.render_model();
        model.busy_frame =
            (animation_origin.elapsed().as_millis() / BUSY_FRAME_INTERVAL.as_millis()) as usize;
        let palette = application.theme.palette.clone();
        let mut viewport_rows = application.viewport_rows;
        terminal.draw(|frame| {
            let area = frame.area();
            viewport_rows = viewport_rows_for_height(area.height, model.query.is_some());
            hits = crate::render::render(frame.buffer_mut(), area, &model, &palette);
            if let Some(position) = hits.query_cursor {
                frame.set_cursor_position(position);
            }
        })?;
        application.set_viewport_rows(viewport_rows);
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => {
                    if let Some(outcome) = application.handle_key(key)? {
                        return Ok(outcome);
                    }
                }
                Event::Mouse(mouse) => {
                    if let Some(outcome) = application.handle_mouse(mouse, &hits)? {
                        return Ok(outcome);
                    }
                }
                Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    SearchQuery,
}

#[derive(Debug, Clone)]
struct ExplorerItem {
    path: PathBuf,
    name: String,
    kind: NodeKind,
    expanded: bool,
    ignored: bool,
    error: Option<String>,
    depth: u16,
}

#[derive(Debug, Clone)]
enum SourceItem {
    Header(SourceControlGroup, usize),
    Directory {
        group: SourceControlGroup,
        path: PathBuf,
        depth: u16,
        expanded: bool,
        git: GitCoordinates,
    },
    Entry {
        group: SourceControlGroup,
        entry: Box<GitEntry>,
        depth: u16,
        name: String,
    },
}

#[derive(Debug, Clone)]
enum SourceSelection {
    Header(SourceControlGroup),
    Path(SourceControlGroup, PathBuf),
}

#[derive(Debug, Clone)]
enum SearchItem {
    Header(PathBuf, usize),
    Match {
        path: PathBuf,
        line: usize,
        snippet: String,
    },
}

struct SidebarApp {
    context: InvocationContext,
    workspace: WorkspaceRoot,
    workspace_id: String,
    tab_id: String,
    state_path: PathBuf,
    persisted: PersistedState,
    loaded_config: LoadedConfig,
    config: Config,
    editor: Option<OsString>,
    tree: FileTree,
    git: GitStatusProvider,
    git_snapshot: Option<GitSnapshot>,
    git_receiver: Option<mpsc::Receiver<Result<GitSnapshot, GitError>>>,
    git_error: Option<String>,
    search: SearchProvider,
    search_handle: Option<SearchHandle>,
    search_results: SearchResults,
    search_error: Option<String>,
    search_query: SearchQuery,
    search_active: bool,
    search_origin: Option<(usize, usize)>,
    view: View,
    git_view_mode: GitViewMode,
    git_tree_expanded: BTreeSet<String>,
    git_tree_initialized: bool,
    selection: usize,
    offset: usize,
    viewport_rows: usize,
    input_mode: InputMode,
    preview_opener: Box<dyn PreviewOpener>,
    file_manager_opener: Box<dyn FileManagerOpener>,
    clipboard: Box<dyn ClipboardWriter>,
    help: bool,
    error: Option<String>,
    busy: bool,
    theme: ThemeResolution,
    theme_path: Option<PathBuf>,
    theme_modified: Option<SystemTime>,
    config_modified: Option<SystemTime>,
    show_ignored: bool,
    _watcher: Option<notify::RecommendedWatcher>,
    watch_events: mpsc::Receiver<notify::Result<notify::Event>>,
    pending_refresh: Option<Instant>,
    last_config_poll: Instant,
    companion_monitor: CompanionMonitor,
}

impl SidebarApp {
    #[allow(clippy::too_many_arguments)]
    fn new(
        context: InvocationContext,
        workspace: WorkspaceRoot,
        workspace_id: String,
        tab_id: String,
        state_path: PathBuf,
        persisted: PersistedState,
        loaded_config: LoadedConfig,
        companion_monitor: CompanionMonitor,
        theme_state: (Option<PathBuf>, ThemeResolution, Option<SystemTime>),
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let config = loaded_config.config.clone();
        let editor = std::env::var_os("EDITOR").filter(|value| !value.is_empty());
        let show_ignored = config.show_ignored;
        let search_query = SearchQuery {
            include_ignored: show_ignored,
            ..SearchQuery::default()
        };
        let saved = persisted
            .tab(&workspace_id, &tab_id)
            .cloned()
            .unwrap_or_default();
        let mut tree = FileTree::new(
            workspace.clone(),
            config.show_ignored,
            config.follow_symlinks,
        );
        tree.set_show_git_directory(config.show_git_directory);
        tree.expand(Path::new(""))?;
        for expanded in &saved.expanded {
            let _ = tree.expand(Path::new(expanded));
        }
        let visible = tree.visible_nodes();
        let selection = saved
            .selection
            .as_deref()
            .and_then(|selected| {
                visible
                    .iter()
                    .filter(|node| !node.path.as_os_str().is_empty())
                    .position(|node| node.path == Path::new(selected))
            })
            .unwrap_or(0);
        let view = match saved.view {
            SidebarView::Explorer => View::Explorer,
            SidebarView::SourceControl => View::SourceControl,
        };
        let (theme_path, theme, theme_modified) = theme_state;
        let git = GitStatusProvider::new(workspace.path());
        let git_receiver = workspace.is_git_worktree.then(|| git.refresh_async());
        let busy = git_receiver.is_some();
        let search = SearchProvider::new(workspace.path());
        let (sender, receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })
        .ok();
        if let Some(active_watcher) = watcher.as_mut() {
            let _ = active_watcher.watch(workspace.path(), RecursiveMode::Recursive);
            if let Some(path) = loaded_config.path.parent() {
                let _ = active_watcher.watch(path, RecursiveMode::NonRecursive);
            }
            if let Some(path) = theme_path.as_ref().and_then(|path| path.parent()) {
                let _ = active_watcher.watch(path, RecursiveMode::NonRecursive);
            }
        }
        let config_modified = file_modified(&loaded_config.path);
        Ok(Self {
            context,
            workspace,
            workspace_id,
            tab_id,
            state_path,
            persisted,
            loaded_config,
            config,
            editor,
            tree,
            git,
            git_snapshot: None,
            git_receiver,
            git_error: None,
            search,
            search_handle: None,
            search_results: SearchResults::default(),
            search_error: None,
            search_query,
            search_active: false,
            search_origin: None,
            view,
            git_view_mode: saved.git_view_mode,
            git_tree_expanded: saved.git_tree_expanded.clone(),
            git_tree_initialized: saved.git_tree_initialized,
            selection,
            offset: saved.scroll,
            viewport_rows: 1,
            input_mode: InputMode::Normal,
            preview_opener: Box::new(HerdrPreviewOpener::new(LiveHerdr::from_env())),
            file_manager_opener: Box::new(SystemFileManagerOpener),
            clipboard: Box::new(HerdrClipboard),
            help: false,
            error: theme.diagnostic.clone(),
            busy,
            theme,
            theme_path,
            theme_modified,
            config_modified,
            show_ignored,
            _watcher: watcher,
            watch_events: receiver,
            pending_refresh: None,
            last_config_poll: Instant::now(),
            companion_monitor,
        })
    }

    fn render_model(&self) -> RenderModel {
        let rows = match self.view {
            View::Explorer if self.uses_filtered_explorer_rows() => self.filtered_explorer_rows(),
            View::Explorer if self.search_active => self.search_rows(),
            View::Explorer => self.explorer_rows(),
            View::SourceControl => self.source_rows(),
        };
        RenderModel {
            view: self.view,
            git_available: self.workspace.is_git_worktree,
            rows,
            offset: self.offset,
            icon_mode: match self.config.icons {
                config::IconMode::NerdFont => IconMode::NerdFont,
                config::IconMode::Plain => IconMode::Plain,
            },
            query: self.search_active.then(|| self.search_query.text.clone()),
            query_input_active: self.search_active && self.input_mode == InputMode::SearchQuery,
            search_scope: if self.search_query.mode == SearchMode::Filename {
                SearchScope::Files
            } else {
                SearchScope::Contents
            },
            search_case_sensitive: self.search_query.case_sensitive,
            search_regex: self.search_query.mode == SearchMode::Regex,
            notice: (self.view == View::SourceControl && !self.workspace.is_git_worktree)
                .then(|| "No Git repository\nThis folder is not tracked by Git.".to_string()),
            error: self
                .error
                .clone()
                .or_else(|| {
                    self.search_active
                        .then(|| self.search_error.clone())
                        .flatten()
                })
                .or_else(|| {
                    (self.view == View::SourceControl && self.workspace.is_git_worktree)
                        .then(|| self.git_error.clone())
                        .flatten()
                }),
            help: self.help,
            busy: self.busy,
            search_busy: self.search_handle.is_some(),
            busy_frame: 0,
            git_view_mode: self.git_view_mode,
        }
    }

    fn explorer_items(&self) -> Vec<ExplorerItem> {
        self.tree
            .visible_nodes()
            .into_iter()
            .filter(|node| !node.path.as_os_str().is_empty())
            .map(|node| ExplorerItem {
                path: node.path.clone(),
                name: node.name.clone(),
                kind: node.kind,
                expanded: node.expanded,
                ignored: node.ignored,
                error: node.error.clone(),
                depth: u16::try_from(node.path.components().count().saturating_sub(1))
                    .unwrap_or(u16::MAX),
            })
            .collect()
    }

    fn explorer_rows(&self) -> Vec<RenderRow> {
        self.explorer_items()
            .into_iter()
            .enumerate()
            .map(|(index, item)| self.explorer_row(index, item))
            .collect()
    }

    fn uses_filtered_explorer_rows(&self) -> bool {
        self.search_active
            && (self.search_query.text.is_empty() || self.search_query.mode == SearchMode::Filename)
    }

    fn filtered_explorer_items(&self) -> Vec<ExplorerItem> {
        let mut items = self.explorer_items();
        if !self.search_query.text.is_empty() {
            items.retain(|item| {
                fuzzy_filename_matches(
                    &item.name,
                    &self.search_query.text,
                    self.search_query.case_sensitive,
                )
            });
        }
        items
    }

    fn filtered_explorer_rows(&self) -> Vec<RenderRow> {
        self.filtered_explorer_items()
            .into_iter()
            .enumerate()
            .map(|(index, item)| self.explorer_row(index, item))
            .collect()
    }

    fn explorer_row(&self, index: usize, item: ExplorerItem) -> RenderRow {
        RenderRow {
            name: item.name,
            path: item.path.to_string_lossy().into_owned(),
            kind: entry_kind(item.kind),
            git: self.git_coordinates(&item.path, item.kind, item.ignored),
            expanded: item.expanded,
            depth: item.depth,
            selected: index == self.selection,
            focused: index == self.selection,
        }
    }

    fn search_items(&self) -> Vec<SearchItem> {
        self.search_results
            .files
            .iter()
            .flat_map(|file| {
                std::iter::once(SearchItem::Header(file.path.clone(), file.matches.len())).chain(
                    file.matches.iter().map(|matched| SearchItem::Match {
                        path: file.path.clone(),
                        line: matched.line,
                        snippet: matched.snippet.clone(),
                    }),
                )
            })
            .collect()
    }

    fn search_rows(&self) -> Vec<RenderRow> {
        self.search_items()
            .into_iter()
            .enumerate()
            .map(|(index, item)| match item {
                SearchItem::Header(path, count) => RenderRow {
                    name: format!("{} ({count})", path.display()),
                    path: path.to_string_lossy().into_owned(),
                    kind: EntryKind::Directory,
                    git: self.git_coordinates(&path, NodeKind::Directory, false),
                    expanded: true,
                    depth: 0,
                    selected: index == self.selection,
                    focused: index == self.selection,
                },
                SearchItem::Match {
                    path,
                    line,
                    snippet,
                } => RenderRow {
                    name: if line == 0 {
                        path.display().to_string()
                    } else {
                        format!("{line}: {snippet}")
                    },
                    path: path.to_string_lossy().into_owned(),
                    kind: EntryKind::File,
                    git: self.git_coordinates(&path, NodeKind::File, false),
                    expanded: false,
                    depth: 1,
                    selected: index == self.selection,
                    focused: index == self.selection,
                },
            })
            .collect()
    }

    fn source_items(&self) -> Vec<SourceItem> {
        let Some(snapshot) = &self.git_snapshot else {
            return Vec::new();
        };
        let groups = snapshot.groups();
        let mut items = Vec::new();
        for group in [
            SourceControlGroup::MergeChanges,
            SourceControlGroup::StagedChanges,
            SourceControlGroup::Changes,
            SourceControlGroup::Untracked,
        ] {
            let entries = groups.get(&group).cloned().unwrap_or_default();
            if entries.is_empty() && !self.config.show_empty_git_groups {
                continue;
            }
            items.push(SourceItem::Header(group, entries.len()));
            match self.git_view_mode {
                GitViewMode::Flat => {
                    items.extend(entries.into_iter().cloned().map(|entry| SourceItem::Entry {
                        group,
                        name: entry.path.display().to_string(),
                        entry: Box::new(entry),
                        depth: 1,
                    }));
                }
                GitViewMode::Tree => append_source_tree_items(
                    &mut items,
                    group,
                    Path::new(""),
                    &entries,
                    1,
                    &self.git_tree_expanded,
                ),
            }
        }
        items
    }

    fn source_rows(&self) -> Vec<RenderRow> {
        self.source_items()
            .into_iter()
            .enumerate()
            .map(|(index, item)| match item {
                SourceItem::Header(group, count) => RenderRow {
                    name: format!("{} ({count})", group_label(group)),
                    path: String::new(),
                    kind: EntryKind::Directory,
                    git: GitCoordinates::default(),
                    expanded: true,
                    depth: 0,
                    selected: index == self.selection,
                    focused: index == self.selection,
                },
                SourceItem::Directory {
                    path,
                    depth,
                    expanded,
                    git,
                    ..
                } => RenderRow {
                    name: path
                        .file_name()
                        .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                    path: path.to_string_lossy().into_owned(),
                    kind: EntryKind::Directory,
                    git,
                    expanded,
                    depth,
                    selected: index == self.selection,
                    focused: index == self.selection,
                },
                SourceItem::Entry {
                    group,
                    entry,
                    depth,
                    name,
                } => RenderRow {
                    name,
                    path: entry.path.to_string_lossy().into_owned(),
                    kind: EntryKind::File,
                    git: coordinates_for_source_entry(&entry, group),
                    expanded: false,
                    depth,
                    selected: index == self.selection,
                    focused: index == self.selection,
                },
            })
            .collect()
    }

    fn git_coordinates(&self, path: &Path, kind: NodeKind, ignored: bool) -> GitCoordinates {
        if kind == NodeKind::Directory {
            let aggregate = self
                .git_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.directories.get(path))
                .map(|summary| summary.status);
            return coordinates_for_directory(aggregate, ignored);
        }
        if ignored {
            return GitCoordinates {
                index: GitState::Clean,
                worktree: GitState::Ignored,
            };
        }
        self.git_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.entries.iter().find(|entry| entry.path == path))
            .map(coordinates_for_entry)
            .unwrap_or_default()
    }

    fn handle_key(&mut self, key: KeyEvent) -> AppResult<Option<LoopOutcome>> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(Some(LoopOutcome::Exit));
        }
        if self.error.is_some() {
            if key.code == KeyCode::Esc {
                self.error = None;
                if self.search_active {
                    self.close_inline_search();
                }
            }
            return Ok(None);
        }
        if self.help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.help = false;
            }
            return Ok(None);
        }
        if self.input_mode != InputMode::Normal {
            return self.handle_text_input(key);
        }
        match key.code {
            KeyCode::Char('q') => return Ok(Some(LoopOutcome::Hide)),
            KeyCode::Char('d') => return Ok(Some(LoopOutcome::ToggleDock)),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('1') => self.switch_view(View::Explorer),
            KeyCode::Char('2') => self.switch_view(View::SourceControl),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => self.previous_view(),
            KeyCode::BackTab => self.previous_view(),
            KeyCode::Tab => self.next_view(),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Esc => {
                if self.search_active {
                    self.close_inline_search();
                }
            }
            other => match self.view {
                View::Explorer if self.search_active => self.handle_search_key(other)?,
                View::Explorer => self.handle_explorer_key(other)?,
                View::SourceControl => self.handle_source_key(other)?,
            },
        }
        self.keep_selection_visible();
        Ok(None)
    }

    fn handle_text_input(&mut self, key: KeyEvent) -> AppResult<Option<LoopOutcome>> {
        if self.input_mode != InputMode::SearchQuery {
            return Ok(None);
        }
        match key.code {
            KeyCode::Esc => self.close_inline_search(),
            KeyCode::Tab | KeyCode::BackTab => self.toggle_search_scope(),
            KeyCode::Enter => {
                self.input_mode = InputMode::Normal;
                self.selection = 0;
                self.offset = 0;
            }
            KeyCode::Backspace => {
                self.search_query.text.pop();
                self.reset_search_selection();
                self.start_search();
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.toggle_search_case();
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.toggle_search_regex();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.search_query.text.push(character);
                self.selection = 0;
                self.offset = 0;
                self.start_search();
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_explorer_key(&mut self, key: KeyCode) -> AppResult<()> {
        match key {
            KeyCode::Char('/') => self.open_inline_search(),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('i') => {
                self.set_show_ignored(!self.show_ignored);
            }
            KeyCode::Left | KeyCode::Char('h') => self.collapse_or_parent(),
            KeyCode::Right | KeyCode::Char('l') => self.expand_or_child(),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_explorer(),
            KeyCode::Char('o') => self.open_selected_explorer(),
            KeyCode::Char('f') => self.reveal_selected_explorer(),
            KeyCode::Char('y') => self.copy_selected_explorer_path(),
            _ => {}
        }
        Ok(())
    }

    fn handle_search_key(&mut self, key: KeyCode) -> AppResult<()> {
        match key {
            KeyCode::Char('/') => self.input_mode = InputMode::SearchQuery,
            KeyCode::Enter if self.uses_filtered_explorer_rows() => {
                self.activate_filtered_explorer();
            }
            KeyCode::Enter => self.preview_selected_search(),
            KeyCode::Char('o') if self.uses_filtered_explorer_rows() => {
                self.open_selected_filtered_explorer();
            }
            KeyCode::Char('o') => self.open_selected_search(),
            KeyCode::Char('f') if self.uses_filtered_explorer_rows() => {
                self.reveal_selected_filtered_explorer();
            }
            KeyCode::Char('f') => self.reveal_selected_search(),
            KeyCode::Char('y') if self.uses_filtered_explorer_rows() => {
                self.copy_selected_filtered_explorer_path();
            }
            KeyCode::Char('y') => self.copy_selected_search_path(),
            _ => {}
        }
        Ok(())
    }

    fn set_search_scope(&mut self, scope: SearchScope) {
        match scope {
            SearchScope::Files if self.search_query.mode != SearchMode::Filename => {
                self.search_query.mode = SearchMode::Filename;
            }
            SearchScope::Contents if self.search_query.mode == SearchMode::Filename => {
                self.search_query.mode = SearchMode::Literal;
            }
            _ => return,
        }
        self.reset_search_selection();
        self.start_search();
    }

    fn toggle_search_scope(&mut self) {
        let scope = if self.search_query.mode == SearchMode::Filename {
            SearchScope::Contents
        } else {
            SearchScope::Files
        };
        self.set_search_scope(scope);
    }

    fn toggle_search_case(&mut self) {
        self.search_query.case_sensitive = !self.search_query.case_sensitive;
        self.start_search();
    }

    fn toggle_search_regex(&mut self) {
        self.search_query.mode = if self.search_query.mode == SearchMode::Regex {
            SearchMode::Literal
        } else {
            SearchMode::Regex
        };
        self.reset_search_selection();
        self.start_search();
    }

    fn set_show_ignored(&mut self, show_ignored: bool) {
        self.show_ignored = show_ignored;
        self.search_query.include_ignored = show_ignored;
        self.tree.set_show_ignored(show_ignored);
        self.tree.refresh();
    }

    fn open_inline_search(&mut self) {
        let restart_preserved_query = !self.search_active && !self.search_query.text.is_empty();
        if !self.search_active {
            self.search_origin = Some((self.selection, self.offset));
            self.search_active = true;
            if !self.search_query.text.is_empty() {
                self.selection = 0;
                self.offset = 0;
            }
        }
        self.input_mode = InputMode::SearchQuery;
        if restart_preserved_query {
            self.start_search();
        }
    }

    fn reset_search_selection(&mut self) {
        if self.search_query.text.is_empty() {
            if let Some((selection, offset)) = self.search_origin {
                self.selection = selection;
                self.offset = offset;
            }
        } else {
            self.selection = 0;
            self.offset = 0;
        }
    }

    fn close_inline_search(&mut self) {
        if let Some(handle) = self.search_handle.take() {
            handle.cancel();
        }
        self.search_active = false;
        self.search_error = None;
        self.input_mode = InputMode::Normal;
        if let Some((selection, offset)) = self.search_origin.take() {
            self.selection = selection;
            self.offset = offset;
        }
        self.busy = self.git_receiver.is_some();
    }

    fn handle_source_key(&mut self, key: KeyCode) -> AppResult<()> {
        match key {
            KeyCode::Char('r') => self.refresh_git(),
            KeyCode::Char('v') => self.toggle_git_view(),
            KeyCode::Left | KeyCode::Char('h') => self.collapse_or_source_parent(),
            KeyCode::Right | KeyCode::Char('l') => self.expand_or_source_child(),
            KeyCode::Enter => self.activate_source(),
            KeyCode::Char('f') => self.reveal_selected_source(),
            KeyCode::Char('y') => self.copy_selected_source_path(),
            _ => {}
        }
        Ok(())
    }

    fn activate_source(&mut self) {
        let changed = match self.source_items().get(self.selection).cloned() {
            Some(SourceItem::Directory {
                group,
                path,
                expanded,
                ..
            }) => {
                let key = source_tree_key(group, &path);
                if expanded {
                    self.git_tree_expanded.remove(&key);
                } else {
                    self.git_tree_expanded.insert(key);
                }
                true
            }
            Some(SourceItem::Entry { .. }) => {
                self.preview_selected_source();
                false
            }
            Some(SourceItem::Header(_, _)) | None => false,
        };
        if changed {
            self.persist_source_preferences();
        }
    }

    fn toggle_git_view(&mut self) {
        let selected = self.selected_source();
        self.git_view_mode = self.git_view_mode.opposite();
        if self.git_view_mode == GitViewMode::Tree {
            self.initialize_git_tree();
            if let Some(SourceSelection::Path(group, path)) = selected.as_ref() {
                self.expand_git_tree_ancestors(*group, path);
            }
        }
        self.restore_source_selection(selected);
        self.persist_source_preferences();
    }

    fn initialize_git_tree(&mut self) {
        if self.git_tree_initialized {
            return;
        }
        let Some(snapshot) = self.git_snapshot.as_ref() else {
            return;
        };
        let paths = snapshot
            .groups()
            .into_iter()
            .flat_map(|(group, entries)| {
                entries
                    .into_iter()
                    .map(move |entry| (group, entry.path.clone()))
            })
            .collect::<Vec<_>>();
        for (group, path) in paths {
            self.expand_git_tree_ancestors(group, &path);
        }
        self.git_tree_initialized = true;
    }

    fn expand_git_tree_ancestors(&mut self, group: SourceControlGroup, path: &Path) {
        let mut parent = path.parent();
        while let Some(directory) = parent {
            if directory.as_os_str().is_empty() {
                break;
            }
            self.git_tree_expanded
                .insert(source_tree_key(group, directory));
            parent = directory.parent();
        }
    }

    fn selected_source(&self) -> Option<SourceSelection> {
        match self.source_items().get(self.selection)? {
            SourceItem::Header(group, _) => Some(SourceSelection::Header(*group)),
            SourceItem::Directory { group, path, .. } => {
                Some(SourceSelection::Path(*group, path.clone()))
            }
            SourceItem::Entry { group, entry, .. } => {
                Some(SourceSelection::Path(*group, entry.path.clone()))
            }
        }
    }

    fn restore_source_selection(&mut self, selected: Option<SourceSelection>) {
        let items = self.source_items();
        let exact = selected.as_ref().and_then(|selected| {
            items.iter().position(|item| match (selected, item) {
                (SourceSelection::Header(expected), SourceItem::Header(actual, _)) => {
                    expected == actual
                }
                (
                    SourceSelection::Path(expected_group, expected_path),
                    SourceItem::Directory { group, path, .. },
                ) => expected_group == group && expected_path == path,
                (
                    SourceSelection::Path(expected_group, expected_path),
                    SourceItem::Entry { group, entry, .. },
                ) => expected_group == group && expected_path == &entry.path,
                _ => false,
            })
        });
        let group_fallback = selected.and_then(|selected| {
            let group = match selected {
                SourceSelection::Header(group) | SourceSelection::Path(group, _) => group,
            };
            items
                .iter()
                .position(|item| matches!(item, SourceItem::Header(actual, _) if *actual == group))
        });
        self.selection = exact.or(group_fallback).unwrap_or(0);
        self.keep_selection_visible();
    }

    fn collapse_or_source_parent(&mut self) {
        if self.git_view_mode != GitViewMode::Tree {
            return;
        }
        let items = self.source_items();
        let Some(item) = items.get(self.selection).cloned() else {
            return;
        };
        if let SourceItem::Directory {
            group,
            path,
            expanded: true,
            ..
        } = &item
        {
            self.git_tree_expanded
                .remove(&source_tree_key(*group, path));
            self.persist_source_preferences();
            return;
        }
        let (group, path) = match item {
            SourceItem::Directory { group, path, .. } => (group, path),
            SourceItem::Entry { group, entry, .. } => (group, entry.path.clone()),
            SourceItem::Header(_, _) => return,
        };
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        self.selection = if parent.as_os_str().is_empty() {
            items
                .iter()
                .position(|item| matches!(item, SourceItem::Header(actual, _) if *actual == group))
                .unwrap_or(self.selection)
        } else {
            items
                .iter()
                .position(|item| {
                    matches!(
                        item,
                        SourceItem::Directory {
                            group: actual_group,
                            path: actual_path,
                            ..
                        } if *actual_group == group && actual_path == parent
                    )
                })
                .unwrap_or(self.selection)
        };
    }

    fn expand_or_source_child(&mut self) {
        if self.git_view_mode != GitViewMode::Tree {
            return;
        }
        let items = self.source_items();
        let Some(item) = items.get(self.selection).cloned() else {
            return;
        };
        match item {
            SourceItem::Directory {
                group,
                path,
                depth,
                expanded,
                ..
            } => {
                if !expanded {
                    self.git_tree_expanded.insert(source_tree_key(group, &path));
                    self.persist_source_preferences();
                } else if items
                    .get(self.selection + 1)
                    .is_some_and(|next| source_item_depth(next) > depth)
                {
                    self.selection += 1;
                }
            }
            SourceItem::Header(group, _) => {
                if items
                    .get(self.selection + 1)
                    .is_some_and(|next| source_item_group(next) == group)
                {
                    self.selection += 1;
                }
            }
            SourceItem::Entry { .. } => {}
        }
    }

    fn persist_source_preferences(&mut self) {
        if let Err(error) = self.persist(true) {
            self.error = Some(format!(
                "Cannot save Source Control view preference\n{error}"
            ));
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        hits: &HitTargets,
    ) -> AppResult<Option<LoopOutcome>> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if hits.close.is_some_and(|rect| {
                    rect.x <= mouse.column
                        && mouse.column < rect.right()
                        && rect.y <= mouse.row
                        && mouse.row < rect.bottom()
                }) {
                    return Ok(Some(LoopOutcome::Hide));
                }
                if hits.git_view_toggle_at(mouse.column, mouse.row) {
                    self.toggle_git_view();
                    return Ok(None);
                }
                if let Some(view) = hits.view_at(mouse.column, mouse.row) {
                    self.switch_view(view);
                    return Ok(None);
                }
                if let Some(control) = hits.search_control_at(mouse.column, mouse.row) {
                    self.input_mode = InputMode::SearchQuery;
                    match control {
                        SearchControl::Input => {}
                        SearchControl::Files => self.set_search_scope(SearchScope::Files),
                        SearchControl::Contents => self.set_search_scope(SearchScope::Contents),
                        SearchControl::CaseSensitive => self.toggle_search_case(),
                        SearchControl::Regex => self.toggle_search_regex(),
                    }
                    return Ok(None);
                }
                if let Some(index) = hits.row_at(mouse.column, mouse.row) {
                    let activate = self.selection == index;
                    self.selection = index;
                    if activate {
                        match self.view {
                            View::Explorer if self.uses_filtered_explorer_rows() => {
                                self.activate_filtered_explorer();
                            }
                            View::Explorer if self.search_active => self.preview_selected_search(),
                            View::Explorer => self.activate_explorer(),
                            View::SourceControl => self.activate_source(),
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp => self.move_selection(-3),
            MouseEventKind::ScrollDown => self.move_selection(3),
            _ => {}
        }
        self.keep_selection_visible();
        Ok(None)
    }

    fn activate_explorer(&mut self) {
        let Some(item) = self.explorer_items().get(self.selection).cloned() else {
            return;
        };
        self.activate_explorer_item(item);
    }

    fn activate_filtered_explorer(&mut self) {
        let Some(item) = self.filtered_explorer_items().get(self.selection).cloned() else {
            return;
        };
        self.activate_explorer_item(item);
    }

    fn activate_explorer_item(&mut self, item: ExplorerItem) {
        match item.kind {
            NodeKind::Directory => {
                if item.expanded {
                    let _ = self.tree.collapse(&item.path);
                } else if let Err(error) = self.tree.expand(&item.path) {
                    self.error = Some(error.to_string());
                }
            }
            NodeKind::File | NodeKind::Symlink => {
                self.open_preview(PreviewRequest::File {
                    path: item.path,
                    line: None,
                });
            }
            NodeKind::Error => {
                self.error = item
                    .error
                    .or_else(|| Some(format!("cannot read {}", item.path.display())));
            }
        }
    }

    fn collapse_or_parent(&mut self) {
        let items = self.explorer_items();
        let Some(item) = items.get(self.selection) else {
            return;
        };
        if item.kind == NodeKind::Directory && item.expanded {
            let _ = self.tree.collapse(&item.path);
            return;
        }
        if let Some(parent) = item.path.parent()
            && let Some(index) = items.iter().position(|candidate| candidate.path == parent)
        {
            self.selection = index;
        }
    }

    fn expand_or_child(&mut self) {
        let items = self.explorer_items();
        let Some(item) = items.get(self.selection).cloned() else {
            return;
        };
        if item.kind != NodeKind::Directory {
            return;
        }
        if !item.expanded {
            if let Err(error) = self.tree.expand(&item.path) {
                self.error = Some(error.to_string());
            }
            return;
        }
        let items = self.explorer_items();
        if let Some(index) = items.iter().position(|candidate| {
            candidate.path.parent() == Some(item.path.as_path()) && candidate.path != item.path
        }) {
            self.selection = index;
        }
    }

    fn copy_selected_explorer_path(&mut self) {
        let path = self
            .explorer_items()
            .get(self.selection)
            .map(|item| item.path.clone());
        if let Some(path) = path {
            self.copy_path(&path);
        }
    }

    fn copy_selected_filtered_explorer_path(&mut self) {
        let path = self
            .filtered_explorer_items()
            .get(self.selection)
            .map(|item| item.path.clone());
        if let Some(path) = path {
            self.copy_path(&path);
        }
    }

    fn copy_selected_search_path(&mut self) {
        let path = match self.search_items().get(self.selection) {
            Some(SearchItem::Match { path, .. }) => Some(path.clone()),
            _ => None,
        };
        if let Some(path) = path {
            self.copy_path(&path);
        }
    }

    fn copy_selected_source_path(&mut self) {
        let path = match self.source_items().get(self.selection) {
            Some(SourceItem::Directory { path, .. }) => Some(path.clone()),
            Some(SourceItem::Entry { entry, .. }) => Some(entry.path.clone()),
            _ => None,
        };
        if let Some(path) = path {
            self.copy_path(&path);
        }
    }

    fn copy_path(&mut self, path: &Path) {
        if let Err(error) = self.clipboard.copy_path(path) {
            self.error = Some(format!("Cannot copy path\n{error}"));
        }
    }

    fn open_selected_explorer(&mut self) {
        if let Some(item) = self.explorer_items().get(self.selection) {
            self.open_external(&item.path);
        }
    }

    fn open_selected_filtered_explorer(&mut self) {
        if let Some(item) = self.filtered_explorer_items().get(self.selection) {
            self.open_external(&item.path);
        }
    }

    fn open_selected_search(&mut self) {
        if let Some(SearchItem::Match { path, .. }) = self.search_items().get(self.selection) {
            self.open_external(path);
        }
    }

    fn reveal_selected_explorer(&mut self) {
        if let Some(item) = self.explorer_items().get(self.selection) {
            self.reveal_in_file_manager(&item.path);
        }
    }

    fn reveal_selected_filtered_explorer(&mut self) {
        if let Some(item) = self.filtered_explorer_items().get(self.selection) {
            self.reveal_in_file_manager(&item.path);
        }
    }

    fn reveal_selected_search(&mut self) {
        if let Some(SearchItem::Match { path, .. }) = self.search_items().get(self.selection) {
            self.reveal_in_file_manager(path);
        }
    }

    fn reveal_selected_source(&mut self) {
        let path = match self.source_items().get(self.selection) {
            Some(SourceItem::Directory { path, .. }) => Some(path.clone()),
            Some(SourceItem::Entry { entry, .. }) => Some(entry.path.clone()),
            _ => None,
        };
        if let Some(path) = path {
            self.reveal_in_file_manager(&path);
        }
    }

    fn reveal_in_file_manager(&mut self, relative: &Path) {
        let absolute = match self.workspace.resolve_path(relative) {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        if let Err(error) = self.file_manager_opener.open(&absolute) {
            self.error = Some(format!("Cannot open system file manager\n{error}"));
        }
    }

    fn open_external(&mut self, relative: &Path) {
        let absolute = match self.workspace.resolve_path(relative) {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let argv = match self
            .config
            .open_argv_with_editor(&absolute, self.editor.as_deref())
        {
            Ok(Some(argv)) => argv,
            Ok(None) => {
                self.error = Some(format!(
                    "Enter opens a centered read-only preview. To edit with o in a new Herdr tab, export EDITOR or add [open] command = [\"nvim\", \"{{path}}\"] to {}.",
                    self.loaded_config.path.display()
                ));
                return;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let Some(workspace_id) = self.context.workspace_id.as_deref() else {
            self.error = Some("Herdr workspace context is unavailable".to_string());
            return;
        };
        let label = relative.file_name().map_or_else(
            || "Preview".to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let params = serde_json::json!({
            "workspace_id": workspace_id,
            "tab_label": format!("Open {label}"),
            "focus": true,
            "root": {
                "type": "pane",
                "label": label,
                "cwd": self.workspace.path().to_string_lossy(),
                "command": argv,
                "env": {}
            }
        });
        if let Err(error) = LiveHerdr::from_env().request("layout.apply", params) {
            self.error = Some(format!("cannot open configured editor in Herdr: {error}"));
        }
    }

    fn preview_selected_search(&mut self) {
        let Some(SearchItem::Match { path, line, .. }) =
            self.search_items().get(self.selection).cloned()
        else {
            return;
        };
        self.open_preview(PreviewRequest::File {
            path,
            line: Some(line.max(1)),
        });
    }

    fn preview_selected_source(&mut self) {
        let Some(SourceItem::Entry { group, entry, .. }) =
            self.source_items().get(self.selection).cloned()
        else {
            return;
        };
        self.open_preview(PreviewRequest::Source {
            path: entry.path,
            group,
        });
    }

    fn open_preview(&mut self, request: PreviewRequest) {
        if let Err(error) = self.preview_opener.open(&self.workspace, &request) {
            self.error = Some(format!("Cannot open centered preview\n{error}"));
        }
    }

    fn start_search(&mut self) {
        if let Some(handle) = self.search_handle.take() {
            handle.cancel();
        }
        self.search_query.include_ignored = self.show_ignored;
        self.search_error = None;
        if self.search_query.text.is_empty() || self.search_query.mode == SearchMode::Filename {
            self.search_results = SearchResults::default();
            self.busy = self.git_receiver.is_some();
            return;
        }
        self.search_results = SearchResults::default();
        self.selection = 0;
        self.offset = 0;
        match self.search.start(self.search_query.clone()) {
            Ok(handle) => {
                self.search_handle = Some(handle);
                self.busy = true;
            }
            Err(error) => self.search_error = Some(error.message),
        }
    }

    fn refresh(&mut self) {
        self.tree.refresh();
        self.refresh_git();
    }

    fn refresh_git(&mut self) {
        if self.workspace.is_git_worktree && self.git_receiver.is_none() {
            self.git_receiver = Some(self.git.refresh_async());
            self.busy = true;
        }
    }

    fn poll_background(&mut self) {
        while let Ok(event) = self.watch_events.try_recv() {
            if event.is_ok() {
                self.pending_refresh = Some(Instant::now());
            }
        }
        if self
            .pending_refresh
            .is_some_and(|since| since.elapsed() >= REFRESH_DEBOUNCE)
        {
            self.pending_refresh = None;
            self.tree.refresh();
            self.refresh_git();
        }

        let git_result = self
            .git_receiver
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        if let Some(result) = git_result {
            self.git_receiver = None;
            match result {
                Ok(snapshot) => {
                    let ignored = snapshot
                        .entries
                        .iter()
                        .filter(|entry| entry.ignored)
                        .map(|entry| entry.path.clone())
                        .collect::<BTreeSet<_>>();
                    self.tree.set_ignored_paths(ignored);
                    self.tree.refresh();
                    self.git_snapshot = Some(snapshot);
                    if self.git_view_mode == GitViewMode::Tree {
                        let initialized = self.git_tree_initialized;
                        self.initialize_git_tree();
                        if !initialized && self.git_tree_initialized {
                            self.persist_source_preferences();
                        }
                    }
                    self.git_error = None;
                }
                Err(error) => {
                    let detail = error
                        .message
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("Git returned no diagnostic")
                        .trim();
                    self.git_error = Some(format!(
                        "Git status unavailable\n{detail}\nLast valid status retained. Run git status in the workspace root."
                    ))
                }
            }
        }

        loop {
            let update = match self.search_handle.as_ref().map(SearchHandle::try_recv) {
                Some(Ok(update)) => update,
                Some(Err(mpsc::TryRecvError::Empty)) | None => break,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.search_handle = None;
                    if self.search_results.files.is_empty() {
                        self.search_error =
                            Some("Content search stopped before it completed.".to_string());
                    }
                    break;
                }
            };
            match update {
                SearchUpdate::Match(file) => {
                    let row_count = file.matches.len() + 1;
                    let position = self
                        .search_results
                        .files
                        .binary_search_by(|existing| existing.path.cmp(&file.path))
                        .unwrap_or_else(|position| position);
                    let inserted_row = self.search_results.files[..position]
                        .iter()
                        .map(|existing| existing.matches.len() + 1)
                        .sum::<usize>();
                    if !self.search_results.files.is_empty() && inserted_row <= self.selection {
                        self.selection += row_count;
                    }
                    self.search_results.files.insert(position, file);
                }
                SearchUpdate::Finished { errors, cancelled } => {
                    self.search_handle = None;
                    self.search_results.cancelled = cancelled;
                    self.search_results.errors = errors;
                    if let Some(first) = self.search_results.errors.first() {
                        let path = first
                            .path
                            .as_ref()
                            .map(|path| format!("{}: ", path.display()))
                            .unwrap_or_default();
                        let remaining = self.search_results.errors.len().saturating_sub(1);
                        self.search_error = Some(format!(
                            "{path}{}{}",
                            first.message,
                            if remaining > 0 {
                                format!("; {remaining} more search errors")
                            } else {
                                String::new()
                            }
                        ));
                    }
                }
            }
        }

        self.busy = self.git_receiver.is_some() || self.search_handle.is_some();
        if self.last_config_poll.elapsed() >= CONFIG_POLL {
            self.last_config_poll = Instant::now();
            self.reload_configuration();
        }
    }

    fn reload_configuration(&mut self) {
        let current = file_modified(&self.loaded_config.path);
        if current != self.config_modified {
            match Config::load_with_metadata(&self.loaded_config.path) {
                Ok(loaded) => {
                    let ignored_visibility_changed =
                        self.show_ignored != loaded.config.show_ignored;
                    self.config = loaded.config.clone();
                    self.loaded_config = loaded;
                    self.config_modified = current;
                    self.tree
                        .set_show_git_directory(self.config.show_git_directory);
                    self.set_show_ignored(self.config.show_ignored);
                    if ignored_visibility_changed && self.search_active {
                        self.start_search();
                    }
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if let Some(path) = &self.theme_path {
            let modified = file_modified(path);
            if modified != self.theme_modified {
                match crate::theme::resolve_config(path, terminal_appearance()) {
                    Ok(theme) => {
                        replace_theme_diagnostic(&mut self.error, &self.theme, &theme);
                        self.theme = theme;
                        self.theme_modified = modified;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
    }

    fn switch_view(&mut self, view: View) {
        let restore_explorer = self.search_active && view == View::Explorer;
        if self.search_active {
            self.close_inline_search();
        }
        self.view = view;
        if !restore_explorer {
            self.selection = 0;
            self.offset = 0;
        }
        self.input_mode = InputMode::Normal;
    }

    fn next_view(&mut self) {
        self.switch_view(match self.view {
            View::Explorer => View::SourceControl,
            View::SourceControl => View::Explorer,
        });
    }

    fn previous_view(&mut self) {
        self.switch_view(match self.view {
            View::Explorer => View::SourceControl,
            View::SourceControl => View::Explorer,
        });
    }

    fn move_selection(&mut self, delta: isize) {
        let length = self.selectable_row_count();
        if length == 0 {
            self.selection = 0;
            return;
        }
        self.selection = self.selection.saturating_add_signed(delta).min(length - 1);
    }

    fn selectable_row_count(&self) -> usize {
        match self.view {
            View::Explorer if self.uses_filtered_explorer_rows() => {
                self.filtered_explorer_items().len()
            }
            View::Explorer if self.search_active => self.search_items().len(),
            View::Explorer => self.explorer_items().len(),
            View::SourceControl => self.source_items().len(),
        }
    }

    fn set_viewport_rows(&mut self, rows: usize) {
        self.viewport_rows = rows;
        self.keep_selection_visible();
    }

    fn keep_selection_visible(&mut self) {
        let length = self.selectable_row_count();
        if length == 0 {
            self.selection = 0;
            self.offset = 0;
            return;
        }
        self.selection = self.selection.min(length - 1);
        self.offset = offset_for_selection(self.selection, self.offset, length, self.viewport_rows);
    }

    fn persist(&mut self, visible: bool) -> Result<(), Box<dyn std::error::Error>> {
        let expanded: BTreeSet<String> = self
            .tree
            .visible_nodes()
            .into_iter()
            .filter(|node| {
                !node.path.as_os_str().is_empty()
                    && node.kind == NodeKind::Directory
                    && node.expanded
            })
            .map(|node| node.path.to_string_lossy().into_owned())
            .collect();
        let (explorer_selection, explorer_scroll) = if self.search_active {
            self.search_origin.unwrap_or((0, 0))
        } else {
            (self.selection, self.offset)
        };
        let selection = (self.view == View::Explorer)
            .then(|| {
                self.explorer_items()
                    .get(explorer_selection)
                    .map(|item| item.path.to_string_lossy().into_owned())
            })
            .flatten();
        let state = self
            .persisted
            .tab_mut(self.workspace_id.clone(), self.tab_id.clone());
        set_visibility(state, visible);
        state.width = std::env::var("HERDR_WORKBENCH_WIDTH")
            .ok()
            .and_then(|width| width.parse().ok())
            .unwrap_or(state.width);
        state.view = match self.view {
            View::Explorer => SidebarView::Explorer,
            View::SourceControl => SidebarView::SourceControl,
        };
        state.expanded = expanded;
        state.git_view_mode = self.git_view_mode;
        state.git_tree_expanded = self.git_tree_expanded.clone();
        state.git_tree_initialized = self.git_tree_initialized;
        state.selection = selection;
        state.scroll = if self.view == View::Explorer {
            explorer_scroll
        } else {
            self.offset
        };
        state.workspace_cwd = Some(self.workspace.path().to_string_lossy().into_owned());
        self.persisted.save_atomic(&self.state_path)?;
        Ok(())
    }

    fn apply_terminal_appearance(&mut self, appearance: Appearance) {
        let Some(path) = self.theme_path.as_ref() else {
            return;
        };
        match crate::theme::resolve_config(path, Some(appearance)) {
            Ok(theme) => {
                replace_theme_diagnostic(&mut self.error, &self.theme, &theme);
                self.theme = theme;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

fn replace_theme_diagnostic(
    error: &mut Option<String>,
    previous: &ThemeResolution,
    next: &ThemeResolution,
) {
    if next.diagnostic.is_some() || error.as_ref() == previous.diagnostic.as_ref() {
        *error = next.diagnostic.clone();
    }
}

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

fn entry_kind(kind: NodeKind) -> EntryKind {
    match kind {
        NodeKind::Directory => EntryKind::Directory,
        NodeKind::File => EntryKind::File,
        NodeKind::Symlink => EntryKind::Symlink,
        NodeKind::Error => EntryKind::Unknown,
    }
}

fn coordinates_for_entry(entry: &GitEntry) -> GitCoordinates {
    if entry.conflict.is_some() {
        return GitCoordinates {
            index: GitState::Conflict,
            worktree: GitState::Clean,
        };
    }
    if entry.ignored {
        return GitCoordinates {
            index: GitState::Clean,
            worktree: GitState::Ignored,
        };
    }
    if entry.untracked {
        return GitCoordinates {
            index: GitState::Clean,
            worktree: GitState::Untracked,
        };
    }
    GitCoordinates {
        index: status_git_state(entry.index_status),
        worktree: status_git_state(entry.worktree_status),
    }
}

fn coordinates_for_source_entry(entry: &GitEntry, group: SourceControlGroup) -> GitCoordinates {
    let coordinates = coordinates_for_entry(entry);
    match group {
        SourceControlGroup::MergeChanges => coordinates,
        SourceControlGroup::StagedChanges => GitCoordinates {
            index: coordinates.index,
            worktree: GitState::Clean,
        },
        SourceControlGroup::Changes => GitCoordinates {
            index: GitState::Clean,
            worktree: coordinates.worktree,
        },
        SourceControlGroup::Untracked => GitCoordinates {
            index: GitState::Clean,
            worktree: GitState::Untracked,
        },
    }
}

fn status_git_state(status: StatusCode) -> GitState {
    match status {
        StatusCode::Clean => GitState::Clean,
        StatusCode::Modified | StatusCode::Unknown(_) => GitState::Modified,
        StatusCode::Added => GitState::Added,
        StatusCode::Deleted => GitState::Deleted,
        StatusCode::Renamed => GitState::Renamed,
        StatusCode::Copied => GitState::Copied,
        StatusCode::TypeChanged => GitState::TypeChanged,
        StatusCode::Unmerged => GitState::Conflict,
    }
}

fn directory_git_state(status: DirectoryStatus) -> GitState {
    match status {
        DirectoryStatus::Clean => GitState::Clean,
        DirectoryStatus::Ignored => GitState::Ignored,
        DirectoryStatus::CopiedOrRenamed => GitState::Renamed,
        DirectoryStatus::AddedOrUntracked => GitState::Untracked,
        DirectoryStatus::ModifiedOrTypeChanged => GitState::Modified,
        DirectoryStatus::Deleted => GitState::Deleted,
        DirectoryStatus::Conflict => GitState::Conflict,
    }
}

fn coordinates_for_directory(
    aggregate: Option<DirectoryStatus>,
    directly_ignored: bool,
) -> GitCoordinates {
    if directly_ignored {
        return GitCoordinates {
            index: GitState::Clean,
            worktree: GitState::Ignored,
        };
    }
    GitCoordinates {
        index: aggregate
            .filter(|status| *status != DirectoryStatus::Ignored)
            .map(directory_git_state)
            .unwrap_or(GitState::Clean),
        worktree: GitState::Clean,
    }
}

fn viewport_rows_for_height(height: u16, query_visible: bool) -> usize {
    let chrome_rows = 1 + 2 * u16::from(query_visible);
    usize::from(height.saturating_sub(chrome_rows))
}

fn offset_for_selection(
    selection: usize,
    offset: usize,
    row_count: usize,
    viewport_rows: usize,
) -> usize {
    if row_count == 0 {
        return 0;
    }
    let viewport_rows = viewport_rows.max(1);
    let max_offset = row_count.saturating_sub(viewport_rows);
    let offset = offset.min(max_offset);
    if selection < offset {
        return selection.min(max_offset);
    }
    if selection >= offset.saturating_add(viewport_rows) {
        return selection
            .saturating_add(1)
            .saturating_sub(viewport_rows)
            .min(max_offset);
    }
    offset
}

fn append_source_tree_items(
    items: &mut Vec<SourceItem>,
    group: SourceControlGroup,
    parent: &Path,
    entries: &[&GitEntry],
    depth: u16,
    expanded: &BTreeSet<String>,
) {
    let mut directories = entries
        .iter()
        .filter_map(|entry| {
            let relative = entry.path.strip_prefix(parent).ok()?;
            let mut components = relative.components();
            let first = components.next()?;
            components.next()?;
            Some(parent.join(first.as_os_str()))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    directories.sort_by(|left, right| compare_source_paths(left, right));

    for directory in directories {
        let is_expanded = expanded.contains(&source_tree_key(group, &directory));
        let git = entries
            .iter()
            .filter(|entry| entry.path.starts_with(&directory))
            .map(|entry| coordinates_for_source_entry(entry, group))
            .max_by_key(|coordinates| coordinates.aggregate().priority())
            .unwrap_or_default();
        items.push(SourceItem::Directory {
            group,
            path: directory.clone(),
            depth,
            expanded: is_expanded,
            git,
        });
        if is_expanded {
            append_source_tree_items(
                items,
                group,
                &directory,
                entries,
                depth.saturating_add(1),
                expanded,
            );
        }
    }

    let mut files = entries
        .iter()
        .filter(|entry| entry.path.parent().unwrap_or_else(|| Path::new("")) == parent)
        .copied()
        .collect::<Vec<_>>();
    files.sort_by(|left, right| compare_source_paths(&left.path, &right.path));
    items.extend(files.into_iter().cloned().map(|entry| {
        let name = entry
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        SourceItem::Entry {
            group,
            entry: Box::new(entry),
            depth,
            name,
        }
    }));
}

fn compare_source_paths(left: &Path, right: &Path) -> std::cmp::Ordering {
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(&right))
}

fn source_tree_key(group: SourceControlGroup, path: &Path) -> String {
    format!(
        "{}\0{}",
        match group {
            SourceControlGroup::MergeChanges => "merge",
            SourceControlGroup::StagedChanges => "staged",
            SourceControlGroup::Changes => "changes",
            SourceControlGroup::Untracked => "untracked",
        },
        path.to_string_lossy()
    )
}

fn source_item_group(item: &SourceItem) -> SourceControlGroup {
    match item {
        SourceItem::Header(group, _)
        | SourceItem::Directory { group, .. }
        | SourceItem::Entry { group, .. } => *group,
    }
}

fn source_item_depth(item: &SourceItem) -> u16 {
    match item {
        SourceItem::Header(_, _) => 0,
        SourceItem::Directory { depth, .. } | SourceItem::Entry { depth, .. } => *depth,
    }
}

fn group_label(group: SourceControlGroup) -> &'static str {
    match group {
        SourceControlGroup::MergeChanges => "Merge Changes",
        SourceControlGroup::StagedChanges => "Staged Changes",
        SourceControlGroup::Changes => "Changes",
        SourceControlGroup::Untracked => "Untracked",
    }
}

fn set_visibility(state: &mut SidebarState, visible: bool) {
    state.visible = visible;
    if !visible {
        state.pane_id = None;
    }
}

fn load_theme() -> (Option<PathBuf>, ThemeResolution, Option<SystemTime>) {
    crate::theme::load_from_env()
}

fn terminal_appearance() -> Option<Appearance> {
    crate::theme::terminal_appearance()
}

fn query_terminal_appearance() -> Option<Appearance> {
    crate::theme::query_terminal_appearance()
}

#[cfg(test)]
fn parse_terminal_appearance_response(response: &[u8]) -> Option<Appearance> {
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

fn file_modified(path: &Path) -> Option<SystemTime> {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Palette;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct RecordingPreviewOpener {
        requests: Arc<Mutex<Vec<PreviewRequest>>>,
    }

    impl PreviewOpener for RecordingPreviewOpener {
        fn open(
            &self,
            _workspace: &WorkspaceRoot,
            request: &PreviewRequest,
        ) -> Result<(), crate::preview::PreviewError> {
            self.requests
                .lock()
                .expect("preview requests mutex")
                .push(request.clone());
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct RecordingFileManagerOpener {
        paths: Arc<Mutex<Vec<PathBuf>>>,
    }

    impl FileManagerOpener for RecordingFileManagerOpener {
        fn open(&self, path: &Path) -> Result<(), crate::file_manager::FileManagerError> {
            self.paths
                .lock()
                .expect("file manager paths mutex")
                .push(path.to_owned());
            Ok(())
        }
    }

    struct FailingFileManagerOpener;

    impl FileManagerOpener for FailingFileManagerOpener {
        fn open(&self, _path: &Path) -> Result<(), crate::file_manager::FileManagerError> {
            Err(crate::file_manager::FileManagerError::UnsupportedPlatform)
        }
    }

    #[derive(Clone, Default)]
    struct RecordingClipboard {
        paths: Arc<Mutex<Vec<PathBuf>>>,
    }

    impl ClipboardWriter for RecordingClipboard {
        fn copy_path(&self, path: &Path) -> Result<(), crate::clipboard::ClipboardError> {
            self.paths
                .lock()
                .expect("clipboard paths mutex")
                .push(path.to_owned());
            Ok(())
        }
    }

    struct FailingClipboard;

    impl ClipboardWriter for FailingClipboard {
        fn copy_path(&self, _path: &Path) -> Result<(), crate::clipboard::ClipboardError> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "closed terminal").into())
        }
    }

    fn test_app() -> (tempfile::TempDir, SidebarApp) {
        let directory = tempfile::tempdir().expect("temporary workspace");
        std::fs::write(directory.path().join("child.txt"), "child\n").expect("workspace child");
        let workspace = WorkspaceRoot::resolve(directory.path()).expect("workspace root");
        let config_path = directory.path().join("config.toml");
        let state_path = directory.path().join("state.json");
        let loaded_config =
            Config::load_with_metadata(&config_path).expect("default plugin config");
        let app = SidebarApp::new(
            InvocationContext::default(),
            workspace,
            "workspace".to_string(),
            "tab".to_string(),
            state_path,
            PersistedState::default(),
            loaded_config,
            CompanionMonitor::inactive(),
            (
                None,
                ThemeResolution {
                    palette: Palette::catppuccin(),
                    name: "catppuccin".to_string(),
                    diagnostic: None,
                },
                None,
            ),
        )
        .expect("sidebar app");
        (directory, app)
    }

    #[test]
    fn explorer_starts_with_root_children_instead_of_a_redundant_root_row() {
        let (_directory, app) = test_app();

        let items = app.explorer_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, Path::new("child.txt"));
        assert_eq!(items[0].depth, 0);
        assert!(items.iter().all(|item| !item.path.as_os_str().is_empty()));
    }

    #[test]
    fn dock_shortcut_requests_a_live_side_toggle() {
        let (_directory, mut app) = test_app();

        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
                .expect("dock shortcut"),
            Some(LoopOutcome::ToggleDock)
        );
    }

    #[test]
    fn explorer_search_is_transient_and_escape_returns_to_the_tree() {
        let (_directory, mut app) = test_app();

        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
            .expect("open inline search");
        assert_eq!(app.view, View::Explorer);
        assert!(app.search_active);
        assert_eq!(app.input_mode, InputMode::SearchQuery);
        assert_eq!(app.render_model().rows.len(), app.explorer_items().len());
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE))
            .expect("type query");
        assert_eq!(app.search_query.text, "c");
        assert!(app.search_handle.is_none());
        assert_eq!(app.render_model().rows.len(), 1);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .expect("return to file tree");
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(!app.search_active);
        assert_eq!(app.view, View::Explorer);

        app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE))
            .expect("switch to Source Control");
        assert_eq!(app.view, View::SourceControl);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .expect("cycle to Explorer");
        assert_eq!(app.view, View::Explorer);
    }

    #[test]
    fn filename_filter_preserves_all_rows_when_empty_and_restores_origin_when_cleared() {
        let (directory, mut app) = test_app();
        let previews = RecordingPreviewOpener::default();
        let clipboard = RecordingClipboard::default();
        app.preview_opener = Box::new(previews.clone());
        app.clipboard = Box::new(clipboard.clone());
        std::fs::write(directory.path().join("other.rs"), "other\n").expect("second file");
        app.tree.refresh();
        app.selection = 1;
        let original_rows = app.explorer_items();
        assert_eq!(original_rows.len(), 2);

        app.open_inline_search();
        assert_eq!(app.selection, 1);
        assert_eq!(app.render_model().rows.len(), 2);

        for character in "child".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .expect("filter character");
        }
        let filtered = app.render_model();
        assert_eq!(filtered.rows.len(), 1);
        assert_eq!(filtered.rows[0].name, "child.txt");
        assert!(app.search_handle.is_none());

        for _ in 0.."child".len() {
            app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
                .expect("clear filter");
        }
        assert!(app.search_query.text.is_empty());
        assert_eq!(app.render_model().rows.len(), 2);
        assert_eq!(app.selection, 1);

        for character in "other".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .expect("second filter character");
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("finish filter entry");
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .expect("copy filtered result path");
        assert_eq!(
            *clipboard.paths.lock().expect("clipboard paths"),
            vec![PathBuf::from("other.rs")]
        );
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("preview filtered result");
        assert_eq!(
            *previews.requests.lock().expect("preview requests"),
            vec![PreviewRequest::File {
                path: PathBuf::from("other.rs"),
                line: None,
            }]
        );
    }

    #[test]
    fn external_open_error_explains_preview_and_editing_roles() {
        let (_directory, mut app) = test_app();
        app.editor = None;

        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE))
            .expect("request external edit");

        let error = app.error.expect("missing editor is actionable");
        assert!(error.contains("Enter opens a centered read-only preview"));
        assert!(error.contains("To edit with o in a new Herdr tab"));
        assert!(error.contains("export EDITOR"));
        assert!(error.ends_with("config.toml."));
    }

    #[test]
    fn file_manager_shortcut_reveals_the_root_bound_selected_path() {
        let (directory, mut app) = test_app();
        let opener = RecordingFileManagerOpener::default();
        app.file_manager_opener = Box::new(opener.clone());

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .expect("reveal selected file");

        assert_eq!(
            *opener.paths.lock().expect("file manager paths"),
            vec![
                directory
                    .path()
                    .join("child.txt")
                    .canonicalize()
                    .expect("canonical fixture path")
            ]
        );
    }

    #[test]
    fn file_manager_shortcut_surfaces_a_launch_error() {
        let (_directory, mut app) = test_app();
        app.file_manager_opener = Box::new(FailingFileManagerOpener);

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .expect("request file manager");

        let error = app.error.expect("file manager error");
        assert!(error.starts_with("Cannot open system file manager"));
        assert!(error.contains("unsupported on this platform"));
    }

    #[test]
    fn copy_shortcut_writes_the_root_relative_path_without_opening_a_preview() {
        let (_directory, mut app) = test_app();
        let clipboard = RecordingClipboard::default();
        let previews = RecordingPreviewOpener::default();
        app.clipboard = Box::new(clipboard.clone());
        app.preview_opener = Box::new(previews.clone());

        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .expect("copy selected path");

        assert_eq!(
            *clipboard.paths.lock().expect("clipboard paths"),
            vec![PathBuf::from("child.txt")]
        );
        assert!(
            previews
                .requests
                .lock()
                .expect("preview requests")
                .is_empty()
        );
    }

    #[test]
    fn copy_shortcut_surfaces_a_terminal_write_error() {
        let (_directory, mut app) = test_app();
        app.clipboard = Box::new(FailingClipboard);

        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .expect("request path copy");

        let error = app.error.expect("clipboard error");
        assert!(error.starts_with("Cannot copy path"));
        assert!(error.contains("closed terminal"));
    }

    #[test]
    fn active_search_exposes_cursor_state_to_the_renderer() {
        let (_directory, mut app) = test_app();

        app.open_inline_search();
        let model = app.render_model();
        assert_eq!(model.query.as_deref(), Some(""));
        assert!(model.query_input_active);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("finish query entry");
        assert!(!app.render_model().query_input_active);
    }

    #[test]
    fn visible_search_controls_switch_scope_without_stealing_query_characters() {
        let (_directory, mut app) = test_app();
        app.open_inline_search();

        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE))
            .expect("type query character");
        assert_eq!(app.search_query.text, "c");
        assert_eq!(app.search_query.mode, SearchMode::Filename);

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .expect("switch to content search");
        assert_eq!(app.search_query.mode, SearchMode::Literal);
        assert_eq!(app.search_query.text, "c");

        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT))
            .expect("enable regex");
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT))
            .expect("enable case sensitivity");
        assert_eq!(app.search_query.mode, SearchMode::Regex);
        assert!(app.search_query.case_sensitive);
        assert!(app.search_query.include_ignored);

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .expect("plain f remains query input");
        assert_eq!(app.search_query.text, "cf");
        assert_eq!(app.search_query.mode, SearchMode::Regex);
    }

    #[test]
    fn content_search_follows_explorer_ignored_visibility() {
        let (_directory, mut app) = test_app();
        assert!(app.show_ignored);
        assert!(app.search_query.include_ignored);

        app.handle_explorer_key(KeyCode::Char('i'))
            .expect("hide ignored Explorer entries");
        assert!(!app.show_ignored);
        app.search_query.text = "child".to_string();
        app.search_query.mode = SearchMode::Literal;
        app.open_inline_search();
        assert!(!app.search_query.include_ignored);
        assert!(app.search_handle.is_some());
        assert!(app.render_model().search_busy);

        app.close_inline_search();
        assert!(!app.render_model().search_busy);
        app.handle_explorer_key(KeyCode::Char('i'))
            .expect("show ignored Explorer entries");
        assert!(app.show_ignored);
        app.open_inline_search();
        assert!(app.search_query.include_ignored);
        assert!(app.search_handle.is_some());
    }

    #[test]
    fn live_content_query_reaches_grouped_results() {
        let (directory, mut app) = test_app();
        let previews = RecordingPreviewOpener::default();
        let file_manager = RecordingFileManagerOpener::default();
        let clipboard = RecordingClipboard::default();
        app.preview_opener = Box::new(previews.clone());
        app.file_manager_opener = Box::new(file_manager.clone());
        app.clipboard = Box::new(clipboard.clone());
        app.open_inline_search();
        for character in "child".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .expect("type content query");
        }
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .expect("switch to content search");

        let deadline = Instant::now() + Duration::from_secs(2);
        while app.search_handle.is_some() && Instant::now() < deadline {
            app.poll_background();
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(app.search_handle.is_none(), "content search did not finish");
        assert_eq!(app.search_results.files.len(), 1);
        assert_eq!(app.search_results.files[0].path, Path::new("child.txt"));
        assert_eq!(app.search_results.files[0].matches[0].line, 1);
        app.selection = 1;
        app.preview_selected_search();
        assert_eq!(
            *previews.requests.lock().expect("preview requests"),
            vec![PreviewRequest::File {
                path: PathBuf::from("child.txt"),
                line: Some(1),
            }]
        );
        app.reveal_selected_search();
        assert_eq!(
            *file_manager.paths.lock().expect("file manager paths"),
            vec![
                directory
                    .path()
                    .join("child.txt")
                    .canonicalize()
                    .expect("canonical search path")
            ]
        );
        app.copy_selected_search_path();
        assert_eq!(
            *clipboard.paths.lock().expect("clipboard paths"),
            vec![PathBuf::from("child.txt")]
        );
    }

    #[test]
    fn clicking_a_search_control_refocuses_the_query() {
        let (_directory, mut app) = test_app();
        app.open_inline_search();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("leave query input");
        assert_eq!(app.input_mode, InputMode::Normal);
        let hits = HitTargets {
            search_contents: Some(ratatui::layout::Rect::new(6, 2, 8, 1)),
            ..HitTargets::default()
        };

        app.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 6,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            &hits,
        )
        .expect("click content scope");

        assert_eq!(app.input_mode, InputMode::SearchQuery);
        assert_eq!(app.search_query.mode, SearchMode::Literal);
    }

    #[test]
    fn live_regex_errors_do_not_intercept_the_next_character() {
        let (_directory, mut app) = test_app();
        app.search_query.mode = SearchMode::Regex;
        app.open_inline_search();

        app.handle_key(KeyEvent::new(KeyCode::Char('('), KeyModifiers::NONE))
            .expect("type incomplete expression");
        assert!(app.search_error.is_some());
        assert!(app.error.is_none());

        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .expect("continue incomplete expression");
        app.handle_key(KeyEvent::new(KeyCode::Char(')'), KeyModifiers::NONE))
            .expect("complete expression");

        assert_eq!(app.search_query.text, "(a)");
        assert!(app.search_error.is_none());
        assert!(app.search_handle.is_some());
    }

    #[test]
    fn non_git_workspace_never_starts_git_and_renders_a_quiet_notice() {
        let (_directory, mut app) = test_app();

        assert!(!app.workspace.is_git_worktree);
        assert!(app.git_receiver.is_none());
        assert!(!app.busy);

        app.switch_view(View::SourceControl);
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
            .expect("refresh non-Git workspace");
        let model = app.render_model();

        assert!(app.git_receiver.is_none());
        assert!(app.git_error.is_none());
        assert_eq!(
            model.notice.as_deref(),
            Some("No Git repository\nThis folder is not tracked by Git.")
        );
    }

    #[test]
    fn git_coordinates_preserve_dual_state_with_group_specific_badges() {
        let dual = GitEntry {
            path: "file".into(),
            rename_origin: None,
            index_status: StatusCode::Added,
            worktree_status: StatusCode::Modified,
            submodule: Default::default(),
            conflict: None,
            ignored: false,
            untracked: false,
        };
        let coordinates = coordinates_for_entry(&dual);
        assert_eq!(coordinates.index, GitState::Added);
        assert_eq!(coordinates.worktree, GitState::Modified);
        assert_eq!(coordinates.badge(), "M");
        assert_eq!(
            coordinates_for_source_entry(&dual, SourceControlGroup::StagedChanges).badge(),
            "A"
        );
        assert_eq!(
            coordinates_for_source_entry(&dual, SourceControlGroup::Changes).badge(),
            "M"
        );
        let untracked = GitEntry {
            untracked: true,
            ..dual
        };
        assert_eq!(coordinates_for_entry(&untracked).badge(), "U");
    }

    #[test]
    fn source_control_opens_the_exact_group_in_the_preview_popup() {
        let (directory, mut app) = test_app();
        let previews = RecordingPreviewOpener::default();
        let file_manager = RecordingFileManagerOpener::default();
        let clipboard = RecordingClipboard::default();
        app.preview_opener = Box::new(previews.clone());
        app.file_manager_opener = Box::new(file_manager.clone());
        app.clipboard = Box::new(clipboard.clone());
        std::fs::write(directory.path().join("dual.rs"), "fn dual() {}\n").expect("source fixture");
        app.git_snapshot = Some(GitSnapshot {
            root: app.workspace.path().to_path_buf(),
            entries: vec![GitEntry {
                path: PathBuf::from("dual.rs"),
                rename_origin: None,
                index_status: StatusCode::Added,
                worktree_status: StatusCode::Modified,
                submodule: Default::default(),
                conflict: None,
                ignored: false,
                untracked: false,
            }],
            ..GitSnapshot::default()
        });
        app.view = View::SourceControl;
        app.selection = 3;

        app.preview_selected_source();

        assert_eq!(
            *previews.requests.lock().expect("preview requests"),
            vec![PreviewRequest::Source {
                path: PathBuf::from("dual.rs"),
                group: SourceControlGroup::Changes,
            }]
        );
        app.handle_source_key(KeyCode::Char('f'))
            .expect("reveal source path");
        assert_eq!(
            *file_manager.paths.lock().expect("file manager paths"),
            vec![
                directory
                    .path()
                    .join("dual.rs")
                    .canonicalize()
                    .expect("canonical source path")
            ]
        );
        app.handle_source_key(KeyCode::Char('y'))
            .expect("copy source path");
        assert_eq!(
            *clipboard.paths.lock().expect("clipboard paths"),
            vec![PathBuf::from("dual.rs")]
        );
    }

    #[test]
    fn source_control_tree_toggle_preserves_groups_selection_and_directory_navigation() {
        let (_directory, mut app) = test_app();
        app.git_snapshot = Some(GitSnapshot {
            root: app.workspace.path().to_path_buf(),
            entries: vec![
                GitEntry {
                    path: PathBuf::from("src/dual.rs"),
                    rename_origin: None,
                    index_status: StatusCode::Added,
                    worktree_status: StatusCode::Modified,
                    submodule: Default::default(),
                    conflict: None,
                    ignored: false,
                    untracked: false,
                },
                GitEntry {
                    path: PathBuf::from("docs/new.md"),
                    rename_origin: None,
                    index_status: StatusCode::Clean,
                    worktree_status: StatusCode::Added,
                    submodule: Default::default(),
                    conflict: None,
                    ignored: false,
                    untracked: true,
                },
            ],
            ..GitSnapshot::default()
        });
        app.view = View::SourceControl;
        app.selection = 3;

        app.handle_source_key(KeyCode::Char('v'))
            .expect("switch to tree view");

        assert_eq!(app.git_view_mode, GitViewMode::Tree);
        assert!(app.git_tree_initialized);
        let rows = app.source_rows();
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            [
                "Staged Changes (1)",
                "src",
                "dual.rs",
                "Changes (1)",
                "src",
                "dual.rs",
                "Untracked (1)",
                "docs",
                "new.md",
            ]
        );
        assert_eq!(app.selection, 5);
        assert_eq!(rows[1].git.index, GitState::Added);
        assert_eq!(rows[4].git.worktree, GitState::Modified);

        app.handle_source_key(KeyCode::Char('v'))
            .expect("return to flat view");
        assert_eq!(app.git_view_mode, GitViewMode::Flat);
        assert_eq!(app.selection, 3);
        assert_eq!(app.source_rows()[3].name, "src/dual.rs");

        app.handle_source_key(KeyCode::Char('v'))
            .expect("restore tree view");
        app.handle_source_key(KeyCode::Char('h'))
            .expect("select parent directory");
        assert_eq!(app.source_rows()[app.selection].name, "src");
        app.handle_source_key(KeyCode::Enter)
            .expect("collapse directory");
        assert_eq!(app.source_items().len(), 8);
        app.handle_source_key(KeyCode::Char('l'))
            .expect("expand directory");
        app.handle_source_key(KeyCode::Char('l'))
            .expect("select first child");
        assert_eq!(app.source_rows()[app.selection].name, "dual.rs");
    }

    #[test]
    fn source_control_view_preference_and_expansion_persist_per_tab() {
        let (_directory, mut app) = test_app();
        app.view = View::SourceControl;
        app.git_snapshot = Some(GitSnapshot {
            root: app.workspace.path().to_path_buf(),
            entries: vec![GitEntry {
                path: PathBuf::from("src/changed.rs"),
                rename_origin: None,
                index_status: StatusCode::Clean,
                worktree_status: StatusCode::Modified,
                submodule: Default::default(),
                conflict: None,
                ignored: false,
                untracked: false,
            }],
            ..GitSnapshot::default()
        });

        app.handle_source_key(KeyCode::Char('v'))
            .expect("toggle and persist source view");

        let saved = PersistedState::load(&app.state_path).expect("load persisted source view");
        let tab = saved
            .tab(&app.workspace_id, &app.tab_id)
            .expect("saved workspace tab");
        assert_eq!(tab.git_view_mode, GitViewMode::Tree);
        assert!(tab.git_tree_initialized);
        assert_eq!(tab.git_tree_expanded, app.git_tree_expanded);
    }

    #[test]
    fn ignored_descendants_do_not_mute_a_non_ignored_directory() {
        assert_eq!(
            coordinates_for_directory(Some(DirectoryStatus::Ignored), false),
            GitCoordinates::default()
        );
        assert_eq!(
            coordinates_for_directory(Some(DirectoryStatus::Ignored), true),
            GitCoordinates {
                index: GitState::Clean,
                worktree: GitState::Ignored,
            }
        );
        assert_eq!(
            coordinates_for_directory(Some(DirectoryStatus::ModifiedOrTypeChanged), false),
            GitCoordinates {
                index: GitState::Modified,
                worktree: GitState::Clean,
            }
        );
    }

    #[test]
    fn selection_scroll_uses_the_actual_content_viewport() {
        assert_eq!(viewport_rows_for_height(78, false), 77);
        assert_eq!(viewport_rows_for_height(78, true), 75);
        assert_eq!(offset_for_selection(39, 20, 40, 77), 0);
        assert_eq!(offset_for_selection(76, 0, 100, 77), 0);
        assert_eq!(offset_for_selection(77, 0, 100, 77), 1);
        assert_eq!(offset_for_selection(99, 1, 100, 77), 23);
        assert_eq!(offset_for_selection(22, 23, 100, 77), 22);
    }

    #[test]
    fn terminal_appearance_env_is_explicit() {
        assert_eq!(
            parse_terminal_appearance_response(b"noise\x1b[?997;1n"),
            Some(Appearance::Dark)
        );
        assert_eq!(
            parse_terminal_appearance_response(b"\x1b[?997;2n"),
            Some(Appearance::Light)
        );
        assert_eq!(parse_terminal_appearance_response(b"unknown"), None);
    }

    #[test]
    fn hidden_state_releases_stale_pane_ownership() {
        let mut state = SidebarState {
            visible: true,
            pane_id: Some("workspace:pane".to_string()),
            ..SidebarState::default()
        };

        set_visibility(&mut state, false);

        assert!(!state.visible);
        assert_eq!(state.pane_id, None);
    }

    #[test]
    fn successful_theme_reload_clears_only_the_previous_theme_diagnostic() {
        let previous = ThemeResolution {
            palette: Palette::terminal(),
            name: "terminal".to_string(),
            diagnostic: Some("appearance unavailable".to_string()),
        };
        let next = ThemeResolution {
            palette: Palette::catppuccin(),
            name: "catppuccin".to_string(),
            diagnostic: None,
        };
        let mut error = previous.diagnostic.clone();
        replace_theme_diagnostic(&mut error, &previous, &next);
        assert_eq!(error, None);

        let mut unrelated = Some("external open failed".to_string());
        replace_theme_diagnostic(&mut unrelated, &previous, &next);
        assert_eq!(unrelated.as_deref(), Some("external open failed"));
    }
}
