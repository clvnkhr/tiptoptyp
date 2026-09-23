//! Worker-side Git execution and immutable results. No UI state or rendering.
use std::{
    env,
    ffi::OsString,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::Duration,
};
pub(crate) mod diff;
pub(crate) mod hunks;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) path: PathBuf,
    pub(super) index: char,
    pub(super) worktree: char,
}

impl Entry {
    pub(super) fn staged(&self) -> bool {
        self.index != ' ' && self.index != '?'
    }
    pub(super) fn unstaged(&self) -> bool {
        self.worktree != ' '
    }
    pub(super) fn stageable(&self) -> bool {
        self.unstaged() && !private_artifact(&self.path)
    }
    pub(super) fn revertible(&self) -> bool {
        ((matches!(self.index, ' ' | 'M' | 'A' | 'T') && matches!(self.worktree, 'M' | 'D' | 'T'))
            || (self.index == '?' && self.worktree == '?'))
            && !private_artifact(&self.path)
    }
}

/// Immutable status rows and their summary, prepared once by the Git worker.
/// Rendering cannot mutate the rows without recomputing the summary.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ChangeList {
    pub(super) items: Vec<Entry>,
    pub(super) staged: usize,
    pub(super) stageable: bool,
    pub(super) staged_private: bool,
    pub(super) revertible: bool,
}

impl From<Vec<Entry>> for ChangeList {
    fn from(items: Vec<Entry>) -> Self {
        let mut list = Self::default();
        for entry in &items {
            list.staged += usize::from(entry.staged());
            list.stageable |= entry.stageable();
            list.staged_private |= entry.staged() && private_artifact(&entry.path);
            list.revertible |= entry.revertible();
        }
        list.items = items;
        list
    }
}

impl std::ops::Deref for ChangeList {
    type Target = [Entry];

    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

pub(super) fn private_artifact(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str() == ".tiptoptyp")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiffKind {
    WorkingTree,
    Staged,
}

impl DiffKind {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::WorkingTree => "Unstaged changes",
            Self::Staged => "Staged changes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffSelection {
    pub(super) path: PathBuf,
    pub(super) kind: DiffKind,
}

#[derive(Debug)]
pub(super) struct DiffResult {
    pub(super) selection: DiffSelection,
    pub(super) content: Result<String, String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) root: PathBuf,
    pub(super) branch: String,
    pub(super) entries: ChangeList,
    pub(super) history: String,
    pub(super) initialized: bool,
}

#[derive(Debug, Clone)]
pub(super) enum Operation {
    Refresh,
    Init,
    Stage(PathBuf),
    Unstage(PathBuf),
    StageAll,
    UnstageAll,
    Commit(String),
    StageAllAndCommit(String),
    Revert(Vec<PathBuf>),
    Diff(PathBuf, DiffKind),
    Fetch,
    Pull,
    Push,
}

#[derive(Debug)]
pub(super) struct ResultData {
    pub(super) workspace: PathBuf,
    pub(super) snapshot: Snapshot,
    pub(super) output: String,
    pub(super) committed: bool,
    pub(super) failed: bool,
    pub(super) diff: Option<DiffResult>,
}

pub(super) struct BufferStatus {
    pub(super) snapshot: Snapshot,
    // A diff failure must not discard successfully collected file badges.
    pub(super) hunks: Result<Vec<diff::Hunk>, String>,
}

/// Cheap borrowed service handle; construction does not discover or launch Git.
/// Call its IO methods only from workers, never from a render callback.
pub(crate) struct Repository<'a> {
    workspace: &'a Path,
}
impl<'a> Repository<'a> {
    pub(crate) fn new(workspace: &'a Path) -> Self {
        Self { workspace }
    }
    pub(super) fn execute(&self, operation: Operation) -> Result<ResultData, String> {
        let mut result = perform(self.workspace, operation)?;
        // The lock resolves the repository root, but result routing belongs to
        // the originating workspace (which may be a subdirectory or alias).
        if result.workspace != self.workspace {
            result.workspace = self.workspace.to_owned();
        }
        Ok(result)
    }
    pub(super) fn status(&self) -> Result<Snapshot, String> {
        status_snapshot(self.workspace)
    }
    pub(super) fn scan_buffer(
        &self,
        path: Option<&Path>,
        source: &str,
    ) -> Result<BufferStatus, String> {
        let snapshot = self.status()?;
        let hunks = diff::buffer_hunks(&snapshot, path, source);
        Ok(BufferStatus { snapshot, hunks })
    }
    pub(crate) fn change_index(
        &self,
        path: &Path,
        source: &str,
        hunk: &diff::Hunk,
        action: hunks::Action,
    ) -> Result<String, String> {
        hunks::change_index(self.workspace, path, source, hunk, action)
    }
}

