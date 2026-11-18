use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    ops::Range,
    sync::atomic::AtomicUsize,
};

const CATALOG: &[u8] = include_bytes!("../../resources/diagnostic-fix/manifest.json");
const MODEL_LICENSE: &[u8] = include_bytes!("../../resources/diagnostic-fix/LICENSE-model.txt");
const RUNTIME_LICENSE: &[u8] = include_bytes!("../../resources/diagnostic-fix/LICENSE-runtime.txt");
const DOWNLOAD_WORKERS: usize = 8;
const RANGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_NATIVE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    url: String,
    bytes: u64,
    sha256: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeFile {
    source: String,
    name: String,
    bytes: u64,
    sha256: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Runtime {
    target: String,
    url: String,
    bytes: u64,
    sha256: String,
    files: Vec<NativeFile>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    format: u32,
    model_id: String,
    model_revision: String,
    runtime_version: String,
    minimum_macos: [u32; 2],
    model: Artifact,
    runtimes: Vec<Runtime>,
}

impl Catalog {
    fn parse() -> Result<Self, String> {
        let catalog: Self = serde_json::from_slice(CATALOG)
            .map_err(|_| "The local fix model catalog is invalid. Reinstall the app.".to_owned())?;
        catalog.validate()?;
        Ok(catalog)
    }

    fn validate(&self) -> Result<(), String> {
        let invalid = || "The local fix model catalog is invalid. Reinstall the app.".to_owned();
        if self.format != 1
            || self.model_id != MODEL_ID
            || self.model_revision != "cd0c74c575b5c416f3011b5b4c298077f26ca8c2"
            || self.runtime_version != "llama.cpp b12110 (v0.9.0)"
            || self.minimum_macos != [13, 3]
            || self.model.bytes != 1_099_627_808
            || !valid_hash(&self.model.sha256)
            || !self
                .model
                .url
                .starts_with("https://huggingface.co/LiquidAI/LFM3-1.7B-GGUF/resolve/")
        {
            return Err(invalid());
        }
        let mut targets = BTreeSet::from(["aarch64-apple-darwin", "x86_64-apple-darwin"]);
        for runtime in &self.runtimes {
            if !targets.remove(runtime.target.as_str())
                || runtime.bytes == 0
                || runtime.bytes > MAX_ARCHIVE_BYTES
                || !valid_hash(&runtime.sha256)
                || !runtime
                    .url
                    .starts_with("https://github.com/ggml-org/llama.cpp/releases/download/b12110/")
                || !(1..=32).contains(&runtime.files.len())
            {
                return Err(invalid());
            }
            let mut names = BTreeSet::new();
            let mut server = false;
            for file in &runtime.files {
                if !safe_name(&file.name)
                    || !names.insert(file.name.as_str())
                    || file.bytes == 0
                    || file.bytes > MAX_NATIVE_BYTES
                    || !valid_hash(&file.sha256)
                    || !file
                        .source
                        .strip_prefix("llama-b12110/")
                        .is_some_and(safe_name)
                {
                    return Err(invalid());
                }
                server |= file.name == "llama-server";
            }
            if !server {
                return Err(invalid());
            }
        }
        if !targets.is_empty() {
            return Err(invalid());
        }
        Ok(())
    }

    fn runtime(&self) -> Result<&Runtime, String> {
        self.runtimes
            .iter()
            .find(|runtime| runtime.target == current_target())
            .ok_or("Liquid fix suggestions do not support this Mac's architecture.".into())
    }
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && name != "."
        && name != ".."
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn current_target() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64-apple-darwin"
    } else {
        "x86_64-apple-darwin"
    }
}

pub(super) fn check_platform() -> Result<(), String> {
    if !cfg!(target_os = "macos") || !cfg!(any(target_arch = "aarch64", target_arch = "x86_64")) {
        return Err("Liquid fix suggestions currently require macOS 13.3 or later.".into());
    }
    let output = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .map_err(|_| "Cannot determine the macOS version for Liquid fix suggestions.".to_owned())?;
    let version = String::from_utf8_lossy(&output.stdout);
    let mut numbers = version.trim().split('.').map(str::parse::<u32>);
    let version = (
        numbers.next().and_then(Result::ok),
        numbers.next().and_then(Result::ok),
    );
    if !output.status.success()
        || !matches!(version, (Some(major), Some(minor)) if (major, minor) >= (13, 3))
    {
        return Err("Liquid fix suggestions require macOS 13.3 or later.".into());
    }
    Ok(())
}

