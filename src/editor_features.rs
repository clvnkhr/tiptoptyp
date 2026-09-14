//! Pure syntax-backed models for richer editor interactions.
//!
//! Public positions in this module are character indices. Typst's syntax tree
//! uses byte offsets internally, so conversions stay at this boundary instead
//! of leaking UTF-8 details into UI state.

use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use typst_syntax::{
    LinkedNode, Source, SyntaxKind,
    ast::{self, AstNode},
};

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff",
];

/// The Typst construct which owns a literal preview target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssetCallKind {
    Image,
    Include,
}

/// The kind of binary asset that the preview loader should request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewAssetKind {
    Image,
    Pdf,
}

/// A literal image or PDF path belonging to the Typst call under the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiteralAssetTarget {
    pub(crate) call_kind: AssetCallKind,
    pub(crate) asset_kind: PreviewAssetKind,
    /// Range of the whole call (without markup's leading `#`).
    pub(crate) call_range: std::ops::Range<usize>,
    /// Range of the quoted string, including its quote characters.
    pub(crate) literal_range: std::ops::Range<usize>,
    /// Range of the string's source payload, excluding its quote characters.
    pub(crate) value_range: std::ops::Range<usize>,
    pub(crate) resolved_path: PathBuf,
}

/// Find a literal `image("…")` or `include "…"` target in the real Typst
/// expression under `char_cursor`.
///
/// Plain markup which merely resembles a call is never returned. The first
/// positional argument must be a string literal, its extension must be
/// previewable, and the resolved path must remain below `workspace_root`.
/// Typst-style leading-slash paths are resolved from the workspace root;
/// other paths are resolved from the current source file's directory.
pub(crate) fn literal_asset_target_at(
    source: &str,
    char_cursor: usize,
    source_path: &Path,
    workspace_root: &Path,
) -> Option<LiteralAssetTarget> {
    let cursor = char_to_byte(source, char_cursor)?;
    let parsed = Source::detached(source);
    let root = LinkedNode::new(parsed.root());
    find_literal_asset(&root, source, cursor, source_path, workspace_root)
}

fn find_literal_asset(
    node: &LinkedNode<'_>,
    source: &str,
    cursor: usize,
    source_path: &Path,
    workspace_root: &Path,
) -> Option<LiteralAssetTarget> {
    if !contains_byte(node.range(), cursor) {
        return None;
    }

    // Prefer the innermost call when calls happen to be nested.
    if let Some(target) = node
        .children()
        .find_map(|child| find_literal_asset(&child, source, cursor, source_path, workspace_root))
    {
        return Some(target);
    }

    let (call_kind, literal) = literal_asset_argument(node)?;
    let literal_range = literal.range();
    if literal_range.end < literal_range.start + 2 {
        return None;
    }
    let value = literal.get().cast::<ast::Str>()?.get();
    let asset_kind = preview_asset_kind(&value)?;
    let resolved_path = resolve_asset_path(workspace_root, source_path, &value)?;

    Some(LiteralAssetTarget {
        call_kind,
        asset_kind,
        call_range: byte_range_to_char(source, node.range()),
        literal_range: byte_range_to_char(source, literal_range.clone()),
        value_range: byte_range_to_char(source, literal_range.start + 1..literal_range.end - 1),
        resolved_path,
    })
}

fn literal_asset_argument<'a>(node: &LinkedNode<'a>) -> Option<(AssetCallKind, LinkedNode<'a>)> {
    match node.kind() {
        SyntaxKind::FuncCall => {
            let call = node.get().cast::<ast::FuncCall>()?;
            let ast::Expr::Ident(callee) = call.callee() else {
                return None;
            };
            let call_kind = match callee.as_str() {
                "image" => AssetCallKind::Image,
                // `include` is normally a language construct rather than a
                // FuncCall, but accepting the AST form keeps this helper valid
                // if the parser exposes call syntax in an embedded code mode.
                "include" => AssetCallKind::Include,
                _ => return None,
            };
            let args = node
                .children()
                .find(|child| child.kind() == SyntaxKind::Args)?;
            let mut literal = None;
            for argument in args.children().filter(|child| !child.kind().is_trivia()) {
                let Some(argument_ast) = argument.get().cast::<ast::Arg>() else {
                    continue;
                };
                match argument_ast {
                    ast::Arg::Named(_) => {}
                    ast::Arg::Pos(ast::Expr::Str(_)) => {
                        literal = Some(argument);
                        break;
                    }
                    ast::Arg::Pos(_) | ast::Arg::Spread(_) => return None,
                }
            }
            let literal = literal?;
            Some((call_kind, literal))
        }
        SyntaxKind::ModuleInclude => {
            let include = node.get().cast::<ast::ModuleInclude>()?;
            if !matches!(include.source(), ast::Expr::Str(_)) {
                return None;
            }
            let literal = node
                .children()
                .find(|child| child.kind() == SyntaxKind::Str)?;
            Some((AssetCallKind::Include, literal))
        }
        _ => None,
    }
}

fn preview_asset_kind(target: &str) -> Option<PreviewAssetKind> {
    if target.is_empty()
        || target.contains('\0')
        || target.contains('\\')
        || url::Url::parse(target).is_ok()
    {
        return None;
    }
    let extension = Path::new(target).extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("pdf") {
        Some(PreviewAssetKind::Pdf)
    } else if IMAGE_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
    {
        Some(PreviewAssetKind::Image)
    } else {
        None
    }
}

fn resolve_asset_path(root: &Path, source_path: &Path, target: &str) -> Option<PathBuf> {
    let root = canonicalize_existing_prefix(&lexically_normalize(root)?);
    let source_path = canonicalize_existing_prefix(&lexically_normalize(source_path)?);
    if !source_path.starts_with(&root) {
        return None;
    }

    let target = Path::new(target);
    let candidate = if target.is_absolute() {
        root.join(path_without_root(target)?)
    } else {
        source_path.parent()?.join(target)
    };
    let candidate = canonicalize_existing_prefix(&lexically_normalize(&candidate)?);
    candidate.starts_with(&root).then_some(candidate)
}

