use std::{
    collections::{BTreeMap, HashMap},
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

/// One lossless editing target, cached per revision and caret rather than parsed per frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TableTarget {
    Typst(crate::editor_features::EditableTable),
    Tex(crate::editor_features::TexTable),
    TypstMath(crate::editor_features::MathTable),
}
impl TableTarget {
    pub(crate) fn at(
        source: &str,
        cursor: usize,
        kind: crate::document::DocumentKind,
    ) -> Option<Self> {
        if kind.is_typst() {
            crate::editor_features::math_table_at(source, cursor)
                .map(Self::TypstMath)
                .or_else(|| {
                    crate::editor_features::editable_table_at(source, cursor).map(Self::Typst)
                })
        } else if kind == crate::document::DocumentKind::Tex {
            crate::editor_features::tex_table_at(source, cursor).map(Self::Tex)
        } else {
            None
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

#[derive(Clone, Copy)]
struct SnippetContextCache {
    key: DocumentKey,
    cursor: usize,
    kind: crate::document::DocumentKind,
    effective: crate::document::DocumentKind,
    scope: Option<crate::snippets::Scope>,
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
    table_query: Option<(DocumentKey, usize, crate::document::DocumentKind)>,
    table_at_cursor: Option<TableTarget>,
    hover_query: Option<(DocumentKey, usize)>,
    hover_range: Option<Range<usize>>,
    snippet_query: Option<SnippetContextCache>,
    diagnostics_key: Option<DiagnosticsKey>,
    line_diagnostics: Arc<[LineDiagnostic]>,
    longest_diagnostic_chars: usize,
    #[cfg(test)]
    source_rebuilds: usize,
    #[cfg(test)]
    diagnostic_rebuilds: usize,
    #[cfg(test)]
    diagnostic_file_checks: usize,
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
            snippet_query: None,
            diagnostics_key: None,
            line_diagnostics: Arc::from([]),
            longest_diagnostic_chars: 0,
            #[cfg(test)]
            source_rebuilds: 0,
            #[cfg(test)]
            diagnostic_rebuilds: 0,
            #[cfg(test)]
            diagnostic_file_checks: 0,
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

/// Fast path for tex completions: index entries only come from embedded TeX
/// calls (`mi`, `mitex` and friends, all containing "mi"), raw tex/latex
/// blocks (containing "tex"), or backslash commands. Without any marker a
/// tree walk can find nothing; hits simply run it.
fn tex_index_triggers_present(source: &str) -> bool {
    source.contains('\\') || source.contains("mi") || source.contains("tex")
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

    pub(crate) fn embedded_tex_projection(&mut self) -> crate::embedded_tex::Projection {
        self.prepare_syntax();
        crate::embedded_tex::Projection::from_parsed(&self.parsed_source)
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

    fn prepare_tex_index(&mut self) {
        self.prepare_syntax();
        if self.tex_key != self.source_key {
            // Projected dollar regions own completion even without a command:
            // Some(empty) suppresses Typst suggestions in plain TeX math.
            // Outside that mode the byte scan still skips trigger-free source.
            let projected_dollars = self.mitex_dollars && self.source_snapshot.contains('$');
            if !projected_dollars && !tex_index_triggers_present(&self.source_snapshot) {
                self.tex_index = crate::tex_completion::Index::default();
            } else if self.mitex_dollars {
                self.tex_index = crate::tex_completion::Index::projected(&self.parsed_source);
            } else {
                self.tex_index = crate::tex_completion::Index::new(&self.parsed_source);
            }
            self.tex_key = self.source_key;
            #[cfg(test)]
            {
                self.tex_rebuilds += 1;
            }
        }
    }

    pub(crate) fn tex_completions(
        &mut self,
        cursor: usize,
    ) -> Option<Vec<crate::tinymist::CompletionItem>> {
        self.prepare_tex_index();
        self.tex_index.items(&self.source_snapshot, cursor)
    }

    #[cfg(test)]
    pub(crate) fn snippet_context(
        &mut self,
        cursor: usize,
        kind: crate::document::DocumentKind,
    ) -> Option<crate::snippets::Scope> {
        self.snippet_language_context(cursor, kind).1
    }

    pub(crate) fn snippet_language_context(
        &mut self,
        cursor: usize,
        kind: crate::document::DocumentKind,
    ) -> (
        crate::document::DocumentKind,
        Option<crate::snippets::Scope>,
    ) {
        use crate::document::DocumentKind;
        let Some(key) = self.source_key else {
            return (kind, None);
        };
        if let Some(cached) = self.snippet_query
            && cached.key == key
            && cached.cursor == cursor
            && cached.kind == kind
        {
            return (cached.effective, cached.scope);
        }
        let byte = self.char_to_byte(cursor);
        let query_start = self.source_snapshot[..byte]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-'))
            .last()
            .map_or(byte, |(start, _)| start);
        let projected = if kind == DocumentKind::Typst
            && self.mitex_dollars
            && self.source_snapshot.contains('$')
        {
            self.prepare_tex_index();
            self.tex_index
                .projected_snippet_context(&self.source_snapshot, query_start)
        } else {
            None
        };
        let (effective, scope) = if let Some(scope) = projected {
            (DocumentKind::Tex, scope)
        } else {
            let scope = match kind {
                DocumentKind::Typst => {
                    self.prepare_syntax();
                    crate::snippets::typst_context(&self.parsed_source, query_start)
                }
                DocumentKind::Tex => {
                    crate::snippets::tex_context(&self.source_snapshot, query_start)
                }
                _ => None,
            };
            (kind, scope)
        };
        self.snippet_query = Some(SnippetContextCache {
            key,
            cursor,
            kind,
            effective,
            scope,
        });
        (effective, scope)
    }

    pub(crate) fn set_mitex_dollars(&mut self, enabled: bool) {
        if self.mitex_dollars != enabled {
            self.mitex_dollars = enabled;
            self.tex_key = None;
            self.snippet_query = None;
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

    pub(crate) fn prepare_table_at_cursor(
        &mut self,
        cursor: usize,
        kind: crate::document::DocumentKind,
    ) {
        let Some(key) = self.source_key else {
            return;
        };
        if self.table_query == Some((key, cursor, kind)) {
            return;
        }
        self.table_at_cursor = if kind.is_typst() {
            self.prepare_syntax();
            let byte = self.char_to_byte(cursor);
            crate::editor_features::math_table_in_source(&self.parsed_source, byte)
                .map(TableTarget::TypstMath)
                .or_else(|| {
                    crate::editor_features::editable_table_in_source(&self.parsed_source, byte)
                        .map(TableTarget::Typst)
                })
        } else if kind == crate::document::DocumentKind::Tex {
            crate::editor_features::tex_table_at(&self.source_snapshot, cursor)
                .map(TableTarget::Tex)
        } else {
            None
        };
        self.table_query = Some((key, cursor, kind));
    }

    pub(crate) fn cached_table_at_cursor(&self, key: DocumentKey) -> Option<&TableTarget> {
        self.table_query.filter(|(cached, _, _)| *cached == key)?;
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

        // Resolve each distinct file once per refresh, not once per warning.
        // Borrow paths from this batch and discard the cache afterward so a
        // later refresh observes changed symlinks or newly created files.
        let mut file_identity = None;
        let mut file_matches = HashMap::new();
        let mut by_line: BTreeMap<usize, Vec<&Diagnostic>> = BTreeMap::new();
        for diagnostic in compiler.iter().chain(language_server) {
            let Some(line) = diagnostic.line() else {
                continue;
            };
            let targets_current_document = match &diagnostic.source {
                DiagnosticSource::Main => current_is_preview,
                DiagnosticSource::File(path) => *file_matches.entry(path).or_insert_with(|| {
                    #[cfg(test)]
                    {
                        self.diagnostic_file_checks += 1;
                    }
                    file_identity
                        .get_or_insert_with(|| {
                            current_path.map_or_else(
                                || FileIdentity::virtual_path(virtual_path),
                                FileIdentity::for_open_file,
                            )
                        })
                        .matches(path)
                }),
                DiagnosticSource::Global => false,
            };

            if targets_current_document {
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
    fn snippet_context_reuses_revision_syntax_and_scalar_boundaries() {
        let mut data = EditorDerivedData::default();
        let source = "α $ ff $ #let f = ff";
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        let math = source[..source.find("ff").unwrap()].chars().count() + 2;
        assert_eq!(
            data.snippet_context(math, crate::document::DocumentKind::Typst),
            Some(crate::snippets::Scope::Math)
        );
        assert_eq!(
            data.snippet_context(source.chars().count(), crate::document::DocumentKind::Typst),
            Some(crate::snippets::Scope::Code)
        );
        assert_eq!(
            data.snippet_context(math, crate::document::DocumentKind::Typst),
            Some(crate::snippets::Scope::Math)
        );
        assert_eq!(
            data.syntax_rebuilds, 1,
            "one parser update per source revision, shared with other editor features"
        );
        data.prepare_source(&DocumentSnapshot::fixture(revision(2), "text ff"));
        assert_eq!(
            data.snippet_context(7, crate::document::DocumentKind::Typst),
            Some(crate::snippets::Scope::Text)
        );
        assert_eq!(data.syntax_rebuilds, 2);
    }

    #[test]
    fn projected_snippet_context_reuses_tex_islands_and_invalidates_on_mode_toggle() {
        use crate::{document::DocumentKind, snippets::Scope};
        let mut data = EditorDerivedData::default();
        let source = "Préface dm\n$ \\text{bf} + frac $\n#mi(\"frac\")\n`$ frac $`\n$ x % frac\n $";
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        data.set_mitex_dollars(true);
        let at = |needle: &str| {
            source[..source.find(needle).unwrap() + needle.len()]
                .chars()
                .count()
        };
        let frac = at("+ frac");
        for _ in 0..20 {
            assert_eq!(
                data.snippet_language_context(frac, DocumentKind::Typst),
                (DocumentKind::Tex, Some(Scope::Math))
            );
        }
        assert_eq!(
            data.snippet_language_context(at("bf"), DocumentKind::Typst),
            (DocumentKind::Tex, Some(Scope::Text))
        );
        assert_eq!(
            data.snippet_language_context(at("dm"), DocumentKind::Typst),
            (DocumentKind::Typst, Some(Scope::Text))
        );
        assert_eq!(
            data.snippet_language_context(at("#mi(\"frac"), DocumentKind::Typst),
            (DocumentKind::Typst, None)
        );
        assert_eq!(
            data.snippet_language_context(at("`$ frac"), DocumentKind::Typst),
            (DocumentKind::Typst, None)
        );
        assert_eq!(
            data.snippet_language_context(at("% frac"), DocumentKind::Typst),
            (DocumentKind::Tex, None)
        );
        assert_eq!(data.syntax_rebuilds, 1);
        assert_eq!(
            data.tex_rebuilds, 1,
            "snippet context and command completion reuse the same projected region index"
        );
        data.set_mitex_dollars(false);
        assert_eq!(
            data.snippet_language_context(frac, DocumentKind::Typst),
            (DocumentKind::Typst, Some(Scope::Math))
        );
        data.set_mitex_dollars(true);
        assert_eq!(
            data.snippet_language_context(frac, DocumentKind::Typst),
            (DocumentKind::Tex, Some(Scope::Math))
        );
        assert_eq!(data.tex_rebuilds, 2);
        assert_eq!(data.syntax_rebuilds, 1);
    }

    #[test]
    fn projected_dollar_regions_keep_completion_ownership_without_commands() {
        for source in [
            "$ x $",
            "$\nplain math\n$",
            "before $x % comment\n y$ after",
        ] {
            let mut data = EditorDerivedData::default();
            data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
            let cursor = source.find('$').unwrap() + 2;
            assert!(data.tex_completions(cursor).is_none());
            data.set_mitex_dollars(true);
            assert_eq!(
                data.tex_completions(cursor),
                Some(Vec::new()),
                "TeX owns the projected dollar region even without commands: {source:?}"
            );
            for _ in 0..20 {
                assert_eq!(data.tex_completions(cursor), Some(Vec::new()));
            }
            assert_eq!(data.tex_rebuilds, 2, "unchanged projected index is reused");
            data.set_mitex_dollars(false);
            assert!(data.tex_completions(cursor).is_none());
            assert_eq!(data.tex_rebuilds, 3);
            assert_eq!(data.syntax_rebuilds, 1);
        }
        for source in ["`$ x $`", "#let s = \"$ x $\"", "\\$ x \\$"] {
            let mut data = EditorDerivedData::default();
            data.set_mitex_dollars(true);
            data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
            assert!(
                data.tex_completions(source.find('x').unwrap()).is_none(),
                "raw, quoted and escaped dollars do not own completion: {source:?}"
            );
        }
    }

    #[test]
    fn tex_index_guard_skips_trigger_free_sources() {
        for source in ["", "plain prose\n", "= Heading\nBody $x + y$ here.\n"] {
            assert!(
                !tex_index_triggers_present(source),
                "guard must skip {source:?}"
            );
            let mut data = EditorDerivedData::default();
            data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
            assert_eq!(data.tex_completions(0), None);
        }
        for source in [
            "#mi(`\\alpha`)",
            "```tex\nhello\n```\n",
            "\\begin{equation}x\\end{equation}",
            "mild prose",
        ] {
            assert!(
                tex_index_triggers_present(source),
                "guard must walk {source:?}"
            );
        }
        // Triggered sources still complete.
        let mut data = EditorDerivedData::default();
        let source = "#mi(`\\newcommand{\\custom}{x} \\cu`)";
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        let cursor = source.find("\\cu`)").unwrap() + 3;
        assert!(
            data.tex_completions(cursor)
                .unwrap()
                .iter()
                .any(|item| item.label == "\\custom")
        );
    }

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
    #[ignore = "manual optimized diagnostic refresh measurement; no timing threshold"]
    fn diagnostic_refresh_cost_probe() {
        use std::{hint::black_box, time::Instant};
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("main.typ");
        std::fs::write(&path, "= Main").unwrap();
        for count in [1, 100, 1_000] {
            let diagnostics = (1..=count)
                .map(|line| diagnostic(&path, line, "A representative warning"))
                .collect::<Vec<_>>();
            let mut data = EditorDerivedData::default();
            let mut revision_number = 0;
            let mut refresh = || {
                revision_number += 1;
                data.prepare_diagnostics(
                    revision(revision_number),
                    1,
                    Some(&path),
                    &path,
                    true,
                    &diagnostics,
                    &[],
                );
                assert_eq!(black_box(data.line_diagnostics()).len(), count);
            };
            for _ in 0..10 {
                refresh();
            }
            for batch in 0..5 {
                let start = Instant::now();
                for _ in 0..100 {
                    refresh();
                }
                eprintln!(
                    "diagnostic_refresh,count={count},batch={batch},ns={}",
                    start.elapsed().as_nanos() / 100
                );
            }
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
            data.prepare_table_at_cursor(4, crate::document::DocumentKind::Typst);
        }
        assert_eq!(data.syntax_rebuild_count(), 1);
        let table = data.cached_table_at_cursor(revision(1)).unwrap() as *const _;
        data.prepare_table_at_cursor(4, crate::document::DocumentKind::Typst);
        assert_eq!(
            table,
            data.cached_table_at_cursor(revision(1)).unwrap() as *const _
        );
        data.prepare_table_at_cursor(5, crate::document::DocumentKind::Typst);
        assert_eq!(data.syntax_rebuild_count(), 1);
        data.prepare_source(&DocumentSnapshot::fixture(revision(2), "not a table"));
        assert!(data.cached_table_at_cursor(revision(2)).is_none());
        data.prepare_table_at_cursor(4, crate::document::DocumentKind::Typst);
        assert!(data.cached_table_at_cursor(revision(2)).is_none());
        assert_eq!(data.syntax_rebuild_count(), 2);
    }

    #[test]
    fn math_table_target_reuses_syntax_and_selects_the_nested_grid() {
        let source = "#table(columns: 1, [$mat(x, alpha; beta, 0)$])";
        let cursor = source[..source.find("alpha").unwrap()].chars().count();
        let mut data = EditorDerivedData::default();
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        data.prepare_table_at_cursor(cursor, crate::document::DocumentKind::Typst);
        assert!(matches!(
            data.cached_table_at_cursor(revision(1)),
            Some(TableTarget::TypstMath(_))
        ));
        assert_eq!(
            TableTarget::at(source, cursor, crate::document::DocumentKind::Typst).as_ref(),
            data.cached_table_at_cursor(revision(1))
        );
        let cached = data.cached_table_at_cursor(revision(1)).unwrap() as *const _;
        for _ in 0..128 {
            data.prepare_table_at_cursor(cursor, crate::document::DocumentKind::Typst);
            assert_eq!(
                cached,
                data.cached_table_at_cursor(revision(1)).unwrap() as *const _
            );
        }
        assert_eq!(data.syntax_rebuild_count(), 1);
        data.prepare_source(&DocumentSnapshot::fixture(revision(2), "$mat(..rows)$"));
        data.prepare_table_at_cursor(5, crate::document::DocumentKind::Typst);
        assert!(data.cached_table_at_cursor(revision(2)).is_none());
    }

    #[test]
    fn table_query_cache_keeps_language_and_unsupported_geometry_separate() {
        let mut data = EditorDerivedData::default();
        let source = "#table(columns: sizes, ..cells)";
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        data.prepare_table_at_cursor(4, crate::document::DocumentKind::Typst);
        assert!(data.cached_table_at_cursor(revision(1)).is_none());
        for _ in 0..100 {
            data.prepare_table_at_cursor(4, crate::document::DocumentKind::Typst);
        }
        assert_eq!(data.syntax_rebuild_count(), 1);
        data.prepare_table_at_cursor(4, crate::document::DocumentKind::Tex);
        assert!(data.cached_table_at_cursor(revision(1)).is_none());
        let source = "\\begin{tabular}{cc} A & B \\\\ \\end{tabular}";
        data.prepare_source(&DocumentSnapshot::fixture(revision(2), source));
        data.prepare_table_at_cursor(4, crate::document::DocumentKind::Tex);
        assert!(matches!(
            data.cached_table_at_cursor(revision(2)),
            Some(TableTarget::Tex(_))
        ));
        for _ in 0..100 {
            data.prepare_table_at_cursor(4, crate::document::DocumentKind::Tex);
        }
        assert_eq!(
            data.syntax_rebuild_count(),
            1,
            "TeX table queries must not parse Typst syntax"
        );
    }

    #[test]
    fn generated_and_inherited_typst_tables_are_not_editable_in_either_route() {
        for source in [
            "#let widths = (40%, auto)\n#let rows = ([Alpha], [Beta])\n#table(columns: widths, ..rows)",
            "#table(columns: 2, [A], /* preserved comment */ [B])",
            "#table(columns: 2, [A], [B], [incomplete])",
            "#table([Label], [#price])",
            "#set table(columns: 2)\n#table([A], [B])",
            "#set grid(columns: widths)\n#grid([A], [B])",
            "#{ set table(columns: 2); table([A], [B]) }",
        ] {
            let cursor = source
                .rfind("table(")
                .or_else(|| source.rfind("grid("))
                .unwrap();
            assert!(
                TableTarget::at(source, cursor, crate::document::DocumentKind::Typst).is_none(),
                "{source}"
            );
            let mut data = EditorDerivedData::default();
            data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
            for _ in 0..10 {
                data.prepare_table_at_cursor(cursor, crate::document::DocumentKind::Typst);
                assert!(
                    data.cached_table_at_cursor(revision(1)).is_none(),
                    "{source}"
                );
            }
            assert_eq!(data.syntax_rebuild_count(), 1);
        }
    }

    #[test]
    fn unsupported_nested_typst_table_does_not_edit_its_enclosing_table() {
        let source = "#let widths = 2\n#table(columns: 1, [#table(columns: widths, [A])])";
        for cursor in [
            source.rfind("#table").unwrap(),
            source.find("widths, [A]").unwrap(),
        ] {
            assert!(
                TableTarget::at(source, cursor, crate::document::DocumentKind::Typst).is_none()
            );
            let mut data = EditorDerivedData::default();
            data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
            data.prepare_table_at_cursor(cursor, crate::document::DocumentKind::Typst);
            assert!(data.cached_table_at_cursor(revision(1)).is_none());
        }
        assert!(
            TableTarget::at(
                source,
                source.find("#table").unwrap(),
                crate::document::DocumentKind::Typst
            )
            .is_some()
        );
    }

    #[test]
    fn nested_typst_table_sigil_targets_the_inner_grid() {
        let source = "#table(columns: 1, [#grid(columns: 1, [Inner])])";
        let cursor = source.find("#grid").unwrap();
        let Some(TableTarget::Typst(inner)) =
            TableTarget::at(source, cursor, crate::document::DocumentKind::Typst)
        else {
            panic!("inner grid must be editable")
        };
        assert_eq!(inner.kind, crate::editor_features::TableKind::Grid);
        assert_eq!(inner.source_range.start, cursor + 1);
        let mut data = EditorDerivedData::default();
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
        data.prepare_table_at_cursor(cursor, crate::document::DocumentKind::Typst);
        assert_eq!(
            data.cached_table_at_cursor(revision(1)),
            Some(&TableTarget::Typst(inner))
        );
        let Some(TableTarget::Typst(outer)) =
            TableTarget::at(source, 0, crate::document::DocumentKind::Typst)
        else {
            panic!("outer table must remain editable")
        };
        assert_eq!(outer.kind, crate::editor_features::TableKind::Table);
        assert_eq!(outer.source_range.start, 1);
    }

    #[test]
    fn concrete_typst_tables_keep_literals_and_computed_cells_in_both_routes() {
        for source in [
            "#table([A])",
            "#set table(fill: red)\n#table([A])",
            "#set grid(columns: 2)\n#table([A])",
            "#set table(columns: 1)\n#table([A])",
            "#[#set table(columns: 2)]\n#table([A])",
            "#set table(columns: 2)\n#table(columns: 2, [Literal], [#price])",
            "#grid(columns: (auto, 1fr), [Label], [$price$])",
        ] {
            let cursor = source
                .rfind("table(")
                .or_else(|| source.rfind("grid("))
                .unwrap();
            let expected = TableTarget::at(source, cursor, crate::document::DocumentKind::Typst)
                .expect(source);
            let mut data = EditorDerivedData::default();
            data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
            data.prepare_table_at_cursor(cursor, crate::document::DocumentKind::Typst);
            assert_eq!(
                data.cached_table_at_cursor(revision(1)),
                Some(&expected),
                "{source}"
            );
        }
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
    fn diagnostics_resolve_each_distinct_file_once_per_refresh() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("main.typ");
        let missing = project.path().join("missing.typ");
        std::fs::write(&path, "= Main").unwrap();
        let diagnostics = (1..=1_000)
            .flat_map(|line| {
                [
                    diagnostic(&path, line, "shown"),
                    diagnostic(&missing, line, "not shown"),
                ]
            })
            .collect::<Vec<_>>();
        let mut data = EditorDerivedData::default();
        // Both provider lists share the per-refresh cache, including misses.
        for _ in 0..3 {
            data.prepare_diagnostics(
                revision(1),
                1,
                Some(&path),
                &path,
                true,
                &diagnostics,
                &diagnostics,
            );
            assert_eq!(data.line_diagnostics().len(), 1_000);
            assert_eq!(data.diagnostic_file_checks, 2);
        }
        data.prepare_diagnostics(
            revision(2),
            1,
            Some(&path),
            &path,
            true,
            &diagnostics,
            &diagnostics,
        );
        assert_eq!(data.diagnostic_file_checks, 4);
        let mut unlocated = diagnostic(&path, 1, "no location");
        unlocated.location = None;
        data.prepare_diagnostics(revision(3), 1, Some(&path), &path, true, &[unlocated], &[]);
        assert_eq!(data.diagnostic_file_checks, 4);
        assert!(data.line_diagnostics().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn diagnostic_file_cache_does_not_survive_symlink_changes() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("main.typ");
        let other = project.path().join("other.typ");
        let alias = project.path().join("alias.typ");
        std::fs::write(&path, "= Main").unwrap();
        std::fs::write(&other, "= Other").unwrap();
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let diagnostics = [diagnostic(&alias, 1, "shown")];
        let mut data = EditorDerivedData::default();
        data.prepare_diagnostics(revision(1), 1, Some(&path), &path, true, &diagnostics, &[]);
        assert_eq!(data.line_diagnostics().len(), 1);
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&other, &alias).unwrap();
        data.prepare_diagnostics(revision(1), 2, Some(&path), &path, true, &diagnostics, &[]);
        assert!(data.line_diagnostics().is_empty());
    }

    #[test]
    fn diagnostic_targeting_preserves_untitled_and_missing_file_ownership() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("unsaved.typ");
        let mut main = diagnostic(&path, 1, "main");
        main.source = DiagnosticSource::Main;
        let mut global = main.clone();
        global.source = DiagnosticSource::Global;
        let diagnostics = [main, global, diagnostic(&path, 2, "file")];
        for current in [None, Some(path.as_path())] {
            for previewed in [false, true] {
                let mut data = EditorDerivedData::default();
                data.prepare_diagnostics(
                    revision(1),
                    1,
                    current,
                    &path,
                    previewed,
                    &diagnostics,
                    &[],
                );
                let lines = data.line_diagnostics();
                assert_eq!(lines.len(), if previewed { 2 } else { 1 });
                assert_eq!(lines.last().unwrap().summary, "file");
                if previewed {
                    assert_eq!(lines[0].summary, "main");
                }
                assert_eq!(data.diagnostic_file_checks, 1);
            }
        }
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
