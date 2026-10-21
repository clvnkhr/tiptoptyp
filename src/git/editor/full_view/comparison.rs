//! Immutable unified presentation of the docked editor's syntax and diff layout.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Line {
    old: Option<usize>,
    new: Option<usize>,
    changed: bool,
    row: usize,
    top: f32,
    height: f32,
}

struct Unified {
    galley: Arc<Galley>,
    lines: Vec<Line>,
}
#[derive(Clone)]
struct Cache {
    paired: Arc<Layout>,
    unified: Arc<Unified>,
}

fn source_lines(diff: &DocumentDiff) -> Vec<Pair> {
    let mut lines = Vec::with_capacity(diff.pairs.len() * 2);
    let mut at = 0;
    while at < diff.pairs.len() {
        if !diff.pairs[at].changed {
            lines.push(diff.pairs[at].clone());
            at += 1;
            continue;
        }
        let first = at;
        while at < diff.pairs.len() && diff.pairs[at].changed {
            at += 1;
        }
        for pair in &diff.pairs[first..at] {
            if let Some(old) = pair.old {
                lines.push(Pair {
                    old: Some(old),
                    new: None,
                    changed: true,
                });
            }
        }
        for pair in &diff.pairs[first..at] {
            if let Some(new) = pair.new {
                lines.push(Pair {
                    old: None,
                    new: Some(new),
                    changed: true,
                });
            }
        }
    }
    lines
}

fn append_job_line(target: &mut LayoutJob, job: &LayoutJob, range: Range<usize>, newline: bool) {
    let base = target.text.len();
    target.text.push_str(&job.text[range.clone()]);
    let first = job
        .sections
        .partition_point(|section| section.byte_range.end.0 <= range.start);
    for section in job.sections[first..]
        .iter()
        .take_while(|section| section.byte_range.start.0 < range.end)
    {
        let start = section.byte_range.start.0.max(range.start);
        let end = section.byte_range.end.0.min(range.end);
        target.sections.push(LayoutSection {
            leading_space: if start == section.byte_range.start.0 {
                section.leading_space
            } else {
                0.0
            },
            byte_range: egui::text::ByteIndex(base + start - range.start)
                ..egui::text::ByteIndex(base + end - range.start),
            format: section.format.clone(),
        });
    }
    if newline && !target.text.ends_with('\n') {
        let format = target
            .sections
            .last()
            .map(|section| section.format.clone())
            .unwrap_or_default();
        target.append("\n", 0.0, format);
    }
}

fn build(diff: &DocumentDiff, paired: &Layout) -> Unified {
    let mut specs = source_lines(diff);
    let old_count = diff.pairs.iter().filter(|pair| pair.old.is_some()).count();
    let new_count = diff.pairs.iter().filter(|pair| pair.new.is_some()).count();
    if old_count < paired.old_rows.len() || new_count < paired.new_rows.len() {
        specs.push(Pair {
            old: (old_count < paired.old_rows.len()).then_some(old_count),
            new: (new_count < paired.new_rows.len()).then_some(new_count),
            changed: false,
        });
    }
    let old_ranges = line_ranges(&paired.old.job.text);
    let new_ranges = line_ranges(&paired.new.job.text);
    let mut galley = (*paired.new).clone();
    galley.rows.clear();
    galley.num_vertices = 0;
    galley.num_indices = 0;
    galley.mesh_bounds = Rect::NOTHING;
    let mut job = LayoutJob {
        wrap: paired.new.job.wrap.clone(),
        ..Default::default()
    };
    let mut lines = Vec::with_capacity(specs.len());
    let mut top = 0.0;
    for (index, spec) in specs.iter().enumerate() {
        let (source, group, ranges, line) = if let Some(line) = spec.new {
            (&paired.new, &paired.new_rows[line], &new_ranges, line)
        } else {
            let line = spec.old.unwrap();
            (&paired.old, &paired.old_rows[line], &old_ranges, line)
        };
        let height =
            source.rows[group.end - 1].rect().bottom() - source.rows[group.start].rect().top();
        let shift = top - source.rows[group.start].pos.y;
        let start = galley.rows.len();
        for row in &source.rows[group.clone()] {
            let mut row = row.clone();
            row.pos.y += shift;
            galley.mesh_bounds = galley
                .mesh_bounds
                .union(row.visuals.mesh_bounds.translate(row.pos.to_vec2()));
            galley.num_vertices += row.visuals.mesh.vertices.len();
            galley.num_indices += row.visuals.mesh.indices.len();
            galley.rows.push(row);
        }
        let newline = index + 1 < specs.len();
        if newline {
            galley.rows.last_mut().unwrap().ends_with_newline = true;
        }
        let range = ranges
            .get(line)
            .cloned()
            .unwrap_or(source.job.text.len()..source.job.text.len());
        append_job_line(&mut job, &source.job, range, newline);
        lines.push(Line {
            old: spec.old,
            new: spec.new,
            changed: spec.changed,
            row: start,
            top,
            height,
        });
        top += height;
    }
    galley.rect = Rect::from_min_size(
        Pos2::ZERO,
        vec2(paired.old.size().x.max(paired.new.size().x), top),
    );
    galley.job = Arc::new(job);
    Unified {
        galley: Arc::new(galley),
        lines,
    }
}

