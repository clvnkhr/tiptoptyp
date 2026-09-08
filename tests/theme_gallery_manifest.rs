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

fn gallery_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/ui-snapshots/latest")
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
        ("main", "problems-panel"),
        ("main", "find-replace"),
        ("main", "preview-compiling"),
        ("popup", "file-menu"),
        ("popup", "edit-menu"),
        ("popup", "editor-context-menu"),
        ("popup", "explorer-context-menu"),
        ("settings", "settings-window"),
        ("settings", "settings-theme-picker"),
        ("settings", "settings-dark-theme-picker"),
        ("settings", "settings-tooltip"),
        ("diagnostic", "diagnostic-tooltip"),
        ("modal", "save-dialog"),
        ("modal", "alert-dialog"),
        ("modal", "overwrite-dialog"),
        ("rename", "rename-dialog"),
        ("workspace", "workspace-chooser"),
    ];
    let mut expected = Vec::new();
    for theme in themes {
        expected.push(format!("main--{theme}.png"));
    }
    expected.push("main--catppuccin-latte-inverted-hue-p30.png".to_owned());
    for (target, scene) in scenes {
        for theme in ["catppuccin-latte", "catppuccin-mocha"] {
            expected.push(format!("{target}-{scene}--{theme}.png"));
        }
    }
    expected.push("popup-file-menu--catppuccin-latte-inverted-hue-p30.png".to_owned());

    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 68);
    assert_eq!(actual.iter().collect::<BTreeSet<_>>().len(), 68);
}

#[test]
fn checked_in_gallery_matches_the_manifest_and_every_png_decodes() {
    let expected = lines(manifest_command().output().expect("run gallery manifest"));
    let mut actual = std::fs::read_dir(gallery_directory())
        .expect("read checked-in gallery")
        .map(|entry| entry.expect("read gallery entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .map(|path| {
            path.file_name()
                .expect("gallery PNG has a filename")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    actual.sort();

    let mut expected_sorted = expected.clone();
    expected_sorted.sort();
    assert_eq!(actual, expected_sorted);

    for name in expected {
        let path = gallery_directory().join(&name);
        let image = image::ImageReader::open(&path)
            .unwrap_or_else(|error| panic!("open gallery PNG {name}: {error}"))
            .with_guessed_format()
            .unwrap_or_else(|error| panic!("identify gallery PNG {name}: {error}"))
            .decode()
            .unwrap_or_else(|error| panic!("decode gallery PNG {name}: {error}"));
        assert!(
            image.width() > 0 && image.height() > 0,
            "empty gallery PNG {name}"
        );
    }
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
fn gallery_runs_all_capture_steps_in_one_app_session() {
    let script = std::fs::read_to_string(gallery_script()).expect("read gallery script");
    assert!(script.contains("--ui-screenshot-step"));
    assert!(script.contains("capture_gallery"));
    assert!(!script.contains("capture_job"));
    assert!(!script.contains("while (( job_index"));
}

#[test]
fn agent_workflow_uses_risk_based_ui_evidence() {
    let instructions =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("AGENTS.md"))
            .expect("read agent workflow");
    for required in [
        "Risk-based UI workflow",
        "Routine behavior changes, refactors, and fixes with adequate deterministic",
        "only when correctness is",
        "--ui-snapshot-scene",
        "TIPTOPTYP_UI_TRACE=1",
        "scripts/capture-theme-gallery.sh --validate-latest",
        "Do not claim visual verification unless a fresh PNG was inspected",
    ] {
        assert!(
            instructions.contains(required),
            "missing workflow rule: {required}"
        );
    }
    assert!(!instructions.contains("Mandatory UI workflow"));
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
