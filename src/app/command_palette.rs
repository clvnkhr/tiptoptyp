//! Compact searchable commands; the popup owner executes the returned action.
use crate::shortcuts::ShortcutAction;
use eframe::egui::{self, Pos2, Rect, Vec2};

const ROW_HEIGHT: f32 = 22.0;
const VISIBLE_ROWS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub action: ShortcutAction,
    pub label: &'static str,
    pub category: &'static str,
    pub shortcut: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Execute(ShortcutAction),
    Dismiss,
}

#[derive(Clone, Debug)]
struct SearchText {
    label: String,
    category: String,
}

#[derive(Clone, Copy, Debug)]
struct Matched {
    entry: usize,
    rank: usize,
}

#[derive(Clone, Debug, Default)]
struct Results {
    catalog: Vec<Entry>,
    text: Vec<SearchText>,
    query: Option<String>,
    matched: Vec<Matched>,
    #[cfg(test)]
    rebuilds: usize,
}

#[derive(Clone, Debug)]
pub(super) struct State {
    pub query: String,
    /// Index in the filtered results, rather than the unfiltered catalog.
    pub selected: usize,
    focus_search: bool,
    scroll_selection: bool,
    composing: bool,
    results: Results,
    #[cfg(test)]
    scroll_offset: f32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            focus_search: true,
            scroll_selection: true,
            composing: false,
            results: Results::default(),
            #[cfg(test)]
            scroll_offset: 0.0,
        }
    }
}

impl State {
    pub fn open(&mut self) {
        self.query.clear();
        self.selected = 0;
        self.focus_search = true;
        self.scroll_selection = true;
        self.composing = false;
    }

