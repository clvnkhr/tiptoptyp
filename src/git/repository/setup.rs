//! Explicit, worker-side repository setup. Never stages, commits or contacts a remote.
use super::*;
use sha2::{Digest, Sha256};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const PRIVATE_EXCLUDE: &[u8] = b".tiptoptyp/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SetupSnapshot {
    pub(crate) workspace: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) initialized: bool,
    pub(crate) author_name: String,
    pub(crate) author_email: String,
    pub(crate) origin: String,
    pub(crate) branch: String,
    // Include full local configuration and inherited identity without exposing
    // credential-bearing remote strings to the form or its debug output.
    stamp: [u8; 32],
}

#[derive(Clone, Debug)]
pub(crate) struct SetupRequest {
    pub(crate) expected: SetupSnapshot,
    pub(crate) initial_branch: String,
    pub(crate) author_name: String,
    pub(crate) author_email: String,
    pub(crate) origin: String,
}

pub(crate) struct GitHubCredential {
    pub(crate) scope: String,
    pub(crate) username: String,
    pub(crate) helper: String,
}

fn canonical_workspace(workspace: &Path) -> Result<PathBuf, String> {
    let path = workspace
        .canonicalize()
        .map_err(|e| format!("The workspace is unavailable: {e}"))?;
    if !path.is_dir() {
        return Err("Choose an existing workspace folder for Git.".into());
    }
    Ok(path)
}

pub(super) fn config_value(root: &Path, key: &str) -> Result<Vec<u8>, String> {
    // Exit 1 means an unset key, not a failed Git installation.
    let mut value = run_command(root, &args(&["config", "--null", "--get", key]), true)?;
    if value.last() == Some(&0) {
        value.pop();
    }
    Ok(value)
}

fn local_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    let bytes = run(root, &args(&["rev-parse", "--git-path", name]))?;
    let path = path_from_bytes(bytes.strip_suffix(b"\n").unwrap_or(&bytes))?;
    Ok(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

fn read_regular(path: &Path) -> Result<Vec<u8>, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("Git setup cannot replace linked configuration files.".into());
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Could not read Git configuration: {error}")),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err("Git setup requires regular configuration files smaller than 1 MiB.".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err("Git configuration grew beyond 1 MiB. Review it in a terminal.".into());
    }
    Ok(bytes)
}

