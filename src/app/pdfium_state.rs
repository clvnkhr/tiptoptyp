//! Independent PDF request, search and navigation owners. Resident textures and
//! their document revision remain outside these transient operation machines.
use super::{Reply, RequestKey, SearchKey, Vec2};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct Rendering {
    target: Option<RequestKey>,
    phase: RenderPhase,
}
#[derive(Default)]
enum RenderPhase {
    #[default]
    Idle,
    Requested(RequestKey),
    Received(Reply),
    Settled(RequestKey),
}
impl Rendering {
    pub(super) fn target(&self) -> Option<&RequestKey> {
        self.target.as_ref()
    }
    pub(super) fn set_target(&mut self, target: RequestKey) {
        self.target = Some(target);
    }
    pub(super) fn requested(&self) -> Option<&RequestKey> {
        match &self.phase {
            RenderPhase::Requested(key) | RenderPhase::Settled(key) => Some(key),
            RenderPhase::Received(reply) => Some(&reply.key),
            RenderPhase::Idle => None,
        }
    }
    pub(super) fn request(&mut self, key: &RequestKey) -> bool {
        if self.requested() == Some(key) {
            return false;
        }
        self.phase = RenderPhase::Requested(key.clone());
        true
    }
    pub(super) fn receive(&mut self, reply: Reply) -> bool {
        if !matches!(&self.phase, RenderPhase::Requested(key) if *key == reply.key) {
            return false;
        }
        self.phase = RenderPhase::Received(reply);
        true
    }
    pub(super) fn reply(&self) -> Option<&Reply> {
        if let RenderPhase::Received(reply) = &self.phase {
            Some(reply)
        } else {
            None
        }
    }
    pub(super) fn take_reply(&mut self) -> Option<Reply> {
        let previous = std::mem::take(&mut self.phase);
        if let RenderPhase::Received(reply) = previous {
            self.phase = RenderPhase::Settled(reply.key.clone());
            Some(reply)
        } else {
            self.phase = previous;
            None
        }
    }
    #[cfg(test)]
    pub(super) fn fixture(key: RequestKey) -> Self {
        Self {
            target: Some(key.clone()),
            phase: RenderPhase::Requested(key),
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn phase(&self) -> &'static str {
        match self.phase {
            RenderPhase::Idle => "idle",
            RenderPhase::Requested(_) => "requested",
            RenderPhase::Received(_) => "received",
            RenderPhase::Settled(_) => "settled",
        }
    }
}

#[derive(Default)]
pub(super) struct Searching {
    query: Arc<str>,
    pages: Vec<usize>,
    phase: SearchPhase,
}
#[derive(Default)]
enum SearchPhase {
    #[default]
    Unrequested,
    Queued(SearchIntent),
    Requested {
        key: SearchKey,
        intent: SearchIntent,
    },
    Ready(SearchKey),
}
#[derive(Clone, Copy)]
enum SearchIntent {
    Highlight,
    Navigate,
}
impl Searching {
    pub(super) fn query(&self) -> &Arc<str> {
        &self.query
    }
    pub(super) fn pages(&self) -> &[usize] {
        &self.pages
    }
    pub(super) fn clear_pages(&mut self) {
        self.pages.clear();
    }
    pub(super) fn pending(&self) -> bool {
        !self.query.is_empty()
            && matches!(
                self.phase,
                SearchPhase::Queued(_) | SearchPhase::Requested { .. }
            )
    }
    pub(super) fn requested(&self) -> Option<&SearchKey> {
        match &self.phase {
            SearchPhase::Requested { key, .. } | SearchPhase::Ready(key) => Some(key),
            _ => None,
        }
    }
    /// True asks the caller to advance through already admitted matches.
    pub(super) fn find(&mut self, query: String) -> bool {
        if self.query.as_ref() != query {
            self.query = query.into();
            self.pages.clear();
            self.phase = SearchPhase::Queued(if self.query.is_empty() {
                SearchIntent::Highlight
            } else {
                SearchIntent::Navigate
            });
            false
        } else if self.pending() {
            match &mut self.phase {
                SearchPhase::Queued(intent) | SearchPhase::Requested { intent, .. } => {
                    *intent = SearchIntent::Navigate
                }
                _ => unreachable!(),
            }
            false
        } else {
            true
        }
    }
    pub(super) fn request(&mut self, revision: u64) -> Option<SearchKey> {
        let key = SearchKey {
            revision,
            query: self.query.clone(),
        };
        if self.requested() == Some(&key) {
            return None;
        }
        let intent = match self.phase {
            SearchPhase::Queued(intent) | SearchPhase::Requested { intent, .. } => intent,
            _ => SearchIntent::Highlight,
        };
        self.phase = SearchPhase::Requested {
            key: key.clone(),
            intent,
        };
        Some(key)
    }
    /// A viewport-only render change does not retire a document search. The
    /// exact document/query request must still own its result and one-shot jump.
    pub(super) fn accept(
        &mut self,
        key: SearchKey,
        revision: u64,
        pages: Vec<usize>,
    ) -> Option<bool> {
        if key.revision != revision || key.query != self.query {
            return None;
        }
        let SearchPhase::Requested {
            key: requested,
            intent,
        } = &self.phase
        else {
            return None;
        };
        if *requested != key {
            return None;
        }
        let navigate = matches!(intent, SearchIntent::Navigate);
        self.pages = pages;
        self.phase = SearchPhase::Ready(key);
        Some(navigate)
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn phase(&self) -> &'static str {
        match self.phase {
            SearchPhase::Unrequested => "unrequested",
            SearchPhase::Queued(_) => "queued",
            SearchPhase::Requested { .. } => "requested",
            SearchPhase::Ready(_) => "ready",
        }
    }
}

#[derive(Default)]
pub(super) struct Navigation {
    pending: Option<PositionJump>,
    scroll: Scroll,
}
#[derive(Default, Clone, Copy)]
pub(super) enum Scroll {
    #[default]
    Idle,
    Page(usize),
    Anchor(Option<Vec2>),
}
#[derive(Clone, Copy)]
pub(super) enum PositionIntent {
    Manual,
    Automatic,
}
pub(super) struct PositionJump {
    pub(super) page: usize,
    pub(super) coordinate: Vec2,
    expected_pdf: Option<Arc<[u8]>>,
    intent: PositionIntent,
}
impl PositionJump {
    fn matches(&self, bytes: Option<&Arc<[u8]>>) -> bool {
        self.expected_pdf.as_ref().is_none_or(|expected| {
            bytes.is_some_and(|bytes| Arc::ptr_eq(expected, bytes) || **expected == **bytes)
        })
    }
}
impl Navigation {
    pub(super) fn wait_for_position(
        &mut self,
        page: usize,
        coordinate: Vec2,
        intent: PositionIntent,
        expected_pdf: Option<Arc<[u8]>>,
    ) {
        self.pending = Some(PositionJump {
            page,
            coordinate,
            intent,
            expected_pdf,
        });
    }
    pub(super) fn take_ready_position(
        &mut self,
        same_revision: bool,
        bytes: Option<&Arc<[u8]>>,
    ) -> Option<PositionJump> {
        if same_revision
            && self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.matches(bytes))
        {
            self.pending.take()
        } else {
            None
        }
    }
    pub(super) fn clear_position(&mut self) {
        self.pending = None;
        if let Scroll::Anchor(point) = &mut self.scroll {
            *point = None;
        }
    }
    pub(super) fn automatic_pending(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| matches!(pending.intent, PositionIntent::Automatic))
    }
    pub(super) fn expected_matches(&self, bytes: &Arc<[u8]>) -> bool {
        self.pending
            .as_ref()
            .and_then(|pending| pending.expected_pdf.as_ref())
            .is_some_and(|expected| Arc::ptr_eq(expected, bytes) || **expected == **bytes)
    }
    pub(super) fn transfer_pending(&mut self, bytes: &Arc<[u8]>) -> Self {
        if self.expected_matches(bytes) {
            Self {
                pending: self.pending.take(),
                scroll: Scroll::Idle,
            }
        } else {
            Self::default()
        }
    }
    pub(super) fn go_to_page(&mut self, page: usize) {
        self.scroll = Scroll::Page(page);
    }
    pub(super) fn restore(&mut self) {
        if !matches!(self.scroll, Scroll::Page(_)) && !matches!(self.scroll, Scroll::Anchor(_)) {
            self.scroll = Scroll::Anchor(None);
        }
    }
    pub(super) fn restore_point(&mut self, point: Option<Vec2>) {
        self.scroll = Scroll::Anchor(point);
    }
    pub(super) fn cancel_restore(&mut self) {
        if matches!(self.scroll, Scroll::Anchor(_)) {
            self.scroll = Scroll::Idle;
        }
    }
    pub(super) fn take_scroll(&mut self) -> Scroll {
        std::mem::take(&mut self.scroll)
    }
    #[cfg(test)]
    pub(super) fn page(&self) -> Option<usize> {
        if let Scroll::Page(page) = self.scroll {
            Some(page)
        } else {
            None
        }
    }
    #[cfg(test)]
    pub(super) fn clear_page(&mut self) {
        if matches!(self.scroll, Scroll::Page(_)) {
            self.scroll = Scroll::Idle;
        }
    }
    #[cfg(test)]
    pub(super) fn pending_position(&self) -> Option<(usize, f32)> {
        self.pending
            .as_ref()
            .map(|pending| (pending.page, pending.coordinate.y))
    }
    #[cfg(test)]
    pub(super) fn point(&self) -> Option<Vec2> {
        self.pending.as_ref().map(|pending| pending.coordinate).or(
            if let Scroll::Anchor(point) = self.scroll {
                point
            } else {
                None
            },
        )
    }
    #[cfg(test)]
    pub(super) fn expected_pdf(&self) -> Option<&Arc<[u8]>> {
        self.pending
            .as_ref()
            .and_then(|pending| pending.expected_pdf.as_ref())
    }
    #[cfg(test)]
    pub(super) fn restoring(&self) -> bool {
        matches!(self.scroll, Scroll::Anchor(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(revision: u64) -> RequestKey {
        RequestKey {
            revision,
            pages: vec![0],
            scale: 100,
            palette: Default::default(),
        }
    }
    #[test]
    fn render_receipts_are_owned_and_consumed_once_without_discarding_the_target() {
        let mut state = Rendering::default();
        let current = key(1);
        state.set_target(current.clone());
        assert!(state.request(&current));
        assert!(!state.receive(Reply {
            key: key(2),
            result: Err("stale".into())
        }));
        assert_eq!(state.requested(), Some(&current));
        assert!(state.receive(Reply {
            key: current.clone(),
            result: Err("controlled".into())
        }));
        assert!(!state.request(&current));
        assert!(state.take_reply().is_some());
        assert!(state.take_reply().is_none());
        assert_eq!(state.target(), Some(&current));
        assert!(!state.receive(Reply {
            key: current.clone(),
            result: Err("duplicate".into())
        }));
    }
    #[test]
    fn a_query_round_trip_cannot_reuse_a_receipt_after_its_hits_were_cleared() {
        let mut state = Searching::default();
        assert!(!state.find("norm".into()));
        let first = state.request(1).unwrap();
        assert_eq!(state.accept(first.clone(), 1, vec![2]), Some(true));
        assert!(!state.find("other".into()));
        assert!(!state.find("norm".into()));
        assert!(state.pending());
        let next = state.request(1).unwrap();
        assert_eq!(next, first);
        assert!(
            state.request(1).is_none(),
            "idle frames cannot enqueue the same search again"
        );
        assert_eq!(state.accept(next.clone(), 1, vec![3]), Some(true));
        assert_eq!(state.pages(), [3]);
        assert_eq!(
            state.accept(next, 1, vec![0]),
            None,
            "a duplicate receipt cannot replace admitted hits or jump again"
        );
        assert_eq!(state.pages(), [3]);
    }
    #[test]
    fn stale_search_results_do_not_consume_the_current_query_or_queued_jump() {
        let mut state = Searching::default();
        state.find("norm".into());
        let stale = state.request(1).unwrap();
        state.find("energy".into());
        let current = state.request(1).unwrap();
        assert_eq!(state.accept(stale, 1, vec![0]), None);
        assert!(state.pending());
        assert_eq!(state.accept(current, 1, vec![2]), Some(true));
        assert!(!state.pending());
        state.find(String::new());
        assert!(!state.pending());
        assert!(state.pages().is_empty());
    }
    #[test]
    fn retained_layout_cannot_consume_coordinates_waiting_for_another_pdf() {
        let incoming: Arc<[u8]> = Arc::from(&b"incoming"[..]);
        let previous: Arc<[u8]> = Arc::from(&b"previous"[..]);
        let mut state = Navigation::default();
        let point = Vec2::new(50.0, 75.0);
        state.wait_for_position(2, point, PositionIntent::Automatic, Some(incoming.clone()));
        state.restore();
        assert!(matches!(state.take_scroll(), Scroll::Anchor(None)));
        assert_eq!(state.point(), Some(point));
        assert!(state.take_ready_position(false, Some(&incoming)).is_none());
        assert!(state.take_ready_position(true, Some(&previous)).is_none());
        assert_eq!(state.point(), Some(point));
        let pending = state.take_ready_position(true, Some(&incoming)).unwrap();
        assert_eq!(pending.page, 2);
        assert_eq!(pending.coordinate, point);
        assert!(state.take_ready_position(true, Some(&incoming)).is_none());
    }
    #[test]
    fn explicit_page_navigation_precedes_anchor_restore_and_pointer_zoom_cancels_only_restore() {
        let mut state = Navigation::default();
        state.go_to_page(4);
        state.restore();
        assert!(matches!(state.take_scroll(), Scroll::Page(4)));
        state.wait_for_position(2, Vec2::new(0.0, 75.0), PositionIntent::Manual, None);
        state.restore_point(Some(Vec2::new(25.0, 75.0)));
        state.cancel_restore();
        assert!(matches!(state.take_scroll(), Scroll::Idle));
        assert_eq!(state.pending_position(), Some((2, 75.0)));
        assert!(!state.automatic_pending());
    }
}