    pub fn result_count(&self) -> usize {
        self.results.matched.len()
    }

    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
    pub fn result_actions(&self) -> impl Iterator<Item = ShortcutAction> + '_ {
        self.results
            .matched
            .iter()
            .map(|matched| self.results.catalog[matched.entry].action)
    }

    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
    pub fn enabled_actions(&self) -> impl Iterator<Item = ShortcutAction> + '_ {
        self.results.matched.iter().filter_map(|matched| {
            let entry = &self.results.catalog[matched.entry];
            entry.enabled.then_some(entry.action)
        })
    }

    pub fn selected_action(&self) -> Option<ShortcutAction> {
        self.results
            .matched
            .get(self.selected)
            .map(|matched| self.results.catalog[matched.entry].action)
    }

    fn refresh(&mut self, entries: &[Entry]) {
        let catalog_changed = self.results.catalog != entries;
        let query_changed = self.results.query.as_ref() != Some(&self.query);
        if !catalog_changed && !query_changed {
            self.selected = self.selected.min(self.result_count().saturating_sub(1));
            if self
                .results
                .matched
                .get(self.selected)
                .is_some_and(|matched| !self.results.catalog[matched.entry].enabled)
            {
                self.selected = self.first_enabled().unwrap_or(0);
            }
            return;
        }
        let selected_action = self.selected_action();
        if catalog_changed {
            self.results.catalog.clear();
            self.results.catalog.extend_from_slice(entries);
            self.results.text = entries
                .iter()
                .map(|entry| SearchText {
                    label: entry.label.to_lowercase(),
                    category: entry.category.to_lowercase(),
                })
                .collect();
        }
        let normalized = self.query.to_lowercase();
        let terms: Vec<_> = normalized.split_whitespace().collect();
        self.results.matched.clear();
        for (entry, text) in self.results.text.iter().enumerate() {
            if let Some(rank) = search_rank(text, &terms) {
                self.results.matched.push(Matched { entry, rank });
            }
        }
        self.results
            .matched
            .sort_by_key(|matched| (matched.rank, matched.entry));
        self.results.query = Some(self.query.clone());
        let old_selected = self.selected;
        self.selected = if query_changed {
            self.first_enabled().unwrap_or(0)
        } else {
            selected_action
                .and_then(|action| {
                    self.results.matched.iter().position(|matched| {
                        let entry = &self.results.catalog[matched.entry];
                        entry.action == action && entry.enabled
                    })
                })
                .or_else(|| self.first_enabled())
                .unwrap_or(0)
        };
        self.scroll_selection |= query_changed || old_selected != self.selected;
        #[cfg(test)]
        {
            self.results.rebuilds += 1;
        }
    }

    fn first_enabled(&self) -> Option<usize> {
        self.results
            .matched
            .iter()
            .position(|matched| self.results.catalog[matched.entry].enabled)
    }

    fn move_selection(&mut self, down: bool) {
        let count = self.result_count();
        let mut next = self.selected.min(count.saturating_sub(1));
        for _ in 0..count {
            next = step_selection(next, count, down);
            if self.results.catalog[self.results.matched[next].entry].enabled {
                self.selected = next;
                return;
            }
        }
        // Keep disabled commands visible when every result is unavailable.
        // Enter still checks enabled rather than executing another command.
        self.selected = 0;
    }

    fn composition_protects_keys(&mut self, events: &[egui::Event]) -> bool {
        let mut protected = self.composing;
        for event in events {
            match event {
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                    self.composing = !text.is_empty();
                    protected |= self.composing;
                }
                egui::Event::Ime(egui::ImeEvent::Commit(_)) => {
                    self.composing = false;
                    protected = true;
                }
                _ => {}
            }
        }
        protected
    }

    pub fn show(&mut self, ui: &mut egui::Ui, entries: &[Entry]) -> Option<Action> {
        let protected = ui.input(|input| self.composition_protects_keys(&input.events));
        let enabled = ui.is_enabled();
        let (dismiss, up, down, enter) = ui.input_mut(|input| {
            if !input.focused || !enabled || protected {
                return (false, false, false, false);
            }
            (
                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            )
        });
        ui.spacing_mut().item_spacing.y = 2.0;
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .id_salt("command-palette-search")
                .return_key(None)
                .hint_text("Search commands")
                .char_limit(256)
                .desired_width(ui.available_width()),
        );
        search.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(
                egui::WidgetType::TextEdit,
                ui.is_enabled(),
                "Search commands",
            );
            info.current_text_value = Some(self.query.clone());
            info
        });
        if std::mem::take(&mut self.focus_search) {
            search.request_focus();
        }
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("palette.search", &search);
        self.refresh(entries);
        if up != down {
            self.move_selection(down);
            self.scroll_selection = true;
        }
        let mut action = if dismiss {
            Some(Action::Dismiss)
        } else if enter {
            self.results.matched.get(self.selected).and_then(|matched| {
                let entry = &self.results.catalog[matched.entry];
                entry.enabled.then_some(Action::Execute(entry.action))
            })
        } else {
            None
        };
        ui.separator();
        let row_height = ROW_HEIGHT.max(ui.spacing().interact_size.y);
        let height = result_window_height(
            row_height,
            ui.spacing().item_spacing.y,
            ui.available_height(),
        );
        if height <= 0.0 {
            return action;
        }
        if self.results.matched.is_empty() {
            let (rect, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), height),
                egui::Sense::hover(),
            );
            ui.put(
                rect,
                egui::Label::new("No matching commands").selectable(false),
            );
            self.scroll_selection = false;
            return action;
        }
        let selected = self.selected;
        let scroll_selection = std::mem::take(&mut self.scroll_selection);
        let output = egui::ScrollArea::vertical()
            .id_salt("command-palette-results")
            .max_height(height)
            .min_scrolled_height(0.0)
            .auto_shrink([false, false])
            .show_rows(ui, row_height, self.result_count(), |ui, visible| {
                if scroll_selection {
                    let top = ui.max_rect().top()
                        + (selected as f32 - visible.start as f32)
                            * (row_height + ui.spacing().item_spacing.y);
                    ui.scroll_to_rect(
                        Rect::from_min_size(
                            Pos2::new(ui.max_rect().left(), top),
                            Vec2::new(ui.available_width(), row_height),
                        ),
                        None,
                    );
                }
                for result in visible {
                    let entry = &self.results.catalog[self.results.matched[result].entry];
                    let response = ui
                        .push_id(entry.action, |ui| {
                            ui.add_enabled_ui(entry.enabled, |ui| {
                                command_row(ui, entry, result == selected, row_height)
                            })
                            .inner
                        })
                        .inner;
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    if ui.is_rect_visible(response.rect) {
                        crate::desktop_test::observe(
                            &format!("palette.command.{}", entry.action.id()),
                            &response,
                        );
                    }
                    if response.clicked() && entry.enabled {
                        self.selected = result;
                        action = Some(Action::Execute(entry.action));
                    }
                }
            });
        #[cfg(test)]
        {
            self.scroll_offset = output.state.offset.y;
        }
        #[cfg(not(test))]
        let _ = output;
        action
    }
}

