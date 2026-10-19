//! Idle snapshots and input-driven Core Animation transforms. AppKit's main
//! thread is independent of the WebContent thread running Tinymist's WASM/SVG.
//! No CPU image resizing, decoding, JS evaluation wait, or pixel IPC on input.
#[path = "preview_zoom_snapshot/raster.rs"]
mod raster;

use eframe::egui::{Context, ViewportId};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGColorSpace, CGImage};

use std::{
    cell::RefCell,
    ptr::NonNull,
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use block2::RcBlock;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_app_kit::{
    NSEvent, NSEventMask, NSEventModifierFlags, NSEventPhase, NSEventType, NSImage, NSView,
};
use objc2_foundation::{
    NSDate, NSError, NSNumber, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRunLoop,
    NSRunLoopCommonModes, NSSize, NSString, NSTimer,
};
use objc2_web_kit::{
    WKInactiveSchedulingPolicy, WKNavigation, WKNavigationDelegate, WKSnapshotConfiguration,
    WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use wry::WebViewExtMacOS;
#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}

const MAX_PIXELS: f64 = 4_000_000.0;
const ZOOM_QUIET: Duration = Duration::from_millis(120);
const LOST_PINCH_END: Duration = Duration::from_secs(1);

// Trackpads send zero-delta scroll phase boundaries around gestures. They do
// not move content and must not discard a snapshot prepared before the pinch.
fn manual_scroll(dx: f64, dy: f64) -> bool {
    dx.is_finite() && dy.is_finite() && (dx != 0.0 || dy != 0.0)
}

// Magnification events can carry a gesture's original/central position. The
// live window cursor owns pinch zoom, while wheel events retain their location.
fn input_position(kind: NSEventType, event: NSPoint, cursor: NSPoint) -> NSPoint {
    if kind == NSEventType::Magnify {
        cursor
    } else {
        event
    }
}

fn viewport_point(pointer: NSPoint, bounds: NSRect, flipped: bool) -> Option<(f64, f64)> {
    let x = pointer.x - bounds.origin.x;
    let y = pointer.y - bounds.origin.y;
    if !x.is_finite()
        || !y.is_finite()
        || x < 0.0
        || y < 0.0
        || x >= bounds.size.width
        || y >= bounds.size.height
    {
        return None;
    }
    Some((x, if flipped { y } else { bounds.size.height - y }))
}

/// Native deadlines don't wait behind WebContent. One one-shot timer is moved
/// forward on input; an explicit pinch end bypasses the quiet period.
#[derive(Default)]
struct RenderGate {
    due: Option<Instant>,
    pinch: bool,
}
impl RenderGate {
    fn input(&mut self, now: Instant) {
        self.due = Some(
            now + if self.pinch {
                LOST_PINCH_END
            } else {
                ZOOM_QUIET
            },
        );
    }
    fn take(&mut self, now: Instant, force: bool) -> bool {
        if !force && self.due.is_some_and(|due| due > now) {
            return false;
        }
        self.due = None;
        self.pinch = false;
        true
    }
    fn clear(&mut self) {
        self.due = None;
        self.pinch = false;
    }
}

#[derive(Clone, Copy, Debug)]
struct Request {
    revision: u64,
    scale: f64,
    x: f64,
    y: f64,
    pan_x: f64,
    pan_y: f64,
    resize: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Transform {
    ratio: f64,
    dx: f64,
    dy: f64,
}

impl Transform {
    fn zoom(self, ratio: f64, x: f64, y: f64) -> Self {
        let incremental = if self.ratio > 0.0 {
            ratio / self.ratio
        } else {
            ratio
        };
        Self {
            ratio,
            dx: incremental * self.dx + x * (1.0 - incremental),
            dy: incremental * self.dy + y * (1.0 - incremental),
        }
    }
}

/// At most one vector request plus one replacement, regardless of input rate.
#[derive(Default)]
struct Pipeline {
    scale: f64,
    revision: u64,
    flight: Option<Request>,
    latest: Option<Request>,
    pending_transform: Transform,
    pending_base: Option<f64>,
}

impl Pipeline {
    #[cfg(test)]
    fn input(&mut self, factor: f64, reset: bool, x: f64, y: f64) -> Option<Request> {
        self.queue(factor, reset, x, y, false)
    }
    fn queue(&mut self, factor: f64, reset: bool, x: f64, y: f64, defer: bool) -> Option<Request> {
        if !factor.is_finite() || factor <= 0.0 || !x.is_finite() || !y.is_finite() {
            return None;
        }
        let next_scale = if reset {
            1.0
        } else {
            (self.scale.max(0.1) * factor).clamp(0.1, 10.0)
        };
        if (next_scale - self.scale).abs() < 1e-12 {
            return None;
        }
        let base = self
            .pending_base
            .unwrap_or_else(|| self.flight.map_or(self.scale, |flight| flight.scale));
        self.pending_base = Some(base);
        self.scale = next_scale;
        self.revision += 1;
        self.pending_transform = self.pending_transform.zoom(self.scale / base, x, y);
        let transform = self.pending_transform;
        let (anchor_x, anchor_y, pan_x, pan_y) = if (transform.ratio - 1.0).abs() < 1e-9 {
            (x, y, transform.dx, transform.dy)
        } else {
            (
                transform.dx / (1.0 - transform.ratio),
                transform.dy / (1.0 - transform.ratio),
                0.0,
                0.0,
            )
        };
        let request = Request {
            revision: self.revision,
            scale: self.scale,
            x: anchor_x,
            y: anchor_y,
            pan_x,
            pan_y,
            resize: false,
        };
        self.latest = Some(request);
        if defer { None } else { self.dispatch() }
    }

    fn resize(&mut self, defer: bool) -> Option<Request> {
        self.revision += 1;
        let mut request = self.latest.unwrap_or(Request {
            revision: self.revision,
            scale: self.scale,
            x: 0.0,
            y: 0.0,
            pan_x: 0.0,
            pan_y: 0.0,
            resize: true,
        });
        request.revision = self.revision;
        self.latest = Some(request);
        if defer { None } else { self.dispatch() }
    }

    fn dispatch(&mut self) -> Option<Request> {
        if self.flight.is_some() {
            return None;
        }
        self.flight = self.latest.take();
        self.pending_transform = Transform::default();
        self.pending_base = None;
        self.flight
    }

    fn ready(&mut self, revision: u64, scale: f64) -> bool {
        if !self.flight.is_some_and(|request| {
            request.revision == revision && (request.scale - scale).abs() < 1e-9
        }) {
            return false;
        }
        self.flight = None;
        if self.latest.is_none() {
            self.pending_transform = Transform::default();
            self.pending_base = None;
        }
        true
    }
}

/// Scroll limits and centering must match the live SVG, rather than freely
/// scaling a viewport photograph past the first/last edge of the document.
#[derive(Clone, Debug, serde::Deserialize)]
struct Geometry {
    paper: [f64; 4],
    viewport: [f64; 4],
    scroll: [f64; 2],
    view: [f64; 2],
    #[serde(default)]
    source_scale: f64,
    #[serde(default)]
    content_size: [f64; 2],
    #[serde(default)]
    pixel_rounded: bool,
    #[serde(default)]
    pages: Vec<[f64; 2]>,
}
impl Geometry {
    fn valid(&self) -> bool {
        self.paper
            .iter()
            .chain(&self.viewport)
            .chain(&self.scroll)
            .chain(&self.view)
            .all(|n| n.is_finite())
            && self.paper[2..]
                .iter()
                .chain(&self.viewport[2..])
                .all(|n| *n > 0.0)
            && self.scroll.iter().all(|n| *n >= 0.0)
            && self
                .content_size
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)
            && (!self.pixel_rounded
                || (self.content_size.iter().all(|value| *value > 0.0) && self.source_scale > 0.0))
            && self.pages.len() <= 10_000
            && (self.pages.is_empty() || (self.source_scale.is_finite() && self.source_scale > 0.0))
            && self
                .pages
                .iter()
                .all(|p| p[0].is_finite() && p[1].is_finite() && p[1] > 0.0)
            && self.pages.windows(2).all(|p| p[0][0] <= p[1][0])
    }
    // Tinymist keeps physical page gaps at 10 CSS pixels in both SVG modes.
    // Only page ink scales: a global bitmap transform drifts on later pages.
    fn offset(&self, index: usize, ratio: f64) -> f64 {
        self.pages[index][0] * self.source_scale * ratio + index as f64 * 10.0 * (1.0 - ratio)
    }
    fn pixels(value: f64) -> f64 {
        // The predicted CSS dimension may be an integer plus floating-point
        // noise. Do not invent another scrollable pixel at that boundary.
        if (value - value.round()).abs() < 1e-7 {
            value.round()
        } else {
            value.ceil()
        }
    }
    fn render_ratio(&self, ratio: f64, fit_width: f64, scale: f64) -> f64 {
        if !self.pixel_rounded {
            return ratio;
        }
        // Full SVG rounds its CSS box, then fits the viewBox inside it. Its ink
        // scale differs slightly from the requested scale; over many pages that
        // fractional error becomes a visible last-page clamp/handoff mismatch.
        let layout = fit_width / self.content_size[0] * scale;
        let gaps = self.pages.len().saturating_sub(1) as f64 * 10.0;
        let sx = (self.content_size[0] * layout).ceil() / self.content_size[0];
        let sy = (((self.content_size[1] + gaps / layout) * layout).ceil() - gaps)
            / self.content_size[1];
        sx.min(sy) / self.source_scale
    }
    fn width(&self, ratio: f64) -> f64 {
        if self.pixel_rounded {
            Self::pixels(self.content_size[0] * self.source_scale * ratio)
        } else {
            (self.paper[2] * ratio).ceil()
        }
    }
    fn height(&self, ratio: f64) -> f64 {
        if self.pixel_rounded {
            return Self::pixels(
                self.content_size[1] * self.source_scale * ratio
                    + self.pages.len().saturating_sub(1) as f64 * 10.0,
            );
        }
        self.pages
            .last()
            .map_or(self.paper[3] * ratio, |last| {
                self.offset(self.pages.len() - 1, ratio) + last[1] * self.source_scale * ratio
            })
            .ceil()
    }
    fn natural_x(&self, ratio: f64, size: NSSize) -> f64 {
        let width = (self.viewport[2] + size.width - self.view[0]).max(1.0);
        self.viewport[0] + ((width - self.width(ratio)) / 2.0).max(0.0)
    }
    fn locate(&self, y: f64, ratio: f64) -> SourceY {
        let mut lo = 0;
        let mut hi = self.pages.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.offset(mid, ratio) <= y {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let page = lo.saturating_sub(1);
        let start = self.offset(page, ratio);
        let end = start + self.pages[page][1] * self.source_scale * ratio;
        if lo > 0 && lo < self.pages.len() && y > end {
            SourceY::Gap(page, lo, (y - end) / (self.offset(lo, ratio) - end))
        } else {
            SourceY::Page(page, (y - start) / (self.source_scale * ratio))
        }
    }
    fn project(&self, point: SourceY, ratio: f64) -> f64 {
        match point {
            SourceY::Page(page, local) => {
                self.offset(page, ratio) + local * self.source_scale * ratio
            }
            SourceY::Gap(before, after, fraction) => {
                let end =
                    self.offset(before, ratio) + self.pages[before][1] * self.source_scale * ratio;
                end + fraction * (self.offset(after, ratio) - end)
            }
        }
    }
    fn constrain(&self, mut t: Transform, size: NSSize) -> Transform {
        let [px, py, _, _] = self.paper;
        let width = (self.viewport[2] + size.width - self.view[0]).max(1.0);
        let height = (self.viewport[3] + size.height - self.view[1]).max(1.0);
        let natural_x = self.natural_x(t.ratio, size);
        let natural_y = py + self.scroll[1];
        let max_x = (self.width(t.ratio) - width).max(0.0);
        let max_y = (self.height(t.ratio) - height).max(0.0);
        let x = px * t.ratio + t.dx;
        t.dx = x.clamp(natural_x - max_x, natural_x) - px * t.ratio;
        t.dy = (py * t.ratio + t.dy).clamp(natural_y - max_y, natural_y) - py * t.ratio;
        t
    }
    fn zoom(
        &self,
        old: Transform,
        ratio: f64,
        anchor: NSPoint,
        previous: NSSize,
        size: NSSize,
        resize: bool,
    ) -> Transform {
        let old = if old.ratio > 0.0 {
            old
        } else {
            Transform {
                ratio: 1.0,
                dx: 0.0,
                dy: 0.0,
            }
        };
        let mut next = old.zoom(ratio, anchor.x, anchor.y);
        if !self.pages.is_empty() {
            let point = self.locate(anchor.y - self.paper[1] * old.ratio - old.dy, old.ratio);
            next.dy = anchor.y - self.project(point, ratio) - self.paper[1] * ratio;
        }
        if resize {
            // Keep the current horizontal scroll, including any zoom/pan since
            // capture. Restoring capture-time scroll snaps the divider gesture.
            let left =
                (self.natural_x(old.ratio, previous) - self.paper[0] * old.ratio - old.dx).max(0.0);
            next.dx = self.natural_x(ratio, size) - left - self.paper[0] * ratio;
        }
        self.constrain(next, size)
    }
    fn point(&self, t: Transform, anchor: NSPoint) -> Option<serde_json::Value> {
        if self.pages.is_empty() || self.source_scale <= 0.0 {
            return None;
        }
        let point = self.locate(anchor.y - self.paper[1] * t.ratio - t.dy, t.ratio);
        let y = match point {
            SourceY::Page(page, local) => serde_json::json!({"page":page,"local":local}),
            SourceY::Gap(before, after, fraction) => {
                serde_json::json!({"before":before,"after":after,"fraction":fraction})
            }
        };
        Some(
            serde_json::json!({"x":((anchor.x-t.dx)/t.ratio-self.paper[0])/self.source_scale,
            "y":y,"clientX":anchor.x,"clientY":anchor.y}),
        )
    }

    fn target(&self, t: Transform, size: NSSize) -> serde_json::Value {
        let [px, py, _, _] = self.paper;
        let [vx, _, _, _] = self.viewport;
        let width = self.viewport[2] + size.width - self.view[0];
        let natural_x = vx + ((width - self.width(t.ratio)) / 2.0).max(0.0);
        let natural_y = py + self.scroll[1];
        serde_json::json!({"left":(natural_x - px*t.ratio-t.dx).max(0.0),
            "top":(natural_y - py*t.ratio-t.dy).max(0.0)})
    }
}

// A retained raster can serve another gesture only while it covers the visible
// paper. Exclude gutters, and map partial page gaps back without scaling them.
fn image_covers(g: &Geometry, t: Transform, origin: NSPoint, image: NSSize, view: NSSize) -> bool {
    if t.ratio <= 0.0 {
        return false;
    }
    let x = g.paper[0] * t.ratio + t.dx;
    let y = g.paper[1] * t.ratio + t.dy;
    let left = x.max(0.0);
    let right = (x + g.width(t.ratio)).min(view.width);
    let top = y.max(0.0);
    let bottom = (y + g.height(t.ratio)).min(view.height);
    if right <= left || bottom <= top {
        return true;
    }
    let source_y = |value| {
        if g.pages.is_empty() {
            (value - t.dy) / t.ratio
        } else {
            g.paper[1] + g.project(g.locate(value - y, t.ratio), 1.0)
        }
    };
    const EPSILON: f64 = 0.5;
    (left - t.dx) / t.ratio >= origin.x - EPSILON
        && (right - t.dx) / t.ratio <= origin.x + image.width + EPSILON
        && source_y(top) >= origin.y - EPSILON
        && source_y(bottom) <= origin.y + image.height + EPSILON
}

fn capture_geometry(request: &serde_json::Value, size: NSSize) -> Option<Geometry> {
    let g: Geometry = serde_json::from_value(request["geometry"].clone()).ok()?;
    (g.valid()
        && [
            (g.view[0], size.width),
            (g.view[1], size.height),
            (request["width"].as_f64()?, size.width),
            (request["height"].as_f64()?, size.height),
        ]
        .iter()
        .all(|(a, b)| (a - b).abs() <= 0.5))
    .then_some(g)
}

fn paper_clip(g: &Geometry, t: Transform, clip: NSSize) -> Option<NSRect> {
    let x = g.paper[0] * t.ratio + t.dx;
    let y = g.paper[1] * t.ratio + t.dy;
    let left = x.max(0.0);
    let top = y.max(0.0);
    let right = (x + g.width(t.ratio)).min(clip.width);
    let bottom = (y + g.height(t.ratio)).min(clip.height);
    (right > left && bottom > top).then(|| {
        NSRect::new(
            NSPoint::new(left, top),
            NSSize::new(right - left, bottom - top),
        )
    })
}

#[derive(Clone, Copy)]
enum SourceY {
    Page(usize, f64),
    Gap(usize, usize, f64),
}

#[derive(Clone, Copy, Debug)]
struct ImageSlice {
    // Rectangle in capture-time viewport coordinates. All slices share one
    // immutable CGImage; contentsRect selects texels without CPU image copies.
    source: NSRect,
    page: Option<usize>,
}
struct ImageLayer {
    layer: Retained<AnyObject>,
    slice: ImageSlice,
}

// A replacement must not alter the admitted image or its coordinate system
// before JS confirms that its source/scroll epoch is still current.
struct PreparedImage {
    capture: Capture,
    clip: Retained<AnyObject>,
    layers: Vec<ImageLayer>,
    paper_layer: Retained<AnyObject>,
    outline_layer: Retained<AnyObject>,
    pixels: (usize, usize),
}

fn image_slices(geometry: Option<&Geometry>, origin: NSPoint, size: NSSize) -> Vec<ImageSlice> {
    let paper = geometry
        .map(|g| {
            let left = origin.x.max(g.paper[0]);
            let right = (origin.x + size.width).min(g.paper[0] + g.paper[2]);
            (left, (right - left).max(0.0))
        })
        .unwrap_or((origin.x, size.width));
    let Some(g) = geometry.filter(|g| !g.pages.is_empty()) else {
        return vec![ImageSlice {
            source: NSRect::new(
                NSPoint::new(paper.0, origin.y),
                NSSize::new(paper.1, size.height),
            ),
            page: None,
        }];
    };
    let first = match g.locate(origin.y - g.paper[1], 1.0) {
        SourceY::Page(i, _) | SourceY::Gap(i, _, _) => i,
    };
    let mut slices = Vec::new();
    for index in first..g.pages.len() {
        let top = g.paper[1] + g.offset(index, 1.0);
        if top >= origin.y + size.height {
            break;
        }
        let start = top.max(origin.y);
        let end = (top + g.pages[index][1] * g.source_scale).min(origin.y + size.height);
        if end > start {
            slices.push(ImageSlice {
                source: NSRect::new(
                    NSPoint::new(paper.0, start),
                    NSSize::new(paper.1, end - start),
                ),
                page: Some(index),
            });
            // A pathological document uses the ordinary renderer instead of
            // creating thousands of native layers for a tiny-paper cache.
            if slices.len() > 256 {
                return Vec::new();
            }
        }
    }
    slices
}

// Bound every compositor layer to the visible clip. Offscreen buffered texels
// stay in the shared texture; no enlarged offscreen surface is submitted.
fn image_crop(
    slice: ImageSlice,
    geometry: Option<&Geometry>,
    t: Transform,
    origin: NSPoint,
    image: NSSize,
    clip: NSSize,
) -> Option<(NSRect, NSRect)> {
    let source = slice.source;
    let mut y = source.origin.y * t.ratio + t.dy;
    if let Some((g, index)) = geometry.zip(slice.page) {
        y += g.offset(index, t.ratio) - g.offset(index, 1.0) * t.ratio;
    }
    let x = source.origin.x * t.ratio + t.dx;
    let width = source.size.width * t.ratio;
    let height = source.size.height * t.ratio;
    let left = x.max(0.0);
    let top = y.max(0.0);
    let right = (x + width).min(clip.width);
    let bottom = (y + height).min(clip.height);
    if right <= left || bottom <= top {
        return None;
    }
    let texture_height = (bottom - top) / t.ratio / image.height;
    let texture_top = (source.origin.y - origin.y + (top - y) / t.ratio) / image.height;
    // The snapshot is top-origin. Keep its crop in the same coordinates as
    // the AppKit-managed WebKit parent; do not flip the crop or child geometry.
    // A second flip can reverse page slices while reported anchors still agree.
    let contents = NSRect::new(
        NSPoint::new(
            (source.origin.x - origin.x + (left - x) / t.ratio) / image.width,
            texture_top,
        ),
        NSSize::new((right - left) / t.ratio / image.width, texture_height),
    );
    Some((
        NSRect::new(
            NSPoint::new(left, top),
            NSSize::new(right - left, bottom - top),
        ),
        contents,
    ))
}

#[derive(Clone)]
struct Capture {
    token: String,
    revision: u64,
    viewport: NSSize,
    origin: NSPoint,
    size: NSSize,
    fit_width: f64,
    density: f64,
    geometry: Option<Geometry>,
    background: [f64; 4],
    paper_background: [f64; 4],
    buffered: bool,
}

// A bare, inert WKWebView avoids Wry's unconditional NSApplication.activate()
// on creation. The delegate retains only a weak owner and never takes focus.
define_class!(
    #[unsafe(super = NSObject)]
    #[name = "TiptoptypZoomSnapshotNavigation"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Weak<RefCell<Native>>]
    struct SnapshotNavigation;

    unsafe impl NSObjectProtocol for SnapshotNavigation {}
    unsafe impl WKNavigationDelegate for SnapshotNavigation {
        #[unsafe(method(webView:didFinishNavigation:))]
        fn finished(&self, _web: &WKWebView, navigation: Option<&WKNavigation>) {
            if let Some(owner) = self.ivars().upgrade() {
                let current = {
                    let mut native = owner.borrow_mut();
                    native.mirror.as_mut().is_some_and(|mirror| {
                        if !mirror.matches(navigation) { return false; }
                        mirror.ready = true;
                        true
                    })
                };
                if current { CaptureState(owner).capture_ready(); }
            }
        }
        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn failed(&self, _web: &WKWebView, navigation: Option<&WKNavigation>, _error: &NSError) {
            self.reject_capture(navigation);
        }
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn failed_provisional(&self, _web: &WKWebView, navigation: Option<&WKNavigation>, _error: &NSError) {
            self.reject_capture(navigation);
        }
    }
);
impl SnapshotNavigation {
    fn reject_capture(&self, navigation: Option<&WKNavigation>) {
        if let Some(owner) = self.ivars().upgrade() {
            let mut native = owner.borrow_mut();
            if !native
                .mirror
                .as_ref()
                .is_some_and(|m| m.matches(navigation))
            {
                return;
            }
            let capture = native.busy.take();
            native.deferred_raster = None;
            if let Some(mirror) = native.mirror.as_mut() {
                mirror.park();
            }
            native.rasterizing = false;
            let web = native.web.clone();
            drop(native);
            if let Some((capture, web)) = capture.zip(web) {
                CaptureState::resolve_failure(&web, &capture.token);
            }
        }
    }
}
struct Mirror {
    web: Retained<WKWebView>,
    _navigation: Retained<SnapshotNavigation>,
    active: Option<Retained<WKNavigation>>,
    ready: bool,
}
impl Mirror {
    fn new(
        parent: &NSView,
        owner: Weak<RefCell<Native>>,
        size: NSSize,
        html: &str,
    ) -> Option<Self> {
        let marker = MainThreadMarker::new()?;
        unsafe {
            let configuration = WKWebViewConfiguration::new(marker);
            configuration.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(marker));
            let preferences = configuration.preferences();
            if preferences.respondsToSelector(sel!(setInactiveSchedulingPolicy:)) {
                preferences.setInactiveSchedulingPolicy(WKInactiveSchedulingPolicy::None);
            }
            let web = WKWebView::initWithFrame_configuration(
                WKWebView::alloc(marker),
                NSRect::new(NSPoint::new(-100_000.0, 0.0), size),
                &configuration,
            );
            let navigation = SnapshotNavigation::alloc(marker).set_ivars(owner);
            let navigation: Retained<SnapshotNavigation> = msg_send![super(navigation), init];
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*navigation)));
            parent.addSubview(&web);
            let mut mirror = Self {
                web,
                _navigation: navigation,
                active: None,
                ready: false,
            };
            mirror.load(size, html).then_some(mirror)
        }
    }
    fn matches(&self, navigation: Option<&WKNavigation>) -> bool {
        self.active
            .as_deref()
            .zip(navigation)
            .is_some_and(|(a, b)| std::ptr::eq(a, b))
    }
    fn park(&mut self) {
        // The admitted CGImage owns its pixels independently of WebKit. Drop
        // the copied SVG DOM and its large backing surface between captures.
        self.active = None;
        self.ready = false;
        unsafe {
            self.web.stopLoading();
            self.web
                .loadHTMLString_baseURL(&NSString::from_str("<html></html>"), None);
            self.web.setFrame(NSRect::new(
                NSPoint::new(-100_000.0, 0.0),
                NSSize::new(1.0, 1.0),
            ));
        }
    }
    fn load(&mut self, size: NSSize, html: &str) -> bool {
        self.ready = false;
        unsafe {
            self.web
                .setFrame(NSRect::new(NSPoint::new(-100_000.0, 0.0), size));
            self.active = self
                .web
                .loadHTMLString_baseURL(&NSString::from_str(html), None);
            self.active.is_some()
        }
    }
}
impl Drop for Mirror {
    fn drop(&mut self) {
        unsafe {
            self.web.setNavigationDelegate(None);
            self.web.stopLoading();
            self.web.removeFromSuperview();
        }
    }
}

