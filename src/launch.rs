//! Startup argument and persistence policy. Parsing performs no filesystem or tool probes.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::screenshot::{
    CaptureConfig, CaptureSpec, CaptureThemeProfile, LATEST_OUTPUT_SUBDIRECTORY, UiCaptureStep,
    UiSnapshotScene, parse_bool, parse_hue_shift, parse_settle_frames, parse_shortcut,
    parse_theme_name, private_output_directory,
};

/// Application launch policy, capture configuration and optional workspace/document path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchOptions {
    pub mode: LaunchMode,
    /// Profiling uses session-owned storage, never the normal preferences path.
    pub persistence_path: Option<PathBuf>,
    pub initial_path: Option<PathBuf>,
    pub captures: CaptureConfig,
    /// Explicit QA-only theme override. Normal launches leave this unset so
    /// persisted appearance settings remain authoritative.
    pub theme_profile: Option<CaptureThemeProfile>,
    /// QA-only state for exposing one themed component before capture.
    pub ui_snapshot_scene: Option<UiSnapshotScene>,
    /// Ordered captures performed by one live application instance.
    pub ui_capture_steps: Vec<UiCaptureStep>,
}

/// Outer-shell policy selected before any editor state is constructed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    Interactive,
    DeterministicCapture,
    /// Profiling can drive either normal interaction or a non-persistent QA scene.
    Profiling {
        deterministic_capture: bool,
    },
}

impl LaunchMode {
    pub const fn persists_settings(self) -> bool {
        matches!(
            self,
            Self::Interactive
                | Self::Profiling {
                    deterministic_capture: false
                }
        )
    }
}

impl LaunchOptions {
    /// Parse process arguments and `TIPTOPTYP_UI_SCREENSHOT_*` environment
    /// variables. Capture flags are removed before selecting the initial path.
    pub fn from_process(profile_storage: Option<PathBuf>) -> Result<Self, String> {
        let working_directory = std::env::current_dir()
            .map_err(|error| format!("could not determine the working directory: {error}"))?;
        parse_launch_options(std::env::args_os().skip(1), &working_directory, |name| {
            std::env::var_os(name)
        })
        .map(|options| options.with_profile_storage(profile_storage))
    }

    fn with_profile_storage(mut self, storage: Option<PathBuf>) -> Self {
        if storage.is_some() {
            self.mode = LaunchMode::Profiling {
                deterministic_capture: !self.mode.persists_settings(),
            };
        }
        self.persistence_path = storage;
        self
    }
}

