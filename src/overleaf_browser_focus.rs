//! Native input ownership for an embedded browser and its surrounding controls.
//! A page's delayed autofocus must not overrule the user's address-field click.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Owner {
    #[default]
    Browser,
    Parent,
}

#[derive(Clone, Copy, Debug)]
enum Input {
    PointerDown { inside_browser: bool },
    KeyDown,
    Other,
}

#[derive(Default)]
struct Policy {
    owner: Owner,
}
impl Policy {
    fn claim_parent(&mut self) {
        self.owner = Owner::Parent;
    }

    fn browser_owns_keyboard(&self, native_browser_focused: bool) -> bool {
        self.owner == Owner::Browser && native_browser_focused
    }

    /// Returns whether the exact owning parent should receive this event.
    /// Foreign windows and idle frames never change ownership or responders.
    fn input(&mut self, owned_window: bool, key_window: bool, input: Input) -> bool {
        if !owned_window {
            return false;
        }
        match input {
            Input::PointerDown { inside_browser } => {
                self.owner = if inside_browser {
                    Owner::Browser
                } else {
                    Owner::Parent
                };
                self.owner == Owner::Parent
            }
            Input::KeyDown => key_window && self.owner == Owner::Parent,
            Input::Other => false,
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{Input, Policy};
    use block2::RcBlock;
    use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSView};
    use std::{cell::RefCell, ptr::NonNull, rc::Rc};
    use wry::WebViewExtMacOS as _;

    pub(crate) struct Lease {
        monitor: Option<Retained<AnyObject>>,
        policy: Rc<RefCell<Policy>>,
        parent: Retained<NSView>,
    }
    impl Lease {
        pub(crate) fn new(browser: &wry::WebView) -> Result<Self, String> {
            let _main = MainThreadMarker::new()
                .ok_or("Browser input must be configured on the main thread.")?;
            let web = browser.webview();
            // SAFETY: Browser construction just attached this live WebKit view
            // to the retained winit view on AppKit's main thread.
            let parent =
                unsafe { web.superview() }.ok_or("The browser's input owner is unavailable.")?;
            let policy = Rc::new(RefCell::new(Policy::default()));
            let events_policy = Rc::clone(&policy);
            let events_parent = parent.clone();
            let block = RcBlock::new(move |event: NonNull<NSEvent>| {
                // SAFETY: AppKit lends the live event to this synchronous local
                // monitor. Return that exact event without copying or consuming it.
                let event_ref = unsafe { event.as_ref() };
                let Some(window) = web.window() else {
                    return event.as_ptr();
                };
                if !window.isVisible() || web.isHiddenOrHasHiddenAncestor() {
                    return event.as_ptr();
                }
                let owned = event_ref.windowNumber() == window.windowNumber();
                let input = match event_ref.r#type() {
                    NSEventType::LeftMouseDown
                    | NSEventType::RightMouseDown
                    | NSEventType::OtherMouseDown => {
                        let point = web.convertPoint_fromView(event_ref.locationInWindow(), None);
                        let bounds = web.bounds();
                        let inside = point.x >= bounds.origin.x
                            && point.y >= bounds.origin.y
                            && point.x < bounds.origin.x + bounds.size.width
                            && point.y < bounds.origin.y + bounds.size.height;
                        Input::PointerDown {
                            inside_browser: inside,
                        }
                    }
                    NSEventType::KeyDown => Input::KeyDown,
                    _ => Input::Other,
                };
                let parent_event =
                    events_policy
                        .borrow_mut()
                        .input(owned, window.isKeyWindow(), input);
                if parent_event {
                    // WebKit can autofocus an HTML field after the egui click.
                    // Repair only at an explicit input boundary, before AppKit
                    // dispatches this unchanged event to its first responder.
                    window.makeFirstResponder(Some(&events_parent));
                }
                event.as_ptr()
            });
            // SAFETY: the block returns the same live event pointer supplied by
            // AppKit; the token is retained and removed before browser teardown.
            let monitor = unsafe {
                NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                    NSEventMask::LeftMouseDown
                        | NSEventMask::RightMouseDown
                        | NSEventMask::OtherMouseDown
                        | NSEventMask::KeyDown,
                    &block,
                )
            }
            .ok_or("Could not configure browser input ownership.")?;
            Ok(Self {
                monitor: Some(monitor),
                policy,
                parent,
            })
        }

