//! One synchronized scroll surface with a live editable current document.
use super::document_diff::{DocumentDiff, Pair};
use crate::{
    app::{code_editor, icons},
    settings::AppSettings,
    theme,
};
use eframe::egui::{
    self,
    text::{LayoutJob, LayoutSection},
    *,
};
use std::{ops::Range, sync::Arc};

pub(crate) enum Action {
    Close,
    Previous,
    Next,
    Stage,
    Unstage,
    Revert,
    RevertLine(usize),
}
pub(crate) struct Output {
    pub(crate) edit: egui::text_edit::TextEditOutput,
    pub(crate) action: Option<Action>,
}
#[derive(Clone)]
struct Layout {
    old: Arc<Galley>,
    new: Arc<Galley>,
    tops: Vec<f32>,
    heights: Vec<f32>,
    height: f32,
    old_rows: Vec<Range<usize>>,
    new_rows: Vec<Range<usize>>,
}
#[derive(Clone)]
struct LayoutCache {
    old: String,
    new: String,
    extension: String,
    width: f32,
    style: Arc<Style>,
    settings: AppSettings,
    scale: f32,
    mitex_dollars: bool,
    layout: Arc<Layout>,
}

fn line_ranges(source: &str) -> Vec<Range<usize>> {
    let mut offset = 0;
    source
        .split_inclusive('\n')
        .map(|line| {
            let range = offset..offset + line.len();
            offset = range.end;
            range
        })
        .collect()
}
fn changed_range(a: &str, b: &str) -> Range<usize> {
    let prefix = a.chars().zip(b.chars()).take_while(|(a, b)| a == b).count();
    let ac: Vec<_> = a.chars().collect();
    let bc: Vec<_> = b.chars().collect();
    let suffix = ac[prefix..]
        .iter()
        .rev()
        .zip(bc[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let start = a.char_indices().nth(prefix).map_or(a.len(), |(i, _)| i);
    let end = a
        .char_indices()
        .nth(a.chars().count() - suffix)
        .map_or(a.len(), |(i, _)| i);
    start..end
}
fn decorate(job: &mut LayoutJob, other: &str, pairs: &[Pair], old: bool, context: &Context) {
    let ranges = line_ranges(&job.text);
    let other_lines: Vec<_> = other.split_inclusive('\n').collect();
    let palette = theme::palette(context);
    let color = if old { palette.error } else { palette.success };
    let mut sections = Vec::new();
    let mut changes = Vec::new();
    for pair in pairs.iter().filter(|p| p.changed) {
        let line = if old { pair.old } else { pair.new };
        let counterpart = if old { pair.new } else { pair.old };
        if let Some(line) = line {
            let range = ranges[line].clone();
            let changed = counterpart.map_or(0..range.len(), |i| {
                changed_range(&job.text[range.clone()], other_lines[i])
            });
            changes.push((
                range.clone(),
                range.start + changed.start..range.start + changed.end,
            ));
        }
    }
    let mut change_index = 0;
    for section in &job.sections {
        let mut at = section.byte_range.start.0;
        while at < section.byte_range.end.0 {
            while changes.get(change_index).is_some_and(|(r, _)| r.end <= at) {
                change_index += 1;
            }
            let change = changes.get(change_index);
            let mut end = section.byte_range.end.0;
            let mut format = section.format.clone();
            if let Some((range, strong)) = change {
                if at < range.start {
                    end = end.min(range.start);
                } else {
                    end = end.min(range.end);
                    format.color = color;
                    if at < strong.start {
                        end = end.min(strong.start);
                    } else if at < strong.end {
                        end = end.min(strong.end);
                        format.background = color.gamma_multiply(0.25);
                    }
                }
            }
            sections.push(LayoutSection {
                leading_space: if at == section.byte_range.start.0 {
                    section.leading_space
                } else {
                    0.0
                },
                byte_range: egui::text::ByteIndex(at)..egui::text::ByteIndex(end),
                format,
            });
            at = end;
        }
    }
    job.sections = sections;
}
fn logical_rows(galley: &Galley) -> Vec<Range<usize>> {
    let mut start = 0;
    let mut groups = Vec::new();
    for (i, row) in galley.rows.iter().enumerate() {
        if row.ends_with_newline {
            groups.push(start..i + 1);
            start = i + 1;
        }
    }
    if start < galley.rows.len() {
        groups.push(start..galley.rows.len());
    }
    groups
}
fn align(mut old: Galley, mut new: Galley, pairs: &[Pair]) -> Layout {
    let old_rows = logical_rows(&old);
    let new_rows = logical_rows(&new);
    let height = |g: &Galley, r: &Range<usize>| {
        g.rows[r.end - 1].rect().bottom() - g.rows[r.start].rect().top()
    };
    let mut tops = Vec::new();
    let mut heights = Vec::new();
    let mut y = 0.0;
    let mut rows = pairs.to_vec();
    let count_old = pairs.iter().filter(|p| p.old.is_some()).count();
    let count_new = pairs.iter().filter(|p| p.new.is_some()).count();
    if count_old < old_rows.len() || count_new < new_rows.len() {
        rows.push(Pair {
            old: (count_old < old_rows.len()).then_some(count_old),
            new: (count_new < new_rows.len()).then_some(count_new),
            changed: false,
        });
    }
    for pair in &rows {
        let a = pair.old.map(|i| &old_rows[i]);
        let b = pair.new.map(|i| &new_rows[i]);
        let h = a
            .map_or(0.0, |r| height(&old, r))
            .max(b.map_or(0.0, |r| height(&new, r)));
        tops.push(y);
        heights.push(h);
        for (g, r) in [(&mut old, a), (&mut new, b)] {
            if let Some(r) = r {
                let shift = y - g.rows[r.start].pos.y;
                for row in &mut g.rows[r.clone()] {
                    row.pos.y += shift;
                }
            }
        }
        y += h;
    }
    for g in [&mut old, &mut new] {
        g.rect.max.y = y;
        g.mesh_bounds = Rect::NOTHING;
        for row in &g.rows {
            g.mesh_bounds = g
                .mesh_bounds
                .union(row.visuals.mesh_bounds.translate(row.pos.to_vec2()));
        }
    }
    Layout {
        old: Arc::new(old),
        new: Arc::new(new),
        tops,
        heights,
        height: y,
        old_rows,
        new_rows,
    }
}
fn layout(
    ui: &Ui,
    id: Id,
    diff: &mut DocumentDiff,
    current: &str,
    extension: &str,
    settings: &AppSettings,
    dimensions: (f32, bool),
) -> Arc<Layout> {
    let (width, mitex_dollars) = dimensions;
    diff.refresh(current);
    let style = ui.style().clone();
    let scale = ui.ctx().pixels_per_point();
    let cache_id = id.with("full-diff-layout");
    if let Some(cache) = ui
        .ctx()
        .data(|data| data.get_temp::<Arc<LayoutCache>>(cache_id))
        .filter(|c| {
            c.new == current
                && c.old == diff.before
                && c.extension == extension
                && c.width == width
                && c.style == style
                && c.settings.line_wrap == settings.line_wrap
                && c.settings.line_numbers == settings.line_numbers
                && c.settings.rainbow_brackets == settings.rainbow_brackets
                && c.settings.highlight_local_uses == settings.highlight_local_uses
                && c.settings.highlight_rules == settings.highlight_rules
                && c.settings.typst_overrides == settings.typst_overrides
                && c.settings.code_font_path == settings.code_font_path
                && c.settings.code_font_family == settings.code_font_family
                && c.settings.code_font_face_index == settings.code_font_face_index
                && c.settings.code_font_weight == settings.code_font_weight
                && crate::presentation::active_theme_request(
                    &c.settings,
                    Some(ui.ctx().theme()),
                    None,
                ) == crate::presentation::active_theme_request(
                    settings,
                    Some(ui.ctx().theme()),
                    None,
                )
                && c.scale == scale
                && c.mitex_dollars == mitex_dollars
        })
    {
        return cache.layout.clone();
    }
    let mut a = code_editor::highlight_job_with_mitex(
        ui,
        id.with("before"),
        &diff.before,
        extension,
        settings,
        mitex_dollars,
    );
    let mut b =
        code_editor::highlight_job_with_mitex(ui, id, current, extension, settings, mitex_dollars);
    decorate(&mut a, current, &diff.pairs, true, ui.ctx());
    decorate(&mut b, &diff.before, &diff.pairs, false, ui.ctx());
    a.wrap.max_width = if settings.line_wrap {
        width
    } else {
        f32::INFINITY
    };
    b.wrap.max_width = a.wrap.max_width;
    let result = Arc::new(align(
        (*ui.fonts_mut(|f| f.layout_job(a))).clone(),
        (*ui.fonts_mut(|f| f.layout_job(b))).clone(),
        &diff.pairs,
    ));
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            cache_id,
            Arc::new(LayoutCache {
                old: diff.before.clone(),
                new: current.into(),
                extension: extension.into(),
                width,
                style,
                settings: settings.clone(),
                scale,
                mitex_dollars,
                layout: result.clone(),
            }),
        )
    });
    result
}

