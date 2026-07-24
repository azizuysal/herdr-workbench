use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::workspace::{WorkspaceError, WorkspaceRoot};

const PREVIEW_BYTE_LIMIT: usize = 64 * 1024;
const PREVIEW_LINE_LIMIT: usize = 1_000;
const PREVIEW_LINE_WIDTH: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Directory,
    File,
    Symlink,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub path: PathBuf,
    pub name: String,
    pub kind: NodeKind,
    pub expanded: bool,
    pub loaded: bool,
    pub ignored: bool,
    pub symlink_target: Option<String>,
    pub error: Option<String>,
    children: Vec<TreeNode>,
}

#[derive(Debug)]
pub enum FileTreeError {
    Workspace(WorkspaceError),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    NotDirectory(PathBuf),
    NotFile(PathBuf),
}
impl std::fmt::Display for FileTreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Workspace(error) => error.fmt(f),
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotDirectory(path) => write!(f, "{} is not a directory", path.display()),
            Self::NotFile(path) => write!(f, "{} is not a file", path.display()),
        }
    }
}
impl std::error::Error for FileTreeError {}
impl From<WorkspaceError> for FileTreeError {
    fn from(value: WorkspaceError) -> Self {
        Self::Workspace(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    Text {
        rendered: String,
        source: String,
        bytes: usize,
        truncated: bool,
    },
    Binary {
        bytes: usize,
        truncated: bool,
    },
}

/// A lazily-read root-relative tree. `visit_count` counts directory entries inspected, for tests
/// and for reporting traversal work without recursively loading the repository.
#[derive(Debug)]
pub struct FileTree {
    workspace: WorkspaceRoot,
    root: TreeNode,
    ignored_paths: BTreeSet<PathBuf>,
    show_ignored: bool,
    show_git_directory: bool,
    follow_symlinks: bool,
    visit_count: usize,
}

impl FileTree {
    pub fn new(workspace: WorkspaceRoot, show_ignored: bool, follow_symlinks: bool) -> Self {
        let name = workspace.path().file_name().map_or_else(
            || workspace.path().display().to_string(),
            |name| sanitize_terminal(&name.to_string_lossy()),
        );
        Self {
            workspace,
            root: TreeNode {
                path: PathBuf::new(),
                name,
                kind: NodeKind::Directory,
                expanded: true,
                loaded: false,
                ignored: false,
                symlink_target: None,
                error: None,
                children: Vec::new(),
            },
            ignored_paths: BTreeSet::new(),
            show_ignored,
            show_git_directory: false,
            follow_symlinks,
            visit_count: 0,
        }
    }
    pub fn root(&self) -> &TreeNode {
        &self.root
    }
    pub fn visit_count(&self) -> usize {
        self.visit_count
    }
    pub fn set_show_ignored(&mut self, show: bool) {
        self.show_ignored = show;
    }
    pub fn set_show_git_directory(&mut self, show: bool) {
        self.show_git_directory = show;
    }
    pub fn set_ignored_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.ignored_paths = paths.into_iter().collect();
    }

    pub fn expand(&mut self, relative: &Path) -> Result<(), FileTreeError> {
        let ignored_paths = &self.ignored_paths;
        let show_ignored = self.show_ignored;
        let show_git_directory = self.show_git_directory;
        let follow_symlinks = self.follow_symlinks;
        let workspace = &self.workspace;
        let visits = &mut self.visit_count;
        let node = find_node_mut(&mut self.root, relative)
            .ok_or_else(|| FileTreeError::NotDirectory(relative.to_owned()))?;
        if node.kind != NodeKind::Directory && !(node.kind == NodeKind::Symlink && follow_symlinks)
        {
            return Err(FileTreeError::NotDirectory(relative.to_owned()));
        }
        if node.kind == NodeKind::Symlink && would_cycle(workspace, &node.path) {
            node.error = Some("following this symlink would create a directory cycle".to_owned());
            return Ok(());
        }
        if !node.loaded {
            node.children = read_children(
                workspace,
                &node.path,
                ignored_paths,
                show_ignored,
                show_git_directory,
                follow_symlinks,
                visits,
            );
            node.loaded = true;
        }
        node.expanded = true;
        Ok(())
    }
    pub fn collapse(&mut self, relative: &Path) -> Result<(), FileTreeError> {
        let node = find_node_mut(&mut self.root, relative)
            .ok_or_else(|| FileTreeError::NotDirectory(relative.to_owned()))?;
        if node.kind != NodeKind::Directory {
            return Err(FileTreeError::NotDirectory(relative.to_owned()));
        }
        node.expanded = false;
        Ok(())
    }
    /// Re-reads only directories that were already loaded, retaining expansion and matching paths.
    pub fn refresh(&mut self) {
        let ignored_paths = &self.ignored_paths;
        let show_ignored = self.show_ignored;
        let show_git_directory = self.show_git_directory;
        let follow_symlinks = self.follow_symlinks;
        refresh_loaded(
            &self.workspace,
            &mut self.root,
            ignored_paths,
            show_ignored,
            show_git_directory,
            follow_symlinks,
            &mut self.visit_count,
        );
    }
    pub fn visible_nodes(&self) -> Vec<&TreeNode> {
        let mut nodes = Vec::new();
        collect_visible(&self.root, &mut nodes);
        nodes
    }
    pub fn preview(&self, relative: &Path) -> Result<Preview, FileTreeError> {
        let path = self.workspace.resolve_path(relative)?;
        let metadata = fs::metadata(&path).map_err(|source| FileTreeError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(FileTreeError::NotFile(relative.to_owned()));
        }
        sanitized_preview(&path)
    }
}

fn read_children(
    workspace: &WorkspaceRoot,
    parent: &Path,
    ignored: &BTreeSet<PathBuf>,
    show_ignored: bool,
    show_git_directory: bool,
    _follow_symlinks: bool,
    visits: &mut usize,
) -> Vec<TreeNode> {
    let absolute = match workspace.resolve_path(parent) {
        Ok(path) => path,
        Err(error) => return vec![error_node(parent, error.to_string())],
    };
    let entries = match fs::read_dir(&absolute) {
        Ok(entries) => entries,
        Err(error) => return vec![error_node(parent, error.to_string())],
    };
    let mut children = Vec::new();
    for entry in entries {
        *visits += 1;
        match entry {
            Ok(entry) => {
                let name = entry.file_name();
                if parent.as_os_str().is_empty()
                    && name == std::ffi::OsStr::new(".git")
                    && !show_git_directory
                {
                    continue;
                }
                let path = parent.join(&name);
                let is_ignored = ignored.contains(&path)
                    || ignored
                        .iter()
                        .any(|ignored_path| path.starts_with(ignored_path));
                if !show_ignored && is_ignored {
                    continue;
                }
                let metadata = match fs::symlink_metadata(entry.path()) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        children.push(error_node(&path, error.to_string()));
                        continue;
                    }
                };
                let kind = if metadata.file_type().is_symlink() {
                    NodeKind::Symlink
                } else if metadata.is_dir() {
                    NodeKind::Directory
                } else {
                    NodeKind::File
                };
                let target = if kind == NodeKind::Symlink {
                    fs::read_link(entry.path())
                        .ok()
                        .map(|target| sanitize_terminal(&target.to_string_lossy()))
                } else {
                    None
                };
                children.push(TreeNode {
                    path,
                    name: sanitize_terminal(&name.to_string_lossy()),
                    kind,
                    expanded: false,
                    loaded: false,
                    ignored: is_ignored,
                    symlink_target: target,
                    error: None,
                    children: Vec::new(),
                });
            }
            Err(error) => children.push(error_node(parent, error.to_string())),
        }
    }
    children.sort_by(|left, right| {
        let group = |kind| if kind == NodeKind::Directory { 0 } else { 1 };
        group(left.kind)
            .cmp(&group(right.kind))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
    children
}

fn refresh_loaded(
    workspace: &WorkspaceRoot,
    node: &mut TreeNode,
    ignored: &BTreeSet<PathBuf>,
    show_ignored: bool,
    show_git_directory: bool,
    follow_symlinks: bool,
    visits: &mut usize,
) {
    if (node.kind != NodeKind::Directory && node.kind != NodeKind::Symlink) || !node.loaded {
        return;
    }
    let old_children = std::mem::take(&mut node.children);
    let mut fresh = read_children(
        workspace,
        &node.path,
        ignored,
        show_ignored,
        show_git_directory,
        follow_symlinks,
        visits,
    );
    for child in &mut fresh {
        if let Some(old) = old_children
            .iter()
            .find(|old| old.path == child.path && old.kind == child.kind)
        {
            child.expanded = old.expanded;
            child.loaded = old.loaded;
            child.children = old.children.clone();
            refresh_loaded(
                workspace,
                child,
                ignored,
                show_ignored,
                show_git_directory,
                follow_symlinks,
                visits,
            );
        }
    }
    node.children = fresh;
}

fn find_node_mut<'a>(node: &'a mut TreeNode, path: &Path) -> Option<&'a mut TreeNode> {
    if node.path == path {
        return Some(node);
    }
    for child in &mut node.children {
        if let Some(found) = find_node_mut(child, path) {
            return Some(found);
        }
    }
    None
}
fn collect_visible<'a>(node: &'a TreeNode, output: &mut Vec<&'a TreeNode>) {
    output.push(node);
    if (node.kind == NodeKind::Directory || node.kind == NodeKind::Symlink)
        && node.expanded
        && node.loaded
    {
        for child in &node.children {
            collect_visible(child, output);
        }
    }
}