pub(super) fn prepare(directory: &Path, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let catalog = Catalog::parse()?;
    ensure_directory(directory)?;
    let root = directory.join("foundation").join(MODEL_ID);
    ensure_directory(&root)?;
    let lock_path = root.join("setup.lock");
    if let Ok(metadata) = fs::symlink_metadata(&lock_path)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err("The Liquid fix setup lock must be a regular file.".into());
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW).mode(0o600);
    }
    let lock = options
        .open(lock_path)
        .map_err(|_| "Cannot lock the Liquid fix setup folder.".to_owned())?;
    loop {
        check_cancelled(cancel)?;
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(25)),
            Err(std::fs::TryLockError::Error(_)) => {
                return Err("Cannot lock the Liquid fix setup folder.".into());
            }
        }
    }
    prepare_model(&catalog.model, &root, cancel)?;
    prepare_runtime(catalog.runtime()?, &root, cancel)?;
    write_notice(&root.join("LICENSE-model.txt"), MODEL_LICENSE)?;
    write_notice(&root.join("LICENSE-runtime.txt"), RUNTIME_LICENSE)?;
    Ok(root)
}

fn ensure_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err("The Liquid fix cache must use regular folders.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or("The Liquid fix folder has no parent.")?;
            ensure_directory(parent)?;
            match fs::create_dir(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    ensure_directory(path)
                }
                Err(_) => Err("Cannot create the Liquid fix cache folder.".into()),
            }
        }
        Err(_) => Err("Cannot read the Liquid fix cache folder.".into()),
    }
}

fn verify_file(path: &Path, bytes: u64, hash: &str, cancel: &AtomicBool) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "Liquid fix model files are missing.".to_owned())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != bytes {
        return Err("Liquid fix model files have an invalid size or type.".into());
    }
    let mut file =
        File::open(path).map_err(|_| "Cannot read Liquid fix model files.".to_owned())?;
    let mut hasher = Sha256::new();
    let mut actual = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancelled(cancel)?;
        let count = file
            .read(&mut buffer)
            .map_err(|_| "Cannot verify Liquid fix model files.".to_owned())?;
        if count == 0 {
            break;
        }
        actual += count as u64;
        if actual > bytes {
            return Err("Liquid fix model files changed during verification.".into());
        }
        hasher.update(&buffer[..count]);
    }
    if actual != bytes || hex::encode(hasher.finalize()) != hash {
        return Err("Liquid fix model files failed their SHA-256 check.".into());
    }
    Ok(())
}

fn replace_file(staging: tempfile::NamedTempFile, destination: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(destination)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err("Liquid fix cache files must be regular files.".into());
    }
    staging
        .as_file()
        .sync_all()
        .map_err(|_| "Cannot save Liquid fix model files.".to_owned())?;
    staging
        .persist(destination)
        .map_err(|_| "Cannot publish Liquid fix model files.".to_owned())?;
    Ok(())
}

fn write_notice(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() == bytes.len() as u64
    }) && fs::read(path).is_ok_and(|existing| existing == bytes)
    {
        return Ok(());
    }
    let mut staging = tempfile::NamedTempFile::new_in(path.parent().unwrap())
        .map_err(|_| "Cannot prepare Liquid fix model notices.".to_owned())?;
    staging
        .write_all(bytes)
        .map_err(|_| "Cannot save Liquid fix model notices.".to_owned())?;
    replace_file(staging, path)
}

fn prepare_model(artifact: &Artifact, root: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let destination = root.join("model.gguf");
    if verify_file(&destination, artifact.bytes, &artifact.sha256, cancel).is_ok() {
        return Ok(());
    }
    check_cancelled(cancel)?;
    let chunks = tempfile::Builder::new()
        .prefix(".model-download-")
        .tempdir_in(root)
        .map_err(|_| "Cannot prepare the Liquid fix model download.".to_owned())?;
    download_model_ranges(artifact, chunks.path(), cancel)?;
    let mut staging = tempfile::NamedTempFile::new_in(root)
        .map_err(|_| "Cannot prepare the Liquid fix model download.".to_owned())?;
    for index in 0..artifact.bytes.div_ceil(RANGE_BYTES) {
        check_cancelled(cancel)?;
        let mut part = File::open(chunks.path().join(index.to_string()))
            .map_err(|_| "The Liquid fix model download is incomplete.".to_owned())?;
        let mut buffer = [0; 64 * 1024];
        loop {
            check_cancelled(cancel)?;
            let count = part
                .read(&mut buffer)
                .map_err(|_| "Cannot assemble the Liquid fix model.".to_owned())?;
            if count == 0 {
                break;
            }
            staging
                .write_all(&buffer[..count])
                .map_err(|_| "Cannot save the Liquid fix model.".to_owned())?;
        }
    }
    staging
        .flush()
        .map_err(|_| "Cannot save the Liquid fix model.".to_owned())?;
    verify_file(staging.path(), artifact.bytes, &artifact.sha256, cancel)?;
    replace_file(staging, &destination)
}

