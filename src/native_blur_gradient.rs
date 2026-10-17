//! A spatial blur behind the renderer, without touching its layer or input tree.
//! The private filter is optional; callers retain their uniform-blur fallback.
use std::{cell::RefCell, ffi::c_void, ptr::NonNull};

use objc2::{
    MainThreadMarker, msg_send,
    rc::{Retained, Weak},
    runtime::{AnyClass, AnyObject},
    sel,
};
use objc2_app_kit::NSView;
use objc2_foundation::{NSArray, NSNumber, NSRect, NSSize, NSString};

use super::FadeDirection;

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDataProviderCreateWithData(
        info: *mut c_void,
        data: *const c_void,
        size: usize,
        release: Option<unsafe extern "C" fn(*mut c_void, *const c_void, usize)>,
    ) -> *mut c_void;
    fn CGDataProviderRelease(provider: *mut c_void);
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: *const c_void,
        bitmap_info: u32,
        provider: *const c_void,
        decode: *const f64,
        interpolate: bool,
        rendering_intent: i32,
    ) -> *mut c_void;
    fn CGImageRelease(image: *mut c_void);
    #[cfg(any(test, feature = "native-ui-tests", feature = "desktop-ui-tests"))]
    fn CGImageGetWidth(image: *const AnyObject) -> usize;
    #[cfg(any(test, feature = "native-ui-tests", feature = "desktop-ui-tests"))]
    fn CGImageGetHeight(image: *const AnyObject) -> usize;
}

#[derive(Clone, Copy)]
struct Runtime {
    backdrop: &'static AnyClass,
    filter: &'static AnyClass,
    transaction: &'static AnyClass,
}
impl Runtime {
    fn load() -> Option<Self> {
        let runtime = Self {
            backdrop: AnyClass::get(c"CABackdropLayer")?,
            filter: AnyClass::get(c"CAFilter")?,
            transaction: AnyClass::get(c"CATransaction")?,
        };
        // Unsupported systems must fall back before sending a private selector.
        let filter_available: bool =
            unsafe { msg_send![runtime.filter, respondsToSelector: sel!(filterWithType:)] };
        (filter_available
            && runtime.backdrop.responds_to(sel!(setWindowServerAware:))
            && runtime.backdrop.responds_to(sel!(setFilters:))
            && runtime.filter.responds_to(sel!(setValue:forKey:)))
        .then_some(runtime)
    }

    fn transaction(self, changes: impl FnOnce()) {
        // No implicit fade/resize animation may lag behind the renderer frame.
        unsafe {
            let _: () = msg_send![self.transaction, begin];
            let _: () = msg_send![self.transaction, setDisableActions: true];
        }
        changes();
        unsafe {
            let _: () = msg_send![self.transaction, commit];
        }
    }