pub(super) fn run(root: &Path, args: &[OsString]) -> Result<Vec<u8>, String> {
    run_command(root, args, false)
}

fn git_executable() -> Option<PathBuf> {
    static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();
    if let Some(program) = EXECUTABLE.get() {
        return Some(program.clone());
    }
    let program = discover_git_executable()?;
    let _ = EXECUTABLE.set(program.clone());
    Some(program)
}

fn discover_git_executable() -> Option<PathBuf> {
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            #[cfg(windows)]
            {
                let extensions =
                    env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
                for extension in extensions.to_string_lossy().split(';') {
                    let candidate = directory.join(format!("git{extension}"));
                    if is_executable(&candidate) {
                        return Some(candidate);
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let candidate = directory.join("git");
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    // GUI applications on macOS do not always inherit the shell's PATH.
    // Check the system and common Homebrew locations after PATH so a Git
    // installation remains usable when the app was launched from Finder.
    #[cfg(target_os = "macos")]
    for candidate in [
        "/usr/bin/git",
        "/opt/homebrew/bin/git",
        "/usr/local/bin/git",
    ] {
        let candidate = PathBuf::from(candidate);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }

    None
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(super) fn run_command(
    root: &Path,
    args: &[OsString],
    diff_exit_status: bool,
) -> Result<Vec<u8>, String> {
    // File-backed output prevents pipe deadlocks and bounds resident output.
    if !root.is_dir() {
        return Err(format!("Git workspace is unavailable: {}", root.display()));
    }
    let program = git_executable()
        .ok_or_else(|| "Could not start Git: no Git executable was found".to_owned())?;
    let mut stdout = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut stderr = tempfile::tempfile().map_err(|e| e.to_string())?;
    let child = Command::new(program)
        .arg("--no-pager")
        .arg("--literal-pathspecs")
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(stdout.try_clone().map_err(|e| e.to_string())?)
        .stderr(stderr.try_clone().map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound && !root.is_dir() {
                format!("Git workspace is unavailable: {}", root.display())
            } else {
                format!("Could not start Git: {error}")
            }
        })?;
    let status = crate::process::wait(child, Duration::from_secs(60), || {
        if stdout.metadata()?.len() > 4 * 1024 * 1024 || stderr.metadata()?.len() > 4 * 1024 * 1024
        {
            return Err(std::io::Error::other(
                "Git output exceeds 4 MiB; narrow the operation in a terminal",
            ));
        }
        Ok(())
    })
    .map_err(|error| format!("Git: {error}"))?;
    let read = |file: &mut fs::File| -> Result<Vec<u8>, String> {
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("Git output exceeds 4 MiB; narrow the operation in a terminal".into());
        }
        Ok(bytes)
    };
    let output = read(&mut stdout)?;
    let errors = read(&mut stderr)?;
    if status.success() || (diff_exit_status && status.code() == Some(1)) {
        Ok(output)
    } else {
        let error = String::from_utf8_lossy(&errors).trim().to_owned();
        Err(if error.is_empty() {
            format!("Git exited with {status}")
        } else {
            error
        })
    }
}
pub(super) fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

/// Both diff consumers parse unified output, so user presentation settings
/// must not change its line prefixes or remove blank context markers.
pub(super) fn diff_args() -> Vec<OsString> {
    args(&[
        "-c",
        "diff.suppressBlankEmpty=false",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--output-indicator-new=+",
        "--output-indicator-old=-",
        "--output-indicator-context= ",
    ])
}
pub(super) fn text_run(root: &Path, values: &[&str]) -> Result<String, String> {
    run(root, &args(values)).map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
}

pub(super) fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(
            std::str::from_utf8(bytes).map_err(|e| e.to_string())?,
        ))
    }
}

pub(super) fn parse_status(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .filter(|record| !record.starts_with(b"## "))
        .map(|record| {
            if record.len() < 4 || record[2] != b' ' {
                return Err("Invalid Git status record".into());
            }
            let path = path_from_bytes(&record[3..])?;
            Ok(Entry {
                path,
                index: record[0] as char,
                worktree: record[1] as char,
            })
        })
        .collect()
}

pub(super) fn snapshot(workspace: &Path) -> Result<Snapshot, String> {
    let mut snapshot = status_snapshot(workspace)?;
    if snapshot.initialized {
        snapshot.history = text_run(&snapshot.root, &["log", "-10", "--oneline"])
            .unwrap_or_else(|_| "No commits yet".into());
    }
    Ok(snapshot)
}

