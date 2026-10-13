//! Isolated, cancellable LaTeXdiff jobs. Never writes either input document.
use crate::tool_command::CommandCustomization;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) const DOCS: &str = "https://mirrors.ctan.org/support/latexdiff/doc/latexdiff-man.pdf";
const MAX_SOURCE: u64 = 16 * 1024 * 1024;
const MAX_LOG: u64 = 16 * 1024;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Markup {
    #[default]
    Underline,
    Color,
    Traditional,
}
impl Markup {
    pub(crate) const ALL: [Self; 3] = [Self::Underline, Self::Color, Self::Traditional];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Underline => "Underline",
            Self::Color => "Colour",
            Self::Traditional => "Traditional",
        }
    }
    fn argument(self) -> &'static str {
        match self {
            Self::Underline => "UNDERLINE",
            Self::Color => "COLOR",
            Self::Traditional => "TRADITIONAL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    /// Blank discovers PATH or MacTeX, without running a program on the UI thread.
    pub(crate) executable: String,
    pub(crate) flatten: bool,
    pub(crate) markup: Markup,
    pub(crate) command: CommandCustomization,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            executable: String::new(),
            flatten: true,
            markup: Markup::default(),
            command: CommandCustomization::default(),
        }
    }
}
impl Settings {
    pub(crate) fn validate(&self) -> Result<(), String> {
        self.command.validate()?;
        if self.executable.contains('\0') {
            return Err("LaTeXdiff executable contains NUL".into());
        }
        Ok(())
    }
    fn program(&self) -> PathBuf {
        if self.executable.trim().is_empty() {
            super::tools::distribution_tool("latexdiff").unwrap_or_else(|| "latexdiff".into())
        } else {
            self.executable.trim().into()
        }
    }
}

#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) older: PathBuf,
    pub(crate) newer: PathBuf,
    /// Snapshot of the editor, only when it is the selected newer file.
    pub(crate) source: Option<String>,
    pub(crate) settings: Settings,
}
#[derive(Debug, Clone)]
pub(crate) struct Output {
    pub(crate) source: String,
    pub(crate) warnings: String,
    pub(crate) older: PathBuf,
    pub(crate) newer: PathBuf,
}
impl Output {
    pub(crate) fn suggested_name(&self) -> String {
        format!(
            "{}-diff.tex",
            self.newer.file_stem().unwrap_or_default().to_string_lossy()
        )
    }
    pub(crate) fn save(&self, path: &Path) -> Result<PathBuf, String> {
        // Canonicalize existing destinations too: aliases cannot overwrite an input.
        let destination = std::path::absolute(path).map_err(|e| e.to_string())?;
        let resolved = destination.canonicalize().unwrap_or(destination.clone());
        for original in [&self.older, &self.newer] {
            if original.canonicalize().unwrap_or(original.clone()) == resolved {
                return Err("Choose a different output file; a diff cannot overwrite either original document.".into());
            }
        }
        let mut file = tempfile::NamedTempFile::new_in(
            destination.parent().ok_or("Choose an output directory")?,
        )
        .map_err(|e| format!("Cannot create diff: {e}"))?;
        file.write_all(self.source.as_bytes())
            .map_err(|e| e.to_string())?;
        file.persist(&destination)
            .map_err(|e| format!("Cannot save diff: {e}"))?;
        Ok(destination)
    }
}

