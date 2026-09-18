//! Explorer sections and rows: borrowed presentation inputs and returned actions.
use super::{
    FileDropTarget, METRICS, TooltipPlacement, UiIcon, approximate_char_capacity,
    clipped_panel_content_ui, error_color, icon_button, native_hover_text, offer_asset_hover,
    offer_file_drop_target, offer_folder_row_drop, static_icon, tail_elide,
};
use crate::workspace::{WorkspaceNode, WorkspaceSnapshot};
use crate::{
    explorer::{ExplorerOrder, ExplorerPanelState, ExplorerSection},
    project_index::ProjectIndex,
    theme,
};
use eframe::egui::Color32;
use eframe::egui::{self, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use egui_ltreeview::{Action as TreeAction, NodeBuilder, TreeView, TreeViewBuilder, TreeViewState};
use std::path::{Path, PathBuf};

pub(super) struct Input<'a> {
    pub root: &'a Path,
    pub label: &'a str,
    pub generation: Option<u64>,
    pub snapshot: Option<&'a WorkspaceSnapshot>,
    pub index: &'a ProjectIndex,
    pub active: Option<&'a Path>,
    pub preview: Option<&'a Path>,
    pub statuses: &'a crate::git::editor::FileStatuses,
    pub error: Option<&'a str>,
    pub order: ExplorerOrder,
    pub git_visible: bool,
}

pub(super) struct ContextMenu {
    pub anchor: Pos2,
    pub path: PathBuf,
    pub is_file: bool,
}

/// Bounded output: painting cannot start scans, navigation or Git operations.
pub(super) struct Output<G> {
    pub change_root: bool,
    pub refresh: bool,
    pub repaint: bool,
    pub packages: bool,
    pub open: Option<PathBuf>,
    pub index_target: Option<(PathBuf, usize)>,
    pub context_menu: Option<ContextMenu>,
    pub git: Option<G>,
}