/// Background decorations need status only, never commit history or a second
/// full status scan just to obtain the branch header.
pub(super) fn status_snapshot(workspace: &Path) -> Result<Snapshot, String> {
    // No-renames gives one NUL-delimited record per path, including both sides
    // of a rename, without ambiguous quoting or arrow parsing.
    let Some(root) = repository_root(workspace)? else {
        return Ok(Snapshot {
            root: workspace.to_path_buf(),
            ..Default::default()
        });
    };
    status_at_root(root)
}

fn repository_root(workspace: &Path) -> Result<Option<PathBuf>, String> {
    let root_bytes = match run(workspace, &args(&["rev-parse", "--show-toplevel"])) {
        Ok(root) => root,
        Err(error) if error.contains("not a git repository") => return Ok(None),
        Err(error) => return Err(error),
    };
    path_from_bytes(root_bytes.strip_suffix(b"\n").unwrap_or(&root_bytes)).map(Some)
}

pub(super) fn status_at_root(root: PathBuf) -> Result<Snapshot, String> {
    let status = run(
        &root,
        &args(&[
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "--branch",
            "-z",
            "--no-renames",
            "--untracked-files=all",
        ]),
    )?;
    let mut entries = parse_status(&status)?;
    // A preview mirror is app-owned, even in repositories without a matching
    // ignore rule. Keep tracked/staged artifacts visible so they can be unstaged.
    entries.retain(|entry| entry.index != '?' || !private_artifact(&entry.path));
    let branch =
        String::from_utf8_lossy(status.split(|byte| *byte == 0).next().unwrap_or_default())
            .trim_start_matches("## ")
            .to_owned();
    Ok(Snapshot {
        root,
        branch,
        entries: entries.into(),
        history: String::new(),
        initialized: true,
    })
}

pub(super) fn perform(workspace: &Path, operation: Operation) -> Result<ResultData, String> {
    if matches!(operation, Operation::Refresh | Operation::Diff(..)) {
        return perform_locked(workspace, operation);
    }
    with_repository_transaction(workspace, |root| perform_locked(root, operation))
}

/// All app-owned index/repository mutations share this lease. Resolve identity
/// first, but read status, HEAD and index preconditions only after acquiring it.
/// Git's own locks and patch checks still handle unrelated external writers.
pub(super) fn with_repository_transaction<T>(
    workspace: &Path,
    operation: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    let root = repository_root(workspace)?.unwrap_or_else(|| workspace.to_owned());
    crate::resource_lock::with_resource(&root, || operation(&root))
}

