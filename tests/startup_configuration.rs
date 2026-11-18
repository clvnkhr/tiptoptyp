//! Exercise the shipped executable's read-only admission, without a desktop.
use std::{collections::BTreeMap, process::Command};

use serde_json::{Value, json};

fn valid_profile() -> Value {
    json!({
        "interface_theme": "System",
        "light_theme": {"source":"builtin", "value":"tiptop-light"},
        "dark_theme": {"source":"builtin", "value":"tiptop-dark"},
        "theme_invert": false,
        "theme_hue_shift_degrees": 0,
        "document_theme": "FollowInterface",
        "line_wrap": true,
        "line_numbers": true,
        "source_preview_trigger": "DoubleClick",
        "auto_save": true,
        "auto_save_delay_ms": 750,
        "hover_delay_ms": 300,
        "ui_scale_percent": 100,
        "ui_font_monospace": false,
        "ui_font_path": null,
        "ui_font_family": null,
        "ui_font_face_index": 0,
        "ui_font_weight": 400,
        "code_font_path": null,
        "code_font_family": null,
        "code_font_face_index": 0,
        "code_font_weight": 400,
        "titlebar_menus": false,
        "typst_overrides": {},
        "recent_workspaces": [],
        "last_opened_files": {},
        "typst": {"mode":"Bundled", "custom_path":""},
        "tinymist": {"mode":"Bundled", "custom_path":""},
        "tinymist_configuration": {"formatterProseWrap":"none"}
    })
}

#[test]
fn packaged_preflight_rejects_incompatible_profiles_without_modifying_them() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.ron");
    let mut retired = valid_profile();
    retired["tinymist_configuration"]["formatterProseWrap"] = false.into();
    let mut unknown = valid_profile();
    unknown["retired_setting"] = true.into();
    for (profile, diagnostic) in [
        (valid_profile(), None),
        (retired, Some("formatterProseWrap")),
        (unknown, Some("retired_setting")),
    ] {
        let envelope = BTreeMap::from([("tiptoptyp.settings.v1", profile.to_string())]);
        let bytes = ron::to_string(&envelope).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_tiptoptyp"))
            .arg("--check-settings")
            .arg(&path)
            .current_dir(directory.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        if let Some(diagnostic) = diagnostic {
            assert!(
                !output.status.success(),
                "incompatible settings exited successfully"
            );
            assert!(stderr.contains(diagnostic), "{stderr}");
            assert!(stderr.contains(path.to_str().unwrap()), "{stderr}");
        } else {
            assert!(output.status.success(), "{stderr}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes.as_bytes());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

#[test]
fn packaged_preflight_checks_defaults_without_creating_first_run_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.ron");
    let output = Command::new(env!("CARGO_BIN_EXE_tiptoptyp"))
        .arg("--check-settings")
        .arg(&path)
        .current_dir(directory.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
