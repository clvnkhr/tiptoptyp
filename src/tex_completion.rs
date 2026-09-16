//! Revision-indexed, local TeX completions. No macro expansion or package IO.
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

use typst_syntax::{LinkedNode, Source, SyntaxKind};

use crate::{
    highlight::{self, EmbeddedLanguage},
    tinymist::CompletionItem,
};
use tiptoptyp_core::text::{LspRange, LspTextEdit, ScalarOffset, lsp_position_at_scalar};

const COMMON: &str = "begin end label ref eqref cite emph textbf textit textnormal textrm textsf texttt textsc underline newcommand renewcommand providecommand DeclareRobustCommand NewDocumentCommand RenewDocumentCommand ProvideDocumentCommand DeclareDocumentCommand def gdef edef xdef let";
const MATH: &str = "alpha beta gamma delta epsilon varepsilon zeta eta theta vartheta iota kappa lambda mu nu xi pi varpi rho varrho sigma varsigma tau upsilon phi varphi chi psi omega Gamma Delta Theta Lambda Xi Pi Sigma Upsilon Phi Psi Omega frac dfrac tfrac sqrt sum prod int iint iiint oint lim sup inf max min sin cos tan log ln exp det gcd operatorname DeclareMathOperator mathbb mathcal mathfrak mathrm mathbf mathit mathsf mathtt boldsymbol text binom dbinom overline hat widehat bar vec dot ddot tilde widetilde left right middle big Big bigg Bigg leq geq neq approx equiv sim simeq in notin subset subseteq supset supseteq cup cap emptyset infty partial nabla forall exists neg land lor to mapsto rightarrow leftarrow Rightarrow Leftrightarrow cdot times div pm mp ldots cdots vdots ddots underbrace overbrace overset underset substack displaystyle textstyle";
const TEXT: &str = "section subsection subsubsection paragraph chapter part title author date maketitle tableofcontents item footnote url href includegraphics caption centering textcolor color pagebreak newpage noindent par vspace hspace documentclass usepackage";

#[derive(Default)]
pub(crate) struct Index {
    fragments: Vec<Fragment>,
    macros: BTreeSet<String>,
}

struct Fragment {
    range: Range<usize>,
    boundaries: Option<Vec<usize>>,
    controls: Vec<Control>,
}

impl Fragment {
    fn physical(&self, byte: usize) -> usize {
        self.boundaries
            .as_ref()
            .map_or(byte, |boundaries| boundaries[byte])
    }
}

struct Control {
    name: Range<usize>,
    math: bool,
}

impl Index {
    pub(crate) fn projected(parsed: &Source) -> Self {
        let mut index = Self::new(parsed);
        let ranges = tiptoptyp::mitex_projection::dollar_payloads(parsed);
        index.fragments.retain(|fragment| {
            let next = ranges.partition_point(|range| range.end <= fragment.range.start);
            ranges
                .get(next)
                .is_none_or(|range| range.start >= fragment.range.end)
        });
        for range in ranges {
            index.add(parsed.text(), range, false, true);
        }
        index.fragments.sort_by_key(|fragment| fragment.range.start);
        index
    }
    pub(crate) fn new(parsed: &Source) -> Self {
        let mut index = Self::default();
        index.visit(LinkedNode::new(parsed.root()), parsed.text());
        index
    }

    fn visit(&mut self, node: LinkedNode<'_>, source: &str) {
        if node.kind() == SyntaxKind::FuncCall
            && let Some(literal) = highlight::embedded_literal(&node, source)
            && literal.language != EmbeddedLanguage::Markdown
        {
            self.add(
                source,
                literal.payload_range,
                literal.quoted,
                literal.language == EmbeddedLanguage::TexMath,
            );
            return;
        }
        if node.kind() == SyntaxKind::Raw {
            let tex = node
                .children()
                .find(|child| child.kind() == SyntaxKind::RawLang)
                .is_some_and(|lang| matches!(lang.leaf_text().as_str(), "tex" | "latex"));
            if tex && let Some(range) = highlight::literal_payload_range(&node) {
                self.add(source, range, false, false);
            }
            return;
        }
        for child in node.children() {
            self.visit(child, source);
        }
    }

    fn add(&mut self, source: &str, range: Range<usize>, quoted: bool, math: bool) {
        let payload = &source[range.clone()];
        let (controls, boundaries) = if quoted {
            let (text, boundaries) = crate::embedded_structure::decode_literal(payload);
            (scan(&text, math, &mut self.macros), Some(boundaries))
        } else {
            (scan(payload, math, &mut self.macros), None)
        };
        self.fragments.push(Fragment {
            range,
            boundaries,
            controls,
        });
    }

