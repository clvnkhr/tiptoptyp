use eframe::egui::{
    Color32,
    text::{ByteIndex, LayoutJob, LayoutSection},
};
#[cfg(test)]
use eframe::egui::{Stroke, TextFormat};
use typst_syntax::{LinkedNode, Source, SyntaxKind, Tag};

use crate::{
    generic_highlight::GenericSyntaxHighlighter, syntax_theme::ResolvedTypstStyles, theme,
};

/// Syntax highlighting backed by Typst's own error-tolerant parser.
///
/// Keeping a [`Source`] alive lets `Source::replace` incrementally reparse the
/// smallest changed region instead of rebuilding the syntax tree after every
/// keystroke. The completed layout job is cached as well because egui can ask
/// its layouter for the same text more than once in a frame.
pub struct SyntaxHighlighter {
    parsed_source: Source,
    cached_dark_mode: bool,
    cached_job: LayoutJob,
    has_cache: bool,
    cached_syntect_revision: u64,
    cached_code_mode: bool,
    styles: ResolvedTypstStyles,
    rainbow: Option<crate::rainbow::RainbowBrackets>,
    mitex_dollars: bool,
    #[cfg(test)]
    cache_builds: usize,
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self {
            parsed_source: Source::detached(String::new()),
            cached_dark_mode: true,
            cached_job: LayoutJob::default(),
            has_cache: false,
            cached_syntect_revision: 0,
            cached_code_mode: false,
            styles: ResolvedTypstStyles::default(),
            rainbow: None,
            mitex_dollars: false,
            #[cfg(test)]
            cache_builds: 0,
        }
    }
}

impl SyntaxHighlighter {
    pub(crate) fn set_mitex_dollars(&mut self, enabled: bool) {
        if self.mitex_dollars != enabled {
            self.mitex_dollars = enabled;
            self.has_cache = false;
        }
    }
    pub(crate) fn set_rainbow_brackets(&mut self, settings: crate::rainbow::RainbowBrackets) {
        if self.rainbow != Some(settings) {
            self.rainbow = Some(settings);
            self.has_cache = false;
        }
    }

    pub(crate) fn set_styles(&mut self, styles: ResolvedTypstStyles) {
        if self.styles != styles {
            self.styles = styles;
            self.has_cache = false;
        }
    }

    pub fn highlight(
        &mut self,
        source: &str,
        dark_mode: bool,
        syntect: &GenericSyntaxHighlighter,
    ) -> LayoutJob {
        self.highlight_mode(source, dark_mode, syntect, false)
    }

    /// Highlight a Typst snippet as code, matching the editor's `#{...}` mode.
    ///
    /// Typst parses bare fenced payloads as markup, which makes words such as
    /// `let` look like ordinary prose. Wrapping the payload in a synthetic
    /// code-mode expression gives the parser the same context as the editor;
    /// the synthetic delimiters are removed again before returning the job.
    pub(crate) fn highlight_code(
        &mut self,
        source: &str,
        dark_mode: bool,
        syntect: &GenericSyntaxHighlighter,
    ) -> LayoutJob {
        self.highlight_mode(source, dark_mode, syntect, true)
    }

    fn highlight_mode(
        &mut self,
        source: &str,
        dark_mode: bool,
        syntect: &GenericSyntaxHighlighter,
        code_mode: bool,
    ) -> LayoutJob {
        let _span = crate::performance::span("highlight.total");
        let parse_source = if code_mode {
            format!("#{{{source}}}")
        } else {
            source.to_owned()
        };
        let source_changed = self.parsed_source.text() != parse_source;
        let syntect_revision = syntect.theme_revision();
        if self.has_cache
            && !source_changed
            && self.cached_dark_mode == dark_mode
            && self.cached_syntect_revision == syntect_revision
            && self.cached_code_mode == code_mode
        {
            crate::performance::counter("highlight.cache_hit");
            return self.cached_job.clone();
        }

        crate::performance::counter("highlight.cache_miss");

        let _rebuild = crate::performance::span("highlight.rebuild");

        if source_changed {
            // `replace` computes the common prefix/suffix and reparses only the
            // changed syntax-tree region. It is the incremental API supplied by
            // Typst itself.
            self.parsed_source.replace(&parse_source);
        }

        #[cfg(test)]
        {
            self.cache_builds += 1;
        }
        let mut job = LayoutJob::default();
        let root = LinkedNode::new(self.parsed_source.root());
        append_node(
            &mut job,
            &root,
            None,
            dark_mode,
            &parse_source,
            &self.styles,
            syntect,
        );
        if let Some(rainbow) = self.rainbow {
            crate::rainbow::apply(&mut job, root, rainbow, dark_mode);
        }
        if self.mitex_dollars && !code_mode {
            let ranges = tiptoptyp::mitex_projection::dollar_payloads(&self.parsed_source);
            let mut sections = Vec::new();
            let mut start = 0;
            let copy = |sections: &mut Vec<LayoutSection>, range: std::ops::Range<usize>| {
                let first = job
                    .sections
                    .partition_point(|section| section.byte_range.end.0 <= range.start);
                for section in &job.sections[first..] {
                    if section.byte_range.start.0 >= range.end {
                        break;
                    }
                    let mut part = section.clone();
                    part.byte_range = ByteIndex(part.byte_range.start.0.max(range.start))
                        ..ByteIndex(part.byte_range.end.0.min(range.end));
                    sections.push(part);
                }
            };
            for range in ranges {
                copy(&mut sections, start..range.start);
                if let Some(tex) =
                    EmbeddedLanguage::TexMath.highlight(&source[range.clone()], dark_mode, syntect)
                {
                    sections.extend(tex.sections.into_iter().map(|mut section| {
                        section.byte_range = ByteIndex(section.byte_range.start.0 + range.start)
                            ..ByteIndex(section.byte_range.end.0 + range.start);
                        section
                    }));
                } else {
                    copy(&mut sections, range.clone());
                }
                start = range.end;
            }
            copy(&mut sections, start..source.len());
            job.sections = sections;
        }
        if code_mode {
            trim_layout_job(&mut job, 2, 1);
        }
        job.wrap.break_anywhere = false;

        // TextEdit requires the galley's byte positions to map exactly back to
        // the editable string. This catches accidental omission or injection if
        // Typst's syntax-tree representation changes in the future.
        debug_assert_eq!(job.text, source);

        self.cached_dark_mode = dark_mode;
        self.cached_syntect_revision = syntect_revision;
        self.cached_code_mode = code_mode;
        self.cached_job = job.clone();
        self.has_cache = true;
        job
    }
}

