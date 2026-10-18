//! Bounded, lossless TeX table editing. Only tables with a known grid are
//! exposed to the editor; unsupported structural expansion is rejected.
use super::{SourceEdit, byte_range_to_char, char_to_byte};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TexTableEnvironment {
    Tabular,
    TabularStar,
    TabularX,
    Tabulary,
    LongTable,
    XlTabular,
    NiceTabular,
    NiceTabularStar,
    Tblr,
    LongTblr,
    TallTblr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnDialect {
    Array,
    Tabulary,
    Tabularray,
}

impl TexTableEnvironment {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Tabular => "tabular",
            Self::TabularStar => "tabular*",
            Self::TabularX => "tabularx",
            Self::Tabulary => "tabulary",
            Self::LongTable => "longtable",
            Self::XlTabular => "xltabular",
            Self::NiceTabular => "NiceTabular",
            Self::NiceTabularStar => "NiceTabular*",
            Self::Tblr => "tblr",
            Self::LongTblr => "longtblr",
            Self::TallTblr => "talltblr",
        }
    }
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "tabular" => Some(Self::Tabular),
            "tabular*" => Some(Self::TabularStar),
            "tabularx" => Some(Self::TabularX),
            "tabulary" => Some(Self::Tabulary),
            "longtable" => Some(Self::LongTable),
            "xltabular" => Some(Self::XlTabular),
            "NiceTabular" => Some(Self::NiceTabular),
            "NiceTabular*" => Some(Self::NiceTabularStar),
            "tblr" => Some(Self::Tblr),
            "longtblr" => Some(Self::LongTblr),
            "talltblr" => Some(Self::TallTblr),
            _ => None,
        }
    }
    fn dialect(self) -> ColumnDialect {
        match self {
            Self::Tabulary => ColumnDialect::Tabulary,
            Self::Tblr | Self::LongTblr | Self::TallTblr => ColumnDialect::Tabularray,
            _ => ColumnDialect::Array,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TexTable {
    pub(crate) source_range: std::ops::Range<usize>,
    pub(crate) source: String,
    pub(crate) environment: TexTableEnvironment,
    pub(crate) packages: Vec<String>,
    pub(crate) column_spec: String,
    pub(crate) cells: Option<Vec<Vec<String>>>,
    header: String,
    spec_range: std::ops::Range<usize>,
    footer: String,
    body_range: std::ops::Range<usize>,
    rows: Vec<RowSource>,
    spans: Vec<Vec<(usize, usize)>>,
    owners: Vec<Vec<(usize, usize)>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RowSource {
    cells: Vec<CellSource>,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CellSource {
    range: std::ops::Range<usize>,
    column: usize,
    columns: usize,
    requested_rows: usize,
    content_prefix: String,
    content_suffix: String,
}

impl TexTable {
    pub(crate) fn columns(&self) -> Option<usize> {
        column_count(&self.column_spec, self.environment.dialect(), 0)
    }
    /// Logical (rowspan, colspan), matching sampled table geometry. Covered
    /// coordinates have no independent cell and return (0, 0).
    pub(crate) fn cell_span(&self, (row, column): (usize, usize)) -> (usize, usize) {
        self.spans
            .get(row)
            .and_then(|spans| spans.get(column))
            .copied()
            .unwrap_or((0, 0))
    }
    /// Constant-time owner lookup for virtualized rows intersecting a merge.
    pub(crate) fn cell_owner(&self, position: (usize, usize)) -> (usize, usize) {
        self.owners
            .get(position.0)
            .and_then(|row| row.get(position.1))
            .copied()
            .unwrap_or(position)
    }
    pub(crate) fn cell_is_covered(&self, position: (usize, usize)) -> bool {
        self.cell_span(position) == (0, 0)
    }
    /// The editable body between the original cell's retained wrappers. This
    /// remains stable while the user types incomplete TeX inside that body;
    /// sampling can also wrap it without moving alignment commands into groups.
    pub(crate) fn cell_content_range(
        &self,
        (row, column): (usize, usize),
    ) -> Option<std::ops::Range<usize>> {
        if self.cell_is_covered((row, column)) {
            return None;
        }
        let cell = self.cells.as_ref()?.get(row)?.get(column)?;
        let Some(source) = self
            .rows
            .get(row)?
            .cells
            .iter()
            .find(|cell| cell.column == column)
        else {
            // TeX allows trailing empty cells to be implicit in the source.
            return Some(0..cell.len());
        };
        let start = source.content_prefix.len();
        let end = cell.len().checked_sub(source.content_suffix.len())?;
        (start <= end
            && cell.starts_with(&source.content_prefix)
            && cell.ends_with(&source.content_suffix))
        .then_some(start..end)
    }
    pub(crate) fn set_column_spec(&mut self, spec: &str) -> Result<(), String> {
        let columns = column_count(spec, self.environment.dialect(), 0)
            .ok_or("Enter a supported column specification for this table")?;
        if self
            .cells
            .as_ref()
            .is_some_and(|rows| rows.iter().any(|row| row.len() != columns))
        {
            return Err(
                "Keep one column in the specification for every existing cell in a row".into(),
            );
        }
        let mut header = self.header.clone();
        header.replace_range(self.spec_range.clone(), spec);
        // Validate the complete header so a brace or alignment injection cannot
        // write outside the column specification.
        let wrapper = format!("{header}{}", self.footer);
        let parsed = tex_table_at(&wrapper, 1)
            .ok_or("Finish the column specification before applying it")?;
        if parsed.column_spec != spec || parsed.source_range != (0..wrapper.chars().count()) {
            return Err("Enter one column specification without surrounding braces".into());
        }
        self.header = header;
        self.spec_range.end = self.spec_range.start + spec.len();
        self.column_spec = spec.to_owned();
        Ok(())
    }

    pub(crate) fn source_edit(&self, draft: &str) -> Result<SourceEdit, String> {
        if !balanced_braces(draft) {
            return Err("Balance the table's braces before applying it".into());
        }
        let Some(parsed) = tex_table_at(draft, 1) else {
            return Err("Enter one complete supported TeX table environment".into());
        };
        if parsed.environment != self.environment
            || parsed.source_range != (0..draft.chars().count())
        {
            return Err(format!(
                "Keep one complete {} environment, without surrounding text",
                self.environment.name()
            ));
        }
        Ok(SourceEdit {
            range: self.source_range.clone(),
            replacement: draft.to_owned(),
        })
    }
    /// Serialize edited owner cells without normalizing surrounding TeX.
    /// Apply separately validates the resulting table geometry.
    pub(crate) fn render_source_draft(&self) -> Result<String, String> {
        let cells = self
            .cells
            .as_ref()
            .ok_or("The table has no editable grid")?;
        let columns = self
            .columns()
            .ok_or("Enter a supported column specification")?;
        if cells.len() != self.rows.len() || cells.len().saturating_mul(columns) > 4096 {
            return Err("Keep the table's row and column structure".into());
        }
        let body = &self.source[self.body_range.clone()];
        let mut replacement = self.header.clone();
        let mut copied = 0;
        for (r, (cells, row)) in cells.iter().zip(&self.rows).enumerate() {
            if cells.len() != columns {
                return Err(format!("Row {} must have {columns} cells", r + 1));
            }
            for (column, cell) in cells.iter().enumerate() {
                if self.cell_is_covered((r, column)) && !cell.is_empty() {
                    return Err("A covered position belongs to its merged cell".into());
                }
            }
            for source in &row.cells {
                let cell = &cells[source.column];
                if *cell != cell_source(&body[source.range.clone()]) {
                    replacement.push_str(&body[copied..source.range.start]);
                    replacement.push_str(cell);
                    copied = source.range.end;
                }
            }
            // TeX permits the last empty cells to be omitted. Keep that exact
            // spelling unless one of those implicit cells has actually changed.
            let explicit_columns = row
                .cells
                .last()
                .map_or(0, |cell| cell.column + cell.columns);
            if let Some(last) = cells[explicit_columns..]
                .iter()
                .rposition(|cell| !cell.is_empty())
            {
                replacement.push_str(&body[copied..row.end]);
                for cell in &cells[explicit_columns..=explicit_columns + last] {
                    replacement.push_str(" & ");
                    replacement.push_str(cell);
                }
                copied = row.end;
            }
        }
        replacement.push_str(&body[copied..]);
        replacement.push_str(&self.footer);
        Ok(replacement)
    }
    pub(crate) fn render_replacement(&self) -> Result<String, String> {
        let replacement = self.render_source_draft()?;
        let Some(cells) = &self.cells else {
            return Ok(replacement);
        };
        // A cell edit must not inject a row delimiter or escape the environment.
        let candidate =
            tex_table_at(&replacement, 1).ok_or("Finish the table source before applying it")?;
        if candidate.cells.as_ref() != Some(cells)
            || candidate.spans != self.spans
            || candidate.source_range != (0..replacement.chars().count())
        {
            return Err(
                "Keep alignment markers, row breaks and merged-cell geometry outside cell edits"
                    .into(),
            );
        }
        Ok(replacement)
    }
    pub(crate) fn grid_source_edit(&self) -> Result<SourceEdit, String> {
        let replacement = self.render_replacement()?;
        self.source_edit(&replacement)
    }
}

pub(crate) fn tex_table_at(source: &str, char_cursor: usize) -> Option<TexTable> {
    let cursor = char_to_byte(source, char_cursor)?;
    let mut stack: Vec<(String, usize, usize)> = Vec::new();
    let mut found = None;
    let mut smallest = usize::MAX;
    let mut i = 0;
    while i < source.len() {
        match source.as_bytes()[i] {
            b'%' => {
                i = skip_comment(source, i);
            }
            b'\\' => {
                let start = i;
                let Some((name, end)) = command(source, i) else {
                    // An unfinished command outside a previously matched table
                    // must not hide that valid table while the user is typing.
                    break;
                };
                i = end;
                if name == "verb" {
                    i = skip_verb(source, i);
                    continue;
                }
                if name != "begin" && name != "end" {
                    continue;
                }
                let Some((env, end)) = group(source, i, b'{', b'}') else {
                    continue;
                };
                i = end;
                if name == "begin" {
                    if matches!(env, "verbatim" | "verbatim*" | "lstlisting" | "minted") {
                        let closing = format!("\\end{{{env}}}");
                        i = source[end..]
                            .find(&closing)
                            .map_or(source.len(), |offset| end + offset + closing.len());
                        continue;
                    }
                    stack.push((env.to_owned(), start, end));
                } else {
                    let Some((opening, begin, args)) = stack.pop() else {
                        continue;
                    };
                    if opening != env {
                        stack.clear();
                        continue;
                    }
                    if begin <= cursor
                        && cursor < end
                        && let Some(environment) = TexTableEnvironment::from_name(env)
                    {
                        // Inner matched tables are encountered first. Keep
                        // the smallest containing environment, even if its
                        // unsupported geometry prevents opening an editor.
                        if end - begin < smallest {
                            smallest = end - begin;
                            found = parse_table(source, begin, args, start, end, environment);
                        }
                    }
                }
            }
            _ => i = next_char(source, i),
        }
    }
    found
}

fn parse_table(
    source: &str,
    begin: usize,
    mut args: usize,
    close: usize,
    end: usize,
    environment: TexTableEnvironment,
) -> Option<TexTable> {
    // xltabular's position is before its width. The other width-taking
    // environments put position after width. NiceTabular additionally accepts
    // package options after the column specification.
    if environment == TexTableEnvironment::XlTabular
        && let Some((_, next)) = group(source, args, b'[', b']')
    {
        args = next;
    }
    if matches!(
        environment,
        TexTableEnvironment::TabularStar
            | TexTableEnvironment::TabularX
            | TexTableEnvironment::Tabulary
            | TexTableEnvironment::XlTabular
            | TexTableEnvironment::NiceTabularStar
    ) {
        let (_, next) = group(source, args, b'{', b'}')?;
        args = next;
    }
    let mut outer_simple = true;
    if let Some((options, next)) = group(source, args, b'[', b']') {
        outer_simple = simple_package_options(options, environment);
        args = next;
    }
    let (inner, mut body_start) = group(source, args, b'{', b'}')?;
    let dialect = environment.dialect();
    let (spec, simple_layout) = if dialect == ColumnDialect::Tabularray {
        tabularray_spec(inner)
    } else {
        (inner, true)
    };
    let spec_start = spec.as_ptr() as usize - source.as_ptr() as usize;
    if matches!(
        environment,
        TexTableEnvironment::NiceTabular | TexTableEnvironment::NiceTabularStar
    ) && let Some((options, next)) = group(source, body_start, b'[', b']')
    {
        outer_simple &= simple_package_options(options, environment);
        body_start = next;
    }
    let columns = column_count(spec, dialect, 0);
    let body = source.get(body_start..close)?;
    let parsed = (simple_layout && outer_simple)
        .then_some(columns)
        .flatten()
        .and_then(|columns| split_rows(body, columns));
    let (cells, rows) = parsed?;
    let spans = cell_spans(&cells, &rows, columns?);
    let owners = cell_owners(&spans);
    Some(TexTable {
        source_range: byte_range_to_char(source, begin..end),
        source: source.get(begin..end)?.to_owned(),
        environment,
        packages: packages(source),
        column_spec: spec.to_owned(),
        cells: Some(cells),
        header: source.get(begin..body_start)?.to_owned(),
        spec_range: (spec_start - begin)..(spec_start - begin + spec.len()),
        footer: source.get(close..end)?.to_owned(),
        body_range: (body_start - begin)..(close - begin),
        rows,
        spans,
        owners,
    })
}

/// Key/value specifications can contain nested commas, comments and escaped
/// punctuation. Return slices, so only the actual colspec value is rewritten.
/// Missing/duplicate colspec or structural cell options stay in exact source.
fn tabularray_spec(inner: &str) -> (&str, bool) {
    if column_count(inner, ColumnDialect::Tabularray, 0).is_some() {
        return (inner, true);
    }
    let Some(entries) = top_level_parts(inner, b',') else {
        return (inner, false);
    };
    let mut spec = None;
    let mut simple = true;
    for entry in entries {
        let Some(parts) = top_level_parts(entry, b'=') else {
            return (inner, false);
        };
        let key = parts[0][skip_trivia(parts[0], 0)..].trim();
        if key == "colspec" && parts.len() == 2 {
            if spec.is_some() {
                return (inner, false);
            }
            let value = parts[1][skip_trivia(parts[1], 0)..].trim();
            spec = Some(if let Some((inside, end)) = group(value, 0, b'{', b'}') {
                if skip_trivia(value, end) != value.len() {
                    return (inner, false);
                }
                inside
            } else {
                value
            });
        }
        // Cell keys may change row/column spans. Never pretend those covered
        // positions are independently editable cells.
        if key.starts_with("cell") || key == "expand" {
            simple = false;
        }
    }
    spec.map_or((inner, false), |spec| (spec, simple))
}
fn simple_package_options(options: &str, environment: TexTableEnvironment) -> bool {
    if environment.dialect() != ColumnDialect::Tabularray
        && !matches!(
            environment,
            TexTableEnvironment::NiceTabular | TexTableEnvironment::NiceTabularStar
        )
    {
        return true;
    }
    let Some(entries) = top_level_parts(options, b',') else {
        return false;
    };
    entries.into_iter().all(|entry| {
        let Some(parts) = top_level_parts(entry, b'=') else {
            return false;
        };
        let key = parts[0][skip_trivia(parts[0], 0)..].trim();
        !matches!(
            key,
            "expand"
                | "expand+"
                | "create-blocks-in-col"
                | "light-syntax"
                | "light-syntax-expanded"
        )
    })
}
fn top_level_parts(source: &str, separator: u8) -> Option<Vec<&str>> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < source.len() {
        match source.as_bytes()[i] {
            b'%' => i = skip_comment(source, i),
            b'\\' => {
                let (name, end) = command(source, i)?;
                i = if name == "verb" {
                    skip_verb(source, end)
                } else {
                    end
                };
            }
            open @ (b'{' | b'[' | b'(') => {
                let close = match open {
                    b'{' => b'}',
                    b'[' => b']',
                    _ => b')',
                };
                let (_, end) = group(source, i, open, close)?;
                i = end;
            }
            b'}' | b']' | b')' => return None,
            byte if byte == separator => {
                if result.len() >= 4096 {
                    return None;
                }
                result.push(&source[start..i]);
                i += 1;
                start = i;
            }
            _ => i = next_char(source, i),
        }
    }
    result.push(&source[start..]);
    Some(result)
}

type ParsedRows = (Vec<Vec<String>>, Vec<RowSource>);
fn split_rows(body: &str, columns: usize) -> Option<ParsedRows> {
    let mut rows = Vec::new();
    let mut sources = Vec::new();
    let mut row = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut depth = 0usize;
    let mut math = false;
    while i < body.len() {
        match body.as_bytes()[i] {
            b'%' => i = skip_comment(body, i),
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth = depth.checked_sub(1)?;
                i += 1;
            }
            b'$' => {
                math = !math;
                i += 1;
            }
            b'&' if depth == 0 && !math => {
                row.push(start..i);
                // This delimiter requires another cell. Reject an overwide
                // row now instead of allocating every remaining source segment.
                if row.len() >= columns {
                    return None;
                }
                i += 1;
                start = i;
            }
            b'\\' => {
                let (name, end) = command(body, i)?;
                if matches!(
                    name,
                    "input"
                        | "include"
                        | "Block"
                        | "SetCell"
                        | "SetCells"
                        | "CodeBefore"
                        | "CodeAfter"
                        | "caption"
                        | "cr"
                        | "crcr"
                        | "omit"
                        | "span"
                        | "multispan"
                        | "halign"
                        | "valign"
                        | "ialign"
                        | "noalign"
                        | "loop"
                        | "repeat"
                        | "foreach"
                        | "@for"
                        | "@tfor"
                ) {
                    return None;
                }
                if name == "verb" {
                    i = skip_verb(body, end);
                    continue;
                }
                if depth == 0 && name == "begin" {
                    return None;
                }
                if depth == 0 && !math && matches!(name, "\\" | "tabularnewline") {
                    row.push(start..i);
                    if (rows.len() + 1).saturating_mul(columns) > 4096 {
                        return None;
                    }
                    let (cells, source) = row_source(body, std::mem::take(&mut row), i, columns)?;
                    rows.push(cells);
                    sources.push(source);
                    let mut ending = end;
                    if body.as_bytes().get(ending) == Some(&b'*') {
                        ending += 1;
                    }
                    if let Some((_, next)) = group(body, ending, b'[', b']') {
                        ending = next;
                    }
                    // Keep the control-word delimiter's terminating trivia in
                    // the source rather than moving it into the next cell.
                    i = skip_trivia(body, ending);
                    start = i;
                } else {
                    i = end;
                }
            }
            _ => i = next_char(body, i),
        }
    }
    if depth != 0 || math {
        return None;
    }
    let rest = body.get(start..)?;
    let (_, first) = row_prefix(rest)?;
    if !row.is_empty() || !first.trim().is_empty() {
        if (rows.len() + 1).saturating_mul(columns) > 4096 {
            return None;
        }
        row.push(start..body.len());
        let (cells, source) = row_source(body, row, body.len(), columns)?;
        rows.push(cells);
        sources.push(source);
    }
    Some((rows, sources))
}

fn row_source(
    body: &str,
    ranges: Vec<std::ops::Range<usize>>,
    end: usize,
    columns: usize,
) -> Option<(Vec<String>, RowSource)> {
    if ranges.len() > columns {
        return None;
    }
    let mut cells = vec![String::new(); columns];
    let mut sources = Vec::with_capacity(ranges.len());
    let mut column = 0usize;
    for (index, mut range) in ranges.into_iter().enumerate() {
        let mut source = &body[range.clone()];
        if index == 0 {
            source = row_prefix(source)?.1;
            range.start = range.end - source.len();
        }
        range.start += source.len() - source.trim_start().len();
        range.end = range.start + source.trim().len();
        let value = cell_source(&body[range.clone()]);
        let (requested_rows, cell_columns, content) = cell_layout(&value, 0)?;
        if column.checked_add(cell_columns)? > columns {
            return None;
        }
        let content_prefix = value[..content.start].to_owned();
        let content_suffix = value[content.end..].to_owned();
        cells[column] = value;
        sources.push(CellSource {
            range,
            column,
            columns: cell_columns,
            requested_rows,
            content_prefix,
            content_suffix,
        });
        column += cell_columns;
    }
    Some((
        cells,
        RowSource {
            cells: sources,
            end,
        },
    ))
}

/// TeX multirow is also used for vertical positioning, with occupied cells in
/// subsequent source rows. Only merge genuinely empty positions; preserving the
/// original wrapper must never hide later data from the editable grid.
fn cell_spans(
    cells: &[Vec<String>],
    rows: &[RowSource],
    columns: usize,
) -> Vec<Vec<(usize, usize)>> {
    let mut spans = vec![vec![(1, 1); columns]; cells.len()];
    for (row, source) in rows.iter().enumerate() {
        for cell in &source.cells {
            spans[row][cell.column] = (1, cell.columns);
            spans[row][cell.column + 1..cell.column + cell.columns].fill((0, 0));
        }
    }
    for (row, source) in rows.iter().enumerate() {
        for cell in &source.cells {
            let end = row.saturating_add(cell.requested_rows);
            if cell.requested_rows <= 1 || end > rows.len() {
                continue;
            }
            let columns = cell.column..cell.column + cell.columns;
            if (row + 1..end).all(|r| {
                columns
                    .clone()
                    .all(|c| cells[r][c].is_empty() && spans[r][c] == (1, 1))
            }) {
                spans[row][cell.column] = (cell.requested_rows, cell.columns);
                for row_spans in &mut spans[row + 1..end] {
                    for column in columns.clone() {
                        row_spans[column] = (0, 0);
                    }
                }
            }
        }
    }
    spans
}

fn cell_owners(spans: &[Vec<(usize, usize)>]) -> Vec<Vec<(usize, usize)>> {
    let mut owners: Vec<Vec<_>> = spans
        .iter()
        .enumerate()
        .map(|(row, cells)| (0..cells.len()).map(|column| (row, column)).collect())
        .collect();
    for (row, cells) in spans.iter().enumerate() {
        for (column, &(rows, columns)) in cells.iter().enumerate() {
            for owner_row in &mut owners[row..row + rows] {
                owner_row[column..column + columns].fill((row, column));
            }
        }
    }
    owners
}

/// Interpret only explicit bounded span wrappers. Other formatting remains
/// opaque cell text. Full source ranges retain all wrapper arguments verbatim.
fn cell_layout(source: &str, depth: usize) -> Option<(usize, usize, std::ops::Range<usize>)> {
    if depth > 16 {
        return None;
    }
    let start = skip_trivia(source, 0);
    if let Some((inside, end)) = group(source, start, b'{', b'}')
        && skip_trivia(source, end) == source.len()
    {
        return inner_cell_layout(source, inside, depth + 1);
    }
    let Some((name, mut end)) = command(source, start) else {
        return Some((1, 1, 0..source.len()));
    };
    match name {
        "multicolumn" => {
            let (count, next) = group(source, end, b'{', b'}')?;
            let columns = count
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=128).contains(n))?;
            let (spec, next) = group(source, next, b'{', b'}')?;
            if column_count(spec, ColumnDialect::Array, 0) != Some(1) {
                return None;
            }
            let (inside, next) = group(source, next, b'{', b'}')?;
            if skip_trivia(source, next) != source.len() {
                return None;
            }
            let (rows, nested_columns, content) = inner_cell_layout(source, inside, depth + 1)?;
            (nested_columns == 1).then_some((rows, columns, content))
        }
        "multirow" => {
            if let Some((_, next)) = group(source, end, b'[', b']') {
                end = next;
            }
            let (count, next) = group(source, end, b'{', b'}')?;
            // Negative/fractional counts are vertical positioning directives;
            // they do not consume physical source rows in the editable model.
            let count = count.trim().parse::<f64>().ok()?;
            if !count.is_finite() || count == 0.0 || count.abs() > 4096.0 {
                return None;
            }
            let rows = if count > 0.0 && count.fract() == 0.0 {
                count as usize
            } else {
                1
            };
            end = next;
            if let Some((_, next)) = group(source, end, b'[', b']') {
                end = next;
            }
            let (_, next) = group(source, end, b'{', b'}')?;
            end = next;
            if let Some((_, next)) = group(source, end, b'[', b']') {
                end = next;
            }
            let (inside, next) = group(source, end, b'{', b'}')?;
            if skip_trivia(source, next) != source.len() {
                return None;
            }
            let (_, columns, content) = inner_cell_layout(source, inside, depth + 1)?;
            Some((rows, columns, content))
        }
        "tiny" | "scriptsize" | "footnotesize" | "small" | "normalsize" | "large" | "Large"
        | "LARGE" | "huge" | "Huge" | "bfseries" | "mdseries" | "itshape" | "slshape"
        | "scshape" | "upshape" | "rmfamily" | "sffamily" | "ttfamily" | "normalfont" => {
            let start = skip_trivia(source, end);
            let (rows, columns, content) = inner_cell_layout(source, &source[start..], depth + 1)?;
            // Preserve a delimiter after control words. Replacing the `1` in
            // `\small1` with letters must not produce a new command name.
            if start < source.len() && content.start > end {
                Some((rows, columns, content))
            } else {
                Some((1, 1, 0..source.len()))
            }
        }
        "vspace" | "hspace" => {
            if source.as_bytes().get(end) == Some(&b'*') {
                end += 1;
            }
            if let Some((dimension, next)) = group(source, end, b'{', b'}')
                && static_spacing_dimension(dimension)
            {
                let start = skip_trivia(source, next);
                return inner_cell_layout(source, &source[start..], depth + 1);
            }
            Some((1, 1, 0..source.len()))
        }
        "thead" | "makecell" | "textbf" | "textit" | "textrm" | "textsf" | "texttt" | "emph"
        | "underline" => {
            if let Some((_, next)) = group(source, end, b'[', b']') {
                end = next;
            }
            if let Some((inside, next)) = group(source, end, b'{', b'}')
                && skip_trivia(source, next) == source.len()
            {
                let (rows, columns, content) = inner_cell_layout(source, inside, depth + 1)?;
                // A multicolumn inside ordinary text formatting is not a legal
                // alignment primitive. Do not invent covered coordinates.
                return (columns == 1).then_some((rows, columns, content));
            }
            Some((1, 1, 0..source.len()))
        }
        _ => Some((1, 1, 0..source.len())),
    }
}

