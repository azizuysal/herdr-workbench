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
        let cwd = cwd
            .canonicalize()
            .map_err(|source| WorkspaceError::Canonicalize {
                path: cwd.to_owned(),
                source,
            })?;
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
    pub fn from_current_dir() -> Result<Self, WorkspaceError> {
        let cwd = std::env::current_dir().map_err(WorkspaceError::CurrentDirectory)?;
        Self::resolve(&cwd)
    }
    pub fn path(&self) -> &Path {
        &self.root
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
