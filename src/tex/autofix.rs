//! Explicitly enabled TeX corrections, planned against one immutable snapshot.
//! Badness supplies parser-owned fixes; source rules cover references split
//! over lines and blank paragraphs in structurally paired display math.
use crate::{
    comment_controls::{self, Feature, Regions},
    delimiters::TexDelimiterIndex,
    document::DocumentKind,
    private_workspace::PrivateWorkspace,
    process::OwnedChild,
    toolchain::ToolResolution,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read},
    ops::Range,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Rule {
    ReferenceTies,
    CitationTies,
    DisplayMathParagraphs,
    BlankLinesInKeyValues,
    DeprecatedCommands,
    PrimitiveScripts,
    DollarDisplayMath,
    TextEllipses,
    MathEllipses,
    RedundantScriptBraces,
    SwallowedSpaces,
    SpaceBeforeCommands,
    AbbreviationSpacing,
    StraightQuotes,
    NumberRangeDashes,
    MultiplicationSigns,
    MathOperatorNames,
    ExtraMathLinebreaks,
    LabelPositions,
    ObsoleteEnvironments,
    DeprecatedSuppressions,
}
impl Rule {
    pub(crate) const ALL: &'static [Self] = &[
        Self::ReferenceTies,
        Self::CitationTies,
        Self::DisplayMathParagraphs,
        Self::BlankLinesInKeyValues,
        Self::DeprecatedCommands,
        Self::PrimitiveScripts,
        Self::DollarDisplayMath,
        Self::TextEllipses,
        Self::MathEllipses,
        Self::RedundantScriptBraces,
        Self::SwallowedSpaces,
        Self::SpaceBeforeCommands,
        Self::AbbreviationSpacing,
        Self::StraightQuotes,
        Self::NumberRangeDashes,
        Self::MultiplicationSigns,
        Self::MathOperatorNames,
        Self::ExtraMathLinebreaks,
        Self::LabelPositions,
        Self::ObsoleteEnvironments,
        Self::DeprecatedSuppressions,
    ];
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::ReferenceTies => "reference-ties",
            Self::CitationTies => "citation-ties",
            Self::DisplayMathParagraphs => "display-math-paragraphs",
            Self::BlankLinesInKeyValues => "blank-lines-in-key-values",
            Self::DeprecatedCommands => "deprecated-commands",
            Self::PrimitiveScripts => "primitive-scripts",
            Self::DollarDisplayMath => "dollar-display-math",
            Self::TextEllipses => "text-ellipses",
            Self::MathEllipses => "math-ellipses",
            Self::RedundantScriptBraces => "redundant-script-braces",
            Self::SwallowedSpaces => "swallowed-spaces",
            Self::SpaceBeforeCommands => "space-before-commands",
            Self::AbbreviationSpacing => "abbreviation-spacing",
            Self::StraightQuotes => "straight-quotes",
            Self::NumberRangeDashes => "number-range-dashes",
            Self::MultiplicationSigns => "multiplication-signs",
            Self::MathOperatorNames => "math-operator-names",
            Self::ExtraMathLinebreaks => "extra-math-linebreaks",
            Self::LabelPositions => "label-positions",
            Self::ObsoleteEnvironments => "obsolete-environments",
            Self::DeprecatedSuppressions => "deprecated-suppressions",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ReferenceTies => "Non-breaking spaces before references",
            Self::CitationTies => "Non-breaking spaces before bracketed citations",
            Self::DisplayMathParagraphs => "Blank lines inside display math",
            Self::BlankLinesInKeyValues => "Blank lines in key-value arguments",
            Self::DeprecatedCommands => "Deprecated font commands",
            Self::PrimitiveScripts => "Primitive subscript and superscript commands",
            Self::DollarDisplayMath => "Use \\[...\\] instead of $$...$$",
            Self::TextEllipses => "Ellipses in text",
            Self::MathEllipses => "Ellipses in math",
            Self::RedundantScriptBraces => "Redundant script braces",
            Self::SwallowedSpaces => "Spaces swallowed by TeX logo commands",
            Self::SpaceBeforeCommands => "Unwanted spaces before footnotes and labels",
            Self::AbbreviationSpacing => "Spacing after abbreviations and acronyms",
            Self::StraightQuotes => "Straight quotation marks",
            Self::NumberRangeDashes => "Dashes in number ranges",
            Self::MultiplicationSigns => "Multiplication signs between numbers",
            Self::MathOperatorNames => "Backslashes before math operator names",
            Self::ExtraMathLinebreaks => "Extra final math line breaks",
            Self::LabelPositions => "Labels before captions or list items",
            Self::ObsoleteEnvironments => "Replace obsolete math environments",
            Self::DeprecatedSuppressions => "Obsolete Badness suppression comments",
        }
    }
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::ReferenceTies => {
                "Replace whitespace before \\ref, \\eqref and \\pageref with ~, including source line breaks."
            }
            Self::CitationTies => {
                "Keep bracketed citation commands with the preceding word using ~. Self-describing references and textual citations are excluded."
            }
            Self::DisplayMathParagraphs => {
                "Add % to empty lines inside paired \\[...\\] or display environments, preserving line numbers and indentation."
            }
            Self::BlankLinesInKeyValues => {
                "Remove a paragraph break only where Badness proves a key-value processor rejects it."
            }
            Self::DeprecatedCommands => {
                "Replace obsolete font switches with the parser's modern declaration. Locally redefined commands are excluded."
            }
            Self::PrimitiveScripts => {
                "Replace \\sb and \\sp with _ and ^ only where the parser supplies an equivalent fix."
            }
            Self::DollarDisplayMath => {
                "Change both matched display delimiters. Preserve the formula exactly."
            }
            Self::TextEllipses => {
                "Replace literal text ellipses with \\dots, keeping adjacent letters separate."
            }
            Self::MathEllipses => {
                "Let Badness choose \\ldots or \\cdots from nearby operators. Changes typeset math spacing."
            }
            Self::RedundantScriptBraces => {
                "Remove braces around a single script token only when the parser verifies token boundaries stay intact."
            }
            Self::SwallowedSpaces => {
                "Insert {} after argument-free logo commands such as \\LaTeX so the following space is printed."
            }
            Self::SpaceBeforeCommands => {
                "Apply parser-provided spacing fixes before footnotes, footnote marks, index entries and labels. Changes typeset spacing."
            }
            Self::AbbreviationSpacing => {
                "Apply parser-provided sentence/interword spacing corrections. Changes typeset spacing."
            }
            Self::StraightQuotes => {
                "Choose opening and closing TeX quotes from context. May change the intended quotation direction."
            }
            Self::NumberRangeDashes => {
                "Use an en dash between numbers when Badness identifies a range. A literal hyphen can be intentional."
            }
            Self::MultiplicationSigns => {
                "Replace x between numeric operands with a multiplication sign. A variable x can be intentional."
            }
            Self::MathOperatorNames => {
                "Interpret bare sin, log and similar names as operators. A product of variables can be intentional."
            }
            Self::ExtraMathLinebreaks => {
                "Remove parser-identified empty final equation rows. Can change spacing and equation numbering."
            }
            Self::LabelPositions => {
                "Move a label after its proven caption or item. Changes the number returned by references."
            }
            Self::ObsoleteEnvironments => {
                "Apply paired environment renames supplied by Badness. Review amsmath requirements and alignment layout."
            }
            Self::DeprecatedSuppressions => {
                "Update an obsolete Badness suppression spelling while retaining the rule selector and explanation."
            }
        }
    }
    /// Maps stable provider codes, never free-form compiler messages. The
    /// action still rescans the current source before admitting any edit.
    pub(crate) fn for_diagnostic(code: &str, message: &str) -> Option<Self> {
        let code = code.trim().trim_matches(['"', '\'']);
        match code {
            "missing-nonbreaking-space" => {
                if ["ref", "eqref", "pageref"]
                    .iter()
                    .any(|name| mentions_command(message, name))
                {
                    Some(Self::ReferenceTies)
                } else if [
                    "cite",
                    "citep",
                    "parencite",
                    "autocite",
                    "footcite",
                    "footfullcite",
                ]
                .iter()
                .any(|name| mentions_command(message, name))
                {
                    Some(Self::CitationTies)
                } else {
                    None
                }
            }
            "math-paragraph-break" => Some(Self::DisplayMathParagraphs),
            "ellipsis"
                if mentions_command(message, "ldots") || mentions_command(message, "cdots") =>
            {
                Some(Self::MathEllipses)
            }
            "ellipsis" if mentions_command(message, "dots") => Some(Self::TextEllipses),
            "ellipsis" => None,
            "primitive-command"
                if !mentions_command(message, "sb") && !mentions_command(message, "sp") =>
            {
                None
            }
            _ => Self::ALL
                .iter()
                .copied()
                .find(|rule| rule.provider_code() == Some(code)),
        }
    }
    pub(crate) fn enabled(self, settings: &Settings) -> bool {
        settings.enabled.contains(&self)
    }
    pub(crate) fn set_enabled(self, settings: &mut Settings, enabled: bool) {
        if enabled {
            settings.enabled.insert(self);
        } else {
            settings.enabled.remove(&self);
        }
    }
    fn provider_code(self) -> Option<&'static str> {
        Some(match self {
            Self::ReferenceTies | Self::DisplayMathParagraphs => return None,
            Self::CitationTies => "missing-nonbreaking-space",
            Self::BlankLinesInKeyValues => "blank-line-in-keyval",
            Self::DeprecatedCommands => "deprecated-command",
            Self::PrimitiveScripts => "primitive-command",
            Self::DollarDisplayMath => "dollar-display-math",
            Self::TextEllipses | Self::MathEllipses => "ellipsis",
            Self::RedundantScriptBraces => "redundant-script-braces",
            Self::SwallowedSpaces => "swallowed-space",
            Self::SpaceBeforeCommands => "space-before-command",
            Self::AbbreviationSpacing => "abbreviation-spacing",
            Self::StraightQuotes => "straight-quotes",
            Self::NumberRangeDashes => "dash-length",
            Self::MultiplicationSigns => "times-variable",
            Self::MathOperatorNames => "math-operator-name",
            Self::ExtraMathLinebreaks => "extra-math-linebreak",
            Self::LabelPositions => "label-before-caption",
            Self::ObsoleteEnvironments => "obsolete-environment",
            Self::DeprecatedSuppressions => "deprecated-suppression-syntax",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) enabled: BTreeSet<Rule>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: [
                Rule::ReferenceTies,
                Rule::DisplayMathParagraphs,
                Rule::BlankLinesInKeyValues,
                Rule::DeprecatedCommands,
                Rule::PrimitiveScripts,
                Rule::DollarDisplayMath,
                Rule::TextEllipses,
                Rule::DeprecatedSuppressions,
            ]
            .into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edit {
    pub(crate) range: Range<usize>,
    pub(crate) original: String,
    pub(crate) replacement: String,
    pub(crate) rule: Rule,
}
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    pub(crate) edits: Vec<Edit>,
    pub(crate) fixes: usize,
    pub(crate) warning: Option<String>,
    source: String,
}
impl Plan {
    pub(crate) fn apply(&self, source: &str) -> Result<String, &'static str> {
        if source != self.source {
            return Err("The document changed while autofixing.");
        }
        let mut previous = 0;
        for edit in &self.edits {
            if edit.range.start < previous || source.get(edit.range.clone()) != Some(&edit.original)
            {
                return Err("The autofix edits no longer match the document.");
            }
            previous = edit.range.end;
        }
        let mut result = String::with_capacity(source.len());
        previous = 0;
        for edit in &self.edits {
            result.push_str(&source[previous..edit.range.start]);
            result.push_str(&edit.replacement);
            previous = edit.range.end;
        }
        result.push_str(&source[previous..]);
        Ok(result)
    }
}

