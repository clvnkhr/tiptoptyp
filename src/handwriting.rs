//! Offline Detypify inference. Model loading and recognition only run on a worker.
use crate::worker::{LatestJob, LatestJobPoll};
use eframe::egui::{self, Pos2};
use std::sync::OnceLock;
use tract_onnx::prelude::*;

#[derive(Clone, serde::Deserialize)]
pub(crate) struct Symbol {
    pub char: String,
    pub names: Vec<String>,
    pub tex: Option<String>,
    #[serde(default)]
    pub typst: Option<String>,
    #[serde(default)]
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
        .take(8)
        .map(|(index, _)| symbols()[index].clone())
        .collect();
    Ok(results)
}
fn recognize(strokes: Vec<Vec<Pos2>>) -> Result<Vec<Symbol>, String> {
    let mut results = recognize_symbols(&strokes)?;
    results.extend(crate::handwriting_detexify::recognize(&strokes));
    Ok(results)
}
#[derive(Default)]
pub(crate) struct Drawing {
    strokes: Vec<Vec<Pos2>>,
    artboard: Option<egui::Rect>,
    job: LatestJob<(u64, Vec<Symbol>)>,
    generation: u64,
    pending: bool,
    results: Vec<Symbol>,
    error: Option<String>,
}
impl Drawing {
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
            results: symbols()
                .iter()
                .filter(|symbol| {
                    matches!(
                        symbol.char.as_str(),
                        "†" | "‡" | "⊥" | "Δ" | "Λ" | "∀" | "∧" | "∠"
                    )
                })
                .cloned()
                .chain(crate::handwriting_detexify::fixture_predictions())
                .collect(),
            ..Self::default()
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(crate) fn inspection(&self) -> serde_json::Value {
        serde_json::json!({
            "strokes": self.strokes.iter().map(|s|s.iter().map(|p|[p.x,p.y]).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "predictions": self.results.iter().enumerate().map(|(index,s)|serde_json::json!({"index":index,"typst":if s.tex_only { None } else { Some(s.typst.as_deref().unwrap_or(&s.char)) },"tex":s.tex,"detexify":s.detexify})).collect::<Vec<_>>(),
            "busy":self.pending || self.job.is_running(),
        })
    }
    pub(crate) fn poll(&mut self) {
        match self.job.poll() {
            LatestJobPoll::Ready((generation, results)) if generation == self.generation => {
                self.results = results;
                self.error = None
            }
            LatestJobPoll::Failed(error) => self.error = Some(error),
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

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, tex: bool) -> Option<String> {
        self.poll();
        let mut selected = None;
        ui.horizontal(|ui| {
            ui.label("Draw a symbol");
            if ui.small_button("Clear").clicked() {
                self.strokes.clear();
                self.artboard = None;
                self.results.clear();
                self.error = None;
                self.generation += 1;
                self.pending = false;
            }
        });
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
            self.results.clear();
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
                    ui.visuals()
                        .extreme_bg_color
                        .lerp_to_gamma(ui.visuals().text_color(), 0.25),
                );
            }
            ui.painter().with_clip_rect(rect).add(egui::Shape::line(
                stroke.iter().map(|p| transform.to_screen(*p)).collect(),
                egui::Stroke::new(
                    4.0,
                    ui.visuals()
                        .extreme_bg_color
                        .lerp_to_gamma(ui.visuals().text_color(), 0.25),
                ),
            ));
        }
        if response.drag_stopped() {
            self.pending = true;
        }
        if self.pending && !self.job.is_running() {
            self.pending = false;
            let strokes = self.strokes.clone();
            let generation = self.generation;
            if let Err(error) =
                self.job
                    .start_and_repaint("symbol-recognition", ui.ctx(), move || {
                        recognize(strokes).map(|results| (generation, results))
                    })
            {
                self.error = Some(error);
            }
        }
        // Paint last so predictions remain readable over the ink. Labels do not
        // select text or occupy extra panel height; clicking still inserts a result.
        let mut overlay = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)));
        overlay.set_clip_rect(rect);
        if let Some(error) = &self.error {
            overlay.label(error);
        }
        if self.job.is_running() {
            overlay.label("Recognizing…");
        }
        let group = |symbol: &Symbol| {
            if !symbol.detexify {
                0
            } else if symbol
                .tex
                .as_deref()
                .is_some_and(|s| s.starts_with("\\mathcal{"))
            {
                2
            } else {
                1
            }
        };
        for (kind, title) in [(0, "Symbols:"), (1, "More symbols:"), (2, "Calligraphic:")] {
            if !self.results.iter().any(|s| group(s) == kind) {
                continue;
            }
            overlay.horizontal_wrapped(|ui| {
                ui.add(egui::Label::new(title).selectable(false));
                for (_index, symbol) in self
                    .results
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| group(s) == kind)
                {
                    let name = if tex || (symbol.detexify && symbol.names[0] == symbol.char) {
                        symbol.tex.as_deref().unwrap_or(&symbol.char)
                    } else {
                        &symbol.names[0]
                    };
                    let prediction = ui
                        .add(
                            egui::Label::new(format!("{}  {}", symbol.char, name))
                                .wrap_mode(egui::TextWrapMode::Extend)
                                .selectable(false)
                                .sense(if tex || !symbol.tex_only {
                                    egui::Sense::click()
                                } else {
                                    egui::Sense::hover()
                                }),
                        )
                        .on_hover_text(if !tex && symbol.tex_only {
                            "No verified Typst mapping yet".to_string()
                        } else if let Some(package) = &symbol.package {
                            format!("Click to insert. LaTeX package: {package}")
                        } else {
                            "Click to insert this symbol".to_string()
                        });
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(
                        &format!("drawing.prediction.{_index}"),
                        &prediction,
                    );
                    if prediction.clicked() {
                        selected = Some(if tex {
                            symbol.tex.clone().unwrap_or_else(|| symbol.char.clone())
                        } else {
                            symbol.typst.clone().unwrap_or_else(|| symbol.char.clone())
                        });
                    }
                    ui.add_space(8.0);
                }
            });
        }
        if response.clicked()
            && selected.is_none()
            && self.strokes.iter().map(Vec::len).sum::<usize>() < 4096
            && let Some(point) = response.interact_pointer_pos()
        {
            self.strokes.push(vec![transform.to_model(point)]);
            self.artboard = Some(egui::Rect::from_min_max(
                transform.to_model(rect.min),
                transform.to_model(rect.max),
            ));
            self.generation += 1;
            self.results.clear();
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
    fn predictions_insert_commands_without_text_selection() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        for tex in [false, true] {
            let mut harness = Harness::builder().build_ui_state(
                |ui, state: &mut (Drawing, Option<String>)| {
                    if let Some(value) = state.0.show(ui, tex) {
                        state.1 = Some(value);
                    }
                },
                (Drawing::fixture(), None),
            );
            harness.run_steps(2);
            let original_strokes = harness.state().0.strokes.clone();
            for size in [egui::vec2(230.0, 300.0), egui::vec2(500.0, 700.0)] {
                harness.set_size(size);
                harness.run_steps(2);
                assert_eq!(harness.state().0.strokes, original_strokes);
            }
            harness
                .get_by_label(if tex {
                    "𝒜  \\mathcal{A}"
                } else {
                    "𝒜  cal(A)"
                })
                .click();
            harness.run_steps(2);
            assert_eq!(
                harness.state().1.as_deref(),
                Some(if tex { "\\mathcal{A}" } else { "cal(A)" })
            );
        }
    }
    #[test]
    fn newly_mapped_predictions_are_clickable_in_typst_and_tex() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let symbols: Vec<Symbol> =
            serde_json::from_str(include_str!("../assets/handwriting/detexify-symbols.json"))
                .unwrap();
        for (command, caption, output) in [
            ("\\mathscr{A}", "  scr(A)", "scr(A)"),
            ("\\textturnv", "ʌ  \\textturnv", "\"ʌ\""),
            ("\\textdollar", "$  \\textdollar", "\"$\""),
        ] {
            for tex in [false, true] {
                let symbol = symbols
                    .iter()
                    .find(|s| s.tex.as_deref() == Some(command))
                    .unwrap()
                    .clone();
                let label = if tex {
                    format!("{}  {command}", symbol.char)
                } else {
                    caption.to_owned()
                };
                let mut drawing = Drawing::fixture();
                drawing.results = vec![symbol];
                let mut harness = Harness::builder().build_ui_state(
                    |ui, state: &mut (Drawing, Option<String>)| {
                        state.1 = state.0.show(ui, tex).or(state.1.take());
                    },
                    (drawing, None),
                );
                harness.run_steps(2);
                harness.get_by_label(&label).click();
                harness.run_steps(2);
                assert_eq!(
                    harness.state().1.as_deref(),
                    Some(if tex { command } else { output })
                );
            }
        }
    }
    #[test]
    fn bundled_model_recognizes_a_dagger_offline() {
        let strokes = vec![
            vec![egui::pos2(20.0, 30.0), egui::pos2(80.0, 30.0)],
            vec![egui::pos2(50.0, 0.0), egui::pos2(50.0, 100.0)],
        ];
        let results = recognize(strokes).unwrap();
        assert!(
            results.iter().any(|s| s.char == "†"),
            "{:?}",
            results.iter().map(|s| &s.names).collect::<Vec<_>>()
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
        for combined in [false, true] {
            let start = std::time::Instant::now();
            for _ in 0..20 {
                if combined {
                    std::hint::black_box(recognize(strokes.clone()).unwrap());
                } else {
                    std::hint::black_box(recognize_symbols(&strokes).unwrap());
                }
            }
            eprintln!(
                "combined={combined}, warm mean over 20: {:?}",
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
