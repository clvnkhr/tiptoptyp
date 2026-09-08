use std::process::{Command, Output};

const SCREENSHOT_ENVIRONMENT: &[&str] = &[
    "TIPTOPTYP_UI_SCREENSHOTS",
    "TIPTOPTYP_UI_SCREENSHOTS_ENABLED",
    "TIPTOPTYP_UI_SCREENSHOT_EXIT",
    "TIPTOPTYP_UI_SCREENSHOT_LATEST",
    "TIPTOPTYP_UI_SCREENSHOT_SETTLE_FRAMES",
    "TIPTOPTYP_UI_SCREENSHOT_SHORTCUT",
    "TIPTOPTYP_UI_SCREENSHOT_SUBDIR",
    "TIPTOPTYP_UI_SNAPSHOT_SCENE",
    "TIPTOPTYP_UI_THEME",
    "TIPTOPTYP_UI_THEME_HUE_SHIFT",
    "TIPTOPTYP_UI_THEME_INVERT",
];

fn run_invalid_launch(arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tiptoptyp"));
    command.args(arguments);
    for name in SCREENSHOT_ENVIRONMENT {
        command.env_remove(name);
    }
    command.output().expect("run tiptoptyp launch validation")
}

#[test]
fn invalid_snapshot_scene_exits_unsuccessfully_before_gui_startup() {
    let output = run_invalid_launch(&["--ui-snapshot-scene=__invalid__"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown UI snapshot scene"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn disabled_capture_batch_exits_unsuccessfully_instead_of_reaching_shell() {
    let output = run_invalid_launch(&[
        "--ui-screenshot-step",
        "catppuccin-latte,main,false,0",
        "--no-ui-screenshots",
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--no-ui-screenshots"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unknown_flags_exit_unsuccessfully_instead_of_becoming_document_paths() {
    let output = run_invalid_launch(&["--unknown-launch-option"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown option"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
