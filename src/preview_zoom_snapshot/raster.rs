//! Bounded, owner-independent bitmap preparation. No AppKit/WebKit objects cross threads.
use eframe::egui::{Context, ViewportId};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGBlendMode, CGColorSpace, CGContext,
    CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, CGInterpolationQuality,
};
use std::thread;

use crate::worker::{LatestReceiver, LatestSender, latest_channel};

pub(super) struct Request {
    pub token: String,
    pub revision: u64,
    pub source: CFRetained<CGImage>,
    pub space: CFRetained<CGColorSpace>,
    pub wake: Option<(Context, ViewportId)>,
}

pub(super) struct Prepared {
    pub token: String,
    pub revision: u64,
    pub source: CFRetained<CGImage>,
    pub space: CFRetained<CGColorSpace>,
    pub image: Option<CFRetained<CGImage>>,
}

pub(super) struct Worker {
    requests: LatestSender<Request>,
    results: LatestReceiver<Prepared>,
    #[cfg(test)]
    thread_id: thread::ThreadId,
}
impl Worker {
    pub fn new() -> std::io::Result<Self> {
        Self::spawn_with(|request| prepare(&request.source, &request.space))
    }

    fn spawn_with(
        work: impl Fn(&Request) -> Option<CFRetained<CGImage>> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (requests, receiver) = latest_channel::<Request>();
        let (sender, results) = latest_channel();
        let _worker = thread::Builder::new()
            .name("tiptoptyp-preview-raster".into())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    let image =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&request)))
                            .ok()
                            .flatten();
                    let result = Prepared {
                        token: request.token,
                        revision: request.revision,
                        source: request.source,
                        space: request.space,
                        image,
                    };
                    if sender.send(result).is_err() {
                        break;
                    }
                    if let Some((context, viewport)) = request.wake {
                        context.request_repaint_of(viewport);
                    }
                }
            })?;
        Ok(Self {
            requests,
            results,
            #[cfg(test)]
            thread_id: _worker.thread().id(),
        })
    }

    #[cfg(test)]
    pub(super) fn thread_id(&self) -> thread::ThreadId {
        self.thread_id
    }

    pub fn request(&self, request: Request) -> bool {
        self.requests.send(request).is_ok()
    }

    pub fn poll(&self) -> Result<Option<Prepared>, ()> {
        match self.results.try_recv() {
            Ok(prepared) => Ok(Some(prepared)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(()),
        }
    }
}

fn bitmap_geometry(width: usize, height: usize) -> Option<(usize, usize)> {
    let pixels = width.checked_mul(height)?;
    if width == 0 || height == 0 || pixels > super::MAX_PIXELS as usize {
        return None;
    }
    let stride = width.checked_mul(4)?.checked_add(63)? & !63;
    let bytes = stride.checked_mul(height)?;
    (bytes <= super::MAX_PIXELS as usize * 4).then_some((stride, bytes))
}

