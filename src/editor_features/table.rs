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
        if has_unsupported_cell_content(node) {
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
        candidate.rules.clear();
        candidate.rules_revision = candidate.rules_revision.wrapping_add(1);
        candidate.reset_column_widths();
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

/// Rules do not occupy cells. Preserve their source, ordering and explicit
/// coordinates; only implicit horizontal rules depend on the placement cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableAxis {
    Horizontal,
    Vertical,
}

impl TableAxis {
    fn helper(self) -> &'static str {
        match self {
            Self::Horizontal => "hline",
            Self::Vertical => "vline",
        }
    }
    fn coordinate(self) -> &'static str {
        match self {
            Self::Horizontal => "y",
            Self::Vertical => "x",
        }
    }
}

pub(super) fn rule_axis(node: &LinkedNode<'_>, kind: TableKind) -> Option<TableAxis> {
    let call = node.get().cast::<ast::FuncCall>()?;
    let ast::Expr::FieldAccess(access) = call.callee() else {
        return None;
    };
    let ast::Expr::Ident(target) = access.target() else {
        return None;
    };
    if target.as_str() != kind.callee() {
        return None;
    }
    match access.field().as_str() {
        "hline" => Some(TableAxis::Horizontal),
        "vline" => Some(TableAxis::Vertical),
        _ => None,
    }
}

pub(super) fn rule_has_explicit_position(node: &LinkedNode<'_>, axis: TableAxis) -> bool {
    let Some(call) = node.get().cast::<ast::FuncCall>() else {
        return false;
    };
    call.args().items().any(|arg| matches!(arg, ast::Arg::Named(named) if named.name().as_str() == axis.coordinate() && !matches!(named.expr(), ast::Expr::Auto(_))))
}

pub(super) fn parse_column_widths(argument: &LinkedNode<'_>, source: &str) -> Option<Vec<String>> {
    let array = argument
        .children()
        .find(|node| node.kind() == SyntaxKind::Array)?;
    array
        .children()
        .filter(|node| node.get().cast::<ast::ArrayItem>().is_some())
        .map(|node| {
            let ast::ArrayItem::Pos(_) = node.get().cast::<ast::ArrayItem>()? else {
                return None;
            };
            Some(source.get(node.range())?.to_owned())
        })
        .collect()
}

impl EditableTable {
    pub(crate) fn set_column_widths(&mut self, widths: &[String]) -> Result<(), String> {
        if widths.len() != self.columns {
            return Err(format!(
                "Enter one width for each of the {} columns",
                self.columns
            ));
        }
        for width in widths {
            let source = format!("#table(columns: ({width},))");
            let parsed = Source::detached(source);
            let root = LinkedNode::new(parsed.root());
            let valid = !has_descendant_kind(&root, SyntaxKind::Error)
                && find_first_kind(&root, SyntaxKind::FuncCall)
                    .and_then(|call| {
                        let args = call
                            .children()
                            .find(|node| node.kind() == SyntaxKind::Args)?;
                        let named = args.children().find(|node| {
                            matches!(node.get().cast::<ast::Arg>(), Some(ast::Arg::Named(_)))
                        })?;
                        let list = parse_column_widths(&named, parsed.text())?;
                        (list.len() == 1 && list[0].trim() == width.trim()).then_some(())
                    })
                    .is_some();
            if !valid {
                return Err("Each column width must be one Typst expression, for example auto, 25%, 2cm or 1fr".into());
            }
        }
        self.column_widths = Some(widths.to_vec());
        self.write_column_widths();
        Ok(())
    }

    pub(crate) fn reset_column_widths(&mut self) {
        self.column_widths = None;
        self.options.retain(|option| option.name != "columns");
        self.original_columns = self.columns;
        self.columns_were_explicit = self.columns > 1;
        if self.columns > 1 {
            self.options.push(PreservedTableOption {
                name: "columns".into(),
                source: format!("columns: {}", self.columns),
            });
        }
    }

    pub(super) fn write_column_widths(&mut self) {
        let Some(widths) = &self.column_widths else {
            return;
        };
        let values = widths.join(", ");
        let source = format!(
            "columns: ({values}{})",
            if widths.len() == 1 { "," } else { "" }
        );
        if let Some(option) = self
            .options
            .iter_mut()
            .find(|option| option.name == "columns")
        {
            option.source = source;
        } else {
            self.options.insert(
                0,
                PreservedTableOption {
                    name: "columns".into(),
                    source,
                },
            );
        }
        self.original_columns = self.columns;
        self.columns_were_explicit = true;
    }

