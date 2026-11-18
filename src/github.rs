//! Worker-side GitHub device sign-in and explicit empty-repository creation.
//! Credentials stay in the official helper's secure store, never in app state.
use crate::{git::repository::setup, github_cli::Prepared, process::OwnedChild};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const HOST: &str = "github.com";
const DEVICE_URL: &str = "https://github.com/login/device";
const OUTPUT_LIMIT: u64 = 256 * 1024;
const STDERR_LIMIT: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Account {
    pub(crate) id: u64,
    pub(crate) login: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    Preparing,
    DeviceCode {
        code: String,
        verification_uri: String,
    },
    WaitingForBrowser,
}

#[derive(Clone, Debug)]
pub(crate) struct CreateRequest {
    pub(crate) expected: setup::SetupSnapshot,
    pub(crate) account: Account,
    pub(crate) name: String,
    pub(crate) private: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct CreatedRepository {
    pub(crate) account: Account,
    pub(crate) name: String,
    pub(crate) html_url: String,
    pub(crate) clone_url: String,
    pub(crate) snapshot: Result<setup::SetupSnapshot, String>,
    // A created remote cannot be reassigned to whichever workspace is active
    // when its reply arrives. Retry attaches only to this originating root.
    workspace: PathBuf,
    root: PathBuf,
    confirmation: Confirmation,
    pub(crate) private: bool,
}

#[derive(Clone, Debug)]
enum Confirmation {
    Requested,
    Confirmed(u64),
}

impl CreatedRepository {
    pub(crate) fn confirmed(&self) -> bool {
        matches!(self.confirmation, Confirmation::Confirmed(_))
    }

    fn repository_id(&self) -> Option<u64> {
        match self.confirmation {
            Confirmation::Requested => None,
            Confirmation::Confirmed(id) => Some(id),
        }
    }
}

/// Connecting a working copy retains the exact remote identity verified by
/// GitHub, rather than reducing that receipt to an unowned `confirmed` flag.
#[derive(Debug)]
pub(crate) struct AttachedRepository {
    repository: CreatedRepository,
    snapshot: setup::SetupSnapshot,
}

impl AttachedRepository {
    pub(crate) fn into_parts(mut self) -> (setup::SetupSnapshot, CreatedRepository) {
        self.repository.snapshot = Ok(self.snapshot.clone());
        (self.snapshot, self.repository)
    }

    fn into_created(self) -> CreatedRepository {
        self.into_parts().1
    }

