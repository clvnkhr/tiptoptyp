//! Lossless grids for literal Typst math calls. Cells remain math expressions.
use super::{SourceEdit, byte_range_to_char, char_to_byte, contains_byte};
use std::{
    collections::{BTreeSet, HashSet},
    fmt,
    ops::Range,
};
use typst_syntax::{LinkedNode, Source, SyntaxKind, ast};

const MAX_CELLS: usize = 4096;
const MAX_COLUMNS: usize = 128;
const MAX_SOURCE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MathTableKind {
    Matrix,
    Vector,
    Cases,
}
impl MathTableKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Matrix => "Matrix",
            Self::Vector => "Vector",
            Self::Cases => "Cases",
        }
    }
    fn from_callee(callee: &str) -> Option<Self> {
        match callee {
            "mat" | "math.mat" => Some(Self::Matrix),
            "vec" | "math.vec" => Some(Self::Vector),
            "cases" | "math.cases" => Some(Self::Cases),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MathTable {
    pub(crate) source_range: Range<usize>,
    pub(crate) source: String,
    pub(crate) kind: MathTableKind,
    pub(crate) columns: usize,
    pub(crate) cells: Vec<Vec<String>>,
    original_cells: Vec<Vec<String>>,
    cell_ranges: Vec<Vec<Option<Range<usize>>>>,
    options: Vec<MathOption>,
    open_end: usize,
    closing_trivia: String,
    delimiter_override: Option<Option<String>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct MathOption {
    name: String,
    source: String,
    value_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MathTableError {
    TooLarge,
    InvalidGeometry,
    InvalidMath,
    InvalidDelimiter,
    InvalidAugmentation,
}
impl fmt::Display for MathTableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "The math editor supports up to 4,096 cells and 128 columns.",
            Self::InvalidGeometry => "Every row must have the same number of cells; vectors have one column.",
            Self::InvalidMath => "Enter one complete math expression per cell, without an outer $ pair or extra cell separators.",
            Self::InvalidDelimiter => "Choose inherited, parentheses, brackets, braces, vertical bars, double bars, or no delimiters.",
            Self::InvalidAugmentation => "The preserved augmentation lines do not fit the smaller matrix, or their offsets are computed. Adjust augment in the document before shrinking this grid.",
        })
    }
}
impl std::error::Error for MathTableError {}

pub(crate) fn math_table_at(source: &str, char_cursor: usize) -> Option<MathTable> {
    math_table_in_source(
        &Source::detached(source),
        char_to_byte(source, char_cursor)?,
    )
}
pub(crate) fn math_table_in_source(parsed: &Source, byte_cursor: usize) -> Option<MathTable> {
    find_math_table(&LinkedNode::new(parsed.root()), parsed.text(), byte_cursor).flatten()
}
fn find_math_table(
    node: &LinkedNode<'_>,
    source: &str,
    cursor: usize,
) -> Option<Option<MathTable>> {
    if !contains_byte(node.range(), cursor) {
        return None;
    }
    if let Some(found) = node
        .children()
        .find_map(|child| find_math_table(&child, source, cursor))
    {
        return Some(found);
    }
    if node.kind() != SyntaxKind::MathCall {
        return None;
    }
    let callee = node.children().next()?;
    MathTableKind::from_callee(source.get(callee.range())?)?;
    Some(parse_math_table(node, source))
}

