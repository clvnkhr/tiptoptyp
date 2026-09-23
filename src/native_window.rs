//! Retained native window handles. Prefer the host-supplied handle; keyboard
//! focus is only a bootstrap fallback for deferred windows whose eframe
//! callback does not expose its native handle.

#[cfg(target_os = "macos")]
mod platform {
    use std::{ffi::c_void, ptr::NonNull};

    use objc2::{MainThreadMarker, rc::Retained};
    use objc2_app_kit::{NSApplication, NSView, NSWindow};
    use raw_window_handle::{
        AppKitWindowHandle, DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle,
        RawWindowHandle, WindowHandle,
    };

    /// A retained AppKit content view keeps the borrowed raw handle valid for
    /// the complete Wry/rfd construction call.
    #[derive(Clone)]
    pub(crate) struct ActiveWindowHandle {
        view: Retained<NSView>,
    }

    impl ActiveWindowHandle {
        pub(crate) fn from_owner(owner: &impl HasWindowHandle) -> Option<Self> {
            let RawWindowHandle::AppKit(handle) = owner.window_handle().ok()?.as_raw() else {
                return None;
            };
            let _marker = MainThreadMarker::new()?;
            // SAFETY: HasWindowHandle guarantees a live NSView for this borrow.
            // Retain it on AppKit's main thread before the borrow ends.
            let view = unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }?;
            Some(Self { view })
        }

        #[cfg(test)]
        #[allow(dead_code)] // Also compiled by the opt-in native integration harness.
        pub(crate) fn from_test_view(view: Retained<NSView>) -> Self {
            Self { view }
        }
        pub(crate) fn is_same_window(&self, other: &Self) -> bool {
            std::ptr::eq(&*self.view, &*other.view)
        }

        /// AppKit's file-drag events do not supply winit CursorMoved events.
        /// Query this retained document view, including while Finder has focus.
        pub(crate) fn file_drag_pointer(&self, zoom: f32) -> Option<eframe::egui::Pos2> {
            let window = self.view.window()?;
            let point = self
                .view
                .convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
            let bounds = self.view.bounds();
            let x = point.x - bounds.origin.x;
            let y = if self.view.isFlipped() {
                point.y - bounds.origin.y
            } else {
                bounds.origin.y + bounds.size.height - point.y
            };
            Some(eframe::egui::pos2(x as f32 / zoom, y as f32 / zoom))
        }

        pub(crate) fn suppress_titlebar_drag(&self) -> Option<TitlebarDragGuard> {
            let window = self.view.window()?;
            let was_movable = window.isMovable();
            // Full-size content does not remove AppKit's title-bar dragging.
            // egui gesture ownership alone cannot prevent that native path.
            window.setMovable(false);
            Some(TitlebarDragGuard {
                parent: self.clone(),
                window,
                was_movable,
            })
        }
    }

    /// Retains the exact document window and restores its previous policy on
    /// pointer exit, focus loss, viewport replacement, or session destruction.
    pub(crate) struct TitlebarDragGuard {
        parent: ActiveWindowHandle,
        window: Retained<NSWindow>,
        was_movable: bool,
    }

    impl TitlebarDragGuard {
        pub(crate) fn matches_parent(&self, parent: &ActiveWindowHandle) -> bool {
            self.parent.is_same_window(parent)
        }
    }

    impl Drop for TitlebarDragGuard {
        fn drop(&mut self) {
            self.window.setMovable(self.was_movable);
        }
    }

    impl HasWindowHandle for ActiveWindowHandle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let view = NonNull::from(&*self.view).cast::<c_void>();
            let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(view));
            // SAFETY: `self.view` retains the NSView for at least the lifetime
            // of the returned handle, and this type is constructed only on the
            // AppKit main thread.
            unsafe { Ok(WindowHandle::borrow_raw(raw)) }
        }
    }

    impl HasDisplayHandle for ActiveWindowHandle {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::appkit())
        }
    }

    pub(crate) fn active_window_handle() -> Option<ActiveWindowHandle> {
        let marker = MainThreadMarker::new()?;
        let application = NSApplication::sharedApplication(marker);
        if !application.isActive() {
            return None;
        }
        let window = application.keyWindow()?;
        let view = window.contentView()?;
        Some(ActiveWindowHandle { view })
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{ffi::c_void, num::NonZeroIsize};

    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
        Win32WindowHandle, WindowHandle,
    };

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetActiveWindow() -> *mut c_void;
    }

    #[derive(Clone, Copy)]
    pub(crate) struct ActiveWindowHandle {
        hwnd: NonZeroIsize,
    }

    impl ActiveWindowHandle {
        pub(crate) fn from_owner(owner: &impl HasWindowHandle) -> Option<Self> {
            let RawWindowHandle::Win32(handle) = owner.window_handle().ok()?.as_raw() else {
                return None;
            };
            Some(Self { hwnd: handle.hwnd })
        }

        pub(crate) fn is_same_window(&self, other: &Self) -> bool {
            self.hwnd == other.hwnd
        }
    }

    impl HasWindowHandle for ActiveWindowHandle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.hwnd));
            // SAFETY: the foreground HWND remains owned by the running winit
            // event loop for the duration of this borrowed handle.
            unsafe { Ok(WindowHandle::borrow_raw(raw)) }
        }
    }

    impl HasDisplayHandle for ActiveWindowHandle {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::windows())
        }
    }

    pub(crate) fn active_window_handle() -> Option<ActiveWindowHandle> {
        // SAFETY: this is a read-only Win32 query. A null pointer means the
        // current event-loop thread has no active window and is handled
        // without constructing a raw handle.
        let hwnd = unsafe { GetActiveWindow() } as isize;
        NonZeroIsize::new(hwnd).map(|hwnd| ActiveWindowHandle { hwnd })
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) use platform::{ActiveWindowHandle, active_window_handle};

#[cfg(target_os = "macos")]
pub(crate) use platform::TitlebarDragGuard;

/// Enable WKWebView's native trackpad magnification gesture.
///
/// Wry exposes this AppKit property as unsafe because the caller must uphold
/// WKWebView's main-thread requirement. tiptoptyp creates and configures every
/// webview from eframe's UI callback, so the requirement is established once
/// here instead of leaking an unsafe block into application state code.
#[cfg(target_os = "macos")]
pub(crate) fn enable_native_webview_magnification(webview: &wry::WebView) {
    use wry::WebViewExtMacOS as _;

    // SAFETY: this helper is called synchronously from eframe's main-thread UI
    // callback immediately after Wry constructs the WKWebView on that thread.
    unsafe { webview.webview().setAllowsMagnification(true) };
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[derive(Clone, Copy)]
pub(crate) struct ActiveWindowHandle;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl ActiveWindowHandle {
    pub(crate) fn is_same_window(&self, _other: &Self) -> bool {
        true
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn active_window_handle() -> Option<ActiveWindowHandle> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn platform_handle_api_is_non_panicking_without_a_window() {
        // Merely exercising the symbol in unit-test mode catches target-gated
        // import and raw-window-handle drift. No application window is created.
        let _ = super::active_window_handle();
    }
}
