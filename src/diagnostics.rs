/// Refreshes retain published diagnostics for this document, even when revisions differ.
/// Adapters replace them only on a successful response (including an empty one).
/// Disabling a provider or changing documents explicitly retires its results.
pub(crate) fn same_document(
    previous: crate::document::DocumentKey,
    current: crate::document::DocumentKey,
) -> bool {
    previous.owner == current.owner && previous.epoch == current.epoch
}

use std::{collections::HashMap, ops::Range, path::PathBuf};

/// Severity shared by build and language-service adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DiagnosticSeverity {
    Error,
    Warning,
    Help,
    Note,
    /// Unstructured output retained by an adapter instead of silently lost.
    Unknown,
}

impl DiagnosticSeverity {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Help => "help",
            Self::Note => "note",
            Self::Unknown => "diagnostic",
        }
    }
}

/// Which source a diagnostic belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum DiagnosticSource {
    Main,
    File(PathBuf),
    Global,
}

/// One-based line and Unicode-scalar column in canonical source.
/// Adapters convert native byte/UTF-16 locations before publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DiagnosticLocation {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

/// Provider-neutral corrections retain exact scalar spans and source text. A
/// published result is bound to the document revision that produced it; stale
/// diagnostics may remain visible, but can never edit a newer revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagnosticCorrection {
    pub(crate) range: Range<usize>,
    pub(crate) original: String,
    pub(crate) rule: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) replacements: Vec<String>,
    pub(crate) document: Option<crate::document::DocumentKey>,
}

impl DiagnosticCorrection {
    pub(crate) fn new(
        range: Range<usize>,
        original: String,
        rule: Option<String>,
        category: Option<String>,
        replacements: Vec<String>,
    ) -> Self {
        Self {
            range,
            original,
            rule,
            category,
            replacements,
            document: None,
        }
    }

    pub(crate) fn is_current(&self, key: crate::document::DocumentKey) -> bool {
        self.document == Some(key)
    }

    /// Convert only when applying, so drawing a Problems menu never scans a
    /// large document. Reject invalid endpoints rather than clamping them.
    pub(crate) fn checked_byte_range(
        &self,
        source: &str,
        key: crate::document::DocumentKey,
    ) -> Result<Range<usize>, &'static str> {
        if !self.is_current(key) {
            return Err("The document changed; wait for refreshed suggestions.");
        }
        if self.range.start > self.range.end {
            return Err("The correction has an invalid source range.");
        }
        let mut start = None;
        let mut end = None;
        for (scalar, byte) in source
            .char_indices()
            .map(|(byte, _)| byte)
            .chain([source.len()])
            .enumerate()
        {
            if scalar == self.range.start {
                start = Some(byte);
            }
            if scalar == self.range.end {
                end = Some(byte);
                break;
            }
        }
        let range = start
            .zip(end)
            .map(|(start, end)| start..end)
            .ok_or("The correction is outside the document.")?;
        if source.get(range.clone()) != Some(self.original.as_str()) {
            return Err("The source no longer matches this suggestion.");
        }
        Ok(range)
    }
}

