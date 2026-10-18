//! Lazy, verified expansion of architecture-specific packaged sidecars.
//!
//! Resolution only reads the small manifest. Actual extraction belongs to the
//! existing tool-launch/PDF worker, never the Settings or frame paths.
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

#[path = "../build_support/payload_codec.rs"]
mod codec;

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_PAYLOAD_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug)]
struct Payload {
    archive: PathBuf,
    name: String,
    bytes: u64,
    sha256: String,
    executable: bool,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// All supported resource layouts. This is pure path construction: probing
/// availability and expanding payloads remain separate operation boundaries.
pub(crate) fn resource_roots(executable: &Path) -> Vec<PathBuf> {
    let Some(directory) = executable.parent() else {
        return Vec::new();
    };
    vec![
        directory.join("../Resources"),
        directory.to_owned(),
        directory.join("../lib/tiptoptyp"),
    ]
}

/// Native-architecture payloads are kept separate even in a universal bundle.
/// These paths are inexpensive resolution tokens, not executable cache files.
pub(crate) fn packaged_candidates(name: &str) -> Vec<PathBuf> {
    let Ok(executable) = std::env::current_exe() else {
        return Vec::new();
    };
    resource_roots(&executable)
        .into_iter()
        .map(|root| {
            root.join("toolchain")
                .join(crate::toolchain::target_triple())
                .join(format!("{name}.zst"))
        })
        .collect()
}

pub(crate) fn is_packaged_payload(path: &Path) -> bool {
    describe(path).is_ok_and(|payload| payload.is_some())
}

/// Checks the manifest and resource metadata without creating a cache or
/// reading/decompressing the payload. Full integrity is verified at first use.
pub(crate) fn validate_packaged(path: &Path) -> io::Result<()> {
    describe(path)?
        .map(|_| ())
        .ok_or_else(|| invalid("Packaged payload manifest is missing"))
}

/// A custom executable is left alone. A manifest-backed Zstandard path is expanded
/// only here, after operation arguments have been validated by its caller.
pub(crate) fn prepare_program(program: &OsStr) -> io::Result<PathBuf> {
    let path = Path::new(program);
    let Some(payload) = describe(path)? else {
        return Ok(path.to_owned());
    };
    let root = cache_root(crate::toolchain::target_triple())?;
    expand(&payload, &root)
}

/// Libraries are required to be a declared payload rather than a custom tool.
pub(crate) fn prepare_packaged(path: &Path) -> io::Result<PathBuf> {
    let payload = describe(path)?.ok_or_else(|| invalid("Packaged payload manifest is missing"))?;
    expand(&payload, &cache_root(crate::toolchain::target_triple())?)
}

/// Fonts and recognition models are shared by both universal-binary slices.
/// They stay non-executable and reuse one content-addressed cache across CPUs.
#[cfg(all(feature = "production", not(test)))]
pub(crate) fn prepare_shared_asset(path: &Path) -> io::Result<PathBuf> {
    let mut payload =
        describe(path)?.ok_or_else(|| invalid("Packaged asset manifest is missing"))?;
    payload.executable = false;
    expand(&payload, &cache_root("shared")?)
}

fn describe(archive: &Path) -> io::Result<Option<Payload>> {
    if archive.extension() != Some(OsStr::new("zst")) {
        return Ok(None);
    }
    let Some(parent) = archive.parent() else {
        return Ok(None);
    };
    let manifest = parent.join("manifest.tsv");
    if !manifest.try_exists()? {
        return Ok(None);
    }
    let name = archive
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| invalid("Packaged payload has an invalid filename"))?;
    if !fs::symlink_metadata(&manifest)?.file_type().is_file() {
        return Err(invalid("Packaged payload manifest must be a regular file"));
    }
    let mut text = String::new();
    File::open(&manifest)?
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(invalid("Packaged payload manifest is too large"));
    }
    let mut names = HashSet::new();
    let mut selected = None;
    for (index, line) in text.lines().enumerate() {
        let mut fields = line.split('\t');
        let entry = fields.next().unwrap_or("");
        let bytes = fields.next().and_then(|value| value.parse::<u64>().ok());
        let sha256 = fields.next().unwrap_or("");
        if !valid_name(entry)
            || fields.next().is_some()
            || !matches!(bytes, Some(1..=MAX_PAYLOAD_BYTES))
            || sha256.len() != 64
            || !sha256
                .bytes()
                .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
            || !names.insert(entry)
            || index >= 32
        {
            return Err(invalid(format!(
                "Invalid packaged payload manifest at line {}",
                index + 1
            )));
        }
        if entry == name {
            selected = Some(Payload {
                archive: archive.to_owned(),
                name: entry.to_owned(),
                bytes: bytes.unwrap(),
                sha256: sha256.to_owned(),
                executable: !is_library(entry),
            });
        }
    }
    if names.is_empty() {
        return Err(invalid("Packaged payload manifest is empty"));
    }
    let payload = selected.ok_or_else(|| {
        invalid(format!(
            "Packaged payload {name} is absent from its manifest"
        ))
    })?;
    if !fs::symlink_metadata(archive)?.file_type().is_file() {
        return Err(invalid("Packaged payload must be a regular Zstandard file"));
    }
    Ok(Some(payload))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b'.'))
        && !name.contains("..")
}