    #[cfg(test)]
    pub(crate) fn fixture(snapshot: setup::SetupSnapshot) -> Self {
        let mut repository = CreatedRepository::fixture(true);
        repository.workspace = snapshot.workspace.clone();
        repository.root = snapshot.root.clone();
        repository.clone_url.clone_from(&snapshot.origin);
        Self {
            repository,
            snapshot,
        }
    }
}

#[cfg(test)]
impl CreatedRepository {
    pub(crate) fn fixture(confirmed: bool) -> Self {
        Self {
            account: Account {
                id: 42,
                login: "example".into(),
            },
            name: "paper".into(),
            html_url: "https://github.com/example/paper".into(),
            clone_url: "https://github.com/example/paper.git".into(),
            snapshot: Err("Review the repository connection.".into()),
            workspace: "/test/workspace".into(),
            root: "/test/workspace".into(),
            confirmation: if confirmed {
                Confirmation::Confirmed(101)
            } else {
                Confirmation::Requested
            },
            private: true,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreationRecord {
    version: u8,
    workspace: PathBuf,
    root: PathBuf,
    account: Account,
    name: String,
    private: bool,
    requested_at_ms: u64,
    receipt: Option<PublicReceipt>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicReceipt {
    id: u64,
    html_url: String,
    clone_url: String,
}

const RECORD_LIMIT: u64 = 64 * 1024;

fn workspace_hash(workspace: &Path) -> String {
    hex::encode(Sha256::digest(workspace.as_os_str().as_encoded_bytes()))
}

fn record_directory(config: &Path, create: bool) -> Result<PathBuf, String> {
    let directory = config.join("repositories");
    match fs::symlink_metadata(&directory) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
        Ok(_) => return Err("GitHub repository recovery storage is invalid.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => return Ok(directory),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder
                .create(&directory)
                .map_err(|_| "Could not create GitHub repository recovery storage.")?;
        }
        Err(_) => return Err("GitHub repository recovery storage is unavailable.".into()),
    }
    Ok(directory)
}

fn record_path(config: &Path, record: &CreationRecord) -> Result<PathBuf, String> {
    let suffix = hex::encode(Sha256::digest(record.name.to_ascii_lowercase().as_bytes()));
    Ok(record_directory(config, true)?.join(format!(
        "{}-{}-{suffix}.json",
        workspace_hash(&record.workspace),
        record.account.id
    )))
}

fn read_record(path: &Path) -> Result<CreationRecord, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "GitHub repository recovery data is unavailable.")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > RECORD_LIMIT {
        return Err("GitHub repository recovery data is invalid.".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut bytes = Vec::new();
    options
        .open(path)
        .map_err(|_| "GitHub repository recovery data is unavailable.")?
        .take(RECORD_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "GitHub repository recovery data is unavailable.")?;
    if bytes.len() as u64 > RECORD_LIMIT {
        return Err("GitHub repository recovery data is too large.".into());
    }
    let record: CreationRecord = serde_json::from_slice(&bytes)
        .map_err(|_| "GitHub repository recovery data is invalid.")?;
    if record.version != 1
        || !record.workspace.is_absolute()
        || !record.root.is_absolute()
        || !record.workspace.starts_with(&record.root)
        || !valid_login(&record.account.login)
        || record.account.id == 0
        || validate_name(&record.name).is_err()
    {
        return Err("GitHub repository recovery data does not describe a valid request.".into());
    }
    if let Some(receipt) = &record.receipt {
        let expected = format!(
            "https://github.com/{}/{}",
            record.account.login, record.name
        );
        if receipt.id == 0
            || receipt.html_url != expected
            || receipt.clone_url != format!("{expected}.git")
        {
            return Err("GitHub repository recovery data contains an unexpected address.".into());
        }
    }
    Ok(record)
}

fn write_record(config: &Path, record: &CreationRecord, new: bool) -> Result<bool, String> {
    let path = record_path(config, record)?;
    let bytes = serde_json::to_vec(record)
        .map_err(|_| "Could not encode GitHub repository recovery data.")?;
    if bytes.len() as u64 > RECORD_LIMIT {
        return Err("The GitHub repository recovery record is too large.".into());
    }
    let parent = path.parent().ok_or("GitHub recovery data has no folder")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "Could not write GitHub repository recovery data.")?;
    staged
        .write_all(&bytes)
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|_| "Could not save GitHub repository recovery data.")?;
    if new {
        match staged.persist_noclobber(&path) {
            Ok(_) => {}
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
            Err(_) => {
                return Err(
                    "Could not save the GitHub creation intent. No creation request was sent."
                        .into(),
                );
            }
        }
    } else {
        staged.persist(&path).map_err(
            |_| "The repository was created, but its recovery receipt could not be saved.",
        )?;
    }
    #[cfg(unix)]
    fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|_| "Could not synchronize GitHub repository recovery data.")?;
    Ok(true)
}

fn from_record(record: CreationRecord) -> CreatedRepository {
    let expected_url = format!(
        "https://github.com/{}/{}",
        record.account.login, record.name
    );
    let receipt = record.receipt;
    CreatedRepository {
        account: record.account,
        name: record.name,
        html_url: receipt
            .as_ref()
            .map_or_else(|| expected_url.clone(), |r| r.html_url.clone()),
        clone_url: receipt
            .as_ref()
            .map_or_else(|| format!("{expected_url}.git"), |r| r.clone_url.clone()),
        snapshot: Err("Review the created repository and retry its connection.".into()),
        confirmation: receipt.as_ref().map_or(Confirmation::Requested, |receipt| {
            Confirmation::Confirmed(receipt.id)
        }),
        private: record.private,
        workspace: record.workspace,
        root: record.root,
    }
}

fn as_record(created: &CreatedRepository) -> CreationRecord {
    CreationRecord {
        version: 1,
        workspace: created.workspace.clone(),
        root: created.root.clone(),
        account: created.account.clone(),
        name: created.name.clone(),
        private: created.private,
        requested_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64,
        receipt: created.repository_id().map(|id| PublicReceipt {
            id,
            html_url: created.html_url.clone(),
            clone_url: created.clone_url.clone(),
        }),
    }
}

pub(crate) fn load_created(workspace: &Path) -> Result<Option<CreatedRepository>, String> {
    let workspace = workspace
        .canonicalize()
        .map_err(|_| "The GitHub workspace is unavailable.")?;
    let config = crate::github_cli::config_directory()?;
    let directory = record_directory(&config, false)?;
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("GitHub repository recovery storage is unavailable.".into()),
    };
    let prefix = format!("{}-", workspace_hash(&workspace));
    let mut newest: Option<CreationRecord> = None;
    for (index, entry) in entries.enumerate() {
        if index >= 4096 {
            return Err(
                "GitHub repository recovery storage contains too many entries to inspect safely."
                    .into(),
            );
        }
        let entry = entry.map_err(|_| "GitHub repository recovery storage could not be read.")?;
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let record = read_record(&entry.path())?;
        if record.workspace == workspace
            && newest
                .as_ref()
                .is_none_or(|old| record.requested_at_ms > old.requested_at_ms)
        {
            newest = Some(record);
        }
    }
    Ok(newest.map(|record| {
        let mut created = from_record(record);
        if created.confirmed()
            && let Ok(snapshot) = setup::inspect(&workspace)
            && snapshot.workspace == created.workspace
            && snapshot.root == created.root
            && snapshot.origin == created.clone_url
        {
            created.snapshot = Ok(snapshot);
        }
        created
    }))
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("The GitHub operation was cancelled.".into())
    } else {
        Ok(())
    }
}

