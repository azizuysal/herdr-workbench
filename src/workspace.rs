use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRoot {
    root: PathBuf,
    pub is_git_worktree: bool,
}

#[derive(Debug)]
pub enum WorkspaceError {
    CurrentDirectory(std::io::Error),
    Canonicalize {
        path: PathBuf,
        source: std::io::Error,
    },
    NotDirectory {
        path: PathBuf,
    },
    OutsideRoot {
        path: PathBuf,
    },
    AbsolutePath {
        path: PathBuf,
    },
    ParentTraversal {
        path: PathBuf,
    },
    GitOutput,
}
impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CurrentDirectory(e) => write!(f, "cannot determine workspace directory: {e}"),
            Self::Canonicalize { path, source } => {
                write!(f, "cannot resolve {}: {source}", path.display())
            }
            Self::NotDirectory { path } => {
                write!(f, "workspace path {} is not a directory", path.display())
            }
            Self::OutsideRoot { path } => write!(
                f,
                "path {} resolves outside the workspace root",
                path.display()
            ),
            Self::AbsolutePath { path } => {
                write!(f, "absolute path {} is not allowed", path.display())
            }
            Self::ParentTraversal { path } => {
                write!(f, "path {} contains parent traversal", path.display())
            }
            Self::GitOutput => write!(f, "git returned a non-UTF-8 workspace root"),
        }
    }
}
impl std::error::Error for WorkspaceError {}

impl WorkspaceRoot {
    pub fn resolve(cwd: &Path) -> Result<Self, WorkspaceError> {
        let cwd = canonical_directory(cwd)?;
        if let Some(root) = git_worktree_root(&cwd)? {
            return Ok(Self {
                root,
                is_git_worktree: true,
            });
        }
        Ok(Self {
            root: cwd,
            is_git_worktree: false,
        })
    }

    /// Creates an immutable workspace boundary at this exact directory.
    ///
    /// Unlike `resolve`, this never promotes the boundary to an ancestor Git
    /// worktree. This is used for paths received from an already-running pane.
    pub fn from_directory(directory: &Path) -> Result<Self, WorkspaceError> {
        let root = canonical_directory(directory)?;
        let is_git_worktree =
            git_worktree_root(&root)?.is_some_and(|worktree_root| worktree_root == root);
        Ok(Self {
            root,
            is_git_worktree,
        })
    }

    /// Follows a terminal directory without narrowing an existing workspace boundary.
    pub fn following(&self, cwd: &Path) -> Result<Self, WorkspaceError> {
        let cwd = canonical_directory(cwd)?;
        if cwd.starts_with(&self.root) {
            return Ok(self.clone());
        }
        Self::resolve(&cwd)
    }

    pub fn from_current_dir() -> Result<Self, WorkspaceError> {
        let cwd = std::env::current_dir().map_err(WorkspaceError::CurrentDirectory)?;
        Self::resolve(&cwd)
    }
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Promotes an immutable workspace root when that exact directory becomes a Git worktree.
    /// A repository created in an ancestor never expands the workspace security boundary.
    pub fn refresh_git_worktree(&mut self) -> Result<bool, WorkspaceError> {
        if self.is_git_worktree {
            return Ok(false);
        }
        let Some(root) = git_worktree_root(&self.root)? else {
            return Ok(false);
        };
        if root != self.root {
            return Ok(false);
        }
        self.is_git_worktree = true;
        Ok(true)
    }