// Render once into native BGRA8 with premultiplied alpha and the window's
// display color space. CALayer can consume this eager bitmap rather than
// lazily color-converting the WebKit provider during its main-thread commit.
fn prepare(source: &CGImage, space: &CGColorSpace) -> Option<CFRetained<CGImage>> {
    let width = CGImage::width(Some(source));
    let height = CGImage::height(Some(source));
    let (stride, _) = bitmap_geometry(width, height)?;
    let context = unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            width,
            height,
            8,
            stride,
            Some(space),
            CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0,
        )
    }?;
    CGContext::set_blend_mode(Some(&context), CGBlendMode::Copy);
    CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::None);
    CGContext::draw_image(
        Some(&context),
        CGRect {
            origin: CGPoint::new(0.0, 0.0),
            size: CGSize::new(width as f64, height as f64),
        },
        Some(source),
    );
    CGBitmapContextCreateImage(Some(&context))
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::CFData;
    use objc2_core_graphics::{
        CGBitmapInfo, CGColor, CGColorRenderingIntent, CGDataProvider, kCGColorSpaceDisplayP3,
        kCGColorSpaceSRGB,
    };
    use std::{
        sync::{Arc, Mutex, mpsc},
        time::Duration,
    };

    fn srgb() -> CFRetained<CGColorSpace> {
        CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).unwrap()
    }
    fn fixture() -> CFRetained<CGImage> {
        let bytes =
            CFData::from_bytes(&[255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 255, 0, 0, 0, 0]);
        let provider = CGDataProvider::with_cf_data(Some(&bytes)).unwrap();
        unsafe {
            CGImage::new(
                2,
                2,
                8,
                32,
                8,
                Some(&srgb()),
                CGBitmapInfo(CGImageAlphaInfo::Last.0 | CGImageByteOrderInfo::Order32Big.0),
                Some(&provider),
                std::ptr::null(),
                false,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }
        .unwrap()
    }
    fn pixels(image: &CGImage) -> Vec<[u8; 4]> {
        let data = CGDataProvider::data(CGImage::data_provider(Some(image)).as_deref()).unwrap();
        let bytes = data.to_vec();
        let stride = CGImage::bytes_per_row(Some(image));
        (0..CGImage::height(Some(image)))
            .flat_map(|y| {
                let bytes = &bytes;
                (0..CGImage::width(Some(image)))
                    .map(move |x| bytes[y * stride + x * 4..][..4].try_into().unwrap())
            })
            .collect()
    }
    #[test]
    fn eager_bitmap_preserves_orientation_and_premultiplies_alpha() {
        let prepared = prepare(&fixture(), &srgb()).unwrap();
        assert_eq!(CGImage::bits_per_component(Some(&prepared)), 8);
        assert_eq!(CGImage::bits_per_pixel(Some(&prepared)), 32);
        assert_eq!(
            CGImage::alpha_info(Some(&prepared)),
            CGImageAlphaInfo::PremultipliedFirst
        );
        assert_eq!(CGImage::bytes_per_row(Some(&prepared)) % 64, 0);
        let output = pixels(&prepared);
        for (actual, expected) in output.iter().zip([
            [0, 0, 255, 255],
            [0, 128, 0, 128],
            [255, 0, 0, 255],
            [0, 0, 0, 0],
        ]) {
            for (&actual, expected) in actual.iter().zip(expected) {
                assert!(
                    (i16::from(actual) - expected).abs() <= 1,
                    "orientation/alpha changed: {output:?}"
                );
            }
        }
    }
    fn scalar_color_match(pixel: [u8; 4], from: &CGColorSpace, to: &CGColorSpace) -> [u8; 4] {
        let alpha = f64::from(pixel[3]);
        if alpha == 0.0 {
            return [0; 4];
        }
        // CGColor's components are straight RGBA, whereas the prepared native
        // image stores premultiplied BGRA. This scalar API never draws a bitmap.
        let rgba = [
            f64::from(pixel[2]) / alpha,
            f64::from(pixel[1]) / alpha,
            f64::from(pixel[0]) / alpha,
            alpha / 255.0,
        ];
        let color = unsafe { CGColor::new(Some(from), rgba.as_ptr()) }.unwrap();
        let matched = unsafe {
            CGColor::new_copy_by_matching_to_color_space(
                Some(to),
                CGColorRenderingIntent::RenderingIntentDefault,
                Some(&color),
                None,
            )
        }
        .unwrap();
        assert_eq!(CGColor::number_of_components(Some(&matched)), 4);
        let components =
            unsafe { std::slice::from_raw_parts(CGColor::components(Some(&matched)), 4) };
        let alpha = components[3].clamp(0.0, 1.0) * 255.0;
        [2, 1, 0, 3].map(|channel| {
            if channel == 3 {
                alpha.round() as u8
            } else {
                (components[channel].clamp(0.0, 1.0) * alpha).round() as u8
            }
        })
    }

    fn assert_scalar_conversion(
        actual: &[[u8; 4]],
        input: &[[u8; 4]],
        from: &CGColorSpace,
        to: &CGColorSpace,
        max_error: i16,
    ) {
        let expected: Vec<_> = input
            .iter()
            .map(|&pixel| scalar_color_match(pixel, from, to))
            .collect();
        let errors: Vec<[i16; 4]> = actual
            .iter()
            .zip(&expected)
            .map(|(actual, expected)| {
                std::array::from_fn(|channel| {
                    i16::from(actual[channel]) - i16::from(expected[channel])
                })
            })
            .collect();
        assert_eq!(actual.len(), expected.len());
        assert!(
            errors
                .iter()
                .flatten()
                .all(|error| error.abs() <= max_error),
            "bitmap conversion differs from scalar color matching: input={input:?}; scalar={expected:?}; bitmap={actual:?}; signed BGRA errors={errors:?}; allowed={max_error}"
        );
    }

    #[test]
    fn pixel_preparation_uses_target_display_profile_and_round_trips_color() {
        let target = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceDisplayP3 })).unwrap();
        let prepared = prepare(&fixture(), &target).unwrap();
        assert_eq!(
            CGImage::color_space(Some(&prepared)).as_deref(),
            Some(&*target)
        );
        assert_ne!(
            pixels(&prepared)[0],
            [0, 0, 255, 255],
            "sRGB red must be converted to Display P3"
        );
        let source_space = srgb();
        let source = pixels(&prepare(&fixture(), &source_space).unwrap());
        let converted = pixels(&prepared);
        // Production's single forward conversion matches the independent
        // scalar converter within two 8-bit code values.
        assert_scalar_conversion(&converted, &source, &source_space, &target, 2);
        let restored = pixels(&prepare(&prepared, &source_space).unwrap());
        // Near a gamut boundary, quantizing a half-transparent wide-gamut
        // sample loses information. Compare its return leg with the scalar
        // conversion of those actual quantized pixels, not a lossless round trip.
        // This test-only inverse conversion unpremultiplies quantized P3.
        // At the half-transparent green gamut boundary, the independent
        // scalar and fixed-point bitmap ICC paths differ by three code values
        // (scalar red=5, bitmap red=8); the forward production limit stays two.
        assert_scalar_conversion(&restored, &converted, &target, &source_space, 3);
        for (actual, original) in restored.iter().zip(&source) {
            assert_eq!(
                actual[3], original[3],
                "profile conversion must preserve alpha"
            );
        }
    }
    #[test]
    fn bitmap_work_is_bounded_before_allocating() {
        assert_eq!(bitmap_geometry(2, 2), Some((64, 128)));
        assert_eq!(bitmap_geometry(1920, 1080), Some((7680, 8_294_400)));
        assert!(
            bitmap_geometry(1, 4_000_000).is_none(),
            "row padding must not multiply the memory bound"
        );
        for (w, h) in [
            (0, 1),
            (1, 0),
            (usize::MAX, 2),
            (4_000_001, 1),
            (2001, 2000),
        ] {
            assert!(bitmap_geometry(w, h).is_none());
        }
    }
    #[test]
    fn one_persistent_worker_drops_superseded_pending_requests_and_results() {
        let owner = thread::current().id();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = Worker::spawn_with(move |request| {
            observed
                .lock()
                .unwrap()
                .push((request.revision, thread::current().id()));
            if request.revision == 1 {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }
            let image = prepare(&request.source, &request.space);
            done_tx.send(()).unwrap();
            image
        })
        .unwrap();
        let request = |revision: u64| Request {
            token: revision.to_string(),
            revision,
            source: fixture(),
            space: srgb(),
            wake: None,
        };
        assert!(worker.request(request(1)));
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        for revision in 2..=100 {
            assert!(worker.request(request(revision)));
        }
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let result = worker
            .results
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        let result = if result.revision == 1 {
            worker
                .results
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
        } else {
            result
        };
        assert_eq!(result.revision, 100);
        assert!(worker.poll().unwrap().is_none());
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.iter().map(|call| call.0).collect::<Vec<_>>(),
            [1, 100]
        );
        assert_ne!(calls[0].1, owner);
        assert_eq!(calls[0].1, calls[1].1);
    }
}