    /// Some(empty) owns a TeX region but has no applicable command; do not ask
    /// Tinymist for Typst suggestions in comments, plain TeX text or verbatim.
    pub(crate) fn items(&self, source: &str, cursor: usize) -> Option<Vec<CompletionItem>> {
        if self.fragments.is_empty() {
            return None;
        }
        let byte = source
            .char_indices()
            .nth(cursor)
            .map_or(source.len(), |(i, _)| i);
        let fragment_index = self
            .fragments
            .partition_point(|part| part.range.start <= byte)
            .checked_sub(1)?;
        let fragment = &self.fragments[fragment_index];
        if byte > fragment.range.end {
            return None;
        }
        let relative = byte - fragment.range.start;
        let index = fragment
            .controls
            .partition_point(|control| fragment.physical(control.name.start) <= relative);
        let Some(control) = index
            .checked_sub(1)
            .map(|index| &fragment.controls[index])
            .filter(|control| relative <= fragment.physical(control.name.end))
        else {
            return Some(Vec::new());
        };
        let start_byte = fragment.range.start + fragment.physical(control.name.start);
        let end_byte = fragment.range.start + fragment.physical(control.name.end);
        let start = source[..start_byte].chars().count();
        let end = source[..end_byte].chars().count();
        let range = LspRange {
            start: lsp_position_at_scalar(source, ScalarOffset::new(start)),
            end: lsp_position_at_scalar(source, ScalarOffset::new(end)),
        };
        let mut names = BTreeMap::new();
        for (vocabulary, mode, detail) in [
            (COMMON, None, "TeX · command"),
            (MATH, Some(true), "TeX · math"),
            (TEXT, Some(false), "TeX · text"),
        ] {
            for name in vocabulary.split_whitespace() {
                names.insert(name, (mode.is_none_or(|math| math == control.math), detail));
            }
        }
        for name in &self.macros {
            names.insert(name.as_str(), (true, "TeX · document macro"));
        }
        Some(
            names
                .into_iter()
                .map(|(name, (preferred, detail))| CompletionItem {
                    label: format!("\\{name}"),
                    detail: Some(detail.to_owned()),
                    documentation: None,
                    filter_text: Some(name.to_owned()),
                    sort_text: Some(format!("{}{name}", if preferred { '0' } else { '1' })),
                    insert_text: name.to_owned(),
                    insert_text_is_snippet: false,
                    text_edit: Some(LspTextEdit {
                        range,
                        new_text: name.to_owned(),
                    }),
                    additional_text_edits: Vec::new(),
                })
                .collect(),
        )
    }
}

fn word_end(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
        i += 1;
    }
    i
}

fn skip_space(text: &str, mut i: usize) -> usize {
    let bytes = text.as_bytes();
    loop {
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if bytes.get(i) != Some(&b'%') {
            return i;
        }
        while bytes.get(i).is_some_and(|b| *b != b'\n') {
            i += 1;
        }
    }
}

fn group(text: &str, i: usize) -> Option<(&str, usize)> {
    let start = skip_space(text, i);
    let rest = text.get(start..)?.strip_prefix('{')?;
    let end = rest.find('}')?;
    Some((&rest[..end], start + end + 2))
}

fn math_environment(name: &str) -> bool {
    matches!(
        name.trim_end_matches('*'),
        "math"
            | "displaymath"
            | "equation"
            | "align"
            | "alignat"
            | "gather"
            | "multline"
            | "flalign"
            | "eqnarray"
            | "aligned"
            | "gathered"
            | "split"
            | "cases"
            | "matrix"
            | "pmatrix"
            | "bmatrix"
    )
}

