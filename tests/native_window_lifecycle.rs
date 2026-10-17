//! Opt-in real-window contract test. Run with:
//! cargo test --features native-ui-tests --test native_window_lifecycle
//! A desktop/WindowServer is required; this target never silently skips.
#![allow(dead_code, unused_imports)]
#[path = "../src/native_window.rs"]
mod native_window;
#[path = "../src/viewport_fonts.rs"]
mod viewport_fonts;
#[path = "../src/window_host.rs"]
mod window_host;
#[path = "../src/window_policy.rs"]
mod window_policy;

use eframe::egui;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn child(n: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("native-contract", n))
}
#[derive(Default)]
struct State {
    identities: HashMap<egui::ViewportId, u64>,
    #[cfg(target_os = "macos")]
    blurs: HashMap<egui::ViewportId, native_window::NativeBlur>,
    canceled: usize,
    popup_presented: bool,
    allow_close: bool,
    document_closed: bool,
    settings_visible: bool,
    done: bool,
    failure: Option<String>,
}
struct Fixture {
    state: Arc<Mutex<State>>,
    started: Instant,
    phase: u8,
    phase_started: Instant,
}

fn inspect(context: &egui::Context, id: egui::ViewportId, popup: bool, state: &mut State) {
    let window = eframe::window_host::window(context, id).expect("callback has its native binding");
    let identity = eframe::window_host::identity(context, id).unwrap();
    let previous_identity = state.identities.insert(id, identity);
    if let Some(previous) = previous_identity {
        assert_eq!(
            identity, previous,
            "focus, hide and minimize must retain native identity"
        );
    }
    if popup {
        if !state.popup_presented && window.has_focus() {
            state.failure = Some("inactive popup stole focus when first presented".to_owned());
        }
        let visible = window.is_visible() == Some(true);
        if previous_identity.is_none() && visible {
            state.failure = Some("popup was visible before its first buffer swap".to_owned());
        }
        state.popup_presented |= visible;
    }
    assert_eq!(window.is_decorated(), !popup);
    assert_eq!(
        window
            .enabled_buttons()
            .contains(winit::window::WindowButtons::CLOSE),
        !popup
    );
    assert_eq!(
        window
            .enabled_buttons()
            .contains(winit::window::WindowButtons::MINIMIZE),
        !popup
    );
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::WindowExtMacOS;
        // AppKit intentionally removes shadows in fullscreen, including while
        // the transition is in flight. Verify the window policy at creation;
        // the fullscreen journey checks restoration after returning to desktop.
        if popup || previous_identity.is_none() {
            assert_eq!(window.has_shadow(), !popup);
        }
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::AppKit(handle) = window.window_handle().unwrap().as_raw() else {
            panic!("expected AppKit window");
        };
        // SAFETY: this callback runs on the main thread and holds the live window.
        let view = unsafe { handle.ns_view.cast::<objc2_app_kit::NSView>().as_ref() };
        assert!(
            !view.window().unwrap().isOpaque(),
            "all macOS surfaces retain alpha capability"
        );
        if !popup && previous_identity.is_none() {
            let parent = native_window::NativeWindowHandle::from_owner(window.as_ref()).unwrap();
            let content = view.window().unwrap().contentView().unwrap();
            let children = view.subviews();
            assert!(parent.set_translucent_background());
            assert!(parent.set_translucent_background());
            assert_eq!(
                view.subviews(),
                children,
                "native backgrounds must not cover CGL output with decoration children"
            );
            assert_eq!(
                objc2::rc::Retained::as_ptr(&content),
                objc2::rc::Retained::as_ptr(&view.window().unwrap().contentView().unwrap()),
                "background configuration cannot replace the rendering view"
            );
            let blur = parent
                .blur_background(native_window::BlurConfig {
                    radius: 8,
                    end_radius: 0,
                    fade: native_window::FadeDirection::None,
                })
                .expect("native compositor blur");
            if view.window().unwrap().isVisible() {
                assert!(blur.synchronize(&parent, None));
                assert_eq!(blur.inspection(&parent).unwrap().0, 8);
            } else {
                assert!(blur.awaiting_visibility());
            }
            assert_eq!(view.subviews(), children, "blur cannot be a renderer child");
            assert!(
                window.has_shadow(),
                "compositor blur preserves window shadow"
            );
            state.blurs.insert(id, blur);
            assert_eq!(eframe::window_host::identity(context, id), Some(identity));
        }
        if let Some(blur) = state.blurs.get(&id) {
            let parent = native_window::NativeWindowHandle::from_owner(window.as_ref()).unwrap();
            assert!(blur.synchronize(&parent, None));
            if view.window().unwrap().isVisible() {
                assert_eq!(blur.inspection(&parent).unwrap().0, 8);
            }
        }
    }
}
impl eframe::App for Fixture {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let mut state = self.state.lock().unwrap();
        if state.failure.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.started.elapsed() > Duration::from_secs(30) {
            state.failure = Some(format!(
                "native lifecycle timed out in phase {}; native state {:?}; app active {:?}",
                self.phase,
                [egui::ViewportId::ROOT, child(1), child(2), child(3)].map(|id| {
                    eframe::window_host::window(ctx, id).map(|window| {
                        (
                            window.is_visible(),
                            window.is_minimized(),
                            window.has_focus(),
                        )
                    })
                }),
                native_window::application_active(ctx)
            ));
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(40));
        let root = eframe::window_host::window(ctx, egui::ViewportId::ROOT).unwrap();
        let document = eframe::window_host::window(ctx, child(1));
        let settings = eframe::window_host::window(ctx, child(2));
        match self.phase {
            0 if state.identities.len() == 4 && state.popup_presented => {
                let unique: std::collections::HashSet<_> = state.identities.values().collect();
                assert_eq!(unique.len(), 4, "native windows must not share ownership");
                let _ = root.request_inner_size(winit::dpi::LogicalSize::new(640.0, 380.0));
                root.set_minimized(true);
                self.phase_started = Instant::now();
                self.phase = 1;
            }
            1 if root.is_minimized() == Some(true)
                && self.phase_started.elapsed() > Duration::from_millis(600) =>
            {
                root.set_minimized(false);
                self.phase = 2;
            }
            2 if root.is_minimized() == Some(false) => {
                window_host::focus(
                    ctx,
                    egui::ViewportId::ROOT,
                    window_host::FocusCause::UserAction,
                );
                self.phase = 20;
            }
            20 if root.has_focus() => {
                root.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                self.phase_started = Instant::now();
                self.phase = 21;
            }
            21 if root.fullscreen().is_some()
                && self.phase_started.elapsed() > Duration::from_secs(2) =>
            {
                root.set_fullscreen(None);
                self.phase_started = Instant::now();
                self.phase = 22;
            }
            22 if root.fullscreen().is_none()
                && self.phase_started.elapsed() > Duration::from_secs(2) =>
            {
                #[cfg(target_os = "macos")]
                {
                    use winit::platform::macos::WindowExtMacOS;
                    assert!(root.has_shadow(), "window shadow returns after fullscreen");
                }
                document.unwrap().focus_window();
                self.phase = 3;
            }
            3 if document.as_ref().is_some_and(|w| w.has_focus()) => {
                window_host::focus(ctx, child(2), window_host::FocusCause::UserAction);
                self.phase = 30;
            }
            30 if settings.as_ref().is_some_and(|w| w.has_focus()) => {
                state.settings_visible = false;
                ctx.send_viewport_cmd_to(child(2), egui::ViewportCommand::Visible(false));
                window_host::return_from_closed_window(ctx, child(2));
                self.phase = 31;
            }
            31 if document.as_ref().is_some_and(|w| w.has_focus()) => {
                document.unwrap().set_minimized(true);
                self.phase = 4;
            }
            4 if document
                .as_ref()
                .is_some_and(|w| w.is_minimized() == Some(true)) =>
            {
                ctx.send_viewport_cmd_to(child(1), egui::ViewportCommand::Close);
                self.phase = 5;
            }
            5 if state.canceled == 1 => {
                assert!(document.is_some(), "cancel retains minimized document");
                state.allow_close = true;
                ctx.send_viewport_cmd_to(child(1), egui::ViewportCommand::Close);
                self.phase = 6;
            }
            6 if state.document_closed && document.is_none() => {
                #[cfg(target_os = "macos")]
                state.blurs.remove(&child(1));
                state.settings_visible = false;
                self.phase = 7;
            }
            7 if settings
                .as_ref()
                .is_some_and(|w| w.is_visible() == Some(false)) =>
            {
                state.settings_visible = true;
                ctx.send_viewport_cmd_to(child(2), egui::ViewportCommand::Visible(true));
                window_host::focus(ctx, child(2), window_host::FocusCause::UserAction);
                self.phase = 8;
            }
            8 if settings
                .as_ref()
                .is_some_and(|w| w.is_visible() == Some(true) && w.has_focus()) =>
            {
                state.settings_visible = false;
                ctx.send_viewport_cmd_to(child(2), egui::ViewportCommand::Visible(false));
                window_host::return_from_closed_window(ctx, child(2));
                self.phase = 9;
            }
            9 if root.has_focus() => {
                state.done = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            _ => {}
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        window_host::ensure(ctx, egui::ViewportId::ROOT, egui::ViewportId::ROOT);
        inspect(
            ctx,
            egui::ViewportId::ROOT,
            false,
            &mut self.state.lock().unwrap(),
        );
        ui.label("Isolated window lifecycle test");
        for n in 1..=3 {
            let state = self.state.lock().unwrap();
            if n == 1 && state.document_closed {
                continue;
            }
            let visible = n != 2 || state.settings_visible;
            drop(state);
            let id = child(n);
            let popup = n == 3;
            window_host::show(
                ctx,
                id,
                if n == 1 { id } else { egui::ViewportId::ROOT },
                visible,
                n == 2,
            );
            let builder = if popup {
                window_policy::popup("Native test popup").with_active(false)
            } else {
                window_policy::document().with_title(format!("Native test window {n}"))
            };
            let shared = self.state.clone();
            viewport_fonts::show_deferred(
                ctx,
                id,
                builder
                    .with_inner_size([320.0, 200.0])
                    .with_visible(visible),
                move |ui, _| {
                    let mut state = shared.lock().unwrap();
                    inspect(ui.ctx(), id, popup, &mut state);
                    if n == 1 && ui.ctx().input(|input| input.viewport().close_requested()) {
                        if !state.allow_close {
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                            state.canceled += 1;
                        } else {
                            state.document_closed = true;
                            window_host::transition(
                                ui.ctx(),
                                id,
                                window_host::Lifecycle::DurablyClosed,
                            );
                        }
                        window_host::acknowledge_close(ui.ctx(), id);
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    ui.label(format!("Owner {n}"));
                },
            );
        }
    }
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        window_host::observe_focus(ctx, input);
        window_host::flush_focus(ctx);
        if input.viewport_id == child(1) && input.viewport().close_requested() {
            window_host::observe_close(ctx, child(1), child(1));
        }
    }
    fn persist_egui_memory(&self) -> bool {
        false
    }
}
#[cfg(target_os = "macos")]
fn alpha_context_error() -> Option<String> {
    use std::ffi::c_void;
    #[link(name = "OpenGL", kind = "framework")]
    unsafe extern "C" {
        fn CGLGetCurrentContext() -> *mut c_void;
        fn CGLGetParameter(context: *mut c_void, parameter: u32, value: *mut i32) -> i32;
    }
    // SAFETY: eframe makes the root GL context current before its app creator.
    // 236 is Apple's kCGLCPSurfaceOpacity; the output points to a live GLint.
    unsafe {
        let context = CGLGetCurrentContext();
        if context.is_null() {
            return Some("native test requires a current CGL context".to_owned());
        }
        let mut opacity = -1;
        let error = CGLGetParameter(context, 236, &mut opacity);
        (error != 0 || opacity != 0).then(|| {
            format!(
                "opaque root disabled shared renderer alpha: CGL error={error}, opacity={opacity}"
            )
        })
    }
}

fn main() {
    let state = Arc::new(Mutex::new(State {
        settings_visible: true,
        ..Default::default()
    }));
    let shared = state.clone();
    eframe::run_native(
        "tiptoptyp native window contract",
        eframe::NativeOptions {
            viewport: window_policy::document().with_inner_size([480.0, 300.0]),
            persist_window: false,
            ..Default::default()
        },
        Box::new(move |_| {
            #[cfg(target_os = "macos")]
            {
                shared.lock().unwrap().failure = alpha_context_error();
            }
            native_window::activate_application();
            Ok(Box::new(Fixture {
                state: shared,
                started: Instant::now(),
                phase: 0,
                phase_started: Instant::now(),
            }))
        }),
    )
    .expect("native desktop required");
    let state = state.lock().unwrap();
    if let Some(path) = std::env::var_os("TIPTOPTYP_NATIVE_TEST_REPORT") {
        std::fs::write(
            path,
            format!(
                "completed={} cancellations={} closed={} failure={:?}\n",
                state.done, state.canceled, state.document_closed, state.failure
            ),
        )
        .expect("write requested native test report");
    }
    assert!(state.failure.is_none(), "{:?}", state.failure);
    assert!(state.done, "native lifecycle did not complete");
    assert_eq!(state.canceled, 1);
    assert!(state.document_closed);
    println!(
        "Native contract passed: distinct owners, stable identities, compositor blur, fullscreen, minimize/restore, focus handoff, minimized close/cancel/commit, Settings hide/reopen, native decorations/buttons/shadow"
    );
}