fn path_without_root(path: &Path) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(component) => relative.push(component),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    Some(relative)
}

fn lexically_normalize(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::Normal(component) => normalized.push(component),
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
        }
    }
    Some(normalized)
}

/// Canonicalize every existing component and retain a normalized missing tail.
/// This catches an existing symlink which would otherwise escape the workspace
/// while still allowing a not-yet-created target to be represented.
fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let mut prefix = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(mut canonical) = prefix.canonicalize() {
            for component in missing.iter().rev() {
                canonical.push(component);
            }
            return lexically_normalize(&canonical).unwrap_or(canonical);
        }
        let Some(name) = prefix.file_name() else {
            return path.to_path_buf();
        };
        missing.push(name.to_os_string());
        let Some(parent) = prefix.parent() else {
            return path.to_path_buf();
        };
        prefix = parent;
    }
}

/// Kind of context represented by a sticky editor row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StickyContextKind {
    Heading { level: usize },
    LetBinding,
    Function,
    Block,
}

/// One source line kept at the top of the editor to explain current context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StickyContextRow {
    pub(crate) kind: StickyContextKind,
    /// One-based source line.
    pub(crate) line: usize,
    /// Character position of the first non-whitespace source character.
    pub(crate) char_index: usize,
    /// One-based source line whose top pushes this context out of the overlay.
    /// A value past the final source line means the context lasts to EOF.
    pub(crate) end_line: usize,
    /// Trimmed, otherwise verbatim original source line.
    pub(crate) text: String,
}

/// Derive document heading ancestry and enclosing lexical scopes at a source
/// character position. The returned order is outermost to innermost.
#[cfg(test)]
pub(crate) fn sticky_context_rows(source: &str, char_position: usize) -> Vec<StickyContextRow> {
    let parsed = Source::detached(source);
    let mut char_starts = Vec::with_capacity(source.len().saturating_add(1));
    char_starts.push(0);
    char_starts.extend(
        source
            .char_indices()
            .map(|(byte, character)| byte + character.len_utf8()),
    );
    StickyContextQuery::new(&parsed, source, &char_starts).rows(char_position)
}

/// A reusable sticky-context view over one already parsed source revision.
///
/// Constructing the query builds the line index once. Individual probes only
/// index the prepared character offsets and walk the existing syntax tree, so
/// cumulative sticky-stack resolution does not clone or reparse the document.
pub(crate) struct StickyContextQuery<'a> {
    parsed: &'a Source,
    source: &'a str,
    char_starts: &'a [usize],
    lines: LineMap,
}

impl<'a> StickyContextQuery<'a> {
    pub(crate) fn new(parsed: &'a Source, source: &'a str, char_starts: &'a [usize]) -> Self {
        debug_assert_eq!(parsed.text(), source);
        Self {
            parsed,
            source,
            char_starts,
            lines: LineMap::new(source),
        }
    }

    /// Derive document heading ancestry and enclosing lexical scopes at a
    /// character position without rebuilding revision-derived source data.
    pub(crate) fn rows(&self, char_position: usize) -> Vec<StickyContextRow> {
        let Some(byte_position) = self.char_starts.get(char_position).copied() else {
            return Vec::new();
        };
        let root = LinkedNode::new(self.parsed.root());

        let mut rows = heading_context(&root, self.source, byte_position, &self.lines);
        let mut scopes = Vec::new();
        collect_scope_path(&root, self.source, byte_position, &self.lines, &mut scopes);
        push_unique_context_rows(&mut rows, scopes);
        rows
    }
}