struct Segment {
    range: Range<usize>,
    separator: Option<char>,
}
fn parse_math_table(node: &LinkedNode<'_>, source: &str) -> Option<MathTable> {
    if node.range().len() > MAX_SOURCE_BYTES || node.get().diagnosis().errors {
        return None;
    }
    let call_start = node.offset();
    let callee = node.children().next()?;
    let kind = MathTableKind::from_callee(source.get(callee.range())?)?;
    let args = node
        .children()
        .find(|child| child.kind() == SyntaxKind::MathArgs)?;
    let args_ast = args.get().cast::<ast::MathArgs>()?;
    let punctuation: HashSet<_> = args_ast
        .content_items()
        .filter_map(|item| match item {
            ast::MathArgItem::Comma(_, token) | ast::MathArgItem::Semicolon(_, token) => {
                Some(token.span())
            }
            _ => None,
        })
        .collect();
    let children: Vec<_> = args.children().collect();
    let open = children
        .iter()
        .find(|child| child.kind() == SyntaxKind::LeftParen)?;
    let close = children
        .iter()
        .rev()
        .find(|child| child.kind() == SyntaxKind::RightParen)?;
    let mut start = open.range().end;
    let mut segments = Vec::new();
    for child in &children {
        if punctuation.contains(&child.get().span()) {
            segments.push(Segment {
                range: start..child.offset(),
                separator: source.get(child.range())?.chars().next(),
            });
            start = child.range().end;
        }
    }
    segments.push(Segment {
        range: start..close.offset(),
        separator: None,
    });
    if segments.len() > MAX_CELLS + 32 {
        return None;
    }
    let mut rows: Vec<Vec<String>> = vec![Vec::new()];
    let mut ranges: Vec<Vec<Option<Range<usize>>>> = vec![Vec::new()];
    let mut options = Vec::new();
    let mut names = BTreeSet::new();
    let mut closing_trivia = String::new();
    let mut saw_semicolon = false;
    let mut argument_nodes = children
        .iter()
        .filter_map(|child| child.get().cast::<ast::Arg>().map(|arg| (child, arg)))
        .peekable();
    for (index, segment) in segments.iter().enumerate() {
        let mut arguments = Vec::new();
        while argument_nodes
            .peek()
            .is_some_and(|(child, _)| child.range().end <= segment.range.end)
        {
            let argument = argument_nodes.next()?;
            if argument.0.offset() < segment.range.start {
                return None;
            }
            arguments.push(argument);
        }
        if arguments.is_empty() {
            if index + 1 == segments.len() && segment.separator.is_none() {
                closing_trivia = source.get(segment.range.clone())?.to_owned();
                continue;
            }
            // An empty mathematical argument is valid; the parser exposes it
            // as an empty Math node. A missing node instead indicates trivia.
            return None;
        }
        if arguments.len() != 1 {
            return None;
        }
        let (argument, arg) = arguments[0];
        match arg {
            ast::Arg::Spread(_) => return None,
            ast::Arg::Named(named) => {
                if segment.separator == Some(';') || !names.insert(named.name().as_str().to_owned())
                {
                    return None;
                }
                let colon = argument
                    .children()
                    .find(|child| child.kind() == SyntaxKind::Colon)?;
                let value = trim_range(source, colon.range().end..argument.range().end)?;
                options.push(MathOption {
                    name: named.name().as_str().into(),
                    source: source.get(segment.range.clone())?.trim().into(),
                    value_range: value.start - call_start..value.end - call_start,
                });
            }
            ast::Arg::Pos(_) => {
                let range = trim_range(source, segment.range.clone())?;
                if kind == MathTableKind::Cases {
                    let mut alignment = Vec::new();
                    alignment_points(argument, &mut alignment);
                    let mut cell_start = range.start;
                    let mut row = Vec::new();
                    let mut row_ranges = Vec::new();
                    for amp in alignment {
                        let cell = trim_range(source, cell_start..amp.start)?;
                        row.push(source.get(cell.clone())?.into());
                        row_ranges.push(Some(cell.start - call_start..cell.end - call_start));
                        cell_start = amp.end;
                    }
                    let cell = trim_range(source, cell_start..range.end)?;
                    row.push(source.get(cell.clone())?.into());
                    row_ranges.push(Some(cell.start - call_start..cell.end - call_start));
                    rows.push(row);
                    ranges.push(row_ranges);
                } else if kind == MathTableKind::Vector {
                    rows.push(vec![source.get(range.clone())?.into()]);
                    ranges.push(vec![Some(range.start - call_start..range.end - call_start)]);
                } else {
                    rows.last_mut()?.push(source.get(range.clone())?.into());
                    ranges
                        .last_mut()?
                        .push(Some(range.start - call_start..range.end - call_start));
                    if segment.separator == Some(';') {
                        saw_semicolon = true;
                        rows.push(Vec::new());
                        ranges.push(Vec::new());
                    }
                }
                if kind != MathTableKind::Matrix && segment.separator == Some(';') {
                    return None;
                }
            }
        }
    }
    if kind == MathTableKind::Matrix {
        // Without a semicolon these are scalar arguments, not matrix rows.
        if !saw_semicolon {
            return None;
        }
        if rows.last().is_some_and(Vec::is_empty) {
            rows.pop();
            ranges.pop();
        }
    } else {
        rows.remove(0);
        ranges.remove(0);
    }
    let columns = rows.iter().map(Vec::len).max()?;
    if columns == 0
        || columns > MAX_COLUMNS
        || rows.is_empty()
        || rows.len().saturating_mul(columns) > MAX_CELLS
    {
        return None;
    }
    if kind == MathTableKind::Cases {
        for (row, ranges) in rows.iter_mut().zip(&mut ranges) {
            row.resize(columns, String::new());
            ranges.resize(columns, None);
        }
    } else if rows.iter().any(|row| row.len() != columns) {
        return None;
    }
    Some(MathTable {
        source_range: byte_range_to_char(source, node.range()),
        source: source.get(node.range())?.into(),
        kind,
        columns,
        original_cells: rows.clone(),
        cells: rows,
        cell_ranges: ranges,
        options,
        open_end: open.range().end - call_start,
        closing_trivia,
        delimiter_override: None,
    })
}

