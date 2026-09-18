//! Process-wide accounting for fallback PDF pixels and GPU textures.
//!
//! Texture destruction still happens on the owning UI thread. Eviction marks a
//! lease invalid and wakes that owner; the next frame drops the corresponding
//! `TextureHandle` and decoded bytes. Accounting is released immediately so
//! independent windows cannot each assume they own the complete budget.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use tiptoptyp_core::document::WindowSessionId;

use crate::worker::RepaintTarget;

pub(crate) const DECODED_PIXEL_BUDGET: usize = 96 * 1024 * 1024;
pub(crate) const TEXTURE_BUDGET: usize = 192 * 1024 * 1024;

struct Entry {
    owner: WindowSessionId,
    decoded_bytes: usize,
    texture_bytes: usize,
    visible: bool,
    touched: u64,
    alive: Arc<AtomicBool>,
    repaint: RepaintTarget,
}

struct Budget {
    decoded_limit: usize,
    texture_limit: usize,
    decoded_bytes: usize,
    texture_bytes: usize,
    next_id: u64,
    clock: u64,
    entries: HashMap<u64, Entry>,
}

impl Budget {
    fn new(decoded_limit: usize, texture_limit: usize) -> Self {
        Self {
            decoded_limit,
            texture_limit,
            decoded_bytes: 0,
            texture_bytes: 0,
            next_id: 1,
            clock: 1,
            entries: HashMap::new(),
        }
    }

    fn admit(
        &mut self,
        owner: WindowSessionId,
        decoded_bytes: usize,
        texture_bytes: usize,
        repaint: RepaintTarget,
    ) -> (u64, Arc<AtomicBool>, Vec<RepaintTarget>) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.clock = self.clock.wrapping_add(1).max(1);
        let alive = Arc::new(AtomicBool::new(true));
        self.decoded_bytes = self.decoded_bytes.saturating_add(decoded_bytes);
        self.texture_bytes = self.texture_bytes.saturating_add(texture_bytes);
        self.entries.insert(
            id,
            Entry {
                owner,
                decoded_bytes,
                texture_bytes,
                visible: false,
                touched: self.clock,
                alive: alive.clone(),
                repaint,
            },
        );

        let oversized = decoded_bytes > self.decoded_limit || texture_bytes > self.texture_limit;
        let mut wake = Vec::new();
        while self.entries.len() > 1
            && (oversized
                || self.decoded_bytes > self.decoded_limit
                || self.texture_bytes > self.texture_limit)
        {
            let victim = self
                .entries
                .iter()
                .filter(|(candidate, _)| **candidate != id)
                .min_by_key(|(_, entry)| (entry.visible, entry.touched))
                .map(|(candidate, _)| *candidate);
            let Some(victim) = victim else { break };
            if let Some(entry) = self.remove(victim) {
                entry.alive.store(false, Ordering::Release);
                wake.push(entry.repaint);
            }
        }
        (id, alive, wake)
    }

    fn remove(&mut self, id: u64) -> Option<Entry> {
        let entry = self.entries.remove(&id)?;
        self.decoded_bytes = self.decoded_bytes.saturating_sub(entry.decoded_bytes);
        self.texture_bytes = self.texture_bytes.saturating_sub(entry.texture_bytes);
        Some(entry)
    }

    fn set_owner_visible(&mut self, owner: WindowSessionId, visible: &HashSet<u64>) {
        self.clock = self.clock.wrapping_add(1).max(1);
        for (id, entry) in &mut self.entries {
            if entry.owner == owner {
                entry.visible = visible.contains(id);
                if entry.visible {
                    entry.touched = self.clock;
                }
            }
        }
    }
}

static BUDGET: LazyLock<Mutex<Budget>> =
    LazyLock::new(|| Mutex::new(Budget::new(DECODED_PIXEL_BUDGET, TEXTURE_BUDGET)));
pub(crate) struct ResidencyLease {
    id: u64,
    alive: Arc<AtomicBool>,
}

impl std::fmt::Debug for ResidencyLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResidencyLease")
            .field("id", &self.id)
            .field("resident", &self.is_resident())
            .finish()
    }
}

