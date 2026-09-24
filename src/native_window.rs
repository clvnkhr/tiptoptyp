//! Native ownership comes from the renderer's viewport binding, never focus.

#[cfg(target_os = "macos")]
mod platform {
    use std::{ffi::c_void, ptr::NonNull};

    use objc2::{MainThreadMarker, rc::Retained};
    use objc2_app_kit::{NSView, NSWindow};
    use raw_window_handle::{
        AppKitWindowHandle, DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle,
        RawWindowHandle, WindowHandle,
    };

    /// A retained AppKit content view keeps the borrowed raw handle valid for
    /// the complete Wry/rfd construction call.
    #[derive(Clone)]
    pub(crate) struct NativeWindowHandle {
        view: Retained<NSView>,
    }

    impl NativeWindowHandle {
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
        parent: NativeWindowHandle,
        window: Retained<NSWindow>,
        was_movable: bool,
    }

    impl TitlebarDragGuard {
        pub(crate) fn matches_parent(&self, parent: &NativeWindowHandle) -> bool {
            self.parent.is_same_window(parent)
        }
    }

    impl Drop for TitlebarDragGuard {
        fn drop(&mut self) {
            self.window.setMovable(self.was_movable);
        }
    }

    impl HasWindowHandle for NativeWindowHandle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let view = NonNull::from(&*self.view).cast::<c_void>();
            let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(view));
            // SAFETY: `self.view` retains the NSView for at least the lifetime
            // of the returned handle, and this type is constructed only on the
            // AppKit main thread.
            unsafe { Ok(WindowHandle::borrow_raw(raw)) }
        }
    }

    impl HasDisplayHandle for NativeWindowHandle {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::appkit())
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::num::NonZeroIsize;

    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
        Win32WindowHandle, WindowHandle,
    };

    #[derive(Clone, Copy)]
    pub(crate) struct NativeWindowHandle {
        hwnd: NonZeroIsize,
    }

    impl NativeWindowHandle {
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

    impl HasWindowHandle for NativeWindowHandle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.hwnd));
            // SAFETY: the foreground HWND remains owned by the running winit
            // event loop for the duration of this borrowed handle.
            unsafe { Ok(WindowHandle::borrow_raw(raw)) }
        }
    }

    impl HasDisplayHandle for NativeWindowHandle {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::windows())
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) use platform::NativeWindowHandle;

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
pub(crate) struct NativeWindowHandle;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl NativeWindowHandle {
    pub(crate) fn is_same_window(&self, _other: &Self) -> bool {
        true
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn for_viewport(
    context: &eframe::egui::Context,
    id: eframe::egui::ViewportId,
) -> Option<NativeWindowHandle> {
    let window = eframe::window_host::window(context, id)?;
    NativeWindowHandle::from_owner(window.as_ref())
}

/// Fresh platform activation beats delayed egui focus observations. Headless
/// tests have no native binding and deliberately use their injected input.
pub(crate) fn application_active(context: &eframe::egui::Context) -> Option<bool> {
    eframe::window_host::window(context, context.viewport_id())?;
    #[cfg(target_os = "macos")]
    {
        let marker = objc2::MainThreadMarker::new()?;
        Some(objc2_app_kit::NSApplication::sharedApplication(marker).isActive())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let ids = context.input(|input| input.raw.viewports.keys().copied().collect::<Vec<_>>());
        Some(ids.into_iter().any(|id| {
            eframe::window_host::window(context, id).is_some_and(|window| window.has_focus())
        }))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn absent_viewport_never_falls_back_to_another_window() {
        let context = eframe::egui::Context::default();
        assert!(super::for_viewport(&context, eframe::egui::ViewportId::ROOT).is_none());
    }
}