/// A literal spacing length is safe to retain outside the editable body.
/// Expressions and user macros remain visible, without guessing their meaning.
fn static_spacing_dimension(source: &str) -> bool {
    let source = source.trim();
    let number = source.strip_prefix(['+', '-']).unwrap_or(source);
    let end = number
        .bytes()
        .take_while(|byte| byte.is_ascii_digit() || *byte == b'.')
        .count();
    let digits = &number[..end];
    !digits.is_empty()
        && digits.bytes().any(|byte| byte.is_ascii_digit())
        && digits.bytes().filter(|byte| *byte == b'.').count() <= 1
        && matches!(
            number[end..].trim(),
            "pt" | "pc" | "in" | "bp" | "cm" | "mm" | "dd" | "cc" | "sp" | "em" | "ex"
        )
}

fn inner_cell_layout(
    source: &str,
    inside: &str,
    depth: usize,
) -> Option<(usize, usize, std::ops::Range<usize>)> {
    let offset = inside.as_ptr() as usize - source.as_ptr() as usize;
    let (rows, columns, content) = cell_layout(inside, depth)?;
    Some((rows, columns, offset + content.start..offset + content.end))
}

/// A comment at a cell's end still needs its newline: otherwise a following
/// alignment delimiter or row break becomes part of the comment when rendered.
fn cell_source(source: &str) -> String {
    let trimmed = source.trim();
    let mut i = 0;
    while i < trimmed.len() {
        match trimmed.as_bytes()[i] {
            b'%' => {
                if !trimmed[i..].contains('\n') {
                    return format!("{trimmed}\n");
                }
                i = skip_comment(trimmed, i);
            }
            b'\\' => {
                let Some((name, end)) = command(trimmed, i) else {
                    break;
                };
                i = if name == "verb" {
                    skip_verb(trimmed, end)
                } else {
                    end
                };
            }
            _ => i = next_char(trimmed, i),
        }
    }
    trimmed.to_owned()
}