pub(super) fn show<G>(
    ui: &mut egui::Ui,
    input: Input<'_>,
    state: &mut ExplorerPanelState,
    mut show_git: impl FnMut(&mut egui::Ui) -> G,
) -> Output<G> {
    offer_file_drop_target(
        ui,
        ui.max_rect(),
        FileDropTarget::Folder(input.root.to_owned()),
    );
    let label = input.label;
    let mut change_root = false;
    let mut refresh = false;
    let mut repaint = false;
    theme::panel_header(ui, "workspace-header", |ui| {
        let action_width = METRICS.explorer.header_refresh_width;
        let actions_width = action_width + ui.spacing().item_spacing.x;
        let path_width = (ui.available_width() - actions_width).max(1.0);
        let max_chars = approximate_char_capacity(path_width, theme::TYPE.supporting);
        let response = ui.add_sized(
            [path_width, METRICS.explorer.header_row_height],
            egui::Label::new(
                RichText::new(tail_elide(input.label, max_chars))
                    .color(ui.visuals().weak_text_color()),
            )
            .truncate()
            .sense(Sense::click()),
        );
        let mut hover = input.generation.map_or_else(
                || format!("{label}\nDouble-click to change workspace root"),
                |generation| {
                    format!(
                        "{label}\nFilesystem snapshot generation {generation}\nDouble-click to change workspace root"
                    )
                },
            );
        if let Some(warning) = input.index.warning() {
            hover.push_str("\nSome Explorer entries could not be discovered.\n");
            hover.push_str(warning);
        }
        if native_hover_text(response, hover).double_clicked() {
            change_root = true;
        }
        if icon_button(ui, UiIcon::Refresh, "Refresh filesystem").clicked() {
            refresh = true;
        }
    });
    theme::panel_header(ui, "workspace-search-header", |ui| {
        let show_clear = !state.query().is_empty();
        let clear_width = if show_clear {
            METRICS.icon.button_size.x + ui.spacing().item_spacing.x
        } else {
            0.0
        };
        let search = ui.add_sized(
            [
                (ui.available_width() - clear_width).max(1.0),
                METRICS.explorer.header_row_height,
            ],
            egui::TextEdit::singleline(state.query_mut())
                .id_salt("explorer-search")
                .hint_text("Search Explorer"),
        );
        if state.take_search_focus() {
            search.request_focus();
        }
        if show_clear && icon_button(ui, UiIcon::Close, "Clear Explorer search").clicked() {
            state.query_mut().clear();
        }
    });

    // A tree row can be much wider than the pane. Keep the body width in a
    // clipped child UI so it becomes scrollable content instead of feeding
    // back into `PanelState` and growing the resizable explorer each frame.
    let mut content_ui = clipped_panel_content_ui(ui, "workspace-clipped-content");
    let ui = &mut content_ui;
    ui.spacing_mut().item_spacing.y = METRICS.explorer.section_gap;

    let snapshot = input.snapshot;
    let project_root = snapshot.map_or(input.root, |snapshot| snapshot.root.as_path());
    let preview_path = input.preview;
    let project_index = input.index;
    let active = snapshot.as_ref().and_then(|snapshot| {
        input.active.and_then(|path| {
            path.strip_prefix(&snapshot.root)
                .ok()
                .and_then(|relative| snapshot.find(relative))
                .map(|node| node.path.as_path())
        })
    });
    let preview = snapshot
        .as_ref()
        .and_then(|snapshot| {
            preview_path.and_then(|path| {
                path.strip_prefix(&snapshot.root)
                    .ok()
                    .and_then(|relative| snapshot.find(relative))
                    .map(|node| node.path.as_path())
            })
        })
        .or(preview_path);
    let mut open_path = None;
    let mut popup_request = None;
    let context = ui.ctx().clone();
    let explorer_query = normalize_explorer_query(state.query());
    let filter_active = !explorer_query.is_empty();
    let mut section_defaults = if filter_active {
        explorer_section_query_matches(snapshot, project_index, &explorer_query)
    } else {
        std::array::from_fn(|index| ExplorerSection::ALL[index].default_open())
    };
    let git_in_explorer = input.git_visible;
    section_defaults[ExplorerSection::Git.index()] = git_in_explorer;
    if !git_in_explorer {
        // Keep the hidden section genuinely collapsed. Merely removing
        // it from the height budget still lets a persisted open state
        // render an empty body and consume a frame of layout.
        set_explorer_section_open(ui, "workspace-git", false);
    }
    if git_in_explorer && state.take_git_reveal() {
        // `CollapsingState` survives across frames, so a Git panel that
        // was previously closed can otherwise remain visually hidden when
        // the user opens Git from the command menu. Reveal it once, then
        // let the user control the section normally.
        set_explorer_section_open(ui, "workspace-git", true);
    }
    let mut open_sections = explorer_section_open_states(ui, filter_active, section_defaults);
    if !git_in_explorer {
        // A persisted open state must not reserve space for a hidden Git
        // section after the panel has been closed.
        open_sections[ExplorerSection::Git.index()] = false;
    }
    let open_section_count = open_sections.iter().filter(|is_open| **is_open).count();
    let section_frame_height = theme::explorer_section_frame(ui.style())
        .total_margin()
        .sum()
        .y;
    let section_body_budget = available_explorer_section_body_height(
        ui.available_height(),
        open_section_count,
        section_frame_height,
    ) * open_section_count as f32;
    let section_layout_id = explorer_section_layout_id(ui);
    let mut section_layout = ui.ctx().data_mut(|data| {
        data.get_temp::<ExplorerSectionLayout>(section_layout_id)
            .unwrap_or_default()
    });
    let section_body_heights = section_layout.body_heights(open_sections, section_body_budget);
    let order = input.order;
    let mut open_package_manager = false;
    let mut index_target = None;
    let mut git_output = None;
    let section_resize = show_explorer_sections(
        ui,
        ExplorerSectionsSpec {
            order,
            defaults: section_defaults,
            heights: section_body_heights,
            open: open_sections,
            filtered: filter_active,
            git_visible: git_in_explorer,
        },
        |ui, section| match section {
            ExplorerSection::Files => {
                if let Some(snapshot) = &snapshot {
                    // A scan generation describes fresh filesystem data, not a
                    // new UI. Keeping it out of the identity preserves opened
                    // folders, selection, and the surrounding ScrollArea's
                    // offset when a file open triggers a background rescan.
                    let tree_id = workspace_tree_state_id(ui, &snapshot.root, filter_active);
                    let mut tree_state = TreeViewState::load(ui, tree_id).unwrap_or_default();
                    if filter_active {
                        open_matching_workspace_ancestors(
                            &mut tree_state,
                            &snapshot.nodes,
                            &explorer_query,
                        );
                    }
                    if let Some(active) = &active {
                        // Keep the document shown in the editor selected so the
                        // entire explorer row gets the same kind of tint as the
                        // editor's active line.
                        tree_state.set_one_selected((*active).to_owned());
                    }
                    let tree = TreeView::new(tree_id)
                        .allow_multi_selection(false)
                        .fallback_context_menu(|ui, selected: &Vec<PathBuf>| {
                            let Some(path) = selected.first().cloned() else {
                                ui.close();
                                return;
                            };
                            let is_file = path
                                .strip_prefix(&snapshot.root)
                                .ok()
                                .and_then(|relative| snapshot.find(relative))
                                .is_some_and(WorkspaceNode::is_file);
                            let anchor = ui
                                .ctx()
                                .pointer_latest_pos()
                                .unwrap_or_else(|| ui.min_rect().left_top());
                            popup_request = Some(ContextMenu {
                                anchor,
                                path,
                                is_file,
                            });
                            ui.close();
                        });
                    let (_, actions) = ui
                        .scope(|ui| {
                            theme::apply_active_row_selection(ui);
                            tree.show_state(ui, &mut tree_state, |builder| {
                                add_workspace_nodes(
                                    builder,
                                    &snapshot.nodes,
                                    active,
                                    preview,
                                    &context,
                                    &explorer_query,
                                    input.statuses,
                                );
                            })
                        })
                        .inner;
                    tree_state.store(ui, tree_id);
                    for action in actions {
                        if let TreeAction::Activate(activate) = action {
                            open_path = activate.selected.into_iter().find(|path| {
                                path.strip_prefix(&snapshot.root)
                                    .ok()
                                    .and_then(|relative| snapshot.find(relative))
                                    .is_some_and(WorkspaceNode::is_file)
                            });
                        }
                    }
                    if filter_active
                        && !snapshot
                            .nodes
                            .iter()
                            .any(|node| workspace_node_matches_query(node, &explorer_query))
                    {
                        ui.label(RichText::new("No matching files").weak());
                    }
                } else {
                    ui.label(RichText::new("No project folder").weak());
                }
                if let Some(error) = input.error {
                    ui.colored_label(error_color(ui.ctx()), error);
                }
            }
            ExplorerSection::Git => {
                git_output = Some(show_git(ui));
            }
            _ => {
                let outcome = show_project_index_section(
                    ui,
                    section,
                    project_root,
                    project_index,
                    &explorer_query,
                );
                open_package_manager |= outcome.open_package_manager;
                if outcome.target.is_some() {
                    index_target = outcome.target;
                }
            }
        },
    );

    if let Some((section, delta)) = section_resize
        && section_layout.resize_after(open_sections, section_body_budget, order, section, delta)
    {
        ui.ctx()
            .data_mut(|data| data.insert_temp(section_layout_id, section_layout));
        repaint = true;
    }

    Output {
        change_root,
        refresh,
        repaint,
        packages: open_package_manager,
        open: open_path,
        index_target,
        context_menu: popup_request,
        git: git_output,
    }
}

