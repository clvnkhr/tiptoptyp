//! Canonical application-icon loading and platform installation.
//!
//! The packaged applications and the live process both use derivatives of
//! `assets/icons/tiptoptyp.svg`. Keeping the runtime icon embedded in the
//! binary is important for development/release executables, which do not have
//! a bundle resource for the operating system to discover.

use std::sync::{Arc, OnceLock};

use eframe::egui;

const RUNTIME_ICON_PNG: &[u8] = include_bytes!("../assets/icons/tiptoptyp-256.png");
#[cfg(target_os = "macos")]
const MACOS_APPLICATION_ICON_PNG: &[u8] = include_bytes!("../assets/icons/tiptoptyp-512@2x.png");

pub(crate) fn runtime_icon() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| {
        let image = image::load_from_memory_with_format(RUNTIME_ICON_PNG, image::ImageFormat::Png)
            .expect("the embedded application icon must be a valid PNG")
            .into_rgba8();
        let (width, height) = image.dimensions();
        Arc::new(egui::IconData {
            rgba: image.into_raw(),
            width,
            height,
        })
    })
    .clone()
}

/// Install the application-level Dock icon for an unbundled macOS process.
///
/// Winit intentionally ignores window icons on macOS because AppKit has one
/// icon per application rather than per window. A packaged build gets the
/// same artwork from `tiptoptyp.icns`; a directly launched release executable
/// needs this explicit application-level installation instead.
#[cfg(target_os = "macos")]
pub(crate) fn install_macos_application_icon() -> Result<(), String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let marker = MainThreadMarker::new().ok_or_else(|| {
        "the macOS application icon must be installed on the main thread".to_owned()
    })?;
    let data = NSData::with_bytes(MACOS_APPLICATION_ICON_PNG);
    let image = NSImage::initWithData(marker.alloc(), &data)
        .ok_or_else(|| "AppKit could not decode the embedded application icon".to_owned())?;
    let application = NSApplication::sharedApplication(marker);

    // SAFETY: objc2 marks this setter unsafe only because AppKit may reject a
    // null image. `image` is a live, successfully decoded NSImage and is passed
    // as `Some`; AppKit retains the application icon after the call.
    unsafe { application.setApplicationIconImage(Some(&image)) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_is_square_rgba_at_runtime_size() {
        let icon = runtime_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_application_icon_keeps_a_retina_source() {
        let image = image::load_from_memory_with_format(
            MACOS_APPLICATION_ICON_PNG,
            image::ImageFormat::Png,
        )
        .expect("embedded macOS icon");
        assert_eq!((image.width(), image.height()), (1024, 1024));
    }

    #[test]
    fn embedded_icon_contains_the_neutral_and_accent_t_marks() {
        let icon = runtime_icon();
        let mut neutral = 0usize;
        let mut accent = 0usize;
        for pixel in icon.rgba.chunks_exact(4) {
            if pixel[3] > 240 && pixel[0] > 230 && pixel[1] > 225 && pixel[2] > 215 {
                neutral += 1;
            }
            if pixel[3] > 240 && pixel[2] > 220 && pixel[0] < 110 && pixel[1] > 110 {
                accent += 1;
            }
        }

        // These deliberately broad bounds catch a missing/replaced wordmark
        // without tying the test to antialiasing at individual edge pixels.
        assert!(neutral > 6_000, "neutral t area was only {neutral} pixels");
        assert!(accent > 3_000, "accent t area was only {accent} pixels");
        assert!(
            neutral > accent * 3 / 2,
            "the two neutral t marks are missing"
        );
    }
}
