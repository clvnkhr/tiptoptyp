//! Native ownership comes from the renderer's viewport binding, never focus.

#[cfg(target_os = "macos")]
mod platform {
    use std::{cell::Cell, ffi::c_void, ptr::NonNull, sync::Arc};

    use dispatch2::MainThreadBound;
    use objc2::{
        DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
        runtime::AnyObject,
    };
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAutoresizingMaskOptions, NSColor, NSView, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
        NSWindowOrderingMode,
    };
    use objc2_foundation::{NSArray, NSNumber, NSString};

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

        /// Retain alpha capability without changing renderer or input ownership.
        /// Native blur must be a sibling below this view, never a child covering CGL.
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

        pub(crate) fn blur_background(&self, dark: bool, radius: u8) -> Option<NativeBlur> {
            let marker = MainThreadMarker::new()?;
            let host = superview(&self.view)?;
            let effect = RadiusVisualEffectView::alloc(marker).set_ivars(Cell::new(radius));
            // SAFETY: initialize the NSVisualEffectView superclass on AppKit's
            // main thread; only the decorative view is subclassed.
            let effect: Retained<RadiusVisualEffectView> =
                unsafe { msg_send![super(effect), initWithFrame: self.view.frame()] };
            effect.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
            effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            effect.setState(NSVisualEffectState::FollowsWindowActiveState);
            effect.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            set_blur_appearance(&effect, dark);
            host.addSubview_positioned_relativeTo(
                &effect,
                NSWindowOrderingMode::Below,
                Some(&self.view),
            );
            Some(NativeBlur(Arc::new(MainThreadBound::new(
                BlurView { effect },
                marker,
            ))))
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

    /// egui's viewport cache owns the underlay. It retains no renderer/window;
    /// dropping or replacing the cache removes only this decoration on AppKit's
    /// main thread, including when a context is released on another thread.
    #[derive(Clone)]
    pub(crate) struct NativeBlur(Arc<MainThreadBound<BlurView>>);

    struct BlurView {
        effect: Retained<RadiusVisualEffectView>,
    }
    // AppKit has no public radius property. Restrict the private filter access
    // to its existing gaussianBlur filter; do not build or replace its layers.
    // updateLayer reapplies after AppKit regenerates active/theme materials.
    define_class!(
        #[unsafe(super = NSVisualEffectView)]
        #[name = "TiptoptypWindowBlur"]
        #[thread_kind = MainThreadOnly]
        #[ivars = Cell<u8>]
        struct RadiusVisualEffectView;

        impl RadiusVisualEffectView {
            #[unsafe(method(updateLayer))]
            fn update_layer(&self) {
                // SAFETY: invoke AppKit's normal material update before changing
                // the existing filter, on the same main-thread view.
                unsafe { let _: () = msg_send![super(self), updateLayer]; }
                self.apply_radius();
            }
        }
    );
    impl RadiusVisualEffectView {
        fn set_radius(&self, radius: u8) {
            if self.ivars().replace(radius) != radius {
                self.apply_radius();
                self.setNeedsDisplay(true);
            }
        }
        fn apply_radius(&self) {
            let Some((layer, name)) = gaussian_blur_layer(self) else {
                return;
            };
            let value = NSNumber::new_f64(f64::from(self.ivars().get()));
            // SAFETY: the layer is Apple's CALayer and has a named gaussianBlur
            // CAFilter. Its inputRadius accepts NSNumber. No KVC lookup is made
            // on a missing/unknown filter; unsupported materials stay native.
            unsafe {
                let _: () = msg_send![&*layer, setValue: &*value, forKeyPath: &*NSString::from_str(&format!("filters.{name}.inputRadius"))];
            }
        }
    }
    fn gaussian_blur_layer(
        effect: &NSVisualEffectView,
    ) -> Option<(Retained<AnyObject>, Retained<NSString>)> {
        // SAFETY: NSView.layer returns a retained CALayer; traversal only sends
        // CALayer selectors to its sublayers and CAFilter.name to its filters.
        unsafe {
            let layer: Option<Retained<AnyObject>> = msg_send![effect, layer];
            find_gaussian_blur_layer(layer?, 0)
        }
    }
    unsafe fn find_gaussian_blur_layer(
        layer: Retained<AnyObject>,
        depth: usize,
    ) -> Option<(Retained<AnyObject>, Retained<NSString>)> {
        if depth >= 8 {
            return None;
        }
        // SAFETY: caller establishes CALayer identity for the complete bounded
        // traversal. Private CAFilter.name is only read on CALayer's filters.
        unsafe {
            let filters: Option<Retained<NSArray<AnyObject>>> = msg_send![&*layer, filters];
            if let Some(filters) = filters {
                for filter in filters.iter().take(8) {
                    let name: Option<Retained<NSString>> = msg_send![&*filter, name];
                    if let Some(name) = name {
                        // AppKit can uniquify names as "gaussianBlur 2" when
                        // multiple windows/materials exist. Use its actual name
                        // in the CALayer filter key path, not a fixed alias.
                        if name.to_string().split_whitespace().next() == Some("gaussianBlur") {
                            return Some((layer, name));
                        }
                    }
                }
            }
            let children: Option<Retained<NSArray<AnyObject>>> = msg_send![&*layer, sublayers];
            for child in children?.iter().take(16) {
                if let Some(found) = find_gaussian_blur_layer(child, depth + 1) {
                    return Some(found);
                }
            }
            None
        }
    }
    impl Drop for BlurView {
        fn drop(&mut self) {
            self.effect.removeFromSuperview();
        }
    }
    fn superview(view: &NSView) -> Option<Retained<NSView>> {
        let _marker = MainThreadMarker::new()?;
        // SAFETY: all callers supply a retained AppKit view and this lookup is
        // performed on the main thread while the view hierarchy is stable.
        unsafe { view.superview() }
    }
    fn set_blur_appearance(effect: &NSVisualEffectView, dark: bool) {
        // Match the app's explicit Light/Dark choice, not the OS's appearance.
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        if let Some(appearance) = NSAppearance::appearanceNamed(name) {
            effect.setAppearance(Some(&appearance));
        }
    }
    impl NativeBlur {
        /// Native resizing is handled by AppKit. This bounded identity check also
        /// repairs placement if AppKit moves the renderer to a different host
        /// during a fullscreen transition; settled frames make no mutations.
        pub(crate) fn synchronize(
            &self,
            parent: &NativeWindowHandle,
            appearance: Option<(bool, u8)>,
        ) -> bool {
            let Some(marker) = MainThreadMarker::new() else {
                return false;
            };
            let Some(host) = superview(&parent.view) else {
                return false;
            };
            let effect = &self.0.get(marker).effect;
            if superview(effect).as_deref() != Some(&*host) {
                effect.removeFromSuperview();
                effect.setFrame(parent.view.frame());
                host.addSubview_positioned_relativeTo(
                    effect,
                    NSWindowOrderingMode::Below,
                    Some(&parent.view),
                );
            }
            if let Some((dark, radius)) = appearance {
                set_blur_appearance(effect, dark);
                effect.set_radius(radius);
            }
            true
        }

        #[cfg(any(test, feature = "native-ui-tests", feature = "desktop-ui-tests"))]
        #[allow(dead_code)]
        pub(crate) fn effective_radius(&self) -> Option<f64> {
            let marker = MainThreadMarker::new()?;
            let (layer, name) = gaussian_blur_layer(&self.0.get(marker).effect)?;
            // SAFETY: known native gaussianBlur inputRadius is NSNumber.
            unsafe {
                let value: Option<Retained<NSNumber>> = msg_send![&*layer, valueForKeyPath: &*NSString::from_str(&format!("filters.{name}.inputRadius"))];
                value.map(|value| value.doubleValue())
            }
        }

        #[cfg(any(test, feature = "native-ui-tests", feature = "desktop-ui-tests"))]
        #[allow(dead_code)]
        pub(crate) fn inspection(
            &self,
            parent: &NativeWindowHandle,
        ) -> Option<([f64; 4], bool, usize)> {
            let marker = MainThreadMarker::new()?;
            let effect = &self.0.get(marker).effect;
            let frame = effect.frame();
            let host = superview(&parent.view)?;
            let siblings = host.subviews();
            let below = siblings
                .iter()
                .position(|v| {
                    std::ptr::eq(
                        Retained::as_ptr(&v),
                        Retained::as_ptr(effect).cast::<NSView>(),
                    )
                })
                .zip(siblings.iter().position(|v| {
                    std::ptr::eq(Retained::as_ptr(&v), Retained::as_ptr(&parent.view))
                }))
                .is_some_and(|(effect, renderer)| effect < renderer);
            Some((
                [
                    frame.origin.x,
                    frame.origin.y,
                    frame.size.width,
                    frame.size.height,
                ],
                superview(effect).as_deref() == Some(&*host) && below,
                Retained::as_ptr(effect) as usize,
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

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn absent_viewport_never_falls_back_to_another_window() {
        let context = eframe::egui::Context::default();
        assert!(super::for_viewport(&context, eframe::egui::ViewportId::ROOT).is_none());
    }
}