impl ResidencyLease {
    pub(crate) fn is_resident(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for ResidencyLease {
    fn drop(&mut self) {
        let _ = BUDGET
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(self.id);
    }
}

pub(crate) fn admit(
    owner: WindowSessionId,
    decoded_bytes: usize,
    texture_bytes: usize,
    repaint: RepaintTarget,
) -> ResidencyLease {
    let (id, alive, wake) = BUDGET
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .admit(owner, decoded_bytes, texture_bytes, repaint);
    for repaint in wake {
        repaint.request_repaint();
    }
    ResidencyLease { id, alive }
}

pub(crate) fn set_owner_visible(owner: WindowSessionId, visible: impl IntoIterator<Item = u64>) {
    let visible = visible.into_iter().collect::<HashSet<_>>();
    BUDGET
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .set_owner_visible(owner, &visible);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn admit_page(
        budget: &mut Budget,
        owner: WindowSessionId,
        bytes: usize,
    ) -> (u64, Arc<AtomicBool>) {
        let (id, alive, _) =
            budget.admit(owner, bytes, bytes, crate::worker::RepaintTarget::test());
        (id, alive)
    }

    #[test]
    fn one_twenty_and_one_hundred_pages_have_bounded_process_residency() {
        let page_bytes = 8 * 1024 * 1024;
        for pages in [1, 20, 100] {
            let mut budget = Budget::new(page_bytes * 3, page_bytes * 4);
            let owner = WindowSessionId::new(1);
            for _ in 0..pages {
                admit_page(&mut budget, owner, page_bytes);
            }
            assert!(budget.entries.len() <= 3, "{pages}-page fixture");
            assert!(budget.decoded_bytes <= page_bytes * 3);
            assert!(budget.texture_bytes <= page_bytes * 4);
        }
    }

    #[test]
    fn eviction_prefers_nonvisible_pages_across_windows() {
        let mut budget = Budget::new(20, 20);
        let first_owner = WindowSessionId::new(1);
        let second_owner = WindowSessionId::new(2);
        let (visible_id, visible_alive) = admit_page(&mut budget, first_owner, 10);
        budget.set_owner_visible(first_owner, &HashSet::from([visible_id]));
        let (_, hidden_alive) = admit_page(&mut budget, second_owner, 10);
        let (_, newest_alive) = admit_page(&mut budget, second_owner, 10);
        assert!(visible_alive.load(Ordering::Acquire));
        assert!(!hidden_alive.load(Ordering::Acquire));
        assert!(newest_alive.load(Ordering::Acquire));
    }

    #[test]
    fn an_oversized_page_is_admitted_alone() {
        let mut budget = Budget::new(10, 10);
        let owner = WindowSessionId::new(1);
        let (_, old) = admit_page(&mut budget, owner, 5);
        let (_, oversized) = admit_page(&mut budget, owner, 25);
        assert!(!old.load(Ordering::Acquire));
        assert!(oversized.load(Ordering::Acquire));
        assert_eq!(budget.entries.len(), 1);
        assert_eq!(budget.decoded_bytes, 25);
    }

    #[test]
    #[ignore = "optimized PDF residency probe"]
    fn optimized_pdf_residency_probe() {
        let page_bytes = 8 * 1024 * 1024;
        let owner = WindowSessionId::new(1);
        for pages in [1_usize, 20, 100] {
            let old_started = Instant::now();
            let mut legacy_bytes = 0_usize;
            for _ in 0..pages {
                legacy_bytes = legacy_bytes.saturating_add(page_bytes * 2);
                std::hint::black_box(legacy_bytes);
            }
            let old_ns = old_started.elapsed().as_nanos();

            let cold_started = Instant::now();
            let mut budget = Budget::new(page_bytes * 3, page_bytes * 4);
            for _ in 0..pages.min(3) {
                admit_page(&mut budget, owner, page_bytes);
            }
            let cold_ns = cold_started.elapsed().as_nanos();
            let ids = budget.entries.keys().copied().collect::<HashSet<_>>();
            let warm_started = Instant::now();
            budget.set_owner_visible(owner, &ids);
            let warm_ns = warm_started.elapsed().as_nanos();
            let idle_started = Instant::now();
            std::hint::black_box((&budget.decoded_bytes, &budget.texture_bytes));
            let idle_ns = idle_started.elapsed().as_nanos();
            let scroll_started = Instant::now();
            for page in 0..pages.max(1) {
                let visible = budget
                    .entries
                    .keys()
                    .nth(page % budget.entries.len().max(1))
                    .copied()
                    .into_iter()
                    .collect::<HashSet<_>>();
                budget.set_owner_visible(owner, &visible);
            }
            let scroll_ns = scroll_started.elapsed().as_nanos();
            let zoom_started = Instant::now();
            for id in ids {
                budget.remove(id);
            }
            for _ in 0..pages.min(3) {
                admit_page(&mut budget, owner, page_bytes);
            }
            let zoom_ns = zoom_started.elapsed().as_nanos();
            println!(
                "pdf_residency,pages={pages},legacy_bytes={legacy_bytes},resident_pages={},decoded_bytes={},texture_bytes={},legacy_ns={old_ns},cold_ns={cold_ns},warm_ns={warm_ns},idle_ns={idle_ns},scroll_ns={scroll_ns},zoom_ns={zoom_ns}",
                budget.entries.len(),
                budget.decoded_bytes,
                budget.texture_bytes
            );
        }
    }
}
