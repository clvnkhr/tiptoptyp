//! On-demand, verified official GitHub CLI. Credentials belong to gh's secure
//! store; its app-owned configuration and executable stay outside workspaces.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const LICENSE: &[u8] = include_bytes!("../resources/github-cli/LICENSE");
const MAX_ENTRIES: usize = 4096;
const MAX_EXPANDED: u64 = 192 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    pub(crate) program: PathBuf,
    pub(crate) config_dir: PathBuf,
    fixture: bool,
}
impl Prepared {
    pub(crate) fn is_test_fixture(&self) -> bool {
        self.fixture && cfg!(any(test, feature = "desktop-ui-tests"))
    }

    #[cfg(test)]
    pub(crate) fn for_test(program: PathBuf, config_dir: PathBuf) -> Result<Self, String> {
        private_directory(&config_dir)?;
        Ok(Self {
            program,
            config_dir,
            fixture: true,
        })
    }
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    license_sha256: String,
    assets: BTreeMap<String, Asset>,
}
#[derive(Clone, Deserialize)]
struct Asset {
    url: String,
    bytes: u64,
    sha256: String,
    entry: String,
    binary_bytes: u64,
    binary_sha256: String,
    zip: bool,
}
fn manifest() -> Result<Manifest, String> {
    let manifest: Manifest =
        serde_json::from_str(include_str!("../resources/github-cli/manifest.json"))
            .map_err(|_| "The GitHub helper catalog is invalid. Reinstall the app.")?;
    if manifest.license_sha256 != hex::encode(Sha256::digest(LICENSE)) {
        return Err("The GitHub helper license catalog is invalid. Reinstall the app.".into());
    }
    Ok(manifest)
}

fn application_directory() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|path| PathBuf::from(path).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|path| PathBuf::from(path).join(".local/share")));
    let base = base
        .filter(|path| path.is_absolute())
        .ok_or("The application data folder is unavailable for GitHub sign-in.")?;
    fs::create_dir_all(&base).map_err(error)?;
    let mut path = base.canonicalize().map_err(error)?;
    for name in ["dev.tiptoptyp.editor", "github"] {
        path.push(name);
        private_directory(&path)?;
    }
    Ok(path)
}

pub(crate) fn config_directory() -> Result<PathBuf, String> {
    let path = if let Some(root) = desktop_fixture_directory()? {
        root.join("github-auth")
    } else {
        application_directory()?.join("auth")
    };
    private_directory(&path)?;
    Ok(path)
}

