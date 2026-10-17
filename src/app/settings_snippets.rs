//! Source-first snippet editing. Invalid drafts never reach live completions.
use crate::{
    settings::AppSettings,
    snippets::{Language, Scope, Snippet},
};
use eframe::egui;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Origin {
    #[default]
    All,
    Preset,
    Custom,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Status {
    #[default]
    All,
    Enabled,
    Disabled,
}
#[derive(Clone, Default)]
struct Filter {
    query: String,
    language: Option<Language>,
    origin: Origin,
    status: Status,
    focus_requested: bool,
}
impl Filter {
    fn accepts(&self, snippet: &Snippet, preset: bool) -> bool {
        self.language.is_none_or(|language| {
            snippet.language == language || snippet.language == Language::Both
        }) && match self.origin {
            Origin::All => true,
            Origin::Preset => preset,
            Origin::Custom => !preset,
        } && match self.status {
            Status::All => true,
            Status::Enabled => snippet.enabled,
            Status::Disabled => !snippet.enabled,
        } && (self.query.trim().is_empty()
            || format!(
                "{} {} {}",
                snippet.prefix, snippet.description, snippet.body
            )
            .to_lowercase()
            .contains(&self.query.trim().to_lowercase()))
    }
}

fn fresh_editor_id() -> u64 {
    // Widget undo/cursor state follows the entry, never its current list index.
    // New/reverted collections get fresh identities even in the same viewport.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Identity includes the saved position and value, so duplicate prefixes and
/// stale settings actions cannot open a different entry by accident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    preset: bool,
    index: usize,
    snippet: Snippet,
}
#[derive(Default)]
pub(super) struct LibraryState {
    filter: Filter,
}

#[derive(Clone)]
pub(super) struct Draft {
    base: Vec<Snippet>,
    base_presets: Vec<Snippet>,
    entries: Vec<Snippet>,
    presets: Vec<Snippet>,
    // Delimiters may be temporarily invalid while typing. Remember the last
    // applied pair per entry so correcting it can still rewrite its markers.
    source_delimiters: Vec<(String, String)>,
    editor_ids: Vec<u64>,
    saved_entries: Vec<Option<Selection>>,
    selected: usize,
    reveal_selected: bool,
    filter: Filter,
    validation: Result<(), String>,
}
impl Draft {
    pub(super) fn new(entries: &[Snippet], presets: &[Snippet]) -> Self {
        Self {
            base: entries.to_vec(),
            base_presets: presets.to_vec(),
            entries: entries.to_vec(),
            presets: presets.to_vec(),
            source_delimiters: presets
                .iter()
                .chain(entries)
                .map(|s| (s.placeholder_open.clone(), s.placeholder_close.clone()))
                .collect(),
            editor_ids: (0..presets.len() + entries.len())
                .map(|_| fresh_editor_id())
                .collect(),
            saved_entries: saved_selections(entries, presets),
            selected: 0,
            reveal_selected: false,
            filter: Filter::default(),
            validation: crate::snippets::validate(presets)
                .and_then(|()| crate::snippets::validate(entries)),
        }
    }
    fn changed(&self) -> bool {
        self.entries != self.base || self.presets != self.base_presets
    }
    fn conflicts(&self, entries: &[Snippet], presets: &[Snippet]) -> bool {
        self.changed() && (self.base != entries || self.base_presets != presets)
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn selected_snippet(&self) -> Option<&Snippet> {
        self.get(self.selected)
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn query(&self) -> &str {
        &self.filter.query
    }
    pub(super) fn request_search_focus(&mut self) {
        self.filter.focus_requested = true;
    }
    pub(super) fn save(&mut self, settings: &mut AppSettings) -> bool {
        if !self.changed()
            || self.validation.is_err()
            || self.conflicts(&settings.snippets, &settings.snippet_presets)
        {
            return false;
        }
        settings.snippets.clone_from(&self.entries);
        settings.snippet_presets.clone_from(&self.presets);
        self.base.clone_from(&self.entries);
        self.base_presets.clone_from(&self.presets);
        self.saved_entries = saved_selections(&self.entries, &self.presets);
        true
    }
    pub(super) fn restore_saved_base(&mut self, settings: &AppSettings) {
        self.base.clone_from(&settings.snippets);
        self.base_presets.clone_from(&settings.snippet_presets);
    }
    pub(super) fn select_saved(&mut self, selection: &Selection) -> bool {
        let Some(index) = self
            .saved_entries
            .iter()
            .position(|saved| saved.as_ref() == Some(selection))
        else {
            return false;
        };
        self.selected = index;
        self.reveal_selected = true;
        self.filter = Filter::default();
        true
    }
    pub(super) fn refresh_saved(&mut self, settings: &AppSettings) {
        if (self.base != settings.snippets || self.base_presets != settings.snippet_presets)
            && !self.changed()
        {
            *self = Self::new(&settings.snippets, &settings.snippet_presets);
        }
    }
    pub(super) fn select_saved_if_current(
        &mut self,
        settings: &AppSettings,
        selection: &Selection,
    ) -> Result<(), &'static str> {
        self.refresh_saved(settings);
        if self.conflicts(&settings.snippets, &settings.snippet_presets) {
            return Err(
                "The saved library changed. Your editor draft is kept; Revert changes before opening this snippet.",
            );
        }
        if !self.select_saved(selection) {
            return Err(
                "This snippet was removed from your draft. Revert changes to open the saved entry.",
            );
        }
        Ok(())
    }
    fn validate(&mut self) {
        self.validation = crate::snippets::validate(&self.presets)
            .and_then(|()| crate::snippets::validate(&self.entries))
            .and_then(|()| {
                for (snippet, (opening, closing)) in self.all().zip(&self.source_delimiters) {
                    if snippet.placeholder_open != *opening || snippet.placeholder_close != *closing {
                        return Err(format!(
                            "Snippet “{}”: choose Rewrite markers or Keep source to apply the new delimiters",
                            snippet.prefix
                        ));
                    }
                }
                Ok(())
            });
    }
    fn all(&self) -> impl Iterator<Item = &Snippet> {
        self.presets.iter().chain(&self.entries)
    }
    fn len(&self) -> usize {
        self.presets.len() + self.entries.len()
    }
    fn get(&self, index: usize) -> Option<&Snippet> {
        if index < self.presets.len() {
            self.presets.get(index)
        } else {
            self.entries.get(index - self.presets.len())
        }
    }
    fn get_mut(&mut self, index: usize) -> Option<&mut Snippet> {
        if index < self.presets.len() {
            self.presets.get_mut(index)
        } else {
            self.entries.get_mut(index - self.presets.len())
        }
    }
    fn selected_mut(&mut self) -> Option<&mut Snippet> {
        self.get_mut(self.selected)
    }
    fn visible(&self) -> Vec<usize> {
        self.all()
            .enumerate()
            .filter_map(|(index, snippet)| {
                self.filter
                    .accepts(snippet, index < self.presets.len())
                    .then_some(index)
            })
            .collect()
    }
    fn unique_prefix(&self, prefix: &str) -> String {
        let mut unique = prefix.to_owned();
        let mut suffix = 2;
        while self.all().any(|entry| entry.prefix == unique) {
            unique = format!("{prefix}{suffix}");
            suffix += 1;
        }
        unique
    }
    fn add(&mut self, language: Language) {
        let (prefix, description, body) = match language {
            Language::Tex => (
                "env",
                "LaTeX environment",
                "\\begin{{{1:equation}}}\n  {{2:x = y}}\n\\end{{{1}}}{{0}}",
            ),
            _ => ("eq", "Display equation", "$ {{1:x}} = {{2:y}} $\n{{0}}"),
        };
        let prefix = self.unique_prefix(prefix);
        self.selected = self.len();
        self.reveal_selected = true;
        self.entries.push(Snippet {
            prefix,
            description: description.into(),
            language,
            scope: Scope::Text,
            line_start: true,
            body: body.into(),
            enabled: true,
            placeholder_open: crate::snippets::default_placeholder_open(),
            placeholder_close: crate::snippets::default_placeholder_close(),
        });
        self.source_delimiters.push((
            crate::snippets::default_placeholder_open(),
            crate::snippets::default_placeholder_close(),
        ));
        self.editor_ids.push(fresh_editor_id());
        self.saved_entries.push(None);
        self.filter = Filter::default();
        self.validate();
    }
    fn duplicate(&mut self) {
        let Some(mut entry) = self.get(self.selected).cloned() else {
            return;
        };
        entry.prefix = self.unique_prefix(&entry.prefix);
        let delimiters = self.source_delimiters[self.selected].clone();
        self.selected = self.len();
        self.reveal_selected = true;
        self.entries.push(entry);
        self.source_delimiters.push(delimiters);
        self.editor_ids.push(fresh_editor_id());
        self.saved_entries.push(None);
        self.filter = Filter::default();
        self.validate();
    }
    fn remove(&mut self) {
        if self.selected >= self.len() {
            return;
        }
        self.source_delimiters.remove(self.selected);
        self.editor_ids.remove(self.selected);
        self.saved_entries.remove(self.selected);
        if self.selected < self.presets.len() {
            self.presets.remove(self.selected);
        } else {
            self.entries.remove(self.selected - self.presets.len());
        }
        self.selected = self.selected.min(self.len().saturating_sub(1));
        self.validate();
    }
    fn restore(&mut self) {
        let custom_selection = if self.selected < self.len() {
            self.selected.checked_sub(self.presets.len())
        } else {
            None
        };
        let custom_delimiters = self.source_delimiters.split_off(self.presets.len());
        let custom_editor_ids = self.editor_ids.split_off(self.presets.len());
        let custom_saved_entries = self.saved_entries.split_off(self.presets.len());
        self.presets = crate::snippet_presets::defaults();
        self.source_delimiters = self
            .presets
            .iter()
            .map(|s| (s.placeholder_open.clone(), s.placeholder_close.clone()))
            .chain(custom_delimiters)
            .collect();
        self.editor_ids = (0..self.presets.len())
            .map(|_| fresh_editor_id())
            .chain(custom_editor_ids)
            .collect();
        self.saved_entries = saved_selections(&[], &self.presets)
            .into_iter()
            .chain(custom_saved_entries)
            .collect();
        self.selected = custom_selection.map_or(0, |index| self.presets.len() + index);
        self.validate();
    }
    fn apply_delimiters(&mut self) {
        let Some(entry) = self.get(self.selected) else {
            return;
        };
        match crate::snippets::rewrite_placeholders(
            &entry.body,
            &self.source_delimiters[self.selected].0,
            &self.source_delimiters[self.selected].1,
            &entry.placeholder_open,
            &entry.placeholder_close,
        ) {
            Ok(body) => {
                let delimiters = (
                    entry.placeholder_open.clone(),
                    entry.placeholder_close.clone(),
                );
                self.selected_mut().unwrap().body = body;
                self.source_delimiters[self.selected] = delimiters;
                self.editor_ids[self.selected] = fresh_editor_id();
                self.validate();
            }
            Err(error) => {
                self.validation = Err(format!("Snippet “{}”: {error}", entry.prefix));
            }
        }
    }
    fn keep_source(&mut self) {
        let Some(entry) = self.get(self.selected) else {
            return;
        };
        if let Err(error) = crate::snippets::validate(std::slice::from_ref(entry)) {
            self.validation = Err(error);
            return;
        }
        self.source_delimiters[self.selected] = (
            entry.placeholder_open.clone(),
            entry.placeholder_close.clone(),
        );
        self.editor_ids[self.selected] = fresh_editor_id();
        self.validate();
    }
}
fn observe(_name: &str, _response: &egui::Response) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(_name, _response);
}
fn language_label(language: Language) -> &'static str {
    match language {
        Language::Tex => "TeX",
        Language::Typst => "Typst",
        Language::Both => "Both",
    }
}
fn target(snippet: &Snippet, preset: bool) -> String {
    format!(
        "{}.{}.{}",
        if preset { "preset" } else { "custom" },
        match snippet.language {
            Language::Tex => "tex",
            Language::Typst => "typst",
            Language::Both => "both",
        },
        snippet.prefix
    )
}
fn filters(ui: &mut egui::Ui, filter: &mut Filter, namespace: &str) {
    ui.horizontal(|ui| {
        let label = ui.label("Search snippets");
        let search = ui
            .add(egui::TextEdit::singleline(&mut filter.query).desired_width(f32::INFINITY))
            .labelled_by(label.id);
        observe(&format!("{namespace}.search"), &search);
        if std::mem::take(&mut filter.focus_requested) {
            search.request_focus();
            search.scroll_to_me(Some(egui::Align::Min));
        }
    });
    filter_selectors(ui, filter, namespace, false);
}