pub(crate) fn parse_launch_options<I, S, F>(
    arguments: I,
    working_directory: &Path,
    environment: F,
) -> Result<LaunchOptions, String>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    F: Fn(&str) -> Option<OsString>,
{
    let mut captures = CaptureConfig::for_working_directory(working_directory);
    let mut theme_profile = None;
    let mut ui_snapshot_scene = None;
    let mut ui_capture_steps = Vec::new();
    let mut explicit_theme_name = false;
    apply_environment(
        &mut captures,
        &mut theme_profile,
        &mut ui_snapshot_scene,
        &mut explicit_theme_name,
        working_directory,
        environment,
    )?;

    let arguments: Vec<OsString> = arguments.into_iter().map(Into::into).collect();
    let mut initial_path = None;
    let mut index = 0;
    let mut parse_options = true;
    while index < arguments.len() {
        let argument = &arguments[index];
        let text = argument.to_string_lossy();
        if parse_options && text == "--" {
            parse_options = false;
            index += 1;
            continue;
        }
        if parse_options && text == "--ui-screenshots" {
            captures.enabled = true;
        } else if parse_options && text == "--no-ui-screenshots" {
            captures.enabled = false;
        } else if parse_options && text.starts_with("--ui-screenshot=") {
            captures.enabled = true;
            captures.startup_captures.push(CaptureSpec::parse(
                text.trim_start_matches("--ui-screenshot="),
            )?);
        } else if parse_options && text == "--ui-screenshot" {
            captures.enabled = true;
            let value = next_utf8_argument(&arguments, &mut index, "--ui-screenshot")?;
            captures.startup_captures.push(CaptureSpec::parse(value)?);
        } else if parse_options && text == "--ui-screenshot-latest" {
            select_latest_output(&mut captures, working_directory);
        } else if parse_options && text == "--ui-screenshot-exit" {
            captures.close_after_captures = true;
        } else if parse_options && text == "--no-ui-screenshot-exit" {
            captures.close_after_captures = false;
        } else if parse_options && text.starts_with("--ui-screenshot-step=") {
            captures.enabled = true;
            ui_capture_steps.push(UiCaptureStep::parse(
                text.trim_start_matches("--ui-screenshot-step="),
            )?);
        } else if parse_options && text == "--ui-screenshot-step" {
            captures.enabled = true;
            let value = next_utf8_argument(&arguments, &mut index, "--ui-screenshot-step")?;
            ui_capture_steps.push(UiCaptureStep::parse(value)?);
        } else if parse_options && text.starts_with("--ui-snapshot-scene=") {
            ui_snapshot_scene = Some(UiSnapshotScene::parse(
                text.trim_start_matches("--ui-snapshot-scene="),
            )?);
        } else if parse_options && text == "--ui-snapshot-scene" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-snapshot-scene")?;
            ui_snapshot_scene = Some(UiSnapshotScene::parse(value)?);
        } else if parse_options && text.starts_with("--ui-screenshot-subdir=") {
            let value = text.trim_start_matches("--ui-screenshot-subdir=");
            captures.output_directory =
                private_output_directory(working_directory, Path::new(value))?;
            captures.latest_filenames = false;
        } else if parse_options && text == "--ui-screenshot-subdir" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-screenshot-subdir")?;
            captures.output_directory =
                private_output_directory(working_directory, Path::new(value))?;
            captures.latest_filenames = false;
        } else if parse_options && text.starts_with("--ui-screenshot-settle=") {
            captures.settle_frames =
                parse_settle_frames(text.trim_start_matches("--ui-screenshot-settle="))?;
        } else if parse_options && text == "--ui-screenshot-settle" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-screenshot-settle")?;
            captures.settle_frames = parse_settle_frames(value)?;
        } else if parse_options && text.starts_with("--ui-screenshot-shortcut=") {
            captures.shortcut =
                parse_shortcut(text.trim_start_matches("--ui-screenshot-shortcut="))?;
        } else if parse_options && text == "--ui-screenshot-shortcut" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-screenshot-shortcut")?;
            captures.shortcut = parse_shortcut(value)?;
        } else if parse_options && text.starts_with("--ui-theme=") {
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .name = parse_theme_name(text.trim_start_matches("--ui-theme="))?;
            explicit_theme_name = true;
        } else if parse_options && text == "--ui-theme" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-theme")?;
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .name = parse_theme_name(value)?;
            explicit_theme_name = true;
        } else if parse_options && text == "--ui-theme-invert" {
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .invert = true;
        } else if parse_options && text == "--no-ui-theme-invert" {
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .invert = false;
        } else if parse_options && text.starts_with("--ui-theme-hue-shift=") {
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .hue_shift_degrees =
                parse_hue_shift(text.trim_start_matches("--ui-theme-hue-shift="))?;
        } else if parse_options && text == "--ui-theme-hue-shift" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-theme-hue-shift")?;
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .hue_shift_degrees = parse_hue_shift(value)?;
        } else if parse_options
            && argument
                .to_str()
                .is_some_and(|argument| argument.starts_with('-'))
        {
            return Err(format!(
                "unknown option {argument:?}; use -- before a dash-prefixed path"
            ));
        } else if initial_path.is_none() {
            initial_path = Some(PathBuf::from(argument));
        } else {
            return Err(format!("unexpected extra launch path {argument:?}"));
        }
        index += 1;
    }

    if !ui_capture_steps.is_empty() {
        if theme_profile.is_some()
            || ui_snapshot_scene.is_some()
            || !captures.startup_captures.is_empty()
        {
            return Err(
                "--ui-screenshot-step cannot be combined with --ui-theme, --ui-snapshot-scene, or --ui-screenshot"
                    .to_owned(),
            );
        }
        let first = &ui_capture_steps[0];
        theme_profile = Some(first.theme.clone());
        ui_snapshot_scene = Some(first.scene);
    }

    if captures.latest_filenames && !explicit_theme_name && ui_capture_steps.is_empty() {
        return Err(
            "--ui-screenshot-latest needs an explicit theme or screenshot step so its files cannot be mislabeled"
                .to_owned(),
        );
    }
    if !captures.enabled
        && (ui_snapshot_scene.is_some()
            || !ui_capture_steps.is_empty()
            || !captures.startup_captures.is_empty())
    {
        return Err(
            "--no-ui-screenshots cannot follow a snapshot scene or requested capture".to_owned(),
        );
    }
    captures.set_launch_naming(theme_profile.clone().unwrap_or_default(), ui_snapshot_scene);
    let mode = if ui_snapshot_scene.is_some()
        || !ui_capture_steps.is_empty()
        || !captures.startup_captures.is_empty()
    {
        LaunchMode::DeterministicCapture
    } else {
        LaunchMode::Interactive
    };

    Ok(LaunchOptions {
        mode,
        persistence_path: None,
        initial_path,
        captures,
        theme_profile,
        ui_snapshot_scene,
        ui_capture_steps,
    })
}

