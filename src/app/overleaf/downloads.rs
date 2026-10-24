//! Bounded retry of authoritative files that could not fit the mirror queue.
//! Retain identities, never overflowing file contents; drain one retry at a time.
use super::*;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(35);

#[derive(Default)]
pub(super) struct Deferred {
    session: Option<(u64, u64)>,
    entries: BTreeMap<String, Entry>,
    attempted: BTreeSet<String>,
    active: Option<(Entry, Instant)>,
}
impl Deferred {
    fn session(&mut self, epoch: u64, hydration: u64) {
        if self.session != Some((epoch, hydration)) {
            *self = Self {
                session: Some((epoch, hydration)),
                ..Default::default()
            };
        }
    }
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    pub(super) fn defer(&mut self, epoch: u64, hydration: u64, entry: Entry) -> Result<(), String> {
        self.session(epoch, hydration);
        if self.attempted.contains(&entry.id) {
            return Err("The Overleaf working copy is still too busy to cache a retried file. Refresh after it finishes.".into());
        }
        if self.entries.len() >= crate::overleaf::MAX_ENTRIES
            && !self.entries.contains_key(&entry.id)
        {
            return Err("Too many Overleaf files are waiting to sync. Refresh the project.".into());
        }
        self.entries.insert(entry.id.clone(), entry);
        Ok(())
    }
    pub(super) fn received(&mut self, epoch: u64, hydration: u64, entry: &Entry) {
        if self.session != Some((epoch, hydration)) {
            return;
        }
        if self.entries.get(&entry.id) == Some(entry) {
            self.entries.remove(&entry.id);
        }
        if self
            .active
            .as_ref()
            .is_some_and(|(active, _)| active == entry)
        {
            self.active = None;
        }
    }
    pub(super) fn has_pending(&self) -> bool {
        !self.entries.is_empty() || self.active.is_some()
    }
    pub(super) fn remaining(&self, now: Instant) -> Option<Duration> {
        self.active.as_ref().map(|(_, started)| {
            REQUEST_TIMEOUT.saturating_sub(now.saturating_duration_since(*started))
        })
    }
    pub(super) fn request_failed(&mut self) {
        self.active = None;
    }
    /// Call after the mirror queue and worker have drained. A second overflow
    /// or timeout fails visibly instead of spinning an unbounded retry loop.
    pub(super) fn next(
        &mut self,
        epoch: u64,
        hydration: u64,
        tree: &Tree,
        now: Instant,
    ) -> Result<Option<BrowserCommand>, String> {
        self.session(epoch, hydration);
        if self
            .active
            .as_ref()
            .is_some_and(|(entry, _)| tree.entry(&entry.id) != Some(entry))
            && let Some((entry, _)) = self.active.take()
        {
            self.attempted.remove(&entry.id);
        }
        if let Some(remaining) = self.remaining(now) {
            if remaining.is_zero() {
                self.active = None;
                return Err("Overleaf did not redownload a deferred file in time. Refresh to retry syncing.".into());
            }
            return Ok(None);
        }
        while let Some((id, entry)) = self.entries.pop_first() {
            if tree.entry(&id) != Some(&entry) {
                continue;
            }
            if self.attempted.len() >= crate::overleaf::MAX_ENTRIES {
                self.entries.clear();
                return Err("Too many deferred Overleaf downloads have been retried. Refresh the project to continue syncing.".into());
            }
            self.attempted.insert(id.clone());
            let command = BrowserCommand::Download {
                epoch,
                id,
                path: entry.path.clone(),
            };
            self.active = Some((entry, now));
            return Ok(Some(command));
        }
        Ok(None)
    }
}

