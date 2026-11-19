//! Cached Problems browsing. Filtering never mutates provider diagnostics.
mod filter;
#[cfg(test)]
mod scroll_tests;
#[cfg(test)]
mod tests;
use super::filter_bar::{ScrollLayout, initial_offset};
use super::*;
use crate::tool_profiles::{self, ToolProfile};
use filter::{Filter, Model};
use std::collections::HashSet;

#[derive(Clone, Copy)]
struct AutofixTarget {
    tab: Option<u64>,
    document: DocumentKey,
    rule: crate::tex::autofix::Rule,
}

pub(super) struct State {
    pub(super) filter: Filter,
    pub(super) model: Model,
    pub(super) focus_search: bool,
    pub(super) popup_open: bool,
    pub(super) grouped: bool,
    pub(super) controls_visible: bool,
    pub(super) sticky_filters: bool,
    pub(super) scroll_offset: f32,
    pub(super) scroll_rect: Rect,
    pub(super) controls_rect: Rect,
    filters_popup_open: bool,
    controls_height: f32,
    scroll_layout: Option<ScrollLayout>,
    autofix_menu_owner: Option<(egui::Id, DocumentKey)>,
    collapsed: HashSet<(String, DiagnosticSeverity)>,
    actions: Actions,
}

/// Resolve source ownership only on publication/revision changes, never while
/// painting every row. Applying an action validates its exact source again.
#[derive(Default)]
struct Actions {
    stamp: Option<(DocumentKey, u64, PathBuf)>,
    current: HashSet<usize>,
    automatic: HashSet<usize>,
    suggestions: HashSet<usize>,
    filter: Option<filter::Filter>,
    shown_fixes: Vec<usize>,
    #[cfg(test)]
    rebuilds: usize,
}
impl Default for State {
    fn default() -> Self {
        Self {
            filter: Filter::default(),
            model: Model::default(),
            focus_search: false,
            popup_open: false,
            grouped: false,
            controls_visible: false,
            sticky_filters: false,
            scroll_offset: 0.0,
            scroll_rect: Rect::NOTHING,
            controls_rect: Rect::NOTHING,
            filters_popup_open: false,
            controls_height: 0.0,
            scroll_layout: None,
            autofix_menu_owner: None,
            collapsed: HashSet::new(),
            actions: Actions::default(),
        }
    }
}