fn with_profile<T>(
    tool: &Prepared,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    crate::resource_lock::try_with_resource(&tool.config_dir, || Ok(operation())).map_err(|_| {
        "Another GitHub operation is running. Finish or cancel it before trying again.".to_owned()
    })?
}

fn prepare(cancel: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<Prepared, String> {
    check_cancel(cancel)?;
    progress(Progress::Preparing);
    crate::github_cli::prepare(cancel, &mut |_| {})
}

pub(crate) fn inspect_account(cancel: &AtomicBool) -> Result<Option<Account>, String> {
    check_cancel(cancel)?;
    // Opening setup does not download a helper when sign-in has never begun.
    let directory = crate::github_cli::config_directory()?;
    if !directory.join("hosts.yml").exists() {
        return Ok(None);
    }
    let tool = prepare(cancel, &mut |_| {})?;
    with_profile(&tool, || account(&tool, cancel))
}

pub(crate) fn sign_in(
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<Account, String> {
    let tool = prepare(cancel, &mut progress)?;
    with_profile(&tool, || {
        reject_plaintext_profile(&tool)?;
        let mut emitted = false;
        let output = run(
            &tool,
            &[
                "auth",
                "login",
                "--web",
                "--hostname",
                HOST,
                "--git-protocol",
                "https",
                "--skip-ssh-key",
                "--clipboard=false",
            ],
            cancel,
            Duration::from_secs(15 * 60),
            |stderr| {
                if !emitted
                    && let Some((code, verification_uri)) =
                        device_progress(stderr, tool.is_test_fixture())
                {
                    emitted = true;
                    progress(Progress::DeviceCode {
                        code,
                        verification_uri,
                    });
                    progress(Progress::WaitingForBrowser);
                }
            },
        );
        // A failed or cancelled login can still have reached local credential
        // persistence. Remove only this app profile's plaintext fallback.
        reject_plaintext_profile(&tool)?;
        let output = output?;
        if !output.success {
            return Err("GitHub sign-in did not finish. Start again to get a new code.".into());
        }
        account(&tool, cancel)?.ok_or_else(|| {
            "GitHub did not confirm a securely stored sign-in. Try signing in again.".into()
        })
    })
}

#[derive(Deserialize)]
struct AuthStatus {
    hosts: BTreeMap<String, Vec<AuthEntry>>,
}
#[derive(Deserialize)]
struct AuthEntry {
    state: String,
    active: bool,
    host: String,
    login: String,
    #[serde(rename = "tokenSource")]
    token_source: String,
}
#[derive(Deserialize)]
struct User {
    id: u64,
    login: String,
    #[serde(rename = "type")]
    kind: String,
}

fn account(tool: &Prepared, cancel: &AtomicBool) -> Result<Option<Account>, String> {
    reject_plaintext_profile(tool)?;
    let status = run(
        tool,
        &[
            "auth",
            "status",
            "--active",
            "--hostname",
            HOST,
            "--json",
            "hosts",
        ],
        cancel,
        Duration::from_secs(60),
        |_| {},
    )?;
    reject_plaintext_profile(tool)?;
    let status: AuthStatus = serde_json::from_slice(&status.stdout).map_err(|_| {
        "GitHub could not read the sign-in status. Try signing in again.".to_owned()
    })?;
    let mut active = status
        .hosts
        .get(HOST)
        .into_iter()
        .flatten()
        .filter(|entry| entry.active);
    let Some(entry) = active.next() else {
        return Ok(None);
    };
    if active.next().is_some() || entry.host != HOST || entry.state != "success" {
        return Err("GitHub could not confirm the active account. Try signing in again.".into());
    }
    if entry.token_source != "keyring" {
        reject_plaintext_profile(tool)?;
        return Err(
            "GitHub requires the system's secure credential store. Enable it, then sign in again."
                .into(),
        );
    }
    let user = run(
        tool,
        &["api", "--hostname", HOST, "--method", "GET", "user"],
        cancel,
        Duration::from_secs(60),
        |_| {},
    )?;
    reject_plaintext_profile(tool)?;
    if !user.success {
        return Err("GitHub could not verify the signed-in account. Try signing in again.".into());
    }
    let user: User = serde_json::from_slice(&user.stdout)
        .map_err(|_| "GitHub returned an invalid account response.".to_owned())?;
    if user.id == 0
        || user.kind != "User"
        || !valid_login(&user.login)
        || !entry.login.eq_ignore_ascii_case(&user.login)
    {
        return Err("Sign in to a personal GitHub account before creating a repository.".into());
    }
    Ok(Some(Account {
        id: user.id,
        login: user.login,
    }))
}

fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && !login.ends_with('-')
        && login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 100
        || matches!(name, "." | "..")
        || name.eq_ignore_ascii_case(".git")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        Err(
            "Use a repository name of up to 100 letters, numbers, dots, hyphens or underscores."
                .into(),
        )
    } else {
        Ok(())
    }
}

