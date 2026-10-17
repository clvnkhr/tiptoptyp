//! Small, deterministic resource archives with verified, architecture-specific payloads.
use super::*;
use flate2::{Compression, GzBuilder, read::MultiGzDecoder};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
};

#[path = "../../build_support/bundled_assets.rs"]
mod bundled_assets;

const TOOLS: [&str; 6] = [
    "typst", "tinymist", "tectonic", "texlab", "badness", "tex-fmt",
];
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_PAYLOAD_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    bytes: u64,
    sha256: String,
}

pub(super) fn stage(target: &str) -> Result<(), String> {
    let root = repository_root();
    let toolchain = root.join("toolchain");
    let payloads = temporary_tree(&toolchain)?;
    let notices = temporary_tree(&toolchain)?;
    let assets = temporary_tree(&toolchain)?;
    let artifacts = parse_manifest(MANIFEST)?;
    for target in macos::targets(target) {
        let directory = payloads.path().join(target);
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let mut entries = Vec::new();
        for tool in TOOLS {
            let artifact = artifacts
                .iter()
                .find(|artifact| artifact.app_target == target && artifact.tool == tool)
                .ok_or_else(|| format!("missing pinned {tool} for {target}"))?;
            let name = artifact.extracted_name();
            entries.push(stage_native(
                &root.join("toolchain/bin").join(artifact.staged_name()),
                &directory,
                &name,
                target,
            )?);
        }
        let pdfium = pdfium::bundle_directory(&root, target);
        let library = library_name(target);
        entries.push(stage_native(
            &pdfium.join(library),
            &directory,
            library,
            target,
        )?);
        write_manifest(&directory, &entries)?;
        copy_notices(&pdfium, notices.path(), target)?;
        print_sizes(target, &directory, &entries)?;
    }
    stage_asset_tree(&root, assets.path())?;
    // Nothing is published until all inputs have been read, stripped, signed and
    // compressed. Replacing each complete tree removes stale architecture files.
    publish_tree(payloads, &toolchain.join("payloads"))?;
    publish_tree(notices, &toolchain.join("pdfium-notices"))?;
    publish_tree(assets, &toolchain.join("shared-assets"))
}

fn stage_asset_tree(root: &Path, directory: &Path) -> Result<(), String> {
    let directory = directory.join("shared");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let entries = bundled_assets::SHARED_ASSETS
        .iter()
        .map(|asset| pack_file(&root.join(asset.source), &directory, asset.name))
        .collect::<Result<Vec<_>, _>>()?;
    write_manifest(&directory, &entries)?;
    print_sizes("shared fonts and handwriting", &directory, &entries)
}

fn stage_native(
    source: &Path,
    directory: &Path,
    name: &str,
    target: &str,
) -> Result<Entry, String> {
    let scratch = temporary_tree(directory)?;
    let stripped = scratch.path().join(name);
    fs::copy(source, &stripped)
        .map_err(|error| format!("copying {}: {error}", source.display()))?;
    if target.ends_with("apple-darwin") {
        macos::verify_architectures(&stripped, target)?;
        run_status(
            Command::new("strip").args(["-S", "-x"]).arg(&stripped),
            &format!("stripping {name} for {target}"),
        )?;
        run_status(
            Command::new("codesign")
                .args(["--force", "--sign", "-", "--timestamp=none", "--identifier"])
                .arg(format!("dev.tiptoptyp.payload.{name}"))
                .arg(&stripped),
            &format!("signing compressed payload {name} for {target}"),
        )?;
        macos::verify_architectures(&stripped, target)?;
    }
    pack_file(&stripped, directory, name)
}

fn library_name(target: &str) -> &'static str {
    if target.ends_with("apple-darwin") {
        "libpdfium.dylib"
    } else if target.contains("windows") {
        "pdfium.dll"
    } else {
        "libpdfium.so"
    }
}

