//! Self-contained LTeX+ package: one shared server and one Java runtime per CPU.
use super::*;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

#[path = "../../build_support/ltex_manifest.rs"]
mod manifest;

const DESTINATION: &str = "toolchain/ltex";
const MAX_NOTICE_BYTES: u64 = 16 * 1024 * 1024;
const COMPRESSION_LEVEL: i32 = 9;
const LICENSE_SHA256: &str = "94536a6248d4b2c02844716045ea6e4c3fb46ab799bc8fa1d200a21a857e48f7";
const ACKNOWLEDGMENTS_SHA256: &str =
    "59398f0e2158b1fd3d817633b6d3c89d798c2046167607ba2cdfbf429ffc5b47";
type Fingerprint = BTreeMap<String, (u64, u32, String)>;

pub(super) fn stage(target: &str) -> Result<(), String> {
    let targets = macos::targets(target);
    if targets
        .iter()
        .any(|target| manifest::upstream(target).is_none())
    {
        return Err(format!("Bundled LTeX+ does not support {target}"));
    }
    let root = repository_root();
    let destination = root.join(DESTINATION);
    if verify_tree(&destination, &targets, false).is_ok() {
        return Ok(());
    }
    let cache = root.join("toolchain/cache");
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let staged = payloads::temporary_tree(&root.join("toolchain"))?;
    let mut files = Vec::new();
    let mut common = None;
    for target in &targets {
        let upstream = manifest::upstream(target).expect("checked target");
        let archive = cache.join(upstream.archive);
        if verify_upstream(&archive, upstream).is_err() {
            download(&upstream.url(), &archive)?;
        }
        verify_upstream(&archive, upstream)?;
        let scratch = payloads::temporary_tree(staged.path())?;
        let (shared, java) = split(&archive, scratch.path())?;
        if let Some(expected) = &common {
            if expected != &shared {
                return Err("The LTeX+ Intel and Apple Silicon server files differ; shared files were not deduplicated".into());
            }
        } else {
            common = Some(shared);
            files.push(compress(
                &scratch.path().join("shared.tar"),
                staged.path(),
                manifest::SHARED_NAME,
                None,
            )?);
            for name in ["LICENSE.md", "ACKNOWLEDGMENTS.md"] {
                fs::copy(scratch.path().join(name), staged.path().join(name))
                    .map_err(|e| e.to_string())?;
            }
        }
        if !java
            .keys()
            .any(|name| name == &format!("{}/bin/java", manifest::JDK_DIRECTORY))
            || !java
                .keys()
                .any(|name| name == &format!("{}/lib/server/libjvm.dylib", manifest::JDK_DIRECTORY))
        {
            return Err("The LTeX+ distribution is missing its bundled Java runtime".into());
        }
        files.push(compress(
            &scratch.path().join("native.tar"),
            staged.path(),
            &format!("{target}.tar"),
            Some(target),
        )?);
    }
    let package = manifest::Manifest {
        format: manifest::FORMAT,
        version: manifest::VERSION.into(),
        jdk_directory: manifest::JDK_DIRECTORY.into(),
        files,
    };
    package.validate()?;
    fs::write(
        staged.path().join(manifest::MANIFEST_NAME),
        serde_json::to_vec_pretty(&package).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let provenance = serde_json::json!({
        "name": "LTeX+", "version": manifest::VERSION,
        "source": manifest::SOURCE_URL,
        "release": format!("https://github.com/ltex-plus/ltex-ls-plus/releases/tag/{}", manifest::VERSION),
        "license": "MPL-2.0; upstream ACKNOWLEDGMENTS.md and bundled Java legal/ contain dependency notices",
        "packaging": "Unmodified server and Java files from pinned official releases. Shared server bytes are verified identical across architectures; archive paths and metadata are normalized.",
        "upstream": targets.iter().map(|target| { let source = manifest::upstream(target).unwrap(); serde_json::json!({"target":target,"url":source.url(),"bytes":source.bytes,"sha256":source.sha256}) }).collect::<Vec<_>>()
    });
    fs::write(
        staged.path().join("PROVENANCE.json"),
        serde_json::to_vec_pretty(&provenance).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    verify_tree(staged.path(), &targets, true)?;
    let bytes: u64 = package.files.iter().map(|file| file.compressed_bytes).sum();
    println!(
        "LTeX+ {}: {} MiB compressed, shared server stored once",
        manifest::VERSION,
        bytes / 1024 / 1024
    );
    payloads::publish_tree(staged, &destination)
}

pub(super) fn verify(resources: &Path, target: &str) -> Result<(), String> {
    verify_tree(&resources.join("ltex"), &macos::targets(target), true)
}
fn verify_upstream(path: &Path, source: manifest::Upstream) -> Result<(), String> {
    if !fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_file() && metadata.len() == source.bytes)
    {
        return Err(format!(
            "LTeX+ archive is missing or has an invalid size: {}",
            path.display()
        ));
    }
    verify_hash(path, source.sha256)
}

fn member_path(path: &Path) -> Result<PathBuf, String> {
    let mut components = path.components().filter(|part| *part != Component::CurDir);
    let prefix = format!("ltex-ls-plus-{}", manifest::VERSION);
    if components.next() != Some(Component::Normal(OsStr::new(&prefix))) {
        return Err("LTeX+ archive has an unexpected root directory".into());
    }
    let mut relative = PathBuf::new();
    for component in components {
        if !matches!(component, Component::Normal(_)) {
            return Err("LTeX+ archive contains an unsafe path".into());
        }
        relative.push(component.as_os_str());
    }
    Ok(relative)
}
fn native_path(path: &Path) -> bool {
    path.components().next() == Some(Component::Normal(OsStr::new(manifest::JDK_DIRECTORY)))
}
fn validate_link(path: &Path, target: &Path) -> Result<(), String> {
    if !native_path(path) || target.is_absolute() {
        return Err("LTeX+ archive contains an unsafe link".into());
    }
    let mut resolved = path.parent().unwrap().to_path_buf();
    for component in target.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => resolved.push(value),
            Component::ParentDir if resolved.components().count() > 1 => {
                resolved.pop();
            }
            _ => return Err("LTeX+ archive link escapes its Java runtime".into()),
        }
    }
    Ok(())
}
fn split(archive: &Path, destination: &Path) -> Result<(Fingerprint, Fingerprint), String> {
    let file = fs::File::open(archive).map_err(|e| e.to_string())?;
    split_reader(GzDecoder::new(file), destination)
}
fn split_reader(
    reader: impl Read,
    destination: &Path,
) -> Result<(Fingerprint, Fingerprint), String> {
    let mut source = tar::Archive::new(reader);
    let mut shared = tar::Builder::new(
        fs::File::create(destination.join("shared.tar")).map_err(|e| e.to_string())?,
    );
    let mut native = tar::Builder::new(
        fs::File::create(destination.join("native.tar")).map_err(|e| e.to_string())?,
    );
    let mut shared_files = Fingerprint::new();
    let mut native_files = Fingerprint::new();
    let mut total = 0u64;
    for entry in source.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let original = entry.path().map_err(|e| e.to_string())?.into_owned();
        if original.components().all(|part| part == Component::CurDir) {
            continue;
        }
        let relative = member_path(&original)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() && !kind.is_symlink() {
            return Err("LTeX+ archive contains an unsupported entry".into());
        }
        let bytes = entry.size();
        total = total
            .checked_add(bytes)
            .ok_or("LTeX+ archive is too large")?;
        if total > manifest::MAX_ARCHIVE_BYTES {
            return Err("LTeX+ archive is too large".into());
        }
        let is_native = native_path(&relative);
        let fingerprint = if is_native {
            &mut native_files
        } else {
            &mut shared_files
        };
        let builder = if is_native { &mut native } else { &mut shared };
        let name = relative
            .to_str()
            .ok_or("LTeX+ archive filename is not UTF-8")?
            .to_owned();
        if fingerprint.contains_key(&name) {
            return Err("LTeX+ archive contains duplicate paths".into());
        }
        let mode = if kind.is_dir()
            || kind.is_symlink()
            || entry.header().mode().map_err(|e| e.to_string())? & 0o111 != 0
        {
            0o755
        } else {
            0o644
        };
        let mut header = tar::Header::new_gnu();
        header.set_path(&relative).map_err(|e| e.to_string())?;
        header.set_entry_type(kind);
        header.set_mode(mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(bytes);
        let digest = if kind.is_symlink() {
            let link = entry
                .link_name()
                .map_err(|e| e.to_string())?
                .ok_or("LTeX+ archive link has no target")?
                .into_owned();
            validate_link(&relative, &link)?;
            header.set_link_name(&link).map_err(|e| e.to_string())?;
            header.set_size(0);
            header.set_cksum();
            builder
                .append(&header, std::io::empty())
                .map_err(|e| e.to_string())?;
            format!("link:{}", link.display())
        } else {
            header.set_cksum();
            let notice = !is_native && matches!(name.as_str(), "LICENSE.md" | "ACKNOWLEDGMENTS.md");
            if notice {
                if bytes > MAX_NOTICE_BYTES {
                    return Err("LTeX+ notice is too large".into());
                }
                let mut contents = Vec::new();
                entry
                    .read_to_end(&mut contents)
                    .map_err(|e| e.to_string())?;
                fs::write(destination.join(&relative), &contents).map_err(|e| e.to_string())?;
                builder
                    .append(&header, &contents[..])
                    .map_err(|e| e.to_string())?;
                format!("{:x}", Sha256::digest(&contents))
            } else {
                let mut reader = HashReader {
                    inner: &mut entry,
                    hash: Sha256::new(),
                };
                builder
                    .append(&header, &mut reader)
                    .map_err(|e| e.to_string())?;
                format!("{:x}", reader.hash.finalize())
            }
        };
        fingerprint.insert(name, (bytes, mode, digest));
    }
    shared.finish().map_err(|e| e.to_string())?;
    native.finish().map_err(|e| e.to_string())?;
    if !shared_files.contains_key(manifest::LAUNCHER)
        || !shared_files.contains_key("LICENSE.md")
        || !shared_files.contains_key("ACKNOWLEDGMENTS.md")
        || !shared_files
            .keys()
            .any(|path| path.starts_with("lib/") && path.ends_with(".jar"))
    {
        return Err("LTeX+ archive is missing its server or license files".into());
    }
    Ok((shared_files, native_files))
}
struct HashReader<R> {
    inner: R,
    hash: Sha256,
}
impl<R: Read> Read for HashReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.hash.update(&buffer[..count]);
        Ok(count)
    }
}
fn compress(
    source: &Path,
    destination: &Path,
    name: &str,
    target: Option<&str>,
) -> Result<manifest::Asset, String> {
    let archive = destination.join(format!("{name}.zst"));
    let mut file = fs::File::open(source).map_err(|e| e.to_string())?;
    let bytes = file.metadata().map_err(|e| e.to_string())?.len();
    let mut encoder = zstd::stream::write::Encoder::new(
        fs::File::create(&archive).map_err(|e| e.to_string())?,
        COMPRESSION_LEVEL,
    )
    .map_err(|e| e.to_string())?;
    encoder
        .include_checksum(true)
        .and_then(|_| encoder.window_log(payloads::codec::WINDOW_LOG))
        .and_then(|_| encoder.set_pledged_src_size(Some(bytes)))
        .map_err(|e| e.to_string())?;
    std::io::copy(&mut file, &mut encoder).map_err(|e| e.to_string())?;
    encoder.finish().map_err(|e| e.to_string())?;
    Ok(manifest::Asset {
        name: name.into(),
        target: target.map(str::to_owned),
        bytes,
        sha256: sha256(source)?,
        compressed_bytes: fs::metadata(&archive).map_err(|e| e.to_string())?.len(),
        compressed_sha256: sha256(&archive)?,
    })
}
fn verify_tree(directory: &Path, targets: &[&str], full: bool) -> Result<(), String> {
    let manifest_path = directory.join(manifest::MANIFEST_NAME);
    if !fs::symlink_metadata(&manifest_path)
        .is_ok_and(|m| m.file_type().is_file() && m.len() <= manifest::MAX_MANIFEST_BYTES)
    {
        return Err("LTeX+ package manifest is missing or invalid".into());
    }
    let package = manifest::Manifest::parse(&fs::read(manifest_path).map_err(|e| e.to_string())?)?;
    if targets.len() + 1 != package.files.len()
        || targets.iter().any(|target| !package.has_target(target))
    {
        return Err("LTeX+ package is missing a required Mac runtime".into());
    }
    let expected: BTreeSet<_> = [
        manifest::MANIFEST_NAME.to_owned(),
        "LICENSE.md".into(),
        "ACKNOWLEDGMENTS.md".into(),
        "PROVENANCE.json".into(),
    ]
    .into_iter()
    .chain(package.files.iter().map(manifest::Asset::archive_name))
    .collect();
    let actual: BTreeSet<_> = fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .map(|entry| {
            entry.map_err(|e| e.to_string()).and_then(|entry| {
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "LTeX+ package filename is invalid".to_owned())
            })
        })
        .collect::<Result<_, _>>()?;
    if actual != expected {
        return Err("LTeX+ package contains missing or unexpected files".into());
    }
    for name in ["LICENSE.md", "ACKNOWLEDGMENTS.md", "PROVENANCE.json"] {
        let path = directory.join(name);
        if !fs::symlink_metadata(&path)
            .is_ok_and(|m| m.file_type().is_file() && (1..=MAX_NOTICE_BYTES).contains(&m.len()))
        {
            return Err(format!("LTeX+ package is missing {name}"));
        }
    }
    verify_hash(&directory.join("LICENSE.md"), LICENSE_SHA256)?;
    verify_hash(
        &directory.join("ACKNOWLEDGMENTS.md"),
        ACKNOWLEDGMENTS_SHA256,
    )?;
    for asset in &package.files {
        let path = directory.join(asset.archive_name());
        if !fs::symlink_metadata(&path)
            .is_ok_and(|m| m.file_type().is_file() && m.len() == asset.compressed_bytes)
        {
            return Err(format!("LTeX+ package is missing {}", asset.archive_name()));
        }
        verify_hash(&path, &asset.compressed_sha256)?;
        if full {
            payloads::codec::decode(
                fs::File::open(&path).map_err(|e| e.to_string())?,
                asset.bytes,
                &asset.sha256,
                &mut std::io::sink(),
            )
            .map_err(|e| format!("Invalid LTeX+ archive: {e}"))?;
            if let Some(target) = asset.target.as_deref() {
                verify_native_archive(&path, asset, target)?;
            }
        }
    }
    Ok(())
}

