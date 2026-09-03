#![cfg(unix)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Command, Output};

const OVERRIDE_ENVIRONMENT: [&str; 5] = [
    "TIPTOPTYP_UI_GALLERY_THEMES",
    "TIPTOPTYP_UI_GALLERY_SCENES",
    "TIPTOPTYP_UI_GALLERY_SCENE_THEMES",
    "TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS",
    "TIPTOPTYP_UI_GALLERY_CAPTURE_TIMEOUT_SECONDS",
];

fn gallery_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/capture-theme-gallery.sh")
}

fn manifest_command() -> Command {
    let mut command = Command::new("bash");
    command.arg(gallery_script()).arg("--print-manifest");
    for name in OVERRIDE_ENVIRONMENT {
        command.env_remove(name);
    }
    command
}

fn lines(output: Output) -> Vec<String> {
    assert!(
        output.status.success(),
        "manifest command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("manifest is UTF-8")
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

#[test]
fn default_gallery_manifest_is_the_exact_maintained_matrix() {
    let actual = lines(manifest_command().output().expect("run gallery manifest"));
    let themes = [
        "tiptop-light",
        "tiptop-dark",
        "paper-light",
        "paper-dark",
        "ocean-light",
        "ocean-dark",
        "forest-light",
        "forest-dark",
        "catppuccin-latte",
        "catppuccin-frappe",
        "catppuccin-macchiato",
        "catppuccin-mocha",
        "solarized-light",
        "solarized-dark",
        "gruvbox-light",
        "gruvbox-dark",
        "github-light-default",
        "github-dark-default",
        "rose-pine-dawn",
        "rose-pine",
        "tokyo-night-light",
        "tokyo-night",
        "kanagawa-lotus",
        "kanagawa-wave",
        "everforest-light",
        "everforest-dark",
        "ayu-light",
        "ayu-dark",
        "flexoki-light",
        "flexoki-dark",
        "dracula-alucard",
        "dracula",
    ];
    let scenes = [
        ("popup", "file-menu"),
        ("popup", "edit-menu"),
        ("settings", "settings-window"),
        ("settings", "settings-theme-picker"),
        ("settings", "settings-dark-theme-picker"),
        ("settings", "settings-tooltip"),
        ("diagnostic", "diagnostic-tooltip"),
        ("modal", "save-dialog"),
        ("modal", "alert-dialog"),
        ("modal", "overwrite-dialog"),
        ("popup", "editor-context-menu"),
        ("popup", "explorer-context-menu"),
        ("rename", "rename-dialog"),
        ("workspace", "workspace-chooser"),
        ("main", "problems-panel"),
        ("main", "find-replace"),
        ("main", "preview-compiling"),
    ];
    let mut expected = Vec::new();
    for theme in themes {
        expected.push(format!("main--{theme}.png"));
    }
    for theme in ["catppuccin-latte", "catppuccin-mocha"] {
        for (target, scene) in scenes {
            expected.push(format!("{target}-{scene}--{theme}.png"));
        }
    }
    expected.extend([
        "main--catppuccin-latte-inverted-hue-p30.png".to_owned(),
        "popup-file-menu--catppuccin-latte-inverted-hue-p30.png".to_owned(),
    ]);

    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 68);
    assert_eq!(actual.iter().collect::<BTreeSet<_>>().len(), 68);
}

#[test]
fn subset_manifest_respects_every_override() {
    let output = manifest_command()
        .env("TIPTOPTYP_UI_GALLERY_THEMES", "paper-dark")
        .env("TIPTOPTYP_UI_GALLERY_SCENE_THEMES", "catppuccin-latte")
        .env(
            "TIPTOPTYP_UI_GALLERY_SCENES",
            "alert-dialog overwrite-dialog",
        )
        .env("TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS", "1")
        .output()
        .expect("run subset gallery manifest");

    assert_eq!(
        lines(output),
        [
            "main--paper-dark.png",
            "modal-alert-dialog--catppuccin-latte.png",
            "modal-overwrite-dialog--catppuccin-latte.png",
        ]
    );
}

#[test]
fn gallery_script_never_invokes_a_desktop_capture_api() {
    let script = std::fs::read_to_string(gallery_script()).expect("read gallery script");
    assert!(!script.contains("screencapture"));
}

#[test]
fn gallery_script_checks_every_elevated_target_and_all_four_corners() {
    let script = std::fs::read_to_string(gallery_script()).expect("read gallery script");
    assert!(script.contains("popup-*|diagnostic-*|modal-*|rename-*|workspace-*"));
    assert!(script.contains("p{0,0}.r"));
    assert!(script.contains("p{w-1,0}.r"));
    assert!(script.contains("p{0,h-1}.r"));
    assert!(script.contains("p{w-1,h-1}.r"));
}

#[test]
fn gallery_watchdog_timeout_is_validated_without_launching_the_app() {
    for invalid in ["0", "3601", "1.5", "forever"] {
        let output = manifest_command()
            .env("TIPTOPTYP_UI_GALLERY_CAPTURE_TIMEOUT_SECONDS", invalid)
            .output()
            .expect("validate gallery timeout");
        assert!(!output.status.success(), "timeout {invalid:?} was accepted");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Gallery capture timeout"),
            "missing timeout diagnostic for {invalid:?}"
        );
    }

    let valid = manifest_command()
        .env("TIPTOPTYP_UI_GALLERY_CAPTURE_TIMEOUT_SECONDS", "90")
        .output()
        .expect("validate gallery timeout");
    assert!(valid.status.success());
}
