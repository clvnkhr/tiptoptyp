//! Per-action effects exercised through the real source and editable-diff widgets.
use super::*;
use crate::settings::{ActionEffectSettings, EditorAction, LingeringStyle, ParticleStyle};
use egui_kittest::Harness;

fn fixture(diff: bool) -> Harness<'static, EditorApp> {
    let mut harness = editor_particles::tests::editor(diff);
    for action in EditorAction::ALL {
        *harness
            .state_mut()
            .settings
            .editor_effects
            .actions
            .get_mut(action) = off();
    }
    harness
}

fn off() -> ActionEffectSettings {
    ActionEffectSettings {
        lingering: None,
        particles: None,
        shake: false,
        flash: false,
        glow: false,
    }
}

fn observe(harness: &Harness<'_, EditorApp>) -> serde_json::Value {
    editor_particles::inspected(
        &harness.ctx,
        source_editor_id(&harness.ctx),
        harness.state().document().key(),
    )
}

fn type_character(harness: &mut Harness<'_, EditorApp>) {
    harness
        .input_mut()
        .events
        .push(egui::Event::Text("x".into()));
    harness.step();
}

#[test]
fn actual_typing_copy_and_comment_use_distinct_particle_styles_in_both_editors() {
    for diff in [false, true] {
        let mut harness = fixture(diff);
        for (action, style) in [
            (EditorAction::Typing, ParticleStyle::Plasma),
            (EditorAction::Copy, ParticleStyle::Shotgun),
            (EditorAction::Comment, ParticleStyle::Railgun),
        ] {
            harness
                .state_mut()
                .settings
                .editor_effects
                .actions
                .get_mut(action)
                .particles = Some(style);
        }
        type_character(&mut harness);
        let typed = observe(&harness);
        assert_eq!(typed["last_style"], "Plasma");
        assert_eq!(typed["last_action"], "Typing");
        assert_eq!(typed["burst_count"], 1);
        assert_eq!(harness.state().document().source(), "prefix x");

        harness.state_mut().pending_editor_selection = Some(EditorSelection::Focus(0..6));
        harness.step();
        harness.input_mut().events.push(egui::Event::Copy);
        harness.step();
        let copied = observe(&harness);
        assert_eq!(copied["last_style"], "Shotgun");
        assert_eq!(copied["last_action"], "Copy");
        assert_eq!(copied["burst_count"], 2);
        assert!(copied["live_styles"]["Plasma"].as_u64().unwrap_or(0) > 0);
        assert!(copied["live_styles"]["Shotgun"].as_u64().unwrap_or(0) > 0);
        assert_eq!(harness.state().document().source(), "prefix x");
        assert!(harness.output().platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "prefix")
        ));

        let context = harness.ctx.clone();
        harness.state_mut().toggle_comments(&context);
        harness.step();
        let commented = observe(&harness);
        assert_eq!(commented["last_style"], "Railgun");
        assert_eq!(commented["last_action"], "Comment");
        assert_eq!(commented["burst_count"], 3);
        for style in ["Plasma", "Shotgun", "Railgun"] {
            assert!(commented["live_styles"][style].as_u64().unwrap_or(0) > 0);
        }
        assert_eq!(harness.state().document().source(), "// prefix x");
        assert_eq!(commented["feedback"]["burst_count"], 0);
    }
}

#[test]
fn each_non_particle_effect_runs_without_particles_even_when_the_gpu_is_unavailable() {
    for diff in [false, true] {
        for name in ["Shake", "Flash", "Glow"] {
            let mut harness = fixture(diff);
            let effects = &mut harness.state_mut().settings.editor_effects.actions.typing;
            effects.shake = name == "Shake";
            effects.flash = name == "Flash";
            effects.glow = name == "Glow";
            editor_particles::Renderer::for_context(&harness.ctx).simulate_unavailable_for_tests();
            type_character(&mut harness);
            harness.step();
            let state = observe(&harness);
            assert_eq!(harness.state().document().source(), "prefix x");
            assert_eq!(state["active_count"], 0);
            assert_eq!(state["burst_count"], 0);
            assert_eq!(state["feedback"]["burst_count"], 1);
            assert_eq!(state["feedback"]["last_action"], "Typing");
            assert_eq!(
                state["feedback"]["active_effects"],
                serde_json::json!([name])
            );
            assert_eq!(state["feedback"]["last_effects"], serde_json::json!([name]));
            let shake = state["feedback"]["shake_offset"].as_array().unwrap();
            let magnitude = shake
                .iter()
                .map(|value| value.as_f64().unwrap().abs())
                .sum::<f64>();
            assert_eq!(magnitude > 0.0, name == "Shake");
            assert_eq!(
                state["feedback"]["flash_alpha"].as_f64().unwrap() > 0.0,
                name == "Flash"
            );
            assert_eq!(
                state["feedback"]["glow_alpha"].as_f64().unwrap() > 0.0,
                name == "Glow"
            );
            assert!(
                harness
                    .output()
                    .shapes
                    .iter()
                    .all(|shape| !matches!(shape.shape, egui::Shape::Callback(_)))
            );
        }
    }
}