    /// Stroke source for a simple full edge. A partial/dynamic rule remains
    /// available in source mode and is never silently converted to a full edge.
    pub(crate) fn boundary_rule(&self, axis: TableAxis, boundary: usize) -> Option<String> {
        self.rules.iter().rev().find_map(|(at, source)| {
            let rule = rule_details(source, self.kind, *at, self.row_count())?;
            (rule.axis == axis && rule.boundary == Some(boundary) && rule.simple)
                .then(|| rule.stroke.unwrap_or_else(|| "1pt + black".into()))
        })
    }

    pub(crate) fn boundary_rules_key(&self) -> (u64, usize, usize) {
        (self.rules_revision, self.row_count(), self.columns)
    }

    pub(crate) fn boundary_rules(&self) -> Vec<(TableAxis, usize, String)> {
        self.rules
            .iter()
            .filter_map(|(at, source)| {
                let rule = rule_details(source, self.kind, *at, self.row_count())?;
                if !rule.simple {
                    return None;
                }
                Some((
                    rule.axis,
                    rule.boundary?,
                    rule.stroke.unwrap_or_else(|| "1pt + black".into()),
                ))
            })
            .collect()
    }

    pub(crate) fn set_boundary_rule(
        &mut self,
        axis: TableAxis,
        boundary: usize,
        stroke: Option<&str>,
    ) -> Result<(), String> {
        let limit = if axis == TableAxis::Horizontal {
            self.row_count()
        } else {
            self.columns
        };
        if boundary > limit {
            return Err("Select an edge inside the table".into());
        }
        let mut indexes = Vec::new();
        for (index, (at, source)) in self.rules.iter().enumerate() {
            let Some(rule) = rule_details(source, self.kind, *at, self.row_count()) else {
                continue;
            };
            if rule.axis == axis && rule.boundary == Some(boundary) {
                if !rule.simple {
                    return Err("This edge has partial or positioned rules. Edit them in table source to retain their options".into());
                }
                indexes.push(index);
            }
        }
        let replacement = stroke.map(|stroke| {
            format!(
                "{}.{}({}: {boundary}, stroke: {stroke})",
                self.kind.callee(),
                axis.helper(),
                axis.coordinate()
            )
        });
        if let Some(source) = &replacement {
            let parsed = Source::detached(format!("#{source}"));
            let root = LinkedNode::new(parsed.root());
            let valid = find_first_kind(&root, SyntaxKind::FuncCall).is_some_and(|node| {
                if node.range() != (1..source.len() + 1) {
                    return false;
                }
                let Some(args) = node
                    .children()
                    .find(|child| child.kind() == SyntaxKind::Args)
                else {
                    return false;
                };
                let arguments: Vec<_> = args
                    .children()
                    .filter_map(|child| child.get().cast::<ast::Arg>().map(|arg| (child, arg)))
                    .collect();
                arguments.len() == 2
                    && arguments.iter().any(|(child, arg)| {
                        matches!(arg, ast::Arg::Named(named) if named.name().as_str() == "stroke")
                            && parsed
                                .text()
                                .get(child.range())
                                .and_then(|raw| raw.split_once(':'))
                                .is_some_and(|(_, value)| {
                                    Some(value.trim()) == stroke.map(str::trim)
                                })
                    })
            });
            if has_descendant_kind(&root, SyntaxKind::Error) || !valid {
                return Err(
                    "Enter one Typst stroke expression, for example none, red or 0.5pt + blue"
                        .into(),
                );
            }
        }
        for index in indexes.into_iter().rev() {
            self.rules.remove(index);
        }
        if let Some(source) = replacement {
            self.rules.push((usize::MAX, source));
        }
        self.rules_revision = self.rules_revision.wrapping_add(1);
        Ok(())
    }
}