    fn filter(self, radius: u8, image: &MaskImage) -> Option<Retained<AnyObject>> {
        // Core Animation does not reliably notice mutations of a reused
        // CAFilter. Replace it only when the radius/direction/mask key changes.
        let filter: Option<Retained<AnyObject>> =
            unsafe { msg_send![self.filter, filterWithType: &*NSString::from_str("variableBlur")] };
        let filter = filter?;
        unsafe {
            let _: () = msg_send![&*filter,
                setValue: &*NSNumber::new_f64(f64::from(radius)),
                forKey: &*NSString::from_str("inputRadius")];
            // CGImage is a CF object accepted by CAFilter's KVC interface. The
            // filter retains it; MaskImage also owns it until replacement.
            let _: () = msg_send![&*filter,
                setValue: image.0.as_ptr().cast::<AnyObject>(),
                forKey: &*NSString::from_str("inputMaskImage")];
            let _: () = msg_send![&*filter,
                setValue: &*NSNumber::new_bool(true),
                forKey: &*NSString::from_str("inputNormalizeEdges")];
        }
        Some(filter)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct MaskKey {
    radius: u8,
    end_radius: u8,
    fade: FadeDirection,
    width: usize,
    height: usize,
}
impl MaskKey {
    fn new(radius: u8, end_radius: u8, fade: FadeDirection, size: NSSize) -> Option<Self> {
        let (width, height) = mask_dimensions(size.width, size.height)?;
        Some(Self {
            radius,
            end_radius,
            fade,
            width,
            height,
        })
    }

    fn maximum(self) -> u8 {
        self.radius.max(self.end_radius)
    }

    fn alpha(self, factor: f32) -> u8 {
        let maximum = self.maximum();
        if maximum == 0 {
            return 0;
        }
        let start = f32::from(self.radius) / f32::from(maximum);
        let end = f32::from(self.end_radius) / f32::from(maximum);
        ((end + (start - end) * factor) * 255.0).round() as u8
    }

    fn same_mask(self, other: Self) -> bool {
        if self.fade != other.fade || self.width != other.width || self.height != other.height {
            return false;
        }
        let maximum = u16::from(self.maximum());
        let other_maximum = u16::from(other.maximum());
        if maximum == 0 || other_maximum == 0 {
            return maximum == other_maximum;
        }
        // Normalize with integer cross-products so proportional ranges retain
        // exactly the same mask without a floating-point cache comparison.
        u16::from(self.radius) * other_maximum == u16::from(other.radius) * maximum
            && u16::from(self.end_radius) * other_maximum == u16::from(other.end_radius) * maximum
    }
}

struct State {
    key: MaskKey,
    frame: NSRect,
    scale: f64,
    _image: MaskImage,
}

pub(super) struct GradientBlur {
    renderer: Weak<NSView>,
    backdrop: Retained<AnyObject>,
    runtime: Runtime,
    state: RefCell<State>,
}
impl GradientBlur {
    /// A missing host or an initially empty renderer may become usable later.
    /// This cheap key lets the owner bound retries to attachment/validity changes.
    pub(super) fn attachment_key(view: &NSView) -> (usize, bool) {
        let size = view.frame().size;
        let valid = mask_dimensions(size.width, size.height).is_some();
        let layer = unsafe { view.superview() }.map_or(std::ptr::null(), |parent| unsafe {
            let layer: *const AnyObject = msg_send![&*parent, layer];
            layer
        });
        (layer as usize, valid)
    }

    pub(super) fn new(
        view: &NSView,
        radius: u8,
        end_radius: u8,
        fade: FadeDirection,
    ) -> Option<Self> {
        let _marker = MainThreadMarker::new()?;
        let parent = unsafe { view.superview() }?;
        // Never turn on layer backing or replace a layer owned by AppKit/CGL.
        // An existing host layer is required for this optional blur mode.
        let host: Option<Retained<AnyObject>> = unsafe { msg_send![&*parent, layer] };
        let host = host?;
        let window = view.window()?;
        let frame = view.frame();
        let key = MaskKey::new(radius, end_radius, fade, frame.size)?;
        let runtime = Runtime::load()?;
        let image = MaskImage::new(key)?;
        let filter = runtime.filter(key.maximum(), &image)?;
        let backdrop: Option<Retained<AnyObject>> = unsafe { msg_send![runtime.backdrop, new] };
        let backdrop = backdrop?;
        let scale = window.backingScaleFactor();
        runtime.transaction(|| unsafe {
            let _: () = msg_send![&*backdrop, setWindowServerAware: true];
            let _: () = msg_send![&*backdrop, setFrame: frame];
            // This controls local geometry, not the rendering orientation of
            // the private filter's image. Keep that distinction explicit.
            let _: () = msg_send![&*backdrop, setGeometryFlipped: true];
            let _: () = msg_send![&*backdrop, setContentsScale: scale];
            let _: () = msg_send![&*backdrop, setMasksToBounds: true];
            let _: () = msg_send![&*backdrop, setAutoresizingMask: 18u32];
            let _: () = msg_send![&*backdrop, setFilters: &*NSArray::from_slice(&[&*filter])];
            // One inert compositor layer beneath the host's existing layers;
            // no NSView is inserted, reparented, or given layer ownership.
            let _: () = msg_send![&*host, insertSublayer: &*backdrop, atIndex: 0u32];
        });
        Some(Self {
            renderer: Weak::new(view),
            backdrop,
            runtime,
            state: RefCell::new(State {
                key,
                frame,
                scale,
                _image: image,
            }),
        })
    }

    pub(super) fn synchronize(
        &self,
        view: &NSView,
        radius: u8,
        end_radius: u8,
        fade: FadeDirection,
    ) -> bool {
        let Some(original) = self.renderer.load() else {
            return false;
        };
        if !std::ptr::eq(&*original, view) {
            return false;
        }
        let Some(parent) = (unsafe { view.superview() }) else {
            return false;
        };
        let host: Option<Retained<AnyObject>> = unsafe { msg_send![&*parent, layer] };
        let attached: Option<Retained<AnyObject>> =
            unsafe { msg_send![&*self.backdrop, superlayer] };
        let Some((host, attached)) = host.zip(attached) else {
            return false;
        };
        // AppKit may replace its backing layer. Do not keep updating an
        // obsolete tree; let the owner recreate on the current host instead.
        if !std::ptr::eq(&*host, &*attached) {
            return false;
        }
        let Some(window) = view.window() else {
            return false;
        };
        let frame = view.frame();
        let Some(key) = MaskKey::new(radius, end_radius, fade, frame.size) else {
            return false;
        };
        let scale = window.backingScaleFactor();
        let mut state = self.state.borrow_mut();
        // AppKit's layer autoresizing can change the native frame without
        // changing our cached state. Repair that drift before reusing it.
        let actual_frame: NSRect = unsafe { msg_send![&*self.backdrop, frame] };
        if state.key == key && state.frame == frame && state.scale == scale && actual_frame == frame
        {
            return true;
        }
        // The filter's radius is the larger endpoint; its alpha mask carries
        // the normalized range. Proportional ranges reuse the same image.
        let replacement_image = if !key.same_mask(state.key) {
            let Some(image) = MaskImage::new(key) else {
                return false;
            };
            Some(image)
        } else {
            None
        };
        let replacement_filter = if key != state.key {
            let image = replacement_image.as_ref().unwrap_or(&state._image);
            let Some(filter) = self.runtime.filter(key.maximum(), image) else {
                return false;
            };
            Some(filter)
        } else {
            None
        };
        self.runtime.transaction(|| unsafe {
            let _: () = msg_send![&*self.backdrop, setFrame: frame];
            let _: () = msg_send![&*self.backdrop, setContentsScale: scale];
            if let Some(filter) = &replacement_filter {
                let _: () = msg_send![&*self.backdrop,
                    setFilters: &*NSArray::from_slice(&[&**filter])];
            }
        });
        if let Some(image) = replacement_image {
            state._image = image;
        }
        state.key = key;
        state.frame = frame;
        state.scale = scale;
        true
    }

    /// Native model readback, not proof of the composed desktop pixels.
    #[cfg(any(test, feature = "native-ui-tests", feature = "desktop-ui-tests"))]
    pub(super) fn inspection(&self) -> Option<(usize, f64, [usize; 2], [f64; 4])> {
        let filters: Option<Retained<NSArray<AnyObject>>> =
            unsafe { msg_send![&*self.backdrop, filters] };
        let filter = filters?.firstObject()?;
        let radius: Option<Retained<NSNumber>> =
            unsafe { msg_send![&*filter, valueForKey: &*NSString::from_str("inputRadius")] };
        // The filter owns its CGImage for this read. Avoid retaining/releasing
        // a CF graphics object through an Objective-C wrapper in inspection.
        let image: *const AnyObject =
            unsafe { msg_send![&*filter, valueForKey: &*NSString::from_str("inputMaskImage")] };
        if image.is_null() {
            return None;
        }
        let size = unsafe { [CGImageGetWidth(image), CGImageGetHeight(image)] };
        let frame: NSRect = unsafe { msg_send![&*self.backdrop, frame] };
        Some((
            std::ptr::from_ref(&*filter) as usize,
            radius?.as_f64(),
            size,
            [
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            ],
        ))
    }
}
impl Drop for GradientBlur {
    fn drop(&mut self) {
        self.runtime.transaction(|| unsafe {
            let _: () = msg_send![&*self.backdrop, removeFromSuperlayer];
        });
    }
}

// A small, interpolated texture suffices for a linear mask. Keep its aspect
// ratio while bounding both dimensions, even for a nearly collapsed window.
fn mask_dimensions(width: f64, height: f64) -> Option<(usize, usize)> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let divisor = width.max(height);
    Some((
        (width / divisor * 256.0).round().clamp(2.0, 256.0) as usize,
        (height / divisor * 256.0).round().clamp(2.0, 256.0) as usize,
    ))
}
fn mask_pixels(key: MaskKey) -> Vec<u8> {
    let mut pixels = vec![0; key.width * key.height * 4];
    for y in 0..key.height {
        for x in 0..key.width {
            let strength = key.fade.factor_at(
                x as f32 / (key.width - 1) as f32,
                y as f32 / (key.height - 1) as f32,
            );
            // Black RGB, premultiplied alpha; rows encode the requested
            // top-left gradient. The private filter's displayed orientation
            // requires separate native composition evidence.
            pixels[(y * key.width + x) * 4 + 3] = key.alpha(strength);
        }
    }
    pixels
}
struct MaskImage(NonNull<c_void>);
impl MaskImage {
    fn new(key: MaskKey) -> Option<Self> {
        let pixels = Box::into_raw(Box::new(mask_pixels(key)));
        // The data provider owns the boxed pixels, including when CGImage or
        // CAFilter retains them after this Rust object has been replaced.
        unsafe extern "C" fn release(info: *mut c_void, _: *const c_void, _: usize) {
            unsafe { drop(Box::from_raw(info.cast::<Vec<u8>>())) };
        }
        unsafe {
            let provider = CGDataProviderCreateWithData(
                pixels.cast(),
                (*pixels).as_ptr().cast(),
                (*pixels).len(),
                Some(release),
            );
            if provider.is_null() {
                drop(Box::from_raw(pixels));
                return None;
            }
            let space = CGColorSpaceCreateDeviceRGB();
            if space.is_null() {
                CGDataProviderRelease(provider);
                return None;
            }
            let image = CGImageCreate(
                key.width,
                key.height,
                8,
                32,
                key.width * 4,
                space,
                1 | (4 << 12), // premultiplied-last alpha, RGBA byte order
                provider,
                std::ptr::null(),
                true,
                0,
            );
            CGColorSpaceRelease(space);
            CGDataProviderRelease(provider);
            NonNull::new(image).map(Self)
        }
    }
}
impl Drop for MaskImage {
    fn drop(&mut self) {
        unsafe { CGImageRelease(self.0.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_key(radius: u8, end_radius: u8, fade: FadeDirection) -> MaskKey {
        MaskKey {
            radius,
            end_radius,
            fade,
            width: 3,
            height: 3,
        }
    }

    #[test]
    fn mask_uses_top_left_directions_and_exact_endpoints() {
        for fade in FadeDirection::ALL {
            let key = tiny_key(24, 0, fade);
            let pixels = mask_pixels(key);
            for y in 0..3 {
                for x in 0..3 {
                    let alpha = pixels[(y * 3 + x) * 4 + 3];
                    assert_eq!(
                        alpha,
                        (fade.factor_at(x as f32 / 2.0, y as f32 / 2.0) * 255.0).round() as u8
                    );
                    assert_eq!(&pixels[(y * 3 + x) * 4..(y * 3 + x) * 4 + 3], &[0, 0, 0]);
                }
            }
        }
    }

    #[test]
    fn mask_represents_arbitrary_inverted_equal_and_zero_endpoint_ranges() {
        for (start, end, expected) in [
            (40, 10, [255, 159, 64]),
            (10, 40, [64, 159, 255]),
            (17, 17, [255, 255, 255]),
            (17, 0, [255, 128, 0]),
            (0, 17, [0, 128, 255]),
            (0, 0, [0, 0, 0]),
        ] {
            let key = tiny_key(start, end, FadeDirection::Right);
            assert_eq!(key.maximum(), start.max(end));
            let pixels = mask_pixels(key);
            for y in 0..3 {
                assert_eq!(
                    std::array::from_fn::<_, 3, _>(|x| pixels[(y * 3 + x) * 4 + 3]),
                    expected
                );
            }
        }
    }

    #[test]
    fn proportional_ranges_reuse_identical_normalized_masks() {
        let base = tiny_key(8, 4, FadeDirection::Down);
        let proportional = tiny_key(16, 8, FadeDirection::Down);
        assert!(base != proportional);
        assert!(base.same_mask(proportional));
        assert_eq!(mask_pixels(base), mask_pixels(proportional));
        assert!(!base.same_mask(tiny_key(8, 2, FadeDirection::Down)));
        assert!(!base.same_mask(tiny_key(4, 8, FadeDirection::Down)));
        assert!(!base.same_mask(tiny_key(0, 0, FadeDirection::Down)));
        assert!(tiny_key(0, 0, FadeDirection::Down).same_mask(tiny_key(0, 0, FadeDirection::Down)));
        assert!(tiny_key(8, 8, FadeDirection::Down).same_mask(tiny_key(
            16,
            16,
            FadeDirection::Down
        )));
    }

    #[test]
    fn mask_storage_is_bounded_and_cache_key_tracks_changes() {
        assert_eq!(mask_dimensions(1000.0, 500.0), Some((256, 128)));
        assert_eq!(mask_dimensions(1.0, 1e12), Some((2, 256)));
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(mask_dimensions(invalid, 100.0).is_none());
            assert!(mask_dimensions(100.0, invalid).is_none());
        }
        let key = |radius, end_radius, fade, width, height| {
            MaskKey::new(radius, end_radius, fade, NSSize::new(width, height))
        };
        let base = key(8, 0, FadeDirection::Left, 1000.0, 500.0);
        assert!(base == key(8, 0, FadeDirection::Left, 1001.0, 500.5));
        assert!(base != key(9, 0, FadeDirection::Left, 1000.0, 500.0));
        assert!(
            base.unwrap()
                .same_mask(key(9, 0, FadeDirection::Left, 1000.0, 500.0).unwrap())
        );
        assert!(base != key(8, 2, FadeDirection::Left, 1000.0, 500.0));
        assert!(
            !base
                .unwrap()
                .same_mask(key(8, 2, FadeDirection::Left, 1000.0, 500.0).unwrap())
        );
        assert!(
            !base
                .unwrap()
                .same_mask(key(8, 0, FadeDirection::Right, 1000.0, 500.0).unwrap())
        );
        assert!(base != key(8, 0, FadeDirection::Right, 1000.0, 500.0));
        assert!(base != key(8, 0, FadeDirection::Left, 500.0, 1000.0));
    }
}