#[derive(Debug, Deserialize)]
struct Finding {
    rule: String,
    start: usize,
    end: usize,
    fix: Option<ProviderFix>,
}
#[derive(Debug, Deserialize)]
struct ProviderFix {
    edits: Vec<ProviderEdit>,
    applicability: String,
}
#[derive(Debug, Deserialize)]
struct ProviderEdit {
    start: usize,
    end: usize,
    content: String,
}

struct Context<'a> {
    source: &'a str,
    delimiters: TexDelimiterIndex,
    regions: Regions,
    lines: Vec<usize>,
    definitions: Vec<Range<usize>>,
}
impl<'a> Context<'a> {
    fn new(source: &'a str, markers: &comment_controls::Settings) -> Self {
        let mut lines = vec![0];
        lines.extend(source.match_indices('\n').map(|(at, _)| at + 1));
        let mut context = Self {
            source,
            delimiters: TexDelimiterIndex::new(source),
            regions: Regions::parse(source, DocumentKind::Tex, markers, Feature::Diagnostics),
            lines,
            definitions: Vec::new(),
        };
        context.definitions = definition_ranges(&context);
        context
    }
    fn inside_definition(&self, byte: usize) -> bool {
        let after = self
            .definitions
            .partition_point(|range| range.start <= byte);
        after
            .checked_sub(1)
            .is_some_and(|index| self.definitions[index].contains(&byte))
    }
    fn admits(&self, range: &Range<usize>, comments: bool) -> bool {
        if self.source.get(range.clone()).is_none() {
            return false;
        }
        let first = self.lines.partition_point(|&start| start <= range.start);
        let last = self
            .lines
            .partition_point(|&start| start <= range.end.saturating_sub(1).max(range.start));
        if (first..=last)
            .any(|line| self.regions.contains_line(line) || self.regions.contains_marker_line(line))
        {
            return false;
        }
        if comments {
            return true;
        }
        let ignored = self.delimiters.ignored_ranges();
        let after = ignored.partition_point(|ignored| ignored.end <= range.start);
        ignored.get(after).is_none_or(|ignored| {
            if range.is_empty() {
                !ignored.contains(&range.start)
            } else {
                ignored.start >= range.end
            }
        })
    }
    fn edit(&self, range: Range<usize>, replacement: &str, rule: Rule) -> Option<Edit> {
        let original = self.source.get(range.clone())?;
        (original != replacement && self.admits(&range, rule == Rule::DeprecatedSuppressions)).then(
            || Edit {
                range,
                original: original.into(),
                replacement: replacement.into(),
                rule,
            },
        )
    }
}

