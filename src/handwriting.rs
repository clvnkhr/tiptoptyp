//! Offline handwriting recognition. Exactly one selected engine runs on a worker.
use crate::settings::{HandwritingEngine, HandwritingSettings};
use crate::worker::{LatestJob, LatestJobPoll};
use eframe::egui::{self, Pos2};
use std::sync::OnceLock;
use tract_onnx::prelude::*;

#[derive(Clone, serde::Deserialize)]
pub(crate) struct Symbol {
    pub char: String,
    pub tex: Option<String>,
    #[serde(default)]
    pub typst: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // Exposed by the real-app drawing journey snapshot.
    pub detexify: bool,
    #[serde(default)]
    pub tex_only: bool,
    #[serde(default)]
    pub package: Option<String>,
}
fn symbols() -> &'static [Symbol] {
    static SYMBOLS: OnceLock<Vec<Symbol>> = OnceLock::new();
    SYMBOLS.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/handwriting/symbols.json"))
            .expect("bundled symbol metadata")
    })
}
/// Isotropic coordinates keep handwriting stable when its panel is resized.
struct CanvasTransform {
    center: Pos2,
    scale: f32,
    origin: Pos2,
}
impl CanvasTransform {
    fn new(rect: egui::Rect, artboard: egui::Rect) -> Self {
        Self {
            center: rect.center(),
            origin: artboard.center(),
            scale: (rect.width() / artboard.width())
                .min(rect.height() / artboard.height())
                .max(0.001),
        }
    }
    fn to_model(&self, point: Pos2) -> Pos2 {
        self.origin + (point - self.center) / self.scale
    }
    fn to_screen(&self, point: Pos2) -> Pos2 {
        self.center + (point - self.origin) * self.scale
    }
}
fn raster(strokes: &[Vec<Pos2>]) -> Vec<f32> {
    let mut pixels = vec![0.0; 224 * 224];
    let mut bounds = egui::Rect::NOTHING;
    for point in strokes.iter().flatten() {
        bounds.extend_with(*point);
    }
    if !bounds.is_finite() {
        return pixels;
    }
    let scale = 204.0 / bounds.width().max(bounds.height()).max(1.0);
    for stroke in strokes {
        let segments = stroke
            .windows(2)
            .map(|p| (p[0], p[1]))
            .chain((stroke.len() == 1).then(|| (stroke[0], stroke[0])));
        for (first, last) in segments {
            let a = egui::pos2(112.0, 112.0) + (first - bounds.center()) * scale;
            let b = egui::pos2(112.0, 112.0) + (last - bounds.center()) * scale;
            let rect = egui::Rect::from_two_pos(a, b).expand(5.0);
            for y in
                (rect.top().floor().max(0.0) as usize)..(rect.bottom().ceil().min(224.0) as usize)
            {
                for x in (rect.left().floor().max(0.0) as usize)
                    ..(rect.right().ceil().min(224.0) as usize)
                {
                    let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                    let segment = b - a;
                    let t = ((p - a).dot(segment) / segment.length_sq().max(0.001)).clamp(0.0, 1.0);
                    let coverage = (4.5 - (p - (a + t * segment)).length()).clamp(0.0, 1.0);
                    pixels[y * 224 + x] = f32::max(pixels[y * 224 + x], coverage);
                }
            }
        }
    }
    pixels
}
pub(crate) const MAX_PREDICTIONS: usize = 16;

