use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use typst_syntax::{LinkedNode, Source, SyntaxKind, ast};

use crate::{
    diagnostics::{Diagnostic, DiagnosticSeverity, DiagnosticSource},
    editor_features::{ContextRegion, StickyContextQuery, context_regions},
};

use crate::document::{DocumentKey, DocumentSnapshot};

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
    document: DocumentKey,
    generation: u64,
    current_is_preview: bool,
    current_path: Option<PathBuf>,
    virtual_path: PathBuf,
}

impl DiagnosticsKey {
    fn matches(
        &self,
        document: DocumentKey,
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
    source_key: Option<DocumentKey>,
    source_snapshot: Arc<str>,
    source_metrics: SourceMetrics,
    char_starts: Vec<usize>,
    parsed_source: Source,
    parsed_key: Option<DocumentKey>,
    tex_key: Option<DocumentKey>,
    mitex_dollars: bool,
    mitex_compatibility: Option<(DocumentKey, String, bool)>,
    tex_index: crate::tex_completion::Index,
    regions_key: Option<DocumentKey>,
    regions: Arc<[ContextRegion]>,
    delimiter_query: Option<(DocumentKey, usize)>,
    delimiter_pair: Option<[Range<usize>; 2]>,
    table_query: Option<(DocumentKey, usize)>,
    table_at_cursor: Option<crate::editor_features::EditableTable>,
    hover_query: Option<(DocumentKey, usize)>,
    hover_range: Option<Range<usize>>,
    diagnostics_key: Option<DiagnosticsKey>,
    line_diagnostics: Arc<[LineDiagnostic]>,
    longest_diagnostic_chars: usize,
    #[cfg(test)]
    source_rebuilds: usize,
    #[cfg(test)]
    diagnostic_rebuilds: usize,
    #[cfg(test)]
    syntax_rebuilds: usize,
    #[cfg(test)]
    hover_queries: usize,
    #[cfg(test)]
    tex_rebuilds: usize,
    #[cfg(test)]
    compatibility_checks: usize,
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
            tex_key: None,
            mitex_dollars: false,
            mitex_compatibility: None,
            tex_index: crate::tex_completion::Index::default(),
            regions_key: None,
            regions: Arc::from([]),
            delimiter_query: None,
            delimiter_pair: None,
            table_query: None,
            table_at_cursor: None,
            hover_query: None,
            hover_range: None,
            diagnostics_key: None,
            line_diagnostics: Arc::from([]),
            longest_diagnostic_chars: 0,
            #[cfg(test)]
            source_rebuilds: 0,
            #[cfg(test)]
            diagnostic_rebuilds: 0,
            #[cfg(test)]
            syntax_rebuilds: 0,
            #[cfg(test)]
            hover_queries: 0,
            #[cfg(test)]
            tex_rebuilds: 0,
            #[cfg(test)]
            compatibility_checks: 0,
        }
    }
}

