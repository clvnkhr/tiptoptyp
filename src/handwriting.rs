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
}
fn symbols() -> &'static [Symbol] {
    static SYMBOLS: OnceLock<Vec<Symbol>> = OnceLock::new();
    SYMBOLS.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/handwriting/symbols.json"))
            .expect("bundled symbol metadata")
    })
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
        for pair in stroke.windows(2) {
            let a = egui::pos2(112.0, 112.0) + (pair[0] - bounds.center()) * scale;
            let b = egui::pos2(112.0, 112.0) + (pair[1] - bounds.center()) * scale;
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
fn recognize(strokes: Vec<Vec<Pos2>>) -> Result<Vec<Symbol>, String> {
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
    let pixels = raster(&strokes);
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
    Ok(ranking
        .into_iter()
        .take(8)
        .map(|(index, _)| symbols()[index].clone())
        .collect())
}
#[derive(Default)]
pub(crate) struct Drawing {
    strokes: Vec<Vec<Pos2>>,
    job: LatestJob<(u64, Vec<Symbol>)>,
    generation: u64,
    pending: bool,
    results: Vec<Symbol>,
    error: Option<String>,
}
impl Drawing {
    pub(crate) fn fixture() -> Self {
        Self {
            strokes: vec![
                vec![egui::pos2(60.0, 80.0), egui::pos2(160.0, 80.0)],
                vec![egui::pos2(110.0, 40.0), egui::pos2(110.0, 180.0)],
            ],
            results: symbols()
                .iter()
                .filter(|symbol| matches!(symbol.char.as_str(), "†" | "‡" | "⊥"))
                .cloned()
                .collect(),
            ..Self::default()
        }
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
                self.results.clear();
                self.error = None;
                self.generation += 1;
                self.pending = false;
            }
        });
        let width = ui.available_width().clamp(32.0, 224.0);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, width), egui::Sense::drag());
        ui.painter()
            .rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        if response.drag_started() {
            let start = ui
                .input(|input| input.pointer.press_origin())
                .map(|point| rect.clamp(point) - rect.min.to_vec2());
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
                let p = rect.clamp(point) - rect.min.to_vec2();
                if stroke.last().is_none_or(|last| last.distance(p) > 0.5) {
                    stroke.push(p);
                }
            }
        }
        for stroke in &self.strokes {
            ui.painter().add(egui::Shape::line(
                stroke.iter().map(|p| *p + rect.min.to_vec2()).collect(),
                egui::Stroke::new(2.0, ui.visuals().text_color()),
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
        if let Some(error) = &self.error {
            ui.label(error);
        }
        if self.job.is_running() {
            ui.label("Recognizing…");
        }
        for symbol in &self.results {
            let name = if tex {
                symbol.tex.as_deref().unwrap_or(&symbol.char)
            } else {
                &symbol.names[0]
            };
            if ui
                .button(format!("{}  {}", symbol.char, name))
                .on_hover_text("Insert this symbol at the editor cursor")
                .clicked()
            {
                selected = Some(if tex {
                    symbol.tex.clone().unwrap_or_else(|| symbol.char.clone())
                } else {
                    symbol.char.clone()
                });
            }
        }
        selected
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
    fn empty_drawing_is_finite_and_black() {
        assert!(raster(&[]).iter().all(|v| *v == 0.0));
    }
}