pub(crate) fn create_repository(
    workspace: &Path,
    request: CreateRequest,
    cancel: &AtomicBool,
) -> Result<CreatedRepository, String> {
    check_creation_request(workspace, &request)?;
    let tool = prepare(cancel, &mut |_| {})?;
    with_profile(&tool, || {
        create_with_tool(workspace, request, &tool, cancel)
    })
}

fn check_creation_request(workspace: &Path, request: &CreateRequest) -> Result<(), String> {
    let name = request.name.trim();
    validate_name(name)?;
    if !valid_login(&request.account.login) || request.account.id == 0 {
        return Err("Sign in to GitHub again before creating a repository.".into());
    }
    check_expected(workspace, &request.expected)?;
    if !request.expected.initialized || !request.expected.origin.is_empty() {
        return Err("Set up the local repository and clear its existing remote before creating a GitHub repository.".into());
    }
    Ok(())
}

fn create_with_tool(
    workspace: &Path,
    request: CreateRequest,
    tool: &Prepared,
    cancel: &AtomicBool,
) -> Result<CreatedRepository, String> {
    check_creation_request(workspace, &request)?;
    let name = request.name.trim();
    let current =
        account(tool, cancel)?.ok_or("Sign in to GitHub before creating a repository.")?;
    if current != request.account {
        return Err(
            "The GitHub account changed. Review the account before creating a repository.".into(),
        );
    }
    check_expected(workspace, &request.expected)?;
    check_cancel(cancel)?;
    let mut record = CreationRecord {
        version: 1,
        workspace: request.expected.workspace.clone(),
        root: request.expected.root.clone(),
        account: current.clone(),
        name: name.into(),
        private: request.private,
        requested_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64,
        receipt: None,
    };
    let path = record_path(&tool.config_dir, &record)?;
    if path.exists() {
        record = read_record(&path)?;
        if record.workspace != request.expected.workspace
            || record.root != request.expected.root
            || record.account != current
            || record.name != name
            || record.private != request.private
        {
            return Err("An earlier creation request has different settings. Recover that request before reviewing another repository.".into());
        }
        let recovered = from_record(record);
        let mut recovered = verify_repository(tool, &request.expected, &recovered, cancel)?
            .ok_or("An earlier creation request is still unconfirmed. Check the repository page or review a new request; it will not be created again automatically.")?;
        return match attach_with_tool(workspace, request.expected, &recovered, tool, cancel) {
            Ok(attached) => Ok(attached.into_created()),
            Err(error) => {
                recovered.snapshot = Err(error);
                Ok(recovered)
            }
        };
    }
    // Distinguish an already-existing personal repository from a request
    // whose network acknowledgement was lost after creating a new one.
    let route = format!("repos/{}/{name}", current.login);
    let probe = run(
        tool,
        &["api", "--hostname", HOST, "--method", "GET", &route],
        cancel,
        Duration::from_secs(60),
        |_| {},
    )?;
    if probe.success {
        return Err("A GitHub repository with this name already exists. Choose its URL in Existing remote, or review another name.".into());
    }
    if probe.http_status != Some(404) {
        return Err("GitHub could not check whether this repository name is available. Try again when it is reachable.".into());
    }
    if !write_record(&tool.config_dir, &record, true)? {
        return Err(
            "Another creation request was recorded first. Reopen GitHub setup to recover it."
                .into(),
        );
    }
    check_cancel(cancel)?;
    let private = if request.private {
        "private=true"
    } else {
        "private=false"
    };
    let field = format!("name={name}");
    let expected_url = format!("https://github.com/{}/{name}", current.login);
    let output = run(
        tool,
        &[
            "api",
            "--hostname",
            HOST,
            "--method",
            "POST",
            "user/repos",
            "--raw-field",
            &field,
            "--field",
            private,
            "--field",
            "auto_init=false",
        ],
        cancel,
        Duration::from_secs(60),
        |_| {},
    );
    reject_plaintext_profile(tool)?;
    let output = output.map_err(|_| {
        format!(
            "Repository creation could not be confirmed. Check {expected_url} before trying again."
        )
    })?;
    if !output.success {
        return Err(format!(
            "GitHub could not create the repository. The name may already exist or creation may not be allowed. Check {expected_url} before trying again."
        ));
    }
    let mut created = parse_created(&request, &output.stdout)
        .map_err(|_| format!("GitHub returned an unexpected repository response. Check {expected_url} before trying again."))?;
    // From here onward creation is confirmed. Always return its identity,
    // even on cancellation/local failure, so Retry never repeats the POST.
    let connected = write_record(&tool.config_dir, &as_record(&created), false)
        .and_then(|_| attach_with_tool(workspace, request.expected, &created, tool, cancel));
    match connected {
        Ok(attached) => Ok(attached.into_created()),
        Err(error) => {
            created.snapshot = Err(error);
            Ok(created)
        }
    }
}