fn cache_root(scope: &str) -> io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")));
    let base = base
        .filter(|path| path.is_absolute())
        .ok_or_else(|| invalid("Cannot locate the private application cache"))?;
    fs::create_dir_all(&base)?;
    let mut root = base.canonicalize()?;
    for component in ["dev.tiptoptyp.editor", "payloads", scope] {
        root.push(component);
        private_directory(&root)?;
    }
    Ok(root)
}

fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(invalid(
            "Packaged payload cache directory is not a regular directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != nix::unistd::geteuid().as_raw() {
            return Err(invalid("Packaged payload cache belongs to another user"));
        }
        if metadata.mode() & 0o777 != 0o700 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

fn expand(payload: &Payload, root: &Path) -> io::Result<PathBuf> {
    private_directory(root)?;
    let directory = root.join(&payload.sha256);
    private_directory(&directory)?;
    let destination = directory.join(&payload.name);
    if valid_cached(&destination, payload)? {
        return Ok(destination);
    }
    // Unique temporary files prevent concurrent first launches from reading an
    // incomplete extraction. No process lock can survive a crashed first use.
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
    codec::decode(
        File::open(&payload.archive)?,
        payload.bytes,
        &payload.sha256,
        &mut temporary,
    )?;
    temporary.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if payload.executable { 0o700 } else { 0o600 };
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }
    temporary.as_file().sync_all()?;
    // A corrupt older entry is replaced atomically. Concurrent writers produce
    // identical verified bytes, so either complete result is valid.
    match temporary.persist(&destination) {
        Ok(file) => {
            remember_verified(&destination, payload, CacheStamp::of(&file.metadata()?));
            Ok(destination)
        }
        Err(_) if valid_cached(&destination, payload)? => Ok(destination),
        Err(error) => Err(error.error),
    }
}

fn is_library(name: &str) -> bool {
    name.ends_with(".dylib") || name.ends_with(".dll") || name.ends_with(".so")
}

