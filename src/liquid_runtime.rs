//! Optional native grammar model. The shared client is inert until an explicit
//! selected-text request runs on an application worker; preparation is automatic and offline.
use crate::{
    bundled_payload::codec,
    liquid_text::{Prediction, TextProcessor},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};
use std::{process::Command, time::Duration};
#[path = "liquid_process.rs"]
mod process;
use process::JsonLineProcess;

#[path = "../build_support/liquid_manifest.rs"]
mod manifest;

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
            "Cannot find the application data folder. Choose a Liquid folder in Settings → Writing."
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
impl Client {
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
            self.model = Some(LoadedModel::load(config.clone())?);
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
                        loaded.inference.predict(ids)
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
    fn load(config: Config) -> Result<Self, String> {
        check_platform()?;
        let _lock = setup_lock(&config.directory)?;
        let root = config.model_directory();
        // First use transparently verifies/expands the bundled files. This
        // remains entirely on the application worker, never the frame path.
        prepare_assets(&packaged_directory()?, &config)?;
        let processor = TextProcessor::load(&root.join(manifest::TOKENIZER_NAME))?;
        let inference = NativeModel::load(&root, config.threads)?;
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
            "Liquid's runtime or model failed its integrity check. Check its data folder in Settings → Writing.".to_owned()
        })?;
    }
    Ok(())
}
#[cfg(test)]
fn unavailable_files() -> String {
    "Liquid’s local files are unavailable. Check its data folder in Settings → Writing.".into()
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
    Err("The bundled Liquid runtime currently supports macOS only.".into())
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
    fn load(root: &Path, threads: u8) -> Result<Self, String> {
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
        let line = process.receive(Duration::from_secs(120))?;
        validate_ready(&line, current_target(), threads)?;
        Ok(Self {
            process,
            next_id: 0,
            failed: false,
        })
    }
    fn predict(&mut self, ids: &[i64]) -> Result<Prediction, String> {
        if ids.is_empty() || ids.len() > 128 || ids.iter().any(|id| !(0..64_400).contains(id)) {
            return Err(
                "A selected sentence is too long or invalid for Liquid. Select shorter sentences."
                    .into(),
            );
        }
        let result = self.predict_validated(ids);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn predict_validated(&mut self, ids: &[i64]) -> Result<Prediction, String> {
        self.next_id = self.next_id.checked_add(1).ok_or_else(protocol_error)?;
        let mut request = self.next_id.to_string();
        for id in ids {
            use std::fmt::Write;
            write!(&mut request, " {id}").map_err(|_| protocol_error())?;
        }
        self.process.send(request.as_bytes())?;
        let line = self.process.receive(Duration::from_secs(30))?;
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

fn packaged_directory() -> Result<PathBuf, String> {
    let executable =
        std::env::current_exe().map_err(|_| "Cannot locate Liquid's bundled model.".to_owned())?;
    let candidates = crate::bundled_payload::resource_roots(&executable)
        .into_iter()
        .map(|root| root.join("liquid"))
        .collect::<Vec<_>>();
    #[cfg(not(feature = "production"))]
    let candidates = {
        let mut candidates = candidates;
        candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("toolchain/liquid"));
        candidates
    };
    candidates
        .into_iter()
        .find(|root| root.join(manifest::MANIFEST_NAME).is_file())
        .ok_or_else(|| {
            "Liquid setup cannot find its bundled model. Reinstall the complete app package."
                .to_owned()
        })
}
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
fn verify_file(path: &Path, bytes: u64, expected: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "Liquid file is missing".to_owned())?;
    if !metadata.file_type().is_file() || metadata.len() != bytes {
        return Err("Liquid file has an invalid size or type".into());
    }
    let mut file = fs::File::open(path)
        .map_err(|_| "Cannot read Liquid file".to_owned())?
        .take(bytes + 1);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut actual = 0u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "Cannot verify Liquid file".to_owned())?;
        if count == 0 {
            break;
        }
        actual += count as u64;
        hasher.update(&buffer[..count]);
    }
    if actual != bytes || format!("{:x}", hasher.finalize()) != expected {
        return Err("Liquid file checksum does not match".into());
    }
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
                "Liquid’s local files are unavailable. Check its data folder in Settings → Writing."
                    .into(),
            );
        }
    }
    Ok(())
}
fn prepare_assets(source: &Path, config: &Config) -> Result<(), String> {
    let packaged = read_manifest(source).map_err(|e| {
        format!(
            "Liquid setup found an invalid bundled model: {e}. Reinstall the complete app package."
        )
    })?;
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
    for file in packaged.files_for_target(current_target()) {
        let target = root.join(file.relative_path());
        if verify_file(&target, file.bytes, &file.sha256).is_ok() {
            #[cfg(unix)]
            if file.name == manifest::HELPER_NAME {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o700))
                    .map_err(|e| format!("Liquid setup cannot make its helper executable: {e}"))?;
            }
            continue;
        }
        let archive = source.join(file.archive_relative_path());
        verify_file(&archive, file.compressed_bytes, &file.compressed_sha256).map_err(|_| {
            "Liquid setup found a damaged bundled model. Reinstall the complete app package."
                .to_owned()
        })?;
        let input = fs::File::open(archive)
            .map_err(|e| format!("Liquid setup cannot read its bundled model: {e}"))?;
        let mut output = tempfile::NamedTempFile::new_in(&root)
            .map_err(|e| format!("Liquid setup cannot prepare its model: {e}"))?;
        codec::decode_bounded(
            input,
            file.bytes,
            &file.sha256,
            &mut output,
            manifest::MAX_FILE_BYTES,
        )
        .map_err(|e| format!("Liquid setup cannot unpack its model: {e}"))?;
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
        output
            .persist(target)
            .map_err(|e| format!("Liquid setup cannot publish its model: {e}"))?;
    }
    let mut output = tempfile::NamedTempFile::new_in(&root)
        .map_err(|e| format!("Liquid setup cannot prepare its manifest: {e}"))?;
    serde_json::to_writer(&mut output, &packaged)
        .map_err(|e| format!("Liquid setup cannot save its manifest: {e}"))?;
    output
        .flush()
        .map_err(|e| format!("Liquid setup cannot save its manifest: {e}"))?;
    output
        .persist(root.join(manifest::MANIFEST_NAME))
        .map_err(|e| format!("Liquid setup cannot publish its manifest: {e}"))?;
    Ok(())
}
#[cfg(test)]
fn prepare_for_test(source: &Path, config: &Config) -> Result<(), String> {
    let _lock = setup_lock(&config.directory)?;
    prepare_assets(source, config)
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
    fn config(directory: &Path) -> Config {
        Config {
            directory: directory.into(),
            threads: 4,
        }
    }
    fn fixture(directory: &Path) -> manifest::Manifest {
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
    fn automatic_preparation_expands_verified_assets_offline_and_reuses_existing_files() {
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
    fn automatic_preparation_rejects_corrupt_or_incomplete_bundled_model() {
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
    fn invalid_bundled_manifest_requires_repairing_the_app_package() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join(manifest::MANIFEST_NAME), b"{}").unwrap();
        let cache = tempfile::tempdir().unwrap();
        let error = prepare_for_test(source.path(), &config(cache.path())).unwrap_err();
        assert!(
            error.contains("Reinstall the complete app package"),
            "{error}"
        );
        assert!(!error.contains("Install Liquid"));
    }
    #[cfg(unix)]
    #[test]
    fn automatic_preparation_rejects_redirected_cache_directories_before_writing_assets() {
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
    fn automatic_preparation_extracts_only_this_architecture_and_sets_helper_permissions() {
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
