//! Cached, display-only source decorations. Literal matches never rewrite source.
use crate::syntax_theme::TypstStyleOverride;
use eframe::egui::{self, Color32, Stroke, text::LayoutJob};
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HighlightRule {
    pub(crate) text: String,
    pub(crate) enabled: bool,
    pub(crate) style: TypstStyleOverride,
}
pub(crate) fn default_rules() -> Vec<HighlightRule> {
    ["TODO:", "FIXME:"]
        .into_iter()
        .map(|text| HighlightRule {
            text: text.into(),
            enabled: true,
            style: TypstStyleOverride {
                weight: Some(700),
                underline: Some(true),
                ..Default::default()
            },
        })
        .collect()
}
pub(crate) fn default_guide() -> String {
    "│".into()
}
pub(crate) fn enabled() -> bool {
    true
}

#[derive(Default)]
pub(crate) struct Options {
    definitions: bool,
    rules: Vec<HighlightRule>,
    accent: Color32,
}
impl Options {
    pub(crate) fn set(
        &mut self,
        definitions: bool,
        rules: &[HighlightRule],
        accent: Color32,
    ) -> bool {
        if self.definitions == definitions && self.rules == rules && self.accent == accent {
            return false;
        }
        self.definitions = definitions;
        self.rules = rules.to_vec();
        self.accent = accent;
        true
    }
    pub(crate) fn decorate(&self, cache: &mut Decorations, job: &mut LayoutJob, typst: bool) {
        if !self.definitions && !self.rules.iter().any(|r| r.enabled && !r.text.is_empty()) {
            return;
        }
        cache.prepare(&job.text, typst, self.definitions, &self.rules);
        cache.apply(job, self.accent);
    }
}

#[derive(Default)]
pub(crate) struct Decorations {
    source: String,
    typst: bool,
    definitions: bool,
    rules: Vec<HighlightRule>,
    spans: Vec<(Range<usize>, TypstStyleOverride)>,
    valid: bool,
    #[cfg(test)]
    builds: usize,
}
impl Decorations {
    pub(crate) fn prepare(
        &mut self,
        source: &str,
        typst: bool,
        definitions: bool,
        rules: &[HighlightRule],
    ) {
        if self.valid
            && self.source == source
            && self.typst == typst
            && self.definitions == definitions
            && self.rules == rules
        {
            return;
        }
        self.source.clear();
        self.source.push_str(source);
        self.typst = typst;
        self.definitions = definitions;
        self.rules = rules.to_vec();
        self.spans.clear();
        self.valid = true;
        #[cfg(test)]
        {
            self.builds += 1;
        }
        if definitions {
            let style = TypstStyleOverride {
                weight: Some(700),
                underline: Some(true),
                ..Default::default()
            };
            for range in definition_ranges(source, typst) {
                self.spans.push((range, style.clone()));
            }
        }
        // Later rules win for overlapping matches. Empty text is deliberately inert.
        for rule in rules.iter().filter(|r| r.enabled && !r.text.is_empty()) {
            for (start, _) in source.match_indices(&rule.text) {
                self.spans
                    .push((start..start + rule.text.len(), rule.style.clone()));
            }
        }
    }
    pub(crate) fn apply(&self, job: &mut LayoutJob, accent: Color32) {
        if self.spans.is_empty() {
            return;
        }
        // Sweep ordered boundaries instead of scanning every match for every
        // syntax section. Later matches have priority, including overlaps.
        let mut events = Vec::with_capacity(self.spans.len() * 2);
        for (index, (range, _)) in self.spans.iter().enumerate() {
            events.push((range.start, true, index));
            events.push((range.end, false, index));
        }
        events.sort_unstable();
        let mut active = std::collections::BTreeSet::new();
        let mut at = 0;
        let mut sections = Vec::new();
        for section in &job.sections {
            let mut start = section.byte_range.start.0;
            let end = section.byte_range.end.0;
            while start < end {
                while at < events.len() && events[at].0 <= start {
                    let (_, add, index) = events[at];
                    if add {
                        active.insert(index);
                    } else {
                        active.remove(&index);
                    }
                    at += 1;
                }
                let next = events.get(at).map_or(end, |e| e.0.min(end));
                let mut part = section.clone();
                part.byte_range = start.into()..next.into();
                for &index in &active {
                    apply_style(&mut part.format, &self.spans[index].1, accent);
                }
                sections.push(part);
                start = next;
            }
        }
        job.sections = sections;
    }
}