pub(super) fn add_workspace_nodes(
    builder: &mut TreeViewBuilder<'_, PathBuf>,
    nodes: &[WorkspaceNode],
    active: Option<&Path>,
    preview: Option<&Path>,
    context: &egui::Context,
    query: &str,
    git: &crate::git::editor::FileStatuses,
) {
    for node in nodes {
        if !workspace_node_matches_query(node, query) {
            continue;
        }
        let is_active = active.is_some_and(|path| path == node.path);
        let is_preview = preview.is_some_and(|path| path == node.path);
        let label = node.display_name().into_owned();
        let git_status = git.get(&node.path);
        if node.is_directory() {
            let color = workspace_entry_color(&node.path, true, false, context);
            let drop_directory = node.path.clone();
            let open = builder.node(
                NodeBuilder::dir(node.path.clone())
                    .default_open(active.is_some_and(|path| path.starts_with(&node.path)))
                    .icon(|ui| paint_tree_icon(ui, true))
                    .label_ui(move |ui| {
                        let color = workspace_entry_resolved_color(
                            color,
                            is_active,
                            ui.visuals().strong_text_color(),
                        );
                        let text = RichText::new(&label).color(color);
                        let response = ui.add(workspace_entry_label(text, is_active));
                        offer_folder_row_drop(ui, response.rect, &drop_directory);
                    }),
            );
            if open {
                add_workspace_nodes(
                    builder,
                    &node.children,
                    active,
                    preview,
                    context,
                    query,
                    git,
                );
            }
            builder.close_dir();
        } else if node.is_file() {
            let color = workspace_entry_color(&node.path, false, false, context);
            let hover_path = node.path.clone();
            let hover_kind = crate::document::preview_kind_for_path(&hover_path);
            builder.node(
                NodeBuilder::leaf(node.path.clone())
                    .icon(|ui| paint_tree_icon(ui, false))
                    .label_ui(move |ui| {
                        let row = ui.horizontal(|ui| {
                            let color = workspace_entry_resolved_color(
                                color,
                                is_active,
                                ui.visuals().strong_text_color(),
                            );
                            let text = RichText::new(&label).color(color);
                            ui.add(workspace_entry_label(text, is_active));
                            if let Some(status) = git_status {
                                ui.add(egui::Label::new(
                                    RichText::new(status.letter())
                                        .monospace()
                                        .strong()
                                        .color(status.color(context)),
                                ))
                                .on_hover_text(status.description());
                            }
                            if is_preview {
                                let blue = theme::palette(ui.ctx()).accent;
                                native_hover_text(
                                    static_icon(ui, UiIcon::Eye, blue),
                                    "Used for preview",
                                );
                            }
                        });
                        if let Some(parent) = hover_path.parent() {
                            offer_folder_row_drop(ui, row.response.rect, parent);
                        }
                        if let Some(kind) = hover_kind {
                            let hover_rect =
                                workspace_asset_hover_rect(row.response.rect, ui.clip_rect());
                            let hover_response = ui.interact(
                                hover_rect,
                                ui.id().with(("asset-row-hover", &hover_path)),
                                Sense::hover(),
                            );
                            offer_asset_hover(
                                &hover_response,
                                hover_rect,
                                hover_path.clone(),
                                kind,
                                TooltipPlacement::Right,
                            );
                        }
                    }),
            );
        } else if node.is_symlink() {
            let color = workspace_entry_color(&node.path, false, true, context);
            builder.node(
                NodeBuilder::leaf(node.path.clone())
                    .icon(|ui| paint_tree_icon(ui, false))
                    .label_ui(move |ui| {
                        let color = workspace_entry_resolved_color(
                            color,
                            is_active,
                            ui.visuals().strong_text_color(),
                        );
                        let text = RichText::new(format!("{label} (link)")).color(color);
                        ui.add(workspace_entry_label(text, is_active));
                    }),
            );
        }
    }
}

