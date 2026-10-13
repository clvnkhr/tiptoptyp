// AppKit must run on the process main thread, not a libtest worker.
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/native_window.rs"]
mod native_window;

fn main() {
    if std::env::var("TIPTOPTYP_NATIVE_DRAG_TEST").as_deref() != Ok("1") {
        println!(
            "native window drag check skipped; set TIPTOPTYP_NATIVE_DRAG_TEST=1 in a macOS desktop session"
        );
        return;
    }
    #[cfg(target_os = "macos")]
    check();
}

#[cfg(target_os = "macos")]
fn check() {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSVisualEffectState, NSVisualEffectView, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSDate, NSPoint, NSRect, NSRunLoop, NSSize};

    let marker = MainThreadMarker::new().expect("AppKit main thread");
    let _app = NSApplication::sharedApplication(marker);
    let make_window = || {
        // SAFETY: AppKit main thread; retained windows are never activated.
        // The material check briefly orders one onscreen without taking focus.
        unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(marker),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0)),
                NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        }
    };
    let first = make_window();
    let second = make_window();
    first.setMovable(true);
    second.setMovable(true);
    let parent = native_window::NativeWindowHandle::from_test_view(first.contentView().unwrap());
    let other = native_window::NativeWindowHandle::from_test_view(second.contentView().unwrap());
    let retained = native_window::NativeWindowHandle::from_owner(&parent).unwrap();
    assert!(retained.is_same_window(&parent));
    assert!(!retained.is_same_window(&other));

    let guard = parent.suppress_titlebar_drag().unwrap();
    assert!(!first.isMovable());
    assert!(
        second.isMovable(),
        "a sibling window must retain its policy"
    );
    assert!(guard.matches_parent(&parent));
    assert!(!guard.matches_parent(&other));
    drop(guard);
    assert!(
        first.isMovable(),
        "empty toolbar space can move the window again"
    );
    first.setMovable(false);
    drop(parent.suppress_titlebar_drag().unwrap());
    assert!(
        !first.isMovable(),
        "restore the previous policy, not always true"
    );
    let renderer = first.contentView().unwrap();
    let children = renderer.subviews();
    assert!(parent.set_translucent_background());
    assert!(!first.isOpaque());
    assert_eq!(
        renderer.subviews(),
        children,
        "background must never insert a native child over the OpenGL renderer"
    );
    assert!(parent.set_translucent_background());
    first.setContentSize(NSSize::new(720.0, 460.0));
    assert_eq!(
        renderer.subviews(),
        children,
        "resize does not insert an overlay"
    );
    assert_eq!(
        objc2::rc::Retained::as_ptr(&renderer),
        objc2::rc::Retained::as_ptr(&first.contentView().unwrap()),
        "renderer identity must not change"
    );
    assert!(
        second.isOpaque(),
        "background changes must stay on their owning window"
    );
    let responder = first.firstResponder();
    let host = unsafe { renderer.superview() }.unwrap();
    let original_siblings = host.subviews().count();
    let blur = parent
        .blur_background(false, 8)
        .expect("native blur underlay");
    let (frame, below, identity) = blur.inspection(&parent).unwrap();
    assert!(below, "blur must be a sibling below CGL, never a child");
    assert_eq!(host.subviews().count(), original_siblings + 1);
    assert_eq!(renderer.subviews(), children);
    assert_eq!(
        frame[2..],
        [renderer.frame().size.width, renderer.frame().size.height]
    );
    assert_eq!(first.firstResponder(), responder);
    // AppKit builds material filters only for visible windows. Do not activate
    // this window or take input focus; wait briefly for its layer update.
    // A background test app has no active material. Simulate an active effect
    // directly instead of stealing the user's application/keyboard focus.
    let material = unsafe {
        // SAFETY: inspection returned the retained, live NSVisualEffectView
        // owned by `blur`; this additional retain is on the AppKit main thread.
        objc2::rc::Retained::retain(identity as *mut NSVisualEffectView).unwrap()
    };
    material.setState(NSVisualEffectState::Active);
    material.setWantsLayer(true);
    first.orderFront(None);
    let settle = || {
        first.displayIfNeeded();
        NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.15));
    };
    settle();
    material.updateLayer();
    assert_eq!(blur.effective_radius(), Some(8.0));
    for radius in [0, 16, 50, 8] {
        assert!(blur.synchronize(&parent, Some((false, radius))));
        settle();
        assert_eq!(blur.effective_radius(), Some(f64::from(radius)));
        assert_eq!(blur.inspection(&parent).unwrap().2, identity);
    }
    assert!(blur.synchronize(&parent, Some((true, 8))));
    settle();
    assert_eq!(
        blur.effective_radius(),
        Some(8.0),
        "theme refresh retains radius"
    );
    first.orderOut(None);
    for _ in 0..100 {
        assert!(blur.synchronize(&parent, None));
        assert_eq!(
            blur.inspection(&parent).unwrap().2,
            identity,
            "idle frames reuse the material view"
        );
    }
    assert!(blur.synchronize(&parent, Some((true, 8))));
    first.setContentSize(NSSize::new(480.0, 320.0));
    let (frame, below, same) = blur.inspection(&parent).unwrap();
    assert!(below);
    assert_eq!(same, identity);
    assert_eq!(
        frame[2..],
        [renderer.frame().size.width, renderer.frame().size.height]
    );
    assert_eq!(first.firstResponder(), responder);
    assert!(second.isOpaque(), "sibling-window isolation");
    let retained_blur = blur.clone();
    drop(blur);
    assert_eq!(host.subviews().count(), original_siblings + 1);
    drop(retained_blur);
    assert_eq!(
        host.subviews().count(),
        original_siblings,
        "last owner removes only its underlay"
    );
    assert_eq!(renderer.subviews(), children);
    for _ in 0..8 {
        let blur = parent.blur_background(true, 8).unwrap();
        assert!(blur.inspection(&parent).unwrap().1);
        drop(blur);
        assert_eq!(host.subviews().count(), original_siblings);
    }
    assert_eq!(
        objc2::rc::Retained::as_ptr(&renderer),
        objc2::rc::Retained::as_ptr(&first.contentView().unwrap())
    );
    println!(
        "native window drag guard, transparency and blur: stable content, resize, input, cleanup and sibling isolation passed"
    );
}
