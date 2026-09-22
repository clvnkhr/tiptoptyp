//! Decoder for Typst's short diagnostic format, including watcher prefixes.
use crate::diagnostics::{
    Diagnostic, DiagnosticLocation, DiagnosticReport, DiagnosticSeverity, DiagnosticSource,
    clean_continuation, normalize_diagnostics,
};
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

pub(super) fn report(raw: String, main_path: Option<&Path>) -> DiagnosticReport {
    let diagnostics = parse_typst_short_output(&raw, main_path);
    DiagnosticReport { raw, diagnostics }
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

    normalize_diagnostics(&mut diagnostics);
    diagnostics
}

struct ParsedLine {
    diagnostic: Diagnostic,
    original_severity: &'static str,
}

impl ParsedLine {
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

#[derive(Clone, Copy)]
struct SeverityPattern {
    name: &'static str,
    prefix: &'static str,
    marker: &'static str,
    severity: DiagnosticSeverity,
}

const SEVERITY_PATTERNS: [SeverityPattern; 5] = [
    SeverityPattern {
        name: "error",
        prefix: "error:",
        marker: ": error:",
        severity: DiagnosticSeverity::Error,
    },
    SeverityPattern {
        name: "warning",
        prefix: "warning:",
        marker: ": warning:",
        severity: DiagnosticSeverity::Warning,
    },
    SeverityPattern {
        name: "hint",
        prefix: "hint:",
        marker: ": hint:",
        severity: DiagnosticSeverity::Help,
    },
    SeverityPattern {
        name: "help",
        prefix: "help:",
        marker: ": help:",
        severity: DiagnosticSeverity::Help,
    },
    SeverityPattern {
        name: "note",
        prefix: "note:",
        marker: ": note:",
        severity: DiagnosticSeverity::Note,
    },
];

fn parse_structured_line(line: &str, main_path: Option<&Path>) -> Option<ParsedLine> {
    // First handle unlocated messages such as `error: failed to load package`.
    for pattern in SEVERITY_PATTERNS {
        if let Some(message) = line.strip_prefix(pattern.prefix) {
            return Some(ParsedLine {
                diagnostic: Diagnostic {
                    severity: pattern.severity,
                    source: DiagnosticSource::Global,
                    location: None,
                    message: message.trim_start().to_owned(),
                    details: Vec::new(),
                },
                original_severity: pattern.name,
            });
        }
    }

    // Find a severity marker whose prefix is a valid path:line:column. Merely
    // splitting at the first colon would break both Windows paths and messages
    // containing colons.
    for pattern in SEVERITY_PATTERNS {
        for (marker_start, _) in line.match_indices(pattern.marker) {
            let location_prefix = &line[..marker_start];
            let Some((path, location)) = parse_location_from_right(location_prefix) else {
                continue;
            };
            let message = line[marker_start + pattern.marker.len()..].trim_start();
            return Some(ParsedLine {
                diagnostic: Diagnostic {
                    severity: pattern.severity,
                    source: classify_source(path, main_path),
                    location: Some(location),
                    message: message.to_owned(),
                    details: Vec::new(),
                },
                original_severity: pattern.name,
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
fn strip_ansi_csi(input: &str) -> Cow<'_, str> {
    let bytes = input.as_bytes();
    let mut output = None::<String>;
    let mut copied_until = 0;
    let mut search_from = 0;

    while let Some(relative_start) = bytes[search_from..]
        .windows(2)
        .position(|pair| pair == [0x1b, b'['])
    {
        let start = search_from + relative_start;
        let parameters_start = start + 2;
        let Some(relative_end) = bytes[parameters_start..]
            .iter()
            .position(|byte| (0x40..=0x7e).contains(byte))
        else {
            break;
        };
        let end = parameters_start + relative_end + 1;
        output
            .get_or_insert_with(|| String::with_capacity(input.len()))
            .push_str(&input[copied_until..start]);
        copied_until = end;
        search_from = end;
    }

    match output {
        Some(mut output) => {
            output.push_str(&input[copied_until..]);
            Cow::Owned(output)
        }
        None => Cow::Borrowed(input),
    }
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
                "Hint: did you mean `value`?",
                "check the spelling",
                "Note: names are case-sensitive",
            ]
        );
        assert!(diagnostics[0].full_message().contains("check the spelling"));
    }
    #[test]
    fn keeps_duplicate_hash_errors_at_their_locations_but_trims_redundant_details() {
        let output = concat!(
            "paper.typ:9:29: error: the character `#` is not valid in code\n",
            "paper.typ:9:30: error: the character `#` is not valid in code\n",
            "Hint: you are already in code mode\n",
            "Hint: try removing the `#`\n",
            "source: typst",
        );
        let diagnostics = parse_typst_short_output(output, Some(Path::new("paper.typ")));

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].location.unwrap().column, 29);
        assert_eq!(diagnostics[1].location.unwrap().column, 30);
        assert_eq!(
            diagnostics[0].message,
            "the character `#` is not valid in code"
        );
        assert!(diagnostics[0].details.is_empty());
        assert_eq!(
            diagnostics[1].details,
            [
                "Hint: you are already in code mode",
                "Hint: try removing the `#`",
            ]
        );
        assert!(!diagnostics[1].full_message().contains("source:"));
    }
    #[test]
    fn merges_exact_cli_repeats_without_merging_distinct_columns() {
        let output = concat!(
            "paper.typ:3:4: error: error: broken expression\n",
            "paper.typ:3:4: error: broken expression\n",
            "paper.typ:3:5: error: broken expression",
        );
        let diagnostics = parse_typst_short_output(output, Some(Path::new("paper.typ")));

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].message, "broken expression");
        assert_eq!(diagnostics[0].location.unwrap().column, 4);
        assert_eq!(diagnostics[1].location.unwrap().column, 5);
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
    #[test]
    fn normalization_preserves_first_seen_order_when_merging_non_adjacent_repeats() {
        let diagnostics = parse_typst_short_output(
            concat!(
                "paper.typ:3:4: error: first\n",
                "Hint: first detail\n",
                "paper.typ:8:2: warning: second\n",
                "paper.typ:3:4: error: first\n",
                "hint: FIRST DETAIL\n",
                "Note: later detail",
            ),
            Some(Path::new("paper.typ")),
        );

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].message, "first");
        assert_eq!(
            diagnostics[0].details,
            ["Hint: first detail", "Note: later detail"]
        );
        assert_eq!(diagnostics[1].message, "second");
    }
    #[test]
    fn ansi_stripping_preserves_non_csi_escapes_and_incomplete_sequences() {
        assert_eq!(strip_ansi_csi("plain"), "plain");
        assert_eq!(strip_ansi_csi("a\u{1b}]title"), "a\u{1b}]title");
        assert_eq!(
            strip_ansi_csi("before\u{1b}[31mred\u{1b}[0m after"),
            "beforered after"
        );
        assert_eq!(strip_ansi_csi("tail\u{1b}["), "tail\u{1b}[");
    }
}
