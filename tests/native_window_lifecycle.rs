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
    canceled: usize,
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
    if let Some(previous) = state.identities.insert(id, identity) {
        assert_eq!(
            identity, previous,
            "focus, hide and minimize must retain native identity"
        );
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
        assert_eq!(window.has_shadow(), !popup);
    }
}
impl eframe::App for Fixture {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let mut state = self.state.lock().unwrap();
        if self.started.elapsed() > Duration::from_secs(20) {
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
            0 if state.identities.len() == 4 => {
                let unique: std::collections::HashSet<_> = state.identities.values().collect();
                assert_eq!(unique.len(), 4, "native windows must not share ownership");
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
                document.unwrap().focus_window();
                self.phase = 3;
            }
            3 if document.as_ref().is_some_and(|w| w.has_focus()) => {
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
        window_host::flush_focus(ctx);
        if input.viewport_id == child(1) && input.viewport().close_requested() {
            window_host::observe_close(ctx, child(1), child(1));
        }
    }
    fn persist_egui_memory(&self) -> bool {
        false
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
        "Native contract passed: distinct owners, stable identities, minimize/restore, focus handoff, minimized close/cancel/commit, Settings hide/reopen, native decorations/buttons/shadow"
    );
}