fn alignment_points(node: &LinkedNode<'_>, output: &mut Vec<Range<usize>>) {
    if node.kind() == SyntaxKind::MathAlignPoint {
        output.push(node.range());
    }
    // Nested calls, delimited expressions and embedded code own their '&'.
    if node.kind() == SyntaxKind::Math {
        for child in node.children() {
            alignment_points(&child, output);
        }
    }
}
fn trim_range(source: &str, range: Range<usize>) -> Option<Range<usize>> {
    let text = source.get(range.clone())?;
    let start = range.start + text.len() - text.trim_start().len();
    let end = start + text.trim().len();
    Some(start..end)
}

impl MathTable {
    pub(crate) fn row_count(&self) -> usize {
        self.cells.len()
    }
    pub(crate) fn can_add_row(&self) -> bool {
        (self.row_count() + 1).saturating_mul(self.columns) <= MAX_CELLS
    }
    pub(crate) fn can_add_column(&self) -> bool {
        self.kind != MathTableKind::Vector
            && self.columns < MAX_COLUMNS
            && self.row_count().saturating_mul(self.columns + 1) <= MAX_CELLS
    }
    pub(crate) fn add_row(&mut self) {
        if self.can_add_row() {
            self.cells.push(
                (0..self.columns)
                    .map(|column| {
                        if self.kind == MathTableKind::Cases && column > 0 {
                            String::new()
                        } else {
                            "0".into()
                        }
                    })
                    .collect(),
            );
        }
    }
    pub(crate) fn remove_row(&mut self) {
        if self.row_count() > 1 {
            self.cells.pop();
        }
    }
    pub(crate) fn add_column(&mut self) {
        if self.can_add_column() {
            for row in &mut self.cells {
                row.push(if self.kind == MathTableKind::Cases {
                    String::new()
                } else {
                    "0".into()
                });
            }
            self.columns += 1;
        }
    }
    pub(crate) fn remove_column(&mut self) {
        if self.columns > 1 {
            for row in &mut self.cells {
                row.pop();
            }
            self.columns -= 1;
        }
    }
    pub(crate) fn delimiter(&self) -> Option<String> {
        let raw = match &self.delimiter_override {
            Some(value) => value.as_deref(),
            None => self
                .options
                .iter()
                .find(|option| option.name == "delim")
                .map(|option| &self.source[option.value_range.clone()]),
        }?;
        if raw == "#none" {
            Some("none".into())
        } else if raw.starts_with('"') && raw.ends_with('"') {
            Some(raw[1..raw.len() - 1].into())
        } else {
            Some(raw.into())
        }
    }
    pub(crate) fn set_delimiter(&mut self, delimiter: &str) -> Result<(), MathTableError> {
        self.delimiter_override = Some(match delimiter {
            "" => None,
            "none" => Some("#none".into()),
            "(" | "[" | "{" | "|" | "‖" => Some(format!("\"{delimiter}\"")),
            _ => return Err(MathTableError::InvalidDelimiter),
        });
        Ok(())
    }
    fn validate_geometry(&self) -> Result<(), MathTableError> {
        if self.columns > MAX_COLUMNS
            || self.row_count().saturating_mul(self.columns) > MAX_CELLS
            || self
                .cells
                .iter()
                .flatten()
                .fold(0usize, |bytes, cell| bytes.saturating_add(cell.len()))
                > MAX_SOURCE_BYTES
        {
            return Err(MathTableError::TooLarge);
        }
        if self.columns == 0
            || self.cells.is_empty()
            || self.cells.iter().any(|row| row.len() != self.columns)
            || (self.kind == MathTableKind::Vector && self.columns != 1)
        {
            return Err(MathTableError::InvalidGeometry);
        }
        if self.kind == MathTableKind::Matrix
            && (self.row_count() < self.original_cells.len()
                || self.columns < self.original_cells[0].len())
            && let Some(augment) = self.options.iter().find(|option| option.name == "augment")
            && !augmentation_fits(
                &self.source[augment.value_range.clone()],
                self.row_count(),
                self.columns,
            )
        {
            return Err(MathTableError::InvalidAugmentation);
        }
        Ok(())
    }
    pub(crate) fn render_replacement(&self) -> Result<String, MathTableError> {
        self.validate_geometry()?;
        let same_shape = self.cells.len() == self.original_cells.len()
            && self.columns == self.original_cells[0].len();
        let missing_cell_changed = self
            .cells
            .iter()
            .zip(&self.cell_ranges)
            .any(|(row, ranges)| {
                row.iter()
                    .zip(ranges)
                    .any(|(cell, range)| range.is_none() && !cell.is_empty())
            });
        let candidate = if same_shape
            && !missing_cell_changed
            && !matches!(self.delimiter_override, Some(None))
        {
            let mut patches = Vec::new();
            for (row, cells) in self.cells.iter().enumerate() {
                for (column, cell) in cells.iter().enumerate() {
                    if cell != &self.original_cells[row][column]
                        && let Some(range) = &self.cell_ranges[row][column]
                    {
                        patches.push((range.clone(), cell.clone()));
                    }
                }
            }
            if let Some(Some(value)) = &self.delimiter_override {
                if let Some(option) = self.options.iter().find(|option| option.name == "delim") {
                    patches.push((option.value_range.clone(), value.clone()));
                } else {
                    patches.push((self.open_end..self.open_end, format!("delim: {value}, ")));
                }
            }
            patches.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
            let mut source = self.source.clone();
            for (range, value) in patches {
                source.replace_range(range, &value);
            }
            source
        } else {
            self.render_geometry()
        };
        if candidate.len() > MAX_SOURCE_BYTES {
            return Err(MathTableError::TooLarge);
        }
        // Parse the draft as math and compare structure, including named
        // arguments. This rejects cells that inject another row/option/call.
        let wrapped = format!("$ {candidate} $");
        let parsed = Source::detached(wrapped);
        let root = LinkedNode::new(parsed.root());
        let call = first_math_call(&root).ok_or(MathTableError::InvalidMath)?;
        let roundtrip =
            parse_math_table(&call, parsed.text()).ok_or(MathTableError::InvalidMath)?;
        if roundtrip.source != candidate
            || roundtrip.kind != self.kind
            || roundtrip.columns != self.columns
            || roundtrip
                .cells
                .iter()
                .map(|row| row.iter().map(|cell| cell.trim()).collect::<Vec<_>>())
                .collect::<Vec<_>>()
                != self
                    .cells
                    .iter()
                    .map(|row| row.iter().map(|cell| cell.trim()).collect::<Vec<_>>())
                    .collect::<Vec<_>>()
        {
            return Err(MathTableError::InvalidMath);
        }
        let expected_names: BTreeSet<&str> = self
            .options
            .iter()
            .filter(|option| option.name != "delim" || self.delimiter_override != Some(None))
            .map(|option| option.name.as_str())
            .chain(matches!(self.delimiter_override, Some(Some(_))).then_some("delim"))
            .collect();
        if roundtrip
            .options
            .iter()
            .map(|option| option.name.as_str())
            .collect::<BTreeSet<_>>()
            != expected_names
        {
            return Err(MathTableError::InvalidMath);
        }
        Ok(candidate)
    }
    fn render_geometry(&self) -> String {
        let mut source = self.source[..self.open_end].to_owned();
        source.push('\n');
        for option in &self.options {
            if option.name == "delim" && self.delimiter_override.is_some() {
                continue;
            }
            source.push_str("  ");
            source.push_str(&option.source);
            if option.source.contains("//") {
                source.push('\n');
            }
            source.push_str(",\n");
        }
        if let Some(Some(value)) = &self.delimiter_override {
            source.push_str(&format!("  delim: {value},\n"));
        }
        for row in &self.cells {
            for (column, cell) in row.iter().enumerate() {
                if column == 0 {
                    source.push_str("  ");
                }
                source.push_str(cell.trim());
                if column + 1 < row.len() {
                    // A newline keeps a trailing line comment from swallowing
                    // separators without trying to lex arbitrary math strings.
                    if cell.contains("//") {
                        source.push('\n');
                    }
                    source.push_str(if self.kind == MathTableKind::Cases {
                        " & "
                    } else {
                        ", "
                    });
                }
            }
            if row.last().is_some_and(|cell| cell.contains("//")) {
                source.push('\n');
            }
            source.push_str(if self.kind == MathTableKind::Matrix {
                ";\n"
            } else {
                ",\n"
            });
        }
        if !self.closing_trivia.trim().is_empty() {
            source.push_str(&self.closing_trivia);
        }
        source.push(')');
        source
    }
    pub(crate) fn source_edit(&self) -> Result<SourceEdit, MathTableError> {
        Ok(SourceEdit {
            range: self.source_range.clone(),
            replacement: self.render_replacement()?,
        })
    }
}
fn first_math_call<'a>(node: &LinkedNode<'a>) -> Option<LinkedNode<'a>> {
    if node.kind() == SyntaxKind::MathCall {
        return Some(node.clone());
    }
    node.children().find_map(|child| first_math_call(&child))
}