fn recognize_symbols(strokes: &[Vec<Pos2>]) -> Result<Vec<Symbol>, String> {
    static MODEL: OnceLock<Result<Arc<TypedRunnableModel>, String>> = OnceLock::new();
    let model = MODEL
        .get_or_init(|| {
            tract_onnx::onnx()
                .model_for_read(&mut std::io::Cursor::new(include_bytes!(
                    "../assets/handwriting/detypify.onnx"
                )))
                .and_then(|model| model.into_optimized())
                .and_then(|model| model.into_runnable())
                .map_err(|e| format!("Could not load symbol model: {e:#}"))
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let pixels = raster(strokes);
    let input = Tensor::from_shape(&[1, 1, 224, 224], &pixels).map_err(|e| e.to_string())?;
    let output = model.run(tvec!(input.into())).map_err(|e| e.to_string())?;
    let view = output[0]
        .to_plain_array_view::<f32>()
        .map_err(|e| e.to_string())?;
    let scores = view.as_slice().ok_or("Noncontiguous symbol scores")?;
    if scores.len() != symbols().len() {
        return Err("Symbol model and labels disagree".into());
    }
    let mut ranking: Vec<_> = scores.iter().enumerate().collect();
    ranking.sort_by(|a, b| b.1.total_cmp(a.1));
    let results: Vec<_> = ranking
        .into_iter()
        .take(MAX_PREDICTIONS)
        .map(|(index, _)| symbols()[index].clone())
        .collect();
    Ok(results)
}
fn recognize(strokes: Vec<Vec<Pos2>>, engine: HandwritingEngine) -> Result<Vec<Symbol>, String> {
    match engine {
        HandwritingEngine::Detypify => recognize_symbols(&strokes),
        HandwritingEngine::Detexify => Ok(crate::handwriting_detexify::recognize(&strokes)),
    }
}

pub(crate) fn settings_ui(ui: &mut egui::Ui, settings: &mut HandwritingSettings) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Handwriting recognition");
        for engine in HandwritingEngine::ALL {
            let response = ui.selectable_value(&mut settings.engine, engine, engine.label()).on_hover_text(match engine {
                HandwritingEngine::Detypify => "411 symbol classes; neural model. Works with Typst and TeX. Model and labels: 4.43 MiB, plus ONNX runtime code.",
                HandwritingEngine::Detexify => "1,123 symbol definitions, including calligraphic letters; sample matching. Works with Typst and TeX. Samples and labels: 5.03 MiB.",
            });
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe(&format!("settings.handwriting.{}", engine.label()), &response);
            let _ = response;
        }
    });
    let response = ui.checkbox(&mut settings.prefer_typst_names, "Prefer Typst symbol names")
        .on_hover_text("Insert escaped punctuation first (\\$), then a verified Typst name (alpha or #sym.alpha), then Unicode. Off prefers Unicode. Delimiters remain escaped; Unicode is never wrapped in quotes.");
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe("settings.handwriting.names", &response);
    let _ = response;
    ui.checkbox(&mut settings.prefer_math_mode, "Prefer Typst math-mode names")
        .on_hover_text("Use xi instead of #sym.xi for insertion into math. Turn off for full names usable in markup.");
}

fn symbol_names() -> &'static std::collections::BTreeMap<String, String> {
    static NAMES: OnceLock<std::collections::BTreeMap<String, String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../assets/handwriting/typst-symbol-names.json"
        ))
        .expect("verified Typst symbol names")
    })
}

fn prediction_insertion(
    symbol: &Symbol,
    tex: bool,
    prefer_names: bool,
    prefer_math: bool,
) -> Option<String> {
    // This dataset entry represents a layout operation, not a drawn character.
    if symbol.tex.as_deref() == Some("\\\\") {
        return None;
    }
    if tex {
        return symbol
            .tex
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| prediction_glyph(symbol));
    }
    if symbol.tex_only {
        return None;
    }
    let glyph = prediction_glyph(symbol)?;
    if glyph.chars().count() != 1 {
        return None;
    }
    // Escapes beat both named and Unicode output so punctuation cannot become syntax.
    if glyph.len() == 1 && glyph.as_bytes()[0].is_ascii_punctuation() {
        return Some(format!("\\{glyph}"));
    }
    if prefer_names {
        let normalized = glyph.replace(['\u{fe0e}', '\u{fe0f}'], "");
        if let Some(name) = symbol_names().get(&normalized) {
            return Some(if prefer_math {
                name.clone()
            } else {
                format!("#sym.{name}")
            });
        }
        if let Some(expression) = &symbol.typst
            && [
                "cal(", "scr(", "frak(", "bb(", "bold(", "italic(", "sans(", "mono(", "upright(",
            ]
            .iter()
            .any(|prefix| expression.starts_with(prefix))
        {
            return Some(expression.clone());
        }
    }
    Some(glyph)
}

struct Prediction {
    index: usize,
    insertion: String,
    caption: String,
}
fn prediction_rows(
    results: &[Symbol],
    tex: bool,
    prefer_names: bool,
    prefer_math: bool,
) -> Vec<Prediction> {
    let mut seen = std::collections::HashSet::new();
    results
        .iter()
        .enumerate()
        .filter_map(|(index, symbol)| {
            let insertion = prediction_insertion(symbol, tex, prefer_names, prefer_math)?;
            if !seen.insert(insertion.clone()) {
                return None;
            }
            let caption = match prediction_glyph(symbol) {
                Some(glyph) if glyph != insertion => format!("{insertion}  {glyph}"),
                Some(glyph) => glyph,
                _ => insertion.clone(),
            };
            Some(Prediction {
                index,
                insertion,
                caption,
            })
        })
        .collect()
}

