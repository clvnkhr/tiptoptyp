//! A background Overleaf frontend owned by its native connection window.
//!
//! WebKit/WebView2 own authentication and website storage. Cookies and passwords
//! never cross the application bridge.
use crate::overleaf_inbox::Sender;
use eframe::egui;

#[cfg(any(target_os = "macos", target_os = "windows", test))]
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn navigation_allowed(candidate: &str) -> bool {
    if candidate == "about:blank" {
        return true;
    }
    let Ok(url) = url::Url::parse(candidate) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() || url.host().is_none() {
        return false;
    }
    url.scheme() == "https"
        || (url.scheme() == "http"
            && match url.host() {
                Some(url::Host::Domain("localhost")) => true,
                Some(url::Host::Ipv4(address)) => address.is_loopback(),
                Some(url::Host::Ipv6(address)) => address.is_loopback(),
                _ => false,
            })
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn message_allowed(origin: &str, candidate: &str, bytes: usize) -> bool {
    bytes <= MAX_MESSAGE_BYTES
        && navigation_allowed(candidate)
        && url::Url::parse(candidate).is_ok_and(|url| url.origin().ascii_serialization() == origin)
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn native_bounds(
    rect: egui::Rect,
    viewport_origin: egui::Pos2,
    scale: f32,
) -> Option<tiptoptyp_core::geometry::NativeRect> {
    use tiptoptyp_core::geometry::{EguiRect, ViewportTransform};
    if !rect.is_positive() {
        return None;
    }
    ViewportTransform::new([viewport_origin.x, viewport_origin.y], scale)?.to_native(EguiRect::new(
        [rect.min.x, rect.min.y],
        [rect.max.x, rect.max.y],
    )?)
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditAction {
    Copy,
    Cut,
    Paste,
    SelectAll,
    Undo,
    Redo,
}
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn edit_action(command: crate::native_menu::AppCommand) -> Option<EditAction> {
    use crate::native_menu::AppCommand;
    Some(match command {
        AppCommand::Copy => EditAction::Copy,
        AppCommand::Cut => EditAction::Cut,
        AppCommand::Paste => EditAction::Paste,
        AppCommand::SelectAll => EditAction::SelectAll,
        AppCommand::Undo => EditAction::Undo,
        AppCommand::Redo => EditAction::Redo,
        _ => return None,
    })
}

pub(crate) struct Browser {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview: wry::WebView,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    context: egui::Context,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    viewport: egui::ViewportId,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    origin: String,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    bounds: Option<tiptoptyp_core::geometry::NativeRect>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    visible: std::cell::Cell<bool>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Browser {
    pub(crate) fn new(
        context: &egui::Context,
        url: &str,
        epoch: u64,
        events: Sender,
    ) -> Result<Self, String> {
        if !navigation_allowed(url) || url == "about:blank" {
            return Err("Overleaf requires an HTTPS address.".into());
        }
        let origin = url::Url::parse(url)
            .map_err(|_| "Invalid Overleaf address.".to_owned())?
            .origin()
            .ascii_serialization();
        let viewport = context.viewport_id();
        let parent = crate::native_window::for_viewport(context, viewport)
            .ok_or_else(|| "The Overleaf connection window is not ready.".to_owned())?;
        // Wry injects into subframes on Windows even when main-only is requested.
        // The explicit guard also keeps the bridge out of third-party SSO pages.
        let script = format!(
            "if(window===window.top && location.origin==={}){{window.tiptoptypOverleafEpoch={epoch};\n{}\n}}",
            serde_json::json!(origin),
            include_str!("overleaf_bridge.js"),
        );
        let allowed_origin = origin.clone();
        #[cfg(target_os = "macos")]
        let termination_events = events.clone();
        let builder = wry::WebViewBuilder::new()
            .with_url(url)
            .with_focused(false)
            .with_visible(false)
            .with_background_throttling(wry::BackgroundThrottlingPolicy::Disabled)
            .with_hotkeys_zoom(false)
            .with_bounds(wry_bounds([0.0, 0.0, 1.0, 1.0]))
            .with_initialization_script_for_main_only(script, true)
            .with_navigation_handler(|candidate| navigation_allowed(&candidate))
            .with_new_window_req_handler(|candidate, _| {
                if navigation_allowed(&candidate) {
                    wry::NewWindowResponse::Allow
                } else {
                    wry::NewWindowResponse::Deny
                }
            })
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_ipc_handler(move |request| {
                if message_allowed(
                    &allowed_origin,
                    &request.uri().to_string(),
                    request.body().len(),
                ) {
                    // The decoder wakes the owner only after a typed result or
                    // explicit overflow; raw arrivals never force a UI pass.
                    let _ = events.send(request.into_body());
                }
            });
        #[cfg(target_os = "macos")]
        let builder = {
            use wry::WebViewBuilderExtDarwin as _;
            builder.with_on_web_content_process_terminate_handler(move || {
                let _ = termination_events.send(
                    serde_json::json!({
                        "event": "status",
                        "epoch": epoch,
                        "state": "disconnected",
                        "message": "The Overleaf browser stopped. Reconnect to continue.",
                    })
                    .to_string(),
                );
            })
        };
        let webview = builder
            .build_as_child(&parent)
            .map_err(|error| format!("Could not open the Overleaf frontend: {error}"))?;
        Ok(Self {
            webview,
            context: context.clone(),
            viewport,
            origin,
            bounds: None,
            visible: std::cell::Cell::new(false),
        })
    }

    /// `scale` converts egui points into native logical points, as for previews.
    pub(crate) fn set_bounds(&mut self, rect: egui::Rect, scale: f32) -> Result<(), String> {
        let viewport_origin = self
            .context
            .input_for(self.viewport, |input| input.viewport_rect().min);
        let bounds = native_bounds(rect, viewport_origin, scale)
            .ok_or_else(|| "Invalid Overleaf frontend bounds.".to_owned())?;
        if self.bounds == Some(bounds) {
            return Ok(());
        }
        self.webview
            .set_bounds(wry_bounds([
                bounds.left(),
                bounds.top(),
                bounds.width(),
                bounds.height(),
            ]))
            .map_err(|error| format!("Could not position the Overleaf frontend: {error}"))?;
        self.bounds = Some(bounds);
        if std::env::var_os("TIPTOPTYP_UI_TRACE").is_some() {
            eprintln!("ui.overleaf.bounds egui={rect:?} native={bounds:?} scale={scale}");
        }
        Ok(())
    }

    pub(crate) fn owns_keyboard(&self) -> bool {
        if !self.visible.get() {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            self.keyboard_target().is_some()
        }
        #[cfg(target_os = "windows")]
        {
            windows_input::owns_keyboard(&self.webview)
        }
    }

    /// Release native focus before moving it to an egui field in this window.
    pub(crate) fn focus_parent(&self) {
        if self.owns_keyboard() {
            let _ = self.webview.focus_parent();
        }
    }

    #[cfg(target_os = "macos")]
    fn keyboard_target(&self) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
        use wry::WebViewExtMacOS as _;
        let _main_thread = objc2::MainThreadMarker::new()?;
        let web = self.webview.webview();
        let window = web.window()?;
        if !window.isKeyWindow() || !window.isVisible() || web.isHiddenOrHasHiddenAncestor() {
            return None;
        }
        let target = window
            .firstResponder()?
            .downcast::<objc2_app_kit::NSView>()
            .ok()?;
        target.isDescendantOf(&web).then_some(target)
    }

    pub(crate) fn edit_command(&self, command: crate::native_menu::AppCommand) -> bool {
        let Some(action) = edit_action(command) else {
            return false;
        };
        if !self.owns_keyboard() {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            use objc2::sel;
            use objc2_foundation::NSObjectProtocol as _;
            let Some(marker) = objc2::MainThreadMarker::new() else {
                return false;
            };
            let Some(target) = self.keyboard_target() else {
                return false;
            };
            let selector = match action {
                EditAction::Copy => sel!(copy:),
                EditAction::Cut => sel!(cut:),
                EditAction::Paste => sel!(paste:),
                EditAction::SelectAll => sel!(selectAll:),
                EditAction::Undo => sel!(undo:),
                EditAction::Redo => sel!(redo:),
            };
            if !target.respondsToSelector(selector) {
                return false;
            }
            // SAFETY: the six standard AppKit editing selectors accept one
            // optional sender. The retained, focused WebKit descendant responds
            // to the selected method; explicit targeting prevents responder-chain
            // fallback into the editor or application.
            unsafe {
                objc2_app_kit::NSApplication::sharedApplication(marker).sendAction_to_from(
                    selector,
                    Some(&target),
                    None,
                )
            }
        }
        #[cfg(target_os = "windows")]
        {
            let action = match action {
                EditAction::Copy => "copy",
                EditAction::Cut => "cut",
                EditAction::Paste => "paste",
                EditAction::SelectAll => "selectAll",
                EditAction::Undo => "undo",
                EditAction::Redo => "redo",
            };
            let script = format!(
                "if(window===window.top && location.origin==={} && document.hasFocus()){{document.execCommand({});}}",
                serde_json::json!(self.origin),
                serde_json::json!(action),
            );
            self.webview.evaluate_script(&script).is_ok()
        }
    }

    pub(crate) fn set_visible(&self, visible: bool) -> Result<(), String> {
        if self.visible.get() != visible {
            if !visible {
                self.focus_parent();
            }
            self.webview
                .set_visible(visible)
                .map_err(|error| format!("Could not show the Overleaf frontend: {error}"))?;
            self.visible.set(visible);
        }
        Ok(())
    }

    pub(crate) fn command(&self, command: &serde_json::Value) -> Result<(), String> {
        let payload = serde_json::to_string(command)
            .map_err(|_| "Could not encode the Overleaf command.".to_owned())?;
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err("The Overleaf command exceeds the 16 MiB limit.".into());
        }
        let script = format!(
            "if(window===window.top && location.origin==={}){{window.tiptoptypOverleaf?.command({payload});}}",
            serde_json::json!(self.origin),
        );
        self.webview
            .evaluate_script(&script)
            .map_err(|error| format!("Could not send the Overleaf command: {error}"))
    }
}

#[cfg(target_os = "windows")]
mod windows_input {
    use std::ffi::c_void;
    use wry::WebViewExtWindows as _;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetFocus() -> *mut c_void;
        fn GetForegroundWindow() -> *mut c_void;
        fn GetAncestor(window: *mut c_void, flags: u32) -> *mut c_void;
        fn IsChild(parent: *mut c_void, child: *mut c_void) -> i32;
    }
    pub(super) fn owns_keyboard(web: &wry::WebView) -> bool {
        let parent = web.hwnd().0 as *mut c_void;
        // SAFETY: HWNDs are borrowed from a live Wry view on its owning event
        // thread. These user32 queries neither mutate nor retain the windows.
        unsafe {
            let focused = GetFocus();
            !focused.is_null()
                && (focused == parent || IsChild(parent, focused) != 0)
                && GetForegroundWindow() == GetAncestor(parent, 2)
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn wry_bounds([left, top, width, height]: [f32; 4]) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(f64::from(left), f64::from(top)).into(),
        size: wry::dpi::LogicalSize::new(f64::from(width), f64::from(height)).into(),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl Browser {
    pub(crate) fn new(
        _context: &egui::Context,
        _url: &str,
        _epoch: u64,
        _events: Sender,
    ) -> Result<Self, String> {
        Err("The Overleaf frontend is available on macOS and Windows.".into())
    }

    pub(crate) fn set_bounds(&mut self, _rect: egui::Rect, _scale: f32) -> Result<(), String> {
        Err("The Overleaf frontend is unavailable on this platform.".into())
    }

    pub(crate) fn set_visible(&self, _visible: bool) -> Result<(), String> {
        Err("The Overleaf frontend is unavailable on this platform.".into())
    }

    pub(crate) fn command(&self, _command: &serde_json::Value) -> Result<(), String> {
        Err("The Overleaf frontend is unavailable on this platform.".into())
    }

    pub(crate) fn owns_keyboard(&self) -> bool {
        false
    }

    pub(crate) fn focus_parent(&self) {}

    pub(crate) fn edit_command(&self, _command: crate::native_menu::AppCommand) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_edit_dispatch_is_a_closed_whitelist() {
        use crate::native_menu::AppCommand;
        for spec in crate::native_menu::COMMAND_SPECS {
            assert_eq!(
                edit_action(spec.command).is_some(),
                matches!(
                    spec.command,
                    AppCommand::Copy
                        | AppCommand::Cut
                        | AppCommand::Paste
                        | AppCommand::SelectAll
                        | AppCommand::Undo
                        | AppCommand::Redo
                )
            );
        }
    }

    #[test]
    fn navigation_allows_secure_sso_and_local_fixtures_without_url_credentials() {
        for candidate in [
            "https://www.overleaf.com/login",
            "https://institution.example/sso",
            "about:blank",
            "http://localhost:3421/project/test",
            "http://127.0.0.1:3421/",
            "http://[::1]:3421/",
        ] {
            assert!(navigation_allowed(candidate), "{candidate}");
        }
        for candidate in [
            "http://www.overleaf.com/login",
            "https://user:password@www.overleaf.com/login",
            "javascript:alert(1)",
            "file:///tmp/project.tex",
            "https://",
            "http://localhost.example/",
        ] {
            assert!(!navigation_allowed(candidate), "{candidate}");
        }
    }

    #[test]
    fn ipc_requires_the_exact_origin_and_bounded_body() {
        let origin = "https://www.overleaf.com";
        assert!(message_allowed(
            origin,
            "https://www.overleaf.com/project/abc",
            MAX_MESSAGE_BYTES
        ));
        for candidate in [
            "https://www.overleaf.com.evil.example/project/abc",
            "https://overleaf.com/project/abc",
            "https://www.overleaf.com:8443/project/abc",
            "https://institution.example/sso",
            "http://www.overleaf.com/project/abc",
            "data:text/plain,hello",
            "about:blank",
        ] {
            assert!(!message_allowed(origin, candidate, 10), "{candidate}");
        }
        assert!(!message_allowed(origin, origin, MAX_MESSAGE_BYTES + 1));
    }

    #[test]
    fn bounds_use_the_preview_transform_and_reject_degenerate_rectangles() {
        let rect = egui::Rect::from_min_max(egui::pos2(30.0, 40.0), egui::pos2(130.0, 90.0));
        let bounds = native_bounds(rect, egui::pos2(10.0, 20.0), 1.5).unwrap();
        assert_eq!(
            [bounds.left(), bounds.top(), bounds.width(), bounds.height()],
            [40.0, 50.0, 150.0, 75.0]
        );
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(native_bounds(rect, egui::Pos2::ZERO, scale).is_none());
        }
        assert!(native_bounds(egui::Rect::NOTHING, egui::Pos2::ZERO, 1.0).is_none());
        assert!(native_bounds(egui::Rect::ZERO, egui::Pos2::ZERO, 1.0).is_none());
    }
}