fn filter_selector(
    ui: &mut egui::Ui,
    id: &'static str,
    width: f32,
    selected: &str,
    contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            egui::ComboBox::from_id_salt(id)
                .selected_text(selected)
                .width(width)
                .truncate()
                .show_ui(ui, contents)
                .response
        },
    )
    .inner
}

fn filter_selectors(ui: &mut egui::Ui, filter: &mut Filter, namespace: &str, compact: bool) {
    let available = ui
        .available_width()
        .min((ui.clip_rect().right() - ui.next_widget_position().x).max(0.0));
    let columns = if available >= 285.0 { 3.0 } else { 2.0 };
    let width = (available - ui.spacing().item_spacing.x * (columns - 1.0)) / columns;
    ui.horizontal_wrapped(|ui| {
        let language_label = filter.language.map_or("All languages", language_label);
        let display = if compact && filter.language.is_none() {
            "Language"
        } else {
            language_label
        };
        let language = filter_selector(
            ui,
            "snippet-language-filter",
            if compact { width } else { 110.0 },
            display,
            |ui| {
                for (value, label, target) in [
                    (None, "All languages", "all"),
                    (Some(Language::Typst), "Typst", "typst"),
                    (Some(Language::Tex), "TeX", "tex"),
                    (Some(Language::Both), "Both", "both"),
                ] {
                    let row = ui.selectable_value(&mut filter.language, value, label);
                    observe(&format!("{namespace}.filter.language.{target}"), &row);
                }
            },
        );
        language.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, language_label)
        });
        observe(&format!("{namespace}.filter.language"), &language);
        let origin_label = match filter.origin {
            Origin::All => "All origins",
            Origin::Preset => "Suggested",
            Origin::Custom => "Your snippets",
        };
        let display = match (compact, filter.origin) {
            (true, Origin::All) => "Origin",
            (true, Origin::Custom) => "Custom",
            _ => origin_label,
        };
        let origin = filter_selector(
            ui,
            "snippet-origin-filter",
            if compact { width } else { 110.0 },
            display,
            |ui| {
                for (value, label, target) in [
                    (Origin::All, "All origins", "all"),
                    (Origin::Preset, "Suggested", "preset"),
                    (Origin::Custom, "Your snippets", "custom"),
                ] {
                    let row = ui.selectable_value(&mut filter.origin, value, label);
                    observe(&format!("{namespace}.filter.origin.{target}"), &row);
                }
            },
        );
        origin.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, origin_label)
        });
        observe(&format!("{namespace}.filter.origin"), &origin);
        let status_label = match filter.status {
            Status::All => "All states",
            Status::Enabled => "Enabled",
            Status::Disabled => "Disabled",
        };
        let display = if compact && filter.status == Status::All {
            "State"
        } else {
            status_label
        };
        let status = filter_selector(
            ui,
            "snippet-status-filter",
            if compact { width } else { 100.0 },
            display,
            |ui| {
                for (value, label, target) in [
                    (Status::All, "All states", "all"),
                    (Status::Enabled, "Enabled", "enabled"),
                    (Status::Disabled, "Disabled", "disabled"),
                ] {
                    let row = ui.selectable_value(&mut filter.status, value, label);
                    observe(&format!("{namespace}.filter.status.{target}"), &row);
                }
            },
        );
        status.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, status_label)
        });
        observe(&format!("{namespace}.filter.status"), &status);
    });
}

fn saved_selections(entries: &[Snippet], presets: &[Snippet]) -> Vec<Option<Selection>> {
    [(true, presets), (false, entries)]
        .into_iter()
        .flat_map(|(preset, entries)| {
            entries.iter().enumerate().map(move |(index, snippet)| {
                Some(Selection {
                    preset,
                    index,
                    snippet: snippet.clone(),
                })
            })
        })
        .collect()
}

fn library_row_rects(row: egui::Rect, button: f32, gap: f32) -> [egui::Rect; 4] {
    let enabled = egui::Rect::from_min_size(row.min, egui::vec2(button, row.height()));
    let delete = egui::Rect::from_min_size(
        egui::pos2(row.right() - button, row.top()),
        egui::vec2(button, row.height()),
    );
    let edit = delete.translate(egui::vec2(-button - gap, 0.0));
    let text = egui::Rect::from_min_max(
        egui::pos2(enabled.right() + gap, row.top()),
        egui::pos2((edit.left() - gap).max(enabled.right() + gap), row.bottom()),
    );
    [enabled, text, edit, delete]
}
fn library_cell(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    rect: egui::Rect,
) -> egui::Ui {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.set_clip_rect(ui.clip_rect().intersect(rect));
    child
}