#[derive(Deserialize)]
struct RepositoryResponse {
    id: u64,
    name: String,
    private: bool,
    owner: OwnerResponse,
    html_url: String,
    clone_url: String,
}
#[derive(Deserialize)]
struct OwnerResponse {
    id: u64,
    login: String,
}

fn parse_created(request: &CreateRequest, bytes: &[u8]) -> Result<CreatedRepository, String> {
    let repository: RepositoryResponse =
        serde_json::from_slice(bytes).map_err(|_| "Invalid repository response")?;
    if repository.id == 0
        || repository.name != request.name.trim()
        || repository.private != request.private
        || repository.owner.id != request.account.id
        || !repository
            .owner
            .login
            .eq_ignore_ascii_case(&request.account.login)
    {
        return Err("The created repository does not match the reviewed request.".into());
    }
    let html = format!(
        "https://github.com/{}/{}",
        request.account.login, repository.name
    );
    let clone = format!("{html}.git");
    if repository.html_url != html || repository.clone_url != clone {
        return Err("The repository address is not an expected GitHub address.".into());
    }
    Ok(CreatedRepository {
        account: request.account.clone(),
        name: repository.name,
        html_url: repository.html_url,
        clone_url: repository.clone_url,
        snapshot: Err("The repository was created but has not been connected yet.".into()),
        workspace: request.expected.workspace.clone(),
        root: request.expected.root.clone(),
        confirmation: Confirmation::Confirmed(repository.id),
        private: repository.private,
    })
}