#[derive(Default)]
struct Native {
    owner: Weak<RefCell<Native>>,
    timer: Option<Retained<NSTimer>>,
    gate: RenderGate,
    web: Option<Retained<WKWebView>>,
    monitor: Option<Retained<AnyObject>>,
    clip: Option<Retained<AnyObject>>,
    image_layers: Vec<ImageLayer>,
    paper_layer: Option<Retained<AnyObject>>,
    outline_layer: Option<Retained<AnyObject>>,
    outline_width: f64,
    outline_color: [f64; 4],
    enabled: bool,
    allowed: bool,
    initialized: bool,
    busy: Option<Capture>,
    mirror: Option<Mirror>,
    rasterizing: bool,
    capture_deferred: bool,
    preparing_raster: bool,
    raster_worker: Option<raster::Worker>,
    repaint: Option<(Context, ViewportId)>,
    deferred_raster: Option<(String, raster::Prepared)>,
    deferred_admission: Option<(String, f64)>,
    geometry: Option<Geometry>,
    image_origin: NSPoint,
    image_size: NSSize,
    layout_size: NSSize,
    anchor: NSPoint,
    candidate: Option<PreparedImage>,
    overlay_visible: bool,
    token: Option<String>,
    flight_token: Option<String>,
    base_scale: f64,
    fit_width: f64,
    size: NSSize,
    pixels: (usize, usize),
    transform: Transform,
    pipeline: Pipeline,
    inputs: u64,
    hits: u64,
    handoffs: u64,
}

impl Drop for Native {
    fn drop(&mut self) {
        self.stop_timer();
        if let Some(monitor) = self.monitor.take() {
            // Owned local monitor installed below; remove before releasing its block.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        if let Some(layer) = self.clip.take() {
            unsafe {
                let _: () = msg_send![&*layer, removeFromSuperlayer];
            }
        }
    }
}

impl Native {
    fn candidate_token(&self) -> Option<&str> {
        self.candidate
            .as_ref()
            .map(|image| image.capture.token.as_str())
    }

    fn finish_handoff(&mut self) {
        self.stop_timer();
        self.gate.clear();
        self.flight_token = None;
        self.hide_feedback();
        // The sharp SVG is visible, but keep this valid raster and transform
        // ready for another input while a fresh idle capture is being prepared.
    }

