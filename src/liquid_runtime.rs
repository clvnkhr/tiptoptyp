//! Optional native grammar model. The shared client is inert until an explicit
//! selected-text request runs on an application worker. Missing assets are downloaded
//! on demand; a verified installed model works offline.
use crate::{
    bundled_payload::codec,
    liquid_text::{Prediction, TextProcessor},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};
use std::{process::Command, time::Duration};
#[path = "liquid_process.rs"]
mod process;
use process::JsonLineProcess;
#[path = "liquid_notices.rs"]
mod notices;
#[path = "liquid_resources.rs"]
mod resources;

#[path = "../build_support/liquid_manifest.rs"]
pub(crate) mod manifest;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) directory: PathBuf,
    pub(crate) threads: u8,
}
impl Config {
    pub(crate) fn from_settings(settings: &crate::settings::AppSettings) -> Result<Self, String> {
        validate_settings(&settings.liquid_cache_directory, settings.liquid_threads)?;
        let directory = if settings.liquid_cache_directory.trim().is_empty() {
            default_directory()?
        } else {
            PathBuf::from(settings.liquid_cache_directory.trim())
        };
        Ok(Self {
            directory,
            threads: settings.liquid_threads,
        })
    }
    fn model_directory(&self) -> PathBuf {
        self.directory.join("onnxruntime").join(manifest::MODEL_ID)
    }
}

pub(crate) fn validate_settings(directory: &str, threads: u8) -> Result<(), String> {
    if directory.len() > 4096
        || directory.chars().any(char::is_control)
        || (!directory.trim().is_empty() && !Path::new(directory.trim()).is_absolute())
    {
        return Err("liquid_cache_directory: choose an absolute folder path, or leave it empty for the app data folder.".into());
    }
    if !(1..=8).contains(&threads) {
        return Err("liquid_threads: choose between 1 and 8 CPU threads.".into());
    }
    Ok(())
}
fn default_directory() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    base.filter(|p| p.is_absolute())
        .map(|p| p.join("dev.tiptoptyp.editor/liquid"))
        .ok_or_else(|| {
            "Cannot find the application data folder. Choose a Liquid folder in Settings → Grammar & spelling."
                .into()
        })
}

