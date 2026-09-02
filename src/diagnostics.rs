use std::path::{Path, PathBuf};

/// Severity emitted by Typst's short diagnostic format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DiagnosticSeverity {
    Error,
    Warning,
    Help,
    Note,
    /// Text that did not match the documented short format. It is retained so
    /// an unexpected Typst message is visible instead of being silently lost.
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

/// A one-based location, matching Typst's CLI output and editor line numbers.
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
    pub(crate) fn is_for_main_file(&self) -> bool {
        self.source == DiagnosticSource::Main
    }

    pub(crate) fn line(&self) -> Option<usize> {
        self.location.map(|location| location.line)
    }

    pub(crate) fn full_message(&self) -> String {
        if self.details.is_empty() {
            return self.message.clone();
        }

        let mut full = self.message.clone();
        for detail in &self.details {
            full.push('\n');
            full.push_str(detail);
        }
        full
    }
}

/// Parse output produced by `typst ... --diagnostic-format short`.
///
/// Short output is deliberately human-readable rather than a stable machine
/// protocol. This parser therefore keeps malformed and continuation lines,
/// parses locations from the right (so Windows drive letters and colons in
/// filenames work), and accepts the timestamp prefix added by `typst watch`.
/// `main_path` may be either the full path or just the displayed filename.
pub(crate) fn parse_typst_short_output(output: &str, main_path: Option<&Path>) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    for raw_line in output.lines() {
        let uncolored = strip_ansi_csi(raw_line);
        let line = strip_watch_timestamp(uncolored.trim());
        if line.is_empty() {
            continue;
        }

        if let Some(parsed) = parse_structured_line(line, main_path) {
            if matches!(
                parsed.diagnostic.severity,
                DiagnosticSeverity::Help | DiagnosticSeverity::Note
            ) && diagnostics
                .last_mut()
                .is_some_and(|previous| parsed.belongs_to(previous))
            {
                let previous = diagnostics.last_mut().unwrap();
                previous.details.push(format!(
                    "{}: {}",
                    parsed.original_severity, parsed.diagnostic.message
                ));
            } else {
                diagnostics.push(parsed.diagnostic);
            }
            continue;
        }

        let detail = clean_continuation(line);
        if detail.is_empty() {
            continue;
        }

        if let Some(previous) = diagnostics.last_mut() {
            previous.details.push(detail.to_owned());
        } else {
            diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Unknown,
                source: DiagnosticSource::Global,
                location: None,
                message: detail.to_owned(),
                details: Vec::new(),
            });
        }
    }

    diagnostics
}

struct ParsedLine<'a> {
    diagnostic: Diagnostic,
    original_severity: &'a str,
}

impl ParsedLine<'_> {
    fn belongs_to(&self, previous: &Diagnostic) -> bool {
        match (&self.diagnostic.source, &previous.source) {
            (DiagnosticSource::Global, _) => true,
            (left, right) if left == right => {
                self.diagnostic.location.is_none()
                    || previous.location.is_none()
                    || self.diagnostic.location == previous.location
            }
            _ => false,
        }
    }
}

fn parse_structured_line<'a>(line: &'a str, main_path: Option<&Path>) -> Option<ParsedLine<'a>> {
    const SEVERITIES: [(&str, DiagnosticSeverity); 5] = [
        ("error", DiagnosticSeverity::Error),
        ("warning", DiagnosticSeverity::Warning),
        ("hint", DiagnosticSeverity::Help),
        ("help", DiagnosticSeverity::Help),
        ("note", DiagnosticSeverity::Note),
    ];

    // First handle unlocated messages such as `error: failed to load package`.
    for (name, severity) in SEVERITIES {
        let prefix = format!("{name}:");
        if let Some(message) = line.strip_prefix(&prefix) {
            return Some(ParsedLine {
                diagnostic: Diagnostic {
                    severity,
                    source: DiagnosticSource::Global,
                    location: None,
                    message: message.trim_start().to_owned(),
                    details: Vec::new(),
                },
                original_severity: name,
            });
        }
    }

    // Find a severity marker whose prefix is a valid path:line:column. Merely
    // splitting at the first colon would break both Windows paths and messages
    // containing colons.
    for (name, severity) in SEVERITIES {
        let marker = format!(": {name}:");
        for (marker_start, _) in line.match_indices(&marker) {
            let location_prefix = &line[..marker_start];
            let Some((path, location)) = parse_location_from_right(location_prefix) else {
                continue;
            };
            let message = line[marker_start + marker.len()..].trim_start();
            return Some(ParsedLine {
                diagnostic: Diagnostic {
                    severity,
                    source: classify_source(path, main_path),
                    location: Some(location),
                    message: message.to_owned(),
                    details: Vec::new(),
                },
                original_severity: name,
            });
        }
    }

    None
}

