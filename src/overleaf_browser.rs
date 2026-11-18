//! A background Overleaf frontend owned by its native connection window.
//!
//! WebKit/WebView2 own authentication and website storage. Cookies and passwords
//! never cross the application bridge.
use crate::overleaf_inbox::Sender;
use eframe::egui;
#[path = "overleaf_browser_auth.rs"]
mod auth;
#[cfg(any(target_os = "macos", test))]
#[path = "overleaf_browser_focus.rs"]
mod native_focus;
#[cfg(any(target_os = "macos", test))]
#[path = "overleaf_browser_passwords.rs"]
mod passwords;
pub(crate) use auth::SignIn;
#[cfg(target_os = "macos")]
pub(crate) use passwords::open_passwords;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[path = "overleaf_browser_profile.rs"]
mod profile;

pub(crate) const LOGIN_URL: &str = "https://www.overleaf.com/login";
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const OVERLEAF_ORIGIN: &str = "https://www.overleaf.com";
const MAX_URL_BYTES: usize = 16 * 1024;

/// Browsing is independent of access to the authoritative project bridge.
/// A valid browsing address alone does not grant access to the project bridge.
pub(crate) fn navigation_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.len() > MAX_URL_BYTES {
        return Err("The browser address exceeds the 16 KiB limit.".into());
    }
    if input == "about:blank" {
        return Ok(input.into());
    }
    let url = url::Url::parse(input).map_err(|_| "Enter an HTTP or HTTPS address.".to_owned())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host().is_none()
    {
        return Err("Use an HTTP or HTTPS address without URL credentials.".into());
    }
    Ok(url.into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Navigation {
    pub(crate) url: String,
    pub(crate) project: Option<crate::overleaf::ProjectUrl>,
    pub(crate) sign_in: SignIn,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn navigation_allowed(candidate: &str) -> bool {
    navigation_url(candidate).is_ok()
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn message_allowed(navigation: &ProjectNavigation, candidate: &str) -> bool {
    navigation.is_active() && navigation.owns_page(candidate)
}

/// Shared by the native callback and deterministic routing checks. The caller
/// already owns the IPC body, so rejection never allocates another copy.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn receive_message(
    navigation: &ProjectNavigation,
    candidate: &str,
    message: String,
    events: &Sender,
) {
    if message_allowed(navigation, candidate) {
        // A missing authoritative event invalidates the stream. Let the bounded
        // sender report oversized bodies instead of silently dropping them.
        let _ = events.send(message);
    }
}

/// Track committed main-frame pages, never generic navigation-policy requests:
/// WebKit also calls its policy for iframes, which do not end our project session.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageAuthority {
    Uncommitted,
    ProjectPage,
    Browsing,
}
#[cfg(any(target_os = "macos", target_os = "windows", test))]
struct ProjectNavigation {
    // A page can browse anywhere, but only this project belongs to the native
    // model's epoch. Discovery is a request to establish a new connection.
    project: Option<crate::overleaf::ProjectUrl>,
    trusted_origin: String,
    authority: PageAuthority,
    current_url: String,
    pending: Option<Navigation>,
    recovery: auth::Recovery,
}
#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl ProjectNavigation {
    fn new(address: &str) -> Result<Self, String> {
        let address = navigation_url(address)?;
        let project = crate::overleaf::ProjectUrl::parse(&address).ok();
        let trusted_origin = project
            .as_ref()
            .map_or(OVERLEAF_ORIGIN, |project| project.origin())
            .to_owned();
        Ok(Self {
            project,
            trusted_origin,
            authority: PageAuthority::Uncommitted,
            current_url: address,
            pending: None,
            recovery: auth::Recovery::default(),
        })
    }
    /// Returning to the project permits browser commands, but never reports a
    /// live connection. Native editing still requires fresh project hydration.
    fn started(&mut self, candidate: &str) -> bool {
        let active = self.owns_page(candidate);
        let departed = self.is_active() && !active;
        self.authority = if active {
            PageAuthority::ProjectPage
        } else {
            PageAuthority::Browsing
        };
        if let Ok(address) = navigation_url(candidate) {
            self.current_url.clone_from(&address);
            self.pending = Some(Navigation {
                project: self.project_on_trusted_origin(&address),
                sign_in: self.recovery.observe(self.project.as_ref(), &address),
                url: address,
            });
        } else {
            self.pending = None;
        }
        departed
    }
    fn is_active(&self) -> bool {
        self.authority == PageAuthority::ProjectPage
    }
    fn project_on_trusted_origin(&self, candidate: &str) -> Option<crate::overleaf::ProjectUrl> {
        let mut url = url::Url::parse(candidate).ok()?;
        if url.origin().ascii_serialization() != self.trusted_origin
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        url.set_query(None);
        url.set_fragment(None);
        crate::overleaf::ProjectUrl::parse(url.as_str()).ok()
    }
    fn owns_page(&self, candidate: &str) -> bool {
        self.project.as_ref().is_some_and(|project| {
            url::Url::parse(candidate).is_ok_and(|url| {
                url.origin().ascii_serialization() == project.origin()
                    && url.path().strip_suffix('/').unwrap_or(url.path())
                        == format!("/project/{}", project.id())
                    && url.username().is_empty()
                    && url.password().is_none()
            })
        })
    }
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
    // Field order removes the native event monitor before the WebView drops.
    #[cfg(target_os = "macos")]
    native_focus: native_focus::Lease,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview: wry::WebView,
    // Drop the WebView before its context; moving/rebuilding the executable
    // must not select another WebView2 cookie profile.
    #[cfg(target_os = "windows")]
    _profile: Box<wry::WebContext>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    context: egui::Context,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    viewport: egui::ViewportId,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    navigation: std::rc::Rc<std::cell::RefCell<ProjectNavigation>>,
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
        profile_id: Option<[u8; 16]>,
    ) -> Result<Self, String> {
        let url = navigation_url(url)?;
        let navigation = std::rc::Rc::new(std::cell::RefCell::new(ProjectNavigation::new(&url)?));
        let viewport = context.viewport_id();
        let parent = crate::native_window::for_viewport(context, viewport)
            .ok_or_else(|| "The Overleaf connection window is not ready.".to_owned())?;
        // Wry injects into subframes on Windows even when main-only is requested.
        // The explicit guard also keeps the bridge out of third-party SSO pages.
        let script = navigation.borrow().project.as_ref().map_or_else(String::new, |project| {
            format!(
                "if(window===window.top && location.origin==={} && location.pathname.replace(/\\/$/,'')==={}){{window.tiptoptypOverleafEpoch={epoch};\n{}\n}}",
                serde_json::json!(project.origin()),
                serde_json::json!(format!("/project/{}", project.id())),
                include_str!("overleaf_bridge.js"),
            )
        });
        let load_navigation = std::rc::Rc::clone(&navigation);
        let ipc_navigation = std::rc::Rc::clone(&navigation);
        let navigation_events = events.clone();
        let navigation_context = context.clone();
        #[cfg(target_os = "macos")]
        let termination_events = events.clone();
        #[cfg(target_os = "windows")]
        let mut profile = {
            let local_data = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);
            Box::new(wry::WebContext::new(Some(profile::directory(
                local_data.as_deref(),
                profile_id,
            )?)))
        };
        #[cfg(target_os = "windows")]
        let builder = wry::WebViewBuilder::new_with_web_context(&mut profile);
        #[cfg(not(target_os = "windows"))]
        let builder = wry::WebViewBuilder::new();
        #[cfg(target_os = "macos")]
        let builder = profile::configure_macos(builder, profile_id)?;
        let builder = builder
            .with_incognito(false)
            .with_url(&url)
            .with_focused(false)
            .with_visible(false)
            .with_background_throttling(wry::BackgroundThrottlingPolicy::Disabled)
            .with_hotkeys_zoom(false)
            .with_bounds(wry_bounds([0.0, 0.0, 1.0, 1.0]))
            .with_initialization_script_for_main_only(script, true)
            .with_navigation_handler(|candidate| navigation_allowed(&candidate))
            .with_on_page_load_handler(move |event, candidate| {
                if !matches!(event, wry::PageLoadEvent::Started) {
                    return;
                }
                let departed = load_navigation.borrow_mut().started(&candidate);
                navigation_context.request_repaint();
                if departed {
                    let _ = navigation_events.send(
                        serde_json::json!({
                            "event": "status",
                            "epoch": epoch,
                            "state": "disconnected",
                            "message": "The Overleaf browser left this project. Sign in or reconnect to continue.",
                        })
                        .to_string(),
                    );
                }
            })
            .with_new_window_req_handler(|candidate, _| {
                if navigation_allowed(&candidate) {
                    wry::NewWindowResponse::Allow
                } else {
                    wry::NewWindowResponse::Deny
                }
            })
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_ipc_handler_for_main_frame(move |request| {
                // The decoder wakes the owner only after a typed result or
                // explicit overflow; raw arrivals never force a UI pass.
                receive_message(
                    &ipc_navigation.borrow(),
                    &request.uri().to_string(),
                    request.into_body(),
                    &events,
                );
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
        #[cfg(target_os = "macos")]
        let native_focus = native_focus::Lease::new(&webview)?;
        Ok(Self {
            #[cfg(target_os = "macos")]
            native_focus,
            webview,
            #[cfg(target_os = "windows")]
            _profile: profile,
            context: context.clone(),
            viewport,
            navigation,
            bounds: None,
            visible: std::cell::Cell::new(false),
        })
    }

    pub(crate) fn navigate(&self, address: &str) -> Result<(), String> {
        let address = navigation_url(address)?;
        self.navigation.borrow_mut().recovery.cancel();
        self.webview
            .load_url(&address)
            .map_err(|error| format!("Could not open the browser address: {error}"))
    }

    pub(crate) fn back(&self) -> Result<(), String> {
        self.navigation.borrow_mut().recovery.cancel();
        self.webview
            .evaluate_script("window.history.back();")
            .map_err(|error| format!("Could not go back in the browser: {error}"))
    }

    pub(crate) fn forward(&self) -> Result<(), String> {
        self.navigation.borrow_mut().recovery.cancel();
        self.webview
            .evaluate_script("window.history.forward();")
            .map_err(|error| format!("Could not go forward in the browser: {error}"))
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(crate) fn current_url(&self) -> String {
        self.navigation.borrow().current_url.clone()
    }

    pub(crate) fn project_ready(&self) {
        self.navigation.borrow_mut().recovery.ready();
    }
    pub(crate) fn cancel_sign_in_return(&self) {
        self.navigation.borrow_mut().recovery.cancel();
    }

    /// Main-frame callbacks coalesce redirects, so a stale project route never
    /// starts a connection after the browser has already navigated elsewhere.
    pub(crate) fn take_navigation(&self) -> Option<Navigation> {
        self.navigation.borrow_mut().pending.take()
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
            self.native_focus
                .browser_owns_keyboard(self.keyboard_target().is_some())
        }
        #[cfg(target_os = "windows")]
        {
            windows_input::owns_keyboard(&self.webview)
        }
    }

    /// Explicit widget focus repairs the native responder even if this window
    /// is not key yet. `owns_keyboard` is an observation, not an admission gate.
    pub(crate) fn focus_parent(&self) {
        #[cfg(target_os = "macos")]
        self.native_focus.claim_parent();
        #[cfg(target_os = "windows")]
        let _ = self.webview.focus_parent();
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(crate) fn keyboard_inspection(&self) -> serde_json::Value {
        use wry::WebViewExtMacOS as _;
        let web = self.webview.webview();
        let window = web.window();
        let browser_responder = window
            .as_ref()
            .and_then(|window| window.firstResponder())
            .and_then(|responder| responder.downcast::<objc2_app_kit::NSView>().ok())
            .is_some_and(|responder| responder.isDescendantOf(&web));
        let parent_responder = crate::native_window::for_viewport(&self.context, self.viewport)
            .and_then(|parent| parent.ime_cursor_area())
            .map(|(_, focused)| focused);
        let rect = |rect: objc2_foundation::NSRect| {
            [
                rect.origin.x,
                rect.origin.y,
                rect.size.width,
                rect.size.height,
            ]
        };
        let web_window_rect = web.convertRect_toView(web.bounds(), None);
        let screen_rect = window
            .as_ref()
            .map(|window| window.convertRectToScreen(web_window_rect));
        let screen_top = objc2::MainThreadMarker::new()
            .and_then(|marker| objc2_app_kit::NSScreen::screens(marker).firstObject())
            .map(|screen| screen.frame().origin.y + screen.frame().size.height);
        let screen_rect_top_left = screen_rect.zip(screen_top).map(|(screen, top)| {
            [
                screen.origin.x,
                top - screen.origin.y - screen.size.height,
                screen.size.width,
                screen.size.height,
            ]
        });
        // SAFETY: this live WebView is queried on its owning AppKit thread;
        // the returned retained parent is used only for read-only geometry.
        let host = unsafe { web.superview() };
        serde_json::json!({
            "window_key": window.as_ref().map(|window| window.isKeyWindow()),
            "window_visible": window.as_ref().map(|window| window.isVisible()),
            "browser_first_responder": browser_responder,
            "parent_first_responder": parent_responder,
            "keyboard_owner": self.native_focus.owner(),
            "viewport": format!("{:?}", self.viewport),
            "window_title_fingerprint": window.as_ref().map(|window|crate::desktop_test::fingerprint(&window.title().to_string())),
            "window_frame": window.as_ref().map(|window| rect(window.frame())),
            "web_frame": rect(web.frame()),
            "web_bounds": rect(web.bounds()),
            "host_frame": host.as_ref().map(|host| rect(host.frame())),
            "host_bounds": host.as_ref().map(|host| rect(host.bounds())),
            "web_window_rect": rect(web_window_rect),
            "web_screen_rect_appkit": screen_rect.map(rect),
            "web_screen_rect_top_left": screen_rect_top_left,
        })
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
                "if(window===window.top && document.hasFocus()){{document.execCommand({});}}",
                serde_json::json!(action),
            );
            self.webview.evaluate_script(&script).is_ok()
        }
    }

    pub(crate) fn set_visible(&self, visible: bool) -> Result<(), String> {
        if self.visible.get() != visible {
            if !visible && self.owns_keyboard() {
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
        let navigation = self.navigation.borrow();
        let Some(project) = navigation
            .project
            .as_ref()
            .filter(|_| navigation.is_active())
        else {
            return Err("The Overleaf browser is outside this project. Sign in or reconnect before continuing.".into());
        };
        let payload = serde_json::to_string(command)
            .map_err(|_| "Could not encode the Overleaf command.".to_owned())?;
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err("The Overleaf command exceeds the 16 MiB limit.".into());
        }
        let script = format!(
            "if(window===window.top && location.origin==={} && location.pathname.replace(/\\/$/,'')==={}){{window.tiptoptypOverleaf?.command({payload});}}",
            serde_json::json!(project.origin()),
            serde_json::json!(format!("/project/{}", project.id())),
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
        _profile_id: Option<[u8; 16]>,
    ) -> Result<Self, String> {
        Err("The Overleaf frontend is available on macOS and Windows.".into())
    }

    pub(crate) fn navigate(&self, _address: &str) -> Result<(), String> {
        Err("The Overleaf frontend is unavailable on this platform.".into())
    }

    pub(crate) fn back(&self) -> Result<(), String> {
        Err("The Overleaf frontend is unavailable on this platform.".into())
    }

    pub(crate) fn forward(&self) -> Result<(), String> {
        Err("The Overleaf frontend is unavailable on this platform.".into())
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(crate) fn current_url(&self) -> String {
        LOGIN_URL.into()
    }

    pub(crate) fn take_navigation(&self) -> Option<Navigation> {
        None
    }
    pub(crate) fn project_ready(&self) {}
    pub(crate) fn cancel_sign_in_return(&self) {}

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
    fn browsing_allows_web_pages_and_sso_without_granting_native_access() {
        for candidate in [
            "https://www.overleaf.com/login",
            "https://institution.example/sso",
            "about:blank",
            "http://localhost:3421/project/test",
            "http://127.0.0.1:3421/",
            "http://[::1]:3421/",
            "http://www.overleaf.com/login",
            "http://self-hosted.example/",
            "http://localhost.example/",
        ] {
            assert!(navigation_allowed(candidate), "{candidate}");
        }
        for candidate in [
            "https://user:password@www.overleaf.com/login",
            "javascript:alert(1)",
            "file:///tmp/project.tex",
            "https://",
            "data:text/html,hello",
            "mailto:alice@example.org",
        ] {
            assert!(!navigation_allowed(candidate), "{candidate}");
        }
    }

    #[test]
    fn login_starts_without_a_project_and_discovers_only_verified_routes() {
        let mut navigation = ProjectNavigation::new(LOGIN_URL).unwrap();
        assert!(navigation.project.is_none());
        assert!(!navigation.is_active());
        assert_eq!(navigation.current_url, LOGIN_URL);
        assert!(!navigation.started(LOGIN_URL));
        assert_eq!(navigation.pending.take().unwrap().project, None);
        for candidate in [
            "https://institution.example/sso",
            "http://www.overleaf.com/project/0123456789abcdef01234567",
            "https://www.overleaf.com.evil.example/project/0123456789abcdef01234567",
            "https://www.overleaf.com:8443/project/0123456789abcdef01234567",
            "https://www.overleaf.com/project/0123456789abcdef01234567/download",
            "https://www.overleaf.com/project/0123456789abcdef012345670",
        ] {
            assert!(
                !navigation.started(candidate),
                "unbound browser: {candidate}"
            );
            assert!(
                navigation.pending.take().unwrap().project.is_none(),
                "{candidate}"
            );
            assert!(!message_allowed(&navigation, candidate));
        }
        let candidate = "https://www.overleaf.com/project/0123456789abcdef01234567/?doc=abcdef0123456789abcdef01#editor";
        assert!(!navigation.started(candidate));
        let publication = navigation.pending.take().unwrap();
        assert_eq!(publication.url, candidate);
        assert_eq!(
            publication.project.unwrap().id(),
            "0123456789abcdef01234567"
        );
        assert!(
            !navigation.is_active(),
            "discovery is not a live project connection"
        );
        assert!(!message_allowed(&navigation, candidate));
    }

    #[test]
    fn trusted_discovery_coalesces_redirects_and_does_not_change_bound_project() {
        let project = crate::overleaf::ProjectUrl::parse(
            "https://self-hosted.example/project/0123456789abcdef01234567",
        )
        .unwrap();
        let mut navigation = ProjectNavigation::new(project.url()).unwrap();
        assert!(!navigation.started(project.url()));
        navigation.pending.take();
        let next = "https://self-hosted.example/project/abcdef0123456789abcdef01";
        assert!(navigation.started(next));
        assert_eq!(
            navigation
                .pending
                .as_ref()
                .unwrap()
                .project
                .as_ref()
                .unwrap()
                .id(),
            "abcdef0123456789abcdef01"
        );
        assert!(!message_allowed(&navigation, next));
        assert!(!navigation.started("https://self-hosted.example/login"));
        let redirect = navigation.pending.take().unwrap();
        assert_eq!(redirect.url, "https://self-hosted.example/login");
        assert!(
            redirect.project.is_none(),
            "no stale project discovery after a redirect"
        );
        assert_eq!(navigation.project.as_ref(), Some(&project));
        assert!(!navigation.started("https://www.overleaf.com/project/abcdef0123456789abcdef01"));
        assert!(
            navigation.pending.take().unwrap().project.is_none(),
            "explicit self-host origin remains exact"
        );
    }

    #[test]
    fn navigation_bounds_addresses_and_does_not_promote_arbitrary_sites_to_trusted_hosts() {
        assert_eq!(
            navigation_url("  https://example.org/page?q=1#section  ").unwrap(),
            "https://example.org/page?q=1#section"
        );
        assert!(
            navigation_url(&format!(
                "https://example.org/{}",
                "a".repeat(MAX_URL_BYTES)
            ))
            .is_err()
        );
        let mut navigation = ProjectNavigation::new("https://institution.example/sso").unwrap();
        navigation.started("https://institution.example/project/0123456789abcdef01234567");
        assert!(navigation.pending.take().unwrap().project.is_none());
        navigation.started("https://www.overleaf.com/project/0123456789abcdef01234567");
        assert!(navigation.pending.take().unwrap().project.is_some());
    }

    #[test]
    fn ipc_requires_the_active_project_page() {
        let project = crate::overleaf::ProjectUrl::parse(
            "https://www.overleaf.com/project/0123456789abcdef01234567",
        )
        .unwrap();
        let mut navigation = ProjectNavigation::new(project.url()).unwrap();
        assert!(!message_allowed(&navigation, project.url()));
        navigation.started(project.url());
        assert!(message_allowed(&navigation, project.url()));
        assert!(message_allowed(
            &navigation,
            "https://www.overleaf.com/project/0123456789abcdef01234567?doc=abcdef0123456789abcdef01",
        ));
        for candidate in [
            "https://www.overleaf.com.evil.example/project/0123456789abcdef01234567",
            "https://overleaf.com/project/0123456789abcdef01234567",
            "https://www.overleaf.com:8443/project/0123456789abcdef01234567",
            "https://institution.example/sso",
            "http://www.overleaf.com/project/0123456789abcdef01234567",
            "https://www.overleaf.com/login",
            "https://www.overleaf.com/project/abcdef0123456789abcdef01",
            "https://www.overleaf.com/project/0123456789abcdef01234567/download",
            "data:text/plain,hello",
            "about:blank",
        ] {
            assert!(!message_allowed(&navigation, candidate), "{candidate}");
        }
        assert!(navigation.started("https://institution.example/sso"));
        assert!(
            !message_allowed(&navigation, project.url()),
            "late old-page IPC cannot revive a departed project"
        );
    }

    #[test]
    fn oversized_active_project_ipc_is_terminal_but_wrong_route_cannot_poison_inbox() {
        let project = crate::overleaf::ProjectUrl::parse(
            "https://www.overleaf.com/project/0123456789abcdef01234567",
        )
        .unwrap();
        let mut navigation = ProjectNavigation::new(project.url()).unwrap();
        navigation.started(project.url());
        let mut inbox = crate::overleaf_inbox::Inbox::new(&egui::Context::default());
        let events = inbox.sender();
        for candidate in [
            "https://www.overleaf.com/login",
            "https://www.overleaf.com/project/abcdef0123456789abcdef01",
            "https://institution.example/sso",
        ] {
            receive_message(
                &navigation,
                candidate,
                "x".repeat(MAX_MESSAGE_BYTES + 1),
                &events,
            );
            assert!(inbox.poll().is_none(), "unowned page: {candidate}");
        }
        receive_message(
            &navigation,
            project.url(),
            "x".repeat(MAX_MESSAGE_BYTES + 1),
            &events,
        );
        let error = inbox
            .poll()
            .expect("an oversized owned message must end the stream")
            .unwrap_err();
        assert!(error.contains("16 MiB"));
        assert!(
            events
                .send(
                    serde_json::json!({
                        "event": "project", "epoch": 7,
                        "project_id": "0123456789abcdef01234567",
                        "name": "Stale publication", "entries": [],
                    })
                    .to_string()
                )
                .is_err()
        );
        assert!(
            inbox.poll().is_none(),
            "later Project cannot revive the failed stream"
        );
    }

    #[test]
    fn committed_project_navigation_invalidates_departures_without_enabling_on_return() {
        let project = crate::overleaf::ProjectUrl::parse(
            "https://www.overleaf.com/project/0123456789abcdef01234567",
        )
        .unwrap();
        let mut navigation = ProjectNavigation::new(project.url()).unwrap();
        assert!(
            !navigation.is_active(),
            "commands wait for an actual committed page"
        );
        assert!(!navigation.started(project.url()));
        assert!(navigation.is_active());
        for candidate in [
            "https://www.overleaf.com/project/0123456789abcdef01234567/?doc=abcdef0123456789abcdef01",
            "https://www.overleaf.com/project/0123456789abcdef01234567#editor",
        ] {
            assert!(!navigation.started(candidate), "same project: {candidate}");
            assert!(navigation.is_active());
        }
        for candidate in [
            "https://www.overleaf.com/login",
            "https://institution.example/sso",
            "https://www.overleaf.com/project/abcdef0123456789abcdef01",
            "https://www.overleaf.com/project/0123456789abcdef01234567/download",
            "https://www.overleaf.com/project/0123456789abcdef012345670",
            "https://www.overleaf.com:8443/project/0123456789abcdef01234567",
            "https://www.overleaf.com.evil.example/project/0123456789abcdef01234567",
            "https://user@www.overleaf.com/project/0123456789abcdef01234567",
            "about:blank",
            "not a URL",
        ] {
            assert!(navigation.started(candidate), "departure: {candidate}");
            assert!(!navigation.is_active());
            assert!(
                !navigation.started(candidate),
                "do not flood offline events"
            );
            assert!(
                !navigation.started(project.url()),
                "return never emits connected"
            );
            assert!(navigation.is_active());
        }
    }

    #[test]
    fn committed_local_fixture_navigation_checks_exact_origin_and_project_route() {
        let project = crate::overleaf::ProjectUrl::parse(
            "http://127.0.0.1:43210/project/0123456789abcdef01234567",
        )
        .unwrap();
        let mut navigation = ProjectNavigation::new(project.url()).unwrap();
        assert!(!navigation.started(project.url()));
        assert!(navigation.is_active());
        assert!(navigation.started("http://127.0.0.1:43211/project/0123456789abcdef01234567"));
        assert!(!navigation.is_active());
        assert!(!navigation.started(
            "http://127.0.0.1:43210/project/0123456789abcdef01234567?doc=abcdef0123456789abcdef01"
        ));
        assert!(navigation.is_active());
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