pub(crate) fn generate(request: Request, cancel: &AtomicBool) -> Result<Output, String> {
    generate_with_timeout(request, cancel, Duration::from_secs(120))
}
fn generate_with_timeout(
    request: Request,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<Output, String> {
    request.settings.validate()?;
    let older = validate_input(&request.older)?;
    let newer = validate_input(&request.newer)?;
    if cancel.load(Ordering::Acquire) {
        return Err("LaTeXdiff canceled".into());
    }
    // Use the original directory so --flatten resolves relative \input/include.
    // A uniquely named snapshot is removed on every success/error/cancel path.
    let snapshot = if let Some(source) = request.source {
        if source.len() as u64 > MAX_SOURCE {
            return Err("The document exceeds LaTeXdiff's 16 MiB input limit".into());
        }
        let mut file = tempfile::Builder::new()
            .prefix(".tiptoptyp-latexdiff-")
            .suffix(".tex")
            .tempfile_in(
                newer
                    .parent()
                    .ok_or("The newer file needs a parent directory")?,
            )
            .map_err(|e| format!("Cannot snapshot unsaved edits: {e}"))?;
        file.write_all(source.as_bytes())
            .map_err(|e| e.to_string())?;
        Some(file)
    } else {
        None
    };
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let stdout_path = directory.path().join("diff.tex");
    let stderr_path = directory.path().join("latexdiff.log");
    let mut command = Command::new(request.settings.program());
    command.current_dir(newer.parent().unwrap());
    request
        .settings
        .command
        .apply(&mut command)
        .map_err(|e| e.to_string())?;
    command.arg(format!("--type={}", request.settings.markup.argument()));
    if request.settings.flatten {
        command.arg("--flatten");
    }
    command.args(["--encoding=utf8", "--label"]);
    command.arg(older.as_os_str());
    command.arg("--label").arg(newer.as_os_str());
    command
        .arg("--")
        .arg(&older)
        .arg(snapshot.as_ref().map_or(newer.as_path(), |f| f.path()));
    command
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&stdout_path).map_err(|e| e.to_string())?)
        .stderr(std::fs::File::create(&stderr_path).map_err(|e| e.to_string())?);
    let mut child = crate::process::OwnedChild::spawn(&mut command).map_err(|e| format!("Cannot start LaTeXdiff: {e}. Install LaTeXdiff (for example with TeX Live/MacTeX), or set its executable path."))?;
    let start = Instant::now();
    let status = loop {
        let stopped = if cancel.load(Ordering::Acquire) {
            Some("LaTeXdiff canceled")
        } else if start.elapsed() >= timeout {
            Some("LaTeXdiff timed out after 120 seconds")
        } else if [(&stdout_path, MAX_SOURCE), (&stderr_path, MAX_SOURCE)]
            .iter()
            .any(|(p, max)| std::fs::metadata(p).is_ok_and(|m| m.len() > *max))
        {
            Some("LaTeXdiff output exceeds the 16 MiB limit")
        } else {
            None
        };
        if let Some(message) = stopped {
            return Err(message.into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                return Err(format!("Cannot wait for LaTeXdiff: {e}"));
            }
        }
    };
    let warnings = read_bounded(&stderr_path, MAX_LOG)?;
    if !status.success() {
        return Err(format!("LaTeXdiff failed ({status}).\n{warnings}"));
    }
    let source = read_bounded(&stdout_path, MAX_SOURCE)?;
    if source.trim().is_empty() {
        return Err(
            "LaTeXdiff produced no source. Check the input documents and arguments.".into(),
        );
    }
    Ok(Output {
        source,
        warnings,
        older,
        newer,
    })
}
fn validate_input(path: &Path) -> Result<PathBuf, String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let meta = path.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if meta.len() > MAX_SOURCE {
        return Err(format!(
            "{} exceeds LaTeXdiff's 16 MiB input limit",
            path.display()
        ));
    }
    Ok(path)
}
fn read_bounded(path: &Path, limit: u64) -> Result<String, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        if limit == MAX_SOURCE {
            return Err("LaTeXdiff output exceeds the 16 MiB limit".into());
        }
        bytes.truncate(limit as usize);
        return Ok(format!(
            "{}\n[Log truncated]",
            String::from_utf8_lossy(&bytes)
        ));
    }
    String::from_utf8(bytes).map_err(|e| format!("LaTeXdiff output is not UTF-8: {e}"))
}