fn definition_ranges(source: &str, typst: bool) -> Vec<Range<usize>> {
    if typst {
        fn visit(
            node: typst_syntax::LinkedNode<'_>,
            parsed: &typst_syntax::Source,
            result: &mut Vec<Range<usize>>,
        ) {
            use typst_syntax::SyntaxKind;
            use typst_syntax::ast::{self, AstNode};
            if let Some(binding) = node.get().cast::<ast::LetBinding>() {
                for name in binding.kind().bindings() {
                    if let Some(range) = parsed
                        .find(name.to_untyped().span())
                        .map(|node| node.range())
                    {
                        result.push(range);
                    }
                }
            } else if let Some(binary) = node.get().cast::<ast::Binary>()
                && matches!(
                    binary.op(),
                    ast::BinOp::Assign
                        | ast::BinOp::AddAssign
                        | ast::BinOp::SubAssign
                        | ast::BinOp::MulAssign
                        | ast::BinOp::DivAssign
                )
                && let Some(name) = parsed.find(binary.lhs().to_untyped().span())
            {
                result.push(name.range());
            } else if node.kind() == SyntaxKind::SetRule
                && let Some(name) = node
                    .children()
                    .find(|child| child.kind() == SyntaxKind::Ident)
            {
                result.push(name.range());
            }
            for child in node.children() {
                visit(child, parsed, result);
            }
        }
        let parsed = typst_syntax::Source::detached(source);
        let mut result = Vec::new();
        visit(
            typst_syntax::LinkedNode::new(parsed.root()),
            &parsed,
            &mut result,
        );
        result
    } else {
        // Commands in comments are not declarations. TeX's dynamic expansion is
        // intentionally outside this local, lexical decoration.
        let mut ranges = Vec::new();
        let mut offset = 0;
        for line in source.split_inclusive('\n') {
            let comment = line
                .char_indices()
                .find(|(at, c)| *c == '%' && !escaped(line, *at))
                .map_or(line.len(), |(at, _)| at);
            let code = &line[..comment];
            for command in [
                "\\newcommand",
                "\\renewcommand",
                "\\providecommand",
                "\\DeclareMathOperator",
                "\\NewDocumentCommand",
                "\\RenewDocumentCommand",
                "\\ProvideDocumentCommand",
                "\\newtheorem",
                "\\let",
                "\\DeclareRobustCommand",
                "\\def",
                "\\gdef",
                "\\edef",
                "\\xdef",
                "\\newenvironment",
                "\\renewenvironment",
            ] {
                for (at, _) in code.match_indices(command) {
                    if escaped(code, at) {
                        continue;
                    }
                    let mut start = at + command.len();
                    if code[start..].starts_with(|c: char| c.is_ascii_alphabetic()) {
                        continue;
                    }
                    while code[start..]
                        .starts_with(|c: char| c.is_whitespace() || c == '*' || c == '{')
                    {
                        start += code[start..].chars().next().unwrap().len_utf8();
                    }
                    let mut end = start;
                    if code[end..].starts_with('\\') {
                        end += 1;
                    }
                    while code[end..].starts_with(|c: char| c.is_alphabetic() || c == '@') {
                        end += code[end..].chars().next().unwrap().len_utf8();
                    }
                    if end > start {
                        ranges.push(offset + start..offset + end);
                    }
                }
            }
            offset += line.len();
        }
        ranges
    }
}

fn escaped(source: &str, at: usize) -> bool {
    source[..at]
        .bytes()
        .rev()
        .take_while(|&b| b == b'\\')
        .count()
        % 2
        != 0
}

fn indentation_columns(chars: impl Iterator<Item = char>, step: usize, limit: usize) -> usize {
    let mut columns = 0;
    for c in chars {
        match c {
            ' ' => columns += 1,
            '\t' => columns += step - columns % step,
            _ => break,
        }
        if columns >= limit {
            break;
        }
    }
    columns
}

