use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use typst_syntax::{LinkedNode, Source, SyntaxKind, ast};

use crate::diagnostics::{Diagnostic, DiagnosticSeverity, DiagnosticSource};

/// Identity of the source metadata derived for one open document revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EditorRevision {
    pub(crate) document_epoch: u64,
    pub(crate) revision: u64,
}

impl EditorRevision {
    pub(crate) const fn new(document_epoch: u64, revision: u64) -> Self {
        Self {
            document_epoch,
            revision,
        }
    }

    pub(crate) const fn after_edit(self) -> Self {
        Self {
            document_epoch: self.document_epoch,
            revision: self.revision.wrapping_add(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceMetrics {
    pub(crate) line_count: usize,
    pub(crate) longest_line_chars: usize,
    pub(crate) char_count: usize,
}

impl Default for SourceMetrics {
    fn default() -> Self {
        Self {
            line_count: 1,
            longest_line_chars: 0,
            char_count: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FontArgumentTarget {
    pub(crate) value_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineDiagnostic {
    pub(crate) line: usize,
    pub(crate) severity: DiagnosticSeverity,
    pub(crate) summary: String,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiagnosticsKey {
    document: EditorRevision,
    generation: u64,
    current_is_preview: bool,
    current_path: Option<PathBuf>,
    virtual_path: PathBuf,
}

impl DiagnosticsKey {
    fn matches(
        &self,
        document: EditorRevision,
        generation: u64,
        current_path: Option<&Path>,
        virtual_path: &Path,
        current_is_preview: bool,
    ) -> bool {
        self.document == document
            && self.generation == generation
            && self.current_is_preview == current_is_preview
            && self.current_path.as_deref() == current_path
            && self.virtual_path == virtual_path
    }
}

/// Revision-derived text, syntax and diagnostic data consumed by the editor.
/// Unchanged frames only copy small values or `Arc` handles.
pub(crate) struct EditorDerivedData {
    source_key: Option<EditorRevision>,
    source_snapshot: Arc<str>,
    source_metrics: SourceMetrics,
    char_starts: Vec<usize>,
    parsed_source: Source,
    parsed_key: Option<EditorRevision>,
    diagnostics_key: Option<DiagnosticsKey>,
    line_diagnostics: Arc<[LineDiagnostic]>,
    longest_diagnostic_chars: usize,
    #[cfg(test)]
    source_rebuilds: usize,
    #[cfg(test)]
    diagnostic_rebuilds: usize,
    #[cfg(test)]
    syntax_rebuilds: usize,
}

impl Default for EditorDerivedData {
    fn default() -> Self {
        Self {
            source_key: None,
            source_snapshot: Arc::from(""),
            source_metrics: SourceMetrics::default(),
            char_starts: vec![0],
            parsed_source: Source::detached(String::new()),
            parsed_key: None,
            diagnostics_key: None,
            line_diagnostics: Arc::from([]),
            longest_diagnostic_chars: 0,
            #[cfg(test)]
            source_rebuilds: 0,
            #[cfg(test)]
            diagnostic_rebuilds: 0,
            #[cfg(test)]
            syntax_rebuilds: 0,
        }
    }
}

impl EditorDerivedData {
    pub(crate) fn prepare_source(&mut self, key: EditorRevision, source: &str) {
        if self.source_key == Some(key) {
            return;
        }

        let mut line_count = 1;
        let mut char_starts = Vec::with_capacity(source.len().saturating_add(1));
        char_starts.push(0);
        let mut current_line_chars = 0;
        let mut longest_line_chars = 0;
        for (byte, character) in source.char_indices() {
            let next_byte = byte + character.len_utf8();
            char_starts.push(next_byte);
            if character == '\n' {
                longest_line_chars = longest_line_chars.max(current_line_chars);
                current_line_chars = 0;
                line_count += 1;
            } else {
                current_line_chars += 1;
            }
        }
        longest_line_chars = longest_line_chars.max(current_line_chars);

        self.source_snapshot = Arc::from(source);
        self.source_metrics = SourceMetrics {
            line_count,
            longest_line_chars,
            char_count: char_starts.len() - 1,
        };
        self.char_starts = char_starts;
        self.source_key = Some(key);
        #[cfg(test)]
        {
            self.source_rebuilds += 1;
        }
    }

    fn prepare_syntax(&mut self) {
        if self.parsed_key == self.source_key {
            return;
        }
        self.parsed_source.replace(&self.source_snapshot);
        self.parsed_key = self.source_key;
        #[cfg(test)]
        {
            self.syntax_rebuilds += 1;
        }
    }

    pub(crate) fn source_metrics(&self) -> SourceMetrics {
        self.source_metrics
    }

    pub(crate) fn source_snapshot(&self) -> Arc<str> {
        Arc::clone(&self.source_snapshot)
    }

    pub(crate) fn char_to_byte(&self, character: usize) -> usize {
        self.char_starts
            .get(character)
            .copied()
            .unwrap_or(self.source_snapshot.len())
    }

    pub(crate) fn char_range_to_byte(&self, range: Range<usize>) -> Range<usize> {
        self.char_to_byte(range.start)..self.char_to_byte(range.end)
    }

    pub(crate) fn font_argument_at(&mut self, char_index: usize) -> Option<FontArgumentTarget> {
        fn walk(node: LinkedNode<'_>, cursor: usize) -> Option<FontArgumentTarget> {
            if node.kind() == SyntaxKind::SetRule
                && node.range().start <= cursor
                && cursor <= node.range().end
            {
                let target_is_text = node
                    .children()
                    .find(|child| child.kind() == SyntaxKind::Ident)
                    .is_some_and(|target| target.leaf_text() == "text");
                if target_is_text
                    && let Some(arguments) = node
                        .children()
                        .find(|child| child.kind() == SyntaxKind::Args)
                {
                    for named in arguments
                        .children()
                        .filter(|child| child.kind() == SyntaxKind::Named)
                    {
                        if cursor < named.range().start || cursor > named.range().end {
                            continue;
                        }
                        let name_is_font = named
                            .children()
                            .find(|child| child.kind() == SyntaxKind::Ident)
                            .is_some_and(|name| name.leaf_text() == "font");
                        let value = named
                            .children()
                            .find(|child| child.kind() == SyntaxKind::Str);
                        if name_is_font
                            && let Some(value) = value
                            && value.len() >= 2
                        {
                            return Some(FontArgumentTarget {
                                value_range: value.range().start + 1..value.range().end - 1,
                            });
                        }
                    }
                }
            }
            node.children().find_map(|child| walk(child, cursor))
        }

        self.prepare_syntax();
        walk(
            LinkedNode::new(self.parsed_source.root()),
            self.char_to_byte(char_index),
        )
    }

    /// Return the literal argument of a `#link("...")` call under the cursor.
    /// URL policy remains with the application boundary.
    pub(crate) fn web_link_at(&mut self, char_index: usize) -> Option<String> {
        fn walk(node: LinkedNode<'_>, source: &str, cursor: usize) -> Option<String> {
            if node.kind() == SyntaxKind::FuncCall
                && node.range().start <= cursor
                && cursor <= node.range().end
                && node.range().start > 0
                && source.as_bytes().get(node.range().start - 1) == Some(&b'#')
                && let Some(call) = node.get().cast::<ast::FuncCall>()
                && matches!(call.callee(), ast::Expr::Ident(ident) if ident.as_str() == "link")
                && let Some(ast::Arg::Pos(ast::Expr::Str(target))) = call.args().items().next()
            {
                return Some(target.get().into());
            }
            node.children()
                .find_map(|child| walk(child, source, cursor))
        }

        self.prepare_syntax();
        walk(
            LinkedNode::new(self.parsed_source.root()),
            &self.source_snapshot,
            self.char_to_byte(char_index),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_diagnostics(
        &mut self,
        document: EditorRevision,
        generation: u64,
        current_path: Option<&Path>,
        virtual_path: &Path,
        current_is_preview: bool,
        compiler: &[Diagnostic],
        language_server: &[Diagnostic],
    ) {
        if self.diagnostics_key.as_ref().is_some_and(|key| {
            key.matches(
                document,
                generation,
                current_path,
                virtual_path,
                current_is_preview,
            )
        }) {
            return;
        }

        let key = DiagnosticsKey {
            document,
            generation,
            current_is_preview,
            current_path: current_path.map(Path::to_owned),
            virtual_path: virtual_path.to_owned(),
        };

        let current_identity = current_path.map(FileIdentity::for_open_file);
        let virtual_identity = current_path
            .is_none()
            .then(|| FileIdentity::virtual_path(virtual_path));
        let targets_current_document = |diagnostic: &Diagnostic| match &diagnostic.source {
            DiagnosticSource::Main => current_is_preview,
            DiagnosticSource::File(path) => {
                current_identity
                    .as_ref()
                    .is_some_and(|current| current.matches(path))
                    || virtual_identity
                        .as_ref()
                        .is_some_and(|current| current.matches(path))
            }
            DiagnosticSource::Global => false,
        };

        let mut by_line: BTreeMap<usize, Vec<&Diagnostic>> = BTreeMap::new();
        for diagnostic in compiler.iter().chain(language_server) {
            if targets_current_document(diagnostic)
                && let Some(line) = diagnostic.line()
            {
                let entries = by_line.entry(line).or_default();
                if let Some(existing) = entries.iter_mut().find(|existing| {
                    existing.message == diagnostic.message
                        && existing.severity == diagnostic.severity
                }) {
                    if diagnostic.details.len() > existing.details.len() {
                        *existing = diagnostic;
                    }
                } else {
                    entries.push(diagnostic);
                }
            }
        }

        let diagnostics = by_line
            .into_iter()
            .map(|(line, diagnostics)| {
                let severity = diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.severity)
                    .min_by_key(|severity| severity_rank(*severity))
                    .unwrap_or(DiagnosticSeverity::Unknown);
                let summary = diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let detail = diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.full_message())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                LineDiagnostic {
                    line,
                    severity,
                    summary,
                    detail,
                }
            })
            .collect::<Vec<_>>();
        self.longest_diagnostic_chars = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.summary.chars().count())
            .max()
            .unwrap_or(0);
        self.line_diagnostics = diagnostics.into();
        self.diagnostics_key = Some(key);
        #[cfg(test)]
        {
            self.diagnostic_rebuilds += 1;
        }
    }

    pub(crate) fn line_diagnostics(&self) -> Arc<[LineDiagnostic]> {
        Arc::clone(&self.line_diagnostics)
    }

    pub(crate) fn longest_diagnostic_chars(&self) -> usize {
        self.longest_diagnostic_chars
    }

    #[cfg(test)]
    fn rebuild_counts(&self) -> (usize, usize) {
        (self.source_rebuilds, self.diagnostic_rebuilds)
    }

    #[cfg(test)]
    fn syntax_rebuild_count(&self) -> usize {
        self.syntax_rebuilds
    }
}

/// Canonical identity is resolved only while rebuilding prepared diagnostics,
/// never for each row of an unchanged frame.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    path: PathBuf,
    canonical: bool,
}

impl FileIdentity {
    fn for_open_file(path: &Path) -> Self {
        path.canonicalize().map_or_else(
            |_| Self::virtual_path(path),
            |path| Self {
                path,
                canonical: true,
            },
        )
    }

    fn virtual_path(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            canonical: false,
        }
    }

    fn matches(&self, path: &Path) -> bool {
        if self.canonical {
            path.canonicalize().is_ok_and(|path| path == self.path)
        } else {
            path == self.path
        }
    }
}

fn severity_rank(severity: DiagnosticSeverity) -> u8 {
    match severity {
        DiagnosticSeverity::Error => 0,
        DiagnosticSeverity::Warning => 1,
        DiagnosticSeverity::Help => 2,
        DiagnosticSeverity::Note => 3,
        DiagnosticSeverity::Unknown => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::DiagnosticLocation;

    fn revision(revision: u64) -> EditorRevision {
        EditorRevision::new(3, revision)
    }

    fn diagnostic(path: &Path, line: usize, message: &str) -> Diagnostic {
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            source: DiagnosticSource::File(path.to_owned()),
            location: Some(DiagnosticLocation { line, column: 1 }),
            message: message.to_owned(),
            details: Vec::new(),
        }
    }

    #[test]
    fn unchanged_revision_reuses_source_metrics_offsets_and_snapshot() {
        let mut data = EditorDerivedData::default();
        let source = "short\n🦀 longest\n";
        data.prepare_source(revision(4), source);
        assert_eq!(
            data.source_metrics(),
            SourceMetrics {
                line_count: 3,
                longest_line_chars: 9,
                char_count: 16,
            }
        );
        assert_eq!(data.char_range_to_byte(6..7), 6..10);
        let first_snapshot = data.source_snapshot();
        data.prepare_source(revision(4), source);
        let second_snapshot = data.source_snapshot();
        assert!(Arc::ptr_eq(&first_snapshot, &second_snapshot));
        assert_eq!(data.rebuild_counts(), (1, 0));

        data.prepare_source(revision(5), "one line");
        assert_eq!(data.source_metrics().line_count, 1);
        assert_eq!(&*first_snapshot, source, "undo snapshot retains old text");
        assert!(!Arc::ptr_eq(&first_snapshot, &data.source_snapshot()));
        assert_eq!(data.rebuild_counts(), (2, 0));
    }

    #[test]
    fn parsed_queries_reuse_one_revision_snapshot() {
        let mut data = EditorDerivedData::default();
        let source = "#set text(font: \"Libertinus Serif\")\n#link(\"https://example.com\")[site]";
        data.prepare_source(revision(1), source);

        let font = source[..source.find("font").unwrap()].chars().count();
        let link = source[..source.find("https").unwrap()].chars().count();
        assert_eq!(
            &source[data.font_argument_at(font).unwrap().value_range],
            "Libertinus Serif"
        );
        assert_eq!(
            data.web_link_at(link).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(data.rebuild_counts(), (1, 0));
        assert_eq!(data.syntax_rebuild_count(), 1);
    }

    #[test]
    fn diagnostics_are_cached_by_document_and_diagnostic_generation() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("main.typ");
        std::fs::write(&path, "= Main").unwrap();
        let diagnostics = [diagnostic(&path, 2, "broken")];
        let mut data = EditorDerivedData::default();
        data.prepare_source(revision(1), "= Main");

        data.prepare_diagnostics(revision(1), 8, Some(&path), &path, true, &diagnostics, &[]);
        data.prepare_diagnostics(revision(1), 8, Some(&path), &path, true, &diagnostics, &[]);
        assert_eq!(data.line_diagnostics().len(), 1);
        assert_eq!(data.longest_diagnostic_chars(), 6);
        assert_eq!(data.rebuild_counts(), (1, 1));

        data.prepare_diagnostics(revision(1), 9, Some(&path), &path, true, &[], &[]);
        assert!(data.line_diagnostics().is_empty());
        assert_eq!(data.rebuild_counts(), (1, 2));
    }

    #[test]
    fn diagnostic_cache_is_invalidated_when_the_document_path_changes() {
        let project = tempfile::tempdir().unwrap();
        let first = project.path().join("first.typ");
        let second = project.path().join("second.typ");
        std::fs::write(&first, "= First").unwrap();
        std::fs::write(&second, "= Second").unwrap();
        let diagnostics = [diagnostic(&first, 1, "first only")];
        let mut data = EditorDerivedData::default();

        data.prepare_diagnostics(
            revision(1),
            3,
            Some(&first),
            &first,
            true,
            &diagnostics,
            &[],
        );
        assert_eq!(data.line_diagnostics().len(), 1);

        // A Save As/open transition can preserve both revision counters. Path
        // identity must still invalidate diagnostic targeting.
        data.prepare_diagnostics(
            revision(1),
            3,
            Some(&second),
            &second,
            true,
            &diagnostics,
            &[],
        );
        assert!(data.line_diagnostics().is_empty());
        assert_eq!(data.rebuild_counts(), (0, 2));
    }

    #[test]
    fn representative_large_document_is_scanned_once_per_revision() {
        let source = (0..50_000)
            .map(|line| format!("line {line}: αβγ\n"))
            .collect::<String>();
        let mut data = EditorDerivedData::default();
        for _ in 0..120 {
            data.prepare_source(revision(11), &source);
            assert_eq!(data.source_metrics().line_count, 50_001);
        }
        assert_eq!(data.rebuild_counts(), (1, 0));
    }
}