#[test]
fn no_op_commands_and_late_receipts_after_settings_takes_focus_do_not_emit_effects() {
    for diff in [false, true] {
        for event in [
            egui::Event::Copy,
            egui::Event::Cut,
            egui::Event::Paste(String::new()),
            egui::Event::Key {
                key: egui::Key::Delete,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            let mut harness = fixture(diff);
            for action in EditorAction::ALL {
                *harness
                    .state_mut()
                    .settings
                    .editor_effects
                    .actions
                    .get_mut(action) = ActionEffectSettings {
                    shake: true,
                    flash: true,
                    glow: true,
                    ..off()
                };
            }
            harness.input_mut().events.push(event);
            harness.step();
            assert_eq!(harness.state().document().source(), "prefix ");
            assert_eq!(observe(&harness)["feedback"]["burst_count"], 0);
            let context = harness.ctx.clone();
            harness.state_mut().undo_editor(&context, false);
            harness.step();
            assert_eq!(observe(&harness)["feedback"]["burst_count"], 0);
        }
        let mut harness = fixture(diff);
        harness
            .state_mut()
            .settings
            .editor_effects
            .actions
            .copy
            .glow = true;
        harness.state_mut().pending_editor_selection = Some(EditorSelection::Focus(0..6));
        harness.step();
        let context = harness.ctx.clone();
        harness.state_mut().copy_editor_selection(&context);
        let settings = scoped_child_viewport_id(&context, "tiptoptyp-settings");
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(false);
        harness.input_mut().viewports.insert(
            settings,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                focused: Some(true),
                ..Default::default()
            },
        );
        harness.step();
        assert_eq!(focused_input_viewport(&harness.ctx), settings);
        assert_eq!(observe(&harness)["feedback"]["burst_count"], 0);
        assert_eq!(harness.state().document().source(), "prefix ");
    }
}

#[test]
fn disabling_one_action_preserves_other_actions_and_the_master_switch_clears_feedback() {
    for diff in [false, true] {
        let mut harness = fixture(diff);
        harness
            .state_mut()
            .settings
            .editor_effects
            .actions
            .paste
            .glow = true;
        type_character(&mut harness);
        let typed = observe(&harness);
        assert_eq!(typed["burst_count"], 0);
        assert_eq!(typed["feedback"]["burst_count"], 0);
        harness
            .input_mut()
            .events
            .push(egui::Event::Paste(" pasted".into()));
        harness.step();
        let pasted = observe(&harness);
        assert_eq!(pasted["feedback"]["last_action"], "Paste");
        assert_eq!(pasted["feedback"]["burst_count"], 1);
        assert_eq!(
            pasted["feedback"]["active_effects"],
            serde_json::json!(["Glow"])
        );
        harness.state_mut().settings.editor_effects.enabled = false;
        harness.step();
        assert_eq!(
            observe(&harness)["feedback"]["active_effects"],
            serde_json::json!([])
        );
        type_character(&mut harness);
        assert_eq!(harness.state().document().source(), "prefix x pastedx");
        assert_eq!(observe(&harness)["feedback"]["burst_count"], 1);
    }
}

