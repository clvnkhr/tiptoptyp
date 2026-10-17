//! Native ownership comes from the renderer's viewport binding, never focus.

#[path = "window_fade.rs"]
mod fade;
pub(crate) use fade::FadeDirection;

#[cfg(target_os = "macos")]
#[path = "native_blur_gradient.rs"]
mod gradient;

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BlurConfig {
    pub(crate) radius: u8,
    pub(crate) end_radius: u8,
    pub(crate) fade: FadeDirection,
}

#[cfg(target_os = "macos")]
impl BlurConfig {
    fn maximum_radius(self) -> u8 {
        if self.fade == FadeDirection::None {
            self.radius
        } else {
            self.radius.max(self.end_radius)
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::{
        cell::{Cell, RefCell},
        ffi::{c_char, c_int, c_void},
        ptr::NonNull,
        sync::{Arc, OnceLock},
    };

    use super::{BlurConfig, FadeDirection, gradient::GradientBlur};
    use dispatch2::MainThreadBound;
    #[cfg(feature = "desktop-ui-tests")]
    use objc2::msg_send;
    use objc2::{
        MainThreadMarker,
        rc::{Retained, Weak},
    };
    use objc2_app_kit::{NSColor, NSView, NSWindow};

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

        #[cfg(any(test, feature = "native-ui-tests", feature = "desktop-ui-tests"))]
        #[allow(dead_code)]
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

        /// Read the actual rectangle AppKit asks winit's text-input client for.
        /// This observes the native hint after backend forwarding; it does not
        /// reconstruct that hint from egui's cursor or cached platform output.
        #[cfg(feature = "desktop-ui-tests")]
        pub(crate) fn ime_cursor_area(&self) -> Option<([f64; 4], bool)> {
            use objc2::sel;
            use objc2_foundation::{NSRange, NSRect};
            let _marker = MainThreadMarker::new()?;
            let window = self.view.window()?;
            // SAFETY: the retained raw-window view is alive on AppKit's main
            // thread. Only ask for NSTextInputClient geometry if implemented.
            let supported: bool = unsafe {
                msg_send![&*self.view, respondsToSelector: sel!(firstRectForCharacterRange:actualRange:)]
            };
            if !supported {
                return None;
            }
            // SAFETY: selector support is established above. winit implements
            // this read-only query and accepts a null actualRange pointer.
            let screen: NSRect = unsafe {
                msg_send![&*self.view,
                    firstRectForCharacterRange: NSRange::new(0, 0),
                    actualRange: std::ptr::null_mut::<NSRange>()]
            };
            let local = self
                .view
                .convertRect_fromView(window.convertRectFromScreen(screen), None);
            let rect = [
                local.origin.x,
                local.origin.y,
                local.size.width,
                local.size.height,
            ];
            if !rect.iter().all(|value| value.is_finite()) {
                return None;
            }
            let focused = window.firstResponder().is_some_and(|responder| {
                std::ptr::eq(
                    std::ptr::from_ref(&*responder).cast::<c_void>(),
                    (&*self.view as *const NSView).cast::<c_void>(),
                )
            });
            Some((rect, focused))
        }

        /// Retain alpha capability without changing renderer or input ownership.
        /// Compositor blur never changes the renderer's view/layer hierarchy.
        pub(crate) fn set_translucent_background(&self) -> bool {
            let Some(_marker) = MainThreadMarker::new() else {
                return false;
            };
            let Some(window) = self.view.window() else {
                return false;
            };
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            true
        }

        pub(crate) fn blur_background(&self, config: BlurConfig) -> Option<NativeBlur> {
            let marker = MainThreadMarker::new()?;
            let window = self.view.window()?;
            let api = WindowBlurApi::load()?;
            let effect = NativeBlur(Arc::new(MainThreadBound::new(
                WindowBlur {
                    window: Weak::new(&window),
                    api,
                    config: Cell::new(config),
                    gradient: RefCell::new(None),
                    gradient_attempt: Cell::new((0, false)),
                    applied: Cell::new(false),
                    applications: Cell::new(0),
                },
                marker,
            )));
            effect.synchronize(self, None).then_some(effect)
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

    // AppKit materials tint in addition to blurring, deactivate with child-window
    // focus, and can replace their backdrop with desktop tint on native zoom.
    // Use WindowServer's window-wide blur instead. It neither changes the CGL/
    // WebKit hierarchy nor adds an extra color/opacity layer. The private API is
    // resolved once (also used by Ghostty); unsupported systems keep transparency.
    type BlurConnection = unsafe extern "C" fn() -> *mut c_void;
    type SetWindowBlur = unsafe extern "C" fn(*mut c_void, usize, c_int) -> i32;

    struct WindowBlurApi {
        connection: BlurConnection,
        set_radius: SetWindowBlur,
    }

    impl WindowBlurApi {
        fn load() -> Option<&'static Self> {
            static API: OnceLock<Option<WindowBlurApi>> = OnceLock::new();
            API.get_or_init(|| {
                unsafe extern "C" {
                    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
                }
                // SAFETY: RTLD_DEFAULT is Darwin's documented -2 pseudo-handle.
                // Look up the known C ABI without creating a hard dependency on
                // private exports at launch. Both symbols must exist before use.
                unsafe {
                    let default = (-2_isize) as *mut c_void;
                    let connection = dlsym(default, c"CGSDefaultConnectionForThread".as_ptr());
                    let set_radius = dlsym(default, c"CGSSetWindowBackgroundBlurRadius".as_ptr());
                    if connection.is_null() || set_radius.is_null() {
                        return None;
                    }
                    Some(Self {
                        connection: std::mem::transmute::<*mut c_void, BlurConnection>(connection),
                        set_radius: std::mem::transmute::<*mut c_void, SetWindowBlur>(set_radius),
                    })
                }
            })
            .as_ref()
        }

        fn apply(&self, window: &NSWindow, radius: u8) -> bool {
            if MainThreadMarker::new().is_none() {
                return false;
            }
            let Ok(number) = usize::try_from(window.windowNumber()) else {
                return false;
            };
            if number == 0 {
                return false;
            }
            // SAFETY: called on AppKit's main thread for this process's live
            // NSWindow. The system compositor owns the effect and resize. A
            // nonzero status is a failure, never cached as an applied radius.
            unsafe {
                let connection = (self.connection)();
                !connection.is_null()
                    && (self.set_radius)(connection, number, c_int::from(radius)) == 0
            }
        }
    }

    #[derive(Clone)]
    pub(crate) struct NativeBlur(Arc<MainThreadBound<WindowBlur>>);

    struct WindowBlur {
        // A cached decoration must not extend the native window's lifetime.
        window: Weak<NSWindow>,
        api: &'static WindowBlurApi,
        config: Cell<BlurConfig>,
        gradient: RefCell<Option<GradientBlur>>,
        gradient_attempt: Cell<(usize, bool)>,
        applied: Cell<bool>,
        applications: Cell<u64>,
    }

    impl WindowBlur {
        fn apply(
            &self,
            parent: &NativeWindowHandle,
            window: &NSWindow,
            config: BlurConfig,
        ) -> bool {
            let directional = config.fade != FadeDirection::None && config.maximum_radius() > 0;
            let mut gradient = self.gradient.borrow_mut();
            if directional {
                self.gradient_attempt
                    .set(GradientBlur::attachment_key(&parent.view));
                let reused = gradient.as_ref().is_some_and(|effect| {
                    effect.synchronize(&parent.view, config.radius, config.end_radius, config.fade)
                });
                if !reused {
                    // A host replacement needs a fresh attachment even when
                    // the old controller still owns an obsolete layer.
                    gradient.take();
                    *gradient = GradientBlur::new(
                        &parent.view,
                        config.radius,
                        config.end_radius,
                        config.fade,
                    );
                }
                let configured = gradient.is_some();
                if configured {
                    // Uniform pre-blur would prevent the sharp edge from ever
                    // becoming sharp. The gradient alone owns native sampling.
                    if !self.api.apply(window, 0) {
                        gradient.take();
                        return false;
                    }
                } else {
                    // Optional private layer/filter support may be absent on a
                    // future macOS. Keep the established blur; report its actual
                    // uniform direction rather than pretending the mask applied.
                    gradient.take();
                    if !self.api.apply(window, config.maximum_radius()) {
                        return false;
                    }
                }
            } else {
                gradient.take();
                if !self.api.apply(window, config.maximum_radius()) {
                    return false;
                }
            }
            self.config.set(config);
            self.applied.set(true);
            self.applications.set(self.applications.get() + 1);
            true
        }
    }

    impl Drop for WindowBlur {
        fn drop(&mut self) {
            self.gradient.get_mut().take();
            if self.applications.get() > 0
                && let Some(window) = self.window.load()
            {
                self.api.apply(&window, 0);
            }
        }
    }

    impl NativeBlur {
        /// Reuse the exact native owner. Uniform blur needs no resize/focus
        /// calls; a directional layer only updates its bounded mask on resize.
        pub(crate) fn synchronize(
            &self,
            parent: &NativeWindowHandle,
            config: Option<BlurConfig>,
        ) -> bool {
            let Some(marker) = MainThreadMarker::new() else {
                return false;
            };
            let state = self.0.get(marker);
            let Some(window) = state.window.load() else {
                return false;
            };
            if parent.view.window().as_deref() != Some(&*window) {
                return false;
            }
            let config = config.unwrap_or(state.config.get());
            if !window.isVisible() {
                if config != state.config.get() {
                    state.config.set(config);
                    state.applied.set(false);
                }
                return true;
            }
            if !state.applied.get() || config != state.config.get() {
                return state.apply(parent, &window, config);
            }
            let geometry_valid = state.gradient.borrow().as_ref().is_none_or(|effect| {
                effect.synchronize(&parent.view, config.radius, config.end_radius, config.fade)
            });
            let recovered_host = config.fade != FadeDirection::None
                && config.maximum_radius() > 0
                && state.gradient.borrow().is_none()
                && GradientBlur::attachment_key(&parent.view) != state.gradient_attempt.get();
            if !geometry_valid || recovered_host {
                // Retry after a native host/size-eligibility transition, never
                // on unchanged idle callbacks or ordinary size changes.
                return state.apply(parent, &window, config);
            }
            true
        }

        pub(crate) fn applied_fade(&self) -> FadeDirection {
            let Some(marker) = MainThreadMarker::new() else {
                return FadeDirection::None;
            };
            let state = self.0.get(marker);
            if state.applied.get() && state.gradient.borrow().is_some() {
                state.config.get().fade
            } else {
                FadeDirection::None
            }
        }

        pub(crate) fn awaiting_visibility(&self) -> bool {
            MainThreadMarker::new().is_some_and(|marker| !self.0.get(marker).applied.get())
        }

        #[cfg(any(test, feature = "native-ui-tests", feature = "desktop-ui-tests"))]
        #[allow(dead_code)]
        pub(crate) fn gradient_inspection(
            &self,
            parent: &NativeWindowHandle,
        ) -> Option<(usize, f64, [usize; 2], [f64; 4])> {
            self.inspection(parent)?;
            let marker = MainThreadMarker::new()?;
            self.0.get(marker).gradient.borrow().as_ref()?.inspection()
        }

        /// Successful native configuration, not a composed-pixel readback.
        #[cfg(any(test, feature = "native-ui-tests", feature = "desktop-ui-tests"))]
        #[allow(dead_code)]
        pub(crate) fn inspection(&self, parent: &NativeWindowHandle) -> Option<(u8, usize, u64)> {
            let marker = MainThreadMarker::new()?;
            let state = self.0.get(marker);
            let window = state.window.load()?;
            if !state.applied.get() || parent.view.window().as_deref() != Some(&*window) {
                return None;
            }
            Some((
                state.config.get().maximum_radius(),
                Arc::as_ptr(&self.0) as usize,
                state.applications.get(),
            ))
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
pub(crate) use platform::{NativeBlur, TitlebarDragGuard};

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

/// Bring the application to the foreground before honoring an explicit user
/// focus request. A viewport focus command alone can leave an AppKit child
/// marked key while another application still owns the active NSApplication.
/// Keeping activation here makes every user initiated focus handoff obey the
/// same rule, including Settings reopening after an app switch.
pub(crate) fn activate_application() {
    #[cfg(target_os = "macos")]
    if let Some(marker) = objc2::MainThreadMarker::new() {
        // Explicit user actions need the stronger activation semantics here:
        // cooperative `activate` can leave a renderer/settings child as the
        // apparent key window while another app remains frontmost.
        #[allow(deprecated)]
        objc2_app_kit::NSApplication::sharedApplication(marker).activateIgnoringOtherApps(true);
    }
}

/// Native NSView coordinates are already scaled to macOS screen points;
/// egui's desktop origin still needs the interface zoom applied once.
#[cfg(any(test, feature = "desktop-ui-tests"))]
pub(crate) fn ime_screen_rect(origin: eframe::egui::Pos2, local: [f64; 4], zoom: f32) -> [f64; 4] {
    [
        f64::from(origin.x * zoom) + local[0],
        f64::from(origin.y * zoom) + local[1],
        local[2],
        local[3],
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_ime_screen_coordinates_scale_only_the_egui_window_origin() {
        assert_eq!(
            super::ime_screen_rect(
                eframe::egui::pos2(-100.0, 200.0),
                [10.0, 20.0, 1.0, 30.0],
                1.5,
            ),
            [-140.0, 320.0, 1.0, 30.0],
        );
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn absent_viewport_never_falls_back_to_another_window() {
        let context = eframe::egui::Context::default();
        assert!(super::for_viewport(&context, eframe::egui::ViewportId::ROOT).is_none());
    }
}