/// No download or authentication at startup. Call only from an explicit worker.
pub(crate) fn prepare(
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<Prepared, String> {
    check_cancel(cancel)?;
    if let Some(prepared) = desktop_fixture()? {
        return Ok(prepared);
    }
    let catalog = manifest()?;
    let target = crate::toolchain::target_triple();
    let asset = catalog
        .assets
        .get(target)
        .ok_or("GitHub sign-in is unavailable on this system architecture.")?;
    let mut root = application_directory()?;
    for name in ["cli", &catalog.version, target] {
        root.push(name);
        private_directory(&root)?;
    }
    let config_dir = config_directory()?;
    crate::resource_lock::try_with_resource(&root, || {
        prepare_at(&root, &config_dir, asset, cancel, progress).map_err(io::Error::other)
    })
    .map_err(|failure| {
        if failure.kind() == io::ErrorKind::WouldBlock {
            "The GitHub helper is being prepared in another window. Try again when it finishes."
                .into()
        } else {
            failure.to_string()
        }
    })
}

fn prepare_at(
    root: &Path,
    config_dir: &Path,
    asset: &Asset,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<Prepared, String> {
    let program = root.join(if cfg!(windows) { "gh.exe" } else { "gh" });
    if !verify_file(&program, asset.binary_bytes, &asset.binary_sha256, cancel)? {
        progress("Downloading GitHub sign-in helper…");
        let mut archive = tempfile::NamedTempFile::new_in(root).map_err(error)?;
        download(asset, archive.path(), cancel)?;
        if !verify_file(archive.path(), asset.bytes, &asset.sha256, cancel)? {
            return Err("The GitHub helper download did not match its verified release.".into());
        }
        progress("Verifying GitHub sign-in helper…");
        archive.as_file_mut().rewind().map_err(error)?;
        let mut staged = tempfile::NamedTempFile::new_in(root).map_err(error)?;
        extract(archive.as_file_mut(), staged.as_file_mut(), asset, cancel)?;
        staged.as_file().sync_all().map_err(error)?;
        executable_permissions(staged.as_file())?;
        staged.persist(&program).map_err(|e| error(e.error))?;
    }
    let file = open_regular(&program)?;
    executable_permissions(&file)?;
    let license_path = root.join("LICENSE");
    let license_digest = hex::encode(Sha256::digest(LICENSE));
    if !verify_file(&license_path, LICENSE.len() as u64, &license_digest, cancel)? {
        let mut license = tempfile::NamedTempFile::new_in(root).map_err(error)?;
        license
            .write_all(LICENSE)
            .and_then(|_| license.as_file().sync_all())
            .map_err(error)?;
        license.persist(&license_path).map_err(|e| error(e.error))?;
    }
    check_cancel(cancel)?;
    Ok(Prepared {
        program,
        config_dir: config_dir.to_owned(),
        fixture: false,
    })
}

fn download(asset: &Asset, output: &Path, cancel: &AtomicBool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let curl = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join("System32/curl.exe"));
    #[cfg(not(target_os = "windows"))]
    let curl = ["/usr/bin/curl", "/bin/curl"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file());
    let curl = curl
        .filter(|path| path.is_file())
        .ok_or("The system download tool is unavailable for GitHub setup.")?;
    let mut command = Command::new(curl);
    command
        .args([
            "--disable",
            "--fail",
            "--location",
            "--max-redirs",
            "5",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            "300",
            "--silent",
            "--max-filesize",
            &asset.bytes.to_string(),
            "--output",
        ])
        .arg(output)
        .args(["--url", &asset.url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = crate::process::OwnedChild::spawn(&mut command).map_err(error)?;
    let status = child
        .wait(Duration::from_secs(310), || {
            if cancel.load(Ordering::Acquire) {
                return Err(io::Error::other("GitHub helper preparation cancelled."));
            }
            if fs::metadata(output)?.len() > asset.bytes {
                return Err(io::Error::other(
                    "The GitHub helper download exceeds its size limit.",
                ));
            }
            Ok(())
        })
        .map_err(error)?;
    if !status.success() {
        return Err(
            "Could not download the GitHub sign-in helper. Check your connection and try again."
                .into(),
        );
    }
    check_cancel(cancel)
}

fn extract(
    input: &mut File,
    output: &mut File,
    asset: &Asset,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if asset.zip {
        let mut archive = zip::ZipArchive::new(input).map_err(error)?;
        if archive.len() > MAX_ENTRIES {
            return Err("The GitHub helper archive has too many entries.".into());
        }
        let mut found = None;
        for index in 0..archive.len() {
            check_cancel(cancel)?;
            let entry = archive.by_index(index).map_err(error)?;
            if entry.name().map_err(error)? == asset.entry
                && (found.replace(index).is_some()
                    || entry.size() != asset.binary_bytes
                    || entry
                        .unix_mode()
                        .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000))
            {
                return Err("The GitHub helper entry is duplicated or invalid.".into());
            }
        }
        let mut entry = archive
            .by_index(found.ok_or("The GitHub helper is missing from its archive.")?)
            .map_err(error)?;
        copy_verified(
            &mut entry,
            output,
            asset.binary_bytes,
            &asset.binary_sha256,
            cancel,
        )
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(input));
        let mut total = 0u64;
        let mut found = false;
        for (index, entry) in archive.entries().map_err(error)?.enumerate() {
            check_cancel(cancel)?;
            let mut entry = entry.map_err(error)?;
            total = total
                .checked_add(entry.size())
                .ok_or("The GitHub helper archive is too large.")?;
            if index >= MAX_ENTRIES || total > MAX_EXPANDED {
                return Err("The GitHub helper archive is too large.".into());
            }
            if entry.path().map_err(error)?.as_ref() == Path::new(&asset.entry) {
                if found
                    || !entry.header().entry_type().is_file()
                    || entry.size() != asset.binary_bytes
                {
                    return Err("The GitHub helper entry is duplicated or invalid.".into());
                }
                copy_verified(
                    &mut entry,
                    output,
                    asset.binary_bytes,
                    &asset.binary_sha256,
                    cancel,
                )?;
                found = true;
            }
        }
        if found {
            Ok(())
        } else {
            Err("The GitHub helper is missing from its archive.".into())
        }
    }
}

fn copy_verified(
    input: &mut impl Read,
    output: &mut impl Write,
    bytes: u64,
    expected: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut remaining = bytes;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        check_cancel(cancel)?;
        let capacity = remaining.min(buffer.len() as u64) as usize;
        let count = input.read(&mut buffer[..capacity]).map_err(error)?;
        if count == 0 {
            return Err("The GitHub helper was truncated.".into());
        }
        output.write_all(&buffer[..count]).map_err(error)?;
        digest.update(&buffer[..count]);
        remaining -= count as u64;
    }
    let mut extra = [0];
    if input.read(&mut extra).map_err(error)? != 0 || hex::encode(digest.finalize()) != expected {
        return Err("The GitHub helper does not match its verified release.".into());
    }
    check_cancel(cancel)
}