// This memo is bounded and touched only by operation workers. A tool launch
// stats its immutable cache file; unchanged executables do not get re-read or
// hashed for every compile/format request. Any metadata change rechecks bytes.
const MAX_VERIFIED_FILES: usize = 64;
#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheStamp {
    bytes: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    readonly: bool,
    #[cfg(unix)]
    native: (u64, u64, i64, i64, i64, i64, u32, u32, u32),
}
impl CacheStamp {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            bytes: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            readonly: metadata.permissions().readonly(),
            #[cfg(unix)]
            native: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.mtime(),
                    metadata.mtime_nsec(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                    metadata.mode(),
                    metadata.uid(),
                    metadata.gid(),
                )
            },
        }
    }
}
struct VerifiedFile {
    sha256: String,
    bytes: u64,
    executable: bool,
    stamp: CacheStamp,
}
fn verified_files() -> &'static Mutex<HashMap<PathBuf, VerifiedFile>> {
    static FILES: OnceLock<Mutex<HashMap<PathBuf, VerifiedFile>>> = OnceLock::new();
    FILES.get_or_init(Mutex::default)
}
fn remember_verified(path: &Path, payload: &Payload, stamp: CacheStamp) {
    let mut files = verified_files()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if files.len() >= MAX_VERIFIED_FILES && !files.contains_key(path) {
        files.clear();
    }
    files.insert(
        path.to_owned(),
        VerifiedFile {
            sha256: payload.sha256.clone(),
            bytes: payload.bytes,
            executable: payload.executable,
            stamp,
        },
    );
}
#[cfg(test)]
thread_local! {
    static CACHE_HASH_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn valid_cached(path: &Path, payload: &Payload) -> io::Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file() {
        return Err(invalid(
            "Packaged payload cache entry is not a regular file",
        ));
    }
    if metadata.len() != payload.bytes {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != nix::unistd::geteuid().as_raw() {
            return Err(invalid("Packaged payload cache belongs to another user"));
        }
        let expected = if payload.executable { 0o700 } else { 0o600 };
        if metadata.mode() & 0o777 != expected {
            return Ok(false);
        }
    }
    let stamp = CacheStamp::of(&metadata);
    if verified_files()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(path)
        .is_some_and(|entry| {
            entry.sha256 == payload.sha256
                && entry.bytes == payload.bytes
                && entry.executable == payload.executable
                && entry.stamp == stamp
        })
    {
        return Ok(true);
    }
    let file = File::open(path)?;
    if CacheStamp::of(&file.metadata()?) != stamp {
        return Err(invalid("Packaged payload cache changed while opening"));
    }
    #[cfg(test)]
    CACHE_HASH_READS.with(|reads| reads.set(reads.get() + 1));
    let mut hasher = Sha256::new();
    let mut limited = file.take(payload.bytes + 1);
    let mut count = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = limited.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        hasher.update(&buffer[..read]);
    }
    if count != payload.bytes || format!("{:x}", hasher.finalize()) != payload.sha256 {
        return Ok(false);
    }
    let current = fs::symlink_metadata(path)?;
    if !current.file_type().is_file()
        || CacheStamp::of(&limited.get_ref().metadata()?) != stamp
        || CacheStamp::of(&current) != stamp
    {
        return Err(invalid(
            "Packaged payload cache changed during verification",
        ));
    }
    remember_verified(path, payload, stamp);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(bytes: &[u8]) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("typst.zst");
        fs::write(&archive, codec::encode_fixture(bytes)).unwrap();
        fs::write(
            directory.path().join("manifest.tsv"),
            format!("typst\t{}\t{:x}\n", bytes.len(), Sha256::digest(bytes)),
        )
        .unwrap();
        let cache = directory.path().join("cache");
        (directory, archive, cache)
    }

    #[test]
    fn resource_roots_cover_bundle_portable_and_linux_package_layouts() {
        let directory = Path::new("package").join("bin");
        let executable = directory.join("tiptoptyp");
        assert_eq!(
            resource_roots(&executable),
            vec![
                directory.join("../Resources"),
                directory.clone(),
                directory.join("../lib/tiptoptyp"),
            ]
        );
        for root in resource_roots(&executable) {
            let tools = root
                .join("toolchain")
                .join(crate::toolchain::target_triple())
                .join("typst.zst");
            let assets = root.join("assets/shared/noto-sans.ttf.zst");
            assert_eq!(
                tools.strip_prefix(&root).unwrap(),
                Path::new("toolchain")
                    .join(crate::toolchain::target_triple())
                    .join("typst.zst")
            );
            assert_eq!(
                assets.strip_prefix(&root).unwrap(),
                Path::new("assets/shared/noto-sans.ttf.zst")
            );
        }
        assert!(resource_roots(Path::new("/")).is_empty());
    }

    #[test]
    fn discovery_is_lazy_and_expansion_is_exact_and_reusable() {
        let (fixture, archive, cache) = fixture(b"#!/bin/sh\nprintf ready\n");
        assert!(is_packaged_payload(&archive));
        assert!(
            !cache.exists(),
            "availability checks must never inflate payloads"
        );
        let payload = describe(&archive).unwrap().unwrap();
        let extracted = expand(&payload, &cache).unwrap();
        assert_eq!(fs::read(&extracted).unwrap(), b"#!/bin/sh\nprintf ready\n");
        let modified = extracted.metadata().unwrap().modified().unwrap();
        fs::remove_file(&archive).unwrap();
        assert_eq!(expand(&payload, &cache).unwrap(), extracted);
        assert_eq!(extracted.metadata().unwrap().modified().unwrap(), modified);
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 2);
    }

    #[test]
    fn presence_validation_requires_a_declared_resource_without_expanding_it() {
        let (directory, archive, cache) = fixture(b"verified payload");
        validate_packaged(&archive).unwrap();
        assert!(!cache.exists());
        std::fs::remove_file(&archive).unwrap();
        assert!(validate_packaged(&archive).is_err());
        std::fs::write(&archive, b"deferred integrity check").unwrap();
        std::fs::remove_file(directory.path().join("manifest.tsv")).unwrap();
        assert!(validate_packaged(&archive).is_err());
        assert!(!cache.exists());
    }

    #[test]
    #[ignore = "optimized PDFium payload presence versus extraction component measurement"]
    fn payload_presence_cache_measurement() {
        use std::{hint::black_box, time::Instant};
        assert!(
            black_box(!cfg!(debug_assertions)),
            "run this measurement with --release"
        );
        let name = pdfium_render::prelude::Pdfium::pdfium_platform_library_name();
        let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("toolchain/payloads")
            .join(crate::toolchain::target_triple())
            .join(format!("{}.zst", name.to_string_lossy()));
        let payload = describe(&archive).unwrap().unwrap();
        assert!(payload.bytes <= 64 * 1024 * 1024);
        // Warm archive I/O equally before both paths. This measures a fresh
        // application cache, never a claimed cold filesystem or whole launch.
        let compressed = fs::read(&archive).unwrap();
        let compressed_sha256 = format!("{:x}", Sha256::digest(&compressed));
        let compressed_bytes = compressed.len();
        drop(compressed);
        let directory = tempfile::tempdir().unwrap();
        let reused_cache = directory.path().join("reused");
        let extracted = expand(&payload, &reused_cache).unwrap();
        let mut presence_us = Vec::new();
        let mut fresh_cache_us = Vec::new();
        let mut existing_cache_first_use_us = Vec::new();
        let mut memo_hit_us = Vec::new();
        const SAMPLES: usize = 5;
        for sample in 0..SAMPLES {
            let cache = directory.path().join(format!("fresh-{sample}"));
            let started = Instant::now();
            black_box(expand(&describe(&archive).unwrap().unwrap(), &cache).unwrap());
            fresh_cache_us.push(started.elapsed().as_secs_f64() * 1e6);

            verified_files().lock().unwrap().remove(&extracted);
            let started = Instant::now();
            black_box(expand(&describe(&archive).unwrap().unwrap(), &reused_cache).unwrap());
            existing_cache_first_use_us.push(started.elapsed().as_secs_f64() * 1e6);

            let started = Instant::now();
            black_box(expand(&describe(&archive).unwrap().unwrap(), &reused_cache).unwrap());
            memo_hit_us.push(started.elapsed().as_secs_f64() * 1e6);

            let started = Instant::now();
            validate_packaged(black_box(&archive)).unwrap();
            presence_us.push(started.elapsed().as_secs_f64() * 1e6);
        }
        println!(
            "PAYLOAD_PRESENCE_CACHE {}",
            serde_json::json!({
                "profile": "release", "target": crate::toolchain::target_triple(),
                "fixture": archive.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap(),
                "compressed_bytes": compressed_bytes, "compressed_sha256": compressed_sha256,
                "uncompressed_bytes": payload.bytes, "uncompressed_sha256": payload.sha256,
                "samples": SAMPLES, "warmup_extractions": 1,
                "reference_fresh_application_cache_us": fresh_cache_us,
                "reference_existing_cache_first_use_us": existing_cache_first_use_us,
                "reference_memo_hit_us": memo_hit_us, "presence_only_us": presence_us,
                "scope": "manifest/resource presence versus prepare (decode/hash/atomic publication or cache verification), warmed source archive; excludes native library binding, fonts, process startup, GUI and whole-app launch"
            })
        );
    }

    #[test]
    fn shared_data_is_verified_and_never_marked_executable() {
        let (_fixture, archive, cache) = fixture(b"font or recognition model bytes");
        let mut payload = describe(&archive).unwrap().unwrap();
        payload.executable = false;
        let path = expand(&payload, &cache).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"font or recognition model bytes");
        assert!(valid_cached(&path, &payload).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn unchanged_cache_skips_byte_reads_but_same_size_edits_are_reverified() {
        let (_fixture, archive, cache) = fixture(b"complete verified payload");
        let payload = describe(&archive).unwrap().unwrap();
        let extracted = expand(&payload, &cache).unwrap();
        let before = CACHE_HASH_READS.with(std::cell::Cell::get);
        for _ in 0..4 {
            assert!(valid_cached(&extracted, &payload).unwrap());
        }
        assert_eq!(
            CACHE_HASH_READS.with(std::cell::Cell::get),
            before,
            "freshly verified extraction needs no immediate second read"
        );
        // Model the first launch of another app process, whose memo is empty.
        verified_files().lock().unwrap().remove(&extracted);
        assert!(valid_cached(&extracted, &payload).unwrap());
        assert_eq!(CACHE_HASH_READS.with(std::cell::Cell::get), before + 1);
        assert!(valid_cached(&extracted, &payload).unwrap());
        assert_eq!(
            CACHE_HASH_READS.with(std::cell::Cell::get),
            before + 1,
            "unchanged subsequent launches must not rehash the executable"
        );
        fs::write(&extracted, vec![b'!'; payload.bytes as usize]).unwrap();
        assert!(!valid_cached(&extracted, &payload).unwrap());
        assert_eq!(
            CACHE_HASH_READS.with(std::cell::Cell::get),
            before + 2,
            "same-length corruption must invalidate the metadata memo"
        );
        expand(&payload, &cache).unwrap();
        assert_eq!(fs::read(&extracted).unwrap(), b"complete verified payload");
    }

    #[test]
    fn corrupted_cache_is_replaced_without_exposing_partial_output() {
        let (_fixture, archive, cache) = fixture(b"complete verified payload");
        let payload = describe(&archive).unwrap().unwrap();
        let extracted = expand(&payload, &cache).unwrap();
        fs::write(&extracted, vec![b'!'; payload.bytes as usize]).unwrap();
        assert_eq!(expand(&payload, &cache).unwrap(), extracted);
        assert_eq!(fs::read(extracted).unwrap(), b"complete verified payload");
        assert_eq!(fs::read_dir(cache.join(payload.sha256)).unwrap().count(), 1);
    }

    #[test]
    fn concurrent_first_use_only_publishes_complete_verified_files() {
        let (_fixture, archive, cache) = fixture(&vec![b'x'; 128 * 1024]);
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let archive = archive.clone();
                let cache = cache.clone();
                std::thread::spawn(move || {
                    expand(&describe(&archive).unwrap().unwrap(), &cache).unwrap()
                })
            })
            .collect();
        let paths: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(paths.iter().all(|path| path == &paths[0]));
        assert_eq!(fs::read(&paths[0]).unwrap(), vec![b'x'; 128 * 1024]);
        assert_eq!(fs::read_dir(paths[0].parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn rejects_bad_manifests_hashes_sizes_and_truncated_zstd() {
        for record in [
            "../escape\t3\tHASH",
            "typst\t0\tHASH",
            "typst\t1073741825\tHASH",
            "typst\t3\tbad",
            "typst\t3\tHASH\textra",
            "",
            "typst\t3\tHASH\ntypst\t3\tHASH",
        ] {
            let (fixture, archive, _) = fixture(b"abc");
            let record = record.replace("HASH", &format!("{:x}", Sha256::digest(b"abc")));
            fs::write(fixture.path().join("manifest.tsv"), record).unwrap();
            assert!(describe(&archive).is_err());
        }
        for bytes in [b"wrong".as_slice(), b"ab".as_slice(), b"abcdef".as_slice()] {
            let (fixture, archive, cache) = fixture(bytes);
            fs::write(
                fixture.path().join("manifest.tsv"),
                format!("typst\t3\t{:x}", Sha256::digest(b"abc")),
            )
            .unwrap();
            assert!(expand(&describe(&archive).unwrap().unwrap(), &cache).is_err());
            assert!(
                fs::read_dir(cache)
                    .unwrap()
                    .flat_map(|entry| fs::read_dir(entry.unwrap().path()).unwrap())
                    .next()
                    .is_none()
            );
        }
        let (_fixture, archive, cache) = fixture(b"abc");
        let bytes = fs::read(&archive).unwrap();
        fs::write(&archive, &bytes[..bytes.len() - 3]).unwrap();
        assert!(expand(&describe(&archive).unwrap().unwrap(), &cache).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn cache_symlinks_cannot_redirect_extraction() {
        use std::os::unix::fs::symlink;
        let (fixture, archive, cache) = fixture(b"abc");
        let payload = describe(&archive).unwrap().unwrap();
        fs::create_dir(&cache).unwrap();
        symlink(fixture.path(), cache.join(&payload.sha256)).unwrap();
        assert!(expand(&payload, &cache).is_err());
        fs::remove_file(cache.join(&payload.sha256)).unwrap();
        let extracted = expand(&payload, &cache).unwrap();
        fs::remove_file(&extracted).unwrap();
        let outside = fixture.path().join("outside");
        fs::write(&outside, b"untouched").unwrap();
        symlink(&outside, &extracted).unwrap();
        assert!(expand(&payload, &cache).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"untouched");
    }

    #[test]
    fn non_payload_commands_remain_unchanged() {
        assert_eq!(
            prepare_program(OsStr::new("typst")).unwrap(),
            Path::new("typst")
        );
        assert_eq!(
            prepare_program(OsStr::new("/bin/sh")).unwrap(),
            Path::new("/bin/sh")
        );
    }
}