#[test]
fn non_particle_feedback_expires_and_does_not_request_idle_animation_frames() {
    for diff in [false, true] {
        let mut harness = fixture(diff);
        harness.state_mut().settings.editor_effects.actions.typing = ActionEffectSettings {
            shake: true,
            flash: true,
            glow: true,
            ..off()
        };
        type_character(&mut harness);
        assert_eq!(observe(&harness)["feedback"]["burst_count"], 1);
        harness.run_steps(40);
        let expired = observe(&harness);
        assert_eq!(expired["active_count"], 0);
        assert_eq!(expired["feedback"]["active_effects"], serde_json::json!([]));
        assert_eq!(
            expired["feedback"]["shake_offset"],
            serde_json::json!([0.0, 0.0])
        );
        assert_eq!(expired["feedback"]["flash_alpha"], 0.0);
        assert_eq!(expired["feedback"]["glow_alpha"], 0.0);
        harness.run_steps(3);
        assert_eq!(observe(&harness)["feedback"]["burst_count"], 1);
        assert!(
            harness
                .ctx
                .repaint_causes()
                .iter()
                .all(|cause| !cause.file.ends_with("editor_particles.rs")
                    && !cause.file.ends_with("editor_particles/feedback.rs")),
            "expired effects must not request animation frames"
        );
        assert_eq!(harness.state().document().source(), "prefix x");
    }
}

#[test]
fn a_late_particle_gpu_failure_retires_invisible_sparks_but_preserves_glow() {
    for diff in [false, true] {
        let mut harness = fixture(diff);
        harness.state_mut().settings.editor_effects.actions.typing = ActionEffectSettings {
            particles: Some(ParticleStyle::Plasma),
            glow: true,
            ..off()
        };
        type_character(&mut harness);
        assert!(observe(&harness)["active_count"].as_u64().unwrap() > 0);
        editor_particles::Renderer::for_context(&harness.ctx).simulate_unavailable_for_tests();
        harness.step();
        let current = observe(&harness);
        assert_eq!(current["active_count"], 0);
        assert_eq!(current["feedback"]["burst_count"], 1);
        assert_eq!(
            current["feedback"]["active_effects"],
            serde_json::json!(["Glow"])
        );
        assert!(
            harness
                .output()
                .shapes
                .iter()
                .all(|shape| !matches!(shape.shape, egui::Shape::Callback(_)))
        );
        harness.run_steps(40);
        assert_eq!(
            observe(&harness)["feedback"]["active_effects"],
            serde_json::json!([])
        );
        assert!(
            harness
                .ctx
                .repaint_causes()
                .iter()
                .all(|cause| !cause.file.ends_with("editor_particles.rs"))
        );
    }
}

#[test]
fn cpu_only_and_all_off_actions_never_create_a_particle_renderer() {
    for diff in [false, true] {
        for glow in [false, true] {
            let mut harness = fixture(diff);
            harness
                .state_mut()
                .settings
                .editor_effects
                .actions
                .typing
                .glow = glow;
            // Remove the renderer warmed by the general particle fixture before
            // observing these independently configured real editor frames.
            let gpu_id = egui::Id::new("editor-particle-gpu");
            harness
                .ctx
                .data_mut(|data| data.remove::<editor_particles::Renderer>(gpu_id));
            harness.step();
            type_character(&mut harness);
            assert!(
                harness
                    .ctx
                    .data(|data| data.get_temp::<editor_particles::Renderer>(gpu_id))
                    .is_none(),
                "source/diff CPU feedback and fully Off actions must not initialize or consult a GPU backend"
            );
            assert_eq!(harness.state().document().source(), "prefix x");
            // Inspection intentionally creates a read-only renderer handle, so
            // check its absence before querying the effect trace.
            assert_eq!(
                observe(&harness)["feedback"]["burst_count"],
                u64::from(glow)
            );
        }
    }
}

#[test]
fn disabling_the_last_shake_keeps_the_captured_pulse_until_expiry() {
    for diff in [false, true] {
        let mut harness = fixture(diff);
        harness
            .state_mut()
            .settings
            .editor_effects
            .actions
            .typing
            .shake = true;
        type_character(&mut harness);
        assert_eq!(
            observe(&harness)["feedback"]["last_effects"],
            serde_json::json!(["Shake"])
        );
        harness
            .state_mut()
            .settings
            .editor_effects
            .actions
            .typing
            .shake = false;
        let settings = harness.state().settings.editor_effects;
        let now = harness.ctx.input(|input| input.time);
        for (after, expected) in [(1.0 / 60.0, true), (1.0, false)] {
            let output = harness.ctx.run_ui(egui::RawInput { time: Some(now + after), ..Default::default() }, |ui| {
                assert_eq!(editor_particles::prepare_shake(ui, source_editor_id(ui.ctx()), settings).is_some(), expected,
                    "the actual source/diff action captures its feedback flags until the finite envelope expires");
            });
            output.drop_without_applying_deltas();
        }
    }
}

