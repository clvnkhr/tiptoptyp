//! Curated, editable TeX and Typst starter snippets, written for this editor.
//!
//! The catalogue uses original adaptations of common language constructs. Its
//! coverage was informed by these primary snippet collections, without importing
//! their Lua implementations or copying substantial collections:
//! - https://github.com/iurimateus/luasnip-latex-snippets.nvim/tree/main/lua/luasnip-latex-snippets
//! - https://github.com/evesdropper/luasnip-latex-snippets.nvim/tree/main/lua/luasnip-latex-snippets/luasnippets/tex
//! - https://github.com/rafamadriz/friendly-snippets/blob/main/snippets/latex.json
//! - https://github.com/abhinandh-s/typst-snippets/tree/main/snippets
//! - https://github.com/pseudofractal/typst-notes.nvim/blob/main/snippets/typst.lua
//!
//! Each numbered field is flat so the linked editing session can remain active.
//! Ordinary source dollars, backslashes, and line breaks are deliberately literal.

use crate::snippets::{Language, Scope, Snippet};

type Preset = (&'static str, &'static str, Scope, bool, &'static str);

const TEX: &[Preset] = &[
    (
        "mk",
        "Inline math with an editable expression",
        Scope::Text,
        false,
        r"${{1:x}}${{0}}",
    ),
    (
        "dm",
        "Display math on its own lines",
        Scope::Text,
        true,
        r"\[
  {{1:x}}
\]
{{0}}",
    ),
    (
        "frac",
        "Fraction: numerator, then denominator",
        Scope::Math,
        false,
        r"\frac{{{1:a}}}{{{2:b}}}{{0}}",
    ),
    (
        "sqrt",
        "Square root with an editable expression",
        Scope::Math,
        false,
        r"\sqrt{{{1:x}}}{{0}}",
    ),
    (
        "sum",
        "Summation with linked index and editable bounds",
        Scope::Math,
        false,
        r"\sum_{{{1:i}}={{2:1}}}^{{{3:n}}} {{4:a}}_{{{1}}}{{0}}",
    ),
    (
        "prod",
        "Product with linked index and editable bounds",
        Scope::Math,
        false,
        r"\prod_{{{1:i}}={{2:1}}}^{{{3:n}}} {{4:a}}_{{{1}}}{{0}}",
    ),
    (
        "lim",
        "Limit with a linked variable and editable destination",
        Scope::Math,
        false,
        r"\lim_{{{1:x}}\to {{2:\infty}}} {{3:f}}({{1}}){{0}}",
    ),
    (
        "int",
        "Definite integral with a linked differential variable",
        Scope::Math,
        false,
        r"\int_{{{1:a}}}^{{{2:b}}} {{3:f}}({{4:x}})\,\mathrm{d}{{4}}{{0}}",
    ),
    (
        "part",
        "Partial derivative: function, then variable",
        Scope::Math,
        false,
        r"\frac{\partial {{1:f}}}{\partial {{2:x}}}{{0}}",
    ),
    (
        "ddx",
        "Ordinary derivative with upright differential symbols",
        Scope::Math,
        false,
        r"\frac{\mathrm{d}{{1:f}}}{\mathrm{d}{{2:x}}}{{0}}",
    ),
    (
        "lr",
        "Automatically sized parentheses",
        Scope::Math,
        false,
        r"\left({{1:x}}\right){{0}}",
    ),
    (
        "norm",
        "Automatically sized norm; requires amsmath",
        Scope::Math,
        false,
        r"\left\lVert {{1:x}}\right\rVert{{0}}",
    ),
    (
        "hat",
        "Hat accent over an editable expression",
        Scope::Math,
        false,
        r"\hat{{{1:x}}}{{0}}",
    ),
    (
        "vec",
        "Vector arrow over an editable expression",
        Scope::Math,
        false,
        r"\vec{{{1:v}}}{{0}}",
    ),
    (
        "bb",
        "Blackboard bold symbol; requires amsfonts or amssymb",
        Scope::Math,
        false,
        r"\mathbb{{{1:R}}}{{0}}",
    ),
    (
        "env",
        "Environment with a linked begin/end name",
        Scope::Text,
        true,
        r"\begin{{{1:equation}}}
  {{2:x = y}}
\end{{{1}}}
{{0}}",
    ),
    (
        "eq",
        "Numbered equation on its own lines",
        Scope::Text,
        true,
        r"\begin{equation}
  {{1:x = y}}
\end{equation}
{{0}}",
    ),
    (
        "align",
        "Two aligned equations; requires amsmath",
        Scope::Text,
        true,
        r"\begin{align}
  {{1:x}} &= {{2:y}} \\
  {{3:z}} &= {{4:w}}
\end{align}
{{0}}",
    ),
    (
        "pmat",
        "Editable two-by-two matrix; requires amsmath",
        Scope::Math,
        false,
        r"\begin{pmatrix}
  {{1:a}} & {{2:b}} \\
  {{3:c}} & {{4:d}}
\end{pmatrix}{{0}}",
    ),
    (
        "cases",
        "Two-branch piecewise expression; requires amsmath",
        Scope::Math,
        false,
        r"\begin{cases}
  {{1:x}} & {{2:x > 0}} \\
  {{3:0}} & {{4:x \le 0}}
\end{cases}{{0}}",
    ),
    (
        "sec",
        "Section heading with an editable reference label",
        Scope::Text,
        true,
        r"\section{{{1:Title}}}
\label{sec:{{2:label}}}
{{0}}",
    ),
    (
        "bf",
        "Bold text with an editable field",
        Scope::Text,
        false,
        r"\textbf{{{1:text}}}{{0}}",
    ),
    (
        "emph",
        "Emphasized text with an editable field",
        Scope::Text,
        false,
        r"\emph{{{1:text}}}{{0}}",
    ),
    (
        "cite",
        "Citation with an editable bibliography key",
        Scope::Text,
        false,
        r"\cite{{{1:key}}}{{0}}",
    ),
    (
        "itemize",
        "Bulleted list with two editable items",
        Scope::Text,
        true,
        r"\begin{itemize}
  \item {{1:item}}
  \item {{2:item}}
\end{itemize}
{{0}}",
    ),
];

