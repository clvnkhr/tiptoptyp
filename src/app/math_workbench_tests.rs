use super::*;
use egui_kittest::{
    Harness,
    kittest::{Queryable as _, by},
};

fn key() -> DocumentKey {
    DocumentKey {
        owner: tiptoptyp_core::document::WindowSessionId::new(1),
        epoch: 1,
        revision: 2,
    }
}

fn dialog(source: &str) -> TableEditorDialog {
    let table = crate::editor_features::math_table_at(
        source,
        source
            .find("mat(")
            .or_else(|| source.find("cases("))
            .unwrap()
            + 2,
    )
    .unwrap();
    TableEditorDialog::Math(Box::new(MathDialog {
        original: table.source.clone(),
        table,
        key: key(),
        error: None,
        validation: None,
    }))
}

#[test]
fn math_workbench_keeps_formulas_editable_and_tabs_between_cells_at_minimum_size() {
    let mut ready = false;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(620.0, 420.0))
        .build_ui_state(
            move |ui, state| {
                if !ready {
                    theme::configure_test_fonts(ui.ctx());
                    ready = true;
                    return;
                }
                show(ui, state, 600.0, 380.0, &AppSettings::default());
            },
            dialog("$mat(x, alpha; beta, frac(1, 2))$"),
        );
    harness.run();
    let input = by().role(egui::accesskit::Role::MultilineTextInput);
    assert_eq!(harness.query_all(input.clone()).count(), 4);
    harness.get(input.clone().value("x")).click();
    harness.run();
    harness.key_press(egui::Key::Tab);
    harness.run();
    assert!(harness.get(input.clone().value("alpha")).is_focused());
    harness.get(input.value("alpha")).type_text(" + 1");
    harness.run();
    let TableEditorDialog::Math(draft) = harness.state() else {
        unreachable!()
    };
    assert!(draft.table.cells[0][1].contains(" + 1"));
    assert!(draft.validation.is_none());
    for label in ["Apply", "Cancel"] {
        let rect = harness.get_by_label(label).rect();
        assert!(
            rect.left() >= 0.0 && rect.right() <= 620.0 && rect.bottom() <= 420.0,
            "{rect:?}"
        );
    }
}

#[test]
fn math_workbench_apply_is_unicode_safe_and_checks_the_original_revision() {
    let source = "Préface α🙂\n$mat(x, alpha; beta, frac(1, 2))$\nAfter.";
    let mut draft = dialog(source);
    let TableEditorDialog::Math(math) = &mut draft else {
        unreachable!()
    };
    math.table.cells[0][0] = "sqrt(x^2 + y^2)".into();
    let edit = prepare_source_edit(source, key(), &draft).unwrap();
    let mut changed = source.to_owned();
    changed.replace_range(edit.byte_range, &edit.replacement);
    assert_eq!(
        changed,
        source.replacen("mat(x,", "mat(sqrt(x^2 + y^2),", 1)
    );
    assert!(prepare_source_edit(&source.replace("mat(x,", "mat(y,"), key(), &draft).is_err());
    assert!(
        prepare_source_edit(
            source,
            DocumentKey {
                revision: 3,
                ..key()
            },
            &draft
        )
        .is_err()
    );
    let TableEditorDialog::Math(math) = &mut draft else {
        unreachable!()
    };
    math.table.cells[0][0] = "x, outside".into();
    assert!(prepare_source_edit(source, key(), &draft).is_err());
}

#[test]
fn math_workbench_highlighting_keeps_bare_unicode_formula_ranges() {
    let context = egui::Context::default();
    theme::configure_test_fonts(&context);
    context
        .run_ui(Default::default(), |ui| {
            let source = "alpha + β + frac(1, 2)";
            for language in ["typ", "tex"] {
                let job = math_cell_highlight(
                    ui,
                    ui.id().with(language),
                    source,
                    language,
                    true,
                    &AppSettings::default(),
                );
                assert_eq!(job.text, source);
                assert!(!job.sections.is_empty());
                for section in job.sections {
                    assert!(
                        source
                            .get(section.byte_range.start.0..section.byte_range.end.0)
                            .is_some()
                    );
                }
            }
        })
        .drop_without_applying_deltas();
}

#[test]
fn math_workbench_virtualizes_large_grids_and_refuses_invalid_resize_without_mutation() {
    let source = format!(
        "$mat({})$",
        (0..400)
            .map(|i| format!("x_{i}, 0"))
            .collect::<Vec<_>>()
            .join("; ")
    );
    let draft = dialog(&source);
    let TableEditorDialog::Math(math) = &draft else {
        unreachable!()
    };
    let before = math.table.source_edit().unwrap();
    assert!(resize_typst_math(&math.table, 4096, 128).is_err());
    assert_eq!(math.table.source_edit().unwrap(), before);
    let mut ready = false;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(620.0, 420.0))
        .build_ui_state(
            move |ui, state| {
                if !ready {
                    theme::configure_test_fonts(ui.ctx());
                    ready = true;
                    return;
                }
                show(ui, state, 600.0, 380.0, &AppSettings::default());
            },
            draft,
        );
    harness.run();
    let fields = harness
        .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
        .count();
    assert!(fields > 0 && fields < 30, "{fields}");
    assert!(harness.get_by_label("Apply").rect().bottom() <= 420.0);
}

#[test]
fn math_workbench_does_not_clamp_existing_cases_alignment_columns_on_paint() {
    let source = r#"$cases(x & x > 0 & 1, -x & x < 0 & 2)$"#;
    let mut ready = false;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(620.0, 420.0))
        .build_ui_state(
            move |ui, state| {
                if !ready {
                    theme::configure_test_fonts(ui.ctx());
                    ready = true;
                    return;
                }
                show(ui, state, 600.0, 380.0, &AppSettings::default());
            },
            dialog(source),
        );
    harness.run();
    let TableEditorDialog::Math(draft) = harness.state() else {
        unreachable!()
    };
    assert_eq!(draft.table.columns, 3);
    assert_eq!(
        draft.table.source_edit().unwrap().replacement,
        "cases(x & x > 0 & 1, -x & x < 0 & 2)"
    );
}
