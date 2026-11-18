//! User-initiated access to the Mac's saved passwords.
//!
//! Opening Apple's UI lets the user unlock and copy a saved login. This is not
//! Safari cookie sharing or a credential API: no password or clipboard content
//! is read, and no login-page script is injected.

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Destination {
    Passwords,
    Settings,
}

#[cfg(any(target_os = "macos", test))]
impl Destination {
    fn arguments(self) -> &'static [&'static str] {
        match self {
            Self::Passwords => &["-b", "com.apple.Passwords"],
            Self::Settings => &["x-apple.systempreferences:com.apple.Passwords-Settings.extension"],
        }
    }
}

/// Prefer the standalone Passwords app (macOS 15+), with the Passwords settings
/// pane as the fallback on earlier systems. Launch failures remain visible.
#[cfg(any(target_os = "macos", test))]
fn open_with(mut launch: impl FnMut(Destination) -> Result<(), String>) -> Result<(), String> {
    if launch(Destination::Passwords).is_ok() {
        return Ok(());
    }
    launch(Destination::Settings).map_err(|error| {
        format!(
            "Could not open saved passwords: {error}. Open Passwords, or System Settings > Passwords, to find your saved login."
        )
    })
}

/// Run on a launcher worker, never during a UI frame. Each OS launch is bounded
/// and reaped; opening the app does not wait for its unlock prompt to finish.
#[cfg(target_os = "macos")]
pub(crate) fn open_passwords() -> Result<(), String> {
    use std::{process::Command, process::Stdio, time::Duration};

    open_with(|destination| {
        let child = Command::new("/usr/bin/open")
            .args(destination.arguments())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("The password launcher could not start ({error})"))?;
        let status = crate::process::wait(child, Duration::from_secs(15), || Ok(()))
            .map_err(|error| format!("The password launcher failed ({error})"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("The password launcher exited with {status}"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_passwords_app_does_not_also_open_settings() {
        let mut attempts = Vec::new();
        assert!(
            open_with(|destination| {
                attempts.push(destination);
                Ok(())
            })
            .is_ok()
        );
        assert_eq!(attempts, [Destination::Passwords]);
    }

    #[test]
    fn systems_without_passwords_app_open_the_passwords_settings_pane() {
        let mut attempts = Vec::new();
        assert!(
            open_with(|destination| {
                attempts.push(destination);
                match destination {
                    Destination::Passwords => Err("Application unavailable".into()),
                    Destination::Settings => Ok(()),
                }
            })
            .is_ok()
        );
        assert_eq!(attempts, [Destination::Passwords, Destination::Settings]);
        assert_eq!(
            Destination::Passwords.arguments(),
            ["-b", "com.apple.Passwords"]
        );
        assert_eq!(
            Destination::Settings.arguments(),
            ["x-apple.systempreferences:com.apple.Passwords-Settings.extension"]
        );
    }

    #[test]
    fn unsuccessful_fallback_returns_recovery_guidance_without_retrying() {
        let mut attempts = Vec::new();
        let error = open_with(|destination| {
            attempts.push(destination);
            Err("Launch unavailable".into())
        })
        .unwrap_err();
        assert_eq!(attempts, [Destination::Passwords, Destination::Settings]);
        assert!(error.contains("Launch unavailable"));
        assert!(error.contains("System Settings > Passwords"));
    }
}
