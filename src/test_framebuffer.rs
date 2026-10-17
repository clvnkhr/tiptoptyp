#![allow(unsafe_code)]
//! Opt-in offscreen review using the app's Glow renderer, without opening or
//! activating a window or posting desktop input. PNGs are component viewport
//! framebuffers; they cannot establish native child-view composition.
use eframe::{
    egui,
    glow::{self, HasContext as _},
};
use std::{
    ffi::{CString, c_char, c_int, c_void},
    sync::Arc,
};

#[link(name = "OpenGL", kind = "framework")]
unsafe extern "C" {
    fn CGLChoosePixelFormat(
        attributes: *const u32,
        format: *mut *mut c_void,
        count: *mut c_int,
    ) -> c_int;
    fn CGLDestroyPixelFormat(format: *mut c_void) -> c_int;
    fn CGLCreateContext(
        format: *mut c_void,
        share: *mut c_void,
        context: *mut *mut c_void,
    ) -> c_int;
    fn CGLGetCurrentContext() -> *mut c_void;
    fn CGLSetCurrentContext(context: *mut c_void) -> c_int;
    fn CGLDestroyContext(context: *mut c_void) -> c_int;
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

pub(crate) struct Framebuffer {
    painter: eframe::egui_glow::Painter,
    gl: Arc<glow::Context>,
    framebuffer: glow::Framebuffer,
    texture: glow::Texture,
    context: *mut c_void,
    previous: *mut c_void,
    library: *mut c_void,
    size: [u32; 2],
}
impl Framebuffer {
    pub(crate) fn new(size: [u32; 2]) -> Self {
        // Apple's CGL profile/color/alpha attributes; a private texture-backed
        // framebuffer needs neither an NSApplication nor an on-screen drawable.
        unsafe {
            let attributes = [99, 0x3200, 8, 24, 11, 8, 0];
            let mut format = std::ptr::null_mut();
            let mut count = 0;
            assert_eq!(
                CGLChoosePixelFormat(attributes.as_ptr(), &mut format, &mut count),
                0
            );
            assert!(!format.is_null());
            let mut context = std::ptr::null_mut();
            let status = CGLCreateContext(format, std::ptr::null_mut(), &mut context);
            CGLDestroyPixelFormat(format);
            assert_eq!(status, 0);
            let previous = CGLGetCurrentContext();
            assert_eq!(CGLSetCurrentContext(context), 0);
            let library = dlopen(
                c"/System/Library/Frameworks/OpenGL.framework/OpenGL".as_ptr(),
                1,
            );
            assert!(!library.is_null());
            let gl = Arc::new(glow::Context::from_loader_function(|name| {
                dlsym(library, CString::new(name).unwrap().as_ptr()).cast_const()
            }));
            let framebuffer = gl.create_framebuffer().unwrap();
            let texture = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size[0] as i32,
                size[1] as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            assert_eq!(
                gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE
            );
            let painter = eframe::egui_glow::Painter::new(gl.clone(), "", None, false).unwrap();
            Self {
                painter,
                gl,
                framebuffer,
                texture,
                context,
                previous,
                library,
                size,
            }
        }
    }
    pub(crate) fn gl(&self) -> Arc<glow::Context> {
        Arc::clone(&self.gl)
    }

    pub(crate) fn read_rgba(&self) -> egui::ColorImage {
        self.painter.read_screen_rgba(self.size)
    }

    pub(crate) fn render(
        &mut self,
        context: &egui::Context,
        mut output: egui::FullOutput,
        path: Option<&std::path::Path>,
    ) {
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
        }
        self.painter.clear(self.size, [1.0, 1.0, 1.0, 1.0]);
        self.painter.paint_and_update_textures(
            self.size,
            output.pixels_per_point,
            &primitives,
            &mut output.textures_delta,
        );
        if let Some(path) = path {
            let image = self.painter.read_screen_rgba(self.size);
            let pixels: Vec<u8> = image
                .pixels
                .iter()
                .flat_map(|pixel| pixel.to_array())
                .collect();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            image::save_buffer(
                path,
                &pixels,
                self.size[0],
                self.size[1],
                image::ColorType::Rgba8,
            )
            .unwrap();
            println!("fresh offscreen Glow viewport: {}", path.display());
        }
    }
}
impl Drop for Framebuffer {
    fn drop(&mut self) {
        self.painter.destroy();
        unsafe {
            self.gl.delete_framebuffer(self.framebuffer);
            self.gl.delete_texture(self.texture);
            CGLSetCurrentContext(self.previous);
            CGLDestroyContext(self.context);
            dlclose(self.library);
        }
    }
}
