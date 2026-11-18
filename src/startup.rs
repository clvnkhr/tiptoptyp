//! Read-only configuration admission, before windows, workers or persistence.
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
};

use crate::settings::{AppSettings, SettingsLoadError};

const MAX_PERSISTENCE_BYTES: u64 = 16 * 1024 * 1024;

// The macOS-owned event loop does not return eframe's app-creator error.
// Retain it so a failed admission still produces a failing process exit.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Default)]
pub(crate) struct AppCreationFailure(std::rc::Rc<std::cell::RefCell<Option<String>>>);

#[cfg(any(target_os = "macos", test))]
impl AppCreationFailure {
    pub(crate) fn record<T, E: std::fmt::Display>(&self, result: Result<T, E>) -> Result<T, E> {
        if let Err(error) = &result {
            *self.0.borrow_mut() = Some(error.to_string());
        }
        result
    }

    pub(crate) fn take(&self) -> Option<String> {
        self.0.borrow_mut().take()
    }
}

struct ReadOnlyStorage(BTreeMap<String, String>);

impl eframe::Storage for ReadOnlyStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, _: &str, _: String) {
        panic!("startup validation cannot write preferences");
    }
    fn remove_string(&mut self, _: &str) {
        panic!("startup validation cannot remove preferences");
    }
    fn flush(&mut self) {
        panic!("startup validation cannot flush preferences");
    }
}

pub(crate) fn persistence_path(explicit: Option<&Path>, app_id: &str) -> Option<PathBuf> {
    explicit
        .map(Path::to_owned)
        .or_else(|| eframe::storage_dir(app_id).map(|root| root.join("app.ron")))
}

pub(crate) fn check_settings(explicit: Option<&Path>, app_id: &str) -> Result<(), String> {
    // A saved override must not conceal a broken new built-in default.
    AppSettings::load(None).map_err(|error| error.to_string())?;
    if let Some(path) = persistence_path(explicit, app_id) {
        check_settings_file(&path)
    } else {
        Ok(())
    }
}

fn check_settings_file(path: &Path) -> Result<(), String> {
    let fail = |error: String| {
        format!(
            "Cannot start with preferences {}: {error}. The file is unchanged",
            path.display()
        )
    };
    let mut options = OpenOptions::new();
    options.read(true);
    // Inspect the opened object without blocking on a FIFO, including a
    // symlink to one. Checking path metadata before opening would race.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(fail(error.to_string())),
    };
    let metadata = file.metadata().map_err(|error| fail(error.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_PERSISTENCE_BYTES {
        return Err(fail(
            "expected a regular preferences file smaller than 16 MiB".into(),
        ));
    }
    let mut text = String::new();
    file.take(MAX_PERSISTENCE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| fail(error.to_string()))?;
    if text.len() as u64 > MAX_PERSISTENCE_BYTES {
        return Err(fail("preferences file grew beyond 16 MiB".into()));
    }
    let values = ron::from_str::<BTreeMap<String, String>>(&text)
        .map_err(|error| fail(format!("invalid preferences envelope: {error}")))?;
    AppSettings::load(Some(&ReadOnlyStorage(values)))
        .map(|_| ())
        .map_err(|error| fail(error.to_string()))
}

/// An invalid profile must never reach a constructor that starts sidecars.
pub(crate) fn construct<T>(
    storage: Option<&dyn eframe::Storage>,
    create: impl FnOnce(AppSettings) -> T,
) -> Result<T, SettingsLoadError> {
    let settings = AppSettings::load(storage)?;
    Ok(create(settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::Storage;
    use std::fs::File;

    #[test]
    fn event_loop_success_cannot_hide_app_creation_failure() {
        let failure = AppCreationFailure::default();
        let creator_failure = failure.clone();
        let error = creator_failure.record::<(), _>(Err("invalid settings after preflight"));
        assert!(error.is_err());
        assert_eq!(
            failure.take().as_deref(),
            Some("invalid settings after preflight")
        );
        assert!(failure.take().is_none());
        failure.record::<_, &str>(Ok(())).unwrap();
        assert!(failure.take().is_none());
    }

    fn old_configuration() -> String {
        let mut settings = serde_json::to_value(AppSettings::default()).unwrap();
        settings["tinymist_configuration"]["formatterProseWrap"] = false.into();
        serde_json::to_string(&settings).unwrap()
    }

    #[test]
    fn incompatible_saved_settings_never_reach_the_runtime_constructor() {
        let value = old_configuration();
        let storage = ReadOnlyStorage(BTreeMap::from([(
            "tiptoptyp.settings.v1".into(),
            value.clone(),
        )]));
        let called = std::cell::Cell::new(false);
        let error = construct(Some(&storage), |_| called.set(true)).unwrap_err();
        assert!(!called.get());
        assert!(error.to_string().contains("formatterProseWrap"));
        assert_eq!(storage.get_string("tiptoptyp.settings.v1"), Some(value));
    }

    #[test]
    fn persistence_preflight_rejects_incompatible_and_malformed_profiles_without_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app.ron");
        let values = BTreeMap::from([("tiptoptyp.settings.v1".to_owned(), old_configuration())]);
        let invalid = ron::to_string(&values).unwrap();
        for (text, diagnostic) in [
            (&invalid[..], "formatterProseWrap"),
            ("{broken", "preferences envelope"),
        ] {
            std::fs::write(&path, text).unwrap();
            let error = check_settings(Some(&path), "unused").unwrap_err();
            assert!(error.contains(diagnostic), "{error}");
            assert!(error.contains(path.to_str().unwrap()));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
    }

    #[test]
    fn current_and_first_run_profiles_are_read_only() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app.ron");
        check_settings(Some(&path), "unused").unwrap();
        assert!(!path.exists());
        let values = BTreeMap::from([
            (
                "tiptoptyp.settings.v1".to_owned(),
                serde_json::to_string(&AppSettings::default()).unwrap(),
            ),
            ("egui".into(), "framework state".into()),
        ]);
        let text = ron::to_string(&values).unwrap();
        std::fs::write(&path, &text).unwrap();
        check_settings(Some(&path), "unused").unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), text);
    }

    #[test]
    fn preflight_rejects_directories_and_oversized_envelopes_before_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app.ron");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_PERSISTENCE_BYTES + 1).unwrap();
        let error = check_settings(Some(&path), "unused").unwrap_err();
        assert!(error.contains("16 MiB"), "{error}");
        assert_eq!(file.metadata().unwrap().len(), MAX_PERSISTENCE_BYTES + 1);
        let error = check_settings(Some(directory.path()), "unused").unwrap_err();
        assert!(
            error.contains(directory.path().to_str().unwrap()),
            "{error}"
        );
        assert!(path.is_file());
    }

    #[test]
    #[cfg(unix)]
    fn preflight_rejects_named_pipes_and_links_without_waiting_for_a_writer() {
        use std::os::unix::fs::{FileTypeExt, symlink};
        let directory = tempfile::tempdir().unwrap();
        let pipe = directory.path().join("pipe.ron");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&pipe)
                .status()
                .unwrap()
                .success()
        );
        let link = directory.path().join("app.ron");
        symlink(&pipe, &link).unwrap();
        for path in [&pipe, &link] {
            let error = check_settings(Some(path), "unused").unwrap_err();
            assert!(error.contains("regular preferences file"), "{error}");
        }
        assert!(std::fs::metadata(pipe).unwrap().file_type().is_fifo());
        assert!(link.is_symlink());
    }
}
