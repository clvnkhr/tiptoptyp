//! Installing a bundle must preserve target selection and the running app.
#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Output},
};

struct Installation {
    directory: tempfile::TempDir,
}

impl Installation {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
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
            (
                "cargo",
                "printf '%s|%s\n' \"$TIPTOPTYP_PACKAGE_TARGET\" \"$*\" >> \"$TEST_LOG\"",
            ),
            ("ditto", "cp -R \"$3\" \"$4\""),
            ("pgrep", "echo pgrep >> \"$TEST_LOG\"; exit 0"),
            ("open", "echo open >> \"$TEST_LOG\"; exit 99"),
            ("osascript", "echo osascript >> \"$TEST_LOG\"; exit 99"),
        ] {
            let path = root.join("bin").join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { directory }
    }

    fn run(&self, arguments: &[&str], environment_target: Option<&str>) -> Output {
        let root = self.directory.path();
        let mut command = Command::new("sh");
        command
            .arg(root.join("scripts/install.sh"))
            .args(arguments)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", root.join("bin").display()),
            )
            .env("TEST_LOG", root.join("log"))
            .env("TIPTOPTYP_APPLICATIONS_DIR", root.join("Applications"))
            .env_remove("TIPTOPTYP_PACKAGE_TARGET");
        if let Some(target) = environment_target {
            command.env("TIPTOPTYP_PACKAGE_TARGET", target);
        }
        command.output().unwrap()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.directory.path().join("log")).unwrap()
    }
}

#[test]
fn installer_copies_verified_universal_bundle_without_quitting_or_launching() {
    let installation = Installation::new();
    let output = installation.run(&[], None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let root = installation.directory.path();
    assert_eq!(
        fs::read_to_string(root.join("Applications/tiptoptyp.app/new")).unwrap(),
        "new"
    );
    assert!(!root.join("Applications/tiptoptyp.app/old").exists());
    let log = installation.log();
    assert!(log.contains("packager --config"));
    assert!(log.contains("verify-package --target universal-apple-darwin"));
    assert_eq!(log.matches("--target universal-apple-darwin").count(), 2);
    assert!(
        log.lines()
            .all(|line| line.starts_with("universal-apple-darwin|"))
    );
    for forbidden in ["pgrep", "open", "osascript"] {
        assert!(!log.contains(forbidden), "installer controlled app: {log}");
    }
}

#[test]
fn installer_applies_explicit_thin_target_to_packager_and_hook() {
    let installation = Installation::new();
    let output = installation.run(
        &["--target", "x86_64-apple-darwin"],
        Some("aarch64-apple-darwin"),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = installation.log();
    assert_eq!(log.matches("--target x86_64-apple-darwin").count(), 2);
    assert!(
        log.lines()
            .all(|line| line.starts_with("x86_64-apple-darwin|"))
    );
    assert!(!log.contains("aarch64-apple-darwin"));
}

#[test]
fn installer_uses_target_environment_when_no_override_is_supplied() {
    let installation = Installation::new();
    let output = installation.run(&[], Some("aarch64-apple-darwin"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        installation
            .log()
            .matches("--target aarch64-apple-darwin")
            .count(),
        2
    );
}

#[test]
fn installer_rejects_invalid_arguments_before_replacing_existing_app() {
    for arguments in [
        vec!["--target"],
        vec!["--target", "unknown"],
        vec!["--typo"],
    ] {
        let installation = Installation::new();
        let output = installation.run(&arguments, None);
        assert_eq!(output.status.code(), Some(2));
        let root = installation.directory.path();
        assert!(!root.join("log").exists());
        assert_eq!(
            fs::read_to_string(root.join("Applications/tiptoptyp.app/old")).unwrap(),
            "old"
        );
    }
}
