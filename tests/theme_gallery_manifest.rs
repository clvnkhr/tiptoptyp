#![cfg(unix)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
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

fn gallery_transaction_source() -> String {
    let source = std::fs::read_to_string(gallery_script()).expect("read gallery script");
    let start = source
        .find("backup_directory=\"\"\nrestore_outputs_on_exit=0\n")
        .expect("gallery transaction start");
    let end = source[start..]
        .find("\napp_binary=\"\"\n")
        .map(|offset| start + offset)
        .expect("gallery transaction end");
    source[start..end].to_owned()
}

fn run_gallery_transaction_fault(root: &Path, failed_move: usize, restore: bool) -> Output {
    let shell = format!(
        r#"set -euo pipefail
{}
latest_directory="$1/gallery"
backup_directory="$1/backup"
expected_outputs=(first.png second.png)
move_count=0
failed_move="$2"
mv() {{
  move_count=$((move_count + 1))
  if (( move_count == failed_move )); then
    printf 'injected move failure %d\n' "${{move_count}}" >&2
    return 1
  fi
  command mv "$@"
}}
trap cleanup EXIT
backup_requested_outputs
if [[ "$3" == restore ]]; then
  printf generated-first > "${{latest_directory}}/first.png"
  printf generated-second > "${{latest_directory}}/second.png"
  exit 23
fi
"#,
        gallery_transaction_source()
    );
    Command::new("bash")
        .arg("-c")
        .arg(shell)
        .arg("gallery-transaction-test")
        .arg(root)
        .arg(failed_move.to_string())
        .arg(if restore { "restore" } else { "backup" })
        .output()
        .expect("run gallery transaction fault injection")
}

fn assert_original_survives(root: &Path, name: &str, expected: &[u8]) {
    let gallery = root.join("gallery").join(name);
    let backup = root.join("backup").join(name);
    let survivors = [gallery, backup]
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| std::fs::read(path).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        survivors.len(),
        1,
        "original bytes for {name} did not survive exactly once"
    );
    assert_eq!(survivors[0], expected);
}

#[test]
fn gallery_transaction_recovers_every_partial_backup_step() {
    for failed_move in 1..=2 {
        let temporary = tempfile::tempdir().unwrap();
        let gallery = temporary.path().join("gallery");
        let backup = temporary.path().join("backup");
        std::fs::create_dir(&gallery).unwrap();
        std::fs::create_dir(&backup).unwrap();
        std::fs::write(gallery.join("first.png"), b"original-first").unwrap();
        std::fs::write(gallery.join("second.png"), b"original-second").unwrap();

        let output = run_gallery_transaction_fault(temporary.path(), failed_move, false);
        assert!(!output.status.success());
        assert_eq!(
            std::fs::read(gallery.join("first.png")).unwrap(),
            b"original-first"
        );
        assert_eq!(
            std::fs::read(gallery.join("second.png")).unwrap(),
            b"original-second"
        );
        assert!(
            !backup.exists(),
            "successful recovery should remove its backup"
        );
    }
}

#[test]
fn gallery_transaction_retains_backups_after_every_failed_restore_step() {
    for failed_move in 3..=4 {
        let temporary = tempfile::tempdir().unwrap();
        let gallery = temporary.path().join("gallery");
        let backup = temporary.path().join("backup");
        std::fs::create_dir(&gallery).unwrap();
        std::fs::create_dir(&backup).unwrap();
        std::fs::write(gallery.join("first.png"), b"original-first").unwrap();
        std::fs::write(gallery.join("second.png"), b"original-second").unwrap();

        let output = run_gallery_transaction_fault(temporary.path(), failed_move, true);
        assert!(!output.status.success());
        assert!(backup.exists(), "failed recovery must retain its backup");
        assert_original_survives(temporary.path(), "first.png", b"original-first");
        assert_original_survives(temporary.path(), "second.png", b"original-second");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("recovery was incomplete"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
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