fn verify_native_archive(path: &Path, asset: &manifest::Asset, target: &str) -> Result<(), String> {
    let mut raw = tempfile::tempfile().map_err(|e| e.to_string())?;
    payloads::codec::decode(
        fs::File::open(path).map_err(|e| e.to_string())?,
        asset.bytes,
        &asset.sha256,
        &mut raw,
    )
    .map_err(|e| e.to_string())?;
    use std::io::Seek;
    raw.rewind().map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(raw);
    let expected = [
        format!("{}/bin/java", manifest::JDK_DIRECTORY),
        format!("{}/lib/server/libjvm.dylib", manifest::JDK_DIRECTORY),
    ];
    let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut remaining: BTreeSet<_> = expected.iter().cloned().collect();
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let relative = entry.path().map_err(|e| e.to_string())?.into_owned();
        let name = relative.to_str().ok_or("LTeX+ Java path is not UTF-8")?;
        if expected.iter().any(|expected| expected == name) {
            if !remaining.remove(name) {
                return Err("LTeX+ Java archive contains duplicate binaries".into());
            }
            if !entry.header().entry_type().is_file() {
                return Err("LTeX+ Java binary is not a regular file".into());
            }
            let binary = scratch.path().join(format!("binary-{}", remaining.len()));
            let mut output = fs::File::create(&binary).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut output).map_err(|e| e.to_string())?;
            macos::verify_architectures(&binary, target)?;
        }
    }
    if !remaining.is_empty() {
        return Err("LTeX+ Java runtime is missing native binaries".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_archive_paths_cannot_escape_or_use_another_release() {
        assert_eq!(
            member_path(Path::new("./ltex-ls-plus-18.7.0/bin/ltex-ls-plus")).unwrap(),
            Path::new("bin/ltex-ls-plus")
        );
        for path in [
            "../ltex-ls-plus-18.7.0/bin/java",
            "/ltex-ls-plus-18.7.0/bin/java",
            "ltex-ls-plus-18.6.0/bin/java",
            "ltex-ls-plus-18.7.0/../outside",
        ] {
            assert!(member_path(Path::new(path)).is_err(), "{path}");
        }
    }
    #[test]
    fn java_notice_links_are_contained_and_shared_server_links_rejected() {
        let path = Path::new("jdk-21.0.10+7/legal/java.compiler/LICENSE");
        validate_link(path, Path::new("../java.base/LICENSE")).unwrap();
        for target in ["/tmp/license", "../../../outside"] {
            assert!(validate_link(path, Path::new(target)).is_err());
        }
        assert!(validate_link(Path::new("bin/ltex-ls-plus"), Path::new("elsewhere")).is_err());
    }
    #[test]
    fn split_preserves_server_content_and_separates_java_without_extracting_links() {
        let mut archive = tar::Builder::new(Vec::new());
        for (name, contents) in [
            ("bin/ltex-ls-plus", "#!/bin/sh\n"),
            ("lib/server.jar", "jar content"),
            ("LICENSE.md", "License"),
            ("ACKNOWLEDGMENTS.md", "Notices"),
            ("jdk-21.0.10+7/bin/java", "native java"),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            header.set_size(contents.len() as u64);
            header.set_cksum();
            archive
                .append_data(
                    &mut header,
                    format!("ltex-ls-plus-18.7.0/{name}"),
                    contents.as_bytes(),
                )
                .unwrap();
        }
        let bytes = archive.into_inner().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let (shared, native) = split_reader(&bytes[..], directory.path()).unwrap();
        assert_eq!(shared.len(), 4);
        assert_eq!(native.len(), 1);
        assert!(shared.contains_key("lib/server.jar"));
        assert!(
            !shared
                .keys()
                .any(|path| path.starts_with(manifest::JDK_DIRECTORY))
        );
        assert!(!directory.path().join("jdk-21.0.10+7").exists());
        assert_eq!(
            fs::read(directory.path().join("LICENSE.md")).unwrap(),
            b"License"
        );
    }
}