fn cached(ui: &Ui, id: Id, diff: &DocumentDiff, paired: Arc<Layout>) -> Arc<Unified> {
    let key = id.with("unified-comparison-layout");
    if let Some(cache) = ui.ctx().data(|data| data.get_temp::<Cache>(key))
        && Arc::ptr_eq(&cache.paired, &paired)
    {
        return cache.unified;
    }
    let unified = Arc::new(build(diff, &paired));
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            key,
            Cache {
                paired,
                unified: unified.clone(),
            },
        )
    });
    unified
}

fn visible_lines(layout: &Unified, base: Pos2, clip: Rect) -> Range<usize> {
    if !base.is_finite() || !clip.is_finite() || !clip.is_positive() {
        return 0..0;
    }
    let first = layout
        .lines
        .partition_point(|line| base.y + line.top + line.height < clip.top());
    let end = layout
        .lines
        .partition_point(|line| base.y + line.top <= clip.bottom());
    first..end
}

pub(super) fn show(
    ui: &mut Ui,
    diff: &DocumentDiff,
    id: Id,
    path: &std::path::Path,
    settings: &AppSettings,
) -> Rect {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("txt");
    let available = ui.available_size();
    ScrollArea::both()
        .id_salt(id.with("unified-comparison-scroll"))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(available.x);
            let number_width = if settings.line_numbers {
                line_number_gutter(ui, &diff.before, &diff.current, settings)
            } else {
                0.0
            };
            let sign_width = ui
                .painter()
                .layout_no_wrap("+".into(), theme::editor_font(), ui.visuals().text_color())
                .size()
                .x
                + 6.0;
            let gutter = number_width * 2.0 + sign_width;
            let paired = cached_layout(
                ui,
                id,
                diff,
                &diff.current,
                extension,
                settings,
                ((available.x - gutter - 4.0).max(1.0), false),
            );
            let layout = cached(ui, id, diff, paired);
            let (rect, response) = ui.allocate_exact_size(
                vec2(
                    available.x.max(layout.galley.size().x + gutter + 4.0),
                    layout.galley.size().y.max(available.y),
                ),
                Sense::hover(),
            );
            response.widget_info(|| {
                WidgetInfo::labeled(WidgetType::Label, true, "Unified comparison contents")
            });
            let base = rect.min + vec2(gutter, 0.0);
            let clip = ui.clip_rect();
            let palette = theme::palette(ui.ctx());
            let visible = visible_lines(&layout, base, clip);
            for line in &layout.lines[visible] {
                let row = &layout.galley.rows[line.row];
                let y = base.y + line.top;
                let color = if line.old.is_some() {
                    palette.error
                } else {
                    palette.success
                };
                if line.changed {
                    ui.painter().rect_filled(
                        Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), line.height)),
                        0.0,
                        color.gamma_multiply(0.12),
                    );
                    let sign = if line.old.is_some() { "−" } else { "+" };
                    let galley =
                        ui.painter()
                            .layout_no_wrap(sign.into(), theme::editor_font(), color);
                    let pos = crate::app::line_number_position(base.x - 4.0, y, row, &galley);
                    ui.painter().galley(pos, galley, color);
                }
                if settings.line_numbers {
                    for (number, x) in [
                        (line.old, rect.left() + number_width - 4.0),
                        (line.new, rect.left() + 2.0 * number_width - 4.0),
                    ] {
                        if let Some(number) = number {
                            let galley = ui.painter().layout_no_wrap(
                                (number + 1).to_string(),
                                theme::editor_font(),
                                ui.visuals().weak_text_color(),
                            );
                            let pos = crate::app::line_number_position(x, y, row, &galley);
                            ui.painter()
                                .galley(pos, galley, ui.visuals().weak_text_color());
                        }
                    }
                }
            }
            ui.painter()
                .galley(base, layout.galley.clone(), ui.visuals().text_color());
        })
        .inner_rect
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unified_rows_keep_all_context_and_both_versions_of_changed_lines() {
        let diff = DocumentDiff::new("before\nold\nafter\n".into(), "before\nnew\nafter\n");
        let rows = source_lines(&diff);
        assert_eq!(rows.iter().filter(|row| !row.changed).count(), 2);
        assert_eq!(
            rows.iter()
                .map(|row| (row.old, row.new))
                .collect::<Vec<_>>(),
            vec![
                (Some(0), Some(0)),
                (Some(1), None),
                (None, Some(1)),
                (Some(2), Some(2))
            ]
        );
    }

    #[test]
    fn cached_unified_galley_preserves_context_syntax_and_change_highlights() {
        let context = Context::default();
        theme::configure_editor_fonts(
            &context,
            Default::default(),
            Default::default(),
            false,
            400,
            400,
            None,
        );
        let before = "#let value = 1\nOld result.\nUnchanged $x$.\n";
        let after = "#let value = 1\nNew result.\nUnchanged $x$.\n";
        let diff = DocumentDiff::new(before.into(), after);
        context
            .run_ui(Default::default(), |ui| {
                let paired = cached_layout(
                    ui,
                    Id::new("unified-test"),
                    &diff,
                    &diff.current,
                    "typ",
                    &AppSettings::default(),
                    (600.0, false),
                );
                let first = cached(ui, Id::new("unified-test"), &diff, paired.clone());
                assert_eq!(
                    first.galley.job.text,
                    "#let value = 1\nOld result.\nNew result.\nUnchanged $x$.\n"
                );
                assert_eq!(first.lines.iter().filter(|line| line.changed).count(), 2);
                let colors: std::collections::HashSet<_> = first
                    .galley
                    .job
                    .sections
                    .iter()
                    .map(|section| section.format.color)
                    .collect();
                assert!(colors.len() > 2, "syntax and change colors must coexist");
                assert!(
                    first
                        .galley
                        .job
                        .sections
                        .iter()
                        .any(|section| section.format.background != Color32::TRANSPARENT)
                );
                let next = cached(ui, Id::new("unified-test"), &diff, paired);
                assert!(Arc::ptr_eq(&first, &next));
                for clip in [
                    Rect::NOTHING,
                    Rect::from_min_max(pos2(0.0, 30.0), pos2(100.0, 10.0)),
                    Rect::from_min_size(Pos2::ZERO, Vec2::ZERO),
                ] {
                    assert!(visible_lines(&first, Pos2::ZERO, clip).is_empty());
                }
                assert!(
                    visible_lines(
                        &first,
                        pos2(f32::NAN, 0.0),
                        Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0))
                    )
                    .is_empty()
                );
            })
            .drop_without_applying_deltas();
    }
}