pub(super) fn normalize_explorer_query(query: &str) -> String {
    query.trim().to_lowercase()
}

pub(super) fn explorer_text_matches_query(text: &str, normalized_query: &str) -> bool {
    normalized_query.is_empty() || text.to_lowercase().contains(normalized_query)
}

pub(super) fn explorer_path_matches_query(path: &Path, normalized_query: &str) -> bool {
    explorer_text_matches_query(&path.to_string_lossy(), normalized_query)
}

pub(super) fn workspace_node_self_matches_query(
    node: &WorkspaceNode,
    normalized_query: &str,
) -> bool {
    explorer_text_matches_query(&node.display_name(), normalized_query)
        || explorer_path_matches_query(&node.relative_path, normalized_query)
}

pub(super) fn workspace_node_matches_query(node: &WorkspaceNode, normalized_query: &str) -> bool {
    workspace_node_self_matches_query(node, normalized_query)
        || node
            .children
            .iter()
            .any(|child| workspace_node_matches_query(child, normalized_query))
}

pub(super) fn open_matching_workspace_ancestors(
    state: &mut TreeViewState<PathBuf>,
    nodes: &[WorkspaceNode],
    normalized_query: &str,
) {
    for node in nodes.iter().filter(|node| node.is_directory()) {
        if workspace_node_matches_query(node, normalized_query) {
            state.set_openness(node.path.clone(), true);
            open_matching_workspace_ancestors(state, &node.children, normalized_query);
        }
    }
}

pub(super) fn outline_entry_matches_query(
    entry: &crate::project_index::OutlineEntry,
    normalized_query: &str,
) -> bool {
    explorer_text_matches_query(&entry.title, normalized_query)
        || explorer_path_matches_query(&entry.path, normalized_query)
        || explorer_text_matches_query(&entry.line.to_string(), normalized_query)
}

pub(super) fn symbol_entry_matches_query(
    entry: &crate::project_index::SymbolEntry,
    normalized_query: &str,
) -> bool {
    explorer_text_matches_query(&entry.name, normalized_query)
        || explorer_text_matches_query(entry.kind.label(), normalized_query)
        || explorer_path_matches_query(&entry.path, normalized_query)
        || explorer_text_matches_query(&entry.line.to_string(), normalized_query)
}

pub(super) fn reference_entry_matches_query(
    entry: &crate::project_index::ReferenceEntry,
    normalized_query: &str,
) -> bool {
    explorer_text_matches_query(&entry.label, normalized_query)
        || explorer_path_matches_query(&entry.path, normalized_query)
        || explorer_text_matches_query(&entry.line.to_string(), normalized_query)
}

pub(super) fn explorer_section_query_matches(
    snapshot: Option<&WorkspaceSnapshot>,
    index: &ProjectIndex,
    normalized_query: &str,
) -> [bool; ExplorerSection::ALL.len()] {
    let mut matches = [
        snapshot.is_some_and(|snapshot| {
            snapshot
                .nodes
                .iter()
                .any(|node| workspace_node_matches_query(node, normalized_query))
        }),
        false, // Git actions are not searchable, but keep the section slot stable.
        index
            .outline
            .iter()
            .any(|entry| outline_entry_matches_query(entry, normalized_query)),
        index
            .subfiles
            .iter()
            .any(|path| explorer_path_matches_query(path, normalized_query)),
        index
            .symbols
            .iter()
            .any(|entry| symbol_entry_matches_query(entry, normalized_query)),
        index
            .packages
            .iter()
            .any(|package| explorer_text_matches_query(package, normalized_query)),
        index
            .tags
            .iter()
            .any(|entry| reference_entry_matches_query(entry, normalized_query)),
        index
            .references
            .iter()
            .any(|entry| reference_entry_matches_query(entry, normalized_query)),
    ];
    if !matches.into_iter().any(|matched| matched) {
        // Keep one result surface visible so an empty search has a clear
        // outcome instead of presenting closed section headers.
        matches[0] = true;
    }
    matches
}

