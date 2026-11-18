//! One source-bound formatting transaction, including its deferred admission.
//!
//! Saving after formatting is an intent on the request, never a second flag
//! that can survive its owner or leak into a later math-conversion request.
use super::DocumentKey;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Intent {
    Edit,
    Save,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum State {
    #[default]
    Idle,
    WaitingForSavedDocument(DocumentKey),
    WaitingForMathDocument(DocumentKey),
    Requested {
        key: DocumentKey,
        intent: Intent,
        token: u64,
    },
}

impl State {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn phase(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::WaitingForSavedDocument(_) => "waiting_for_saved_document",
            Self::WaitingForMathDocument(_) => "waiting_for_math_document",
            Self::Requested { .. } => "requested",
        }
    }

    pub(super) fn cancel(&mut self) {
        *self = Self::Idle;
    }

    pub(super) fn request(&mut self, key: DocumentKey, intent: Intent) -> u64 {
        let token = NEXT_REQUEST
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("formatting request identities exhausted");
        *self = Self::Requested { key, intent, token };
        token
    }

    pub(super) fn request_key(self) -> Option<DocumentKey> {
        match self {
            Self::Requested { key, .. } => Some(key),
            _ => None,
        }
    }

    pub(super) fn request_key_for(self, token: u64) -> Option<DocumentKey> {
        match self {
            Self::Requested {
                key,
                token: current,
                ..
            } if current == token => Some(key),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(super) fn request_token(self) -> Option<u64> {
        match self {
            Self::Requested { token, .. } => Some(token),
            _ => None,
        }
    }

    pub(super) fn saved_document(self) -> Option<DocumentKey> {
        match self {
            Self::WaitingForSavedDocument(key) => Some(key),
            _ => None,
        }
    }

    pub(super) fn math_document(self) -> Option<DocumentKey> {
        match self {
            Self::WaitingForMathDocument(key) => Some(key),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(super) fn saves_after_reply(self) -> bool {
        matches!(
            self,
            Self::Requested {
                intent: Intent::Save,
                ..
            }
        )
    }

    pub(super) fn take_ready_saved_document(&mut self, current: DocumentKey, ready: bool) -> bool {
        if ready && self.saved_document() == Some(current) {
            self.cancel();
            true
        } else {
            false
        }
    }

    /// Consume only the admitted request. Stale results cannot retire newer work.
    pub(super) fn complete(&mut self, current: DocumentKey, token: u64) -> Option<Intent> {
        match *self {
            Self::Requested {
                key,
                intent,
                token: admitted,
            } if key == current && token == admitted => {
                self.cancel();
                Some(intent)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(epoch: u64, revision: u64) -> DocumentKey {
        DocumentKey {
            owner: tiptoptyp_core::document::WindowSessionId::new(1),
            epoch,
            revision,
        }
    }

    #[test]
    fn save_intent_cannot_survive_completion_cancellation_or_a_new_math_request() {
        for terminal in 0..3 {
            let mut state = State::Requested {
                key: key(1, 7),
                intent: Intent::Save,
                token: 0,
            };
            match terminal {
                0 => {
                    assert_eq!(state.complete(key(1, 7), 0), Some(Intent::Save));
                }
                1 => state.cancel(),
                _ => state = State::WaitingForMathDocument(key(1, 8)),
            }
            assert!(!state.saves_after_reply());
            let token = state.request(key(1, 8), Intent::Edit);
            assert_eq!(state.complete(key(1, 8), token), Some(Intent::Edit));
            assert_eq!(state.complete(key(1, 8), token), None);
            assert_eq!(state, State::Idle);
        }
    }

    #[test]
    fn deferred_save_requires_the_matching_ready_document_and_hands_off_once() {
        let mut state = State::WaitingForSavedDocument(key(1, 7));
        assert!(!state.take_ready_saved_document(key(1, 7), false));
        assert!(!state.take_ready_saved_document(key(2, 7), true));
        assert_eq!(state.saved_document(), Some(key(1, 7)));
        assert!(state.take_ready_saved_document(key(1, 7), true));
        assert!(!state.take_ready_saved_document(key(1, 7), true));
    }

    #[test]
    fn stale_reply_preserves_every_newer_phase() {
        for next in [
            State::WaitingForSavedDocument(key(2, 7)),
            State::WaitingForMathDocument(key(1, 8)),
            State::Requested {
                key: key(1, 8),
                intent: Intent::Save,
                token: 1,
            },
        ] {
            let mut state = next;
            assert_eq!(state.complete(key(1, 7), 0), None);
            assert_eq!(state, next);
        }
    }

    #[test]
    fn replacing_a_same_document_request_retires_only_its_own_token() {
        let mut state = State::Idle;
        let old = state.request(key(1, 7), Intent::Edit);
        let current = state.request(key(1, 7), Intent::Save);
        assert_ne!(old, current);
        assert_eq!(state.request_key_for(old), None);
        assert_eq!(state.complete(key(1, 7), old), None);
        assert_eq!(state.request_key_for(current), Some(key(1, 7)));
        assert_eq!(state.complete(key(1, 7), current), Some(Intent::Save));
        assert_eq!(state.complete(key(1, 7), current), None);
        let mut other_window = State::Idle;
        assert_ne!(current, other_window.request(key(1, 7), Intent::Edit));
    }
}
