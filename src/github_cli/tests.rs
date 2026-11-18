use super::*;

fn asset(bytes: &[u8], zip: bool) -> Asset {
    Asset {
        url: "https://github.com/cli/cli/releases/download/test/test.zip".into(),
        bytes: 0,
        sha256: String::new(),
        entry: "distribution/bin/gh".into(),
        binary_bytes: bytes.len() as u64,
        binary_sha256: hex::encode(Sha256::digest(bytes)),
        zip,
    }
}

#[test]
fn pinned_catalog_covers_supported_platforms_and_pins_executable_bytes() {
    let manifest = manifest().unwrap();
    assert_eq!(manifest.version, "2.103.3");
    assert_eq!(manifest.assets.len(), 6);
    for asset in manifest.assets.values() {
        assert!(
            asset
                .url
                .starts_with("https://github.com/cli/cli/releases/download/v2.103.3/")
        );
        assert!((1..=32 * 1024 * 1024).contains(&asset.bytes));
        assert!((1..=80 * 1024 * 1024).contains(&asset.binary_bytes));
        assert_eq!(hex::decode(&asset.sha256).unwrap().len(), 32);
        assert_eq!(hex::decode(&asset.binary_sha256).unwrap().len(), 32);
        assert!(asset.entry.ends_with("bin/gh") || asset.entry.ends_with("bin/gh.exe"));
    }
}

#[test]
fn copying_rejects_wrong_digest_truncated_excess_and_cancelled_content() {
    let cancel = AtomicBool::new(false);
    let expected = hex::encode(Sha256::digest(b"good"));
    for bytes in [&b"evil"[..], &b"goo"[..], &b"good-extra"[..]] {
        assert!(copy_verified(&mut &bytes[..], &mut Vec::new(), 4, &expected, &cancel).is_err());
    }
    let mut output = Vec::new();
    copy_verified(&mut &b"good"[..], &mut output, 4, &expected, &cancel).unwrap();
    assert_eq!(output, b"good");
    output.clear();
    cancel.store(true, Ordering::Release);
    assert!(copy_verified(&mut &b"good"[..], &mut output, 4, &expected, &cancel).is_err());
    assert!(output.is_empty());
}

#[test]
fn cached_receipts_cannot_authorize_a_modified_executable() {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("gh");
    fs::write(&program, b"evil").unwrap();
    fs::write(
        directory.path().join("receipt.json"),
        br#"{"healthy":true}"#,
    )
    .unwrap();
    let asset = asset(b"good", true);
    assert!(!verify_file(&program, 4, &asset.binary_sha256, &AtomicBool::new(false)).unwrap());
}

#[test]
fn healthy_cached_helper_requires_no_download_and_preserves_auth_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let auth = directory.path().join("auth");
    private_directory(&auth).unwrap();
    fs::write(auth.join("hosts.yml"), b"account metadata").unwrap();
    let program = directory
        .path()
        .join(if cfg!(windows) { "gh.exe" } else { "gh" });
    fs::write(&program, b"good").unwrap();
    let prepared = prepare_at(
        directory.path(),
        &auth,
        &asset(b"good", true),
        &AtomicBool::new(false),
        &mut |_| panic!("a verified cache must not start a download"),
    )
    .unwrap();
    assert_eq!(prepared.program, program);
    assert!(!prepared.is_test_fixture());
    assert_eq!(
        fs::read(auth.join("hosts.yml")).unwrap(),
        b"account metadata"
    );
    assert_eq!(fs::read(directory.path().join("LICENSE")).unwrap(), LICENSE);
}

#[test]
fn zip_extracts_only_the_pinned_file_without_extracting_paths() {
    let directory = tempfile::tempdir().unwrap();
    let mut archive = tempfile::tempfile().unwrap();
    let asset = asset(b"good", true);
    {
        let mut writer = zip::ZipWriter::new(&mut archive);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file(&asset.entry, options).unwrap();
        writer.write_all(b"good").unwrap();
        writer.start_file("../outside", options).unwrap();
        writer.write_all(b"ignored").unwrap();
        writer.finish().unwrap();
    }
    archive.rewind().unwrap();
    let mut output = tempfile::tempfile().unwrap();
    extract(&mut archive, &mut output, &asset, &AtomicBool::new(false)).unwrap();
    output.rewind().unwrap();
    let mut bytes = Vec::new();
    output.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"good");
    assert!(!directory.path().join("outside").exists());
}

#[test]
fn linked_helper_archive_entries_are_never_published() {
    let mut archive = tempfile::tempfile().unwrap();
    let asset = asset(b"target", true);
    {
        let mut writer = zip::ZipWriter::new(&mut archive);
        writer
            .add_symlink(
                &asset.entry,
                "target",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.finish().unwrap();
    }
    archive.rewind().unwrap();
    let mut output = tempfile::tempfile().unwrap();
    assert!(extract(&mut archive, &mut output, &asset, &AtomicBool::new(false)).is_err());
    assert_eq!(output.metadata().unwrap().len(), 0);
}

#[test]
fn cancelled_prepare_stops_before_accessing_application_data() {
    assert!(
        prepare(&AtomicBool::new(true), &mut |_| panic!(
            "cancelled preparation cannot start work"
        ))
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn private_data_directories_and_executables_cannot_follow_links() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("gh");
    fs::write(&target, b"good").unwrap();
    let link = directory.path().join("gh");
    symlink(&target, &link).unwrap();
    assert!(open_regular(&link).is_err());
    let linked_directory = directory.path().join("auth");
    symlink(outside.path(), &linked_directory).unwrap();
    assert!(private_directory(&linked_directory).is_err());
    assert_eq!(fs::read(target).unwrap(), b"good");
}