fn row_prefix(cell: &str) -> Option<(&str, &str)> {
    let mut i = skip_trivia(cell, 0);
    loop {
        if cell.as_bytes().get(i) != Some(&b'\\') {
            break;
        }
        let (name, end) = command(cell, i)?;
        if !matches!(
            name,
            "hline"
                | "cline"
                | "toprule"
                | "midrule"
                | "bottomrule"
                | "cmidrule"
                | "addlinespace"
                | "endhead"
                | "endfirsthead"
                | "endfoot"
                | "endlastfoot"
                | "rowcolor"
                | "hdashline"
                | "cdashline"
                | "specialrule"
                | "arrayrulecolor"
        ) {
            break;
        }
        i = end;
        if name == "cmidrule"
            && let Some((_, next)) = group(cell, i, b'(', b')')
        {
            i = next;
        }
        if let Some((_, next)) = group(cell, i, b'[', b']') {
            i = next;
        }
        if matches!(
            name,
            "cline" | "cmidrule" | "cdashline" | "rowcolor" | "arrayrulecolor"
        ) {
            let (_, next) = group(cell, i, b'{', b'}')?;
            i = next;
        }
        if name == "specialrule" {
            for _ in 0..3 {
                let (_, next) = group(cell, i, b'{', b'}')?;
                i = next;
            }
        }
        // colortbl overhangs and arydshln dash patterns follow the mandatory group.
        if matches!(name, "rowcolor" | "cdashline") {
            for _ in 0..2 {
                if let Some((_, next)) = group(cell, i, b'[', b']') {
                    i = next;
                }
            }
        }
        i = skip_trivia(cell, i);
    }
    Some((cell.get(..i)?, cell.get(i..)?))
}