    /// Resolves only a root-relative path. Existing symlinks must resolve within the immutable root.
    pub fn resolve_path(&self, relative: &Path) -> Result<PathBuf, WorkspaceError> {
        if relative.is_absolute() {
            return Err(WorkspaceError::AbsolutePath {
                path: relative.to_owned(),
            });
        }
        if relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(WorkspaceError::ParentTraversal {
                path: relative.to_owned(),
            });
        }
        let candidate = self.root.join(relative);
        if candidate.exists() {
            return self.ensure_inside(candidate.canonicalize().map_err(|source| {
                WorkspaceError::Canonicalize {
                    path: candidate,
                    source,
                }
            })?);
        }
        let mut existing = candidate.as_path();
        let mut suffix = Vec::new();
        while !existing.exists() {
            let name = existing
                .file_name()
                .ok_or_else(|| WorkspaceError::OutsideRoot {
                    path: candidate.clone(),
                })?;
            suffix.push(name.to_owned());
            existing = existing
                .parent()
                .ok_or_else(|| WorkspaceError::OutsideRoot {
                    path: candidate.clone(),
                })?;
        }
        let mut resolved = self.ensure_inside(existing.canonicalize().map_err(|source| {
            WorkspaceError::Canonicalize {
                path: existing.to_owned(),
                source,
            }
        })?)?;
        for part in suffix.iter().rev() {
            resolved.push(part);
        }
        Ok(resolved)
    }
    pub fn relative_path(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        let canonical = path
            .canonicalize()
            .map_err(|source| WorkspaceError::Canonicalize {
                path: path.to_owned(),
                source,
            })?;
        self.ensure_inside(canonical)?
            .strip_prefix(&self.root)
            .map(Path::to_owned)
            .map_err(|_| WorkspaceError::OutsideRoot {
                path: path.to_owned(),
            })
    }
    fn ensure_inside(&self, path: PathBuf) -> Result<PathBuf, WorkspaceError> {
        if path.starts_with(&self.root) {
            Ok(path)
        } else {
            Err(WorkspaceError::OutsideRoot { path })
        }
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, WorkspaceError> {
    let canonical = path
        .canonicalize()
        .map_err(|source| WorkspaceError::Canonicalize {
            path: path.to_owned(),
            source,
        })?;
    if canonical.is_dir() {
        Ok(canonical)
    } else {
        Err(WorkspaceError::NotDirectory { path: canonical })
    }
}

fn git_worktree_root(cwd: &Path) -> Result<Option<PathBuf>, WorkspaceError> {
    let output = match Command::new("git")
        .args([
            OsStr::new("-C"),
            cwd.as_os_str(),
            OsStr::new("rev-parse"),
            OsStr::new("--show-toplevel"),
        ])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(_) | Err(_) => return Ok(None),
    };
    let mut raw = output.stdout;
    while matches!(raw.last(), Some(b'\r' | b'\n')) {
        raw.pop();
    }
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(raw))
    };
    #[cfg(not(unix))]
    let path = PathBuf::from(String::from_utf8(raw).map_err(|_| WorkspaceError::GitOutput)?);
    let root = path
        .canonicalize()
        .map_err(|source| WorkspaceError::Canonicalize {
            path: path.clone(),
            source,
        })?;
    Ok(Some(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repository(path: &Path) {
        std::fs::create_dir_all(path).expect("create repository directory");
        let output = Command::new("git")
            .args(["init", "-q"])
            .current_dir(path)
            .output()
            .expect("run git");
        assert!(output.status.success());
    }

    #[test]
    fn from_directory_does_not_broaden_to_an_ancestor_worktree() {
        let repository = tempfile::tempdir().expect("temporary repository");
        init_repository(repository.path());
        let nested = repository.path().join("nested");
        std::fs::create_dir(&nested).expect("create nested directory");

        let workspace = WorkspaceRoot::from_directory(&nested).expect("exact workspace");

        assert_eq!(
            workspace.path(),
            nested.canonicalize().as_deref().expect("canonical")
        );
        assert!(!workspace.is_git_worktree);
    }

    #[test]
    fn following_a_subfolder_keeps_the_current_repository_root() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let repository = directory.path().join("repository-a");
        init_repository(&repository);
        let subfolder = repository.join("src");
        std::fs::create_dir(&subfolder).expect("create subfolder");
        let workspace = WorkspaceRoot::resolve(&repository).expect("repository workspace");

        let followed = workspace.following(&subfolder).expect("follow subfolder");

        assert_eq!(followed, workspace);
    }

    #[test]
    fn following_another_repository_switches_to_its_root() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let repository_a = directory.path().join("repository-a");
        let repository_b = directory.path().join("repository-b");
        init_repository(&repository_a);
        init_repository(&repository_b);
        let workspace = WorkspaceRoot::resolve(&repository_a).expect("repository workspace");

        let followed = workspace
            .following(&repository_b)
            .expect("follow repository");

        assert_eq!(
            followed.path(),
            repository_b.canonicalize().as_deref().expect("canonical")
        );
        assert!(followed.is_git_worktree);
    }

    #[test]
    fn following_a_linked_worktree_switches_to_its_root() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let repository = directory.path().join("repository");
        let linked_worktree = directory.path().join("linked-worktree");
        init_repository(&repository);
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Workbench Tests",
                "-c",
                "user.email=workbench-tests@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ])
            .current_dir(&repository)
            .output()
            .expect("create initial commit");
        assert!(output.status.success());
        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "linked-worktree",
                linked_worktree.to_str().expect("UTF-8 worktree path"),
            ])
            .current_dir(&repository)
            .output()
            .expect("create linked worktree");
        assert!(output.status.success());
        let workspace = WorkspaceRoot::resolve(&repository).expect("repository workspace");

        let followed = workspace
            .following(&linked_worktree)
            .expect("follow linked worktree");

        assert_eq!(
            followed.path(),
            linked_worktree
                .canonicalize()
                .as_deref()
                .expect("canonical")
        );
        assert!(followed.is_git_worktree);
    }

    #[test]
    fn following_a_child_repository_keeps_a_parent_directory_workspace() {
        let parent = tempfile::tempdir().expect("temporary parent");
        let child_repository = parent.path().join("child-repository");
        init_repository(&child_repository);
        let workspace = WorkspaceRoot::from_directory(parent.path()).expect("parent workspace");

        let followed = workspace
            .following(&child_repository)
            .expect("follow child repository");

        assert_eq!(followed, workspace);
        assert!(!followed.is_git_worktree);
    }

    #[test]
    fn following_outside_a_parent_directory_workspace_switches_to_a_repository() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let parent = directory.path().join("parent");
        let repository = directory.path().join("repository");
        std::fs::create_dir(&parent).expect("create parent");
        init_repository(&repository);
        let workspace = WorkspaceRoot::from_directory(&parent).expect("parent workspace");

        let followed = workspace.following(&repository).expect("follow repository");

        assert_eq!(
            followed.path(),
            repository.canonicalize().as_deref().expect("canonical")
        );
        assert!(followed.is_git_worktree);
    }

    #[test]
    fn following_a_non_git_directory_uses_that_directory() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir(&first).expect("create first directory");
        std::fs::create_dir(&second).expect("create second directory");
        let workspace = WorkspaceRoot::resolve(&first).expect("first workspace");

        let followed = workspace
            .following(&second)
            .expect("follow non-git directory");

        assert_eq!(
            followed.path(),
            second.canonicalize().as_deref().expect("canonical")
        );
        assert!(!followed.is_git_worktree);
    }

    #[test]
    fn non_directories_fail_without_changing_the_existing_workspace() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace = WorkspaceRoot::resolve(directory.path()).expect("workspace");
        let file = directory.path().join("file");
        std::fs::write(&file, "file").expect("write file");
        let missing = directory.path().join("missing");

        for path in [&file, &missing] {
            assert!(matches!(
                workspace.following(path),
                Err(WorkspaceError::NotDirectory { .. }) | Err(WorkspaceError::Canonicalize { .. })
            ));
            assert_eq!(
                workspace.path(),
                directory
                    .path()
                    .canonicalize()
                    .as_deref()
                    .expect("canonical")
            );
        }
        assert!(matches!(
            WorkspaceRoot::resolve(&file),
            Err(WorkspaceError::NotDirectory { .. })
        ));
        assert!(matches!(
            WorkspaceRoot::from_directory(&file),
            Err(WorkspaceError::NotDirectory { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn following_a_symlink_uses_its_canonical_repository_root() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let repository = directory.path().join("repository");
        init_repository(&repository);
        let link = directory.path().join("repository-link");
        symlink(&repository, &link).expect("create symlink");
        let outside = directory.path().join("outside");
        std::fs::create_dir(&outside).expect("create outside directory");
        let workspace = WorkspaceRoot::resolve(&outside).expect("outside workspace");

        let followed = workspace.following(&link).expect("follow symlink");

        assert_eq!(
            followed.path(),
            repository.canonicalize().as_deref().expect("canonical")
        );
        assert!(followed.is_git_worktree);
    }
}
