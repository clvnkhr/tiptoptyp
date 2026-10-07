//! Lossless delimiter changes; never rewrite or evaluate a formula's contents.
use crate::{document::DocumentKind, editor_math_guides};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) range: Range<usize>,
    pub(crate) original: String,
    body: String,
    kind: DocumentKind,
    typst_display: bool,
    pub(crate) document_key: Option<tiptoptyp_core::document::DocumentKey>,
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
            body: body.into(),
            kind,
            document_key: None,
            typst_display: kind == DocumentKind::Typst
                && typst_syntax::parse(original)
                    .children()
                    .find_map(|node| node.cast::<typst_syntax::ast::Equation>())
                    .is_some_and(|equation| equation.block()),
        })
    }
    pub(crate) fn bind_document(mut self, key: tiptoptyp_core::document::DocumentKey) -> Self {
        self.document_key = Some(key);
        self
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
    /// Direct menu label when only one conversion makes sense, or `None` when
    /// the choices need a submenu. Typst has exactly two math forms, so the
    /// menu offers the other one directly instead of a one-item flyout.
    pub(crate) fn single_alternative(&self) -> Option<(&'static str, &'static str)> {
        if self.kind != DocumentKind::Typst {
            return None;
        }
        // Typst uses whitespace on both sides of the equation body, rather
        // than line breaks, to distinguish display math from inline math.
        Some(if self.typst_display {
            ("Convert to inline math", "Inline math")
        } else {
            ("Convert to display math", "Display math")
        })
    }
    pub(crate) fn replacement(&self, choice: &str) -> Option<String> {
        if !self.choices().contains(&choice) {
            return None;
        }
        Some(if self.kind == DocumentKind::Typst {
            // A final newline may terminate a // comment. Remove padding but
            // retain that newline before putting the closing delimiter back.
            let body = self.body.trim_start().trim_end_matches([' ', '\t', '\r']);
            if choice == "Inline math" {
                format!("${body}$")
            } else {
                format!("$\n{body}{}$", if body.ends_with('\n') { "" } else { "\n" })
            }
        } else if choice.starts_with("Inline math") {
            format!("\\({}\\)", self.body)
        } else {
            format!("\\begin{{{choice}}}{}\\end{{{choice}}}", self.body)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn single_alternative_offers_the_other_typst_form_directly() {
        let inline = Target::at("$x$", 1, DocumentKind::Typst).expect("inline Typst math");
        assert_eq!(
            inline.single_alternative(),
            Some(("Convert to display math", "Display math"))
        );
        let display = Target::at("$\nx\n$", 2, DocumentKind::Typst).expect("display Typst math");
        assert_eq!(
            display.single_alternative(),
            Some(("Convert to inline math", "Inline math"))
        );
        let tex = Target::at("\\[ x \\]", 2, DocumentKind::Tex).expect("TeX math");
        assert_eq!(tex.single_alternative(), None);
    }
    #[test]
    fn typst_conversion_matches_parsed_equation_mode() {
        use typst_syntax::ast::Equation;
        for (source, display) in [
            ("$x$", false),
            ("$ x $", true),
            ("$\tx\t$", true),
            ("$x\n+ y$", false),
            ("$\nx + y\n$", true),
            ("$ x$", false),
            ("$x $", false),
        ] {
            let parsed = typst_syntax::parse(source);
            let equation = parsed
                .children()
                .find_map(|node| node.cast::<Equation>())
                .unwrap();
            assert_eq!(equation.block(), display, "fixture {source:?}");
            let target = Target::at(source, 1, DocumentKind::Typst).unwrap();
            let (label, choice) = target.single_alternative().unwrap();
            assert_eq!(
                choice,
                if display {
                    "Inline math"
                } else {
                    "Display math"
                },
                "{source:?}"
            );
            assert_eq!(
                label,
                if display {
                    "Convert to inline math"
                } else {
                    "Convert to display math"
                }
            );
            let replacement = target.replacement(choice).unwrap();
            let reparsed = typst_syntax::parse(&replacement);
            let converted = reparsed
                .children()
                .find_map(|node| node.cast::<Equation>())
                .unwrap();
            assert_ne!(
                converted.block(),
                display,
                "conversion did not change mode: {replacement:?}"
            );
        }
    }

    #[test]
    fn tex_conversion_keeps_newlines_that_terminate_comments() {
        let source = "\\[\nx % explanation\n\\]";
        let target = Target::at(source, 4, DocumentKind::Tex).unwrap();
        assert_eq!(
            target.replacement(target.choices()[0]).unwrap(),
            "\\(\nx % explanation\n\\)"
        );
    }
    #[test]
    fn typst_inline_conversion_keeps_its_closing_delimiter_out_of_comments() {
        let source = "$\nx // explanation\n$";
        let target = Target::at(source, 4, DocumentKind::Typst).unwrap();
        let converted = target.replacement("Inline math").unwrap();
        assert_eq!(converted, "$x // explanation\n$");
        assert!(
            typst_syntax::parse(&converted)
                .errors_and_warnings()
                .0
                .is_empty()
        );
        assert!(Target::at(&converted, 1, DocumentKind::Typst).is_some());
    }
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
                if kind == DocumentKind::Tex {
                    assert_eq!(converted.body, target.body);
                } else {
                    assert_eq!(converted.body.trim(), target.body.trim());
                }
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