/// Runs only on the owning background job. The CLI reads a private source
/// snapshot on stdin and never receives a writable user-file path or --fix.
pub(crate) fn compute(
    source: &str,
    settings: &Settings,
    markers: &comment_controls::Settings,
    project_root: &Path,
    tool: Option<&ToolResolution>,
    cancel: &AtomicBool,
) -> Result<Plan, String> {
    cancelled(cancel)?;
    let context = Context::new(source, markers);
    let initial = finish_plan(source, source_candidates(&context, settings, cancel)?, None);
    let staged = initial.apply(source).map_err(str::to_owned)?;
    let mapping = SourceMap::new(&initial.edits);
    let mut candidates: Vec<_> = initial.edits.into_iter().map(|edit| vec![edit]).collect();
    let codes: BTreeSet<_> = settings
        .enabled
        .iter()
        .filter_map(|rule| rule.provider_code())
        .collect();
    let mut warning = None;
    if !codes.is_empty() {
        if let Some(tool) = tool {
            match run_provider(&staged, project_root, tool, &codes, cancel) {
                Ok(findings) => {
                    let staged_context = Context::new(&staged, markers);
                    candidates.extend(
                        provider_candidates(&staged_context, settings, findings)
                            .into_iter()
                            .filter_map(|edits| mapping.rebase(&context, edits)),
                    );
                }
                Err(error) => {
                    cancelled(cancel)?;
                    warning = Some(error);
                }
            }
        } else {
            warning = Some(
                "Additional autofixes need Badness. Enable or install it in Settings → Tools."
                    .into(),
            );
        }
    }
    cancelled(cancel)?;
    Ok(finish_plan(source, candidates, warning))
}

/// Source corrections happen before parser fixes so a repaired paragraph does
/// not conceal a convertible display. Map provider spans back to the original
/// snapshot; edits touching newly inserted/replaced text remain a later choice.
struct SourceMap {
    changes: Vec<(Range<usize>, Range<usize>)>,
}
impl SourceMap {
    fn new(edits: &[Edit]) -> Self {
        let mut changes = Vec::with_capacity(edits.len());
        let (mut original, mut staged) = (0, 0);
        for edit in edits {
            let start = staged + edit.range.start - original;
            let end = start + edit.replacement.len();
            changes.push((edit.range.clone(), start..end));
            original = edit.range.end;
            staged = end;
        }
        Self { changes }
    }
    fn point(&self, point: usize) -> Option<usize> {
        let after = self
            .changes
            .partition_point(|(_, staged)| staged.end <= point);
        if self
            .changes
            .get(after)
            .is_some_and(|(_, staged)| staged.start < point && point < staged.end)
        {
            return None;
        }
        if let Some((original, staged)) = self.changes.get(after)
            && point == staged.start
        {
            return Some(original.start);
        }
        Some(if after == 0 {
            point
        } else {
            let (original, staged) = &self.changes[after - 1];
            original.end + point - staged.end
        })
    }
    fn rebase(&self, context: &Context<'_>, edits: Vec<Edit>) -> Option<Vec<Edit>> {
        edits
            .into_iter()
            .map(|edit| {
                let after = self
                    .changes
                    .partition_point(|(_, staged)| staged.end <= edit.range.start);
                if self.changes.get(after).is_some_and(|(_, staged)| {
                    if edit.range.is_empty() {
                        staged.contains(&edit.range.start)
                    } else {
                        staged.start < edit.range.end
                    }
                }) {
                    return None;
                }
                let range = self.point(edit.range.start)?..self.point(edit.range.end)?;
                if context.source.get(range.clone()) != Some(edit.original.as_str()) {
                    return None;
                }
                context.edit(range, &edit.replacement, edit.rule)
            })
            .collect()
    }
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Autofixing cancelled.".into())
    } else {
        Ok(())
    }
}

fn run_provider(
    source: &str,
    root: &Path,
    tool: &ToolResolution,
    codes: &BTreeSet<&str>,
    cancel: &AtomicBool,
) -> Result<Vec<Finding>, String> {
    const LIMIT: u64 = 64 * 1024 * 1024;
    let run = || -> Result<Vec<Finding>, String> {
        let scratch = PrivateWorkspace::open(root)
            .and_then(|private| private.temp_dir("autofix-"))
            .map_err(|e| e.to_string())?;
        let input = scratch.path().join("stdin.tex");
        let output = scratch.path().join("findings.json");
        let error = scratch.path().join("stderr.log");
        fs::write(&input, source).map_err(|e| e.to_string())?;
        let mut command = Command::new(&tool.program);
        command.current_dir(root).args([
            "lint",
            "--no-config",
            "--output",
            "json",
            "--stdin-filepath",
            "document.tex",
        ]);
        for code in codes {
            command.args(["--select", code]);
        }
        command.arg("-");
        let mut customization = tool.command.clone();
        customization.arguments.clear();
        customization
            .apply(&mut command)
            .map_err(|error| error.to_string())?;
        command
            .stdin(File::open(input).map_err(|e| e.to_string())?)
            .stdout(File::create(&output).map_err(|e| e.to_string())?)
            .stderr(Stdio::from(
                File::create(&error).map_err(|e| e.to_string())?,
            ));
        let status = OwnedChild::spawn(&mut command)
            .map_err(|e| e.to_string())?
            .wait(Duration::from_secs(30), || {
                if cancel.load(Ordering::Acquire) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Autofixing cancelled.",
                    ));
                }
                if [&output, &error]
                    .iter()
                    .any(|path| fs::metadata(path).is_ok_and(|meta| meta.len() > LIMIT))
                {
                    return Err(io::Error::other("Autofix output exceeded its limit."));
                }
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        // Badness uses exit 1 for findings, including completely valid JSON.
        if !status.success() && status.code() != Some(1) {
            return Err(format!("Badness exited with {status}."));
        }
        let mut bytes = Vec::new();
        File::open(&output)
            .and_then(|file| file.take(LIMIT + 1).read_to_end(&mut bytes))
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > LIMIT {
            return Err("Autofix output exceeded its limit.".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "Badness returned invalid autofix data.".into())
    };
    run().map_err(|error| format!("Could not check TeX autofixes: {error}"))
}

