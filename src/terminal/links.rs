//! Clickable links in terminal output: URLs and `path:line:col` locations.
//!
//! Detection is a pure byte scan over one grid row at a time; wrapped rows
//! can split a link across lines and are a known limitation. Candidates
//! never touch the filesystem here: existence is checked on click, when the
//! pane's working directory is known.

use super::engine::Cell;
/// A detected link with its byte range inside the row text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowLink {
    pub range: std::ops::Range<usize>,
    pub target: LinkTarget,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    Url(String),
    /// File location with 1-based line/column, resolved against the pane's
    /// working directory on click.
    Path {
        path: String,
        line: Option<usize>,
        column: Option<usize>,
    },
}

fn is_url_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_'
                | b'.'
                | b'~'
                | b'/'
                | b'?'
                | b'#'
                | b'['
                | b']'
                | b'@'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'%'
        )
}

fn trim_url_end(text: &str) -> &str {
    text.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '\'', '"'])
}

/// Split a `path:line:column` suffix. Returns the path part and 1-based
/// line/column; a trailing bare number is a line, two numbers are line:col.
fn split_location(text: &str) -> (&str, Option<usize>, Option<usize>) {
    // Compiler output trails the location with a colon (`file:12: msg`).
    let text = text.trim_end_matches(':');
    let Some(colon) = text.rfind(':') else {
        return (text, None, None);
    };
    let last = &text[colon + 1..];
    // Windows drive prefix (`C:\...`) is not a line number.
    if last.len() == 1 && last.as_bytes()[0].is_ascii_alphabetic() {
        return (text, None, None);
    }
    let Ok(number) = last.parse::<usize>() else {
        return (text, None, None);
    };
    if number == 0 {
        return (text, None, None);
    }
    let path = &text[..colon];
    // A second trailing number is the line; the first was the column.
    if let Some(colon2) = path.rfind(':') {
        let second = &path[colon2 + 1..];
        if second.len() != 1
            && let Ok(second_number) = second.parse::<usize>()
            && second_number > 0
        {
            return (&path[..colon2], Some(second_number), Some(number));
        }
    }
    (path, Some(number), None)
}

fn looks_like_path(text: &str) -> bool {
    let (path, _, _) = split_location(text);
    if path.is_empty() {
        return false;
    }
    // Skip scheme-like `word:` prefixes and bare words without structure.
    if !path.contains(['/', '.', '~']) {
        return false;
    }
    // Version numbers (`1.2.3`) have structure but no name.
    if !path
        .bytes()
        .any(|byte| byte.is_ascii_alphabetic() || byte == b'/')
    {
        return false;
    }
    !path.contains([' ', '\t', '"', '\'', '<', '>', '|'])
}

/// Find links in one row of terminal text. URLs win over paths: a path scan
/// never starts inside a detected URL.
pub(crate) fn find_links(row: &str) -> Vec<RowLink> {
    let bytes = row.as_bytes();
    let mut links = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        // URL scan.
        let rest = &row[index..];
        let url_start = rest
            .find("https://")
            .map(|at| (at, 8))
            .or_else(|| rest.find("http://").map(|at| (at, 7)));
        // Path token scan: runs of path-ish bytes.
        let mut path_start = None;
        let mut scan = index;
        while scan < bytes.len() {
            let byte = bytes[scan];
            let is_path_byte = byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/' | b':' | b'+' | b'@');
            if is_path_byte {
                if path_start.is_none() {
                    path_start = Some(scan);
                }
            } else if path_start.is_some() {
                break;
            }
            scan += 1;
        }
        let url_first = url_start
            .is_some_and(|(relative, _)| path_start.is_none_or(|at| index + relative <= at));
        if url_first {
            let (relative, scheme_len) = url_start.expect("url wins");
            let start = index + relative;
            let mut end = start + scheme_len;
            while end < bytes.len() && is_url_char(bytes[end]) {
                end += 1;
            }
            let url = trim_url_end(&row[start..end]);
            if !url.is_empty() {
                links.push(RowLink {
                    range: start..start + url.len(),
                    target: LinkTarget::Url(url.to_owned()),
                });
                index = start + url.len().max(1);
            } else {
                index = start + scheme_len;
            }
        } else if let Some(start) = path_start {
            let token = &row[start..scan];
            if looks_like_path(token) {
                let trimmed = token.trim_end_matches(':');
                let (path, line, column) = split_location(trimmed);
                links.push(RowLink {
                    range: start..start + trimmed.len(),
                    target: LinkTarget::Path {
                        path: path.to_owned(),
                        line,
                        column,
                    },
                });
            }
            index = scan.max(start + 1);
        } else {
            break;
        }
    }
    links
}

/// Row text with the cell column of every byte, so link byte ranges map to
/// pointer cells. Wide glyphs occupy their head column; spacer cells
/// contribute no bytes.
pub(crate) fn row_text_and_columns(cells: &[Cell]) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut columns = Vec::new();
    for (col, cell) in cells.iter().enumerate() {
        if cell.width == 0 {
            continue;
        }
        let cell_text = if cell.text.is_empty() {
            " "
        } else {
            cell.text.as_str()
        };
        for _ in cell_text.bytes() {
            columns.push(col);
        }
        text.push_str(cell_text);
    }
    (text, columns)
}

/// The link under a cell column, if any.
pub(crate) fn link_at_column<'a>(
    links: &'a [RowLink],
    columns: &[usize],
    col: usize,
) -> Option<&'a RowLink> {
    links.iter().find(|link| {
        let start = link.range.start;
        let end = link.range.end;
        if start >= columns.len() || end == 0 {
            return false;
        }
        let first = columns[start];
        let last = columns[end.min(columns.len()) - 1] + 1;
        first <= col && col < last
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_detect_and_trim_trailing_punctuation() {
        let links = find_links("see https://example.com/docs, then");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target,
            LinkTarget::Url("https://example.com/docs".into())
        );
        let links = find_links("visit (https://example.com/a).");
        assert_eq!(
            links[0].target,
            LinkTarget::Url("https://example.com/a".into())
        );
    }

    #[test]
    fn paths_split_line_and_column() {
        let links = find_links("error at src/main.rs:12:5: oops");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target,
            LinkTarget::Path {
                path: "src/main.rs".into(),
                line: Some(12),
                column: Some(5),
            }
        );
        let links = find_links("open ~/docs/note.typ:3");
        assert_eq!(
            links[0].target,
            LinkTarget::Path {
                path: "~/docs/note.typ".into(),
                line: Some(3),
                column: None,
            }
        );
    }

    #[test]
    fn prose_without_structure_has_no_links() {
        for row in [
            "just words here",
            "a & b",
            "$100 + $200",
            "version 1.2.3 released",
        ] {
            assert!(find_links(row).is_empty(), "{row:?}");
        }
    }

    #[test]
    fn urls_win_over_path_tokens() {
        let links = find_links("https://example.com/a:b");
        assert_eq!(links.len(), 1);
        assert!(matches!(links[0].target, LinkTarget::Url(_)));
    }
}
