//! Cancellable, root-bound filename and content search.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use regex::{Regex, RegexBuilder};

const CONTENT_SEARCH_BYTE_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SearchMode {
    #[default]
    Filename,
    Literal,
    Regex,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchQuery {
    pub text: String,
    pub mode: SearchMode,
    pub case_sensitive: bool,
    pub include_ignored: bool,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            mode: SearchMode::Filename,
            case_sensitive: false,
            include_ignored: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchMatch {
    pub line: usize,
    pub snippet: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchFileResult {
    pub path: PathBuf,
    pub matches: Vec<SearchMatch>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchError {
    pub path: Option<PathBuf>,
    pub message: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchResults {
    pub files: Vec<SearchFileResult>,
    pub errors: Vec<SearchError>,
    pub cancelled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchUpdate {
    Match(SearchFileResult),
    Finished {
        errors: Vec<SearchError>,
        cancelled: bool,
    },
}

#[derive(Clone)]
pub struct SearchProvider {
    root: PathBuf,
}

pub struct SearchHandle {
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<SearchUpdate>,
}

impl SearchHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn try_recv(&self) -> Result<SearchUpdate, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
    pub fn recv(self) -> Result<SearchResults, mpsc::RecvError> {
        let mut results = SearchResults::default();
        loop {
            match self.receiver.recv()? {
                SearchUpdate::Match(file) => results.files.push(file),
                SearchUpdate::Finished { errors, cancelled } => {
                    results.errors = errors;
                    results.cancelled = cancelled;
                    results
                        .files
                        .sort_by(|left, right| left.path.cmp(&right.path));
                    return Ok(results);
                }
            }
        }
    }
}

impl SearchProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validates the query before work is queued, so invalid expressions are
    /// immediate local UI errors rather than a failed background task.
    pub fn start(&self, query: SearchQuery) -> Result<SearchHandle, SearchError> {
        validate_query(&query)?;
        let root = self.root.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let task_cancel = Arc::clone(&cancel);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let results = run_search(&root, &query, &task_cancel, Some(&sender));
            let _ = sender.send(SearchUpdate::Finished {
                errors: results.errors,
                cancelled: results.cancelled,
            });
        });
        Ok(SearchHandle { cancel, receiver })
    }

    pub fn search(
        &self,
        query: &SearchQuery,
        cancel: &AtomicBool,
    ) -> Result<SearchResults, SearchError> {
        validate_query(query)?;
        Ok(run_search(&self.root, query, cancel, None))
    }
}

fn validate_query(query: &SearchQuery) -> Result<(), SearchError> {
    if query.mode == SearchMode::Regex {
        RegexBuilder::new(&query.text)
            .case_insensitive(!query.case_sensitive)
            .build()
            .map_err(|error| SearchError {
                path: None,
                message: format!("invalid regular expression: {error}"),
            })?;
    }
    Ok(())
}

fn run_search(
    root: &Path,
    query: &SearchQuery,
    cancel: &AtomicBool,
    updates: Option<&mpsc::Sender<SearchUpdate>>,
) -> SearchResults {
    let canonical_root = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(error) => {
            return SearchResults {
                errors: vec![SearchError {
                    path: Some(root.to_path_buf()),
                    message: format!("cannot read workspace root: {error}"),
                }],
                ..SearchResults::default()
            };
        }
    };
    let matcher = match matcher(query) {
        Ok(value) => value,
        Err(error) => {
            return SearchResults {
                errors: vec![error],
                ..SearchResults::default()
            };
        }
    };
    let ignored = IgnoreRules::read(&canonical_root);
    let mut results = SearchResults::default();
    SearchWalker {
        root: &canonical_root,
        query,
        matcher: &matcher,
        ignored: &ignored,
        cancel,
        updates,
    }
    .walk(&canonical_root, &mut results);
    results
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    results.cancelled = cancel.load(Ordering::Relaxed);
    results
}

enum Matcher {
    Filename {
        needle: String,
        case_sensitive: bool,
    },
    Literal {
        needle: String,
        case_sensitive: bool,
    },
    Regex(Regex),
}
fn matcher(query: &SearchQuery) -> Result<Matcher, SearchError> {
    let text = if query.case_sensitive {
        query.text.clone()
    } else {
        query.text.to_lowercase()
    };
    match query.mode {
        SearchMode::Filename => Ok(Matcher::Filename {
            needle: text,
            case_sensitive: query.case_sensitive,
        }),
        SearchMode::Literal => Ok(Matcher::Literal {
            needle: text,
            case_sensitive: query.case_sensitive,
        }),
        SearchMode::Regex => RegexBuilder::new(&query.text)
            .case_insensitive(!query.case_sensitive)
            .build()
            .map(Matcher::Regex)
            .map_err(|e| SearchError {
                path: None,
                message: format!("invalid regular expression: {e}"),
            }),
    }
}

struct SearchWalker<'a> {
    root: &'a Path,
    query: &'a SearchQuery,
    matcher: &'a Matcher,
    ignored: &'a IgnoreRules,
    cancel: &'a AtomicBool,
    updates: Option<&'a mpsc::Sender<SearchUpdate>>,
}

