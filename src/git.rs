//! Read-only Git status and preview support for the sidebar.
//!
//! The provider deliberately uses one porcelain-v2 invocation per refresh.  All
//! row, directory, and Source Control data is derived from that immutable
//! snapshot so a render never combines state from separate Git invocations.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
};

const PREVIEW_BYTE_LIMIT: u64 = 256 * 1024;
pub const HISTORY_LIMIT: usize = 50;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitCommit {
    pub oid: String,
    pub short_oid: String,
    pub timestamp: i64,
    pub summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitError {
    pub command: String,
    pub message: String,
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.command, self.message)
    }
}
impl std::error::Error for GitError {}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum StatusCode {
    #[default]
    Clean,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Unknown(char),
}

impl StatusCode {
    fn from_porcelain(value: u8) -> Self {
        match value {
            b'.' | b' ' => Self::Clean,
            b'M' => Self::Modified,
            b'A' => Self::Added,
            b'D' => Self::Deleted,
            b'R' => Self::Renamed,
            b'C' => Self::Copied,
            b'T' => Self::TypeChanged,
            b'U' => Self::Unmerged,
            other => Self::Unknown(other as char),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SubmoduleStatus {
    pub is_submodule: bool,
    pub commit_changed: bool,
    pub modified: bool,
    pub untracked: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConflictStatus {
    pub index: char,
    pub worktree: char,
    pub modes: [String; 4],
    pub stages: [String; 3],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitEntry {
    /// A root-relative path. This is an `OsString` internally, so Unix names
    /// never need to be decoded as UTF-8 while parsing porcelain output.
    pub path: PathBuf,
    pub rename_origin: Option<PathBuf>,
    pub index_status: StatusCode,
    pub worktree_status: StatusCode,
    pub submodule: SubmoduleStatus,
    pub conflict: Option<ConflictStatus>,
    pub ignored: bool,
    pub untracked: bool,
}

impl GitEntry {
    pub fn badge(&self) -> String {
        if self.conflict.is_some() {
            return "C".into();
        }
        if self.ignored {
            return String::new();
        }
        if self.untracked {
            return "U".into();
        }
        let status = if self.worktree_status != StatusCode::Clean {
            self.worktree_status
        } else {
            self.index_status
        };
        badge_for(status).map_or_else(String::new, |badge| badge.to_string())
    }

    pub fn semantic_status(&self) -> DirectoryStatus {
        if self.conflict.is_some() {
            return DirectoryStatus::Conflict;
        }
        if self.ignored {
            return DirectoryStatus::Ignored;
        }
        [self.index_status, self.worktree_status]
            .into_iter()
            .map(DirectoryStatus::from)
            .max()
            .unwrap_or(DirectoryStatus::Clean)
    }
}

fn badge_for(status: StatusCode) -> Option<char> {
    match status {
        StatusCode::Clean => None,
        StatusCode::Modified => Some('M'),
        StatusCode::Added => Some('A'),
        StatusCode::Deleted => Some('D'),
        StatusCode::Renamed => Some('R'),
        StatusCode::Copied => Some('C'),
        StatusCode::TypeChanged => Some('T'),
        StatusCode::Unmerged => Some('C'),
        StatusCode::Unknown(_) => Some('?'),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum DirectoryStatus {
    #[default]
    Clean,
    Ignored,
    CopiedOrRenamed,
    AddedOrUntracked,
    ModifiedOrTypeChanged,
    Deleted,
    Conflict,
}

impl From<StatusCode> for DirectoryStatus {
    fn from(value: StatusCode) -> Self {
        match value {
            StatusCode::Clean => Self::Clean,
            StatusCode::Renamed | StatusCode::Copied => Self::CopiedOrRenamed,
            StatusCode::Added => Self::AddedOrUntracked,
            StatusCode::Modified | StatusCode::TypeChanged | StatusCode::Unknown(_) => {
                Self::ModifiedOrTypeChanged
            }
            StatusCode::Deleted => Self::Deleted,
            StatusCode::Unmerged => Self::Conflict,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DirectorySummary {
    pub status: DirectoryStatus,
    pub counts: BTreeMap<DirectoryStatus, usize>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BranchStatus {
    pub oid: Option<String>,
    pub name: Option<String>,
    pub detached: bool,
    pub unborn: bool,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub stash_count: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitSnapshot {
    pub root: PathBuf,
    pub branch: BranchStatus,
    pub entries: Vec<GitEntry>,
    /// Includes the root (`PathBuf::new()`) and every affected ancestor.
    pub directories: BTreeMap<PathBuf, DirectorySummary>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SourceControlGroup {
    MergeChanges,
    StagedChanges,
    Changes,
    Untracked,
}

impl GitSnapshot {
    pub fn groups(&self) -> BTreeMap<SourceControlGroup, Vec<&GitEntry>> {
        let mut groups = BTreeMap::new();
        for group in [
            SourceControlGroup::MergeChanges,
            SourceControlGroup::StagedChanges,
            SourceControlGroup::Changes,
            SourceControlGroup::Untracked,
        ] {
            groups.insert(group, Vec::new());
        }
        for entry in &self.entries {
            if entry.conflict.is_some() {
                groups
                    .get_mut(&SourceControlGroup::MergeChanges)
                    .unwrap()
                    .push(entry);
            }
            if !entry.untracked
                && !entry.ignored
                && entry.conflict.is_none()
                && entry.index_status != StatusCode::Clean
            {
                groups
                    .get_mut(&SourceControlGroup::StagedChanges)
                    .unwrap()
                    .push(entry);
            }
            if !entry.untracked
                && !entry.ignored
                && entry.conflict.is_none()
                && entry.worktree_status != StatusCode::Clean
            {
                groups
                    .get_mut(&SourceControlGroup::Changes)
                    .unwrap()
                    .push(entry);
            }
            if entry.untracked {
                groups
                    .get_mut(&SourceControlGroup::Untracked)
                    .unwrap()
                    .push(entry);
            }
        }
        groups
    }
}

#[derive(Clone)]
pub struct GitStatusProvider {
    root: PathBuf,
    last_valid: Arc<Mutex<Option<GitSnapshot>>>,
}

impl GitStatusProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            last_valid: Arc::new(Mutex::new(None)),
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn last_valid(&self) -> Option<GitSnapshot> {
        self.last_valid
            .lock()
            .expect("git snapshot mutex poisoned")
            .clone()
    }

    pub fn refresh(&self) -> Result<GitSnapshot, GitError> {
        let snapshot = snapshot_from_git(&self.root)?;
        *self.last_valid.lock().expect("git snapshot mutex poisoned") = Some(snapshot.clone());
        Ok(snapshot)
    }

    /// Starts a read-only refresh. On failure, `last_valid` remains unchanged.
    pub fn refresh_async(&self) -> mpsc::Receiver<Result<GitSnapshot, GitError>> {
        let provider = self.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(provider.refresh());
        });
        receiver
    }

    pub fn history(&self) -> Result<Vec<GitCommit>, GitError> {
        let head = Command::new("git")
            .args([
                "--no-optional-locks",
                "rev-parse",
                "--verify",
                "--quiet",
                "HEAD",
            ])
            .current_dir(&self.root)
            .output()
            .map_err(|error| GitError {
                command: "git rev-parse --verify --quiet HEAD".into(),
                message: error.to_string(),
            })?;
        if !head.status.success() {
            if head.stderr.is_empty() {
                return Ok(Vec::new());
            }
            return Err(GitError {
                command: "git rev-parse --verify --quiet HEAD".into(),
                message: sanitize_bytes(&head.stderr),
            });
        }

        let output = Command::new("git")
            .args(["--no-optional-locks", "log"])
            .arg(format!("--max-count={HISTORY_LIMIT}"))
            .args(["--date-order", "-z", "--format=%H%x00%h%x00%at%x00%s"])
            .current_dir(&self.root)
            .output()
            .map_err(|error| GitError {
                command: "git log".into(),
                message: error.to_string(),
            })?;
        if !output.status.success() {
            return Err(GitError {
                command: "git log".into(),
                message: sanitize_bytes(&output.stderr),
            });
        }
        parse_history(&output.stdout)
    }

    pub fn history_async(&self) -> mpsc::Receiver<Result<Vec<GitCommit>, GitError>> {
        let provider = self.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(provider.history());
        });
        receiver
    }

    pub fn commit_preview(&self, oid: &str) -> Result<String, GitError> {
        if !valid_full_oid(oid) {
            return Err(GitError {
                command: "commit preview".into(),
                message: "commit id must be a full hexadecimal object id".into(),
            });
        }
        let output = Command::new("git")
            .args([
                "--no-optional-locks",
                "show",
                "--no-ext-diff",
                "--no-color",
                "--format=fuller",
                "--stat",
                "--patch",
                "--max-count=1",
                oid,
                "--",
            ])
            .current_dir(&self.root)
            .output()
            .map_err(|error| GitError {
                command: "git show".into(),
                message: error.to_string(),
            })?;
        if !output.status.success() {
            return Err(GitError {
                command: "git show".into(),
                message: sanitize_bytes(&output.stderr),
            });
        }
        let mut stdout = output.stdout;
        let truncated = stdout.len() as u64 > PREVIEW_BYTE_LIMIT;
        stdout.truncate(PREVIEW_BYTE_LIMIT as usize);
        let mut preview = sanitize_bytes(&stdout);
        if truncated {
            preview.push_str("\n[commit preview truncated]");
        }
        Ok(preview)
    }

    pub fn preview(&self, entry: &GitEntry, group: SourceControlGroup) -> Result<String, GitError> {
        let path = safe_relative(&entry.path).ok_or_else(|| GitError {
            command: "preview".into(),
            message: "path escapes the workspace root".into(),
        })?;
        match group {
            SourceControlGroup::StagedChanges => self.git_diff(true, path),
            SourceControlGroup::Changes => self.git_diff(false, path),
            SourceControlGroup::MergeChanges => {
                let conflict = entry.conflict.as_ref().ok_or_else(|| GitError {
                    command: "preview unmerged path".into(),
                    message: "Merge Changes row has no unmerged metadata".into(),
                })?;
                let diff = self.git_diff(false, path)?;
                Ok(format!(
                    "unmerged {}{}: {}\n{}",
                    conflict.index,
                    conflict.worktree,
                    entry.path.display(),
                    diff
                ))
            }
            SourceControlGroup::Untracked => {
                let full = self.root.join(path);
                let canonical_root = self.root.canonicalize().map_err(|e| GitError {
                    command: "preview".into(),
                    message: e.to_string(),
                })?;
                let canonical = full.canonicalize().map_err(|e| GitError {
                    command: format!("read {}", full.display()),
                    message: e.to_string(),
                })?;
                if !canonical.starts_with(canonical_root) {
                    return Err(GitError {
                        command: "preview".into(),
                        message: "path resolves outside the workspace root".into(),
                    });
                }
                let file = std::fs::File::open(&full).map_err(|e| GitError {
                    command: format!("read {}", full.display()),
                    message: e.to_string(),
                })?;
                let mut bytes = Vec::new();
                file.take(PREVIEW_BYTE_LIMIT + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| GitError {
                        command: format!("read {}", full.display()),
                        message: e.to_string(),
                    })?;
                let truncated = bytes.len() as u64 > PREVIEW_BYTE_LIMIT;
                bytes.truncate(PREVIEW_BYTE_LIMIT as usize);
                if bytes.contains(&0) {
                    return Ok(format!(
                        "untracked binary: {} · at least {} bytes{}",
                        entry.path.display(),
                        bytes.len(),
                        if truncated {
                            " · preview truncated"
                        } else {
                            ""
                        }
                    ));
                }
                Ok(format!(
                    "untracked: {}{}\n{}",
                    entry.path.display(),
                    if truncated {
                        " · preview truncated"
                    } else {
                        ""
                    },
                    sanitize_bytes(&bytes)
                ))
            }
        }
    }

    fn git_diff(&self, staged: bool, path: &Path) -> Result<String, GitError> {
        let mut command = Command::new("git");
        command.arg("--no-optional-locks").arg("diff");
        if staged {
            command.arg("--cached");
        }
        let output = command
            .arg("--no-ext-diff")
            .arg("--")
            .arg(path)
            .current_dir(&self.root)
            .output()
            .map_err(|e| GitError {
                command: "git diff".into(),
                message: e.to_string(),
            })?;
        if !output.status.success() {
            return Err(GitError {
                command: "git diff".into(),
                message: sanitize_bytes(&output.stderr),
            });
        }
        let mut stdout = output.stdout;
        let truncated = stdout.len() as u64 > PREVIEW_BYTE_LIMIT;
        stdout.truncate(PREVIEW_BYTE_LIMIT as usize);
        let mut preview = sanitize_bytes(&stdout);
        if truncated {
            preview.push_str("\n[diff preview truncated]");
        }
        Ok(preview)
    }
}

fn parse_history(bytes: &[u8]) -> Result<Vec<GitCommit>, GitError> {
    let mut fields = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    if fields.last().is_some_and(|field| field.is_empty()) {
        fields.pop();
    }
    if !fields.len().is_multiple_of(4) {
        return Err(GitError {
            command: "git log".into(),
            message: "unexpected commit history output".into(),
        });
    }
    fields
        .chunks_exact(4)
        .map(|fields| {
            let oid = String::from_utf8_lossy(fields[0]).into_owned();
            let short_oid = String::from_utf8_lossy(fields[1]).into_owned();
            let timestamp = String::from_utf8_lossy(fields[2])
                .parse::<i64>()
                .map_err(|_| GitError {
                    command: "git log".into(),
                    message: "invalid commit timestamp".into(),
                })?;
            if !valid_full_oid(&oid)
                || short_oid.is_empty()
                || !short_oid.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(GitError {
                    command: "git log".into(),
                    message: "invalid commit object id".into(),
                });
            }
            Ok(GitCommit {
                oid,
                short_oid,
                timestamp,
                summary: sanitize_history_summary(fields[3]),
            })
        })
        .collect()
}

fn valid_full_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sanitize_history_summary(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

pub fn snapshot_from_git(root: &Path) -> Result<GitSnapshot, GitError> {
    let output = Command::new("git")
        .args(["--no-optional-locks", "status", "--porcelain=v2", "-z", "--branch", "--show-stash", "--ignored=matching", "--untracked-files=all"])
        .current_dir(root).output()
        .map_err(|e| GitError { command: "git --no-optional-locks status --porcelain=v2 -z --branch --show-stash --ignored=matching --untracked-files=all".into(), message: e.to_string() })?;
    if !output.status.success() {
        return Err(GitError {
            command: "git status --porcelain=v2".into(),
            message: sanitize_bytes(&output.stderr),
        });
    }
    parse_porcelain_v2(root.to_path_buf(), &output.stdout)
}

pub fn parse_porcelain_v2(root: PathBuf, bytes: &[u8]) -> Result<GitSnapshot, GitError> {
    let mut snapshot = GitSnapshot {
        root,
        ..GitSnapshot::default()
    };
    let mut records = bytes.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        match record[0] {
            b'#' => parse_header(record, &mut snapshot.branch),
            b'1' => snapshot.entries.push(parse_ordinary(record)?),
            b'2' => {
                let mut entry = parse_renamed(record)?;
                entry.rename_origin = records
                    .next()
                    .filter(|p| !p.is_empty())
                    .map(path_from_bytes);
                snapshot.entries.push(entry);
            }
            b'u' => snapshot.entries.push(parse_unmerged(record)?),
            b'?' => snapshot.entries.push(GitEntry {
                path: path_from_bytes(strip_prefix(record, b"? ")?),
                index_status: StatusCode::Clean,
                worktree_status: StatusCode::Added,
                submodule: SubmoduleStatus::default(),
                conflict: None,
                ignored: false,
                untracked: true,
                rename_origin: None,
            }),
            b'!' => snapshot.entries.push(GitEntry {
                path: path_from_bytes(strip_prefix(record, b"! ")?),
                index_status: StatusCode::Clean,
                worktree_status: StatusCode::Clean,
                submodule: SubmoduleStatus::default(),
                conflict: None,
                ignored: true,
                untracked: false,
                rename_origin: None,
            }),
            _ => return Err(parse_error("unknown porcelain-v2 record", record)),
        }
    }
    snapshot.directories = aggregate_directories(&snapshot.entries);
    Ok(snapshot)
}

fn parse_header(record: &[u8], branch: &mut BranchStatus) {
    let value = String::from_utf8_lossy(record);
    if let Some(oid) = value.strip_prefix("# branch.oid ") {
        branch.unborn = oid == "(initial)";
        branch.oid = (!branch.unborn).then(|| oid.to_string());
    } else if let Some(head) = value.strip_prefix("# branch.head ") {
        branch.detached = head == "(detached)";
        branch.name = (!branch.detached).then(|| head.to_string());
    } else if let Some(upstream) = value.strip_prefix("# branch.upstream ") {
        branch.upstream = Some(upstream.to_string());
    } else if let Some(counts) = value.strip_prefix("# branch.ab ") {
        let mut counts = counts.split_ascii_whitespace();
        branch.ahead = counts
            .next()
            .and_then(|value| value.strip_prefix('+'))
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        branch.behind = counts
            .next()
            .and_then(|value| value.strip_prefix('-'))
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
    } else if let Some(stash) = value.strip_prefix("# stash ") {
        branch.stash_count = stash.parse().unwrap_or(0);
    }
}

fn parse_ordinary(record: &[u8]) -> Result<GitEntry, GitError> {
    let fields = split_fields(record, 9)?;
    let xy = fields[1];
    if xy.len() != 2 {
        return Err(parse_error("invalid XY field", record));
    }
    Ok(GitEntry {
        path: path_from_bytes(fields[8]),
        rename_origin: None,
        index_status: StatusCode::from_porcelain(xy[0]),
        worktree_status: StatusCode::from_porcelain(xy[1]),
        submodule: parse_submodule(fields[2]),
        conflict: None,
        ignored: false,
        untracked: false,
    })
}

fn parse_renamed(record: &[u8]) -> Result<GitEntry, GitError> {
    let fields = split_fields(record, 10)?;
    let xy = fields[1];
    if xy.len() != 2 {
        return Err(parse_error("invalid XY field", record));
    }
    Ok(GitEntry {
        path: path_from_bytes(fields[9]),
        rename_origin: None,
        index_status: StatusCode::from_porcelain(xy[0]),
        worktree_status: StatusCode::from_porcelain(xy[1]),
        submodule: parse_submodule(fields[2]),
        conflict: None,
        ignored: false,
        untracked: false,
    })
}

fn parse_unmerged(record: &[u8]) -> Result<GitEntry, GitError> {
    let fields = split_fields(record, 11)?;
    let xy = fields[1];
    if xy.len() != 2 {
        return Err(parse_error("invalid unmerged XY field", record));
    }
    Ok(GitEntry {
        path: path_from_bytes(fields[10]),
        rename_origin: None,
        index_status: StatusCode::Unmerged,
        worktree_status: StatusCode::Unmerged,
        submodule: parse_submodule(fields[2]),
        conflict: Some(ConflictStatus {
            index: xy[0] as char,
            worktree: xy[1] as char,
            modes: [
                String::from_utf8_lossy(fields[3]).into_owned(),
                String::from_utf8_lossy(fields[4]).into_owned(),
                String::from_utf8_lossy(fields[5]).into_owned(),
                String::from_utf8_lossy(fields[6]).into_owned(),
            ],
            stages: [
                String::from_utf8_lossy(fields[7]).into_owned(),
                String::from_utf8_lossy(fields[8]).into_owned(),
                String::from_utf8_lossy(fields[9]).into_owned(),
            ],
        }),
        ignored: false,
        untracked: false,
    })
}

fn split_fields(record: &[u8], required: usize) -> Result<Vec<&[u8]>, GitError> {
    let fields: Vec<_> = record.splitn(required, |b| *b == b' ').collect();
    if fields.len() != required {
        return Err(parse_error("truncated porcelain-v2 record", record));
    }
    Ok(fields)
}
fn strip_prefix<'a>(value: &'a [u8], prefix: &[u8]) -> Result<&'a [u8], GitError> {
    value
        .strip_prefix(prefix)
        .ok_or_else(|| parse_error("malformed porcelain-v2 record", value))
}
fn parse_error(message: &str, record: &[u8]) -> GitError {
    GitError {
        command: "parse git status --porcelain=v2".into(),
        message: format!("{message}: {}", sanitize_bytes(record)),
    }
}
fn parse_submodule(value: &[u8]) -> SubmoduleStatus {
    SubmoduleStatus {
        is_submodule: value.first() == Some(&b'S'),
        commit_changed: value.get(1) == Some(&b'C'),
        modified: value.get(2) == Some(&b'M'),
        untracked: value.get(3) == Some(&b'U'),
    }
}

pub fn aggregate_directories(entries: &[GitEntry]) -> BTreeMap<PathBuf, DirectorySummary> {
    let mut result = BTreeMap::new();
    for entry in entries {
        let state = if entry.untracked {
            DirectoryStatus::AddedOrUntracked
        } else {
            entry.semantic_status()
        };
        let mut ancestors = BTreeSet::new();
        for path in std::iter::once(&entry.path).chain(entry.rename_origin.iter()) {
            let mut parent = path.parent();
            while let Some(directory) = parent {
                ancestors.insert(directory.to_path_buf());
                parent = directory.parent();
            }
            ancestors.insert(PathBuf::new());
        }
        for directory in ancestors {
            let summary = result
                .entry(directory)
                .or_insert_with(DirectorySummary::default);
            summary.status = summary.status.max(state);
            *summary.counts.entry(state).or_insert(0) += 1;
        }
    }
    result
}

fn safe_relative(path: &Path) -> Option<&Path> {
    if path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        None
    } else {
        Some(path)
    }
}

pub fn sanitize_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn history_parser_rejects_partial_records_and_sanitizes_subjects() {
        assert!(parse_history(b"abc\0def\0").is_err());
        let oid = "a".repeat(40);
        let input = format!("{oid}\0abcdef0\01700000000\0subject\twith\ncontrols\0");

        let commits = parse_history(input.as_bytes()).expect("parse history");

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].summary, "subject with controls");
    }

    #[test]
    fn commit_preview_requires_a_full_object_id() {
        let error = GitStatusProvider::new(".")
            .commit_preview("HEAD")
            .expect_err("symbolic revision must be rejected");

        assert_eq!(
            error.message,
            "commit id must be a full hexadecimal object id"
        );
    }
}
