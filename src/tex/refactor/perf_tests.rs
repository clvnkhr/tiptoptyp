use super::*;

#[test]
fn repeated_large_parameter_words_are_rejected_before_unbounded_cloning() {
    let argument = vec![Kind::Word("a".repeat(MAX_SOURCE_BYTES / 2))];
    let body = (0..3)
        .flat_map(|_| [Kind::Character('#'), Kind::Character('1')])
        .collect::<Vec<_>>();
    let error = substitute(&body, &[argument]).unwrap_err();
    assert!(error.contains("size limit"), "{error}");
}

#[test]
fn large_literal_words_and_serialization_share_the_output_byte_limit() {
    let words = vec![Kind::Word("a".repeat(MAX_SOURCE_BYTES / 2)); 3];
    assert!(substitute(&words, &[]).unwrap_err().contains("size limit"));
    assert!(render(&words).unwrap_err().contains("size limit"));
}

#[test]
fn all_occurrences_cap_owned_word_bytes_even_with_few_output_tokens() {
    let name = "z".repeat(20_000);
    let source = format!(
        "\\newcommand{{\\{name}}}{{x}}\\newcommand{{\\target}}{{\\{name}}}\\begin{{document}}{}\\end{{document}}",
        "\\target ".repeat(300),
    );
    assert!(source.len() < MAX_SOURCE_BYTES / 10);
    let at = source.rfind("\\target").unwrap();
    let error = inline(&source, at, InlineScope::AllOccurrences).unwrap_err();
    assert!(error.contains("size limit"), "{error}");
}

#[test]
fn unused_large_defaults_have_a_finite_cumulative_work_budget() {
    let name = "z".repeat(20_000);
    let source = format!(
        "\\newcommand{{\\{name}}}{{x}}\\newcommand{{\\target}}[1][\\{name}]{{x}}\\begin{{document}}{}\\end{{document}}",
        // The literal y keeps the optional reader's lookahead away from the
        // next invocation, so refusal must be its work budget, not admission.
        "\\target y ".repeat(4000),
    );
    let at = source.rfind("\\target").unwrap();
    let error = inline(&source, at, InlineScope::AllOccurrences).unwrap_err();
    assert!(error.contains("too much expansion work"), "{error}");
}

#[test]
fn repeated_empty_substitutions_are_bounded_even_without_output_growth() {
    let body = "#1".repeat(1000);
    let source = format!(
        "\\newcommand{{\\target}}[1]{{{body}}}\\begin{{document}}{}\\end{{document}}",
        "\\target{}".repeat(10_000),
    );
    let at = source.rfind("\\target").unwrap();
    let error = inline(&source, at, InlineScope::AllOccurrences).unwrap_err();
    assert!(error.contains("too much expansion work"), "{error}");
}

#[test]
fn inline_all_keeps_the_clicked_occurrence_selected_after_earlier_edits() {
    let source =
        "\\newcommand{\\target}{\\alpha}\\begin{document}λ $\\target_i+\\target_j$\\end{document}";
    let at = source.rfind("\\target").unwrap();
    let plan = inline(source, at, InlineScope::AllOccurrences).unwrap();
    let applied = plan.apply(source).unwrap();
    assert_eq!(&applied.source[applied.selection.clone()], "\\alpha");
    assert_eq!(
        applied.selection.start,
        applied.source.rfind("\\alpha_j").unwrap()
    );
}

#[test]
fn adjacent_calls_use_the_expanded_previous_token_for_boundary_separators() {
    let source = "\\newcommand{\\target}{x}\\begin{document}$\\target\\target_i$\\end{document}";
    let at = source.rfind("\\target").unwrap();
    let plan = inline(source, at, InlineScope::AllOccurrences).unwrap();
    assert!(plan.preview().contains("$xx_i$"));
    let applied = plan.apply(source).unwrap();
    assert_eq!(&applied.source[applied.selection.clone()], "x");
    assert_eq!(
        applied.selection.start,
        applied.source.find("xx_i").unwrap() + 1
    );
}