fn press_navigation(harness: &mut Harness<'_, EditorApp>, key: egui::Key) {
    for pressed in [true, false] {
        harness.input_mut().events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.step();
}

#[test]
fn actual_editor_smoke_outlives_the_primary_burst_and_then_releases_animation() {
    for diff in [false, true] {
        let mut harness = fixture(diff);
        harness.state_mut().settings.editor_effects.actions.typing = ActionEffectSettings {
            particles: Some(ParticleStyle::Plasma),
            lingering: Some(LingeringStyle::Smoke),
            ..off()
        };
        type_character(&mut harness);
        let emitted = observe(&harness);
        assert_eq!(
            emitted["burst_count"], 1,
            "primary and smoke form one action burst"
        );
        assert_eq!(emitted["max_lifetime_ms"], 2800);
        assert_eq!(emitted["last_style"], "Plasma");
        for style in ["Plasma", "Smoke"] {
            assert!(emitted["live_styles"][style].as_u64().unwrap_or(0) > 0);
        }
        // The fixture uses a manual 60 Hz clock. This exercises the real
        // source/diff paint path without sleeping or wall-time assertions.
        harness.run_steps(45);
        let lingering = observe(&harness);
        assert_eq!(lingering["live_styles"]["Plasma"], serde_json::Value::Null);
        assert!(lingering["live_styles"]["Smoke"].as_u64().unwrap_or(0) > 0);
        assert!(lingering["active_count"].as_u64().unwrap() > 0);
        harness.run_steps(130);
        let expired = observe(&harness);
        assert_eq!(expired["active_count"], 0);
        assert_eq!(expired["burst_count"], 1);
        assert_eq!(harness.state().document().source(), "prefix x");
        assert!(
            harness
                .ctx
                .repaint_causes()
                .iter()
                .all(|cause| !cause.file.ends_with("editor_particles.rs")),
            "finite smoke must stop requesting animation frames"
        );
    }
}

#[test]
fn actual_editor_lingering_runs_independently_when_primary_particles_are_off() {
    for diff in [false, true] {
        let mut harness = fixture(diff);
        harness
            .state_mut()
            .settings
            .editor_effects
            .actions
            .typing
            .lingering = Some(LingeringStyle::Embers);
        type_character(&mut harness);
        let emitted = observe(&harness);
        assert_eq!(emitted["last_style"], "Embers");
        assert_eq!(emitted["last_action"], "Typing");
        assert_eq!(emitted["burst_count"], 1);
        assert_eq!(emitted["live_styles"].as_object().unwrap().len(), 1);
        assert!(emitted["live_styles"]["Embers"].as_u64().unwrap() > 0);
        assert_eq!(emitted["feedback"]["burst_count"], 0);
        harness.run_steps(120);
        assert_eq!(observe(&harness)["active_count"], 0);
        assert_eq!(harness.state().document().source(), "prefix x");
    }
}

#[test]
fn unified_effects_apply_to_real_copy_and_comment_without_overwriting_action_preferences() {
    for diff in [false, true] {
        let mut harness = fixture(diff);
        let effects = &mut harness.state_mut().settings.editor_effects;
        effects.same_for_all = true;
        effects.actions.typing.particles = Some(ParticleStyle::Plasma);
        effects.actions.copy.particles = Some(ParticleStyle::Shotgun);
        effects.actions.comment.particles = Some(ParticleStyle::Railgun);
        let independent = effects.actions;
        harness.input_mut().events.push(egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
        harness.step();
        harness.input_mut().events.push(egui::Event::Copy);
        harness.step();
        let copied = observe(&harness);
        assert_eq!(copied["last_action"], "Copy");
        assert_eq!(copied["last_style"], "Plasma");
        assert_eq!(copied["burst_count"], 1);
        assert_eq!(harness.state().document().source(), "prefix ");
        assert!(harness.output().platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "prefix ")
        ));
        let context = harness.ctx.clone();
        harness.state_mut().toggle_comments(&context);
        harness.step();
        let commented = observe(&harness);
        assert_eq!(commented["last_action"], "Comment");
        assert_eq!(commented["last_style"], "Plasma");
        assert_eq!(commented["burst_count"], 2);
        assert_eq!(harness.state().document().source(), "// prefix ");
        assert_eq!(harness.state().settings.editor_effects.actions, independent);
        harness.state_mut().settings.editor_effects.same_for_all = false;
        harness.state_mut().toggle_comments(&context);
        harness.step();
        assert_eq!(observe(&harness)["last_style"], "Railgun");
        assert_eq!(harness.state().document().source(), "prefix ");
        assert_eq!(harness.state().settings.editor_effects.actions, independent);
    }
}

