//! Lossless delimiter changes; never rewrite or evaluate a formula's contents.
use crate::{document::DocumentKind, editor_math_guides};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) range: Range<usize>,
    pub(crate) original: String,
    body: String,
    kind: DocumentKind,
}
impl Target {
    pub(crate) fn at(source: &str, cursor: usize, kind: DocumentKind) -> Option<Self> {
        let byte = source
            .char_indices()
            .nth(cursor)
            .map_or(source.len(), |(at, _)| at);
        let block = editor_math_guides::blocks(source, kind)
            .into_iter()
            .filter(|block| block.range.contains(&byte))
            .min_by_key(|block| block.range.len())?;
        let original = source.get(block.range.clone())?;
        let (open, close) = if original.starts_with("\\begin") {
            let end = original.find('}')? + 1;
            let name = original[..end].split_once('{')?.1.trim_end_matches('}');
            (end, format!("\\end{{{name}}}"))
        } else if original.starts_with("\\[") {
            (2, "\\]".into())
        } else if original.starts_with("\\(") {
            (2, "\\)".into())
        } else if original.starts_with("$$") {
            (2, "$$".into())
        } else if original.starts_with('$') {
            (1, "$".into())
        } else {
            return None;
        };
        let body = original.get(open..original.len().checked_sub(close.len())?)?;
        if !original.ends_with(&close) {
            return None;
        }
        Some(Self {
            range: block.range,
            original: original.into(),
            body: body.trim().into(),
            kind,
        })
    }
    pub(crate) fn choices(&self) -> &'static [&'static str] {
        if self.kind == DocumentKind::Typst {
            &["Inline math", "Display math"]
        } else {
            &[
                "Inline math (\\(…\\))",
                "align",
                "align*",
                "gather",
                "gather*",
                "equation",
                "equation*",
            ]
        }
    }
    pub(crate) fn replacement(&self, choice: &str) -> Option<String> {
        if !self.choices().contains(&choice) {
            return None;
        }
        Some(if self.kind == DocumentKind::Typst {
            if choice == "Inline math" {
                format!("${}$", self.body)
            } else {
                format!("$\n{}\n$", self.body)
            }
        } else if choice.starts_with("Inline math") {
            format!("\\({}\\)", self.body)
        } else {
            format!("\\begin{{{choice}}}\n{}\n\\end{{{choice}}}", self.body)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formula_contents_survive_all_delimiters() {
        for (source, kind) in [
            ("before $ x + α $ after", DocumentKind::Typst),
            ("before \\[ x + \\text{α} \\] after", DocumentKind::Tex),
        ] {
            let target = Target::at(source, 12, kind).unwrap();
            for choice in target.choices() {
                let replacement = target.replacement(choice).unwrap();
                let converted = Target::at(&replacement, 1, kind).unwrap();
                assert_eq!(converted.body, target.body);
            }
        }
    }
    #[test]
    fn ignores_comments_escapes_and_unclosed_tex() {
        for source in ["% $x$", "\\$x\\$", "\\[x", "\\begin{align}x"] {
            assert!(
                Target::at(source, 3, DocumentKind::Tex).is_none(),
                "{source}"
            );
        }
    }
}