impl SearchWalker<'_> {
    fn walk(&self, directory: &Path, results: &mut SearchResults) {
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }
        let read_dir = match fs::read_dir(directory) {
            Ok(value) => value,
            Err(error) => {
                results.errors.push(SearchError {
                    path: Some(directory.to_path_buf()),
                    message: format!("cannot read directory: {error}"),
                });
                return;
            }
        };
        let mut entries = Vec::new();
        for item in read_dir {
            if self.cancel.load(Ordering::Relaxed) {
                return;
            }
            let entry = match item {
                Ok(value) => value,
                Err(error) => {
                    results.errors.push(SearchError {
                        path: Some(directory.to_path_buf()),
                        message: format!("cannot read directory entry: {error}"),
                    });
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(value) => value,
                Err(error) => {
                    results.errors.push(SearchError {
                        path: Some(path),
                        message: format!("cannot inspect path: {error}"),
                    });
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            let relative = match path.strip_prefix(self.root) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if self.ignored.matches(relative, metadata.is_dir()) && !self.query.include_ignored {
                continue;
            }
            entries.push((path, metadata));
        }
        entries.sort_by(|(left_path, left_metadata), (right_path, right_metadata)| {
            left_metadata
                .is_dir()
                .cmp(&right_metadata.is_dir())
                .then_with(|| left_path.cmp(right_path))
        });

        for (path, metadata) in entries {
            if self.cancel.load(Ordering::Relaxed) {
                return;
            }
            let relative = match path.strip_prefix(self.root) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                self.walk(&path, results);
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let matches = if self.query.mode == SearchMode::Filename {
                filename_matches(relative, self.matcher)
            } else if metadata.len() > CONTENT_SEARCH_BYTE_LIMIT {
                Vec::new()
            } else {
                content_matches(&path, self.matcher, self.cancel, results)
            };
            if !matches.is_empty() {
                let result = SearchFileResult {
                    path: relative.to_path_buf(),
                    matches,
                };
                if self
                    .updates
                    .is_some_and(|sender| sender.send(SearchUpdate::Match(result.clone())).is_err())
                {
                    return;
                }
                results.files.push(result);
            }
        }
    }
}

fn filename_matches(path: &Path, matcher: &Matcher) -> Vec<SearchMatch> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let haystack = match matcher {
        Matcher::Filename { needle, .. } if needle.is_empty() => return Vec::new(),
        Matcher::Filename {
            needle,
            case_sensitive,
        } if fuzzy_filename_matches(&name, needle, *case_sensitive) => name.into_owned(),
        Matcher::Filename { .. } => return Vec::new(),
        _ => return Vec::new(),
    };
    vec![SearchMatch {
        line: 0,
        snippet: sanitize_text(&haystack),
    }]
}

pub fn fuzzy_filename_matches(value: &str, needle: &str, case_sensitive: bool) -> bool {
    if needle.is_empty() {
        return true;
    }
    if case_sensitive {
        fuzzy_contains(value, needle)
    } else {
        fuzzy_contains(&value.to_lowercase(), &needle.to_lowercase())
    }
}

