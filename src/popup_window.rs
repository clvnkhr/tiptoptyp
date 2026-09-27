//! A popup keeps keyboard focus while native window-management actions target its owner.
//! Windows keep their original native class and lifecycle; only external management messages are forwarded.
#[cfg(target_os = "macos")]
mod macos {
    use objc2::{
        ffi, msg_send,
        rc::{Retained, Weak},
        runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel},
        sel,
    };
    use objc2_app_kit::{NSView, NSWindow};
    use objc2_foundation::NSString;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{cell::RefCell, collections::HashMap, sync::OnceLock};

    type WindowOwners = HashMap<usize, (Weak<NSWindow>, Weak<NSWindow>)>;
    thread_local! {
        static OWNERS: RefCell<WindowOwners> = RefCell::new(HashMap::new());
    }
    fn base() -> &'static AnyClass {
        AnyClass::get(c"NSWindow").expect("native window class")
    }
    fn owner(window: &AnyObject) -> Option<Retained<NSWindow>> {
        OWNERS.with(|owners| {
            owners
                .borrow()
                .get(&(window as *const _ as usize))
                .and_then(|(_, owner)| owner.load())
        })
    }
    fn managed(attribute: &NSString) -> bool {
        matches!(
            attribute.to_string().as_str(),
            "AXPosition" | "AXSize" | "AXMinimized" | "AXFullScreen"
        )
    }
    unsafe extern "C-unwind" fn value(
        window: &AnyObject,
        _: Sel,
        attribute: &NSString,
    ) -> *mut AnyObject {
        if managed(attribute)
            && let Some(owner) = owner(window)
        {
            // SAFETY: forward the same AppKit accessibility getter to a live owner.
            return unsafe { msg_send![&*owner, accessibilityAttributeValue: attribute] };
        }
        // SAFETY: unhandled attributes use the original NSWindow implementation.
        unsafe { msg_send![super(window, base()), accessibilityAttributeValue: attribute] }
    }
    unsafe extern "C-unwind" fn settable(window: &AnyObject, _: Sel, attribute: &NSString) -> Bool {
        if managed(attribute)
            && let Some(owner) = owner(window)
        {
            // SAFETY: identical accessibility query signature.
            return unsafe { msg_send![&*owner, accessibilityIsAttributeSettable: attribute] };
        }
        unsafe { msg_send![super(window, base()), accessibilityIsAttributeSettable: attribute] }
    }
    unsafe extern "C-unwind" fn set_value(
        window: &AnyObject,
        _: Sel,
        value: *mut AnyObject,
        attribute: &NSString,
    ) {
        if managed(attribute)
            && let Some(owner) = owner(window)
        {
            // SAFETY: AppKit supplies the value required by this same attribute.
            unsafe {
                let _: () =
                    msg_send![&*owner, accessibilitySetValue: value, forAttribute: attribute];
            }
        } else {
            unsafe {
                let _: () = msg_send![super(window, base()), accessibilitySetValue: value, forAttribute: attribute];
            }
        }
    }
    unsafe extern "C-unwind" fn minimize(window: &AnyObject, _: Sel, sender: *mut AnyObject) {
        if let Some(owner) = owner(window) {
            unsafe {
                let _: () = msg_send![&*owner, miniaturize: sender];
            }
        } else {
            unsafe {
                let _: () = msg_send![super(window, base()), miniaturize: sender];
            }
        }
    }
    unsafe extern "C-unwind" fn zoom(window: &AnyObject, _: Sel, sender: *mut AnyObject) {
        if let Some(owner) = owner(window) {
            unsafe {
                let _: () = msg_send![&*owner, zoom: sender];
            }
        } else {
            unsafe {
                let _: () = msg_send![super(window, base()), zoom: sender];
            }
        }
    }
    unsafe extern "C-unwind" fn fullscreen(window: &AnyObject, _: Sel, sender: *mut AnyObject) {
        if let Some(owner) = owner(window) {
            unsafe {
                let _: () = msg_send![&*owner, toggleFullScreen: sender];
            }
        } else {
            unsafe {
                let _: () = msg_send![super(window, base()), toggleFullScreen: sender];
            }
        }
    }
    fn install_bridge() -> &'static AnyClass {
        static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
        CLASS.get_or_init(|| {
            let mut builder = ClassBuilder::new(c"TiptoptypWindowManagementBridge", base())
                .expect("unique window-management bridge");
            // SAFETY: selectors and function encodings match AppKit. No ivars are added.
            unsafe {
                builder.add_method(
                    sel!(accessibilityAttributeValue:),
                    value as unsafe extern "C-unwind" fn(_, _, _) -> _,
                );
                builder.add_method(
                    sel!(accessibilityIsAttributeSettable:),
                    settable as unsafe extern "C-unwind" fn(_, _, _) -> _,
                );
                builder.add_method(
                    sel!(accessibilitySetValue:forAttribute:),
                    set_value as unsafe extern "C-unwind" fn(_, _, _, _),
                );
                builder.add_method(
                    sel!(miniaturize:),
                    minimize as unsafe extern "C-unwind" fn(_, _, _),
                );
                builder.add_method(sel!(zoom:), zoom as unsafe extern "C-unwind" fn(_, _, _));
                builder.add_method(
                    sel!(toggleFullScreen:),
                    fullscreen as unsafe extern "C-unwind" fn(_, _, _),
                );
            }
            let bridge = builder.register();
            let target = AnyClass::get(c"WinitWindow").expect("winit window class");
            for selector in [
                sel!(accessibilityAttributeValue:),
                sel!(accessibilityIsAttributeSettable:),
                sel!(accessibilitySetValue:forAttribute:),
                sel!(miniaturize:),
                sel!(zoom:),
                sel!(toggleFullScreen:),
            ] {
                let method = bridge.instance_method(selector).unwrap();
                // SAFETY: copy exact compiler-generated Objective-C signatures; these
                // selectors are inherited from NSWindow, not implemented by winit.
                assert!(
                    unsafe {
                        ffi::class_addMethod(
                            target as *const _ as *mut _,
                            selector,
                            method.implementation(),
                            ffi::method_getTypeEncoding(method as *const _ as *mut _),
                        )
                    }
                    .as_bool()
                );
            }
            bridge
        })
    }
    fn native(window: &impl HasWindowHandle) -> Option<Retained<NSWindow>> {
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: called only with a live renderer-owned window on the main thread.
        unsafe { handle.ns_view.cast::<NSView>().as_ref().window() }
    }
    pub(super) fn attach(
        context: &eframe::egui::Context,
        child: eframe::egui::ViewportId,
        parent: eframe::egui::ViewportId,
    ) {
        let Some(child) = eframe::window_host::window(context, child).and_then(|w| native(&*w))
        else {
            return;
        };
        let Some(parent) = eframe::window_host::window(context, parent).and_then(|w| native(&*w))
        else {
            return;
        };
        let key = Retained::as_ptr(&child) as usize;
        let fresh = OWNERS.with(|owners| {
            let mut owners = owners.borrow_mut();
            owners.retain(|_, (child, parent)| child.load().is_some() && parent.load().is_some());
            if owners.contains_key(&key) {
                return false;
            }
            owners.insert(key, (Weak::new(&child), Weak::new(&parent)));
            true
        });
        if fresh {
            install_bridge();
        }
    }
}

pub(crate) fn attach(context: &eframe::egui::Context) {
    let child = context.viewport_id();
    let parent = crate::window_host::management_target(context, child);
    if child == parent {
        return;
    }
    #[cfg(target_os = "macos")]
    macos::attach(context, child, parent);
}
