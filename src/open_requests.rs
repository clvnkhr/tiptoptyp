use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SendError, Sender, TryRecvError},
    },
};

#[derive(Clone)]
pub(crate) struct OpenRequestSender {
    sender: Sender<PathBuf>,
    repaint: Arc<Mutex<Option<eframe::egui::Context>>>,
}

impl OpenRequestSender {
    pub(crate) fn send(&self, path: PathBuf) -> Result<(), SendError<PathBuf>> {
        self.sender.send(path)?;
        let context = self.repaint.lock().ok().and_then(|context| context.clone());
        if let Some(context) = context {
            // The root is the process dispatcher, including when all document
            // windows are closed. Do not rely on mouse input to wake it.
            context.request_repaint_of(eframe::egui::ViewportId::ROOT);
        }
        Ok(())
    }
}

/// File-system paths the operating system asks the running application to open.
///
/// Window drag-and-drop is delivered by egui itself. This channel covers
/// application-level requests such as Finder's Open With and dropping a file
/// onto the packaged app icon.
pub(crate) struct OpenRequestReceiver {
    receiver: Receiver<PathBuf>,
    repaint: Arc<Mutex<Option<eframe::egui::Context>>>,
}

impl OpenRequestReceiver {
    pub(crate) fn set_repaint_context(&self, context: eframe::egui::Context) {
        *self
            .repaint
            .lock()
            .expect("open-request repaint context poisoned") = Some(context);
    }

    pub(crate) fn try_recv(&self) -> Result<PathBuf, TryRecvError> {
        self.receiver.try_recv()
    }
}

pub(crate) fn channel() -> (OpenRequestSender, OpenRequestReceiver) {
    let (sender, receiver) = mpsc::channel();
    let repaint = Arc::new(Mutex::new(None));
    (
        OpenRequestSender {
            sender,
            repaint: repaint.clone(),
        },
        OpenRequestReceiver { receiver, repaint },
    )
}

/// Add Finder document-open support without replacing winit's application
/// delegate. This must run after the winit event loop is built and before it is
/// started.
#[cfg(target_os = "macos")]
pub(crate) fn install_macos_handler(sender: OpenRequestSender) -> Result<(), String> {
    macos::install(sender)
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::Mutex,
    };

    use objc2::{
        ffi,
        runtime::{AnyClass, AnyObject, Imp, Sel},
        sel,
    };
    use objc2_foundation::{NSArray, NSURL};

    use super::{OpenRequestSender, PathBuf};

    // The Objective-C callback cannot carry Rust state, so keep just the
    // clonable channel endpoint here. A mutex also lets repeated native test
    // runs replace a disconnected endpoint without leaking application state.
    static OPEN_REQUEST_SENDER: Mutex<Option<OpenRequestSender>> = Mutex::new(None);

    pub(super) fn install(sender: OpenRequestSender) -> Result<(), String> {
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
    fn external_open_wakes_only_the_process_host_and_failed_sends_do_not_repaint() {
        let (sender, receiver) = channel();
        sender.send(PathBuf::from("/startup.typ")).unwrap();
        let context = eframe::egui::Context::default();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        context.set_request_repaint_callback(move |request| {
            observed.lock().unwrap().push(request.viewport_id);
        });
        receiver.set_repaint_context(context);
        sender.send(PathBuf::from("/later.typ")).unwrap();
        assert_eq!(*requests.lock().unwrap(), [eframe::egui::ViewportId::ROOT]);
        assert_eq!(receiver.try_recv().unwrap(), PathBuf::from("/startup.typ"));
        assert_eq!(receiver.try_recv().unwrap(), PathBuf::from("/later.typ"));
        requests.lock().unwrap().clear();
        drop(receiver);
        assert!(sender.send(PathBuf::from("/closed.typ")).is_err());
        assert!(requests.lock().unwrap().is_empty());
    }

    #[test]
    fn receiver_drains_application_open_requests_in_order() {
        let (sender, receiver) = channel();
        sender.send(PathBuf::from("/project/one.typ")).unwrap();
        sender.send(PathBuf::from("/project/two.pdf")).unwrap();
        assert_eq!(
            receiver.try_recv().unwrap(),
            PathBuf::from("/project/one.typ")
        );
        assert_eq!(
            receiver.try_recv().unwrap(),
            PathBuf::from("/project/two.pdf")
        );
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }
}