#[derive(Default)]
pub(crate) struct Client {
    model: Option<LoadedModel>,
}
/// Every editor window shares one lazily loaded native model. Constructing this
/// handle performs no filesystem, model loading, thread-pool or network work.
pub(crate) fn shared_client() -> Arc<Mutex<Client>> {
    static CLIENT: LazyLock<Arc<Mutex<Client>>> =
        LazyLock::new(|| Arc::new(Mutex::new(Client::default())));
    Arc::clone(&CLIENT)
}
/// Wait only on a worker, keeping a cancelled selection or download independent
/// of another window's potentially lengthy first-use setup.
pub(crate) fn lock_client<'a>(
    client: &'a Mutex<Client>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<std::sync::MutexGuard<'a, Client>, String> {
    loop {
        check_cancelled(cancel)?;
        match client.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(error)) => {
                let mut guard = error.into_inner();
                *guard = Client::default();
                client.clear_poison();
                return Ok(guard);
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}
impl Client {
    /// Called only by the explicit recovery worker while holding the shared
    /// client lock, so no inference can race replacement of cached resources.
    pub(crate) fn redownload(
        &mut self,
        config: &Config,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(), String> {
        check_cancelled(cancel)?;
        check_platform()?;
        let catalog = trusted_manifest()?;
        self.model = None;
        let _lock = setup_lock(&config.directory)?;
        prepare_assets(&catalog, config, cancel, true, download_asset)?;
        notices::prepare(config, cancel, true)
    }

    #[cfg(test)]
    pub(crate) fn correct(
        &mut self,
        config: &Config,
        texts: &[String],
    ) -> Result<Vec<String>, String> {
        self.correct_stream(
            config,
            texts,
            &std::sync::atomic::AtomicBool::new(false),
            |_, _| Ok(()),
        )
    }

    pub(crate) fn correct_stream(
        &mut self,
        config: &Config,
        texts: &[String],
        cancel: &std::sync::atomic::AtomicBool,
        mut progress: impl FnMut(usize, crate::liquid_text::ChunkProgress<'_>) -> Result<(), String>,
    ) -> Result<Vec<String>, String> {
        validate_input(texts)?;
        check_cancelled(cancel)?;
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.config != *config)
        {
            self.model = None;
        }
        if self.model.is_none() {
            self.model = Some(LoadedModel::load(config.clone(), cancel)?);
        }
        let loaded = self.model.as_mut().unwrap();
        let result: Result<Vec<String>, String> = texts
            .iter()
            .enumerate()
            .map(|(fragment, text)| {
                check_cancelled(cancel)?;
                loaded.processor.correct_stream(
                    text,
                    |ids| {
                        check_cancelled(cancel)?;
                        loaded.inference.predict_cancellable(ids, cancel)
                    },
                    |chunk| {
                        check_cancelled(cancel)?;
                        progress(fragment, chunk)
                    },
                )
            })
            .collect();
        // Retain a healthy helper after an input-length/tokenizer error. A
        // failed IPC exchange is dropped here on the application worker.
        if loaded.inference.failed {
            self.model = None;
        }
        result
    }
}
fn check_cancelled(cancel: &std::sync::atomic::AtomicBool) -> Result<(), String> {
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        Err("Liquid check cancelled.".into())
    } else {
        Ok(())
    }
}
fn validate_input(texts: &[String]) -> Result<(), String> {
    if texts.is_empty()
        || texts.len() > 16_384
        || texts
            .iter()
            .try_fold(0usize, |size, text| size.checked_add(text.len()))
            .is_none_or(|bytes| bytes > 1024 * 1024)
    {
        return Err("Select between one and 16,384 prose passages, up to 1 MiB of text.".into());
    }
    Ok(())
}
struct LoadedModel {
    config: Config,
    processor: TextProcessor,
    inference: NativeModel,
}
impl LoadedModel {
    fn load(config: Config, cancel: &std::sync::atomic::AtomicBool) -> Result<Self, String> {
        check_cancelled(cancel)?;
        check_platform()?;
        let _lock = setup_lock(&config.directory)?;
        let root = config.model_directory();
        // Verification and first-use downloading run only on this worker.
        // A healthy installed cache never needs a network connection.
        prepare_assets(&trusted_manifest()?, &config, cancel, false, download_asset)?;
        notices::prepare(&config, cancel, false)?;
        let processor = TextProcessor::load(&root.join(manifest::TOKENIZER_NAME))?;
        check_cancelled(cancel)?;
        let inference = NativeModel::load(&root, config.threads, cancel)?;
        Ok(Self {
            config,
            processor,
            inference,
        })
    }
}
#[cfg(test)]
fn verify_installed_assets(config: &Config, packaged: &manifest::Manifest) -> Result<(), String> {
    verify_model_directories(config)?;
    let root = config.model_directory();
    read_manifest(&root).map_err(|_| unavailable_files())?;
    for file in packaged.files_for_target(current_target()) {
        verify_file(&root.join(file.relative_path()), file.bytes, &file.sha256).map_err(|_| {
            "Liquid's runtime or model failed its integrity check. Check its data folder in Settings → Grammar & spelling.".to_owned()
        })?;
    }
    Ok(())
}
#[cfg(test)]
fn unavailable_files() -> String {
    "Liquid’s local files are unavailable. Check its data folder in Settings → Grammar & spelling."
        .into()
}
fn current_target() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64-apple-darwin"
    } else {
        "x86_64-apple-darwin"
    }
}
#[cfg(any(target_os = "macos", test))]
fn supports_macos(target: &str, major: u32, minor: u32) -> bool {
    manifest::minimum_macos(target).is_some_and(|minimum| (major, minor) >= minimum)
}
fn check_platform() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .map_err(|_| "Cannot determine the macOS version for Liquid.".to_owned())?;
        let version = std::str::from_utf8(&output.stdout).ok().and_then(|v| {
            let mut pieces = v.trim().split('.');
            Some((
                pieces.next()?.parse::<u32>().ok()?,
                pieces.next().unwrap_or("0").parse::<u32>().ok()?,
            ))
        });
        if !output.status.success() || version.is_none() {
            return Err("Cannot determine the macOS version for Liquid.".into());
        }
        let (major, minor) = version.unwrap();
        if supports_macos(current_target(), major, minor) {
            return Ok(());
        }
        let (major, minor) = manifest::minimum_macos(current_target()).unwrap_or((14, 0));
        Err(format!(
            "Liquid needs macOS {major}.{minor} or later on this Mac. The rest of the editor remains available."
        ))
    }
    #[cfg(not(target_os = "macos"))]
    Err("Liquid currently supports macOS only.".into())
}
struct NativeModel {
    process: JsonLineProcess,
    next_id: u64,
    failed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    ready: bool,
    protocol: u32,
    target: String,
    threads: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    id: u64,
    ok: bool,
    best_tags: Option<Vec<i64>>,
    error_probs: Option<Vec<f32>>,
    error: Option<String>,
}
impl NativeModel {
    fn load(
        root: &Path,
        threads: u8,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Self, String> {
        check_cancelled(cancel)?;
        let executable = root
            .join("runtime")
            .join(current_target())
            .join(manifest::HELPER_NAME);
        let mut command = Command::new(executable);
        command
            .arg(root.join(manifest::MODEL_NAME))
            .arg(threads.to_string());
        // The native helper has a closed sibling-library dependency set and
        // needs neither Python, a shell, nor inherited dynamic-loader overrides.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("DYLD_") {
                command.env_remove(key);
            }
        }
        for key in [
            "OMP_NUM_THREADS",
            "MKL_NUM_THREADS",
            "OPENBLAS_NUM_THREADS",
            "VECLIB_MAXIMUM_THREADS",
            "NUMEXPR_NUM_THREADS",
        ] {
            command.env(key, threads.to_string());
        }
        let mut process = JsonLineProcess::spawn(&mut command)?;
        let line = process.receive_cancellable(Duration::from_secs(120), cancel)?;
        validate_ready(&line, current_target(), threads)?;
        Ok(Self {
            process,
            next_id: 0,
            failed: false,
        })
    }
    #[cfg(test)]
    fn predict(&mut self, ids: &[i64]) -> Result<Prediction, String> {
        self.predict_cancellable(ids, &std::sync::atomic::AtomicBool::new(false))
    }
    fn predict_cancellable(
        &mut self,
        ids: &[i64],
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Prediction, String> {
        check_cancelled(cancel)?;
        if ids.is_empty() || ids.len() > 128 || ids.iter().any(|id| !(0..64_400).contains(id)) {
            return Err(
                "A selected sentence is too long or invalid for Liquid. Select shorter sentences."
                    .into(),
            );
        }
        let result = self.predict_validated(ids, cancel);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn predict_validated(
        &mut self,
        ids: &[i64],
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Prediction, String> {
        self.next_id = self.next_id.checked_add(1).ok_or_else(protocol_error)?;
        let mut request = self.next_id.to_string();
        for id in ids {
            use std::fmt::Write;
            write!(&mut request, " {id}").map_err(|_| protocol_error())?;
        }
        self.process.send(request.as_bytes())?;
        let line = self
            .process
            .receive_cancellable(Duration::from_secs(30), cancel)?;
        prediction_from_reply(&line, self.next_id, ids.len())
    }
}
fn protocol_error() -> String {
    "Liquid's local helper returned an invalid response. Try the selection again.".into()
}
fn validate_ready(line: &[u8], target: &str, threads: u8) -> Result<(), String> {
    let ready: Ready = serde_json::from_slice(line).map_err(|_| protocol_error())?;
    if !ready.ready || ready.protocol != 1 || ready.target != target || ready.threads != threads {
        return Err(protocol_error());
    }
    Ok(())
}
fn prediction_from_reply(line: &[u8], id: u64, length: usize) -> Result<Prediction, String> {
    let reply: Reply = serde_json::from_slice(line).map_err(|_| protocol_error())?;
    if reply.id != id {
        return Err(protocol_error());
    }
    if !reply.ok {
        return Err(
            "Liquid could not check this selection. Try shorter, complete sentences.".into(),
        );
    }
    if reply.error.is_some() {
        return Err(protocol_error());
    }
    let tags = reply.best_tags.ok_or_else(protocol_error)?;
    let error_probabilities = reply.error_probs.ok_or_else(protocol_error)?;
    if tags.len() != length
        || error_probabilities.len() != length
        || tags.iter().any(|tag| !(0..128_802).contains(tag))
        || error_probabilities
            .iter()
            .any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
    {
        return Err(protocol_error());
    }
    Ok(Prediction {
        tags,
        error_probabilities,
    })
}

const DOWNLOAD_BASE: &str =
    "https://github.com/clvnkhr/tiptoptyp/releases/download/liquid-onnxruntime-v1/";

fn trusted_manifest() -> Result<manifest::Manifest, String> {
    manifest::Manifest::parse(include_bytes!("../resources/liquid/manifest.json")).map_err(
        |error| format!("Liquid's download catalog is invalid: {error}. Reinstall the app."),
    )
}
fn archive_url(file: &manifest::Asset) -> String {
    let name = match file.target.as_deref() {
        Some(target) => format!("{target}-{}.zst", file.name),
        None => format!("{}.zst", file.name),
    };
    format!("{DOWNLOAD_BASE}{name}")
}
fn download_asset(
    file: &manifest::Asset,
    output: &mut fs::File,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    resources::download(
        &archive_url(file),
        file.compressed_bytes,
        &file.compressed_sha256,
        cancel,
        output,
    )
}
#[cfg(test)]
fn read_manifest(directory: &Path) -> Result<manifest::Manifest, String> {
    let path = directory.join(manifest::MANIFEST_NAME);
    if !fs::symlink_metadata(&path).is_ok_and(|metadata| {
        metadata.file_type().is_file() && metadata.len() <= manifest::MAX_MANIFEST_BYTES
    }) {
        return Err("Liquid manifest is missing or invalid".into());
    }
    let file = fs::File::open(path).map_err(|_| "Liquid manifest is missing".to_owned())?;
    let mut bytes = Vec::new();
    file.take(manifest::MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read Liquid manifest".to_owned())?;
    manifest::Manifest::parse(&bytes)
}
/// File I/O can span a large model; cancellation must interrupt each bounded
/// read/write rather than waiting until the next complete asset. Use Other:
/// std::io helpers retry Interrupted, which would spin on a cancelled token.
struct CancellableIo<'a, T> {
    inner: T,
    cancel: &'a std::sync::atomic::AtomicBool,
}
impl<T: Read> Read for CancellableIo<'_, T> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        check_io_cancelled(self.cancel)?;
        let count = self.inner.read(output)?;
        check_io_cancelled(self.cancel)?;
        Ok(count)
    }
}
impl<T: Write> Write for CancellableIo<'_, T> {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        check_io_cancelled(self.cancel)?;
        let count = self.inner.write(input)?;
        check_io_cancelled(self.cancel)?;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        check_io_cancelled(self.cancel)?;
        self.inner.flush()?;
        check_io_cancelled(self.cancel)
    }
}
fn check_io_cancelled(cancel: &std::sync::atomic::AtomicBool) -> std::io::Result<()> {
    check_cancelled(cancel).map_err(std::io::Error::other)
}
fn cancellation_or(cancel: &std::sync::atomic::AtomicBool, error: impl Into<String>) -> String {
    check_cancelled(cancel)
        .err()
        .unwrap_or_else(|| error.into())
}
#[cfg(test)]
fn verify_file(path: &Path, bytes: u64, expected: &str) -> Result<(), String> {
    verify_file_cancellable(
        path,
        bytes,
        expected,
        &std::sync::atomic::AtomicBool::new(false),
    )
}
fn verify_file_cancellable(
    path: &Path,
    bytes: u64,
    expected: &str,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    check_cancelled(cancel)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| "Liquid file is missing".to_owned())?;
    if !metadata.file_type().is_file() || metadata.len() != bytes {
        return Err("Liquid file has an invalid size or type".into());
    }
    let file = fs::File::open(path).map_err(|_| "Cannot read Liquid file".to_owned())?;
    verify_contents(file, bytes, expected, cancel)
}
fn verify_contents(
    reader: impl Read,
    bytes: u64,
    expected: &str,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    let mut file = CancellableIo {
        inner: reader.take(bytes + 1),
        cancel,
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut actual = 0u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| cancellation_or(cancel, "Cannot verify Liquid file"))?;
        if count == 0 {
            break;
        }
        actual += count as u64;
        hasher.update(&buffer[..count]);
    }
    check_cancelled(cancel)?;
    if actual != bytes || format!("{:x}", hasher.finalize()) != expected {
        return Err("Liquid file checksum does not match".into());
    }
    Ok(())
}
fn unpack_asset(
    input: impl Read,
    file: &manifest::Asset,
    root: &Path,
    target: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    check_cancelled(cancel)?;
    let mut output = tempfile::NamedTempFile::new_in(root)
        .map_err(|e| format!("Liquid setup cannot prepare its model: {e}"))?;
    codec::decode_bounded(
        CancellableIo {
            inner: input,
            cancel,
        },
        file.bytes,
        &file.sha256,
        &mut CancellableIo {
            inner: &mut output,
            cancel,
        },
        manifest::MAX_FILE_BYTES,
    )
    .map_err(|e| cancellation_or(cancel, format!("Liquid setup cannot unpack its model: {e}")))?;
    #[cfg(unix)]
    if file.name == manifest::HELPER_NAME {
        use std::os::unix::fs::PermissionsExt;
        output
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("Liquid setup cannot make its helper executable: {e}"))?;
    }
    output
        .as_file()
        .sync_all()
        .map_err(|e| format!("Liquid setup cannot save its model: {e}"))?;
    check_cancelled(cancel)?;
    output
        .persist(target)
        .map_err(|e| format!("Liquid setup cannot publish its model: {e}"))?;
    Ok(())
}

