//! Shared ONNX model and separately verified Intel/Apple Silicon runtimes.
//! Export/build tools prepare raw inputs; packaging never mutates those inputs.
use super::payloads::codec;
use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
};
#[path = "../../build_support/liquid_manifest.rs"]
mod manifest;

const SOURCE: &str = "toolchain/liquid-source";
const DESTINATION: &str = "toolchain/liquid";
const COMPRESSION_LEVEL: i32 = 9;
const MAX_PROVENANCE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LICENSE_BYTES: u64 = 16 * 1024 * 1024;
const PARITY_FIXTURE_SHA256: &str =
    "3567a30d66d213ad47a3fbd7cbedad4974ea9b6e19037326da82ee5050159dc2";
const PARITY_CASES: u64 = 268;
const PARITY_STEPS: u64 = 427;
const LICENSE_SHA256: &str = "4d28ca14dedc0b3d0fcc2b3339f0e79931faa33874f3d24f522183a8fc70068c";
type NativeVerifier = fn(&Path, &str) -> Result<(), String>;

fn native_archive_sha256(target: &str) -> Option<&'static str> {
    match target {
        "aarch64-apple-darwin" => {
            Some("1031b36dd3bdaa8d976a6130d9cf2a05e9be320b93ae804f8427b91dc3432afc")
        }
        "x86_64-apple-darwin" => {
            Some("6536e36d7ea92e32d53dad7ddd0fdf10be5b62d1dace85a13e1295ff81e9b5d4")
        }
        _ => None,
    }
}
fn notice_files() -> Vec<(String, u64)> {
    std::iter::once(("LICENSE.txt".into(), MAX_LICENSE_BYTES))
        .chain(std::iter::once((
            "PROVENANCE.json".into(),
            MAX_PROVENANCE_BYTES,
        )))
        .chain(
            manifest::MACOS_TARGETS
                .map(|target| (format!("licenses/{target}.txt"), MAX_LICENSE_BYTES)),
        )
        .collect()
}
fn descriptors() -> Vec<manifest::Asset> {
    let mut files: Vec<_> = [manifest::MODEL_NAME, manifest::TOKENIZER_NAME]
        .into_iter()
        .map(|name| descriptor(None, name))
        .collect();
    for target in manifest::MACOS_TARGETS {
        files.extend(
            manifest::native_names(target)
                .unwrap()
                .iter()
                .map(|name| descriptor(Some(target), name)),
        );
    }
    files
}
fn descriptor(target: Option<&str>, name: &str) -> manifest::Asset {
    manifest::Asset {
        name: name.into(),
        target: target.map(str::to_owned),
        bytes: 1,
        sha256: "0".repeat(64),
        compressed_bytes: 1,
        compressed_sha256: "0".repeat(64),
    }
}

/// Fail before downloading unrelated sidecars or compiling both app slices.
pub(super) fn preflight() -> Result<(), String> {
    preflight_tree(&repository_root().join(SOURCE))
}
fn preflight_tree(source: &Path) -> Result<(), String> {
    check_package_layout(source).map(|_| ()).map_err(|error| {
        format!("The native Liquid package is not prepared: {error}. Recover an existing compressed package with cargo xtask ensure-liquid --source APP/Contents/Resources/liquid, or build/export raw inputs as described in docs/grammar-comparison.md and run cargo xtask prepare-liquid --source PATH. Python is a developer export tool only; the app does not need it.")
    })
}

/// Recover generated assets without exporting the model again. An existing
/// prepared source remains authoritative, including any validation errors.
pub(super) fn ensure_prepared(installed: Option<&Path>) -> Result<(), String> {
    let root = repository_root();
    let mut candidates = vec![root.join(DESTINATION)];
    candidates.extend(installed.map(Path::to_path_buf));
    if let Some(recovered) = ensure_prepared_tree(&root.join(SOURCE), &candidates, verify_native)? {
        println!(
            "Restored the verified Liquid package from {}",
            recovered.display()
        );
    }
    Ok(())
}
fn exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}
fn ensure_prepared_tree(
    prepared: &Path,
    candidates: &[PathBuf],
    verifier: NativeVerifier,
) -> Result<Option<PathBuf>, String> {
    if exists(prepared)? {
        preflight_tree(prepared)?;
        return Ok(None);
    }
    for candidate in candidates {
        if exists(candidate)? {
            stage_tree(candidate, prepared, verifier).map_err(|error| {
                format!("Cannot recover Liquid from {}: {error}. The prepared package was not replaced.", candidate.display())
            })?;
            return Ok(Some(candidate.clone()));
        }
    }
    preflight_tree(prepared)?;
    Ok(None)
}
pub(super) fn prepare(source: &Path) -> Result<(), String> {
    prepare_tree(source, &repository_root().join(SOURCE), verify_native)
}
pub(super) fn stage() -> Result<(), String> {
    let root = repository_root();
    stage_tree(&root.join(SOURCE), &root.join(DESTINATION), verify_native)
}