fn parse_location_from_right(prefix: &str) -> Option<(&str, DiagnosticLocation)> {
    let mut components = prefix.rsplitn(3, ':');
    let column = components.next()?.trim().parse::<usize>().ok()?;
    let line = components.next()?.trim().parse::<usize>().ok()?;
    let path = components.next()?.trim();

    if path.is_empty() || line == 0 || column == 0 {
        return None;
    }

    Some((path, DiagnosticLocation { line, column }))
}

fn classify_source(path: &str, main_path: Option<&Path>) -> DiagnosticSource {
    if path == "<stdin>" || main_path.is_some_and(|main| path_matches_main(path, main)) {
        DiagnosticSource::Main
    } else {
        DiagnosticSource::File(PathBuf::from(path))
    }
}

fn path_matches_main(path: &str, main: &Path) -> bool {
    if path == main.to_string_lossy() {
        return true;
    }

    // The watch worker replaces its private shadow path with the editor's
    // display name. Permit that basename only when the diagnostic itself does
    // not contain a directory, avoiding accidental matches for included files.
    let diagnostic_path = Path::new(path);
    diagnostic_path
        .parent()
        .is_none_or(|parent| parent.as_os_str().is_empty())
        && diagnostic_path.file_name().is_some()
        && diagnostic_path.file_name() == main.file_name()
}

fn clean_continuation(line: &str) -> &str {
    let trimmed = line.trim();
    trimmed
        .strip_prefix("= ")
        .or_else(|| trimmed.strip_prefix("- "))
        .unwrap_or(trimmed)
        .trim()
}

fn strip_watch_timestamp(line: &str) -> &str {
    if line.starts_with('[')
        && let Some(closing) = line.find("] ")
    {
        return &line[closing + 2..];
    }
    line
}

