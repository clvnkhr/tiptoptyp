//! Font samples use a private atlas, never the application's font definitions.
use crate::{
    font_catalog::FontFamily,
    worker::{LatestJob, LatestJobPoll},
};
use eframe::egui::{
    self,
    epaint::text::{Fonts, TextOptions},
};
use std::{
    collections::{BTreeMap, VecDeque},
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex},
};

const SAMPLE_TEXT: &str = "Aa Bb Cc · 0123456789";
const SAMPLE_SIZE: f32 = 20.0;
const CACHE_SIZE: usize = 4;
const MAX_FONT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
struct Request {
    path: PathBuf,
    index: u32,
    pixels_per_point: f32,
    options: TextOptions,
}

struct RasterizedSample {
    atlas: egui::ColorImage,
    mesh: egui::Mesh,
    size: egui::Vec2,
}

struct Sample {
    // Own the texture for as long as the cached mesh refers to it.
    _texture: egui::TextureHandle,
    mesh: egui::Mesh,
    size: egui::Vec2,
}

#[derive(Default)]
struct Slot {
    requested: Option<Request>,
    samples: VecDeque<(Request, Sample)>,
    job: LatestJob<RasterizedSample>,
    error: Option<String>,
}

#[derive(Default)]
struct Previews {
    slots: BTreeMap<(egui::ViewportId, String), Slot>,
}

fn state(context: &egui::Context) -> Arc<Mutex<Previews>> {
    context.data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<Previews>>>(egui::Id::new("font-preview-cache"))
            .clone()
    })
}

