//! One shared, instanced GPU pass. Particle motion is analytic in the vertex
//! shader; callbacks run on egui's GL owner and never wait for a CPU worker.
use eframe::{
    egui::{self, Color32, Id, Rect, Ui},
    glow::{self, HasContext as _},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};

pub(super) type Instance = [f32; 8];
const MAX_INSTANCES: usize = super::LIMIT;
const VERTEX: &str = include_str!("particles.vert");
const FRAGMENT: &str = include_str!("particles.frag");

#[derive(Default)]
struct Backend {
    pipeline: Option<Pipeline>,
    error: Option<String>,
    draws: u64,
    uploads: u64,
    uploaded_bytes_total: u64,
    uploaded_instances: usize,
    last_draw_instances: usize,
}
#[derive(Default)]
struct Shared {
    // 0 cold, 1 ready, 2 unavailable, 3 destroyed. Idle warm-up checks need no lock.
    status: AtomicU8,
    backend: Mutex<Backend>,
}
#[derive(Clone, Default)]
pub(crate) struct Renderer(Arc<Shared>);
impl Renderer {
    pub(crate) fn for_context(context: &egui::Context) -> Self {
        context.data_mut(|data| {
            let id = Id::new("editor-particle-gpu");
            if let Some(renderer) = data.get_temp::<Self>(id) {
                renderer
            } else {
                let renderer = Self::default();
                data.insert_temp(id, renderer.clone());
                renderer
            }
        })
    }

    pub(super) fn is_unavailable(&self) -> bool {
        self.0.status.load(Ordering::Acquire) >= 2
    }

    #[cfg(test)]
    pub(super) fn simulate_unavailable_for_tests(&self) {
        self.0.status.store(2, Ordering::Release);
    }

    // Compile while the option is being enabled, before the first keystroke.
    // A discarded layout pass may discard its callback too, so only the actual
    // render callback marks initialization complete. No idle repaint is requested.
    pub(super) fn warm(&self, ui: &Ui, clip: Rect) {
        if self.0.status.load(Ordering::Acquire) != 0 || !clip.is_positive() {
            return;
        }
        let renderer = self.clone();
        ui.painter().with_clip_rect(clip).add(egui::PaintCallback {
            rect: clip,
            callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_, painter| {
                renderer.initialize(painter.gl());
            })),
        });
    }

    fn initialize(&self, gl: &glow::Context) {
        if self.0.status.load(Ordering::Acquire) != 0 {
            return;
        }
        let mut backend = self.0.backend.lock().unwrap();
        if self.0.status.load(Ordering::Acquire) != 0 {
            return;
        }
        match Pipeline::new(gl) {
            Ok(pipeline) => {
                backend.pipeline = Some(pipeline);
                self.0.status.store(1, Ordering::Release);
            }
            Err(error) => {
                eprintln!("Typing particle shader unavailable: {error}");
                backend.error = Some(error);
                self.0.status.store(2, Ordering::Release);
            }
        }
    }

    pub(super) fn submit(
        &self,
        ui: &Ui,
        clip: Rect,
        instances: Arc<[Instance]>,
        time: f32,
        accent: Color32,
        dark: bool,
    ) {
        if instances.is_empty()
            || instances.len() > MAX_INSTANCES
            || !clip.is_finite()
            || !clip.is_positive()
            || !time.is_finite()
            || self.0.status.load(Ordering::Acquire) >= 2
        {
            return;
        }
        let renderer = self.clone();
        ui.painter().with_clip_rect(clip).add(egui::PaintCallback {
            rect: clip,
            callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_, painter| {
                renderer.initialize(painter.gl());
                if renderer.0.status.load(Ordering::Acquire) != 1 {
                    return;
                }
                let mut backend = renderer.0.backend.lock().unwrap();
                let Some(pipeline) = backend.pipeline.as_mut() else {
                    return;
                };
                let uploaded = pipeline.draw(painter.gl(), clip, &instances, time, accent, dark);
                backend.draws += 1;
                backend.last_draw_instances = instances.len();
                if uploaded {
                    backend.uploads += 1;
                    backend.uploaded_instances = instances.len();
                    backend.uploaded_bytes_total += std::mem::size_of_val(&*instances) as u64;
                }
            })),
        });
    }

    /// Called only by the process owner while its GL context is current.
    /// Taking the pipeline makes teardown idempotent and late callbacks inert.
    pub(crate) fn destroy(&self, gl: &glow::Context) {
        let mut backend = self.0.backend.lock().unwrap();
        self.0.status.store(3, Ordering::Release);
        if let Some(pipeline) = backend.pipeline.take() {
            pipeline.destroy(gl);
        }
    }

    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
    pub(super) fn inspected(&self) -> serde_json::Value {
        let backend = self.0.backend.lock().unwrap();
        serde_json::json!({
            "backend": "instanced-glow",
            "ready": self.0.status.load(Ordering::Acquire) == 1,
            "draws": backend.draws,
            "uploads": backend.uploads,
            "uploaded_bytes_total": backend.uploaded_bytes_total,
            "uploaded_instances": backend.uploaded_instances,
            "last_draw_instances": backend.last_draw_instances,
            "error": backend.error,
        })
    }
}