pub(crate) fn bind_document(diagnostics: &mut [Diagnostic], key: crate::document::DocumentKey) {
    for diagnostic in diagnostics {
        if let Some(correction) = &mut diagnostic.correction {
            correction.document = Some(key);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Diagnostic {
    pub(crate) correction: Option<DiagnosticCorrection>,
    /// Tool that produced this diagnostic, separate from its file location.
    pub(crate) provider: Option<String>,
    pub(crate) severity: DiagnosticSeverity,
    pub(crate) source: DiagnosticSource,
    pub(crate) location: Option<DiagnosticLocation>,
    pub(crate) message: String,
    /// Hints, notes, and otherwise-unstructured continuation lines belonging
    /// to this diagnostic. These are useful for a richer hover tooltip while
    /// `message` remains short enough for inline virtual text.
    pub(crate) details: Vec<String>,
}

impl Diagnostic {
    #[cfg(test)]
    pub(crate) fn is_for_main_file(&self) -> bool {
        self.source == DiagnosticSource::Main
    }

    pub(crate) fn line(&self) -> Option<usize> {
        self.location.map(|location| location.line)
    }

    /// Put CLI and language-server diagnostics into the same compact display
    /// shape: one headline followed by distinct, labelled detail lines.
    ///
    /// Tinymist sometimes embeds the severity, hints and provider in its
    /// multiline `message`, while Typst's short CLI format emits those as
    /// continuation lines. Keeping that difference out of the UI means the
    /// Problems panel and hover card can render `message` and `details`
    /// identically. File/line/column information remains on the diagnostic and
    /// is never folded into its text.
    pub(crate) fn normalize(&mut self) {
        let original_message = std::mem::take(&mut self.message);
        let original_details = std::mem::take(&mut self.details);
        let mut message = None;
        let mut details = Vec::new();

        for line in original_message.lines() {
            let line = clean_continuation(line);
            if line.is_empty() || is_provider_metadata(line) {
                continue;
            }

            if message.is_none() {
                let headline = strip_primary_severity(line, self.severity);
                if headline.is_empty() {
                    continue;
                }
                message = Some(headline.to_owned());
            } else if let Some(detail) = normalize_detail(line, self.severity, message.as_deref()) {
                push_distinct(&mut details, detail);
            }
        }

        for detail in original_details {
            for line in detail.lines() {
                if let Some(detail) = normalize_detail(line, self.severity, message.as_deref()) {
                    push_distinct(&mut details, detail);
                }
            }
        }

        self.message = message.unwrap_or_default();
        self.details = details;
    }

    pub(crate) fn full_message(&self) -> String {
        let capacity = self.message.len()
            + self.details.iter().map(String::len).sum::<usize>()
            + self.details.len();
        let mut message = String::with_capacity(capacity);
        for line in self.display_lines() {
            if !message.is_empty() {
                message.push('\n');
            }
            message.push_str(line);
        }
        message
    }

    /// The exact text lines shared by compact and expanded diagnostic views.
    pub(crate) fn display_lines(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.message.as_str())
            .filter(|line| !line.is_empty())
            .chain(self.details.iter().map(String::as_str))
            .chain(self.provider.as_deref())
    }
}

/// Normalize a batch and combine only exact repeats. Diagnostics at distinct
/// locations deliberately remain distinct so navigation never loses a target.
pub(crate) fn normalize_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    let mut positions: HashMap<DiagnosticIdentity, Vec<usize>> =
        HashMap::with_capacity(diagnostics.len());
    let mut normalized: Vec<Diagnostic> = Vec::with_capacity(diagnostics.len());

    for mut diagnostic in diagnostics.drain(..) {
        diagnostic.normalize();
        if diagnostic.message.is_empty() && diagnostic.details.is_empty() {
            continue;
        }

        let identity = DiagnosticIdentity::from(&diagnostic);
        let duplicates = positions.entry(identity).or_default();
        if let Some(index) = duplicates
            .iter()
            .copied()
            .find(|&index| normalized[index].correction == diagnostic.correction)
        {
            let existing = &mut normalized[index];
            for detail in diagnostic.details {
                push_distinct(&mut existing.details, detail);
            }
        } else {
            duplicates.push(normalized.len());
            normalized.push(diagnostic);
        }
    }

    *diagnostics = normalized;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DiagnosticIdentity {
    provider: Option<String>,
    severity: DiagnosticSeverity,
    source: DiagnosticSource,
    location: Option<DiagnosticLocation>,
    message: String,
}

impl From<&Diagnostic> for DiagnosticIdentity {
    fn from(diagnostic: &Diagnostic) -> Self {
        Self {
            provider: diagnostic.provider.clone(),
            severity: diagnostic.severity,
            source: diagnostic.source.clone(),
            location: diagnostic.location,
            message: diagnostic.message.clone(),
        }
    }
}

pub(crate) fn clean_continuation(line: &str) -> &str {
    let trimmed = line.trim();
    trimmed
        .strip_prefix("= ")
        .or_else(|| trimmed.strip_prefix("- "))
        .unwrap_or(trimmed)
        .trim()
}

fn strip_primary_severity(mut line: &str, severity: DiagnosticSeverity) -> &str {
    loop {
        let trimmed = line.trim();
        if severity_names(severity)
            .iter()
            .any(|name| trimmed.eq_ignore_ascii_case(name))
        {
            return "";
        }

        let Some((label, value)) = trimmed.split_once(':') else {
            return trimmed;
        };
        if !severity_names(severity)
            .iter()
            .any(|name| label.trim().eq_ignore_ascii_case(name))
        {
            return trimmed;
        }
        line = value;
    }
}

fn severity_names(severity: DiagnosticSeverity) -> &'static [&'static str] {
    match severity {
        DiagnosticSeverity::Error => &["error"],
        DiagnosticSeverity::Warning => &["warning"],
        DiagnosticSeverity::Help => &["hint", "help"],
        DiagnosticSeverity::Note => &["note", "info", "information"],
        DiagnosticSeverity::Unknown => &["diagnostic"],
    }
}

fn normalize_detail(
    line: &str,
    severity: DiagnosticSeverity,
    headline: Option<&str>,
) -> Option<String> {
    let line = clean_continuation(line);
    if line.is_empty() || is_provider_metadata(line) {
        return None;
    }

    let line = strip_repeated_primary_detail(line, severity);
    if line.is_empty() || headline.is_some_and(|headline| line.eq_ignore_ascii_case(headline)) {
        return None;
    }

    let Some((label, value)) = line.split_once(':') else {
        return Some(line.to_owned());
    };
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let canonical_label = match label.trim().to_ascii_lowercase().as_str() {
        "hint" | "help" => "Hint",
        "note" | "info" | "information" => "Note",
        "warning" => "Warning",
        "error" => "Error",
        "code" => "Code",
        _ => return Some(line.to_owned()),
    };
    Some(format!("{canonical_label}: {value}"))
}

fn strip_repeated_primary_detail(mut line: &str, severity: DiagnosticSeverity) -> &str {
    loop {
        let Some((label, value)) = line.split_once(':') else {
            return line.trim();
        };
        if !severity_names(severity)
            .iter()
            .any(|name| label.trim().eq_ignore_ascii_case(name))
        {
            return line.trim();
        }
        line = value.trim();
    }
}

fn is_provider_metadata(line: &str) -> bool {
    line.split_once(':')
        .is_some_and(|(label, _)| label.trim().eq_ignore_ascii_case("source"))
}

fn push_distinct(lines: &mut Vec<String>, candidate: String) {
    if !lines
        .iter()
        .any(|line| line.eq_ignore_ascii_case(&candidate))
    {
        lines.push(candidate);
    }
}

/// Build output is decoded by its engine before it reaches the application.
/// Retain raw output for troubleshooting alongside normalized source diagnostics.
#[derive(Debug, Clone, Default)]
pub(crate) struct DiagnosticReport {
    pub(crate) raw: String,
    pub(crate) diagnostics: Vec<Diagnostic>,
}
impl DiagnosticReport {
    pub(crate) fn error(raw: String) -> Self {
        let mut diagnostics = vec![Diagnostic {
            correction: None,
            provider: None,
            severity: DiagnosticSeverity::Error,
            source: DiagnosticSource::Global,
            location: None,
            message: raw.clone(),
            details: Vec::new(),
        }];
        normalize_diagnostics(&mut diagnostics);
        Self { raw, diagnostics }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn correction_scalar_mapping_requires_exact_revision_source_and_endpoints() {
        let key = crate::document::DocumentKey::new(
            tiptoptyp_core::document::WindowSessionId::new(1),
            2,
            3,
        );
        let source = "é 😀 teh\r\nresult";
        let mut correction = DiagnosticCorrection::new(
            4..7,
            "teh".into(),
            Some("SpellCheck".into()),
            Some("Spelling".into()),
            vec!["the".into()],
        );
        assert!(correction.checked_byte_range(source, key).is_err());
        correction.document = Some(key);
        let bytes = correction.checked_byte_range(source, key).unwrap();
        assert_eq!(source.get(bytes), Some("teh"));
        assert!(
            correction
                .checked_byte_range(source, key.after_edit())
                .is_err()
        );
        assert!(
            correction
                .checked_byte_range("é 😀 the\r\nresult", key)
                .is_err()
        );
        correction.range.end = usize::MAX;
        assert!(correction.checked_byte_range(source, key).is_err());
        correction.range = Range { start: 9, end: 4 };
        assert!(correction.checked_byte_range(source, key).is_err());
        correction.range = source.chars().count()..source.chars().count();
        correction.original.clear();
        assert!(correction.checked_byte_range(source, key).is_ok());
    }

    #[test]
    fn normalization_preserves_distinct_correction_ranges_and_rule_ids() {
        let make = |range, rule: &str| Diagnostic {
            correction: Some(DiagnosticCorrection::new(
                range,
                "word".into(),
                Some(rule.into()),
                None,
                vec!["fix".into()],
            )),
            provider: Some("Harper".into()),
            severity: DiagnosticSeverity::Help,
            source: DiagnosticSource::Main,
            location: Some(DiagnosticLocation { line: 1, column: 1 }),
            message: "Same message".into(),
            details: Vec::new(),
        };
        let mut diagnostics = vec![
            make(0..4, "First"),
            make(0..4, "Second"),
            make(0..8, "First"),
            make(0..4, "First"),
            make(0..4, "Second"),
        ];
        normalize_diagnostics(&mut diagnostics);
        assert_eq!(diagnostics.len(), 3);
        assert_eq!(
            diagnostics[1].correction.as_ref().unwrap().rule.as_deref(),
            Some("Second")
        );
        assert_eq!(diagnostics[2].correction.as_ref().unwrap().range, 0..8);
    }

    #[test]
    fn normalizes_multiline_tinymist_messages_to_the_cli_display_shape() {
        let mut diagnostics = vec![Diagnostic {
            correction: None,
            provider: None,
            severity: DiagnosticSeverity::Error,
            source: DiagnosticSource::Main,
            location: Some(DiagnosticLocation {
                line: 9,
                column: 30,
            }),
            message: concat!(
                "error\n",
                "error: the character `#` is not valid in code\n",
                "hint: you are already in code mode\n",
                "Hint: try removing the `#`\n",
                "source: typst",
            )
            .to_owned(),
            details: vec![
                "source: typst".to_owned(),
                "hint: try removing the `#`".to_owned(),
                "code: invalid-code".to_owned(),
            ],
        }];

        normalize_diagnostics(&mut diagnostics);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message,
            "the character `#` is not valid in code"
        );
        assert_eq!(
            diagnostics[0].details,
            [
                "Hint: you are already in code mode",
                "Hint: try removing the `#`",
                "Code: invalid-code",
            ]
        );
        assert_eq!(
            diagnostics[0].full_message(),
            concat!(
                "the character `#` is not valid in code\n",
                "Hint: you are already in code mode\n",
                "Hint: try removing the `#`\n",
                "Code: invalid-code",
            )
        );
    }
    #[test]
    fn metadata_only_diagnostics_disappear_without_reordering_real_diagnostics() {
        let mut diagnostics = vec![
            Diagnostic {
                correction: None,
                provider: None,
                severity: DiagnosticSeverity::Unknown,
                source: DiagnosticSource::Global,
                location: None,
                message: "source: typst".to_owned(),
                details: vec![],
            },
            Diagnostic {
                correction: None,
                provider: None,
                severity: DiagnosticSeverity::Error,
                source: DiagnosticSource::Main,
                location: None,
                message: "error: retained".to_owned(),
                details: vec!["source: tinymist".to_owned()],
            },
        ];

        normalize_diagnostics(&mut diagnostics);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "retained");
        assert!(diagnostics[0].details.is_empty());
    }
}
