//! Worker-only preparation of pinned LTeX+ and its matching Java runtime.
//! The signed application is immutable. Repairs publish a new private generation,
//! leaving files used by existing Java processes untouched.
use crate::bundled_payload::codec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};

#[path = "../build_support/ltex_manifest.rs"]
pub(crate) mod manifest;

const MAX_FILES: usize = 4096;
const MAX_STATE_BYTES: u64 = 1024 * 1024;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Prepared {
    pub(crate) program: PathBuf,
    pub(crate) java_home: PathBuf,
}
impl Prepared {
    fn at(root: &Path) -> Self {
        Self {
            program: root.join(manifest::LAUNCHER),
            java_home: root.join(manifest::JDK_DIRECTORY),
        }
    }
}

pub(crate) fn prepare() -> Result<Prepared, String> {
    prepare_with_cancel(&AtomicBool::new(false))
}

pub(crate) fn prepare_with_cancel(cancel: &AtomicBool) -> Result<Prepared, String> {
    prepare_at(&packaged_directory()?, &cache_directory()?, cancel, false)
}

/// Fetch only the running architecture's pinned official distribution. Never
/// deletes the previous generation, even when a server is currently using it.
pub(crate) fn redownload(cancel: &AtomicBool) -> Result<Prepared, String> {
    let source = packaged_directory()?;
    let (catalog, catalog_hash) = read_catalog(&source)?;
    let target = crate::toolchain::target_triple();
    if !catalog.has_target(target) {
        return Err("The packaged LTeX+ does not support this Mac architecture.".into());
    }
    let upstream = manifest::upstream(target)
        .ok_or("Bundled LTeX+ currently supports Intel and Apple Silicon Macs.")?;
    let root = cache_directory()?;
    let _lock = wait_for_setup(&root, cancel)?;
    check_cancel(cancel)?;
    let mut archive = tempfile::NamedTempFile::new_in(&root).map_err(error)?;
    download(&upstream, cancel, &mut archive)?;
    archive.as_file_mut().rewind().map_err(error)?;
    activate_verified_download(archive.as_file_mut(), &root, &catalog_hash, target, cancel)
}

fn activate_verified_download(
    archive: impl Read,
    root: &Path,
    catalog: &str,
    target: &str,
    cancel: &AtomicBool,
) -> Result<Prepared, String> {
    check_cancel(cancel)?;
    // Download verification has already established the pinned version. An
    // unchanged healthy cache contains that same version, so another identical
    // expanded generation would only consume disk space. Damaged files still
    // receive a fresh generation while existing sessions retain the old one.
    if let Some(prepared) = cached(root, catalog, target) {
        check_cancel(cancel)?;
        return Ok(prepared);
    }
    let stage = new_generation(root)?;
    let reader = flate2::read::GzDecoder::new(archive);
    extract(reader, stage.path(), ArchiveKind::Official, cancel)?;
    publish(stage, root, catalog, target, cancel)
}

fn packaged_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(error)?;
    let candidates = crate::bundled_payload::resource_roots(&executable)
        .into_iter()
        .map(|root| root.join("ltex"));
    #[cfg(not(feature = "production"))]
    let candidates = candidates.chain(std::iter::once(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("toolchain/ltex"),
    ));
    candidates
        .into_iter()
        .find(|root| root.join(manifest::MANIFEST_NAME).is_file())
        .ok_or_else(|| "LTeX+ is missing from this app package. Reinstall the complete app.".into())
}

fn cache_directory() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(not(target_os = "macos"))]
    let base: Option<PathBuf> = None;
    let base = base
        .filter(|root| root.is_absolute())
        .ok_or("Bundled LTeX+ currently supports Intel and Apple Silicon Macs.")?;
    fs::create_dir_all(&base).map_err(error)?;
    let mut root = base.canonicalize().map_err(error)?;
    for component in [
        "dev.tiptoptyp.editor",
        "ltex",
        manifest::VERSION,
        crate::toolchain::target_triple(),
    ] {
        root.push(component);
        private_directory(&root)?;
    }
    Ok(root)
}

fn prepare_at(
    source: &Path,
    root: &Path,
    cancel: &AtomicBool,
    force: bool,
) -> Result<Prepared, String> {
    check_cancel(cancel)?;
    let (catalog, catalog_hash) = read_catalog(source)?;
    let target = crate::toolchain::target_triple();
    if !catalog.has_target(target) {
        return Err("The packaged LTeX+ does not support this Mac architecture.".into());
    }
    private_directory(root)?;
    if !force && let Some(prepared) = cached(root, &catalog_hash, target) {
        return Ok(prepared);
    }
    let _lock = wait_for_setup(root, cancel)?;
    if !force && let Some(prepared) = cached(root, &catalog_hash, target) {
        return Ok(prepared);
    }
    let stage = new_generation(root)?;
    for asset in catalog.files_for_target(target) {
        check_cancel(cancel)?;
        let archive = source.join(asset.archive_name());
        verify_file(
            &archive,
            asset.compressed_bytes,
            &asset.compressed_sha256,
            cancel,
        )?;
        let mut decoded = tempfile::NamedTempFile::new_in(root).map_err(error)?;
        codec::decode_bounded(
            CancelReader {
                inner: File::open(&archive).map_err(error)?,
                cancel,
            },
            asset.bytes,
            &asset.sha256,
            &mut decoded,
            manifest::MAX_ARCHIVE_BYTES,
        )
        .map_err(error)?;
        decoded.as_file_mut().rewind().map_err(error)?;
        extract(
            decoded.as_file_mut(),
            stage.path(),
            if asset.target.is_some() {
                ArchiveKind::Native
            } else {
                ArchiveKind::Shared
            },
            cancel,
        )?;
    }
    publish(stage, root, &catalog_hash, target, cancel)
}