fn raw_manifest(source: &Path, verifier: NativeVerifier) -> Result<manifest::Manifest, String> {
    let mut files = descriptors();
    let expected = notice_files()
        .into_iter()
        .map(|(name, _)| name)
        .chain(files.iter().map(manifest::Asset::relative_path))
        .collect();
    exact_layout(source, &expected)?;
    for file in &mut files {
        let path = source.join(file.relative_path());
        file.bytes = regular_file(&path, manifest::MAX_FILE_BYTES)?.len();
    }
    let value = manifest::Manifest {
        format: manifest::FORMAT,
        model_id: manifest::MODEL_ID.into(),
        upstream_revision: manifest::UPSTREAM_REVISION.into(),
        files,
    };
    value.validate()?;
    let mut value = value;
    for file in &mut value.files {
        let path = source.join(file.relative_path());
        file.sha256 = sha256(&path)?;
        if let Some(target) = &file.target {
            verifier(&path, target)?;
        }
    }
    validate_notices(source, &value)?;
    Ok(value)
}
fn separate_trees(source: &Path, destination: &Path) -> Result<(), String> {
    let canonical_source = fs::canonicalize(source).map_err(|error| error.to_string())?;
    let canonical_destination =
        fs::canonicalize(destination)
            .or_else(|_| {
                Ok::<_, std::io::Error>(
                    fs::canonicalize(destination.parent().ok_or_else(|| {
                        std::io::Error::other("Liquid destination has no parent")
                    })?)?
                    .join(
                        destination.file_name().ok_or_else(|| {
                            std::io::Error::other("Liquid destination has no name")
                        })?,
                    ),
                )
            })
            .map_err(|error| error.to_string())?;
    if canonical_source.starts_with(&canonical_destination)
        || canonical_destination.starts_with(&canonical_source)
    {
        return Err("Liquid source and destination must not overlap".into());
    }
    Ok(())
}
fn prepare_tree(source: &Path, destination: &Path, verifier: NativeVerifier) -> Result<(), String> {
    separate_trees(source, destination)?;
    let mut manifest = raw_manifest(source, verifier)?;
    let parent = destination
        .parent()
        .ok_or("Liquid destination has no parent")?;
    let temporary = payloads::temporary_tree(parent)?;
    for (name, limit) in notice_files() {
        copy_bounded(&source.join(&name), &temporary.path().join(&name), limit)?;
    }
    for file in &mut manifest.files {
        let archive = temporary.path().join(file.archive_relative_path());
        if let Some(parent) = archive.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        compress_file(&source.join(file.relative_path()), &archive, file)?;
    }
    manifest.validate()?;
    fs::write(
        temporary.path().join(manifest::MANIFEST_NAME),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    // Decode and inspect the produced native files, not the originals. Failure
    // preserves both raw inputs and the preceding complete prepared package.
    verify_tree(temporary.path(), verifier)?;
    payloads::publish_tree(temporary, destination)
}
fn compress_file(source: &Path, archive: &Path, asset: &mut manifest::Asset) -> Result<(), String> {
    let mut input = fs::File::open(source)
        .map_err(|error| error.to_string())?
        .take(asset.bytes + 1);
    let output = fs::File::create(archive).map_err(|error| error.to_string())?;
    let mut encoder = zstd::stream::write::Encoder::new(output, COMPRESSION_LEVEL)
        .map_err(|error| error.to_string())?;
    encoder
        .include_checksum(true)
        .and_then(|_| encoder.window_log(codec::WINDOW_LOG))
        .and_then(|_| encoder.set_pledged_src_size(Some(asset.bytes)))
        .map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut actual = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        actual += count as u64;
        hash.update(&buffer[..count]);
        encoder
            .write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
    }
    if actual != asset.bytes || format!("{:x}", hash.finalize()) != asset.sha256 {
        return Err(format!(
            "Liquid input changed while packaging: {}",
            asset.relative_path()
        ));
    }
    encoder.finish().map_err(|error| error.to_string())?;
    asset.compressed_bytes = regular_file(archive, manifest::MAX_FILE_BYTES)?.len();
    asset.compressed_sha256 = sha256(archive)?;
    Ok(())
}
fn stage_tree(source: &Path, destination: &Path, verifier: NativeVerifier) -> Result<(), String> {
    separate_trees(source, destination)?;
    let manifest = check_package_layout(source)?;
    let temporary = payloads::temporary_tree(
        destination
            .parent()
            .ok_or("Liquid destination has no parent")?,
    )?;
    let files = notice_files()
        .into_iter()
        .chain(std::iter::once((
            manifest::MANIFEST_NAME.into(),
            manifest::MAX_MANIFEST_BYTES,
        )))
        .chain(
            manifest
                .files
                .iter()
                .map(|file| (file.archive_relative_path(), file.compressed_bytes)),
        );
    for (name, limit) in files {
        copy_bounded(&source.join(&name), &temporary.path().join(&name), limit)?;
    }
    verify_tree(temporary.path(), verifier)?;
    payloads::publish_tree(temporary, destination)
}

