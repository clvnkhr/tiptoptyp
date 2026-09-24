//! Weak native bindings: querying a viewport never activates a window or keeps
//! a retired native window alive. Bindings are installed by the renderer before
//! invoking root, immediate or deferred UI callbacks.
use egui::{Context, ViewportId};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use winit::window::Window;

#[derive(Clone, Default)]
struct Bindings(Arc<Mutex<Registry>>);
#[derive(Default)]
struct Registry {
    windows: HashMap<ViewportId, (u64, Weak<Window>)>,
    next: u64,
}

/// Return the native window belonging to exactly this viewport, if still alive.
/// Use this on the event-loop thread; do not cache the returned strong reference.
pub fn window(context: &Context, id: ViewportId) -> Option<Arc<Window>> {
    context.data(|data| {
        data.get_temp::<Bindings>(egui::Id::new("eframe-native-windows"))
            .and_then(|bindings| {
                bindings
                    .0
                    .lock()
                    .expect("native window bindings")
                    .windows
                    .get(&id)
                    .and_then(|(_, window)| window.upgrade())
            })
    })
}

pub(crate) fn bind(context: &Context, id: ViewportId, window: &Arc<Window>) {
    context.data_mut(|data| {
        let bindings =
            data.get_temp_mut_or_default::<Bindings>(egui::Id::new("eframe-native-windows"));
        let mut registry = bindings.0.lock().expect("native window bindings");
        if registry
            .windows
            .get(&id)
            .is_some_and(|(_, old)| old.ptr_eq(&Arc::downgrade(window)))
        {
            return;
        }
        registry
            .windows
            .retain(|_, (_, window)| window.strong_count() != 0);
        registry.next = registry
            .next
            .checked_add(1)
            .expect("native identity exhausted");
        let identity = registry.next;
        registry
            .windows
            .insert(id, (identity, Arc::downgrade(window)));
    });
}

/// Monotonic identity of the live native incarnation; unchanged by focus or visibility.
pub fn identity(context: &Context, id: ViewportId) -> Option<u64> {
    context.data(|data| {
        data.get_temp::<Bindings>(egui::Id::new("eframe-native-windows"))
            .and_then(|bindings| {
                bindings
                    .0
                    .lock()
                    .expect("native window bindings")
                    .windows
                    .get(&id)
                    .and_then(|(identity, window)| {
                        (window.strong_count() != 0).then_some(*identity)
                    })
            })
    })
}

/// A close request needs its owning callback even for minimized/occluded
/// windows. Otherwise deferred windows have neither UI nor a logic callback
/// in which to answer the close transaction. This does not force presentation.
pub fn requires_ui(any_visible: bool, close_requested: bool) -> bool {
    any_visible || close_requested
}
