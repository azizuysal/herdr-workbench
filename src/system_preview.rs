use std::{
    ffi::OsString,
    io,
    path::Path,
    process::{Command, Stdio},
    thread,
};

#[derive(Debug)]
pub enum SystemPreviewError {
    UnsupportedPlatform,
    Launch {
        program: OsString,
        source: io::Error,
    },
}

impl std::fmt::Display for SystemPreviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                write!(
                    formatter,
                    "the system visual preview is unsupported on this platform"
                )
            }
            Self::Launch { program, source } => {
                write!(
                    formatter,
                    "cannot launch system preview {program:?}: {source}"
                )
            }
        }
    }
}

impl std::error::Error for SystemPreviewError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Launch { source, .. } => Some(source),
            Self::UnsupportedPlatform => None,
        }
    }
}

pub fn open(path: &Path) -> Result<(), SystemPreviewError> {
    let command = command_for_current_platform(path)?;
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| SystemPreviewError::Launch {
            program: command.program,
            source,
        })?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct SystemPreviewCommand {
    program: OsString,
    args: Vec<OsString>,
}

#[cfg(target_os = "macos")]
fn command_for_current_platform(path: &Path) -> Result<SystemPreviewCommand, SystemPreviewError> {
    Ok(macos_command(path))
}

#[cfg(target_os = "linux")]
fn command_for_current_platform(path: &Path) -> Result<SystemPreviewCommand, SystemPreviewError> {
    Ok(linux_command(path))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn command_for_current_platform(_path: &Path) -> Result<SystemPreviewCommand, SystemPreviewError> {
    Err(SystemPreviewError::UnsupportedPlatform)
}

#[cfg(any(target_os = "macos", test))]
fn macos_command(path: &Path) -> SystemPreviewCommand {
    SystemPreviewCommand {
        program: OsString::from("/usr/bin/open"),
        args: vec![
            OsString::from("-n"),
            OsString::from("-b"),
            OsString::from("com.apple.quicklook.qlmanage"),
            OsString::from("--args"),
            OsString::from("-p"),
            path.as_os_str().to_owned(),
        ],
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_command(path: &Path) -> SystemPreviewCommand {
    SystemPreviewCommand {
        program: OsString::from("xdg-open"),
        args: vec![path.as_os_str().to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_activates_quick_look_through_launch_services_without_a_shell() {
        assert_eq!(
            macos_command(Path::new("/project/design.pdf")),
            SystemPreviewCommand {
                program: OsString::from("/usr/bin/open"),
                args: vec![
                    OsString::from("-n"),
                    OsString::from("-b"),
                    OsString::from("com.apple.quicklook.qlmanage"),
                    OsString::from("--args"),
                    OsString::from("-p"),
                    OsString::from("/project/design.pdf"),
                ],
            }
        );
    }

    #[test]
    fn linux_opens_the_exact_visual_in_the_system_viewer() {
        assert_eq!(
            linux_command(Path::new("/project/design.png")),
            SystemPreviewCommand {
                program: OsString::from("xdg-open"),
                args: vec![OsString::from("/project/design.png")],
            }
        );
    }
}