/// Only leading whitespace on visible physical rows receives guides. Wrapped
/// continuations and folded zero-height rows stay clear.
pub(crate) fn paint_guides(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    character: &str,
    indent: u8,
) {
    let font = crate::theme::editor_font();
    let color = ui.visuals().weak_text_color().gamma_multiply(0.4);
    let glyph = ui
        .painter()
        .layout_no_wrap(character.into(), font.clone(), color);
    let space = ui.fonts_mut(|fonts| fonts.glyph_width(&font, ' '));
    let step = usize::from(if indent == 0 { 4 } else { indent });
    let painter = ui
        .painter()
        .with_clip_rect(output.response.rect.intersect(ui.clip_rect()));
    let mut physical_start = true;
    let top = painter.clip_rect().top() - output.galley_pos.y;
    let first = output
        .galley
        .rows
        .partition_point(|row| row.rect().bottom() < top);
    if first > 0 {
        physical_start = output.galley.rows[first - 1].ends_with_newline;
    }
    for row in &output.galley.rows[first..] {
        if row.rect().top() + output.galley_pos.y > painter.clip_rect().bottom() {
            break;
        }
        if physical_start && row.size.y > 0.0 {
            let row_origin = output.galley_pos + row.pos.to_vec2();
            let visible_columns = ((painter.clip_rect().right() - row_origin.x) / space)
                .ceil()
                .max(0.0) as usize;
            let columns = indentation_columns(
                row.glyphs.iter().map(|glyph| glyph.chr),
                step,
                visible_columns,
            );
            let first_column = (((painter.clip_rect().left() - row_origin.x) / space)
                .floor()
                .max(0.0) as usize
                / step
                + 1)
                * step;
            for column in (first_column..=columns.min(visible_columns)).step_by(step) {
                let pos = output.galley_pos
                    + row.pos.to_vec2()
                    + egui::vec2((column as f32 - 0.5) * space - glyph.size().x / 2.0, 0.0);
                if painter
                    .clip_rect()
                    .intersects(egui::Rect::from_min_size(pos, glyph.size()))
                {
                    painter.galley(pos, glyph.clone(), color);
                }
            }
        }
        physical_start = row.ends_with_newline;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indentation_counts_spaces_tabs_and_bounds_visible_work() {
        assert_eq!(indentation_columns("  body".chars(), 2, 80), 2);
        assert_eq!(indentation_columns(" \t\ttext".chars(), 4, 80), 8);
        assert_eq!(indentation_columns("body  ".chars(), 2, 80), 0);
        assert_eq!(indentation_columns(std::iter::repeat(' '), 2, 80), 80);
    }
    #[test]
    fn decorations_share_syntax_cache_and_invalidate_on_options_theme_and_edits() {
        let generic = crate::generic_highlight::GenericSyntaxHighlighter::default();
        let mut syntax = crate::highlight::SyntaxHighlighter::default();
        let source = "#let foo = 1\n#foo = 2 // TODO: λ";
        let baseline = syntax.highlight(source, false, &generic);
        syntax.set_decorations(true, &default_rules(), Color32::RED);
        let decorated = syntax.highlight(source, false, &generic);
        assert_ne!(decorated, baseline);
        assert_eq!(decorated, syntax.highlight(source, false, &generic));
        syntax.set_decorations(true, &default_rules(), Color32::BLUE);
        assert_ne!(decorated, syntax.highlight(source, false, &generic));
        syntax.set_decorations(false, &[], Color32::RED);
        assert_eq!(baseline, syntax.highlight(source, false, &generic));
        let edited = syntax.highlight("plain", false, &generic);
        assert_eq!(edited.text, "plain");
        let mut tex = crate::generic_highlight::GenericSyntaxHighlighter::default();
        let path = std::path::Path::new("sample.tex");
        let source = "\\newcommand{\\foo}{x} % TODO: test";
        let baseline = tex.highlight(source, Some(path), false);
        tex.set_decorations(true, &default_rules(), Color32::RED);
        let marked = tex.highlight(source, Some(path), false);
        assert_ne!(marked, baseline);
        assert_eq!(marked, tex.highlight(source, Some(path), false));
        tex.set_decorations(false, &[], Color32::RED);
        assert_eq!(baseline, tex.highlight(source, Some(path), false));
    }
    #[test]
    fn definitions_are_local_and_ignore_comments_strings_and_tex_command_prefixes() {
        let typ = "// #let fake = 1\n#let foo(x) = x\n#let foo = 2\n`#let nope = 3`";
        assert_eq!(
            definition_ranges(typ, true)
                .iter()
                .map(|r| &typ[r.clone()])
                .collect::<Vec<_>>(),
            ["foo", "foo"]
        );
        let tex = "% \\newcommand{\\fake}{}\n\\newcommand{\\foo}[1]{#1}\n\\renewcommand*{\\foo}{x}\n\\def\\bar{x}\n\\newcommandfake{\\nope}";
        assert_eq!(
            definition_ranges(tex, false)
                .iter()
                .map(|r| &tex[r.clone()])
                .collect::<Vec<_>>(),
            ["\\foo", "\\foo", "\\bar"]
        );
    }
    #[test]
    fn literal_unicode_overlaps_toggle_and_cache_keep_source_intact() {
        let mut decorations = Decorations::default();
        let mut rules = default_rules();
        rules.push(HighlightRule {
            text: "λ".into(),
            enabled: true,
            style: Default::default(),
        });
        let source = "TODO: λ #mycomment";
        decorations.prepare(source, true, false, &rules);
        decorations.prepare(source, true, false, &rules);
        assert_eq!(decorations.builds, 1);
        let mut job = LayoutJob::simple(
            source.into(),
            crate::theme::editor_font(),
            Color32::WHITE,
            f32::INFINITY,
        );
        decorations.apply(&mut job, Color32::RED);
        assert_eq!(job.text, source);
        assert!(
            job.sections.iter().any(
                |s| &job.text[s.byte_range.start.0..s.byte_range.end.0] == "λ"
                    && s.format.color == Color32::RED
            )
        );
        decorations.prepare(source, true, false, &[]);
        assert!(decorations.spans.is_empty());
    }
}

fn color32(color: crate::sublime_theme::Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

fn apply_style(format: &mut egui::TextFormat, style: &TypstStyleOverride, accent: Color32) {
    format.color = style.foreground.map_or(accent, color32);
    if let Some(color) = style.background {
        format.background = color32(color);
    }
    if let Some(weight) = style.weight {
        format.font_id = crate::theme::editor_font_with_weight(weight);
    }
    if let Some(italic) = style.italic {
        format.italics = italic;
    }
    if let Some(value) = style.underline {
        format.underline = if value {
            Stroke::new(1.0, format.color)
        } else {
            Stroke::NONE
        };
    }
    if let Some(value) = style.strikethrough {
        format.strikethrough = if value {
            Stroke::new(1.0, format.color)
        } else {
            Stroke::NONE
        };
    }
}

#[cfg(test)]
mod cost_tests {
    use super::*;
    #[test]
    #[ignore = "opt-in optimized decoration cache measurement"]
    fn decoration_cost_measurement() {
        if cfg!(debug_assertions) {
            panic!("use --release");
        }
        let source = (0..600)
            .map(|i| {
                format!(
                    "  #let value{i} = {i} // {}\n",
                    if i % 30 == 0 { "TODO: check" } else { "note" }
                )
            })
            .collect::<String>();
        let generic = crate::generic_highlight::GenericSyntaxHighlighter::default();
        let mut syntax = crate::highlight::SyntaxHighlighter::default();
        let rules = default_rules();
        let mut measure = |enabled| {
            let mut samples = Vec::new();
            for sample in 0..8 {
                let start = std::time::Instant::now();
                for _ in 0..100 {
                    syntax.set_decorations(false, if enabled { &rules } else { &[] }, Color32::RED);
                    let job = syntax.highlight(&source, false, &generic);
                    std::hint::black_box(job);
                }
                if sample >= 3 {
                    samples.push(start.elapsed().as_micros());
                }
            }
            samples.sort_unstable();
            samples[2]
        };
        let baseline = measure(false);
        let decorated = measure(true);
        eprintln!(
            "DECORATION_COST bytes={} rows=600 iterations=100 baseline_us={baseline} decorated_us={decorated}",
            source.len()
        );
    }
}