#[test]
fn cursor_only_afterburner_follows_arrow_keys_without_edits_or_action_bursts_and_respects_focus() {
    for diff in [false, true] {
        let mut harness = fixture(diff);
        harness
            .state_mut()
            .settings
            .editor_effects
            .cursor_afterburner = true;
        harness.step();
        assert_eq!(
            observe(&harness)["cursor_bursts"],
            0,
            "enabling establishes a baseline"
        );
        for count in 1..=2 {
            press_navigation(&mut harness, egui::Key::ArrowLeft);
            let moved = observe(&harness);
            assert_eq!(moved["cursor_bursts"], count);
            assert_eq!(moved["burst_count"], 0);
            assert_eq!(moved["last_action"], serde_json::Value::Null);
            assert_eq!(moved["last_style"], serde_json::Value::Null);
            assert_eq!(moved["action_bursts"], serde_json::json!({}));
            assert!(moved["live_styles"]["Afterburner"].as_u64().unwrap_or(0) > 0);
            assert_eq!(harness.state().document().source(), "prefix ");
        }
        harness.run_steps(40);
        assert_eq!(observe(&harness)["active_count"], 0);
        harness
            .state_mut()
            .settings
            .editor_effects
            .cursor_afterburner = false;
        harness.step();
        press_navigation(&mut harness, egui::Key::ArrowRight);
        assert_eq!(observe(&harness)["cursor_bursts"], 2);
        harness
            .state_mut()
            .settings
            .editor_effects
            .cursor_afterburner = true;
        harness.step();
        harness.input_mut().focused = false;
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(false);
        harness.step();
        press_navigation(&mut harness, egui::Key::ArrowLeft);
        assert_eq!(observe(&harness)["cursor_bursts"], 2);
        harness.input_mut().focused = true;
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(true);
        harness.step();
        assert_eq!(
            observe(&harness)["cursor_bursts"],
            2,
            "regaining focus cannot bridge a stale cursor"
        );
        press_navigation(&mut harness, egui::Key::ArrowRight);
        assert_eq!(observe(&harness)["cursor_bursts"], 3);
        harness.state_mut().settings.editor_effects.enabled = false;
        harness.step();
        press_navigation(&mut harness, egui::Key::ArrowLeft);
        let disabled = observe(&harness);
        assert_eq!(disabled["cursor_bursts"], 3);
        assert_eq!(disabled["active_count"], 0);
        assert_eq!(disabled["burst_count"], 0);
        assert_eq!(harness.state().document().source(), "prefix ");
    }
}

#[test]
fn cursor_afterburner_does_not_connect_stale_positions_after_external_text_reflow() {
    for diff in [false, true] {
        let mut harness = fixture(diff);
        harness
            .state_mut()
            .settings
            .editor_effects
            .cursor_afterburner = true;
        harness.step();
        press_navigation(&mut harness, egui::Key::ArrowLeft);
        assert_eq!(observe(&harness)["cursor_bursts"], 1);
        let context = harness.ctx.clone();
        let cursor = harness.state_mut().editor_snapshot(&context).cursor;
        assert_eq!(usize::from(cursor.primary.index), 6);
        let before = harness.state().document().key();
        // An incoming same-length source change moves the existing character
        // to another row while leaving the caret index and document unchanged.
        harness.state_mut().document_mut().edit(cursor, |source| {
            source.replace_range(3..4, "\n");
        });
        harness.step();
        let after = harness.state().document().key();
        assert_eq!(after.epoch, before.epoch);
        assert!(after.revision > before.revision);
        assert_eq!(
            usize::from(
                harness
                    .state_mut()
                    .editor_snapshot(&context)
                    .cursor
                    .primary
                    .index
            ),
            6
        );
        assert_eq!(observe(&harness)["cursor_bursts"], 1);
        press_navigation(&mut harness, egui::Key::ArrowLeft);
        assert_eq!(
            observe(&harness)["cursor_bursts"],
            1,
            "the first movement after reflow establishes a new point instead of connecting stale coordinates"
        );
        press_navigation(&mut harness, egui::Key::ArrowLeft);
        assert_eq!(
            observe(&harness)["cursor_bursts"],
            2,
            "subsequent ordinary movement resumes the afterburner"
        );
        assert_eq!(observe(&harness)["burst_count"], 0);
        assert_eq!(harness.state().document().source(), "pre\nix ");
    }
}