/// Closing the dialog cancels its process; no stale completion can open a tab.
pub(crate) struct Cancellation(pub(crate) Arc<AtomicBool>);
impl Default for Cancellation {
    fn default() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inputs(root: &Path) -> Request {
        let older = root.join("old source.tex");
        let newer = root.join("new source.tex");
        std::fs::write(&older, "old disk").unwrap();
        std::fs::write(&newer, "new disk").unwrap();
        Request {
            older,
            newer,
            source: Some("unsaved new".into()),
            settings: Settings::default(),
        }
    }
    #[cfg(unix)]
    fn fake(root: &Path, script: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("fake latexdiff");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path.to_string_lossy().into_owned()
    }
    #[test]
    fn defaults_roundtrip_and_impossible_markup_rejected() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.flatten);
        assert_eq!(settings.markup, Markup::Underline);
        assert_eq!(
            serde_json::from_str::<Settings>(&serde_json::to_string(&settings).unwrap()).unwrap(),
            settings
        );
        let error = serde_json::from_str::<Settings>(r#"{"markup":"sparkles"}"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains("underline") && error.contains("sparkles"));
        assert!(serde_json::from_str::<Settings>(r#"{"unknown":1}"#).is_err());
    }
    #[test]
    fn invalid_options_are_reported_before_launch() {
        let root = tempfile::tempdir().unwrap();
        let mut request = inputs(root.path());
        request.settings.command.arguments = "'unclosed".into();
        assert!(
            generate(request, &AtomicBool::new(false))
                .unwrap_err()
                .contains("unmatched quote")
        );
        let mut request = inputs(root.path());
        request.settings.command.environment = r#"{"x":3}"#.into();
        assert!(
            generate(request, &AtomicBool::new(false))
                .unwrap_err()
                .contains("JSON object of strings")
        );
    }
    #[cfg(unix)]
    #[test]
    fn literal_arguments_environment_snapshot_and_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let mut request = inputs(root.path());
        request.settings.executable = fake(
            root.path(),
            r#"
printf '%s\n' "$@"
printf '%s\n' "$TEST_DIFF"
for last do :; done
cat "$last"
printf 'a warning' >&2
"#,
        );
        request.settings.command.arguments = "--math-markup=1 '$(touch should-not-exist)'".into();
        request.settings.command.environment = r#"{"TEST_DIFF":"literal env"}"#.into();
        let old = request.older.clone();
        let new = request.newer.clone();
        let output = generate(request, &AtomicBool::new(false)).unwrap();
        assert!(output.source.starts_with("--math-markup=1\n$(touch should-not-exist)\n--type=UNDERLINE\n--flatten\n--encoding=utf8\n"));
        assert!(output.source.contains("literal env\nunsaved new"));
        assert_eq!(output.warnings, "a warning");
        assert!(!root.path().join("should-not-exist").exists());
        assert_eq!(std::fs::read_to_string(old).unwrap(), "old disk");
        assert_eq!(std::fs::read_to_string(new).unwrap(), "new disk");
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tiptoptyp-latexdiff-")
        }));
    }
    #[cfg(unix)]
    #[test]
    fn failure_empty_output_and_timeout_cleanup() {
        let root = tempfile::tempdir().unwrap();
        for (script, expected) in [
            ("printf 'bad option' >&2; exit 2", "bad option"),
            ("exit 0", "produced no source"),
            ("exec sleep 10", "timed out"),
        ] {
            let mut request = inputs(root.path());
            request.settings.executable = fake(root.path(), script);
            let error = generate_with_timeout(
                request,
                &AtomicBool::new(false),
                if script == "exec sleep 10" {
                    Duration::ZERO
                } else {
                    Duration::from_secs(30)
                },
            )
            .unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".tiptoptyp-latexdiff-")
            }));
        }
    }
    #[cfg(unix)]
    #[test]
    fn cancel_running_process_and_drop_token() {
        let root = tempfile::tempdir().unwrap();
        let mut request = inputs(root.path());
        request.settings.executable = fake(root.path(), "printf started > ready; exec sleep 30");
        let cancellation = Cancellation::default();
        let flag = cancellation.0.clone();
        let worker = std::thread::spawn(move || generate(request, &flag));
        // Wait for the child itself to acknowledge startup, so this covers
        // cancellation of a running process rather than the pre-launch check.
        let ready = root.path().join("ready");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !ready.exists() && !worker.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let started = ready.exists();
        drop(cancellation);
        let result = worker.join().unwrap();
        assert!(started, "child did not start: {result:?}");
        assert!(result.unwrap_err().contains("canceled"));
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tiptoptyp-latexdiff-")
        }));
    }
    #[test]
    fn save_preserves_originals_and_aliases() {
        let root = tempfile::tempdir().unwrap();
        let request = inputs(root.path());
        let output = Output {
            source: "marked up".into(),
            warnings: String::new(),
            older: request.older,
            newer: request.newer,
        };
        for path in [&output.older, &output.newer] {
            assert!(output.save(path).unwrap_err().contains("cannot overwrite"));
        }
        #[cfg(unix)]
        {
            let alias = root.path().join("alias.tex");
            std::os::unix::fs::symlink(&output.newer, &alias).unwrap();
            assert!(output.save(&alias).is_err());
        }
        let path = root.path().join(output.suggested_name());
        assert_eq!(output.save(&path).unwrap(), path);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "marked up");
        assert_eq!(std::fs::read_to_string(output.newer).unwrap(), "new disk");
    }
    #[test]
    fn rejects_empty_directory_and_large_inputs() {
        let root = tempfile::tempdir().unwrap();
        let mut request = inputs(root.path());
        request.older = root.path().into();
        assert!(
            generate(request, &AtomicBool::new(false))
                .unwrap_err()
                .contains("not a file")
        );
        let mut request = inputs(root.path());
        std::fs::OpenOptions::new()
            .write(true)
            .open(&request.older)
            .unwrap()
            .set_len(MAX_SOURCE + 1)
            .unwrap();
        request.source = None;
        assert!(
            generate(request, &AtomicBool::new(false))
                .unwrap_err()
                .contains("input limit")
        );
    }
    #[test]
    fn bounded_logs_and_output() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), vec![b'x'; MAX_LOG as usize + 1]).unwrap();
        assert!(
            read_bounded(file.path(), MAX_LOG)
                .unwrap()
                .ends_with("[Log truncated]")
        );
        file.as_file().set_len(MAX_SOURCE + 1).unwrap();
        assert!(
            read_bounded(file.path(), MAX_SOURCE)
                .unwrap_err()
                .contains("output exceeds")
        );
    }
    #[test]
    #[ignore = "requires installed LaTeXdiff; run for integration verification"]
    fn installed_latexdiff_expands_subfiles_and_unsaved_source() {
        let root = tempfile::tempdir().unwrap();
        let mut request = inputs(root.path());
        let old_root = root.path().join("old");
        let new_root = root.path().join("new");
        std::fs::create_dir(&old_root).unwrap();
        std::fs::create_dir(&new_root).unwrap();
        request.older = old_root.join("main.tex");
        request.newer = new_root.join("main.tex");
        let source =
            "\\documentclass{article}\n\\begin{document}\n\\input{body}\n\\end{document}\n";
        std::fs::write(&request.older, source).unwrap();
        std::fs::write(&request.newer, source).unwrap();
        std::fs::write(old_root.join("body.tex"), "An old theorem.").unwrap();
        std::fs::write(new_root.join("body.tex"), "A new theorem.").unwrap();
        request.source =
            Some(source.replace("\\input{body}", "An unsaved sentence.\n\\input{body}"));
        request.settings.command.environment = r#"{"LC_ALL":"C"}"#.into();
        let output = generate(request, &AtomicBool::new(false)).unwrap();
        assert!(
            output.source.contains("\\DIFadd") && output.source.contains("\\DIFdel"),
            "{}",
            output.source
        );
        assert!(output.source.contains("unsaved") && output.source.contains("theorem"));
        assert!(!output.source.contains("\\input{body}"));
        let path = output.save(&new_root.join("diff.tex")).unwrap();
        assert!(path.is_file());
        let engine = super::super::tools::distribution_tool("pdflatex")
            .expect("installed pdfLaTeX for integration check");
        let mut command = Command::new(engine);
        command
            .current_dir(&new_root)
            .args([
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-no-shell-escape",
            ])
            .arg(&path);
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let status =
            crate::process::wait(command.spawn().unwrap(), Duration::from_secs(30), || Ok(()))
                .unwrap();
        assert!(
            status.success(),
            "{}",
            std::fs::read_to_string(new_root.join("diff.log")).unwrap_or_default()
        );
        assert!(
            std::fs::read(new_root.join("diff.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
    }
}
