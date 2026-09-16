//! Physical source regions, not rendered text. Markdown uses CommonMark's
//! offset iterator. TeX is deliberately structural: no macro expansion, IO,
//! compilation, or language-server round trips on the editor thread.
use std::ops::Range;

use crate::highlight::EmbeddedLanguage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Region {
    pub(crate) range: Range<usize>,
    pub(crate) heading_level: Option<usize>,
}

/// Decode a Typst string while retaining a boundary map back to its literal
/// spelling. Escaped newlines help parsing but never invent physical UI rows.
pub(crate) fn literal_regions(
    source: &str,
    language: EmbeddedLanguage,
    quoted: bool,
) -> Vec<Region> {
    if !quoted {
        return regions(source, language);
    }
    let (decoded, boundaries) = decode_literal(source);
    regions(&decoded, language)
        .into_iter()
        .map(|mut region| {
            region.range = boundaries[region.range.start]..boundaries[region.range.end];
            region
        })
        .collect()
}

pub(crate) fn decode_literal(source: &str) -> (String, Vec<usize>) {
    let mut decoded = String::new();
    let mut boundaries = vec![0];
    let mut chars = source.char_indices().peekable();
    while let Some((start, mut ch)) = chars.next() {
        let mut end = start + ch.len_utf8();
        if ch == '\\'
            && let Some((offset, escaped)) = chars.next()
        {
            end = offset + escaped.len_utf8();
            ch = match escaped {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '"' => '"',
                'u' if chars.peek().is_some_and(|(_, c)| *c == '{') => {
                    chars.next();
                    let mut hex = String::new();
                    for (offset, digit) in chars.by_ref() {
                        end = offset + digit.len_utf8();
                        if digit == '}' {
                            break;
                        }
                        hex.push(digit);
                    }
                    u32::from_str_radix(&hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .unwrap_or('\u{fffd}')
                }
                _ => escaped,
            };
        }
        decoded.push(ch);
        boundaries.extend(std::iter::repeat_n(start, ch.len_utf8() - 1));
        boundaries.push(end);
    }
    (decoded, boundaries)
}

pub(crate) fn regions(source: &str, language: EmbeddedLanguage) -> Vec<Region> {
    let mut regions = match language {
        EmbeddedLanguage::Markdown => markdown(source),
        EmbeddedLanguage::TexMath | EmbeddedLanguage::TexText => tex(source),
    };
    regions.retain(|region| source[region.range.clone()].contains('\n'));
    regions.sort_by_key(|region| (region.range.start, std::cmp::Reverse(region.range.end)));
    regions
}

fn markdown(source: &str) -> Vec<Region> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut result = Vec::new();
    let mut headings = std::collections::BTreeMap::<(usize, usize), Vec<(usize, usize)>>::new();
    let mut containers = Vec::new();
    for (event, range) in Parser::new(source).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                headings
                    .entry(containers.last().copied().unwrap_or((0, source.len())))
                    .or_default()
                    .push((range.start, level as usize));
            }
            Event::Start(
                Tag::BlockQuote(_) | Tag::List(_) | Tag::Item | Tag::FootnoteDefinition(_),
            ) => {
                containers.push((range.start, range.end));
                result.push(Region {
                    range: trim_final_newline(source, range),
                    heading_level: None,
                });
            }
            Event::End(
                TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item | TagEnd::FootnoteDefinition,
            ) => {
                containers.pop();
            }
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => {
                result.push(Region {
                    range: trim_final_newline(source, range),
                    heading_level: None,
                });
            }
            _ => {}
        }
    }
    for ((_, end), headings) in headings {
        append_sections(&mut result, &headings, end);
    }
    result
}

fn trim_final_newline(source: &str, mut range: Range<usize>) -> Range<usize> {
    if source[..range.end].ends_with('\n') {
        range.end -= 1;
    }
    range
}

fn append_sections(result: &mut Vec<Region>, headings: &[(usize, usize)], end: usize) {
    let mut active: Vec<usize> = Vec::new();
    for &(start, level) in headings {
        while active
            .last()
            .is_some_and(|&i| result[i].heading_level >= Some(level))
        {
            result[active.pop().unwrap()].range.end = start;
        }
        active.push(result.len());
        result.push(Region {
            range: start..end,
            heading_level: Some(level),
        });
    }
}

