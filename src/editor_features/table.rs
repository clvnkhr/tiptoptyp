//! Bounded, loss-safe table geometry and Markdown conversion. No UI or I/O.
use super::*;
use std::collections::BTreeMap;

pub(crate) type CellPosition = (usize, usize);
type CellMatrix = Vec<Vec<String>>;
type CellOptionMap = BTreeMap<CellPosition, CellOptions>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CellOptions {
    pub(super) colspan: usize,
    pub(super) rowspan: usize,
    pub(super) options: Vec<PreservedTableOption>,
}

impl Default for CellOptions {
    fn default() -> Self {
        Self {
            colspan: 1,
            rowspan: 1,
            options: Vec::new(),
        }
    }
}

pub(super) fn parse_cell(
    node: &LinkedNode<'_>,
    source: &str,
    kind: TableKind,
) -> Option<(String, CellOptions)> {
    if node.kind() == SyntaxKind::ContentBlock {
        if has_dynamic_content(node) {
            return None;
        }
        let range = node.range();
        return Some((
            source.get(range.start + 1..range.end - 1)?.into(),
            CellOptions::default(),
        ));
    }
    let call = node.get().cast::<ast::FuncCall>()?;
    let ast::Expr::FieldAccess(access) = call.callee() else {
        return None;
    };
    let ast::Expr::Ident(target) = access.target() else {
        return None;
    };
    if target.as_str() != kind.callee() || access.field().as_str() != "cell" {
        return None;
    }
    let args = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Args)?;
    let mut options = CellOptions::default();
    let mut body = None;
    let mut names = std::collections::BTreeSet::new();
    for argument in args.children() {
        match argument.get().cast::<ast::Arg>() {
            Some(ast::Arg::Named(named)) => {
                let name = named.name().as_str();
                if !names.insert(name.to_owned()) {
                    return None;
                }
                match name {
                    "colspan" | "rowspan" => {
                        let ast::Expr::Int(value) = named.expr() else {
                            return None;
                        };
                        let value = usize::try_from(value.get())
                            .ok()
                            .filter(|v| *v > 0 && *v <= 4096)?;
                        if name == "colspan" {
                            options.colspan = value;
                        } else {
                            options.rowspan = value;
                        }
                    }
                    // Explicit placement/structural expressions stay source-only.
                    "x" | "y" | "body" => return None,
                    _ => options.options.push(PreservedTableOption {
                        name: name.into(),
                        source: source.get(argument.range())?.into(),
                    }),
                }
            }
            Some(ast::Arg::Pos(ast::Expr::ContentBlock(_))) if body.is_none() => {
                body = Some(parse_cell(&argument, source, kind)?.0);
            }
            Some(_) => return None,
            None => {}
        }
    }
    Some((body?, options))
}

pub(super) fn place_cells(
    columns: usize,
    flat: Vec<(String, CellOptions)>,
) -> Option<(CellMatrix, CellOptionMap)> {
    if columns == 0 || columns > 128 || flat.len() > 4096 {
        return None;
    }
    let mut cells: CellMatrix = Vec::new();
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let mut properties = BTreeMap::new();
    let mut next = 0;
    for (body, options) in flat {
        while next < occupied.len() * columns && occupied[next / columns][next % columns] {
            next += 1;
        }
        let (r, c) = (next / columns, next % columns);
        if c + options.colspan > columns || (r + options.rowspan).saturating_mul(columns) > 4096 {
            return None;
        }
        while occupied.len() < r + options.rowspan {
            occupied.push(vec![false; columns]);
            cells.push(vec![String::new(); columns]);
        }
        for row in occupied.iter_mut().skip(r).take(options.rowspan) {
            for slot in row.iter_mut().skip(c).take(options.colspan) {
                if *slot {
                    return None;
                }
                *slot = true;
            }
        }
        cells[r][c] = body;
        if options != CellOptions::default() {
            properties.insert((r, c), options);
        }
    }
    if occupied.iter().flatten().any(|slot| !slot) {
        return None;
    }
    Some((cells, properties))
}

impl EditableTable {
    pub(crate) fn can_add_row(&self) -> bool {
        (self.row_count() + 1).saturating_mul(self.columns) <= 4096
    }
    pub(crate) fn can_add_column(&self) -> bool {
        self.columns < 128 && self.row_count().saturating_mul(self.columns + 1) <= 4096
    }

