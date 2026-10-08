//! Bounded TeX table editing. Unknown column types and structural macros remain
//! editable as exact source rather than being flattened into a guessed grid.
use super::{SourceEdit, byte_range_to_char, char_to_byte};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TexTableEnvironment {
    Tabular,
    TabularX,
    LongTable,
}

impl TexTableEnvironment {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Tabular => "tabular",
            Self::TabularX => "tabularx",
            Self::LongTable => "longtable",
        }
    }
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "tabular" => Some(Self::Tabular),
            "tabularx" => Some(Self::TabularX),
            "longtable" => Some(Self::LongTable),
            _ => None,
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
    row_prefixes: Vec<String>,
    row_endings: Vec<String>,
    trailing: String,
}

impl TexTable {
    pub(crate) fn columns(&self) -> Option<usize> {
        column_count(&self.column_spec, 0)
    }
    pub(crate) fn set_column_spec(&mut self, spec: &str) -> Result<(), String> {
        let columns = column_count(spec, 0).ok_or("Use supported l, c, r, p{width}, m{width}, b{width} or X columns here; custom column types can be edited in source mode")?;
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
            return Err("Enter one complete tabular, tabularx or longtable environment".into());
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
    /// Serialize the user's current draft, including temporarily incomplete
    /// cell expressions, so source mode can repair structural edits. Apply
    /// still goes through source_edit or the validated grid replacement.
    pub(crate) fn render_source_draft(&self) -> Result<String, String> {
        let Some(cells) = &self.cells else {
            return Ok(self.source.clone());
        };
        let columns = self
            .columns()
            .ok_or("Edit this column specification in source mode")?;
        if cells.len() != self.row_prefixes.len()
            || cells.len() != self.row_endings.len()
            || cells.len().saturating_mul(columns) > 4096
        {
            return Err(
                "Keep the table's row structure; change structural commands in source mode".into(),
            );
        }
        let mut replacement = self.header.clone();
        for (r, row) in cells.iter().enumerate() {
            if row.len() != columns {
                return Err(format!("Row {} must have {columns} cells", r + 1));
            }
            replacement.push_str(&self.row_prefixes[r]);
            replacement.push_str(&row.join(" & "));
            replacement.push_str(&self.row_endings[r]);
        }
        replacement.push_str(&self.trailing);
        replacement.push_str(&self.footer);
        Ok(replacement)
    }
    /// Preserve a typed custom or incomplete column specification when moving
    /// to source mode. This draft is never a validated grid replacement.
    pub(crate) fn render_source_draft_with_spec(&self, spec: &str) -> Result<String, String> {
        let mut draft = self.render_source_draft()?;
        draft.replace_range(self.spec_range.clone(), spec);
        Ok(draft)
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
            || candidate.source_range != (0..replacement.chars().count())
        {
            return Err("A cell cannot contain a top-level & or row break; use source mode for structural edits".into());
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
                        // the smallest containing environment.
                        if found
                            .as_ref()
                            .is_none_or(|old: &TexTable| old.source.len() > end - begin)
                        {
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
    if environment == TexTableEnvironment::TabularX {
        let (_, next) = group(source, args, b'{', b'}')?;
        args = next;
    }
    if let Some((_, next)) = group(source, args, b'[', b']') {
        args = next;
    }
    let (spec, body_start) = group(source, args, b'{', b'}')?;
    let spec_start = spec.as_ptr() as usize - source.as_ptr() as usize;
    let columns = column_count(spec, 0);
    let body = source.get(body_start..close)?;
    let parsed = columns.and_then(|columns| split_rows(body, columns));
    let (cells, row_prefixes, row_endings, trailing) = match parsed {
        Some((cells, prefixes, endings, trailing)) => (Some(cells), prefixes, endings, trailing),
        None => (None, Vec::new(), Vec::new(), body.to_owned()),
    };
    Some(TexTable {
        source_range: byte_range_to_char(source, begin..end),
        source: source.get(begin..end)?.to_owned(),
        environment,
        packages: packages(source),
        column_spec: spec.to_owned(),
        cells,
        header: source.get(begin..body_start)?.to_owned(),
        spec_range: (spec_start - begin)..(spec_start - begin + spec.len()),
        footer: source.get(close..end)?.to_owned(),
        row_prefixes,
        row_endings,
        trailing,
    })
}

type ParsedRows = (Vec<Vec<String>>, Vec<String>, Vec<String>, String);
fn split_rows(body: &str, columns: usize) -> Option<ParsedRows> {
    let mut rows = Vec::new();
    let mut prefixes = Vec::new();
    let mut endings = Vec::new();
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
                row.push(cell_source(body.get(start..i)?));
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
                    "multicolumn"
                        | "multirow"
                        | "input"
                        | "include"
                        | "endhead"
                        | "endfirsthead"
                        | "endfoot"
                        | "endlastfoot"
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
                    row.push(cell_source(body.get(start..i)?));
                    let (prefix, first) = row_prefix(&row[0])
                        .map(|(prefix, first)| (prefix.to_owned(), first.to_owned()))?;
                    row[0] = cell_source(&first);
                    let mut ending = end;
                    if body.as_bytes().get(ending) == Some(&b'*') {
                        ending += 1;
                    }
                    if let Some((_, next)) = group(body, ending, b'[', b']') {
                        ending = next;
                    }
                    if row.len() != columns || (rows.len() + 1).saturating_mul(columns) > 4096 {
                        return None;
                    }
                    // The terminating whitespace belongs to a control-word
                    // delimiter such as \tabularnewline. Trimming it from the
                    // next cell would concatenate that command with its text.
                    // Keep comments here too, including their terminating newline.
                    ending = skip_trivia(body, ending);
                    prefixes.push(prefix);
                    endings.push(body[i..ending].to_owned());
                    rows.push(std::mem::take(&mut row));
                    i = ending;
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
    let (prefix, first) = row_prefix(if row.is_empty() { rest } else { &row[0] })
        .map(|(prefix, first)| (prefix.to_owned(), first.to_owned()))?;
    let trailing = if row.is_empty() && first.trim().is_empty() {
        rest.to_owned()
    } else {
        row.push(cell_source(rest));
        if row.len() != columns || (rows.len() + 1).saturating_mul(columns) > 4096 {
            return None;
        }
        if !prefix.is_empty() {
            row[0] = cell_source(&first);
        }
        prefixes.push(prefix.to_owned());
        endings.push(String::new());
        rows.push(row);
        String::new()
    };
    Some((rows, prefixes, endings, trailing))
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
            "hline" | "cline" | "toprule" | "midrule" | "bottomrule" | "cmidrule" | "addlinespace"
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
        if matches!(name, "cline" | "cmidrule") {
            let (_, next) = group(cell, i, b'{', b'}')?;
            i = next;
        }
        i = skip_trivia(cell, i);
    }
    Some((cell.get(..i)?, cell.get(i..)?))
}

fn column_count(spec: &str, nesting: usize) -> Option<usize> {
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
            b'l' | b'c' | b'r' | b'X' => {
                count += 1;
                i += 1;
            }
            b'p' | b'm' | b'b' => {
                count += 1;
                let (_, end) = group(spec, i + 1, b'{', b'}')?;
                i = end;
            }
            b'|' => i += 1,
            b'@' | b'!' | b'>' | b'<' => {
                let (_, end) = group(spec, i + 1, b'{', b'}')?;
                i = end;
            }
            b'*' => {
                let (repeat, end) = group(spec, i + 1, b'{', b'}')?;
                let repeat: usize = repeat.trim().parse().ok()?;
                let (inner, end) = group(spec, end, b'{', b'}')?;
                count =
                    count.checked_add(column_count(inner, nesting + 1)?.checked_mul(repeat)?)?;
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
                if name != "usepackage" {
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
                let (_, end) = command(source, i)?;
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
        for spec in ["l", "lll", "lS", "l}injected{X"] {
            assert!(table.set_column_spec(spec).is_err(), "{spec}");
            assert_eq!(table, before);
        }
    }

    #[test]
    fn tex_structural_macros_and_custom_columns_use_exact_source() {
        for body in [
            "\\multicolumn{2}{c}{Heading} \\\\",
            "A & \\input{data} \\\\",
            "\\endhead A & B \\\\",
            "A & B & C \\\\",
        ] {
            let source = format!("\\begin{{longtable}}[l]{{lc}}{body}\\end{{longtable}}");
            let table = tex_table_at(&source, 1).unwrap();
            assert!(table.cells.is_none(), "{source}");
            assert_eq!(
                table.source_edit(&table.source).unwrap().replacement,
                source
            );
        }
        let source = "\\begin{tabular}{S[table-format=2.1]l}1 & A \\\\ \\end{tabular}";
        let table = tex_table_at(source, 1).unwrap();
        assert_eq!(table.columns(), None);
        assert!(table.cells.is_none());
        assert_eq!(table.render_replacement().unwrap(), source);
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
    fn source_mode_can_repair_a_structural_cell_edit_without_unsafe_grid_apply() {
        let original = "\\begin{tabular}{cc}A & B \\\\ \\end{tabular}";
        let mut table = tex_table_at(original, 1).unwrap();
        table.cells.as_mut().unwrap()[0][0] = "X & Y".into();
        assert!(table.render_replacement().is_err());
        let draft = table.render_source_draft().unwrap();
        assert!(draft.contains("X & Y & B"));
        assert!(table.source_edit(&draft).is_ok());
        table.cells.as_mut().unwrap()[0][0] = "{incomplete".into();
        let draft = table.render_source_draft().unwrap();
        assert!(table.source_edit(&draft).is_err());
    }

    #[test]
    fn source_mode_retains_custom_or_incomplete_column_drafts_without_grid_apply() {
        let original = "\\begin{tabular}[t]{lc}A & B \\\\ \\end{tabular}";
        let table = tex_table_at(original, 1).unwrap();
        let draft = table
            .render_source_draft_with_spec("lS[table-format=2.1]")
            .unwrap();
        assert!(draft.contains("[t]{lS[table-format=2.1]}"));
        assert!(table.source_edit(&draft).is_ok());
        let draft = table
            .render_source_draft_with_spec("l>{incomplete")
            .unwrap();
        assert!(table.source_edit(&draft).is_err());
        assert_eq!(table.column_spec, "lc");
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
    fn oversized_rows_keep_allocation_bounded_and_fall_back_to_exact_source() {
        let source = format!(
            "\\begin{{tabular}}{{cc}}{}Z\\end{{tabular}}",
            "A & ".repeat(10_000)
        );
        let (table, allocated) =
            crate::test_allocations::allocated(|| tex_table_at(&source, 1).unwrap());
        assert!(table.cells.is_none());
        assert_eq!(table.render_replacement().unwrap(), source);
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
        assert!(rendered.contains("% second comment\n\\\\"));
        assert_eq!(cell_source("\\% literal"), "\\% literal");
    }

    #[test]
    fn repeated_column_specs_and_escaped_cells_are_bounded() {
        assert_eq!(column_count("*{2}{l|p{2cm}}", 0), Some(4));
        assert_eq!(column_count("*{999999999999}{l}", 0), None);
        assert_eq!(column_count("Q", 0), None);
        let source = "\\begin{tabular}{cc}\\& & {nested & text} \\\\ \\end{tabular}";
        let table = tex_table_at(source, 1).unwrap();
        assert_eq!(table.cells.as_ref().unwrap()[0], ["\\&", "{nested & text}"]);
        assert!(table.grid_source_edit().is_ok());
    }
}
