#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
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

fn gallery_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/ui-snapshots/gallery-manifest.tsv")
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

fn write_executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

struct StubGallery {
    temporary: tempfile::TempDir,
    scenes: String,
}

impl StubGallery {
    fn new(scenes: &str, expected_outputs: &[&str]) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::create_dir_all(root.join("docs/ui-snapshots/latest")).unwrap();
        fs::create_dir_all(root.join("target/release")).unwrap();
        fs::create_dir_all(root.join("stubs")).unwrap();
        fs::copy(
            gallery_script(),
            root.join("scripts/capture-theme-gallery.sh"),
        )
        .unwrap();
        fs::copy(
            gallery_manifest(),
            root.join("docs/ui-snapshots/gallery-manifest.tsv"),
        )
        .unwrap();
        fs::write(
            root.join("docs/ui-snapshots/theme-fixture.typ"),
            "= Fixture\n",
        )
        .unwrap();
        fs::write(
            root.join("expected-outputs.txt"),
            format!("{}\n", expected_outputs.join("\n")),
        )
        .unwrap();

        write_executable(
            &root.join("stubs/cargo"),
            r###"#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >> "${STUB_CARGO_LOG}"
"###,
        );
        write_executable(
            &root.join("stubs/magick"),
            r###"#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >> "${STUB_MAGICK_LOG}"
last=""
for argument in "$@"; do last="${argument}"; done
if [[ "${1:-}" == "identify" ]]; then
  if [[ -n "${STUB_MAGICK_SIGNATURE:-}" ]]; then
    printf '%s' "${STUB_MAGICK_SIGNATURE}"
  else
    printf '%s' "${last##*/}"
  fi
elif [[ "$*" == *"-alpha extract"* ]]; then
  printf '%s' "${STUB_MAGICK_CORNERS:-0,0,0,0}"
fi
"###,
        );
        write_executable(
            &root.join("stubs/perl"),
            r###"#!/usr/bin/env bash
set -eu
if [[ "$#" -lt 4 || "$1" != "-e" ]]; then
  printf 'unexpected watchdog invocation\n' >&2
  exit 92
fi
printf 'watchdog=%s app=%s\n' "$3" "$4" >> "${STUB_PERL_LOG}"
shift 3
exec "$@"
"###,
        );
        write_executable(
            &root.join("stubs/mv"),
            r###"#!/usr/bin/env bash
set -eu
count=0
if [[ -f "${STUB_MV_STATE}" ]]; then
  IFS= read -r count < "${STUB_MV_STATE}"
fi
count=$((count + 1))
printf '%d\n' "${count}" > "${STUB_MV_STATE}"
printf '%d %s\n' "${count}" "$*" >> "${STUB_MV_LOG}"
if [[ "${STUB_MV_FAIL_AT:-0}" == "${count}" ]]; then
  printf 'injected move failure %d\n' "${count}" >&2
  exit 73
fi
exec "${STUB_REAL_MV}" "$@"
"###,
        );
        write_executable(
            &root.join("stubs/screencapture"),
            r###"#!/usr/bin/env bash
set -eu
printf 'called\n' >> "${STUB_DESKTOP_CAPTURE_LOG}"
exit 91
"###,
        );
        write_executable(
            &root.join("target/release/tiptoptyp"),
            r###"#!/usr/bin/env bash
set -eu
printf 'run\n' >> "${STUB_APP_RUN_LOG}"
printf '%s\n' "$@" >> "${STUB_APP_ARGS_LOG}"
while IFS= read -r name; do
  [[ -n "${name}" ]] || continue
  [[ "${name}" == "${STUB_APP_OMIT_OUTPUT:-}" ]] && continue
  printf '\211PNG\r\n\032\nstub' > "${STUB_GALLERY_DIRECTORY}/${name}"
done < "${STUB_EXPECTED_OUTPUTS}"
exit "${STUB_APP_STATUS:-0}"
"###,
        );

