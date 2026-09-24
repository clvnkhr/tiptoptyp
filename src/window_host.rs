//! Shared native-window lifetime, close delivery and focus admission.
//!
//! Visibility and OS focus are observations, not ownership. Tokens become
//! invalid on retirement/recreation; painting never revives a retired token.
use eframe::egui;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

type Id = egui::ViewportId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifecycle {
    Visible,
    TemporarilyHidden,
    DurablyClosed,
    DormantHosted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) id: Id,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CloseRequest {
    token: Token,
    sequence: u64,
}

#[derive(Debug, Clone)]
struct Entry {
    owner: Id,
    generation: u64,
    state: Lifecycle,
    dormant_host: bool,
    pending_close: Option<CloseRequest>,
    native_identity: Option<u64>,
    pending_focus: Option<(FocusRequest, bool)>,
}

#[derive(Default)]
pub(crate) struct Registry {
    entries: HashMap<Id, Entry>,
    dormant_owners: HashSet<Id>,
    next_close: u64,
    next_generation: u64,
    focus_history: Vec<Id>,
}

impl Registry {
    pub(crate) fn show(
        &mut self,
        id: Id,
        owner: Id,
        visible: bool,
        dormant_host: bool,
    ) -> Option<Token> {
        let state = if self.dormant_owners.contains(&owner) {
            if dormant_host {
                Lifecycle::DormantHosted
            } else {
                Lifecycle::DurablyClosed
            }
        } else if visible {
            Lifecycle::Visible
        } else {
            Lifecycle::TemporarilyHidden
        };
        if !self.entries.contains_key(&id) {
            self.next_generation = self
                .next_generation
                .checked_add(1)
                .expect("window generation exhausted");
            self.entries.insert(
                id,
                Entry {
                    owner,
                    generation: self.next_generation,
                    state,
                    dormant_host,
                    pending_close: None,
                    native_identity: None,
                    pending_focus: None,
                },
            );
        }
        let entry = self.entries.get_mut(&id).expect("registered window");
        assert_eq!(entry.owner, owner, "a window cannot change owner");
        entry.state = state;
        entry.dormant_host = dormant_host;
        (state != Lifecycle::DurablyClosed).then_some(Token {
            id,
            generation: entry.generation,
        })
    }