fn would_cycle(workspace: &WorkspaceRoot, path: &Path) -> bool {
    let target = match workspace.resolve_path(path) {
        Ok(target) => target,
        Err(_) => return true,
    };
    let mut ancestor = PathBuf::new();
    for component in path.components() {
        if let std::path::Component::Normal(part) = component {
            ancestor.push(part);
            if ancestor == path {
                break;
            }
            if let Ok(resolved) = workspace.resolve_path(&ancestor)
                && resolved == target
            {
                return true;
            }
        }
    }
    target == workspace.path()
}
fn error_node(path: &Path, error: String) -> TreeNode {
    TreeNode {
        path: path.to_owned(),
        name: format!("[{}]", sanitize_terminal(&error)),
        kind: NodeKind::Error,
        expanded: false,
        loaded: true,
        ignored: false,
        symlink_target: None,
        error: Some(error),
        children: Vec::new(),
    }
}

pub fn sanitized_preview(path: &Path) -> Result<Preview, FileTreeError> {
    let metadata = fs::metadata(path).map_err(|source| FileTreeError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut file = fs::File::open(path).map_err(|source| FileTreeError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(PREVIEW_BYTE_LIMIT + 1);
    file.by_ref()
        .take((PREVIEW_BYTE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|source| FileTreeError::Io {
            path: path.to_owned(),
            source,
        })?;
    let truncated = bytes.len() > PREVIEW_BYTE_LIMIT || metadata.len() > PREVIEW_BYTE_LIMIT as u64;
    bytes.truncate(PREVIEW_BYTE_LIMIT);
    if bytes.contains(&0) {
        return Ok(Preview::Binary {
            bytes: metadata.len() as usize,
            truncated,
        });
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(_) => {
            return Ok(Preview::Binary {
                bytes: metadata.len() as usize,
                truncated,
            });
        }
    };
    let mut lines = Vec::new();
    let mut source_lines = Vec::new();
    for (index, line) in text.lines().take(PREVIEW_LINE_LIMIT).enumerate() {
        let sanitized = sanitize_terminal(line);
        let clipped = truncate_chars(&sanitized, PREVIEW_LINE_WIDTH);
        source_lines.push(clipped.clone());
        lines.push(format!("{:>4}  {clipped}", index + 1));
    }
    let line_truncated = text.lines().count() > PREVIEW_LINE_LIMIT;
    if truncated || line_truncated {
        lines.push("     … [preview truncated]".to_owned());
    }
    Ok(Preview::Text {
        rendered: lines.join("\n"),
        source: source_lines.join("\n"),
        bytes: metadata.len() as usize,
        truncated: truncated || line_truncated,
    })
}

pub fn sanitize_terminal(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\x1b' => "\\x1b".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect(),
            '\r' => "\\r".chars().collect(),
            '\t' => "\\t".chars().collect(),
            character if character.is_control() || character == '\u{7f}' => {
                format!("\\u{{{:04x}}}", character as u32).chars().collect()
            }
            character => vec![character],
        })
        .collect()
}
fn truncate_chars(value: &str, maximum: usize) -> String {
    let mut output: String = value.chars().take(maximum).collect();
    if value.chars().count() > maximum {
        output.push('…');
    }
    output
}