fn provider_candidates(
    context: &Context<'_>,
    settings: &Settings,
    findings: Vec<Finding>,
) -> Vec<Vec<Edit>> {
    let mut candidates = Vec::new();
    for finding in findings {
        if !context.admits(
            &(finding.start..finding.end),
            finding.rule == "deprecated-suppression-syntax",
        ) {
            continue;
        }
        let Some(fix) = finding.fix else {
            continue;
        };
        if !matches!(fix.applicability.as_str(), "safe" | "unsafe") {
            continue;
        }
        let Some(rule) = settings.enabled.iter().copied().find(|rule| {
            rule.provider_code() == Some(finding.rule.as_str())
                && match rule {
                    Rule::TextEllipses => fix.applicability == "safe",
                    Rule::MathEllipses => fix.applicability == "unsafe",
                    _ => true,
                }
        }) else {
            continue;
        };
        if fix.applicability == "unsafe"
            && !matches!(
                rule,
                Rule::CitationTies
                    | Rule::MathEllipses
                    | Rule::SwallowedSpaces
                    | Rule::SpaceBeforeCommands
                    | Rule::AbbreviationSpacing
                    | Rule::StraightQuotes
                    | Rule::NumberRangeDashes
                    | Rule::MultiplicationSigns
                    | Rule::MathOperatorNames
                    | Rule::ExtraMathLinebreaks
                    | Rule::LabelPositions
            )
        {
            continue;
        }
        if rule == Rule::CitationTies {
            let Some((_, command, _)) = following_command(context.source, finding.start) else {
                continue;
            };
            if matches!(command, "ref" | "eqref" | "pageref") {
                continue;
            }
        }
        let edits: Option<Vec<_>> = fix
            .edits
            .into_iter()
            .map(|edit| context.edit(edit.start..edit.end, &edit.content, rule))
            .collect();
        if let Some(mut edits) = edits.filter(|edits| !edits.is_empty()) {
            edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
            if edits
                .windows(2)
                .all(|pair| !overlap(&pair[0].range, &pair[1].range))
            {
                candidates.push(edits);
            }
        }
    }
    candidates
}

fn mentions_command(message: &str, name: &str) -> bool {
    let spelling = format!("\\{name}");
    message.match_indices(&spelling).any(|(at, text)| {
        !message[at + text.len()..].starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '@')
    })
}

fn following_command(source: &str, mut at: usize) -> Option<(usize, &str, usize)> {
    while let Some(ch) = source
        .get(at..)?
        .chars()
        .next()
        .filter(|ch| ch.is_whitespace())
    {
        at += ch.len_utf8();
    }
    if source.as_bytes().get(at) != Some(&b'\\') {
        return None;
    }
    let start = at;
    at += 1;
    while source
        .as_bytes()
        .get(at)
        .is_some_and(u8::is_ascii_alphabetic)
    {
        at += 1;
    }
    Some((start, &source[start + 1..at], at))
}

fn source_candidates(
    context: &Context<'_>,
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<Vec<Vec<Edit>>, String> {
    let source = context.source;
    let mut candidates = Vec::new();
    if Rule::ReferenceTies.enabled(settings) {
        let redefined = redefined_references(context);
        let mut at = 0;
        while at < source.len() {
            if at % 4096 == 0 {
                cancelled(cancel)?;
            }
            if source.as_bytes()[at] != b'\\' || context.delimiters.ignored_at(at) {
                at += source[at..].chars().next().unwrap().len_utf8();
                continue;
            }
            let start = at;
            at += 1;
            if !source
                .as_bytes()
                .get(at)
                .is_some_and(u8::is_ascii_alphabetic)
            {
                at += source[at..].chars().next().map_or(0, char::len_utf8);
                continue;
            }
            while source
                .as_bytes()
                .get(at)
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'@')
            {
                at += 1;
            }
            let name = &source[start + 1..at];
            if let Some(end) = opaque_command_end(context, name, at) {
                at = end;
                continue;
            }
            if definition_command(name) {
                break;
            }
            if !matches!(name, "ref" | "eqref" | "pageref")
                || redefined.contains(name)
                || context.delimiters.math_at_caret(start)
            {
                continue;
            }
            let after_star = at + usize::from(source.as_bytes().get(at) == Some(&b'*'));
            let argument =
                after_star + source[after_star..].len() - source[after_star..].trim_start().len();
            if source.as_bytes().get(argument) != Some(&b'{')
                || context.delimiters.pair_at(argument).is_none()
                || !context.admits(&(start..argument + 1), false)
            {
                continue;
            }
            let prefix = &source[..start];
            let gap = prefix.trim_end_matches(char::is_whitespace).len();
            let whitespace = &source[gap..start];
            // Preserve explicit ties/spaces and leading references without a
            // preceding token. The selected rule joins the complete gap.
            if whitespace.is_empty()
                || gap == 0
                || prefix[..gap].ends_with(['~', '\\', '{', '[', '%'])
            {
                continue;
            }
            // TeX discards whitespace after a control word; inserting ~ there
            // could introduce spacing where no space was typeset before.
            let previous = prefix[..gap].rsplit_once('\\').map(|(_, text)| text);
            if previous.is_some_and(|text| {
                !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphabetic() || b == b'@')
            }) {
                continue;
            }
            if context.delimiters.ignored_at(gap.saturating_sub(1)) {
                continue;
            }
            if let Some(edit) = context.edit(gap..start, "~", Rule::ReferenceTies) {
                candidates.push(vec![edit]);
            }
        }
    }
    if Rule::DisplayMathParagraphs.enabled(settings) {
        let displays = display_interiors(context);
        let mut offset = 0;
        for (line, text) in source.split_inclusive('\n').enumerate() {
            if line % 4096 == 0 {
                cancelled(cancel)?;
            }
            let body = text.trim_end_matches(['\n', '\r']);
            if body.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
                && !context.inside_definition(offset + body.len())
                && displays
                    .get(displays.partition_point(|range| range.end < offset))
                    .is_some_and(|range| range.start <= offset && offset + body.len() <= range.end)
                && let Some(edit) = context.edit(
                    offset + body.len()..offset + body.len(),
                    "%",
                    Rule::DisplayMathParagraphs,
                )
            {
                candidates.push(vec![edit]);
            }
            offset += text.len();
        }
    }
    Ok(candidates)
}

