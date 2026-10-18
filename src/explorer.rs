//! Stable Explorer identities and a validated, user-configurable display order.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::workspace::{WorkspaceNode, WorkspaceSnapshot};

/// One visibility result for an immutable filesystem snapshot and search query.
/// Retaining the snapshot prevents an address from being reused as a cache hit.
#[derive(Default)]
struct WorkspaceFilter {
    snapshot: Option<Arc<WorkspaceSnapshot>>,
    query: String,
    matches: Option<Arc<WorkspaceMatches>>,
    #[cfg(test)]
    builds: usize,
}

#[derive(Default)]
pub(crate) struct WorkspaceMatches {
    paths: HashSet<PathBuf>,
    directories: Vec<PathBuf>,
    #[cfg(test)]
    visited: usize,
}

impl WorkspaceMatches {
    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.paths.contains(path)
    }

    pub(crate) fn directories(&self) -> &[PathBuf] {
        &self.directories
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    fn visit(&mut self, node: &WorkspaceNode, query: &str) -> bool {
        #[cfg(test)]
        {
            self.visited += 1;
        }
        let mut descendant_matches = false;
        for child in &node.children {
            // Visit every child even when a sibling matches: each node's
            // visibility is built once for subsequent tree construction.
            descendant_matches |= self.visit(child, query);
        }
        let matches = node.display_name().to_lowercase().contains(query)
            || node
                .relative_path
                .to_string_lossy()
                .to_lowercase()
                .contains(query)
            || descendant_matches;
        if matches {
            self.paths.insert(node.path.clone());
            if node.is_directory() {
                self.directories.push(node.path.clone());
            }
        }
        matches
    }
}

impl WorkspaceFilter {
    fn prepare(
        &mut self,
        snapshot: Option<&Arc<WorkspaceSnapshot>>,
        query: &str,
    ) -> Option<Arc<WorkspaceMatches>> {
        let Some(snapshot) = snapshot.filter(|_| !query.is_empty()) else {
            // Do not pin an old tree or retain path allocations once filtering
            // is cleared or its workspace disappears.
            *self = Self::default();
            return None;
        };
        if self.query == query
            && self
                .snapshot
                .as_ref()
                .is_some_and(|previous| Arc::ptr_eq(previous, snapshot))
        {
            return self.matches.clone();
        }
        let mut matches = WorkspaceMatches::default();
        for node in &snapshot.nodes {
            matches.visit(node, query);
        }
        let matches = Arc::new(matches);
        self.snapshot = Some(Arc::clone(snapshot));
        self.query = query.to_owned();
        self.matches = Some(Arc::clone(&matches));
        #[cfg(test)]
        {
            self.builds += 1;
        }
        Some(matches)
    }
}

#[cfg(test)]
mod workspace_filter_tests {
    use super::*;
    use crate::workspace::WorkspaceNodeKind;

    fn node(
        root: &Path,
        relative: impl AsRef<Path>,
        children: Vec<WorkspaceNode>,
    ) -> WorkspaceNode {
        let relative = relative.as_ref();
        WorkspaceNode {
            name: relative.file_name().unwrap().to_owned(),
            path: root.join(relative),
            relative_path: relative.to_owned(),
            kind: if children.is_empty() {
                WorkspaceNodeKind::File
            } else {
                WorkspaceNodeKind::Directory
            },
            children,
        }
    }

    fn snapshot(root: &str, filename: &str) -> Arc<WorkspaceSnapshot> {
        let root = PathBuf::from(root);
        Arc::new(WorkspaceSnapshot {
            nodes: vec![node(
                &root,
                "chapters",
                vec![
                    node(&root, Path::new("chapters").join(filename), vec![]),
                    node(&root, "chapters/unrelated.txt", vec![]),
                ],
            )],
            root,
        })
    }

    // The pre-cache production recursive predicate, retained only to compare
    // behavior and operation counts with the new postorder classifier.
    fn reference_matches(node: &WorkspaceNode, query: &str, visits: &mut usize) -> bool {
        *visits += 1;
        node.display_name().to_lowercase().contains(query)
            || node
                .relative_path
                .to_string_lossy()
                .to_lowercase()
                .contains(query)
            || node
                .children
                .iter()
                .any(|child| reference_matches(child, query, visits))
    }