fn check_expected(workspace: &Path, expected: &setup::SetupSnapshot) -> Result<(), String> {
    if setup::inspect(workspace)? != *expected {
        return Err(
            "The workspace or Git configuration changed. Review it before continuing.".into(),
        );
    }
    Ok(())
}

pub(crate) fn attach_repository(
    workspace: &Path,
    expected: setup::SetupSnapshot,
    created: &CreatedRepository,
    cancel: &AtomicBool,
) -> Result<AttachedRepository, String> {
    let tool = prepare(cancel, &mut |_| {})?;
    with_profile(&tool, || {
        attach_with_tool(workspace, expected, created, &tool, cancel)
    })
}

fn attach_with_tool(
    workspace: &Path,
    expected: setup::SetupSnapshot,
    created: &CreatedRepository,
    tool: &Prepared,
    cancel: &AtomicBool,
) -> Result<AttachedRepository, String> {
    check_cancel(cancel)?;
    if expected.workspace != created.workspace
        || expected.root != created.root
        || (!expected.origin.is_empty() && expected.origin != created.clone_url)
    {
        return Err("The repository was created for another workspace, or this workspace already has a remote. Review Git setup before connecting it.".into());
    }
    let current =
        account(tool, cancel)?.ok_or("Sign in to GitHub before connecting this repository.")?;
    if current != created.account {
        return Err(
            "The GitHub account changed. Sign in to the account that created this repository."
                .into(),
        );
    }
    check_expected(workspace, &expected)?;
    let confirmed = verify_repository(tool, &expected, created, cancel)?
        .ok_or("GitHub has not confirmed this repository. Check its page before reviewing a new creation request.")?;
    write_record(&tool.config_dir, &as_record(&confirmed), false)?;
    check_cancel(cancel)?;
    let request = setup::SetupRequest {
        author_name: expected.author_name.clone(),
        author_email: expected.author_email.clone(),
        initial_branch: expected.branch.clone(),
        expected,
        origin: confirmed.clone_url.clone(),
    };
    let scope = format!(
        "https://github.com/{}/{}.git",
        created.account.login, created.name
    );
    let snapshot = setup::connect_github(
        workspace,
        request,
        setup::GitHubCredential {
            scope,
            username: created.account.login.clone(),
            helper: credential_helper(tool)?,
        },
    )?;
    Ok(AttachedRepository {
        repository: confirmed,
        snapshot,
    })
}

fn verify_repository(
    tool: &Prepared,
    expected: &setup::SetupSnapshot,
    created: &CreatedRepository,
    cancel: &AtomicBool,
) -> Result<Option<CreatedRepository>, String> {
    let route = format!("repos/{}/{}", created.account.login, created.name);
    let output = run(
        tool,
        &["api", "--hostname", HOST, "--method", "GET", &route],
        cancel,
        Duration::from_secs(60),
        |_| {},
    )?;
    reject_plaintext_profile(tool)?;
    if !output.success {
        return if output.http_status == Some(404) {
            Ok(None)
        } else {
            Err("GitHub could not check the previously requested repository. No new creation request was sent.".into())
        };
    }
    let request = CreateRequest {
        expected: expected.clone(),
        account: created.account.clone(),
        name: created.name.clone(),
        private: created.private,
    };
    let confirmed = parse_created(&request, &output.stdout)?;
    if created
        .repository_id()
        .is_some_and(|id| Some(id) != confirmed.repository_id())
    {
        return Err("The GitHub repository was replaced after this creation request. Review its address before connecting.".into());
    }
    Ok(Some(confirmed))
}

