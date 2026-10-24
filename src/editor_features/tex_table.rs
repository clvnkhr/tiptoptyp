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
    Array,
    Matrix,
    Pmatrix,
    Bmatrix,
    BraceMatrix,
    Vmatrix,
    DoubleVmatrix,
    SmallMatrix,
    MatrixStar,
    PmatrixStar,
    BmatrixStar,
    BraceMatrixStar,
    VmatrixStar,
    DoubleVmatrixStar,
    Cases,
    DCases,
    RCases,
    DRCases,
    Aligned,
    Gathered,
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
            Self::Array => "array",
            Self::Matrix => "matrix",
            Self::Pmatrix => "pmatrix",
            Self::Bmatrix => "bmatrix",
            Self::BraceMatrix => "Bmatrix",
            Self::Vmatrix => "vmatrix",
            Self::DoubleVmatrix => "Vmatrix",
            Self::SmallMatrix => "smallmatrix",
            Self::MatrixStar => "matrix*",
            Self::PmatrixStar => "pmatrix*",
            Self::BmatrixStar => "bmatrix*",
            Self::BraceMatrixStar => "Bmatrix*",
            Self::VmatrixStar => "vmatrix*",
            Self::DoubleVmatrixStar => "Vmatrix*",
            Self::Cases => "cases",
            Self::DCases => "dcases",
            Self::RCases => "rcases",
            Self::DRCases => "drcases",
            Self::Aligned => "aligned",
            Self::Gathered => "gathered",
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
            "array" => Some(Self::Array),
            "matrix" => Some(Self::Matrix),
            "pmatrix" => Some(Self::Pmatrix),
            "bmatrix" => Some(Self::Bmatrix),
            "Bmatrix" => Some(Self::BraceMatrix),
            "vmatrix" => Some(Self::Vmatrix),
            "Vmatrix" => Some(Self::DoubleVmatrix),
            "smallmatrix" => Some(Self::SmallMatrix),
            "matrix*" => Some(Self::MatrixStar),
            "pmatrix*" => Some(Self::PmatrixStar),
            "bmatrix*" => Some(Self::BmatrixStar),
            "Bmatrix*" => Some(Self::BraceMatrixStar),
            "vmatrix*" => Some(Self::VmatrixStar),
            "Vmatrix*" => Some(Self::DoubleVmatrixStar),
            "cases" => Some(Self::Cases),
            "dcases" => Some(Self::DCases),
            "rcases" => Some(Self::RCases),
            "drcases" => Some(Self::DRCases),
            "aligned" => Some(Self::Aligned),
            "gathered" => Some(Self::Gathered),
            _ => None,
        }
    }
    fn is_math(self) -> bool {
        !matches!(
            self,
            Self::Tabular
                | Self::TabularStar
                | Self::TabularX
                | Self::Tabulary
                | Self::LongTable
                | Self::XlTabular
                | Self::NiceTabular
                | Self::NiceTabularStar
                | Self::Tblr
                | Self::LongTblr
                | Self::TallTblr
        )
    }
    fn has_column_spec(self) -> bool {
        !self.is_math() || self == Self::Array
    }
    fn is_star_matrix(self) -> bool {
        matches!(
            self,
            Self::MatrixStar
                | Self::PmatrixStar
                | Self::BmatrixStar
                | Self::BraceMatrixStar
                | Self::VmatrixStar
                | Self::DoubleVmatrixStar
        )
    }
    fn uses_max_matrix_columns(self) -> bool {
        matches!(
            self,
            Self::Matrix
                | Self::Pmatrix
                | Self::Bmatrix
                | Self::BraceMatrix
                | Self::Vmatrix
                | Self::DoubleVmatrix
        ) || self.is_star_matrix()
    }
    fn fixed_columns(self) -> Option<usize> {
        match self {
            Self::Cases | Self::DCases | Self::RCases | Self::DRCases => Some(2),
            Self::Gathered => Some(1),
            _ => None,
        }
    }
    fn valid_columns(self, columns: usize) -> bool {
        (1..=128).contains(&columns)
            && self.fixed_columns().is_none_or(|fixed| columns == fixed)
            && (self != Self::Aligned || columns.is_multiple_of(2))
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
    spec_range: Option<std::ops::Range<usize>>,
    grid_columns: usize,
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
        if self.has_column_spec() {
            column_count(&self.column_spec, self.environment.dialect(), 0)
        } else {
            Some(self.grid_columns)
        }
    }
    pub(crate) fn is_math(&self) -> bool {
        self.environment.is_math()
    }
    pub(crate) fn has_column_spec(&self) -> bool {
        self.environment.has_column_spec()
    }
    pub(crate) fn column_spec_for_imports(&self) -> &str {
        if self.has_column_spec() {
            &self.column_spec
        } else if self.environment.is_star_matrix() {
            command(&self.header, 0)
                .and_then(|(_, end)| group(&self.header, end, b'{', b'}'))
                .and_then(|(_, end)| group(&self.header, end, b'[', b']'))
                .map_or("", |(alignment, _)| alignment)
        } else {
            ""
        }
    }
    pub(crate) fn fixed_math_columns(&self) -> Option<usize> {
        self.environment.fixed_columns()
    }
    pub(crate) fn math_column_step(&self) -> usize {
        if self.environment == TexTableEnvironment::Aligned {
            2
        } else {
            1
        }
    }
    pub(crate) fn math_environment_names() -> &'static [&'static str] {
        &[
            "array",
            "matrix",
            "pmatrix",
            "bmatrix",
            "Bmatrix",
            "vmatrix",
            "Vmatrix",
            "smallmatrix",
            "matrix*",
            "pmatrix*",
            "bmatrix*",
            "Bmatrix*",
            "vmatrix*",
            "Vmatrix*",
            "cases",
            "dcases",
            "rcases",
            "drcases",
            "aligned",
            "gathered",
        ]
    }
    /// Change the mathematical enclosure without entering or leaving its outer
    /// math wrapper. Existing body bytes and supported optional arguments stay
    /// exact; options with a different meaning are rejected rather than lost.
    pub(crate) fn set_math_environment(&mut self, name: &str) -> Result<(), String> {
        let environment = TexTableEnvironment::from_name(name)
            .filter(|environment| environment.is_math())
            .ok_or("Choose a supported mathematical grid environment")?;
        if !self.is_math() {
            return Err("Only mathematical grids can change their math enclosure".into());
        }
        if environment == self.environment {
            return Ok(());
        }
        let columns = self
            .columns()
            .ok_or("Finish the column specification first")?;
        if environment.uses_max_matrix_columns()
            && columns > 10
            && !self.environment.uses_max_matrix_columns()
        {
            return Err("AMS matrices default to 10 columns. Keep a wider grid as an array".into());
        }
        if !environment.valid_columns(columns) {
            return Err(math_dimension_error(environment));
        }
        let current = self.render_replacement()?;
        let table =
            tex_table_at(&current, 1).ok_or("Finish the grid before changing its enclosure")?;
        let (_, opening_end) = command(&table.header, 0).ok_or("Invalid grid opening")?;
        let (opening_name, arguments) =
            group(&table.header, opening_end, b'{', b'}').ok_or("Invalid grid opening")?;
        let opening_range =
            (opening_name.as_ptr() as usize - table.header.as_ptr() as usize)..arguments - 1;
        let rest = &table.header[arguments..];
        if group(rest, 0, b'[', b']').is_some() {
            let compatible = (table.environment.is_star_matrix() && environment.is_star_matrix())
                || (matches!(
                    table.environment,
                    TexTableEnvironment::Array
                        | TexTableEnvironment::Aligned
                        | TexTableEnvironment::Gathered
                ) && matches!(
                    environment,
                    TexTableEnvironment::Array
                        | TexTableEnvironment::Aligned
                        | TexTableEnvironment::Gathered
                ));
            if !compatible {
                return Err("This grid has positioning or alignment options; keep an environment with the same option type".into());
            }
        }
        let mut header = table.header.clone();
        if let Some(spec_range) = &table.spec_range {
            if !environment.has_column_spec() {
                if !table
                    .column_spec
                    .chars()
                    .all(|c| c.is_whitespace() || matches!(c, 'l' | 'c' | 'r'))
                {
                    return Err("This array has column rules or formatting. Keep array, or simplify its columns to l, c and r before converting".into());
                }
                // Retain comments and trivia before and after the braces.
                header.replace_range(spec_range.start - 1..spec_range.end + 1, "");
            }
        } else if environment.has_column_spec() {
            header.push_str(&format!("{{{}}}", "c".repeat(columns)));
        }
        header.replace_range(opening_range, name);
        let mut footer = table.footer.clone();
        let (_, args) = command(&footer, 0).ok_or("Invalid grid ending")?;
        let (closing_name, end) = group(&footer, args, b'{', b'}').ok_or("Invalid grid ending")?;
        let range = (closing_name.as_ptr() as usize - footer.as_ptr() as usize)..end - 1;
        footer.replace_range(range, name);
        let candidate = format!("{header}{}{footer}", &current[table.body_range.clone()]);
        if tex_table_at(&candidate, 1).is_none_or(|candidate| candidate.cells != table.cells) {
            return Err("The new environment would reinterpret the first formula as an option. Keep this enclosure or group the formula first".into());
        }
        self.replace_math_source(&candidate)
    }
    /// Explicit resize; cell typing remains lossless and does not use this
    /// formatter. Refuse layout directives that cannot safely move with a row.
    pub(crate) fn resize_math_grid(&mut self, rows: usize, columns: usize) -> Result<(), String> {
        if !self.is_math() {
            return Err("Resize is available for mathematical grids".into());
        }
        if rows == 0
            || rows.saturating_mul(columns) > 4096
            || !self.environment.valid_columns(columns)
        {
            return Err(math_dimension_error(self.environment));
        }
        let old_columns = self
            .columns()
            .ok_or("Finish the column specification first")?;
        if self.environment.uses_max_matrix_columns() && columns > 10 && columns > old_columns {
            return Err("AMS matrices default to 10 columns. Use array for a wider grid".into());
        }
        if self.cells.as_ref().is_some_and(|cells| cells.len() == rows) && old_columns == columns {
            return Ok(());
        }
        let current = self.render_replacement()?;
        let table = tex_table_at(&current, 1).ok_or("Finish the grid before resizing it")?;
        let body = &current[table.body_range.clone()];
        let mut copied = 0;
        for row in &table.rows {
            for cell in &row.cells {
                if cell.columns != 1
                    || cell.requested_rows != 1
                    || !simple_math_gap(&body[copied..cell.range.start])
                {
                    return Err("This grid has row rules, comments or merged cells. Edit its dimensions in the source to preserve them".into());
                }
                copied = cell.range.end;
            }
        }
        if !simple_math_gap(&body[copied..]) {
            return Err("This grid has row rules, comments or spacing options. Edit its dimensions in the source to preserve them".into());
        }
        let mut header = table.header.clone();
        if table.has_column_spec() && columns != old_columns {
            let mut alignments: Vec<char> = table
                .column_spec
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            if !alignments.iter().all(|c| matches!(c, 'l' | 'c' | 'r')) {
                return Err("Keep this array's column count, or simplify its column specification to l, c and r before resizing".into());
            }
            alignments.resize(columns, 'c');
            header.replace_range(
                table.spec_range.clone().expect("array spec"),
                &alignments.into_iter().collect::<String>(),
            );
        }
        let mut cells = table
            .cells
            .clone()
            .ok_or("The grid has no editable cells")?;
        cells.resize_with(rows, || vec![String::new(); columns]);
        for row in &mut cells {
            row.resize(columns, String::new());
        }
        let candidate = format!(
            "{header}\n{} \\\\\n{}",
            cells
                .iter()
                .map(|row| row.join(" & "))
                .collect::<Vec<_>>()
                .join(" \\\\\n"),
            table.footer
        );
        self.replace_math_source(&candidate)
    }
    fn replace_math_source(&mut self, source: &str) -> Result<(), String> {
        let mut replacement = tex_table_at(source, 1)
            .ok_or("The mathematical grid could not be represented safely")?;
        if !replacement.is_math() || replacement.source_range != (0..source.chars().count()) {
            return Err("Keep one complete mathematical grid".into());
        }
        replacement.source_range = self.source_range.clone();
        *self = replacement;
        Ok(())
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
        let spec_range = self.spec_range.clone().ok_or(
            "This matrix infers its columns from the cells and has no column specification",
        )?;
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
        header.replace_range(spec_range.clone(), spec);
        // Validate the complete header so a brace or alignment injection cannot
        // write outside the column specification.
        let wrapper = format!("{header}{}", self.footer);
        let parsed = tex_table_at(&wrapper, 1)
            .ok_or("Finish the column specification before applying it")?;
        if parsed.column_spec != spec || parsed.source_range != (0..wrapper.chars().count()) {
            return Err("Enter one column specification without surrounding braces".into());
        }
        self.header = header;
        self.spec_range = Some(spec_range.start..spec_range.start + spec.len());
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
                if let Some(literal) = literal_span(source, start, name, end) {
                    i = literal.end;
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
                    if stack.len() >= 128 {
                        break;
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
    if environment.is_math() && !environment.has_column_spec() {
        return parse_math_table(source, begin, args, close, end, environment);
    }
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
        outer_simple = simple_package_options(options, environment)
            && (environment != TexTableEnvironment::Array
                || matches!(options.trim(), "t" | "c" | "b"));
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
        spec_range: Some((spec_start - begin)..(spec_start - begin + spec.len())),
        grid_columns: columns?,
        footer: source.get(close..end)?.to_owned(),
        body_range: (body_start - begin)..(close - begin),
        rows,
        spans,
        owners,
    })
}