/// The saved library fills its Settings category; editing remains in the
/// independent editor and never replaces that window's unsaved draft.
pub(super) fn library(
    ui: &mut egui::Ui,
    state: &mut LibraryState,
    settings: &mut AppSettings,
) -> Option<Selection> {
    filters(ui, &mut state.filter, "settings.snippet_library");
    let mut edit = None;
    let mut remove = None;
    let filter = &state.filter;
    let visible: Vec<_> = [
        (true, &settings.snippet_presets),
        (false, &settings.snippets),
    ]
    .into_iter()
    .flat_map(|(preset, entries)| {
        entries
            .iter()
            .enumerate()
            .filter_map(move |(index, snippet)| {
                filter.accepts(snippet, preset).then_some((preset, index))
            })
    })
    .collect();
    ui.weak(format!("{} snippets", visible.len()));
    let row_height = ui.spacing().interact_size.y;
    let height = ui.available_height().max(row_height);
    egui::ScrollArea::vertical()
        .id_salt("settings-snippet-library")
        .max_height(height)
        .auto_shrink([false, false])
        .show_rows(ui, row_height, visible.len(), |ui, rows| {
            for row in rows {
                let (preset, index) = visible[row];
                let snippet = if preset {
                    &mut settings.snippet_presets[index]
                } else {
                    &mut settings.snippets[index]
                };
                let id = target(snippet, preset);
                let name = format!(
                    "{} snippet {} ({})",
                    language_label(snippet.language),
                    snippet.prefix,
                    if preset { "suggested" } else { "custom" }
                );
                let width = ui
                    .available_width()
                    .min((ui.clip_rect().right() - ui.next_widget_position().x).max(0.0));
                let (row, _) =
                    ui.allocate_exact_size(egui::vec2(width, row_height), egui::Sense::hover());
                let [enabled_rect, text_rect, edit_rect, delete_rect] =
                    library_row_rects(row, row_height, ui.spacing().item_spacing.x);
                let mut enabled_ui = library_cell(ui, (preset, index, "enabled"), enabled_rect);
                let enabled = enabled_ui.checkbox(&mut snippet.enabled, "");
                enabled.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Checkbox,
                        true,
                        snippet.enabled,
                        format!("Enable {name}"),
                    )
                });
                observe(&format!("settings.snippet_library.enabled.{id}"), &enabled);
                let mut text_ui = library_cell(ui, (preset, index, "text"), text_rect);
                let prefix_width = (text_rect.width() * 0.28).min(120.0);
                text_ui.add_sized(
                    [prefix_width, row_height],
                    egui::Label::new(egui::RichText::new(&snippet.prefix).monospace())
                        .halign(egui::Align::Min)
                        .truncate()
                        .show_tooltip_when_elided(false),
                );
                if text_rect.width() > 100.0 {
                    text_ui.add_sized(
                        [36.0, row_height],
                        egui::Label::new(
                            egui::RichText::new(language_label(snippet.language)).weak(),
                        )
                        .halign(egui::Align::Min)
                        .truncate()
                        .show_tooltip_when_elided(false),
                    );
                }
                if text_rect.width() > 160.0 {
                    let width = text_ui.available_width();
                    text_ui.add_sized(
                        [width, row_height],
                        egui::Label::new(egui::RichText::new(&snippet.description).weak())
                            .halign(egui::Align::Min)
                            .truncate()
                            .show_tooltip_when_elided(false),
                    );
                }
                let mut edit_ui = library_cell(ui, (preset, index, "edit"), edit_rect);
                let edit_button = super::icons::quiet_square_icon_button(
                    &mut edit_ui,
                    super::UiIcon::Spanner,
                    &format!("Edit {name}"),
                    row_height,
                    false,
                );
                observe(&format!("settings.snippet_library.edit.{id}"), &edit_button);
                if edit_button.clicked() {
                    edit = Some(Selection {
                        preset,
                        index,
                        snippet: snippet.clone(),
                    });
                }
                let mut delete_ui = library_cell(ui, (preset, index, "delete"), delete_rect);
                let delete = super::icons::quiet_square_icon_button(
                    &mut delete_ui,
                    super::UiIcon::Trash,
                    &format!("Delete {name}"),
                    row_height,
                    false,
                );
                observe(&format!("settings.snippet_library.delete.{id}"), &delete);
                if delete.clicked() {
                    remove = Some((preset, index));
                }
            }
        });
    if let Some((preset, index)) = remove {
        if preset {
            settings.snippet_presets.remove(index);
        } else {
            settings.snippets.remove(index);
        }
    }
    edit
}

#[derive(Clone, Copy, Debug)]
struct EditorRegions {
    library: egui::Rect,
    editor: egui::Rect,
    footer: egui::Rect,
    side_by_side: bool,
}

fn editor_regions(bounds: egui::Rect, row: f32, gap: f32) -> EditorRegions {
    let footer_height = (row + gap).min(bounds.height());
    let footer = egui::Rect::from_min_max(
        egui::pos2(bounds.left(), bounds.bottom() - footer_height),
        bounds.max,
    );
    let body = egui::Rect::from_min_max(
        bounds.min,
        egui::pos2(bounds.right(), (footer.top() - gap).max(bounds.top())),
    );
    let side_by_side = bounds.width() >= 720.0;
    let (library, editor) = if side_by_side {
        let width = (body.width() * 0.3).clamp(220.0, 300.0);
        (
            egui::Rect::from_min_max(body.min, egui::pos2(body.left() + width, body.bottom())),
            egui::Rect::from_min_max(egui::pos2(body.left() + width + gap, body.top()), body.max),
        )
    } else {
        let height = (body.height() * 0.3)
            .clamp(row * 5.0, row * 8.0)
            .min(body.height() * 0.45);
        (
            egui::Rect::from_min_max(body.min, egui::pos2(body.right(), body.top() + height)),
            egui::Rect::from_min_max(egui::pos2(body.left(), body.top() + height + gap), body.max),
        )
    };
    EditorRegions {
        library,
        editor,
        footer,
        side_by_side,
    }
}

fn editor_source_height(viewport: f32, metadata: f32, row: f32, line: f32) -> f32 {
    (viewport - metadata - row * 3.0).max(line * 5.0)
}

fn visible_source_rect(field: egui::Rect, viewport: egui::Rect, clip: egui::Rect) -> egui::Rect {
    field.intersect(viewport).intersect(clip)
}

#[cfg(test)]
pub(super) fn source_viewport(context: &egui::Context) -> Option<egui::Rect> {
    let id = egui::Id::new(("snippet-source-viewport", context.viewport_id()));
    context.data(|data| data.get_temp(id))
}

fn editor_filters(ui: &mut egui::Ui, filter: &mut Filter) {
    let search = ui.add(
        egui::TextEdit::singleline(&mut filter.query)
            .hint_text("Search snippets")
            .desired_width(f32::INFINITY),
    );
    search.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Search snippets")
    });
    observe("settings.snippets.search", &search);
    if std::mem::take(&mut filter.focus_requested) {
        search.request_focus();
    }
    // Reuse the existing filter selectors without a second search field.
    filter_selectors(ui, filter, "settings.snippets", true);
}

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut Option<Draft>,
    settings: &mut AppSettings,
    appearance: &AppSettings,
    _viewport_height: f32,
) {
    let draft =
        state.get_or_insert_with(|| Draft::new(&settings.snippets, &settings.snippet_presets));
    draft.refresh_saved(settings);
    let conflict = draft.conflicts(&settings.snippets, &settings.snippet_presets);
    let bounds = ui.available_rect_before_wrap();
    let gap = ui.spacing().item_spacing.y.max(4.0);
    let regions = editor_regions(bounds, ui.spacing().interact_size.y, gap);
    ui.allocate_rect(bounds, egui::Sense::hover());
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter().line_segment(
        [regions.footer.left_top(), regions.footer.right_top()],
        stroke,
    );
    let separator = if regions.side_by_side {
        let x = regions.library.right() + gap * 0.5;
        [
            egui::pos2(x, regions.library.top()),
            egui::pos2(x, regions.library.bottom()),
        ]
    } else {
        let y = regions.library.bottom() + gap * 0.5;
        [
            egui::pos2(regions.library.left(), y),
            egui::pos2(regions.library.right(), y),
        ]
    };
    ui.painter().line_segment(separator, stroke);
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("snippet-library-pane")
            .max_rect(regions.library),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(regions.library));
            editor_library(ui, draft);
        },
    );
    ui.scope_builder(egui::UiBuilder::new().id_salt("snippet-editor-pane").max_rect(regions.editor), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(regions.editor));
        egui::ScrollArea::vertical().id_salt("snippet-editor-properties").auto_shrink([false, false]).show(ui, |ui| {
            if conflict {
                ui.colored_label(ui.visuals().error_fg_color,
                    "Snippets were saved elsewhere. Your draft has been kept; Revert changes loads the saved collection.");
            }
            editor_detail(ui, draft, appearance, regions.editor.height());
        });
    });
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("snippet-footer")
            .max_rect(regions.footer),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(regions.footer));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = draft.changed() && draft.validation.is_ok() && !conflict;
                let save = ui.add_enabled(
                    enabled,
                    egui::Button::new("Save snippets").fill(ui.visuals().selection.bg_fill),
                );
                observe("settings.snippets.save", &save);
                if save.clicked() {
                    draft.save(settings);
                }
                let revert = ui.add_enabled(draft.changed(), egui::Button::new("Revert changes"));
                observe("settings.snippets.revert", &revert);
                if revert.clicked() {
                    *draft = Draft::new(&settings.snippets, &settings.snippet_presets);
                }
                let status = if conflict {
                    "Saved elsewhere"
                } else if draft.validation.is_err() {
                    "Invalid snippet"
                } else if draft.changed() {
                    "Unsaved changes"
                } else {
                    "Saved"
                };
                ui.add(
                    egui::Label::new(egui::RichText::new(status).weak())
                        .truncate()
                        .show_tooltip_when_elided(false),
                );
            });
        },
    );
}