fn download_model_ranges(
    artifact: &Artifact,
    root: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let abort = Arc::new(AtomicBool::new(false));
    let next = Arc::new(AtomicUsize::new(0));
    let count = artifact.bytes.div_ceil(RANGE_BYTES) as usize;
    let (sender, receiver) = mpsc::sync_channel(DOWNLOAD_WORKERS);
    let mut workers = Vec::new();
    for _ in 0..DOWNLOAD_WORKERS.min(count) {
        let (artifact, root, abort, next, sender) = (
            artifact.clone(),
            root.to_owned(),
            Arc::clone(&abort),
            Arc::clone(&next),
            sender.clone(),
        );
        workers.push(thread::spawn(move || {
            let result = (|| {
                loop {
                    check_cancelled(&abort)?;
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= count {
                        return Ok(());
                    }
                    let start = index as u64 * RANGE_BYTES;
                    let range = start..(start + RANGE_BYTES).min(artifact.bytes);
                    download_part(
                        &root.join(index.to_string()),
                        range,
                        &abort,
                        |range, cancel, output| {
                            crate::liquid_runtime::resources::download_range(
                                &artifact.url,
                                range,
                                artifact.bytes,
                                cancel,
                                output,
                            )
                        },
                    )?;
                }
            })();
            let failed = result.is_err();
            let _ = sender.send(result);
            if failed {
                abort.store(true, Ordering::Release);
            }
        }));
    }
    drop(sender);
    let mut result = Ok(());
    let mut remaining = workers.len();
    let deadline = Instant::now() + Duration::from_secs(30 * 60);
    while remaining != 0 {
        if let Err(error) = check_cancelled(cancel) {
            abort.store(true, Ordering::Release);
            result = Err(error);
        }
        if Instant::now() >= deadline && result.is_ok() {
            abort.store(true, Ordering::Release);
            result = Err(
                "The Liquid fix model download timed out. Check the connection and retry.".into(),
            );
        }
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(reply) => {
                remaining -= 1;
                if result.is_ok() {
                    result = reply;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    for worker in workers {
        let _ = worker.join();
    }
    result
}

/// A transient failure retries only its own range. A failed attempt's private
/// file is dropped, so partial stdout can never be appended to the next try.
fn download_part(
    destination: &Path,
    range: Range<u64>,
    cancel: &AtomicBool,
    mut fetch: impl FnMut(Range<u64>, &AtomicBool, &mut File) -> Result<(), String>,
) -> Result<(), String> {
    let mut last_error = String::new();
    for attempt in 0..3 {
        check_cancelled(cancel)?;
        let mut output = tempfile::NamedTempFile::new_in(destination.parent().unwrap())
            .map_err(|_| "Cannot prepare a Liquid fix model download part.".to_owned())?;
        let result = fetch(range.clone(), cancel, output.as_file_mut()).and_then(|()| {
            if output
                .as_file()
                .metadata()
                .map_err(|_| "Cannot inspect a Liquid model download part.".to_owned())?
                .len()
                != range.end - range.start
            {
                return Err("The Liquid model download part is incomplete.".into());
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                output
                    .persist_noclobber(destination)
                    .map_err(|_| "Cannot save a Liquid fix model download part.".to_owned())?;
                return Ok(());
            }
            Err(error) => last_error = error,
        }
        drop(output);
        if attempt != 2 {
            let until = Instant::now() + Duration::from_millis(250 * (attempt + 1));
            while Instant::now() < until {
                check_cancelled(cancel)?;
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
    Err(last_error)
}

fn prepare_runtime(runtime: &Runtime, root: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let destination = root.join("runtime").join(&runtime.target);
    ensure_directory(&destination)?;
    if runtime.files.iter().all(|file| {
        verify_file(
            &destination.join(&file.name),
            file.bytes,
            &file.sha256,
            cancel,
        )
        .is_ok()
    }) {
        return Ok(());
    }
    check_cancelled(cancel)?;
    let mut archive = tempfile::NamedTempFile::new_in(root)
        .map_err(|_| "Cannot prepare the local Liquid runtime download.".to_owned())?;
    crate::liquid_runtime::resources::download(
        &runtime.url,
        runtime.bytes,
        &runtime.sha256,
        cancel,
        &mut archive,
    )?;
    archive
        .flush()
        .map_err(|_| "Cannot save the local Liquid runtime download.".to_owned())?;
    extract_runtime(archive.path(), runtime, &destination, cancel)
}

fn extract_runtime(
    archive: &Path,
    runtime: &Runtime,
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut required: BTreeMap<&str, Vec<&NativeFile>> = BTreeMap::new();
    for file in &runtime.files {
        required.entry(&file.source).or_default().push(file);
    }
    let input =
        File::open(archive).map_err(|_| "Cannot open the downloaded Liquid runtime.".to_owned())?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(input));
    let mut total = 0u64;
    let mut count = 0;
    let scratch = tempfile::Builder::new()
        .prefix(".runtime-download-")
        .tempdir_in(destination)
        .map_err(|_| "Cannot prepare the downloaded Liquid runtime.".to_owned())?;
    for entry in tar
        .entries()
        .map_err(|_| "The downloaded Liquid runtime is invalid.".to_owned())?
    {
        check_cancelled(cancel)?;
        count += 1;
        let mut entry =
            entry.map_err(|_| "The downloaded Liquid runtime is invalid.".to_owned())?;
        let path = entry
            .path()
            .map_err(|_| "The downloaded Liquid runtime has an invalid path.".to_owned())?;
        if count > 256
            || path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err("The downloaded Liquid runtime has an unsafe path.".into());
        }
        let kind = entry.header().entry_type();
        if kind.is_symlink() {
            let link = entry
                .link_name()
                .map_err(|_| "The downloaded Liquid runtime has an unsafe link.".to_owned())?
                .ok_or("The downloaded Liquid runtime has an unsafe link.")?;
            if link.components().count() != 1 || !link.to_str().is_some_and(safe_name) {
                return Err("The downloaded Liquid runtime has an unsafe link.".into());
            }
            // The signed upstream archive's aliases are not extracted. Each
            // needed alias becomes an independently verified regular file.
            continue;
        }
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err("The downloaded Liquid runtime contains a special file.".into());
        }
        let size = entry.size();
        total = total
            .checked_add(size)
            .ok_or("The downloaded Liquid runtime is too large.")?;
        if size > MAX_NATIVE_BYTES || total > MAX_EXPANDED_BYTES {
            return Err("The downloaded Liquid runtime is too large.".into());
        }
        let Some(files) = path.to_str().and_then(|path| required.remove(path)) else {
            continue;
        };
        let mut bytes = Vec::with_capacity(size as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|_| "Cannot unpack the downloaded Liquid runtime.".to_owned())?;
        for file in files {
            if bytes.len() as u64 != file.bytes
                || hex::encode(Sha256::digest(&bytes)) != file.sha256
            {
                return Err("The downloaded Liquid runtime failed its SHA-256 check.".into());
            }
            let mut output = tempfile::NamedTempFile::new_in(scratch.path())
                .map_err(|_| "Cannot prepare the downloaded Liquid runtime.".to_owned())?;
            output
                .write_all(&bytes)
                .map_err(|_| "Cannot save the downloaded Liquid runtime.".to_owned())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                output
                    .as_file()
                    .set_permissions(fs::Permissions::from_mode(0o700))
                    .map_err(|_| {
                        "Cannot make the downloaded Liquid runtime executable.".to_owned()
                    })?;
            }
            output
                .persist(scratch.path().join(&file.name))
                .map_err(|_| "Cannot stage the downloaded Liquid runtime.".to_owned())?;
        }
    }
    if !required.is_empty() {
        return Err("The downloaded Liquid runtime is incomplete.".into());
    }
    // Publish only after every dependency has passed its compiled checksum.
    for file in &runtime.files {
        check_cancelled(cancel)?;
        let mut output = tempfile::NamedTempFile::new_in(destination)
            .map_err(|_| "Cannot prepare the local Liquid runtime.".to_owned())?;
        let mut input = File::open(scratch.path().join(&file.name))
            .map_err(|_| "The local Liquid runtime is incomplete.".to_owned())?;
        std::io::copy(&mut input, &mut output)
            .map_err(|_| "Cannot save the local Liquid runtime.".to_owned())?;
        output
            .as_file()
            .set_permissions(
                input
                    .metadata()
                    .map_err(|_| "Cannot inspect the local Liquid runtime.".to_owned())?
                    .permissions(),
            )
            .map_err(|_| "Cannot set local Liquid runtime permissions.".to_owned())?;
        replace_file(output, &destination.join(&file.name))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "setup/tests.rs"]
mod tests;