/// Remove ANSI Control Sequence Introducer escapes without depending on the
/// terminal's colour configuration. Non-CSI escapes are preserved verbatim.
fn strip_ansi_csi(input: &str) -> String {
    if !input.as_bytes().contains(&0x1b) {
        return input.to_owned();
    }

    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut copied_until = 0;
    let mut index = 0;

    while index + 1 < bytes.len() {
        if bytes[index] != 0x1b || bytes[index + 1] != b'[' {
            index += 1;
            continue;
        }

        output.push_str(&input[copied_until..index]);
        index += 2;
        while index < bytes.len() {
            let byte = bytes[index];
            index += 1;
            if (0x40..=0x7e).contains(&byte) {
                break;
            }
        }
        copied_until = index;
    }

    output.push_str(&input[copied_until..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unix_main_location_and_colons_in_message() {
        let diagnostics = parse_typst_short_output(
            "/work/paper.typ:12:7: error: expected integer: found string",
            Some(Path::new("/work/paper.typ")),
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostics[0].source, DiagnosticSource::Main);
        assert_eq!(
            diagnostics[0].location,
            Some(DiagnosticLocation {
                line: 12,
                column: 7
            })
        );
        assert_eq!(diagnostics[0].message, "expected integer: found string");
    }

    #[test]
    fn parses_windows_drive_path_from_the_right() {
        let path = r"C:\Users\Calvin\paper.typ";
        let output = format!(r"{path}:23:4: warning: unknown font: Example Sans");
        let diagnostics = parse_typst_short_output(&output, Some(Path::new(path)));

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].source, DiagnosticSource::Main);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Warning);
        assert_eq!(diagnostics[0].line(), Some(23));
        assert_eq!(diagnostics[0].location.unwrap().column, 4);
    }

    #[test]
    fn accepts_colons_in_paths_and_keeps_other_files_distinct() {
        let diagnostics = parse_typst_short_output(
            "/tmp/chapter:one.typ:3:9: error: broken",
            Some(Path::new("/tmp/main.typ")),
        );

        assert_eq!(
            diagnostics[0].source,
            DiagnosticSource::File(PathBuf::from("/tmp/chapter:one.typ"))
        );
        assert_eq!(diagnostics[0].line(), Some(3));
    }

    #[test]
    fn displayed_basename_can_identify_the_main_shadow_document() {
        let diagnostics = parse_typst_short_output(
            "paper.typ:1:2: error: bad",
            Some(Path::new("/projects/report/paper.typ")),
        );

        assert!(diagnostics[0].is_for_main_file());
    }

    #[test]
    fn groups_hints_notes_and_continuations_with_the_primary_diagnostic() {
        let output = concat!(
            "paper.typ:4:2: error: unknown variable\n",
            "paper.typ:4:2: hint: did you mean `value`?\n",
            "  = check the spelling\n",
            "paper.typ:4:2: note: names are case-sensitive",
        );
        let diagnostics = parse_typst_short_output(output, Some(Path::new("paper.typ")));

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].details,
            [
                "hint: did you mean `value`?",
                "check the spelling",
                "note: names are case-sensitive",
            ]
        );
        assert!(diagnostics[0].full_message().contains("check the spelling"));
    }

    #[test]
    fn a_hint_for_another_file_is_not_attached_to_the_previous_error() {
        let output = concat!(
            "main.typ:1:1: error: first\n",
            "other.typ:2:2: hint: independent",
        );
        let diagnostics = parse_typst_short_output(output, Some(Path::new("main.typ")));

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[1].severity, DiagnosticSeverity::Help);
        assert_eq!(
            diagnostics[1].source,
            DiagnosticSource::File(PathBuf::from("other.typ"))
        );
    }

    #[test]
    fn parses_global_messages_and_retains_malformed_lines() {
        let output = concat!(
            "error: package download failed\n",
            "network connection was reset\n",
            "warning: cached package may be stale",
        );
        let diagnostics = parse_typst_short_output(output, None);

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].source, DiagnosticSource::Global);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostics[0].details, ["network connection was reset"]);
        assert_eq!(diagnostics[1].severity, DiagnosticSeverity::Warning);

        let malformed = parse_typst_short_output("not a known diagnostic form", None);
        assert_eq!(malformed[0].severity, DiagnosticSeverity::Unknown);
        assert_eq!(malformed[0].message, "not a known diagnostic form");
    }

    #[test]
    fn supports_watch_timestamps_and_ansi_colours() {
        let diagnostics = parse_typst_short_output(
            "[12:34:56] \u{1b}[31mpaper.typ:8:5: error:\u{1b}[0m invalid syntax",
            Some(Path::new("paper.typ")),
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostics[0].line(), Some(8));
        assert_eq!(diagnostics[0].message, "invalid syntax");
    }

    #[test]
    fn malformed_locations_do_not_panic_or_disappear() {
        let diagnostics = parse_typst_short_output(
            "paper.typ:not-a-line:2: error: malformed\npaper.typ:0:0: error: zero",
            Some(Path::new("paper.typ")),
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Unknown);
        assert_eq!(
            diagnostics[0].message,
            "paper.typ:not-a-line:2: error: malformed"
        );
        assert_eq!(diagnostics[0].details, ["paper.typ:0:0: error: zero"]);
    }
}