    fn admit(&mut self, scale: f64, web: &WKWebView) -> bool {
        let parent: Option<Retained<AnyObject>> = unsafe { msg_send![web, layer] };
        let Some(parent) = parent else {
            self.candidate = None;
            return false;
        };
        let Some(image) = self.candidate.take() else {
            return false;
        };
        unsafe {
            let _: () = msg_send![class!(CATransaction), begin];
            let _: () = msg_send![class!(CATransaction), setDisableActions: true];
            if let Some(old) = self.clip.take() {
                let _: () = msg_send![&*old, removeFromSuperlayer];
            }
            let _: () = msg_send![&*parent, addSublayer: &*image.clip];
            let _: () = msg_send![class!(CATransaction), commit];
        }
        self.token = Some(image.capture.token);
        self.clip = Some(image.clip);
        self.image_layers = image.layers;
        self.paper_layer = Some(image.paper_layer);
        self.outline_layer = Some(image.outline_layer);
        if let Some(layer) = &self.outline_layer {
            set_outline(layer, self.outline_width, self.outline_color);
        }
        self.size = image.capture.viewport;
        self.layout_size = image.capture.viewport;
        self.image_origin = image.capture.origin;
        self.image_size = image.capture.size;
        self.geometry = image.capture.geometry;
        self.fit_width = image.capture.fit_width;
        self.pixels = image.pixels;
        self.base_scale = scale;
        self.pipeline.scale = scale;
        self.transform = Transform::default();
        self.overlay_visible = false;
        true
    }

    fn observe_frame(&mut self, scale: f64, size: NSSize) {
        self.initialized = true;
        if self.pipeline.flight.is_none() && self.pipeline.latest.is_none() {
            if self.token.is_some() && (self.pipeline.scale - scale).abs() > 1e-9 {
                self.retire_image();
            }
            if self.token.is_none() {
                self.pipeline.scale = scale;
                self.layout_size = size;
            }
            // With warmed pixels, layout_size records the last native layout,
            // not a browser observation which can race ahead of that layout.
        }
    }

    fn hide_feedback(&mut self) {
        self.overlay_visible = false;
        if let Some(clip) = &self.clip {
            unsafe {
                let _: () = msg_send![&**clip, setHidden: true];
            }
        }
    }

    fn retire_image(&mut self) {
        self.deferred_admission = None;
        self.token = None;
        self.candidate = None;
        self.transform = Transform::default();
        self.overlay_visible = false;
        if let Some(layer) = &self.clip {
            unsafe {
                let _: () = msg_send![&**layer, setHidden: true];
            }
        }
        if let Some(layer) = self.outline_layer.take() {
            unsafe {
                let _: () = msg_send![&*layer, removeFromSuperlayer];
            }
        }
        if let Some(layer) = self.paper_layer.take() {
            unsafe {
                let _: () = msg_send![&*layer, removeFromSuperlayer];
            }
        }
        for image in self.image_layers.drain(..) {
            unsafe {
                let _: () = msg_send![&*image.layer, removeFromSuperlayer];
            }
        }
    }

    fn clear(&mut self) {
        self.stop_timer();
        self.gate.clear();
        self.retire_image();
        self.pipeline.flight = None;
        self.pipeline.latest = None;
        self.pipeline.pending_transform = Transform::default();
        self.pipeline.pending_base = None;
        self.flight_token = None;
    }

    fn reject_request(&mut self, revision: u64) -> bool {
        if !self
            .pipeline
            .flight
            .is_some_and(|request| request.revision == revision)
        {
            return false;
        }
        // A missing/reset renderer cannot own the queued anchors. Wait for a
        // fresh ready frame to restore scale and layout before accepting input.
        self.clear();
        self.stop_capture();
        self.initialized = false;
        true
    }

    fn stop_capture(&mut self) {
        self.deferred_raster = None;
        self.preparing_raster = false;
        if let Some(capture) = self.busy.take()
            && let Some(web) = &self.web
        {
            CaptureState::resolve_failure(web, &capture.token);
        }
        self.mirror = None;
        self.rasterizing = false;
    }

    fn stop_timer(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.invalidate();
        }
    }

    fn arm_timer(&mut self) {
        let Some(due) = self.gate.due else {
            return;
        };
        let delay = due
            .saturating_duration_since(Instant::now())
            .as_secs_f64()
            .max(0.0001);
        if let Some(timer) = &self.timer {
            timer.setFireDate(&NSDate::dateWithTimeIntervalSinceNow(delay));
            return;
        }
        MainThreadMarker::new().expect("native zoom timers belong to AppKit");
        let weak = self.owner.clone();
        let block = RcBlock::new(move |_: NonNull<NSTimer>| {
            if let Some(owner) = weak.upgrade() {
                let mut native = owner.borrow_mut();
                native.timer = None;
                native.flush(false);
            }
        });
        // The private timer is installed only on the main run loop. Its block
        // and non-Send view state never move to another thread. Common modes
        // also deliver the deadline during native event tracking.
        unsafe {
            let timer = NSTimer::timerWithTimeInterval_repeats_block(delay, false, &block);
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
            self.timer = Some(timer);
        }
    }

    fn flush(&mut self, force: bool) {
        if !self.enabled || !self.allowed {
            return;
        }
        if !self.gate.take(Instant::now(), force) {
            self.arm_timer();
            return;
        }
        self.stop_timer();
        if let Some(request) = self.pipeline.dispatch() {
            self.send(request);
        }
    }

    fn send(&mut self, request: Request) {
        self.flight_token = self.token.clone();
        let Some(web) = &self.web else {
            return;
        };
        let script = format!(
            "window.tiptoptypNativeZoom?.({});",
            serde_json::json!({
                "revision":request.revision,"scale":request.scale,"x":request.x,"y":request.y,"panX":request.pan_x,"panY":request.pan_y,"token":self.token,"resize":request.resize,
                "scroll":self.geometry.as_ref().filter(|g| self.token.is_some() && g.pages.is_empty()).map(|g| g.target(self.transform, web.bounds().size)),
                "source":self.geometry.as_ref().filter(|_| self.token.is_some()).and_then(|g| g.point(self.transform,self.anchor))
            })
        );
        // Queue one vector request without waiting. Further events update the
        // native bitmap and replace latest; they do not queue WebKit scripts.
        let weak = self.owner.clone();
        let completion = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
            if zoom_request_accepted(unsafe { result.as_ref() }, !error.is_null()) {
                return;
            }
            let Some(owner) = weak.upgrade() else {
                return;
            };
            let web = {
                let mut native = owner.borrow_mut();
                if !native.reject_request(request.revision) {
                    return;
                }
                native.web.clone()
            };
            if let Some(web) = web {
                unsafe {
                    web.evaluateJavaScript_completionHandler(
                        &NSString::from_str(&format!(
                            "window.tiptoptypZoomSnapshot?.rejected({});",
                            request.revision
                        )),
                        None,
                    );
                }
            }
        });
        unsafe {
            web.evaluateJavaScript_completionHandler(
                &NSString::from_str(&script),
                Some(&completion),
            );
        }
    }

    fn zoom(&mut self, factor: f64, reset: bool, point: Option<(f64, f64)>) -> bool {
        if !self.enabled || !self.allowed || !self.initialized {
            return false;
        }
        let Some(web) = &self.web else {
            return false;
        };
        if web.isHiddenOrHasHiddenAncestor() {
            return false;
        }
        let bounds = web.bounds();
        let (x, y) = point.unwrap_or((bounds.size.width / 2.0, bounds.size.height / 2.0));
        if !factor.is_finite() || factor <= 0.0 || !x.is_finite() || !y.is_finite() {
            return false;
        }
        let old_revision = self.pipeline.revision;
        let deferred = self.token.is_some();
        let next = self.pipeline.queue(factor, reset, x, y, deferred);
        if self.pipeline.revision == old_revision {
            if self.gate.pinch && self.pipeline.latest.is_some() {
                self.gate.input(Instant::now());
                self.arm_timer();
            }
            return true; // Consume unchanged/capped gesture events; no duplicate render or fallback magnification.
        }
        self.inputs += 1;
        // Never admit a screenshot captured across input, even a scale round trip.
        self.candidate = None;
        if self.token.is_some() {
            self.draw_image(bounds, x, y, false);
        }
        if deferred && self.overlay_visible {
            self.gate.input(Instant::now());
            self.arm_timer();
        } else {
            self.gate.clear();
            self.stop_timer();
            if let Some(request) = next.or_else(|| self.pipeline.dispatch()) {
                self.send(request);
            }
        }
        true
    }
    fn draw_image(&mut self, bounds: NSRect, x: f64, y: f64, resize: bool) {
        self.anchor = NSPoint::new(x, y);
        let ratio = self.pipeline.scale / self.base_scale
            * (bounds.size.width - (self.size.width - self.fit_width)).max(1.0)
            / self.fit_width.max(1.0);
        let ratio = self.geometry.as_ref().map_or(ratio, |g| {
            g.render_ratio(
                ratio,
                (bounds.size.width - (self.size.width - self.fit_width)).max(1.0),
                self.pipeline.scale,
            )
        });
        self.transform = self.geometry.as_ref().map_or_else(
            || self.transform.zoom(ratio, x, y),
            |g| {
                g.zoom(
                    self.transform,
                    ratio,
                    self.anchor,
                    self.layout_size,
                    bounds.size,
                    resize,
                )
            },
        );
        if self.geometry.as_ref().is_some_and(|g| {
            !image_covers(
                g,
                self.transform,
                self.image_origin,
                self.image_size,
                bounds.size,
            )
        }) {
            // The buffer cannot cover this particular view. Show sharp content
            // immediately, but retain its pixels and transformed source point:
            // the next pinch can return inside the buffer before recapture.
            self.hide_feedback();
            return;
        }
        if let Some(clip) = &self.clip {
            unsafe {
                let _: () = msg_send![class!(CATransaction), begin];
                let _: () = msg_send![class!(CATransaction), setDisableActions: true];
                if let Some(layer) = &self.outline_layer {
                    let _: () = msg_send![&**layer,setFrame:NSRect::new(NSPoint::new(0.0,0.0),clip_bounds(bounds).size)];
                }
                if let Some((layer, g)) = self.paper_layer.as_ref().zip(self.geometry.as_ref()) {
                    if let Some(frame) = paper_clip(g, self.transform, clip_bounds(bounds).size) {
                        let _: () = msg_send![&**layer, setFrame: frame];
                        let _: () = msg_send![&**layer, setHidden: false];
                    } else {
                        let _: () = msg_send![&**layer, setHidden: true];
                    }
                }
                for image in &self.image_layers {
                    if let Some((frame, contents)) = image_crop(
                        image.slice,
                        self.geometry.as_ref(),
                        self.transform,
                        self.image_origin,
                        self.image_size,
                        clip_bounds(bounds).size,
                    ) {
                        let _: () = msg_send![&*image.layer, setHidden: false];
                        let _: () = msg_send![&*image.layer, setFrame: frame];
                        let _: () = msg_send![&*image.layer, setContentsRect: contents];
                    } else {
                        let _: () = msg_send![&*image.layer, setHidden: true];
                    }
                }
                let _: () = msg_send![&**clip, setHidden: false];
                let _: () = msg_send![class!(CATransaction), commit];
                // Let Core Animation commit at its normal display boundary.
                // flush() on every event forces work for obsolete intermediate frames.
            }
            self.overlay_visible = true;
            self.hits += 1;
        }
    }

    fn resize_view(&mut self, bounds: NSRect) {
        if self.layout_size == bounds.size || !self.allowed || !self.enabled || !self.initialized {
            return;
        }
        self.candidate = None;
        let deferred = self.token.is_some();
        if deferred {
            let y = self.geometry.as_ref().map_or(0.0, |g| g.viewport[1]);
            self.draw_image(bounds, 0.0, y, true);
        }
        self.layout_size = bounds.size;
        let deferred = self.token.is_some() && self.overlay_visible;
        let request = self.pipeline.resize(deferred);
        if deferred {
            self.gate.input(Instant::now());
            self.arm_timer();
        } else if let Some(request) = request {
            self.send(request);
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct CaptureState(Rc<RefCell<Native>>);

fn clip_bounds(bounds: NSRect) -> NSRect {
    // Leaving even one unmasked pixel can expose live glyph fragments while
    // cached ink moves. The focus outline is a separate layer above this mask.
    NSRect::new(
        bounds.origin,
        NSSize::new(bounds.size.width.max(0.0), bounds.size.height.max(0.0)),
    )
}

fn outline(value: &serde_json::Value) -> Option<(f64, [f64; 4])> {
    let width = value["width"]
        .as_f64()
        .filter(|v| v.is_finite() && (0.0..=64.0).contains(v))?;
    let color = value["color"].as_array().filter(|v| v.len() == 4)?;
    let rgba = [
        color[0].as_f64()? / 255.0,
        color[1].as_f64()? / 255.0,
        color[2].as_f64()? / 255.0,
        color[3].as_f64()?,
    ];
    rgba.iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        .then_some((width, rgba))
}

fn set_outline(layer: &AnyObject, width: f64, rgba: [f64; 4]) {
    unsafe extern "C" {
        static kCGColorSpaceSRGB: *const std::ffi::c_void;
        fn CGColorSpaceCreateWithName(name: *const std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGColorSpaceRelease(space: *mut std::ffi::c_void);
        fn CGColorCreate(
            space: *const std::ffi::c_void,
            components: *const f64,
        ) -> *mut std::ffi::c_void;
        fn CGColorRelease(color: *mut std::ffi::c_void);
    }
    unsafe {
        let _: () = msg_send![class!(CATransaction), begin];
        let _: () = msg_send![class!(CATransaction),setDisableActions:true];
        let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        let color = CGColorCreate(space, rgba.as_ptr());
        let _: () = msg_send![layer,setBorderWidth:width];
        let _: () = msg_send![layer,setBorderColor:color];
        CGColorRelease(color);
        CGColorSpaceRelease(space);
        let _: () = msg_send![class!(CATransaction), commit];
    }
}

fn zoom_request_accepted(result: Option<&AnyObject>, failed: bool) -> bool {
    !failed
        && result
            .and_then(|value| value.downcast_ref::<NSNumber>())
            .is_some_and(NSNumber::boolValue)
}

fn snapshot_width(width: f64, height: f64, backing: f64, density: f64) -> Option<f64> {
    if [width, height, backing, density]
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
        || backing > 8.0
        || density > 16.0
    {
        return None;
    }
    let pixel_width = (width * density)
        .min((3_900_000.0 * width / height).sqrt())
        .floor();
    (pixel_width >= 1.0).then_some(pixel_width / backing)
}

fn target_color_space(web: &WKWebView) -> Option<CFRetained<CGColorSpace>> {
    let display = web
        .window()
        .and_then(|window| {
            window
                .colorSpace()
                .or_else(|| window.screen().and_then(|screen| screen.colorSpace()))
        })
        .and_then(|space| space.CGColorSpace());
    display
        .map(|space| unsafe { CFRetained::retain(NonNull::from(&*space)) })
        .or_else(|| {
            CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB }))
        })
}