pub(super) fn workspace_entry_label(text: RichText, is_active: bool) -> egui::Label {
    let text = if is_active {
        // Emphasize weight, not size: Explorer inherits its local UI text
        // size, which need not match the editor's content-font size.
        text.family(theme::strong_ui_font().family).strong()
    } else {
        text
    };
    theme::nonselectable_label(text)
}

pub(super) fn workspace_asset_hover_rect(row: Rect, visible_panel: Rect) -> Rect {
    Rect::from_min_max(
        row.left_top(),
        Pos2::new(visible_panel.right().max(row.left()), row.bottom()),
    )
}

pub(super) fn workspace_entry_resolved_color(
    category_color: Color32,
    is_active: bool,
    strong_text_color: Color32,
) -> Color32 {
    if is_active {
        strong_text_color
    } else {
        category_color
    }
}

pub(super) fn workspace_entry_color(
    path: &Path,
    directory: bool,
    symlink: bool,
    context: &egui::Context,
) -> Color32 {
    if directory {
        return theme::palette(context).accent;
    }
    if symlink {
        return theme::syntax_palette(context).comment;
    }
    let syntax = theme::syntax_palette(context);
    let extension = path.extension().and_then(|extension| extension.to_str());
    if extension_matches(extension, &["typ"]) {
        return syntax.keyword;
    }
    if extension_matches(
        extension,
        &[
            "pdf", "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff",
        ],
    ) {
        return theme::palette(context).info;
    }
    if extension_matches(
        extension,
        &[
            "txt", "md", "markdown", "json", "jsonc", "toml", "yaml", "yml", "xml", "html", "htm",
            "css", "scss", "js", "jsx", "ts", "tsx", "rs", "py", "rb", "go", "java", "c", "h",
            "cc", "cpp", "hpp", "sh", "bash", "zsh", "fish", "sql", "csv", "tsv", "ini", "cfg",
            "conf", "log", "tex", "bib",
        ],
    ) {
        return syntax.plain;
    }
    syntax.comment
}

pub(super) fn extension_matches(extension: Option<&str>, expected: &[&str]) -> bool {
    extension.is_some_and(|extension| {
        expected
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    })
}

pub(super) const EXPLORER_SECTION_MIN_BODY_HEIGHT: f32 = 44.0;
pub(super) const EXPLORER_SECTION_RESIZE_HANDLE_HEIGHT: f32 = 5.0;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ExplorerSectionLayout {
    pub(super) weights: [f32; ExplorerSection::ALL.len()],
}

impl Default for ExplorerSectionLayout {
    fn default() -> Self {
        Self {
            weights: [1.0; ExplorerSection::ALL.len()],
        }
    }
}

impl ExplorerSectionLayout {
    pub(super) fn body_heights(
        &self,
        open: [bool; ExplorerSection::ALL.len()],
        available: f32,
    ) -> [f32; ExplorerSection::ALL.len()] {
        let mut heights = [0.0; ExplorerSection::ALL.len()];
        let open_count = open.iter().filter(|is_open| **is_open).count();
        if open_count == 0 {
            return heights;
        }

        let available = available.max(0.0);
        let minimum = EXPLORER_SECTION_MIN_BODY_HEIGHT.min(available / open_count as f32);
        let remainder = (available - minimum * open_count as f32).max(0.0);
        let weight_sum = self
            .weights
            .iter()
            .zip(open)
            .filter_map(|(weight, is_open)| {
                is_open.then_some(if weight.is_finite() && *weight > 0.0 {
                    *weight
                } else {
                    1.0
                })
            })
            .sum::<f32>()
            .max(f32::EPSILON);

        for (index, is_open) in open.into_iter().enumerate() {
            if is_open {
                let weight = self.weights[index];
                let weight = if weight.is_finite() && weight > 0.0 {
                    weight
                } else {
                    1.0
                };
                heights[index] = minimum + remainder * weight / weight_sum;
            }
        }
        heights
    }

    pub(super) fn resize_after(
        &mut self,
        open: [bool; ExplorerSection::ALL.len()],
        available: f32,
        order: ExplorerOrder,
        upper: ExplorerSection,
        requested_delta: f32,
    ) -> bool {
        if !requested_delta.is_finite() || requested_delta.abs() <= f32::EPSILON {
            return false;
        }
        let Some(lower) = order.next_open(open, upper) else {
            return false;
        };
        let upper_index = upper.index();
        let lower_index = lower.index();
        let mut heights = self.body_heights(open, available);
        let open_count = open.iter().filter(|is_open| **is_open).count();
        let minimum =
            EXPLORER_SECTION_MIN_BODY_HEIGHT.min(available.max(0.0) / open_count.max(1) as f32);
        let applied_delta = requested_delta.clamp(
            minimum - heights[upper_index],
            heights[lower_index] - minimum,
        );
        if applied_delta.abs() <= f32::EPSILON {
            return false;
        }
        heights[upper_index] += applied_delta;
        heights[lower_index] -= applied_delta;

        // Store relative preferences. Recomputing from these weights lets the
        // split scale with the panel while retaining the user's proportions.
        for (index, is_open) in open.into_iter().enumerate() {
            if is_open {
                self.weights[index] = (heights[index] - minimum).max(f32::EPSILON);
            }
        }
        true
    }
}