        Self {
            temporary,
            scenes: scenes.to_owned(),
        }
    }

    fn root(&self) -> &Path {
        self.temporary.path()
    }

    fn latest(&self) -> PathBuf {
        self.root().join("docs/ui-snapshots/latest")
    }

    fn manifest(&self) -> PathBuf {
        self.root().join("docs/ui-snapshots/gallery-manifest.tsv")
    }

    fn log(&self, name: &str) -> PathBuf {
        self.root().join(name)
    }

    fn run(&self, failed_move: usize, app_status: i32) -> Output {
        self.run_process(failed_move, app_status, None, None, None, true)
    }

    fn run_with_image_results(
        &self,
        failed_move: usize,
        app_status: i32,
        corners: Option<&str>,
        signature: Option<&str>,
    ) -> Output {
        self.run_process(failed_move, app_status, corners, signature, None, true)
    }

    fn run_omitting_output(&self, output: &str) -> Output {
        self.run_process(0, 0, None, None, Some(output), true)
    }

    fn run_default(&self) -> Output {
        self.run_process(0, 0, None, None, None, false)
    }

    fn run_process(
        &self,
        failed_move: usize,
        app_status: i32,
        corners: Option<&str>,
        signature: Option<&str>,
        omitted_output: Option<&str>,
        subset: bool,
    ) -> Output {
        let mut paths = vec![self.root().join("stubs")];
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        let path = std::env::join_paths(paths).unwrap();
        let real_mv = ["/bin/mv", "/usr/bin/mv"]
            .into_iter()
            .find(|candidate| Path::new(candidate).is_file())
            .expect("system mv");
        let mut command = Command::new("bash");
        command
            .arg(self.root().join("scripts/capture-theme-gallery.sh"))
            .current_dir(self.root())
            .env("PATH", path)
            .env_remove("CARGO_TARGET_DIR")
            .env("TIPTOPTYP_UI_GALLERY_CAPTURE_TIMEOUT_SECONDS", "5")
            .env("STUB_CARGO_LOG", self.log("cargo.log"))
            .env("STUB_MAGICK_LOG", self.log("magick.log"))
            .env_remove("STUB_MAGICK_CORNERS")
            .env_remove("STUB_MAGICK_SIGNATURE")
            .env("STUB_PERL_LOG", self.log("perl.log"))
            .env("STUB_MV_LOG", self.log("mv.log"))
            .env("STUB_MV_STATE", self.log("mv-state"))
            .env("STUB_MV_FAIL_AT", failed_move.to_string())
            .env("STUB_REAL_MV", real_mv)
            .env("STUB_DESKTOP_CAPTURE_LOG", self.log("desktop-capture.log"))
            .env("STUB_APP_RUN_LOG", self.log("app-runs.log"))
            .env("STUB_APP_ARGS_LOG", self.log("app-args.log"))
            .env("STUB_APP_STATUS", app_status.to_string())
            .env_remove("STUB_APP_OMIT_OUTPUT")
            .env("STUB_GALLERY_DIRECTORY", self.latest())
            .env("STUB_EXPECTED_OUTPUTS", self.log("expected-outputs.txt"));
        for name in OVERRIDE_ENVIRONMENT {
            if name != "TIPTOPTYP_UI_GALLERY_CAPTURE_TIMEOUT_SECONDS" {
                command.env_remove(name);
            }
        }
        if subset {
            command
                .env("TIPTOPTYP_UI_GALLERY_THEMES", "paper-dark")
                .env("TIPTOPTYP_UI_GALLERY_SCENE_THEMES", "catppuccin-latte")
                .env("TIPTOPTYP_UI_GALLERY_SCENES", &self.scenes)
                .env("TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS", "1");
        }
        if let Some(corners) = corners {
            command.env("STUB_MAGICK_CORNERS", corners);
        }
        if let Some(signature) = signature {
            command.env("STUB_MAGICK_SIGNATURE", signature);
        }
        if let Some(output) = omitted_output {
            command.env("STUB_APP_OMIT_OUTPUT", output);
        }
        command.output().expect("run complete stubbed gallery")
    }

    fn print_manifest(&self) -> Output {
        self.print_manifest_with_environment(&[])
    }

    fn print_manifest_with_environment(&self, environment: &[(&str, &str)]) -> Output {
        let mut command = Command::new("bash");
        command
            .arg(self.root().join("scripts/capture-theme-gallery.sh"))
            .arg("--print-manifest")
            .current_dir(self.root());
        for name in OVERRIDE_ENVIRONMENT {
            command.env_remove(name);
        }
        for (name, value) in environment {
            command.env(name, value);
        }
        command.output().expect("parse stub gallery manifest")
    }

    fn read_log(&self, name: &str) -> String {
        fs::read_to_string(self.log(name)).unwrap_or_default()
    }

    fn backup_directories(&self) -> Vec<PathBuf> {
        let private = self.root().join(".tiptoptyp");
        fs::read_dir(private)
            .into_iter()
            .flatten()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("theme-gallery."))
            })
            .collect()
    }
}