fn result_window_height(row_height: f32, spacing: f32, available: f32) -> f32 {
    let rows = row_height * VISIBLE_ROWS as f32;
    let gaps = spacing * (VISIBLE_ROWS - 1) as f32;
    available.max(0.0).min(rows + gaps)
}

fn step_selection(selected: usize, count: usize, down: bool) -> usize {
    if count == 0 {
        return 0;
    }
    let selected = selected.min(count - 1);
    if down {
        if selected + 1 == count {
            0
        } else {
            selected + 1
        }
    } else if selected == 0 {
        count - 1
    } else {
        selected - 1
    }
}

fn search_rank(text: &SearchText, terms: &[&str]) -> Option<usize> {
    let mut total = 0;
    for term in terms {
        let label = term_rank(&text.label, term);
        let category = term_rank(&text.category, term).map(|rank| rank + 80);
        total += match (label, category) {
            (Some(a), Some(b)) => a.min(b),
            (Some(rank), None) | (None, Some(rank)) => rank,
            (None, None) => return None,
        };
    }
    Some(total)
}

fn term_rank(text: &str, term: &str) -> Option<usize> {
    if text == term {
        return Some(0);
    }
    if text.starts_with(term) {
        return Some(10 + text.len().saturating_sub(term.len()));
    }
    if let Some((at, _)) = text.match_indices(term).find(|(at, _)| {
        text[..*at]
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_alphanumeric())
    }) {
        return Some(40 + at);
    }
    if let Some(at) = text.find(term) {
        return Some(80 + at);
    }
    let mut wanted = term.chars();
    let mut next = wanted.next()?;
    let mut first = None;
    let mut matched = 0;
    for (at, character) in text.chars().enumerate() {
        if character != next {
            continue;
        }
        first.get_or_insert(at);
        matched += 1;
        if let Some(character) = wanted.next() {
            next = character;
        } else {
            return Some(200 + first.unwrap() * 2 + (at + 1 - first.unwrap() - matched) * 3);
        }
    }
    None
}

struct RowColumns {
    label: Rect,
    shortcut: Rect,
}

fn row_columns(rect: Rect, shortcut_width: f32) -> RowColumns {
    let padding = 6.0_f32.min(rect.width().max(0.0) / 2.0);
    let content = rect.shrink2(Vec2::new(padding, 0.0));
    let width = shortcut_width.max(0.0).min(content.width().max(0.0) * 0.45);
    let gap = if width > 0.0 {
        8.0_f32.min((content.width() - width).max(0.0))
    } else {
        0.0
    };
    let shortcut = Rect::from_min_max(
        Pos2::new(content.right() - width, content.top()),
        content.max,
    );
    let label = Rect::from_min_max(
        content.min,
        Pos2::new(
            (shortcut.left() - gap).max(content.left()),
            content.bottom(),
        ),
    );
    RowColumns { label, shortcut }
}