fn display_interiors(context: &Context<'_>) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for block in crate::editor_math_guides::blocks(context.source, DocumentKind::Tex) {
        if context.delimiters.ignored_at(block.range.start)
            || context.inside_definition(block.range.start)
        {
            continue;
        }
        let Some([open, close]) = context.delimiters.pair_at(block.range.start) else {
            continue;
        };
        let header = &context.source[open.clone()];
        let display = header == "\\["
            || header == "$$"
            || header
                .strip_prefix("\\begin")
                .and_then(|text| text.trim().strip_prefix('{'))
                .and_then(|text| text.strip_suffix('}'))
                .is_some_and(|name| {
                    matches!(
                        name.trim().trim_end_matches('*'),
                        "displaymath"
                            | "equation"
                            | "align"
                            | "alignat"
                            | "flalign"
                            | "gather"
                            | "multline"
                            | "eqnarray"
                    )
                });
        if display && open.start == block.range.start && close.end == block.range.end {
            ranges.push(open.end..close.start);
        }
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

/// Skip definitions and arguments whose contents are names or paths, rather
/// than document prose. Literal payloads are already owned by the shared index.
fn opaque_command_end(context: &Context<'_>, name: &str, after: usize) -> Option<usize> {
    let definition = definition_command(name);
    let groups = if matches!(
        name,
        "NewDocumentEnvironment"
            | "RenewDocumentEnvironment"
            | "ProvideDocumentEnvironment"
            | "DeclareDocumentEnvironment"
    ) {
        4
    } else if matches!(
        name,
        "newenvironment"
            | "renewenvironment"
            | "NewDocumentCommand"
            | "RenewDocumentCommand"
            | "ProvideDocumentCommand"
            | "DeclareDocumentCommand"
    ) {
        3
    } else if matches!(
        name,
        "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand"
    ) {
        2
    } else if definition
        || matches!(
            name,
            "url"
                | "path"
                | "nolinkurl"
                | "href"
                | "label"
                | "input"
                | "include"
                | "includegraphics"
                | "bibliography"
                | "addbibresource"
                | "bibliographystyle"
                | "includeonly"
                | "documentclass"
                | "usepackage"
                | "RequirePackage"
                | "cite"
                | "citep"
                | "citet"
                | "parencite"
                | "textcite"
                | "autocite"
                | "nocite"
                | "footcite"
                | "footfullcite"
        )
    {
        1
    } else {
        return None;
    };
    let source = context.source;
    let mut at = after;
    let mut count = 0;
    if source.as_bytes().get(at) == Some(&b'*') {
        at += 1;
    }
    while at < source.len() && count < groups {
        let ch = source[at..].chars().next()?;
        if context.delimiters.ignored_at(at) {
            let ranges = context.delimiters.ignored_ranges();
            let index = ranges
                .partition_point(|range| range.start <= at)
                .checked_sub(1)?;
            at = ranges[index].end;
            continue;
        }
        if ch == '{' || ch == '[' {
            let [open, close] = context.delimiters.pair_at(at)?;
            if open.start != at {
                return None;
            }
            at = close.end;
            if ch == '{' {
                count += 1;
            }
        } else if ch == '\\' && definition {
            let (_, _, end) = following_command(source, at)?;
            at = end;
            if groups > 1 && count == 0 {
                count += 1;
            }
        } else if ch.is_whitespace() || definition && !matches!(ch, '\n' | '\r') {
            at += ch.len_utf8();
        } else {
            return None;
        }
    }
    (count == groups).then_some(at)
}

fn definition_command(name: &str) -> bool {
    matches!(
        name,
        "def"
            | "gdef"
            | "edef"
            | "xdef"
            | "newcommand"
            | "renewcommand"
            | "providecommand"
            | "DeclareRobustCommand"
            | "NewDocumentCommand"
            | "RenewDocumentCommand"
            | "ProvideDocumentCommand"
            | "DeclareDocumentCommand"
            | "newenvironment"
            | "renewenvironment"
            | "NewDocumentEnvironment"
            | "RenewDocumentEnvironment"
            | "ProvideDocumentEnvironment"
            | "DeclareDocumentEnvironment"
    )
}

/// The shared delimiter index intentionally recognizes stored delimiter tokens.
/// Source autofixes need the additional boundary: macro definitions are data,
/// and a malformed definition conservatively protects the rest of the file.
fn definition_ranges(context: &Context<'_>) -> Vec<Range<usize>> {
    let source = context.source;
    let mut ranges = Vec::new();
    let mut at = 0;
    while at < source.len() {
        let ignored = context.delimiters.ignored_ranges();
        let after = ignored.partition_point(|range| range.start <= at);
        if let Some(region) = after.checked_sub(1).and_then(|index| ignored.get(index))
            && region.contains(&at)
        {
            at = region.end;
            continue;
        }
        if source.as_bytes()[at] != b'\\' {
            at += source[at..].chars().next().unwrap().len_utf8();
            continue;
        }
        let start = at;
        at += 1;
        let name_start = at;
        while source
            .as_bytes()
            .get(at)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'@')
        {
            at += 1;
        }
        if at == name_start {
            at += source[at..].chars().next().map_or(0, char::len_utf8);
            continue;
        }
        let name = &source[name_start..at];
        if let Some(end) = opaque_command_end(context, name, at) {
            if definition_command(name) {
                ranges.push(start..end);
            }
            at = end;
        } else if definition_command(name) {
            ranges.push(start..source.len());
            break;
        }
    }
    ranges
}
fn redefined_references<'a>(context: &Context<'a>) -> BTreeSet<&'a str> {
    static DEFINITIONS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\\(?:def|gdef|edef|xdef|let|newcommand|renewcommand|providecommand|DeclareRobustCommand|NewDocumentCommand|RenewDocumentCommand|ProvideDocumentCommand|DeclareDocumentCommand)(?:\s|%[^\r\n]*(?:\r?\n|$))*\*?(?:\s|%[^\r\n]*(?:\r?\n|$))*(?:\{(?:\s|%[^\r\n]*(?:\r?\n|$))*)?\\(ref|eqref|pageref)\b").unwrap()
    });
    DEFINITIONS
        .captures_iter(context.source)
        .filter_map(|captures| {
            let whole = captures.get(0)?;
            (!context.delimiters.ignored_at(whole.start()))
                .then(|| captures.get(1).unwrap().as_str())
        })
        .collect()
}