fn assert_original_survives(harness: &StubGallery, name: &str, expected: &[u8]) {
    let mut candidates = vec![harness.latest().join(name)];
    candidates.extend(
        harness
            .backup_directories()
            .into_iter()
            .map(|backup| backup.join(name)),
    );
    let survivors = candidates
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| fs::read(path).unwrap())
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
        let names = [
            "main--paper-dark.png",
            "popup-file-menu--catppuccin-latte.png",
        ];
        let harness = StubGallery::new("file-menu", &names);
        fs::write(harness.latest().join(names[0]), b"original-first").unwrap();
        fs::write(harness.latest().join(names[1]), b"original-second").unwrap();

        let output = harness.run(failed_move, 0);
        assert!(!output.status.success());
        assert_eq!(
            fs::read(harness.latest().join(names[0])).unwrap(),
            b"original-first"
        );
        assert_eq!(
            fs::read(harness.latest().join(names[1])).unwrap(),
            b"original-second"
        );
        assert!(
            harness.backup_directories().is_empty(),
            "successful recovery should remove its backup"
        );
    }
}

#[test]
fn gallery_transaction_retains_backups_after_every_failed_restore_step() {
    for failed_move in 3..=4 {
        let names = [
            "main--paper-dark.png",
            "popup-file-menu--catppuccin-latte.png",
        ];
        let harness = StubGallery::new("file-menu", &names);
        fs::write(harness.latest().join(names[0]), b"original-first").unwrap();
        fs::write(harness.latest().join(names[1]), b"original-second").unwrap();

        let output = harness.run(failed_move, 23);
        assert!(!output.status.success());
        assert!(
            !harness.backup_directories().is_empty(),
            "failed recovery must retain its backup"
        );
        assert_original_survives(&harness, names[0], b"original-first");
        assert_original_survives(&harness, names[1], b"original-second");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("recovery was incomplete"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn missing_successful_capture_is_detected_and_restores_the_complete_set() {
    let names = [
        "main--paper-dark.png",
        "popup-file-menu--catppuccin-latte.png",
    ];
    let harness = StubGallery::new("file-menu", &names);
    fs::write(harness.latest().join(names[0]), b"original-main").unwrap();
    fs::write(harness.latest().join(names[1]), b"original-popup").unwrap();

    let output = harness.run_omitting_output(names[1]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Missing or empty gallery capture"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(harness.latest().join(names[0])).unwrap(),
        b"original-main"
    );
    assert_eq!(
        fs::read(harness.latest().join(names[1])).unwrap(),
        b"original-popup"
    );
    assert!(harness.backup_directories().is_empty());
}

#[test]
fn default_gallery_manifest_has_the_maintained_shape_and_order_boundaries() {
    let actual = lines(manifest_command().output().expect("run gallery manifest"));
    assert_eq!(actual.len(), 23);
    assert_eq!(actual.iter().collect::<BTreeSet<_>>().len(), 23);
    assert_eq!(actual.first().unwrap(), "main--catppuccin-latte.png");
    assert_eq!(
        actual
            .iter()
            .filter(|name| !name.ends_with("--catppuccin-latte.png"))
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "main--catppuccin-mocha.png",
            "popup-file-menu--catppuccin-mocha.png",
            "modal-save-dialog--catppuccin-mocha.png"
        ]
    );
    assert_eq!(
        actual.last().unwrap(),
        "workspace-workspace-chooser--catppuccin-latte.png"
    );
}

#[test]
fn checked_in_gallery_contains_required_images_and_every_png_decodes() {
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

    // A reduced capture policy does not require deleting existing evidence.
    // Obsolete slots are pruned only after the next successful default run,
    // which the transaction tests cover separately.
    for name in expected {
        assert!(
            actual.contains(&name),
            "missing required gallery PNG: {name}"
        );
    }

    for name in actual {
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
fn obsolete_pngs_are_pruned_only_by_a_successful_default_run() {
    let default_outputs = lines(manifest_command().output().expect("run gallery manifest"));
    let output_refs = default_outputs
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let default_harness = StubGallery::new("", &output_refs);
    fs::write(default_harness.latest().join("obsolete.png"), b"obsolete").unwrap();
    fs::write(default_harness.latest().join("README.md"), b"keep").unwrap();

    let default_result = default_harness.run_default();
    assert!(
        default_result.status.success(),
        "{}",
        String::from_utf8_lossy(&default_result.stderr)
    );
    assert!(!default_harness.latest().join("obsolete.png").exists());
    assert_eq!(
        fs::read(default_harness.latest().join("README.md")).unwrap(),
        b"keep"
    );

    let subset_names = [
        "main--paper-dark.png",
        "popup-file-menu--catppuccin-latte.png",
    ];
    let subset_harness = StubGallery::new("file-menu", &subset_names);
    fs::write(subset_harness.latest().join("obsolete.png"), b"preserve").unwrap();

    let subset_result = subset_harness.run(0, 0);
    assert!(
        subset_result.status.success(),
        "{}",
        String::from_utf8_lossy(&subset_result.stderr)
    );
    assert_eq!(
        fs::read(subset_harness.latest().join("obsolete.png")).unwrap(),
        b"preserve"
    );
}

#[test]
fn malformed_gallery_contract_is_rejected_before_processes_run() {
    for (record, diagnostic) in [
        (
            "theme\ttiptop-light\tduplicate-slug\n",
            "Duplicate theme id",
        ),
        (
            "scene\tescape\tmain\t../escape\ttargeted\t-\t-\n",
            "Unsafe scene token",
        ),
        (
            "variant\tbefore-components\tcatppuccin-latte\tmain\tmaybe\t30\tprofile\n",
            "Invalid variant inversion",
        ),
        (
            "variant\tbefore-components\tcatppuccin-latte\tmain\t1\t181\tprofile\n",
            "Variant hue is outside",
        ),
        (
            "variant\tbefore-components\tcatppuccin-latte\tmain\t1\t0180\tprofile\n",
            "canonical decimal form",
        ),
    ] {
        let harness = StubGallery::new("file-menu", &[]);
        let mut manifest = fs::read_to_string(harness.manifest()).unwrap();
        manifest.push_str(record);
        fs::write(harness.manifest(), manifest).unwrap();

        let output = harness.print_manifest();
        assert!(
            !output.status.success(),
            "malformed record was accepted: {record:?}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "missing {diagnostic:?} for {record:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(harness.read_log("cargo.log").is_empty());
        assert!(harness.read_log("app-runs.log").is_empty());
    }
}

#[test]
fn manifest_preflight_cannot_be_bypassed_by_subset_overrides() {
    for (record, environment, diagnostic) in [
        (
            "variant\tbefore-components\tmissing-theme\tmain\t0\t0\tmissing-theme\n",
            vec![("TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS", "1")],
            "Unknown built-in gallery theme in variant",
        ),
        (
            "scene-theme\tmissing-theme\n",
            vec![("TIPTOPTYP_UI_GALLERY_SCENE_THEMES", "catppuccin-latte")],
            "Unknown built-in gallery scene theme",
        ),
    ] {
        let harness = StubGallery::new("file-menu", &[]);
        let mut manifest = fs::read_to_string(harness.manifest()).unwrap();
        manifest.push_str(record);
        fs::write(harness.manifest(), manifest).unwrap();

        let output = harness.print_manifest_with_environment(&environment);
        assert!(!output.status.success(), "override masked {record:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "missing {diagnostic:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn manifest_reader_keeps_an_unterminated_final_record() {
    let harness = StubGallery::new("file-menu", &[]);
    let manifest = fs::read_to_string(harness.manifest()).unwrap();
    fs::write(harness.manifest(), manifest.trim_end_matches('\n')).unwrap();

    let output = harness.print_manifest();
    let outputs = lines(output);
    assert_eq!(outputs.len(), 23);
    assert_eq!(
        outputs.last().unwrap(),
        "workspace-workspace-chooser--catppuccin-latte.png"
    );
}

#[test]
fn stubbed_gallery_runs_one_session_and_exercises_image_policies() {
    let names = [
        "main--paper-dark.png",
        "popup-file-menu--catppuccin-latte.png",
        "settings-settings-window--catppuccin-latte.png",
        "settings-settings-theme-picker--catppuccin-latte.png",
        "settings-settings-dark-theme-picker--catppuccin-latte.png",
        "settings-settings-tooltip--catppuccin-latte.png",
    ];
    let scenes = concat!(
        "file-menu settings-window settings-theme-picker ",
        "settings-dark-theme-picker settings-tooltip"
    );
    let harness = StubGallery::new(scenes, &names);
    let output = harness.run(0, 0);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(harness.read_log("app-runs.log"), "run\n");
    assert_eq!(
        harness.read_log("cargo.log"),
        "build --release --locked --bin tiptoptyp\n"
    );
    assert_eq!(
        harness.read_log("perl.log"),
        format!(
            "watchdog=11 app={}\n",
            harness.root().join("target/release/tiptoptyp").display()
        )
    );
    let arguments = harness
        .read_log("app-args.log")
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut expected_arguments = vec![
        "--ui-screenshot-latest".to_owned(),
        "--ui-screenshot-exit".to_owned(),
        "--ui-screenshot-settle".to_owned(),
        "30".to_owned(),
    ];
    for step in [
        "paper-dark,main,false,0",
        "catppuccin-latte,file-menu,false,0",
        "catppuccin-latte,settings-window,false,0",
        "catppuccin-latte,settings-theme-picker,false,0",
        "catppuccin-latte,settings-dark-theme-picker,false,0",
        "catppuccin-latte,settings-tooltip,false,0",
    ] {
        expected_arguments.push("--ui-screenshot-step".to_owned());
        expected_arguments.push(step.to_owned());
    }
    expected_arguments.push(
        harness
            .root()
            .join("docs/ui-snapshots/theme-fixture.typ")
            .to_string_lossy()
            .into_owned(),
    );
    assert_eq!(arguments, expected_arguments);

    for name in names {
        assert!(
            fs::read(harness.latest().join(name))
                .unwrap()
                .starts_with(b"\x89PNG\r\n\x1a\n"),
            "stub app did not publish {name}"
        );
    }
    let image_calls = harness.read_log("magick.log");
    assert_eq!(
        image_calls
            .lines()
            .filter(|line| line.ends_with(" null:"))
            .count(),
        names.len()
    );
    assert_eq!(
        image_calls
            .lines()
            .filter(|line| line.contains("-alpha extract"))
            .count(),
        1
    );
    assert_eq!(
        image_calls
            .lines()
            .filter(|line| line.starts_with("identify "))
            .count(),
        4
    );
    for corner in ["p{0,0}", "p{w-1,0}", "p{0,h-1}", "p{w-1,h-1}"] {
        assert!(
            image_calls.contains(corner),
            "runtime alpha inspection omitted {corner}"
        );
    }
    assert!(harness.read_log("desktop-capture.log").is_empty());
}

#[test]
fn gallery_rejects_visual_policy_failures_and_restores_originals() {
    let cases = [
        (
            "file-menu",
            vec![
                "main--paper-dark.png",
                "popup-file-menu--catppuccin-latte.png",
            ],
            Some("1,0,0,0"),
            None,
            "opaque outer corner",
        ),
        (
            "settings-window settings-theme-picker",
            vec![
                "main--paper-dark.png",
                "settings-settings-window--catppuccin-latte.png",
                "settings-settings-theme-picker--catppuccin-latte.png",
            ],
            None,
            Some("identical-pixels"),
            "pixel-identical",
        ),
    ];

    for (scenes, names, corners, signature, diagnostic) in cases {
        let harness = StubGallery::new(scenes, &names);
        for (index, name) in names.iter().enumerate() {
            fs::write(
                harness.latest().join(name),
                format!("original-{index}").as_bytes(),
            )
            .unwrap();
        }

        let output = harness.run_with_image_results(0, 0, corners, signature);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "missing {diagnostic:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        for (index, name) in names.iter().enumerate() {
            assert_eq!(
                fs::read(harness.latest().join(name)).unwrap(),
                format!("original-{index}").as_bytes()
            );
        }
        assert!(harness.backup_directories().is_empty());
    }
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