// Resolve only literal augmentation geometry. User code is never evaluated.
fn augmentation_fits(raw: &str, rows: usize, columns: usize) -> bool {
    let expression = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
    let parsed = Source::detached(format!("#let augmentation = {expression}"));
    if parsed.root().diagnosis().errors {
        return false;
    }
    let Some(binding) = parsed
        .root()
        .children()
        .find_map(|node| node.cast::<ast::LetBinding>())
    else {
        return false;
    };
    let Some(expression) = binding.init() else {
        return false;
    };
    let expression = unparen(expression);
    match expression {
        ast::Expr::None(_) => true,
        ast::Expr::Dict(dictionary) => dictionary.items().all(|item| match item {
            ast::DictItem::Named(named) => match named.name().as_str() {
                "hline" => offsets_fit(named.expr(), rows),
                "vline" => offsets_fit(named.expr(), columns),
                "stroke" => true,
                _ => false,
            },
            _ => false,
        }),
        _ => offsets_fit(expression, columns),
    }
}
fn unparen(mut expression: ast::Expr<'_>) -> ast::Expr<'_> {
    while let ast::Expr::Parenthesized(value) = expression {
        expression = value.expr();
    }
    expression
}
fn literal_integer(expression: ast::Expr<'_>) -> Option<i64> {
    match unparen(expression) {
        ast::Expr::Int(value) => Some(value.get()),
        ast::Expr::Unary(value) => match value.op() {
            ast::UnOp::Pos => literal_integer(value.expr()),
            ast::UnOp::Neg => literal_integer(value.expr())?.checked_neg(),
            _ => None,
        },
        _ => None,
    }
}
fn offsets_fit(expression: ast::Expr<'_>, dimension: usize) -> bool {
    match unparen(expression) {
        ast::Expr::Array(array) => array.items().all(|item| match item {
            ast::ArrayItem::Pos(value) => literal_integer(value)
                .is_some_and(|offset| offset.unsigned_abs() <= dimension as u64),
            _ => false,
        }),
        expression => literal_integer(expression)
            .is_some_and(|offset| offset.unsigned_abs() <= dimension as u64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grid(source: &str) -> MathTable {
        math_table_at(
            source,
            source
                .find("mat(")
                .or_else(|| source.find("vec("))
                .or_else(|| source.find("cases("))
                .unwrap(),
        )
        .unwrap_or_else(|| panic!("not a literal math grid: {source}"))
    }
    #[test]
    fn native_journey_fixture_exposes_literal_formula_grids_and_retains_options() {
        let source = include_str!("../../scripts/fixtures/math-grid-editor.typ");
        for (callee, kind, rows, columns) in [
            ("mat(", MathTableKind::Matrix, 2, 2),
            ("vec(", MathTableKind::Vector, 3, 1),
            ("cases(", MathTableKind::Cases, 2, 2),
        ] {
            let table = math_table_at(source, source.find(callee).unwrap()).unwrap();
            assert_eq!(table.kind, kind);
            assert_eq!((table.row_count(), table.columns), (rows, columns));
            assert_eq!(table.render_replacement().unwrap(), table.source);
        }
        let table = grid(source);
        assert_eq!(
            table.cells,
            vec![vec!["x", "alpha"], vec!["0", "frac(1, 2)"]]
        );
        assert_eq!(table.delimiter().as_deref(), Some("["));
        assert!(table.source.contains("align: std.center"));
    }
    #[test]
    fn single_column_matrices_empty_cells_and_cases_are_still_editable() {
        for source in [
            "$ mat(x; y;) $",
            "$ mat(x;) $",
            "$ mat(,x;y,;) $",
            "$ cases(x &, y & ) $",
        ] {
            let mut table = grid(source);
            assert_eq!(
                table.render_replacement().unwrap(),
                table.source,
                "{source}"
            );
            table.add_row();
            assert!(table.render_replacement().is_ok(), "{source}");
        }
    }
    #[test]
    fn matrix_vector_and_cases_keep_exact_source_and_math_cells() {
        for source in [
            "$ mat(x, alpha; frac(1, 2), x^2) $",
            "$ vec(x, alpha, frac(1,2), delim: \"[\") $",
            "$ cases(x & x >= 0, -x & x < 0, 0) $",
        ] {
            let table = grid(source);
            assert_eq!(table.render_replacement().unwrap(), table.source);
            assert_eq!(table.source_edit().unwrap().replacement, table.source);
            assert!(
                !table
                    .cells
                    .iter()
                    .flatten()
                    .any(|cell| cell.starts_with('$'))
            );
        }
        let cases = grid("$ cases(x & x >= 0, -x & x < 0, 0) $");
        assert_eq!(
            cases.cells,
            vec![vec!["x", "x >= 0"], vec!["-x", "x < 0"], vec!["0", ""]]
        );
    }
    #[test]
    fn formula_edits_preserve_options_comments_and_unicode_ranges() {
        let source = "πé $ mat(\n  delim: \"[\", // fence\n  x, alpha; // row\n  frac(1, 2), beta; augment: #(hline: 1), align: right\n) $ end";
        let mut table = math_table_at(
            source,
            source[..source.find("mat(").unwrap()].chars().count(),
        )
        .unwrap();
        assert_eq!(
            table.source_range.start,
            source[..source.find("mat(").unwrap()].chars().count()
        );
        assert_eq!(table.columns, 2);
        table.cells[1][1] = "beta^2".into();
        let edit = table.source_edit().unwrap();
        assert_eq!(edit.replacement, table.source.replace("beta;", "beta^2;"));
        assert!(edit.replacement.contains("augment: #(hline: 1)"));
        assert!(edit.replacement.contains("// fence"));
        assert!(edit.replacement.contains("// row"));
    }
    #[test]
    fn nested_calls_strings_and_escaped_punctuation_are_not_grid_separators() {
        let source = "$ mat(frac(a, b), mat(1, 2; 3, 4); \"a,b;c\", a\\,b) $";
        let outer = grid(source);
        assert_eq!(outer.row_count(), 2);
        assert_eq!(outer.columns, 2);
        assert_eq!(outer.cells[0][1], "mat(1, 2; 3, 4)");
        let cursor = source.find("3, 4").unwrap();
        let inner = math_table_at(source, cursor).unwrap();
        assert_eq!(inner.source, "mat(1, 2; 3, 4)");
        assert_eq!(outer.render_replacement().unwrap(), outer.source);
        let cases = grid("$ cases(frac(a & b, c) & x > 0, 0 & x <= 0) $");
        assert_eq!(cases.columns, 2);
        assert_eq!(cases.cells[0][0], "frac(a & b, c)");
    }
    #[test]
    fn geometry_changes_keep_options_and_do_not_quote_formulas() {
        let mut table = grid("$ mat(delim: \"[\", x, alpha; 1, frac(1, 2); augment: #1) $");
        table.add_row();
        table.add_column();
        table.cells[2][2] = "sum_(i=1)^n i".into();
        let draft = table.render_replacement().unwrap();
        let reparsed = grid(&format!("$ {draft} $"));
        assert_eq!((reparsed.row_count(), reparsed.columns), (3, 3));
        assert_eq!(reparsed.cells[2][2], "sum_(i=1)^n i");
        assert!(draft.contains("augment: #1"));
        assert!(draft.contains("delim: \"[\""));
        table.remove_column();
        table.remove_row();
        assert_eq!(table.render_replacement().unwrap(), table.source);
        let mut vector = grid("$ vec(x, alpha) $");
        assert!(!vector.can_add_column());
        vector.add_column();
        assert_eq!(vector.columns, 1);
        vector.add_row();
        assert_eq!(
            grid(&format!("$ {} $", vector.render_replacement().unwrap())).row_count(),
            3
        );
    }
    #[test]
    fn delimiter_edits_use_math_syntax_and_preserve_other_named_options() {
        let mut table = grid("$ mat(x, alpha; 1, 2; align: right) $");
        assert_eq!(table.delimiter(), None);
        table.set_delimiter("[").unwrap();
        assert_eq!(table.delimiter().as_deref(), Some("["));
        assert!(table.render_replacement().unwrap().contains("delim: \"[\""));
        table.set_delimiter("‖").unwrap();
        assert!(table.render_replacement().unwrap().contains("delim: \"‖\""));
        assert_eq!(
            table.set_delimiter("||"),
            Err(MathTableError::InvalidDelimiter)
        );
        table.set_delimiter("none").unwrap();
        assert!(table.render_replacement().unwrap().contains("delim: #none"));
        table.set_delimiter("").unwrap();
        let draft = table.render_replacement().unwrap();
        assert!(!draft.contains("delim:"));
        assert!(draft.contains("align: right"));
        assert_eq!(
            table.set_delimiter("invalid"),
            Err(MathTableError::InvalidDelimiter)
        );
    }
    #[test]
    fn rejects_dynamic_geometry_markup_strings_spreads_and_invalid_drafts() {
        for source in [
            "mat(1,2;3,4)",
            "`$ mat(1,2;3,4) $`",
            "// $ mat(1,2;3,4) $",
            "#math.mat((1,2),(3,4))",
            "$ mat(..#data) $",
            "$ mat(1,2;3) $",
            "$ mat(1,2) $",
            "$ vec(1;2) $",
        ] {
            assert!(
                math_table_at(
                    source,
                    source.find("mat").or_else(|| source.find("vec")).unwrap()
                )
                .is_none(),
                "{source}"
            );
        }
        for injection in [
            "x, y",
            "x; y",
            "$x$",
            "x), vec(y",
            "x, delim: \"[\"",
            "frac(x,",
        ] {
            let mut table = grid("$ mat(1,2;3,4) $");
            table.cells[0][0] = injection.into();
            assert_eq!(
                table.render_replacement(),
                Err(MathTableError::InvalidMath),
                "{injection}"
            );
        }
        let mut table = grid("$ cases(x & x > 0, 0 & x <= 0) $");
        table.cells[0][0] = "x & y".into();
        assert_eq!(table.render_replacement(), Err(MathTableError::InvalidMath));
    }
    #[test]
    fn shrinking_validates_preserved_augmentation_without_evaluating_user_code() {
        for augmentation in [
            "#1",
            "#(-1)",
            "#none",
            "#(hline: (0, 1), vline: -1, stroke: 1pt)",
        ] {
            let mut table = grid(&format!("$ mat(1,2;3,4; augment: {augmentation}) $"));
            table.remove_row();
            table.remove_column();
            assert!(table.render_replacement().is_ok(), "{augmentation}");
        }
        for augmentation in [
            "#2",
            "#(-2)",
            "#offset",
            "#(hline: 2)",
            "#(vline: (1,2))",
            "#(..options)",
        ] {
            let mut table = grid(&format!("$ mat(1,2;3,4; augment: {augmentation}) $"));
            assert!(
                table.render_replacement().is_ok(),
                "unchanged {augmentation}"
            );
            table.add_row();
            assert!(table.render_replacement().is_ok(), "growth {augmentation}");
            table.remove_row();
            table.remove_row();
            table.remove_column();
            assert_eq!(
                table.render_replacement(),
                Err(MathTableError::InvalidAugmentation),
                "{augmentation}"
            );
        }
    }
    #[test]
    fn geometry_is_bounded_and_comments_survive_reconstruction() {
        let mut table = grid("$ mat(1,2;3,4) $");
        table.columns = 129;
        assert_eq!(table.render_replacement(), Err(MathTableError::TooLarge));
        let mut oversized = grid("$ vec(x) $");
        oversized.cells[0][0] = "x".repeat(MAX_SOURCE_BYTES + 1);
        assert_eq!(
            oversized.render_replacement(),
            Err(MathTableError::TooLarge)
        );
        let mut cases = grid("$ cases(x & x > 0, 0) $");
        cases.cells[1][1] = "x <= 0".into();
        let draft = cases.render_replacement().unwrap();
        assert_eq!(grid(&format!("$ {draft} $")).cells, cases.cells);
        let mut table = grid("$ mat(1, 2; // preserve this\n3, 4; // final\n) $");
        table.add_row();
        let draft = table.render_replacement().unwrap();
        assert!(draft.contains("// preserve this"));
        assert!(draft.contains("// final"));
    }
}