// Keep the rightmost glyph visible; only the leading insertion text is elided.
fn left_elide(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> std::borrow::Cow<'_, str> {
    if measure(text) <= width {
        return std::borrow::Cow::Borrowed(text);
    }
    let boundaries: Vec<_> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut low = 0;
    let mut high = boundaries.len() - 1;
    while low < high {
        let middle = (low + high) / 2;
        if measure(&format!("…{}", &text[boundaries[middle]..])) <= width {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    std::borrow::Cow::Owned(format!("…{}", &text[boundaries[low]..]))
}

fn prediction_glyph(symbol: &Symbol) -> Option<String> {
    if !symbol.char.is_empty() {
        return Some(symbol.char.clone());
    }

    let typst = symbol.typst.as_deref()?;
    if let Ok(glyph) = serde_json::from_str::<String>(typst)
        && !glyph.is_empty()
    {
        return Some(glyph);
    }
    if !typst.is_ascii() {
        return Some(typst.to_owned());
    }
    if typst == "dash.em.three" {
        return Some("⸻".into());
    }
    if let Some(letter) = typst
        .strip_prefix("scr(")
        .and_then(|value| value.strip_suffix(')'))
        .and_then(|value| value.chars().next())
    {
        return script_glyph(letter).map(str::to_owned);
    }
    match typst {
        "bb(h)" => return Some("𝕙".into()),
        "bb(k)" => return Some("𝕜".into()),
        "integral dots.h integral" => return Some("∫⋯∫".into()),
        "colon approx" => return Some(":≈".into()),
        "colon.double approx" => return Some("::≈".into()),
        "colon tilde.op" => return Some(":∼".into()),
        "colon.double tilde.op" => return Some("::∼".into()),
        "minus colon.double" => return Some("−::".into()),
        "eq colon.double" => return Some("=::".into()),
        _ => {}
    }
    if let Some(name) = typst
        .strip_prefix("upright(")
        .and_then(|value| value.strip_suffix(')'))
    {
        return greek_glyph(name).map(str::to_owned);
    }
    None
}

fn script_glyph(letter: char) -> Option<&'static str> {
    const UPPER: [&str; 26] = [
        "𝒜", "ℬ", "𝒞", "𝒟", "ℰ", "ℱ", "𝒢", "ℋ", "ℐ", "𝒥", "𝒦", "ℒ", "ℳ", "𝒩", "𝒪", "𝒫", "𝒬", "ℛ",
        "𝒮", "𝒯", "𝒰", "𝒱", "𝒲", "𝒳", "𝒴", "𝒵",
    ];
    const LOWER: [&str; 26] = [
        "𝒶", "𝒷", "𝒸", "𝒹", "ℯ", "𝒻", "ℊ", "𝒽", "𝒾", "𝒿", "𝓀", "𝓁", "𝓂", "𝓃", "ℴ", "𝓅", "𝓆", "𝓇",
        "𝓈", "𝓉", "𝓊", "𝓋", "𝓌", "𝓍", "𝓎", "𝓏",
    ];
    match letter {
        'A'..='Z' => Some(UPPER[(letter as u8 - b'A') as usize]),
        'a'..='z' => Some(LOWER[(letter as u8 - b'a') as usize]),
        _ => None,
    }
}

fn greek_glyph(name: &str) -> Option<&'static str> {
    Some(match name {
        "Delta" => "Δ",
        "Gamma" => "Γ",
        "Lambda" => "Λ",
        "Omega" => "Ω",
        "Phi" => "Φ",
        "Pi" => "Π",
        "Psi" => "Ψ",
        "Sigma" => "Σ",
        "Theta" => "Θ",
        "Upsilon" => "Υ",
        "Xi" => "Ξ",
        _ => return None,
    })
}

#[derive(Default)]
pub(crate) struct Drawing {
    engine: HandwritingEngine,
    strokes: Vec<Vec<Pos2>>,
    artboard: Option<egui::Rect>,
    undo: Vec<(Vec<Vec<Pos2>>, Option<egui::Rect>)>,
    redo: Vec<(Vec<Vec<Pos2>>, Option<egui::Rect>)>,
    job: LatestJob<(u64, Vec<Symbol>)>,
    job_generation: u64,
    generation: u64,
    pending: bool,
    results: Vec<Symbol>,
    error: Option<String>,
}
impl Drawing {
    fn checkpoint(&mut self) {
        if self.undo.len() == 32 {
            self.undo.remove(0);
        }
        self.undo.push((self.strokes.clone(), self.artboard));
        self.redo.clear();
    }
    fn history(&mut self, redo: bool) {
        let from = if redo { &mut self.redo } else { &mut self.undo };
        let Some((strokes, artboard)) = from.pop() else {
            return;
        };
        let previous = (std::mem::replace(&mut self.strokes, strokes), self.artboard);
        self.artboard = artboard;
        if redo {
            self.undo.push(previous);
        } else {
            self.redo.push(previous);
        }
        self.generation += 1;
        self.error = None;
        self.pending = !self.strokes.is_empty();
        if !self.pending {
            self.results.clear();
        }
    }