fn display_origin(origin: &str) -> String {
    let Ok(mut url) = url::Url::parse(origin) else {
        return origin.to_owned();
    };
    if matches!(url.scheme(), "http" | "https") {
        let _ = url.set_username("");
    }
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

pub(crate) fn inspect(workspace: &Path) -> Result<SetupSnapshot, String> {
    let workspace = canonical_workspace(workspace)?;
    if git_executable().is_none() {
        return Err("Git could not be found. Install Git (or Xcode Command Line Tools on macOS), then reopen Set up Git.".into());
    }
    let repository = repository_root(&workspace)?;
    let initialized = repository.is_some();
    let root = repository
        .unwrap_or_else(|| workspace.clone())
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let name = config_value(&root, "user.name")?;
    let email = config_value(&root, "user.email")?;
    let origin_urls = if initialized {
        run_command(
            &root,
            &args(&["config", "--null", "--get-all", "remote.origin.url"]),
            true,
        )?
    } else {
        Vec::new()
    };
    let origin_push_urls = if initialized {
        run_command(
            &root,
            &args(&["config", "--null", "--get-all", "remote.origin.pushurl"]),
            true,
        )?
    } else {
        Vec::new()
    };
    let origin = origin_urls.split(|b| *b == 0).next().unwrap_or_default();
    let branch = if initialized {
        text_run(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .unwrap_or_else(|_| "Detached HEAD".into())
    } else {
        "main".into()
    };
    let mut digest = Sha256::new();
    for part in [
        &name[..],
        &email,
        &origin_urls,
        &origin_push_urls,
        branch.as_bytes(),
    ] {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    if initialized {
        digest.update(read_regular(&local_path(&root, "config")?)?);
    }
    Ok(SetupSnapshot {
        workspace,
        root,
        initialized,
        author_name: String::from_utf8_lossy(&name).into_owned(),
        author_email: String::from_utf8_lossy(&email).into_owned(),
        origin: display_origin(&String::from_utf8_lossy(origin)),
        branch,
        stamp: digest.finalize().into(),
    })
}

fn validate(request: &SetupRequest) -> Result<(), String> {
    let name = request.author_name.trim();
    let email = request.author_email.trim();
    if name.is_empty() != email.is_empty() {
        return Err("Enter both an author name and an email address.".into());
    }
    if name.len() > 256
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>'))
    {
        return Err(
            "Enter an author name without line breaks or angle brackets (up to 256 bytes).".into(),
        );
    }
    if !email.is_empty()
        && (email.len() > 320
            || email
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>'))
            || !email.split_once('@').is_some_and(|(local, host)| {
                !local.is_empty() && !host.is_empty() && !host.contains('@')
            }))
    {
        return Err("Enter an email address such as name@example.com.".into());
    }
    if request.origin.trim() != request.expected.origin {
        validate_origin(request.origin.trim())?;
    }
    if !request.expected.initialized {
        let branch = request.initial_branch.trim();
        if branch.is_empty() || branch.len() > 128 || branch.chars().any(char::is_control) {
            return Err("Enter an initial branch name (up to 128 bytes).".into());
        }
        run(
            &request.expected.workspace,
            &args(&["check-ref-format", "--branch", branch]),
        )
        .map_err(|_| "Enter a valid Git branch name, such as main.".to_owned())?;
    }
    Ok(())
}

fn validate_origin(origin: &str) -> Result<(), String> {
    if origin.is_empty() {
        return Ok(());
    }
    if origin.len() > 4096 || origin.chars().any(char::is_control) {
        return Err("The remote address is too long or contains a line break.".into());
    }
    if Path::new(origin).is_absolute() {
        return Ok(());
    }
    if origin.contains("://") {
        let url =
            url::Url::parse(origin).map_err(|_| "Enter a valid Git remote address.".to_owned())?;
        if !matches!(url.scheme(), "http" | "https" | "ssh" | "file") {
            return Err("Use an HTTPS, SSH, or absolute local repository address.".into());
        }
        if url.password().is_some()
            || (!url.username().is_empty() && matches!(url.scheme(), "http" | "https" | "file"))
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Keep passwords and access tokens out of the remote address. Use Git's credential manager or SSH.".into());
        }
        if url.scheme() == "file" {
            return url
                .to_file_path()
                .map(|_| ())
                .map_err(|_| "Enter an absolute local repository address.".into());
        }
        if url.host_str().is_some_and(|host| !host.is_empty())
            && !url.path().trim_matches('/').is_empty()
            && !origin.chars().any(char::is_whitespace)
        {
            return Ok(());
        }
    } else if let Some((host, path)) = origin.split_once(':')
        && !host.is_empty()
        && !path.is_empty()
        && !host.starts_with('-')
        && !origin.chars().any(char::is_whitespace)
        && !host.contains(['/', '\\'])
        && !path.starts_with(['-', ':'])
    {
        return Ok(());
    }
    Err("Enter a repository address such as https://host/owner/project.git or git@host:owner/project.git.".into())
}

pub(crate) fn apply(workspace: &Path, request: SetupRequest) -> Result<SetupSnapshot, String> {
    apply_with_credential(workspace, request, None)
}

pub(crate) fn connect_github(
    workspace: &Path,
    request: SetupRequest,
    credential: GitHubCredential,
) -> Result<SetupSnapshot, String> {
    if !credential.scope.starts_with("https://github.com/")
        || credential.scope != request.origin.trim()
        || credential.scope.len() > 256
        || credential.username.is_empty()
        || credential.username.len() > 39
        || !credential
            .username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || credential.helper.len() > 16 * 1024
        || credential.helper.chars().any(char::is_control)
    {
        return Err("The GitHub credential helper configuration is invalid.".into());
    }
    apply_with_credential(workspace, request, Some(credential))
}

fn apply_with_credential(
    workspace: &Path,
    request: SetupRequest,
    credential: Option<GitHubCredential>,
) -> Result<SetupSnapshot, String> {
    let workspace = canonical_workspace(workspace)?;
    if workspace != request.expected.workspace {
        return Err("The workspace changed. Reopen Git setup before saving.".into());
    }
    validate(&request)?;
    with_repository_transaction(&workspace, |_| {
        let current = inspect(&workspace)?;
        if current != request.expected {
            return Err(
                "The repository or its configuration changed. Reopen Git setup to review it."
                    .into(),
            );
        }
        if !current.initialized {
            run(
                &workspace,
                &args(&["init", "--initial-branch", request.initial_branch.trim()]),
            )?;
        }
        let result = (|| {
            let root = &current.root;
            ensure_private_excluded(root)?;
            let configured = if current.initialized {
                current.clone()
            } else {
                inspect(&workspace)?
            };
            write_local_configuration(root, &configured, &request, credential.as_ref())?;
            inspect(&workspace)
        })();
        result.map_err(|error: String| {
            format!(
                "Git setup was not completed: {error} Reopen Git setup to review the current state."
            )
        })
    })
}

fn write_local_configuration(
    root: &Path,
    current: &SetupSnapshot,
    request: &SetupRequest,
    credential: Option<&GitHubCredential>,
) -> Result<(), String> {
    let path = local_path(root, "config")?;
    crate::resource_lock::with_resource(&path, || {
        write_local_configuration_at(root, current, request, &path, credential)
    })
}

fn write_local_configuration_at(
    root: &Path,
    current: &SetupSnapshot,
    request: &SetupRequest,
    path: &Path,
    credential: Option<&GitHubCredential>,
) -> Result<(), String> {
    #[cfg(test)]
    assert!(crate::resource_lock::is_locked_for_test(path));
    let original = read_regular(path)?;
    let parent = path
        .parent()
        .ok_or("Git configuration has no parent folder")?;
    let lock_path = parent.join("config.lock");
    let permissions = fs::metadata(path).map_err(|e| e.to_string())?.permissions();
    let lock = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|e| format!("Git configuration is busy or unavailable: {e}"))?;
    if let Err(error) = lock.set_permissions(permissions) {
        drop(lock);
        let _ = fs::remove_file(&lock_path);
        return Err(error.to_string());
    }
    drop(lock);
    let result = (|| {
        if inspect(&current.workspace)? != *current {
            return Err(
                "Git configuration changed before it could be saved. Reopen Git setup.".into(),
            );
        }
        // Retain every unrelated key and comment. Git edits only these local
        // fields in its own configuration syntax, inside the standard lock.
        fs::write(&lock_path, &original).map_err(|e| e.to_string())?;
        let configure = |values: &[&str]| -> Result<(), String> {
            let mut command = args(&["config", "--file"]);
            command.push(lock_path.as_os_str().to_owned());
            command.extend(args(values));
            run(root, &command).map(|_| ())
        };
        if !request.author_name.trim().is_empty() {
            configure(&["--replace-all", "user.name", request.author_name.trim()])?;
            configure(&["--replace-all", "user.email", request.author_email.trim()])?;
        }
        let requested_origin = request.origin.trim();
        if requested_origin != current.origin {
            let urls = run_command(
                root,
                &args(&["config", "--null", "--get-all", "remote.origin.url"]),
                true,
            )?;
            let local_urls = run_command(
                root,
                &args(&[
                    "config",
                    "--local",
                    "--no-includes",
                    "--null",
                    "--get-all",
                    "remote.origin.url",
                ]),
                true,
            )?;
            let push_urls = config_value(root, "remote.origin.pushurl")?;
            if urls != local_urls && !urls.is_empty() {
                return Err("Origin is inherited from another Git configuration. Change that configuration in a terminal before editing its URL here.".into());
            }
            if urls.split(|b| *b == 0).filter(|s| !s.is_empty()).count() > 1
                || !push_urls.is_empty()
            {
                return Err("Origin has multiple URLs or a separate push URL. Change it in a terminal so those destinations are preserved.".into());
            }
            if requested_origin.is_empty() {
                if !current.origin.is_empty() {
                    configure(&["--remove-section", "remote.origin"])?;
                }
            } else {
                configure(&["--replace-all", "remote.origin.url", requested_origin])?;
                if current.origin.is_empty() {
                    configure(&[
                        "--replace-all",
                        "remote.origin.fetch",
                        "+refs/heads/*:refs/remotes/origin/*",
                    ])?;
                }
            }
        }
        if let Some(credential) = credential {
            let prefix = format!("credential.{}", credential.scope);
            configure(&[
                "--replace-all",
                &format!("{prefix}.username"),
                &credential.username,
            ])?;
            configure(&["--replace-all", &format!("{prefix}.useHttpPath"), "true"])?;
            configure(&["--replace-all", &format!("{prefix}.helper"), ""])?;
            configure(&["--add", &format!("{prefix}.helper"), &credential.helper])?;
        }
        // Git's edits replace config.lock, so synchronize its final inode.
        read_regular(&lock_path)?;
        fs::File::open(&lock_path)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(&lock_path, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&lock_path);
    }
    result
}

pub(super) fn ensure_private_excluded(root: &Path) -> Result<(), String> {
    let path = local_path(root, "info/exclude")?;
    crate::resource_lock::with_resource(&path, || ensure_private_excluded_at(&path))
}

fn ensure_private_excluded_at(path: &Path) -> Result<(), String> {
    #[cfg(test)]
    assert!(crate::resource_lock::is_locked_for_test(path));
    let original = read_regular(path)?;
    let mut contents = original.clone();
    if contents
        .split(|b| *b == b'\n')
        .any(|line| line.strip_suffix(b"\r").unwrap_or(line) == PRIVATE_EXCLUDE)
    {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or("Git exclude file has no parent folder")?;
    match fs::symlink_metadata(parent) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
        Ok(_) => return Err(
            "Git's info folder is linked or invalid. Exclude .tiptoptyp/ in Git before continuing."
                .into(),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(parent).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    }
    if !contents.is_empty() && !contents.ends_with(b"\n") {
        contents.push(b'\n');
    }
    contents.extend_from_slice(b"\n# tiptoptyp editor data\n.tiptoptyp/\n");
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    if let Ok(metadata) = fs::metadata(path) {
        staged
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(|e| e.to_string())?;
    }
    staged
        .write_all(&contents)
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    if read_regular(path)? != original {
        return Err(
            "Git's exclude file changed. Reopen Git setup to preserve the new rules.".into(),
        );
    }
    staged.persist(path).map_err(|e| e.error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests;