fn pack_file(source: &Path, directory: &Path, name: &str) -> Result<Entry, String> {
    if !valid_name(name) {
        return Err(format!("invalid payload name: {name:?}"));
    }
    let mut source =
        fs::File::open(source).map_err(|error| format!("opening {}: {error}", source.display()))?;
    let bytes = source.metadata().map_err(|error| error.to_string())?.len();
    if !(1..=MAX_PAYLOAD_BYTES).contains(&bytes) {
        return Err(format!("{name} has unsupported payload size {bytes}"));
    }
    let archive = fs::File::create(directory.join(format!("{name}.gz")))
        .map_err(|error| error.to_string())?;
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(archive, Compression::best());
    let mut hasher = Sha256::new();
    let mut actual = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = source
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        actual += count as u64;
        if actual > bytes {
            return Err(format!("{name} changed while it was being packaged"));
        }
        hasher.update(&buffer[..count]);
        encoder
            .write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
    }
    encoder.finish().map_err(|error| error.to_string())?;
    if actual != bytes {
        return Err(format!("{name} changed while it was being packaged"));
    }
    Ok(Entry {
        name: name.to_owned(),
        bytes,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn write_manifest(directory: &Path, entries: &[Entry]) -> Result<(), String> {
    let mut manifest = String::new();
    for entry in entries {
        use std::fmt::Write;
        writeln!(
            manifest,
            "{}\t{}\t{}",
            entry.name, entry.bytes, entry.sha256
        )
        .expect("writing to a String cannot fail");
    }
    fs::write(directory.join("manifest.tsv"), manifest).map_err(|error| error.to_string())
}

fn print_sizes(label: &str, directory: &Path, entries: &[Entry]) -> Result<(), String> {
    let raw = entries.iter().map(|entry| entry.bytes).sum::<u64>();
    let packed = entries.iter().try_fold(0_u64, |total, entry| {
        fs::metadata(directory.join(format!("{}.gz", entry.name)))
            .map(|metadata| total + metadata.len())
            .map_err(|error| error.to_string())
    })?;
    println!("compressed {label}: {raw} bytes -> {packed} bytes");
    Ok(())
}

fn copy_notices(source: &Path, destination: &Path, target: &str) -> Result<(), String> {
    copy_same_file(&source.join("LICENSE"), &destination.join("LICENSE"))?;
    copy_notice_tree(&source.join("licenses"), &destination.join("licenses"))?;
    copy_same_file(
        &source.join("PROVENANCE.txt"),
        &destination.join(format!("PROVENANCE-{target}.txt")),
    )
}

fn copy_notice_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let output = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_notice_tree(&entry.path(), &output)?;
        } else if kind.is_file() {
            copy_same_file(&entry.path(), &output)?;
        } else {
            return Err(format!(
                "PDFium notices contain a nonregular file: {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn copy_same_file(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        if fs::read(source).map_err(|error| error.to_string())?
            != fs::read(destination).map_err(|error| error.to_string())?
        {
            return Err(format!(
                "PDFium notice differs between architectures: {}",
                destination.display()
            ));
        }
        return Ok(());
    }
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|error| format!("copying {}: {error}", source.display()))
}

fn temporary_tree(parent: &Path) -> Result<tempfile::TempDir, String> {
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    tempfile::Builder::new()
        .prefix(".tiptoptyp-payload-")
        .tempdir_in(parent)
        .map_err(|error| error.to_string())
}

fn publish_tree(staged: tempfile::TempDir, destination: &Path) -> Result<(), String> {
    let backup = staged.path().with_extension("previous");
    let existed = destination
        .try_exists()
        .map_err(|error| error.to_string())?;
    if existed {
        let metadata = fs::symlink_metadata(destination).map_err(|error| error.to_string())?;
        if !metadata.file_type().is_dir() {
            return Err(format!(
                "generated payload destination is not a regular directory: {}",
                destination.display()
            ));
        }
        fs::rename(destination, &backup).map_err(|error| error.to_string())?;
    }
    if let Err(error) = fs::rename(staged.path(), destination) {
        if existed {
            fs::rename(&backup, destination).map_err(|rollback| {
                format!("publishing payloads failed: {error}; rollback failed: {rollback}")
            })?;
        }
        return Err(format!("publishing payloads failed: {error}"));
    }
    if existed {
        fs::remove_dir_all(backup).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(super) fn verify(resources: &Path, target: &str) -> Result<(), String> {
    let artifacts = parse_manifest(MANIFEST)?;
    let host = host_target()?;
    let intel_emulation = host == "aarch64-apple-darwin"
        && Command::new("arch")
            .args(["-x86_64", "/usr/bin/true"])
            .status()
            .is_ok_and(|status| status.success());
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    for target in macos::targets(target) {
        let directory = resources.join("toolchain").join(target);
        let mut expected = TOOLS
            .iter()
            .map(|tool| {
                if target.contains("windows") {
                    format!("{tool}.exe")
                } else {
                    (*tool).to_owned()
                }
            })
            .collect::<Vec<_>>();
        expected.push(library_name(target).to_owned());
        for entry in read_manifest(&directory, &expected)? {
            let extracted = decode_temporary(&directory, &entry, temporary.path())?;
            if target.ends_with("apple-darwin") {
                macos::verify_architectures(&extracted, target)?;
                run_status(
                    Command::new("codesign")
                        .args(["--verify", "--strict"])
                        .arg(extracted.as_os_str()),
                    &format!("checking packaged {} signature", entry.name),
                )?;
            }
            if entry.name == library_name(target) {
                continue;
            }
            let tool = entry.name.strip_suffix(".exe").unwrap_or(&entry.name);
            let artifact = artifacts
                .iter()
                .find(|artifact| artifact.app_target == target && artifact.tool == tool)
                .ok_or_else(|| format!("no pinned version for {tool} on {target}"))?;
            let mut command = if target == host {
                Command::new(extracted.as_os_str())
            } else if target == "x86_64-apple-darwin" && intel_emulation {
                let mut command = Command::new("arch");
                command.arg("-x86_64").arg(extracted.as_os_str());
                command
            } else {
                println!(
                    "verified {} for {target} without execution (different architecture)",
                    entry.name
                );
                continue;
            };
            let output = run_output(
                command.arg("--version"),
                &format!("checking packaged {tool} for {target}"),
            )?;
            let reported = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if !reported.contains(&artifact.version) {
                return Err(format!(
                    "packaged {tool} for {target} reported {reported:?}, expected {}",
                    artifact.version
                ));
            }
        }
    }
    verify_assets(resources, temporary.path())
}

fn verify_assets(resources: &Path, temporary: &Path) -> Result<(), String> {
    let directory = resources.join("assets/shared");
    let expected = bundled_assets::SHARED_ASSETS
        .iter()
        .map(|asset| asset.name.to_owned())
        .collect::<Vec<_>>();
    for entry in read_manifest(&directory, &expected)? {
        let extracted = decode_temporary(&directory, &entry, temporary)?;
        let asset = bundled_assets::SHARED_ASSETS
            .iter()
            .find(|asset| asset.name == entry.name)
            .expect("validated asset name");
        if sha256(&extracted)? != sha256(&repository_root().join(asset.source))? {
            return Err(format!(
                "packaged asset differs from its source: {}",
                entry.name
            ));
        }
    }
    Ok(())
}

fn read_manifest(directory: &Path, expected: &[String]) -> Result<Vec<Entry>, String> {
    let mut text = String::new();
    let manifest = directory.join("manifest.tsv");
    require_regular_file(&manifest)?;
    fs::File::open(&manifest)
        .map_err(|error| error.to_string())?
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!(
            "payload manifest too large: {}",
            manifest.display()
        ));
    }
    let entries = parse_manifest_entries(&text, expected)?;
    let mut files = expected
        .iter()
        .map(|name| format!("{name}.gz"))
        .collect::<BTreeSet<_>>();
    files.insert("manifest.tsv".to_owned());
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "payload directory has a non-UTF-8 entry".to_owned())?;
        if !files.remove(&name) {
            return Err(format!(
                "unexpected packaged payload: {}",
                entry.path().display()
            ));
        }
        require_regular_file(&entry.path())?;
    }
    if !files.is_empty() {
        return Err(format!(
            "missing packaged payloads in {}: {files:?}",
            directory.display()
        ));
    }
    Ok(entries)
}

