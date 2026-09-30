//! Shared syntax-aware code fields with cached highlighting and line numbers.
use crate::{
    generic_highlight::GenericSyntaxHighlighter, highlight::SyntaxHighlighter,
    settings::AppSettings,
};
use eframe::egui;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
#[derive(Default)]
struct Highlight {
    generic: GenericSyntaxHighlighter,
    typst: SyntaxHighlighter,
    theme: Option<crate::presentation::ActiveThemeRequest>,
    overrides: Option<crate::syntax_theme::TypstStyleOverrides>,
}
pub(crate) fn editor(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    text: &mut dyn egui::TextBuffer,
    extension: &str,
    settings: &AppSettings,
    rows: usize,
) -> egui::Response {
    source_editor(
        ui,
        salt,
        text,
        extension,
        settings,
        Options {
            rows,
            ..Default::default()
        },
    )
}

pub(crate) struct Options {
    pub(crate) rows: usize,
    pub(crate) start_line: usize,
    pub(crate) editable: bool,
    pub(crate) frame: Option<egui::Frame>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            rows: 4,
            start_line: 1,
            editable: true,
            frame: None,
        }
    }
}

pub(crate) fn highlight_job(
    ui: &egui::Ui,
    id: egui::Id,
    source: &str,
    extension: &str,
    settings: &AppSettings,
) -> egui::text::LayoutJob {
    highlight_job_with_mitex(ui, id, source, extension, settings, false)
}

pub(crate) fn highlight_job_with_mitex(
    ui: &egui::Ui,
    id: egui::Id,
    source: &str,
    extension: &str,
    settings: &AppSettings,
    mitex_dollars: bool,
) -> egui::text::LayoutJob {
    let cache = ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<Highlight>>>(id.with("syntax"))
            .clone()
    });
    let mut cache = cache.lock().unwrap();
    let request = crate::presentation::active_theme_request(settings, Some(ui.ctx().theme()), None);
    let overrides = settings.typst_overrides.for_dark(ui.visuals().dark_mode);
    if cache.theme.as_ref() != Some(&request) || cache.overrides.as_ref() != Some(overrides) {
        let (active, _) = crate::presentation::load_active_theme_or_fallback(&request);
        cache
            .generic
            .set_custom_theme(Some(active.syntect_theme.clone()));
        cache
            .typst
            .set_styles(crate::syntax_theme::ResolvedTypstStyles::resolve(
                crate::theme::syntax_palette_from_semantic(active.palette),
                Some(&active.syntect_theme),
                settings.typst_overrides.for_dark(active.dark_mode),
            ));
        cache.theme = Some(request);
        cache.overrides = Some(overrides.clone());
    }
    cache
        .generic
        .set_rainbow_brackets(settings.rainbow_brackets);
    cache.typst.set_rainbow_brackets(settings.rainbow_brackets);
    cache.typst.set_mitex_dollars(mitex_dollars);
    let definitions = settings.highlight_definitions && matches!(extension, "typ" | "tex");
    let accent = crate::theme::palette(ui.ctx()).accent;
    cache
        .generic
        .set_decorations(definitions, &settings.highlight_rules, accent);
    cache
        .typst
        .set_decorations(definitions, &settings.highlight_rules, accent);
    let path = format!("source.{extension}");
    let Highlight { generic, typst, .. } = &mut *cache;
    if extension == "typ" {
        typst.highlight(source, ui.visuals().dark_mode, generic)
    } else {
        generic.highlight(source, Some(Path::new(&path)), ui.visuals().dark_mode)
    }
}

pub(crate) fn source_editor(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    text: &mut dyn egui::TextBuffer,
    extension: &str,
    settings: &AppSettings,
    options: Options,
) -> egui::Response {
    let id = ui.make_persistent_id(salt);
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width: f32| {
        let mut job = highlight_job(ui, id, buffer.as_str(), extension, settings);
        job.wrap.max_width = width;
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    let lines = text.as_str().bytes().filter(|byte| *byte == b'\n').count() + options.start_line;
    let font = crate::theme::editor_font();
    let color = ui.visuals().weak_text_color();
    let width = ui
        .painter()
        .layout_no_wrap(lines.to_string(), font.clone(), color)
        .size()
        .x
        + 8.0;
    ui.horizontal_top(|ui| {
        let (gutter, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
        let mut editor = egui::TextEdit::multiline(text)
            .id(id)
            .code_editor()
            .font(font.clone())
            .interactive(options.editable)
            .desired_rows(options.rows)
            .desired_width(f32::INFINITY)
            .layouter(&mut layouter);
        if let Some(frame) = options.frame {
            editor = editor.frame(frame);
        }
        let output = editor.show(ui);
        for (line, rows) in super::logical_line_row_ranges(&output.galley.rows)
            .iter()
            .enumerate()
        {
            let row = &output.galley.rows[rows.start];
            let rect = row.rect().translate(output.galley_pos.to_vec2());
            if row.size.y == 0.0 || !ui.clip_rect().intersects(rect) {
                continue;
            }
            let number = ui.painter().layout_no_wrap(
                (line + options.start_line).to_string(),
                font.clone(),
                color,
            );
            let position =
                super::line_number_position(gutter.right() - 4.0, rect.top(), row, &number);
            ui.painter().galley(position, number, color);
        }
        output.response.response
    })
    .inner
}