    pub(crate) fn cell_owners(&self) -> Result<Vec<Vec<CellPosition>>, TableEditError> {
        if self.columns == 0
            || self.columns > 128
            || self.row_count().saturating_mul(self.columns) > 4096
        {
            return Err(TableEditError::TooLarge);
        }
        let mut owners: Vec<Vec<_>> = (0..self.row_count())
            .map(|r| (0..self.columns).map(|c| (r, c)).collect())
            .collect();
        for (&(r, c), options) in &self.cell_options {
            if options.rowspan == 0
                || options.colspan == 0
                || options.rowspan > self.row_count().saturating_sub(r)
                || options.colspan > self.columns.saturating_sub(c)
            {
                return Err(TableEditError::InvalidSpan);
            }
            for (y, row) in owners.iter_mut().enumerate().skip(r).take(options.rowspan) {
                for (x, owner) in row.iter_mut().enumerate().skip(c).take(options.colspan) {
                    if *owner != (y, x) {
                        return Err(TableEditError::InvalidSpan);
                    }
                    if (y, x) != (r, c)
                        && (self
                            .cells
                            .get(y)
                            .and_then(|row| row.get(x))
                            .is_none_or(|body| !body.is_empty())
                            || self.cell_options.contains_key(&(y, x)))
                    {
                        return Err(TableEditError::OccupiedSpan);
                    }
                    *owner = (r, c);
                }
            }
        }
        Ok(owners)
    }

    pub(crate) fn span(&self, cell: CellPosition) -> (usize, usize) {
        self.cell_options
            .get(&cell)
            .map_or((1, 1), |options| (options.rowspan, options.colspan))
    }

    pub(crate) fn set_span(
        &mut self,
        cell: CellPosition,
        rows: usize,
        columns: usize,
    ) -> Result<(), TableEditError> {
        let old = self.cell_options.get(&cell).cloned();
        let options = self.cell_options.entry(cell).or_default();
        options.rowspan = rows;
        options.colspan = columns;
        if let Err(error) = self.cell_owners() {
            if let Some(old) = old {
                self.cell_options.insert(cell, old);
            } else {
                self.cell_options.remove(&cell);
            }
            return Err(error);
        }
        self.prune_cell_options();
        Ok(())
    }

    fn prune_cell_options(&mut self) {
        self.cell_options
            .retain(|_, options| *options != CellOptions::default());
    }

    pub(crate) fn style(&self, cell: Option<CellPosition>, name: &str) -> Option<&str> {
        let options = match cell {
            Some(cell) => &self.cell_options.get(&cell)?.options,
            None => &self.options,
        };
        options
            .iter()
            .find(|option| option.name == name)?
            .source
            .split_once(':')
            .map(|(_, value)| value.trim())
    }

    pub(crate) fn set_style(
        &mut self,
        cell: Option<CellPosition>,
        name: &str,
        value: Option<&str>,
    ) {
        debug_assert!(matches!(name, "fill" | "stroke" | "align" | "inset"));
        let options = match cell {
            Some(cell) => &mut self.cell_options.entry(cell).or_default().options,
            None => &mut self.options,
        };
        options.retain(|option| option.name != name);
        if let Some(value) = value {
            options.push(PreservedTableOption {
                name: name.into(),
                source: format!("{name}: {value}"),
            });
        }
        self.prune_cell_options();
    }

    /// Replace the draft only after a complete, bounded GFM table is parsed.
    /// Cell contents are imported as escaped plain text; Markdown emphasis is
    /// retained. Links/images/HTML are refused rather than silently discarded.
    pub(crate) fn import_markdown(&mut self, markdown: &str) -> Result<(), String> {
        use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
        if markdown.len() > 1_048_576 {
            return Err("Markdown input is limited to 1 MiB".into());
        }
        let mut alignments = None;
        let mut cells: Vec<Vec<String>> = Vec::new();
        let mut row = Vec::new();
        let mut cell = None::<String>;
        let mut in_table = false;
        for event in Parser::new_ext(markdown, Options::ENABLE_TABLES) {
            match event {
                Event::Start(Tag::Table(align)) => {
                    if alignments.is_some() || align.is_empty() || align.len() > 128 { return Err("Paste exactly one Markdown table (up to 128 columns)".into()); }
                    alignments = Some(align); in_table = true;
                }
                Event::End(TagEnd::Table) => in_table = false,
                Event::Start(Tag::TableCell) => cell = Some(String::new()),
                Event::End(TagEnd::TableCell) => row.push(cell.take().ok_or("Invalid Markdown cell")?),
                Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                    cells.push(std::mem::take(&mut row));
                    if cells.len().saturating_mul(alignments.as_ref().map_or(0, Vec::len)) > 4096 { return Err(TableEditError::TooLarge.to_string()); }
                }
                Event::Text(text) | Event::Code(text) if cell.is_some() => escape_markup(cell.as_mut().unwrap(), &text),
                Event::Start(Tag::Emphasis) | Event::End(TagEnd::Emphasis) if cell.is_some() => cell.as_mut().unwrap().push('_'),
                Event::Start(Tag::Strong) | Event::End(TagEnd::Strong) if cell.is_some() => cell.as_mut().unwrap().push('*'),
                Event::SoftBreak | Event::HardBreak if cell.is_some() => cell.as_mut().unwrap().push(' '),
                Event::Start(Tag::TableHead | Tag::TableRow) => {}
                _ => return Err(if in_table { "Use text, emphasis, and inline code in imported cells; links, images and HTML are not supported" } else { "Paste only a Markdown table, without surrounding prose or a code fence" }.into()),
            }
        }
        let alignments = alignments
            .ok_or("No Markdown table found. Include a header separator such as | --- | --- |")?;
        let mut candidate = self.clone();
        candidate.columns = alignments.len();
        candidate.cells = cells;
        candidate.cell_options.clear();
        for r in 0..candidate.row_count() {
            for (c, alignment) in alignments.iter().enumerate() {
                let value = match alignment {
                    Alignment::None => None,
                    Alignment::Left => Some("left"),
                    Alignment::Center => Some("center"),
                    Alignment::Right => Some("right"),
                };
                if value.is_some() {
                    candidate.set_style(Some((r, c)), "align", value);
                }
            }
        }
        candidate.validate().map_err(|error| error.to_string())?;
        *self = candidate;
        Ok(())
    }
}