fn editor_library(ui: &mut egui::Ui, draft: &mut Draft) {
    ui.horizontal_wrapped(|ui| {
        for (language, text, label, target) in [
            (Language::Typst, "+ Typst", "Add Typst snippet", "typst"),
            (Language::Tex, "+ TeX", "Add TeX snippet", "tex"),
        ] {
            let add = ui.button(text);
            add.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
            observe(&format!("settings.snippets.add.{target}"), &add);
            if add.clicked() {
                draft.add(language);
            }
        }
        let restore = ui.button("Restore");
        restore.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Restore suggestions")
        });
        observe("settings.snippets.restore", &restore);
        if restore.clicked() {
            draft.restore();
        }
    });
    editor_filters(ui, &mut draft.filter);
    let visible = draft.visible();
    let enabled = draft.all().filter(|snippet| snippet.enabled).count();
    ui.weak(format!(
        "{} / {} snippets · {enabled} enabled",
        visible.len(),
        draft.len()
    ));
    let row_height = ui.spacing().interact_size.y;
    let mut list = egui::ScrollArea::vertical()
        .id_salt("snippet-list")
        .max_height(ui.available_height().max(row_height))
        .auto_shrink([false, false]);
    if std::mem::take(&mut draft.reveal_selected)
        && let Some(row) = visible.iter().position(|index| *index == draft.selected)
    {
        list = list.vertical_scroll_offset(row as f32 * (row_height + ui.spacing().item_spacing.y));
    }
    list.show_rows(ui, row_height, visible.len(), |ui, rows| {
        for row_index in rows {
            let index = visible[row_index];
            let preset = index < draft.presets.len();
            let selected = draft.selected == index;
            let Some(snippet) = draft.get_mut(index) else {
                continue;
            };
            let id = target(snippet, preset);
            let mut row_changed = false;
            let mut select = false;
            ui.push_id(index, |ui| {
                let width = ui
                    .available_width()
                    .min((ui.clip_rect().right() - ui.next_widget_position().x).max(0.0));
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, row_height), egui::Sense::hover());
                let gap = ui.spacing().item_spacing.x;
                let checkbox =
                    egui::Rect::from_min_size(rect.min, egui::vec2(row_height, row_height));
                let text = egui::Rect::from_min_max(
                    egui::pos2((checkbox.right() + gap).min(rect.right()), rect.top()),
                    rect.max,
                );
                let mut enabled_ui = library_cell(ui, "enabled", checkbox);
                let enabled = enabled_ui.checkbox(&mut snippet.enabled, "");
                enabled.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Checkbox,
                        true,
                        snippet.enabled,
                        format!(
                            "Enable {} snippet {} ({})",
                            language_label(snippet.language),
                            snippet.prefix,
                            if preset { "suggested" } else { "custom" }
                        ),
                    )
                });
                observe(&format!("settings.snippets.enabled.{id}"), &enabled);
                row_changed = enabled.changed();
                // Exact hit geometry must stay inside the pane. Button's atomic
                // label layout can keep a long description's intrinsic width.
                let row = ui.interact(text, ui.id().with("select"), egui::Sense::click());
                let visuals = ui.style().interact_selectable(&row, selected);
                let painter = ui.painter().with_clip_rect(text.intersect(ui.clip_rect()));
                if selected || row.hovered() || row.has_focus() {
                    painter.rect_filled(text, visuals.corner_radius, visuals.bg_fill);
                }
                if row.has_focus() {
                    painter.rect_stroke(
                        text,
                        visuals.corner_radius,
                        ui.visuals().selection.stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                let padding = ui.spacing().button_padding.x;
                let mut job = egui::text::LayoutJob::simple(
                    format!(
                        "{} · {}  {}",
                        snippet.prefix,
                        language_label(snippet.language),
                        snippet.description
                    ),
                    egui::TextStyle::Button.resolve(ui.style()),
                    visuals.text_color(),
                    (text.width() - padding * 2.0).max(0.0),
                );
                job.wrap.max_rows = 1;
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                painter.galley(
                    egui::pos2(
                        text.left() + padding,
                        text.center().y - galley.size().y * 0.5,
                    ),
                    galley,
                    visuals.text_color(),
                );
                let label = format!(
                    "{} · {} · {}{}",
                    snippet.prefix,
                    language_label(snippet.language),
                    if preset {
                        "Suggested · "
                    } else {
                        "Your snippet · "
                    },
                    snippet.description
                );
                row.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        ui.is_enabled(),
                        selected,
                        &label,
                    )
                });
                observe(&format!("settings.snippets.select.{id}"), &row);
                select = row.clicked();
            });
            if select {
                draft.selected = index;
            }
            if row_changed {
                draft.validate();
            }
        }
    });
}

