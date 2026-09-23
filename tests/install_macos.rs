//! Installing a bundle must not control the user's running app.
#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn installer_copies_verified_bundle_without_quitting_or_launching() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::create_dir_all(root.join("target/release/tiptoptyp.app")).unwrap();
    fs::create_dir_all(root.join("Applications/tiptoptyp.app")).unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(root.join("target/release/tiptoptyp.app/new"), "new").unwrap();
    fs::write(root.join("Applications/tiptoptyp.app/old"), "old").unwrap();
    fs::write(
        root.join("scripts/install.sh"),
        include_str!("../scripts/install-macos-app.sh"),
    )
    .unwrap();
    for (name, body) in [
        ("uname", "echo Darwin"),
        ("cargo-packager", "exit 0"),
        ("cargo", "printf '%s\\n' \"$*\" >> \"$TEST_LOG\""),
        ("ditto", "cp -R \"$3\" \"$4\""),
        ("pgrep", "echo pgrep >> \"$TEST_LOG\"; exit 0"),
        ("open", "echo open >> \"$TEST_LOG\"; exit 99"),
        ("osascript", "echo osascript >> \"$TEST_LOG\"; exit 99"),
    ] {
        let path = root.join("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = Command::new("sh")
        .arg(root.join("scripts/install.sh"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .env("TEST_LOG", root.join("log"))
        .env("TIPTOPTYP_APPLICATIONS_DIR", root.join("Applications"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("Applications/tiptoptyp.app/new")).unwrap(),
        "new"
    );
    assert!(!root.join("Applications/tiptoptyp.app/old").exists());
    let log = fs::read_to_string(root.join("log")).unwrap();
    assert!(log.contains("packager --config"));
    assert!(log.contains("verify-package"));
    for forbidden in ["pgrep", "open", "osascript"] {
        assert!(!log.contains(forbidden), "installer controlled app: {log}");
    }
}