    fn reference_frame(nodes: &[WorkspaceNode], query: &str) -> (usize, usize, usize) {
        fn ancestors(nodes: &[WorkspaceNode], query: &str, visits: &mut usize) -> usize {
            let mut count = 0;
            for node in nodes.iter().filter(|node| node.is_directory()) {
                if reference_matches(node, query, visits) {
                    count += 1 + ancestors(&node.children, query, visits);
                }
            }
            count
        }
        fn rows(nodes: &[WorkspaceNode], query: &str, visits: &mut usize) -> usize {
            let mut count = 0;
            for node in nodes {
                if reference_matches(node, query, visits) {
                    count += 1;
                    if node.is_directory() {
                        count += rows(&node.children, query, visits);
                    }
                }
            }
            count
        }
        let mut visits = 0;
        let directories = ancestors(nodes, query, &mut visits);
        let visible = rows(nodes, query, &mut visits);
        let any = nodes
            .iter()
            .any(|node| reference_matches(node, query, &mut visits));
        assert_eq!(any, visible != 0);
        (visible, directories, visits)
    }

    fn cached_rows(nodes: &[WorkspaceNode], matches: &WorkspaceMatches) -> usize {
        nodes
            .iter()
            .filter(|node| matches.contains(&node.path))
            .map(|node| 1 + cached_rows(&node.children, matches))
            .sum()
    }

    fn assert_reference(snapshot: &WorkspaceSnapshot, query: &str, matches: &WorkspaceMatches) {
        fn verify(nodes: &[WorkspaceNode], query: &str, matches: &WorkspaceMatches) {
            for node in nodes {
                let expected = reference_matches(node, query, &mut 0);
                assert_eq!(matches.contains(&node.path), expected, "{:?}", node.path);
                assert_eq!(
                    matches.directories.contains(&node.path),
                    expected && node.is_directory(),
                    "{:?}",
                    node.path
                );
                verify(&node.children, query, matches);
            }
        }
        verify(&snapshot.nodes, query, matches);
        let (rows, directories, _) = reference_frame(&snapshot.nodes, query);
        assert_eq!(matches.paths.len(), rows);
        assert_eq!(matches.directories.len(), directories);
        assert_eq!(cached_rows(&snapshot.nodes, matches), rows);
    }

    #[test]
    fn unchanged_snapshot_and_query_reuse_filter_without_visiting_nodes() {
        let snapshot = snapshot("/workspace", "Résumé.TYP");
        let mut filter = WorkspaceFilter::default();
        let first = filter.prepare(Some(&snapshot), "résumé").unwrap();
        assert_eq!(first.visited, 3);
        for _ in 0..100 {
            let reused = filter.prepare(Some(&snapshot), "résumé").unwrap();
            assert!(Arc::ptr_eq(&first, &reused));
        }
        assert_eq!(filter.builds, 1);
        assert_reference(&snapshot, "résumé", &first);
    }

    #[test]
    fn query_and_snapshot_replacement_rebuild_even_with_the_same_root() {
        let original = snapshot("/workspace", "needle.typ");
        let mut filter = WorkspaceFilter::default();
        let first = filter.prepare(Some(&original), "needle").unwrap();
        let no_match = filter.prepare(Some(&original), "missing").unwrap();
        assert!(!Arc::ptr_eq(&first, &no_match));
        assert!(no_match.is_empty());
        assert_eq!(no_match.visited, 3);
        let replacement = snapshot("/workspace", "missing.tex");
        let new_match = filter.prepare(Some(&replacement), "missing").unwrap();
        assert!(!new_match.is_empty());
        assert_reference(&replacement, "missing", &new_match);
        let other_root = snapshot("/another", "missing.typ");
        let other_match = filter.prepare(Some(&other_root), "missing").unwrap();
        assert!(other_match.contains(Path::new("/another/chapters/missing.typ")));
        assert!(!other_match.contains(Path::new("/workspace/chapters/missing.tex")));
        assert_eq!(filter.builds, 4);
        assert_eq!(Arc::strong_count(&original), 1);
        assert_eq!(Arc::strong_count(&replacement), 1);
    }

