//! tex-fmt only handles explicit stdin/stdout formatting, never user-file writes.
use super::{Event, Request, Snapshot};
use crate::{private_workspace::PrivateWorkspace, process::OwnedChild};
use std::{
    fs::{self, File},
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tiptoptyp_core::text::{LspRange, LspTextEdit, ScalarOffset, lsp_position_at_scalar};
const MAX_OUTPUT: u64 = 32 * 1024 * 1024;
pub(super) struct Job {
    child: OwnedChild,
    _directory: tempfile::TempDir,
    output: std::path::PathBuf,
    error: std::path::PathBuf,
    source: std::sync::Arc<str>,
    request: Request,
    started: Instant,
}
impl Job {
    pub(super) fn start(snapshot: &Snapshot, request: Request) -> Result<Self, String> {
        let run = || -> std::io::Result<Self> {
            let directory = PrivateWorkspace::open(&snapshot.root)?.temp_dir("tex-format-")?;
            let input = directory.path().join("stdin.tex");
            fs::write(&input, snapshot.source.as_bytes())?;
            let output = directory.path().join("stdout.tex");
            let error = directory.path().join("stderr.log");
            let path = url::Url::parse(&snapshot.uri)
                .ok()
                .and_then(|u| u.to_file_path().ok());
            let cwd = path
                .as_deref()
                .and_then(|p| p.parent())
                .filter(|p| p.is_dir())
                .unwrap_or(&snapshot.root);
            let mut command = Command::new(&snapshot.tools.tex_fmt.program);
            command.args(["--stdin", "--quiet"]).current_dir(cwd);
            snapshot.tools.tex_fmt.command.apply(&mut command)?;
            command
                .stdin(File::open(input)?)
                .stdout(File::create(&output)?)
                .stderr(Stdio::from(File::create(&error)?));
            let child = OwnedChild::spawn(&mut command)?;
            Ok(Self {
                child,
                _directory: directory,
                output,
                error,
                source: snapshot.source.clone(),
                request,
                started: Instant::now(),
            })
        };
        run().map_err(|e| format!("Could not start tex-fmt: {e}"))
    }
    pub(super) fn poll(&mut self) -> Option<Event> {
        let result = match self.child.try_wait() {
            Ok(None)
                if self.started.elapsed() < Duration::from_secs(30)
                    && [&self.output, &self.error]
                        .iter()
                        .all(|p| fs::metadata(p).is_ok_and(|m| m.len() <= MAX_OUTPUT)) =>
            {
                return None;
            }
            Ok(None) => Err("tex-fmt exceeded its time or output limit".to_owned()),
            Err(error) => Err(error.to_string()),
            Ok(Some(status)) if status.success() => {
                read(&self.output).map(|text| minimal_edits(&self.source, &text))
            }
            Ok(Some(status)) => Err(format!(
                "tex-fmt exited with {status}: {}",
                read(&self.error).unwrap_or_default()
            )),
        };
        Some(match result {
            Ok(edits) => Event::Formatted {
                request: self.request.clone(),
                edits: Some(edits),
            },
            Err(message) => Event::RequestFailed {
                request: self.request.clone(),
                message,
            },
        })
    }
}
fn read(path: &std::path::Path) -> Result<String, String> {
    let mut text = String::new();
    File::open(path)
        .and_then(|f| f.take(MAX_OUTPUT + 1).read_to_string(&mut text))
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_OUTPUT {
        return Err("tex-fmt output is too large".into());
    }
    Ok(text)
}

/// Preserve the unchanged prefix/suffix instead of moving every caret to EOF
/// when a CLI formatter returns the whole buffer. Work and allocation are linear.
pub(super) fn minimal_edits(source: &str, formatted: &str) -> Vec<LspTextEdit> {
    if source == formatted {
        return Vec::new();
    }
    let prefix = source
        .chars()
        .zip(formatted.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let source_len = source.chars().count();
    let formatted_len = formatted.chars().count();
    let suffix = source
        .chars()
        .rev()
        .zip(formatted.chars().rev())
        .take((source_len - prefix).min(formatted_len - prefix))
        .take_while(|(a, b)| a == b)
        .count();
    let new_text = formatted
        .chars()
        .skip(prefix)
        .take(formatted_len - prefix - suffix)
        .collect();
    vec![LspTextEdit {
        range: LspRange {
            start: lsp_position_at_scalar(source, ScalarOffset::new(prefix)),
            end: lsp_position_at_scalar(source, ScalarOffset::new(source_len - suffix)),
        },
        new_text,
    }]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_output_preserves_unicode_and_cursors_outside_the_changed_span() {
        use tiptoptyp_core::text::apply_text_edits;
        for (source, output) in [
            ("é😀 left   right\nlast", "é😀 left right\nlast"),
            ("", "é😀"),
            ("abc", ""),
            ("same", "same"),
        ] {
            let edits = minimal_edits(source, output);
            let result = apply_text_edits(
                source,
                &edits,
                [
                    ScalarOffset::new(0),
                    ScalarOffset::new(source.chars().count()),
                ],
            )
            .unwrap();
            assert_eq!(result.text, output);
            assert_eq!(result.mapped_offsets[1].get(), output.chars().count());
        }
    }
}
