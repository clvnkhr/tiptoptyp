//! Read-only snapshots: observing Activity never advances synchronization.
use super::*;
use crate::activity::Activity;

#[derive(Default)]
pub(super) enum BrowserStatus {
    #[default]
    Dormant,
    Connecting,
    Connected,
    SignedOut,
    Disconnected,
    Failed(String),
    Unavailable,
}
impl BrowserStatus {
    pub(super) fn from_event(state: &str, message: Option<&str>) -> Self {
        match state {
            "loading" => Self::Connecting,
            "connected" | "saving" => Self::Connected,
            "signed-out" => Self::SignedOut,
            "disconnected" => Self::Disconnected,
            "error" => Self::Failed(message.unwrap_or("Overleaf connection failed.").into()),
            _ => Self::Unavailable,
        }
    }
}

impl State {
    fn connection_activity(&self) -> Activity {
        let Some(connection) = &self.connection else {
            return Activity::Inactive("Not connected");
        };
        match &self.browser_status {
            BrowserStatus::Connecting => Activity::Running,
            BrowserStatus::SignedOut => Activity::Inactive("Sign in required"),
            BrowserStatus::Disconnected => Activity::Inactive("Disconnected"),
            BrowserStatus::Failed(error) => Activity::Failed(error.clone()),
            BrowserStatus::Unavailable => Activity::Inactive("Connection unavailable"),
            BrowserStatus::Dormant | BrowserStatus::Connected if connection.online => {
                Activity::Idle
            }
            BrowserStatus::Dormant | BrowserStatus::Connected => Activity::Inactive("Offline"),
        }
    }
}

impl EditorApp {
    pub(in crate::app) fn overleaf_connection_activity(&self) -> Activity {
        let state = &self.overleaf;
        let storage = state.storage_job.activity();
        if matches!(storage, Activity::Running | Activity::Failed(_)) {
            return storage;
        }
        if let Some(error) = &state.error {
            return Activity::Failed(error.clone());
        }
        state.connection_activity()
    }

    pub(in crate::app) fn overleaf_sync_activity(&self) -> Activity {
        let state = &self.overleaf;
        let storage = state.storage_job.activity();
        let mirror = state.io.activity();
        if storage == Activity::Running || mirror == Activity::Running {
            return Activity::Running;
        }
        for status in [storage, mirror] {
            if matches!(status, Activity::Failed(_)) {
                return status;
            }
        }
        if let Some(error) = &state.error {
            return Activity::Failed(error.clone());
        }
        let availability = state.connection_activity();
        if availability != Activity::Idle {
            return availability;
        }
        if state
            .connection
            .as_ref()
            .is_some_and(|c| c.hydration.pending)
        {
            Activity::Pending("Syncing working copy")
        } else if state.tree_pending.is_some()
            || !state.queued.is_empty()
            || state.downloads.has_pending()
        {
            Activity::Pending("Working copy queued")
        } else {
            Activity::Idle
        }
    }

    /// A native edit is dirty before the debounce publishes its model draft.
    fn overleaf_has_unsaved_changes(&self) -> bool {
        self.overleaf.connection.as_ref().is_some_and(|connection| {
            connection.model.has_unsynced()
                || self.tabs.ids().any(|tab| {
                    self.document_for_tab(tab).is_some_and(|document| {
                        document.is_dirty()
                            && document.path().as_ref().is_some_and(|path| {
                                self.overleaf_entry(path)
                                    .is_some_and(|entry| entry.kind == EntryKind::Doc)
                            })
                    })
                })
        })
    }

    pub(in crate::app) fn overleaf_save_activity(&self) -> Activity {
        let state = &self.overleaf;
        if !state.blocked.is_empty() {
            return Activity::Failed(state.error.as_deref().unwrap_or(CONFLICT_ERROR).into());
        }
        if let Some(pending) = &state.pending {
            if let Some(error) = &state.error {
                return Activity::Failed(error.clone());
            }
            if pending.started.elapsed() >= SAVE_TIMEOUT {
                return Activity::Failed("Overleaf has not confirmed this save. Your draft is retained; reconnect before retrying.".into());
            }
            if let Activity::Failed(error) = state.connection_activity() {
                return Activity::Failed(error);
            }
            return Activity::Pending("Awaiting Overleaf confirmation");
        }
        let dirty = self.overleaf_has_unsaved_changes();
        if state.save_active || dirty {
            if let Some(error) = &state.error {
                return Activity::Failed(error.clone());
            }
            let mirror = state.io.activity();
            if matches!(mirror, Activity::Failed(_)) {
                return mirror;
            }
            let availability = state.connection_activity();
            match availability {
                Activity::Failed(_) => return availability,
                Activity::Inactive(_) => {
                    return Activity::Pending(
                        if matches!(state.browser_status, BrowserStatus::SignedOut) {
                            "Sign in to save"
                        } else {
                            "Offline · unsaved changes"
                        },
                    );
                }
                Activity::Running => return Activity::Pending("Waiting for connection"),
                _ => {}
            }
            if state.save_active {
                return if mirror == Activity::Running {
                    Activity::Running
                } else {
                    Activity::Pending("Updating working copy")
                };
            }
            return Activity::Pending(
                if state
                    .connection
                    .as_ref()
                    .is_some_and(|c| c.hydration.pending)
                {
                    "Waiting for sync"
                } else {
                    "Unsaved changes"
                },
            );
        }
        state.connection_activity()
    }

    /// Source files use the remote acknowledgement state; generated files do not.
    pub(in crate::app) fn overleaf_save_for_active(&self, status: &Activity) -> Option<Activity> {
        let path = self.document().path().as_ref()?;
        let connection = self.overleaf.connection.as_ref()?;
        if !path.starts_with(&connection.root) {
            return None;
        }
        let bound = self.tabs.active_id().and_then(|tab| {
            self.overleaf
                .tab_entities
                .get(&tab)
                .filter(|id| self.overleaf_tab_matches(tab, id))
                .and_then(|id| connection.model.tree()?.entry(id))
        });
        bound
            .or_else(|| self.overleaf_entry(path))
            .filter(|entry| entry.kind == EntryKind::Doc)
            .map(|_| status.clone())
    }
}
