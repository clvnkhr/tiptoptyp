//! Per-action effects exercised through the real source and editable-diff widgets.
use super::*;
use crate::settings::{ActionEffectSettings, EditorAction, ParticleStyle};
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