fn overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    if left.is_empty() || right.is_empty() {
        left.start <= right.end && right.start <= left.end
    } else {
        left.start < right.end && right.start < left.end
    }
}
fn finish_plan(source: &str, mut candidates: Vec<Vec<Edit>>, warning: Option<String>) -> Plan {
    candidates.sort_by_key(|edits| (edits[0].range.start, edits[0].rule));
    let mut selected: BTreeMap<usize, Edit> = BTreeMap::new();
    let mut fixes = 0;
    for edits in candidates {
        if edits.iter().any(|edit| {
            selected
                .range(..=edit.range.start)
                .next_back()
                .is_some_and(|(_, old)| overlap(&old.range, &edit.range))
                || selected
                    .range(edit.range.start..)
                    .next()
                    .is_some_and(|(_, old)| overlap(&old.range, &edit.range))
        }) {
            continue;
        }
        for edit in edits {
            selected.insert(edit.range.start, edit);
        }
        fixes += 1;
    }
    Plan {
        edits: selected.into_values().collect(),
        fixes,
        warning,
        source: source.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source_plan(source: &str, rules: &[Rule], markers: &comment_controls::Settings) -> Plan {
        let settings = Settings {
            enabled: rules.iter().copied().collect(),
        };
        compute(
            source,
            &settings,
            markers,
            Path::new("."),
            None,
            &AtomicBool::new(false),
        )
        .unwrap()
    }
    #[test]
    fn references_join_all_whitespace_with_one_tie_and_remain_idempotent() {
        let source = "α 😀 Figure \t\n\n  \\ref{fig:a}, equation  \t\\eqref{eq:a}, page \u{a0}\\pageref*{p}.";
        let expected = "α 😀 Figure~\\ref{fig:a}, equation~\\eqref{eq:a}, page~\\pageref*{p}.";
        let plan = source_plan(source, &[Rule::ReferenceTies], &Default::default());
        assert_eq!(plan.fixes, 3);
        assert_eq!(plan.apply(source).unwrap(), expected);
        assert_eq!(
            source_plan(expected, &[Rule::ReferenceTies], &Default::default()).fixes,
            0
        );
    }
    #[test]
    fn references_preserve_comments_literals_math_paths_and_definitions() {
        for source in [
            "% before \\ref{x}",
            "\\verb|word \\ref{x}|",
            "\\begin{verbatim}\nword \\ref{x}\n\\end{verbatim}",
            "$word \\ref{x}$",
            "\\(word \\ref{x}\\)",
            "word~ \\ref{x}",
            "word\\ \\ref{x}",
            "\\newcommand{\\x}{word \\ref{x}}",
            "\\def\\x#1{word \\ref{x}}",
            "\\NewDocumentCommand{\\x}{m}{word \\ref{x}}",
            "\\href{word \\ref{x}}{caption}",
            "\\cite{key \\ref{x}}",
            "\\includegraphics[width=1cm]{path \\ref{x}}",
            "word \\reference{x}",
            "word \\ref",
            "\\LaTeX \\ref{x}",
            "word % comment\n \\ref{x}",
        ] {
            assert_eq!(
                source_plan(source, &[Rule::ReferenceTies], &Default::default()).fixes,
                0,
                "{source}"
            );
        }
    }
    #[test]
    fn redefined_references_and_incomplete_definitions_are_not_rewritten() {
        for source in [
            "word \\ref{x}\n\\renewcommand * { \\ref }[1]{custom}",
            "\\let\\eqref\\myref\nword \\eqref{x}",
            "\\newcommand{\\broken}{word \\ref{x}",
        ] {
            assert_eq!(
                source_plan(source, &[Rule::ReferenceTies], &Default::default()).fixes,
                0,
                "{source}"
            );
        }
    }
    #[test]
    fn custom_diagnostic_controls_protect_both_sides_of_a_multiline_edit() {
        let mut markers = comment_controls::Settings::default();
        markers.diagnostics.off = "quiet".into();
        markers.diagnostics.on = "loud".into();
        let source = "word \\ref{a}\n% q u i e t\nword \\ref{b}\n%loud\nword \\ref{c}\n%ttt:off\nword \\ref{d}\n%ttt:on";
        let plan = source_plan(source, &[Rule::ReferenceTies], &markers);
        assert_eq!(plan.fixes, 2);
        assert_eq!(
            plan.apply(source).unwrap(),
            source
                .replacen("word ", "word~", 1)
                .replace("word \\ref{c}", "word~\\ref{c}")
        );
    }
    #[test]
    fn blank_display_paragraphs_become_comments_without_changing_line_numbers() {
        let source = "\\[\r\nx+y\r\n  \r\n\\]\r\n\\begin{align}\nx&=y\n\n\\end{align}\n";
        let plan = source_plan(source, &[Rule::DisplayMathParagraphs], &Default::default());
        assert_eq!(plan.fixes, 2);
        let fixed = plan.apply(source).unwrap();
        assert_eq!(
            fixed,
            source
                .replace("  \r\n", "  %\r\n")
                .replace("y\n\n", "y\n%\n")
        );
        assert_eq!(fixed.lines().count(), source.lines().count());
        assert_eq!(
            source_plan(&fixed, &[Rule::DisplayMathParagraphs], &Default::default()).fixes,
            0
        );
        for source in [
            "\\(x\n\ny\\)",
            "\\begin{math}x\n\ny\\end{math}",
            "\\[x\n\n",
            "\\begin{verbatim}\n\\[x\n\n\\]\n\\end{verbatim}",
        ] {
            assert_eq!(
                source_plan(source, &[Rule::DisplayMathParagraphs], &Default::default()).fixes,
                0
            );
        }
    }
    #[test]
    fn nested_display_groups_and_dollar_displays_have_their_paragraphs_fixed() {
        for source in ["$$x\n\ny$$", "\\[\\frac{x\n\ny}{z}\\]"] {
            let plan = source_plan(source, &[Rule::DisplayMathParagraphs], &Default::default());
            assert_eq!(plan.fixes, 1, "{source}");
            assert_eq!(plan.apply(source).unwrap(), source.replace("\n\n", "\n%\n"));
        }
    }
    #[test]
    fn display_paragraphs_never_rewrite_stored_macro_definitions_or_parameter_delimiters() {
        for definition in [
            "\\newcommand{\\example}{\\[x\n\ny\\]}",
            "\\def\\example#1{\n$$x\n\ny$$\n}",
            "\\NewDocumentCommand{\\example}{m}{\\[x\n\ny\\]}",
            "\\NewDocumentEnvironment{example}{m}{\\[x\n\ny\\]}{end}",
        ] {
            let source = format!("{definition}\n\\[a\n\nb\\]");
            let plan = source_plan(&source, &[Rule::DisplayMathParagraphs], &Default::default());
            assert_eq!(plan.fixes, 1, "{source}");
            assert_eq!(
                plan.apply(&source).unwrap(),
                format!("{definition}\n\\[a\n%\nb\\]")
            );
        }
        for source in [
            "\\def\\take#1\\[\n\n\\]#2{#1}",
            "\\newcommand{\\unfinished}{\\[x\n\ny\\]",
        ] {
            assert_eq!(
                source_plan(source, &[Rule::DisplayMathParagraphs], &Default::default()).fixes,
                0,
                "{source}"
            );
        }
    }
    #[test]
    fn plans_reject_any_changed_source_without_partial_application() {
        let plan = source_plan("word \\ref{x}", &[Rule::ReferenceTies], &Default::default());
        assert!(plan.apply("word \\ref{y}").is_err());
        assert!(plan.apply("word \\ref{x} extra").is_err());
    }
    #[test]
    fn diagnostic_actions_require_exact_codes_and_unambiguous_rule_families() {
        assert_eq!(
            Rule::for_diagnostic(
                "missing-nonbreaking-space",
                "missing non-breaking space before `\\eqref`; use a tie"
            ),
            Some(Rule::ReferenceTies)
        );
        assert_eq!(
            Rule::for_diagnostic("missing-nonbreaking-space", "before `\\cite`"),
            Some(Rule::CitationTies)
        );
        assert_eq!(
            Rule::for_diagnostic("missing-nonbreaking-space", "before `\\reference`"),
            None
        );
        assert_eq!(
            Rule::for_diagnostic("missing-nonbreaking-space", "unrecognized command"),
            None
        );
        assert_eq!(
            Rule::for_diagnostic(
                "\"math-paragraph-break\"",
                "Blank line inside display math."
            ),
            Some(Rule::DisplayMathParagraphs)
        );
        assert_eq!(
            Rule::for_diagnostic("ellipsis", "literal dots; use `\\ldots`"),
            Some(Rule::MathEllipses)
        );
        assert_eq!(
            Rule::for_diagnostic("ellipsis", "literal dots; use `\\dots`"),
            Some(Rule::TextEllipses)
        );
        assert_eq!(
            Rule::for_diagnostic("primitive-command", "`\\over` is primitive"),
            None
        );
        assert_eq!(
            Rule::for_diagnostic("primitive-command", "`\\sp` is primitive; use ^"),
            Some(Rule::PrimitiveScripts)
        );
        assert_eq!(
            Rule::for_diagnostic("undefined-ref", "perhaps replace missing target"),
            None
        );
        assert_eq!(
            Rule::for_diagnostic("unknown", "missing non-breaking space before `\\ref`"),
            None
        );
    }
    #[test]
    fn staged_provider_edits_map_through_shrunk_gaps_and_inserted_comments() {
        let source = "See \n  \\ref{x}. $$a\n\nb$$";
        let context = Context::new(source, &Default::default());
        let settings = Settings {
            enabled: [Rule::ReferenceTies, Rule::DisplayMathParagraphs].into(),
        };
        let initial = finish_plan(
            source,
            source_candidates(&context, &settings, &AtomicBool::new(false)).unwrap(),
            None,
        );
        let staged = initial.apply(source).unwrap();
        assert_eq!(staged, "See~\\ref{x}. $$a\n%\nb$$");
        let mapping = SourceMap::new(&initial.edits);
        let open = staged.find("$$").unwrap();
        let close = staged.rfind("$$").unwrap();
        let edits = vec![
            Edit {
                range: open..open + 2,
                original: "$$".into(),
                replacement: "\\[".into(),
                rule: Rule::DollarDisplayMath,
            },
            Edit {
                range: close..close + 2,
                original: "$$".into(),
                replacement: "\\]".into(),
                rule: Rule::DollarDisplayMath,
            },
        ];
        let mapped = mapping.rebase(&context, edits).unwrap();
        let mut candidates: Vec<_> = initial.edits.into_iter().map(|edit| vec![edit]).collect();
        candidates.push(mapped);
        let final_plan = finish_plan(source, candidates, None);
        assert_eq!(final_plan.fixes, 3);
        assert_eq!(
            final_plan.apply(source).unwrap(),
            "See~\\ref{x}. \\[a\n%\nb\\]"
        );
        assert!(
            mapping
                .rebase(
                    &context,
                    vec![Edit {
                        range: 0..4,
                        original: staged[..4].into(),
                        replacement: "wrong".into(),
                        rule: Rule::TextEllipses
                    }]
                )
                .is_none()
        );
        let marker = staged.find('%').unwrap();
        assert!(
            mapping
                .rebase(
                    &context,
                    vec![Edit {
                        range: marker..marker + 1,
                        original: "%".into(),
                        replacement: "wrong".into(),
                        rule: Rule::TextEllipses
                    }]
                )
                .is_none()
        );
    }
    #[test]
    fn unsafe_typography_rules_are_opt_in_and_unknown_settings_fail() {
        let settings = Settings::default();
        assert!(Rule::ReferenceTies.enabled(&settings));
        for rule in [
            Rule::MathEllipses,
            Rule::StraightQuotes,
            Rule::LabelPositions,
            Rule::ObsoleteEnvironments,
        ] {
            assert!(!rule.enabled(&settings));
        }
        assert!(serde_json::from_str::<Settings>(r#"{"enabled":["made-up-rule"]}"#).is_err());
        assert!(serde_json::from_str::<Settings>(r#"{"surprise":true}"#).is_err());
        assert_eq!(serde_json::from_str::<Settings>("{}").unwrap(), settings);
    }
    #[test]
    fn malformed_disabled_overlapping_or_partial_provider_fixes_are_rejected_atomically() {
        let source = "α ... $$x$$";
        let context = Context::new(source, &Default::default());
        let settings = Settings {
            enabled: [Rule::TextEllipses, Rule::DollarDisplayMath].into(),
        };
        let findings: Vec<Finding> = serde_json::from_value(serde_json::json!([
            {"rule":"ellipsis","start":3,"end":6,"fix":{"applicability":"safe","edits":[{"start":3,"end":6,"content":"\\dots"}]}},
            {"rule":"dollar-display-math","start":7,"end":9,"fix":{"applicability":"safe","edits":[{"start":7,"end":9,"content":"\\["},{"start":10,"end":900,"content":"\\]"}]}},
            {"rule":"ellipsis","start":0,"end":1,"fix":{"applicability":"safe","edits":[{"start":0,"end":1,"content":"invalid UTF8"}]}},
            {"rule":"straight-quotes","start":3,"end":6,"fix":{"applicability":"unsafe","edits":[{"start":3,"end":6,"content":"wrong"}]}},
            {"rule":"ellipsis","start":3,"end":6,"fix":{"applicability":"safe","edits":[{"start":3,"end":6,"content":"duplicate"}]}}
        ])).unwrap();
        let plan = finish_plan(
            source,
            provider_candidates(&context, &settings, findings),
            None,
        );
        assert_eq!(plan.fixes, 1);
        assert_eq!(plan.apply(source).unwrap(), "α \\dots $$x$$");
    }
    #[test]
    fn safe_default_rules_reject_provider_unsafe_payloads() {
        let source = "{\\bf text}";
        let context = Context::new(source, &Default::default());
        let findings = serde_json::from_value(serde_json::json!([
            {"rule":"deprecated-command","start":1,"end":4,"fix":{"applicability":"unsafe","edits":[{"start":1,"end":4,"content":"\\bfseries"}]}}
        ])).unwrap();
        assert!(provider_candidates(&context, &Default::default(), findings).is_empty());
    }
    #[test]
    fn broken_provider_keeps_source_fixes_with_a_visible_warning() {
        let root = tempfile::tempdir().unwrap();
        let tool = ToolResolution {
            bundled_program: None,
            command: Default::default(),
            kind: crate::toolchain::ToolKind::Badness,
            program: root.path().join("missing-program"),
            origin: crate::toolchain::ToolOrigin::Custom,
            fallback_reason: None,
        };
        let source = "word \\ref{x}";
        let plan = compute(
            source,
            &Default::default(),
            &Default::default(),
            root.path(),
            Some(&tool),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(plan.fixes, 1);
        assert_eq!(plan.apply(source).unwrap(), "word~\\ref{x}");
        assert!(plan.warning.unwrap().contains("Could not check"));
        assert_eq!(
            fs::read_dir(root.path().join(".tiptoptyp"))
                .unwrap()
                .count(),
            0
        );
    }
    #[test]
    fn absent_provider_retains_source_fixes_and_explains_skipped_rules() {
        let plan = compute(
            "word \\ref{x}",
            &Default::default(),
            &Default::default(),
            Path::new("."),
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(plan.fixes, 1);
        assert!(plan.warning.unwrap().contains("Badness"));
    }
    #[test]
    fn cancellation_has_no_effect_or_private_scratch_creation() {
        assert!(
            compute(
                "word \\ref{x}",
                &Default::default(),
                &Default::default(),
                Path::new("/does-not-exist"),
                None,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
    #[test]
    #[ignore = "requires TIPTOPTYP_AUTOFIX_BADNESS naming the pinned native Badness tool"]
    fn actual_badness_applies_compound_source_and_parser_fixes_in_one_action() {
        let root = tempfile::tempdir().unwrap();
        let program = std::env::var_os("TIPTOPTYP_AUTOFIX_BADNESS")
            .map(std::path::PathBuf::from)
            .expect("set TIPTOPTYP_AUTOFIX_BADNESS to the pinned native Badness executable");
        assert!(program.is_absolute() && program.is_file());
        let tool = ToolResolution {
            bundled_program: None,
            command: Default::default(),
            kind: crate::toolchain::ToolKind::Badness,
            program,
            origin: crate::toolchain::ToolOrigin::Custom,
            fallback_reason: None,
        };
        let source = "See \n\n \\ref{key} and $$a\n\nb$$. A ... B. {\\it old}";
        let plan = compute(
            source,
            &Default::default(),
            &Default::default(),
            root.path(),
            Some(&tool),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(plan.warning.is_none(), "{:?}", plan.warning);
        assert_eq!(plan.fixes, 5);
        let fixed = plan.apply(source).unwrap();
        assert_eq!(
            fixed,
            "See~\\ref{key} and \\[a\n%\nb\\]. A \\dots B. {\\itshape old}"
        );
        let repeated = compute(
            &fixed,
            &Default::default(),
            &Default::default(),
            root.path(),
            Some(&tool),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(repeated.warning.is_none());
        assert_eq!(repeated.fixes, 0);
        assert_eq!(
            fs::read_dir(root.path().join(".tiptoptyp"))
                .unwrap()
                .count(),
            0
        );
    }
    #[cfg(unix)]
    #[test]
    fn provider_reads_snapshot_only_and_cleans_scratch_after_findings_exit() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("badness-stub");
        fs::write(&program, "#!/bin/sh\nfor arg in \"$@\"; do\n case \"$arg\" in --fix|--unsafe-fixes) exit 9;; esac\ndone\n/bin/cat >/dev/null\nprintf '%s' '[{\"rule\":\"ellipsis\",\"start\":5,\"end\":8,\"fix\":{\"applicability\":\"safe\",\"edits\":[{\"start\":5,\"end\":8,\"content\":\"\\\\dots\"}]}}]'\nexit 1\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let tool = ToolResolution {
            bundled_program: None,
            command: Default::default(),
            kind: crate::toolchain::ToolKind::Badness,
            program,
            origin: crate::toolchain::ToolOrigin::Custom,
            fallback_reason: None,
        };
        let source = "text ...";
        let user_file = root.path().join("main.tex");
        fs::write(&user_file, source).unwrap();
        let settings = Settings {
            enabled: [Rule::TextEllipses].into(),
        };
        let plan = compute(
            source,
            &settings,
            &Default::default(),
            root.path(),
            Some(&tool),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(plan.fixes, 1);
        assert_eq!(plan.apply(source).unwrap(), "text \\dots");
        assert_eq!(fs::read_to_string(user_file).unwrap(), source);
        assert_eq!(
            fs::read_dir(root.path().join(".tiptoptyp"))
                .unwrap()
                .count(),
            0
        );
    }
}
