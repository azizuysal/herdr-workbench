use std::{
    error::Error,
    fmt,
    io::{self, Write},
    path::Path,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};

#[derive(Debug)]
pub struct ClipboardError(io::Error);

impl fmt::Display for ClipboardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "clipboard write failed: {}", self.0)
    }
}

impl Error for ClipboardError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

impl From<io::Error> for ClipboardError {
    fn from(error: io::Error) -> Self {
        Self(error)
    }
}

pub trait ClipboardWriter: Send + Sync {
    fn copy_path(&self, path: &Path) -> Result<(), ClipboardError>;
}

pub struct HerdrClipboard;

impl ClipboardWriter for HerdrClipboard {
    fn copy_path(&self, path: &Path) -> Result<(), ClipboardError> {
        let stdout = io::stdout();
        write_osc52(&mut stdout.lock(), path.as_os_str().as_encoded_bytes()).map_err(Into::into)
    }
}

pub fn copy_text(text: &str) -> Result<(), ClipboardError> {
    let stdout = io::stdout();
    write_osc52(&mut stdout.lock(), text.as_bytes()).map_err(Into::into)
}

fn write_osc52(output: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let encoded = STANDARD.encode(bytes);
    output.write_all(b"\x1b]52;c;")?;
    output.write_all(encoded.as_bytes())?;
    output.write_all(b"\x07")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_copy_uses_the_herdr_forwarded_bel_terminated_form() {
        let mut output = Vec::new();

        write_osc52(&mut output, b"docs/file name.md").expect("OSC 52 write");

        assert_eq!(output, b"\x1b]52;c;ZG9jcy9maWxlIG5hbWUubWQ=\x07");
    }

    #[test]
    fn osc52_copy_preserves_multiline_preview_text() {
        let mut output = Vec::new();

        write_osc52(&mut output, "first\nsecond".as_bytes()).expect("OSC 52 write");

        assert_eq!(output, b"\x1b]52;c;Zmlyc3QKc2Vjb25k\x07");
    }

    #[test]
    fn clipboard_write_errors_remain_actionable() {
        struct FailingWriter;

        impl Write for FailingWriter {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed terminal"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let error = write_osc52(&mut FailingWriter, b"src/main.rs").expect_err("write failure");

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(error.to_string(), "closed terminal");
    }
}