fn trim_layout_job(job: &mut LayoutJob, prefix_bytes: usize, suffix_bytes: usize) {
    let end = job.text.len().saturating_sub(suffix_bytes);
    let start = prefix_bytes.min(end);
    let text = job.text[start..end].to_owned();
    let sections = job
        .sections
        .iter()
        .filter_map(|section| {
            let section_start = section.byte_range.start.0.max(start);
            let section_end = section.byte_range.end.0.min(end);
            (section_start < section_end).then(|| LayoutSection {
                leading_space: if section_start == section.byte_range.start.0 {
                    section.leading_space
                } else {
                    0.0
                },
                byte_range: ByteIndex(section_start - start)..ByteIndex(section_end - start),
                format: section.format.clone(),
            })
        })
        .collect();
    job.text = text;
    job.sections = sections;
}

/// Walk leaves in source order. A tag on the closest node wins; this preserves
/// structural tags such as headings while allowing nested constructs (strong,
/// raw, interpolation, errors, and so on) to override them.
fn append_node(
    job: &mut LayoutJob,
    node: &LinkedNode<'_>,
    inherited: Option<Tag>,
    dark: bool,
    source: &str,
    styles: &ResolvedTypstStyles,
    syntect: &GenericSyntaxHighlighter,
) {
    if node.kind() == SyntaxKind::FuncCall
        && let Some(embedded) = embedded_literal(node, source)
    {
        let mut candidate = LayoutJob::default();
        if append_node_with_embedded(
            &mut candidate,
            node,
            inherited,
            dark,
            source,
            styles,
            syntect,
            &embedded,
        ) {
            append_layout_job(job, &candidate);
            return;
        }
    }
    if node.kind() == SyntaxKind::FuncCall
        && let Some(color) = color_call(node, source)
    {
        append_node_with_color(job, node, inherited, dark, source, styles, syntect, &color);
        return;
    }

    let tag = typst_syntax::highlight(node).or(inherited);
    let text = node.leaf_text();

    if text.is_empty() {
        if node.kind() == SyntaxKind::Raw
            && append_tagged_raw(job, node, dark, source, styles, syntect)
        {
            return;
        }
        for child in node.children() {
            append_node(job, &child, tag, dark, source, styles, syntect);
        }
    } else {
        job.append(text, 0.0, styles.format(tag));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EmbeddedLanguage {
    TexMath,
    TexText,
    Markdown,
}

impl EmbeddedLanguage {
    fn highlight(
        self,
        source: &str,
        dark: bool,
        syntect: &GenericSyntaxHighlighter,
    ) -> Option<LayoutJob> {
        match self {
            Self::TexMath => {
                let wrapped = format!("${source}$");
                let mut highlighted = syntect.highlight_token(&wrapped, "tex", dark)?;
                trim_layout_job(&mut highlighted, 1, 1);
                Some(highlighted)
            }
            Self::TexText => syntect.highlight_token(source, "tex", dark),
            Self::Markdown => syntect
                .highlight_token(source, "markdown", dark)
                .or_else(|| syntect.highlight_token(source, "md", dark)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EmbeddedLiteral {
    node_range: std::ops::Range<usize>,
    pub(crate) payload_range: std::ops::Range<usize>,
    pub(crate) language: EmbeddedLanguage,
    pub(crate) quoted: bool,
}

fn embedded_language(callee: &str) -> Option<EmbeddedLanguage> {
    let compact = callee
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let compact = compact
        .strip_prefix('(')
        .and_then(|callee| callee.strip_suffix(')'))
        .unwrap_or(&compact);
    let leaf = compact.rsplit('.').next().unwrap_or(compact);
    match leaf {
        "mi" | "mitex" | "mimath" | "mitex-convert" => Some(EmbeddedLanguage::TexMath),
        "mitext" => Some(EmbeddedLanguage::TexText),
        "render" | "render-with-metadata"
            if compact.starts_with("cmarker.") || compact.contains(".cmarker.") =>
        {
            Some(EmbeddedLanguage::Markdown)
        }
        _ => None,
    }
}

pub(crate) fn embedded_literal(node: &LinkedNode<'_>, source: &str) -> Option<EmbeddedLiteral> {
    let mut children = node.children();
    let callee = children.find(|child| !child.kind().is_trivia())?;
    let language = embedded_language(source.get(callee.range())?)?;
    let args = children.find(|child| child.kind() == SyntaxKind::Args)?;
    let literal = first_embedded_argument(&args, source, language)?;
    let node_range = literal.range();
    let payload_range = literal_payload_range(&literal)?;
    Some(EmbeddedLiteral {
        node_range,
        payload_range,
        language,
        quoted: literal.kind() == SyntaxKind::Str,
    })
}

fn first_embedded_argument<'a>(
    args: &LinkedNode<'a>,
    source: &str,
    language: EmbeddedLanguage,
) -> Option<LinkedNode<'a>> {
    for argument in args.children().filter(|child| !child.kind().is_trivia()) {
        match argument.kind() {
            SyntaxKind::Str | SyntaxKind::Raw => return Some(argument),
            SyntaxKind::Named => {
                let mut children = argument.children();
                let name = children
                    .find(|child| child.kind() == SyntaxKind::Ident)
                    .and_then(|child| source.get(child.range()));
                let expected = match language {
                    EmbeddedLanguage::Markdown => "markdown",
                    EmbeddedLanguage::TexMath | EmbeddedLanguage::TexText => "input",
                };
                if name == Some(expected)
                    && let Some(literal) = children
                        .find(|child| matches!(child.kind(), SyntaxKind::Str | SyntaxKind::Raw))
                {
                    return Some(literal);
                }
            }
            SyntaxKind::LeftParen | SyntaxKind::RightParen | SyntaxKind::Comma => {}
            _ => return None,
        }
    }
    None
}

pub(crate) fn literal_payload_range(literal: &LinkedNode<'_>) -> Option<std::ops::Range<usize>> {
    match literal.kind() {
        SyntaxKind::Str => {
            let range = literal.range();
            (range.end >= range.start + 2).then_some(range.start + 1..range.end - 1)
        }
        SyntaxKind::Raw => {
            let children = literal.children().collect::<Vec<_>>();
            let opening = children
                .first()
                .filter(|child| child.kind() == SyntaxKind::RawDelim)?;
            let closing = children
                .last()
                .filter(|child| child.kind() == SyntaxKind::RawDelim)?;
            let mut start = opening.range().end;
            if let Some(language) = children
                .iter()
                .find(|child| child.kind() == SyntaxKind::RawLang)
            {
                start = language.range().end;
            }
            (start <= closing.offset()).then_some(start..closing.offset())
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn append_node_with_embedded(
    job: &mut LayoutJob,
    node: &LinkedNode<'_>,
    inherited: Option<Tag>,
    dark: bool,
    source: &str,
    styles: &ResolvedTypstStyles,
    syntect: &GenericSyntaxHighlighter,
    embedded: &EmbeddedLiteral,
) -> bool {
    if node.range() == embedded.node_range {
        let Some(prefix) = source.get(node.range().start..embedded.payload_range.start) else {
            return false;
        };
        let Some(payload) = source.get(embedded.payload_range.clone()) else {
            return false;
        };
        let Some(suffix) = source.get(embedded.payload_range.end..node.range().end) else {
            return false;
        };
        let Some(highlighted) = embedded.language.highlight(payload, dark, syntect) else {
            return false;
        };
        let literal_tag = if node.kind() == SyntaxKind::Str {
            Some(Tag::String)
        } else {
            Some(Tag::Raw)
        };
        let literal_format = styles.format(literal_tag);
        job.append(prefix, 0.0, literal_format.clone());
        append_layout_job(job, &highlighted);
        job.append(suffix, 0.0, literal_format);
        return true;
    }

    let tag = typst_syntax::highlight(node).or(inherited);
    let text = node.leaf_text();
    if !text.is_empty() {
        job.append(text, 0.0, styles.format(tag));
        return true;
    }

    for child in node.children() {
        if child.range().start <= embedded.node_range.start
            && embedded.node_range.end <= child.range().end
        {
            if !append_node_with_embedded(job, &child, tag, dark, source, styles, syntect, embedded)
            {
                return false;
            }
        } else {
            append_node(job, &child, tag, dark, source, styles, syntect);
        }
    }
    true
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ColorCall {
    argument_ranges: Vec<std::ops::Range<usize>>,
    color: Color32,
}

fn color_call(node: &LinkedNode<'_>, source: &str) -> Option<ColorCall> {
    let mut children = node.children();
    let callee = children.find(|child| !child.kind().is_trivia())?;
    let name = source
        .get(callee.range())?
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let constructor = name.rsplit('.').next().unwrap_or(&name);
    let recognized = matches!(constructor, "rgb" | "luma" | "cmyk") && name == constructor
        || matches!(
            constructor,
            "oklab" | "oklch" | "linear-rgb" | "hsl" | "hsv"
        ) && name.strip_prefix("color.") == Some(constructor);
    if !recognized {
        return None;
    }

    let args = children.find(|child| child.kind() == SyntaxKind::Args)?;
    let arguments = literal_color_arguments(&args, source)?;
    let values = arguments
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>();
    let color = parse_color_constructor(constructor, &values)?;
    Some(ColorCall {
        argument_ranges: arguments.into_iter().map(|(range, _)| range).collect(),
        color,
    })
}

fn literal_color_arguments(
    args: &LinkedNode<'_>,
    source: &str,
) -> Option<Vec<(std::ops::Range<usize>, String)>> {
    let mut arguments = Vec::new();
    for child in args.children().filter(|child| !child.kind().is_trivia()) {
        match child.kind() {
            SyntaxKind::LeftParen | SyntaxKind::RightParen | SyntaxKind::Comma => {}
            SyntaxKind::Int
            | SyntaxKind::Float
            | SyntaxKind::Numeric
            | SyntaxKind::Str
            | SyntaxKind::Unary => {
                let range = child.range();
                arguments.push((range.clone(), source.get(range)?.to_owned()));
            }
            _ => return None,
        }
    }
    (!arguments.is_empty()).then_some(arguments)
}

#[allow(clippy::too_many_arguments)]
fn append_node_with_color(
    job: &mut LayoutJob,
    node: &LinkedNode<'_>,
    inherited: Option<Tag>,
    dark: bool,
    source: &str,
    styles: &ResolvedTypstStyles,
    syntect: &GenericSyntaxHighlighter,
    color: &ColorCall,
) {
    let tag = typst_syntax::highlight(node).or(inherited);
    let text = node.leaf_text();
    if !text.is_empty() {
        let mut format = styles.format(tag);
        if color
            .argument_ranges
            .iter()
            .any(|range| range.start <= node.range().start && node.range().end <= range.end)
        {
            let swatch = composite_over_editor(color.color, dark);
            format.background = swatch;
            format.color = contrast_text(swatch);
        }
        job.append(text, 0.0, format);
        return;
    }

    for child in node.children() {
        if color
            .argument_ranges
            .iter()
            .any(|range| child.range().start <= range.start && range.end <= child.range().end)
        {
            append_node_with_color(job, &child, tag, dark, source, styles, syntect, color);
        } else {
            append_node(job, &child, tag, dark, source, styles, syntect);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ColorComponent {
    Scalar(f32),
    Ratio(f32),
    Degrees(f32),
}

fn parse_color_component(text: &str) -> Option<ColorComponent> {
    let text = text.trim();
    if let Some(value) = text.strip_suffix('%') {
        return Some(ColorComponent::Ratio(value.parse::<f32>().ok()? / 100.0));
    }
    if let Some(value) = text.strip_suffix("deg") {
        return Some(ColorComponent::Degrees(value.parse().ok()?));
    }
    if let Some(value) = text.strip_suffix("rad") {
        return Some(ColorComponent::Degrees(
            value.parse::<f32>().ok()?.to_degrees(),
        ));
    }
    if let Some(value) = text.strip_suffix("turn") {
        return Some(ColorComponent::Degrees(value.parse::<f32>().ok()? * 360.0));
    }
    Some(ColorComponent::Scalar(text.parse().ok()?))
}

fn unit_component(text: &str, integers: bool) -> Option<f32> {
    match parse_color_component(text)? {
        ColorComponent::Ratio(value) if (0.0..=1.0).contains(&value) => Some(value),
        ColorComponent::Scalar(value)
            if integers && value.fract() == 0.0 && (0.0..=255.0).contains(&value) =>
        {
            Some(value / 255.0)
        }
        _ => None,
    }
}

fn angle_component(text: &str) -> Option<f32> {
    match parse_color_component(text)? {
        ColorComponent::Degrees(value) if value.is_finite() => Some(value.rem_euclid(360.0)),
        _ => None,
    }
}

fn oklab_axis_component(text: &str) -> Option<f32> {
    match parse_color_component(text)? {
        ColorComponent::Ratio(value) if (-1.0..=1.0).contains(&value) => Some(value * 0.4),
        _ => None,
    }
}

fn oklch_chroma_component(text: &str) -> Option<f32> {
    match parse_color_component(text)? {
        ColorComponent::Ratio(value) if (0.0..=1.0).contains(&value) => Some(value * 0.4),
        _ => None,
    }
}

fn optional_alpha(values: &[&str], index: usize, integers: bool) -> Option<f32> {
    values
        .get(index)
        .map_or(Some(1.0), |value| unit_component(value, integers))
}

fn parse_color_constructor(name: &str, values: &[&str]) -> Option<Color32> {
    let (red, green, blue, alpha) = match name {
        "rgb" if values.len() == 1 => {
            let color = parse_hex_color_string(values[0])?;
            return Some(color);
        }
        "rgb" if (3..=4).contains(&values.len()) => (
            unit_component(values[0], true)?,
            unit_component(values[1], true)?,
            unit_component(values[2], true)?,
            optional_alpha(values, 3, true)?,
        ),
        "luma" if (1..=2).contains(&values.len()) => {
            let lightness = unit_component(values[0], true)?;
            (
                lightness,
                lightness,
                lightness,
                optional_alpha(values, 1, false)?,
            )
        }
        "cmyk" if values.len() == 4 => {
            let cyan = unit_component(values[0], false)?;
            let magenta = unit_component(values[1], false)?;
            let yellow = unit_component(values[2], false)?;
            let key = unit_component(values[3], false)?;
            (
                (1.0 - cyan) * (1.0 - key),
                (1.0 - magenta) * (1.0 - key),
                (1.0 - yellow) * (1.0 - key),
                1.0,
            )
        }
        "linear-rgb" if (3..=4).contains(&values.len()) => (
            linear_to_srgb(unit_component(values[0], true)?),
            linear_to_srgb(unit_component(values[1], true)?),
            linear_to_srgb(unit_component(values[2], true)?),
            optional_alpha(values, 3, true)?,
        ),
        "hsl" if (3..=4).contains(&values.len()) => {
            let rgb = hsl_to_rgb(
                angle_component(values[0])?,
                unit_component(values[1], true)?,
                unit_component(values[2], true)?,
            );
            (rgb.0, rgb.1, rgb.2, optional_alpha(values, 3, true)?)
        }
        "hsv" if (3..=4).contains(&values.len()) => {
            let rgb = hsv_to_rgb(
                angle_component(values[0])?,
                unit_component(values[1], true)?,
                unit_component(values[2], true)?,
            );
            (rgb.0, rgb.1, rgb.2, optional_alpha(values, 3, true)?)
        }
        "oklab" if (3..=4).contains(&values.len()) => {
            let rgb = oklab_to_srgb(
                unit_component(values[0], false)?,
                oklab_axis_component(values[1])?,
                oklab_axis_component(values[2])?,
            );
            (rgb.0, rgb.1, rgb.2, optional_alpha(values, 3, false)?)
        }
        "oklch" if (3..=4).contains(&values.len()) => {
            let lightness = unit_component(values[0], false)?;
            let chroma = oklch_chroma_component(values[1])?;
            let hue = angle_component(values[2])?.to_radians();
            let rgb = oklab_to_srgb(lightness, chroma * hue.cos(), chroma * hue.sin());
            (rgb.0, rgb.1, rgb.2, optional_alpha(values, 3, false)?)
        }
        _ => return None,
    };
    Some(Color32::from_rgba_unmultiplied(
        float_channel(red),
        float_channel(green),
        float_channel(blue),
        float_channel(alpha),
    ))
}

fn float_channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> (f32, f32, f32) {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue / 60.0;
    let second = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match sector.floor() as u8 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let offset = lightness - chroma / 2.0;
    (red + offset, green + offset, blue + offset)
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> (f32, f32, f32) {
    let chroma = value * saturation;
    let sector = hue / 60.0;
    let second = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match sector.floor() as u8 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let offset = value - chroma;
    (red + offset, green + offset, blue + offset)
}

fn oklab_to_srgb(lightness: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let l = (lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    (
        linear_to_srgb(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        linear_to_srgb(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s),
        linear_to_srgb(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
    )
}

fn append_tagged_raw(
    job: &mut LayoutJob,
    node: &LinkedNode<'_>,
    dark: bool,
    source: &str,
    styles: &ResolvedTypstStyles,
    syntect: &GenericSyntaxHighlighter,
) -> bool {
    let children = node.children().collect::<Vec<_>>();
    let Some(language) = children
        .iter()
        .find(|child| child.kind() == SyntaxKind::RawLang)
    else {
        return false;
    };
    let Some(closing) = children
        .last()
        .filter(|child| child.kind() == SyntaxKind::RawDelim)
    else {
        return false;
    };
    let prefix_range = node.offset()..language.range().end;
    let content_range = language.range().end..closing.offset();
    let suffix_range = closing.offset()..node.range().end;
    let (Some(prefix), Some(content), Some(suffix)) = (
        source.get(prefix_range),
        source.get(content_range),
        source.get(suffix_range),
    ) else {
        return false;
    };
    let Some(highlighted) = syntect.highlight_token(content, language.leaf_text(), dark) else {
        return false;
    };

    let raw = styles.format(Some(Tag::Raw));
    job.append(prefix, 0.0, raw.clone());
    append_layout_job(job, &highlighted);
    job.append(suffix, 0.0, raw);
    true
}

fn append_layout_job(target: &mut LayoutJob, source: &LayoutJob) {
    for section in &source.sections {
        let Some(text) = source
            .text
            .get(section.byte_range.start.0..section.byte_range.end.0)
        else {
            continue;
        };
        target.append(text, section.leading_space, section.format.clone());
    }
}

#[cfg(test)]
fn format_for(tag: Option<Tag>, dark: bool) -> TextFormat {
    ResolvedTypstStyles::resolve(
        theme::default_syntax_palette(dark),
        None,
        &Default::default(),
    )
    .format(tag)
}

fn parse_hex_color_string(text: &str) -> Option<Color32> {
    let hex = text.strip_prefix('"')?.strip_suffix('"')?;
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    let expand = |digit: u8| (digit << 4) | digit;
    let nibble = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    let byte = |pair: &[u8]| Some((nibble(pair[0])? << 4) | nibble(pair[1])?);
    let bytes = hex.as_bytes();

    let (red, green, blue, alpha) = match bytes.len() {
        3 => (
            expand(nibble(bytes[0])?),
            expand(nibble(bytes[1])?),
            expand(nibble(bytes[2])?),
            255,
        ),
        4 => (
            expand(nibble(bytes[0])?),
            expand(nibble(bytes[1])?),
            expand(nibble(bytes[2])?),
            expand(nibble(bytes[3])?),
        ),
        6 => (
            byte(&bytes[0..2])?,
            byte(&bytes[2..4])?,
            byte(&bytes[4..6])?,
            255,
        ),
        8 => (
            byte(&bytes[0..2])?,
            byte(&bytes[2..4])?,
            byte(&bytes[4..6])?,
            byte(&bytes[6..8])?,
        ),
        _ => return None,
    };
    Some(Color32::from_rgba_unmultiplied(red, green, blue, alpha))
}

fn composite_over_editor(color: Color32, dark: bool) -> Color32 {
    composite_over(color, theme::default_syntax_palette(dark).editor_background)
}

fn composite_over(color: Color32, background: Color32) -> Color32 {
    if color.a() == 255 {
        return color;
    }
    let [red, green, blue, alpha] = color.to_srgba_unmultiplied();
    let alpha = alpha as u16;
    let blend = |foreground: u8, background: u8| {
        ((foreground as u16 * alpha + background as u16 * (255 - alpha) + 127) / 255) as u8
    };
    Color32::from_rgb(
        blend(red, background.r()),
        blend(green, background.g()),
        blend(blue, background.b()),
    )
}

fn contrast_text(background: Color32) -> Color32 {
    let linear = |channel: u8| {
        let channel = channel as f32 / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(background.r())
        + 0.7152 * linear(background.g())
        + 0.0722 * linear(background.b());
    let white_contrast = 1.05 / (luminance + 0.05);
    let black_contrast = (luminance + 0.05) / 0.05;
    if white_contrast >= black_contrast {
        Color32::WHITE
    } else {
        Color32::BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sublime_theme::Rgba,
        syntax_theme::{TypstStyleOverrides, TypstSyntaxRole},
    };

    fn highlight(source: &str, dark: bool) -> LayoutJob {
        let mut highlighter = SyntaxHighlighter::default();
        let syntect = GenericSyntaxHighlighter::default();
        highlighter.highlight(source, dark, &syntect)
    }

    fn format_at(job: &LayoutJob, byte: usize) -> &TextFormat {
        &job.sections
            .iter()
            .find(|section| section.byte_range.start.0 <= byte && byte < section.byte_range.end.0)
            .expect("the source byte should be covered by a layout section")
            .format
    }

    fn assert_exact_mapping(job: &LayoutJob, source: &str) {
        assert_eq!(job.text, source);

        let mut next_byte = 0;
        for section in &job.sections {
            assert_eq!(section.byte_range.start.0, next_byte);
            assert!(section.byte_range.end > section.byte_range.start);
            assert!(source.is_char_boundary(section.byte_range.start.0));
            assert!(source.is_char_boundary(section.byte_range.end.0));
            next_byte = section.byte_range.end.0;
        }
        assert_eq!(next_byte, source.len());
    }
    #[test]
    fn dollar_tex_highlighting_is_opt_in_mapped_and_cached() {
        let source = "文 $\\alpha + x_2$\n$\n  \\beta^2\n$\n`$not math$`";
        let syntect = GenericSyntaxHighlighter::default();
        let mut highlighter = SyntaxHighlighter::default();
        let normal = highlighter.highlight(source, true, &syntect);
        highlighter.set_mitex_dollars(false);
        assert_eq!(normal, highlighter.highlight(source, true, &syntect));
        assert_eq!(highlighter.cache_builds, 1);
        highlighter.set_mitex_dollars(true);
        let tex = highlighter.highlight(source, true, &syntect);
        assert_exact_mapping(&tex, source);
        assert_ne!(tex.sections, normal.sections);
        let expected = EmbeddedLanguage::TexMath
            .highlight("\\alpha + x_2", true, &syntect)
            .unwrap();
        assert_eq!(
            format_at(&tex, source.find("\\alpha").unwrap()),
            format_at(&expected, 0)
        );
        assert_eq!(tex, highlighter.highlight(source, true, &syntect));
        assert_eq!(highlighter.cache_builds, 2);
        highlighter.set_mitex_dollars(false);
        assert_eq!(normal, highlighter.highlight(source, true, &syntect));
    }

    #[test]
    fn rainbow_colors_preserve_mapping_and_only_rebuild_on_text_or_palette_changes() {
        use crate::rainbow::{BracketPalette, RainbowBrackets};
        let source = "#let α = (1, (2, (3, (4, (5)))))\n#let x = { [body] }\n$ (a + b] $\n#let str = \"[literal]\"";
        let mut highlighter = SyntaxHighlighter::default();
        let syntect = GenericSyntaxHighlighter::default();
        let baseline = highlighter.highlight(source, false, &syntect);
        let mut settings = RainbowBrackets::default();
        highlighter.set_rainbow_brackets(settings);
        let job = highlighter.highlight(source, false, &syntect);
        assert_exact_mapping(&job, source);
        let colors = settings.palettes[0].colors(false);
        let opens: Vec<_> = source
            .match_indices('(')
            .take(5)
            .map(|(byte, _)| format_at(&job, byte).color)
            .collect();
        assert_eq!(
            opens,
            vec![colors[0], colors[1], colors[2], colors[3], colors[0]]
        );
        for (byte, _) in source.char_indices() {
            let mut actual = format_at(&job, byte).clone();
            actual.color = format_at(&baseline, byte).color;
            assert_eq!(actual, *format_at(&baseline, byte));
        }
        let literal = source.find("[literal]").unwrap();
        assert_eq!(format_at(&job, literal), format_at(&baseline, literal));
        let builds = highlighter.cache_builds;
        highlighter.set_rainbow_brackets(settings);
        assert_eq!(highlighter.highlight(source, false, &syntect), job);
        assert_eq!(highlighter.cache_builds, builds);
        settings.palettes[0] = BracketPalette::Orchid;
        highlighter.set_rainbow_brackets(settings);
        let changed = highlighter.highlight(source, false, &syntect);
        assert_eq!(highlighter.cache_builds, builds + 1);
        assert_eq!(
            format_at(&changed, source.find('(').unwrap()).color,
            BracketPalette::Orchid.colors(false)[0]
        );
        let dark = highlighter.highlight(source, true, &syntect);
        assert_eq!(
            format_at(&dark, source.find('(').unwrap()).color,
            BracketPalette::Orchid.colors(true)[0]
        );
        settings.enabled = false;
        highlighter.set_rainbow_brackets(settings);
        assert_eq!(highlighter.highlight(source, false, &syntect), baseline);
    }

    #[test]
    fn markup_words_that_resemble_keywords_stay_plain() {
        let source = "The PDF preview updates as you type.";
        let job = highlight(source, true);
        let as_byte = source.find(" as ").unwrap() + 1;

        assert_exact_mapping(&job, source);
        assert_eq!(format_at(&job, as_byte).color, format_for(None, true).color);
        assert_ne!(
            format_at(&job, as_byte).color,
            format_for(Some(Tag::Keyword), true).color
        );
    }

    #[test]
    fn code_keywords_use_typsts_keyword_tag() {
        let source = "#import \"library.typ\" as library";
        let job = highlight(source, true);
        let as_byte = source.find(" as ").unwrap() + 1;

        assert_exact_mapping(&job, source);
        assert_eq!(
            format_at(&job, as_byte).color,
            format_for(Some(Tag::Keyword), true).color
        );
    }

    #[test]
    fn code_fenced_typst_payload_uses_code_mode_without_delimiters() {
        let source = "let value = 42\ntext(value)";
        let mut highlighter = SyntaxHighlighter::default();
        let syntect = GenericSyntaxHighlighter::default();
        let job = highlighter.highlight_code(source, true, &syntect);

        assert_exact_mapping(&job, source);
        assert_eq!(
            format_at(&job, source.find("let").unwrap()).color,
            format_for(Some(Tag::Keyword), true).color
        );
        assert_eq!(
            format_at(&job, source.find("42").unwrap()).color,
            format_for(Some(Tag::Number), true).color
        );
        assert_eq!(
            format_at(&job, source.find("text").unwrap()).color,
            format_for(Some(Tag::Function), true).color
        );
    }

    #[test]
    fn typst_formats_use_the_shared_editor_font_role() {
        for tag in [
            None,
            Some(Tag::Keyword),
            Some(Tag::String),
            Some(Tag::Error),
        ] {
            assert_eq!(format_for(tag, true).font_id, theme::editor_font());
            assert_eq!(format_for(tag, false).font_id, theme::editor_font());
        }
    }

    #[test]
    fn malformed_documents_are_highlighted_without_losing_text() {
        let mut highlighter = SyntaxHighlighter::default();
        let syntect = GenericSyntaxHighlighter::default();
        for source in [
            "#let unfinished =",
            "= *unterminated emphasis",
            "$ x + ",
            "#(missing: [bracket",
            "Unicode — λ and emoji 🙂\n#let x = ",
        ] {
            let job = highlighter.highlight(source, true, &syntect);
            assert_exact_mapping(&job, source);
        }
    }

    #[test]
    fn incremental_replacement_updates_tags_and_theme() {
        let mut highlighter = SyntaxHighlighter::default();
        let syntect = GenericSyntaxHighlighter::default();

        let markup = highlighter.highlight("as", true, &syntect);
        assert_eq!(format_at(&markup, 0).color, format_for(None, true).color);

        let source = "#let value = 42";
        highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::default_syntax_palette(true),
            None,
            &Default::default(),
        ));
        let dark = highlighter.highlight(source, true, &syntect);
        highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::default_syntax_palette(false),
            None,
            &Default::default(),
        ));
        let light = highlighter.highlight(source, false, &syntect);
        assert_exact_mapping(&dark, source);
        assert_exact_mapping(&light, source);
        assert_ne!(format_at(&dark, 1).color, format_at(&light, 1).color);

        // A repeat call exercises the completed LayoutJob cache.
        assert_eq!(highlighter.highlight(source, false, &syntect).text, source);
    }

    #[test]
    fn typst_overrides_flow_into_layout_formats() {
        let mut overrides = TypstStyleOverrides::default();
        let keyword = overrides.get_mut_or_default(TypstSyntaxRole::Keyword);
        keyword.foreground = Some(Rgba::rgb(1, 2, 3));
        keyword.background = Some(Rgba::from_rgba(4, 5, 6, 90));
        keyword.weight = Some(theme::FONT_WEIGHT_BOLD);
        keyword.italic = Some(true);
        keyword.underline = Some(true);
        keyword.strikethrough = Some(true);

        let mut highlighter = SyntaxHighlighter::default();
        highlighter.set_styles(ResolvedTypstStyles::resolve(
            theme::default_syntax_palette(true),
            None,
            &overrides,
        ));
        let syntect = GenericSyntaxHighlighter::default();
        let source = "#let value = 1";
        let job = highlighter.highlight(source, true, &syntect);
        let format = format_at(&job, source.find("let").unwrap());

        assert_eq!(format.color, Color32::from_rgb(1, 2, 3));
        assert_eq!(
            format.background,
            Color32::from_rgba_unmultiplied(4, 5, 6, 90)
        );
        assert_eq!(
            format.font_id,
            theme::editor_font_with_weight(theme::FONT_WEIGHT_BOLD)
        );
        assert!(format.italics);
        assert_ne!(format.underline, Stroke::NONE);
        assert_ne!(format.strikethrough, Stroke::NONE);
        assert_exact_mapping(&job, source);
    }

    #[test]
    fn tagged_raw_blocks_delegate_the_payload_to_syntect() {
        let source = "```rust\nfn main() {}\n```";
        let mut syntect_theme = syntect::highlighting::Theme::default();
        syntect_theme.settings.foreground = Some(syntect::highlighting::Color {
            r: 3,
            g: 97,
            b: 211,
            a: 255,
        });
        let mut syntect = GenericSyntaxHighlighter::default();
        syntect.set_custom_theme(Some(syntect_theme));
        let mut highlighter = SyntaxHighlighter::default();
        let job = highlighter.highlight(source, true, &syntect);
        let keyword = format_at(&job, source.find("fn").unwrap());
        let raw_delimiter = format_at(&job, 0);

        assert_eq!(keyword.color, Color32::from_rgb(3, 97, 211));
        assert_ne!(keyword.color, raw_delimiter.color);
        assert_exact_mapping(&job, source);
    }

    #[test]
    fn mitex_literals_use_tex_highlighting_without_changing_source_mapping() {
        for source in [
            r"#mi(`\frac{1}{2}`)",
            r"#mitex(input: `\begin{matrix}a&b\end{matrix}`)",
            r#"#mimath("\\alpha + \\beta")"#,
            r"#mitext(`\section{A text heading}`)",
        ] {
            let job = highlight(source, true);
            let command = source
                .find("frac")
                .or_else(|| source.find("begin"))
                .or_else(|| source.find("alpha"))
                .or_else(|| source.find("section"))
                .unwrap();
            let format = format_at(&job, command);
            let literal_tag = if source[..command].rfind('`') > source[..command].rfind('"') {
                Some(Tag::Raw)
            } else {
                Some(Tag::String)
            };

            assert_ne!(format.color, format_for(literal_tag, true).color);
            assert_exact_mapping(&job, source);
        }
    }

    #[test]
    fn cmarker_positional_and_named_literals_use_markdown_highlighting() {
        for source in [
            "#cmarker.render(`# Heading\n\n*emphasis*`)",
            "#(cmarker.render)(`# Heading\n\n*emphasis*`)",
            "#cmarker.render-with-metadata(markdown: \"# Heading\\n\\n*emphasis*\")",
        ] {
            let job = highlight(source, false);
            let heading = source.find("Heading").unwrap();
            let literal_tag = if source[..heading].rfind('`') > source[..heading].rfind('"') {
                Some(Tag::Raw)
            } else {
                Some(Tag::String)
            };

            assert_ne!(
                format_at(&job, heading).color,
                format_for(literal_tag, false).color
            );
            assert_exact_mapping(&job, source);
        }
    }

    #[test]
    fn unrelated_render_calls_keep_ordinary_typst_string_highlighting() {
        let source = r##"#render("# not embedded markdown")"##;
        let job = highlight(source, true);
        let payload = source.find("not embedded").unwrap();

        assert_eq!(
            format_at(&job, payload).color,
            format_for(Some(Tag::String), true).color
        );
        assert_exact_mapping(&job, source);
    }

    #[test]
    fn unknown_raw_language_falls_back_to_typst_raw_style() {
        let source = "```not-a-language\nplain payload\n```";
        let job = highlight(source, true);
        let payload = format_at(&job, source.find("plain").unwrap());
        assert_eq!(payload.color, format_for(Some(Tag::Raw), true).color);
        assert_exact_mapping(&job, source);
    }

    #[test]
    fn empty_source_has_an_exact_empty_mapping() {
        let job = highlight("", true);
        assert_exact_mapping(&job, "");
    }

    #[test]
    fn hex_color_strings_become_readable_color_swatches() {
        let source = r##"#let accent = rgb("#4f8cff")"##;
        let job = highlight(source, true);
        let format = format_at(&job, source.find("#4f8cff").unwrap());

        assert_eq!(format.background, Color32::from_rgb(0x4f, 0x8c, 0xff));
        assert_eq!(format.color, Color32::BLACK);
        assert_exact_mapping(&job, source);
    }

    #[test]
    fn process_color_constructors_fill_each_literal_with_the_resulting_color() {
        for (source, needle, expected) in [
            ("#luma(128)", "128", Color32::from_rgb(128, 128, 128)),
            ("#cmyk(0%, 100%, 100%, 0%)", "100%", Color32::RED),
            ("#color.linear-rgb(100%, 0%, 0%)", "100%", Color32::RED),
            ("#color.hsl(120deg, 100%, 50%)", "120deg", Color32::GREEN),
            ("#color.hsv(240deg, 100%, 100%)", "240deg", Color32::BLUE),
            ("#color.oklab(100%, 0%, 0%)", "100%", Color32::WHITE),
            ("#color.oklch(0%, 0%, 30deg)", "30deg", Color32::BLACK),
            (
                r##"#rgb("239dad")"##,
                "239dad",
                Color32::from_rgb(0x23, 0x9d, 0xad),
            ),
        ] {
            let job = highlight(source, true);
            assert_eq!(
                format_at(&job, source.find(needle).unwrap()).background,
                expected
            );
            assert_exact_mapping(&job, source);
        }
    }

    #[test]
    fn non_color_literals_are_not_mistaken_for_swatches() {
        for source in [
            r##"#let issue = "#4f8cff""##,
            "#hsl(120deg, 100%, 50%)",
            "#color.oklab(100%, 0, 0)",
            "#rgb(256, 0, 0)",
        ] {
            let job = highlight(source, true);
            assert!(
                job.sections
                    .iter()
                    .all(|section| section.format.background == Color32::TRANSPARENT)
            );
            assert_exact_mapping(&job, source);
        }
    }

    #[test]
    fn shorthand_and_alpha_hex_colors_are_parsed_safely() {
        assert_eq!(
            parse_hex_color_string(r##""#abc""##),
            Some(Color32::from_rgb(0xaa, 0xbb, 0xcc))
        );
        assert_eq!(
            parse_hex_color_string(r##""#10203080""##),
            Some(Color32::from_rgba_unmultiplied(0x10, 0x20, 0x30, 0x80))
        );
        assert_eq!(
            parse_hex_color_string(r#""239dad""#),
            Some(Color32::from_rgb(0x23, 0x9d, 0xad))
        );
        assert_eq!(parse_hex_color_string(r##""#not-a-color""##), None);
        assert_eq!(contrast_text(Color32::BLACK), Color32::WHITE);
        assert_eq!(contrast_text(Color32::WHITE), Color32::BLACK);
    }

    #[test]
    fn transparent_swatches_composite_unpremultiplied_channels_once() {
        let half_red = Color32::from_rgba_unmultiplied(255, 0, 0, 128);
        assert_eq!(
            composite_over(half_red, Color32::WHITE),
            Color32::from_rgb(255, 127, 127)
        );
        assert_eq!(
            composite_over(half_red, Color32::BLACK),
            Color32::from_rgb(128, 0, 0)
        );
    }
}