fn editor_detail(
    ui: &mut egui::Ui,
    draft: &mut Draft,
    appearance: &AppSettings,
    viewport_height: f32,
) {
    let content_top = ui.next_widget_position().y;
    if draft.len() == 0 {
        ui.weak("Add a snippet or restore the suggestions.");
        return;
    }
    draft.selected = draft.selected.min(draft.len() - 1);
    ui.horizontal_wrapped(|ui| {
        let entry = draft.get(draft.selected).unwrap();
        ui.strong(format!(
            "{} · {}",
            entry.prefix,
            if draft.selected < draft.presets.len() {
                "Suggested"
            } else {
                "Your snippet"
            }
        ));
        if !draft
            .filter
            .accepts(entry, draft.selected < draft.presets.len())
        {
            ui.weak("Selected snippet is outside these filters");
        }
        let duplicate = ui.button("Duplicate");
        duplicate.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Duplicate snippet")
        });
        observe("settings.snippets.duplicate", &duplicate);
        if duplicate.clicked() {
            draft.duplicate();
        }
        let remove = ui.button("Delete");
        remove.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Remove snippet")
        });
        observe("settings.snippets.remove", &remove);
        if remove.clicked() {
            draft.remove();
        }
    });
    let mut changed = false;
    let mut rewrite_markers = false;
    let mut keep_source = false;
    let selected = draft.selected;
    let editor_id = draft.editor_ids.get(selected).copied().unwrap_or_default();
    let applied_delimiters = draft
        .source_delimiters
        .get(selected)
        .cloned()
        .unwrap_or_default();
    if let Some(snippet) = draft.selected_mut() {
        ui.push_id(("snippet", editor_id), |ui| {
            ui.horizontal_wrapped(|ui| {
                let enabled = ui.checkbox(&mut snippet.enabled, "Enabled");
                observe("settings.snippets.enabled", &enabled);
                changed |= enabled.changed();
                let label = ui.label("Prefix");
                let prefix = ui
                    .add(
                        egui::TextEdit::singleline(&mut snippet.prefix)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(100.0),
                    )
                    .labelled_by(label.id);
                observe("settings.snippets.prefix", &prefix);
                changed |= prefix.changed();
                for (value, label) in [
                    (Language::Typst, "Typst"),
                    (Language::Tex, "TeX"),
                    (Language::Both, "Both"),
                ] {
                    changed |= ui
                        .selectable_value(&mut snippet.language, value, label)
                        .changed();
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Show in");
                for (value, label) in [
                    (Scope::Anywhere, "Anywhere"),
                    (Scope::Text, "Text"),
                    (Scope::Math, "Math"),
                    (Scope::Code, "Code"),
                ] {
                    let scope = ui.selectable_value(&mut snippet.scope, value, label);
                    observe(&format!("settings.snippets.scope.{label}"), &scope);
                    changed |= scope.changed();
                }
                let line = ui.checkbox(&mut snippet.line_start, "Start of line");
                observe("settings.snippets.line-start", &line);
                changed |= line.changed();
            });
            ui.horizontal(|ui| {
                let label = ui.label("Description");
                let description = ui
                    .add(
                        egui::TextEdit::singleline(&mut snippet.description)
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label.id);
                observe("settings.snippets.description", &description);
                changed |= description.changed();
            });
            ui.horizontal_wrapped(|ui| {
                let label = ui.label("Opening delimiter");
                let opening = ui
                    .add(
                        egui::TextEdit::singleline(&mut snippet.placeholder_open)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(60.0),
                    )
                    .labelled_by(label.id);
                observe("settings.snippets.opening", &opening);
                changed |= opening.changed();
                let label = ui.label("Closing delimiter");
                let closing = ui
                    .add(
                        egui::TextEdit::singleline(&mut snippet.placeholder_close)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(60.0),
                    )
                    .labelled_by(label.id);
                observe("settings.snippets.closing", &closing);
                changed |= closing.changed();
            });
            ui.monospace(format!(
                "{}1:value{} · {}1{} · {}0{}",
                snippet.placeholder_open,
                snippet.placeholder_close,
                snippet.placeholder_open,
                snippet.placeholder_close,
                snippet.placeholder_open,
                snippet.placeholder_close
            ));
            if snippet.placeholder_open != applied_delimiters.0
                || snippet.placeholder_close != applied_delimiters.1
            {
                ui.horizontal_wrapped(|ui| {
                    let rewrite = ui.button("Rewrite markers");
                    observe("settings.snippets.delimiters.rewrite", &rewrite);
                    rewrite_markers = rewrite.clicked();
                    let keep = ui.button("Keep source");
                    observe("settings.snippets.delimiters.keep-source", &keep);
                    keep_source = keep.clicked();
                });
            }
            let line = ui.fonts_mut(|fonts| fonts.row_height(&crate::theme::editor_font()));
            let source_height = editor_source_height(
                viewport_height,
                ui.next_widget_position().y - content_top,
                ui.spacing().interact_size.y,
                line,
            );
            let rows = (source_height / line).floor().max(5.0) as usize;
            let output = egui::ScrollArea::both()
                .id_salt("snippet-source-scroll")
                .max_height(source_height)
                .min_scrolled_height(0.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    super::code_editor::editor(
                        ui,
                        "snippet-body",
                        &mut snippet.body,
                        if snippet.language == Language::Tex {
                            "tex"
                        } else if snippet.scope == Scope::Code {
                            "typ-code"
                        } else {
                            "typ"
                        },
                        appearance,
                        rows,
                    )
                });
            let mut response = output.inner;
            let viewport = output.inner_rect.intersect(ui.clip_rect());
            response.interact_rect =
                visible_source_rect(response.interact_rect, viewport, ui.clip_rect());
            #[cfg(test)]
            {
                let id = egui::Id::new(("snippet-source-viewport", ui.ctx().viewport_id()));
                ui.ctx().data_mut(|data| data.insert_temp(id, viewport));
            }
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            if viewport.is_positive() {
                crate::desktop_test::observe_scroll(
                    "snippets.source",
                    ui.ctx(),
                    viewport,
                    output.content_size.y,
                    output.state.offset.y,
                    false,
                );
            }
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Snippet source")
            });
            if response.interact_rect.is_positive() {
                observe("settings.snippets.source", &response);
            }
            changed |= response.changed();
        });
    }
    if rewrite_markers {
        draft.apply_delimiters();
    } else if keep_source {
        draft.keep_source();
    } else if changed {
        draft.validate();
    }
    if let Err(error) = &draft.validation {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if let Some(snippet) = draft.get(draft.selected) {
        egui::CollapsingHeader::new("Expanded source").show(ui, |ui| {
            match crate::snippets::validate(std::slice::from_ref(snippet))
                .and_then(|()| snippet.source_to_lsp())
                .and_then(|source| crate::completion_edit::expand_lsp_snippet(&source))
            {
                Ok(expansion) => {
                    let mut source = expansion.text;
                    let response = super::code_editor::source_editor(
                        ui,
                        ("snippet-expanded", editor_id),
                        &mut source,
                        if snippet.language == Language::Tex {
                            "tex"
                        } else if snippet.scope == Scope::Code {
                            "typ-code"
                        } else {
                            "typ"
                        },
                        appearance,
                        super::code_editor::Options {
                            rows: 4,
                            editable: false,
                            ..Default::default()
                        },
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::TextEdit,
                            true,
                            "Expanded snippet source",
                        )
                    });
                }
                Err(error) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
        });
    }
    egui::CollapsingHeader::new("How snippets work").show(ui,|ui| {
        ui.label("Type a prefix, choose its completion, then Enter. Tab / Shift+Tab move between fields; Escape ends linked editing.");
        ui.label("Write ordinary Typst or TeX, with real line breaks. Dollars, quotes and backslashes are literal; no JSON escaping.");
        ui.monospace("{{1:name}}   {{1}}   {{2:value}}   {{0}}");
        ui.label("Use the same placeholders in Typst and TeX. Numbered fields are visited in order; repeated numbers stay linked. Field 0 places the final cursor. Opening delimiter and Closing delimiter can be changed per snippet. Rewrite markers changes existing placeholders; Keep source leaves the source unchanged and reserves only the new pair.");
        ui.label("Math shortcuts appear in equations; text shortcuts can wrap inline math. Code means Typst code. Start of line allows leading indentation. Comments and raw/verbatim text are excluded.");
        ui.label("Multiline expansions keep the current indentation. TeX package imports are added when needed. Undo reverses insertion and imports together. Use separate numbered fields rather than nesting fields.");
        ui.label("Uncheck Enabled to keep a snippet temporarily out of completions. Save applies all draft changes; Revert discards them. Removing a suggestion removes it from the library; Restore suggestions restores the bundled library while keeping your snippets. Your enabled eligible snippet takes precedence over a suggestion with the same prefix.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::NodeT as _;
    use egui_kittest::{Harness, kittest::Queryable as _};

    type LibraryHarness = Harness<'static, (AppSettings, LibraryState, Vec<Selection>)>;
    fn library_form(settings: AppSettings, size: egui::Vec2) -> LibraryHarness {
        let mut configured = false;
        let mut harness = Harness::builder().with_size(size).build_ui_state(
            move |ui, state: &mut (AppSettings, LibraryState, Vec<Selection>)| {
                if !configured {
                    crate::theme::configure_test_fonts(ui.ctx());
                    configured = true;
                    return;
                }
                ui.style_mut().scroll_animation = egui::style::ScrollAnimation::none();
                if let Some(selection) = library(ui, &mut state.1, &mut state.0) {
                    state.2.push(selection);
                }
            },
            (settings, LibraryState::default(), Vec::new()),
        );
        harness.run();
        harness
    }

    #[test]
    fn settings_library_edits_toggles_and_deletes_the_exact_saved_entry() {
        let mut settings = AppSettings::default();
        let preset = settings.snippet_presets[0].clone();
        settings.snippet_presets = vec![preset.clone()];
        let mut custom = preset;
        custom.description = "custom version".into();
        settings.snippets = vec![custom];
        let mut harness = library_form(settings, egui::vec2(620.0, 560.0));
        harness
            .get_by_label("Enable TeX snippet mk (custom)")
            .click();
        harness.run();
        assert!(harness.state().0.snippet_presets[0].enabled);
        assert!(!harness.state().0.snippets[0].enabled);
        harness.get_by_label("Edit TeX snippet mk (custom)").click();
        harness.run();
        let selected = &harness.state().2[0];
        assert!(!selected.preset);
        assert_eq!(selected.index, 0);
        assert_eq!(selected.snippet, harness.state().0.snippets[0]);
        harness
            .get_by_label("Delete TeX snippet mk (suggested)")
            .click();
        harness.run();
        assert!(harness.state().0.snippet_presets.is_empty());
        assert_eq!(harness.state().0.snippets.len(), 1);
        harness
            .get_by_label("Delete TeX snippet mk (custom)")
            .click();
        harness.run();
        assert!(harness.state().0.snippets.is_empty());
        assert!(harness.query_by_label("0 snippets").is_some());
    }

    #[test]
    fn settings_library_fills_the_height_and_scrolls_to_custom_entries_without_moving_search() {
        let mut settings = AppSettings::default();
        let mut custom = settings.snippet_presets[0].clone();
        custom.prefix = "lastcustom".into();
        settings.snippets.push(custom);
        let mut harness = library_form(settings, egui::vec2(620.0, 600.0));
        let search = harness.get_by_label("Search snippets").rect();
        let first = harness
            .get_by_label("Edit TeX snippet mk (suggested)")
            .rect();
        let name = format!(
            "Edit TeX snippet {} (suggested)",
            harness.state().0.snippet_presets[12].prefix
        );
        let last_initial = harness.get_by_label(&name).rect();
        assert!(
            last_initial.bottom() <= 600.0,
            "more than five rows should be visible"
        );
        assert!(last_initial.top() - first.top() > 150.0);
        harness.hover_at(egui::pos2(300.0, 400.0));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -5000.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        harness.run_steps(30);
        let last = harness
            .get_by_label("Edit TeX snippet lastcustom (custom)")
            .rect();
        assert!(last.top() >= 0.0 && last.bottom() <= 600.0);
        assert_eq!(harness.get_by_label("Search snippets").rect(), search);
        harness
            .get_by_label("Edit TeX snippet lastcustom (custom)")
            .click();
        harness.run_steps(3);
        assert_eq!(harness.state().2[0].snippet.prefix, "lastcustom");
    }

    #[test]
    fn narrow_settings_library_retains_both_row_actions_for_long_labels() {
        let mut settings = AppSettings::default();
        settings.snippet_presets.truncate(1);
        settings.snippet_presets[0].prefix = "long".repeat(30);
        settings.snippet_presets[0].description = "long description ".repeat(100);
        let name = format!(
            "TeX snippet {} (suggested)",
            settings.snippet_presets[0].prefix
        );
        let harness = library_form(settings, egui::vec2(320.0, 420.0));
        for label in [format!("Edit {name}"), format!("Delete {name}")] {
            let rect = harness.get_by_label(&label).rect();
            assert!(
                rect.left() >= 0.0 && rect.right() <= 320.0,
                "{label}: {rect:?}"
            );
        }
    }

    #[test]
    fn selecting_saved_entry_preserves_draft_text_and_identity_after_other_rows_are_removed() {
        let mut settings = AppSettings::default();
        settings.snippet_presets.truncate(3);
        let selection = saved_selections(&settings.snippets, &settings.snippet_presets)[2]
            .clone()
            .unwrap();
        let mut draft = Draft::new(&settings.snippets, &settings.snippet_presets);
        let id = draft.editor_ids[2];
        draft.presets[2].body = "unfinished edit".into();
        draft.selected = 0;
        draft.remove();
        draft
            .select_saved_if_current(&settings, &selection)
            .unwrap();
        assert_eq!(draft.selected, 1);
        assert_eq!(draft.editor_ids[1], id);
        assert_eq!(draft.get(1).unwrap().body, "unfinished edit");
        assert!(draft.changed());
        settings.snippet_presets[0].enabled = false;
        assert!(
            draft
                .select_saved_if_current(&settings, &selection)
                .is_err()
        );
        assert_eq!(draft.get(1).unwrap().body, "unfinished edit");
        assert_eq!(draft.editor_ids[1], id);
    }

    #[test]
    fn clean_editor_refreshes_before_selecting_a_saved_custom_entry_and_rejects_stale_requests() {
        let mut settings = AppSettings::default();
        let mut draft = Draft::new(&[], &[]);
        let custom = settings.snippet_presets[0].clone();
        settings.snippets.push(custom);
        let selection = saved_selections(&settings.snippets, &settings.snippet_presets)
            .pop()
            .unwrap()
            .unwrap();
        draft
            .select_saved_if_current(&settings, &selection)
            .unwrap();
        assert_eq!(draft.selected, settings.snippet_presets.len());
        assert_eq!(draft.get(draft.selected), Some(&selection.snippet));
        settings.snippets.clear();
        assert!(
            draft
                .select_saved_if_current(&settings, &selection)
                .is_err()
        );
        assert!(draft.entries.is_empty());
    }

    #[test]
    fn editor_regions_keep_panes_separate_and_save_footer_inside_every_window() {
        for size in [
            egui::vec2(420.0, 360.0),
            egui::vec2(700.0, 600.0),
            egui::vec2(820.0, 700.0),
            egui::vec2(1400.0, 900.0),
        ] {
            let bounds = egui::Rect::from_min_size(egui::pos2(8.0, 32.0), size);
            let regions = editor_regions(bounds, 24.0, 6.0);
            for pane in [regions.library, regions.editor, regions.footer] {
                assert!(
                    pane.is_finite() && pane.is_positive() && bounds.contains_rect(pane),
                    "{regions:?}"
                );
            }
            assert!(!regions.library.intersects(regions.editor), "{regions:?}");
            assert!(!regions.library.intersects(regions.footer), "{regions:?}");
            assert!(!regions.editor.intersects(regions.footer), "{regions:?}");
            assert_eq!(regions.footer.bottom(), bounds.bottom());
            if regions.side_by_side {
                assert!(regions.library.right() < regions.editor.left());
                assert_eq!(regions.library.height(), regions.editor.height());
                assert!(regions.editor.width() >= 490.0);
            } else {
                assert!(regions.library.bottom() < regions.editor.top());
                assert_eq!(regions.library.width(), regions.editor.width());
            }
        }
    }

    #[test]
    fn long_source_hit_geometry_uses_only_its_visible_viewport_above_the_footer() {
        let field = egui::Rect::from_min_max(egui::pos2(300.0, 160.0), egui::pos2(810.0, 5000.0));
        let viewport = egui::Rect::from_min_max(egui::pos2(280.0, 160.0), egui::pos2(820.0, 480.0));
        let clip = egui::Rect::from_min_max(egui::pos2(280.0, 30.0), egui::pos2(820.0, 520.0));
        let visible = visible_source_rect(field, viewport, clip);
        assert_eq!(visible.bottom(), 480.0);
        assert!(viewport.contains_rect(visible));
        assert!(visible.center().y < 480.0);
    }

    #[test]
    fn source_height_uses_finite_viewport_space_and_reserves_controls() {
        assert_eq!(editor_source_height(600.0, 180.0, 24.0, 16.0), 348.0);
        assert_eq!(editor_source_height(220.0, 180.0, 24.0, 16.0), 80.0);
        assert!(
            editor_source_height(900.0, 180.0, 24.0, 16.0)
                > editor_source_height(600.0, 180.0, 24.0, 16.0)
        );
    }

    fn form(settings: AppSettings) -> Harness<'static, (AppSettings, Option<Draft>)> {
        form_size(settings, 860.0)
    }
    fn form_size(
        settings: AppSettings,
        width: f32,
    ) -> Harness<'static, (AppSettings, Option<Draft>)> {
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(width, 1100.0))
            .build_ui_state(
                move |ui, state: &mut (AppSettings, Option<Draft>)| {
                    if !configured {
                        crate::theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        return;
                    }
                    let appearance = state.0.clone();
                    let height = ui.available_height();
                    show(ui, &mut state.1, &mut state.0, &appearance, height);
                },
                (settings, None),
            );
        harness.run();
        harness
    }
    fn replace(
        harness: &mut Harness<'static, (AppSettings, Option<Draft>)>,
        label: &str,
        value: &str,
    ) {
        harness.get_by_label(label).click();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run();
        harness.get_by_label(label).type_text(value);
        harness.run();
    }
    #[test]
    fn suggestions_are_editable_removable_and_restorable_without_losing_custom_entries() {
        let mut settings = AppSettings::default();
        let mut custom = settings.snippet_presets[0].clone();
        custom.prefix = "mine".into();
        settings.snippets.push(custom.clone());
        let count = settings.snippet_presets.len();
        let mut harness = form(settings);
        replace(&mut harness, "Prefix", "inline");
        harness.get_by_label("Math").click();
        harness.get_by_label("Start of line").click();
        harness.run();
        assert_eq!(harness.state().0.snippet_presets[0].prefix, "mk");
        harness.get_by_label("Save snippets").click();
        harness.run();
        let saved = &harness.state().0.snippet_presets[0];
        assert_eq!(saved.prefix, "inline");
        assert_eq!(saved.scope, Scope::Math);
        assert!(saved.line_start);
        harness.get_by_label("Remove snippet").click();
        harness.run();
        harness.get_by_label("Save snippets").click();
        harness.run();
        assert_eq!(harness.state().0.snippet_presets.len(), count - 1);
        assert_eq!(harness.state().0.snippets, vec![custom.clone()]);
        harness.get_by_label("Restore suggestions").click();
        harness.run();
        harness.get_by_label("Save snippets").click();
        harness.run();
        assert_eq!(
            harness.state().0.snippet_presets,
            crate::snippet_presets::defaults()
        );
        assert_eq!(harness.state().0.snippets, vec![custom]);
    }
    #[test]
    fn source_form_edits_and_saves_tex_without_json_and_rejects_invalid_drafts() {
        let mut harness = form(AppSettings::default());
        harness.get_by_label("Add TeX snippet").click();
        harness.run();
        assert!(harness.state().0.snippets.is_empty());
        let source = "\\begin{{{1:align}}}\n  $x$ & \\text{{{2:hello}}} \\\\\n\\end{{{1}}}{{0}}";
        replace(&mut harness, "Snippet source", source);
        harness.get_by_label("Save snippets").click();
        harness.run();
        assert_eq!(harness.state().0.snippets[0].body, source);
        assert_eq!(harness.state().0.snippets[0].language, Language::Tex);
        let wire = harness.state().0.snippets[0].source_to_lsp().unwrap();
        assert_eq!(
            crate::completion_edit::expand_lsp_snippet(&wire)
                .unwrap()
                .text,
            "\\begin{align}\n  $x$ & \\text{hello} \\\\\n\\end{align}"
        );
        replace(&mut harness, "Prefix", "");
        assert!(
            harness
                .get_by_label("Save snippets")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("Revert changes").click();
        harness.run();
        assert_eq!(
            harness.state().1.as_ref().unwrap().entries,
            harness.state().0.snippets
        );
    }
    #[test]
    fn manager_disables_duplicates_and_reverts_without_mutating_live_settings() {
        let mut harness = form(AppSettings::default());
        let original = harness.state().0.snippet_presets[0].clone();
        harness.get_by_label("Enabled").click();
        harness.run();
        assert!(harness.state().0.snippet_presets[0].enabled);
        harness.get_by_label("Duplicate snippet").click();
        harness.run();
        let draft = harness.state().1.as_ref().unwrap();
        assert_eq!(draft.entries.len(), 1);
        assert!(!draft.entries[0].enabled);
        assert_ne!(draft.entries[0].prefix, original.prefix);
        harness.get_by_label("Save snippets").click();
        harness.run();
        assert!(!harness.state().0.snippet_presets[0].enabled);
        assert_eq!(harness.state().0.snippets.len(), 1);
        replace(&mut harness, "Description", "Unsaved description");
        replace(&mut harness, "Search snippets", "no-such-snippet");
        assert!(
            harness
                .query_by_label("Selected snippet is outside these filters")
                .is_some()
        );
        assert_eq!(
            harness.state().1.as_ref().unwrap().selected,
            harness.state().0.snippet_presets.len()
        );
        harness.get_by_label("Revert changes").click();
        harness.run();
        assert_eq!(
            harness.state().1.as_ref().unwrap().entries,
            harness.state().0.snippets
        );
    }
    #[test]
    fn searchable_rows_select_and_disable_any_snippet_without_applying_unsaved_changes() {
        let mut settings = AppSettings::default();
        let mut entry = settings.snippet_presets[0].clone();
        settings.snippet_presets.clear();
        entry.prefix = "alpha".into();
        settings.snippets.push(entry.clone());
        entry.prefix = "beta".into();
        entry.language = Language::Tex;
        entry.body = "literal".into();
        settings.snippets.push(entry);
        let mut harness = form(settings);
        replace(&mut harness, "Search snippets", "beta");
        harness
            .get_by_label_contains("beta · TeX · Your snippet")
            .click();
        harness.run();
        assert_eq!(harness.state().1.as_ref().unwrap().selected, 1);
        harness
            .get_by_label("Enable TeX snippet beta (custom)")
            .click();
        harness.run();
        assert!(harness.state().0.snippets[1].enabled);
        assert!(!harness.state().1.as_ref().unwrap().entries[1].enabled);
        harness.get_by_label("Save snippets").click();
        harness.run();
        assert!(!harness.state().0.snippets[1].enabled);
        assert!(harness.state().0.snippets[0].enabled);
    }
    #[test]
    fn narrow_manager_keeps_its_controls_and_source_inside_the_settings_width() {
        let harness = form_size(AppSettings::default(), 420.0);
        for label in [
            "Search snippets",
            "Prefix",
            "Opening delimiter",
            "Closing delimiter",
            "Snippet source",
            "Save snippets",
            "Remove snippet",
        ] {
            let rect = harness.get_by_label(label).rect();
            assert!(
                rect.left() >= 0.0 && rect.right() <= 420.0,
                "{label}: {rect:?}"
            );
        }
    }
    #[test]
    fn delimiter_choices_are_explicit_and_keep_source_retains_literal_original_markers() {
        for language in [Language::Tex, Language::Typst, Language::Both] {
            let source = r"{{1:x}} #snip(1) \snip{1}{x} $x$";
            let mut draft = Draft::new(&[], &[]);
            draft.add(language);
            draft.entries[0].body = source.into();
            draft.entries[0].placeholder_open = "[[".into();
            draft.entries[0].placeholder_close = "]]".into();
            draft.validate();
            assert!(
                draft.validation.is_err(),
                "new delimiters require an explicit choice"
            );
            assert_eq!(draft.entries[0].body, source);
            draft.keep_source();
            assert!(draft.validation.is_ok());
            let wire = draft.entries[0].source_to_lsp().unwrap();
            assert_eq!(
                crate::completion_edit::expand_lsp_snippet(&wire)
                    .unwrap()
                    .text,
                source
            );
        }
        let mut harness = form(AppSettings::default());
        replace(&mut harness, "Opening delimiter", "[[");
        replace(&mut harness, "Closing delimiter", "]]");
        assert!(
            harness
                .get_by_label("Save snippets")
                .accesskit_node()
                .is_disabled()
        );
        let body = harness.state().1.as_ref().unwrap().presets[0].body.clone();
        assert!(body.contains("{{"));
        harness.get_by_label("Rewrite markers").click();
        harness.run();
        assert!(
            harness.state().1.as_ref().unwrap().presets[0]
                .body
                .contains("[[1:")
        );
        assert!(
            !harness
                .get_by_label("Save snippets")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("Revert changes").click();
        harness.run();
        replace(&mut harness, "Opening delimiter", "[[");
        replace(&mut harness, "Closing delimiter", "]]");
        harness.get_by_label("Keep source").click();
        harness.run();
        assert_eq!(harness.state().1.as_ref().unwrap().presets[0].body, body);
        assert!(
            !harness
                .get_by_label("Save snippets")
                .accesskit_node()
                .is_disabled()
        );
    }
    #[test]
    fn delimiter_edits_rewrite_only_fields_and_invalid_intermediate_pairs_are_recoverable() {
        let mut draft = Draft::new(&[], &[]);
        draft.add(Language::Tex);
        let source = r"$${1:literal}$ {{1:x}} {{1}} {{0}} \snip{1}{x} #snip(1)";
        draft.entries[0].body = source.into();
        draft.entries[0].placeholder_open = "letters".into();
        draft.entries[0].placeholder_close = "]]".into();
        draft.apply_delimiters();
        assert!(draft.validation.is_err());
        assert_eq!(draft.entries[0].body, source);
        draft.entries[0].placeholder_open = "[[".into();
        draft.apply_delimiters();
        assert!(draft.validation.is_ok());
        assert_eq!(
            draft.entries[0].body,
            r"$${1:literal}$ [[1:x]] [[1]] [[0]] \snip{1}{x} #snip(1)"
        );
        let wire = draft.entries[0].source_to_lsp().unwrap();
        assert_eq!(
            crate::completion_edit::expand_lsp_snippet(&wire)
                .unwrap()
                .text,
            r"$${1:literal}$ x x  \snip{1}{x} #snip(1)"
        );
    }
    #[test]
    fn expanded_preview_rejects_nested_fields_and_keeps_the_draft_editable() {
        let mut settings = AppSettings::default();
        settings.snippet_presets.truncate(1);
        settings.snippet_presets[0].body = "{{1:outer {{2:inner}}}} {{1}}{{0}}".into();
        let mut harness = form(settings);
        harness.get_by_label("Expanded source").click();
        harness.run_steps(30);
        assert!(harness.query_by_label("Expanded snippet source").is_none());
        assert!(
            harness
                .get_by_label("Save snippets")
                .accesskit_node()
                .is_disabled()
        );
        replace(
            &mut harness,
            "Snippet source",
            "{{1:replacement}} {{1}}{{0}}",
        );
        assert_eq!(
            harness
                .get_by_label("Expanded snippet source")
                .value()
                .as_deref(),
            Some("replacement replacement")
        );
        assert!(
            !harness
                .get_by_label("Save snippets")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("Save snippets").click();
        harness.run();
        assert_eq!(
            harness.state().0.snippet_presets[0].body,
            "{{1:replacement}} {{1}}{{0}}"
        );
    }

    #[test]
    fn typed_source_and_delimiters_survive_external_library_conflicts_without_overwriting() {
        let mut harness = form(AppSettings::default());
        replace(
            &mut harness,
            "Snippet source",
            r"\frac{{{1:left}}}{{{2:right}}}{{0}}",
        );
        replace(&mut harness, "Opening delimiter", "⟪");
        replace(&mut harness, "Closing delimiter", "⟫");
        harness.get_by_label("Rewrite markers").click();
        harness.run();
        let draft_entry = harness.state().1.as_ref().unwrap().presets[0].clone();
        assert_eq!(draft_entry.body, r"\frac{⟪1:left⟫}{⟪2:right⟫}⟪0⟫");
        assert_eq!(draft_entry.placeholder_open, "⟪");
        assert_eq!(draft_entry.placeholder_close, "⟫");
        harness.state_mut().0.snippet_presets[0].description = "Saved elsewhere".into();
        harness.run();
        assert!(
            harness
                .get_by_label("Save snippets")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("Save snippets").click();
        harness.run();
        assert_eq!(harness.state().1.as_ref().unwrap().presets[0], draft_entry);
        assert_eq!(
            harness.state().0.snippet_presets[0].description,
            "Saved elsewhere"
        );
        harness.get_by_label("Revert changes").click();
        harness.run();
        let reloaded = &harness.state().1.as_ref().unwrap().presets[0];
        assert_eq!(reloaded.description, "Saved elsewhere");
        assert_eq!(reloaded.placeholder_open, "{{");
        assert_eq!(reloaded.placeholder_close, "}}");
    }
    #[test]
    fn external_saved_changes_block_overwrite_keep_the_draft_and_revert_to_current_settings() {
        let mut harness = form(AppSettings::default());
        replace(&mut harness, "Prefix", "formdraft");
        harness.state_mut().0.snippet_presets[0].prefix = "jsonsaved".into();
        harness.run();
        let draft = harness.state().1.as_ref().unwrap();
        assert_eq!(draft.presets[0].prefix, "formdraft");
        assert!(draft.conflicts(
            &harness.state().0.snippets,
            &harness.state().0.snippet_presets
        ));
        assert!(
            harness
                .get_by_label("Save snippets")
                .accesskit_node()
                .is_disabled()
        );
        assert!(harness.query_by_label("Snippets were saved elsewhere. Your draft has been kept; Revert changes loads the saved collection.").is_some());
        harness.get_by_label("Save snippets").click();
        harness.run();
        assert_eq!(harness.state().0.snippet_presets[0].prefix, "jsonsaved");
        assert_eq!(
            harness.state().1.as_ref().unwrap().presets[0].prefix,
            "formdraft"
        );
        harness.get_by_label("Revert changes").click();
        harness.run();
        let draft = harness.state().1.as_ref().unwrap();
        assert_eq!(draft.presets, harness.state().0.snippet_presets);
        assert_eq!(draft.presets[0].prefix, "jsonsaved");
        assert!(!draft.changed());
        assert!(!draft.conflicts(
            &harness.state().0.snippets,
            &harness.state().0.snippet_presets
        ));
    }
    fn assert_external_replacement_cannot_undo_another_entry(action: &str) {
        let mut settings = AppSettings::default();
        settings.snippet_presets.truncate(2);
        for (index, entry) in settings.snippet_presets.iter_mut().enumerate() {
            entry.prefix = ["first", "second"][index].into();
            entry.body = ["first source", "second source"][index].into();
        }
        let mut harness = form(settings);
        replace(&mut harness, "Prefix", "editedfirst");
        replace(&mut harness, "Snippet source", "edited first source");
        harness.get_by_label(action).click();
        harness.run();
        let expected = harness.state().1.as_ref().unwrap().get(0).unwrap().clone();
        harness.get_by_label("Prefix").click();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        harness.run();
        harness.get_by_label("Snippet source").click();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        harness.run();
        assert_eq!(
            harness.state().1.as_ref().unwrap().get(0).unwrap(),
            &expected,
            "{action}"
        );
    }
    #[test]
    fn rewriting_delimiters_cannot_undo_only_the_source_half_of_the_change() {
        let mut harness = form(AppSettings::default());
        replace(&mut harness, "Snippet source", "{{1:expression}} {{0}}");
        replace(&mut harness, "Opening delimiter", "[[");
        replace(&mut harness, "Closing delimiter", "]]");
        harness.get_by_label("Rewrite markers").click();
        harness.run();
        let expected = harness.state().1.as_ref().unwrap().get(0).unwrap().clone();
        assert!(expected.body.contains("[[1:expression]]"));
        harness.get_by_label("Snippet source").click();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        harness.run();
        assert_eq!(
            harness.state().1.as_ref().unwrap().get(0).unwrap(),
            &expected
        );
    }
    #[test]
    fn removing_a_snippet_cannot_undo_its_text_into_the_neighbor() {
        assert_external_replacement_cannot_undo_another_entry("Remove snippet");
    }
    #[test]
    fn reverting_snippets_cannot_undo_the_discarded_source_back() {
        assert_external_replacement_cannot_undo_another_entry("Revert changes");
    }
    #[test]
    fn restoring_suggestions_cannot_undo_an_old_entries_text_into_the_library() {
        assert_external_replacement_cannot_undo_another_entry("Restore suggestions");
    }
    #[test]
    fn surviving_snippets_keep_editor_identity_while_restored_entries_get_fresh_identity() {
        let mut draft = Draft::new(&[], &crate::snippet_presets::defaults());
        let second = draft.editor_ids[1];
        draft.remove();
        assert_eq!(draft.editor_ids[0], second);
        draft.add(Language::Tex);
        let custom = draft.editor_ids[draft.selected];
        draft.restore();
        assert_ne!(draft.editor_ids[0], second);
        assert_eq!(draft.editor_ids[draft.selected], custom);
        draft.duplicate();
        assert_ne!(draft.editor_ids[draft.selected], custom);
        assert_eq!(
            draft
                .editor_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            draft.len()
        );
    }
    #[test]
    fn language_filters_include_shared_snippets_but_both_filter_is_specific() {
        let mut draft = Draft::new(&[], &[]);
        draft.add(Language::Typst);
        draft.add(Language::Tex);
        draft.duplicate();
        draft.entries[2].language = Language::Both;
        draft.filter.language = Some(Language::Typst);
        assert_eq!(draft.visible(), vec![0, 2]);
        draft.filter.language = Some(Language::Tex);
        assert_eq!(draft.visible(), vec![1, 2]);
        draft.filter.language = Some(Language::Both);
        assert_eq!(draft.visible(), vec![2]);
    }
    #[test]
    fn filtering_keeps_selected_invalid_unsaved_snippets_and_removal_has_stable_neighbors() {
        let mut draft = Draft::new(&[], &[]);
        draft.add(Language::Typst);
        draft.add(Language::Tex);
        draft.entries[1].description = "second".into();
        draft.entries[1].prefix.clear();
        draft.validate();
        assert!(draft.validation.is_err());
        draft.filter.query = "Display equation".into();
        assert_eq!(draft.visible(), vec![0]);
        assert_eq!(draft.selected, 1);
        assert!(draft.changed());
        assert!(draft.entries[1].prefix.is_empty());
        draft.filter = Filter {
            language: Some(Language::Tex),
            origin: Origin::Custom,
            status: Status::Disabled,
            ..Default::default()
        };
        assert!(draft.visible().is_empty());
        draft.entries[1].enabled = false;
        assert_eq!(draft.visible(), vec![1]);
        draft.remove();
        assert_eq!(draft.selected, 0);
        assert_eq!(draft.entries[0].language, Language::Typst);
        draft.remove();
        draft.remove();
        assert_eq!(draft.len(), 0);
        assert_eq!(draft.selected, 0);
        draft.restore();
        assert_eq!(draft.selected, 0);
        assert!(draft.get(draft.selected).is_some());
    }
    #[test]
    fn restoring_presets_keeps_custom_selection_and_delimiter_rewrite_history() {
        let mut draft = Draft::new(&[], &crate::snippet_presets::defaults());
        draft.add(Language::Tex);
        let selected = draft.selected;
        draft.entries[0].placeholder_open = "letters".into();
        draft.entries[0].placeholder_close = "]]".into();
        draft.apply_delimiters();
        draft.restore();
        assert_eq!(draft.selected, selected);
        draft.entries[0].placeholder_open = "[[".into();
        draft.apply_delimiters();
        assert!(draft.validation.is_ok());
        assert!(draft.entries[0].body.contains("[[1:"));
    }
    #[test]
    fn typst_example_expands_with_literal_math_and_unique_prefixes() {
        let mut draft = Draft::new(&[], &[]);
        draft.add(Language::Typst);
        draft.add(Language::Typst);
        assert_eq!(draft.entries[0].prefix, "eq");
        assert_eq!(draft.entries[1].prefix, "eq2");
        assert!(draft.validation.is_ok());
        let wire = draft.entries[0].source_to_lsp().unwrap();
        assert_eq!(
            crate::completion_edit::expand_lsp_snippet(&wire)
                .unwrap()
                .text,
            "$ x = y $\n"
        );
    }
}