struct RuleDetails {
    axis: TableAxis,
    boundary: Option<usize>,
    stroke: Option<String>,
    simple: bool,
}
fn rule_details(source: &str, kind: TableKind, at: usize, rows: usize) -> Option<RuleDetails> {
    let wrapper = format!("#{source}");
    let parsed = Source::detached(&wrapper);
    let root = LinkedNode::new(parsed.root());
    let node = find_first_kind(&root, SyntaxKind::FuncCall)?;
    let axis = rule_axis(&node, kind)?;
    let call = node.get().cast::<ast::FuncCall>()?;
    let mut result = RuleDetails {
        axis,
        boundary: (axis == TableAxis::Horizontal).then_some(if at == usize::MAX {
            rows
        } else {
            at
        }),
        stroke: None,
        simple: true,
    };
    for argument in node
        .children()
        .find(|child| child.kind() == SyntaxKind::Args)?
        .children()
    {
        let Some(arg) = argument.get().cast::<ast::Arg>() else {
            continue;
        };
        let ast::Arg::Named(named) = arg else {
            result.simple = false;
            continue;
        };
        let value = wrapper.get(argument.range())?.split_once(':')?.1.trim();
        match named.name().as_str() {
            name if name == axis.coordinate() => {
                result.boundary = match named.expr() {
                    ast::Expr::Int(value) => usize::try_from(value.get()).ok(),
                    ast::Expr::Auto(_) => result.boundary,
                    _ => None,
                };
            }
            "stroke" => result.stroke = Some(value.to_owned()),
            _ => result.simple = false,
        }
    }
    // Keep malformed future helper signatures source-only.
    if call.args().items().count() == 0 && axis == TableAxis::Vertical {
        result.simple = false;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(source: &str) -> EditableTable {
        editable_table_at(source, 2).unwrap()
    }
    #[test]
    fn grid_apply_rejects_cells_that_escape_their_content_block() {
        let mut table = parse("#table(columns: 2, [A], [B])");
        table.cells[0][0] = "A])\n#table([injected".into();
        assert!(table.render_replacement().is_err());
    }

    #[test]
    fn grid_apply_rejects_style_arguments_that_inject_other_options() {
        let mut table = parse("#table(columns: 2, [A], [B])");
        table.set_style(None, "fill", Some("red, columns: 1"));
        assert!(table.render_replacement().is_err());
        table.set_style(None, "fill", Some("red)\n#table(columns: 2"));
        assert!(table.render_replacement().is_err());
        table.set_style(None, "fill", None);
        table.set_style(Some((0, 0)), "stroke", Some("none, colspan: 2"));
        assert!(table.render_replacement().is_err());
    }

    #[test]
    fn grid_apply_preserves_nested_markup_and_compound_option_expressions() {
        let mut table = parse("#table(columns: (20%, auto), [A], [B])");
        table.cells[0][0] = "#strong[Hello] $x_1$ and \\[brackets\\]".into();
        table.set_style(
            None,
            "fill",
            Some("(x, y) => if y == 0 { red } else { none }"),
        );
        table.set_style(
            Some((0, 0)),
            "stroke",
            Some("(top: 1pt + blue, rest: none)"),
        );
        let rendered = table.render_replacement().unwrap();
        let reparsed = parse(&format!("#{rendered}"));
        assert_eq!(reparsed.cells, table.cells);
        assert_eq!(reparsed.style(None, "fill"), table.style(None, "fill"));
        assert_eq!(
            reparsed.style(Some((0, 0)), "stroke"),
            table.style(Some((0, 0)), "stroke")
        );
    }

    #[test]
    fn source_drafts_preserve_dynamic_cells_and_incomplete_options_for_repair() {
        let mut table = editable_table_at("#table(columns: 2, [A], [B])", 2).unwrap();
        table.cells[0][0] = "#let value = 1; #value".into();
        assert!(table.render_replacement().is_err());
        let draft = table.render_source_draft().unwrap();
        let target = table_source_at(&format!("#{draft}"), 2).unwrap();
        assert!(target.source_edit(&draft).is_ok());
        table.set_style(None, "fill", Some("rgb("));
        let draft = table.render_source_draft().unwrap();
        assert!(target.source_edit(&draft).is_err());
        table.cells[0].push("Outside geometry".into());
        assert!(table.render_source_draft().is_err());
    }

    #[test]
    fn vertical_and_explicit_horizontal_rules_remain_editable() {
        let source = "#table(columns: 2, table.vline(x: 1, stroke: red), table.hline(y: 1, stroke: blue), [A], [B], [C], [D])";
        let table = editable_table_at(source, 2).expect("static rules must not hide a table");
        let rendered = table.render_replacement().unwrap();
        assert!(rendered.contains("table.vline(x: 1, stroke: red)"));
        assert!(rendered.contains("table.hline(y: 1, stroke: blue)"));
    }

    #[test]
    fn resizing_columns_preserves_explicit_width_expressions() {
        let mut table = parse("#table(columns: (20%, 1fr, auto), [A], [B], [C])");
        table.add_column();
        assert!(
            table
                .render_replacement()
                .unwrap()
                .contains("columns: (20%, 1fr, auto, auto)")
        );
        table.remove_column();
        assert!(
            table
                .render_replacement()
                .unwrap()
                .contains("columns: (20%, 1fr, auto)")
        );
    }

    #[test]
    fn edge_rules_are_addressed_by_actual_coordinates_without_losing_complex_rules() {
        let mut table = parse(
            "#table(columns: 2, table.hline(y: 1, stroke: blue), table.vline(x: 1, stroke: red), [A], [B], [C], [D])",
        );
        assert_eq!(
            table.boundary_rule(TableAxis::Horizontal, 1).as_deref(),
            Some("blue")
        );
        assert_eq!(table.boundary_rule(TableAxis::Horizontal, 0), None);
        assert_eq!(
            table.boundary_rule(TableAxis::Vertical, 1).as_deref(),
            Some("red")
        );
        table
            .set_boundary_rule(TableAxis::Horizontal, 0, Some("0.5pt + green"))
            .unwrap();
        table
            .set_boundary_rule(TableAxis::Vertical, 1, None)
            .unwrap();
        let rendered = format!("#{}", table.render_replacement().unwrap());
        let copy = parse(&rendered);
        assert_eq!(
            copy.boundary_rule(TableAxis::Horizontal, 0).as_deref(),
            Some("0.5pt + green")
        );
        assert_eq!(copy.boundary_rule(TableAxis::Vertical, 1), None);
        assert_eq!(copy.cells, table.cells);

        let mut complex = parse(
            "#table(columns: 2, table.hline(y: 1, start: 1, stroke: blue), [A], [B], [C], [D])",
        );
        let before = complex.clone();
        assert!(
            complex
                .set_boundary_rule(TableAxis::Horizontal, 1, Some("red"))
                .is_err()
        );
        assert_eq!(complex, before);
        assert!(complex.render_replacement().unwrap().contains("start: 1"));
        assert!(
            table
                .set_boundary_rule(TableAxis::Vertical, 3, Some("red"))
                .is_err()
        );
        assert!(
            table
                .set_boundary_rule(TableAxis::Vertical, 0, Some("red, position: end"))
                .is_err()
        );
    }

    #[test]
    fn boundary_cache_key_changes_only_when_geometry_or_rules_change() {
        let mut table =
            parse("#table(columns: 2, table.hline(y: 1, stroke: blue), [A], [B], [C], [D])");
        let before = table.boundary_rules_key();
        table.cells[0][0] = "Edited".into();
        assert_eq!(table.boundary_rules_key(), before);
        table
            .set_boundary_rule(TableAxis::Vertical, 1, Some("red"))
            .unwrap();
        assert_ne!(table.boundary_rules_key(), before);
        assert!(
            table
                .boundary_rules()
                .iter()
                .any(|(axis, boundary, source)| *axis == TableAxis::Vertical
                    && *boundary == 1
                    && source == "red")
        );
        let with_rules = table.boundary_rules_key();
        table
            .import_markdown("| A | B |\n| --- | --- |\n| C | D |")
            .unwrap();
        assert_ne!(table.boundary_rules_key(), with_rules);
        assert!(table.boundary_rules().is_empty());
    }

    #[test]
    fn width_edits_are_validated_transactionally_and_can_return_to_auto() {
        let mut table = parse("#table(columns: (20%, 1fr), [A], [B])");
        assert_eq!(table.column_widths.as_ref().unwrap(), &["20%", "1fr"]);
        let before = table.clone();
        assert!(
            table
                .set_column_widths(&["1fr, 2fr".into(), "auto".into()])
                .is_err()
        );
        assert!(table.set_column_widths(&["1fr".into()]).is_err());
        assert_eq!(table, before);
        table
            .set_column_widths(&["30%".into(), "auto".into()])
            .unwrap();
        assert!(
            table
                .render_replacement()
                .unwrap()
                .contains("columns: (30%, auto)")
        );
        table.reset_column_widths();
        assert!(table.render_replacement().unwrap().contains("columns: 2"));
        assert!(table.column_widths.is_none());
        let mut one = parse("#table(columns: auto, [A])");
        one.set_column_widths(&["2cm".into()]).unwrap();
        assert!(
            one.render_replacement()
                .unwrap()
                .contains("columns: (2cm,)")
        );
    }

    #[test]
    fn implicit_rules_with_spans_are_lossless_source_only_instead_of_guessed() {
        let source = "#table(columns: 2, table.cell(colspan: 2)[Title], table.hline(), [A], [B])";
        assert!(editable_table_at(source, 2).is_none());
        assert_eq!(table_source_at(source, 2).unwrap().source, source[1..]);
        // Explicit positions are safe even when occupancy has spans.
        let explicit =
            "#table(columns: 2, table.hline(y: 1), table.cell(colspan: 2)[Title], [A], [B])";
        assert!(editable_table_at(explicit, 2).is_some());
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