impl State {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn inspect(&self) -> serde_json::Value {
        let groups = self.model.groups().iter().map(|group| {
            let open = !self.collapsed.contains(&(group.provider.clone(), group.severity));
            serde_json::json!({"provider":group.provider,"type":format!("{:?}",group.severity),
                "count":group.indices.len(),"open":open,"target":group_target(&group.provider, group.severity)})
        }).collect::<Vec<_>>();
        let visible_indices = self.model.indices();
        let displayed_count = if self.grouped {
            self.model
                .groups()
                .iter()
                .filter(|g| !self.collapsed.contains(&(g.provider.clone(), g.severity)))
                .map(|g| g.indices.len())
                .sum::<usize>()
        } else {
            self.model.visible_count()
        };
        serde_json::json!({"query":self.filter.query,"source":self.filter.provider,
            "type":self.filter.severity.map(|s|format!("{s:?}")),"category":self.filter.category,
            "visible_count":self.model.visible_count(),"total_count":self.model.total_count(),
            "visible_indices":visible_indices,"displayed_count":displayed_count,"groups":groups,
            "grouped":self.grouped,"controls_visible":self.controls_visible,"sticky_filters":self.sticky_filters})
    }
}
pub(super) fn search_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "problems-search")
}
#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
fn group_target(provider: &str, severity: DiagnosticSeverity) -> String {
    format!("problems.group.{provider}.{severity:?}")
}
fn controls_contents(
    ui: &mut egui::Ui,
    state: &mut State,
    profile: Option<ToolProfile>,
    requested_profile: &mut Option<ToolProfile>,
) -> bool {
    let State {
        filter,
        model,
        focus_search,
        filters_popup_open,
        grouped,
        ..
    } = state;
    *filters_popup_open = false;
    let category_visible = !model.categories().is_empty() || filter.category.is_some();
    let field_width =
        (ui.available_width() - if category_visible { 536.0 } else { 412.0 }).max(110.0);
    let changed = ui
        .horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let mut refocus = false;
            let search = ui.add(
                egui::TextEdit::singleline(&mut filter.query)
                    .id(search_id(ui.ctx()))
                    .hint_text("Search problems")
                    .desired_width(field_width),
            );
            search.widget_info(|| {
                let mut info =
                    egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Search problems");
                info.current_text_value = Some(filter.query.clone());
                info
            });
            if *focus_search {
                search.request_focus();
                *focus_search = false;
            }
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("problems.search", &search);
            let source_popup = egui::ComboBox::from_id_salt("problems-source")
                .selected_text(filter.provider.as_deref().unwrap_or("All sources"))
                .width(108.0)
                .show_ui(ui, |ui| {
                    let all = ui.selectable_value(&mut filter.provider, None, "All sources");
                    refocus |= all.clicked();
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("problems.source.All", &all);
                    let _ = all;
                    for facet in model.providers() {
                        let response = ui.selectable_value(
                            &mut filter.provider,
                            Some(facet.value.clone()),
                            format!("{} ({})", facet.value, facet.count),
                        );
                        refocus |= response.clicked();
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        crate::desktop_test::observe(
                            &format!("problems.source.{}", facet.value),
                            &response,
                        );
                        let _ = response;
                    }
                    if let Some(selected) = filter.provider.clone()
                        && !model.providers().iter().any(|f| f.value == selected)
                    {
                        refocus |= ui
                            .selectable_value(
                                &mut filter.provider,
                                Some(selected.clone()),
                                format!("{selected} (0)"),
                            )
                            .clicked();
                    }
                });
            *filters_popup_open |= source_popup.inner.is_some();
            let source = source_popup.response;
            source.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Source")
            });
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("problems.source", &source);
            let severity_popup = egui::ComboBox::from_id_salt("problems-type")
                .selected_text(
                    filter
                        .severity
                        .map(filter::type_label)
                        .unwrap_or("All types"),
                )
                .width(94.0)
                .show_ui(ui, |ui| {
                    let all = ui.selectable_value(&mut filter.severity, None, "All types");
                    refocus |= all.clicked();
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("problems.type.All", &all);
                    let _ = all;
                    for facet in model.severities() {
                        let response = ui.selectable_value(
                            &mut filter.severity,
                            Some(facet.value),
                            format!("{} ({})", filter::type_label(facet.value), facet.count),
                        );
                        refocus |= response.clicked();
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        crate::desktop_test::observe(
                            &format!("problems.type.{:?}", facet.value),
                            &response,
                        );
                        let _ = response;
                    }
                    if let Some(selected) = filter.severity
                        && !model.severities().iter().any(|f| f.value == selected)
                    {
                        refocus |= ui
                            .selectable_value(
                                &mut filter.severity,
                                Some(selected),
                                format!("{} (0)", filter::type_label(selected)),
                            )
                            .clicked();
                    }
                });
            *filters_popup_open |= severity_popup.inner.is_some();
            let severity = severity_popup.response;
            severity.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Type")
            });
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("problems.type", &severity);
            if category_visible {
                let category_popup = egui::ComboBox::from_id_salt("problems-category")
                    .selected_text(filter.category.as_deref().unwrap_or("All categories"))
                    .width(116.0)
                    .show_ui(ui, |ui| {
                        refocus |= ui
                            .selectable_value(&mut filter.category, None, "All categories")
                            .clicked();
                        for facet in model.categories() {
                            refocus |= ui
                                .selectable_value(
                                    &mut filter.category,
                                    Some(facet.value.clone()),
                                    format!("{} ({})", facet.value, facet.count),
                                )
                                .clicked();
                        }
                        if let Some(selected) = filter.category.clone()
                            && !model.categories().iter().any(|f| f.value == selected)
                        {
                            refocus |= ui
                                .selectable_value(
                                    &mut filter.category,
                                    Some(selected.clone()),
                                    format!("{selected} (0)"),
                                )
                                .clicked();
                        }
                    });
                *filters_popup_open |= category_popup.inner.is_some();
                let category = category_popup.response;
                category.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Category")
                });
            }
            let detail_popup = egui::ComboBox::from_id_salt("problems-diagnostic-detail")
                .selected_text(match profile {
                    Some(ToolProfile::Normal) => "Normal",
                    Some(ToolProfile::Verbose) => "Verbose",
                    None => "Custom",
                })
                .width(72.0)
                .show_ui(ui, |ui| {
                    for choice in ToolProfile::ALL {
                        let option = ui.selectable_label(profile == Some(choice), choice.label());
                        option.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::SelectableLabel,
                                true,
                                format!("{} diagnostics", choice.label()),
                            )
                        });
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        crate::desktop_test::observe(
                            &format!("problems.diagnostic_detail.{}", choice.label()),
                            &option,
                        );
                        if option.clicked() {
                            *requested_profile = Some(choice);
                            refocus = true;
                        }
                    }
                });
            *filters_popup_open |= detail_popup.inner.is_some();
            detail_popup.response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Diagnostic detail")
            });
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("problems.diagnostic_detail", &detail_popup.response);
            let grouping =
                icons::quiet_square_icon_button(ui, UiIcon::Menu, "Group problems", 20.0, *grouped);
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("problems.grouping", &grouping);
            if grouping.clicked() {
                *grouped = !*grouped;
                refocus = true;
            }
            let reset = ui
                .add_enabled_ui(filter.is_active(), |ui| {
                    icons::square_icon_button(ui, UiIcon::Revert, "Reset problem filters", 20.0)
                })
                .inner;
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("problems.reset", &reset);
            if reset.clicked() {
                filter.clear();
                refocus = true;
            }
            // Popup entries can be outside the panel, and selectable labels do
            // not retain keyboard focus after their popup closes. Keep subsequent
            // typing and Find commands with Problems instead of the source editor.
            if refocus {
                search.request_focus();
            }
            let changed = model.refilter(filter) || refocus;
            ui.label(
                RichText::new(format!("{}/{}", model.visible_count(), model.total_count()))
                    .size(theme::TYPE.supporting)
                    .weak(),
            );
            changed
        })
        .inner;
    ui.separator();
    changed
}