    #[test]
    fn clearing_filter_or_workspace_releases_snapshot_and_match_allocations() {
        for no_snapshot in [false, true] {
            let snapshot = snapshot("/workspace", "needle.typ");
            let weak_snapshot = Arc::downgrade(&snapshot);
            let mut filter = WorkspaceFilter::default();
            let matches = filter.prepare(Some(&snapshot), "needle").unwrap();
            let weak_matches = Arc::downgrade(&matches);
            drop(matches);
            if no_snapshot {
                assert!(filter.prepare(None, "needle").is_none());
            } else {
                assert!(filter.prepare(Some(&snapshot), "").is_none());
            }
            drop(snapshot);
            assert!(weak_snapshot.upgrade().is_none());
            assert!(weak_matches.upgrade().is_none());
            assert_eq!(filter.query.capacity(), 0);
            assert!(filter.snapshot.is_none() && filter.matches.is_none());
        }
    }

    #[test]
    fn retained_ancestors_unicode_and_relative_paths_match_recursive_semantics() {
        let snapshot = snapshot("/workspace", "Résumé.TYP");
        let mut filter = WorkspaceFilter::default();
        for query in ["résumé", "chapters/", "chapters", "unrelated", "absent"] {
            let matches = filter.prepare(Some(&snapshot), query).unwrap();
            assert_reference(&snapshot, query, &matches);
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_filenames_retain_lossy_matching_without_changing_identity() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let name = OsString::from_vec(b"bad-\xff.typ".to_vec());
        let root = PathBuf::from("/workspace");
        let snapshot = Arc::new(WorkspaceSnapshot {
            nodes: vec![node(&root, &name, vec![])],
            root,
        });
        let mut filter = WorkspaceFilter::default();
        for query in ["bad-", "�", ".typ"] {
            let matches = filter.prepare(Some(&snapshot), query).unwrap();
            assert_reference(&snapshot, query, &matches);
            assert!(matches.contains(&snapshot.nodes[0].path));
        }
    }

    fn deep_snapshot() -> Arc<WorkspaceSnapshot> {
        fn branch(root: &Path, relative: PathBuf, depth: usize) -> WorkspaceNode {
            let mut children = (0..32)
                .map(|file| node(root, relative.join(format!("notes-{file:02}.typ")), vec![]))
                .collect::<Vec<_>>();
            children.push(if depth == 1 {
                node(root, relative.join("needle.typ"), vec![])
            } else {
                branch(root, relative.join(format!("level-{depth:02}")), depth - 1)
            });
            node(root, relative, children)
        }
        let root = PathBuf::from("/fixture/explorer-filter");
        Arc::new(WorkspaceSnapshot {
            nodes: (0..8)
                .map(|index| branch(&root, format!("branch-{index:02}").into(), 16))
                .collect(),
            root,
        })
    }

    #[test]
    fn deep_tree_filter_visits_each_node_once_and_matches_existing_results() {
        let snapshot = deep_snapshot();
        let mut filter = WorkspaceFilter::default();
        let matches = filter.prepare(Some(&snapshot), "needle").unwrap();
        assert_eq!(matches.visited, 4_232);
        let (rows, directories, recursive_visits) = reference_frame(&snapshot.nodes, "needle");
        assert_eq!(rows, matches.paths.len());
        assert_eq!(directories, matches.directories.len());
        assert_eq!(cached_rows(&snapshot.nodes, &matches), rows);
        assert!(recursive_visits > matches.visited * 10);
        assert_eq!(filter.builds, 1);
    }

    #[test]
    #[ignore = "opt-in optimized Explorer filtering measurement; no timing assertion"]
    fn explorer_filter_cost_measurement() {
        assert!(
            std::hint::black_box(!cfg!(debug_assertions)),
            "use --release"
        );
        let snapshot = deep_snapshot();
        let query = "needle";
        let mut warm_filter = WorkspaceFilter::default();
        let admitted = warm_filter.prepare(Some(&snapshot), query).unwrap();
        let expected = (admitted.paths.len(), admitted.directories.len());
        let mut reference_us = Vec::new();
        let mut cold_us = Vec::new();
        let mut warm_us = Vec::new();
        for sample in 0..47 {
            // Counterbalance order; all three cases use the same process,
            // source tree, normalized query and admitted visible-row walk.
            let cases = if sample % 2 == 0 {
                [0, 1, 2]
            } else {
                [2, 1, 0]
            };
            for case in cases {
                let mut cold_filter = WorkspaceFilter::default();
                let start = std::time::Instant::now();
                let result = match case {
                    0 => {
                        let (rows, directories, _) = reference_frame(
                            std::hint::black_box(&snapshot.nodes),
                            std::hint::black_box(query),
                        );
                        (rows, directories)
                    }
                    _ => {
                        let filter = if case == 1 {
                            &mut cold_filter
                        } else {
                            &mut warm_filter
                        };
                        let matches = filter
                            .prepare(
                                Some(std::hint::black_box(&snapshot)),
                                std::hint::black_box(query),
                            )
                            .unwrap();
                        let rows = cached_rows(&snapshot.nodes, &matches);
                        assert_eq!(matches.is_empty(), rows == 0);
                        (rows, matches.directories().len())
                    }
                };
                let elapsed = start.elapsed().as_secs_f64() * 1_000_000.0;
                assert_eq!(result, expected);
                std::hint::black_box(result);
                if sample >= 7 {
                    match case {
                        0 => reference_us.push(elapsed),
                        1 => cold_us.push(elapsed),
                        _ => warm_us.push(elapsed),
                    }
                }
            }
        }
        let median = |samples: &[f64]| {
            let mut ordered = samples.to_vec();
            ordered.sort_by(f64::total_cmp);
            ordered[ordered.len() / 2]
        };
        eprintln!(
            "EXPLORER_FILTER_MEASUREMENT {}",
            serde_json::json!({
                "fixture": "eight branches, depth16,32 sibling files per directory,one matching leaf per branch",
                "nodes": admitted.visited,
                "visible_rows": expected.0,
                "opened_directories": expected.1,
                "query": query,
                "warmup_samples": 7,
                "measured_samples": reference_us.len(),
                "optimized": true,
                "architecture": std::env::consts::ARCH,
                "operating_system": std::env::consts::OS,
                "reference_median_us": median(&reference_us),
                "cold_median_us": median(&cold_us),
                "warm_median_us": median(&warm_us),
                "reference_us": reference_us,
                "cold_us": cold_us,
                "warm_us": warm_us,
            })
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExplorerSection {
    Files,
    Git,
    Contents,
    Subfiles,
    Symbols,
    Packages,
    Tags,
    References,
    DrawSymbol,
}

impl ExplorerSection {
    pub(crate) const ALL: [Self; 9] = [
        Self::Files,
        Self::Git,
        Self::Contents,
        Self::Subfiles,
        Self::Symbols,
        Self::Packages,
        Self::Tags,
        Self::References,
        Self::DrawSymbol,
    ];

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Git => "Git",
            Self::Contents => "Contents",
            Self::Subfiles => "Subfiles",
            Self::Symbols => "Symbols",
            Self::Packages => "Packages",
            Self::Tags => "Tags",
            Self::References => "References",
            Self::DrawSymbol => "Draw symbol",
        }
    }

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Files => "workspace-files",
            Self::Git => "workspace-git",
            Self::Contents => "workspace-contents",
            Self::Subfiles => "workspace-subfiles",
            Self::Symbols => "workspace-symbols",
            Self::Packages => "workspace-packages",
            Self::Tags => "workspace-tags",
            Self::References => "workspace-references",
            Self::DrawSymbol => "workspace-draw-symbol",
        }
    }