pub(super) fn explorer_section_open_states(
    ui: &egui::Ui,
    filtered: bool,
    defaults: [bool; ExplorerSection::ALL.len()],
) -> [bool; ExplorerSection::ALL.len()] {
    std::array::from_fn(|index| {
        let id_salt = ExplorerSection::ALL[index].id();
        if filtered {
            return defaults[index];
        }
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            explorer_section_state_id(ui, id_salt, false),
            defaults[index],
        )
        .is_open()
    })
}

pub(super) fn git_command_opens_explorer(git_visible: bool) -> bool {
    !git_visible
}

pub(super) fn set_explorer_section_open(ui: &egui::Ui, id_salt: &'static str, open: bool) {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        explorer_section_state_id(ui, id_salt, false),
        open,
    );
    if state.is_open() != open {
        state.set_open(open);
        state.store(ui.ctx());
    }
}

pub(super) fn explorer_section_layout_id(ui: &egui::Ui) -> egui::Id {
    ui.make_persistent_id("explorer-section-layout")
}

pub(super) fn explorer_section_state_id(
    ui: &egui::Ui,
    id_salt: &'static str,
    filtered: bool,
) -> egui::Id {
    ui.make_persistent_id(("explorer-section", id_salt, filtered))
}

pub(super) fn workspace_tree_state_id(ui: &egui::Ui, root: &Path, filtered: bool) -> egui::Id {
    ui.make_persistent_id(("workspace-tree", root, filtered))
}

#[cfg(test)]
pub(super) fn explorer_section_body_height(ui: &egui::Ui) -> f32 {
    let defaults = std::array::from_fn(|index| ExplorerSection::ALL[index].default_open());
    let open_sections = explorer_section_open_states(ui, false, defaults)
        .into_iter()
        .filter(|is_open| *is_open)
        .count();
    let frame_height = theme::explorer_section_frame(ui.style())
        .total_margin()
        .sum()
        .y;
    available_explorer_section_body_height(ui.available_height(), open_sections, frame_height)
}

pub(super) fn available_explorer_section_body_height(
    available_height: f32,
    open_sections: usize,
    frame_height: f32,
) -> f32 {
    if open_sections == 0 {
        return 0.0;
    }
    let section_count = ExplorerSection::ALL.len() as f32;
    let reserved_headers = section_count * (METRICS.explorer.section_header_height + frame_height);
    let reserved_gaps = (section_count - 1.0).max(0.0) * METRICS.explorer.section_gap;
    ((available_height - reserved_headers - reserved_gaps) / open_sections as f32).max(0.0)
}

#[cfg(test)]
pub(super) fn explorer_section(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    title: &'static str,
    default_open: bool,
    body_height: f32,
    add_body: impl FnOnce(&mut egui::Ui),
) {
    let _ = explorer_section_resizable(
        ui,
        ExplorerSectionRenderSpec {
            id_salt,
            title,
            default_open,
            body_height,
            show_resize_handle: false,
            filtered: false,
        },
        add_body,
    );
}

pub(super) struct ExplorerSectionsSpec {
    pub(super) order: ExplorerOrder,
    pub(super) defaults: [bool; 8],
    pub(super) heights: [f32; 8],
    pub(super) open: [bool; 8],
    pub(super) filtered: bool,
    pub(super) git_visible: bool,
}

