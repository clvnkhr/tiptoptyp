//! Owner-local source destinations. Search keeps its focus; navigation takes it.
use super::{
    DeferredDocumentAction, DocumentKind, char_index_at_line_column, line_column_at_char, same_path,
};
use crate::diagnostics::{Diagnostic, DiagnosticSource};
use std::{
    ops::Range,
    path::{Path, PathBuf},
    time::Instant,
};
use tiptoptyp_core::text::{LspRange, ScalarOffset, range_to_scalar_range};

use super::{EditorApp, EditorAttention, ViewMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum EditorSelection {
    Focus(Range<usize>),
    Search(Range<usize>),
}

impl EditorSelection {
    pub(super) fn range(&self) -> &Range<usize> {
        match self {
            Self::Focus(range) | Self::Search(range) => range,
        }
    }

    pub(super) fn takes_focus(&self) -> bool {
        matches!(self, Self::Focus(_))
    }
}

impl EditorApp {
    fn is_current_source_path(&self, path: &Path) -> bool {
        self.document().path().as_deref().map_or_else(
            || same_path(&self.tinymist_document_path(), path),
            |current| same_path(current, path),
        )
    }

    pub(super) fn diagnostic_targets_current_document(&self, diagnostic: &Diagnostic) -> bool {
        match &diagnostic.source {
            DiagnosticSource::Main => self.current_is_preview_document(),
            DiagnosticSource::File(path) => {
                if self.document().config().is_some() && !path.is_absolute() {
                    return self
                        .diagnostic_target_path(diagnostic)
                        .is_some_and(|path| self.is_current_source_path(&path));
                }
                self.is_current_source_path(path)
            }
            DiagnosticSource::Global => false,
        }
    }

    pub(super) fn diagnostic_target_path(&self, diagnostic: &Diagnostic) -> Option<PathBuf> {
        match &diagnostic.source {
            DiagnosticSource::Main => Some(self.preview_document_path()),
            DiagnosticSource::File(path) if path.is_absolute() => Some(path.clone()),
            DiagnosticSource::File(path) => {
                let preview_dir = self
                    .preview_document_path()
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.project_root());
                let from_preview = preview_dir.join(path);
                if from_preview.exists() {
                    Some(from_preview)
                } else {
                    Some(self.project_root().join(path))
                }
            }
            DiagnosticSource::Global => None,
        }
    }

    pub(super) fn jump_to_diagnostic(&mut self, diagnostic: Diagnostic) {
        let Some(location) = diagnostic.location else {
            return;
        };
        if self.diagnostic_targets_current_document(&diagnostic) {
            self.apply_editor_location(None, Some((location.line, location.column)));
            return;
        }
        let Some(path) = self.diagnostic_target_path(&diagnostic) else {
            return;
        };
        self.navigate_file_location(
            path,
            None,
            Some((location.line, location.column)),
            "opening a diagnostic location",
        );
    }

    pub(super) fn navigate_file_location(
        &mut self,
        path: PathBuf,
        page: Option<usize>,
        source_position: Option<(usize, usize)>,
        description: &str,
    ) {
        if self.is_current_source_path(&path) {
            self.apply_file_link_location(page, source_position);
        } else {
            self.request_document_replacement(
                DeferredDocumentAction::FollowFileLink {
                    path,
                    page,
                    source_position,
                },
                description,
            );
        }
    }

    pub(super) fn apply_file_link_location(
        &mut self,
        page: Option<usize>,
        source_position: Option<(usize, usize)>,
    ) {
        let source_position = if self.document().config().is_some() {
            source_position.and_then(|(line, column)| {
                let snapshot = self.document().canonical_snapshot().ok()?;
                let cursor = char_index_at_line_column(snapshot.source(), line, column);
                let cursor = snapshot.editor_scalar_cursor(ScalarOffset::new(cursor))?;
                Some(line_column_at_char(snapshot.editor_source(), cursor.get()))
            })
        } else {
            source_position
        };
        self.apply_editor_location(page, source_position);
    }

    pub(super) fn apply_editor_location(
        &mut self,
        page: Option<usize>,
        source_position: Option<(usize, usize)>,
    ) {
        if self.document().kind() == DocumentKind::Pdf {
            if self.asset_preview.content.pdf().is_none() {
                self.pending_asset_page = page;
            } else if let Some(page) = page {
                self.pdfium_asset.go_to_page(page);
            }
        } else if self.document().kind().is_editable()
            && let Some((line, column)) = source_position
        {
            let char_index = char_index_at_line_column(self.document().source(), line, column);
            self.navigate_editor_range(char_index..char_index);
        }
    }

    pub(super) fn follow_tinymist_location(&mut self, uri: &str, selection: Option<&LspRange>) {
        let Ok(url) = url::Url::parse(uri) else {
            return;
        };
        let Ok(path) = url.to_file_path() else {
            return;
        };
        if !self.is_current_source_path(&path) {
            if path
                .extension()
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("typ"))
            {
                return;
            }
            self.request_document_replacement(
                DeferredDocumentAction::FollowTinymistLocation {
                    path,
                    selection: selection.cloned(),
                },
                "following the preview location",
            );
            return;
        }
        self.apply_tinymist_selection(selection);
    }

    pub(super) fn apply_tinymist_selection(&mut self, selection: Option<&LspRange>) {
        if let Some(selection) = selection {
            let range = if self.document().config().is_some() {
                let Some(range) = self
                    .document()
                    .canonical_snapshot()
                    .ok()
                    .and_then(|snapshot| snapshot.editor_range(*selection))
                else {
                    return;
                };
                range
            } else {
                range_to_scalar_range(self.document().source(), selection).into_range()
            };
            self.navigate_editor_range(range);
        }
        self.view_mode = ViewMode::Split;
    }

    pub(super) fn navigate_editor_range(&mut self, range: Range<usize>) {
        self.editor_attention = Some(EditorAttention {
            char_index: range.start,
            started: Instant::now(),
        });
        self.pending_editor_selection = Some(EditorSelection::Focus(range));
        self.find_bar.focus = false;
        if self.document().kind().is_typst() {
            self.view_mode = ViewMode::Split;
        }
    }
}