    pub(crate) fn bind_native(&mut self, id: Id, identity: u64) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        if entry
            .native_identity
            .is_some_and(|previous| previous != identity)
        {
            self.next_generation = self
                .next_generation
                .checked_add(1)
                .expect("window generation exhausted");
            entry.generation = self.next_generation;
            entry.pending_close = None;
            entry.pending_focus = None;
        }
        entry.native_identity = Some(identity);
    }

    pub(crate) fn current(&self, id: Id) -> Option<Token> {
        let entry = self.entries.get(&id)?;
        (entry.state != Lifecycle::DurablyClosed).then_some(Token {
            id,
            generation: entry.generation,
        })
    }

    pub(crate) fn live(&self, token: Token) -> bool {
        self.entries.get(&token.id).is_some_and(|entry| {
            entry.generation == token.generation
                && (matches!(entry.state, Lifecycle::Visible | Lifecycle::DormantHosted)
                    || (entry.state == Lifecycle::TemporarilyHidden && entry.dormant_host))
        })
    }

    pub(crate) fn transition(&mut self, id: Id, state: Lifecycle) -> bool {
        let Some(entry) = self.entries.get_mut(&id) else {
            return false;
        };
        if entry.state == state {
            return false;
        }
        // Only show() can explicitly reopen a retired window.
        if entry.state == Lifecycle::DurablyClosed {
            return false;
        }
        entry.state = state;
        if state == Lifecycle::DurablyClosed {
            self.next_generation = self
                .next_generation
                .checked_add(1)
                .expect("window generation exhausted");
            entry.generation = self.next_generation;
            entry.pending_close = None;
            entry.pending_focus = None;
        }
        true
    }

    pub(crate) fn retire_owner(&mut self, owner: Id) -> Vec<Id> {
        if !self.dormant_owners.insert(owner) {
            return Vec::new();
        }
        let children: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(id, e)| {
                (e.owner == owner && *id != owner).then_some((*id, e.dormant_host))
            })
            .collect();
        let mut closed = Vec::new();
        for (id, hosted) in children {
            if self.transition(
                id,
                if hosted {
                    Lifecycle::DormantHosted
                } else {
                    Lifecycle::DurablyClosed
                },
            ) && !hosted
            {
                closed.push(id);
            }
        }
        closed
    }

    fn record_focus(&mut self, id: Id) {
        if self.current(id).is_some() && self.focus_history.last() != Some(&id) {
            self.focus_history.retain(|previous| *previous != id);
            self.focus_history.push(id);
        }
    }

    fn focus_return_target(
        &mut self,
        closed: Id,
        mut eligible: impl FnMut(Id) -> bool,
    ) -> Option<Id> {
        self.focus_history.retain(|id| *id != closed);
        self.focus_history.iter().rev().copied().find(|id| {
            self.entries
                .get(id)
                .is_some_and(|entry| entry.state == Lifecycle::Visible)
                && eligible(*id)
        })
    }

    pub(crate) fn collect_retired(&mut self, mut active: impl FnMut(Id) -> bool) {
        self.entries.retain(|id, entry| {
            active(*id)
                || (entry.state != Lifecycle::DurablyClosed && entry.native_identity.is_none())
        });
        self.dormant_owners.retain(|owner| active(*owner));
        self.focus_history
            .retain(|id| self.entries.contains_key(id));
    }

    pub(crate) fn resume_owner(&mut self, owner: Id) {
        self.dormant_owners.remove(&owner);
    }

    pub(crate) fn request_close(&mut self, token: Token) -> Option<CloseRequest> {
        if self.current(token.id) != Some(token) {
            return None;
        }
        let entry = self.entries.get_mut(&token.id)?;
        if let Some(request) = entry.pending_close {
            return Some(request);
        }
        self.next_close = self
            .next_close
            .checked_add(1)
            .expect("close sequence exhausted");
        let request = CloseRequest {
            token,
            sequence: self.next_close,
        };
        entry.pending_close = Some(request);
        Some(request)
    }

    pub(crate) fn pending_close(&self, id: Id) -> Option<CloseRequest> {
        self.entries.get(&id)?.pending_close
    }

    pub(crate) fn acknowledge_close(&mut self, request: CloseRequest) -> bool {
        let Some(entry) = self.entries.get_mut(&request.token.id) else {
            return false;
        };
        if entry.pending_close != Some(request) {
            return false;
        }
        entry.pending_close = None;
        true
    }
}

fn with_registry<R>(context: &egui::Context, f: impl FnOnce(&mut Registry) -> R) -> R {
    let shared = context.data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<Registry>>>(egui::Id::new("window-host-registry"))
            .clone()
    });
    f(&mut shared.lock().expect("window host registry"))
}

pub(crate) fn show(
    context: &egui::Context,
    id: Id,
    owner: Id,
    visible: bool,
    dormant: bool,
) -> Option<Token> {
    with_registry(context, |r| {
        let token = r.show(id, owner, visible, dormant)?;
        if let Some(identity) = eframe::window_host::identity(context, id) {
            r.bind_native(id, identity);
        }
        r.current(token.id)
    })
}
pub(crate) fn live(context: &egui::Context, token: Token) -> bool {
    let closing = context
        .input(|input| input.raw.viewport_id == token.id && input.viewport().close_requested());
    with_registry(context, |r| {
        if let Some(identity) = eframe::window_host::identity(context, token.id) {
            r.bind_native(token.id, identity);
        } else if r
            .entries
            .get(&token.id)
            .is_some_and(|entry| entry.native_identity.is_some())
        {
            return false;
        }
        r.live(token) || (closing && r.current(token.id) == Some(token))
    })
}
/// Ensure a host exists without reopening an explicitly retired window.
pub(crate) fn ensure(context: &egui::Context, id: Id, owner: Id) -> Option<Token> {
    with_registry(context, |r| {
        if !r.entries.contains_key(&id) {
            r.show(id, owner, true, false);
        }
        if let Some(identity) = eframe::window_host::identity(context, id) {
            r.bind_native(id, identity);
        }
        r.current(id)
    })
}
pub(crate) fn transition(context: &egui::Context, id: Id, state: Lifecycle) -> bool {
    with_registry(context, |r| r.transition(id, state))
}
pub(crate) fn retire_owner(context: &egui::Context, owner: Id) -> Vec<Id> {
    with_registry(context, |r| r.retire_owner(owner))
}
pub(crate) fn resume_owner(context: &egui::Context, owner: Id) {
    with_registry(context, |r| r.resume_owner(owner));
}