fn require_regular_file(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.file_type().is_file() && metadata.len() > 0 {
        Ok(())
    } else {
        Err(format!(
            "payload is not a nonempty regular file: {}",
            path.display()
        ))
    }
}

fn parse_manifest_entries(text: &str, expected: &[String]) -> Result<Vec<Entry>, String> {
    let mut remaining = expected.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 3 || !valid_name(fields[0]) || !remaining.remove(fields[0]) {
            return Err(format!(
                "invalid, duplicate or unexpected payload at manifest line {}",
                index + 1
            ));
        }
        let bytes = fields[1]
            .parse::<u64>()
            .ok()
            .filter(|bytes| (1..=MAX_PAYLOAD_BYTES).contains(bytes));
        let hash = fields[2];
        if bytes.is_none()
            || hash.len() != 64
            || !hash
                .bytes()
                .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
        {
            return Err(format!(
                "invalid payload size or SHA256 at manifest line {}",
                index + 1
            ));
        }
        entries.push(Entry {
            name: fields[0].to_owned(),
            bytes: bytes.unwrap(),
            sha256: hash.to_owned(),
        });
    }
    if !remaining.is_empty() {
        return Err(format!(
            "payload manifest is missing entries: {remaining:?}"
        ));
    }
    Ok(entries)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b'.'))
}

fn decode_temporary(
    directory: &Path,
    entry: &Entry,
    temporary: &Path,
) -> Result<tempfile::TempPath, String> {
    let mut file = tempfile::NamedTempFile::new_in(temporary).map_err(|error| error.to_string())?;
    decode_payload(
        &directory.join(format!("{}.gz", entry.name)),
        entry,
        file.as_file_mut(),
    )?;
    file.flush().map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    Ok(file.into_temp_path())
}