pub(crate) fn forget_missing_creation(
    workspace: &Path,
    created: &CreatedRepository,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let tool = prepare(cancel, &mut |_| {})?;
    with_profile(&tool, || {
        let expected = setup::inspect(workspace)?;
        if expected.workspace != created.workspace
            || expected.root != created.root
            || !expected.origin.is_empty()
        {
            return Err("Review the original workspace with no remote before reviewing a new creation request.".into());
        }
        let current = account(&tool, cancel)?
            .ok_or("Sign in to the original GitHub account before checking this request.")?;
        if current != created.account {
            return Err(
                "Sign in to the original GitHub account before checking this request.".into(),
            );
        }
        if verify_repository(&tool, &expected, created, cancel)?.is_some() {
            return Err("This repository exists on GitHub. Retry its connection, or use Existing remote to choose another destination.".into());
        }
        check_cancel(cancel)?;
        let path = record_path(&tool.config_dir, &as_record(created))?;
        let record = read_record(&path)?;
        if record.workspace != created.workspace
            || record.root != created.root
            || record.account != created.account
            || record.name != created.name
        {
            return Err(
                "The GitHub creation record changed. Reopen setup before reviewing it.".into(),
            );
        }
        fs::remove_file(path)
            .map_err(|_| "Could not clear the checked, missing GitHub creation request.".to_owned())
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn credential_helper(tool: &Prepared) -> Result<String, String> {
    let program = tool
        .program
        .to_str()
        .ok_or("The GitHub helper path cannot be used by Git.")?;
    let directory = tool
        .config_dir
        .to_str()
        .ok_or("The GitHub sign-in path cannot be used by Git.")?;
    if !tool.program.is_absolute()
        || !tool.config_dir.is_absolute()
        || program
            .chars()
            .chain(directory.chars())
            .any(char::is_control)
    {
        return Err("The GitHub helper paths are invalid.".into());
    }
    Ok(format!(
        "!unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN GH_DEBUG GH_CONFIG_DIR GH_HOST; GH_CONFIG_DIR={} GH_HOST=github.com GH_PROMPT_DISABLED=1 {} auth git-credential",
        shell_quote(directory),
        shell_quote(program)
    ))
}

fn allowed_verification_uri(value: &str, fixture: bool) -> Option<String> {
    if value == DEVICE_URL {
        return Some(value.into());
    }
    let url = url::Url::parse(value).ok()?;
    (fixture
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then(|| value.into())
}

fn device_progress(stderr: &[u8], fixture: bool) -> Option<(String, String)> {
    let stderr = std::str::from_utf8(stderr).ok()?;
    let code = stderr.lines().find_map(|line| {
        let code = line
            .split_once("First copy your one-time code:")?
            .1
            .split_whitespace()
            .next()?;
        let bytes = code.as_bytes();
        (bytes.len() == 9
            && bytes[4] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(i, b)| i == 4 || b.is_ascii_uppercase() || b.is_ascii_digit()))
        .then(|| code.to_owned())
    })?;
    let verification_uri = stderr.lines().find_map(|line| {
        line.split_whitespace()
            .find_map(|part| allowed_verification_uri(part, fixture))
    })?;
    Some((code, verification_uri))
}

struct Output {
    success: bool,
    stdout: Vec<u8>,
    http_status: Option<u16>,
}

fn run(
    tool: &Prepared,
    arguments: &[&str],
    cancel: &AtomicBool,
    timeout: Duration,
    mut progress: impl FnMut(&[u8]),
) -> Result<Output, String> {
    check_cancel(cancel)?;
    let stdout = tempfile::NamedTempFile::new_in(&tool.config_dir)
        .map_err(|_| "Could not prepare the GitHub helper output.")?;
    let stderr = tempfile::NamedTempFile::new_in(&tool.config_dir)
        .map_err(|_| "Could not prepare the GitHub helper output.")?;
    let mut reader = stderr
        .reopen()
        .map_err(|_| "Could not read GitHub sign-in progress.")?;
    let mut seen = Vec::new();
    let mut command = Command::new(&tool.program);
    for variable in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
        "GH_DEBUG",
        "GH_FORCE_TTY",
        "GH_BROWSER",
        "BROWSER",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_CONFIG_COUNT",
    ] {
        command.env_remove(variable);
    }
    command
        .args(arguments)
        .current_dir(&tool.config_dir)
        .env("GH_CONFIG_DIR", &tool.config_dir)
        .env("GH_HOST", HOST)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(
            stdout
                .reopen()
                .map_err(|_| "Could not prepare GitHub output.")?,
        )
        .stderr(
            stderr
                .reopen()
                .map_err(|_| "Could not prepare GitHub output.")?,
        );
    let child = OwnedChild::spawn(&mut command)
        .map_err(|_| "Could not start the GitHub helper. Try signing in again.")?;
    let status = child
        .wait(timeout, || {
            if cancel.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            let length = stderr.as_file().metadata()?.len();
            if length > STDERR_LIMIT || stdout.as_file().metadata()?.len() > OUTPUT_LIMIT {
                return Err(std::io::Error::other("GitHub output limit"));
            }
            if length > seen.len() as u64 {
                Read::by_ref(&mut reader)
                    .take(STDERR_LIMIT + 1 - seen.len() as u64)
                    .read_to_end(&mut seen)?;
                progress(&seen);
            }
            Ok(())
        })
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::Interrupted {
                "The GitHub operation was cancelled.".to_owned()
            } else {
                "The GitHub helper could not finish the request. Try again.".to_owned()
            }
        })?;
    let mut bytes = Vec::new();
    stdout
        .reopen()
        .map_err(|_| "Could not read GitHub output.")?
        .take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read GitHub output.")?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Err("The GitHub helper returned too much data.".into());
    }
    let mut errors = Vec::new();
    stderr
        .reopen()
        .map_err(|_| "Could not inspect GitHub request status.")?
        .take(STDERR_LIMIT + 1)
        .read_to_end(&mut errors)
        .map_err(|_| "Could not inspect GitHub request status.")?;
    if errors.len() as u64 > STDERR_LIMIT {
        return Err("The GitHub helper returned too much data.".into());
    }
    let http_status = errors.windows(10).find_map(|window| {
        (window.starts_with(b"(HTTP ") && window[9] == b')')
            .then(|| std::str::from_utf8(&window[6..9]).ok()?.parse::<u16>().ok())
            .flatten()
    });
    Ok(Output {
        success: status.success(),
        stdout: bytes,
        http_status,
    })
}