#[cfg(test)]
fn verify_model_directories(config: &Config) -> Result<(), String> {
    let root = config.model_directory();
    for directory in [
        config.directory.join("onnxruntime"),
        root.clone(),
        root.join("runtime"),
        root.join("runtime").join(current_target()),
    ] {
        if !fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.file_type().is_dir()) {
            return Err(
                "Liquid’s local files are unavailable. Check its data folder in Settings → Grammar & spelling."
                    .into(),
            );
        }
    }
    Ok(())
}
fn prepare_assets(
    catalog: &manifest::Manifest,
    config: &Config,
    cancel: &std::sync::atomic::AtomicBool,
    force_download: bool,
    mut fetch: impl FnMut(
        &manifest::Asset,
        &mut fs::File,
        &std::sync::atomic::AtomicBool,
    ) -> Result<(), String>,
) -> Result<(), String> {
    check_cancelled(cancel)?;
    catalog
        .validate()
        .map_err(|error| format!("Liquid's download catalog is invalid: {error}"))?;
    let root = config.model_directory();
    for directory in [
        config.directory.join("onnxruntime"),
        root.clone(),
        root.join("runtime"),
        root.join("runtime").join(current_target()),
    ] {
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!(
                    "Liquid setup cannot create its model folder: {error}"
                ));
            }
        }
        if !fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.file_type().is_dir()) {
            return Err("Liquid setup model folder must be a regular directory.".into());
        }
    }
    for file in catalog.files_for_target(current_target()) {
        check_cancelled(cancel)?;
        let target = root.join(file.relative_path());
        let ready = verify_file_cancellable(&target, file.bytes, &file.sha256, cancel);
        check_cancelled(cancel)?;
        if ready.is_ok() {
            #[cfg(unix)]
            if file.name == manifest::HELPER_NAME {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o700))
                    .map_err(|e| format!("Liquid setup cannot make its helper executable: {e}"))?;
            }
            if !force_download {
                continue;
            }
        }
        let mut archive = tempfile::NamedTempFile::new_in(&root)
            .map_err(|error| format!("Liquid setup cannot prepare a download: {error}"))?;
        fetch(file, archive.as_file_mut(), cancel)?;
        archive
            .rewind()
            .map_err(|error| format!("Liquid setup cannot read its download: {error}"))?;
        verify_contents(
            &mut archive,
            file.compressed_bytes,
            &file.compressed_sha256,
            cancel,
        )
        .map_err(|error| cancellation_or(cancel, format!("Liquid download is damaged: {error}")))?;
        if ready.is_ok() {
            // A successful explicit redownload verifies the source again but
            // does not rewrite identical installed files or disturb mmap users.
            continue;
        }
        archive
            .rewind()
            .map_err(|error| format!("Liquid setup cannot read its download: {error}"))?;
        unpack_asset(archive, file, &root, &target, cancel)?;
    }
    check_cancelled(cancel)?;
    let mut output = tempfile::NamedTempFile::new_in(&root)
        .map_err(|e| format!("Liquid setup cannot prepare its manifest: {e}"))?;
    serde_json::to_writer(
        &mut CancellableIo {
            inner: &mut output,
            cancel,
        },
        catalog,
    )
    .map_err(|e| {
        cancellation_or(
            cancel,
            format!("Liquid setup cannot save its manifest: {e}"),
        )
    })?;
    output
        .flush()
        .map_err(|e| format!("Liquid setup cannot save its manifest: {e}"))?;
    check_cancelled(cancel)?;
    output
        .persist(root.join(manifest::MANIFEST_NAME))
        .map_err(|e| format!("Liquid setup cannot publish its manifest: {e}"))?;
    Ok(())
}
#[cfg(test)]
fn prepare_for_test(source: &Path, config: &Config) -> Result<(), String> {
    let _lock = setup_lock(&config.directory)?;
    prepare_assets_cancellable(source, config, &std::sync::atomic::AtomicBool::new(false))
}
#[cfg(test)]
fn prepare_assets_cancellable(
    source: &Path,
    config: &Config,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    check_cancelled(cancel)?;
    let catalog = read_manifest(source).map_err(|error| {
        format!("Liquid's download catalog is invalid: {error}. Reinstall the app.")
    })?;
    prepare_assets(&catalog, config, cancel, false, |file, output, token| {
        let mut input = CancellableIo {
            inner: fs::File::open(source.join(file.archive_relative_path()))
                .map_err(|error| format!("Liquid download is unavailable: {error}"))?,
            cancel: token,
        };
        std::io::copy(&mut input, output)
            .map(|_| ())
            .map_err(|error| cancellation_or(token, format!("Liquid download failed: {error}")))
    })
}
/// Explicitly unlock before dropping the descriptor. Another thread can fork a
/// sidecar between acquiring this lock and dropping it; that child temporarily
/// inherits the open file description until exec. Relying only on descriptor
/// closure leaves a spurious live lock in that child after setup has finished.
#[derive(Debug)]
struct SetupLock {
    file: fs::File,
}
impl Drop for SetupLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
/// Keep the lock inode: unlinking it lets another process lock a new inode while
/// an existing installer is still writing. Lock acquisition never blocks the UI.
fn setup_lock(directory: &Path) -> Result<SetupLock, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("Cannot create Liquid's setup folder: {error}"))?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".setup.lock"))
        .map_err(|error| format!("Cannot open Liquid's setup lock: {error}"))?;
    file.try_lock().map_err(|error| match error {
        fs::TryLockError::WouldBlock => {
            "Liquid setup is already running in this folder. Try the selection again in a moment."
                .to_owned()
        }
        fs::TryLockError::Error(error) => format!("Cannot lock Liquid's setup folder: {error}"),
    })?;
    Ok(SetupLock { file })
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn config(directory: &Path) -> Config {
        Config {
            directory: directory.into(),
            threads: 4,
        }
    }
    pub(super) fn fixture(directory: &Path) -> manifest::Manifest {
        fs::create_dir_all(directory).unwrap();
        let mut files = Vec::new();
        for target in std::iter::once(None).chain(manifest::MACOS_TARGETS.map(Some)) {
            let names: Vec<&str> = match target {
                None => vec![manifest::MODEL_NAME, manifest::TOKENIZER_NAME],
                Some(target) => manifest::native_names(target).unwrap().to_vec(),
            };
            for name in names {
                let bytes = format!("A small fixture for {target:?}/{name}").into_bytes();
                let archive = codec::encode_fixture(&bytes);
                let file = manifest::Asset {
                    name: name.into(),
                    target: target.map(str::to_owned),
                    bytes: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                    compressed_bytes: archive.len() as u64,
                    compressed_sha256: format!("{:x}", Sha256::digest(&archive)),
                };
                let path = directory.join(file.archive_relative_path());
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, archive).unwrap();
                files.push(file);
            }
        }
        let value = manifest::Manifest {
            format: manifest::FORMAT,
            model_id: manifest::MODEL_ID.into(),
            upstream_revision: manifest::UPSTREAM_REVISION.into(),
            files,
        };
        value.validate().unwrap();
        fs::write(
            directory.join(manifest::MANIFEST_NAME),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        value
    }
    #[test]
    fn configuration_and_shared_handle_do_no_eager_work() {
        let root = tempfile::tempdir().unwrap();
        let config = config(&root.path().join("never-created"));
        let _client = Client::default();
        assert!(!config.directory.exists());
        assert!(Arc::ptr_eq(&shared_client(), &shared_client()));
        assert!(validate_settings("", 4).is_ok());
        assert!(validate_settings(root.path().to_str().unwrap(), 1).is_ok());
        for (folder, threads) in [("relative", 4), ("/new\nfolder", 4), ("", 0), ("", 9)] {
            assert!(validate_settings(folder, threads).is_err());
        }
        assert!(
            Config::from_settings(&crate::settings::AppSettings::default())
                .unwrap()
                .directory
                .is_absolute()
        );
    }
    #[test]
    fn cancelled_shared_client_wait_does_not_wait_for_the_other_window() {
        let client = Mutex::new(Client::default());
        let held = client.lock().unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let (entered, started) = std::sync::mpsc::channel();
            let (finished, result) = std::sync::mpsc::channel();
            let client = &client;
            let cancel = &cancel;
            scope.spawn(move || {
                entered.send(()).unwrap();
                let error = match lock_client(client, cancel) {
                    Ok(_) => panic!("the other window still holds the shared client"),
                    Err(error) => error,
                };
                finished.send(error).unwrap();
            });
            started.recv_timeout(Duration::from_secs(5)).unwrap();
            cancel.store(true, std::sync::atomic::Ordering::Release);
            assert!(
                result
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .contains("cancelled")
            );
            assert!(
                client.try_lock().is_err(),
                "the first window remains active"
            );
        });
        drop(held);
        cancel.store(false, std::sync::atomic::Ordering::Release);
        drop(lock_client(&client, &cancel).unwrap());
    }
    #[test]
    fn shared_client_lock_recovers_a_poisoned_worker_without_staying_poisoned() {
        let client = Mutex::new(Client::default());
        std::thread::scope(|scope| {
            assert!(
                scope
                    .spawn(|| {
                        let _held = client.lock().unwrap();
                        panic!("a failed application worker");
                    })
                    .join()
                    .is_err()
            );
        });
        assert!(client.is_poisoned());
        let cancel = std::sync::atomic::AtomicBool::new(false);
        assert!(lock_client(&client, &cancel).unwrap().model.is_none());
        assert!(!client.is_poisoned());
        drop(lock_client(&client, &cancel).unwrap());
    }
    #[test]
    fn invalid_requests_fail_before_any_model_or_filesystem_work() {
        let root = tempfile::tempdir().unwrap();
        let config = config(&root.path().join("absent"));
        let mut client = Client::default();
        for texts in [
            vec![],
            vec![String::new(); 16_385],
            vec!["x".repeat(1024 * 1024 + 1)],
            vec!["x".repeat(1024); 1025],
        ] {
            assert!(client.correct(&config, &texts).is_err());
            assert!(!config.directory.exists());
        }
    }
    #[test]
    fn long_selections_are_admitted_and_cancel_before_model_work() {
        let texts = vec!["The results are stable. ".repeat(1000)];
        assert!(texts[0].len() > 16_384);
        validate_input(&texts).unwrap();
        let root = tempfile::tempdir().unwrap();
        let config = config(&root.path().join("absent"));
        let mut client = Client::default();
        let cancelled = std::sync::atomic::AtomicBool::new(true);
        let error = client
            .correct_stream(&config, &texts, &cancelled, |_, _| {
                panic!("a cancelled check must not publish progress")
            })
            .unwrap_err();
        assert!(error.contains("cancelled"));
        assert!(!config.directory.exists());
        assert!(client.model.is_none());
    }

    #[test]
    fn on_demand_install_expands_verified_downloads_and_reuses_cache_offline() {
        let source = tempfile::tempdir().unwrap();
        let manifest = fixture(source.path());
        let cache = tempfile::tempdir().unwrap();
        let config = config(cache.path());
        prepare_for_test(source.path(), &config).unwrap();
        let root = config.model_directory();
        for file in manifest.files_for_target(current_target()) {
            verify_file(&root.join(file.relative_path()), file.bytes, &file.sha256).unwrap();
        }
        let modified = fs::metadata(root.join(manifest::MODEL_NAME))
            .unwrap()
            .modified()
            .unwrap();
        // A ready cache needs neither payload archive, Python nor network.
        for file in &manifest.files {
            fs::remove_file(source.path().join(file.archive_relative_path())).unwrap();
        }
        prepare_for_test(source.path(), &config).unwrap();
        assert_eq!(
            fs::metadata(root.join(manifest::MODEL_NAME))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
    }
    #[test]
    fn on_demand_install_rejects_corrupt_or_incomplete_downloads() {
        let source = tempfile::tempdir().unwrap();
        let manifest = fixture(source.path());
        let cache = tempfile::tempdir().unwrap();
        let config = config(cache.path());
        fs::write(
            source
                .path()
                .join(manifest.files[0].archive_relative_path()),
            b"broken",
        )
        .unwrap();
        let error = prepare_for_test(source.path(), &config).unwrap_err();
        assert!(error.contains("damaged"));
        assert!(
            !config
                .model_directory()
                .join(manifest::MANIFEST_NAME)
                .exists()
        );
        assert!(!config.model_directory().join(manifest::MODEL_NAME).exists());
    }
    #[test]
    fn compiled_catalog_uses_exact_pinned_release_assets_for_each_architecture() {
        let catalog = trusted_manifest().unwrap();
        assert_eq!(catalog.files.len(), 6);
        assert_eq!(
            archive_url(&catalog.files[0]),
            format!("{DOWNLOAD_BASE}liquid.onnx.zst")
        );
        for target in manifest::MACOS_TARGETS {
            let helper = catalog
                .files_for_target(target)
                .find(|file| file.name == manifest::HELPER_NAME)
                .unwrap();
            assert_eq!(
                archive_url(helper),
                format!("{DOWNLOAD_BASE}{target}-liquid-onnx.zst")
            );
            assert!(catalog.files_for_target(target).all(|file| {
                let url = archive_url(file);
                url.starts_with(
                    "https://github.com/clvnkhr/tiptoptyp/releases/download/liquid-onnxruntime-v1/",
                ) && !url.contains("/latest/")
                    && !url.contains("..")
            }));
        }
    }

    #[test]
    fn force_redownload_verifies_current_architecture_and_reuses_healthy_files() {
        let source = tempfile::tempdir().unwrap();
        let catalog = fixture(source.path());
        let cache = tempfile::tempdir().unwrap();
        let config = config(cache.path());
        prepare_for_test(source.path(), &config).unwrap();
        let before: Vec<_> = catalog
            .files_for_target(current_target())
            .map(|file| {
                let metadata =
                    fs::metadata(config.model_directory().join(file.relative_path())).unwrap();
                (file.relative_path(), metadata.modified().unwrap())
            })
            .collect();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut fetched = Vec::new();
        prepare_assets(&catalog, &config, &cancel, true, |file, output, _| {
            fetched.push(file.relative_path());
            std::io::copy(
                &mut fs::File::open(source.path().join(file.archive_relative_path())).unwrap(),
                output,
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(fetched.len(), 4);
        for (path, modified) in before {
            assert!(fetched.contains(&path));
            assert_eq!(
                fs::metadata(config.model_directory().join(path))
                    .unwrap()
                    .modified()
                    .unwrap(),
                modified
            );
        }
        verify_installed_assets(&config, &catalog).unwrap();
        prepare_assets(&catalog, &config, &cancel, false, |_, _, _| {
            panic!("a complete verified cache must not make a network request")
        })
        .unwrap();
    }

    #[test]
    fn failed_or_cancelled_download_preserves_installed_asset_and_cleans_staging() {
        let source = tempfile::tempdir().unwrap();
        let catalog = fixture(source.path());
        for cancelled in [false, true] {
            let cache = tempfile::tempdir().unwrap();
            let config = config(cache.path());
            prepare_for_test(source.path(), &config).unwrap();
            let root = config.model_directory();
            let model = root.join(manifest::MODEL_NAME);
            let before = fs::read(&model).unwrap();
            let names: std::collections::BTreeSet<_> = fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let error = prepare_assets(&catalog, &config, &cancel, true, |_, output, token| {
                output.write_all(b"partial download").unwrap();
                if cancelled {
                    token.store(true, std::sync::atomic::Ordering::Release);
                    check_cancelled(token)
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(error.contains("cancelled"), cancelled, "{error}");
            assert_eq!(fs::read(&model).unwrap(), before);
            assert_eq!(
                fs::read_dir(&root)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect::<std::collections::BTreeSet<_>>(),
                names
            );
            verify_installed_assets(&config, &catalog).unwrap();
        }
    }

    #[test]
    fn interrupted_first_install_reuses_complete_assets_without_publishing_a_receipt() {
        let source = tempfile::tempdir().unwrap();
        let catalog = fixture(source.path());
        let cache = tempfile::tempdir().unwrap();
        let config = config(cache.path());
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut count = 0;
        prepare_assets(&catalog, &config, &cancel, false, |file, output, _| {
            count += 1;
            if count == 2 {
                return Err("interrupted".into());
            }
            std::io::copy(
                &mut fs::File::open(source.path().join(file.archive_relative_path())).unwrap(),
                output,
            )
            .unwrap();
            Ok(())
        })
        .unwrap_err();
        assert_eq!(count, 2);
        assert!(
            config
                .model_directory()
                .join(manifest::MODEL_NAME)
                .is_file()
        );
        assert!(
            !config
                .model_directory()
                .join(manifest::MANIFEST_NAME)
                .exists()
        );
        let mut fetched = Vec::new();
        prepare_assets(&catalog, &config, &cancel, false, |file, output, _| {
            fetched.push(file.name.clone());
            std::io::copy(
                &mut fs::File::open(source.path().join(file.archive_relative_path())).unwrap(),
                output,
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(fetched.len(), 3);
        assert!(!fetched.iter().any(|name| name == manifest::MODEL_NAME));
        verify_installed_assets(&config, &catalog).unwrap();
    }

    #[test]
    fn cancelled_restore_preserves_existing_files_and_does_not_create_new_assets() {
        let source = tempfile::tempdir().unwrap();
        let package = fixture(source.path());
        let cache = tempfile::tempdir().unwrap();
        let config = config(cache.path());
        let cancel = std::sync::atomic::AtomicBool::new(true);
        assert!(
            prepare_assets_cancellable(source.path(), &config, &cancel)
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(!config.model_directory().exists());
        prepare_for_test(source.path(), &config).unwrap();
        let model = config.model_directory().join(manifest::MODEL_NAME);
        fs::write(&model, b"preserve interrupted recovery").unwrap();
        assert!(prepare_assets_cancellable(source.path(), &config, &cancel).is_err());
        assert_eq!(fs::read(&model).unwrap(), b"preserve interrupted recovery");
        cancel.store(false, std::sync::atomic::Ordering::Release);
        prepare_assets_cancellable(source.path(), &config, &cancel).unwrap();
        verify_installed_assets(&config, &package).unwrap();
    }

    struct CancelDuringRead<'a, R> {
        inner: R,
        cancel: &'a std::sync::atomic::AtomicBool,
        at_eof: bool,
    }
    impl<R: Read> Read for CancelDuringRead<'_, R> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = self.inner.read(output)?;
            if (count == 0) == self.at_eof {
                self.cancel
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            Ok(count)
        }
    }
    #[test]
    fn cancellation_during_hashing_is_reported_as_cancelled_without_retrying_or_finishing() {
        let bytes = vec![b'x'; 128 * 1024];
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let input = CancelDuringRead {
            inner: &bytes[..],
            cancel: &cancel,
            at_eof: false,
        };
        let error = verify_contents(input, bytes.len() as u64, &hash, &cancel).unwrap_err();
        assert!(error.contains("cancelled"));
        assert!(!error.contains("checksum"));
        let error = CancellableIo {
            inner: &bytes[..],
            cancel: &cancel,
        }
        .read(&mut [0; 1])
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
    }
    #[test]
    fn cancellation_during_decode_and_its_final_read_never_replaces_the_existing_asset() {
        let source = tempfile::tempdir().unwrap();
        let package = fixture(source.path());
        let file = package
            .files
            .iter()
            .find(|file| file.name == manifest::MODEL_NAME)
            .unwrap();
        for at_eof in [false, true] {
            let cache = tempfile::tempdir().unwrap();
            let target = cache.path().join(manifest::MODEL_NAME);
            fs::write(&target, b"preserve existing model").unwrap();
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let input = CancelDuringRead {
                inner: fs::File::open(source.path().join(file.archive_relative_path())).unwrap(),
                cancel: &cancel,
                at_eof,
            };
            let error = unpack_asset(input, file, cache.path(), &target, &cancel).unwrap_err();
            assert!(error.contains("cancelled"), "{error}");
            assert_eq!(fs::read(&target).unwrap(), b"preserve existing model");
            assert_eq!(
                fs::read_dir(cache.path()).unwrap().count(),
                1,
                "temporary decode output must be removed"
            );
        }
    }
    #[test]
    fn codec_output_observes_cancellation_even_when_the_input_is_already_buffered() {
        struct CancelDuringWrite<'a> {
            bytes: Vec<u8>,
            cancel: &'a std::sync::atomic::AtomicBool,
        }
        impl Write for CancelDuringWrite<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                self.cancel
                    .store(true, std::sync::atomic::Ordering::Release);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let bytes = b"decoded but not published";
        let archive = codec::encode_fixture(bytes);
        let hash = format!("{:x}", Sha256::digest(bytes));
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut output = CancelDuringWrite {
            bytes: Vec::new(),
            cancel: &cancel,
        };
        let error = codec::decode_bounded(
            &archive[..],
            bytes.len() as u64,
            &hash,
            &mut CancellableIo {
                inner: &mut output,
                cancel: &cancel,
            },
            manifest::MAX_FILE_BYTES,
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert!(error.to_string().contains("cancelled"));
        assert_eq!(output.bytes, bytes);
    }

    #[test]
    fn invalid_download_catalog_requires_repairing_the_app_package() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join(manifest::MANIFEST_NAME), b"{}").unwrap();
        let cache = tempfile::tempdir().unwrap();
        let error = prepare_for_test(source.path(), &config(cache.path())).unwrap_err();
        assert!(error.contains("Reinstall the app"), "{error}");
        assert!(!error.contains("Install Liquid"));
    }
    #[cfg(unix)]
    #[test]
    fn on_demand_install_rejects_redirected_cache_directories_before_writing_assets() {
        let source = tempfile::tempdir().unwrap();
        fixture(source.path());
        let cache = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), cache.path().join("onnxruntime")).unwrap();
        let config = config(cache.path());
        assert!(
            prepare_for_test(source.path(), &config)
                .unwrap_err()
                .contains("regular directory")
        );
        assert!(!outside.path().join(manifest::MODEL_ID).exists());
        assert!(verify_model_directories(&config).is_err());
    }
    #[test]
    fn setup_lock_rejects_another_installer_and_keeps_the_inode() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".setup.lock");
        fs::write(&path, b"existing lock").unwrap();
        let first = setup_lock(root.path()).unwrap();
        assert!(
            setup_lock(root.path())
                .unwrap_err()
                .contains("already running")
        );
        drop(first);
        drop(setup_lock(root.path()).unwrap());
        assert_eq!(fs::read(path).unwrap(), b"existing lock");
    }
    #[test]
    fn setup_lock_releases_even_when_a_child_inherits_its_descriptor() {
        let root = tempfile::tempdir().unwrap();
        let first = setup_lock(root.path()).unwrap();
        // A duplicated descriptor has the same lock lifetime as a descriptor
        // inherited across fork. Keep it alive to model a concurrent spawn.
        let inherited = first.file.try_clone().unwrap();
        drop(first);
        let next = setup_lock(root.path()).unwrap();
        assert!(setup_lock(root.path()).is_err());
        drop(inherited);
        assert!(
            setup_lock(root.path()).is_err(),
            "the next owner must remain locked"
        );
        drop(next);
        drop(setup_lock(root.path()).unwrap());
    }
    #[test]
    fn installed_integrity_rejects_same_size_changes_and_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("asset");
        fs::write(&path, b"correct").unwrap();
        let hash = format!("{:x}", Sha256::digest(b"correct"));
        verify_file(&path, 7, &hash).unwrap();
        fs::write(&path, b"changed").unwrap();
        assert!(verify_file(&path, 7, &hash).is_err());
        #[cfg(unix)]
        {
            fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(root.path(), &path).unwrap();
            assert!(verify_file(&path, 7, &hash).is_err());
        }
    }
    #[test]
    fn native_handshake_requires_matching_build_protocol_and_thread_count() {
        let valid = br#"{"ready":true,"protocol":1,"target":"aarch64-apple-darwin","threads":4}"#;
        validate_ready(valid, "aarch64-apple-darwin", 4).unwrap();
        assert!(validate_ready(valid, "x86_64-apple-darwin", 4).is_err());
        assert!(validate_ready(valid, "aarch64-apple-darwin", 2).is_err());
        for line in [
            br#"{"ready":false,"protocol":1,"target":"aarch64-apple-darwin","threads":4}"#
                .as_slice(),
            br#"{"ready":true,"protocol":2,"target":"aarch64-apple-darwin","threads":4}"#,
            br#"{"ready":true,"protocol":1,"target":"aarch64-apple-darwin","threads":4,"other":1}"#,
        ] {
            assert!(validate_ready(line, "aarch64-apple-darwin", 4).is_err());
        }
    }
    #[test]
    fn native_predictions_reject_stale_ids_invalid_shapes_values_and_child_errors() {
        let valid = br#"{"id":1,"ok":true,"best_tags":[0,64402],"error_probs":[0.0,0.9]}"#;
        assert_eq!(prediction_from_reply(valid, 1, 2).unwrap().tags, [0, 64402]);
        assert!(prediction_from_reply(valid, 2, 2).is_err());
        assert!(prediction_from_reply(valid, 1, 3).is_err());
        for line in [br#"{"id":1,"ok":true,"best_tags":[128802],"error_probs":[0.5]}"#.as_slice(),
            br#"{"id":1,"ok":true,"best_tags":[-1],"error_probs":[0.5]}"#,
            br#"{"id":1,"ok":true,"best_tags":[0],"error_probs":[1.01]}"#,
            br#"{"id":1,"ok":true,"best_tags":[0],"error_probs":[null]}"#,
            br#"{"id":1,"ok":true,"best_tags":[0],"error_probs":[0.5],"error":"secret selected prose"}"#,
            br#"{"id":1,"ok":false,"error":"secret selected prose"}"#] {
            let error = prediction_from_reply(line, 1, 1).err().unwrap();
            assert!(!error.contains("secret"));
        }
    }
    #[test]
    fn minimum_macos_is_specific_to_the_optional_runtime() {
        assert!(!supports_macos("aarch64-apple-darwin", 13, 9));
        assert!(supports_macos("aarch64-apple-darwin", 14, 0));
        assert!(!supports_macos("x86_64-apple-darwin", 11, 9));
        assert!(supports_macos("x86_64-apple-darwin", 12, 0));
        assert!(!supports_macos("unknown", 99, 0));
    }
    #[test]
    fn on_demand_install_downloads_only_this_architecture_and_sets_helper_permissions() {
        let source = tempfile::tempdir().unwrap();
        let value = fixture(source.path());
        let cache = tempfile::tempdir().unwrap();
        let config = config(cache.path());
        prepare_for_test(source.path(), &config).unwrap();
        for target in manifest::MACOS_TARGETS {
            assert_eq!(
                config
                    .model_directory()
                    .join("runtime")
                    .join(target)
                    .is_dir(),
                target == current_target()
            );
        }
        for file in value.files_for_target(current_target()) {
            verify_file(
                &config.model_directory().join(file.relative_path()),
                file.bytes,
                &file.sha256,
            )
            .unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let helper = config
                .model_directory()
                .join("runtime")
                .join(current_target())
                .join(manifest::HELPER_NAME);
            assert_eq!(
                fs::metadata(helper).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
    #[test]
    fn cache_receipt_cannot_authorize_a_modified_native_executable() {
        let source = tempfile::tempdir().unwrap();
        let trusted = fixture(source.path());
        let cache = tempfile::tempdir().unwrap();
        let config = config(cache.path());
        prepare_for_test(source.path(), &config).unwrap();
        verify_installed_assets(&config, &trusted).unwrap();
        let mut forged = trusted.clone();
        let file = forged
            .files
            .iter_mut()
            .find(|file| {
                file.target.as_deref() == Some(current_target())
                    && file.name == manifest::HELPER_NAME
            })
            .unwrap();
        let path = config.model_directory().join(file.relative_path());
        let changed = vec![b'x'; file.bytes as usize];
        fs::write(path, &changed).unwrap();
        file.sha256 = format!("{:x}", Sha256::digest(&changed));
        fs::write(
            config.model_directory().join(manifest::MANIFEST_NAME),
            serde_json::to_vec(&forged).unwrap(),
        )
        .unwrap();
        assert!(
            verify_installed_assets(&config, &trusted)
                .unwrap_err()
                .contains("integrity")
        );
        prepare_for_test(source.path(), &config).unwrap();
        verify_installed_assets(&config, &trusted).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn input_rejection_keeps_worker_but_a_bad_prediction_invalidates_it() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", r#"read request; printf '%s\n' '{"id":99,"ok":true,"best_tags":[0],"error_probs":[0.0]}'"#]);
        let mut model = NativeModel {
            process: JsonLineProcess::spawn(&mut command).unwrap(),
            next_id: 0,
            failed: false,
        };
        assert!(model.predict(&[]).is_err());
        assert!(model.predict(&[64_400]).is_err());
        assert!(
            !model.failed,
            "invalid input must not discard a healthy model"
        );
        assert!(model.predict(&[1]).is_err());
        assert!(model.failed, "a stale reply must invalidate the process");
    }
}