/// Called at input admission, before any visible/hidden rendering decision.
pub(crate) fn observe_close(context: &egui::Context, id: Id, owner: Id) -> bool {
    with_registry(context, |registry| {
        if !registry.entries.contains_key(&id) {
            registry.show(id, owner, true, false);
        }
        let was_pending = registry.pending_close(id).is_some();
        registry
            .current(id)
            .and_then(|token| registry.request_close(token))
            .is_some()
            && !was_pending
    })
}
pub(crate) fn close_requested(context: &egui::Context, id: Id) -> bool {
    with_registry(context, |r| r.pending_close(id).is_some())
}
/// Transfer the request to the owning document's save/discard transaction.
pub(crate) fn acknowledge_close(context: &egui::Context, id: Id) {
    with_registry(context, |r| {
        if let Some(request) = r.pending_close(id) {
            r.acknowledge_close(request);
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusCause {
    UserAction,
    ReturnFromChild,
}

pub(crate) fn focus_allowed(
    cause: FocusCause,
    live: bool,
    active: bool,
    visible: bool,
    minimized: bool,
) -> bool {
    live && visible && !minimized && (cause == FocusCause::UserAction || active)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FocusRequest {
    token: Token,
    cause: FocusCause,
}

pub(crate) fn focus(context: &egui::Context, id: Id, cause: FocusCause) {
    let Some(token) = ensure(context, id, context.viewport_id()) else {
        return;
    };
    apply_focus(context, FocusRequest { token, cause });
}

fn apply_focus(context: &egui::Context, request: FocusRequest) {
    let FocusRequest { token, cause } = request;
    let id = token.id;

    let (exists, active, visible, minimized) = context.input(|input| {
        let info = input.raw.viewports.get(&id);
        (
            info.is_some(),
            input
                .raw
                .viewports
                .values()
                .any(|v| v.focused == Some(true) && v.visible() != Some(false)),
            info.is_some_and(|v| v.visible() != Some(false)),
            info.is_some_and(|v| v.minimized == Some(true)),
        )
    });
    let active = crate::native_window::application_active(context).unwrap_or(active);
    let (visible, minimized) = eframe::window_host::window(context, id)
        .map(|window| {
            (
                window.is_visible().unwrap_or(visible),
                window.is_minimized().unwrap_or(minimized),
            )
        })
        .unwrap_or((visible, minimized));
    let eligible = with_registry(context, |r| {
        if let Some(identity) = eframe::window_host::identity(context, id) {
            r.bind_native(id, identity);
        } else if r
            .entries
            .get(&id)
            .is_some_and(|entry| entry.native_identity.is_some())
        {
            return false;
        }
        r.current(id) == Some(token)
    });
    if cause == FocusCause::UserAction && exists && eligible && (!visible || minimized) {
        let queued = with_registry(context, |r| {
            let entry = r.entries.get_mut(&id).expect("focus owner exists");
            if entry.pending_focus.is_some() {
                return false;
            }
            entry.pending_focus = Some((request, active));
            true
        });
        if queued {
            context.request_repaint_of(id);
        }
        return;
    }
    if focus_allowed(cause, exists && eligible, active, visible, minimized) {
        with_registry(context, |r| {
            if let Some(entry) = r.entries.get_mut(&id) {
                entry.pending_focus = None;
            }
        });
        context.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
    }
}

/// Native restore is asynchronous. Dispatch a queued user focus only after
/// the window is actually visible and restored, on subsequent native input.
/// There is no timer or idle repaint loop. Switching away cancels the intent.
pub(crate) fn flush_focus(context: &egui::Context) {
    let pending = with_registry(context, |r| {
        r.entries
            .values()
            .filter_map(|entry| entry.pending_focus)
            .collect::<Vec<_>>()
    });
    for (request, was_active) in pending {
        if was_active && crate::native_window::application_active(context) == Some(false) {
            with_registry(context, |r| {
                if let Some(entry) = r.entries.get_mut(&request.token.id) {
                    entry.pending_focus = None;
                }
            });
        } else {
            apply_focus(context, request);
        }
    }
}

/// Record actual activation, not a requested focus or a child's ownership.
pub(crate) fn observe_focus(context: &egui::Context, raw: &egui::RawInput) {
    // Focus can change before the newly focused child has its own repaint.
    // Observe all live native bindings at input admission, not just the viewport
    // whose event happened to wake the loop.
    let mut native = false;
    let focused = raw
        .viewports
        .keys()
        .copied()
        .find(|id| {
            eframe::window_host::window(context, *id).is_some_and(|window| {
                native = true;
                // Transient cards can become key during OS handoffs; they are
                // not durable return destinations for closing a tool window.
                window.has_focus() && window.is_decorated()
            })
        })
        .or_else(|| (!native && raw.viewport().focused == Some(true)).then_some(raw.viewport_id));
    if let Some(id) = focused {
        with_registry(context, |registry| registry.record_focus(id));
    }
}

/// Explicit tool-window close returns to the most recently used surviving window.
pub(crate) fn return_from_closed_window(context: &egui::Context, closed: Id) {
    let target = with_registry(context, |registry| {
        registry.focus_return_target(closed, |id| {
            if let Some(window) = eframe::window_host::window(context, id) {
                window.is_visible() != Some(false) && window.is_minimized() != Some(true)
            } else {
                context.input(|input| {
                    input.raw.viewports.get(&id).is_some_and(|info| {
                        info.visible() != Some(false) && info.minimized != Some(true)
                    })
                })
            }
        })
    });
    if let Some(target) = target {
        focus(context, target, FocusCause::ReturnFromChild);
    }
}

pub(crate) fn return_focus(context: &egui::Context) {
    focus(context, context.viewport_id(), FocusCause::ReturnFromChild);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(n: u64) -> Id {
        Id::from_hash_of(n)
    }

    #[test]
    fn focus_history_returns_to_recent_surviving_windows_without_duplicates() {
        let mut r = Registry::default();
        for n in 1..=4 {
            r.show(id(n), id(n), true, false);
        }
        for n in [1, 2, 1, 3, 2, 4, 4] {
            r.record_focus(id(n));
        }
        assert_eq!(r.focus_history, vec![id(1), id(3), id(2), id(4)]);
        assert_eq!(r.focus_return_target(id(4), |_| true), Some(id(2)));
        r.transition(id(2), Lifecycle::DurablyClosed);
        assert_eq!(r.focus_return_target(id(4), |_| true), Some(id(3)));
        assert_eq!(
            r.focus_return_target(id(4), |candidate| candidate != id(3)),
            Some(id(1))
        );
        r.transition(id(1), Lifecycle::TemporarilyHidden);
        assert_eq!(r.focus_return_target(id(4), |_| false), None);
        r.collect_retired(|_| false);
        assert!(r.focus_history.iter().all(|id| r.entries.contains_key(id)));
    }

    #[test]
    fn settings_close_returns_to_recent_document_and_never_steals_background_focus() {
        for (background, minimized, expected) in [
            (false, false, Some(id(2))),
            (false, true, Some(Id::ROOT)),
            (true, false, None),
        ] {
            let context = egui::Context::default();
            let mut raw = egui::RawInput::default();
            for target in [Id::ROOT, id(2), id(3)] {
                show(&context, target, target, true, false);
                raw.viewports.entry(target).or_default();
                with_registry(&context, |registry| registry.record_focus(target));
            }
            raw.viewports.get_mut(&id(2)).unwrap().minimized = Some(minimized);
            for info in raw.viewports.values_mut() {
                info.focused = Some(false);
            }
            raw.viewports.get_mut(&id(3)).unwrap().focused = Some(!background);
            let output =
                context.run_logic(&raw, |context| return_from_closed_window(context, id(3)));
            let focused: Vec<_> = output
                .viewport_commands
                .iter()
                .filter_map(|(id, commands)| {
                    commands
                        .contains(&egui::ViewportCommand::Focus)
                        .then_some(*id)
                })
                .collect();
            assert_eq!(focused, expected.into_iter().collect::<Vec<_>>());
        }
    }

    #[test]
    fn close_delivery_survives_hidden_passes_and_acknowledges_exactly_once() {
        let mut registry = Registry::default();
        let first = registry.show(id(1), id(1), true, false).unwrap();
        let request = registry.request_close(first).unwrap();
        for _ in 0..100 {
            registry.transition(id(1), Lifecycle::TemporarilyHidden);
            assert_eq!(registry.request_close(first), Some(request));
            assert_eq!(registry.pending_close(id(1)), Some(request));
        }
        assert!(registry.acknowledge_close(request));
        assert!(!registry.acknowledge_close(request));
        let next = registry.request_close(first).unwrap();
        assert_ne!(next, request);
        assert!(!registry.acknowledge_close(request));
        assert_eq!(registry.pending_close(id(1)), Some(next));
    }

    #[test]
    fn native_recreation_invalidates_callbacks_and_pending_results_once() {
        let mut r = Registry::default();
        let before = r.show(id(1), id(1), true, false).unwrap();
        r.bind_native(id(1), 10);
        assert!(
            r.live(before),
            "initial attachment preserves the creation callback"
        );
        let close = r.request_close(before).unwrap();
        r.bind_native(id(1), 11);
        assert!(!r.live(before));
        assert!(!r.acknowledge_close(close));
        let after = r.current(id(1)).unwrap();
        assert_eq!(after.generation, before.generation + 1);
        for _ in 0..100 {
            r.bind_native(id(1), 11);
        }
        assert_eq!(r.current(id(1)), Some(after));
        assert!(r.live(after));
    }

    #[test]
    fn retired_owner_cannot_close_siblings_or_revive_stale_children() {
        let mut r = Registry::default();
        let child = r.show(id(2), id(1), true, false).unwrap();
        let settings = r.show(id(3), id(1), true, true).unwrap();
        let sibling = r.show(id(4), id(4), true, false).unwrap();
        assert_eq!(r.retire_owner(id(1)), vec![id(2)]);
        assert!(r.retire_owner(id(1)).is_empty());
        assert!(!r.live(child));
        assert!(r.live(settings));
        assert!(r.live(sibling));
        assert!(r.show(id(2), id(1), true, false).is_none());
        r.resume_owner(id(1));
        let new = r.show(id(2), id(1), true, false).unwrap();
        assert_ne!(new, child);
        assert!(!r.live(child));
        assert!(r.live(new));
    }

    #[test]
    fn focus_admission_exhausts_every_input_combination() {
        for cause in [FocusCause::UserAction, FocusCause::ReturnFromChild] {
            for bits in 0..16 {
                let live = bits & 1 != 0;
                let active = bits & 2 != 0;
                let visible = bits & 4 != 0;
                let minimized = bits & 8 != 0;
                let allowed = focus_allowed(cause, live, active, visible, minimized);
                if !live {
                    assert!(!allowed);
                }
                if cause == FocusCause::ReturnFromChild {
                    assert_eq!(allowed, live && active && visible && !minimized);
                } else {
                    assert_eq!(allowed, live && visible && !minimized);
                }
            }
        }
    }

    #[test]
    fn background_popup_dismissal_and_minimized_owner_emit_no_focus() {
        for (focused, minimized, expect_focus) in [
            (false, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            let ctx = egui::Context::default();
            let mut raw = egui::RawInput::default();
            let info = raw.viewports.get_mut(&Id::ROOT).unwrap();
            info.focused = Some(focused);
            info.minimized = Some(minimized);
            let mut output = ctx.run_ui(raw, |ui| return_focus(ui.ctx()));
            let focused = output.viewport_output[&Id::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Focus);
            output.textures_delta.clear();
            assert_eq!(focused, expect_focus);
        }
    }

    #[test]
    fn pruning_closed_windows_bounds_storage_without_reusing_generations() {
        let mut registry = Registry::default();
        let original = registry.show(id(1), id(1), true, false).unwrap();
        for _ in 0..1000 {
            registry.transition(id(1), Lifecycle::DurablyClosed);
            registry.collect_retired(|_| false);
            assert!(registry.entries.is_empty());
            let current = registry.show(id(1), id(1), true, false).unwrap();
            assert_ne!(current, original);
            assert!(!registry.live(original));
        }
    }

    #[test]
    fn queued_focus_waits_for_restore_and_is_consumed_once() {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.viewports.get_mut(&Id::ROOT).unwrap().minimized = Some(true);
        let output = ctx.run_logic(&raw, |ctx| focus(ctx, Id::ROOT, FocusCause::UserAction));
        assert!(
            !output
                .viewport_commands
                .get(&Id::ROOT)
                .is_some_and(|cmd| cmd.contains(&egui::ViewportCommand::Focus))
        );
        raw.viewports.get_mut(&Id::ROOT).unwrap().minimized = Some(false);
        let output = ctx.run_logic(&raw, flush_focus);
        assert_eq!(
            output.viewport_commands[&Id::ROOT]
                .iter()
                .filter(|cmd| **cmd == egui::ViewportCommand::Focus)
                .count(),
            1
        );
        let output = ctx.run_logic(&raw, flush_focus);
        assert!(
            !output
                .viewport_commands
                .get(&Id::ROOT)
                .is_some_and(|cmd| cmd.contains(&egui::ViewportCommand::Focus))
        );
    }

    #[test]
    fn stale_focus_cannot_target_a_reopened_window() {
        let ctx = egui::Context::default();
        let token = show(&ctx, Id::ROOT, Id::ROOT, true, false).unwrap();
        transition(&ctx, Id::ROOT, Lifecycle::DurablyClosed);
        show(&ctx, Id::ROOT, Id::ROOT, true, false);
        let output = ctx.run_logic(&egui::RawInput::default(), |ctx| {
            apply_focus(
                ctx,
                FocusRequest {
                    token,
                    cause: FocusCause::UserAction,
                },
            )
        });
        assert!(
            !output
                .viewport_commands
                .get(&Id::ROOT)
                .is_some_and(|cmd| cmd.contains(&egui::ViewportCommand::Focus))
        );
    }

    #[test]
    fn exhaustive_five_event_sequences_preserve_ownership_and_retirement() {
        // Every sequence over ten operations: 100,000 leaves. This is a bounded
        // state-machine proof, not a claim about all OS event interleavings.
        fn visit(r: Registry, retired: Vec<Token>, depth: usize, leaves: &mut usize) {
            let sibling = r.current(id(9)).unwrap();
            for token in &retired {
                assert!(!r.live(*token));
            }
            assert!(r.live(sibling));
            if depth == 0 {
                *leaves += 1;
                return;
            }
            for event in 0..10 {
                let mut next = Registry {
                    entries: r.entries.clone(),
                    dormant_owners: r.dormant_owners.clone(),
                    next_close: r.next_close,
                    next_generation: r.next_generation,
                    focus_history: r.focus_history.clone(),
                };
                let mut retired = retired.clone();
                let token = next.current(id(2));
                match event {
                    0 => {
                        next.show(id(2), id(1), true, false);
                    }
                    1 => {
                        next.transition(id(2), Lifecycle::TemporarilyHidden);
                    }
                    2 => {
                        if next.transition(id(2), Lifecycle::DurablyClosed) {
                            retired.extend(token);
                        }
                    }
                    3 => {
                        if let Some(token) = token {
                            next.request_close(token);
                        }
                    }
                    4 => {
                        if let Some(request) = next.pending_close(id(2)) {
                            assert!(next.acknowledge_close(request));
                            assert!(!next.acknowledge_close(request));
                        }
                    }
                    5 => {
                        if next.retire_owner(id(1)).contains(&id(2)) {
                            retired.extend(token);
                        }
                    }
                    6 => next.resume_owner(id(1)),
                    7 => {
                        let old = next.entries[&id(2)].native_identity.unwrap_or(0);
                        next.bind_native(id(2), old + 1);
                        if old != 0 {
                            retired.extend(token);
                        }
                    }
                    8 => {
                        for old in &retired {
                            assert!(next.request_close(*old).is_none());
                        }
                    }
                    9 => {
                        if let Some(token) = token {
                            let first = next.request_close(token);
                            assert_eq!(next.request_close(token), first);
                        }
                    }
                    _ => unreachable!(),
                }
                assert_eq!(next.current(id(9)), Some(sibling));
                assert!(next.pending_close(id(9)).is_none());
                visit(next, retired, depth - 1, leaves);
            }
        }
        let mut registry = Registry::default();
        registry.show(id(2), id(1), true, false);
        registry.show(id(9), id(9), true, false);
        let mut leaves = 0;
        visit(registry, vec![], 5, &mut leaves);
        assert_eq!(leaves, 100_000);
    }
}

#[cfg(test)]
mod adapter_tests {
    #[test]
    fn native_close_dispatch_exhausts_visibility_and_close_combinations() {
        for visible in [false, true] {
            for close in [false, true] {
                assert_eq!(
                    eframe::window_host::requires_ui(visible, close),
                    visible || close
                );
            }
        }
        assert!(
            eframe::window_host::requires_ui(false, true),
            "minimized deferred windows must answer close"
        );
        assert!(
            !eframe::window_host::requires_ui(false, false),
            "hidden idle windows must not paint"
        );
    }
}

/// Prune retired tombstones after the native renderer has removed the viewport.
/// Generations are registry-wide, so reusing an id after pruning stays safe.
pub(crate) fn collect_retired(context: &egui::Context, input: &egui::RawInput) {
    with_registry(context, |r| {
        r.collect_retired(|id| input.viewports.contains_key(&id))
    });
}