pub(super) fn show_explorer_sections(
    ui: &mut egui::Ui,
    spec: ExplorerSectionsSpec,
    mut add_body: impl FnMut(&mut egui::Ui, ExplorerSection),
) -> Option<(ExplorerSection, f32)> {
    let mut resize = None;
    for section in spec.order.sections() {
        if section == ExplorerSection::Git && !spec.git_visible {
            continue;
        }
        let index = section.index();
        let delta = explorer_section_resizable(
            ui,
            ExplorerSectionRenderSpec {
                id_salt: section.id(),
                title: section.title(),
                default_open: spec.defaults[index],
                body_height: spec.heights[index],
                show_resize_handle: spec.order.next_open(spec.open, section).is_some(),
                filtered: spec.filtered,
            },
            |ui| add_body(ui, section),
        );
        if delta.abs() > f32::EPSILON {
            resize = Some((section, delta));
        }
    }
    resize
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ExplorerSectionRenderSpec {
    pub(super) id_salt: &'static str,
    pub(super) title: &'static str,
    pub(super) default_open: bool,
    pub(super) body_height: f32,
    pub(super) show_resize_handle: bool,
    pub(super) filtered: bool,
}

pub(super) fn explorer_section_resizable(
    ui: &mut egui::Ui,
    spec: ExplorerSectionRenderSpec,
    add_body: impl FnOnce(&mut egui::Ui),
) -> f32 {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        explorer_section_state_id(ui, spec.id_salt, spec.filtered),
        spec.default_open,
    );
    if spec.filtered {
        // Filtered results are transient and should always expose the sections
        // that contain matches without mutating the user's normal open state.
        state.set_open(spec.default_open);
    }
    let mut resize_delta = 0.0;
    theme::explorer_section_frame(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width().max(0.0));
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut title_clicked = false;
        let mut header = state.show_header(ui, |ui| {
            let response = ui.add_sized(
                [
                    ui.available_width().max(0.0),
                    METRICS.explorer.section_header_height,
                ],
                egui::Button::new((RichText::new(spec.title).strong(), egui::Atom::grow()))
                    .frame(false),
            );
            title_clicked = response.clicked();
        });
        if title_clicked {
            header.toggle();
        }
        header.body_unindented(|ui| {
            let handle_height = if spec.show_resize_handle {
                EXPLORER_SECTION_RESIZE_HANDLE_HEIGHT.min(spec.body_height.max(0.0))
            } else {
                0.0
            };
            egui::ScrollArea::both()
                .id_salt((spec.id_salt, "scroll", spec.filtered))
                .max_width(ui.available_width().max(0.0))
                .max_height((spec.body_height - handle_height).max(0.0))
                .min_scrolled_width(0.0)
                .min_scrolled_height(0.0)
                .auto_shrink([false, false])
                .show(ui, add_body);
            if handle_height > 0.0 {
                let (rect, response) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width().max(0.0), handle_height),
                    Sense::drag(),
                );
                let response = response.on_hover_cursor(egui::CursorIcon::ResizeVertical);
                let stroke = if response.hovered() || response.dragged() {
                    Stroke::new(1.5, ui.visuals().widgets.hovered.fg_stroke.color)
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke
                };
                ui.painter().line_segment(
                    [
                        Pos2::new(rect.left(), rect.center().y),
                        Pos2::new(rect.right(), rect.center().y),
                    ],
                    stroke,
                );
                resize_delta = response.drag_delta().y;
            }
        });
    });
    resize_delta
}

#[derive(Default)]
pub(super) struct ExplorerProjectSectionOutcome {
    pub(super) open_package_manager: bool,
    pub(super) target: Option<(PathBuf, usize)>,
}

pub(super) fn show_project_index_section(
    ui: &mut egui::Ui,
    section: ExplorerSection,
    root: &Path,
    index: &ProjectIndex,
    query: &str,
) -> ExplorerProjectSectionOutcome {
    let mut outcome = ExplorerProjectSectionOutcome::default();
    let filtered = !query.is_empty();
    match section {
        ExplorerSection::Contents => {
            let mut entries = index
                .outline
                .iter()
                .filter(|entry| outline_entry_matches_query(entry, query))
                .peekable();
            if entries.peek().is_none() {
                ui.label(
                    RichText::new(if filtered {
                        "No matches"
                    } else {
                        "No headings"
                    })
                    .size(theme::TYPE.supporting)
                    .weak(),
                );
                return outcome;
            }
            for entry in entries {
                let indent = entry
                    .level
                    .saturating_sub(1)
                    .min(METRICS.explorer.outline_max_depth) as f32
                    * METRICS.explorer.outline_indent;
                let response =
                    explorer_index_row(ui, &entry.title, Some(&entry.line.to_string()), indent);
                if response.clicked() {
                    outcome.target = Some((entry.path.clone(), entry.line));
                }
            }
        }
        ExplorerSection::Subfiles => {
            let mut paths = index
                .subfiles
                .iter()
                .filter(|path| explorer_path_matches_query(path, query))
                .peekable();
            if paths.peek().is_none() {
                ui.label(
                    RichText::new(if filtered {
                        "No matches"
                    } else {
                        "No included files"
                    })
                    .size(theme::TYPE.supporting)
                    .weak(),
                );
                return outcome;
            }
            for path in paths {
                let label = project_relative_path(root, path);
                let response = explorer_index_row(ui, &label, None, 0.0);
                if native_hover_text(response, path.display().to_string()).clicked() {
                    outcome.target = Some((path.clone(), 1));
                }
            }
        }
        ExplorerSection::Symbols => {
            let mut symbols = index
                .symbols
                .iter()
                .filter(|entry| symbol_entry_matches_query(entry, query))
                .peekable();
            if symbols.peek().is_none() {
                ui.label(
                    RichText::new(if filtered {
                        "No matches"
                    } else {
                        "No definitions or functions"
                    })
                    .size(theme::TYPE.supporting)
                    .weak(),
                );
                return outcome;
            }
            for symbol in symbols {
                let kind = symbol.kind.label();
                let location = format!(
                    "{}:{} · {kind}",
                    project_relative_path(root, &symbol.path),
                    symbol.line
                );
                let response = explorer_index_row(ui, &symbol.name, Some(kind), 0.0);
                if native_hover_text(response, location).clicked() {
                    outcome.target = Some((symbol.path.clone(), symbol.line));
                }
            }
        }
        ExplorerSection::Packages => {
            if ui.button("Browse packages…").clicked() {
                outcome.open_package_manager = true;
            }
            ui.separator();
            let mut packages = index
                .packages
                .iter()
                .filter(|package| explorer_text_matches_query(package, query))
                .peekable();
            if packages.peek().is_none() {
                ui.label(
                    RichText::new(if filtered {
                        "No matches"
                    } else {
                        "No packages"
                    })
                    .size(theme::TYPE.supporting)
                    .weak(),
                );
            } else {
                for package in packages {
                    explorer_index_row(ui, package, None, 0.0);
                }
            }
        }
        ExplorerSection::Tags | ExplorerSection::References => {
            let entries = if section == ExplorerSection::Tags {
                &index.tags
            } else {
                &index.references
            };
            let mut references = entries
                .iter()
                .filter(|entry| reference_entry_matches_query(entry, query))
                .peekable();
            if references.peek().is_none() {
                ui.label(
                    RichText::new(if filtered {
                        "No matches"
                    } else if section == ExplorerSection::Tags {
                        "No tags"
                    } else {
                        "No references"
                    })
                    .size(theme::TYPE.supporting)
                    .weak(),
                );
                return outcome;
            }
            for reference in references {
                let location = format!(
                    "{}:{}",
                    project_relative_path(root, &reference.path),
                    reference.line
                );
                let response = explorer_index_row(
                    ui,
                    &reference.label,
                    Some(&reference.line.to_string()),
                    0.0,
                );
                if native_hover_text(response, location).clicked() {
                    outcome.target = Some((reference.path.clone(), reference.line));
                }
            }
        }
        ExplorerSection::Files | ExplorerSection::Git => unreachable!("not a project index panel"),
    }
    outcome
}