pub(crate) struct ViewConfig<'a> {
    pub(crate) id: Id,
    pub(crate) path: &'a std::path::Path,
    pub(crate) settings: &'a AppSettings,
    pub(crate) repository_ready: bool,
    pub(crate) mitex_dollars: bool,
    pub(crate) jump: Option<Range<usize>>,
}
pub(crate) fn show(
    ui: &mut Ui,
    diff: &mut DocumentDiff,
    source: &mut dyn TextBuffer,
    config: ViewConfig<'_>,
) -> Output {
    let ViewConfig {
        id,
        path,
        settings,
        repository_ready,
        mitex_dollars,
        jump,
    } = config;
    let mut action = None;
    ui.horizontal(|ui| {
        ui.strong("Changes since last commit");
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            for (icon, label, command, enabled) in [
                (
                    icons::UiIcon::Close,
                    "Close diff editor",
                    Action::Close,
                    true,
                ),
                (
                    icons::UiIcon::Revert,
                    "Revert hunk",
                    Action::Revert,
                    repository_ready,
                ),
                (
                    icons::UiIcon::Unstage,
                    "Unstage",
                    Action::Unstage,
                    repository_ready,
                ),
                (
                    icons::UiIcon::Stage,
                    "Stage",
                    Action::Stage,
                    repository_ready,
                ),
                (
                    icons::UiIcon::Next,
                    "Next",
                    Action::Next,
                    !diff.hunks.is_empty(),
                ),
                (
                    icons::UiIcon::Previous,
                    "Previous",
                    Action::Previous,
                    !diff.hunks.is_empty(),
                ),
            ] {
                let response = icons::toolbar_button(
                    ui,
                    enabled,
                    false,
                    icon,
                    label,
                    settings.toolbar_style,
                    false,
                );
                let response = crate::app::native_hover_text(response, label);
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe(&format!("git.hunk.{label}"), &response);
                if response.clicked() {
                    action = Some(command);
                }
            }
        });
    });
    let extension = path.extension().and_then(|x| x.to_str()).unwrap_or("txt");
    let available = ui.available_size();
    let edit = ScrollArea::both()
        .id_salt("full-document-diff")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(available.x);
            let gap = theme::SPACE.small;
            let minimum_column = (available.x - gap) / 2.0;
            let digits = (diff
                .before
                .lines()
                .count()
                .max(source.as_str().lines().count())
                + 1)
            .to_string();
            let font = theme::editor_font();
            let muted = ui.visuals().weak_text_color();
            let gutter = if settings.line_numbers {
                ui.painter()
                    .layout_no_wrap(digits, font.clone(), muted)
                    .size()
                    .x
                    + theme::SPACE.small
            } else {
                22.0
            };
            let width = (minimum_column - gutter - 4.0).max(1.0);
            let backgrounds = ui.painter().add(Shape::Noop);
            let mut current_layout = layout(
                ui,
                id,
                diff,
                source.as_str(),
                extension,
                settings,
                (width, mitex_dollars),
            );
            let column = if settings.line_wrap {
                minimum_column
            } else {
                minimum_column.max(
                    current_layout.old.size().x.max(current_layout.new.size().x) + gutter + 4.0,
                )
            };
            let output = ui
                .horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let (old_rect, _) =
                        ui.allocate_exact_size(vec2(column + gap, 1.0), Sense::hover());
                    ui.allocate_exact_size(vec2(gutter, 1.0), Sense::hover());
                    let mut layouter = |ui: &Ui, buffer: &dyn TextBuffer, _: f32| {
                        current_layout = layout(
                            ui,
                            id,
                            diff,
                            buffer.as_str(),
                            extension,
                            settings,
                            (width, mitex_dollars),
                        );
                        current_layout.new.clone()
                    };
                    let output = TextEdit::multiline(source)
                        .id(id)
                        .code_editor()
                        .font(font.clone())
                        .frame(Frame::NONE)
                        .desired_width(width)
                        .min_size(vec2(width, 0.0))
                        .layouter(&mut layouter)
                        .show(ui);
                    (old_rect, output)
                })
                .inner;
            let (old_rect, output) = output;
            let origin = output.galley_pos;
            let old_origin = pos2(old_rect.left() + gutter, origin.y);
            ui.painter()
                .with_clip_rect(ui.clip_rect().intersect(Rect::from_min_max(
                    old_rect.min,
                    pos2(old_rect.left() + column, ui.clip_rect().bottom()),
                )))
                .galley(
                    old_origin,
                    current_layout.old.clone(),
                    ui.visuals().text_color(),
                );
            let palette = theme::palette(ui.ctx());
            let mut shapes = Vec::new();
            for (i, pair) in diff.pairs.iter().enumerate().filter(|(_, p)| p.changed) {
                let y = origin.y + current_layout.tops[i];
                let h = current_layout.heights[i];
                for (present, x, color) in [
                    (pair.old.is_some(), old_rect.left(), palette.error),
                    (pair.new.is_some(), origin.x - gutter, palette.success),
                ] {
                    if present {
                        shapes.push(Shape::rect_filled(
                            Rect::from_min_size(pos2(x, y), vec2(column, h)),
                            0.0,
                            color.gamma_multiply(0.12),
                        ));
                    }
                }
                let rect = Rect::from_min_size(
                    pos2(old_rect.left() + column - 22.0, y),
                    vec2(22.0, h.min(22.0)),
                );
                if ui.clip_rect().intersects(rect) {
                    let response =
                        ui.interact(rect, id.with(("revert-diff-line", i)), Sense::click());
                    if response.hovered() {
                        ui.painter().rect_filled(rect, 2.0, ui.visuals().panel_fill);
                        icons::paint_ui_icon(
                            ui.painter(),
                            rect.shrink(4.0),
                            icons::UiIcon::Revert,
                            ui.visuals().text_color(),
                        );
                    }
                    response.widget_info(|| {
                        WidgetInfo::labeled(
                            WidgetType::Button,
                            true,
                            format!("Revert changed line {}", i + 1),
                        )
                    });
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(&format!("git.hunk.line.{i}"), &response);
                    if response.clicked() {
                        action = Some(Action::RevertLine(i));
                    }
                }
            }
            ui.painter().set(backgrounds, Shape::Vec(shapes));
            for (g, groups, base, x) in [
                (
                    &current_layout.old,
                    &current_layout.old_rows,
                    old_origin,
                    old_origin.x - 4.0,
                ),
                (
                    &output.galley,
                    &current_layout.new_rows,
                    origin,
                    origin.x - 4.0,
                ),
            ] {
                for (line, rows) in groups.iter().enumerate().filter(|_| settings.line_numbers) {
                    let row = &g.rows[rows.start];
                    let rect = row.rect().translate(base.to_vec2());
                    if ui.clip_rect().intersects(rect) {
                        let number = ui.painter().layout_no_wrap(
                            (line + 1).to_string(),
                            font.clone(),
                            muted,
                        );
                        let position =
                            crate::app::line_number_position(x, rect.top(), row, &number);
                        ui.painter().galley(position, number, muted);
                    }
                }
            }
            ui.set_min_height(current_layout.height.max(available.y));
            if let Some(range) = &jump {
                let rect = output
                    .galley
                    .pos_from_cursor(egui::text::CCursor::new(range.end))
                    .translate(origin.to_vec2());
                ui.scroll_to_rect(rect, Some(Align::Center));
            }
            output
        })
        .inner;
    if edit.response.changed() {
        ui.ctx().request_repaint();
    }
    edit.response
        .widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, "Document diff contents"));
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    {
        crate::desktop_test::observe("git.hunk.editor", &edit.response);
        crate::desktop_test::observe("editor.source", &edit.response);
    }
    Output { edit, action }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};
    fn fonts(context: &Context) {
        theme::configure_editor_fonts(
            context,
            Default::default(),
            Default::default(),
            false,
            400,
            400,
            None,
        );
    }

    #[test]
    fn identical_text_with_a_different_language_invalidates_layout() {
        let context = Context::default();
        fonts(&context);
        context
            .run_ui(Default::default(), |ui| {
                let source = "\\section{Introduction}\n#let value = 2\n";
                let settings = AppSettings::default();
                let mut diff = DocumentDiff::new(source.into(), source);
                let typst = layout(
                    ui,
                    Id::new("same-editor"),
                    &mut diff,
                    source,
                    "typ",
                    &settings,
                    (400.0, false),
                );
                let tex = layout(
                    ui,
                    Id::new("same-editor"),
                    &mut diff,
                    source,
                    "tex",
                    &settings,
                    (400.0, false),
                );
                let fresh = layout(
                    ui,
                    Id::new("fresh-editor"),
                    &mut diff,
                    source,
                    "tex",
                    &settings,
                    (400.0, false),
                );
                assert!(
                    !Arc::ptr_eq(&typst, &tex),
                    "cached Typst layout reused for TeX"
                );
                assert_eq!(tex.new.job.sections, fresh.new.job.sections);
                let repeated = layout(
                    ui,
                    Id::new("same-editor"),
                    &mut diff,
                    source,
                    "tex",
                    &settings,
                    (400.0, false),
                );
                assert!(Arc::ptr_eq(&tex, &repeated));
            })
            .textures_delta
            .clear();
    }

    #[test]
    fn paired_layout_aligns_wraps_without_changing_character_indices() {
        let context = Context::default();
        fonts(&context);
        context.run_ui(Default::default(), |ui| {
            let before =
                "unchanged\na removed long line that wraps over and over\nsecond removed\nend\n";
            let current = "unchanged\nreplacement\nend\n";
            let mut diff = DocumentDiff::new(before.into(), current);
            let settings = AppSettings::default();
            let positioned = layout(
                ui,
                Id::new("test"),
                &mut diff,
                current,
                "typ",
                &settings,
                (110.0, false),
            );
            let old = logical_rows(&positioned.old);
            let new = logical_rows(&positioned.new);
            assert_eq!(positioned.old.job.text, before);
            assert_eq!(positioned.new.job.text, current);
            for pair in &diff.pairs {
                if let (Some(a), Some(b)) = (pair.old, pair.new) {
                    assert_eq!(
                        positioned.old.rows[old[a].start].pos.y,
                        positioned.new.rows[new[b].start].pos.y
                    );
                }
            }
            assert!(
                positioned.new.rows[new[2].start].pos.y
                    > positioned.new.rows[new[1].end - 1].rect().bottom()
            );
            assert_eq!(positioned.new.end().index.0, current.chars().count());
            let cursor = positioned
                .new
                .cursor_from_pos(positioned.new.rows[new[2].start].pos.to_vec2());
            assert_eq!(cursor.index.0, current.find("end").unwrap());
            let reused = layout(
                ui,
                Id::new("test"),
                &mut diff,
                current,
                "typ",
                &settings,
                (110.0, false),
            );
            assert!(Arc::ptr_eq(&positioned, &reused));
            assert_eq!(diff.builds, 1);
            let mut unrelated_settings = settings.clone();
            unrelated_settings.auto_save = !settings.auto_save;
            let same = layout(ui, Id::new("test"), &mut diff, current, "typ", &unrelated_settings, (110.0, false));
            assert!(Arc::ptr_eq(&positioned, &same), "unrelated settings invalidated editor layout");
            let resized = layout(ui, Id::new("test"), &mut diff, current, "typ", &settings, (220.0, false));
            assert!(!Arc::ptr_eq(&positioned, &resized));
            let mitex = layout(ui, Id::new("test"), &mut diff, current, "typ", &settings, (110.0, true));
            assert!(!Arc::ptr_eq(&positioned, &mitex));
            assert_eq!(mitex.new.job.text, current);

        }).textures_delta.clear();
    }
    #[test]
    fn git_colors_override_only_changed_lines_and_keep_intraline_emphasis() {
        let context = Context::default();
        fonts(&context);
        context
            .run_ui(Default::default(), |ui| {
                let mut diff = DocumentDiff::new(
                    "#let same = 1\n#let value = 1\n".into(),
                    "#let same = 1\n#let value = 2\n",
                );
                let original = code_editor::highlight_job(
                    ui,
                    Id::new("syntax"),
                    &diff.current,
                    "typ",
                    &AppSettings::default(),
                );
                let positioned = layout(
                    ui,
                    Id::new("syntax"),
                    &mut diff,
                    "#let same = 1\n#let value = 2\n",
                    "typ",
                    &AppSettings::default(),
                    (400.0, false),
                );
                let boundary = diff.current.find('\n').unwrap() + 1;
                for section in original
                    .sections
                    .iter()
                    .filter(|s| s.byte_range.end.0 <= boundary)
                {
                    assert!(positioned.new.job.sections.contains(section));
                }
                let palette = theme::palette(ui.ctx());
                assert!(
                    positioned
                        .new
                        .job
                        .sections
                        .iter()
                        .filter(|s| s.byte_range.start.0 >= boundary)
                        .all(|s| s.format.color == palette.success)
                );
                assert!(
                    positioned
                        .new
                        .job
                        .sections
                        .iter()
                        .any(|s| s.format.background == palette.success.gamma_multiply(0.25))
                );
            })
            .textures_delta
            .clear();
    }
    #[test]
    fn one_live_edit_mode_has_no_apply_or_mode_switch_and_updates_full_document() {
        let text = "one\nchanged\nthree\n";
        let mut harness = Harness::builder()
            .with_size(vec2(900.0, 500.0))
            .build_ui_state(
                |ui, state: &mut (DocumentDiff, String)| {
                    let ready = ui
                        .ctx()
                        .data(|data| data.get_temp::<bool>(Id::new("test-fonts")))
                        .unwrap_or(false);
                    if !ready {
                        fonts(ui.ctx());
                        ui.ctx()
                            .data_mut(|data| data.insert_temp(Id::new("test-fonts"), true));
                        return;
                    }

                    show(
                        ui,
                        &mut state.0,
                        &mut state.1,
                        ViewConfig {
                            id: Id::new("source"),
                            path: std::path::Path::new("main.typ"),
                            settings: &AppSettings::default(),
                            repository_ready: false,
                            mitex_dollars: false,
                            jump: None,
                        },
                    );
                },
                (
                    DocumentDiff::new("one\ntwo\nthree\n".into(), text),
                    text.into(),
                ),
            );
        harness.run();
        assert!(harness.query_by_label("Apply").is_none());
        assert!(harness.query_by_label("Edit").is_none());
        assert!(harness.query_by_label("Diff").is_none());
        harness.get_by_label("Document diff contents").click();
        harness.key_press(egui::Key::End);
        harness
            .get_by_label("Document diff contents")
            .type_text("α");
        harness.run();
        assert!(harness.state().1.contains('α'));
        assert_eq!(harness.state().0.current, harness.state().1);
        assert_eq!(harness.state().0.before, "one\ntwo\nthree\n");
    }
}