impl EditorApp {
    pub(super) fn retry_overleaf_downloads(&mut self, context: &egui::Context) {
        if self.overleaf.io.is_running()
            || self.overleaf.tree_pending.is_some()
            || !self.overleaf.queued.is_empty()
        {
            return;
        }
        let Some(connection) = self
            .overleaf
            .connection
            .as_ref()
            .filter(|connection| connection.online)
        else {
            return;
        };
        let Some(tree) = connection.model.tree() else {
            return;
        };
        let now = Instant::now();
        match self.overleaf.downloads.next(
            connection.model.epoch(),
            connection.hydration.session,
            tree,
            now,
        ) {
            Ok(Some(command)) => {
                if let Err(error) = self.overleaf.send(command) {
                    self.overleaf.downloads.request_failed();
                    self.overleaf.error = Some(error);
                }
            }
            Err(error) => self.overleaf.error = Some(error),
            Ok(None) => {}
        }
        if let Some(remaining) = self.overleaf.downloads.remaining(now) {
            context.request_repaint_after(remaining);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(id: &str) -> Entry {
        Entry {
            id: id.into(),
            path: format!("{id}.tex"),
            kind: EntryKind::Doc,
        }
    }
    fn tree(entries: Vec<Entry>) -> Tree {
        Tree::new("Paper".into(), None, entries).unwrap()
    }
    #[test]
    fn coalesces_identities_and_requests_exactly_one_file_at_a_time() {
        let a = entry("a");
        let b = entry("b");
        let tree = tree(vec![a.clone(), b.clone()]);
        let mut pending = Deferred::default();
        let now = Instant::now();
        for _ in 0..100 {
            pending.defer(1, 2, a.clone()).unwrap();
        }
        pending.defer(1, 2, b.clone()).unwrap();
        assert_eq!(pending.entries.len(), 2);
        assert_eq!(
            pending.next(1, 2, &tree, now).unwrap(),
            Some(BrowserCommand::Download {
                epoch: 1,
                id: "a".into(),
                path: "a.tex".into()
            })
        );
        assert!(pending.next(1, 2, &tree, now).unwrap().is_none());
        pending.received(1, 2, &a);
        assert!(
            matches!(pending.next(1,2,&tree,now).unwrap(),Some(BrowserCommand::Download{id,..}) if id=="b")
        );
        pending.received(1, 2, &b);
        assert!(!pending.has_pending());
    }
    #[test]
    fn timeout_and_second_overflow_stop_automatic_retries() {
        let a = entry("a");
        let tree = tree(vec![a.clone()]);
        let now = Instant::now();
        let mut pending = Deferred::default();
        pending.defer(1, 1, a.clone()).unwrap();
        pending.next(1, 1, &tree, now).unwrap();
        assert_eq!(pending.remaining(now), Some(REQUEST_TIMEOUT));
        assert!(pending.next(1, 1, &tree, now + REQUEST_TIMEOUT).is_err());
        assert!(pending.defer(1, 1, a.clone()).is_err());
        assert!(
            pending
                .next(1, 1, &tree, now + REQUEST_TIMEOUT)
                .unwrap()
                .is_none()
        );
        pending.defer(1, 2, a).unwrap();
        assert!(pending.next(1, 2, &tree, now).unwrap().is_some());
    }
    #[test]
    fn old_sessions_and_renames_cannot_complete_a_new_download() {
        let a = entry("a");
        let tree = tree(vec![a.clone()]);
        let now = Instant::now();
        let mut pending = Deferred::default();
        pending.defer(1, 1, a.clone()).unwrap();
        pending.next(1, 1, &tree, now).unwrap();
        pending.defer(1, 2, a.clone()).unwrap();
        pending.next(1, 2, &tree, now).unwrap();
        pending.received(1, 1, &a);
        assert!(pending.has_pending());
        let mut renamed = a.clone();
        renamed.path = "renamed.tex".into();
        pending.received(1, 2, &renamed);
        assert!(pending.has_pending());
        pending.received(1, 2, &a);
        assert!(!pending.has_pending());
        pending.clear();
        assert!(!pending.has_pending());
    }
    #[test]
    fn removed_or_renamed_entries_are_not_downloaded_from_stale_paths() {
        let a = entry("a");
        let mut pending = Deferred::default();
        pending.defer(1, 1, a.clone()).unwrap();
        assert!(
            pending
                .next(1, 1, &tree(Vec::new()), Instant::now())
                .unwrap()
                .is_none()
        );
        assert!(!pending.has_pending());
        pending.defer(1, 1, a).unwrap();
        pending.request_failed();
        assert!(
            pending
                .next(2, 1, &tree(Vec::new()), Instant::now())
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn full_size_asset_pressure_is_retained_as_identities_then_drained_serially() {
        let assets: Vec<_> = (0..25)
            .map(|index| Entry {
                id: format!("asset{index:02}"),
                path: format!("asset{index:02}.png"),
                kind: EntryKind::File,
            })
            .collect();
        // These uploads would exceed the 64 MiB source queue even before
        // base64 overhead. Deferred stores no asset bytes whatsoever.
        assert!(assets.len() * crate::overleaf::MAX_FILE_BYTES > MAX_PENDING_BYTES);
        let tree = tree(assets.clone());
        let mut pending = Deferred::default();
        for asset in &assets {
            pending.defer(1, 1, asset.clone()).unwrap();
        }
        let now = Instant::now();
        for asset in assets {
            let command = pending.next(1, 1, &tree, now).unwrap().unwrap();
            assert_eq!(
                command,
                BrowserCommand::Download {
                    epoch: 1,
                    id: asset.id.clone(),
                    path: asset.path.clone()
                }
            );
            assert!(pending.next(1, 1, &tree, now).unwrap().is_none());
            pending.received(1, 1, &asset);
        }
        assert!(!pending.has_pending());
        assert!(pending.next(1, 1, &tree, now).unwrap().is_none());
    }
    #[test]
    fn current_rename_or_removal_cancels_an_obsolete_active_request_before_its_timeout() {
        let old = entry("a");
        let mut renamed = old.clone();
        renamed.path = "renamed.tex".into();
        let previous = tree(vec![old.clone()]);
        let now = Instant::now();
        for current in [tree(vec![renamed.clone()]), tree(Vec::new())] {
            let mut pending = Deferred::default();
            pending.defer(1, 1, old.clone()).unwrap();
            assert!(pending.next(1, 1, &previous, now).unwrap().is_some());
            // A fresh receipt for the current name cannot match the old path.
            pending.received(1, 1, &renamed);
            assert!(pending.has_pending());
            // Tree validation must precede timeout handling: the old request is
            // no longer relevant and must neither delay readiness nor error.
            assert!(
                pending
                    .next(1, 1, &current, now + REQUEST_TIMEOUT)
                    .unwrap()
                    .is_none()
            );
            assert!(!pending.has_pending());
            assert_eq!(pending.remaining(now), None);
        }
    }
}