fn controls(
    ui: &mut egui::Ui,
    state: &mut State,
    profile: Option<ToolProfile>,
    requested_profile: &mut Option<ToolProfile>,
) -> bool {
    let start = ui.cursor().top();
    let width = ui.available_width();
    let rect = Rect::from_min_size(ui.cursor().min, Vec2::new(width, ui.available_height()));
    let id = viewport_scoped_id(ui.ctx(), "problems-controls");
    // A wrapped toolbar must not enlarge a vertical-only scroll area's width.
    // Allocate its measured height while keeping its width within the viewport.
    let mut child = ui.new_child(egui::UiBuilder::new().id(id).max_rect(rect));
    child.set_clip_rect(ui.clip_rect().intersect(Rect::from_x_y_ranges(
        rect.x_range(),
        ui.clip_rect().y_range(),
    )));
    let changed = controls_contents(&mut child, state, profile, requested_profile);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(width, child.min_rect().height()), Sense::hover());
    state.controls_rect = rect;
    state.controls_visible = rect.intersect(ui.clip_rect()).is_positive();
    state.controls_height = (ui.cursor().top() - start).max(0.0);
    state.popup_open = state.filters_popup_open;
    changed
}

impl EditorApp {
    fn refresh_problem_actions(&mut self) {
        if self.tabs.is_empty() {
            self.problems.actions = Actions::default();
            return;
        }
        let stamp = (
            self.document().key(),
            self.diagnostic_regions.generation,
            self.preview_document_path(),
        );
        if self.problems.actions.stamp.as_ref() == Some(&stamp) {
            return;
        }
        let mut ownership = std::collections::HashMap::new();
        let mut current = HashSet::new();
        let mut automatic = HashSet::new();
        let mut suggestions = HashSet::new();
        let suggestion_kind = matches!(
            self.document().kind(),
            DocumentKind::Typst | DocumentKind::Tex
        );
        for (index, diagnostic) in self
            .preview
            .diagnostics
            .iter()
            .chain(&self.preview.editor_diagnostics)
            .enumerate()
        {
            let correction_current = diagnostic
                .correction
                .as_ref()
                .is_some_and(|correction| correction.is_current(stamp.0));
            // Generating a proposal does not require an existing LSP code action.
            // Revision-bound corrections still provide a useful staleness guard.
            let suggestion = suggestion_kind
                && diagnostic
                    .location
                    .is_some_and(|location| location.line > 0 && location.column > 0)
                && diagnostic
                    .correction
                    .as_ref()
                    .is_none_or(|correction| correction.document.is_none_or(|key| key == stamp.0));
            if !(correction_current || suggestion)
                || !*ownership
                    .entry(&diagnostic.source)
                    .or_insert_with(|| self.diagnostic_targets_current_document(diagnostic))
            {
                continue;
            }
            if correction_current {
                current.insert(index);
                if diagnostic
                    .correction
                    .as_ref()
                    .and_then(diagnostic_corrections::automatic_index)
                    .is_some()
                {
                    automatic.insert(index);
                }
            }
            if suggestion {
                suggestions.insert(index);
            }
        }
        self.problems.actions = Actions {
            stamp: Some(stamp),
            current,
            automatic,
            suggestions,
            #[cfg(test)]
            rebuilds: self.problems.actions.rebuilds + 1,
            ..Default::default()
        };
    }