fn apply_environment<F>(
    captures: &mut CaptureConfig,
    theme_profile: &mut Option<CaptureThemeProfile>,
    ui_snapshot_scene: &mut Option<UiSnapshotScene>,
    explicit_theme_name: &mut bool,
    working_directory: &Path,
    environment: F,
) -> Result<(), String>
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOTS_ENABLED") {
        captures.enabled = parse_bool(&value.to_string_lossy())?;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOT_SUBDIR") {
        captures.output_directory = private_output_directory(working_directory, Path::new(&value))?;
        captures.latest_filenames = false;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOT_SETTLE_FRAMES") {
        captures.settle_frames = parse_settle_frames(&value.to_string_lossy())?;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOT_SHORTCUT") {
        captures.shortcut = parse_shortcut(&value.to_string_lossy())?;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOTS") {
        let value = value.to_string_lossy();
        for spec in value.split(',').filter(|spec| !spec.trim().is_empty()) {
            captures.startup_captures.push(CaptureSpec::parse(spec)?);
        }
        if !captures.startup_captures.is_empty() {
            captures.enabled = true;
        }
    }
    let latest = environment("TIPTOPTYP_UI_SCREENSHOT_LATEST")
        .map(|value| parse_bool(&value.to_string_lossy()))
        .transpose()?
        .unwrap_or(false);
    if latest {
        select_latest_output(captures, working_directory);
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOT_EXIT") {
        captures.close_after_captures = parse_bool(&value.to_string_lossy())?;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SNAPSHOT_SCENE") {
        *ui_snapshot_scene = Some(UiSnapshotScene::parse(&value.to_string_lossy())?);
    }
    if let Some(value) = environment("TIPTOPTYP_UI_THEME") {
        theme_profile
            .get_or_insert_with(CaptureThemeProfile::default)
            .name = parse_theme_name(&value.to_string_lossy())?;
        *explicit_theme_name = true;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_THEME_INVERT") {
        theme_profile
            .get_or_insert_with(CaptureThemeProfile::default)
            .invert = parse_bool(&value.to_string_lossy())?;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_THEME_HUE_SHIFT") {
        theme_profile
            .get_or_insert_with(CaptureThemeProfile::default)
            .hue_shift_degrees = parse_hue_shift(&value.to_string_lossy())?;
    }
    Ok(())
}

fn select_latest_output(captures: &mut CaptureConfig, working_directory: &Path) {
    captures.output_directory = working_directory.join(LATEST_OUTPUT_SUBDIRECTORY);
    captures.latest_filenames = true;
    captures.enabled = true;
}

fn next_utf8_argument<'a>(
    arguments: &'a [OsString],
    index: &mut usize,
    option: &str,
) -> Result<&'a str, String> {
    *index += 1;
    arguments
        .get(*index)
        .ok_or_else(|| format!("{option} needs a value"))?
        .to_str()
        .ok_or_else(|| format!("{option} needs a UTF-8 value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_capture_and_profile_launches_keep_their_storage_policy() {
        for capture in [false, true] {
            let args = if capture {
                vec!["--ui-snapshot-scene=main", "test.typ"]
            } else {
                vec!["test.typ"]
            };
            let launch = parse_launch_options(args, Path::new("/workspace"), |_| None).unwrap();
            assert_eq!(launch.initial_path, Some(PathBuf::from("test.typ")));
            assert_eq!(launch.mode.persists_settings(), !capture);
            assert_eq!(launch.persistence_path, None);
            assert_eq!(launch.clone().with_profile_storage(None), launch);

            let storage = PathBuf::from("/isolated-profile/app-state/egui.ron");
            let profile = launch.with_profile_storage(Some(storage.clone()));
            assert_eq!(
                profile.mode,
                LaunchMode::Profiling {
                    deterministic_capture: capture
                }
            );
            assert_eq!(profile.persistence_path, Some(storage));
            // Non-QA profiles may write their isolated store. QA never persists fixtures.
            assert_eq!(profile.mode.persists_settings(), !capture);
        }
    }
}