    pub(crate) const fn default_open(self) -> bool {
        matches!(self, Self::Files | Self::Contents)
    }

    pub(crate) const fn searchable(self) -> bool {
        !matches!(self, Self::Git | Self::DrawSymbol)
    }
}

/// Every section occurs exactly once. Neither settings nor a reorder operation
/// can create duplicate panels or omit one; UI state remains keyed by identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[ExplorerSection; 9]", into = "[ExplorerSection; 9]")]
pub(crate) struct ExplorerOrder([ExplorerSection; 9]);

impl Default for ExplorerOrder {
    fn default() -> Self {
        Self(ExplorerSection::ALL)
    }
}

impl TryFrom<[ExplorerSection; 9]> for ExplorerOrder {
    type Error = &'static str;

    fn try_from(sections: [ExplorerSection; 9]) -> Result<Self, Self::Error> {
        let mut seen = [false; 9];
        for section in sections {
            if std::mem::replace(&mut seen[section.index()], true) {
                return Err("Explorer order must contain each panel exactly once");
            }
        }
        Ok(Self(sections))
    }
}

impl From<ExplorerOrder> for [ExplorerSection; 9] {
    fn from(order: ExplorerOrder) -> Self {
        order.0
    }
}

impl ExplorerOrder {
    pub(crate) fn sections(self) -> [ExplorerSection; 9] {
        self.0
    }

