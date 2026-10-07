//! Lossless fallback for tables whose cell geometry cannot be evaluated locally.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableSource {
    pub(crate) kind: TableKind,
    pub(crate) source_range: std::ops::Range<usize>,
    pub(crate) source: String,
}

pub(crate) fn table_source_at(source: &str, char_cursor: usize) -> Option<TableSource> {
    let cursor = char_to_byte(source, char_cursor)?;
    table_source_in_source(&Source::detached(source), cursor)
}

pub(crate) fn table_source_in_source(parsed: &Source, cursor: usize) -> Option<TableSource> {
    let source = parsed.text();
    if cursor > source.len() || !source.is_char_boundary(cursor) {
        return None;
    }
    let root = LinkedNode::new(parsed.root());
    find_table_source(&root, source, cursor).or_else(|| {
        source[cursor..]
            .starts_with('#')
            .then(|| find_table_source(&root, source, cursor + 1))
            .flatten()
    })
}

fn find_table_source(node: &LinkedNode<'_>, source: &str, cursor: usize) -> Option<TableSource> {
    if !contains_byte(node.range(), cursor) {
        return None;
    }
    if let Some(table) = node
        .children()
        .find_map(|child| find_table_source(&child, source, cursor))
    {
        return Some(table);
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
    Some(TableSource {
        kind,
        source_range: byte_range_to_char(source, node.range()),
        source: source.get(node.range())?.to_owned(),
    })
}

impl TableSource {
    /// Source mode retains computed cells, spreads, comments and structural
    /// helpers. Apply is admitted only when the draft is precisely one complete
    /// call of the original kind; surrounding source can never be overwritten.
    pub(crate) fn source_edit(&self, draft: &str) -> Result<SourceEdit, String> {
        let wrapper = format!("#{draft}");
        let parsed = Source::detached(&wrapper);
        let root = LinkedNode::new(parsed.root());
        if has_descendant_kind(&root, SyntaxKind::Error) {
            return Err("Finish the table expression before applying it".into());
        }
        let Some(table) = find_table_source(&root, &wrapper, 1) else {
            return Err("Keep the table or grid call in this editor".into());
        };
        if table.kind != self.kind || table.source_range != (1..wrapper.chars().count()) {
            return Err(format!(
                "Enter one complete {}(...) call, without surrounding text",
                self.kind.callee()
            ));
        }
        Ok(SourceEdit {
            range: self.source_range.clone(),
            replacement: draft.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_tables_are_detected_without_guessing_cells() {
        for source in [
            "#table(columns: sizes, ..rows)",
            "#table(columns: 2, [A], /* retain me */ [B])",
            "#table(table.header([Name]), ..data.map(row => table.cell(fill: blue)[#row]))",
            "#grid(columns: 2, [#let value = 1; #value], [])",
            "#table(columns: 2, [A], [B], [Incomplete row])",
            "#table(columns: 2, table.cell(x: 1, y: 0)[Positioned])",
        ] {
            let table = table_source_at(source, 0).unwrap();
            assert_eq!(table.source, source[1..]);
            assert_eq!(
                table.source_edit(&table.source).unwrap().replacement,
                source[1..]
            );
        }
    }

    #[test]
    fn source_fallback_keeps_unicode_ranges_and_innermost_table() {
        let source = "é before #table([outer #grid(..inner)]) after";
        let cursor = source[..source.find("inner").unwrap()].chars().count();
        let table = table_source_at(source, cursor).unwrap();
        assert_eq!(table.kind, TableKind::Grid);
        let edit = table.source_edit("grid(columns: count, ..new)").unwrap();
        assert_eq!(
            &source[char_to_byte(source, edit.range.start).unwrap()
                ..char_to_byte(source, edit.range.end).unwrap()],
            "grid(..inner)"
        );
        assert_eq!(edit.replacement, "grid(columns: count, ..new)");
    }

    #[test]
    fn source_apply_rejects_broken_or_surrounding_source() {
        let original = table_source_at("#table(..cells)", 1).unwrap();
        for draft in [
            "table([",
            "grid([A])",
            "table([A]) after",
            "table([A])\n#table([B])",
            "custom.table([A])",
            "table([A]) // hidden source",
        ] {
            assert!(original.source_edit(draft).is_err(), "{draft}");
        }
        assert_eq!(original.source, "table(..cells)");
        for source in [
            "table([A])",
            "#custom.table([A])",
            "`#table([A])`",
            "// #table([A])",
        ] {
            assert!(
                table_source_at(source, source.find("table").unwrap()).is_none(),
                "{source}"
            );
        }
        assert!(table_source_at("#table([A])", 999).is_none());
    }
}
