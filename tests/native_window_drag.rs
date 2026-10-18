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
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("native window drag check requires macOS and a desktop session");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
fn check() {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSBackingStoreType, NSWindow, NSWindowStyleMask};
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
    let original_siblings = host.subviews();
    let blur = parent
        .blur_background(native_window::BlurConfig {
            radius: 8,
            end_radius: 0,
            fade: native_window::FadeDirection::None,
        })
        .expect("native compositor blur");
    assert!(blur.awaiting_visibility());
    assert!(
        blur.inspection(&parent).is_none(),
        "hidden setup is not a successful blur application"
    );
    assert_eq!(
        host.subviews(),
        original_siblings,
        "blur adds no native views"
    );
    assert_eq!(renderer.subviews(), children);
    assert_eq!(first.firstResponder(), responder);
    assert!(blur.inspection(&other).is_none(), "native owner isolation");
    assert!(!blur.synchronize(
        &other,
        Some(native_window::BlurConfig {
            radius: 50,
            end_radius: 0,
            fade: native_window::FadeDirection::None
        })
    ));
    first.orderFront(None);
    let settle = || {
        first.displayIfNeeded();
        NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.15));
    };
    settle();
    assert!(blur.synchronize(&parent, None));
    assert!(!blur.awaiting_visibility());
    let (radius, identity, applications) = blur.inspection(&parent).unwrap();
    assert_eq!((radius, applications), (8, 1));
    for (count, radius) in [0, 16, 50, 8].into_iter().enumerate() {
        assert!(blur.synchronize(
            &parent,
            Some(native_window::BlurConfig {
                radius,
                end_radius: 0,
                fade: native_window::FadeDirection::None
            })
        ));
        settle();
        assert_eq!(
            blur.inspection(&parent),
            Some((radius, identity, count as u64 + 2))
        );
    }
    let stable = blur.inspection(&parent);
    let normal_frame = first.frame();
    let maximized_frame = first.screen().expect("native display").visibleFrame();
    for _ in 0..3 {
        first.setFrame_display(maximized_frame, true);
        settle();
        assert!(blur.synchronize(&parent, None));
        assert_eq!(
            blur.inspection(&parent),
            stable,
            "maximize does not rebuild blur"
        );
        first.setFrame_display(normal_frame, true);
        settle();
        assert_eq!(blur.inspection(&parent), stable);
    }
    first.orderOut(None);
    for _ in 0..1000 {
        assert!(blur.synchronize(&parent, None));
        assert!(blur.synchronize(
            &parent,
            Some(native_window::BlurConfig {
                radius: 8,
                end_radius: 0,
                fade: native_window::FadeDirection::None
            })
        ));
        assert_eq!(
            blur.inspection(&parent),
            stable,
            "idle calls reuse compositor state"
        );
    }
    assert_eq!(first.firstResponder(), responder);
    assert!(second.isOpaque(), "sibling-window isolation");
    let retained_blur = blur.clone();
    drop(blur);
    assert_eq!(retained_blur.inspection(&parent), stable);
    drop(retained_blur);
    assert_eq!(host.subviews(), original_siblings);
    assert_eq!(renderer.subviews(), children);
    for _ in 0..8 {
        let blur = parent
            .blur_background(native_window::BlurConfig {
                radius: 8,
                end_radius: 0,
                fade: native_window::FadeDirection::None,
            })
            .unwrap();
        assert!(blur.awaiting_visibility());
        assert!(blur.inspection(&parent).is_none());
        drop(blur);
        assert_eq!(host.subviews(), original_siblings);
    }
    assert_eq!(
        objc2::rc::Retained::as_ptr(&renderer),
        objc2::rc::Retained::as_ptr(&first.contentView().unwrap())
    );
    // A directional backdrop is a layer below the renderer. Its native
    // filter and bounded mask must stay stable on ordinary frames.
    first.orderFront(None);
    settle();
    let host_layer: *mut objc2::runtime::AnyObject = unsafe { objc2::msg_send![&*host, layer] };
    println!(
        "directional host layer before attach: present={} wants_layer={} flipped={}",
        !host_layer.is_null(),
        host.wantsLayer(),
        host.isFlipped()
    );
    let directional = parent
        .blur_background(native_window::BlurConfig {
            radius: 8,
            end_radius: 0,
            fade: native_window::FadeDirection::Right,
        })
        .expect("directional blur controller");
    for direction in [
        native_window::FadeDirection::Right,
        native_window::FadeDirection::Left,
        native_window::FadeDirection::Up,
        native_window::FadeDirection::Down,
    ] {
        let config = native_window::BlurConfig {
            radius: 8,
            end_radius: 0,
            fade: direction,
        };
        assert!(directional.synchronize(&parent, Some(config)));
        settle();
        assert_eq!(
            directional.applied_fade(),
            direction,
            "directional support cannot silently fall back in this native check"
        );
        let model = directional
            .gradient_inspection(&parent)
            .expect("actual native filter");
        assert_eq!(model.1, 8.0);
        assert!(model.2.into_iter().all(|size| (2..=256).contains(&size)));
        let frame = renderer.frame();
        assert_eq!(
            model.3,
            [
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height
            ]
        );
        let stable_controller = directional.inspection(&parent);
        for _ in 0..1000 {
            assert!(directional.synchronize(&parent, None));
            assert_eq!(directional.inspection(&parent), stable_controller);
            assert_eq!(
                directional.gradient_inspection(&parent),
                Some(model),
                "idle must reuse the same filter and mask"
            );
        }
        assert_eq!(renderer.subviews(), children);
        assert_eq!(first.firstResponder(), responder);
        assert_eq!(host.subviews().len(), original_siblings.len());
        assert_eq!(
            objc2::rc::Retained::as_ptr(&renderer),
            objc2::rc::Retained::as_ptr(&first.contentView().unwrap())
        );
    }
    for (start, end) in [(3, 12), (20, 5), (0, 18), (9, 9)] {
        let config = native_window::BlurConfig {
            radius: start,
            end_radius: end,
            fade: native_window::FadeDirection::Down,
        };
        assert!(directional.synchronize(&parent, Some(config)));
        assert_eq!(
            directional.applied_fade(),
            native_window::FadeDirection::Down
        );
        let model = directional.gradient_inspection(&parent).unwrap();
        assert_eq!(model.1, f64::from(start.max(end)));
        let stable = directional.inspection(&parent);
        for _ in 0..1000 {
            assert!(directional.synchronize(&parent, None));
            assert_eq!(directional.inspection(&parent), stable);
            assert_eq!(directional.gradient_inspection(&parent), Some(model));
        }
    }
    assert!(directional.synchronize(
        &parent,
        Some(native_window::BlurConfig {
            radius: 8,
            end_radius: 0,
            fade: native_window::FadeDirection::Down,
        })
    ));
    let before_resize = directional.gradient_inspection(&parent).unwrap();
    first.setContentSize(NSSize::new(720.0, 520.0));
    settle();
    assert!(directional.synchronize(&parent, None));
    let after_resize = directional.gradient_inspection(&parent).unwrap();
    assert_ne!(after_resize.3, before_resize.3);
    assert!(
        after_resize
            .2
            .into_iter()
            .all(|size| (2..=256).contains(&size))
    );
    assert_eq!(renderer.subviews(), children);
    let resized_frame = renderer.frame();
    assert_eq!(
        after_resize.3,
        [
            resized_frame.origin.x,
            resized_frame.origin.y,
            resized_frame.size.width,
            resized_frame.size.height
        ]
    );
    // Temporarily invalid renderer geometry falls back once, then recovers
    // when the same owner becomes usable again without any setting change.
    renderer.setFrame(NSRect::new(resized_frame.origin, NSSize::new(0.0, 0.0)));
    assert!(directional.synchronize(&parent, None));
    assert_eq!(
        directional.applied_fade(),
        native_window::FadeDirection::None
    );
    let fallback = directional.inspection(&parent);
    for _ in 0..1000 {
        assert!(directional.synchronize(&parent, None));
        assert_eq!(
            directional.inspection(&parent),
            fallback,
            "no idle fallback retries"
        );
    }
    renderer.setFrame(resized_frame);
    assert!(directional.synchronize(&parent, None));
    assert_eq!(
        directional.applied_fade(),
        native_window::FadeDirection::Down
    );
    assert!(directional.gradient_inspection(&parent).is_some());
    // Native autoresizing can move the layer while leaving our renderer frame
    // unchanged. Synchronization must repair actual geometry, not just its cache.
    let layers: objc2::rc::Retained<objc2_foundation::NSArray<objc2::runtime::AnyObject>> =
        unsafe { objc2::msg_send![host_layer, sublayers] };
    let backdrop = layers.firstObject().expect("backdrop layer");
    let displaced = NSRect::new(NSPoint::new(13.0, 17.0), NSSize::new(30.0, 40.0));
    unsafe {
        let _: () = objc2::msg_send![&*backdrop, setFrame: displaced];
    }
    assert!(directional.synchronize(&parent, None));
    assert_eq!(
        directional.gradient_inspection(&parent).unwrap().3,
        after_resize.3
    );
    assert!(directional.synchronize(
        &parent,
        Some(native_window::BlurConfig {
            radius: 8,
            end_radius: 0,
            fade: native_window::FadeDirection::None
        })
    ));
    assert_eq!(
        directional.applied_fade(),
        native_window::FadeDirection::None
    );
    assert!(directional.gradient_inspection(&parent).is_none());
    assert_eq!(
        host.subviews(),
        original_siblings,
        "uniform mode removes the gradient's own layer"
    );
    assert!(directional.synchronize(
        &parent,
        Some(native_window::BlurConfig {
            radius: 16,
            end_radius: 0,
            fade: native_window::FadeDirection::Right
        })
    ));
    assert_eq!(directional.gradient_inspection(&parent).unwrap().1, 16.0);
    drop(directional);
    assert_eq!(
        host.subviews(),
        original_siblings,
        "cleanup removes only its own layer"
    );
    assert_eq!(renderer.subviews(), children);
    first.orderOut(None);
    // A decoration can outlive a closed window in egui's cache; it must not
    // retain that window or later apply its radius to a recycled window number.
    let (orphan, weak) = objc2::rc::autoreleasepool(|_| {
        let ephemeral = make_window();
        let ephemeral_parent =
            native_window::NativeWindowHandle::from_test_view(ephemeral.contentView().unwrap());
        let orphan = ephemeral_parent
            .blur_background(native_window::BlurConfig {
                radius: 8,
                end_radius: 0,
                fade: native_window::FadeDirection::None,
            })
            .unwrap();
        (orphan, objc2::rc::Weak::new(&*ephemeral))
    });
    assert!(
        weak.load().is_none(),
        "blur must not retain its native window"
    );
    drop(orphan);
    println!(
        "native window drag guard, transparency and blur: stable content, resize, input, cleanup and window isolation passed"
    );
}