    pub(crate) fn fixture() -> Self {
        Self {
            artboard: Some(egui::Rect::from_min_max(
                Pos2::ZERO,
                egui::pos2(224.0, 224.0),
            )),
            strokes: vec![
                vec![egui::pos2(60.0, 80.0), egui::pos2(160.0, 80.0)],
                vec![egui::pos2(110.0, 40.0), egui::pos2(110.0, 180.0)],
            ],
            results: crate::handwriting_detexify::fixture_predictions(),
            ..Self::default()
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(crate) fn inspection(&self, tex: bool, settings: HandwritingSettings) -> serde_json::Value {
        serde_json::json!({
            "engine":self.engine,
            "strokes": self.strokes.iter().map(|s|s.iter().map(|p|[p.x,p.y]).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "predictions": prediction_rows(&self.results, tex, settings.prefer_typst_names, settings.prefer_math_mode).iter().map(|row| {
                let s = &self.results[row.index];
                serde_json::json!({"index":row.index,"typst":prediction_insertion(s,false,settings.prefer_typst_names, settings.prefer_math_mode),"tex":s.tex,"detexify":s.detexify,"caption":row.caption})
            }).collect::<Vec<_>>(),
            "busy":self.pending || self.job.is_running(),
        })
    }
    fn configure(&mut self, engine: HandwritingEngine) {
        if self.engine == engine {
            return;
        }
        self.engine = engine;
        self.generation += 1;
        self.results.clear();
        self.error = None;
        self.pending = !self.strokes.is_empty();
        // Let any current worker finish before starting the selected engine.
        // Its generation cannot publish results after this switch.
    }
    pub(crate) fn poll(&mut self) {
        match self.job.poll() {
            LatestJobPoll::Ready((generation, results)) if generation == self.generation => {
                self.results = results;
                self.error = None
            }
            LatestJobPoll::Failed(error) if self.job_generation == self.generation => {
                self.error = Some(error)
            }
            LatestJobPoll::Failed(_) => self.job.supersede(),
            _ => {}
        }
    }
    pub(crate) fn activity(&self) -> crate::activity::Activity {
        if let Some(error) = &self.error {
            crate::activity::Activity::Failed(error.clone())
        } else if self.pending {
            crate::activity::Activity::Pending("Drawing queued")
        } else {
            self.job.activity()
        }
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        tex: bool,
        settings: HandwritingSettings,
    ) -> Option<String> {
        self.configure(settings.engine);
        self.poll();
        let mut selected = None;
        let size = egui::vec2(
            ui.available_width().max(1.0),
            (ui.clip_rect().bottom() - ui.cursor().top()).max(1.0),
        );
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("drawing.canvas", &response);
        let artboard = *self.artboard.get_or_insert_with(|| {
            egui::Rect::from_center_size(
                egui::pos2(112.0, 112.0),
                rect.size() * (224.0 / rect.size().min_elem().max(1.0)),
            )
        });
        let transform = CanvasTransform::new(rect, artboard);
        ui.painter()
            .rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        if response.drag_started() && self.strokes.iter().map(Vec::len).sum::<usize>() < 4096 {
            self.checkpoint();
            // Extend the logical paper to the current panel before accepting
            // ink in its margins. A later resize fits the entire paper.
            self.artboard = Some(egui::Rect::from_min_max(
                transform.to_model(rect.min),
                transform.to_model(rect.max),
            ));
            let start = ui
                .input(|input| input.pointer.press_origin())
                .map(|point| transform.to_model(rect.clamp(point)));
            self.strokes.push(start.into_iter().collect());
            self.generation += 1;
            self.pending = false;
            self.error = None;
        }
        if response.dragged()
            && let Some(point) = response.interact_pointer_pos()
        {
            let count: usize = self.strokes.iter().map(Vec::len).sum();
            if count < 4096
                && let Some(stroke) = self.strokes.last_mut()
            {
                let p = transform.to_model(rect.clamp(point));
                if stroke.last().is_none_or(|last| last.distance(p) > 0.5) {
                    stroke.push(p);
                }
            }
        }
        for stroke in &self.strokes {
            if stroke.len() == 1 {
                ui.painter().with_clip_rect(rect).circle_filled(
                    transform.to_screen(stroke[0]),
                    2.0,
                    ui.visuals().text_color(),
                );
            }
            ui.painter().with_clip_rect(rect).add(egui::Shape::line(
                stroke.iter().map(|p| transform.to_screen(*p)).collect(),
                egui::Stroke::new(4.0, ui.visuals().text_color()),
            ));
        }
        if response.drag_stopped() {
            self.pending = true;
        }
        if self.pending && !self.job.is_running() {
            self.pending = false;
            let strokes = self.strokes.clone();
            let generation = self.generation;
            self.job_generation = generation;
            let engine = self.engine;
            if let Err(error) =
                self.job
                    .start_and_repaint("symbol-recognition", ui.ctx(), move || {
                        recognize(strokes, engine).map(|results| (generation, results))
                    })
            {
                self.error = Some(error);
            }
        }
        // Paint translucent result backgrounds after the ink. Each row has a single
        // right-aligned command for the active language and never selects text.
        let mut overlay = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(4.0)));
        overlay.set_clip_rect(rect);
        let mut control_clicked = false;
        let controls = overlay
            .horizontal(|ui| {
                for (label, icon, enabled) in [
                    (
                        "Clear",
                        crate::app::icons::UiIcon::Trash,
                        !self.strokes.is_empty(),
                    ),
                    (
                        "Undo stroke",
                        crate::app::icons::UiIcon::Previous,
                        !self.undo.is_empty(),
                    ),
                    (
                        "Redo stroke",
                        crate::app::icons::UiIcon::Next,
                        !self.redo.is_empty(),
                    ),
                ] {
                    let button = ui
                        .add_enabled_ui(enabled, |ui| {
                            crate::app::icons::square_icon_button(ui, icon, label, 24.0)
                        })
                        .inner
                        .on_hover_text(label);
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(
                        match label {
                            "Clear" => "drawing.clear",
                            "Undo stroke" => "drawing.undo",
                            _ => "drawing.redo",
                        },
                        &button,
                    );
                    if button.clicked() {
                        control_clicked = true;
                        match label {
                            "Clear" => {
                                self.checkpoint();
                                self.strokes.clear();
                                self.artboard = None;
                                self.results.clear();
                                self.error = None;
                                self.generation += 1;
                                self.pending = false;
                            }
                            "Undo stroke" => self.history(false),
                            _ => self.history(true),
                        }
                    }
                }
            })
            .response;
        if let Some(error) = &self.error {
            overlay.label(error);
        }
        let result_rect = egui::Rect::from_min_max(
            egui::pos2(controls.rect.right() + 4.0, rect.top() + 4.0),
            rect.max - egui::vec2(4.0, 4.0),
        );
        let mut results_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(result_rect)
                .layout(egui::Layout::top_down(egui::Align::Max)),
        );
        results_ui.set_clip_rect(result_rect.intersect(rect));
        results_ui.spacing_mut().item_spacing.y = 2.0;
        // Hover must not expand a row or alter the scroll layout.
        let widgets = &mut results_ui.visuals_mut().widgets;
        for state in [
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
        ] {
            state.expansion = 0.0;
            state.bg_stroke = egui::Stroke::NONE;
        }
        egui::ScrollArea::vertical()
            .id_salt("handwriting-results")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .auto_shrink([false, true])
            .show(&mut results_ui, |ui| {
                for row in prediction_rows(
                    &self.results,
                    tex,
                    settings.prefer_typst_names,
                    settings.prefer_math_mode,
                ) {
                    let mut tooltip = format!("Insert {}", row.insertion);
                    if tex && let Some(package) = &self.results[row.index].package {
                        tooltip.push_str(&format!("\nLaTeX package: {package}"));
                    }
                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                    let caption = left_elide(
                        &row.caption,
                        (ui.available_width() - 2.0 * ui.spacing().button_padding.x).max(0.0),
                        |text| {
                            ui.painter()
                                .layout_no_wrap(
                                    text.to_owned(),
                                    font.clone(),
                                    ui.visuals().text_color(),
                                )
                                .size()
                                .x
                        },
                    );
                    let prediction = ui
                        .add(
                            egui::Button::new(egui::RichText::new(caption.as_ref()).monospace())
                                .fill(ui.visuals().extreme_bg_color.gamma_multiply(0.85))
                                .stroke(egui::Stroke::NONE)
                                .wrap_mode(egui::TextWrapMode::Extend),
                        )
                        .on_hover_text(tooltip);
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(
                        &format!("drawing.prediction.{}", row.index),
                        &prediction,
                    );
                    if prediction.clicked() {
                        selected = Some(row.insertion);
                    }
                }
            });
        if response.clicked()
            && selected.is_none()
            && !control_clicked
            && self.strokes.iter().map(Vec::len).sum::<usize>() < 4096
            && let Some(point) = response.interact_pointer_pos()
        {
            self.checkpoint();
            self.strokes.push(vec![transform.to_model(point)]);
            self.artboard = Some(egui::Rect::from_min_max(
                transform.to_model(rect.min),
                transform.to_model(rect.max),
            ));
            self.generation += 1;
            self.error = None;
            self.pending = true;
            ui.ctx().request_repaint();
        }
        selected
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typst_symbol_insertion_escapes_delimiters_and_never_quotes_unicode() {
        let symbols: Vec<Symbol> =
            serde_json::from_str(include_str!("../assets/handwriting/detexify-symbols.json"))
                .unwrap();
        let dollar = symbols
            .iter()
            .find(|s| s.tex.as_deref() == Some("\\textdollar"))
            .unwrap();
        assert_eq!(
            prediction_insertion(dollar, false, true, false).as_deref(),
            Some("\\$")
        );
    }
    fn symbol(character: &str, tex: &str, typst: Option<&str>) -> Symbol {
        Symbol {
            char: character.into(),
            tex: Some(tex.into()),
            typst: typst.map(str::to_owned),
            detexify: false,
            tex_only: false,
            package: None,
        }
    }
    #[test]
    fn insertion_precedence_and_raw_unicode_apply_to_both_engines() {
        for (candidate, expected, unicode) in [
            (symbol("$", "\\textdollar", Some("\"$\"")), "\\$", "\\$"),
            (symbol("#", "\\#", None), "\\#", "\\#"),
            (symbol("%", "\\%", None), "\\%", "\\%"),
            (symbol("α", "\\alpha", None), "#sym.alpha", "α"),
            (symbol("₫", "\\textdong", Some("\"₫\"")), "#sym.dong", "₫"),
            (symbol("ʌ", "\\textturnv", Some("\"ʌ\"")), "ʌ", "ʌ"),
            (symbol("", "\\mathscr{A}", Some("scr(A)")), "scr(A)", "𝒜"),
        ] {
            assert_eq!(
                prediction_insertion(&candidate, false, true, false).as_deref(),
                Some(expected)
            );
            assert_eq!(
                prediction_insertion(&candidate, false, false, false).as_deref(),
                Some(unicode)
            );
            assert_eq!(
                prediction_insertion(&candidate, true, true, false),
                candidate.tex
            );
        }
        assert!(
            prediction_insertion(
                &symbol("", "\\\\", Some("#linebreak()")),
                false,
                true,
                false
            )
            .is_none()
        );
        let mut unsupported = symbol("", "\\ataribox", None);
        unsupported.tex_only = true;
        assert!(prediction_insertion(&unsupported, false, true, false).is_none());
        assert_eq!(
            prediction_insertion(&unsupported, true, true, false).as_deref(),
            Some("\\ataribox")
        );
        // Same glyph may legitimately have distinct TeX commands, but an identical
        // insertion is never offered twice. Keep the first, highest-ranked result.
        let candidates = vec![
            symbol("$", "\\$", None),
            symbol("$", "\\textdollar", None),
            symbol("α", "\\alpha", None),
        ];
        let rows = prediction_rows(&candidates, false, true, false);
        assert_eq!(rows.iter().map(|r| r.index).collect::<Vec<_>>(), [0, 2]);
    }
    #[test]
    fn settings_select_one_engine_and_roundtrip_the_unicode_preference() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut harness =
            Harness::builder().build_ui_state(settings_ui, HandwritingSettings::default());
        harness.run_steps(2);
        assert_eq!(harness.state().engine, HandwritingEngine::Detexify);
        harness.get_by_label("Detypify").click();
        harness.run_steps(2);
        assert_eq!(harness.state().engine, HandwritingEngine::Detypify);
        harness.get_by_label("Prefer Typst symbol names").click();
        harness.run_steps(2);
        assert!(!harness.state().prefer_typst_names);
        let saved = serde_json::to_string(harness.state()).unwrap();
        assert_eq!(
            serde_json::from_str::<HandwritingSettings>(&saved).unwrap(),
            *harness.state()
        );
        harness.get_by_label("Detexify").click();
        harness.run_steps(2);
        assert_eq!(harness.state().engine, HandwritingEngine::Detexify);
    }
    #[test]
    fn ranked_rows_align_right_and_click_inserts_the_displayed_language() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        for tex in [false, true] {
            let mut drawing = Drawing::fixture();
            drawing.results = vec![symbol("α", "\\alpha", None), symbol("$", "\\$", None)];
            let mut harness = Harness::builder().build_ui_state(
                move |ui, state: &mut (Drawing, Option<String>)| {
                    state.1 = state
                        .0
                        .show(ui, tex, HandwritingSettings::default())
                        .or(state.1.take());
                },
                (drawing, None),
            );
            let alpha = if tex { "\\alpha  α" } else { "alpha  α" };
            for size in [egui::vec2(230.0, 300.0), egui::vec2(500.0, 700.0)] {
                harness.set_size(size);
                harness.run_steps(2);
                let first = harness.get_by_label(alpha).rect();
                let second = harness.get_by_label("\\$  $").rect();
                assert!(second.top() >= first.bottom());
                assert!((first.right() - second.right()).abs() < 0.5);
                assert!(first.left() >= 0.0 && first.right() <= size.x);
                harness.event(egui::Event::PointerMoved(first.center()));
                harness.run_steps(3);
                assert_eq!(harness.get_by_label(alpha).rect(), first);
                assert_eq!(harness.get_by_label("\\$  $").rect(), second);
            }
            let ink = harness.state().0.strokes.clone();
            harness.get_by_label(alpha).click();
            harness.run_steps(2);
            assert_eq!(
                harness.state().1.as_deref(),
                Some(if tex { "\\alpha" } else { "alpha" })
            );
            assert_eq!(harness.state().0.strokes, ink);
            harness.get_by_label("Clear").click();
            harness.run_steps(2);
            assert!(harness.state().0.strokes.is_empty());
            assert!(harness.state().0.results.is_empty());
            assert!(!harness.state().0.pending);
        }
    }
    #[test]
    fn engine_switch_preserves_ink_and_rejects_the_in_flight_result() {
        let mut drawing = Drawing::fixture();
        let ink = drawing.strokes.clone();
        let (release, wait) = std::sync::mpsc::channel();
        drawing
            .job
            .start("old-engine", move || {
                wait.recv().unwrap();
                Ok((0, vec![symbol("α", "\\alpha", None)]))
            })
            .unwrap();
        drawing.configure(HandwritingEngine::Detypify);
        assert!(
            drawing.job.is_running(),
            "wait for the old worker, never overlap engines"
        );
        assert!(drawing.pending);
        assert_eq!(drawing.strokes, ink);
        assert!(drawing.results.is_empty());
        let generation = drawing.generation;
        drawing.configure(HandwritingEngine::Detypify);
        assert_eq!(
            drawing.generation, generation,
            "idle settings cannot resubmit work"
        );
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while drawing.job.is_running() {
            assert!(std::time::Instant::now() < deadline);
            drawing.poll();
            std::thread::yield_now();
        }
        assert!(drawing.results.is_empty());
        assert!(drawing.pending);
    }
    #[test]
    fn stroke_history_restores_exact_ink_and_discards_redo_on_new_input() {
        let mut drawing = Drawing::fixture();
        let original = drawing.strokes.clone();
        drawing.checkpoint();
        drawing.strokes.push(vec![Pos2::new(1.0, 2.0)]);
        let changed = drawing.strokes.clone();
        drawing.history(false);
        assert_eq!(drawing.strokes, original);
        drawing.history(true);
        assert_eq!(drawing.strokes, changed);
        drawing.history(false);
        drawing.checkpoint();
        assert!(drawing.redo.is_empty());
        for _ in 0..100 {
            drawing.checkpoint();
        }
        assert_eq!(drawing.undo.len(), 32);
    }
    #[test]
    fn multiple_character_approximations_have_no_typst_output() {
        let candidate = symbol("⌊⌊", "\\llfloor", Some("\"⌊⌊\""));
        assert!(prediction_insertion(&candidate, false, true, true).is_none());
        assert_eq!(
            prediction_insertion(&candidate, true, true, true).as_deref(),
            Some("\\llfloor")
        );
    }
    #[test]
    fn long_rows_elide_from_the_left_preserving_the_symbol() {
        let measure = |text: &str| text.chars().count() as f32;
        assert_eq!(
            left_elide("triangle.stroked.t  △", 10.0, measure),
            "…oked.t  △"
        );
        assert_eq!(left_elide("ξ", 10.0, measure), "ξ");
        assert_eq!(left_elide("unicode  ξ", 4.0, measure), "…  ξ");
    }
    #[test]
    fn math_names_and_unicode_only_glyphs() {
        let xi = symbol("ξ", "\\xi", None);
        assert_eq!(
            prediction_insertion(&xi, false, true, true).as_deref(),
            Some("xi")
        );
        assert_eq!(
            prediction_insertion(&xi, false, true, false).as_deref(),
            Some("#sym.xi")
        );
        let rows = prediction_rows(&[xi], false, false, true);
        assert_eq!(rows[0].caption, "ξ");
        assert!(HandwritingSettings::default().prefer_math_mode);
    }
    #[test]
    fn drawing_stroke_keeps_predictions_until_replacement() {
        use egui_kittest::Harness;
        let mut drawing = Drawing::fixture();
        drawing.results = vec![symbol("ξ", "\\xi", None)];
        let mut harness = Harness::builder().build_ui_state(
            |ui, drawing: &mut Drawing| {
                drawing.show(ui, false, HandwritingSettings::default());
            },
            drawing,
        );
        harness.run_steps(2);
        let point = egui::pos2(100.0, 180.0);
        harness.event(egui::Event::PointerMoved(point));
        harness.event(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(2);
        harness.event(egui::Event::PointerMoved(point + egui::vec2(20.0, 20.0)));
        harness.run_steps(2);
        assert_eq!(harness.state().results[0].char, "ξ");
        assert!(harness.state().strokes.len() > 1);
    }
    #[test]
    fn selected_engine_returns_only_its_own_ranked_candidates() {
        let strokes = vec![
            vec![egui::pos2(20.0, 30.0), egui::pos2(80.0, 30.0)],
            vec![egui::pos2(50.0, 0.0), egui::pos2(50.0, 100.0)],
        ];
        for engine in HandwritingEngine::ALL {
            let results = recognize(strokes.clone(), engine).unwrap();
            assert_eq!(results.len(), MAX_PREDICTIONS);
            assert!(
                results
                    .iter()
                    .all(|symbol| symbol.detexify == (engine == HandwritingEngine::Detexify))
            );
        }
    }
    #[test]
    fn canvas_resize_preserves_shape_and_pointer_round_trip() {
        for size in [
            egui::vec2(200.0, 300.0),
            egui::vec2(500.0, 800.0),
            egui::vec2(500.0, 100.0),
        ] {
            let rect = egui::Rect::from_min_size(egui::pos2(20.0, 40.0), size);
            let artboard = egui::Rect::from_min_max(Pos2::ZERO, egui::pos2(224.0, 224.0));
            let transform = CanvasTransform::new(rect, artboard);
            assert_eq!(transform.to_screen(egui::pos2(112.0, 112.0)), rect.center());
            let p = egui::pos2(50.0, 150.0);
            assert!(transform.to_model(transform.to_screen(p)).distance(p) < 0.001);
            assert!(
                (transform
                    .to_screen(p)
                    .distance(transform.to_screen(p + egui::vec2(20.0, 0.0)))
                    - 20.0 * size.min_elem() / 224.0)
                    .abs()
                    < 0.001
            );
        }
    }
    #[test]
    fn resizing_between_wide_and_tall_panels_keeps_all_ink_visible() {
        for paper in [egui::vec2(224.0, 900.0), egui::vec2(900.0, 224.0)] {
            let artboard = egui::Rect::from_min_size(Pos2::ZERO, paper);
            let viewport =
                egui::Rect::from_min_size(egui::pos2(30.0, 50.0), egui::vec2(paper.y, paper.x));
            let transform = CanvasTransform::new(viewport, artboard);
            assert!(
                viewport
                    .expand(0.001)
                    .contains(transform.to_screen(artboard.min))
            );
            assert!(
                viewport
                    .expand(0.001)
                    .contains(transform.to_screen(artboard.max))
            );
        }
    }
    #[test]
    fn bundled_model_recognizes_a_dagger_offline() {
        let strokes = vec![
            vec![egui::pos2(20.0, 30.0), egui::pos2(80.0, 30.0)],
            vec![egui::pos2(50.0, 0.0), egui::pos2(50.0, 100.0)],
        ];
        let results = recognize(strokes, HandwritingEngine::Detypify).unwrap();
        assert!(
            results.iter().any(|s| s.char == "†"),
            "{:?}",
            results
                .iter()
                .map(|s| (&s.char, &s.tex, &s.typst))
                .collect::<Vec<_>>()
        );
    }
    #[test]
    #[ignore = "opt-in optimized handwriting worker benchmark"]
    fn worker_cost_probe() {
        let strokes = vec![
            vec![egui::pos2(60.0, 80.0), egui::pos2(160.0, 80.0)],
            vec![egui::pos2(110.0, 40.0), egui::pos2(110.0, 180.0)],
        ];
        let start = std::time::Instant::now();
        recognize_symbols(&strokes).unwrap();
        eprintln!("symbols cold: {:?}", start.elapsed());
        let start = std::time::Instant::now();
        crate::handwriting_detexify::recognize(&strokes);
        eprintln!("Detexify cold: {:?}", start.elapsed());
        for engine in HandwritingEngine::ALL {
            let start = std::time::Instant::now();
            for _ in 0..20 {
                std::hint::black_box(recognize(strokes.clone(), engine).unwrap());
            }
            eprintln!(
                "engine={engine:?}, warm mean over 20: {:?}",
                start.elapsed() / 20
            );
        }
    }
    #[test]
    fn empty_drawing_is_finite_and_black() {
        assert!(raster(&[]).iter().all(|v| *v == 0.0));
        assert!(
            raster(&[vec![egui::pos2(20.0, 30.0)]])
                .iter()
                .any(|v| *v > 0.0)
        );
    }
}