const TYPST: &[Preset] = &[
    (
        "mk",
        "Inline math with an editable expression",
        Scope::Text,
        false,
        r"${{1:x}}${{0}}",
    ),
    (
        "dm",
        "Display math with literal dollar delimiters",
        Scope::Text,
        true,
        r"$
  {{1:x}}
$
{{0}}",
    ),
    (
        "frac",
        "Fraction: numerator, then denominator",
        Scope::Math,
        false,
        r"frac({{1:a}}, {{2:b}}){{0}}",
    ),
    (
        "sqrt",
        "Square root with an editable expression",
        Scope::Math,
        false,
        r"sqrt({{1:x}}){{0}}",
    ),
    (
        "sum",
        "Summation with linked index and editable bounds",
        Scope::Math,
        false,
        r"sum_({{1:i}}={{2:1}})^({{3:n}}) {{4:a}}_{{1}}{{0}}",
    ),
    (
        "prod",
        "Product with linked index and editable bounds",
        Scope::Math,
        false,
        r"product_({{1:i}}={{2:1}})^({{3:n}}) {{4:a}}_{{1}}{{0}}",
    ),
    (
        "lim",
        "Limit with a linked variable and editable destination",
        Scope::Math,
        false,
        r"lim_({{1:x}} -> {{2:oo}}) {{3:f}}({{1}}){{0}}",
    ),
    (
        "int",
        "Definite integral with a linked differential variable",
        Scope::Math,
        false,
        r"integral_({{1:a}})^({{2:b}}) {{3:f}}({{4:x}}) dif {{4}}{{0}}",
    ),
    (
        "part",
        "Partial derivative using a built-in fraction",
        Scope::Math,
        false,
        r"frac(partial {{1:f}}, partial {{2:x}}){{0}}",
    ),
    (
        "lr",
        "Automatically sized parentheses",
        Scope::Math,
        false,
        r"lr(({{1:x}})){{0}}",
    ),
    (
        "norm",
        "Built-in norm with an editable expression",
        Scope::Math,
        false,
        r"norm({{1:x}}){{0}}",
    ),
    (
        "hat",
        "Built-in hat accent over an editable expression",
        Scope::Math,
        false,
        r"hat({{1:x}}){{0}}",
    ),
    (
        "vec",
        "Built-in vector arrow over an editable expression",
        Scope::Math,
        false,
        r"arrow({{1:v}}){{0}}",
    ),
    (
        "mat",
        "Two-by-two matrix with comma cells and semicolon rows",
        Scope::Math,
        false,
        r"mat(
  {{1:a}}, {{2:b}};
  {{3:c}}, {{4:d}}
){{0}}",
    ),
    (
        "cases",
        "Piecewise expression with editable branches",
        Scope::Math,
        false,
        r##"cases(
  {{1:x}} & "if" {{2:x > 0}},
  {{3:0}} & "otherwise"
){{0}}"##,
    ),
    (
        "h1",
        "Level-one heading on a new line",
        Scope::Text,
        true,
        r"= {{1:Title}}
{{0}}",
    ),
    (
        "h2",
        "Level-two heading on a new line",
        Scope::Text,
        true,
        r"== {{1:Title}}
{{0}}",
    ),
    (
        "strong",
        "Strong emphasis with literal star delimiters",
        Scope::Text,
        false,
        r"*{{1:text}}*{{0}}",
    ),
    (
        "emph",
        "Emphasis with literal underscore delimiters",
        Scope::Text,
        false,
        r"_{{1:text}}_{{0}}",
    ),
    (
        "link",
        "Web link with editable destination and visible text",
        Scope::Text,
        false,
        r##"#link("{{1:https://example.com}}")[{{2:text}}]{{0}}"##,
    ),
    (
        "footnote",
        "Footnote with editable content",
        Scope::Text,
        false,
        r"#footnote[{{1:text}}]{{0}}",
    ),
    (
        "cite",
        "Citation or reference with an editable key",
        Scope::Text,
        false,
        r"@{{1:key}}{{0}}",
    ),
    (
        "figure",
        "Figure with editable rectangle, caption and label",
        Scope::Text,
        true,
        r"#figure(
  rect(width: {{1:60%}}, height: {{2:3cm}}),
  caption: [{{3:Caption}}],
) <{{4:fig-example}}>
{{0}}",
    ),
    (
        "table",
        "Two-column table with headers and editable cells",
        Scope::Text,
        true,
        r"#table(
  columns: 2,
  table.header([{{1:Heading}}], [{{2:Heading}}]),
  [{{3:Cell}}], [{{4:Cell}}],
)
{{0}}",
    ),
    (
        "enum",
        "Numbered list with two editable items",
        Scope::Text,
        true,
        r"+ {{1:item}}
+ {{2:item}}
{{0}}",
    ),
    (
        "fn",
        "Declare a function from markup with #let",
        Scope::Text,
        true,
        r"#let {{1:name}}({{2:x}}) = {{3:x}}{{0}}",
    ),
    (
        "fncode",
        "Function declaration inside a code block",
        Scope::Code,
        true,
        r"let {{1:name}}({{2:x}}) = {{3:x}}{{0}}",
    ),
    (
        "let",
        "Declare a variable from markup with #let",
        Scope::Text,
        true,
        r"#let {{1:name}} = {{2:1}}
{{0}}",
    ),
    (
        "show",
        "Style matching elements with a #show rule",
        Scope::Text,
        true,
        r##"#show {{1:heading}}: set text({{2:weight: "bold"}})
{{0}}"##,
    ),
];

