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
    use objc2_app_kit::{NSApplication, NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let marker = MainThreadMarker::new().expect("AppKit main thread");
    let _app = NSApplication::sharedApplication(marker);
    let make_window = || {
        // SAFETY: AppKit main thread; windows are retained and never ordered
        // onscreen, so this property check does not steal keyboard focus.
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
    let parent = native_window::ActiveWindowHandle::from_test_view(first.contentView().unwrap());
    let other = native_window::ActiveWindowHandle::from_test_view(second.contentView().unwrap());
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
    println!("native window drag guard: suppression, restoration and sibling isolation passed");
}