/// The pinned helper emits block YAML. Strip only token-valued fields from our
/// isolated profile; never invoke logout or delete shared keyring records.
fn reject_plaintext_profile(tool: &Prepared) -> Result<(), String> {
    let path = tool.config_dir.join("hosts.yml");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("The GitHub sign-in profile is unavailable.".into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > OUTPUT_LIMIT {
        return Err("The GitHub sign-in profile is invalid. Reopen GitHub setup.".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut bytes = Vec::new();
    options
        .open(&path)
        .map_err(|_| "The GitHub sign-in profile is unavailable.")?
        .take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The GitHub sign-in profile is unavailable.")?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Err("The GitHub sign-in profile is too large.".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "The GitHub sign-in profile is invalid.")?;
    let mut clean = String::new();
    let mut stripped = false;
    let mut skip_indent = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(parent) = skip_indent {
            if trimmed.trim().is_empty() || indent > parent {
                continue;
            }
            skip_indent = None;
        }
        if trimmed
            .split_once(':')
            .is_some_and(|(key, _)| key.trim_matches(['\'', '"']) == "oauth_token")
        {
            stripped = true;
            skip_indent = Some(indent);
            continue;
        }
        clean.push_str(line);
    }
    if stripped {
        let mut staged = tempfile::NamedTempFile::new_in(&tool.config_dir)
            .map_err(|_| "Could not remove an insecure GitHub sign-in fallback.")?;
        staged
            .write_all(clean.as_bytes())
            .and_then(|_| staged.as_file().sync_all())
            .map_err(|_| "Could not remove an insecure GitHub sign-in fallback.")?;
        staged
            .persist(&path)
            .map_err(|_| "Could not remove an insecure GitHub sign-in fallback.")?;
        return Err("GitHub could not use the system's secure credential store. Its plaintext fallback was removed. Enable secure storage, then sign in again.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
