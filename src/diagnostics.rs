use std::{collections::HashMap, path::PathBuf};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Diagnostic {
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
    }
}

/// Normalize a batch and combine only exact repeats. Diagnostics at distinct
/// locations deliberately remain distinct so navigation never loses a target.
pub(crate) fn normalize_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    let mut positions: HashMap<DiagnosticIdentity, usize> =
        HashMap::with_capacity(diagnostics.len());
    let mut normalized: Vec<Diagnostic> = Vec::with_capacity(diagnostics.len());

    for mut diagnostic in diagnostics.drain(..) {
        diagnostic.normalize();
        if diagnostic.message.is_empty() && diagnostic.details.is_empty() {
            continue;
        }

        let identity = DiagnosticIdentity::from(&diagnostic);
        if let Some(index) = positions.get(&identity).copied() {
            let existing = &mut normalized[index];
            for detail in diagnostic.details {
                push_distinct(&mut existing.details, detail);
            }
        } else {
            positions.insert(identity, normalized.len());
            normalized.push(diagnostic);
        }
    }

    *diagnostics = normalized;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DiagnosticIdentity {
    severity: DiagnosticSeverity,
    source: DiagnosticSource,
    location: Option<DiagnosticLocation>,
    message: String,
}

impl From<&Diagnostic> for DiagnosticIdentity {
    fn from(diagnostic: &Diagnostic) -> Self {
        Self {
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
    fn normalizes_multiline_tinymist_messages_to_the_cli_display_shape() {
        let mut diagnostics = vec![Diagnostic {
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
                severity: DiagnosticSeverity::Unknown,
                source: DiagnosticSource::Global,
                location: None,
                message: "source: typst".to_owned(),
                details: vec![],
            },
            Diagnostic {
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