fn scan(text: &str, initial_math: bool, macros: &mut BTreeSet<String>) -> Vec<Control> {
    let bytes = text.as_bytes();
    let mut controls = Vec::new();
    let mut math = initial_math;
    let mut braces = Vec::new();
    let mut environments = Vec::new();
    let mut pending_mode = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'$' => {
                math = !math;
                i += 1;
                if bytes.get(i) == Some(&b'$') {
                    i += 1;
                }
            }
            b'{' => {
                braces.push(math);
                math = pending_mode.take().unwrap_or(math);
                i += 1;
            }
            b'}' => {
                math = braces.pop().unwrap_or(initial_math);
                pending_mode = None;
                i += 1;
            }
            b'\\' => {
                let start = i + 1;
                let end = word_end(bytes, start);
                if end == start && start < bytes.len() {
                    match bytes[start] {
                        b'(' | b'[' => math = true,
                        b')' | b']' => math = false,
                        _ => {}
                    }
                    i = start + text[start..].chars().next().unwrap().len_utf8();
                    continue;
                }
                let name = &text[start..end];
                controls.push(Control {
                    name: start..end,
                    math,
                });
                i = end;
                if matches!(
                    name,
                    "newcommand"
                        | "renewcommand"
                        | "providecommand"
                        | "DeclareRobustCommand"
                        | "NewDocumentCommand"
                        | "RenewDocumentCommand"
                        | "ProvideDocumentCommand"
                        | "DeclareDocumentCommand"
                        | "DeclareMathOperator"
                        | "def"
                        | "gdef"
                        | "edef"
                        | "xdef"
                        | "let"
                ) {
                    let mut at = skip_space(text, end);
                    if bytes.get(at) == Some(&b'*') {
                        at = skip_space(text, at + 1);
                    }
                    if bytes.get(at) == Some(&b'{') {
                        at = skip_space(text, at + 1);
                    }
                    if bytes.get(at) == Some(&b'\\') {
                        let stop = word_end(bytes, at + 1);
                        if stop > at + 1 {
                            macros.insert(text[at + 1..stop].to_owned());
                        }
                    }
                }
                if matches!(name, "verb" | "verb*" | "lstinline") {
                    if bytes.get(i) == Some(&b'*') {
                        i += 1;
                    }
                    if let Some(&delimiter) = bytes.get(i) {
                        i += 1;
                        while i < bytes.len() && bytes[i] != delimiter && bytes[i] != b'\n' {
                            i += 1;
                        }
                        if bytes.get(i) == Some(&delimiter) {
                            i += 1;
                        }
                    }
                } else if matches!(name, "begin" | "end") {
                    if let Some((env, after)) = group(text, end) {
                        if name == "begin" {
                            if matches!(env, "verbatim" | "verbatim*" | "lstlisting" | "minted") {
                                let closing = format!("\\end{{{env}}}");
                                i = text[after..]
                                    .find(&closing)
                                    .map_or(text.len(), |n| after + n + closing.len());
                            } else {
                                environments.push(math);
                                if math_environment(env) {
                                    math = true;
                                }
                                i = after;
                            }
                        } else {
                            math = environments.pop().unwrap_or(initial_math);
                            i = after;
                        }
                    }
                } else if matches!(
                    name,
                    "text"
                        | "textrm"
                        | "textsf"
                        | "texttt"
                        | "textnormal"
                        | "textbf"
                        | "textit"
                        | "mbox"
                        | "hbox"
                ) {
                    pending_mode = Some(false);
                } else if name == "ensuremath" {
                    pending_mode = Some(true);
                }
            }
            b if b.is_ascii_whitespace() => {
                i += 1;
            }
            _ => {
                pending_mode = None;
                i += 1;
            }
        }
    }
    controls
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projected_dollars_complete_inline_block_and_unfinished_tex() {
        for marked in [
            "$\\alp|$",
            "$\n  \\alp|\n$",
            "文 $\\alp|",
            "`$not math$`\n$\\alp|$",
        ] {
            let byte = marked.find('|').unwrap();
            let source = marked.replacen('|', "", 1);
            let cursor = source[..byte].chars().count();
            let parsed = Source::detached(source.clone());
            let index = Index::projected(&parsed);
            let items = index.items(&source, cursor).unwrap();
            assert!(items.iter().any(|item| item.label == "\\alpha"), "{marked}");
            assert!(Index::new(&parsed).items(&source, cursor).is_none());
        }
        let source = "$x % \\alp\n y$";
        assert!(
            Index::projected(&Source::detached(source))
                .items(source, 9)
                .unwrap()
                .is_empty()
        );
    }
    fn items(marked: &str) -> Option<Vec<CompletionItem>> {
        let byte = marked.find('|').unwrap();
        let source = marked.replacen('|', "", 1);
        let cursor = source[..byte].chars().count();
        Index::new(&Source::detached(source.clone())).items(&source, cursor)
    }
    fn has(marked: &str, name: &str) -> bool {
        items(marked).unwrap().iter().any(|item| item.label == name)
    }
    #[test]
    fn detects_embedded_and_tagged_tex_not_typst_or_other_raw() {
        assert!(has("#mi(`\\al|`)", "\\alpha"));
        assert!(has("#mi(input: `\\al|`)", "\\alpha"));
        assert!(has("```latex\n\\sec|\n```", "\\section"));
        assert!(items("Typst \\al|").is_none());
        assert!(items("```rust\n\\al|\n```").is_none());
    }
    #[test]
    fn modes_follow_math_delimiters_environments_and_text_groups() {
        assert!(has("```tex\n\\a|\n```", "\\alpha"));
        assert!(has("```tex\n$\\a|$\n```", "\\alpha"));
        assert!(has(
            "```tex\n\\begin{align}\\a|\\end{align}\n```",
            "\\alpha"
        ));
        assert!(has("#mi(`\\text{\\a|}`)", "\\alpha"));
        assert!(has("#mi(`\\text{x} \\a|`)", "\\alpha"));
        assert!(has("```tex\n\\$ \\a|\n```", "\\alpha"));
        for (source, preferred) in [
            ("```tex\n\\s|\n```", "\\section"),
            ("#mi(`\\a|`)", "\\alpha"),
            ("#mi(`\\text{\\s|}`)", "\\section"),
        ] {
            let candidates = items(source).unwrap();
            assert!(
                candidates
                    .iter()
                    .find(|item| item.label == preferred)
                    .unwrap()
                    .sort_text
                    .as_ref()
                    .unwrap()
                    .starts_with('0')
            );
        }
    }

    #[test]
    fn typed_alpha_prefix_survives_filtering_in_text_mode() {
        let candidates = items("```tex\n\\al|\n```").unwrap();
        let filtered = crate::completion::filtered(&candidates, "al");
        assert!(filtered.iter().any(|item| item.label == "\\alpha"));
        assert!(filtered.len() <= 200);
    }
    #[test]
    fn ignores_comments_and_verbatim_and_escaped_backslashes() {
        for source in [
            "#mi(`% \\al|`)",
            "#mi(`\\verb!\\al|!`)",
            "```tex\n\\begin{verbatim}\\al|\\end{verbatim}\n```",
            "#mi(`\\\\al|`)",
        ] {
            assert!(items(source).unwrap().is_empty(), "{source}");
        }
    }
    #[test]
    fn document_macros_cross_tex_fragments_but_not_comments() {
        assert!(has(
            "#mi(`\\newcommand{\\myvector}[1]{#1}`)\n#mi(`\\myv|`)",
            "\\myvector"
        ));
        assert!(has("#mi(`\\def\\myvector#1{#1} \\myv|`)", "\\myvector"));
        assert!(has(
            "#mi(`\\NewDocumentCommand{\\myvector}{m}{#1} \\myv|`)",
            "\\myvector"
        ));
        assert!(!has("#mi(`% \\def\\fake{x}\n\\fa|`)", "\\fake"));
    }

    #[test]
    fn raw_fragments_need_no_per_byte_offset_table() {
        let source = Source::detached(format!("```tex\n{}\\alpha\n```", "x".repeat(100_000)));
        let index = Index::new(&source);
        assert_eq!(index.fragments.len(), 1);
        assert!(index.fragments[0].boundaries.is_none());
        assert_eq!(index.fragments[0].controls.len(), 1);
    }
    #[test]
    fn quoted_unicode_edit_preserves_escape_and_replaces_command_suffix() {
        let marked = "😀 #mi(\"é \\\\al|pha\")";
        let source = marked.replace('|', "");
        let item = items(marked)
            .unwrap()
            .into_iter()
            .find(|item| item.label == "\\alpha")
            .unwrap();
        let edit = item.text_edit.unwrap();
        let range = tiptoptyp_core::text::range_to_scalar_range(&source, &edit.range).into_range();
        assert_eq!(
            source
                .chars()
                .skip(range.start)
                .take(range.len())
                .collect::<String>(),
            "alpha"
        );
        assert_eq!(edit.new_text, "alpha");
    }

    #[test]
    #[ignore = "opt-in optimized local completion microbenchmark"]
    fn profile_tex_index_reuse() {
        use std::{hint::black_box, time::Instant};
        let source = format!(
            "{}\n#mi(`\\alp`)",
            "#mi(`\\newcommand{\\custom}{x} \\text{word} + \\alpha`)\n".repeat(1000)
        );
        let cursor = source.chars().count() - 2;
        let parsed = Source::detached(source.clone());
        let cached = Index::new(&parsed);
        for _ in 0..10 {
            black_box(cached.items(&source, cursor));
        }
        let start = Instant::now();
        for _ in 0..100 {
            black_box(Index::new(&parsed).items(&source, cursor));
        }
        let uncached = start.elapsed();
        let start = Instant::now();
        for _ in 0..100 {
            black_box(cached.items(&source, cursor));
        }
        let reused = start.elapsed();
        println!(
            "fixture_bytes={} fragments=1001 warmup=10 requests=100 profile=release uncached={uncached:?} cached={reused:?}; parser excluded from both; headless microbenchmark, not GUI latency",
            source.len()
        );
    }
}