fn verify_file(path: &Path, bytes: u64, digest: &str, cancel: &AtomicBool) -> Result<bool, String> {
    let mut file = match open_regular(path) {
        Ok(file) => file,
        Err(error) if !path.try_exists().unwrap_or(true) => {
            let _ = error;
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    if file.metadata().map_err(error)?.len() != bytes {
        return Ok(false);
    }
    let mut sink = io::sink();
    match copy_verified(&mut file, &mut sink, bytes, digest, cancel) {
        Ok(()) => Ok(true),
        Err(failure) => {
            check_cancel(cancel)?;
            let _ = failure;
            Ok(false)
        }
    }
}

fn open_regular(path: &Path) -> Result<File, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("The GitHub helper must be a regular file.".into());
    }
    let file = options.open(path).map_err(error)?;
    if !file.metadata().map_err(error)?.is_file() {
        return Err("The GitHub helper must be a regular file.".into());
    }
    Ok(file)
}

fn private_directory(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(error(e)),
    }
    let metadata = fs::symlink_metadata(path).map_err(error)?;
    if !metadata.file_type().is_dir() {
        return Err("The GitHub data folder must be a regular directory.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != nix::unistd::geteuid().as_raw() {
            return Err("The GitHub data folder belongs to another user.".into());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(error)?;
    }
    Ok(())
}
fn executable_permissions(file: &File) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(error)?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}
fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("GitHub helper preparation cancelled.".into())
    } else {
        Ok(())
    }
}
fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn desktop_fixture_directory() -> Result<Option<PathBuf>, String> {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if crate::desktop_test::enabled() {
        let root = PathBuf::from(
            std::env::var_os("TIPTOPTYP_DESKTOP_TEST_DIR")
                .ok_or("Missing GitHub desktop fixture directory.")?,
        );
        return root.canonicalize().map(Some).map_err(error);
    }
    Ok(None)
}

fn desktop_fixture() -> Result<Option<Prepared>, String> {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if let Some(root) = desktop_fixture_directory()? {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let directory = root.join("github-helper");
        let metadata = fs::symlink_metadata(&directory)
            .map_err(|_| "The GitHub desktop check requires its private helper fixture.")?;
        if !metadata.file_type().is_dir()
            || metadata.uid() != nix::unistd::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err("The GitHub desktop helper directory must be private and owned.".into());
        }
        let program = directory.join("gh");
        let file = open_regular(&program)?;
        let metadata = file.metadata().map_err(error)?;
        if metadata.uid() != nix::unistd::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || metadata.permissions().mode() & 0o100 == 0
        {
            return Err("The GitHub desktop helper must be private, owned and executable.".into());
        }
        let config_dir = root.join("github-auth");
        private_directory(&config_dir)?;
        return Ok(Some(Prepared {
            program,
            config_dir,
            fixture: true,
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