fn rasterize(request: &Request) -> Result<RasterizedSample, String> {
    let file = std::fs::File::open(&request.path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_FONT_BYTES {
        return Err("This font is too large to preview".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_FONT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FONT_BYTES {
        return Err("This font is too large to preview".into());
    }
    skrifa::FontRef::from_index(&bytes, request.index).map_err(|e| e.to_string())?;
    let mut data = egui::FontData::from_owned(bytes);
    data.index = request.index;
    let mut definitions = egui::FontDefinitions::default();
    definitions
        .font_data
        .insert("sample".into(), Arc::new(data));
    definitions
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .unwrap()
        .insert(0, "sample".into());

    // Reuse egui's shaping, collection-face selection, hinting and fallback
    // behavior, but rasterize into a worker-owned atlas. Calling set_fonts on
    // the UI context here would evict every viewport's cached text layout.
    let mut fonts = Fonts::new(request.options, definitions);
    let galley = fonts
        .with_pixels_per_point(request.pixels_per_point)
        .layout_no_wrap(
            SAMPLE_TEXT.into(),
            egui::FontId::proportional(SAMPLE_SIZE),
            egui::Color32::WHITE,
        );
    let bounds = galley.rect.union(galley.mesh_bounds);
    let atlas = fonts.image();
    let atlas_size = egui::vec2(atlas.size[0] as f32, atlas.size[1] as f32);
    let mut mesh = egui::Mesh::default();
    for row in &galley.rows {
        let mut row_mesh = row.visuals.mesh.clone();
        for vertex in &mut row_mesh.vertices {
            vertex.pos += row.pos.to_vec2() - bounds.min.to_vec2();
            // Row meshes store texel UVs; ordinary texture meshes use 0..1.
            vertex.uv = (vertex.uv.to_vec2() / atlas_size).to_pos2();
        }
        mesh.append(row_mesh);
    }
    Ok(RasterizedSample {
        atlas,
        mesh,
        size: bounds.size(),
    })
}

impl Slot {
    fn update(&mut self, context: &egui::Context, request: &Request) {
        if self.requested.as_ref() != Some(request) {
            self.requested = Some(request.clone());
            self.error = None;
            self.job.cancel();
            if let Some(index) = self.samples.iter().position(|(key, _)| key == request) {
                let cached = self.samples.remove(index).unwrap();
                self.samples.push_front(cached);
            } else {
                let request = request.clone();
                if let Err(error) = self
                    .job
                    .start_and_repaint("font-preview", context, move || rasterize(&request))
                {
                    self.error = Some(error);
                }
            }
        }
        match self.job.poll() {
            LatestJobPoll::Ready(mut sample) => {
                let texture = context.load_texture(
                    "font-preview",
                    sample.atlas,
                    egui::TextureOptions::LINEAR,
                );
                sample.mesh.texture_id = texture.id();
                self.samples.push_front((
                    request.clone(),
                    Sample {
                        _texture: texture,
                        mesh: sample.mesh,
                        size: sample.size,
                    },
                ));
                self.samples.truncate(CACHE_SIZE);
            }
            LatestJobPoll::Failed(error) => self.error = Some(error),
            LatestJobPoll::Pending | LatestJobPoll::Idle => {}
        }
    }
}

pub(crate) fn show(ui: &mut egui::Ui, slot_name: &str, family: &FontFamily) -> bool {
    let Some(face) = family.primary_face() else {
        return false;
    };
    let mut options = ui.visuals().text_options;
    // A short sample needs only a small atlas. Cap raster resolution while
    // retaining point-sized geometry on high-DPI / highly zoomed displays.
    options.max_texture_side = 1024;
    let request = Request {
        path: face.path.clone(),
        index: face.index,
        pixels_per_point: ui.ctx().pixels_per_point().clamp(0.5, 8.0),
        options,
    };
    let state = state(ui.ctx());
    let mut previews = state.lock().unwrap();
    let slot = previews
        .slots
        .entry((ui.ctx().viewport_id(), slot_name.into()))
        .or_default();
    slot.update(ui.ctx(), &request);
    if let Some((_, sample)) = slot.samples.front().filter(|(key, _)| key == &request) {
        let size = egui::vec2(
            sample.size.x.min(ui.available_width()).max(0.0),
            sample.size.y,
        );
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Image,
                ui.is_enabled(),
                format!("{}: {SAMPLE_TEXT}", family.name),
            )
        });
        let mut mesh = sample.mesh.clone();
        let origin =
            (rect.min.to_vec2() * request.pixels_per_point).round() / request.pixels_per_point;
        mesh.translate(origin);
        for vertex in &mut mesh.vertices {
            vertex.color = ui.visuals().text_color();
        }
        ui.painter()
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .add(mesh);
        true
    } else {
        ui.label(slot.error.as_deref().unwrap_or("Loading font preview…"));
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_catalog::FontCatalog;
    use std::time::{Duration, Instant};

    fn fixture(directory: &std::path::Path, family: egui::FontFamily) -> FontFamily {
        let definitions = egui::FontDefinitions::default();
        let key = &definitions.families[&family][0];
        let path = directory.join(format!("{key}.ttf"));
        std::fs::write(&path, &definitions.font_data[key].font).unwrap();
        FontCatalog::single_font_fixture(&path).families()[0].clone()
    }

    #[test]
    fn isolated_samples_preserve_font_geometry_and_normalized_texture_coordinates() {
        let directory = tempfile::tempdir().unwrap();
        let mut samples = Vec::new();
        for family in [egui::FontFamily::Monospace, egui::FontFamily::Proportional] {
            let family = fixture(directory.path(), family);
            let face = family.primary_face().unwrap();
            for pixels_per_point in [1.0, 2.0] {
                let sample = rasterize(&Request {
                    path: face.path.clone(),
                    index: face.index,
                    pixels_per_point,
                    options: TextOptions {
                        max_texture_side: 1024,
                        ..Default::default()
                    },
                })
                .unwrap();
                assert!(!sample.mesh.is_empty());
                assert!(sample.atlas.pixels.iter().any(|pixel| pixel.a() > 0));
                let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, sample.size).expand(0.01);
                for vertex in &sample.mesh.vertices {
                    assert!(bounds.contains(vertex.pos));
                    assert!((0.0..=1.0).contains(&vertex.uv.x));
                    assert!((0.0..=1.0).contains(&vertex.uv.y));
                }
                samples.push(sample);
            }
        }
        assert!((samples[0].size.x - samples[1].size.x).abs() < 4.0);
        assert!((samples[2].size.x - samples[3].size.x).abs() < 4.0);
        assert_ne!(
            samples[0].mesh, samples[2].mesh,
            "different fonts must produce different samples"
        );
    }

    #[test]
    fn browsing_samples_keeps_custom_text_cached_and_never_updates_the_global_atlas() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let families = [
            fixture(directory.path(), egui::FontFamily::Monospace),
            fixture(directory.path(), egui::FontFamily::Proportional),
        ];
        let mut definitions = egui::FontDefinitions::default();
        for (name, family) in [
            ("custom-ui", egui::FontFamily::Proportional),
            ("custom-code", egui::FontFamily::Monospace),
        ] {
            definitions.families.insert(
                egui::FontFamily::Name(name.into()),
                definitions.families[&family].clone(),
            );
        }
        context.set_fonts(definitions.clone());
        let mut cached_galleys: Option<Vec<Arc<egui::Galley>>> = None;
        let mut frames = 0;
        let mut textures = Vec::new();
        for index in [0, 1, 0, 1, 0] {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut ready = false;
            while !ready && Instant::now() < deadline {
                let output = context.run_ui(Default::default(), |ui| {
                    // Keep both custom font roles warm on every pass, as in
                    // the editor. Preview selection must not evict either.
                    let galleys: Vec<_> = ["custom-ui", "custom-code"]
                        .into_iter()
                        .map(|name| {
                            ui.fonts_mut(|fonts| {
                                fonts.layout_no_wrap(
                                    SAMPLE_TEXT.into(),
                                    egui::FontId::new(14.0, egui::FontFamily::Name(name.into())),
                                    egui::Color32::WHITE,
                                )
                            })
                        })
                        .collect();
                    if let Some(previous) = &cached_galleys {
                        for (previous, current) in std::iter::zip(previous, &galleys) {
                            assert!(
                                Arc::ptr_eq(previous, current),
                                "preview invalidated unrelated text layouts"
                            );
                        }
                    }
                    cached_galleys = Some(galleys);
                    // Warm up the only ordinary label used by the loader.
                    ui.label("Loading font preview…");
                    ready = show(ui, "test-picker", &families[index]);
                    ui.ctx()
                        .fonts(|fonts| assert_eq!(fonts.definitions(), &definitions));
                });
                if frames > 0 {
                    assert!(
                        output
                            .textures_delta
                            .set
                            .iter()
                            .all(|(id, _)| *id != egui::TextureId::default()),
                        "preview uploaded the shared font atlas"
                    );
                }
                output.drop_without_applying_deltas();
                frames += 1;
                if !ready {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            assert!(ready, "sample did not finish loading");
            let previews = state(&context);
            let previews = previews.lock().unwrap();
            assert_eq!(previews.slots.len(), 1);
            let slot = previews.slots.values().next().unwrap();
            assert!(!slot.job.is_running());
            assert!(slot.samples.len() <= 2);
            textures.push(slot.samples.front().unwrap().1.mesh.texture_id);
        }
        assert_ne!(textures[0], textures[1]);
        assert_eq!(textures[0], textures[2]);
        assert_eq!(textures[1], textures[3]);
    }
}
