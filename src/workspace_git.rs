//! Read-only Git snapshots for workspaces containing multiple repositories.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use crate::git::{GitCommit, GitError, GitSnapshot, GitStatusProvider, aggregate_directories};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceGitSnapshot {
    /// Per-repository status, ordered by repository path within the workspace.
    /// Each snapshot keeps its canonical repository root and repository-relative paths.
    pub repositories: Vec<GitSnapshot>,
    /// A workspace-rooted view of every repository entry. It intentionally has
    /// no branch because a workspace can contain several independent branches.
    pub combined: GitSnapshot,
    /// Failures scoped to a repository. A previous valid snapshot is retained
    /// for a repository when its refresh fails.
    pub errors: Vec<GitError>,
}

impl WorkspaceGitSnapshot {
    fn empty(root: PathBuf) -> Self {
        Self {
            repositories: Vec::new(),
            combined: GitSnapshot {
                root,
                ..GitSnapshot::default()
            },
            errors: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryCommit {
    /// Repository path relative to the workspace root. The root repository is
    /// represented by an empty path.
    pub repository: PathBuf,
    pub commit: GitCommit,
}

#[derive(Clone)]
pub struct WorkspaceGitProvider {
    root: PathBuf,
    last_valid: Arc<Mutex<BTreeMap<PathBuf, GitSnapshot>>>,
}

impl WorkspaceGitProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            last_valid: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn refresh(&self) -> Result<WorkspaceGitSnapshot, GitError> {
        let discovery = discover_repositories(&self.root)?;
        let mut last_valid = self
            .last_valid
            .lock()
            .expect("workspace Git snapshot mutex poisoned");
        let discovered: BTreeSet<_> = discovery.repositories.iter().cloned().collect();
        last_valid.retain(|root, _| discovered.contains(root));

        let mut snapshot = WorkspaceGitSnapshot::empty(discovery.workspace.clone());
        let invalid: BTreeMap<_, _> = discovery
            .errors
            .into_iter()
            .map(|error| (error.repository, error.error))
            .collect();
        snapshot.errors.extend(
            invalid
                .iter()
                .map(|(repository, error)| repository_error(repository, error.clone())),
        );
        for repository in discovery.repositories {
            if invalid.contains_key(&repository) {
                if let Some(previous) = last_valid.get(&repository) {
                    snapshot.repositories.push(previous.clone());
                }
                continue;
            }
            match GitStatusProvider::new(&repository).refresh() {
                Ok(repository_snapshot) => {
                    last_valid.insert(repository.clone(), repository_snapshot.clone());
                    snapshot.repositories.push(repository_snapshot);
                }
                Err(error) => {
                    snapshot.errors.push(repository_error(&repository, error));
                    if let Some(previous) = last_valid.get(&repository) {
                        snapshot.repositories.push(previous.clone());
                    }
                }
            }
        }
        snapshot.combined = combine_snapshots(&discovery.workspace, &snapshot.repositories);
        Ok(snapshot)
    }

    pub fn refresh_async(&self) -> mpsc::Receiver<Result<WorkspaceGitSnapshot, GitError>> {
        let provider = self.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(provider.refresh());
        });
        receiver
    }

    pub fn history(&self) -> Result<Vec<RepositoryCommit>, GitError> {
        let discovery = discover_repositories(&self.root)?;
        let mut commits = Vec::new();
        let mut errors = discovery
            .errors
            .into_iter()
            .map(|error| repository_error(&error.repository, error.error))
            .collect::<Vec<_>>();
        for repository in discovery.repositories {
            let relative = repository
                .strip_prefix(&discovery.workspace)
                .map_err(|_| GitError {
                    command: "workspace git history".into(),
                    message: format!("repository {} escapes workspace root", repository.display()),
                })?
                .to_path_buf();
            match GitStatusProvider::new(&repository).history() {
                Ok(history) => commits.extend(history.into_iter().map(|commit| RepositoryCommit {
                    repository: relative.clone(),
                    commit,
                })),
                Err(error) => errors.push(repository_error(&repository, error)),
            }
        }
        if errors.is_empty() {
            Ok(commits)
        } else {
            Err(combine_errors("workspace git history", errors))
        }
    }

    pub fn history_async(&self) -> mpsc::Receiver<Result<Vec<RepositoryCommit>, GitError>> {
        let provider = self.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(provider.history());
        });
        receiver
    }
}

fn combine_snapshots(workspace: &Path, repositories: &[GitSnapshot]) -> GitSnapshot {
    let mut combined = GitSnapshot {
        root: workspace.to_path_buf(),
        ..GitSnapshot::default()
    };
    for repository in repositories {
        let prefix = repository
            .root
            .strip_prefix(workspace)
            .expect("discovered repository is inside workspace");
        combined
            .entries
            .extend(repository.entries.iter().map(|entry| {
                let mut entry = entry.clone();
                entry.path = prefix.join(&entry.path);
                entry.rename_origin = entry.rename_origin.as_ref().map(|path| prefix.join(path));
                entry
            }));
    }
    combined.directories = aggregate_directories(&combined.entries);
    combined
}