struct Pipeline {
    program: glow::Program,
    vao: glow::VertexArray,
    buffer: glow::Buffer,
    viewport: Option<glow::UniformLocation>,
    time: Option<glow::UniformLocation>,
    color: Option<glow::UniformLocation>,
    dark: Option<glow::UniformLocation>,
    uploaded: Option<Arc<[Instance]>>,
}
impl Pipeline {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let program = gl.create_program()?;
            let prefix = if gl.version().is_embedded {
                "#version 300 es\nprecision highp float;\n"
            } else {
                "#version 150\n"
            };
            let mut shaders = Vec::with_capacity(2);
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX),
                (glow::FRAGMENT_SHADER, FRAGMENT),
            ] {
                let result = gl.create_shader(kind).and_then(|shader| {
                    gl.shader_source(
                        shader,
                        &format!(
                            "{prefix}#define PARTICLE_LIFETIME {:.9}\n{source}",
                            super::LIFETIME
                        ),
                    );
                    gl.compile_shader(shader);
                    if gl.get_shader_compile_status(shader) {
                        Ok(shader)
                    } else {
                        let error = gl.get_shader_info_log(shader);
                        gl.delete_shader(shader);
                        Err(error)
                    }
                });
                match result {
                    Ok(shader) => {
                        gl.attach_shader(program, shader);
                        shaders.push(shader);
                    }
                    Err(error) => {
                        for shader in shaders {
                            gl.delete_shader(shader);
                        }
                        gl.delete_program(program);
                        return Err(error);
                    }
                }
            }
            gl.bind_attrib_location(program, 0, "a_motion");
            gl.bind_attrib_location(program, 1, "a_shape");
            gl.link_program(program);
            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }
            if !gl.get_program_link_status(program) {
                let error = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(error);
            }
            let vao = match gl.create_vertex_array() {
                Ok(vao) => vao,
                Err(error) => {
                    gl.delete_program(program);
                    return Err(error);
                }
            };
            let buffer = match gl.create_buffer() {
                Ok(buffer) => buffer,
                Err(error) => {
                    gl.delete_vertex_array(vao);
                    gl.delete_program(program);
                    return Err(error);
                }
            };
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            for (index, offset) in [(0, 0), (1, 16)] {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_pointer_f32(index, 4, glow::FLOAT, false, 32, offset);
                gl.vertex_attrib_divisor(index, 1);
            }
            gl.bind_vertex_array(None);
            Ok(Self {
                program,
                vao,
                buffer,
                viewport: gl.get_uniform_location(program, "u_viewport"),
                time: gl.get_uniform_location(program, "u_time"),
                color: gl.get_uniform_location(program, "u_color"),
                dark: gl.get_uniform_location(program, "u_dark"),
                uploaded: None,
            })
        }
    }

    fn draw(
        &mut self,
        gl: &glow::Context,
        clip: Rect,
        instances: &Arc<[Instance]>,
        time: f32,
        accent: Color32,
        dark: bool,
    ) -> bool {
        let upload = self
            .uploaded
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, instances));
        unsafe {
            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vao));
            if upload {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
                // Arrays of f32 are contiguous and have no padding; GL copies
                // these bytes before this immutable snapshot can be released.
                let bytes = std::slice::from_raw_parts(
                    instances.as_ptr().cast::<u8>(),
                    std::mem::size_of_val(&**instances),
                );
                // Replace the tiny stream buffer instead of modifying storage
                // an earlier GPU frame may still be reading. The driver can
                // orphan that allocation without synchronizing with the CPU.
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
                self.uploaded = Some(instances.clone());
            }
            gl.uniform_4_f32(
                self.viewport.as_ref(),
                clip.left(),
                clip.top(),
                clip.width(),
                clip.height(),
            );
            gl.uniform_1_f32(self.time.as_ref(), time);
            gl.uniform_3_f32(
                self.color.as_ref(),
                f32::from(accent.r()) / 255.0,
                f32::from(accent.g()) / 255.0,
                f32::from(accent.b()) / 255.0,
            );
            gl.uniform_1_f32(self.dark.as_ref(), if dark { 1.0 } else { 0.0 });
            // Multiply on light pages retains dark text. Additive glow on dark
            // pages makes bright sparks instead of darkening an already dark UI.
            // Multiply preserves destination alpha (atop the app), and additive
            // alpha grows with RGB so translucent windows stay premultiplied.
            gl.blend_func_separate(
                if dark { glow::ONE } else { glow::DST_COLOR },
                if dark {
                    glow::ONE
                } else {
                    glow::ONE_MINUS_SRC_ALPHA
                },
                if dark { glow::ONE } else { glow::ZERO },
                glow::ONE,
            );
            gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, instances.len() as i32);
            gl.bind_vertex_array(None);
            // egui_glow restores its paint state immediately after the callback.
            // Keep its framebuffer and scissor untouched throughout this pass.
        }
        upload
    }

    fn destroy(self, gl: &glow::Context) {
        unsafe {
            gl.delete_buffer(self.buffer);
            gl.delete_vertex_array(self.vao);
            gl.delete_program(self.program);
        }
    }
}
