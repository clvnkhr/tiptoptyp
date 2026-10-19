//! Source layout must stay cached through real animation frames and invalidate safely.
use super::*;
use crate::settings::{ActionEffectSettings, EditorAction, ParticleStyle};
use egui_kittest::Harness;

fn fixture() -> Harness<'static, EditorApp> {
    let mut harness = editor_particles::tests::editor(false);
    let source = format!(
        "prefix \n#let amplitude(x) = {{\n  let y = x + 1\n  y * 2\n}}\n\nneedle {} tail\n",
        "ordinary prose with a sufficiently long wrapped line ".repeat(30),
    );
    let app = harness.state_mut();
    app.settings.line_wrap = true;
    app.settings.line_numbers = true;
    app.settings.sticky_context_rows = true;
    for action in EditorAction::ALL {
        *app.settings.editor_effects.actions.get_mut(action) = ActionEffectSettings {
            particles: None,
            shake: false,
            flash: false,
            glow: false,
        };
    }
    app.settings.editor_effects.actions.typing.particles = Some(ParticleStyle::Plasma);
    app.document_mut().replace_unprojected_untitled(&source);
    app.pending_editor_selection = Some(EditorSelection::Focus(7..7));
    harness.run_steps(3);
    assert_current_text(&harness);
    harness
}

fn snapshot(harness: &Harness<'_, EditorApp>) -> editor_scroll::Snapshot {
    let id = source_editor_id(&harness.ctx).with("resize-anchor");
    harness
        .ctx
        .data(|data| data.get_temp::<editor_scroll::Snapshot>(id))
        .expect("production source editor publishes its painted galley")
}

fn assert_current_text(harness: &Harness<'_, EditorApp>) {
    let painted = snapshot(harness);
    let source = harness.state().document().source();
    assert_eq!(painted.galley.job.text, *source);
    assert_eq!(painted.galley.end().index.0, source.chars().count());
}

#[test]
fn source_animation_frames_reuse_row_indexes_and_keep_paragraph_layouts_warm() {
    let mut harness = fixture();
    let before = snapshot(&harness);
    harness
        .input_mut()
        .events
        .push(egui::Event::Text("x".into()));
    harness.step();
    assert!(
        harness
            .state()
            .document()
            .source()
            .starts_with("prefix x\n")
    );
    assert_current_text(&harness);
    assert!(!Arc::ptr_eq(&before.galley, &snapshot(&harness).galley));
    let effects = editor_particles::inspected(
        &harness.ctx,
        source_editor_id(&harness.ctx),
        harness.state().document().key(),
    );
    assert_eq!(effects["burst_count"], 1);
    assert!(effects["active_count"].as_u64().unwrap() > 0);
    // Settle any layout follow-up before measuring the still-live animation.
    harness.run_steps(2);
    let prepared = snapshot(&harness);
    let rows = harness.state_mut().source_rows.rows(&prepared.galley);
    harness.run_steps(12);
    assert_current_text(&harness);
    let current = snapshot(&harness);
    assert!(Arc::ptr_eq(&prepared.galley, &current.galley));
    let reused = harness.state_mut().source_rows.rows(&current.galley);
    assert!(Arc::ptr_eq(&rows.logical, &reused.logical));
    assert!(Arc::ptr_eq(&rows.sticky, &reused.sticky));
    let effects = editor_particles::inspected(
        &harness.ctx,
        source_editor_id(&harness.ctx),
        harness.state().document().key(),
    );
    assert_eq!(effects["burst_count"], 1);
    assert!(effects["active_count"].as_u64().unwrap() > 0);
    // A later edit must still reuse egui's unchanged paragraph layouts.
    let first_row = Arc::clone(&current.galley.rows[0].row);
    let end = harness.state().document().source().chars().count();
    harness.state_mut().pending_editor_selection = Some(EditorSelection::Focus(end..end));
    harness.step();
    harness
        .input_mut()
        .events
        .push(egui::Event::Text("y".into()));
    harness.step();
    assert_current_text(&harness);
    assert!(Arc::ptr_eq(
        &first_row,
        &snapshot(&harness).galley.rows[0].row
    ));
}

#[test]
fn source_layout_refreshes_after_a_font_atlas_reset_without_editor_settings_changes() {
    let mut harness = fixture();
    let old = snapshot(&harness);
    let font_data = harness.ctx.fonts(|fonts| {
        fonts
            .definitions()
            .font_data
            .values()
            .next()
            .expect("fixture has a valid font")
            .as_ref()
            .clone()
    });
    // An unused valid font resets egui's atlas without changing the selected
    // code font, source, wrap width, palette, or application settings.
    harness.ctx.add_font(egui::epaint::text::FontInsert::new(
        "source-layout-atlas-reset",
        font_data,
        Vec::new(),
    ));
    harness.step();
    assert_current_text(&harness);
    assert!(
        !Arc::ptr_eq(&old.galley, &snapshot(&harness).galley),
        "retained galleys must not keep texture coordinates from a reset atlas"
    );
}

#[test]
fn source_layout_refreshes_for_wrapping_search_rainbow_and_fold_geometry() {
    let mut harness = fixture();
    let wrapped = snapshot(&harness);
    harness.state_mut().settings.line_wrap = false;
    harness.step();
    let unwrapped = snapshot(&harness);
    assert!(unwrapped.galley.rows.len() < wrapped.galley.rows.len());
    assert_current_text(&harness);

    harness.state_mut().settings.line_wrap = true;
    harness.set_size(egui::vec2(410.0, 500.0));
    harness.step();
    assert!(snapshot(&harness).galley.rows.len() > wrapped.galley.rows.len());
    assert_current_text(&harness);

    let plain = snapshot(&harness);
    harness.state_mut().find_bar.visible = true;
    harness.state_mut().find_bar.query = "needle".into();
    harness.step();
    let searched = snapshot(&harness);
    assert_ne!(plain.galley.job.sections, searched.galley.job.sections);
    assert_current_text(&harness);
    harness.state_mut().find_bar.visible = false;
    harness.step();
    assert_ne!(
        searched.galley.job.sections,
        snapshot(&harness).galley.job.sections
    );

    let before_rainbow = snapshot(&harness);
    let old = harness.state().settings.rainbow_brackets.enabled;
    harness.state_mut().settings.rainbow_brackets.enabled = !old;
    harness.step();
    assert_ne!(
        before_rainbow.galley.job.sections,
        snapshot(&harness).galley.job.sections
    );
    assert_current_text(&harness);

    assert!(!harness.state_mut().folding_mut().regions.is_empty());
    harness.state_mut().folding_mut().collapse_all();
    harness.step();
    assert!(
        snapshot(&harness)
            .galley
            .rows
            .iter()
            .any(|row| row.size.y == 0.0)
    );
    assert_current_text(&harness);
    harness.state_mut().folding_mut().expand_all();
    harness.step();
    assert!(
        snapshot(&harness)
            .galley
            .rows
            .iter()
            .all(|row| row.size.y > 0.0)
    );
    assert_current_text(&harness);
}