impl EditorDerivedData {
    pub(crate) fn mitex_compatible(&mut self, version: &str) -> bool {
        let Some(key) = self.source_key else {
            return false;
        };
        let version = version.trim();
        if let Some((cached_key, cached_version, compatible)) = &self.mitex_compatibility
            && *cached_key == key
            && cached_version == version
        {
            return *compatible;
        }
        self.prepare_syntax();
        let compatible = tiptoptyp::mitex_projection::Projection::compatible(
            &self.parsed_source,
            &tiptoptyp::mitex_projection::Config {
                package: format!("@preview/mitex:{version}"),
            },
        );
        self.mitex_compatibility = Some((key, version.to_owned(), compatible));
        #[cfg(test)]
        {
            self.compatibility_checks += 1;
        }
        compatible
    }
    pub(crate) fn prepare_source(&mut self, snapshot: &DocumentSnapshot) {
        let key = snapshot.key();
        let source = snapshot.source();
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

    pub(crate) fn literal_asset_target_at(
        &mut self,
        cursor: usize,
        source_path: &std::path::Path,
        workspace_root: &std::path::Path,
    ) -> Option<crate::editor_features::LiteralAssetTarget> {
        self.prepare_syntax();
        let byte = *self.char_starts.get(cursor)?;
        crate::editor_features::find_literal_asset(
            &typst_syntax::LinkedNode::new(self.parsed_source.root()),
            &self.source_snapshot,
            byte,
            source_path,
            workspace_root,
        )
    }

    pub(crate) fn tex_completions(
        &mut self,
        cursor: usize,
    ) -> Option<Vec<crate::tinymist::CompletionItem>> {
        self.prepare_syntax();
        if self.tex_key != self.source_key {
            self.tex_index = if self.mitex_dollars {
                crate::tex_completion::Index::projected(&self.parsed_source)
            } else {
                crate::tex_completion::Index::new(&self.parsed_source)
            };
            self.tex_key = self.source_key;
            #[cfg(test)]
            {
                self.tex_rebuilds += 1;
            }
        }
        self.tex_index.items(&self.source_snapshot, cursor)
    }

    pub(crate) fn set_mitex_dollars(&mut self, enabled: bool) {
        if self.mitex_dollars != enabled {
            self.mitex_dollars = enabled;
            self.tex_key = None;
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

    /// Hover at an editor insertion position, using the actual syntax leaf.
    /// In math `_`/`-` separate expressions; in code they can belong to an
    /// identifier. A context-free word scan sends requests for the wrong leaf.
    pub(crate) fn hover_token_range(&mut self, character: usize) -> Option<Range<usize>> {
        let key = self.source_key?;
        if self.hover_query == Some((key, character)) {
            return self.hover_range.clone();
        }
        self.prepare_syntax();
        let range = self.char_starts.get(character).and_then(|&cursor| {
            let root = LinkedNode::new(self.parsed_source.root());
            [typst_syntax::Side::After, typst_syntax::Side::Before]
                .into_iter()
                .find_map(|side| {
                    let leaf = root.leaf_at(cursor, side)?;
                    let range = hover_leaf_range(&leaf, cursor)?;
                    Some(
                        self.char_starts.partition_point(|byte| *byte < range.start)
                            ..self.char_starts.partition_point(|byte| *byte < range.end),
                    )
                })
        });
        self.hover_query = Some((key, character));
        self.hover_range = range.clone();
        #[cfg(test)]
        {
            self.hover_queries += 1;
        }
        range
    }

    pub(crate) fn matching_delimiters(&mut self, caret: usize) -> Option<[Range<usize>; 2]> {
        let key = self.source_key?;
        if self.delimiter_query == Some((key, caret)) {
            return self.delimiter_pair.clone();
        }
        // Prefer the character under the caret, then the one immediately before it.
        let candidates = [Some(caret), caret.checked_sub(1)].map(|character| {
            let character =
                character.filter(|character| *character < self.source_metrics.char_count)?;
            let byte = self.char_to_byte(character);
            let candidate = self.source_snapshot[byte..].chars().next()?;
            "()[]{}$\"<>`*_|⟨⟩⌈⌉⌊⌋‖".contains(candidate).then_some(byte)
        });
        if candidates.iter().any(Option::is_some) {
            self.prepare_syntax();
        }
        let pair = candidates
            .into_iter()
            .flatten()
            .find_map(|byte| crate::delimiters::pair_at(&self.parsed_source, byte));
        self.delimiter_pair = pair.map(|pair| {
            pair.map(|endpoint| {
                self.char_starts
                    .partition_point(|byte| *byte < endpoint.start)
                    ..self
                        .char_starts
                        .partition_point(|byte| *byte < endpoint.end)
            })
        });
        self.delimiter_query = Some((key, caret));
        self.delimiter_pair.clone()
    }

    pub(crate) fn context_regions(&mut self) -> Arc<[ContextRegion]> {
        self.prepare_regions();
        Arc::clone(&self.regions)
    }

    pub(crate) fn prepare_table_at_cursor(&mut self, cursor: usize) {
        let Some(key) = self.source_key else {
            return;
        };
        if self.table_query == Some((key, cursor)) {
            return;
        }
        self.prepare_syntax();
        self.table_at_cursor = crate::editor_features::editable_table_in_source(
            &self.parsed_source,
            self.char_to_byte(cursor),
        );
        self.table_query = Some((key, cursor));
    }

    pub(crate) fn cached_table_at_cursor(
        &self,
        key: DocumentKey,
    ) -> Option<&crate::editor_features::EditableTable> {
        self.table_query.filter(|(cached, _)| *cached == key)?;
        self.table_at_cursor.as_ref()
    }

    fn prepare_regions(&mut self) {
        if self.regions_key == self.source_key {
            return;
        }
        self.prepare_syntax();
        self.regions = context_regions(&self.parsed_source, &self.source_snapshot).into();
        self.regions_key = self.source_key;
    }

    /// Scrolling and folding share one structural index per source revision.
    pub(crate) fn sticky_context_query(&mut self) -> StickyContextQuery<'_> {
        self.prepare_regions();
        StickyContextQuery::new(&self.regions, &self.char_starts)
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
        document: DocumentKey,
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

/// Keep word-level hovers in strings/markup, but never cross a syntax leaf.
/// The previous leaf is considered at insertion boundaries because egui maps
/// the right half of a final glyph to the position *after* that glyph. The UI
/// still checks the resulting token rectangle against the actual pointer.
fn hover_leaf_range(leaf: &LinkedNode<'_>, cursor: usize) -> Option<Range<usize>> {
    let range = leaf.range();
    if matches!(leaf.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent) {
        return Some(range);
    }
    let text = leaf.leaf_text();
    let local = cursor.checked_sub(range.start)?;
    let (byte, character) = if local == text.len() {
        text.char_indices().next_back()?
    } else {
        (local, text.get(local..)?.chars().next()?)
    };
    let is_word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-');
    if !is_word(character) {
        return None;
    }
    let start = text[..byte]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(byte, |(byte, _)| byte);
    let end = byte
        + text[byte..]
            .char_indices()
            .find(|(_, c)| !is_word(*c))
            .map_or(text.len() - byte, |(byte, _)| byte);
    // Standalone math attachment/operator leaves are not identifiers.
    text[start..end]
        .chars()
        .any(char::is_alphanumeric)
        .then_some(range.start + start..range.start + end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_hover_reuses_syntax_across_pointer_moves() {
        let source = "α text\n".repeat(1000);
        let mut data = EditorDerivedData::default();
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), &source));
        for cursor in 0..100 {
            assert!(
                data.literal_asset_target_at(
                    cursor,
                    Path::new("/project/main.typ"),
                    Path::new("/project")
                )
                .is_none()
            );
        }
        assert_eq!(data.syntax_rebuilds, 1);
    }
    use crate::diagnostics::DiagnosticLocation;

    #[test]
    fn mitex_compatibility_reuses_syntax_and_only_checks_changed_revisions_or_versions() {
        let mut data = EditorDerivedData::default();
        let source = "#import \"@preview/mitex:0.2.7\": mi\n#mi(`x`)";
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        data.prepare_syntax();
        for _ in 0..100 {
            assert!(data.mitex_compatible("0.2.7"));
        }
        assert_eq!(data.syntax_rebuilds, 1);
        assert_eq!(data.compatibility_checks, 1);
        assert!(
            data.tex_key.is_none(),
            "compatibility must not construct completion data"
        );
        assert!(!data.mitex_compatible("0.2.6"));
        assert_eq!(data.syntax_rebuilds, 1);
        assert_eq!(data.compatibility_checks, 2);
        data.prepare_source(&DocumentSnapshot::fixture(revision(2), "$ native $"));
        assert!(!data.mitex_compatible("0.2.7"));
        assert_eq!(data.syntax_rebuilds, 2);
        assert_eq!(data.compatibility_checks, 3);
    }

    #[test]
    #[ignore = "opt-in matched compatibility-cache measurement; no timing assertion"]
    fn profile_mitex_compatibility_cache() {
        use std::{
            hint::black_box,
            time::{Duration, Instant},
        };
        let fixture =
            "// Unicode 文 😀 representative source for projection caching\n".repeat(5000);
        let mut baseline = EditorDerivedData::default();
        let mut checked = EditorDerivedData::default();
        let mut elapsed = [Duration::ZERO; 2];
        for iteration in 0..210 {
            let source = format!("{fixture}{}", if iteration % 2 == 0 { "a" } else { "b" });
            let snapshot = DocumentSnapshot::fixture(revision(iteration), &source);
            for which in if iteration % 2 == 0 { [0, 1] } else { [1, 0] } {
                let data = if which == 0 {
                    &mut baseline
                } else {
                    &mut checked
                };
                let start = Instant::now();
                data.prepare_source(&snapshot);
                data.prepare_syntax();
                if which == 1 {
                    assert!(black_box(data.mitex_compatible("0.2.7")));
                }
                if iteration >= 10 {
                    elapsed[which] += start.elapsed();
                }
            }
        }
        let start = Instant::now();
        for _ in 0..20_000 {
            black_box(checked.mitex_compatible(black_box("0.2.7")));
        }
        let idle = start.elapsed();
        assert_eq!(checked.compatibility_checks, 210);
        assert_eq!(checked.syntax_rebuilds, baseline.syntax_rebuilds);
        eprintln!(
            "fixture_bytes={} warmup=10 changed_revisions=200 baseline_source_and_syntax_us={} with_compatibility_us={} cached_checks=20000 cached_us={}",
            fixture.len() + 1,
            elapsed[0].as_micros(),
            elapsed[1].as_micros(),
            idle.as_micros()
        );
    }

    fn revision(revision: u64) -> DocumentKey {
        DocumentKey::new(
            tiptoptyp_core::document::WindowSessionId::new(1),
            3,
            revision,
        )
    }

    fn diagnostic(path: &Path, line: usize, message: &str) -> Diagnostic {
        Diagnostic {
            provider: None,
            severity: DiagnosticSeverity::Error,
            source: DiagnosticSource::File(path.to_owned()),
            location: Some(DiagnosticLocation { line, column: 1 }),
            message: message.to_owned(),
            details: Vec::new(),
        }
    }

    #[test]
    fn delimiter_queries_use_character_offsets_and_invalidate_on_document_edits() {
        let mut data = EditorDerivedData::default();
        let source = "你好 🦀 #let x = (1, 2)";
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        assert_eq!(data.matching_delimiters(5), None);
        assert_eq!(
            data.syntax_rebuilds, 0,
            "ordinary typing needs no delimiter parse"
        );
        let start = source[..source.find('(').unwrap()].chars().count();
        let end = source.chars().count() - 1;
        let pair = Some([start..start + 1, end..end + 1]);
        assert_eq!(data.matching_delimiters(start), pair);
        assert_eq!(data.matching_delimiters(start + 1), pair);
        assert_eq!(data.matching_delimiters(end + 1), pair);
        for _ in 0..100 {
            assert_eq!(data.matching_delimiters(end + 1), pair);
        }
        assert_eq!(data.syntax_rebuilds, 1);
        data.prepare_source(&DocumentSnapshot::fixture(
            revision(2),
            "你好 🦀 #let x = (1, 2",
        ));
        assert_eq!(data.matching_delimiters(start), None);
        assert_eq!(data.syntax_rebuilds, 2);
        let other_window =
            DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(2), 3, 2);
        data.prepare_source(&DocumentSnapshot::fixture(other_window, source));
        assert_eq!(data.matching_delimiters(start), pair);
        assert_eq!(data.matching_delimiters(usize::MAX), None);
    }

    #[test]
    fn unchanged_revision_reuses_source_metrics_offsets_and_snapshot() {
        let mut data = EditorDerivedData::default();
        let source = "short\n🦀 longest\n";
        data.prepare_source(&DocumentSnapshot::fixture(revision(4), source));
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
        data.prepare_source(&DocumentSnapshot::fixture(revision(4), source));
        let second_snapshot = data.source_snapshot();
        assert!(Arc::ptr_eq(&first_snapshot, &second_snapshot));
        assert_eq!(data.rebuild_counts(), (1, 0));

        data.prepare_source(&DocumentSnapshot::fixture(revision(5), "one line"));
        assert_eq!(data.source_metrics().line_count, 1);
        assert_eq!(&*first_snapshot, source, "undo snapshot retains old text");
        assert!(!Arc::ptr_eq(&first_snapshot, &data.source_snapshot()));
        assert_eq!(data.rebuild_counts(), (2, 0));
    }

    #[test]
    fn parsed_queries_reuse_one_revision_snapshot() {
        let mut data = EditorDerivedData::default();
        let source = "#set text(font: \"Libertinus Serif\")\n#link(\"https://example.com\")[site]";
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));

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
    fn hover_queries_reuse_syntax_and_stationary_targets_but_invalidate_on_edits() {
        let mut data = EditorDerivedData::default();
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), "$ H_sigma $"));
        for _ in 0..100 {
            assert_eq!(data.hover_token_range(5), Some(4..9));
        }
        assert_eq!(data.syntax_rebuilds, 1);
        assert_eq!(data.hover_queries, 1);
        assert_eq!(data.hover_token_range(6), Some(4..9));
        assert_eq!(data.syntax_rebuilds, 1);
        assert_eq!(data.hover_queries, 2);

        data.prepare_source(&DocumentSnapshot::fixture(revision(2), "$ H^alpha $"));
        assert_eq!(data.hover_token_range(5), Some(4..9));
        assert_eq!(data.syntax_rebuilds, 2);
        data.prepare_source(&DocumentSnapshot::fixture(revision(3), "#let H_alpha = 1"));
        assert_eq!(data.hover_token_range(5), Some(5..12));
        assert_eq!(data.syntax_rebuilds, 3);
        assert_eq!(data.hover_token_range(usize::MAX), None);
    }

    #[test]
    fn sticky_context_queries_reuse_one_syntax_parse_per_revision() {
        let mut data = EditorDerivedData::default();
        let source = "= Section\n#let render(\n  body,\n) = {\n  body\n}\n";
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));

        {
            let query = data.sticky_context_query();
            assert_eq!(
                query
                    .rows(0)
                    .iter()
                    .map(|row| row.text.as_str())
                    .collect::<Vec<_>>(),
                ["= Section"]
            );
            let body = source[..source.rfind("  body\n").unwrap()].chars().count();
            assert_eq!(
                query
                    .rows(body)
                    .iter()
                    .map(|row| row.text.as_str())
                    .collect::<Vec<_>>(),
                ["= Section", "#let render(", "body,", ") = {"]
            );
            assert!(query.rows(source.chars().count() + 1).is_empty());
        }
        assert_eq!(data.syntax_rebuild_count(), 1);
        let regions = data.context_regions();
        for _ in 0..100 {
            assert!(Arc::ptr_eq(&regions, &data.context_regions()));
        }

        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        {
            let query = data.sticky_context_query();
            assert_eq!(query.rows(0).len(), 1);
            assert_eq!(query.rows(0).len(), 1);
        }
        assert_eq!(
            data.syntax_rebuild_count(),
            1,
            "unchanged source revisions must reuse the parsed syntax tree"
        );

        data.prepare_source(&DocumentSnapshot::fixture(
            revision(2),
            "= Next section\nbody",
        ));
        assert_eq!(
            data.syntax_rebuild_count(),
            1,
            "syntax rebuild remains lazy until the next query"
        );
        {
            let query = data.sticky_context_query();
            assert_eq!(
                query
                    .rows("= Next section\n".chars().count())
                    .iter()
                    .map(|row| row.text.as_str())
                    .collect::<Vec<_>>(),
                ["= Next section"]
            );
        }
        assert_eq!(data.syntax_rebuild_count(), 2);
        data.sticky_context_query();
        assert_eq!(data.syntax_rebuild_count(), 2);
    }

    #[test]
    fn table_cursor_queries_reuse_syntax_and_invalidate_on_document_changes() {
        let mut data = EditorDerivedData::default();
        let source = "#table(columns: 2, [A], [B])";
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        for _ in 0..100 {
            data.prepare_table_at_cursor(4);
        }
        assert_eq!(data.syntax_rebuild_count(), 1);
        let table = data.cached_table_at_cursor(revision(1)).unwrap() as *const _;
        data.prepare_table_at_cursor(4);
        assert_eq!(
            table,
            data.cached_table_at_cursor(revision(1)).unwrap() as *const _
        );
        data.prepare_table_at_cursor(5);
        assert_eq!(data.syntax_rebuild_count(), 1);
        data.prepare_source(&DocumentSnapshot::fixture(revision(2), "not a table"));
        assert!(data.cached_table_at_cursor(revision(2)).is_none());
        data.prepare_table_at_cursor(4);
        assert!(data.cached_table_at_cursor(revision(2)).is_none());
        assert_eq!(data.syntax_rebuild_count(), 2);
    }

    #[test]
    fn diagnostics_are_cached_by_document_and_diagnostic_generation() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("main.typ");
        std::fs::write(&path, "= Main").unwrap();
        let diagnostics = [diagnostic(&path, 2, "broken")];
        let mut data = EditorDerivedData::default();
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), "= Main"));

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
            data.prepare_source(&DocumentSnapshot::fixture(revision(11), &source));
            assert_eq!(data.source_metrics().line_count, 50_001);
        }
        assert_eq!(data.rebuild_counts(), (1, 0));
    }

    #[test]
    fn tex_index_is_lazy_and_reused_until_revision_changes() {
        let mut data = EditorDerivedData::default();
        let source = "#mi(`\\newcommand{\\custom}{x} \\cu`)";
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        assert_eq!(data.tex_rebuilds, 0);
        let cursor = source.find("\\cu`)").unwrap() + 3;
        for _ in 0..100 {
            assert!(
                data.tex_completions(cursor)
                    .unwrap()
                    .iter()
                    .any(|item| item.label == "\\custom")
            );
        }
        assert_eq!(data.tex_rebuilds, 1);
        assert_eq!(data.syntax_rebuilds, 1);
        let source = "#mi(`\\cu`)";
        data.prepare_source(&DocumentSnapshot::fixture(revision(2), source));
        assert!(
            !data
                .tex_completions(8)
                .unwrap()
                .iter()
                .any(|item| item.label == "\\custom")
        );
        assert_eq!(data.tex_rebuilds, 2);
    }
}