        pub(crate) fn claim_parent(&self) {
            self.policy.borrow_mut().claim_parent();
            if let Some(window) = self.parent.window() {
                window.makeFirstResponder(Some(&self.parent));
            }
        }

        pub(crate) fn browser_owns_keyboard(&self, native_browser_focused: bool) -> bool {
            self.policy
                .borrow()
                .browser_owns_keyboard(native_browser_focused)
        }

        #[cfg(feature = "desktop-ui-tests")]
        pub(crate) fn owner(&self) -> &'static str {
            match self.policy.borrow().owner {
                super::Owner::Parent => "Parent",
                super::Owner::Browser => "Browser",
            }
        }
    }
    impl Drop for Lease {
        fn drop(&mut self) {
            if let Some(monitor) = self.monitor.take() {
                // SAFETY: this is the exact AppKit monitor token installed by
                // this lease, dropped on the owning main thread exactly once.
                unsafe { NSEvent::removeMonitor(&monitor) };
            }
        }
    }
}
#[cfg(target_os = "macos")]
pub(crate) use platform::Lease;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_controls_retain_typing_despite_late_page_autofocus() {
        let mut policy = Policy::default();
        policy.claim_parent();
        assert!(policy.input(true, true, Input::KeyDown));
        assert!(policy.input(true, true, Input::KeyDown));
        assert!(!policy.input(true, true, Input::Other));
        assert!(!policy.input(true, false, Input::KeyDown));
        assert_eq!(policy.owner, Owner::Parent);
    }

    #[test]
    fn real_browser_click_releases_the_lease_and_a_control_click_claims_it() {
        let mut policy = Policy::default();
        policy.claim_parent();
        assert!(!policy.input(
            true,
            true,
            Input::PointerDown {
                inside_browser: true
            }
        ));
        assert_eq!(policy.owner, Owner::Browser);
        assert!(!policy.input(true, true, Input::KeyDown));
        assert!(policy.input(
            true,
            true,
            Input::PointerDown {
                inside_browser: false
            }
        ));
        assert_eq!(policy.owner, Owner::Parent);
        assert!(policy.input(true, true, Input::KeyDown));
    }

    #[test]
    fn native_menu_edit_routing_uses_the_lease_before_the_next_key_event() {
        let mut policy = Policy::default();
        assert!(policy.browser_owns_keyboard(true));
        assert!(!policy.browser_owns_keyboard(false));
        policy.claim_parent();
        // AppKit can invoke Select All before a KeyDown monitor gets a turn.
        // An HTML autofocus responder must not redirect the user's URL command.
        assert!(!policy.browser_owns_keyboard(true));
        assert!(!policy.browser_owns_keyboard(false));
        assert!(!policy.input(
            true,
            true,
            Input::PointerDown {
                inside_browser: true
            }
        ));
        assert!(
            policy.browser_owns_keyboard(true),
            "a real page click retains its normal native editing commands"
        );
    }

    #[test]
    fn other_windows_never_change_or_capture_either_owners_input() {
        let mut policy = Policy::default();
        for owner in [Owner::Browser, Owner::Parent] {
            policy.owner = owner;
            for input in [
                Input::KeyDown,
                Input::Other,
                Input::PointerDown {
                    inside_browser: false,
                },
                Input::PointerDown {
                    inside_browser: true,
                },
            ] {
                assert!(!policy.input(false, true, input));
                assert_eq!(policy.owner, owner);
            }
        }
    }
}
