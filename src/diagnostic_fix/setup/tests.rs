use super::*;

#[test]
fn transient_range_failures_retry_only_fresh_private_files() {
    let directory = tempfile::tempdir().unwrap();
    let complete = directory.path().join("0");
    fs::write(&complete, b"prior completed range").unwrap();
    let destination = directory.path().join("1");
    let mut attempts = 0;
    download_part(
        &destination,
        16..24,
        &AtomicBool::new(false),
        |range, _, output| {
            attempts += 1;
            assert_eq!(range, 16..24);
            assert_eq!(output.metadata().unwrap().len(), 0);
            if attempts == 1 {
                output.write_all(b"partial").unwrap();
                Err("temporary connection failure".into())
            } else {
                output.write_all(b"complete").unwrap();
                Ok(())
            }
        },
    )
    .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(fs::read(destination).unwrap(), b"complete");
    assert_eq!(fs::read(complete).unwrap(), b"prior completed range");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn canceled_range_retry_publishes_nothing_and_does_not_start_another_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("0");
    let cancel = AtomicBool::new(false);
    let mut attempts = 0;
    let result = download_part(&destination, 0..8, &cancel, |_, token, output| {
        attempts += 1;
        output.write_all(b"partial").unwrap();
        token.store(true, Ordering::Release);
        Err("temporary failure".into())
    });
    assert!(result.unwrap_err().contains("canceled"));
    assert_eq!(attempts, 1);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn catalog_pins_one_model_and_complete_native_closures_for_both_architectures() {
    let catalog = Catalog::parse().unwrap();
    assert_eq!(catalog.model.bytes, 1_099_627_808);
    for runtime in &catalog.runtimes {
        assert!(runtime.files.iter().any(|file| file.name == "llama-server"));
        assert!(
            runtime
                .files
                .iter()
                .any(|file| file.name == "libllama-server-impl.dylib")
        );
        assert!(runtime.files.iter().all(|file| safe_name(&file.name)));
    }
    for name in ["../outside", "/outside", ".", "..", "name/path"] {
        assert!(!safe_name(name));
    }
    let mut catalog = Catalog::parse().unwrap();
    catalog.runtimes[0].files[0].source = "../outside".into();
    assert!(catalog.validate().is_err());
}

#[test]
fn cache_checks_regular_file_size_checksum_and_cancellation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("file");
    fs::write(&path, b"complete").unwrap();
    let hash = hex::encode(Sha256::digest(b"complete"));
    let cancel = AtomicBool::new(false);
    verify_file(&path, 8, &hash, &cancel).unwrap();
    assert!(verify_file(&path, 7, &hash, &cancel).is_err());
    assert!(verify_file(&path, 8, &"0".repeat(64), &cancel).is_err());
    cancel.store(true, Ordering::Release);
    assert!(verify_file(&path, 8, &hash, &cancel).is_err());
}

#[cfg(unix)]
#[test]
fn native_cache_and_notice_publication_never_follow_links() {
    let directory = tempfile::tempdir().unwrap();
    let outside = directory.path().join("outside");
    fs::write(&outside, b"retained").unwrap();
    let link = directory.path().join("license");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert!(write_notice(&link, b"retained").is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"retained");
    assert!(
        verify_file(
            &link,
            8,
            &hex::encode(Sha256::digest(b"retained")),
            &AtomicBool::new(false)
        )
        .is_err()
    );
}

#[test]
fn runtime_extraction_rejects_unsafe_archive_links_before_publication() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.tgz");
    let output = File::create(&path).unwrap();
    let encoder = flate2::write::GzEncoder::new(output, flate2::Compression::fast());
    let mut archive = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_cksum();
    archive
        .append_link(&mut header, "llama-b12110/alias", "../../outside")
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap();
    let catalog = Catalog::parse().unwrap();
    let destination = directory.path().join("installed");
    fs::create_dir(&destination).unwrap();
    assert!(
        extract_runtime(
            &path,
            &catalog.runtimes[0],
            &destination,
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .contains("unsafe link")
    );
    assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
}

#[test]
fn runtime_extraction_publishes_verified_regular_aliases_and_ignores_other_tools() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.tgz");
    let output = File::create(&path).unwrap();
    let encoder = flate2::write::GzEncoder::new(output, flate2::Compression::fast());
    let mut archive = tar::Builder::new(encoder);
    for name in ["libfixture.0.1.dylib", "unused-tool"] {
        let mut header = tar::Header::new_gnu();
        header.set_size(8);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(
                &mut header,
                format!("llama-b12110/{name}"),
                b"complete".as_slice(),
            )
            .unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap();
    let hash = hex::encode(Sha256::digest(b"complete"));
    let runtime = Runtime {
        target: current_target().into(),
        url: String::new(),
        bytes: 1,
        sha256: String::new(),
        files: ["libfixture.0.dylib", "libfixture.dylib"]
            .into_iter()
            .map(|name| NativeFile {
                source: "llama-b12110/libfixture.0.1.dylib".into(),
                name: name.into(),
                bytes: 8,
                sha256: hash.clone(),
            })
            .collect(),
    };
    let destination = directory.path().join("installed");
    fs::create_dir(&destination).unwrap();
    extract_runtime(&path, &runtime, &destination, &AtomicBool::new(false)).unwrap();
    for file in &runtime.files {
        let file = destination.join(&file.name);
        assert_eq!(fs::read(&file).unwrap(), b"complete");
        assert!(!fs::symlink_metadata(file).unwrap().file_type().is_symlink());
    }
    assert_eq!(fs::read_dir(destination).unwrap().count(), 2);
}