fn heading_context(
    root: &LinkedNode<'_>,
    source: &str,
    byte_position: usize,
    lines: &LineMap,
) -> Vec<StickyContextRow> {
    let all = root
        .children()
        .filter_map(|node| {
            (node.kind() == SyntaxKind::Heading)
                .then(|| {
                    node.get()
                        .cast::<ast::Heading>()
                        .map(|heading| (node.offset(), heading.depth().get()))
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    let mut active = Vec::new();
    // Only root-document headings establish section ancestry. Headings inside
    // content values or function bodies are local content, not document peers.
    for (index, &(offset, level)) in all.iter().enumerate() {
        if offset > byte_position {
            break;
        }
        while active
            .last()
            .is_some_and(|&old: &usize| all[old].1 >= level)
        {
            active.pop();
        }
        active.push(index);
    }
    active
        .into_iter()
        .map(|index| {
            let (offset, level) = all[index];
            let end_line = all[index + 1..]
                .iter()
                .find(|(_, next_level)| *next_level <= level)
                .map_or(lines.starts.len() + 1, |(next, _)| {
                    lines.line_index_at(*next) + 1
                });
            context_row(
                source,
                lines,
                offset,
                end_line,
                StickyContextKind::Heading { level },
            )
        })
        .collect()
}

fn collect_scope_path(
    node: &LinkedNode<'_>,
    source: &str,
    byte_position: usize,
    lines: &LineMap,
    rows: &mut Vec<StickyContextRow>,
) {
    if !contains_byte(node.range(), byte_position) {
        return;
    }

    let kind = match node.kind() {
        SyntaxKind::LetBinding => node
            .get()
            .cast::<ast::LetBinding>()
            .map(|binding| match binding.kind() {
                ast::LetBindingKind::Closure(_) => StickyContextKind::Function,
                ast::LetBindingKind::Normal(_) => StickyContextKind::LetBinding,
            }),
        SyntaxKind::Closure => Some(StickyContextKind::Function),
        SyntaxKind::CodeBlock
        | SyntaxKind::ContentBlock
        | SyntaxKind::SetRule
        | SyntaxKind::ShowRule
        | SyntaxKind::ModuleImport
        | SyntaxKind::ModuleInclude
        | SyntaxKind::Contextual
        | SyntaxKind::Conditional
        | SyntaxKind::WhileLoop
        | SyntaxKind::ForLoop
        | SyntaxKind::FuncCall
        | SyntaxKind::Parenthesized
        | SyntaxKind::Array
        | SyntaxKind::Dict
        | SyntaxKind::Raw
        | SyntaxKind::Equation => Some(StickyContextKind::Block),
        _ => None,
    }
    .filter(|_| node_spans_multiple_lines(node, lines));
    if let Some(kind) = kind {
        let end_line = lines.line_index_at(node.range().end.saturating_sub(1)) + 2;
        let context = match node.kind() {
            SyntaxKind::LetBinding | SyntaxKind::Closure => {
                declaration_header_rows(node, source, lines, end_line, kind)
            }
            _ => vec![context_row(source, lines, node.offset(), end_line, kind)],
        };
        push_unique_context_rows(rows, context);
    }

    for child in node.children() {
        if contains_byte(child.range(), byte_position) {
            collect_scope_path(&child, source, byte_position, lines, rows);
        }
    }
}

fn node_spans_multiple_lines(node: &LinkedNode<'_>, lines: &LineMap) -> bool {
    lines.line_index_at(node.offset()) < lines.line_index_at(node.range().end.saturating_sub(1))
}

fn push_unique_context_rows(
    rows: &mut Vec<StickyContextRow>,
    candidates: impl IntoIterator<Item = StickyContextRow>,
) {
    for row in candidates {
        if rows
            .iter()
            .any(|existing| existing.line == row.line && existing.char_index == row.char_index)
        {
            continue;
        }
        rows.push(row);
    }
}

/// Return every non-empty logical line in a declaration header. The body-start
/// line belongs to the header because it carries the `=` and, for block
/// bodies, the opening delimiter. Later body lines deliberately stay out of
/// sticky context.
fn declaration_header_rows(
    node: &LinkedNode<'_>,
    source: &str,
    lines: &LineMap,
    end_line: usize,
    kind: StickyContextKind,
) -> Vec<StickyContextRow> {
    let header_end = declaration_body_start(node)
        .unwrap_or_else(|| node.range().end.saturating_sub(1))
        .min(source.len().saturating_sub(1));
    let first_line = lines.line_index_at(node.offset());
    let last_line = lines.line_index_at(header_end);
    (first_line..=last_line)
        .filter_map(|line| {
            let row = context_row_for_line(source, lines, line, end_line, kind);
            (!row.text.is_empty()).then_some(row)
        })
        .collect()
}

fn declaration_body_start(node: &LinkedNode<'_>) -> Option<usize> {
    let body = match node.kind() {
        SyntaxKind::LetBinding => {
            let binding = node.get().cast::<ast::LetBinding>()?;
            match binding.kind() {
                ast::LetBindingKind::Closure(_) => node
                    .get()
                    .children()
                    .find_map(|child| child.cast::<ast::Closure>())?
                    .body()
                    .to_untyped(),
                ast::LetBindingKind::Normal(_) => binding.init()?.to_untyped(),
            }
        }
        SyntaxKind::Closure => node.get().cast::<ast::Closure>()?.body().to_untyped(),
        _ => return None,
    };
    linked_descendant_offset(node, body)
}

fn linked_descendant_offset(
    node: &LinkedNode<'_>,
    target: &typst_syntax::SyntaxNode,
) -> Option<usize> {
    if std::ptr::eq(node.get(), target) {
        return Some(node.offset());
    }
    node.children()
        .find_map(|child| linked_descendant_offset(&child, target))
}

fn context_row(
    source: &str,
    lines: &LineMap,
    byte_position: usize,
    end_line: usize,
    kind: StickyContextKind,
) -> StickyContextRow {
    let line_index = lines.line_index_at(byte_position);
    context_row_for_line(source, lines, line_index, end_line, kind)
}

fn context_row_for_line(
    source: &str,
    lines: &LineMap,
    line_index: usize,
    end_line: usize,
    kind: StickyContextKind,
) -> StickyContextRow {
    let range = lines.line_range(source, line_index);
    let line_source = &source[range.clone()];
    let leading = line_source.len() - line_source.trim_start().len();
    StickyContextRow {
        kind,
        line: line_index + 1,
        char_index: byte_to_char(source, range.start + leading),
        end_line,
        text: line_source.trim().to_owned(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableKind {
    Table,
    Grid,
}

impl TableKind {
    fn callee(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Grid => "grid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreservedTableOption {
    name: String,
    source: String,
}

/// A conservative, rectangular table/grid model suitable for direct editing.
///
/// Cell strings are Typst markup from inside their original `[...]` blocks.
/// Only static content blocks are accepted. Named non-structural options are
/// kept as exact argument source and re-emitted unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditableTable {
    pub(crate) kind: TableKind,
    pub(crate) source_range: std::ops::Range<usize>,
    pub(crate) columns: usize,
    pub(crate) cells: Vec<Vec<String>>,
    options: Vec<PreservedTableOption>,
    original_columns: usize,
    columns_were_explicit: bool,
    indentation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceEdit {
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TableEditError {
    ZeroColumns,
    NonRectangular {
        row: usize,
        expected: usize,
        actual: usize,
    },
    DynamicCell {
        row: usize,
        column: usize,
    },
}

impl fmt::Display for TableEditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroColumns => formatter.write_str("a table must have at least one column"),
            Self::NonRectangular {
                row,
                expected,
                actual,
            } => write!(
                formatter,
                "table row {} has {actual} cells; expected {expected}",
                row + 1
            ),
            Self::DynamicCell { row, column } => write!(
                formatter,
                "table cell at row {}, column {} is not static Typst markup",
                row + 1,
                column + 1
            ),
        }
    }
}

impl std::error::Error for TableEditError {}

/// Find the innermost statically editable `table(...)` or `grid(...)` call
/// under a character cursor.
///
/// The parser deliberately refuses spreads, dynamic positional expressions,
/// computed column counts, structural helpers, malformed syntax, comments
/// between arguments, and incomplete final rows. Returning `None` guarantees
/// that opening and cancelling an editor can never discard unsupported source.
pub(crate) fn editable_table_at(source: &str, char_cursor: usize) -> Option<EditableTable> {
    let cursor = char_to_byte(source, char_cursor)?;
    let parsed = Source::detached(source);
    let root = LinkedNode::new(parsed.root());
    find_editable_table(&root, source, cursor).or_else(|| {
        // The expression node starts after markup's `#`, but the sigil is a
        // natural right-click target and should open the same table editor.
        source[cursor..]
            .starts_with('#')
            .then(|| find_editable_table(&root, source, cursor + 1))
            .flatten()
    })
}

fn find_editable_table(
    node: &LinkedNode<'_>,
    source: &str,
    cursor: usize,
) -> Option<EditableTable> {
    if !contains_byte(node.range(), cursor) {
        return None;
    }
    if let Some(table) = node
        .children()
        .find_map(|child| find_editable_table(&child, source, cursor))
    {
        return Some(table);
    }
    parse_editable_table(node, source)
}

fn parse_editable_table(node: &LinkedNode<'_>, source: &str) -> Option<EditableTable> {
    if node.kind() != SyntaxKind::FuncCall || has_descendant_kind(node, SyntaxKind::Error) {
        return None;
    }
    let call = node.get().cast::<ast::FuncCall>()?;
    let ast::Expr::Ident(callee) = call.callee() else {
        return None;
    };
    let kind = match callee.as_str() {
        "table" => TableKind::Table,
        "grid" => TableKind::Grid,
        _ => return None,
    };
    let args = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Args)?;
    if has_descendant_kind(&args, SyntaxKind::LineComment)
        || has_descendant_kind(&args, SyntaxKind::BlockComment)
    {
        return None;
    }

    let mut columns = 1usize;
    let mut columns_were_explicit = false;
    let mut options = Vec::new();
    let mut flat_cells = Vec::new();
    let mut saw_cell = false;

    for argument in args.children().filter(|child| !child.kind().is_trivia()) {
        let Some(argument_ast) = argument.get().cast::<ast::Arg>() else {
            continue;
        };
        match argument_ast {
            ast::Arg::Named(named) => {
                // Reordering named arguments around positional cells can subtly
                // alter malformed or future syntax, so keep the supported form
                // deliberately canonical.
                if saw_cell {
                    return None;
                }
                let name = named.name().as_str();
                if name == "columns" {
                    if columns_were_explicit {
                        return None;
                    }
                    columns = literal_column_count(named.expr())?;
                    columns_were_explicit = true;
                }
                options.push(PreservedTableOption {
                    name: name.to_owned(),
                    source: source.get(argument.range())?.to_owned(),
                });
            }
            ast::Arg::Pos(ast::Expr::ContentBlock(_)) => {
                saw_cell = true;
                if has_dynamic_content(&argument) {
                    return None;
                }
                let range = argument.range();
                if range.end < range.start + 2 {
                    return None;
                }
                flat_cells.push(source.get(range.start + 1..range.end - 1)?.to_owned());
            }
            ast::Arg::Pos(_) | ast::Arg::Spread(_) => return None,
        }
    }

    if columns == 0 || flat_cells.len() % columns != 0 {
        return None;
    }
    let cells = flat_cells.chunks(columns).map(<[String]>::to_vec).collect();
    let source_range = byte_range_to_char(source, node.range());
    let line_start = source[..node.offset()]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let indentation = source[line_start..node.offset()]
        .chars()
        .take_while(|character| character.is_whitespace())
        .collect();

    Some(EditableTable {
        kind,
        source_range,
        columns,
        cells,
        options,
        original_columns: columns,
        columns_were_explicit,
        indentation,
    })
}

fn literal_column_count(expression: ast::Expr<'_>) -> Option<usize> {
    match expression {
        ast::Expr::Int(value) => usize::try_from(value.get()).ok().filter(|count| *count > 0),
        ast::Expr::Array(array) => {
            let mut count = 0usize;
            for item in array.items() {
                match item {
                    ast::ArrayItem::Pos(_) => count += 1,
                    ast::ArrayItem::Spread(_) => return None,
                }
            }
            (count > 0).then_some(count)
        }
        _ => None,
    }
}

fn has_dynamic_content(node: &LinkedNode<'_>) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::Hash
            | SyntaxKind::Error
            | SyntaxKind::FuncCall
            | SyntaxKind::LetBinding
            | SyntaxKind::ModuleImport
            | SyntaxKind::ModuleInclude
            | SyntaxKind::Spread
    ) || node.children().any(|child| has_dynamic_content(&child))
}

fn has_descendant_kind(node: &LinkedNode<'_>, kind: SyntaxKind) -> bool {
    node.kind() == kind
        || node
            .children()
            .any(|child| has_descendant_kind(&child, kind))
}

impl EditableTable {
    pub(crate) fn row_count(&self) -> usize {
        self.cells.len()
    }

    /// Append an empty row while preserving the model's rectangular shape.
    pub(crate) fn add_row(&mut self) {
        self.cells.push(vec![String::new(); self.columns]);
    }

    /// Remove one row. Empty tables remain a valid, lossless representation.
    pub(crate) fn remove_row(&mut self, row: usize) -> bool {
        if row >= self.cells.len() {
            return false;
        }
        self.cells.remove(row);
        true
    }

    /// Append an empty column to every row.
    pub(crate) fn add_column(&mut self) {
        self.columns += 1;
        for row in &mut self.cells {
            row.push(String::new());
        }
    }

    /// Remove the last column, refusing a zero-column table.
    pub(crate) fn remove_column(&mut self) -> bool {
        if self.columns <= 1 {
            return false;
        }
        self.columns -= 1;
        for row in &mut self.cells {
            row.pop();
        }
        true
    }

    /// Render a canonical replacement call, preserving supported named options
    /// verbatim. If the editor changes the width, a literal numeric `columns`
    /// option replaces the original column specification.
    pub(crate) fn render_replacement(&self) -> Result<String, TableEditError> {
        self.validate()?;
        let child_indent = format!("{}  ", self.indentation);
        let mut arguments = Vec::new();
        let mut emitted_columns = false;
        for option in &self.options {
            if option.name == "columns" {
                emitted_columns = true;
                if self.columns == self.original_columns {
                    arguments.push(option.source.trim().to_owned());
                } else {
                    arguments.push(format!("columns: {}", self.columns));
                }
            } else {
                arguments.push(option.source.trim().to_owned());
            }
        }
        if !emitted_columns && (self.columns != 1 || self.columns_were_explicit) {
            arguments.insert(0, format!("columns: {}", self.columns));
        }
        for row in &self.cells {
            for cell in row {
                arguments.push(format!("[{cell}]"));
            }
        }

        if arguments.is_empty() {
            return Ok(format!("{}()", self.kind.callee()));
        }
        let mut rendered = format!("{}(\n", self.kind.callee());
        for argument in arguments {
            rendered.push_str(&child_indent);
            rendered.push_str(&argument);
            rendered.push_str(",\n");
        }
        rendered.push_str(&self.indentation);
        rendered.push(')');
        Ok(rendered)
    }

    pub(crate) fn source_edit(&self) -> Result<SourceEdit, TableEditError> {
        Ok(SourceEdit {
            range: self.source_range.clone(),
            replacement: self.render_replacement()?,
        })
    }

    fn validate(&self) -> Result<(), TableEditError> {
        if self.columns == 0 {
            return Err(TableEditError::ZeroColumns);
        }
        for (row_index, row) in self.cells.iter().enumerate() {
            if row.len() != self.columns {
                return Err(TableEditError::NonRectangular {
                    row: row_index,
                    expected: self.columns,
                    actual: row.len(),
                });
            }
            for (column_index, cell) in row.iter().enumerate() {
                if !is_static_cell_source(cell) {
                    return Err(TableEditError::DynamicCell {
                        row: row_index,
                        column: column_index,
                    });
                }
            }
        }
        Ok(())
    }
}

fn is_static_cell_source(cell: &str) -> bool {
    let wrapper = format!("#table([{cell}])");
    let parsed = Source::detached(&wrapper);
    let root = LinkedNode::new(parsed.root());
    let Some(table) = find_first_kind(&root, SyntaxKind::FuncCall) else {
        return false;
    };
    let Some(args) = table
        .children()
        .find(|child| child.kind() == SyntaxKind::Args)
    else {
        return false;
    };
    let mut positional = args.children().filter_map(|child| {
        let ast::Arg::Pos(ast::Expr::ContentBlock(_)) = child.get().cast::<ast::Arg>()? else {
            return None;
        };
        Some(child)
    });
    let Some(content) = positional.next() else {
        return false;
    };
    positional.next().is_none()
        && !has_descendant_kind(&table, SyntaxKind::Error)
        && !has_dynamic_content(&content)
}

fn find_first_kind<'a>(node: &LinkedNode<'a>, kind: SyntaxKind) -> Option<LinkedNode<'a>> {
    if node.kind() == kind {
        return Some(node.clone());
    }
    node.children()
        .find_map(|child| find_first_kind(&child, kind))
}

fn contains_byte(range: std::ops::Range<usize>, byte: usize) -> bool {
    range.start <= byte && byte < range.end
}

fn char_to_byte(source: &str, character: usize) -> Option<usize> {
    if character == source.chars().count() {
        return Some(source.len());
    }
    source.char_indices().nth(character).map(|(byte, _)| byte)
}

fn byte_to_char(source: &str, byte: usize) -> usize {
    source[..byte].chars().count()
}

fn byte_range_to_char(source: &str, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
    byte_to_char(source, range.start)..byte_to_char(source, range.end)
}

struct LineMap {
    starts: Vec<usize>,
}

impl LineMap {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(source.match_indices('\n').map(|(byte, _)| byte + 1));
        Self { starts }
    }

    fn line_index_at(&self, byte: usize) -> usize {
        self.starts.partition_point(|start| *start <= byte) - 1
    }

    fn line_range(&self, source: &str, line: usize) -> std::ops::Range<usize> {
        let start = self.starts[line];
        let end = self
            .starts
            .get(line + 1)
            .copied()
            .map_or(source.len(), |next| next.saturating_sub(1));
        start..end
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn char_at(source: &str, needle: &str) -> usize {
        byte_to_char(source, source.find(needle).expect("test needle must exist"))
    }

    #[test]
    fn asset_target_detects_real_calls_and_reports_character_ranges() {
        let root = Path::new("/workspace");
        let path = Path::new("/workspace/chapters/main.typ");
        let source = "é #image(\"../assets/figure.PNG\", width: 2cm)";

        let target = literal_asset_target_at(source, char_at(source, "figure"), path, root)
            .expect("literal image target");

        assert_eq!(target.call_kind, AssetCallKind::Image);
        assert_eq!(target.asset_kind, PreviewAssetKind::Image);
        assert_eq!(
            &source[target.call_range_bytes_for_test(source)],
            "image(\"../assets/figure.PNG\", width: 2cm)"
        );
        assert_eq!(
            &source[target.literal_range_bytes_for_test(source)],
            "\"../assets/figure.PNG\""
        );
        assert_eq!(
            &source[target.value_range_bytes_for_test(source)],
            "../assets/figure.PNG"
        );
        assert_eq!(
            target.resolved_path,
            PathBuf::from("/workspace/assets/figure.PNG")
        );
    }

    #[test]
    fn asset_target_supports_literal_module_include_and_root_relative_paths() {
        let source = "#include \"/exports/report.pdf\"";
        let target = literal_asset_target_at(
            source,
            char_at(source, "report"),
            Path::new("/workspace/main.typ"),
            Path::new("/workspace"),
        )
        .unwrap();

        assert_eq!(target.call_kind, AssetCallKind::Include);
        assert_eq!(target.asset_kind, PreviewAssetKind::Pdf);
        assert_eq!(
            target.resolved_path,
            PathBuf::from("/workspace/exports/report.pdf")
        );
    }

    #[test]
    fn asset_target_accepts_named_options_before_the_literal_but_not_a_dynamic_first_position() {
        let root = Path::new("/workspace");
        let path = Path::new("/workspace/main.typ");
        let named_first = "#image(width: 4cm, \"figure.webp\")";
        assert_eq!(
            literal_asset_target_at(named_first, char_at(named_first, "image"), path, root,)
                .unwrap()
                .resolved_path,
            PathBuf::from("/workspace/figure.webp")
        );

        let dynamic_first = "#image(path, \"fallback.png\")";
        assert!(
            literal_asset_target_at(dynamic_first, char_at(dynamic_first, "image"), path, root,)
                .is_none()
        );
    }

    #[test]
    fn asset_target_ignores_markup_lookalikes_and_non_target_calls() {
        for source in [
            "image(\"figure.png\")",
            "#figure(image: \"figure.png\")",
            "#other(\"figure.png\")",
        ] {
            assert!(
                literal_asset_target_at(
                    source,
                    char_at(source, "figure"),
                    Path::new("/workspace/main.typ"),
                    Path::new("/workspace"),
                )
                .is_none(),
                "unexpected target for {source:?}"
            );
        }
    }

    #[test]
    fn asset_target_rejects_dynamic_urls_extensions_and_workspace_escape() {
        let cases = [
            "#image(path)",
            "#image(\"https://example.com/a.png\")",
            "#image(\"data:image/png;base64,abc.png\")",
            "#image(\"notes.txt\")",
            "#image(\"../../outside.png\")",
            "#include (\"report.pdf\" + suffix)",
        ];
        for source in cases {
            let cursor = source
                .chars()
                .position(|character| character != '#')
                .unwrap();
            assert!(
                literal_asset_target_at(
                    source,
                    cursor,
                    Path::new("/workspace/chapters/main.typ"),
                    Path::new("/workspace"),
                )
                .is_none(),
                "unexpected target for {source:?}"
            );
        }
    }

    #[test]
    fn asset_target_rejects_a_source_document_outside_the_workspace() {
        let source = "#image(\"figure.png\")";
        assert!(
            literal_asset_target_at(
                source,
                char_at(source, "figure"),
                Path::new("/other/main.typ"),
                Path::new("/workspace"),
            )
            .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn asset_target_rejects_existing_symlink_escape() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.png"), b"not really an image").unwrap();
        symlink(outside.path(), workspace.path().join("escape")).unwrap();
        let source_path = workspace.path().join("main.typ");
        fs::write(&source_path, "#image(\"escape/secret.png\")").unwrap();

        assert!(
            literal_asset_target_at(
                "#image(\"escape/secret.png\")",
                10,
                &source_path,
                workspace.path(),
            )
            .is_none()
        );
    }

    #[test]
    fn sticky_rows_track_heading_hierarchy_and_nested_scopes() {
        let source = "= First section\nintro\n== Old subsection\ntext\n== Current subsection\n#let render(body) = {\n  let local = {\n    body\n  }\n}\n";
        let rows = sticky_context_rows(source, char_at(source, "body\n"));

        assert_eq!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            [
                "= First section",
                "== Current subsection",
                "#let render(body) = {",
                "let local = {",
            ]
        );
        assert_eq!(
            rows.iter().map(|row| row.kind).collect::<Vec<_>>(),
            [
                StickyContextKind::Heading { level: 1 },
                StickyContextKind::Heading { level: 2 },
                StickyContextKind::Function,
                StickyContextKind::LetBinding,
            ]
        );
        assert_eq!(
            rows.iter().map(|row| row.line).collect::<Vec<_>>(),
            [1, 5, 6, 7]
        );
        assert_eq!(rows[2].char_index, char_at(source, "#let render"));
    }

    #[test]
    fn sticky_rows_replace_sibling_and_descendant_headings() {
        let source = "= One\n== One A\n=== One A i\n== One B\ntext\n= Two\nend";
        let one_b = sticky_context_rows(source, char_at(source, "text"));
        assert_eq!(
            one_b
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>(),
            ["= One", "== One B"]
        );
        let two = sticky_context_rows(source, char_at(source, "end"));
        assert_eq!(
            two.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            ["= Two"]
        );
    }

    #[test]
    fn sticky_rows_ignore_headings_nested_in_content_values() {
        let source =
            "= Document\n#let fragment = [\n  == Not a document section\n  nested\n]\nafter";
        let rows = sticky_context_rows(source, char_at(source, "after"));
        assert_eq!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            ["= Document"]
        );
    }

    #[test]
    fn sticky_rows_use_character_positions_for_unicode_source() {
        let source = "= Héading\n#let f() = {\n  世界\n}";
        let rows = sticky_context_rows(source, char_at(source, "世界"));
        assert_eq!(rows[0].char_index, 0);
        assert_eq!(rows[1].char_index, "= Héading\n".chars().count());
        assert_eq!(rows[1].line, 2);
    }

    #[test]
    fn sticky_rows_include_a_standalone_block_and_stop_after_it() {
        let source = "= Section\n#for item in values {\n  item\n}\nafter";
        let inside = sticky_context_rows(source, char_at(source, "  item"));
        assert_eq!(
            inside
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>(),
            ["= Section", "#for item in values {"]
        );
        assert_eq!(inside[1].kind, StickyContextKind::Block);

        let after = sticky_context_rows(source, char_at(source, "after"));
        assert_eq!(
            after
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>(),
            ["= Section"]
        );
        assert!(sticky_context_rows(source, source.chars().count() + 1).is_empty());
    }

    #[test]
    fn sticky_rows_expand_a_multiline_definition_header() {
        let source = "#let a(\n  b,\n  c,\n) = 2\nnext";
        let rows = sticky_context_rows(source, char_at(source, "2"));

        assert_eq!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            ["#let a(", "b,", "c,", ") = 2"]
        );
        assert_eq!(
            rows.iter().map(|row| row.line).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert!(
            rows.iter()
                .all(|row| row.kind == StickyContextKind::Function)
        );
        assert_eq!(rows[0].char_index, char_at(source, "#let a"));
        assert_eq!(rows[1].char_index, char_at(source, "b,"));
        assert_eq!(rows[2].char_index, char_at(source, "c,"));
        assert_eq!(rows[3].char_index, char_at(source, ") = 2"));
    }

    #[test]
    fn sticky_rows_stop_a_multiline_function_header_at_its_body() {
        let source = "#let render(\n  body,\n) = {\n  let ordinary = 1\n  body\n}\nafter";
        let rows = sticky_context_rows(source, char_at(source, "  body\n}"));

        assert_eq!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            ["#let render(", "body,", ") = {"]
        );
        assert_eq!(
            rows.iter().map(|row| row.line).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(!rows.iter().any(|row| row.text.contains("ordinary")));
        assert_eq!(
            rows.iter()
                .map(|row| row.line)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            rows.len(),
            "the LetBinding, Closure, and CodeBlock nodes must not duplicate header lines"
        );
    }

    #[test]
    fn sticky_rows_stack_nested_multiline_headers_without_body_lines() {
        let source = "#let outer(\n  x,\n) = {\n  let ordinary = 1\n  #let inner(\n    y,\n  ) = {\n    y\n  }\n}";
        let rows = sticky_context_rows(source, char_at(source, "    y\n"));

        assert_eq!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            ["#let outer(", "x,", ") = {", "#let inner(", "y,", ") = {",]
        );
        assert_eq!(
            rows.iter().map(|row| row.line).collect::<Vec<_>>(),
            [1, 2, 3, 5, 6, 7]
        );
        assert!(!rows.iter().any(|row| row.text == "let ordinary = 1"));
        assert!(!rows.iter().any(|row| row.text == "y"));
    }

    #[test]
    fn sticky_multiline_header_indices_remain_character_based_with_unicode() {
        let source = "  #let café(\n    世界,\n  ) = {\n    世界\n  }";
        let rows = sticky_context_rows(source, char_at(source, "    世界\n"));

        assert_eq!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            ["#let café(", "世界,", ") = {"]
        );
        assert_eq!(rows[0].char_index, char_at(source, "#let café"));
        assert_eq!(rows[1].char_index, char_at(source, "世界,"));
        assert_eq!(rows[2].char_index, char_at(source, ") = {"));
    }

    #[test]
    fn sticky_rows_exclude_single_line_code_but_keep_headings() {
        for statement in [
            "#let answer = 42",
            "#set text(size: 12pt)",
            "#show heading: emph",
        ] {
            let source = format!("= Section\n{statement}\nafter");
            let rows = sticky_context_rows(&source, char_at(&source, statement));
            assert_eq!(
                rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
                ["= Section"],
                "single-line statement became sticky: {statement}"
            );
        }
    }

    #[test]
    fn sticky_rows_include_multiline_rules_calls_and_raw_blocks() {
        for (source, needle, expected) in [
            (
                "#set text(\n  size: 12pt,\n)\nafter",
                "size: 12pt",
                "#set text(",
            ),
            (
                "#show heading: it => [\n  #emph(it.body)\n]\nafter",
                "#emph",
                "#show heading: it => [",
            ),
            (
                "#figure(\n  image(\"plot.png\"),\n)\nafter",
                "image",
                "#figure(",
            ),
            ("```rust\nfn main() {\n}\n```\nafter", "fn main", "```rust"),
        ] {
            let rows = sticky_context_rows(source, char_at(source, needle));
            assert!(
                rows.iter().any(|row| row.text == expected),
                "missing {expected:?} for {source:?}; got {rows:?}"
            );
        }
    }

    #[test]
    fn sticky_rows_record_the_line_that_pushes_each_context_away() {
        let source = "= Outer\nintro\n== Inner\nbody\n= Next\nafter";
        let rows = sticky_context_rows(source, char_at(source, "body"));

        assert_eq!(
            rows.iter()
                .map(|row| (row.text.as_str(), row.end_line))
                .collect::<Vec<_>>(),
            [("= Outer", 5), ("== Inner", 5)]
        );

        let source = "#let value = (\n  1 + 2\n)\nafter";
        let rows = sticky_context_rows(source, char_at(source, "1 + 2"));
        assert!(rows.iter().all(|row| row.end_line == 4), "{rows:?}");
    }

    #[test]
    fn table_parser_builds_a_rectangular_model_and_preserves_options() {
        let source = "#table(\n  columns: (1fr, 2fr),\n  inset: 4pt,\n  [*Name*], [Value],\n  [Alpha], [42],\n)";
        let table = editable_table_at(source, char_at(source, "Alpha")).unwrap();

        assert_eq!(table.kind, TableKind::Table);
        assert_eq!(table.columns, 2);
        assert_eq!(
            table.cells,
            vec![
                vec!["*Name*".to_owned(), "Value".to_owned()],
                vec!["Alpha".to_owned(), "42".to_owned()],
            ]
        );
        let edit = table.source_edit().unwrap();
        assert_eq!(
            &source[char_range_to_byte(source, edit.range.clone())],
            &source[1..]
        );
        assert!(edit.replacement.contains("columns: (1fr, 2fr),"));
        assert!(edit.replacement.contains("inset: 4pt,"));
        assert!(edit.replacement.contains("[*Name*],"));
    }

    #[test]
    fn table_edit_updates_dimensions_and_round_trips() {
        let source = "#grid(columns: 2, [A], [B], [C], [D])";
        let mut table = editable_table_at(source, char_at(source, "[C]")).unwrap();
        table.columns = 3;
        table.cells = vec![
            vec!["A".into(), "B".into(), "C".into()],
            vec!["D".into(), "E".into(), "F".into()],
        ];

        let replacement = table.render_replacement().unwrap();
        assert!(replacement.starts_with("grid(\n"));
        assert!(replacement.contains("columns: 3,"));
        let reparsed = format!("#{replacement}");
        let round_trip = editable_table_at(&reparsed, char_at(&reparsed, "[E]")).unwrap();
        assert_eq!(round_trip.columns, 3);
        assert_eq!(round_trip.cells, table.cells);
    }

    #[test]
    fn table_resize_operations_keep_the_model_rectangular() {
        let source = "#table(columns: 2, [A], [B], [C], [D])";
        let mut table = editable_table_at(source, char_at(source, "[A]")).unwrap();

        table.add_row();
        assert_eq!(table.row_count(), 3);
        assert_eq!(table.cells[2], vec![String::new(), String::new()]);

        table.add_column();
        assert_eq!(table.columns, 3);
        assert!(table.cells.iter().all(|row| row.len() == 3));
        assert!(table.remove_row(1));
        assert!(!table.remove_row(20));
        assert_eq!(table.row_count(), 2);

        assert!(table.remove_column());
        assert!(table.remove_column());
        assert!(!table.remove_column());
        assert_eq!(table.columns, 1);
        assert!(table.cells.iter().all(|row| row.len() == 1));
        assert!(table.render_replacement().is_ok());
    }

    #[test]
    fn table_parser_supports_default_columns_and_unicode_source_ranges() {
        let source = "é\n  #table([Héllo], [World])";
        let table = editable_table_at(source, char_at(source, "Héllo")).unwrap();
        assert_eq!(table.columns, 1);
        assert_eq!(
            table.cells,
            vec![vec!["Héllo".to_owned()], vec!["World".to_owned()]]
        );

        let edit = table.source_edit().unwrap();
        assert_eq!(
            &source[char_range_to_byte(source, edit.range)],
            "table([Héllo], [World])"
        );
        assert!(edit.replacement.contains("\n    [Héllo],\n"));
        assert!(edit.replacement.ends_with("\n  )"));
    }

    #[test]
    fn table_parser_rejects_dynamic_or_lossy_shapes() {
        let cases = [
            "#table(columns: count, [A])",
            "#table(columns: 2, ..cells)",
            "#table(columns: 2, [A], [B], [incomplete])",
            "#table(columns: 2, [A], table.cell(colspan: 2)[B])",
            "#table(columns: 2, [#value], [B])",
            "#table(columns: 2, [A], /* keep me */ [B])",
            "#table([A], columns: 1)",
            "#table(columns: (1fr, ..sizes), [A], [B])",
        ];
        for source in cases {
            assert!(
                editable_table_at(source, char_at(source, "table")).is_none(),
                "unexpected editable model for {source:?}"
            );
        }
    }

    #[test]
    fn table_parser_ignores_markup_lookalikes_and_qualified_calls() {
        for source in ["table(columns: 1, [A])", "#custom.table(columns: 1, [A])"] {
            assert!(editable_table_at(source, char_at(source, "table")).is_none());
        }
    }

    #[test]
    fn table_parser_treats_the_markup_hash_as_part_of_the_click_target() {
        let source = "before #table([A]) after";
        let hash = char_at(source, "#table");
        let table = editable_table_at(source, hash).expect("table at leading hash");
        assert_eq!(table.cells, vec![vec!["A".to_owned()]]);
    }

    #[test]
    fn table_edit_refuses_invalid_mutations() {
        let source = "#table(columns: 2, [A], [B])";
        let mut table = editable_table_at(source, char_at(source, "[A]")).unwrap();
        table.cells[0].pop();
        assert!(matches!(
            table.render_replacement(),
            Err(TableEditError::NonRectangular {
                row: 0,
                expected: 2,
                actual: 1
            })
        ));

        table.cells[0].push("#dynamic".into());
        assert!(matches!(
            table.render_replacement(),
            Err(TableEditError::DynamicCell { row: 0, column: 1 })
        ));

        table.columns = 0;
        assert_eq!(table.render_replacement(), Err(TableEditError::ZeroColumns));
    }

    impl LiteralAssetTarget {
        fn call_range_bytes_for_test(&self, source: &str) -> std::ops::Range<usize> {
            char_range_to_byte(source, self.call_range.clone())
        }

        fn literal_range_bytes_for_test(&self, source: &str) -> std::ops::Range<usize> {
            char_range_to_byte(source, self.literal_range.clone())
        }

        fn value_range_bytes_for_test(&self, source: &str) -> std::ops::Range<usize> {
            char_range_to_byte(source, self.value_range.clone())
        }
    }

    fn char_range_to_byte(source: &str, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
        char_to_byte(source, range.start).unwrap()..char_to_byte(source, range.end).unwrap()
    }
}