fn content_matches(
    path: &Path,
    matcher: &Matcher,
    cancel: &AtomicBool,
    results: &mut SearchResults,
) -> Vec<SearchMatch> {
    let file = match fs::File::open(path) {
        Ok(value) => value,
        Err(error) => {
            results.errors.push(SearchError {
                path: Some(path.to_path_buf()),
                message: format!("cannot read file: {error}"),
            });
            return Vec::new();
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = file
        .take(CONTENT_SEARCH_BYTE_LIMIT + 1)
        .read_to_end(&mut bytes)
    {
        results.errors.push(SearchError {
            path: Some(path.to_path_buf()),
            message: format!("cannot read file: {error}"),
        });
        return Vec::new();
    }
    if bytes.len() as u64 > CONTENT_SEARCH_BYTE_LIMIT {
        return Vec::new();
    }
    if bytes.contains(&0) {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let matches = match matcher {
            Matcher::Literal {
                needle,
                case_sensitive,
            } => {
                if *case_sensitive {
                    line.contains(needle)
                } else {
                    line.to_lowercase().contains(needle)
                }
            }
            Matcher::Regex(regex) => regex.is_match(line),
            Matcher::Filename { .. } => false,
        };
        if matches {
            found.push(SearchMatch {
                line: index + 1,
                snippet: sanitize_text(line),
            });
        }
    }
    found
}

fn fuzzy_contains(value: &str, needle: &str) -> bool {
    let mut wanted = needle.chars();
    let mut current = wanted.next();
    for character in value.chars() {
        if Some(character) == current {
            current = wanted.next();
            if current.is_none() {
                return true;
            }
        }
    }
    current.is_none()
}

/// Removes every terminal control character, including ESC, while retaining
/// printable Unicode. Tabs are rendered as spaces to keep one-line snippets.
pub fn sanitize_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character == '\t' {
                ' '
            } else if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}

#[derive(Default)]
struct IgnoreRules {
    exact: BTreeSet<PathBuf>,
    patterns: Vec<String>,
}
impl IgnoreRules {
    fn read(root: &Path) -> Self {
        if let Ok(output) = Command::new("git")
            .args([
                "--no-optional-locks",
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "--no-empty-directory",
                "-z",
            ])
            .current_dir(root)
            .output()
            && output.status.success()
        {
            let exact = output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
                .map(path_from_bytes)
                .collect();
            return Self {
                exact,
                patterns: Vec::new(),
            };
        }
        let patterns = fs::read_to_string(root.join(".gitignore"))
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('!'))
            .map(ToOwned::to_owned)
            .collect();
        Self {
            exact: BTreeSet::new(),
            patterns,
        }
    }
    fn matches(&self, path: &Path, directory: bool) -> bool {
        if path == Path::new(".git")
            || path.starts_with(".git")
            || self
                .exact
                .iter()
                .any(|ignored| path == ignored || path.starts_with(ignored))
        {
            return true;
        }
        let candidate = path.to_string_lossy();
        self.patterns.iter().any(|rule| {
            let anchored = rule.starts_with('/');
            let directory_only = rule.ends_with('/');
            let rule = rule.trim_start_matches('/').trim_end_matches('/');
            if rule.is_empty() {
                return false;
            }
            let direct = if anchored || rule.contains('/') {
                candidate == rule || candidate.starts_with(&format!("{rule}/"))
            } else {
                path.components().any(|part| part.as_os_str() == rule)
            };
            let wildcard = rule.contains('*') && glob_match(rule, &candidate);
            (direct || wildcard) && (!directory_only || directory)
        })
    }
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

fn glob_match(pattern: &str, value: &str) -> bool {
    let mut expression = String::from("^");
    for character in pattern.chars() {
        match character {
            '*' => expression.push_str(".*"),
            '?' => expression.push('.'),
            other => expression.push_str(&regex::escape(&other.to_string())),
        }
    }
    expression.push('$');
    Regex::new(&expression).is_ok_and(|regex| regex.is_match(value))
}