fn decode_payload(archive: &Path, entry: &Entry, output: &mut impl Write) -> Result<(), String> {
    let mut decoder =
        MultiGzDecoder::new(fs::File::open(archive).map_err(|error| error.to_string())?)
            .take(entry.bytes + 1);
    let mut hasher = Sha256::new();
    let mut actual = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = decoder
            .read(&mut buffer)
            .map_err(|error| format!("decoding {}: {error}", entry.name))?;
        if count == 0 {
            break;
        }
        actual += count as u64;
        if actual > entry.bytes {
            return Err(format!("{} exceeds its declared payload size", entry.name));
        }
        hasher.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
    }
    if actual != entry.bytes || format!("{:x}", hasher.finalize()) != entry.sha256 {
        return Err(format!(
            "{} failed payload size or SHA256 verification",
            entry.name
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_is_deterministic_and_round_trips_exact_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let first = temporary.path().join("first");
        let second = temporary.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let contents = (0..=255).cycle().take(80_000).collect::<Vec<u8>>();
        fs::write(&source, &contents).unwrap();
        let entry = pack_file(&source, &first, "test.bin").unwrap();
        fs::write(&source, &contents).unwrap();
        pack_file(&source, &second, "test.bin").unwrap();
        assert_eq!(
            fs::read(first.join("test.bin.gz")).unwrap(),
            fs::read(second.join("test.bin.gz")).unwrap()
        );
        let mut decoded = Vec::new();
        decode_payload(&first.join("test.bin.gz"), &entry, &mut decoded).unwrap();
        assert_eq!(decoded, contents);
        assert_eq!(fs::read(source).unwrap(), contents);
    }

    #[test]
    fn corrupt_hash_size_stream_and_overlong_output_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::write(&source, vec![b'a'; 200_000]).unwrap();
        let entry = pack_file(&source, temporary.path(), "test.bin").unwrap();
        let archive = temporary.path().join("test.bin.gz");
        let mut wrong = entry.clone();
        wrong.sha256 = "0".repeat(64);
        assert!(decode_payload(&archive, &wrong, &mut Vec::new()).is_err());
        wrong = entry.clone();
        wrong.bytes -= 1;
        assert!(decode_payload(&archive, &wrong, &mut Vec::new()).is_err());
        wrong = entry.clone();
        wrong.bytes += 1;
        assert!(decode_payload(&archive, &wrong, &mut Vec::new()).is_err());
        wrong.bytes = 1;
        let mut output = Vec::new();
        assert!(decode_payload(&archive, &wrong, &mut output).is_err());
        assert!(output.len() <= 1, "decoder wrote beyond its declared bound");
        fs::write(&archive, b"not gzip").unwrap();
        assert!(decode_payload(&archive, &entry, &mut Vec::new()).is_err());
    }

    #[test]
    fn manifest_requires_exact_allowlist_unique_names_and_bounded_sizes() {
        let expected = vec!["one".to_owned(), "two".to_owned()];
        let valid = format!("one\t2\t{}\ntwo\t3\t{}\n", "a".repeat(64), "b".repeat(64));
        assert_eq!(parse_manifest_entries(&valid, &expected).unwrap().len(), 2);
        for invalid in [
            valid.replace("two", "one"),
            valid.replace("two", "../two"),
            valid.replace("two", "extra"),
            valid.replace("\t2\t", "\t0\t"),
            valid.replace("\t2\t", "\t1073741825\t"),
            valid.replace(&"a".repeat(64), &"A".repeat(64)),
            valid.lines().next().unwrap().to_owned(),
        ] {
            assert!(
                parse_manifest_entries(&invalid, &expected).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn directory_verification_rejects_missing_extra_and_symlink_files() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let directory = temporary.path().join("packed");
        fs::create_dir(&directory).unwrap();
        fs::write(&source, b"payload").unwrap();
        let entry = pack_file(&source, &directory, "one").unwrap();
        write_manifest(&directory, &[entry]).unwrap();
        let expected = ["one".to_owned()];
        assert!(read_manifest(&directory, &expected).is_ok());
        fs::write(directory.join("extra.gz"), "unexpected").unwrap();
        assert!(read_manifest(&directory, &expected).is_err());
        fs::remove_file(directory.join("extra.gz")).unwrap();
        fs::remove_file(directory.join("one.gz")).unwrap();
        assert!(read_manifest(&directory, &expected).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&source, directory.join("one.gz")).unwrap();
            assert!(read_manifest(&directory, &expected).is_err());
        }
    }

    #[test]
    fn complete_tree_replacement_removes_stale_architecture_and_leaves_no_backup() {
        let temporary = tempfile::tempdir().unwrap();
        let destination = temporary.path().join("payloads");
        fs::create_dir_all(destination.join("stale-architecture")).unwrap();
        let staged = temporary_tree(temporary.path()).unwrap();
        fs::create_dir(staged.path().join("current-architecture")).unwrap();
        publish_tree(staged, &destination).unwrap();
        assert!(destination.join("current-architecture").is_dir());
        assert!(!destination.join("stale-architecture").exists());
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
    }
}