fn column_count(spec: &str, dialect: ColumnDialect, nesting: usize) -> Option<usize> {
    if nesting > 16 {
        return None;
    }
    let mut count = 0usize;
    let mut i = 0;
    while i < spec.len() {
        i = skip_trivia(spec, i);
        if i == spec.len() {
            break;
        }
        match spec.as_bytes()[i] {
            b'l' | b'c' | b'r' | b'X' | b'S' => {
                count += 1;
                i += 1;
                if (matches!(spec.as_bytes()[i - 1], b'S') || dialect == ColumnDialect::Tabularray)
                    && let Some((_, end)) = group(spec, i, b'[', b']')
                {
                    i = end;
                }
            }
            b'Q' if dialect == ColumnDialect::Tabularray => {
                count += 1;
                i += 1;
                if let Some((_, end)) = group(spec, i, b'[', b']') {
                    i = end;
                }
            }
            b'L' | b'C' | b'R' | b'J' if dialect == ColumnDialect::Tabulary => {
                count += 1;
                i += 1;
            }
            kind @ (b'D' | b'w' | b'W') => {
                count += 1;
                i += 1;
                for _ in 0..if kind == b'D' { 3 } else { 2 } {
                    let (_, end) = group(spec, i, b'{', b'}')?;
                    i = end;
                }
            }
            b'p' | b'm' | b'b' => {
                count += 1;
                let (_, end) = group(spec, i + 1, b'{', b'}')?;
                i = end;
            }
            b'|' | b':' => {
                i += 1;
                if dialect == ColumnDialect::Tabularray
                    && let Some((_, end)) = group(spec, i, b'[', b']')
                {
                    i = end;
                }
            }
            b'@' | b'!' | b'>' | b'<' => {
                let (_, end) = group(spec, i + 1, b'{', b'}')?;
                i = end;
            }
            b'*' => {
                let (repeat, end) = group(spec, i + 1, b'{', b'}')?;
                let repeat: usize = repeat.trim().parse().ok()?;
                let (inner, end) = group(spec, end, b'{', b'}')?;
                count = count
                    .checked_add(column_count(inner, dialect, nesting + 1)?.checked_mul(repeat)?)?;
                i = end;
            }
            _ => return None,
        }
        if count > 128 {
            return None;
        }
    }
    (count > 0).then_some(count)
}

fn packages(source: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < source.len() {
        match source.as_bytes()[i] {
            b'%' => i = skip_comment(source, i),
            b'\\' => {
                let Some((name, mut end)) = command(source, i) else {
                    break;
                };
                i = end;
                if name == "verb" {
                    i = skip_verb(source, i);
                    continue;
                }
                if !matches!(name, "usepackage" | "RequirePackage") {
                    continue;
                }
                if let Some((_, next)) = group(source, end, b'[', b']') {
                    end = next;
                }
                if let Some((names, next)) = group(source, end, b'{', b'}') {
                    for name in names
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                    {
                        if !result.iter().any(|old| old == name) {
                            result.push(name.to_owned());
                        }
                    }
                    i = next;
                }
            }
            _ => i = next_char(source, i),
        }
    }
    result
}

fn balanced_braces(source: &str) -> bool {
    let mut depth = 0usize;
    let mut i = 0;
    while i < source.len() {
        match source.as_bytes()[i] {
            b'%' => i = skip_comment(source, i),
            b'\\' => {
                let Some((name, end)) = command(source, i) else {
                    return false;
                };
                i = if name == "verb" {
                    skip_verb(source, end)
                } else {
                    end
                };
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                let Some(next) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next;
                i += 1;
            }
            _ => i = next_char(source, i),
        }
    }
    depth == 0
}

