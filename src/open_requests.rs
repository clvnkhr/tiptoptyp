use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryIter},
};

/// File-system paths the operating system asks the running application to open.
///
/// Window drag-and-drop is delivered by egui itself. This channel covers
/// application-level requests such as Finder's Open With and dropping a file
/// onto the packaged app icon.
pub(crate) struct OpenRequestReceiver {
    receiver: Receiver<PathBuf>,
}

impl OpenRequestReceiver {
    pub(crate) fn pending(&self) -> TryIter<'_, PathBuf> {
        self.receiver.try_iter()
    }
}

pub(crate) fn channel() -> (Sender<PathBuf>, OpenRequestReceiver) {
    let (sender, receiver) = mpsc::channel();
    (sender, OpenRequestReceiver { receiver })
}

/// Add Finder document-open support without replacing winit's application
/// delegate. This must run after the winit event loop is built and before it is
/// started.
#[cfg(target_os = "macos")]
pub(crate) fn install_macos_handler(sender: Sender<PathBuf>) -> Result<(), String> {
    macos::install(sender)
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::{Mutex, mpsc::Sender},
    };

    use objc2::{
        ffi,
        runtime::{AnyClass, AnyObject, Imp, Sel},
        sel,
    };
    use objc2_foundation::{NSArray, NSURL};

    use super::PathBuf;

    // The Objective-C callback cannot carry Rust state, so keep just the
    // clonable channel endpoint here. A mutex also lets repeated native test
    // runs replace a disconnected endpoint without leaking application state.
    static OPEN_REQUEST_SENDER: Mutex<Option<Sender<PathBuf>>> = Mutex::new(None);

    pub(super) fn install(sender: Sender<PathBuf>) -> Result<(), String> {
        *OPEN_REQUEST_SENDER
            .lock()
            .map_err(|_| "macOS open-request channel was poisoned".to_owned())? = Some(sender);

        // eframe 0.36 uses winit 0.30.13, whose delegate class is registered
        // while EventLoop::build runs. Adding one selector leaves all of
        // winit's lifecycle callbacks and ivars untouched.
        let class = AnyClass::get(c"WinitApplicationDelegate")
            .ok_or_else(|| "winit's macOS application delegate is unavailable".to_owned())?;
        let selector = sel!(application:openURLs:);
        if class.responds_to(selector) {
            return Ok(());
        }

        let implementation = application_open_urls
            as unsafe extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, &NSArray<NSURL>);
        // SAFETY: Objective-C IMP erases the typed arguments. The function's
        // ABI and the encoding passed below describe the same four arguments.
        let implementation: Imp = unsafe { std::mem::transmute(implementation) };
        // SAFETY: The class is registered for the lifetime of the process;
        // Objective-C permits adding methods to registered classes. The type
        // encoding is `void self selector object object`, matching the typed
        // implementation above and NSApplicationDelegate's documented method.
        let added = unsafe {
            ffi::class_addMethod(
                class as *const AnyClass as *mut AnyClass,
                selector,
                implementation,
                c"v@:@@".as_ptr(),
            )
        };
        added
            .as_bool()
            .then_some(())
            .ok_or_else(|| "could not register macOS document-open handling".to_owned())
    }

    unsafe extern "C-unwind" fn application_open_urls(
        _delegate: &AnyObject,
        _selector: Sel,
        _application: &AnyObject,
        urls: &NSArray<NSURL>,
    ) {
        // No Rust panic may cross an AppKit callback boundary.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let sender = OPEN_REQUEST_SENDER
                .lock()
                .ok()
                .and_then(|sender| sender.clone());
            let Some(sender) = sender else {
                return;
            };
            for url in urls.to_vec() {
                if !url.isFileURL() {
                    continue;
                }
                let Some(path) = url.path() else {
                    continue;
                };
                let _ = sender.send(PathBuf::from(path.to_string()));
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receiver_drains_application_open_requests_in_order() {
        let (sender, receiver) = channel();
        sender.send(PathBuf::from("/project/one.typ")).unwrap();
        sender.send(PathBuf::from("/project/two.pdf")).unwrap();
        assert_eq!(
            receiver.pending().collect::<Vec<_>>(),
            [
                PathBuf::from("/project/one.typ"),
                PathBuf::from("/project/two.pdf")
            ]
        );
        assert!(receiver.pending().next().is_none());
    }
}