    pub(crate) fn move_to(&mut self, section: ExplorerSection, destination: usize) -> bool {
        let source = self.0.iter().position(|entry| *entry == section).unwrap();
        if destination >= self.0.len() || destination == source {
            return false;
        }
        if source < destination {
            self.0[source..=destination].rotate_left(1);
        } else {
            self.0[destination..=source].rotate_right(1);
        }
        true
    }

    pub(crate) fn next_open(
        self,
        open: [bool; 9],
        after: ExplorerSection,
    ) -> Option<ExplorerSection> {
        self.0
            .into_iter()
            .skip_while(|section| *section != after)
            .skip(1)
            .find(|section| open[section.index()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revealing_git_remains_visible_when_another_section_was_maximized() {
        let mut panel = ExplorerPanelState::default();
        panel.toggle_section_maximized(ExplorerSection::Files);
        panel.set_git_reveal(true);
        assert_eq!(panel.maximized_section(true), Some(ExplorerSection::Git));
        panel.set_git_reveal(false);
        assert_eq!(panel.maximized_section(false), None);
        panel.set_git_reveal(true);
        assert_eq!(panel.maximized_section(true), None);
    }

    #[test]
    fn every_move_preserves_each_panel_once_and_roundtrips() {
        for section in ExplorerSection::ALL {
            for destination in 0..8 {
                let mut order = ExplorerOrder::default();
                order.move_to(section, destination);
                assert_eq!(order.sections()[destination], section);
                assert_eq!(ExplorerOrder::try_from(order.sections()), Ok(order));
                let encoded = serde_json::to_string(&order).unwrap();
                assert_eq!(
                    serde_json::from_str::<ExplorerOrder>(&encoded).unwrap(),
                    order
                );
            }
        }
        let mut order = ExplorerOrder::default();
        assert!(!order.move_to(ExplorerSection::Files, usize::MAX));
        assert_eq!(order.sections()[1], ExplorerSection::Git);
    }

    #[test]
    fn duplicate_or_missing_panels_are_rejected() {
        let mut sections = ExplorerSection::ALL;
        sections[7] = ExplorerSection::Files;
        assert!(ExplorerOrder::try_from(sections).is_err());
        let encoded = serde_json::to_string(&sections).unwrap();
        assert!(serde_json::from_str::<ExplorerOrder>(&encoded).is_err());
        assert!(serde_json::from_str::<ExplorerOrder>("[\"files\"]").is_err());
    }

    #[test]
    fn resize_neighbor_follows_display_order_and_skips_closed_panels() {
        let mut order = ExplorerOrder::default();
        order.move_to(ExplorerSection::References, 0);
        order.move_to(ExplorerSection::Tags, 2);
        let mut open = [false; 9];
        for section in [
            ExplorerSection::References,
            ExplorerSection::Tags,
            ExplorerSection::Git,
        ] {
            open[section.index()] = true;
        }
        assert_eq!(
            order.next_open(open, ExplorerSection::References),
            Some(ExplorerSection::Tags)
        );
        assert_eq!(
            order.next_open(open, ExplorerSection::Tags),
            Some(ExplorerSection::Git)
        );
        assert_eq!(order.next_open(open, ExplorerSection::Git), None);
    }
}
/// Visibility and size restoration belong to the panel, not keyboard/menu adapters.
///
/// This is also the smallest useful width to persist. The live panel can be
/// temporarily constrained by a narrow viewport, but retaining that transient
/// constraint would make a later normal-sized window reopen with a nearly
/// invisible Explorer.
pub(crate) const EXPLORER_MIN_WIDTH: f32 = 160.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExplorerPanelPhase {
    Open,
    Closed,
}

impl ExplorerPanelPhase {
    pub(crate) fn panel_visible(self) -> bool {
        self != Self::Closed
    }
}

pub(crate) struct ExplorerPanelState {
    phase: ExplorerPanelPhase,
    width: Option<f32>,
    restore_pending: bool,
    startup_pending: bool,
    queries: [String; ExplorerSection::ALL.len()],
    focus_search: Option<ExplorerSection>,
    focused_section: Option<ExplorerSection>,
    active_search: Option<ExplorerSection>,
    reveal_git: bool,
    selected_path: Option<PathBuf>,
    maximized: Option<ExplorerSection>,
    workspace_filter: WorkspaceFilter,
}
impl Default for ExplorerPanelState {
    fn default() -> Self {
        Self {
            phase: ExplorerPanelPhase::Open,
            width: None,
            restore_pending: false,
            startup_pending: true,
            queries: std::array::from_fn(|_| String::new()),
            focus_search: None,
            focused_section: None,
            active_search: None,
            reveal_git: true,
            selected_path: None,
            maximized: None,
            workspace_filter: WorkspaceFilter::default(),
        }
    }
}
impl ExplorerPanelState {
    pub(crate) fn clear_workspace_filter(&mut self) {
        self.workspace_filter = WorkspaceFilter::default();
    }

    pub(crate) fn filter_workspace(
        &mut self,
        snapshot: Option<&Arc<WorkspaceSnapshot>>,
        normalized_query: &str,
    ) -> Option<Arc<WorkspaceMatches>> {
        self.workspace_filter.prepare(snapshot, normalized_query)
    }

    pub(crate) fn maximized_section(&self, git_visible: bool) -> Option<ExplorerSection> {
        self.maximized
            .filter(|section| *section != ExplorerSection::Git || git_visible)
    }
    pub(crate) fn toggle_section_maximized(&mut self, section: ExplorerSection) {
        self.maximized = if self.maximized == Some(section) {
            None
        } else {
            Some(section)
        };
    }

    pub(crate) fn focus_search(&mut self) {
        self.focus_section_search(ExplorerSection::Files);
    }
    pub(crate) fn focus_section_search(&mut self, section: ExplorerSection) {
        if section.searchable() {
            self.focus_search = Some(section);
            self.active_search = Some(section);
        }
    }
    pub(crate) fn take_search_focus(&mut self) -> Option<ExplorerSection> {
        std::mem::take(&mut self.focus_search)
    }
    pub(crate) fn focused_section(&self) -> Option<ExplorerSection> {
        self.focused_section
    }
    pub(crate) fn set_focused_section(&mut self, section: Option<ExplorerSection>) {
        self.focused_section = section;
    }
    pub(crate) fn update_section_focus(
        &mut self,
        focused: Option<ExplorerSection>,
        pointer_pressed: bool,
        pointer_inside: bool,
    ) {
        if pointer_pressed && !pointer_inside {
            self.focused_section = None;
        } else if let Some(section) = focused {
            self.focused_section = Some(section);
        }
    }
    pub(crate) fn active_search(&self) -> Option<ExplorerSection> {
        self.active_search
    }
    pub(crate) fn set_active_search(&mut self, section: Option<ExplorerSection>) {
        self.active_search = section;
    }
    pub(crate) fn set_git_reveal(&mut self, reveal: bool) {
        self.reveal_git = reveal;
        if reveal && self.maximized.is_some() {
            self.maximized = Some(ExplorerSection::Git);
        } else if !reveal && self.maximized == Some(ExplorerSection::Git) {
            self.maximized = None;
        }
    }
    pub(crate) fn take_git_reveal(&mut self) -> bool {
        std::mem::take(&mut self.reveal_git)
    }
    pub(crate) fn select_path(&mut self, path: PathBuf) {
        self.selected_path = Some(path);
    }
    pub(crate) fn selected_path(&self) -> Option<&Path> {
        self.selected_path.as_deref()
    }
    pub(crate) fn clear_selection(&mut self) {
        self.selected_path = None;
    }
    pub(crate) fn startup_width(&mut self, persisted: f32, default: f32) -> Option<f32> {
        if std::mem::take(&mut self.startup_pending)
            && (!persisted.is_finite() || persisted < EXPLORER_MIN_WIDTH)
        {
            Some(default)
        } else {
            None
        }
    }
    pub(crate) fn open(&mut self) {
        self.restore_pending |= !self.phase.panel_visible();
        self.phase = ExplorerPanelPhase::Open;
    }
    pub(crate) fn clear_keyboard_focus(&mut self) {
        self.focused_section = None;
        self.focus_search = None;
    }
    pub(crate) fn hide(&mut self) {
        self.phase = ExplorerPanelPhase::Closed;
        self.clear_keyboard_focus();
    }
    pub(crate) fn toggle(&mut self) {
        if self.panel_visible() {
            self.hide();
        } else {
            self.open();
        }
    }
    pub(crate) fn panel_visible(&self) -> bool {
        self.phase.panel_visible()
    }
    pub(crate) fn query(&self, section: ExplorerSection) -> &str {
        &self.queries[section.index()]
    }
    #[cfg(test)]
    pub(crate) fn queries(&self) -> &[String; ExplorerSection::ALL.len()] {
        &self.queries
    }
    pub(crate) fn take_queries(&mut self) -> [String; ExplorerSection::ALL.len()] {
        std::mem::take(&mut self.queries)
    }
    pub(crate) fn set_queries(&mut self, queries: [String; ExplorerSection::ALL.len()]) {
        self.queries = queries;
    }
    pub(crate) fn remember_width(&mut self, width: f32) {
        if self.panel_visible() && width.is_finite() && width >= EXPLORER_MIN_WIDTH {
            self.width = Some(width);
        }
    }
    pub(crate) fn take_restored_width(&mut self) -> Option<f32> {
        std::mem::take(&mut self.restore_pending)
            .then_some(self.width)
            .flatten()
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;

    #[test]
    fn presentation_requests_are_owner_local_and_consumed_once() {
        let mut first = ExplorerPanelState::default();
        let mut second = ExplorerPanelState::default();
        first.focus_search();
        assert_eq!(first.take_search_focus(), Some(ExplorerSection::Files));
        assert_eq!(first.take_search_focus(), None);
        assert_eq!(second.take_search_focus(), None);
        assert!(first.take_git_reveal());
        assert!(!first.take_git_reveal());
        assert!(second.take_git_reveal());
        first.set_git_reveal(true);
        first.set_git_reveal(false);
        assert!(!first.take_git_reveal());
    }

    #[test]
    fn search_queries_and_focus_are_section_local() {
        let mut panel = ExplorerPanelState::default();
        let mut queries = panel.queries().clone();
        queries[ExplorerSection::Files.index()] = "chapter".into();
        queries[ExplorerSection::Tags.index()] = "lemma".into();
        panel.set_queries(queries);
        assert_eq!(panel.query(ExplorerSection::Files), "chapter");
        assert_eq!(panel.query(ExplorerSection::Tags), "lemma");
        assert_eq!(panel.query(ExplorerSection::Contents), "");
        panel.set_focused_section(Some(ExplorerSection::Tags));
        panel.focus_section_search(panel.focused_section().unwrap());
        assert_eq!(panel.take_search_focus(), Some(ExplorerSection::Tags));
        panel.focus_section_search(ExplorerSection::Git);
        assert_eq!(panel.take_search_focus(), None);
        panel.update_section_focus(Some(ExplorerSection::Files), true, false);
        assert_eq!(
            panel.focused_section(),
            None,
            "an outside click wins over stale text focus"
        );
        panel.update_section_focus(Some(ExplorerSection::Contents), true, true);
        assert_eq!(panel.focused_section(), Some(ExplorerSection::Contents));
    }

    #[test]
    fn hiding_explorer_retires_keyboard_focus_without_discarding_search_queries() {
        for toggle in [false, true] {
            let mut panel = ExplorerPanelState::default();
            let mut queries = panel.queries().clone();
            queries[ExplorerSection::Tags.index()] = "lemma".into();
            panel.set_queries(queries);
            panel.set_focused_section(Some(ExplorerSection::Tags));
            panel.focus_section_search(ExplorerSection::Tags);
            if toggle {
                panel.toggle();
            } else {
                panel.hide();
            }
            assert!(!panel.panel_visible());
            assert!(panel.focused_section().is_none());
            assert!(panel.take_search_focus().is_none());
            assert_eq!(panel.query(ExplorerSection::Tags), "lemma");
            assert_eq!(panel.active_search(), Some(ExplorerSection::Tags));
            panel.open();
            assert!(panel.focused_section().is_none());
            assert!(panel.take_search_focus().is_none());
            assert_eq!(panel.query(ExplorerSection::Tags), "lemma");
        }
    }

    #[test]
    fn startup_repairs_tiny_persisted_sizes_without_overwriting_valid_sizes() {
        for width in [12.0, 96.0, f32::NAN] {
            let mut state = ExplorerPanelState::default();
            assert_eq!(state.startup_width(width, 230.0), Some(230.0));
            assert_eq!(state.startup_width(100.0, 230.0), None);
        }
        let mut state = ExplorerPanelState::default();
        assert_eq!(state.startup_width(310.0, 230.0), None);
    }
    #[test]
    fn explorer_toggle_closes_immediately_without_an_empty_panel_frame() {
        let mut state = ExplorerPanelState::default();
        state.remember_width(260.0);
        state.toggle();
        assert!(
            !state.panel_visible(),
            "close must retire the panel in the same layout pass"
        );
        state.toggle();
        assert!(state.panel_visible());
        assert_eq!(state.take_restored_width(), Some(260.0));
    }

    #[test]
    fn closing_frames_cannot_replace_saved_width_and_every_open_path_restores_it() {
        let mut state = ExplorerPanelState::default();
        state.remember_width(260.0);
        state.toggle();
        state.remember_width(12.0);
        assert!(!state.panel_visible());
        state.toggle();
        assert_eq!(state.take_restored_width(), Some(260.0));
        assert_eq!(state.take_restored_width(), None);
        state.remember_width(330.0);
        state.toggle();
        state.open();
        assert_eq!(state.take_restored_width(), Some(330.0));
        state.remember_width(EXPLORER_MIN_WIDTH - 1.0);
        state.hide();
        state.open();
        assert_eq!(state.take_restored_width(), Some(330.0));
        state.remember_width(f32::NAN);
        state.hide();
        state.open();
        assert_eq!(state.take_restored_width(), Some(330.0));
    }

    #[test]
    fn imported_file_selection_is_owner_local_and_can_be_replaced() {
        let mut state = ExplorerPanelState::default();
        let first = PathBuf::from("/workspace/first.typ");
        let second = PathBuf::from("/workspace/second.typ");
        state.select_path(first.clone());
        assert_eq!(state.selected_path(), Some(first.as_path()));
        state.select_path(second.clone());
        assert_eq!(state.selected_path(), Some(second.as_path()));
        state.clear_selection();
        assert_eq!(state.selected_path(), None);
    }
}
