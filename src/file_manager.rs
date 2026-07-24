use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

#[derive(Debug)]
pub enum FileManagerError {
    UnsupportedPlatform,
    Launch {
        program: OsString,
        source: io::Error,
    },
}

impl std::fmt::Display for FileManagerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                write!(
                    formatter,
                    "the system file manager is unsupported on this platform"
                )
            }
            Self::Launch { program, source } => write!(
                formatter,
                "cannot launch system file manager {:?}: {source}",
                program
            ),
        }
    }
}

impl std::error::Error for FileManagerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Launch { source, .. } => Some(source),
            Self::UnsupportedPlatform => None,
        }
    }
}

pub trait FileManagerOpener: Send + Sync {
    fn open(&self, path: &Path) -> Result<(), FileManagerError>;
}

#[derive(Debug, Default)]
pub struct SystemFileManagerOpener;

impl FileManagerOpener for SystemFileManagerOpener {
    fn open(&self, path: &Path) -> Result<(), FileManagerError> {
        let command = command_for_current_platform(path)?;
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| FileManagerError::Launch {
                program: command.program,
                source,
            })?;
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FileManagerCommand {
    program: OsString,
    args: Vec<OsString>,
}

#[cfg(target_os = "macos")]
fn command_for_current_platform(path: &Path) -> Result<FileManagerCommand, FileManagerError> {
    Ok(macos_command(path))
}

#[cfg(target_os = "linux")]
fn command_for_current_platform(path: &Path) -> Result<FileManagerCommand, FileManagerError> {
    Ok(linux_command(path))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn command_for_current_platform(_path: &Path) -> Result<FileManagerCommand, FileManagerError> {
    Err(FileManagerError::UnsupportedPlatform)
}

#[cfg(any(target_os = "macos", test))]
fn macos_command(path: &Path) -> FileManagerCommand {
    if path.exists() {
        FileManagerCommand {
            program: OsString::from("/usr/bin/open"),
            args: vec![OsString::from("-R"), path.as_os_str().to_owned()],
        }
    } else {
        FileManagerCommand {
            program: OsString::from("/usr/bin/open"),
            args: vec![path.parent().unwrap_or(path).as_os_str().to_owned()],
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_command(path: &Path) -> FileManagerCommand {
    FileManagerCommand {
        program: OsString::from("xdg-open"),
        args: vec![linux_directory(path).as_os_str().to_owned()],
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_directory(path: &Path) -> &Path {
    if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_reveals_the_exact_selected_path() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let file = directory.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").expect("fixture file");

        assert_eq!(
            macos_command(&file),
            FileManagerCommand {
                program: OsString::from("/usr/bin/open"),
                args: vec![OsString::from("-R"), file.as_os_str().to_owned()],
            }
        );

        assert_eq!(
            macos_command(Path::new("/project/deleted.rs")),
            FileManagerCommand {
                program: OsString::from("/usr/bin/open"),
                args: vec![OsString::from("/project")],
            }
        );
    }

    #[test]
    fn linux_opens_a_directory_or_the_selected_files_parent() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let file = directory.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").expect("fixture file");

        assert_eq!(
            linux_command(&file),
            FileManagerCommand {
                program: OsString::from("xdg-open"),
                args: vec![directory.path().as_os_str().to_owned()],
            }
        );
        assert_eq!(
            linux_command(directory.path()),
            FileManagerCommand {
                program: OsString::from("xdg-open"),
                args: vec![directory.path().as_os_str().to_owned()],
            }
        );
    }
}