fn command(source: &str, start: usize) -> Option<(&str, usize)> {
    if source.as_bytes().get(start) != Some(&b'\\') {
        return None;
    }
    let mut end = start + 1;
    let first = *source.as_bytes().get(end)?;
    if first.is_ascii_alphabetic() || first == b'@' {
        while source
            .as_bytes()
            .get(end)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'@')
        {
            end += 1;
        }
    } else {
        end = next_char(source, end);
    }
    Some((source.get(start + 1..end)?, end))
}
fn group(source: &str, start: usize, open: u8, close: u8) -> Option<(&str, usize)> {
    let start = skip_trivia(source, start);
    if source.as_bytes().get(start) != Some(&open) {
        return None;
    }
    let mut i = start + 1;
    let mut depth = 1usize;
    while i < source.len() {
        match source.as_bytes()[i] {
            b'%' => i = skip_comment(source, i),
            b'\\' => {
                let (name, end) = command(source, i)?;
                i = if name == "verb" {
                    skip_verb(source, end)
                } else {
                    end
                };
            }
            b'{' if open != b'{' => {
                let (_, end) = group(source, i, b'{', b'}')?;
                i = end;
            }
            byte if byte == open => {
                depth += 1;
                i += 1;
            }
            byte if byte == close => {
                depth -= 1;
                if depth == 0 {
                    return Some((source.get(start + 1..i)?, i + 1));
                }
                i += 1;
            }
            _ => i = next_char(source, i),
        }
    }
    None
}
fn skip_comment(source: &str, start: usize) -> usize {
    source[start..]
        .find('\n')
        .map_or(source.len(), |offset| start + offset + 1)
}
fn skip_trivia(source: &str, mut start: usize) -> usize {
    loop {
        while source
            .as_bytes()
            .get(start)
            .is_some_and(u8::is_ascii_whitespace)
        {
            start += 1;
        }
        if source.as_bytes().get(start) != Some(&b'%') {
            return start;
        }
        start = skip_comment(source, start);
    }
}
fn skip_verb(source: &str, mut start: usize) -> usize {
    if source.as_bytes().get(start) == Some(&b'*') {
        start += 1;
    }
    let Some(delimiter) = source.get(start..).and_then(|tail| tail.chars().next()) else {
        return source.len();
    };
    let content_start = start + delimiter.len_utf8();
    source[content_start..]
        .find(delimiter)
        .map_or(source.len(), |offset| {
            content_start + offset + delimiter.len_utf8()
        })
}
fn next_char(source: &str, start: usize) -> usize {
    start + source[start..].chars().next().map_or(0, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arxiv_table(source: &str) -> TexTable {
        let cursor = source[..source.find("\\begin{tabular}").unwrap()]
            .chars()
            .count()
            + 1;
        tex_table_at(source, cursor).unwrap()
    }

    #[test]
    fn arxiv_tables_detect_their_real_grid_and_structural_layouts() {
        for (name, source, rows, columns) in [
            (
                "scipy-roadmap",
                include_str!("../../tests/fixtures/arxiv-tables/scipy-roadmap.tex"),
                16,
                2,
            ),
            (
                "scipy-optimizers",
                include_str!("../../tests/fixtures/arxiv-tables/scipy-optimizers.tex"),
                13,
                14,
            ),
            (
                "platonic-metrics",
                include_str!("../../tests/fixtures/arxiv-tables/platonic-metrics.tex"),
                10,
                6,
            ),
        ] {
            let table = arxiv_table(source);
            let cells = table.cells.as_ref().expect(name);
            assert_eq!(table.environment, TexTableEnvironment::Tabular);
            assert_eq!(table.columns(), Some(columns), "{name}");
            assert_eq!(cells.len(), rows, "{name}");
            assert!(cells.iter().all(|row| row.len() == columns), "{name}");
        }
    }

    #[test]
    fn arxiv_grid_apply_changes_only_the_selected_literal_cell() {
        for (name, source, before, after) in [
            (
                "scipy-roadmap",
                include_str!("../../tests/fixtures/arxiv-tables/scipy-roadmap.tex"),
                "a few more high quality global optimizers",
                "additional global optimizers",
            ),
            (
                "scipy-optimizers",
                include_str!("../../tests/fixtures/arxiv-tables/scipy-optimizers.tex"),
                "Version added",
                "Version introduced",
            ),
            (
                "platonic-metrics",
                include_str!("../../tests/fixtures/arxiv-tables/platonic-metrics.tex"),
                "Unbiased estimator of CKA",
                "Bias-corrected estimator of CKA",
            ),
        ] {
            let mut table = arxiv_table(source);
            assert_eq!(
                table.grid_source_edit().unwrap().replacement,
                table.source,
                "{name}: an unchanged grid must retain the original bytes"
            );
            let original = table.clone();
            let cell = table
                .cells
                .as_mut()
                .expect(name)
                .iter_mut()
                .flatten()
                .find(|cell| cell.contains(before))
                .expect(before);
            *cell = cell.replacen(before, after, 1);
            let edit = table.grid_source_edit().unwrap();
            assert_eq!(edit.range, original.source_range, "{name}");
            assert_eq!(
                edit.replacement,
                original.source.replacen(before, after, 1),
                "{name}"
            );
            let mut applied = source.to_owned();
            let range = char_to_byte(source, edit.range.start).unwrap()
                ..char_to_byte(source, edit.range.end).unwrap();
            applied.replace_range(range, &edit.replacement);
            assert_eq!(applied, source.replacen(before, after, 1), "{name}");
            let reparsed = arxiv_table(&applied);
            assert_eq!(reparsed.cells, table.cells, "{name}");
            assert_eq!(reparsed.column_spec, original.column_spec, "{name}");
            assert_eq!(reparsed.packages, original.packages, "{name}");
            assert_eq!(
                reparsed.render_replacement().unwrap(),
                edit.replacement,
                "{name}"
            );
        }
    }
    #[test]
    fn platonic_merged_headers_preserve_occupied_metric_rows() {
        let source = include_str!("../../tests/fixtures/arxiv-tables/platonic-metrics.tex");
        let table = arxiv_table(source);
        assert_eq!(table.cell_span((0, 0)), (2, 1));
        assert_eq!(table.cell_span((0, 1)), (1, 4));
        assert_eq!(table.cell_span((0, 5)), (2, 1));
        for position in [(0, 2), (0, 3), (0, 4), (1, 0), (1, 5)] {
            assert!(table.cell_is_covered(position), "{position:?}");
        }
        for position in [(0, 2), (0, 3), (0, 4)] {
            assert_eq!(table.cell_owner(position), (0, 1));
        }
        assert_eq!(table.cell_owner((1, 0)), (0, 0));
        assert_eq!(table.cell_owner((1, 5)), (0, 5));
        assert_eq!(table.cell_owner((3, 5)), (3, 5));
        let cells = table.cells.as_ref().unwrap();
        assert_eq!(cells[0][1], r"\multicolumn{4}{c}{\textbf{Property}}");
        assert!(cells[0][5].contains("Description"));
        assert!(cells[1][1].contains("symmetric"));
        // The author's oversized multirows position text vertically, while
        // the following physical rows still contain independent metric data.
        for row in 2..10 {
            for column in 0..6 {
                assert_eq!(table.cell_span((row, column)), (1, 1), "{row},{column}");
            }
        }
        assert!(cells[3][0].contains("Unbiased CKA"));
        assert!(cells[4][0].contains("SVCCA"));
    }

    #[test]
    fn multicolumn_edits_follow_logical_columns_and_keep_implicit_tail_cells() {
        let source = "\\begin{tabular}{ccccc}\n\\multicolumn{2}{|c|}{Héader} & Right \\\\[2pt]\nA & B & C & D & E\\\\\n\\end{tabular}";
        let mut table = tex_table_at(source, 1).unwrap();
        assert_eq!(table.cell_span((0, 0)), (1, 2));
        assert!(table.cell_is_covered((0, 1)));
        assert_eq!(table.cells.as_ref().unwrap()[0][2], "Right");
        assert_eq!(table.render_replacement().unwrap(), source);
        table.cells.as_mut().unwrap()[0][2] = "Changed".into();
        assert_eq!(
            table.render_replacement().unwrap(),
            source.replacen("Right", "Changed", 1)
        );
        table.cells.as_mut().unwrap()[0][4] = "Tail".into();
        let rendered = table.render_replacement().unwrap();
        assert!(
            rendered.contains("Changed  &  & Tail\\\\[2pt]"),
            "{rendered}"
        );
        assert_eq!(tex_table_at(&rendered, 1).unwrap().cells, table.cells);
    }

    #[test]
    fn merged_positions_and_structural_span_edits_cannot_overwrite_neighbors() {
        let source =
            r"\begin{tabular}{ccc}\multicolumn{2}{c}{Header} & Tail\\ A & B & C\\\end{tabular}";
        let original = tex_table_at(source, 1).unwrap();
        let mut covered = original.clone();
        covered.cells.as_mut().unwrap()[0][1] = "Hidden".into();
        assert!(covered.grid_source_edit().is_err());
        let mut changed_span = original;
        changed_span.cells.as_mut().unwrap()[0][0] = r"\multicolumn{1}{c}{Header}".into();
        assert!(changed_span.grid_source_edit().is_err());
        for body in [
            r"\multicolumn{0}{c}{Bad}",
            r"\multicolumn{4}{c}{Bad}",
            r"\multicolumn{\count}{c}{Bad}",
            r"\multicolumn{2}{cc}{Bad}",
            r"\multicolumn{2}{c}{Bad} & A & B",
            r"\multicolumn{2}{c}{\multicolumn{2}{c}{Bad}}",
            r"\multirow{\rows}{*}{Bad} & A & B",
        ] {
            let source = format!(r"\begin{{tabular}}{{ccc}}{body}\end{{tabular}}");
            assert!(tex_table_at(&source, 1).is_none(), "{source}");
        }
    }

    #[test]
    fn multirow_optional_arguments_and_nested_bodies_are_lossless() {
        let source = r"\begin{tabular}{ccc}\multirow[t]{2}[1]{*}[-0.5em]{\textbf{Héader}} & A & B\\ & C & D\\\end{tabular}";
        let mut table = tex_table_at(source, 1).unwrap();
        assert_eq!(table.cell_span((0, 0)), (2, 1));
        assert!(table.cell_is_covered((1, 0)));
        let cell = &table.cells.as_ref().unwrap()[0][0];
        let content = table.cell_content_range((0, 0)).unwrap();
        assert_eq!(&cell[content.clone()], "Héader");
        table.cells.as_mut().unwrap()[0][0].replace_range(content, "Updated");
        assert_eq!(
            table.render_replacement().unwrap(),
            source.replacen("Héader", "Updated", 1)
        );
        for (position, expected) in [((0, 1), "A"), ((1, 1), "C")] {
            let cell = &table.cells.as_ref().unwrap()[position.0][position.1];
            assert_eq!(&cell[table.cell_content_range(position).unwrap()], expected);
        }
        assert!(table.cell_content_range((1, 0)).is_none());
        assert!(table.cell_content_range((99, 0)).is_none());
    }

    #[test]
    fn platonic_cell_projection_exposes_content_and_keeps_every_wrapper_on_edit() {
        let source = include_str!("../../tests/fixtures/arxiv-tables/platonic-metrics.tex");
        let mut table = arxiv_table(source);
        for (position, expected) in [
            ((0, 0), "Metric"),
            ((0, 1), "Property"),
            ((0, 5), "Description"),
            ((1, 1), "symmetric"),
            ((2, 0), "CKA"),
            ((3, 0), "Unbiased CKA"),
        ] {
            let cell = &table.cells.as_ref().unwrap()[position.0][position.1];
            assert_eq!(&cell[table.cell_content_range(position).unwrap()], expected);
        }
        let position = (3, 5);
        let range = table.cell_content_range(position).unwrap();
        let cell = &mut table.cells.as_mut().unwrap()[position.0][position.1];
        let visible = &cell[range.clone()];
        assert!(visible.starts_with("Unbiased estimator of CKA"));
        assert!(!visible.contains("vspace"));
        let edited = visible.replacen("Unbiased estimator", "Bias-corrected estimator", 1);
        cell.replace_range(range, &edited);
        assert_eq!(
            table.grid_source_edit().unwrap().replacement,
            table
                .source
                .replacen("Unbiased estimator", "Bias-corrected estimator", 1)
        );
    }

    #[test]
    fn cell_projection_keeps_font_and_static_spacing_controls_outside_the_edit() {
        for cell in [
            r"\small Héllo",
            r"{\vspace{-0.5em}Héllo}",
            r"\multirow[t]{2}[1]{*}[-0.5em]{\vspace*{2pt}\textbf{Héllo}}",
            r"\footnotesize \itshape {Héllo}",
            r"\hspace{1cm}Héllo",
            "\\small% font delimiter\nHéllo",
        ] {
            let source = format!(r"\begin{{tabular}}{{l}}{cell}\\\end{{tabular}}");
            let mut table = tex_table_at(&source, 1).unwrap();
            let range = table.cell_content_range((0, 0)).unwrap();
            let value = &mut table.cells.as_mut().unwrap()[0][0];
            assert_eq!(&value[range.clone()], "Héllo", "{cell}");
            value.replace_range(range, "Updated");
            assert_eq!(
                table.grid_source_edit().unwrap().replacement,
                source.replacen("Héllo", "Updated", 1),
                "{cell}"
            );
        }
        // A font declaration without a delimiter cannot safely be detached
        // from the next token; arbitrary spacing expressions remain visible.
        for cell in [
            r"\small1",
            r"\small",
            r"\vspace{\baselineskip}Héllo",
            r"\hspace{2pt plus 1pt}Héllo",
            r"\textbf{Héllo} and more",
        ] {
            let source = format!(r"\begin{{tabular}}{{l}}{cell}\\\end{{tabular}}");
            let table = tex_table_at(&source, 1).unwrap();
            let value = &table.cells.as_ref().unwrap()[0][0];
            assert_eq!(&value[table.cell_content_range((0, 0)).unwrap()], cell);
        }
    }

    #[test]
    fn projected_body_range_stays_stable_while_typing_incomplete_tex() {
        let source =
            r"\begin{tabular}{cc}\multicolumn{2}{c}{\textbf{Héader}}\\ A & B\\\end{tabular}";
        let mut table = tex_table_at(source, 1).unwrap();
        let content = table.cell_content_range((0, 0)).unwrap();
        let prefix = table.cells.as_ref().unwrap()[0][0][..content.start].to_owned();
        let suffix = table.cells.as_ref().unwrap()[0][0][content.end..].to_owned();
        for draft in ["{", "\\textbf{", "unfinished \\", "{α\\textit{"] {
            let range = table.cell_content_range((0, 0)).unwrap();
            table.cells.as_mut().unwrap()[0][0].replace_range(range, draft);
            let cell = &table.cells.as_ref().unwrap()[0][0];
            assert_eq!(&cell[table.cell_content_range((0, 0)).unwrap()], draft);
            assert!(cell.starts_with(&prefix) && cell.ends_with(&suffix));
            assert!(table.grid_source_edit().is_err(), "{draft}");
        }
        let range = table.cell_content_range((0, 0)).unwrap();
        table.cells.as_mut().unwrap()[0][0].replace_range(range, "Repaired");
        assert_eq!(
            table.grid_source_edit().unwrap().replacement,
            source.replacen("Héader", "Repaired", 1)
        );
        // An implicit cell has no wrappers to preserve and remains editable
        // even while its draft is temporarily incomplete.
        let mut table = tex_table_at(r"\begin{tabular}{cc}A\\\end{tabular}", 1).unwrap();
        table.cells.as_mut().unwrap()[0][1] = "{".into();
        assert_eq!(table.cell_content_range((0, 1)), Some(0..1));
        assert!(table.grid_source_edit().is_err());
    }

    #[test]
    fn package_environments_open_a_grid_and_preserve_their_headers() {
        for (name, args) in [
            ("tabular*", r"{\linewidth}[t]{lc}"),
            ("tabulary", r"{\linewidth}{LJ}"),
            ("xltabular", r"[l]{\linewidth}{lX}"),
            ("NiceTabular", r"{lc}[hvlines]"),
            ("NiceTabular*", r"{\linewidth}{lc}[hvlines]"),
            ("tblr", r"[baseline=T]{colspec={Q[l]X[2,r]},hlines,vlines}"),
            ("longtblr", r"[caption={A, B}]{colspec={lc},rowhead=1}"),
            ("talltblr", r"{lc}"),
        ] {
            let header = format!(r"\begin{{{name}}}{args}");
            let source = format!(r"{header}Alpha & Beta \\\end{{{name}}}");
            let mut table = tex_table_at(&source, 2).expect(name);
            assert_eq!(table.environment.name(), name);
            assert_eq!(table.columns(), Some(2), "{source}");
            assert_eq!(
                table.cells.as_ref().unwrap()[0],
                ["Alpha", "Beta"],
                "{source}"
            );
            table.cells.as_mut().unwrap()[0][1] = "Edited".into();
            let rendered = table.grid_source_edit().unwrap().replacement;
            assert!(rendered.starts_with(&header), "{rendered}");
            assert!(rendered.ends_with(&format!(r"\end{{{name}}}")));
            assert_eq!(tex_table_at(&rendered, 2).unwrap().cells, table.cells);
        }
    }

    #[test]
    fn package_numeric_columns_are_not_mistaken_for_custom_column_types() {
        for spec in [
            r"lS[table-format=2.1,table-number-alignment=center]",
            r"lD{.}{,}{2.1}",
            r"lw{r}{2cm}",
            r"lW{c}{2cm}",
        ] {
            let source = format!(r"\begin{{tabular}}{{{spec}}}Alpha & 1.2 \\\end{{tabular}}");
            let table = tex_table_at(&source, 1).unwrap();
            assert_eq!(table.columns(), Some(2), "{spec}");
            assert!(table.cells.is_some(), "{spec}");
            assert_eq!(table.column_spec, spec);
            assert_eq!(
                tex_table_at(&table.render_replacement().unwrap(), 1)
                    .unwrap()
                    .column_spec,
                spec
            );
        }
    }

    #[test]
    fn tabularray_column_edit_preserves_other_inner_and_outer_options() {
        let source = r"\begin{tblr}[caption={One, two}]{width=\linewidth,colspec={lX[2,r]},row{1}={font=\bfseries},hlines}A & B\\\end{tblr}";
        let mut table = tex_table_at(source, 1).unwrap();
        table.set_column_spec("Q[c]X[l]").unwrap();
        let rendered = table.grid_source_edit().unwrap().replacement;
        assert!(rendered.contains("colspec={Q[c]X[l]}"));
        for keep in [
            "caption={One, two}",
            r"width=\linewidth",
            r"row{1}={font=\bfseries}",
            "hlines",
        ] {
            assert!(rendered.contains(keep), "{rendered}");
        }
        let before = table.clone();
        assert!(table.set_column_spec("l},evil={lc").is_err());
        assert_eq!(table, before);
    }

    #[test]
    fn package_row_directives_are_preserved_without_becoming_cell_values() {
        let source = "\\begin{longtable}{lc}\n\\toprule\nHeader & Value \\\\\n\\endhead\n\\rowcolor[gray]{0.9}\nAlpha & Beta \\\\\n\\bottomrule\n\\end{longtable}";
        let mut table = tex_table_at(source, 1).unwrap();
        assert_eq!(
            table.cells.as_ref().unwrap(),
            &vec![
                vec!["Header".to_owned(), "Value".to_owned()],
                vec!["Alpha".to_owned(), "Beta".to_owned()]
            ]
        );
        table.cells.as_mut().unwrap()[1][0] = "Edited".into();
        let rendered = table.grid_source_edit().unwrap().replacement;
        for keep in [r"\endhead", r"\rowcolor[gray]{0.9}", r"\bottomrule"] {
            assert!(rendered.contains(keep), "{rendered}");
        }
    }

    #[test]
    fn package_structural_layouts_and_missing_specs_are_rejected() {
        for (name, args, body) in [
            ("tblr", "{colspec={lc},cell{1}{1}={c=2}}", r"Heading & \\"),
            ("tblr", "{colspec={lc}}", r"\SetCell[c=2]{c}Heading & \\"),
            ("tblr", "{hlines}", r"A & B \\"),
            ("tblr", r"[expand=\expanded]{lc}", r"A & B \\"),
            ("NiceTabular", "{lc}[create-blocks-in-col=1]", r"A & B \\"),
            ("tblr", "{colspec=lc,colspec=cc}", r"A & B \\"),
            ("NiceTabular", "{lc}[hvlines]", r"\Block{1-2}{Heading} & \\"),
            (
                "NiceTabular",
                "{lc}",
                r"A & B \\ \CodeAfter \line{1-1}{2-2}",
            ),
            ("longtable", "{lc}", r"\caption{Table} \\ A & B \\"),
        ] {
            let source = format!(r"\begin{{{name}}}{args}{body}\end{{{name}}}");
            assert!(tex_table_at(&source, 1).is_none(), "{source}");
        }
    }

    #[test]
    fn tabularray_nested_options_comments_and_rule_brackets_are_lossless() {
        let source = "\\begin{tblr}[caption={A ] bracket, B}]{\n % keep options\ncolspec = {|[1pt,blue]Q[l]X[si={table-format=2.1},r]|},\nrow{1}={font=\\bfseries},hlines\n}A & 2.3\\\\\\end{tblr}";
        let mut table = tex_table_at(source, 1).unwrap();
        assert_eq!(table.columns(), Some(2));
        assert!(table.cells.is_some());
        table.set_column_spec("cc").unwrap();
        let rendered = table.grid_source_edit().unwrap().replacement;
        assert!(rendered.contains("caption={A ] bracket, B}"));
        assert!(rendered.contains("% keep options\ncolspec = {cc}"));
    }

    #[test]
    fn package_column_expansion_and_key_lists_are_bounded() {
        let source = r"\begin{tblr}{colspec={*{129}{Q[l]}}}A\end{tblr}";
        assert!(tex_table_at(source, 1).is_none());
        let options = format!("{},colspec=lc", "hlines,".repeat(4097));
        let source = format!(r"\begin{{tblr}}{{{options}}}A & B\end{{tblr}}");
        assert!(tex_table_at(&source, 1).is_none());
        assert!(
            column_count(
                "*{2}{S[table-format=2.1]D{.}{,}{2}}",
                ColumnDialect::Array,
                0
            ) == Some(4)
        );
    }

    #[test]
    fn basic_tex_rows_preserve_column_options_rules_and_packages() {
        let source = "\\usepackage[table]{xcolor}\n\\usepackage{array,booktabs,tabularx}\n\\begin{tabularx}{\\linewidth}[t]{@{}l>{\\raggedleft\\arraybackslash}X@{}}\n\\toprule\nName & Value \\\\[2pt]\n\\midrule\n\\textbf{Héllo} & $x_1$ \\\\\n\\bottomrule\n\\end{tabularx}";
        let cursor = source[..source.find("Name").unwrap()].chars().count();
        let mut table = tex_table_at(source, cursor).unwrap();
        assert_eq!(table.environment, TexTableEnvironment::TabularX);
        assert_eq!(table.columns(), Some(2));
        assert_eq!(table.packages, ["xcolor", "array", "booktabs", "tabularx"]);
        let cells = table.cells.as_mut().unwrap();
        assert_eq!(cells[1][0], "\\textbf{Héllo}");
        cells[1][1] = "$y_2$".into();
        let rendered = table.grid_source_edit().unwrap().replacement;
        for keep in [
            "[t]",
            ">{\\raggedleft\\arraybackslash}X",
            "\\toprule",
            "\\midrule",
            "\\bottomrule",
            "\\\\[2pt]",
            "$y_2$",
        ] {
            assert!(rendered.contains(keep), "missing {keep}: {rendered}");
        }
    }
    #[test]
    fn changing_tex_column_spec_keeps_width_position_and_existing_rows() {
        let source = "\\begin{tabularx}{\\linewidth}[t]{lX}A & B \\\\ \\end{tabularx}";
        let mut table = tex_table_at(source, 1).unwrap();
        table
            .set_column_spec("@{}r>{\\raggedright\\arraybackslash}X@{}")
            .unwrap();
        let rendered = table.grid_source_edit().unwrap().replacement;
        assert!(rendered.starts_with("\\begin{tabularx}{\\linewidth}[t]{@{}r>"));
        assert_eq!(table.cells.as_ref().unwrap()[0], ["A", "B"]);
        let before = table.clone();
        for spec in ["l", "lll", "lY", "l}injected{X"] {
            assert!(table.set_column_spec(spec).is_err(), "{spec}");
            assert_eq!(table, before);
        }
    }

    #[test]
    fn tex_unknown_structural_macros_and_custom_columns_are_rejected() {
        for body in [
            "A & \\input{data} \\\\",
            "\\caption{Example}\\\\ A & B \\\\",
            "A & B & C \\\\",
        ] {
            let source = format!("\\begin{{longtable}}[l]{{lc}}{body}\\end{{longtable}}");
            assert!(tex_table_at(&source, 1).is_none(), "{source}");
        }
        let source = "\\begin{tabular}{Yl}1 & A \\\\ \\end{tabular}";
        assert!(tex_table_at(source, 1).is_none());
    }
    #[test]
    fn tex_detection_ignores_comments_and_verbatim_and_picks_inner_table() {
        assert!(tex_table_at("% \\begin{tabular}{l}A\\end{tabular}", 12).is_none());
        assert!(tex_table_at("\\verb|\\begin{tabular}{l}A\\end{tabular}|", 20).is_none());
        let source = "é \\begin{tabular}{l}{\\begin{tabular}{c}Inside\\end{tabular}}\\end{tabular}";
        let cursor = source[..source.find("Inside").unwrap()].chars().count();
        let table = tex_table_at(source, cursor).unwrap();
        assert_eq!(table.column_spec, "c");
        assert_eq!(table.cells.as_ref().unwrap()[0][0], "Inside");
        assert!(tex_table_at(source, 999).is_none());
        let verbatim = "\\begin{verbatim}\\begin{tabular}{l}A\\end{tabular}\\end{verbatim}";
        assert!(tex_table_at(verbatim, 30).is_none());
        let unrelated = "\\begin{tabular}{l}A\\end{tabular}\\begin{broken}\\end{other}";
        assert!(tex_table_at(unrelated, 10).is_some());
    }
    #[test]
    fn unsupported_inner_table_does_not_open_its_enclosing_grid() {
        let source = r"\begin{tabular}{l}{\begin{tabular}{Y}Inner\end{tabular}}\end{tabular}";
        let cursor = source.find("Inner").unwrap();
        assert!(tex_table_at(source, cursor).is_none());
        assert!(tex_table_at(source, 1).is_some());
    }

    #[test]
    fn native_alignment_primitives_and_row_generators_do_not_guess_a_grid() {
        for body in [
            r"A\cr B\cr",
            r"\omit A",
            r"\multispan2 A",
            r"A\span B",
            r"\noalign{\vskip1pt}A",
            r"\foreach\n in {A,B}{\n\\}",
            r"\loop A\\\repeat",
            r"\input{rows}",
        ] {
            let source = format!(r"\begin{{tabular}}{{l}}{body}\end{{tabular}}");
            assert!(tex_table_at(&source, 1).is_none(), "{source}");
        }
        // Literal cell macros remain source-preserving content; only explicit
        // geometry-generating primitives are rejected by static detection.
        let source = r"\begin{tabular}{l}\textbf{A}\\\cmark\\\end{tabular}";
        let table = tex_table_at(source, 1).unwrap();
        assert_eq!(table.cells.as_ref().unwrap().len(), 2);
        assert_eq!(table.render_replacement().unwrap(), source);
    }

    #[test]
    fn unfinished_unrelated_command_does_not_hide_a_complete_table() {
        let source = "\\begin{tabular}{l}A\\end{tabular} unfinished \\";
        let table = tex_table_at(source, 10).unwrap();
        assert_eq!(table.cells.as_ref().unwrap()[0], ["A"]);
        assert_eq!(table.source, "\\begin{tabular}{l}A\\end{tabular}");
        let mut draft = table;
        draft.cells.as_mut().unwrap()[0][0] = "unfinished \\".into();
        assert!(draft.grid_source_edit().is_err());
    }

    #[test]
    fn unicode_verbatim_delimiters_keep_scanner_offsets_on_character_boundaries() {
        for delimiter in ['α', 'é', '🦀'] {
            let source = format!(
                "\\verb{delimiter}ignored{delimiter} \\begin{{tabular}}{{l}}A\\end{{tabular}}"
            );
            let cursor = source[..source.find("A\\end").unwrap()].chars().count();
            let mut table = tex_table_at(&source, cursor).unwrap();
            table.cells.as_mut().unwrap()[0][0] =
                format!("\\verb{delimiter}escaped & literal{delimiter}");
            assert!(table.grid_source_edit().is_ok());
        }
    }

    #[test]
    fn tex_grid_apply_rejects_injected_alignment_and_rows() {
        let source = "\\begin{tabular}{lc}A & B \\\\ \\end{tabular}";
        for body in ["X & Y", "X \\\\ Y", "X\\end{tabular}", "{X", "$X"] {
            let mut table = tex_table_at(source, 1).unwrap();
            table.cells.as_mut().unwrap()[0][0] = body.into();
            assert!(table.grid_source_edit().is_err(), "{body}");
        }
        let table = tex_table_at(source, 1).unwrap();
        for draft in [
            "\\begin{tabular}{l}A",
            "\\begin{longtable}{l}A\\end{longtable}",
            "\\begin{tabular}{l}A\\end{tabular}after",
        ] {
            assert!(table.source_edit(draft).is_err(), "{draft}");
        }
    }
    #[test]
    fn repeated_grid_apply_keeps_row_delimiters_and_newlines_stable() {
        for source in [
            r"\begin{tabular}{cc}A & B \\
C & D \\
\end{tabular}",
            r"\begin{tabular}[t]{cc}A & B \\[2pt]C & D \\\end{tabular}",
            r"\begin{longtable}{cc}A & B\tabularnewline
\hline C & D\tabularnewline
\end{longtable}",
            r"\begin{tabular}{cc}A & B % comment
 \\
C & D \\
\end{tabular}",
        ] {
            let first = tex_table_at(source, 1)
                .unwrap()
                .render_replacement()
                .unwrap();
            let second = tex_table_at(&first, 1)
                .unwrap()
                .render_replacement()
                .unwrap();
            let third = tex_table_at(&second, 1)
                .unwrap()
                .render_replacement()
                .unwrap();
            assert_eq!(second, first, "second Apply changed row endings: {source}");
            assert_eq!(third, first, "third Apply changed row endings: {source}");
        }
    }

    #[test]
    fn oversized_rows_are_rejected_with_bounded_allocation() {
        let source = format!(
            "\\begin{{tabular}}{{cc}}{}Z\\end{{tabular}}",
            "A & ".repeat(10_000)
        );
        let (table, allocated) = crate::test_allocations::allocated(|| tex_table_at(&source, 1));
        assert!(table.is_none());
        // Retaining the source needs a few complete buffer copies; discovering
        // an unsupported row must not allocate a separate String per delimiter.
        assert!(
            allocated < source.len() * 4 + 4096,
            "oversized row allocated {allocated} bytes for {} source bytes",
            source.len()
        );
    }

    #[test]
    fn named_row_delimiters_keep_their_terminating_trivia() {
        for source in [
            r"\begin{tabular}{cc}A & B\tabularnewline
C & D\tabularnewline
\end{tabular}",
            r"\begin{tabular}{cc}A & B\tabularnewline C & D\end{tabular}",
            r"\begin{tabular}{cc}A & B\tabularnewline % keep this row comment
C & D\tabularnewline[2pt]
\end{tabular}",
        ] {
            let mut table = tex_table_at(source, 1).unwrap();
            assert_eq!(table.cells.as_ref().unwrap().len(), 2);
            table.cells.as_mut().unwrap()[1][1] = "Edited".into();
            let rendered = table.grid_source_edit().unwrap().replacement;
            let parsed = tex_table_at(&rendered, 1).unwrap();
            assert_eq!(parsed.cells, table.cells, "{rendered}");
            assert!(!rendered.contains("\\tabularnewlineC"), "{rendered}");
            if source.contains("% keep this row comment") {
                assert!(rendered.contains("% keep this row comment\nC"));
            }
            assert_eq!(parsed.render_replacement().unwrap(), rendered);
        }
    }

    #[test]
    fn comments_at_cell_ends_keep_their_terminating_newline() {
        let source = "\\begin{tabular}{cc}A % first comment\n & B % second comment\n \\\\ \n C & D \\\\ \\end{tabular}";
        let mut table = tex_table_at(source, 1).unwrap();
        let cells = table.cells.as_mut().unwrap();
        assert_eq!(cells[0][0], "A % first comment\n");
        assert_eq!(cells[0][1], "B % second comment\n");
        cells[1][1] = "Changed".into();
        let rendered = table.grid_source_edit().unwrap().replacement;
        assert!(rendered.contains("% first comment\n &"));
        assert_eq!(rendered, source.replacen("C & D", "C & Changed", 1));
        assert_eq!(cell_source("\\% literal"), "\\% literal");
    }

    #[test]
    fn omitted_trailing_cells_are_materialized_only_when_edited() {
        let source =
            "\\begin{tabular}[t]{ccc}\n A \\\\[2pt]\n B & C \\\\\n D & E & F\n\\end{tabular}";
        let mut table = tex_table_at(source, 1).unwrap();
        assert_eq!(
            table.cells.as_ref().unwrap(),
            &vec![
                vec!["A".to_owned(), String::new(), String::new()],
                vec!["B".to_owned(), "C".to_owned(), String::new()],
                vec!["D".to_owned(), "E".to_owned(), "F".to_owned()],
            ]
        );
        assert_eq!(table.render_replacement().unwrap(), source);
        table.cells.as_mut().unwrap()[0][2] = "Last".into();
        table.cells.as_mut().unwrap()[1][2] = "New".into();
        let replacement = table.render_replacement().unwrap();
        assert_eq!(
            replacement,
            source
                .replacen("A \\\\[2pt]", "A  &  & Last\\\\[2pt]", 1)
                .replacen("B & C \\\\", "B & C  & New\\\\", 1)
        );
        let reparsed = tex_table_at(&replacement, 1).unwrap();
        assert_eq!(reparsed.cells, table.cells);
        assert_eq!(reparsed.render_replacement().unwrap(), replacement);
        table.set_column_spec("@{}lcr@{}").unwrap();
        assert_eq!(
            table.render_replacement().unwrap(),
            replacement.replacen("[t]{ccc}", "[t]{@{}lcr@{}}", 1)
        );
    }

    #[test]
    fn implicit_cells_after_comments_and_empty_payloads_keep_valid_delimiters() {
        for source in [
            "\\begin{tabular}{ccc}A % keep comment\n\\\\\\end{tabular}",
            "\\begin{tabular}{ccc}A &   \\\\\\end{tabular}",
        ] {
            let mut table = tex_table_at(source, 1).unwrap();
            assert_eq!(table.render_replacement().unwrap(), source);
            table.cells.as_mut().unwrap()[0][2] = "Changed".into();
            let replacement = table.render_replacement().unwrap();
            assert_eq!(tex_table_at(&replacement, 1).unwrap().cells, table.cells);
            if source.contains("% keep comment") {
                assert!(replacement.contains("% keep comment\n &"));
            }
        }
    }

    #[test]
    fn repeated_column_specs_and_escaped_cells_are_bounded() {
        assert_eq!(
            column_count("*{2}{l|p{2cm}}", ColumnDialect::Array, 0),
            Some(4)
        );
        assert_eq!(
            column_count("*{999999999999}{l}", ColumnDialect::Array, 0),
            None
        );
        assert_eq!(column_count("Q", ColumnDialect::Array, 0), None);
        let source = "\\begin{tabular}{cc}\\& & {nested & text} \\\\ \\end{tabular}";
        let table = tex_table_at(source, 1).unwrap();
        assert_eq!(table.cells.as_ref().unwrap()[0], ["\\&", "{nested & text}"]);
        assert!(table.grid_source_edit().is_ok());
    }
}