/// A tolerant lexical reader: comments, escaped control symbols and verbatim
/// payloads cannot introduce structure. Unclosed groups/environments extend to
/// EOF so folding remains useful while typing incomplete input.
fn tex(source: &str) -> Vec<Region> {
    let bytes = source.as_bytes();
    let mut result = Vec::new();
    let mut headings = Vec::new();
    let mut groups: Vec<(usize, &str)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                i += source[i..].find('\n').unwrap_or(bytes.len() - i);
            }
            b'{' => {
                groups.push((i, "}"));
                i += 1;
            }
            b'}' => {
                close_group(&mut groups, "}", i + 1, &mut result);
                i += 1;
            }
            b'\\' => {
                let start = i;
                i += 1;
                let word_start = i;
                while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let command = &source[word_start..i];
                if command.is_empty() {
                    if i < bytes.len() {
                        match bytes[i] {
                            b'[' => groups.push((start, "\\]")),
                            b']' => close_group(&mut groups, "\\]", i + 1, &mut result),
                            _ => {}
                        }
                        i += source[i..].chars().next().unwrap().len_utf8();
                    }
                    continue;
                }
                if command == "verb" {
                    if bytes.get(i) == Some(&b'*') {
                        i += 1;
                    }
                    if let Some(delimiter) = source[i..].chars().next() {
                        i += delimiter.len_utf8();
                        let tail = &source[i..];
                        i += tail.find([delimiter, '\n']).unwrap_or(tail.len());
                        if source[i..].starts_with(delimiter) {
                            i += delimiter.len_utf8();
                        }
                    }
                } else if matches!(command, "begin" | "end") {
                    let mut argument = i;
                    while bytes.get(argument).is_some_and(u8::is_ascii_whitespace) {
                        argument += 1;
                    }
                    if bytes.get(argument) == Some(&b'{')
                        && let Some(length) = source[argument + 1..].find('}')
                    {
                        let name = &source[argument + 1..argument + 1 + length];
                        i = argument + length + 2;
                        if command == "end" {
                            close_group(&mut groups, name, i, &mut result);
                        } else if matches!(name, "verbatim" | "verbatim*" | "lstlisting" | "minted")
                        {
                            let closing = format!("\\end{{{name}}}");
                            i = source[i..]
                                .find(&closing)
                                .map_or(bytes.len(), |offset| i + offset + closing.len());
                            result.push(Region {
                                range: start..i,
                                heading_level: None,
                            });
                        } else {
                            groups.push((start, name));
                        }
                    }
                } else if let Some(level) = [
                    "part",
                    "chapter",
                    "section",
                    "subsection",
                    "subsubsection",
                    "paragraph",
                    "subparagraph",
                ]
                .iter()
                .position(|candidate| *candidate == command)
                {
                    headings.push((start, level + 1));
                }
            }
            b'$' if bytes.get(i + 1) == Some(&b'$') => {
                if groups.last().is_some_and(|(_, close)| *close == "$$") {
                    close_group(&mut groups, "$$", i + 2, &mut result);
                } else {
                    groups.push((i, "$$"));
                }
                i += 2;
            }
            _ => {
                i += source[i..].chars().next().unwrap().len_utf8();
            }
        }
    }
    for (start, _) in groups {
        result.push(Region {
            range: start..source.len(),
            heading_level: None,
        });
    }
    append_sections(&mut result, &headings, source.len());
    result
}

fn close_group(groups: &mut Vec<(usize, &str)>, close: &str, end: usize, result: &mut Vec<Region>) {
    if let Some(index) = groups.iter().rposition(|(_, expected)| *expected == close) {
        // In malformed input, abandon intervening unmatched opens, never
        // fabricate crossing ranges that would conceal a later sibling.
        let (start, _) = groups[index];
        groups.truncate(index);
        result.push(Region {
            range: start..end,
            heading_level: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_local_headings_do_not_end_document_sections() {
        let source = "# Outer\ntext\n> # Local\n> quote body\n\nOutside quote\n# Next\nend";
        let parsed = regions(source, EmbeddedLanguage::Markdown);
        assert_eq!(
            parsed
                .iter()
                .find(|r| r.range.start == 0)
                .unwrap()
                .range
                .end,
            source.find("# Next").unwrap()
        );
        let local = parsed
            .iter()
            .find(|r| r.heading_level.is_some() && source[r.range.clone()].starts_with("# Local"))
            .unwrap();
        assert!(local.range.end <= source.find("Outside").unwrap());
    }

    #[test]
    fn markdown_sections_setext_lists_and_fences() {
        let source = "# Outer\nbody\nChild\n-----\n- first\n  continuation\n- second\n~~~tex\n# not a heading\n~~~\n# Sibling\ntail";
        let parsed = regions(source, EmbeddedLanguage::Markdown);
        let outer = parsed.iter().find(|r| r.range.start == 0).unwrap();
        assert_eq!(outer.range.end, source.find("# Sibling").unwrap());
        assert!(parsed.iter().any(|r| r.heading_level == Some(2)));
        assert!(
            !parsed
                .iter()
                .any(|r| r.range.start == source.find("# not").unwrap())
        );
        assert!(
            parsed
                .iter()
                .any(|r| source[r.range.clone()].starts_with("~~~tex"))
        );
        assert!(
            parsed
                .iter()
                .any(|r| source[r.range.clone()].starts_with("- first"))
        );
    }

    #[test]
    fn tex_nested_environments_groups_unicode_and_ignored_literals() {
        let source = "\\section{É}\n\\begin{align}\n{α\nβ}\n\\begin{matrix}\na&b\n\\end{matrix}\n\\end{align}\n% \\section{ignored}\n\\verb|\\section{ignored}|\n\\begin{verbatim}\n\\section{ignored}\n\\end{verbatim}\n\\section*{Next}\nend";
        let parsed = regions(source, EmbeddedLanguage::TexText);
        assert_eq!(
            parsed.iter().filter(|r| r.heading_level.is_some()).count(),
            2
        );
        let align = parsed
            .iter()
            .find(|r| source[r.range.clone()].starts_with("\\begin{align}"))
            .unwrap();
        assert!(source[align.range.clone()].ends_with("\\end{align}"));
        assert!(parsed.iter().any(|r| &source[r.range.clone()] == "{α\nβ}"));
    }

    #[test]
    fn incomplete_tex_and_display_math_remain_foldable() {
        let source = "\\[\na+b\n\\]\n$$\nc+d\n$$\n\\begin{equation}\nx";
        let parsed = regions(source, EmbeddedLanguage::TexMath);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed.last().unwrap().range.end, source.len());
    }
}