fn math_dimension_error(environment: TexTableEnvironment) -> String {
    match environment.fixed_columns() {
        Some(2) => "Cases need exactly two columns: expression and condition".into(),
        Some(1) => "Gathered equations need exactly one column".into(),
        _ if environment == TexTableEnvironment::Aligned =>
            "Aligned equations need an even number of columns (left/right pairs), up to 128, and at most 4,096 cells".into(),
        _ => "Use 1–128 columns and at most 4,096 mathematical cells".into(),
    }
}
fn simple_math_gap(gap: &str) -> bool {
    gap.bytes()
        .all(|byte| byte.is_ascii_whitespace() || matches!(byte, b'&' | b'\\'))
}
fn parse_math_table(
    source: &str,
    begin: usize,
    mut body_start: usize,
    close: usize,
    end: usize,
    environment: TexTableEnvironment,
) -> Option<TexTable> {
    if environment.is_star_matrix() {
        if let Some((alignment, next)) = group(source, body_start, b'[', b']') {
            if column_count(alignment, ColumnDialect::Array, 0) != Some(1) {
                return None;
            }
            body_start = next;
        }
    } else if matches!(
        environment,
        TexTableEnvironment::Aligned | TexTableEnvironment::Gathered
    ) && let Some((position, next)) = group(source, body_start, b'[', b']')
    {
        if !matches!(position.trim(), "t" | "c" | "b") {
            return None;
        }
        body_start = next;
    }
    let body = source.get(body_start..close)?;
    let inferred = inferred_math_columns(body)?;
    let columns = match environment {
        TexTableEnvironment::Aligned => inferred.next_multiple_of(2),
        _ => environment.fixed_columns().unwrap_or(inferred),
    };
    if !environment.valid_columns(columns) {
        return None;
    }
    let (cells, rows) = split_rows(body, columns)?;
    let spans = cell_spans(&cells, &rows, columns);
    let owners = cell_owners(&spans);
    Some(TexTable {
        source_range: byte_range_to_char(source, begin..end),
        source: source.get(begin..end)?.to_owned(),
        environment,
        packages: packages(source),
        column_spec: String::new(),
        cells: Some(cells),
        header: source.get(begin..body_start)?.to_owned(),
        spec_range: None,
        grid_columns: columns,
        footer: source.get(close..end)?.to_owned(),
        body_range: (body_start - begin)..(close - begin),
        rows,
        spans,
        owners,
    })
}
/// One bounded scan determines inferred widths, then the ordinary lossless row
/// parser builds the grid. Nested environments own their separators, including
/// nested matrices that are not enclosed in a TeX brace group.
fn inferred_math_columns(body: &str) -> Option<usize> {
    let mut columns = 1usize;
    let mut maximum = 1usize;
    let mut rows = 1usize;
    let mut depth = 0usize;
    let mut math = false;
    let mut i = 0;
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
                columns += 1;
                maximum = maximum.max(columns);
                if maximum > 128 || maximum.saturating_mul(rows) > 4096 {
                    return None;
                }
                i += 1;
            }
            b'\\' => {
                let (name, end) = command(body, i)?;
                if let Some(literal) = literal_span(body, i, name, end) {
                    if !literal.closed {
                        return None;
                    }
                    i = literal.end;
                } else if name == "begin" {
                    i = skip_environment(body, i)?;
                } else if depth == 0 && !math && matches!(name, "\\" | "tabularnewline") {
                    rows += 1;
                    // A trailing terminator does not add a logical row.
                    if rows > 4097 {
                        return None;
                    }
                    columns = 1;
                    i = end;
                    if body.as_bytes().get(i) == Some(&b'*') {
                        i += 1;
                    }
                    if let Some((_, next)) = group(body, i, b'[', b']') {
                        i = next;
                    }
                } else {
                    i = end;
                }
            }
            _ => i = next_char(body, i),
        }
    }
    (depth == 0 && !math).then_some(maximum)
}
fn skip_environment(source: &str, start: usize) -> Option<usize> {
    let (name, args) = command(source, start)?;
    if name != "begin" {
        return None;
    }
    if let Some(literal) = literal_span(source, start, name, args) {
        return literal.closed.then_some(literal.end);
    }
    let (name, mut i) = group(source, args, b'{', b'}')?;
    let mut stack = vec![name];
    while i < source.len() {
        match source.as_bytes()[i] {
            b'%' => i = skip_comment(source, i),
            b'\\' => {
                let start = i;
                let (command_name, end) = command(source, i)?;
                i = end;
                if let Some(literal) = literal_span(source, start, command_name, end) {
                    if !literal.closed {
                        return None;
                    }
                    i = literal.end;
                } else if matches!(command_name, "begin" | "end") {
                    let (name, next) = group(source, i, b'{', b'}')?;
                    i = next;
                    if command_name == "begin" {
                        if stack.len() >= 128 {
                            return None;
                        }
                        stack.push(name);
                    } else if stack.pop()? != name {
                        return None;
                    } else if stack.is_empty() {
                        return Some(i);
                    }
                }
            }
            _ => i = next_char(source, i),
        }
    }
    None
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
                i = if let Some(literal) = literal_span(source, i, name, end) {
                    if !literal.closed {
                        return None;
                    }
                    literal.end
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
                if let Some(literal) = literal_span(body, i, name, end) {
                    if !literal.closed {
                        return None;
                    }
                    i = literal.end;
                    continue;
                }
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
                if name == "begin" {
                    i = skip_environment(body, i)?;
                    continue;
                }
                if name == "end" {
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
                i = literal_span(trimmed, i, name, end).map_or(end, |literal| literal.end);
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
                let start = i;
                let Some((name, mut end)) = command(source, i) else {
                    break;
                };
                i = end;
                if let Some(literal) = literal_span(source, start, name, end) {
                    i = literal.end;
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
                i = if let Some(literal) = literal_span(source, i, name, end) {
                    if !literal.closed {
                        return false;
                    }
                    literal.end
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
                i = if let Some(literal) = literal_span(source, i, name, end) {
                    if !literal.closed {
                        return None;
                    }
                    literal.end
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
#[derive(Clone, Copy)]
struct LiteralSpan {
    end: usize,
    closed: bool,
}

fn literal_span(source: &str, start: usize, name: &str, end: usize) -> Option<LiteralSpan> {
    if let Some(literal) = crate::editor_math_guides::tex_inline_literal(source, start) {
        return Some(LiteralSpan {
            end: literal.end,
            closed: literal.closed,
        });
    }
    // fancyvrb's capitalised command has the same delimiter syntax as \verb.
    if name == "Verb" {
        let at = end + usize::from(source.as_bytes().get(end) == Some(&b'*'));
        let Some(delimiter) = source[at..]
            .chars()
            .next()
            .filter(|c| !matches!(c, '\n' | '\r'))
        else {
            return Some(LiteralSpan {
                end: at,
                closed: false,
            });
        };
        let body = at + delimiter.len_utf8();
        let stop = source[body..]
            .find([delimiter, '\n', '\r'])
            .map_or(source.len(), |n| body + n);
        let closed = source[stop..].starts_with(delimiter);
        return Some(LiteralSpan {
            end: stop + if closed { delimiter.len_utf8() } else { 0 },
            closed,
        });
    }
    if name != "begin" {
        return None;
    }
    // Environment names here must be literal. Do not recurse through the
    // general group reader while it is itself checking a literal span.
    let opening = skip_trivia(source, end);
    if source.as_bytes().get(opening) != Some(&b'{') {
        return None;
    }
    let close = source[opening + 1..].find('}')? + opening + 1;
    let environment = &source[opening + 1..close];
    let body = close + 1;
    if !matches!(
        environment,
        "verbatim"
            | "verbatim*"
            | "Verbatim"
            | "Verbatim*"
            | "BVerbatim"
            | "LVerbatim"
            | "SaveVerbatim"
            | "lstlisting"
            | "minted"
            | "comment"
            | "filecontents"
            | "filecontents*"
    ) {
        return None;
    }
    let closing = format!("\\end{{{environment}}}");
    let mut at = body;
    for line in source[body..].split_inclusive('\n') {
        at += line.len();
        if line.trim() == closing {
            return Some(LiteralSpan {
                end: at,
                closed: true,
            });
        }
    }
    Some(LiteralSpan {
        end: source.len(),
        closed: false,
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
    fn mathematical_grids_are_lossless_and_have_real_cell_dimensions() {
        for name in TexTable::math_environment_names() {
            let (options, body, columns) = match *name {
                "array" => (
                    "[t]{rcl}",
                    r"a_{ij} & = & \frac{1}{2}\\[2pt] b & = & \alpha",
                    3,
                ),
                "aligned" => ("[b]", r"f(x) & = \frac{1}{2}\\ g(x) & = \alpha", 2),
                "gathered" => ("[t]", r"f(x)=\frac{1}{2}\\g(x)=\alpha", 1),
                name if name.ends_with('*') => ("[r]", r"-1 & \frac{1}{2}\\[2pt] \alpha & 4", 2),
                "cases" | "dcases" | "rcases" | "drcases" => {
                    ("", r"x^2 & \text{if $x>0$},\\ 0 & \text{otherwise}.", 2)
                }
                _ => ("", r"a_{ij} & \frac{1}{2}\\[2pt]\alpha & 4", 2),
            };
            let source = format!(
                "é \\[\\begin{{{name}}}{options}\n % keep prefix\n{body}\n\\end{{{name}}}\\] after"
            );
            let cursor = source[..source.find(body).unwrap()].chars().count();
            let mut table = tex_table_at(&source, cursor).expect(name);
            assert!(table.is_math(), "{name}");
            assert_eq!(table.has_column_spec(), *name == "array", "{name}");
            assert_eq!(table.columns(), Some(columns), "{name}");
            assert_eq!(table.cells.as_ref().unwrap().len(), 2, "{name}");
            assert_eq!(
                table.grid_source_edit().unwrap().replacement,
                table.source,
                "{name}"
            );
            let original = table.source.clone();
            let old = table.cells.as_ref().unwrap()[0][0].clone();
            table.cells.as_mut().unwrap()[0][0] = r"\beta_{ij}".into();
            assert_eq!(
                table.render_replacement().unwrap(),
                original.replacen(&old, r"\beta_{ij}", 1),
                "{name}"
            );
            if *name != "array" {
                assert!(table.column_spec.is_empty());
                assert!(table.set_column_spec("cc").is_err());
            }
        }
    }

    #[test]
    fn nested_math_grids_own_their_alignment_and_target_the_innermost_grid() {
        let source =
            r"\begin{pmatrix}\begin{matrix}a & b\\c & d\end{matrix} & z\\u & v\end{pmatrix}";
        let outer = tex_table_at(source, 1).unwrap();
        assert_eq!(outer.columns(), Some(2));
        assert_eq!(outer.cells.as_ref().unwrap().len(), 2);
        assert_eq!(
            outer.cells.as_ref().unwrap()[0][0],
            r"\begin{matrix}a & b\\c & d\end{matrix}"
        );
        assert_eq!(outer.render_replacement().unwrap(), source);
        let inner = tex_table_at(source, source.find("a & b").unwrap()).unwrap();
        assert_eq!(inner.environment, TexTableEnvironment::Matrix);
        assert_eq!(inner.cells.as_ref().unwrap()[0], ["a", "b"]);
        let outer = r"\begin{tabular}{c}$\begin{bmatrix}a&b\\c&d\end{bmatrix}$\end{tabular}";
        assert_eq!(tex_table_at(outer, 1).unwrap().columns(), Some(1));
    }

    #[test]
    fn inferred_math_cells_preserve_implicit_tails_and_reject_alignment_injection() {
        let source = r"\begin{pmatrix}a & b & c\\d\end{pmatrix}";
        let mut table = tex_table_at(source, 1).unwrap();
        assert_eq!(table.cells.as_ref().unwrap()[1], ["d", "", ""]);
        table.cells.as_mut().unwrap()[1][2] = r"\frac{x}{y}".into();
        let changed = table.render_replacement().unwrap();
        assert_eq!(tex_table_at(&changed, 1).unwrap().cells, table.cells);
        assert!(!changed.contains("{ccc}"));
        for injection in [r"a & b", r"a\\b", r"a\end{pmatrix} outside"] {
            let mut bad = table.clone();
            bad.cells.as_mut().unwrap()[0][0] = injection.into();
            assert!(bad.grid_source_edit().is_err(), "{injection}");
        }
    }

    #[test]
    fn math_grid_resize_retains_formulas_and_never_writes_an_array_spec_into_matrices() {
        for name in [
            "array", "pmatrix", "pmatrix*", "aligned", "gathered", "cases",
        ] {
            let spec = if name == "array" {
                "{lr}"
            } else if name.ends_with('*') {
                "[r]"
            } else {
                ""
            };
            let body = if name == "gathered" {
                r"\alpha\\\beta"
            } else {
                r"\alpha & \frac{1}{2}\\\beta & x^2"
            };
            let source = format!(r"\begin{{{name}}}{spec}{body}\end{{{name}}}");
            let mut table = tex_table_at(&source, 1).unwrap();
            let columns = table.columns().unwrap();
            table.resize_math_grid(2, columns).unwrap();
            assert_eq!(table.render_replacement().unwrap(), source);
            let new_columns = match name {
                "gathered" => 1,
                "cases" => 2,
                _ => 4,
            };
            table.resize_math_grid(3, new_columns).unwrap();
            assert_eq!(table.cells.as_ref().unwrap().len(), 3, "{name}");
            assert_eq!(table.columns(), Some(new_columns), "{name}");
            assert_eq!(table.cells.as_ref().unwrap()[0][0], r"\alpha");
            let rendered = table.grid_source_edit().unwrap().replacement;
            assert_eq!(tex_table_at(&rendered, 1).unwrap().cells, table.cells);
            if name == "array" {
                assert!(rendered.contains("{lrcc}"));
            } else {
                assert!(!rendered.contains("{cccc}"));
            }
            table
                .resize_math_grid(1, if name == "gathered" { 1 } else { 2 })
                .unwrap();
            assert_eq!(table.cells.as_ref().unwrap().len(), 1);
        }
    }

    #[test]
    fn math_grid_dimension_and_layout_errors_leave_the_draft_unchanged() {
        for (source, rows, columns) in [
            (r"\begin{cases}x&y\end{cases}", 3, 3),
            (r"\begin{gathered}x\\y\end{gathered}", 3, 2),
            (r"\begin{aligned}x&y\end{aligned}", 3, 3),
            (r"\begin{pmatrix}x&y\end{pmatrix}", 1, 11),
            (r"\begin{array}{|cc|}x&y\end{array}", 2, 3),
            (r"\begin{pmatrix}x&y\\[2pt]z&w\end{pmatrix}", 3, 2),
            ("\\begin{pmatrix}% do not lose me\nx&y\\end{pmatrix}", 2, 2),
            (r"\begin{pmatrix}x&y\end{pmatrix}", usize::MAX, 128),
        ] {
            let mut table = tex_table_at(source, 1).unwrap();
            let old = table.clone();
            assert!(table.resize_math_grid(rows, columns).is_err(), "{source}");
            assert_eq!(table, old, "{source}");
        }
        let mut table =
            tex_table_at("\\begin{matrix}a % cell comment\n & b\\end{matrix}", 1).unwrap();
        table.resize_math_grid(2, 2).unwrap();
        assert!(
            table
                .render_replacement()
                .unwrap()
                .contains("% cell comment\n &")
        );
    }

    #[test]
    fn math_enclosure_conversion_preserves_math_wrappers_options_and_comments() {
        let source = "prefix $\\begin {pmatrix} a & b\\\\ c & d\\end % keep\n{pmatrix}$ suffix";
        let cursor = source.find(" a &").unwrap();
        let mut table = tex_table_at(source, cursor).unwrap();
        table.set_math_environment("bmatrix").unwrap();
        let edit = table.grid_source_edit().unwrap();
        let range = source.find("\\begin").unwrap()..source.rfind('$').unwrap();
        assert_eq!(
            edit.replacement,
            source[range.clone()].replace("pmatrix", "bmatrix")
        );
        assert_eq!(edit.range, byte_range_to_char(source, range));
        let source = "\\begin{array} % retain header\n{lr}a&b\\end{array}";
        let mut table = tex_table_at(source, 1).unwrap();
        table.set_math_environment("Bmatrix").unwrap();
        assert!(
            table
                .render_replacement()
                .unwrap()
                .contains("% retain header\na&b")
        );
        table.set_math_environment("array").unwrap();
        assert!(table.render_replacement().unwrap().contains("{cc}"));
        let mut table = tex_table_at(r"\begin{pmatrix*}[r]a&b\end{pmatrix*}", 1).unwrap();
        table.set_math_environment("bmatrix*").unwrap();
        assert!(table.render_replacement().unwrap().contains("[r]"));
        let old = table.clone();
        assert!(table.set_math_environment("bmatrix").is_err());
        assert_eq!(table, old);
        let mut table = tex_table_at(r"\begin{matrix}[r]a & b\end{matrix}", 1).unwrap();
        let old = table.clone();
        assert!(table.set_math_environment("pmatrix*").is_err());
        assert_eq!(table, old);
    }

    #[test]
    fn malformed_math_options_nested_grids_and_oversized_widths_are_rejected() {
        for source in [
            r"\begin{pmatrix*}[cc]a&b\end{pmatrix*}",
            r"\begin{aligned}[z]a&b\end{aligned}",
            r"\begin{cases}a&b&c\end{cases}",
            r"\begin{gathered}a&b\end{gathered}",
            r"\begin{pmatrix}\begin{matrix}a&b\end{other}\end{pmatrix}",
        ] {
            assert!(tex_table_at(source, 1).is_none(), "{source}");
        }
        let source = format!(r"\begin{{matrix}}{}\end{{matrix}}", "a&".repeat(128));
        assert!(tex_table_at(&source, 1).is_none());
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
    fn literal_payloads_cannot_offer_a_table_editor_or_fake_packages() {
        let fake = r"\begin{tabular}{l}Fake\end{tabular}";
        let real = r"\begin{tabular}{l}Real\end{tabular}";
        let mut prefixes = vec![
            format!(r"\Verb|{fake}|"),
            format!(r"\lstinline[language=TeX]§{fake}§"),
            format!(r"\lstinline{{{fake}}}"),
            format!(r"\mintinline{{tex}}|{fake}|"),
            format!(r"\mintinline{{tex}}{{{fake}}}"),
        ];
        for env in [
            "comment",
            "Verbatim",
            "verbatim",
            "lstlisting",
            "minted",
            "filecontents",
            "filecontents*",
        ] {
            prefixes.push(format!(
                "\\begin{{{env}}}\n{fake}\n\\usepackage{{fake}}\n\\end{{{env}}}\n"
            ));
        }
        for prefix in prefixes {
            let source = format!("{prefix}\n{real}");
            let fake_cursor = source[..source.find("Fake").unwrap()].chars().count();
            assert!(tex_table_at(&source, fake_cursor).is_none(), "{prefix}");
            let real_cursor = source[..source.find("Real").unwrap()].chars().count();
            let table = tex_table_at(&source, real_cursor).unwrap();
            assert_eq!(table.cells.as_ref().unwrap()[0], ["Real"]);
            assert!(table.packages.is_empty(), "{prefix}");
        }
    }

    #[test]
    fn literal_cells_keep_separators_comments_braces_and_packages_opaque() {
        for payload in [
            r"\verb|& \\ % { \usepackage{fake}|",
            r"\Verb|& \\ % { \usepackage{fake}|",
            r"\lstinline[language=TeX]|& \\ % { \usepackage{fake}|",
            r"\lstinline{& \\ \usepackage{fake}}",
            r"\mintinline{tex}|& \\ % { \usepackage{fake}|",
            r"\mintinline{tex}{& \\ \usepackage{fake}}",
            "\\begin{comment}\n& \\\\ % { \\usepackage{fake}\n\\end{comment}\n",
            "\\begin{Verbatim}\n& \\\\ % { \\usepackage{fake}\n\\end{Verbatim}\n",
        ] {
            let source = format!("\\begin{{tabular}}{{ll}}{payload} & Real\\\\\n\\end{{tabular}}");
            let mut table = tex_table_at(&source, 1).unwrap_or_else(|| panic!("{payload}"));
            assert_eq!(table.cells.as_ref().unwrap().len(), 1, "{payload}");
            assert_eq!(table.cells.as_ref().unwrap()[0][1], "Real", "{payload}");
            assert!(table.packages.is_empty(), "{payload}");
            assert_eq!(table.grid_source_edit().unwrap().replacement, source);
            table.cells.as_mut().unwrap()[0][1] = "Changed".into();
            let edit = table.grid_source_edit().unwrap();
            assert!(edit.replacement.contains(payload.trim()), "{payload}");
            assert!(edit.replacement.contains("Changed"));
        }
    }

    #[test]
    fn raw_environment_closers_must_be_standalone_and_unclosed_inline_literals_stop_at_newline() {
        for env in ["comment", "verbatim", "Verbatim", "filecontents"] {
            let source = format!(
                "\\begin{{{env}}}\nexample \\end{{{env}}} inline\n\\begin{{tabular}}{{l}}Fake\\end{{tabular}}\n\\end{{{env}}}\n\\begin{{tabular}}{{l}}Real\\end{{tabular}}"
            );
            let fake_cursor = source[..source.find("Fake").unwrap()].chars().count();
            assert!(tex_table_at(&source, fake_cursor).is_none(), "{env}");
            let real_cursor = source[..source.find("Real").unwrap()].chars().count();
            assert!(tex_table_at(&source, real_cursor).is_some(), "{env}");
        }
        for command in ["verb", "Verb", "lstinline", "mintinline{tex}"] {
            let source =
                format!("\\{command}|unclosed\n\\begin{{tabular}}{{l}}Real\\end{{tabular}}");
            let cursor = source[..source.find("Real").unwrap()].chars().count();
            assert!(tex_table_at(&source, cursor).is_some(), "{command}");
        }
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