use std::io::Seek;

fn read_catalog(source: &Path) -> Result<(manifest::Manifest, String), String> {
    let bytes = read_regular(
        &source.join(manifest::MANIFEST_NAME),
        manifest::MAX_MANIFEST_BYTES,
    )?;
    Ok((
        manifest::Manifest::parse(&bytes)?,
        format!("{:x}", Sha256::digest(&bytes)),
    ))
}
fn read_regular(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(error)?;
    if !metadata.file_type().is_file() || metadata.len() > maximum {
        return Err("LTeX+ resource has an invalid size or file type.".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(error)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > maximum {
        return Err("LTeX+ resource is too large.".into());
    }
    Ok(bytes)
}
fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("LTeX+ setup cancelled.".into())
    } else {
        Ok(())
    }
}
fn error(error: impl std::fmt::Display) -> String {
    format!("LTeX+ setup: {error}")
}
struct CancelReader<'a, R> {
    inner: R,
    cancel: &'a AtomicBool,
}
impl<R: Read> Read for CancelReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Acquire) {
            return Err(io::Error::other("LTeX+ setup cancelled"));
        }
        self.inner.read(output)
    }
}
fn verify_file(path: &Path, bytes: u64, hash: &str, cancel: &AtomicBool) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(error)?;
    if !metadata.file_type().is_file() || metadata.len() != bytes {
        return Err("LTeX+ archive has an invalid size or file type.".into());
    }
    let mut reader = CancelReader {
        inner: File::open(path).map_err(error)?.take(bytes + 1),
        cancel,
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut actual = 0u64;
    loop {
        let count = reader.read(&mut buffer).map_err(error)?;
        if count == 0 {
            break;
        }
        actual += count as u64;
        hasher.update(&buffer[..count]);
    }
    if actual != bytes || format!("{:x}", hasher.finalize()) != hash {
        return Err("LTeX+ archive failed its integrity check.".into());
    }
    Ok(())
}
fn download(
    upstream: &manifest::Upstream,
    cancel: &AtomicBool,
    output: &mut impl Write,
) -> Result<(), String> {
    // The platform downloader permits slow complete transfers while the owning
    // worker can cancel even during a blocked network read. Disable curlrc so a
    // user's shell configuration cannot change this pinned operation's options.
    let mut command = Command::new("/usr/bin/curl");
    command.args([
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
        "900",
        "--speed-limit",
        "1024",
        "--speed-time",
        "30",
        "--silent",
        "--url",
        &upstream.url(),
    ]);
    download_command(
        &mut command,
        upstream.bytes,
        upstream.sha256,
        cancel,
        output,
    )
}
fn download_command(
    command: &mut Command,
    bytes: u64,
    hash: &str,
    cancel: &AtomicBool,
    output: &mut impl Write,
) -> Result<(), String> {
    check_cancel(cancel)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = crate::process::OwnedChild::spawn(command).map_err(error)?;
    let mut pipe = child
        .child_mut()
        .stdout
        .take()
        .ok_or("LTeX+ download has no output stream.")?;
    let (sender, receiver) = mpsc::sync_channel::<Result<Vec<u8>, String>>(2);
    let reader = thread::Builder::new()
        .name("ltex-download-stream".into())
        .spawn(move || {
            loop {
                let mut chunk = vec![0; 64 * 1024];
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        chunk.truncate(count);
                        if sender.send(Ok(chunk)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(format!("LTeX+ download stream: {error}")));
                        break;
                    }
                }
            }
        })
        .map_err(error)?;
    let result = (|| {
        let deadline = Instant::now() + Duration::from_secs(15 * 60);
        let mut hasher = Sha256::new();
        let mut count = 0u64;
        loop {
            check_cancel(cancel)?;
            if Instant::now() >= deadline {
                return Err("LTeX+ download timed out.".into());
            }
            match receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(chunk) => {
                    let chunk = chunk?;
                    count = count
                        .checked_add(chunk.len() as u64)
                        .ok_or("LTeX+ download is too large.")?;
                    if count > bytes {
                        return Err("LTeX+ download exceeds its pinned size.".into());
                    }
                    hasher.update(&chunk);
                    output.write_all(&chunk).map_err(error)?;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        loop {
            check_cancel(cancel)?;
            if let Some(status) = child.try_wait().map_err(error)? {
                if !status.success() {
                    return Err(format!(
                        "LTeX+ download failed ({status}). Check the connection and retry."
                    ));
                }
                break;
            }
            if Instant::now() >= deadline {
                return Err("LTeX+ download timed out.".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        if count != bytes || format!("{:x}", hasher.finalize()) != hash {
            return Err("LTeX+ download failed its size or SHA-256 check.".into());
        }
        output.flush().map_err(error)
    })();
    // Close the bounded consumer before killing/reaping the downloader. Both a
    // blocked pipe read and a blocked channel send can then finish on every exit.
    drop(receiver);
    drop(child);
    crate::process::finish_reader_with_timeout(reader, Duration::from_millis(100));
    result
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
        return Err("LTeX+ data folder must be a regular directory.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != nix::unistd::geteuid().as_raw() {
            return Err("LTeX+ data folder belongs to another user.".into());
        }
        if metadata.mode() & 0o777 != 0o700 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(error)?;
        }
    }
    Ok(())
}
struct SetupLock(File);
impl Drop for SetupLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
fn try_setup_lock(root: &Path) -> Result<Option<SetupLock>, String> {
    let path = root.join(".setup.lock");
    if fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.file_type().is_file()) {
        return Err("LTeX+ setup lock must be a regular file.".into());
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(error)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(SetupLock(file))),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(failure)) => Err(error(failure)),
    }
}
fn wait_for_setup(root: &Path, cancel: &AtomicBool) -> Result<SetupLock, String> {
    // Separate windows may reach their first check together. Waiting belongs to
    // their workers, and they recheck the completed generation after taking the
    // lock rather than treating another window's normal setup as a fatal error.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        check_cancel(cancel)?;
        if let Some(lock) = try_setup_lock(root)? {
            return Ok(lock);
        }
        if Instant::now() >= deadline {
            return Err(
                "LTeX+ setup is still running in another window. Try again shortly.".into(),
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn new_generation(root: &Path) -> Result<tempfile::TempDir, String> {
    tempfile::Builder::new()
        .prefix("generation-")
        .tempdir_in(root)
        .map_err(error)
}

#[derive(Clone, Copy)]
enum ArchiveKind {
    Shared,
    Native,
    Official,
}
fn relative(path: &Path) -> Result<PathBuf, String> {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(value) => result.push(value),
            Component::CurDir => {}
            _ => return Err("LTeX+ archive contains an unsafe path.".into()),
        }
    }
    if result.to_str().is_none() {
        return Err("LTeX+ archive contains a non-Unicode path.".into());
    }
    Ok(result)
}
fn member(path: &Path, kind: ArchiveKind) -> Result<PathBuf, String> {
    let path = relative(path)?;
    let path = if matches!(kind, ArchiveKind::Official) {
        if path.as_os_str().is_empty() {
            return Ok(path);
        }
        path.strip_prefix(format!("ltex-ls-plus-{}", manifest::VERSION))
            .map_err(|_| "LTeX+ archive has an unexpected root.".to_owned())?
            .to_owned()
    } else {
        path
    };
    if path.as_os_str().is_empty() {
        return Ok(path);
    }
    let top = path.components().next().unwrap().as_os_str();
    let native = top == manifest::JDK_DIRECTORY;
    let shared = matches!(
        top.to_str(),
        Some("bin" | "lib" | "LICENSE.md" | "ACKNOWLEDGMENTS.md" | "README.md" | "changelog.xml")
    );
    let allowed = match kind {
        ArchiveKind::Shared => shared,
        ArchiveKind::Native => native,
        ArchiveKind::Official => native || shared,
    };
    if !allowed {
        return Err("LTeX+ archive contains an unexpected file.".into());
    }
    Ok(path)
}
fn parents(root: &Path, relative: &Path) -> Result<(), String> {
    let mut directory = root.to_owned();
    for part in relative.components() {
        directory.push(part);
        private_directory(&directory)?;
    }
    Ok(())
}
fn link_target(path: &Path, target: &Path) -> Result<PathBuf, String> {
    if target.is_absolute() {
        return Err("LTeX+ archive contains an absolute symbolic link.".into());
    }
    let mut resolved = path.parent().unwrap_or(Path::new("")).to_owned();
    for part in target.components() {
        match part {
            Component::Normal(value) => resolved.push(value),
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return Err("LTeX+ symbolic link escapes its Java runtime.".into());
                }
            }
            _ => return Err("LTeX+ symbolic link is invalid.".into()),
        }
    }
    if !path.starts_with(manifest::JDK_DIRECTORY) || !resolved.starts_with(manifest::JDK_DIRECTORY)
    {
        return Err("LTeX+ symbolic link escapes its Java runtime.".into());
    }
    Ok(resolved)
}
fn extract(
    reader: impl Read,
    root: &Path,
    kind: ArchiveKind,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut archive = tar::Archive::new(CancelReader {
        inner: reader.take(manifest::MAX_ARCHIVE_BYTES + 1),
        cancel,
    });
    let mut seen = BTreeSet::new();
    let mut links = Vec::new();
    let mut total = 0u64;
    for (index, entry) in archive.entries().map_err(error)?.enumerate() {
        check_cancel(cancel)?;
        if index >= MAX_FILES {
            return Err("LTeX+ archive contains too many files.".into());
        }
        let mut entry = entry.map_err(error)?;
        let path = member(&entry.path().map_err(error)?, kind)?;
        if path.as_os_str().is_empty() {
            if !entry.header().entry_type().is_dir() {
                return Err("LTeX+ archive root is invalid.".into());
            }
            continue;
        }
        if !seen.insert(path.clone()) {
            return Err("LTeX+ archive contains duplicate paths.".into());
        }
        let destination = root.join(&path);
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            parents(root, &path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            parents(root, parent)?;
        }
        if entry_type.is_symlink() {
            let target = entry
                .link_name()
                .map_err(error)?
                .ok_or("LTeX+ symbolic link has no target.")?
                .into_owned();
            link_target(&path, &target)?;
            links.push((path, target));
            continue;
        }
        if !entry_type.is_file() {
            return Err("LTeX+ archive contains an unsupported special file.".into());
        }
        let bytes = entry.size();
        total = total
            .checked_add(bytes)
            .ok_or("LTeX+ archive is too large.")?;
        if bytes > MAX_FILE_BYTES || total > manifest::MAX_ARCHIVE_BYTES {
            return Err("LTeX+ archive is too large.".into());
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(error)?;
        let count = io::copy(&mut entry, &mut output).map_err(error)?;
        if count != bytes {
            return Err("LTeX+ archive contains a truncated file.".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = entry.header().mode().map_err(error)? & 0o111 != 0;
            output
                .set_permissions(fs::Permissions::from_mode(if executable {
                    0o700
                } else {
                    0o600
                }))
                .map_err(error)?;
        }
    }
    // Never extract through a symlink. Only now are contained JDK licence links
    // materialised, after every regular file and parent directory is complete.
    for (path, target) in &links {
        check_cancel(cancel)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, root.join(path)).map_err(error)?;
        #[cfg(not(unix))]
        {
            let _ = (path, target);
            return Err("Bundled LTeX+ requires macOS.".into());
        }
    }
    let runtime = if links.is_empty() {
        None
    } else {
        Some(
            root.join(manifest::JDK_DIRECTORY)
                .canonicalize()
                .map_err(error)?,
        )
    };
    for (path, _) in links {
        let resolved = root.join(path).canonicalize().map_err(error)?;
        if !resolved.starts_with(runtime.as_ref().unwrap()) {
            return Err("LTeX+ symbolic link resolves outside its Java runtime.".into());
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Active {
    generation: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Complete {
    catalog: String,
    target: String,
    files: Vec<Stamp>,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Stamp {
    path: PathBuf,
    bytes: u64,
    modified: (u64, u32),
    #[cfg(unix)]
    native: (u64, u64, i64, i64, u32),
    link: Option<PathBuf>,
}
fn stamp(root: &Path, path: &Path) -> Result<Stamp, String> {
    let mut parent = root.to_owned();
    for part in path.parent().unwrap_or(Path::new("")).components() {
        parent.push(part);
        if !fs::symlink_metadata(&parent).is_ok_and(|metadata| metadata.file_type().is_dir()) {
            return Err("LTeX+ cache parent must be a regular directory.".into());
        }
    }
    let metadata = fs::symlink_metadata(root.join(path)).map_err(error)?;
    let modified = metadata
        .modified()
        .map_err(error)?
        .duration_since(UNIX_EPOCH)
        .map_err(error)?;
    if !metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
        return Err("LTeX+ cache has an invalid file type.".into());
    }
    let link = metadata
        .file_type()
        .is_symlink()
        .then(|| fs::read_link(root.join(path)))
        .transpose()
        .map_err(error)?;
    #[cfg(unix)]
    let native = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
            metadata.mode(),
        )
    };
    Ok(Stamp {
        path: path.to_owned(),
        bytes: metadata.len(),
        modified: (modified.as_secs(), modified.subsec_nanos()),
        #[cfg(unix)]
        native,
        link,
    })
}
fn inventory(root: &Path, directory: &Path, result: &mut Vec<Stamp>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(error)? {
        let entry = entry.map_err(error)?;
        let path = entry.path();
        if entry.file_type().map_err(error)?.is_dir() {
            inventory(root, &path, result)?;
        } else {
            result.push(stamp(root, path.strip_prefix(root).map_err(error)?)?);
        }
        if result.len() > MAX_FILES {
            return Err("LTeX+ cache contains too many files.".into());
        }
    }
    Ok(())
}
fn valid_program(prepared: &Prepared) -> bool {
    [
        prepared.program.clone(),
        prepared.java_home.join("bin/java"),
        prepared.java_home.join("lib/server/libjvm.dylib"),
    ]
    .iter()
    .all(|path| {
        fs::symlink_metadata(path).is_ok_and(|meta| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.file_type().is_file()
                    && meta.len() > 0
                    && (path.extension().is_some() || meta.permissions().mode() & 0o100 != 0)
            }
            #[cfg(not(unix))]
            {
                meta.file_type().is_file() && meta.len() > 0
            }
        })
    })
}
fn generation_name(name: &str) -> bool {
    name.starts_with("generation-")
        && name.len() < 128
        && name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_')
}
fn cached(root: &Path, catalog: &str, target: &str) -> Option<Prepared> {
    let active: Active =
        serde_json::from_slice(&read_regular(&root.join("active.json"), 4096).ok()?).ok()?;
    if !generation_name(&active.generation) {
        return None;
    }
    let directory = root.join(active.generation);
    if !fs::symlink_metadata(&directory).ok()?.file_type().is_dir() {
        return None;
    }
    let complete: Complete = serde_json::from_slice(
        &read_regular(&directory.join(".complete.json"), MAX_STATE_BYTES).ok()?,
    )
    .ok()?;
    if complete.catalog != catalog
        || complete.target != target
        || complete.files.is_empty()
        || complete.files.len() > MAX_FILES
    {
        return None;
    }
    for file in complete.files {
        if relative(&file.path).ok()? != file.path || stamp(&directory, &file.path).ok()? != file {
            return None;
        }
    }
    let prepared = Prepared::at(&directory);
    valid_program(&prepared).then_some(prepared)
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or("LTeX+ state path has no parent.")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(error)?;
    serde_json::to_writer(&mut file, value).map_err(error)?;
    file.flush().map_err(error)?;
    file.as_file().sync_all().map_err(error)?;
    file.persist(path).map_err(error)?;
    Ok(())
}
fn publish(
    stage: tempfile::TempDir,
    root: &Path,
    catalog: &str,
    target: &str,
    cancel: &AtomicBool,
) -> Result<Prepared, String> {
    check_cancel(cancel)?;
    let prepared = Prepared::at(stage.path());
    if !valid_program(&prepared) {
        return Err("LTeX+ package is missing its launcher or matching Java runtime.".into());
    }
    let mut files = Vec::new();
    inventory(stage.path(), stage.path(), &mut files)?;
    write_json(
        &stage.path().join(".complete.json"),
        &Complete {
            catalog: catalog.to_owned(),
            target: target.to_owned(),
            files,
        },
    )?;
    check_cancel(cancel)?;
    let name = stage
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("LTeX+ cache generation name is invalid.")?
        .to_owned();
    write_json(&root.join("active.json"), &Active { generation: name })?;
    let directory = stage.keep();
    Ok(Prepared::at(&directory))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tar(entries: &[(&str, &[u8], u32)], links: &[(&str, &str)]) -> Vec<u8> {
        let mut archive = tar::Builder::new(Vec::new());
        for (path, bytes, mode) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            archive.append_data(&mut header, path, *bytes).unwrap();
        }
        for (path, target) in links {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name(target).unwrap();
            header.set_cksum();
            archive.append_data(&mut header, path, io::empty()).unwrap();
        }
        archive.into_inner().unwrap()
    }
    #[cfg(target_os = "macos")]
    fn write_asset(
        source: &Path,
        name: &str,
        target: Option<&str>,
        bytes: &[u8],
    ) -> manifest::Asset {
        let compressed = codec::encode_fixture(bytes);
        fs::write(source.join(format!("{name}.zst")), &compressed).unwrap();
        manifest::Asset {
            name: name.into(),
            target: target.map(str::to_owned),
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            compressed_bytes: compressed.len() as u64,
            compressed_sha256: format!("{:x}", Sha256::digest(&compressed)),
        }
    }
    #[cfg(target_os = "macos")]
    fn fixture(source: &Path) -> manifest::Manifest {
        fs::create_dir(source).unwrap();
        let shared = tar(
            &[
                (manifest::LAUNCHER, b"#!/bin/sh\n", 0o755),
                ("lib/server.jar", b"server fixture", 0o644),
            ],
            &[],
        );
        let native = tar(
            &[
                (
                    &format!("{}/bin/java", manifest::JDK_DIRECTORY),
                    b"java fixture",
                    0o755,
                ),
                (
                    &format!("{}/lib/server/libjvm.dylib", manifest::JDK_DIRECTORY),
                    b"jvm fixture",
                    0o644,
                ),
                (
                    &format!("{}/legal/java.base/LICENSE", manifest::JDK_DIRECTORY),
                    b"licence fixture",
                    0o644,
                ),
            ],
            &[(
                &format!("{}/legal/java.sql/LICENSE", manifest::JDK_DIRECTORY),
                "../java.base/LICENSE",
            )],
        );
        let mut files = vec![write_asset(source, manifest::SHARED_NAME, None, &shared)];
        for target in manifest::MACOS_TARGETS {
            let bytes = if target == crate::toolchain::target_triple() {
                &native[..]
            } else {
                b"wrong CPU must never be decoded"
            };
            files.push(write_asset(
                source,
                &format!("{target}.tar"),
                Some(target),
                bytes,
            ));
        }
        let manifest = manifest::Manifest {
            format: manifest::FORMAT,
            version: manifest::VERSION.into(),
            jdk_directory: manifest::JDK_DIRECTORY.into(),
            files,
        };
        manifest.validate().unwrap();
        fs::write(
            source.join(manifest::MANIFEST_NAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        manifest
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn prepares_only_current_cpu_and_cache_hit_never_reads_archives_or_rewrites_state() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("package");
        let manifest = fixture(&source);
        let cache = temp.path().join("cache");
        let cancel = AtomicBool::new(false);
        let first = prepare_at(&source, &cache, &cancel, false).unwrap();
        let active = fs::read(cache.join("active.json")).unwrap();
        let stamp_before = fs::metadata(cache.join("active.json"))
            .unwrap()
            .modified()
            .unwrap();
        assert!(valid_program(&first));
        for asset in manifest.files {
            fs::remove_file(source.join(asset.archive_name())).unwrap();
        }
        assert_eq!(prepare_at(&source, &cache, &cancel, false).unwrap(), first);
        assert_eq!(fs::read(cache.join("active.json")).unwrap(), active);
        assert_eq!(
            fs::metadata(cache.join("active.json"))
                .unwrap()
                .modified()
                .unwrap(),
            stamp_before
        );
        assert_eq!(
            fs::read(first.java_home.join("legal/java.sql/LICENSE")).unwrap(),
            b"licence fixture"
        );
        assert_eq!(
            fs::read_dir(&cache)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("generation-"))
                .count(),
            1
        );
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn repair_uses_a_new_generation_and_preserves_files_used_by_existing_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("package");
        fixture(&source);
        let cache = temp.path().join("cache");
        let cancel = AtomicBool::new(false);
        let first = prepare_at(&source, &cache, &cancel, false).unwrap();
        let repair = prepare_at(&source, &cache, &cancel, true).unwrap();
        assert_ne!(first.program, repair.program);
        assert!(valid_program(&first));
        assert!(valid_program(&repair));
        assert_eq!(prepare_at(&source, &cache, &cancel, false).unwrap(), repair);
        fs::write(repair.java_home.join("bin/java"), b"damaged").unwrap();
        let recovered = prepare_at(&source, &cache, &cancel, false).unwrap();
        assert_ne!(recovered.program, repair.program);
        assert_eq!(
            fs::read(recovered.java_home.join("bin/java")).unwrap(),
            b"java fixture"
        );
        assert_eq!(
            fs::read(repair.java_home.join("bin/java")).unwrap(),
            b"damaged"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn verified_redownload_reuses_healthy_cache_and_replaces_only_damaged_generations() {
        struct NeverRead;
        impl Read for NeverRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("a healthy pinned generation must not be extracted again");
            }
        }
        fn generations(root: &Path) -> usize {
            fs::read_dir(root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("generation-")
                })
                .count()
        }
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("package");
        fixture(&source);
        let cache = temp.path().join("cache");
        let cancel = AtomicBool::new(false);
        let first = prepare_at(&source, &cache, &cancel, false).unwrap();
        let (_, catalog) = read_catalog(&source).unwrap();
        let target = crate::toolchain::target_triple();
        let active = fs::read(cache.join("active.json")).unwrap();
        assert_eq!(
            activate_verified_download(NeverRead, &cache, &catalog, target, &cancel).unwrap(),
            first
        );
        assert_eq!(generations(&cache), 1);
        assert_eq!(fs::read(cache.join("active.json")).unwrap(), active);
        cancel.store(true, Ordering::Release);
        assert!(
            activate_verified_download(NeverRead, &cache, &catalog, target, &cancel)
                .unwrap_err()
                .contains("cancelled")
        );
        cancel.store(false, Ordering::Release);
        fs::write(first.java_home.join("bin/java"), b"damaged Java").unwrap();
        let prefix = format!("ltex-ls-plus-{}", manifest::VERSION);
        let archive = tar(
            &[
                (
                    &format!("{prefix}/{}", manifest::LAUNCHER),
                    b"#!/bin/sh\n",
                    0o755,
                ),
                (
                    &format!("{prefix}/lib/server.jar"),
                    b"server fixture",
                    0o644,
                ),
                (
                    &format!("{prefix}/{}/bin/java", manifest::JDK_DIRECTORY),
                    b"java fixture",
                    0o755,
                ),
                (
                    &format!(
                        "{prefix}/{}/lib/server/libjvm.dylib",
                        manifest::JDK_DIRECTORY
                    ),
                    b"jvm fixture",
                    0o644,
                ),
            ],
            &[],
        );
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(&archive).unwrap();
        let archive = gzip.finish().unwrap();
        let replacement =
            activate_verified_download(&archive[..], &cache, &catalog, target, &cancel).unwrap();
        assert_ne!(replacement.program, first.program);
        assert_eq!(generations(&cache), 2);
        assert_eq!(
            fs::read(first.java_home.join("bin/java")).unwrap(),
            b"damaged Java"
        );
        assert_eq!(
            fs::read(replacement.java_home.join("bin/java")).unwrap(),
            b"java fixture"
        );
        assert_eq!(
            activate_verified_download(NeverRead, &cache, &catalog, target, &cancel).unwrap(),
            replacement
        );
        assert_eq!(generations(&cache), 2);
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn replaced_cache_parent_is_not_followed_even_when_file_metadata_matches() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("package");
        fixture(&source);
        let cache = temp.path().join("cache");
        let cancel = AtomicBool::new(false);
        let first = prepare_at(&source, &cache, &cancel, false).unwrap();
        let root = first.program.parent().unwrap().parent().unwrap();
        fs::rename(root.join("lib"), root.join("original-lib")).unwrap();
        std::os::unix::fs::symlink("original-lib", root.join("lib")).unwrap();
        let replacement = prepare_at(&source, &cache, &cancel, false).unwrap();
        assert_ne!(first.program, replacement.program);
        assert_eq!(
            fs::read(root.join("original-lib/server.jar")).unwrap(),
            b"server fixture"
        );
        assert!(
            replacement
                .program
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("lib/server.jar")
                .is_file()
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn cancelled_or_damaged_restore_never_replaces_the_active_generation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("package");
        let manifest = fixture(&source);
        let cache = temp.path().join("cache");
        let cancel = AtomicBool::new(false);
        let first = prepare_at(&source, &cache, &cancel, false).unwrap();
        let active = fs::read(cache.join("active.json")).unwrap();
        cancel.store(true, Ordering::Release);
        assert!(
            prepare_at(&source, &cache, &cancel, true)
                .unwrap_err()
                .contains("cancelled")
        );
        cancel.store(false, Ordering::Release);
        let archive = source.join(manifest.files[0].archive_name());
        let mut bytes = fs::read(&archive).unwrap();
        bytes[0] ^= 1;
        fs::write(archive, bytes).unwrap();
        assert!(
            prepare_at(&source, &cache, &cancel, true)
                .unwrap_err()
                .contains("integrity")
        );
        assert_eq!(fs::read(cache.join("active.json")).unwrap(), active);
        assert!(valid_program(&first));
        assert_eq!(prepare_at(&source, &cache, &cancel, false).unwrap(), first);
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn concurrent_cold_windows_wait_then_reuse_the_completed_generation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("package");
        fixture(&source);
        let cache = temp.path().join("cache");
        private_directory(&cache).unwrap();
        let lock = try_setup_lock(&cache).unwrap().unwrap();
        assert!(try_setup_lock(&cache).unwrap().is_none());
        let cancel = AtomicBool::new(false);
        thread::scope(|scope| {
            let (sender, receiver) = mpsc::channel();
            let (worker_source, worker_cache, worker_cancel) = (&source, &cache, &cancel);
            scope.spawn(move || {
                let _ = sender.send(prepare_at(
                    worker_source,
                    worker_cache,
                    worker_cancel,
                    false,
                ));
            });
            assert!(matches!(
                receiver.recv_timeout(Duration::from_millis(75)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            // Simulate the first window's completed preparation while the next
            // waits; it must use this exact generation rather than extract twice.
            let (catalog, hash) = read_catalog(&source).unwrap();
            let stage = new_generation(&cache).unwrap();
            for asset in catalog.files_for_target(crate::toolchain::target_triple()) {
                let mut bytes = Vec::new();
                codec::decode(
                    File::open(source.join(asset.archive_name())).unwrap(),
                    asset.bytes,
                    &asset.sha256,
                    &mut bytes,
                )
                .unwrap();
                extract(
                    &bytes[..],
                    stage.path(),
                    if asset.target.is_some() {
                        ArchiveKind::Native
                    } else {
                        ArchiveKind::Shared
                    },
                    &cancel,
                )
                .unwrap();
            }
            let first = publish(
                stage,
                &cache,
                &hash,
                crate::toolchain::target_triple(),
                &cancel,
            )
            .unwrap();
            drop(lock);
            let second = receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            assert_eq!(first, second);
        });
        assert!(cache.join(".setup.lock").is_file());
    }
    #[test]
    fn waiting_for_another_window_observes_cancellation_without_removing_its_lock() {
        let temp = tempfile::tempdir().unwrap();
        let lock = try_setup_lock(temp.path()).unwrap().unwrap();
        let cancel = AtomicBool::new(false);
        thread::scope(|scope| {
            let (sender, receiver) = mpsc::channel();
            let (path, cancel) = (temp.path(), &cancel);
            scope.spawn(move || {
                let _ = sender.send(wait_for_setup(path, cancel).map(|_| ()));
            });
            assert!(matches!(
                receiver.recv_timeout(Duration::from_millis(75)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            cancel.store(true, Ordering::Release);
            assert!(
                receiver
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap_err()
                    .contains("cancelled")
            );
        });
        assert!(try_setup_lock(temp.path()).unwrap().is_none());
        drop(lock);
        assert!(temp.path().join(".setup.lock").is_file());
        assert!(try_setup_lock(temp.path()).unwrap().is_some());
    }
    #[test]
    fn unsafe_archive_paths_are_rejected_before_writing() {
        for path in ["../outside", "/outside", "jdk-21.0.10+7/../../outside"] {
            assert!(member(Path::new(path), ArchiveKind::Native).is_err());
        }
        for path in ["active.json", ".complete.json", "other/server.jar"] {
            assert!(member(Path::new(path), ArchiveKind::Shared).is_err());
        }
        assert!(
            member(
                Path::new("ltex-ls-plus-18.7.0/bin/ltex-ls-plus"),
                ArchiveKind::Official
            )
            .is_ok()
        );
        assert!(
            member(
                Path::new("ltex-ls-plus-18.6.0/bin/ltex-ls-plus"),
                ArchiveKind::Official
            )
            .is_err()
        );
        for name in ["../../outside", "/outside", "generation-x/../../../outside"] {
            assert!(!generation_name(name));
        }
    }
    #[test]
    #[cfg(unix)]
    fn symbolic_links_are_limited_to_the_java_runtime_and_never_used_as_extract_parents() {
        let cancel = AtomicBool::new(false);
        for target in ["/etc/passwd", "../../../outside", "../../../../outside"] {
            let temp = tempfile::tempdir().unwrap();
            let archive = tar(
                &[],
                &[(
                    &format!("{}/legal/LICENSE", manifest::JDK_DIRECTORY),
                    target,
                )],
            );
            assert!(extract(&archive[..], temp.path(), ArchiveKind::Native, &cancel).is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let archive = tar(
            &[("lib/target.jar", b"shared", 0o644)],
            &[("lib/link.jar", "target.jar")],
        );
        assert!(extract(&archive[..], temp.path(), ArchiveKind::Shared, &cancel).is_err());
        let temp = tempfile::tempdir().unwrap();
        let path = format!("{}/legal/a", manifest::JDK_DIRECTORY);
        let child = format!("{}/legal/a/write", manifest::JDK_DIRECTORY);
        let archive = tar(
            &[(&child, b"safe regular file", 0o644)],
            &[(&path, "../other")],
        );
        assert!(extract(&archive[..], temp.path(), ArchiveKind::Native, &cancel).is_err());
        assert!(temp.path().join(child).is_file());
    }
    #[test]
    fn duplicate_and_special_archive_members_fail() {
        let cancel = AtomicBool::new(false);
        let temp = tempfile::tempdir().unwrap();
        let archive = tar(
            &[("lib/a.jar", b"a", 0o644), ("lib/a.jar", b"b", 0o644)],
            &[],
        );
        assert!(
            extract(&archive[..], temp.path(), ArchiveKind::Shared, &cancel)
                .unwrap_err()
                .contains("duplicate")
        );
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Fifo);
        header.set_cksum();
        builder
            .append_data(&mut header, "lib/pipe", io::empty())
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let temp = tempfile::tempdir().unwrap();
        assert!(
            extract(&archive[..], temp.path(), ArchiveKind::Shared, &cancel)
                .unwrap_err()
                .contains("special")
        );
    }
    #[test]
    #[cfg(unix)]
    fn downloader_requires_success_exact_size_and_checksum() {
        let cancel = AtomicBool::new(false);
        let hash = format!("{:x}", Sha256::digest(b"fixture"));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf fixture"]);
        let mut output = Vec::new();
        download_command(&mut command, 7, &hash, &cancel, &mut output).unwrap();
        assert_eq!(output, b"fixture");
        for (script, size, expected, message) in [
            ("printf fixture", 6, hash.as_str(), "pinned size"),
            ("printf fixture", 8, hash.as_str(), "SHA-256"),
            ("printf fixture", 7, "incorrect", "SHA-256"),
            ("printf fixture; exit 7", 7, hash.as_str(), "failed"),
        ] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            assert!(
                download_command(&mut command, size, expected, &cancel, &mut Vec::new())
                    .unwrap_err()
                    .contains(message)
            );
        }
    }
    #[test]
    #[cfg(unix)]
    fn cancelling_a_blocked_download_reaps_the_child_and_bounded_reader() {
        struct CancelOnWrite<'a> {
            cancel: &'a AtomicBool,
            output: Vec<u8>,
        }
        impl Write for CancelOnWrite<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.output.extend_from_slice(bytes);
                self.cancel.store(true, Ordering::Release);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let cancel = AtomicBool::new(false);
        let mut command = Command::new("/bin/sh");
        command.args(["-c", r#"printf '%s\n' "$$"; exec sleep 30"#]);
        let mut output = CancelOnWrite {
            cancel: &cancel,
            output: Vec::new(),
        };
        let started = Instant::now();
        assert!(
            download_command(&mut command, 4096, "unused", &cancel, &mut output)
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancellation must not wait for the blocked transfer"
        );
        let pid = std::str::from_utf8(&output.output)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        );
    }
    #[test]
    fn cancellation_is_observed_inside_decoding_and_before_extraction() {
        let cancel = AtomicBool::new(true);
        let mut reader = CancelReader {
            inner: &b"bytes"[..],
            cancel: &cancel,
        };
        assert!(
            reader
                .read(&mut [0; 5])
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        let archive = tar(&[("lib/server.jar", b"content", 0o644)], &[]);
        let temp = tempfile::tempdir().unwrap();
        assert!(extract(&archive[..], temp.path(), ArchiveKind::Shared, &cancel).is_err());
        assert!(!temp.path().join("lib").exists());
    }
}