fn command_row(ui: &mut egui::Ui, entry: &Entry, selected: bool, height: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::click(),
    );
    if selected || response.hovered() || response.has_focus() {
        let visuals = ui.style().interact_selectable(&response, selected);
        ui.painter().rect_filled(rect, 2.0, visuals.bg_fill);
    }
    let color = if entry.enabled {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let shortcut = entry.shortcut.as_ref().map(|text| {
        ui.painter().layout_no_wrap(
            text.clone(),
            egui::TextStyle::Small.resolve(ui.style()),
            color,
        )
    });
    let columns = row_columns(
        rect,
        shortcut.as_ref().map_or(0.0, |galley| galley.size().x),
    );
    if columns.label.width() > 0.0 {
        let mut job = egui::text::LayoutJob::simple_singleline(entry.label.to_owned(), font, color);
        job.wrap.max_width = columns.label.width();
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = ui.painter().layout_job(job);
        let position = Pos2::new(
            columns.label.left(),
            columns.label.center().y - galley.size().y / 2.0,
        );
        ui.painter()
            .with_clip_rect(columns.label.intersect(ui.clip_rect()))
            .galley(position, galley, color);
    }
    if let Some(galley) = shortcut {
        let position = Pos2::new(
            columns.shortcut.right() - galley.size().x,
            columns.shortcut.center().y - galley.size().y / 2.0,
        );
        ui.painter()
            .with_clip_rect(columns.shortcut.intersect(ui.clip_rect()))
            .galley(position, galley, color);
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            selected,
            entry.label,
        )
    });
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable as _},
    };

    fn entries() -> Vec<Entry> {
        vec![
            Entry {
                action: ShortcutAction::Save,
                label: "Save",
                category: "File",
                shortcut: Some("⌘S".into()),
                enabled: true,
            },
            Entry {
                action: ShortcutAction::SaveAs,
                label: "Save As",
                category: "File",
                shortcut: Some("⇧⌘S".into()),
                enabled: true,
            },
            Entry {
                action: ShortcutAction::PreviewZoomIn,
                label: "Zoom Preview In",
                category: "Preview",
                shortcut: None,
                enabled: true,
            },
            Entry {
                action: ShortcutAction::CloseTab,
                label: "Close Tab",
                category: "File",
                shortcut: Some("⌘W".into()),
                enabled: false,
            },
        ]
    }

    fn harness(entries: Vec<Entry>, size: Vec2) -> Harness<'static, (State, Vec<Action>)> {
        Harness::builder().with_size(size).build_ui_state(
            move |ui, state: &mut (State, Vec<Action>)| {
                if let Some(action) = state.0.show(ui, &entries) {
                    state.1.push(action);
                }
            },
            (State::default(), Vec::new()),
        )
    }

    #[test]
    fn result_window_fits_eight_whole_rows_including_the_gaps() {
        for (row_height, spacing) in [(22.0, 2.0), (28.0, 4.0)] {
            let height = result_window_height(row_height, spacing, 1000.0);
            let last_row_bottom = 7.0 * (row_height + spacing) + row_height;
            assert_eq!(height, last_row_bottom);
            assert_eq!(result_window_height(row_height, spacing, 75.0), 75.0);
            assert_eq!(result_window_height(row_height, spacing, -5.0), 0.0);
        }
    }

    #[test]
    fn search_requires_every_term_and_prefers_exact_then_prefix_matches() {
        let entries = entries();
        let mut state = State {
            query: "FILE saVE".into(),
            ..Default::default()
        };
        state.refresh(&entries);
        assert_eq!(state.result_count(), 2);
        assert_eq!(state.selected_action(), Some(ShortcutAction::Save));
        assert_eq!(
            state.results.catalog[state.results.matched[1].entry].action,
            ShortcutAction::SaveAs
        );
        state.query = "prvw zm".into();
        state.refresh(&entries);
        assert_eq!(state.result_count(), 1);
        assert_eq!(state.selected_action(), Some(ShortcutAction::PreviewZoomIn));
        state.query = "file nonexistent".into();
        state.refresh(&entries);
        assert_eq!(state.result_count(), 0);
        assert_eq!(state.selected_action(), None);
        assert_eq!(term_rank("überblick", "über"), Some(15));
    }

    #[test]
    fn cache_only_rebuilds_for_query_or_catalog_changes_and_preserves_selection() {
        let mut entries = entries();
        let mut state = State::default();
        state.refresh(&entries);
        state.selected = 1;
        for _ in 0..1000 {
            state.refresh(&entries);
        }
        assert_eq!(state.results.rebuilds, 1);
        assert_eq!(state.result_actions().count(), entries.len());
        assert_eq!(state.enabled_actions().count(), 3);
        assert_eq!(
            state.results.rebuilds, 1,
            "inspection iterators must not rescore"
        );
        entries[0].enabled = false;
        state.refresh(&entries);
        assert_eq!(state.selected_action(), Some(ShortcutAction::SaveAs));
        assert_eq!(state.results.rebuilds, 2);
        state.query = "save".into();
        state.refresh(&entries);
        assert_eq!(state.selected_action(), Some(ShortcutAction::SaveAs));
        assert_eq!(state.results.rebuilds, 3);
        state.open();
        assert!(state.query.is_empty());
        assert!(state.focus_search && state.scroll_selection);
    }

    #[test]
    fn empty_and_disabled_results_never_fall_through_to_another_command() {
        let mut harness = harness(entries(), Vec2::new(420.0, 260.0));
        harness.run_steps(3);
        harness.state_mut().0.query = "Close".into();
        harness.run();
        assert!(
            harness
                .get_by_label("Close Tab")
                .accesskit_node()
                .is_disabled()
        );
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(harness.state().1.is_empty());
        harness.state_mut().0.query = "no matching command".into();
        harness.run();
        harness.get_by_label("No matching commands");
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(harness.state().1.is_empty());
        assert_eq!(step_selection(usize::MAX, 0, false), 0);
    }

    #[test]
    fn typing_autofocus_keyboard_selection_execution_and_escape_are_local() {
        let mut harness = harness(entries(), Vec2::new(420.0, 260.0));
        harness.run_steps(3);
        harness
            .input_mut()
            .events
            .push(egui::Event::Text("save".into()));
        harness.run();
        assert_eq!(harness.state().0.query, "save");
        harness.key_press(egui::Key::ArrowDown);
        harness.run();
        assert_eq!(
            harness.state().0.selected_action(),
            Some(ShortcutAction::SaveAs)
        );
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(harness.state().1, [Action::Execute(ShortcutAction::SaveAs)]);
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert_eq!(harness.state().1.last(), Some(&Action::Dismiss));
    }

    #[test]
    fn initial_query_and_arrow_navigation_skip_disabled_rows_without_reordering_them() {
        let mut entries = entries();
        entries[0].enabled = false;
        entries[2].enabled = false;
        entries[3].enabled = true;
        let mut state = State::default();
        state.refresh(&entries);
        assert_eq!(state.result_count(), 4);
        assert_eq!(state.result_actions().next(), Some(ShortcutAction::Save));
        assert_eq!(state.selected_action(), Some(ShortcutAction::SaveAs));
        for (down, expected) in [
            (true, ShortcutAction::CloseTab),
            (true, ShortcutAction::SaveAs),
            (false, ShortcutAction::CloseTab),
        ] {
            state.move_selection(down);
            assert_eq!(state.selected_action(), Some(expected));
        }
        state.query = "save".into();
        state.refresh(&entries);
        assert_eq!(
            state.selected, 1,
            "the exact but disabled Save remains visible above Save As"
        );
        state.move_selection(true);
        assert_eq!(state.selected, 1);
        for entry in &mut entries {
            entry.enabled = false;
        }
        state.refresh(&entries);
        assert_eq!(state.selected, 0);
        for down in [false, true] {
            state.move_selection(down);
            assert_eq!(state.selected, 0);
        }
        let mut harness = harness(entries, Vec2::new(420.0, 260.0));
        harness.run_steps(3);
        harness.key_press(egui::Key::ArrowDown);
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(harness.state().1.is_empty());
    }

    #[test]
    fn clicking_an_enabled_row_executes_that_command_once() {
        let mut harness = harness(entries(), Vec2::new(420.0, 260.0));
        harness.run_steps(3);
        harness.get_by_label("Save As").click();
        harness.run();
        assert_eq!(harness.state().1, [Action::Execute(ShortcutAction::SaveAs)]);
    }

    #[test]
    fn ime_preedit_and_commit_protect_candidate_keys_until_the_next_frame() {
        let mut state = State::default();
        assert!(
            state.composition_protects_keys(&[egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "ほぞん".into(),
                active_range_chars: None,
            })])
        );
        assert!(state.composition_protects_keys(&[]));
        assert!(
            state.composition_protects_keys(&[egui::Event::Ime(egui::ImeEvent::Commit(
                "保存".into()
            ))])
        );
        assert!(!state.composition_protects_keys(&[]));
    }

    #[test]
    fn row_columns_stay_inside_narrow_bounds_without_overlapping() {
        for width in [1.0, 12.0, 60.0, 170.0, 420.0] {
            let rect = Rect::from_min_size(Pos2::new(17.0, 29.0), Vec2::new(width, ROW_HEIGHT));
            for shortcut in [0.0, 30.0, 200.0] {
                let columns = row_columns(rect, shortcut);
                assert!(rect.contains_rect(columns.label));
                assert!(rect.contains_rect(columns.shortcut));
                assert!(columns.label.width() >= 0.0 && columns.shortcut.width() >= 0.0);
                assert!(columns.label.right() <= columns.shortcut.left());
            }
        }
    }

    #[test]
    fn narrow_long_rows_remain_fixed_height_when_hovered() {
        let mut entries = entries();
        entries[0].label = "Save a very long document under a different filename";
        entries[0].shortcut = Some("Control Shift Command Option S".into());
        let mut harness = harness(entries, Vec2::new(170.0, 240.0));
        harness.run_steps(3);
        let label = "Save a very long document under a different filename";
        let before = harness.get_by_label(label).rect();
        assert!(before.width() <= 170.0);
        assert_eq!(before.height(), ROW_HEIGHT);
        harness.get_by_label(label).hover();
        harness.run_steps(8);
        assert_eq!(harness.get_by_label(label).rect(), before);
        assert_eq!(harness.state().0.selected, 0);
    }

    #[test]
    fn wheel_scroll_persists_on_hover_and_keyboard_wrap_reveals_the_selection() {
        let entries: Vec<_> = ShortcutAction::ALL
            .into_iter()
            .map(|action| Entry {
                action,
                label: action.id(),
                category: "Commands",
                shortcut: None,
                enabled: true,
            })
            .collect();
        let first = entries[0].label;
        let last = entries.last().unwrap().label;
        let mut harness = harness(entries, Vec2::new(420.0, 260.0));
        harness.run_steps(3);
        harness.get_by_label(first).hover();
        harness.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: Vec2::new(0.0, -160.0),
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(24);
        let offset = harness.state().0.scroll_offset;
        assert!(offset > 40.0);
        harness.hover_at(Pos2::new(200.0, 190.0));
        harness.run_steps(24);
        assert!((harness.state().0.scroll_offset - offset).abs() < 0.1);
        assert_eq!(harness.state().0.selected, 0);
        harness.key_press(egui::Key::ArrowUp);
        harness.run_steps(24);
        assert_eq!(harness.state().0.selected, ShortcutAction::ALL.len() - 1);
        harness.get_by_label(last);
        harness.key_press(egui::Key::ArrowDown);
        harness.run_steps(24);
        assert_eq!(harness.state().0.selected, 0);
        harness.get_by_label(first);
        assert!(harness.state().0.scroll_offset < 0.1);
    }
}