// A completed raster remains one bounded slot and belongs to exactly the
// capture which produced it; a canceled token can never claim its successor.
fn take_current_capture<T>(pending: &mut Option<(String, T)>, token: &str) -> Option<T> {
    pending
        .take()
        .and_then(|(owner, image)| (owner == token).then_some(image))
}

impl CaptureState {
    pub(crate) fn invalidate(&mut self) {
        // Reset native ownership without creating overlapping pixel workers
        // during rapid backend/reload transitions. Token checks discard old
        // replies; the existing mailbox still bounds active/pending work.
        let (worker, repaint) = {
            let mut native = self.0.borrow_mut();
            (native.raster_worker.take(), native.repaint.take())
        };
        *self = Self::default();
        let mut native = self.0.borrow_mut();
        native.raster_worker = worker;
        native.repaint = repaint;
    }

    pub(crate) fn bind(&self, view: &wry::WebView) {
        self.bind_web(view.webview().into_super());
    }

    pub(crate) fn bind_web(&self, web: Retained<WKWebView>) {
        if self.0.borrow().web.is_some() {
            return;
        }
        let weak = Rc::downgrade(&self.0);
        let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            let Some(state) = weak.upgrade() else {
                return event.as_ptr();
            };
            let mut native = state.borrow_mut();
            if !native.enabled || !native.allowed || !native.initialized {
                return event.as_ptr();
            }
            let event = unsafe { event.as_ref() };
            let Some(web) = &native.web else {
                return event as *const NSEvent as *mut NSEvent;
            };
            let Some(window) = web.window() else {
                return event as *const NSEvent as *mut NSEvent;
            };
            if web.isHiddenOrHasHiddenAncestor()
                || event
                    .window(
                        MainThreadMarker::new().expect("local monitor runs on the AppKit thread"),
                    )
                    .as_deref()
                    != Some(&*window)
            {
                return event as *const NSEvent as *mut NSEvent;
            }
            let kind = event.r#type();
            let modifiers = event.modifierFlags();
            if kind == NSEventType::LeftMouseUp && native.pipeline.latest.is_some_and(|r| r.resize)
            {
                native.flush(true);
                return event as *const NSEvent as *mut NSEvent;
            }
            let key = kind == NSEventType::KeyDown;
            let event_position = event.locationInWindow();
            let cursor = if kind == NSEventType::Magnify {
                window.mouseLocationOutsideOfEventStream()
            } else {
                event_position
            };
            let pointer =
                web.convertPoint_fromView(input_position(kind, event_position, cursor), None);
            let point = viewport_point(pointer, web.bounds(), web.isFlipped());
            let inside = point.is_some();
            let pinch_end = kind == NSEventType::Magnify
                && inside
                && event
                    .phase()
                    .intersects(NSEventPhase::Ended | NSEventPhase::Cancelled);
            let pinch_start = kind == NSEventType::Magnify
                && inside
                && event
                    .phase()
                    .intersects(NSEventPhase::Began | NSEventPhase::Changed);
            let factor = if kind == NSEventType::Magnify && inside {
                Some((1.0 + event.magnification(), false, point))
            } else if kind == NSEventType::ScrollWheel
                && inside
                && modifiers.contains(NSEventModifierFlags::Control)
            {
                let unit = if event.hasPreciseScrollingDeltas() {
                    1.0
                } else {
                    20.0
                };
                Some(((event.scrollingDeltaY() * unit * 0.002).exp(), false, point))
            } else if key
                && modifiers
                    .intersects(NSEventModifierFlags::Command | NSEventModifierFlags::Control)
                && !modifiers.contains(NSEventModifierFlags::Option)
            {
                let focused = web
                    .window()
                    .and_then(|window| window.firstResponder())
                    .is_some_and(|responder| {
                        // Only native view responders inside this exact preview.
                        responder
                            .downcast_ref::<NSView>()
                            .is_some_and(|view| view.isDescendantOf(web))
                    });
                if focused {
                    match event
                        .charactersIgnoringModifiers()
                        .map(|text| text.to_string())
                        .as_deref()
                    {
                        Some("=" | "+") => Some((1.1, false, None)),
                        Some("-") => Some((1.0 / 1.1, false, None)),
                        Some("0") => Some((1.0, true, None)),
                        _ => None,
                    }
                } else {
                    None
                }
            } else {
                if kind == NSEventType::ScrollWheel
                    && inside
                    // An idle edge wheel or a pinch's incidental scroll phase
                    // may not move the browser. Let actual DOM motion retire
                    // idle pixels; only real navigation interrupts a proxy.
                    && (native.overlay_visible || native.pipeline.flight.is_some() || native.pipeline.latest.is_some()) && !native.gate.pinch
                    && manual_scroll(event.scrollingDeltaX(), event.scrollingDeltaY())
                {
                    native.retire_image();
                    native.flush(true);
                }
                None
            };
            if pinch_start {
                native.gate.pinch = true;
            }
            if let Some((factor, reset, point)) = factor
                && native.zoom(factor, reset, point)
            {
                if pinch_end {
                    native.gate.pinch = false;
                    native.flush(true);
                }
                return std::ptr::null_mut();
            }
            event as *const NSEvent as *mut NSEvent
        });
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::Magnify
                    | NSEventMask::ScrollWheel
                    | NSEventMask::KeyDown
                    | NSEventMask::LeftMouseUp,
                &block,
            )
        };
        let mut native = self.0.borrow_mut();
        native.owner = Rc::downgrade(&self.0);
        native.web = Some(web);
        native.monitor = monitor;
        native.pipeline.scale = 1.0;
        if native.capture_deferred {
            unsafe {
                native
                    .web
                    .as_ref()
                    .unwrap()
                    .evaluateJavaScript_completionHandler(
                        &NSString::from_str("window.tiptoptypZoomSnapshot?.deferCaptures(true);"),
                        None,
                    );
            }
        }
    }

    /// Record the owning viewport and admit at most one worker completion.
    /// Idle frames never request work; each finished raster wakes this viewport.
    pub(crate) fn poll(&self, context: &Context) {
        let (prepared, failed) = {
            let mut native = self.0.borrow_mut();
            native.repaint = Some((context.clone(), context.viewport_id()));
            match native.raster_worker.as_ref().map(raster::Worker::poll) {
                Some(Ok(prepared)) => (prepared, None),
                Some(Err(())) => {
                    native.raster_worker = None;
                    let failed = if native.preparing_raster {
                        native.busy.take()
                    } else {
                        None
                    };
                    native.preparing_raster = false;
                    (
                        None,
                        failed
                            .and_then(|capture| native.web.clone().map(|web| (capture.token, web))),
                    )
                }
                None => (None, None),
            }
        };
        if let Some((token, web)) = failed {
            Self::resolve_failure(&web, &token);
        }
        if let Some(prepared) = prepared {
            self.raster_finished(prepared);
        }
    }

    /// Delay optional pixel warming through finite editor feedback, retaining
    /// the accepted raster and all live zoom/resize requests.
    pub(crate) fn set_capture_deferred(&self, deferred: bool) {
        let (web, admission) = {
            let mut native = self.0.borrow_mut();
            if native.capture_deferred == deferred {
                return;
            }
            native.capture_deferred = deferred;
            let admission = if deferred {
                None
            } else {
                native.deferred_admission.take()
            };
            (native.web.clone(), admission)
        };
        if let Some(web) = web {
            unsafe {
                web.evaluateJavaScript_completionHandler(
                    &NSString::from_str(if deferred {
                        "window.tiptoptypZoomSnapshot?.deferCaptures(true);"
                    } else {
                        "window.tiptoptypZoomSnapshot?.deferCaptures(false);"
                    }),
                    None,
                );
            }
            if let Some((token, scale)) = admission {
                self.acknowledge_admission(&web, Some(&token), Some(scale));
            }
        }
        if !deferred {
            self.capture_ready();
        }
    }

    /// Disable synchronously on hiding/reloading/settings changes. An IPC
    /// configure message alone could wait behind a blocked renderer.
    pub(crate) fn allowed(&self, allowed: bool) {
        let mut native = self.0.borrow_mut();
        if native.allowed == allowed {
            return;
        }
        native.allowed = allowed;
        if !allowed {
            native.initialized = false;
            native.clear();
            native.stop_capture();
        } else if native.enabled
            && let Some(web) = &native.web
        {
            // Restore after a hidden view without relying on stale JS cache
            // metadata; queue once on the visibility transition, never per frame.
            unsafe {
                web.evaluateJavaScript_completionHandler(&NSString::from_str("window.tiptoptypZoomSnapshot?.invalidate(true);window.tiptoptypZoomSnapshot?.settle();"), None);
            }
        }
    }

    pub(crate) fn layout(&self, view: &wry::WebView) {
        let mut native = self.0.borrow_mut();
        let bounds = view.webview().bounds();
        unsafe {
            let _: () = msg_send![class!(CATransaction), begin];
            let _: () = msg_send![class!(CATransaction), setDisableActions: true];
            if let Some(clip) = &native.clip {
                let _: () = msg_send![&**clip, setFrame: clip_bounds(bounds)];
            }
            native.resize_view(bounds);
            let _: () = msg_send![class!(CATransaction), commit];
        }
    }

    pub(crate) fn retire_image(&self) {
        let mut native = self.0.borrow_mut();
        native.retire_image();
        native.flush(true);
    }

    pub(crate) fn action(&self, action: &serde_json::Value) -> bool {
        let mut native = self.0.borrow_mut();
        match action["action"].as_str() {
            Some("zoom-in") => native.zoom(1.1, false, None),
            Some("zoom-out") => native.zoom(1.0 / 1.1, false, None),
            Some("fit") => native.zoom(1.0, true, None),
            Some("page" | "location" | "back" | "forward") => {
                native.retire_image();
                native.flush(true);
                false
            }
            _ => false,
        }
    }

    pub(crate) fn message(&self, view: &wry::WebView, value: &serde_json::Value, allowed: bool) {
        self.message_web(view.webview().into_super(), value, allowed);
    }

    pub(crate) fn message_web(
        &self,
        web: Retained<WKWebView>,
        value: &serde_json::Value,
        allowed: bool,
    ) {
        self.bind_web(web.clone());
        self.allowed(allowed);
        if value["op"] == "capture" {
            self.capture(web, value, allowed);
            return;
        }
        let mut native = self.0.borrow_mut();
        match value["op"].as_str() {
            Some("configure") => {
                let next = value["enabled"] == true;
                if next && !native.enabled {
                    unsafe {
                        web.setMagnification(1.0);
                    }
                }
                native.enabled = next;
                // A navigation recreates the JS controller while native state
                // survives; replay the capture gate once on configuration.
                if native.capture_deferred {
                    unsafe {
                        web.evaluateJavaScript_completionHandler(
                            &NSString::from_str(
                                "window.tiptoptypZoomSnapshot?.deferCaptures(true);",
                            ),
                            None,
                        );
                    }
                }
                if !native.enabled {
                    native.clear();
                    native.stop_capture();
                }
            }
            Some("outline") => {
                if let Some((width, color)) = outline(value) {
                    native.outline_width = width;
                    native.outline_color = color;
                    if let Some(layer) = &native.outline_layer {
                        set_outline(layer, width, color);
                    }
                }
            }
            Some("frame") if allowed => {
                if let Some(scale) = value["scale"]
                    .as_f64()
                    .filter(|scale| scale.is_finite() && (0.1..=10.0).contains(scale))
                {
                    native.observe_frame(scale, web.bounds().size);
                }
            }
            Some("invalidate") => {
                if value["abort"] == true {
                    native.clear();
                } else {
                    native.retire_image();
                    native.flush(true);
                }
            }
            Some("abandon") => {
                if let Some(revision) = value["revision"].as_u64() {
                    native.reject_request(revision);
                }
            }
            Some("reject") => {
                if let Some(token) = value["token"].as_str() {
                    if native.candidate_token() == Some(token) {
                        native.candidate = None;
                    } else if native.token.as_deref() == Some(token) {
                        native.retire_image();
                        native.flush(true);
                    }
                }
            }
            Some("admit") => {
                let token = value["token"].as_str();
                let scale = value["scale"]
                    .as_f64()
                    .filter(|s| s.is_finite() && (0.1..=10.0).contains(s));
                drop(native);
                self.acknowledge_admission(&web, token, scale);
            }
            Some("resize") if allowed => {
                native.resize_view(web.bounds());
            }
            Some("zoom") if allowed => {
                let factor = value["factor"].as_f64().unwrap_or(0.0);
                let point = value["x"].as_f64().zip(value["y"].as_f64());
                native.zoom(factor, value["reset"] == true, point);
            }
            Some("ready")
                if value["token"].as_str() == native.flight_token.as_deref()
                    && native.pipeline.ready(
                        value["revision"].as_u64().unwrap_or(0),
                        value["scale"].as_f64().unwrap_or(0.0),
                    ) =>
            {
                if native.pipeline.latest.is_some() {
                    native.flush(false);
                } else {
                    native.handoffs += 1;
                    native.finish_handoff();
                }
            }
            _ => {}
        }
    }

    fn acknowledge_admission(&self, web: &WKWebView, token: Option<&str>, scale: Option<f64>) {
        let mut native = self.0.borrow_mut();
        if native.capture_deferred
            && let (Some(token), Some(scale)) = (token, scale)
            && native.candidate_token() == Some(token)
        {
            native.deferred_admission = Some((token.to_owned(), scale));
            return;
        }
        let accepted = native.enabled
            && native.allowed
            && token.is_some()
            && native.pipeline.flight.is_none()
            && native.pipeline.latest.is_none()
            && native.candidate_token() == token
            && native
                .candidate
                .as_ref()
                .is_some_and(|image| image.capture.revision == native.pipeline.revision)
            && scale.is_some_and(|scale| native.admit(scale, web));
        unsafe {
            web.evaluateJavaScript_completionHandler(
                &NSString::from_str(&format!(
                    "window.tiptoptypZoomSnapshot?.admitted({},{});",
                    serde_json::json!(token),
                    accepted
                )),
                None,
            );
        }
    }

    fn capture(&self, web: Retained<WKWebView>, request: &serde_json::Value, allowed: bool) {
        let Some(token) = request["token"].as_str().filter(|token| token.len() <= 128) else {
            return;
        };
        if self.0.borrow().capture_deferred {
            unsafe {
                web.evaluateJavaScript_completionHandler(
                    &NSString::from_str(&format!(
                        "window.tiptoptypZoomSnapshot?.deferred({});",
                        serde_json::json!(token)
                    )),
                    None,
                );
            }
            return;
        }
        let Some(marker) = MainThreadMarker::new() else {
            return;
        };
        let bounds = web.bounds();
        let Some(geometry) = capture_geometry(request, bounds.size) else {
            Self::resolve_failure(&web, token);
            return;
        };
        let buffer = &request["buffer"];
        let buffered = buffer["html"]
            .as_str()
            .is_some_and(|html| html.len() <= 20_000_000);
        let origin = if buffered {
            NSPoint::new(
                buffer["x"].as_f64().unwrap_or(0.0),
                buffer["y"].as_f64().unwrap_or(0.0),
            )
        } else {
            NSPoint::new(0.0, 0.0)
        };
        let size = if buffered {
            NSSize::new(
                buffer["width"].as_f64().unwrap_or(0.0),
                buffer["height"].as_f64().unwrap_or(0.0),
            )
        } else {
            bounds.size
        };
        let density = request["density"].as_f64().unwrap_or(0.0);
        let valid = size.width.is_finite()
            && size.height.is_finite()
            && origin.x.is_finite()
            && origin.y.is_finite()
            && size.width > 0.0
            && size.height > 0.0
            && size.width <= bounds.size.width * 3.0
            && size.height <= bounds.size.height * 3.0;
        let mut native = self.0.borrow_mut();
        if !allowed
            || !valid
            || snapshot_width(size.width, size.height, 1.0, density).is_none()
            || native.busy.is_some()
            || native.candidate.is_some()
            || native.pipeline.flight.is_some()
            || native.pipeline.latest.is_some()
        {
            drop(native);
            Self::resolve_failure(&web, token);
            return;
        }
        let background = request["background"]
            .as_array()
            .filter(|v| v.len() == 3)
            .and_then(|v| {
                v.iter()
                    .map(|n| {
                        n.as_f64()
                            .filter(|n| n.is_finite() && (0.0..=255.0).contains(n))
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .map_or([1.0, 1.0, 1.0, 1.0], |v| {
                [v[0] / 255.0, v[1] / 255.0, v[2] / 255.0, 1.0]
            });
        let paper_background = buffer["paperBackground"]
            .as_array()
            .filter(|v| v.len() == 4)
            .and_then(|v| {
                Some([
                    v[0].as_f64()? / 255.0,
                    v[1].as_f64()? / 255.0,
                    v[2].as_f64()? / 255.0,
                    v[3].as_f64()?,
                ])
            })
            .filter(|v| v.iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c)))
            .unwrap_or(background);
        native.busy = Some(Capture {
            token: token.to_owned(),
            revision: native.pipeline.revision,
            viewport: bounds.size,
            origin,
            size,
            density,
            background,
            paper_background,
            buffered,
            fit_width: request["fitWidth"]
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.0 && *v <= bounds.size.width)
                .unwrap_or(bounds.size.width),
            geometry: Some(geometry),
        });
        if buffered {
            let html = buffer["html"].as_str().unwrap();
            if let Some(mirror) = native.mirror.as_mut() {
                if !mirror.load(size, html) {
                    native.busy = None;
                    drop(native);
                    Self::resolve_failure(&web, token);
                }
                return;
            }
            let mirror = unsafe { web.superview() }
                .and_then(|parent| Mirror::new(&parent, Rc::downgrade(&self.0), size, html));
            if let Some(mirror) = mirror {
                native.mirror = Some(mirror);
            } else {
                native.busy = None;
                drop(native);
                Self::resolve_failure(&web, token);
            }
        } else {
            drop(native);
            let _ = marker;
            self.capture_ready();
        }
    }

    fn resolve_failure(web: &WKWebView, token: &str) {
        unsafe {
            web.evaluateJavaScript_completionHandler(
                &NSString::from_str(&format!(
                    "window.tiptoptypZoomSnapshot?.resolve({},null);",
                    serde_json::json!(token)
                )),
                None,
            );
        }
    }

    fn capture_ready(&self) {
        let mut native = self.0.borrow_mut();
        if native.capture_deferred || native.rasterizing || native.preparing_raster {
            return;
        }
        let Some(capture) = native.busy.clone() else {
            return;
        };
        let Some(web) = native.web.clone() else {
            return;
        };
        if let Some(prepared) = take_current_capture(&mut native.deferred_raster, &capture.token) {
            drop(native);
            self.raster_finished(prepared);
            return;
        }
        let capture_web = if capture.buffered {
            let Some(mirror) = &native.mirror else {
                return;
            };
            if !mirror.ready {
                return;
            }
            mirror.web.clone()
        } else {
            web.clone()
        };
        let backing = capture_web.window().map_or(1.0, |w| w.backingScaleFactor());
        let Some(width) = snapshot_width(
            capture.size.width,
            capture.size.height,
            backing,
            capture.density,
        ) else {
            return;
        };
        native.rasterizing = true;
        drop(native);
        let configuration =
            unsafe { WKSnapshotConfiguration::new(MainThreadMarker::new().unwrap()) };
        unsafe {
            configuration.setSnapshotWidth(Some(&NSNumber::new_f64(width)));
            configuration.setAfterScreenUpdates(true);
        }
        let weak = Rc::downgrade(&self.0);
        let token = capture.token.clone();
        let completion = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            let image = if error.is_null() {
                unsafe { Retained::retain(image) }
            } else {
                None
            };
            CaptureState(state).capture_finished(&token, image);
        });
        unsafe {
            capture_web
                .takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &completion);
        }
    }
    fn capture_finished(&self, token: &str, image: Option<Retained<NSImage>>) {
        let mut native = self.0.borrow_mut();
        let Some(capture) = native
            .busy
            .as_ref()
            .filter(|capture| capture.token == token)
            .cloned()
        else {
            return;
        };
        native.rasterizing = false;
        let Some(web) = native.web.clone() else {
            return;
        };
        let source = image.as_deref().and_then(|image| unsafe {
            // AppKit owns this lookup; only its immutable CGImage enters the worker.
            let cg: *mut CGImage = msg_send![image, CGImageForProposedRect: std::ptr::null_mut::<NSRect>(), context: std::ptr::null::<AnyObject>(), hints: std::ptr::null::<AnyObject>()];
            NonNull::new(cg).map(|cg| CFRetained::retain(cg))
        });
        if native.allowed
            && native.enabled
            && native.pipeline.revision == capture.revision
            && native.pipeline.flight.is_none()
            && native.pipeline.latest.is_none()
            && web.bounds().size == capture.viewport
            && let (Some(source), Some(space)) = (source, target_color_space(&web))
        {
            let request = raster::Request {
                token: token.to_owned(),
                revision: capture.revision,
                source,
                space,
                wake: native.repaint.clone(),
            };
            if native.raster_worker.is_none() {
                native.raster_worker = raster::Worker::new().ok();
            }
            if native
                .raster_worker
                .as_ref()
                .is_some_and(|worker| worker.request(request))
            {
                native.preparing_raster = true;
                if let Some(mirror) = native.mirror.as_mut() {
                    mirror.park();
                }
                return;
            }
        }
        native.busy = None;
        if let Some(mirror) = native.mirror.as_mut() {
            mirror.park();
        }
        drop(native);
        Self::resolve_failure(&web, token);
    }

    fn raster_finished(&self, prepared: raster::Prepared) {
        let mut native = self.0.borrow_mut();
        let Some(capture) = native
            .busy
            .as_ref()
            .filter(|capture| {
                capture.token == prepared.token && capture.revision == prepared.revision
            })
            .cloned()
        else {
            return;
        };
        native.preparing_raster = false;
        if native.capture_deferred {
            native.deferred_raster = Some((prepared.token.clone(), prepared));
            return;
        }
        let Some(web) = native.web.clone() else {
            return;
        };
        let mut result = serde_json::Value::Null;
        if native.allowed
            && native.enabled
            && native.pipeline.revision == capture.revision
            && native.pipeline.flight.is_none()
            && native.pipeline.latest.is_none()
            && web.bounds().size == capture.viewport
            && let Some(space) = target_color_space(&web)
            && let Some(image) = prepared.image.as_ref()
        {
            // A display/profile change during the job needs the original pixels,
            // not another quantized conversion of the previous output.
            if *space != *prepared.space {
                let request = raster::Request {
                    token: prepared.token,
                    revision: capture.revision,
                    source: prepared.source,
                    space,
                    wake: native.repaint.clone(),
                };
                if native
                    .raster_worker
                    .as_ref()
                    .is_some_and(|worker| worker.request(request))
                {
                    native.preparing_raster = true;
                    return;
                }
            } else {
                let pixels = (CGImage::width(Some(image)), CGImage::height(Some(image)));
                // CALayer's contents retain this immutable prepared image.
                let cg = CFRetained::as_ptr(image).as_ptr().cast::<AnyObject>();
                unsafe {
                    let clip: Retained<AnyObject> = msg_send![class!(CALayer), layer];
                    let slices =
                        image_slices(capture.geometry.as_ref(), capture.origin, capture.size);
                    // NSView.layer requires the newer Quartz bindings;
                    // these public selectors keep the boundary local.
                    let parent: Option<Retained<AnyObject>> = msg_send![&*web, layer];
                    if parent.is_some() && !slices.is_empty() {
                        let _: () = msg_send![class!(CATransaction), begin];
                        let _: () = msg_send![class!(CATransaction), setDisableActions: true];
                        let _: () = msg_send![&*clip, setFrame: clip_bounds(web.bounds())];
                        // WKWebView's backing layer already supplies
                        // AppKit's flipped coordinates. A standalone
                        // child must not apply that flip a second time.
                        let _: () = msg_send![&*clip, setMasksToBounds: true];
                        let _: () = msg_send![&*clip, setZPosition: 1_000_000.0f64];
                        unsafe extern "C" {
                            static kCGColorSpaceSRGB: *const std::ffi::c_void;
                            fn CGColorSpaceCreateWithName(
                                name: *const std::ffi::c_void,
                            ) -> *mut std::ffi::c_void;
                            fn CGColorSpaceRelease(space: *mut std::ffi::c_void);
                            fn CGColorCreate(
                                space: *const std::ffi::c_void,
                                components: *const f64,
                            ) -> *mut std::ffi::c_void;
                            fn CGColorRelease(color: *mut std::ffi::c_void);
                        }
                        // CSS RGB is sRGB. GenericRGB changes its gamma
                        // and leaves a seam beside the captured paper.
                        let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
                        let color = CGColorCreate(space, capture.background.as_ptr());
                        let _: () = msg_send![&*clip, setBackgroundColor: color];
                        CGColorRelease(color);
                        let paper_layer: Retained<AnyObject> = msg_send![class!(CALayer), layer];
                        let paper_color = CGColorCreate(space, capture.paper_background.as_ptr());
                        let _: () = msg_send![&*paper_layer,setBackgroundColor:paper_color];
                        let _: () = msg_send![&*paper_layer,setHidden:true];
                        let _: () = msg_send![&*clip,addSublayer:&*paper_layer];
                        CGColorRelease(paper_color);
                        CGColorSpaceRelease(space);
                        let _: () = msg_send![&*clip, setHidden: true];
                        let mut layers = Vec::with_capacity(slices.len());
                        for slice in slices {
                            let layer: Retained<AnyObject> = msg_send![class!(CALayer), layer];
                            let _: () = msg_send![&*layer,setContents:cg];
                            let _: () = msg_send![&*layer,setHidden:true];
                            let _: () = msg_send![&*clip,addSublayer:&*layer];
                            layers.push(ImageLayer { layer, slice });
                        }
                        let outline_layer: Retained<AnyObject> = msg_send![class!(CALayer), layer];
                        let _: () = msg_send![&*outline_layer,setFrame:NSRect::new(NSPoint::new(0.0,0.0),clip_bounds(web.bounds()).size)];
                        let _: () = msg_send![&*clip,addSublayer:&*outline_layer];
                        let _: () = msg_send![class!(CATransaction), commit];
                        native.candidate = Some(PreparedImage {
                            capture: capture.clone(),
                            clip,
                            layers,
                            paper_layer,
                            outline_layer,
                            pixels,
                        });
                        result = serde_json::json!({"width":pixels.0,"height":pixels.1});
                    }
                }
            }
        }
        native.busy = None;
        unsafe {
            web.evaluateJavaScript_completionHandler(
                &NSString::from_str(&format!(
                    "window.tiptoptypZoomSnapshot?.resolve({}, {});",
                    serde_json::json!(capture.token),
                    result
                )),
                None,
            );
        }
    }

    // Native-only deterministic layer capture; never used by ordinary rendering.
    #[cfg(any(feature = "native-ui-tests", feature = "desktop-ui-tests"))]
    #[allow(dead_code)]
    pub(crate) fn write_overlay_png(&self, path: &std::path::Path) {
        let native = self.0.borrow();
        // An isolated clip-layer image verifies ink/opacity, not composition
        // in the host view. Its root coordinate system differs from the host.
        let layer = native.clip.as_ref().expect("active native overlay");
        let size = clip_bounds(NSRect::new(NSPoint::new(0.0, 0.0), native.layout_size)).size;
        let width = size.width as usize;
        let height = size.height as usize;
        let mut bytes = vec![0u8; width * height * 4];
        unsafe extern "C" {
            fn CGColorSpaceCreateDeviceRGB() -> *mut std::ffi::c_void;
            fn CGColorSpaceRelease(space: *mut std::ffi::c_void);
            fn CGBitmapContextCreate(
                data: *mut u8,
                width: usize,
                height: usize,
                bits: usize,
                stride: usize,
                space: *mut std::ffi::c_void,
                bitmap: u32,
            ) -> *mut std::ffi::c_void;
            fn CGContextRelease(context: *mut std::ffi::c_void);
            fn CGContextTranslateCTM(context: *mut std::ffi::c_void, x: f64, y: f64);
            fn CGContextScaleCTM(context: *mut std::ffi::c_void, x: f64, y: f64);
        }
        unsafe {
            let space = CGColorSpaceCreateDeviceRGB();
            let context = CGBitmapContextCreate(
                bytes.as_mut_ptr(),
                width,
                height,
                8,
                width * 4,
                space,
                0x4001,
            );
            assert!(!context.is_null());
            // A bitmap CGContext starts bottom-up; PNG rows and the WebKit
            // viewport start at the top. Apply the host coordinate conversion,
            // leaving the actual layer frames and texture crops untouched.
            CGContextTranslateCTM(context, 0.0, height as f64);
            CGContextScaleCTM(context, 1.0, -1.0);
            let _: () = msg_send![&**layer, renderInContext: context];
            CGContextRelease(context);
            CGColorSpaceRelease(space);
        }
        image::save_buffer(
            path,
            &bytes,
            width as u32,
            height as u32,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }

    #[cfg(any(feature = "desktop-ui-tests", feature = "native-ui-tests"))]
    #[allow(dead_code)] // Read by the opt-in native harness, outside the app binary.
    pub(crate) fn inspect(&self) -> serde_json::Value {
        let native = self.0.borrow();
        let transform = if native.transform.ratio > 0.0 {
            native.transform
        } else {
            Transform {
                ratio: 1.0,
                ..Default::default()
            }
        };
        let mask = native.clip.as_ref().map(|layer| {
            let frame: NSRect = unsafe { msg_send![&**layer, frame] };
            [
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            ]
        });
        let view_size = native
            .web
            .as_ref()
            .map(|web| [web.bounds().size.width, web.bounds().size.height]);
        serde_json::json!({"mask_bounds":mask,"view_size":view_size,"enabled":native.enabled,"cached":native.token.is_some(),"active":native.overlay_visible,
            "token":native.token.as_deref(),"source_pages":native.geometry.as_ref().map(|g|&g.pages),
            "source_scale":native.geometry.as_ref().map(|g|g.source_scale),
            "inputs":native.inputs,"hits":native.hits,"handoffs":native.handoffs,"scale":native.pipeline.scale,
            "revision":native.pipeline.revision,"in_flight":native.pipeline.flight.map(|request|request.revision),
            "pending":native.pipeline.latest.map(|request|request.revision),"deferred":native.gate.due.is_some(),"pinch":native.gate.pinch,"pixels":[native.pixels.0,native.pixels.1],"base_scale":native.base_scale,"source_paper":native.geometry.as_ref().map(|g|g.paper),
            "transform":[native.transform.ratio,native.transform.dx,native.transform.dy],
            "source_anchor":native.geometry.as_ref().filter(|_|native.token.is_some()).and_then(|g|g.point(transform,native.anchor)),
            "target_scroll":native.geometry.as_ref().filter(|_|native.token.is_some()).map(|g|g.target(transform,native.layout_size)),"image_origin":[native.image_origin.x,native.image_origin.y],"image_size":[native.image_size.width,native.image_size.height],"image_layers":native.image_layers.len(),"prepared":native.candidate.is_some(),"capturing":native.busy.is_some(),"capture_deferred":native.capture_deferred,"retained_capture":native.deferred_raster.is_some(),"raster_worker":native.raster_worker.is_some(),"preparing_raster":native.preparing_raster,"capture_view":native.mirror.as_ref().map(|m|[m.web.frame().size.width,m.web.frame().size.height])})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_reload_reuses_one_pixel_worker_instead_of_spawning_overlapping_jobs() {
        let mut state = CaptureState::default();
        state.0.borrow_mut().raster_worker = Some(raster::Worker::new().unwrap());
        let worker = state.0.borrow().raster_worker.as_ref().unwrap().thread_id();
        for _ in 0..25 {
            state.invalidate();
            assert_eq!(
                state.0.borrow().raster_worker.as_ref().unwrap().thread_id(),
                worker
            );
            assert!(state.0.borrow().busy.is_none());
        }
    }

    #[test]
    fn deferred_capture_results_are_consumed_once_and_never_cross_tokens() {
        let mut pending = Some(("current".into(), 42));
        assert_eq!(take_current_capture(&mut pending, "current"), Some(42));
        assert_eq!(take_current_capture(&mut pending, "current"), None);
        pending = Some(("old".into(), 7));
        assert_eq!(take_current_capture(&mut pending, "replacement"), None);
        assert!(
            pending.is_none(),
            "a canceled raster must release its bounded slot"
        );
    }

    #[test]
    fn capture_deferral_preserves_warmed_pixels_and_queued_zoom_anchors() {
        let state = CaptureState::default();
        {
            let mut native = state.0.borrow_mut();
            native.token = Some("accepted-raster".into());
            native.pipeline.scale = 1.2;
            native.pipeline.revision = 7;
            native.pipeline.flight = Some(Request {
                revision: 6,
                scale: 1.1,
                x: 20.0,
                y: 30.0,
                pan_x: 4.0,
                pan_y: 5.0,
                resize: false,
            });
            native.pipeline.latest = Some(Request {
                revision: 7,
                scale: 1.2,
                x: 40.0,
                y: 50.0,
                pan_x: 8.0,
                pan_y: 9.0,
                resize: true,
            });
            native.transform = Transform {
                ratio: 1.2,
                dx: 12.0,
                dy: 34.0,
            };
        }
        for deferred in [true, true, false, false] {
            state.set_capture_deferred(deferred);
            let native = state.0.borrow();
            assert_eq!(native.capture_deferred, deferred);
            assert_eq!(native.token.as_deref(), Some("accepted-raster"));
            assert_eq!(native.pipeline.scale, 1.2);
            assert_eq!(native.pipeline.revision, 7);
            assert_eq!(native.pipeline.flight.unwrap().revision, 6);
            assert_eq!(native.pipeline.latest.unwrap().revision, 7);
            assert_eq!(native.pipeline.latest.unwrap().x, 40.0);
            assert_eq!(native.pipeline.latest.unwrap().pan_y, 9.0);
            assert_eq!(
                (
                    native.transform.ratio,
                    native.transform.dx,
                    native.transform.dy
                ),
                (1.2, 12.0, 34.0)
            );
        }
    }

    #[test]
    fn scroll_phase_boundaries_retain_idle_pixels_until_content_actually_moves() {
        assert!(!manual_scroll(0.0, 0.0));
        assert!(!manual_scroll(-0.0, 0.0));
        assert!(!manual_scroll(f64::NAN, 0.0));
        for (dx, dy) in [(0.01, 0.0), (0.0, -0.01), (1.0, 2.0)] {
            assert!(manual_scroll(dx, dy));
        }
    }

    #[test]
    fn full_svg_last_page_uses_rounded_css_box_not_a_scaled_capture_height() {
        // Measured from the real full viewer at its bottom edge, 48 pages with
        // unequal widths. A 5% zoom previously overshot the real edge by 1px;
        // the following 20% zoom amplified the stale source point to 1.2px.
        let g = Geometry {
            paper: [0.0, -41377.0, 699.0, 41796.0],
            viewport: [0.0, -1.0, 640.0, 420.0],
            view: [640.0, 420.0],
            scroll: [0.0, 41376.0],
            source_scale: 1.2661148735757832,

            content_size: [552.0, 32640.0],
            pixel_rounded: true,
            pages: (0..48)
                .map(|i| [i as f64 * (680.0 + 10.0 / 1.2661148735757832), 680.0])
                .collect(),
        };
        let ratio = g.render_ratio(0.95, 640.0, 1.092 * 0.95);
        assert_eq!(g.height(ratio), 39729.0);
        assert_eq!(g.width(ratio), 664.0);
        let size = NSSize::new(640.0, 420.0);
        let t = g.zoom(
            Transform::default(),
            ratio,
            NSPoint::new(630.0, 400.0),
            size,
            size,
            false,
        );
        assert!((g.paper[1] * ratio + t.dy + 39310.0).abs() < 1e-7);
        assert_eq!(g.target(t, size)["top"], 39309.0);
    }

    #[test]
    fn texture_crop_keeps_svg_top_pixels_in_the_on_screen_layer() {
        let slice = ImageSlice {
            source: NSRect::new(NSPoint::new(0., 0.), NSSize::new(640., 1260.)),
            page: None,
        };
        let (frame, texture) = image_crop(
            slice,
            None,
            Transform {
                ratio: 0.875,
                dx: 0.,
                dy: 0.,
            },
            NSPoint::new(-320., -420.),
            NSSize::new(1280., 1260.),
            NSSize::new(560., 420.),
        )
        .unwrap();
        assert_eq!(
            frame,
            NSRect::new(NSPoint::new(0., 0.), NSSize::new(560., 420.))
        );
        assert!((texture.origin.y - 1. / 3.).abs() < 1e-12);
        // Recover the source's top from the on-screen texture crop.
        let source_top = -420. + texture.origin.y * 1260.;
        assert!(source_top.abs() < 1e-10);
    }

    #[test]
    fn pinch_uses_live_cursor_even_when_event_reports_the_center() {
        let event = NSPoint::new(320.0, 210.0);
        let cursor = NSPoint::new(56.0, 330.0);
        assert_eq!(input_position(NSEventType::Magnify, event, cursor), cursor);
        assert_eq!(
            input_position(NSEventType::ScrollWheel, event, cursor),
            event
        );
        let bounds = NSRect::new(NSPoint::new(10.0, 20.0), NSSize::new(640.0, 420.0));
        assert_eq!(viewport_point(cursor, bounds, true), Some((46.0, 310.0)));
        assert_eq!(viewport_point(cursor, bounds, false), Some((46.0, 110.0)));
        assert!(viewport_point(NSPoint::new(9.0, 20.0), bounds, true).is_none());
        assert!(viewport_point(NSPoint::new(650.0, 20.0), bounds, true).is_none());
        assert!(viewport_point(NSPoint::new(f64::NAN, 20.0), bounds, true).is_none());
    }

    #[test]
    fn outline_above_the_mask_validates_theme_strokes() {
        let value = serde_json::json!({"width":2,"color":[255,128,0,0.5]});
        assert_eq!(outline(&value), Some((2.0, [1.0, 128.0 / 255.0, 0.0, 0.5])));
        assert!(outline(&serde_json::json!({"width":-1,"color":[0,0,0,1]})).is_none());
        assert!(outline(&serde_json::json!({"width":1,"color":[0,0,0,2]})).is_none());
    }

    #[test]
    fn browser_frame_cannot_claim_a_resize_before_the_native_cache_owner() {
        let mut native = Native::default();
        native.pipeline.scale = 1.0;
        native.token = Some("warm-raster".into());
        native.layout_size = NSSize::new(640.0, 420.0);
        native.observe_frame(1.0, NSSize::new(560.0, 420.0));
        assert_eq!(native.layout_size, NSSize::new(640.0, 420.0));
        assert_eq!(native.token.as_deref(), Some("warm-raster"));
        native.observe_frame(1.1, NSSize::new(560.0, 420.0));
        assert!(native.token.is_none());
        assert_eq!(native.layout_size, NSSize::new(560.0, 420.0));
        assert_eq!(native.pipeline.scale, 1.1);
    }

    #[test]
    fn capture_rejects_missing_unstable_and_mismatched_geometry() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/tinymist-zoom-edges.json"))
                .unwrap();
        let mut request = serde_json::json!({"width":900,"height":700,"geometry":data["cases"][0]["before"]["geometry"]});
        let size = NSSize::new(900.0, 700.0);
        assert!(capture_geometry(&request, size).is_some());
        assert!(capture_geometry(&request, NSSize::new(640.0, 700.0)).is_none());
        request["geometry"]["scroll"][1] = (-1.0).into();
        assert!(capture_geometry(&request, size).is_none());
        request["geometry"] = serde_json::Value::Null;
        assert!(capture_geometry(&request, size).is_none());
    }

    #[test]
    fn cached_slices_cannot_rescale_chrome_outside_paper_into_the_view() {
        let g = Geometry {
            paper: [80.0, 0.0, 440.0, 1210.0],
            viewport: [0.0, 0.0, 600.0, 400.0],
            view: [600.0, 400.0],
            scroll: [0.0, 0.0],
            source_scale: 1.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: vec![[0.0, 600.0], [610.0, 600.0]],
        };
        let slices = image_slices(
            Some(&g),
            NSPoint::new(-300.0, -400.0),
            NSSize::new(1200.0, 1200.0),
        );
        for slice in slices {
            assert_eq!(slice.source.origin.x, 80.0);
            assert_eq!(slice.source.size.width, 440.0);
        }
        let bounds = paper_clip(
            &g,
            Transform {
                ratio: 0.5,
                dx: 150.0,
                dy: 0.0,
            },
            NSSize::new(596.0, 396.0),
        )
        .unwrap();
        assert_eq!(bounds.origin.x, 190.0);
        assert_eq!(bounds.size.width, 220.0);
        assert_eq!(bounds.origin.y, 0.0);
        assert_eq!(bounds.size.height, 396.0);
    }

    #[test]
    fn zoom_out_centers_paper_and_clamps_to_document_edges() {
        let g = Geometry {
            paper: [0.0, 0.0, 600.0, 2400.0],
            viewport: [0.0, 0.0, 600.0, 400.0],
            view: [600.0, 400.0],
            scroll: [0.0, 0.0],
            source_scale: 0.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: vec![],
        };
        // The old freely scaled viewport would move the first page down 100px.
        let free = Transform::default().zoom(0.5, 100.0, 200.0);
        assert_eq!(free.dy, 100.0);
        let fixed = g.constrain(free, NSSize::new(600.0, 400.0));
        assert_eq!(fixed.dx, 150.0);
        assert_eq!(fixed.dy, 0.0);
        assert_eq!(
            g.target(fixed, NSSize::new(600.0, 400.0)),
            serde_json::json!({"left":0.0,"top":0.0})
        );
        let bottom = Geometry {
            paper: [-500.0, -2000.0, 1200.0, 2400.0],
            scroll: [500.0, 2000.0],
            ..g
        };
        let fixed = bottom.constrain(
            Transform::default().zoom(0.5, 300.0, 200.0),
            NSSize::new(600.0, 400.0),
        );
        assert_eq!(
            bottom.target(fixed, NSSize::new(600.0, 400.0)),
            serde_json::json!({"left":0.0,"top":800.0})
        );
    }

    #[test]
    fn temporarily_uncovered_zoom_retains_the_raster_and_requested_geometry() {
        let geometry = Geometry {
            paper: [0.0, 0.0, 600.0, 2400.0],
            viewport: [0.0, 0.0, 600.0, 400.0],
            view: [600.0, 400.0],
            scroll: [0.0, 0.0],
            source_scale: 0.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: vec![],
        };
        // No AppKit objects are required for this cache/state invariant.
        let mut native = Native::default();
        native.token = Some("idle-raster".into());
        native.geometry = Some(geometry);
        native.base_scale = 1.0;
        native.fit_width = 600.0;
        native.size = NSSize::new(600.0, 400.0);
        native.layout_size = native.size;
        native.image_origin = NSPoint::new(-300.0, -400.0);
        native.image_size = NSSize::new(1200.0, 1200.0);
        native.pipeline.scale = 0.2;
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), native.size);
        native.draw_image(bounds, 0.0, 0.0, false);
        assert_eq!(
            native.token.as_deref(),
            Some("idle-raster"),
            "temporary coverage loss discarded warmed pixels"
        );
        assert!(
            (native.transform.ratio - 0.2).abs() < 1e-9,
            "sharp fallback lost the requested anchor geometry"
        );
        assert!(
            !native.overlay_visible,
            "incomplete pixels must never mask uncaptured paper"
        );
        native.pipeline.scale = 1.0;
        native.draw_image(bounds, 0.0, 0.0, false);
        assert_eq!(native.token.as_deref(), Some("idle-raster"));
        assert!(
            image_covers(
                native.geometry.as_ref().unwrap(),
                native.transform,
                native.image_origin,
                native.image_size,
                bounds.size
            ),
            "returning to a covered zoom should reuse the old raster"
        );
    }

    #[test]
    fn partial_bottom_edge_keeps_fixed_page_gaps() {
        let g = Geometry {
            paper: [0.0, -11790.0, 600.0, 12190.0],
            viewport: [0.0, 0.0, 600.0, 400.0],
            view: [600.0, 400.0],
            scroll: [0.0, 11790.0],
            source_scale: 1.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: (0..20).map(|i| [i as f64 * 610.0, 600.0]).collect(),
        };
        let size = NSSize::new(600.0, 400.0);
        let t = g.zoom(
            Transform::default(),
            0.5,
            NSPoint::new(599.0, 399.0),
            size,
            size,
            false,
        );
        // 20 half-sized pages plus 19 unchanged 10px gaps = 6190px.
        assert_eq!(g.paper[1] * t.ratio + t.dy, -5790.0);
    }

    #[test]
    fn resize_anchors_the_top_and_retains_scrollbar_gutter() {
        let g = Geometry {
            paper: [-80.0, -900.0, 600.0, 2400.0],
            viewport: [0.0, 0.0, 600.0, 400.0],
            view: [615.0, 415.0],
            scroll: [80.0, 900.0],
            source_scale: 0.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: vec![],
        };
        let size = NSSize::new(815.0, 415.0);
        let t = g.zoom(
            Transform::default(),
            4.0 / 3.0,
            NSPoint::new(0.0, 0.0),
            NSSize::new(615.0, 415.0),
            size,
            true,
        );
        let target = g.target(t, size);
        assert_eq!(target["left"], 0.0);
        assert_eq!(target["top"], 1200.0);
        assert!((t.dx - 80.0 * 4.0 / 3.0).abs() < 1e-9);
        assert_eq!(t.dy, 0.0);
    }

    #[test]
    fn cached_resize_burst_has_one_latest_request_and_keeps_pending_zoom() {
        let mut p = Pipeline {
            scale: 1.0,
            ..Default::default()
        };
        for _ in 0..10_000 {
            assert!(p.resize(true).is_none());
            assert!(p.flight.is_none());
        }
        assert!(p.latest.unwrap().resize);
        p.queue(1.1, false, 300.0, 200.0, true);
        p.resize(true);
        let request = p.dispatch().unwrap();
        assert!(!request.resize);
        assert_eq!(request.revision, 10_002);
        assert_eq!(request.scale, 1.1);
        assert!(p.dispatch().is_none());
        p.resize(true);
        assert!(p.ready(request.revision, request.scale));
        assert!(p.dispatch().unwrap().resize);
    }

    #[test]
    fn buffered_partial_anchor_keeps_page_local_positions_and_gap_fractions() {
        let g = Geometry {
            paper: [0.0, -620.0, 600.0, 1830.0],
            viewport: [0.0, 0.0, 600.0, 400.0],
            view: [600.0, 400.0],
            scroll: [0.0, 620.0],
            source_scale: 1.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: vec![[0.0, 600.0], [610.0, 600.0], [1220.0, 600.0]],
        };
        let size = NSSize::new(600.0, 400.0);
        let t = g.zoom(
            Transform::default(),
            2.0,
            NSPoint::new(300.0, 200.0),
            size,
            size,
            false,
        );
        let p = g.point(t, NSPoint::new(300.0, 200.0)).unwrap();
        assert_eq!(p["y"], serde_json::json!({"page":1,"local":210.0}));
        assert_eq!(p["x"], 300.0);
        assert_eq!(p["clientY"], 200.0);
        let gap = g
            .point(
                Transform {
                    ratio: 1.0,
                    dx: 0.0,
                    dy: 0.0,
                },
                NSPoint::new(100.0, -15.0),
            )
            .unwrap();
        assert_eq!(
            gap["y"],
            serde_json::json!({"before":0,"after":1,"fraction":0.5})
        );
    }

    #[test]
    fn cached_geometry_matches_recorded_real_full_and_partial_edge_zooms() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/tinymist-zoom-edges.json"))
                .unwrap();
        for case in data["cases"].as_array().unwrap() {
            let g: Geometry = serde_json::from_value(case["before"]["geometry"].clone()).unwrap();
            let actual: Geometry =
                serde_json::from_value(case["after"]["geometry"].clone()).unwrap();
            let scale = case["scale"].as_f64().unwrap();
            let ratio = g.render_ratio(scale, case["before"]["fitWidth"].as_f64().unwrap(), scale);
            let size = NSSize::new(g.view[0], g.view[1]);
            let anchor = NSPoint::new(
                case["anchor"][0].as_f64().unwrap(),
                case["anchor"][1].as_f64().unwrap(),
            );
            let t = g.zoom(Transform::default(), ratio, anchor, size, size, false);
            for (got, want) in [
                (g.paper[0] * ratio + t.dx, actual.paper[0]),
                (g.paper[1] * ratio + t.dy, actual.paper[1]),
                (g.height(ratio), actual.paper[3]),
            ] {
                assert!(
                    (got - want).abs() <= 1.0,
                    "{} {} {} anchor {anchor:?}: cached={got} live={want}",
                    case["mode"],
                    case["edge"],
                    case["scale"]
                );
            }
        }
    }

    #[test]
    fn partial_page_gaps_stay_fixed_at_every_intermediate_zoom() {
        let g = Geometry {
            paper: [0.0, -650.0, 600.0, 3040.0],
            viewport: [0.0, 0.0, 600.0, 400.0],
            scroll: [0.0, 650.0],
            view: [600.0, 400.0],
            source_scale: 1.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: (0..5).map(|i| [i as f64 * 610.0, 600.0]).collect(),
        };
        let size = NSSize::new(600.0, 400.0);
        let anchor = NSPoint::new(550.0, 350.0);
        let mut t = Transform::default();
        for ratio in [1.2, 1.5, 1.1, 0.8, 1.0] {
            t = g.zoom(t, ratio, anchor, size, size, false);
            let p = g.point(t, anchor).unwrap();
            assert_eq!(p["y"]["page"], 1);
            assert!((p["y"]["local"].as_f64().unwrap() - 390.0).abs() < 1e-9);
            for i in 1..5 {
                assert!(
                    (g.offset(i, ratio) - g.offset(i - 1, ratio) - 600.0 * ratio - 10.0).abs()
                        < 1e-9
                );
            }
        }
    }

    #[test]
    fn divider_resize_preserves_scroll_after_zoom_not_capture_scroll() {
        let g = Geometry {
            paper: [-150.0, -900.0, 600.0, 3000.0],
            viewport: [0.0, 0.0, 600.0, 400.0],
            scroll: [150.0, 900.0],
            view: [600.0, 400.0],
            source_scale: 0.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: vec![],
        };
        let previous = NSSize::new(600.0, 400.0);
        let size = NSSize::new(700.0, 400.0);
        let old = g.zoom(
            Transform::default(),
            2.0,
            NSPoint::new(500.0, 200.0),
            previous,
            previous,
            false,
        );
        let before = g.target(old, previous);
        assert_ne!(before["left"], 150.0);
        let after = g.zoom(
            old,
            2.0 * 700.0 / 600.0,
            NSPoint::new(0.0, 0.0),
            previous,
            size,
            true,
        );
        assert_eq!(g.target(after, size)["left"], before["left"]);
    }

    #[test]
    fn partial_texture_slices_cover_only_the_buffered_pages() {
        let g = Geometry {
            paper: [0.0, -610.0 * 5000.0, 600.0, 6_100_000.0],
            viewport: [0.0, 0.0, 600.0, 400.0],
            scroll: [0.0, 610.0 * 5000.0],
            view: [600.0, 400.0],
            source_scale: 1.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: (0..10_000).map(|i| [i as f64 * 610.0, 600.0]).collect(),
        };
        let slices = image_slices(
            Some(&g),
            NSPoint::new(-300.0, -400.0),
            NSSize::new(1200.0, 1200.0),
        );
        assert_eq!(slices.len(), 3);
        assert_eq!(
            slices.iter().map(|s| s.page.unwrap()).collect::<Vec<_>>(),
            vec![4999, 5000, 5001]
        );
        for slice in slices {
            assert!(
                slice.source.origin.y >= -400.0
                    && slice.source.origin.y + slice.source.size.height <= 800.0
            );
        }
    }

    #[test]
    fn cached_image_crops_never_submit_offscreen_surfaces() {
        let origin = NSPoint::new(-300.0, -400.0);
        let size = NSSize::new(1200.0, 1200.0);
        let clip = NSSize::new(596.0, 396.0);
        let slice = ImageSlice {
            source: NSRect::new(origin, size),
            page: None,
        };
        for ratio in [0.1, 0.9, 1.0, 2.0, 10.0, 100.0] {
            for (dx, dy) in [(-1000.0, -500.0), (0.0, 0.0), (590.0, 390.0), (1e6, 1e6)] {
                if let Some((frame, texels)) =
                    image_crop(slice, None, Transform { ratio, dx, dy }, origin, size, clip)
                {
                    assert!(frame.origin.x >= 0.0 && frame.origin.y >= 0.0);
                    assert!(
                        frame.origin.x + frame.size.width <= clip.width
                            && frame.origin.y + frame.size.height <= clip.height
                    );
                    assert!(texels.origin.x >= -1e-12 && texels.origin.y >= -1e-12);
                    assert!(
                        texels.origin.x + texels.size.width <= 1.0 + 1e-12
                            && texels.origin.y + texels.size.height <= 1.0 + 1e-12
                    );
                }
            }
        }
    }

    #[test]
    fn cached_burst_sends_only_latest_geometry_after_quiet() {
        let mut queue = Pipeline {
            scale: 1.0,
            ..Default::default()
        };
        let mut gate = RenderGate::default();
        let start = Instant::now();
        for index in 0..100 {
            assert!(queue.queue(1.001, false, 300.0, 200.0, true).is_none());
            gate.input(start + Duration::from_millis(index * 10));
            assert!(queue.flight.is_none());
            assert!(!gate.take(start + Duration::from_millis(index * 10 + 119), false));
        }
        assert!(gate.take(start + Duration::from_millis(1110), false));
        let request = queue.dispatch().unwrap();
        assert_eq!(request.revision, 100);
        assert!((request.scale - 1.001f64.powi(100)).abs() < 1e-9);
        assert!(queue.latest.is_none());
        assert!(queue.dispatch().is_none());
    }

    #[test]
    fn pinch_end_flushes_without_waiting_and_missing_end_has_watchdog() {
        let start = Instant::now();
        let mut gate = RenderGate {
            pinch: true,
            ..Default::default()
        };
        gate.input(start);
        assert!(!gate.take(start + ZOOM_QUIET, false));
        assert!(gate.take(start + LOST_PINCH_END, false));
        assert!(!gate.pinch);
        gate.pinch = true;
        gate.input(start);
        assert!(gate.take(start, true));
        gate.input(start);
        gate.clear();
        assert!(gate.due.is_none());
        assert!(!gate.pinch);
    }

    #[test]
    fn new_gesture_waits_even_when_older_render_finishes() {
        let mut queue = Pipeline {
            scale: 1.0,
            ..Default::default()
        };
        let first = queue.input(1.1, false, 300.0, 200.0).unwrap();
        queue.queue(1.1, false, 400.0, 300.0, true);
        let mut gate = RenderGate::default();
        let now = Instant::now();
        gate.input(now);
        assert!(queue.ready(first.revision, first.scale));
        assert!(queue.flight.is_none());
        assert!(!gate.take(now, false));
        queue.queue(1.1, false, 450.0, 350.0, true);
        assert!(gate.take(now + ZOOM_QUIET, false));
        let last = queue.dispatch().unwrap();
        let actual = Transform::default().zoom(last.scale / first.scale, last.x, last.y);
        let expected = Transform::default()
            .zoom(1.1, 400.0, 300.0)
            .zoom(1.21, 450.0, 350.0);
        assert!((actual.dx - expected.dx).abs() < 1e-9);
        assert!((actual.dy - expected.dy).abs() < 1e-9);
    }

    #[test]
    fn deferred_round_trip_preserves_pan_and_cold_zoom_is_immediate() {
        let mut queue = Pipeline {
            scale: 1.0,
            ..Default::default()
        };
        assert!(queue.queue(1.1, false, 300.0, 200.0, true).is_none());
        queue.queue(1.0 / 1.1, false, 400.0, 300.0, true);
        let request = queue.dispatch().unwrap();
        assert!((request.scale - 1.0).abs() < 1e-9);
        assert!(request.pan_x.abs() > 0.0 && request.pan_y.abs() > 0.0);
        queue.ready(request.revision, request.scale);
        assert!(queue.queue(1.1, false, 300.0, 200.0, false).is_some());
    }

    #[test]
    fn retained_raster_never_covers_uncaptured_paper_after_large_zoom_out() {
        let full = Geometry {
            paper: [0.0, 0.0, 640.0, 20000.0],
            viewport: [0.0, 0.0, 640.0, 420.0],
            scroll: [0.0, 0.0],
            view: [640.0, 420.0],
            source_scale: 0.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: vec![],
        };
        let origin = NSPoint::new(-320.0, -420.0);
        let image = NSSize::new(1280.0, 1260.0);
        let view = NSSize::new(640.0, 420.0);
        assert!(image_covers(
            &full,
            Transform {
                ratio: 1.2,
                dx: -30.0,
                dy: -40.0
            },
            origin,
            image,
            view
        ));
        assert!(!image_covers(
            &full,
            full.constrain(
                Transform {
                    ratio: 0.1,
                    dx: 0.0,
                    dy: 0.0
                },
                view
            ),
            origin,
            image,
            view
        ));
        let partial = Geometry {
            source_scale: 1.0,

            content_size: [0.0, 0.0],
            pixel_rounded: false,
            pages: (0..100).map(|i| [i as f64 * 610.0, 600.0]).collect(),
            ..full
        };
        assert!(image_covers(
            &partial,
            Transform {
                ratio: 1.2,
                dx: -30.0,
                dy: -40.0
            },
            origin,
            image,
            view
        ));
        assert!(!image_covers(
            &partial,
            partial.constrain(
                Transform {
                    ratio: 0.1,
                    dx: 0.0,
                    dy: 0.0
                },
                view
            ),
            origin,
            image,
            view
        ));
    }

    #[test]
    fn sharp_handoff_retains_valid_raster_and_transform_but_abort_releases_it() {
        let mut native = Native::default();
        native.token = Some("valid".into());
        native.base_scale = 1.0;
        native.transform = Transform {
            ratio: 1.2,
            dx: -30.0,
            dy: -40.0,
        };
        native.overlay_visible = true;
        native.pipeline.scale = 1.2;
        native.finish_handoff();
        assert_eq!(native.token.as_deref(), Some("valid"));
        assert_eq!(native.transform.ratio, 1.2);
        assert_eq!(native.transform.dx, -30.0);
        assert_eq!(native.transform.dy, -40.0);
        assert!(!native.overlay_visible);
        assert!(
            native
                .pipeline
                .queue(1.1, false, 500.0, 300.0, true)
                .is_none()
        );
        assert!(native.pipeline.flight.is_none());
        native.clear();
        assert!(native.token.is_none());
        assert_eq!(native.transform.ratio, 0.0);
        assert!(native.pipeline.latest.is_none());
    }

    #[test]
    fn abort_discards_deferred_request_but_pixel_retirement_preserves_it() {
        let mut native = Native::default();
        native.pipeline.scale = 1.0;
        native.pipeline.queue(1.1, false, 300.0, 200.0, true);
        native.gate.input(Instant::now());
        native.retire_image();
        assert!(native.pipeline.latest.is_some() && native.gate.due.is_some());
        native.clear();
        assert!(native.pipeline.latest.is_none() && native.gate.due.is_none());
    }

    #[test]
    fn zoom_evaluation_requires_an_accepted_result_without_an_error() {
        let accepted = NSNumber::new_bool(true);
        let rejected = NSNumber::new_bool(false);
        assert!(zoom_request_accepted(Some(&accepted), false));
        assert!(!zoom_request_accepted(Some(&rejected), false));
        assert!(!zoom_request_accepted(None, false));
        assert!(!zoom_request_accepted(Some(&accepted), true));
        assert!(!zoom_request_accepted(
            Some(&NSString::from_str("true")),
            false
        ));
    }

    #[test]
    fn rejected_flight_discards_obsolete_inputs_and_waits_for_a_ready_renderer() {
        let mut native = Native::default();
        native.initialized = true;
        native.pipeline.scale = 1.0;
        let first = native.pipeline.input(1.1, false, 300.0, 200.0).unwrap();
        native.pipeline.queue(1.1, false, 400.0, 300.0, true);
        let revision = native.pipeline.revision;
        native.gate.pinch = true;
        native.gate.input(Instant::now());
        native.token = Some("obsolete".into());
        native.flight_token = native.token.clone();
        native.overlay_visible = true;
        native.transform = Transform {
            ratio: 1.21,
            dx: -70.0,
            dy: -50.0,
        };

        assert!(native.reject_request(first.revision));
        assert!(!native.initialized);
        assert!(native.pipeline.flight.is_none());
        assert!(native.pipeline.latest.is_none());
        assert_eq!(native.pipeline.revision, revision);
        assert!(native.pipeline.pending_base.is_none());
        assert_eq!(native.pipeline.pending_transform.ratio, 0.0);
        assert!(native.gate.due.is_none());
        assert!(!native.gate.pinch);
        assert!(native.token.is_none() && native.flight_token.is_none());
        assert!(!native.overlay_visible);
        assert_eq!(native.transform.ratio, 0.0);
    }

    #[test]
    fn abandoned_renderer_stops_an_idle_capture_and_discards_queued_zoom() {
        let mut native = Native::default();
        native.initialized = true;
        native.pipeline.scale = 1.0;
        let size = NSSize::new(640.0, 420.0);
        native.busy = Some(Capture {
            token: "pre-input-capture".into(),
            revision: native.pipeline.revision,
            viewport: size,
            origin: NSPoint::new(0.0, 0.0),
            size,
            fit_width: size.width,
            density: 1.0,
            geometry: None,
            background: [1.0; 4],
            paper_background: [1.0; 4],
            buffered: false,
        });
        native.rasterizing = true;
        let abandoned = native.pipeline.input(1.1, false, 300.0, 200.0).unwrap();
        native.pipeline.queue(1.1, false, 400.0, 300.0, true);

        assert!(native.reject_request(abandoned.revision));
        assert!(native.busy.is_none());
        assert!(!native.rasterizing);
        assert!(!native.initialized);
        assert!(native.pipeline.flight.is_none() && native.pipeline.latest.is_none());
        assert!(!native.reject_request(abandoned.revision));
    }

    #[test]
    fn late_rejection_cannot_cancel_a_newer_flight_or_its_pending_input() {
        let mut native = Native::default();
        native.initialized = true;
        native.pipeline.scale = 1.0;
        let first = native.pipeline.input(1.1, false, 300.0, 200.0).unwrap();
        assert!(native.pipeline.ready(first.revision, first.scale));
        let current = native.pipeline.input(1.1, false, 400.0, 300.0).unwrap();
        native.pipeline.queue(1.1, false, 450.0, 350.0, true);
        let pending = native.pipeline.latest.unwrap().revision;
        native.gate.input(Instant::now());
        native.token = Some("current".into());
        native.flight_token = native.token.clone();
        native.overlay_visible = true;

        assert!(!native.reject_request(first.revision));
        assert!(native.initialized && native.overlay_visible);
        assert_eq!(native.pipeline.flight.unwrap().revision, current.revision);
        assert_eq!(native.pipeline.latest.unwrap().revision, pending);
        assert_eq!(native.token.as_deref(), Some("current"));
        assert_eq!(native.flight_token.as_deref(), Some("current"));
        assert!(native.gate.due.is_some());
        assert!(native.pipeline.ready(current.revision, current.scale));
        assert_eq!(native.pipeline.dispatch().unwrap().revision, pending);
    }

    #[test]
    fn ready_renderer_restores_zoom_after_a_rejected_request() {
        let mut native = Native::default();
        native.initialized = true;
        native.pipeline.scale = 1.0;
        let rejected = native.pipeline.input(1.1, false, 300.0, 200.0).unwrap();
        native.pipeline.queue(1.1, false, 400.0, 300.0, true);
        let obsolete_revision = native.pipeline.revision;
        assert!(native.reject_request(rejected.revision));
        assert!(!native.initialized);

        let size = NSSize::new(560.0, 420.0);
        native.observe_frame(0.85, size);
        assert!(native.initialized);
        assert_eq!(native.layout_size, size);
        assert_eq!(native.pipeline.scale, 0.85);
        let next = native.pipeline.input(1.1, false, 280.0, 210.0).unwrap();
        assert!(next.revision > obsolete_revision);
        assert!((next.scale - 0.935).abs() < 1e-9);
        assert!(!native.reject_request(rejected.revision));
        assert!(native.pipeline.ready(next.revision, next.scale));
    }

    #[test]
    fn one_vector_flight_and_latest_replacement_for_arbitrarily_many_inputs() {
        let mut queue = Pipeline {
            scale: 1.0,
            ..Default::default()
        };
        let first = queue.input(1.01, false, 300.0, 200.0).unwrap();
        for _ in 0..10_000 {
            assert!(queue.input(1.00001, false, 300.0, 200.0).is_none());
        }
        assert_eq!(queue.flight.unwrap().revision, first.revision);
        assert_eq!(queue.latest.unwrap().revision, 10_001);
        assert!(!queue.ready(first.revision + 1, first.scale));
        assert!(!queue.ready(first.revision, 1.0));
        assert!(queue.ready(first.revision, first.scale));
        let latest = queue.dispatch().unwrap();
        assert_eq!(latest.revision, 10_001);
        assert!(queue.latest.is_none());
        assert!(queue.ready(latest.revision, latest.scale));
        assert!(queue.flight.is_none());
    }
    #[test]
    fn zero_and_capped_gestures_do_not_queue_identical_renders() {
        let mut queue = Pipeline {
            scale: 1.0,
            ..Default::default()
        };
        for _ in 0..1000 {
            assert!(queue.input(1.0, false, 300.0, 200.0).is_none());
        }
        assert_eq!(queue.revision, 0);
        assert!(queue.flight.is_none());
        queue.input(100.0, false, 300.0, 200.0);
        let revision = queue.revision;
        for _ in 0..1000 {
            assert!(queue.input(1.1, false, 300.0, 200.0).is_none());
        }
        assert_eq!(queue.revision, revision);
        assert!(queue.latest.is_none());
    }

    #[test]
    fn moving_pointer_transform_compounds_without_resampling() {
        let transform = Transform::default()
            .zoom(1.1, 300.0, 200.0)
            .zoom(1.21, 400.0, 300.0);
        assert!((transform.dx + 73.0).abs() < 1e-9);
        assert!((transform.dy + 52.0).abs() < 1e-9);
        let reset = transform.zoom(1.0, 400.0, 300.0);
        assert_eq!(reset.ratio, 1.0);
    }
    #[test]
    fn scale_limits_reset_and_invalid_inputs_are_bounded() {
        let mut queue = Pipeline {
            scale: 1.0,
            ..Default::default()
        };
        queue.input(100.0, false, 0.0, 0.0);
        assert_eq!(queue.scale, 10.0);
        queue.input(0.0001, false, 0.0, 0.0);
        assert_eq!(queue.scale, 0.1);
        queue.input(1.0, true, 0.0, 0.0);
        assert_eq!(queue.scale, 1.0);
        let revision = queue.revision;
        for factor in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            queue.input(factor, false, 0.0, 0.0);
        }
        queue.input(1.1, false, f64::NAN, 0.0);
        assert_eq!(queue.revision, revision);
    }
    #[test]
    fn coalescing_moving_anchors_preserves_the_entire_gesture_transform() {
        let mut queue = Pipeline {
            scale: 1.0,
            ..Default::default()
        };
        let first = queue.input(1.1, false, 300.0, 200.0).unwrap();
        queue.input(1.1, false, 400.0, 300.0);
        queue.input(1.1, false, 450.0, 350.0);
        assert!(queue.ready(first.revision, first.scale));
        let last = queue.dispatch().unwrap();
        let actual = Transform::default().zoom(last.scale / first.scale, last.x, last.y);
        let expected = Transform::default()
            .zoom(1.1, 400.0, 300.0)
            .zoom(1.21, 450.0, 350.0);
        assert!((actual.dx - expected.dx).abs() < 1e-9);
        assert!((actual.dy - expected.dy).abs() < 1e-9);
        queue.ready(last.revision, last.scale);
        let next = queue.input(1.1, false, 300.0, 200.0).unwrap();
        queue.input(1.1, false, 400.0, 300.0);
        queue.input(1.0 / 1.1, false, 450.0, 350.0);
        queue.ready(next.revision, next.scale);
        let round_trip = queue.dispatch().unwrap();
        assert!((round_trip.scale - next.scale).abs() < 1e-9);
        assert!(round_trip.pan_x.abs() > 0.0);
        assert!(round_trip.pan_y.abs() > 0.0);
    }

    #[test]
    fn retiring_pixels_preserves_in_flight_and_latest_zoom_until_acknowledged() {
        let mut native = Native::default();
        native.pipeline.scale = 1.0;
        let first = native.pipeline.input(1.1, false, 300.0, 200.0).unwrap();
        native.pipeline.input(1.1, false, 300.0, 200.0);
        native.token = Some("image".to_owned());
        native.flight_token = Some("image".to_owned());
        native.retire_image();
        assert!(native.token.is_none());
        assert_eq!(native.flight_token.as_deref(), Some("image"));
        assert_eq!(native.pipeline.flight.unwrap().revision, first.revision);
        assert!(native.pipeline.latest.is_some());
        assert!(native.pipeline.ready(first.revision, first.scale));
        assert!((native.pipeline.dispatch().unwrap().scale - 1.21).abs() < 1e-9);
        native.clear();
        assert!(native.pipeline.flight.is_none());
        assert!(native.flight_token.is_none());
    }

    #[test]
    fn overlay_mask_covers_every_native_edge_and_respects_tiny_views() {
        let bounds = clip_bounds(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(640.0, 420.0),
        ));
        assert_eq!(bounds.origin, NSPoint::new(0.0, 0.0));
        assert_eq!(bounds.size, NSSize::new(640.0, 420.0));
        let tiny = clip_bounds(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)));
        assert_eq!(tiny.size, NSSize::new(1.0, 1.0));
    }

    #[test]
    fn capture_density_remains_bounded() {
        assert_eq!(snapshot_width(900.0, 700.0, 2.0, 4.0), Some(1119.5));
        for (width, height) in [
            (900.0, 700.0),
            (3000.0, 2000.0),
            (1.0, 4000.0),
            (4000.0, 1.0),
        ] {
            let points = snapshot_width(width, height, 2.0, 4.0).unwrap();
            assert!((points * 2.0).ceil() * (points * 2.0 * height / width).ceil() <= MAX_PIXELS);
        }
        assert!(snapshot_width(f64::NAN, 700.0, 2.0, 4.0).is_none());
    }
}