pub(crate) fn defaults() -> Vec<Snippet> {
    [(Language::Tex, TEX), (Language::Typst, TYPST)]
        .into_iter()
        .flat_map(|(language, presets)| {
            presets.iter().map(
                move |&(prefix, description, scope, line_start, body)| Snippet {
                    prefix: prefix.into(),
                    description: description.into(),
                    language,
                    body: body.into(),
                    enabled: true,
                    placeholder_open: crate::snippets::default_placeholder_open(),
                    placeholder_close: crate::snippets::default_placeholder_close(),
                    scope,
                    line_start,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion_edit::{SnippetExpansion, expand_lsp_snippet};
    use crate::snippets::{Session, validate};
    use std::collections::HashSet;

    fn expand(snippet: &Snippet) -> SnippetExpansion {
        expand_lsp_snippet(&snippet.source_to_lsp().unwrap()).unwrap()
    }

    fn preset(language: Language, prefix: &str) -> Snippet {
        defaults()
            .into_iter()
            .find(|snippet| snippet.language == language && snippet.prefix == prefix)
            .unwrap()
    }

    #[test]
    fn catalogue_is_balanced_and_has_unique_language_prefixes() {
        let snippets = defaults();
        assert_eq!(snippets.len(), 54);
        for (language, count) in [(Language::Tex, 25), (Language::Typst, 29)] {
            assert_eq!(
                snippets
                    .iter()
                    .filter(|snippet| snippet.language == language)
                    .count(),
                count
            );
        }
        let mut seen = HashSet::new();
        for snippet in snippets {
            assert!(!snippet.description.is_empty());
            let language = match snippet.language {
                Language::Tex => "tex",
                Language::Typst => "typst",
                Language::Both => panic!("presets must have a specific language"),
            };
            assert!(seen.insert((language, snippet.prefix)), "duplicate preset");
        }
    }

    #[test]
    fn every_preset_validates_and_keeps_a_flat_linked_session() {
        let snippets = defaults();
        validate(&snippets).unwrap();
        for snippet in snippets {
            let expansion = expand(&snippet);
            let length = expansion.text.chars().count();
            assert_eq!(
                expansion.fields.iter().filter(|(id, _)| *id == 0).count(),
                1,
                "{}",
                snippet.prefix
            );
            for (_, range) in &expansion.fields {
                assert!(
                    range.start <= range.end && range.end <= length,
                    "{}",
                    snippet.prefix
                );
            }
            let key = tiptoptyp_core::document::DocumentKey::new(
                tiptoptyp_core::document::WindowSessionId::new(1),
                1,
                1,
            );
            assert!(
                Session::new(key, &expansion.text, expansion.fields).is_some(),
                "nested or coincident fields: {:?}/{}",
                snippet.language,
                snippet.prefix
            );
        }
    }

    #[test]
    fn every_typst_preset_parses_in_its_intended_context() {
        for snippet in defaults()
            .into_iter()
            .filter(|snippet| snippet.language == Language::Typst)
        {
            let text = expand(&snippet).text;
            let source = match snippet.scope {
                Scope::Math => format!("$ {text} $"),
                Scope::Code => format!("#{{\n{text}\n}}"),
                Scope::Text | Scope::Anywhere => text,
            };
            let errors = typst_syntax::parse(&source).errors_and_warnings().0;
            assert!(
                errors.is_empty(),
                "{}: {source}\n{errors:?}",
                snippet.prefix
            );
        }
    }

    #[test]
    fn function_templates_expand_to_markup_or_code_syntax_instead_of_leaking_field_markers() {
        use crate::document::DocumentKind;
        let function = preset(Language::Typst, "fn");
        let code = preset(Language::Typst, "fncode");
        assert_eq!(function.scope, Scope::Text);
        assert_eq!(code.scope, Scope::Code);
        assert!(function.body.starts_with("#let {{1:"));
        assert_eq!(expand(&function).text, "#let name(x) = x");
        assert_eq!(expand(&code).text, "let name(x) = x");
        for (source, trigger, expected) in [
            ("fn", "fn", "#let name(x) = x"),
            ("#fncode", "fncode", "#let name(x) = x"),
            ("#{\nfncode\n}", "fncode", "#{\nlet name(x) = x\n}"),
        ] {
            let cursor = source.find(trigger).unwrap() + trigger.len();
            let snippets = vec![function.clone(), code.clone()];
            let item = crate::snippets::items(&snippets, DocumentKind::Typst, source, cursor)
                .unwrap()
                .into_iter()
                .find(|item| item.label == trigger)
                .unwrap();
            let expansion = crate::completion_edit::expand_lsp_snippet(&item.insert_text).unwrap();
            let start = source.find(trigger).unwrap();
            let result = format!(
                "{}{}{}",
                &source[..start],
                expansion.text,
                &source[cursor..]
            );
            assert_eq!(result, expected);
            assert!(!result.contains("{{"));
            assert!(
                typst_syntax::parse(&result)
                    .errors_and_warnings()
                    .0
                    .is_empty(),
                "{result}"
            );
        }
    }

    #[test]
    fn math_wrappers_preserve_literal_dollars_backslashes_and_newlines() {
        assert_eq!(expand(&preset(Language::Tex, "mk")).text, "$x$");
        assert_eq!(expand(&preset(Language::Typst, "mk")).text, "$x$");
        assert_eq!(expand(&preset(Language::Tex, "dm")).text, "\\[\n  x\n\\]\n");
        assert_eq!(expand(&preset(Language::Typst, "dm")).text, "$\n  x\n$\n");
        assert_eq!(expand(&preset(Language::Tex, "frac")).text, r"\frac{a}{b}");
        let row_end = format!(" {}\n", r"\\");
        for prefix in ["align", "pmat", "cases"] {
            assert!(
                expand(&preset(Language::Tex, prefix))
                    .text
                    .contains(&row_end),
                "{prefix} needs two literal TeX row-ending backslashes"
            );
        }
    }

    #[test]
    fn bound_variables_and_environment_names_have_editable_mirrors() {
        for language in [Language::Tex, Language::Typst] {
            for (prefix, field) in [("sum", 1), ("lim", 1), ("int", 4)] {
                let expansion = expand(&preset(language, prefix));
                assert_eq!(
                    expansion
                        .fields
                        .iter()
                        .filter(|(id, _)| *id == field)
                        .count(),
                    2,
                    "{language:?}/{prefix}"
                );
            }
        }
        let expansion = expand(&preset(Language::Tex, "env"));
        assert_eq!(
            expansion.text,
            "\\begin{equation}\n  x = y\n\\end{equation}\n"
        );
        assert_eq!(
            expansion.fields.iter().filter(|(id, _)| *id == 1).count(),
            2
        );
    }
}