pub(super) fn project_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

pub(super) fn explorer_index_row(
    ui: &mut egui::Ui,
    label: &str,
    detail: Option<&str>,
    indent: f32,
) -> egui::Response {
    let size = Vec2::new(ui.available_width().max(1.0), METRICS.explorer.row_height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let visuals = ui.style().interact(&response);
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, theme::RADIUS.row as f32, visuals.weak_bg_fill);
    }

    let painter = ui.painter().with_clip_rect(rect);
    let font = theme::supporting_font();
    let left = rect.left() + theme::SPACE.small + indent;
    let show_detail = detail.is_some() && rect.width() >= METRICS.explorer.detail_breakpoint;
    let detail_width = if show_detail {
        METRICS.explorer.detail_width
    } else {
        0.0
    };
    let label_clip = Rect::from_min_max(
        Pos2::new(left, rect.top()),
        Pos2::new(
            (rect.right() - detail_width - theme::SPACE.small).max(left),
            rect.bottom(),
        ),
    );
    painter.with_clip_rect(label_clip).text(
        Pos2::new(left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font.clone(),
        visuals.fg_stroke.color,
    );
    if show_detail {
        painter.text(
            Pos2::new(rect.right() - theme::SPACE.small, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            detail.unwrap_or_default(),
            font,
            ui.visuals().weak_text_color(),
        );
    }
    response
}

pub(super) fn paint_tree_icon(ui: &mut egui::Ui, folder: bool) {
    let rect = ui.available_rect_before_wrap().shrink(theme::SPACE.tight);
    let size = METRICS.explorer.tree_icon_size.min(rect.size());
    let rect = Rect::from_center_size(rect.center(), size);
    let color = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let stroke = Stroke::new(METRICS.explorer.tree_icon_stroke, color);
    if folder {
        let body = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + 3.0),
            Pos2::new(rect.right(), rect.bottom()),
        );
        ui.painter()
            .rect_stroke(body, 1.5, stroke, StrokeKind::Inside);
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 1.5, rect.top() + 3.0),
                Pos2::new(rect.left() + 4.5, rect.top()),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 4.5, rect.top()),
                Pos2::new(rect.left() + 8.0, rect.top() + 3.0),
            ],
            stroke,
        );
    } else {
        ui.painter()
            .rect_stroke(rect, 1.2, stroke, StrokeKind::Inside);
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 3.0, rect.top() + 4.0),
                Pos2::new(rect.right() - 3.0, rect.top() + 4.0),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 3.0, rect.top() + 7.0),
                Pos2::new(rect.right() - 3.0, rect.top() + 7.0),
            ],
            stroke,
        );
    }
}