struct Discovery {
    workspace: PathBuf,
    repositories: Vec<PathBuf>,
    errors: Vec<DiscoveryError>,
}

struct DiscoveryError {
    repository: PathBuf,
    error: GitError,
}

fn discover_repositories(workspace: &Path) -> Result<Discovery, GitError> {
    let workspace = workspace.canonicalize().map_err(|error| GitError {
        command: "discover Git repositories".into(),
        message: format!("cannot resolve workspace {}: {error}", workspace.display()),
    })?;
    let mut repositories = Vec::new();
    let mut errors = Vec::new();
    discover_directory(&workspace, &workspace, &mut repositories, &mut errors);
    repositories.sort();
    Ok(Discovery {
        workspace,
        repositories,
        errors,
    })
}

fn discover_directory(
    workspace: &Path,
    directory: &Path,
    repositories: &mut Vec<PathBuf>,
    errors: &mut Vec<DiscoveryError>,
) {
    match candidate_marker(directory) {
        Ok(Some(())) => match is_exact_worktree(directory) {
            Ok(()) => repositories.push(directory.to_path_buf()),
            Err(error) => {
                repositories.push(directory.to_path_buf());
                errors.push(DiscoveryError {
                    repository: directory.to_path_buf(),
                    error,
                });
            }
        },
        Ok(None) => {}
        Err(error) => {
            errors.push(DiscoveryError {
                repository: directory.to_path_buf(),
                error,
            });
            return;
        }
    }
    if repositories.last().is_some_and(|root| root == directory)
        || errors
            .last()
            .is_some_and(|error| error.repository == directory)
    {
        return;
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(DiscoveryError {
                repository: directory.to_path_buf(),
                error: discovery_error("cannot read", directory, error),
            });
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(DiscoveryError {
                    repository: directory.to_path_buf(),
                    error: discovery_error("cannot read", directory, error),
                });
                continue;
            }
        };
        if entry.file_name() == ".git" {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                errors.push(DiscoveryError {
                    repository: entry.path(),
                    error: discovery_error("cannot inspect", &entry.path(), error),
                });
                continue;
            }
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let child = entry.path();
        let canonical = match child.canonicalize() {
            Ok(canonical) => canonical,
            Err(error) => {
                errors.push(DiscoveryError {
                    repository: child.clone(),
                    error: discovery_error("cannot resolve", &child, error),
                });
                continue;
            }
        };
        if !canonical.starts_with(workspace) {
            continue;
        }
        discover_directory(workspace, &canonical, repositories, errors);
    }
}

fn candidate_marker(directory: &Path) -> Result<Option<()>, GitError> {
    let marker = directory.join(".git");
    match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(GitError {
            command: "discover Git repositories".into(),
            message: format!("unsupported symbolic link {}", marker.display()),
        }),
        Ok(metadata) if metadata.is_dir() || metadata.is_file() => Ok(Some(())),
        Ok(_) => Err(GitError {
            command: "discover Git repositories".into(),
            message: format!("unsupported Git marker {}", marker.display()),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(discovery_error("cannot inspect", &marker, error)),
    }
}

fn is_exact_worktree(directory: &Path) -> Result<(), GitError> {
    let output = Command::new("git")
        .args(["--no-optional-locks", "-C"])
        .arg(directory)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| GitError {
            command: "git rev-parse --show-toplevel".into(),
            message: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(GitError {
            command: "git rev-parse --show-toplevel".into(),
            message: sanitize_output(&output.stderr),
        });
    }
    let root = path_from_git_output(&output.stdout).ok_or_else(|| GitError {
        command: "git rev-parse --show-toplevel".into(),
        message: format!("invalid worktree root from {}", directory.display()),
    })?;
    let root = root.canonicalize().map_err(|error| GitError {
        command: "git rev-parse --show-toplevel".into(),
        message: format!("cannot resolve {}: {error}", root.display()),
    })?;
    if root == directory {
        Ok(())
    } else {
        Err(GitError {
            command: "git rev-parse --show-toplevel".into(),
            message: format!(
                "Git reports {} as the worktree root instead of {}",
                root.display(),
                directory.display()
            ),
        })
    }
}

fn discovery_error(action: &str, path: &Path, error: std::io::Error) -> GitError {
    GitError {
        command: "discover Git repositories".into(),
        message: format!("{action} {}: {error}", path.display()),
    }
}

fn sanitize_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

fn path_from_git_output(output: &[u8]) -> Option<PathBuf> {
    let output = output.strip_suffix(b"\n").unwrap_or(output);
    let output = output.strip_suffix(b"\r").unwrap_or(output);
    if output.is_empty() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(OsString::from_vec(output.to_vec())))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(output.to_vec()).ok().map(PathBuf::from)
    }
}

fn repository_error(repository: &Path, error: GitError) -> GitError {
    GitError {
        command: format!("{}: {}", repository.display(), error.command),
        message: error.message,
    }
}

fn combine_errors(command: &str, errors: Vec<GitError>) -> GitError {
    GitError {
        command: command.into(),
        message: errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; "),
    }
}