fn check_package_layout(directory: &Path) -> Result<manifest::Manifest, String> {
    let manifest = manifest::Manifest::parse(&read_bounded(
        &directory.join(manifest::MANIFEST_NAME),
        manifest::MAX_MANIFEST_BYTES,
    )?)?;
    let expected = notice_files()
        .into_iter()
        .map(|(name, _)| name)
        .chain(std::iter::once(manifest::MANIFEST_NAME.into()))
        .chain(
            manifest
                .files
                .iter()
                .map(manifest::Asset::archive_relative_path),
        )
        .collect();
    exact_layout(directory, &expected)?;
    for file in &manifest.files {
        if regular_file(
            &directory.join(file.archive_relative_path()),
            file.compressed_bytes,
        )?
        .len()
            != file.compressed_bytes
        {
            return Err(format!(
                "Liquid archive has an incorrect size: {}",
                file.relative_path()
            ));
        }
    }
    validate_notices(directory, &manifest)?;
    Ok(manifest)
}
fn validate_notices(directory: &Path, manifest: &manifest::Manifest) -> Result<(), String> {
    for (name, limit) in notice_files() {
        regular_file(&directory.join(name), limit)?;
    }
    if sha256(&directory.join("LICENSE.txt"))? != LICENSE_SHA256 {
        return Err("Liquid licence must match the pinned upstream model licence".into());
    }
    let provenance: serde_json::Value = serde_json::from_slice(&read_bounded(
        &directory.join("PROVENANCE.json"),
        MAX_PROVENANCE_BYTES,
    )?)
    .map_err(|_| "Liquid provenance must be JSON")?;
    let invalid = || {
        "Liquid provenance must bind the model's passing parity and both pinned native runtimes to these exact assets".to_owned()
    };
    let asset = |target: Option<&str>, name: &str| {
        manifest
            .files
            .iter()
            .find(|file| file.target.as_deref() == target && file.name == name)
            .ok_or_else(invalid)
    };
    let model = asset(None, manifest::MODEL_NAME)?;
    let tokenizer = asset(None, manifest::TOKENIZER_NAME)?;
    if provenance["format"].as_u64() != Some(u64::from(manifest::FORMAT))
        || provenance["model_sha256"].as_str() != Some(&model.sha256)
        || provenance["tokenizer_sha256"].as_str() != Some(&tokenizer.sha256)
        || provenance["parity"]["model_sha256"].as_str() != Some(&model.sha256)
        || provenance["parity"]["tokenizer_sha256"].as_str() != Some(&tokenizer.sha256)
        || !complete_parity(&provenance["parity"])
    {
        return Err(invalid());
    }
    let runtimes = provenance["runtimes"].as_array().ok_or_else(invalid)?;
    if runtimes.len() != 2 {
        return Err(invalid());
    }
    let mut targets = BTreeSet::new();
    for runtime in runtimes {
        let target = runtime["target"].as_str().ok_or_else(invalid)?;
        if !targets.insert(target)
            || runtime["runtime_version"].as_str() != manifest::runtime_version(target)
            || runtime["archive_sha256"].as_str() != native_archive_sha256(target)
            || runtime["helper_sha256"].as_str()
                != Some(&asset(Some(target), manifest::HELPER_NAME)?.sha256)
        {
            return Err(invalid());
        }
        let minimum = manifest::minimum_macos(target).ok_or_else(invalid)?;
        if runtime["minimum_macos"] != serde_json::json!([minimum.0, minimum.1]) {
            return Err(invalid());
        }
        let notices = directory.join(format!("licenses/{target}.txt"));
        let text = read_bounded(&notices, MAX_LICENSE_BYTES)?;
        std::str::from_utf8(&text).map_err(|_| "Liquid runtime notices must be UTF-8")?;
        if runtime["notices_sha256"].as_str() != Some(&sha256(&notices)?) {
            return Err(invalid());
        }
        let declared = runtime["files"].as_array().ok_or_else(invalid)?;
        if declared.len() != 2 {
            return Err(invalid());
        }
        let mut names = BTreeSet::new();
        for file in declared {
            let name = file["name"].as_str().ok_or_else(invalid)?;
            let actual = asset(Some(target), name)?;
            if !names.insert(name)
                || file["bytes"].as_u64() != Some(actual.bytes)
                || file["sha256"].as_str() != Some(&actual.sha256)
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
fn complete_parity(parity: &serde_json::Value) -> bool {
    if !parity["matched"]
        .as_u64()
        .is_some_and(|count| count == PARITY_CASES)
        || !parity["compared_reference_steps"]
            .as_u64()
            .is_some_and(|count| count == PARITY_STEPS)
        || !parity["mismatches"].as_array().is_some_and(Vec::is_empty)
        || parity["lossless_weights"].as_bool() != Some(true)
        || !parity["max_error_probability_delta"]
            .as_f64()
            .is_some_and(|value| (0.0..=1e-4).contains(&value))
        || parity["fixture_sha256"].as_str() != Some(PARITY_FIXTURE_SHA256)
    {
        return false;
    }
    let Some(bounds) = parity["shape_boundaries"].as_array() else {
        return false;
    };
    let mut remaining = BTreeSet::from([1, 2, 8, 32, 127, 128]);
    bounds.len() == remaining.len()
        && bounds.iter().all(|bound| {
            bound["sequence"]
                .as_u64()
                .is_some_and(|sequence| remaining.remove(&sequence))
                && bound["tags_match"].as_bool() == Some(true)
                && bound["max_error_probability_delta"]
                    .as_f64()
                    .is_some_and(|value| (0.0..=1e-4).contains(&value))
        })
        && remaining.is_empty()
}
fn verify_tree(directory: &Path, verifier: NativeVerifier) -> Result<(), String> {
    let manifest = check_package_layout(directory)?;
    for file in &manifest.files {
        let path = directory.join(file.archive_relative_path());
        if sha256(&path)? != file.compressed_sha256 {
            return Err(format!(
                "Liquid compressed SHA256 mismatch: {}",
                file.relative_path()
            ));
        }
        if let Some(target) = &file.target {
            let mut output = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
            decode(&path, file, &mut output)?;
            output.flush().map_err(|error| error.to_string())?;
            verifier(output.path(), target)?;
        } else {
            decode(&path, file, &mut std::io::sink())?;
        }
    }
    Ok(())
}
fn decode(path: &Path, file: &manifest::Asset, output: &mut impl Write) -> Result<(), String> {
    codec::decode_bounded(
        fs::File::open(path).map_err(|error| error.to_string())?,
        file.bytes,
        &file.sha256,
        output,
        manifest::MAX_FILE_BYTES,
    )
    .map_err(|error| {
        format!(
            "Liquid payload verification failed for {}: {error}",
            file.relative_path()
        )
    })
}
fn verify_native(path: &Path, target: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::verify_architectures(path, target)?;
        let inspect = |argument| -> Result<String, String> {
            let output = Command::new("otool")
                .arg(argument)
                .arg(path)
                .output()
                .map_err(|error| format!("inspecting Liquid runtime: {error}"))?;
            if !output.status.success() {
                return Err(format!("Cannot inspect Liquid runtime {}", path.display()));
            }
            String::from_utf8(output.stdout)
                .map_err(|_| "Invalid native runtime inspection output".into())
        };
        validate_native_links(&inspect("-L")?, target)?;
        validate_native_load_commands(&inspect("-l")?, target)?;
        run_status(
            Command::new("codesign")
                .args(["--verify", "--strict"])
                .arg(path),
            "verifying the signed Liquid runtime",
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, target);
        Err("Preparing or verifying the Liquid Mac runtimes requires macOS".into())
    }
}
/// Native code may depend on system libraries or its packaged siblings only.
fn validate_native_links(output: &str, target: &str) -> Result<(), String> {
    let names = manifest::native_names(target).ok_or("Unsupported Liquid runtime target")?;
    let mut count = 0;
    for line in output.lines().skip(1) {
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        count += 1;
        if name.starts_with("/System/Library/") || name.starts_with("/usr/lib/") {
            continue;
        }
        let sibling = name
            .strip_prefix("@rpath/")
            .or_else(|| name.strip_prefix("@loader_path/"));
        if !sibling.is_some_and(|name| names.contains(&name)) {
            return Err(format!(
                "Liquid runtime has an unpackaged library dependency: {name}"
            ));
        }
    }
    if count == 0 {
        return Err("Liquid runtime has no inspectable library dependencies".into());
    }
    Ok(())
}
fn validate_native_load_commands(output: &str, target: &str) -> Result<(), String> {
    let maximum = manifest::minimum_macos(target).ok_or("Unsupported Liquid runtime target")?;
    let mut minimum_seen = false;
    for command in output.split("Load command ").skip(1) {
        let field = |key| {
            command.lines().find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some(key)).then(|| words.next()).flatten()
            })
        };
        match field("cmd") {
            Some("LC_RPATH") => {
                if !matches!(field("path"), Some("@loader_path" | "@loader_path/")) {
                    return Err("Liquid runtime has a nonportable library search path".into());
                }
            }
            Some("LC_BUILD_VERSION" | "LC_VERSION_MIN_MACOSX") => {
                let value = if field("cmd") == Some("LC_BUILD_VERSION") {
                    if !matches!(field("platform"), Some("1" | "MACOS")) {
                        return Err("Liquid runtime is not built for macOS".into());
                    }
                    field("minos")
                } else {
                    field("version")
                }
                .ok_or("Liquid runtime lacks a macOS deployment version")?;
                let version: Vec<u32> = value
                    .split('.')
                    .map(str::parse)
                    .collect::<Result<_, _>>()
                    .map_err(|_| "Invalid Liquid runtime deployment version")?;
                if !(2..=3).contains(&version.len())
                    || (version[0], version[1], *version.get(2).unwrap_or(&0))
                        > (maximum.0, maximum.1, 0)
                {
                    return Err(format!(
                        "Liquid runtime requires macOS {value}, newer than its declared support"
                    ));
                }
                minimum_seen = true;
            }
            _ => {}
        }
    }
    if !minimum_seen {
        return Err("Liquid runtime lacks a macOS deployment command".into());
    }
    Ok(())
}