fn escape_markup(output: &mut String, text: &str) {
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '['
                | ']'
                | '#'
                | '$'
                | '*'
                | '_'
                | '`'
                | '@'
                | '<'
                | '>'
                | '='
                | '+'
                | '-'
                | '/'
                | '~'
                | ':'
        ) {
            output.push('\\');
        }
        output.push(ch);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TableSelection {
    pub(crate) anchor: CellPosition,
    pub(crate) focus: CellPosition,
}

impl TableSelection {
    pub(crate) fn contains(self, (r, c): CellPosition) -> bool {
        (self.anchor.0.min(self.focus.0)..=self.anchor.0.max(self.focus.0)).contains(&r)
            && (self.anchor.1.min(self.focus.1)..=self.anchor.1.max(self.focus.1)).contains(&c)
    }
    pub(crate) fn select(&mut self, cell: CellPosition, extend: bool) {
        self.focus = cell;
        if !extend {
            self.anchor = cell;
        }
    }
    pub(crate) fn clamp(&mut self, rows: usize, columns: usize) {
        for cell in [&mut self.anchor, &mut self.focus] {
            cell.0 = cell.0.min(rows.saturating_sub(1));
            cell.1 = cell.1.min(columns.saturating_sub(1));
        }
    }
    pub(crate) fn select_row(&mut self, row: usize, columns: usize) {
        self.anchor = (row, 0);
        self.focus = (row, columns.saturating_sub(1));
    }
    pub(crate) fn select_column(&mut self, column: usize, rows: usize) {
        self.anchor = (0, column);
        self.focus = (rows.saturating_sub(1), column);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(source: &str) -> EditableTable {
        editable_table_at(source, 2).unwrap()
    }
    #[test]
    fn spans_round_trip_and_expansion_never_discards_content() {
        let mut table = parse("#table(columns: 3, [Title], [], [B], [], [], [C])");
        table.set_span((0, 0), 2, 2).unwrap();
        assert_eq!(table.cell_owners().unwrap()[1][1], (0, 0));
        let rendered = format!("#{}", table.render_replacement().unwrap());
        let parsed = parse(&rendered);
        assert_eq!(table.cells, parsed.cells);
        assert_eq!(table.cell_options, parsed.cell_options);
        let before = table.clone();
        assert_eq!(
            table.set_span((0, 0), 2, 3),
            Err(TableEditError::OccupiedSpan)
        );
        assert_eq!(table, before);
        table.set_span((0, 0), 1, 1).unwrap();
        table.cells[1][1] = "Now editable".into();
        assert!(table.render_replacement().unwrap().contains("Now editable"));
    }
    #[test]
    fn spans_survive_dimension_changes_and_styling() {
        let mut table =
            parse("#grid(columns: (1fr, 2fr), grid.cell(rowspan: 2, fill: red)[A], [], [])");
        table.set_style(None, "stroke", Some("0.5pt"));
        table.set_style(Some((0, 1)), "align", Some("right"));
        table.add_column();
        table.add_row();
        table.remove_row(1);
        table.remove_column();
        assert_eq!(table.span((0, 0)), (1, 1));
        let rendered = table.render_replacement().unwrap();
        assert!(rendered.contains("fill: red"));
        assert!(rendered.contains("columns: (1fr, 2fr)"));
        assert_eq!(parse(&format!("#{rendered}")).cells, table.cells);
        table.remove_row(0);
        assert!(table.cell_options.is_empty());
    }
    #[test]
    fn bounded_geometry_rejects_overlap_and_unsupported_placement() {
        for source in [
            "#table(columns: 999999999, [A])",
            "#table(table.cell(rowspan: 999999)[A])",
            "#table(table.cell(x: 0)[A])",
            "#table(table.cell(colspan: 0)[A])",
        ] {
            assert!(editable_table_at(source, 2).is_none());
        }
        let mut table = parse("#table(columns: 2, [], [], [], [])");
        table.set_span((0, 1), 2, 1).unwrap();
        assert_eq!(
            table.set_span((0, 0), 1, 2),
            Err(TableEditError::OccupiedSpan)
        );
        assert_eq!(
            table.set_span((0, 0), usize::MAX, 1),
            Err(TableEditError::InvalidSpan)
        );
    }
    #[test]
    fn markdown_import_is_atomic_escaped_and_preserves_alignment() {
        let mut table = parse("#table(inset: 8pt, [Old])");
        table
            .import_markdown(
                "| Name | Value |\n| :--- | ---: |\n| **Héllo** | #value $x$ \\| ok |\n",
            )
            .unwrap();
        assert_eq!(table.columns, 2);
        assert_eq!(table.row_count(), 2);
        assert_eq!(table.style(Some((1, 1)), "align"), Some("right"));
        assert_eq!(table.cells[1][1], "\\#value \\$x\\$ | ok");
        let output = table.render_replacement().unwrap();
        assert!(output.contains("inset: 8pt"));
        assert_eq!(parse(&format!("#{output}")).cells, table.cells);
        let before = table.clone();
        assert!(
            table
                .import_markdown("| A |\n| --- |\n| [link](https://example.com) |")
                .is_err()
        );
        assert_eq!(table, before);
        assert!(table.import_markdown("not a table").is_err());
        assert_eq!(table, before);
    }
    #[test]
    fn selection_ranges_work_backwards_and_clamp_after_removal() {
        let mut selection = TableSelection::default();
        selection.select((3, 4), false);
        selection.select((1, 2), true);
        assert!(selection.contains((2, 3)));
        assert!(!selection.contains((0, 3)));
        selection.clamp(2, 3);
        assert_eq!(selection.focus, (1, 2));
        assert_eq!(selection.anchor, (1, 2));
        selection.select_row(1, 4);
        assert!(selection.contains((1, 3)));
        selection.select_column(2, 3);
        assert!(selection.contains((2, 2)));
    }

    #[test]
    fn table_insertion_uses_the_syntax_mode_and_rejects_literals() {
        for (source, cursor, prefix) in [
            ("", 0, Some("#")),
            ("Hello ", 6, Some("#")),
            ("#let t = ", 9, Some("")),
            ("#", 1, Some("")),
            ("#block[ ]", 7, Some("#")),
            ("#let t = \"hello\"", 12, None),
            ("`code`", 3, None),
            ("// comment", 5, None),
        ] {
            assert_eq!(
                table_insertion_prefix(source, cursor),
                prefix,
                "{source:?} at {cursor}"
            );
        }
    }

    #[test]
    #[ignore = "opt-in compiler integration; set TIPTOPTYP_TEST_TYPST"]
    fn table_generated_spans_and_markdown_compile_with_typst() {
        let compiler = std::env::var_os("TIPTOPTYP_TEST_TYPST").expect("Typst executable");
        let directory = tempfile::tempdir().unwrap();
        let mut spanned = parse("#table(columns: 3, [Heading], [], [], [A], [], [B], [], [], [C])");
        spanned.set_span((0, 0), 1, 3).unwrap();
        spanned.set_span((1, 0), 2, 2).unwrap();
        spanned.set_style(None, "stroke", Some("0.5pt"));
        spanned.set_style(None, "inset", Some("8pt"));
        spanned.set_style(Some((0, 0)), "fill", Some("rgb(\"#dbeafe\")"));
        spanned.set_style(Some((0, 0)), "align", Some("center + horizon"));
        let mut markdown = parse("#table([Old])");
        markdown.import_markdown("| **Name** | Value |\n| :--- | ---: |\n| Héllo | #value $x$ [brackets] + @ref \\<label> \\| _text_ |\n").unwrap();
        let source = format!(
            "#{}\n\n#{}",
            spanned.render_replacement().unwrap(),
            markdown.render_replacement().unwrap()
        );
        let input = directory.path().join("table.typ");
        std::fs::write(&input, source).unwrap();
        let output = std::process::Command::new(compiler)
            .arg("compile")
            .arg(&input)
            .arg(directory.path().join("table.pdf"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