pub(super) fn perform_locked(workspace: &Path, operation: Operation) -> Result<ResultData, String> {
    #[cfg(test)]
    if !matches!(operation, Operation::Refresh | Operation::Diff(..)) {
        assert!(crate::resource_lock::is_locked_for_test(workspace));
    }
    let before = snapshot(workspace)?;
    let root = &before.root;
    let committed = matches!(
        operation,
        Operation::Commit(_) | Operation::StageAllAndCommit(_)
    );
    // Keep the NUL-delimited pathspec alive until Git has consumed it.
    let mut pathspec = None;
    let command = match operation {
        Operation::Refresh => Vec::new(),
        Operation::Init => args(&["init"]),
        Operation::Stage(path) => {
            if private_artifact(&path) {
                return Err("Temporary .tiptoptyp files are excluded from staging".into());
            }
            let mut command = args(&["add", "--"]);
            command.push(path.into_os_string());
            command
        }
        Operation::Unstage(path) => unstage_command(root, path, false),
        Operation::UnstageAll => unstage_command(root, PathBuf::from("."), true),
        Operation::Revert(paths) => {
            // Restore only the paths covered by the confirmation. Never use
            // `restore .`, which could discard changes added while it was open.
            let eligible: std::collections::HashMap<_, _> = before
                .entries
                .iter()
                .filter(|entry| entry.revertible())
                .map(|entry| (&entry.path, entry))
                .collect();
            if paths.is_empty() || paths.iter().any(|path| !eligible.contains_key(path)) {
                return Err("The working changes changed; review them before reverting".into());
            }
            let (untracked, tracked): (Vec<_>, Vec<_>) =
                paths.iter().partition(|path| eligible[path].index == '?');
            // A nested repository can appear as an untracked directory. Never
            // recursively clean directories or paths beyond the confirmation.
            for path in &untracked {
                let full = root.join(path);
                let parent = full
                    .parent()
                    .ok_or("Missing file parent")?
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
                if !parent.starts_with(root)
                    || fs::symlink_metadata(&full)
                        .map_err(|e| e.to_string())?
                        .is_dir()
                {
                    return Err(
                        "Revert only deletes individual new files; review directories separately"
                            .into(),
                    );
                }
            }
            if !tracked.is_empty() {
                let file = pathspec_file(tracked.iter().map(|path| path.as_path()))?;
                let mut command = args(&[
                    "restore",
                    "--worktree",
                    "--pathspec-file-nul",
                    "--pathspec-from-file",
                ]);
                command.push(file.path().as_os_str().to_owned());
                run(root, &command)?;
            }
            // Git rechecks index membership and ignore rules. No -d, -x or
            // repository-wide pathspec: staged, ignored and private files survive.
            for paths in untracked.chunks(128) {
                let mut command = args(&["clean", "-f", "--"]);
                command.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
                run(root, &command)?;
            }
            return Ok(ResultData {
                workspace: workspace.to_owned(),
                snapshot: snapshot(workspace)?,
                output: format!("Reverted unstaged changes in {} files", paths.len()),
                committed: false,
                failed: false,
                diff: None,
            });
        }
        Operation::StageAll => {
            let (command, file) = stage_all_command(&before)?;
            pathspec = Some(file);
            command
        }
        Operation::StageAllAndCommit(message) => {
            if message.trim().is_empty() {
                return Err("Enter a commit message".into());
            }
            // Check again under the repository lease: another window may
            // have staged a deliberate subset since the button was rendered.
            if before.entries.staged > 0 {
                return Err("The staging area changed; review it before committing".into());
            }
            let (command, _file) = stage_all_command(&before)?;
            run(root, &command)?;
            let mut command = args(&["commit", "-m"]);
            command.push(message.into());
            command
        }
        Operation::Commit(message) => {
            if message.trim().is_empty() {
                return Err("Enter a commit message".into());
            }
            let mut command = args(&["commit", "-m"]);
            command.push(message.into());
            command
        }
        Operation::Diff(path, kind) => {
            let untracked = kind == DiffKind::WorkingTree
                && before
                    .entries
                    .iter()
                    .any(|entry| entry.path == path && entry.index == '?');
            let mut command = diff_args();
            if untracked {
                command.push("--no-index".into());
            }
            if kind == DiffKind::Staged {
                command.push("--cached".into());
            }
            command.push("--".into());
            if untracked {
                command.push(if cfg!(windows) { "NUL" } else { "/dev/null" }.into());
            }
            command.push(path.as_os_str().to_owned());
            // --no-index uses exit code 1 for a successful comparison with changes.
            let bytes = run_command(root, &command, untracked)?;
            return Ok(ResultData {
                workspace: workspace.to_path_buf(),
                snapshot: before,
                output: format!("{}: {}", kind.title(), path.display()),
                committed: false,
                failed: false,
                diff: Some(DiffResult {
                    selection: DiffSelection { path, kind },
                    content: Ok(String::from_utf8_lossy(&bytes).into_owned()),
                }),
            });
        }
        Operation::Fetch => args(&["fetch"]),
        Operation::Pull => args(&["pull", "--ff-only"]),
        Operation::Push => args(&["push"]),
    };
    let output = if command.is_empty() {
        "Status refreshed".into()
    } else {
        let bytes = run(root, &command)?;
        if bytes.is_empty() {
            "Git operation completed".into()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };
    drop(pathspec);
    Ok(ResultData {
        workspace: workspace.to_path_buf(),
        snapshot: if command.is_empty() {
            before
        } else {
            snapshot(workspace)?
        },
        output,
        committed,
        failed: false,
        diff: None,
    })
}

fn stage_all_command(
    snapshot: &Snapshot,
) -> Result<(Vec<OsString>, tempfile::NamedTempFile), String> {
    if !snapshot.entries.stageable {
        return Err("No working changes to stage".into());
    }
    let file = pathspec_file(
        snapshot
            .entries
            .iter()
            .filter(|entry| entry.stageable())
            .map(|entry| entry.path.as_path()),
    )?;
    let mut command = args(&[
        "add",
        "--all",
        "--pathspec-file-nul",
        "--pathspec-from-file",
    ]);
    command.push(file.path().as_os_str().to_owned());
    Ok((command, file))
}

fn pathspec_file<'a>(
    paths: impl Iterator<Item = &'a Path>,
) -> Result<tempfile::NamedTempFile, String> {
    let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    for path in paths {
        file.write_all(path.as_os_str().as_encoded_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&[0]).map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())?;
    Ok(file)
}

fn unstage_command(root: &Path, path: PathBuf, all: bool) -> Vec<OsString> {
    let unborn = text_run(root, &["rev-parse", "--verify", "HEAD"]).is_err();
    let mut command = if unborn {
        // There is no HEAD to restore yet. --cached changes only the index;
        // force also permits files edited again after their initial staging.
        let mut command = args(&["rm", "--cached", "--force"]);
        if all {
            command.push("-r".into());
        }
        command
    } else {
        args(&["restore", "--staged"])
    };
    command.push("--".into());
    command.push(path.into_os_string());
    command
}