fn regular_file(path: &Path, limit: u64) -> Result<fs::Metadata, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > limit {
        return Err(format!(
            "Liquid resource must be a nonempty regular file of at most {limit} bytes: {}",
            path.display()
        ));
    }
    Ok(metadata)
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    regular_file(path, limit)?;
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() as u64 > limit {
        return Err("Liquid resource changed size while reading".into());
    }
    Ok(bytes)
}
fn exact_layout(directory: &Path, expected: &BTreeSet<String>) -> Result<(), String> {
    if !fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return Err("Liquid source must be a regular directory".into());
    }
    fn walk(
        root: &Path,
        relative: &str,
        remaining: &mut BTreeSet<String>,
        expected: &BTreeSet<String>,
    ) -> Result<(), String> {
        for entry in fs::read_dir(root.join(relative)).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "Liquid resource name is not UTF-8")?;
            let name = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_file() {
                if !remaining.remove(&name) {
                    return Err(format!("Unexpected Liquid resource: {name}"));
                }
            } else if kind.is_dir()
                && expected
                    .iter()
                    .any(|file| file.starts_with(&(name.clone() + "/")))
            {
                walk(root, &name, remaining, expected)?;
            } else {
                return Err(format!("Unexpected or linked Liquid resource: {name}"));
            }
        }
        Ok(())
    }
    let mut remaining = expected.clone();
    walk(directory, "", &mut remaining, expected)?;
    if !remaining.is_empty() {
        return Err(format!("Missing Liquid resources: {remaining:?}"));
    }
    Ok(())
}
fn copy_bounded(source: &Path, destination: &Path, limit: u64) -> Result<(), String> {
    regular_file(source, limit)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut input = fs::File::open(source)
        .map_err(|error| error.to_string())?
        .take(limit + 1);
    let mut output = fs::File::create(destination).map_err(|error| error.to_string())?;
    let count = std::io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
    if count == 0 || count > limit {
        return Err("Liquid resource changed size while copying".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_inspection_rejects_unbundled_libraries_search_paths_and_newer_macos() {
        let target = "x86_64-apple-darwin";
        let links = "fixture:\n\t@rpath/libonnxruntime.dylib (compatibility version 0.0.0)\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)";
        validate_native_links(links, target).unwrap();
        for dependency in [
            "/opt/homebrew/lib/libonnxruntime.dylib",
            "@rpath/libpython.dylib",
            "@loader_path/../libonnxruntime.dylib",
            "@executable_path/libonnxruntime.dylib",
        ] {
            assert!(
                validate_native_links(
                    &format!("fixture:\n\t{dependency} (compatibility version 0.0.0)"),
                    target
                )
                .is_err()
            );
        }
        let commands = "Load command 1\n cmd LC_BUILD_VERSION\n platform 1\n minos 12.0\nLoad command 2\n cmd LC_RPATH\n path @loader_path (offset 12)\n";
        validate_native_load_commands(commands, target).unwrap();
        for invalid in [
            commands.replace("12.0", "12.1"),
            commands.replace("platform 1", "platform 6"),
            commands.replace("@loader_path", "/private/tmp/developer-runtime"),
            "Load command 1\n cmd LC_UUID".into(),
        ] {
            assert!(validate_native_load_commands(&invalid, target).is_err());
        }
        validate_native_load_commands(
            "Load command 1\n cmd LC_VERSION_MIN_MACOSX\n version 10.13.6\n",
            target,
        )
        .unwrap();
    }
    #[test]
    fn production_parity_requires_complete_reference_and_dynamic_boundaries() {
        let actual_fixture = include_bytes!("../../assets/liquid/correction-cases.json");
        assert_eq!(
            format!("{:x}", Sha256::digest(actual_fixture)),
            PARITY_FIXTURE_SHA256
        );
        let actual: serde_json::Value = serde_json::from_slice(actual_fixture).unwrap();
        let cases = actual["cases"].as_array().unwrap();
        assert_eq!(cases.len() as u64, PARITY_CASES);
        assert_eq!(
            cases
                .iter()
                .map(|case| case["reference_passes"].as_array().unwrap().len() as u64)
                .sum::<u64>(),
            PARITY_STEPS
        );
        let temporary = tempfile::tempdir().unwrap();
        raw_fixture(temporary.path());
        let proof: serde_json::Value =
            serde_json::from_slice(&fs::read(temporary.path().join("PROVENANCE.json")).unwrap())
                .unwrap();
        assert!(complete_parity(&proof["parity"]));
        for (field, value) in [
            ("matched", serde_json::json!(1)),
            ("compared_reference_steps", serde_json::json!(426)),
            ("lossless_weights", serde_json::json!(false)),
            ("mismatches", serde_json::json!(["different tags"])),
            ("fixture_sha256", serde_json::json!("not-a-digest")),
            ("max_error_probability_delta", serde_json::json!(0.01)),
        ] {
            let mut invalid = proof["parity"].clone();
            invalid[field] = value;
            assert!(!complete_parity(&invalid), "accepted {field}");
        }
        let mut invalid = proof["parity"].clone();
        invalid["shape_boundaries"].as_array_mut().unwrap().pop();
        assert!(!complete_parity(&invalid));
        let mut invalid = proof["parity"].clone();
        invalid["shape_boundaries"][5]["sequence"] = serde_json::json!(127);
        assert!(!complete_parity(&invalid));
        let mut invalid = proof["parity"].clone();
        invalid["shape_boundaries"][5]["tags_match"] = serde_json::json!(false);
        assert!(!complete_parity(&invalid));
    }
    fn fixture_verifier(path: &Path, target: &str) -> Result<(), String> {
        if !fs::read_to_string(path)
            .map_err(|error| error.to_string())?
            .starts_with(target)
        {
            return Err("fixture native architecture does not match".into());
        }
        Ok(())
    }
    fn raw_fixture(directory: &Path) {
        fs::create_dir_all(directory).unwrap();
        let mut files = descriptors();
        for file in &mut files {
            let content = format!(
                "{} {} fixture",
                file.target.as_deref().unwrap_or("shared"),
                file.name
            );
            let path = directory.join(file.relative_path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content.as_bytes()).unwrap();
            file.bytes = content.len() as u64;
            file.sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
        }
        fs::write(
            directory.join("LICENSE.txt"),
            include_bytes!("../../assets/liquid/LFM-Open-License.txt"),
        )
        .unwrap();
        let runtimes: Vec<_> = manifest::MACOS_TARGETS.into_iter().map(|target| {
            let text = format!("ONNX Runtime fixture licence for {target}");
            let path = directory.join(format!("licenses/{target}.txt"));
            fs::create_dir_all(path.parent().unwrap()).unwrap(); fs::write(&path, &text).unwrap();
            let native: Vec<_> = files.iter().filter(|file| file.target.as_deref() == Some(target)).collect();
            let minimum = manifest::minimum_macos(target).unwrap();
            serde_json::json!({"target": target, "runtime_version": manifest::runtime_version(target),
                "minimum_macos": [minimum.0, minimum.1], "archive_sha256": native_archive_sha256(target),
                "helper_sha256": native.iter().find(|file| file.name == manifest::HELPER_NAME).unwrap().sha256,
                "notices_sha256": sha256(&path).unwrap(),
                "files": native.iter().map(|file| serde_json::json!({"name":file.name,"bytes":file.bytes,"sha256":file.sha256})).collect::<Vec<_>>()})
        }).collect();
        let model = &files
            .iter()
            .find(|file| file.name == manifest::MODEL_NAME)
            .unwrap()
            .sha256;
        let tokenizer = &files
            .iter()
            .find(|file| file.name == manifest::TOKENIZER_NAME)
            .unwrap()
            .sha256;
        let provenance = serde_json::json!({"format":manifest::FORMAT,"model_sha256":model,
            "tokenizer_sha256":tokenizer,"parity":{"matched":268,"compared_reference_steps":427,"mismatches":[],"model_sha256":model,"tokenizer_sha256":tokenizer,
            "fixture_sha256":PARITY_FIXTURE_SHA256,"lossless_weights":true,"max_error_probability_delta":0.000001,
            "shape_boundaries":([1,2,8,32,127,128].map(|sequence| serde_json::json!({"sequence":sequence,"tags_match":true,"max_error_probability_delta":0.000001})))},
            "runtimes":runtimes});
        fs::write(
            directory.join("PROVENANCE.json"),
            serde_json::to_vec(&provenance).unwrap(),
        )
        .unwrap();
    }
    fn files_in(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(root: &Path, directory: &Path, result: &mut Vec<(PathBuf, Vec<u8>)>) {
            for entry in fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, result);
                } else {
                    result.push((
                        path.strip_prefix(root).unwrap().to_owned(),
                        fs::read(path).unwrap(),
                    ));
                }
            }
        }
        let mut result = Vec::new();
        walk(directory, directory, &mut result);
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }
    #[test]
    fn prepares_both_runtime_closures_without_mutating_raw_inputs_and_stages_single_model() {
        let temporary = tempfile::tempdir().unwrap();
        let raw = temporary.path().join("raw");
        raw_fixture(&raw);
        let original = files_in(&raw);
        let prepared = temporary.path().join("prepared");
        prepare_tree(&raw, &prepared, fixture_verifier).unwrap();
        assert_eq!(files_in(&raw), original);
        let manifest = check_package_layout(&prepared).unwrap();
        assert_eq!(manifest.files.len(), 6);
        assert_eq!(
            manifest
                .files
                .iter()
                .filter(|file| !file.is_native())
                .count(),
            2
        );
        assert_eq!(files_in(&prepared).len(), 11);
        for target in manifest::MACOS_TARGETS {
            assert!(
                prepared
                    .join(format!("runtime/{target}/liquid-onnx.zst"))
                    .is_file()
            );
            assert!(
                prepared
                    .join(format!("runtime/{target}/libonnxruntime.dylib.zst"))
                    .is_file()
            );
        }
        let staged = temporary.path().join("staged");
        stage_tree(&prepared, &staged, fixture_verifier).unwrap();
        assert_eq!(files_in(&staged), files_in(&prepared));
        verify_tree(&staged, fixture_verifier).unwrap();
        assert!(prepare_tree(&raw, &raw, fixture_verifier).is_err());
        assert!(prepare_tree(&raw, &raw.join("generated"), fixture_verifier).is_err());
        assert_eq!(files_in(&raw), original);
    }
    #[test]
    fn recovers_compressed_universal_package_and_keeps_existing_prepared_assets_authoritative() {
        let root = tempfile::tempdir().unwrap();
        let raw = root.path().join("raw");
        raw_fixture(&raw);
        let staged = root.path().join("staged");
        prepare_tree(&raw, &staged, fixture_verifier).unwrap();
        let original = files_in(&staged);
        let prepared = root.path().join("prepared");
        let missing = root.path().join("absent");
        let invalid_installed = root.path().join("old installed app");
        fs::create_dir(&invalid_installed).unwrap();
        let candidates = [missing, staged.clone(), invalid_installed.clone()];
        assert_eq!(
            ensure_prepared_tree(&prepared, &candidates, fixture_verifier).unwrap(),
            Some(staged.clone())
        );
        assert_eq!(files_in(&prepared), original);
        assert_eq!(files_in(&staged), original);
        verify_tree(&prepared, fixture_verifier).unwrap();
        // A later invalid candidate must never replace, or even be inspected
        // instead of, an already prepared package.
        assert_eq!(
            ensure_prepared_tree(&prepared, &[invalid_installed], |_, _| {
                panic!("existing prepared assets should not be copied or re-exported")
            })
            .unwrap(),
            None
        );
        assert_eq!(files_in(&prepared), original);
    }

    #[test]
    fn installed_package_can_restore_missing_developer_assets_without_raw_inputs() {
        let root = tempfile::tempdir().unwrap();
        let raw = root.path().join("raw");
        raw_fixture(&raw);
        let installed = root
            .path()
            .join("Applications with spaces/app/Contents/Resources/liquid");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        prepare_tree(&raw, &installed, fixture_verifier).unwrap();
        fs::remove_dir_all(raw).unwrap();
        let original = files_in(&installed);
        let prepared = root.path().join("prepared");
        assert_eq!(
            ensure_prepared_tree(
                &prepared,
                &[root.path().join("absent-staging"), installed.clone()],
                fixture_verifier,
            )
            .unwrap(),
            Some(installed.clone())
        );
        assert_eq!(files_in(&prepared), original);
        assert_eq!(files_in(&installed), original);
    }

    #[test]
    fn missing_recovery_inputs_report_actionable_error_without_creating_partial_assets() {
        let root = tempfile::tempdir().unwrap();
        let prepared = root.path().join("prepared");
        let error = ensure_prepared_tree(
            &prepared,
            &[root.path().join("missing-installed")],
            fixture_verifier,
        )
        .unwrap_err();
        assert!(error.contains("ensure-liquid --source"));
        assert!(error.contains("prepare-liquid --source"));
        assert!(!prepared.exists());
    }

    #[test]
    fn invalid_existing_source_and_selected_recovery_candidate_are_not_silently_replaced() {
        let root = tempfile::tempdir().unwrap();
        let raw = root.path().join("raw");
        raw_fixture(&raw);
        let good = root.path().join("installed");
        prepare_tree(&raw, &good, fixture_verifier).unwrap();
        let invalid = root.path().join("invalid");
        fs::create_dir(&invalid).unwrap();
        fs::write(invalid.join("incomplete"), b"preserve this input").unwrap();
        let original = files_in(&invalid);
        assert!(
            ensure_prepared_tree(&invalid, std::slice::from_ref(&good), fixture_verifier).is_err()
        );
        assert_eq!(files_in(&invalid), original);
        let prepared = root.path().join("prepared");
        let error = ensure_prepared_tree(&prepared, &[invalid.clone(), good], fixture_verifier)
            .unwrap_err();
        assert!(error.contains("Cannot recover Liquid"));
        assert!(!prepared.exists());
        assert_eq!(files_in(&invalid), original);
    }

    #[test]
    fn recovery_checks_compressed_expanded_and_native_files_before_publication() {
        for alteration in [
            "compressed",
            "expanded",
            "missing-intel",
            "old-format",
            "native",
        ] {
            let root = tempfile::tempdir().unwrap();
            let raw = root.path().join("raw");
            raw_fixture(&raw);
            let candidate = root.path().join("installed");
            prepare_tree(&raw, &candidate, fixture_verifier).unwrap();
            let manifest_path = candidate.join(manifest::MANIFEST_NAME);
            match alteration {
                "compressed" => {
                    let path = candidate.join("liquid.onnx.zst");
                    let mut bytes = fs::read(&path).unwrap();
                    bytes[3] ^= 0xff;
                    fs::write(path, bytes).unwrap();
                }
                "expanded" => {
                    let mut value = check_package_layout(&candidate).unwrap();
                    value
                        .files
                        .iter_mut()
                        .find(|file| file.name == manifest::TOKENIZER_NAME)
                        .unwrap()
                        .sha256 = "a".repeat(64);
                    fs::write(&manifest_path, serde_json::to_vec(&value).unwrap()).unwrap();
                    let proof_path = candidate.join("PROVENANCE.json");
                    let mut proof: serde_json::Value =
                        serde_json::from_slice(&fs::read(&proof_path).unwrap()).unwrap();
                    proof["tokenizer_sha256"] = serde_json::json!("a".repeat(64));
                    proof["parity"]["tokenizer_sha256"] = serde_json::json!("a".repeat(64));
                    fs::write(proof_path, serde_json::to_vec(&proof).unwrap()).unwrap();
                    check_package_layout(&candidate).unwrap();
                }
                "missing-intel" => {
                    fs::remove_file(candidate.join("runtime/x86_64-apple-darwin/liquid-onnx.zst"))
                        .unwrap()
                }
                "old-format" => {
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
                    value["format"] = serde_json::json!(2);
                    fs::write(&manifest_path, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "native" => {}
                _ => unreachable!(),
            }
            let original = files_in(&candidate);
            let prepared = root.path().join("prepared");
            let verifier = if alteration == "native" {
                (|_: &Path, _: &str| Err("invalid native runtime".into())) as NativeVerifier
            } else {
                fixture_verifier
            };
            let error = ensure_prepared_tree(&prepared, std::slice::from_ref(&candidate), verifier)
                .unwrap_err();
            assert!(
                error.contains("Cannot recover Liquid"),
                "{alteration}: {error}"
            );
            assert!(!prepared.exists(), "{alteration}");
            assert_eq!(files_in(&candidate), original, "{alteration}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn recovery_rejects_linked_inputs_and_overlapping_trees() {
        let root = tempfile::tempdir().unwrap();
        let raw = root.path().join("raw");
        raw_fixture(&raw);
        let candidate = root.path().join("installed");
        prepare_tree(&raw, &candidate, fixture_verifier).unwrap();
        let original = files_in(&candidate);
        assert!(stage_tree(&candidate, &candidate, fixture_verifier).is_err());
        assert!(stage_tree(&candidate, &candidate.join("child"), fixture_verifier).is_err());
        assert_eq!(files_in(&candidate), original);
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(&candidate, &linked).unwrap();
        let prepared = root.path().join("prepared");
        assert!(ensure_prepared_tree(&prepared, &[linked], fixture_verifier).is_err());
        assert!(!prepared.exists());
        let archive = candidate.join("liquid.onnx.zst");
        let outside = root.path().join("outside.zst");
        fs::rename(&archive, &outside).unwrap();
        std::os::unix::fs::symlink(outside, archive).unwrap();
        assert!(ensure_prepared_tree(&prepared, &[candidate], fixture_verifier).is_err());
        assert!(!prepared.exists());
    }

    #[test]
    fn corruption_or_unproven_inputs_cannot_replace_the_previous_complete_package() {
        let temporary = tempfile::tempdir().unwrap();
        let raw = temporary.path().join("raw");
        raw_fixture(&raw);
        let prepared = temporary.path().join("prepared");
        prepare_tree(&raw, &prepared, fixture_verifier).unwrap();
        let staged = temporary.path().join("staged");
        stage_tree(&prepared, &staged, fixture_verifier).unwrap();
        let previous = files_in(&staged);
        let archive = prepared.join("runtime/aarch64-apple-darwin/libonnxruntime.dylib.zst");
        let mut bytes = fs::read(&archive).unwrap();
        bytes[3] ^= 0xff;
        fs::write(&archive, bytes).unwrap();
        assert!(stage_tree(&prepared, &staged, fixture_verifier).is_err());
        assert_eq!(files_in(&staged), previous);
        fs::write(
            raw.join(manifest::MODEL_NAME),
            b"changed model after parity",
        )
        .unwrap();
        assert!(prepare_tree(&raw, &staged, fixture_verifier).is_err());
        assert_eq!(files_in(&staged), previous);
        verify_tree(&staged, fixture_verifier).unwrap();
    }
    #[test]
    fn raw_inputs_require_complete_licences_pinned_versions_and_passing_parity() {
        for alteration in [
            "missing-notice",
            "extra-file",
            "failed-parity",
            "wrong-runtime",
            "changed-helper",
        ] {
            let root = tempfile::tempdir().unwrap();
            raw_fixture(root.path());
            let provenance_path = root.path().join("PROVENANCE.json");
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&provenance_path).unwrap()).unwrap();
            match alteration {
                "missing-notice" => {
                    fs::remove_file(root.path().join("licenses/x86_64-apple-darwin.txt")).unwrap()
                }
                "extra-file" => {
                    fs::write(root.path().join("old-python-worker.py"), b"obsolete").unwrap()
                }
                "failed-parity" => {
                    value["parity"]["mismatches"] = serde_json::json!(["changed correction"])
                }
                "wrong-runtime" => {
                    value["runtimes"][0]["runtime_version"] = serde_json::json!("0.0.0")
                }
                "changed-helper" => {
                    value["runtimes"][0]["helper_sha256"] = serde_json::json!("b".repeat(64))
                }
                _ => unreachable!(),
            }
            fs::write(provenance_path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(
                raw_manifest(root.path(), fixture_verifier).is_err(),
                "{alteration}"
            );
        }
        #[cfg(unix)]
        {
            let root = tempfile::tempdir().unwrap();
            let raw = root.path().join("raw");
            raw_fixture(&raw);
            let model = raw.join(manifest::MODEL_NAME);
            let outside = root.path().join("outside");
            fs::rename(&model, &outside).unwrap();
            std::os::unix::fs::symlink(&outside, model).unwrap();
            assert!(
                raw_manifest(&raw, fixture_verifier)
                    .unwrap_err()
                    .contains("linked")
            );
        }
    }
}