    fn refresh_problems_model(&mut self, context: &egui::Context) -> bool {
        self.prepare_diagnostic_visibility(Some(context));
        let main_path = self.preview_document_path();
        self.problems.model.update(
            self.diagnostic_regions.generation,
            &self.preview.diagnostics,
            &self.preview.editor_diagnostics,
            Some(&main_path),
            &self.diagnostic_regions.suppressed,
        );
        self.refresh_problem_actions();
        let changed = self.problems.model.refilter(&self.problems.filter);
        if self.problems.actions.filter.as_ref() != Some(&self.problems.filter) {
            self.problems.actions.shown_fixes = self
                .problems
                .model
                .indices()
                .iter()
                .copied()
                .filter(|index| self.problems.actions.automatic.contains(index))
                .collect();
            self.problems.actions.filter = Some(self.problems.filter.clone());
        }
        changed
    }

    /// In the existing tab header: no permanent extra toolbar/list row.
    pub(super) fn show_problems_fix_all(&mut self, ui: &mut egui::Ui) {
        if self.refresh_problems_model(ui.ctx()) {
            self.dismiss_hover_on_scroll(ui.ctx());
        }
        let available =
            !self.problems.actions.shown_fixes.is_empty() && !self.source_edit_blocked();
        let response = ui.add_enabled(available, egui::Button::new("Fix all fixable").small());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                available,
                "Fix all fixable problems in this file",
            )
        });
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("problems.fix_all", &response);
        if response.clicked() {
            let candidates: Vec<_> = self
                .problems
                .actions
                .shown_fixes
                .iter()
                .map(|&index| {
                    if index < self.preview.diagnostics.len() {
                        self.preview.diagnostics[index].clone()
                    } else {
                        self.preview.editor_diagnostics[index - self.preview.diagnostics.len()]
                            .clone()
                    }
                })
                .collect();
            match self.apply_automatic_diagnostic_fixes(&candidates, ui.ctx()) {
                Ok(count) => {
                    self.notice = Some(Notice {
                        message: if count == 0 {
                            "No non-conflicting fixes available".into()
                        } else {
                            format!("Applied {count} fixes in this file")
                        },
                        kind: if count == 0 {
                            NoticeKind::Info
                        } else {
                            NoticeKind::Success
                        },
                    });
                }
                Err(error) => self.show_file_error(error),
            }
        }
    }

    pub(super) fn show_problems(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        let refreshed = self.refresh_problems_model(ui.ctx());
        let search = search_id(ui.ctx());
        let sticky = self.problems.filter.is_active()
            || self.problems.focus_search
            || self.problems.filters_popup_open
            || ui.ctx().memory(|memory| memory.has_focus(search));
        let body_height = ui.available_height();
        let height = if self.problems.controls_height > 0.0 {
            self.problems.controls_height
        } else {
            ui.spacing().interact_size.y + 8.0
        };
        let scroll_id = ui.make_persistent_id(egui::IdSalt::new("problems-scroll"));
        let saved = egui::scroll_area::State::load(ui.ctx(), scroll_id);
        let offset = initial_offset(
            self.problems.scroll_layout,
            saved.map(|state| state.offset.y),
            sticky,
            height,
        );
        let profile =
            tool_profiles::selected_all(self.pending_settings.as_ref().unwrap_or(&self.settings));
        let mut requested_profile = None;
        let mut filtered = false;
        self.problems.sticky_filters = sticky;
        if sticky {
            filtered = controls(ui, &mut self.problems, profile, &mut requested_profile);
        }
        let mut groups_changed = false;
        let mut copied = false;
        let mut jump_target = None;
        let mut correction_target = None;
        let mut dictionary_target = None;
        let mut suggestion_target = None;
        let mut autofix_target = None;
        let suggestion_ready = !self.diagnostic_fix.busy();
        let edits_allowed = !self.source_edit_blocked();
        let autofix_ready = edits_allowed && !self.document_flow_busy() && !self.autofix.busy();
        let tex_document = !self.tabs.is_empty() && self.document().kind() == DocumentKind::Tex;
        let current_key = (!self.tabs.is_empty()).then(|| self.document().key());
        // Floating scrollbars expand on hover. Keep row actions outside the
        // full hit area, including when only the hidden toolbar overflows.
        let bars = ui.spacing().scroll;
        let right_inset = if bars.floating {
            (bars.bar_width + bars.bar_outer_margin - bars.floating_allocated_width + 2.0).max(0.0)
        } else {
            0.0
        };
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("problems-scroll")
            .content_margin(egui::Margin {
                right: right_inset.ceil() as i8,
                ..egui::Margin::ZERO
            })
            .min_scrolled_height(0.0)
            .auto_shrink([false, false]);
        if let Some(offset) = offset {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        let mut output = scroll.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            if !sticky {
                filtered = controls(ui, &mut self.problems, profile, &mut requested_profile);
                // A short/empty list must still allow scrolling up into
                // the controls, while its initial first row remains flush.
                ui.set_min_height(body_height + self.problems.controls_height);
            }
            let list_id = viewport_scoped_id(ui.ctx(), "problems-list");
            ui.scope_builder(egui::UiBuilder::new().id(list_id), |ui| {
                if self.problems.model.visible_count() == 0 {
                    ui.label(
                        RichText::new(if self.problems.model.total_count() == 0 {
                            "No problems"
                        } else {
                            "No matching problems"
                        })
                        .weak(),
                    );
                }
                let grouped = self.problems.grouped;
                let sections = self
                    .problems
                    .model
                    .groups()
                    .iter()
                    .map(Some)
                    .take(if grouped { usize::MAX } else { 0 })
                    .chain(std::iter::once(None).take(usize::from(!grouped)));
                for group in sections {
                    let indices = if let Some(group) = group {
                        let group_key = (group.provider.clone(), group.severity);
                        let mut open = !self.problems.collapsed.contains(&group_key);
                        let label = format!("{} ({})", group.label, group.indices.len());
                        let header = ui
                            .push_id((&group.provider, group.severity), |ui| {
                                ui.add(
                                    egui::Button::new(
                                        RichText::new(format!(
                                            "{} {label}",
                                            if open { "▾" } else { "▸" }
                                        ))
                                        .strong(),
                                    )
                                    .frame(false),
                                )
                            })
                            .inner;
                        header.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label)
                        });
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        crate::desktop_test::observe(
                            &group_target(&group.provider, group.severity),
                            &header,
                        );
                        if header.clicked() {
                            header.request_focus();
                            groups_changed = true;
                            open = !open;
                            if open {
                                self.problems.collapsed.remove(&group_key);
                            } else {
                                self.problems.collapsed.insert(group_key);
                            }
                        }
                        if !open {
                            continue;
                        }
                        group.indices.as_slice()
                    } else {
                        self.problems.model.indices()
                    };
                    for &index in indices {
                        let diagnostic = if index < self.preview.diagnostics.len() {
                            &self.preview.diagnostics[index]
                        } else {
                            &self.preview.editor_diagnostics[index - self.preview.diagnostics.len()]
                        };
                        let correction = diagnostic.correction.as_ref();
                        let actionable = self.problems.actions.current.contains(&index);
                        let correction_key = current_key.filter(|_| actionable && edits_allowed);
                        let has_fixes = correction.is_some_and(|c| !c.replacements.is_empty());
                        let dictionary_word = diagnostic_corrections::dictionary_word(diagnostic);
                        let action_count = usize::from(has_fixes) + usize::from(dictionary_word.is_some());
                        let color = diagnostic_color(diagnostic.severity, ui.ctx());
                        let code = diagnostic_code(diagnostic);
                        let autofix_rule = tex_document
                            .then(|| {
                                diagnostic
                                    .provider
                                    .as_deref()
                                    .filter(|provider| provider.eq_ignore_ascii_case("badness"))
                                    .and(code.as_deref())
                                    .and_then(|code| crate::tex::autofix::Rule::for_diagnostic(code, &diagnostic.message))
                            })
                            .flatten();
                        let details_id = ui.make_persistent_id((
                            "problem-details",
                            self.tabs.active_id(),
                            &diagnostic.source,
                            diagnostic.location,
                            &diagnostic.provider,
                            &diagnostic.message,
                            diagnostic.severity,
                            &code,
                        ));
                        let has_details = diagnostic
                            .details
                            .iter()
                            .any(|detail| problem_is_detail(detail));
                        let mut expanded = ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(details_id).unwrap_or(false));
                        let mut disclosure_rect = None;
                        let mut actions_rect = Rect::NOTHING;
                        let background_slot = ui.painter().add(egui::Shape::Noop);
                        let row = ui
                            .push_id((details_id, index), |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    let severity = ui.add(egui::Label::new(
                                        RichText::new(problem_severity_glyph(diagnostic.severity))
                                            .monospace()
                                            .color(color),
                                    ));
                                    severity.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::Label,
                                            true,
                                            diagnostic.severity.label(),
                                        )
                                    });
                                    if let Some(location) = diagnostic.location {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(format!(
                                                    "{}:{}",
                                                    location.line, location.column,
                                                ))
                                                .monospace()
                                                .weak(),
                                            )
                                            .selectable(true),
                                        );
                                    }
                                    let source = match (&diagnostic.provider, &code) {
                                        (Some(provider), Some(code)) => {
                                            format!("{provider} · Code: {code}")
                                        }
                                        (Some(provider), None) => provider.clone(),
                                        (None, Some(code)) => format!("Code: {code}"),
                                        (None, None) => String::new(),
                                    };
                                    let provider_width = if source.is_empty() {
                                        0.0
                                    } else {
                                        ui.painter()
                                            .layout_no_wrap(
                                                source.clone(),
                                                theme::supporting_font(),
                                                ui.visuals().weak_text_color(),
                                            )
                                            .size()
                                            .x
                                    };
                                    let action_width = if action_count > 0 {
                                        let font = egui::TextStyle::Button.resolve(ui.style());
                                        let measure = |text: &str| ui.painter().layout_no_wrap(text.into(), font.clone(), ui.visuals().text_color()).size().x + 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x;
                                        (if has_fixes { measure("Fix…") } else { 0.0 })
                                            + (if dictionary_word.is_some() { measure("Add word") } else { 0.0 })
                                    } else { 0.0 };
                                    let (message_width, provider_width) = problem_text_widths(
                                        (ui.available_width() - action_width).max(0.0),
                                        provider_width,
                                        has_details,
                                        ui.spacing().item_spacing.x,
                                    );
                                    ui.allocate_ui_with_layout(
                                        Vec2::new(message_width, 0.0),
                                        Layout::top_down(Align::Min),
                                        |ui| {
                                            ui.set_min_width(message_width);
                                            ui.add(
                                                egui::Label::new(&diagnostic.message)
                                                    .selectable(true)
                                                    .wrap(),
                                            );
                                        },
                                    );
                                    if provider_width > 0.0 {
                                        ui.allocate_ui_with_layout(
                                            Vec2::new(provider_width, 0.0),
                                            Layout::top_down(Align::Min),
                                            |ui| {
                                                ui.set_min_width(provider_width);
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(source)
                                                            .size(theme::TYPE.supporting)
                                                            .weak(),
                                                    )
                                                    .selectable(true)
                                                    .truncate()
                                                    .show_tooltip_when_elided(false),
                                                );
                                            },
                                        );
                                    }
                                    if let Some(correction) = correction.filter(|_| has_fixes) {
                                        let automatic = diagnostic_corrections::automatic_index(correction);
                                        let fix = ui.add_enabled(actionable && edits_allowed,
                                            egui::Button::new(if automatic.is_some() { "Fix" } else { "Fix…" }).small());
                                        fix.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, actionable && edits_allowed,
                                            if automatic.is_some() { "Fix automatically" } else { "Choose correction" }));
                                        actions_rect |= fix.rect;
                                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                                        if ui.is_rect_visible(fix.rect) { crate::desktop_test::observe(&format!("problem.{index}.fix"), &fix); }
                                        if let Some(selected) = automatic {
                                            if fix.clicked() { correction_target = Some((diagnostic.clone(), selected)); }
                                        } else {
                                            let menu = egui::Popup::menu(&fix).show(|ui| {
                                                if let Some(selected) = diagnostic_corrections::show_corrections(ui, correction, correction_key) {
                                                    correction_target = Some((diagnostic.clone(), selected));
                                                    ui.close();
                                                }
                                            });
                                            self.problems.popup_open |= menu.is_some();
                                        }
                                    }
                                    if let Some(word) = dictionary_word {
                                        let add = ui.add_enabled(actionable, egui::Button::new("Add word").small());
                                        add.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, actionable,
                                            format!("Add “{word}” to Harper dictionary")));
                                        actions_rect |= add.rect;
                                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                                        if ui.is_rect_visible(add.rect) { crate::desktop_test::observe(&format!("problem.{index}.dictionary"), &add); }
                                        if add.clicked() { dictionary_target = Some(diagnostic.clone()); }
                                    }
                                    if has_details {
                                        let label = if expanded {
                                            "Hide diagnostic details"
                                        } else {
                                            "Show diagnostic details"
                                        };
                                        let disclosure = icons::square_icon_button(
                                            ui,
                                            if expanded { UiIcon::Down } else { UiIcon::Next },
                                            label,
                                            18.0,
                                        );
                                        disclosure_rect = Some(disclosure.rect);
                                        #[cfg(all(
                                            feature = "desktop-ui-tests",
                                            target_os = "macos"
                                        ))]
                                        if ui.is_rect_visible(disclosure.rect) {
                                            crate::desktop_test::observe(
                                                &format!("problem.{index}.details"),
                                                &disclosure,
                                            );
                                        }
                                        if disclosure.clicked() {
                                            expanded = !expanded;
                                            ui.ctx().data_mut(|data| {
                                                data.insert_temp(details_id, expanded)
                                            });
                                        }
                                    }
                                });
                                if expanded {
                                    for detail in diagnostic
                                        .details
                                        .iter()
                                        .filter(|detail| problem_is_detail(detail))
                                    {
                                        ui.horizontal(|ui| {
                                            ui.add_space(METRICS.problems.detail_indent);
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(detail)
                                                        .size(theme::TYPE.supporting)
                                                        .monospace()
                                                        .weak(),
                                                )
                                                .selectable(true)
                                                .wrap(),
                                            );
                                        });
                                    }
                                }
                            })
                            .response;
                        // The parent row senses hover so its selectable text can
                        // own pointer drags. Open its context menu from the actual
                        // secondary release rather than making the row steal clicks.
                        let context_requested = ui.rect_contains_pointer(row.rect)
                            && ui.input(|input| {
                                input.pointer.button_clicked(egui::PointerButton::Secondary)
                            });
                        // Keep the popup bound to the revision on which it was
                        // opened. A later repaint must not silently rebind an
                        // already visible action after the source changes.
                        let menu_owner_id = details_id.with(("problem-menu-owner", index));
                        if context_requested {
                            self.problems.autofix_menu_owner = current_key.map(|key| (menu_owner_id, key));
                        }
                        let menu_owner = self.problems.autofix_menu_owner
                            .filter(|(owner, _)| *owner == menu_owner_id)
                            .map(|(_, document)| document);
                        let menu = egui::Popup::context_menu(&row)
                            .open_memory(
                                context_requested.then_some(egui::SetOpenCommand::Bool(true)),
                            )
                            .show(|ui| {
                                if let Some(rule) = autofix_rule {
                                    let enabled = autofix_ready
                                        && self.problems.actions.suggestions.contains(&index)
                                        && current_key.is_some()
                                        && menu_owner == current_key
                                        && rule.enabled(&self.settings.autofix);
                                    let autofix = ui.add_enabled(
                                        enabled,
                                        egui::Button::new(format!("Autofix all: {}", rule.label())),
                                    ).on_hover_text("Fix all occurrences of this enabled rule in the current TeX document in one undo step.");
                                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                                    crate::desktop_test::observe(&format!("problem.{index}.autofix"), &autofix);
                                    if autofix.clicked() {
                                        autofix_target = menu_owner.map(|document| AutofixTarget { tab: self.tabs.active_id(), document, rule });
                                        ui.close();
                                    }
                                }
                                let suggestion = ui.add_enabled(
                                    self.problems.actions.suggestions.contains(&index)
                                        && edits_allowed && suggestion_ready,
                                    egui::Button::new(diagnostic_fix::MENU_LABEL),
                                );
                                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                                {
                                    crate::desktop_test::observe("menu.Suggest fix", &suggestion);
                                    crate::desktop_test::observe(&format!("problem.{index}.suggest_fix"), &suggestion);
                                }
                                if suggestion.clicked() {
                                    suggestion_target = Some(diagnostic.clone());
                                    ui.close();
                                }
                                if let Some(correction) = &diagnostic.correction
                                    && let Some(index) = diagnostic_corrections::show_corrections(
                                        ui,
                                        correction,
                                        correction_key,
                                    )
                                {
                                    correction_target = Some((diagnostic.clone(), index));
                                    ui.close();
                                }
                                if let Some(word) = dictionary_word
                                    && ui.add_enabled(actionable, egui::Button::new(format!("Add “{word}” to Harper dictionary"))).clicked() {
                                    dictionary_target = Some(diagnostic.clone());
                                    ui.close();
                                }
                                if ui.button("Copy diagnostic").clicked() {
                                    ui.ctx().copy_text(diagnostic.full_message());
                                    copied = true;
                                    ui.close();
                                }
                            });
                        if menu.is_none() && self.problems.autofix_menu_owner.is_some_and(|(owner, _)| owner == menu_owner_id) {
                            self.problems.autofix_menu_owner = None;
                        }
                        self.problems.popup_open |= menu.is_some();
                        let row = if self.snapshot_scene == Some(UiSnapshotScene::ProblemsPanel)
                            && index == 0
                        {
                            row.highlight()
                        } else {
                            row
                        };
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        if ui.is_rect_visible(row.rect) {
                            crate::desktop_test::observe(&format!("problem.{index}"), &row);
                        }
                        let pointer_in_row = row
                            .ctx
                            .pointer_latest_pos()
                            .is_some_and(|position| row.rect.contains(position));
                        if pointer_in_row || row.highlighted() {
                            ui.painter().set(
                                background_slot,
                                egui::Shape::rect_filled(
                                    row.rect,
                                    theme::RADIUS.row as f32,
                                    ui.visuals()
                                        .selection
                                        .bg_fill
                                        .gamma_multiply(METRICS.problems.hover_opacity),
                                ),
                            );
                        }
                        let disclosure_hovered = disclosure_rect.is_some_and(|rect| {
                            ui.ctx()
                                .pointer_latest_pos()
                                .is_some_and(|point| rect.contains(point))
                        });
                        if diagnostic.location.is_some()
                            && !disclosure_hovered
                            && !ui.ctx().pointer_latest_pos().is_some_and(|point| actions_rect.contains(point))
                            // Offscreen row rectangles can sit behind the sticky
                            // filters or another popup; only their visible layer
                            // may respond to a diagnostic double-click.
                            && ui.rect_contains_pointer(row.rect)
                            && let Some(diagnostic) = problem_row_jump_target(
                                diagnostic,
                                problem_row_double_clicked(&row),
                            )
                        {
                            jump_target = Some(diagnostic);
                        }
                    }
                }
                if !self.problems.filter.is_active() && !self.preview.raw_diagnostics.is_empty() {
                    ui.collapsing("Raw CLI stdout", |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.preview.raw_diagnostics).monospace(),
                            )
                            .selectable(true)
                            .wrap(),
                        );
                    });
                }
            });
        });
        debug_assert_eq!(output.id, scroll_id);
        let measured = self.problems.controls_height;
        if !sticky
            && (measured - height).abs() > 0.5
            && (saved.is_none() || output.state.offset.y >= height - 0.5)
        {
            output.state.offset.y = (output.state.offset.y + measured - height).max(0.0);
            output.state.store(ui.ctx(), scroll_id);
            // Finish measuring a wrapped toolbar before painting its first
            // visible list frame. This is bounded to a layout-height change.
            ui.ctx().request_discard("Problems filter toolbar height");
        }
        self.problems.scroll_layout = Some(ScrollLayout {
            sticky,
            height: measured,
        });
        self.problems.scroll_offset = output.state.offset.y;
        self.problems.scroll_rect = output.inner_rect;
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe_scroll(
            "problems",
            ui.ctx(),
            output.inner_rect,
            output.content_size.y,
            output.state.offset.y,
            sticky,
        );
        if refreshed || filtered || groups_changed {
            self.dismiss_hover_on_scroll(ui.ctx());
        }
        if let Some(profile) = requested_profile {
            let mut edited = self.settings_snapshot();
            match tool_profiles::apply_all(profile, &mut edited) {
                Ok(true) => self.queue_settings(edited, ui.ctx()),
                Ok(false) => {}
                Err(error) => self.show_file_error(error),
            }
        }
        if copied {
            self.focus_problems_search(ui.ctx());
        }
        if let Some(target) = autofix_target {
            if !self.tabs.is_empty()
                && self.tabs.active_id() == target.tab
                && self.document().key() == target.document
                && target.rule.enabled(&self.settings.autofix)
            {
                self.begin_autofix_rule(target.rule, ui.ctx());
            }
        } else if let Some(diagnostic) = suggestion_target {
            let cursor = egui::text_edit::TextEditState::load(ui.ctx(), source_editor_id(ui.ctx()))
                .and_then(|state| state.cursor.char_range())
                .unwrap_or_default();
            if let Some(target) = self.bind_diagnostic_fix(&diagnostic, cursor) {
                self.begin_diagnostic_fix(target, ui.ctx());
            } else {
                self.show_file_error("The diagnostic changed; reopen its menu.".into());
            }
        } else if let Some(diagnostic) = dictionary_target {
            if let Err(error) = self.add_diagnostic_word(&diagnostic, ui.ctx()) {
                self.show_file_error(error);
            }
        } else if let Some((diagnostic, index)) = correction_target {
            if let Err(error) = self.apply_diagnostic_correction(&diagnostic, index, ui.ctx()) {
                self.show_file_error(error);
            }
        } else if let Some(diagnostic) = jump_target {
            self.jump_to_diagnostic(diagnostic);
        }
    }
}
