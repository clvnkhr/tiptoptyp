use super::*;

mod compiler;

fn document(definitions: &str, body: &str) -> String {
    format!(
        "\\documentclass{{article}}\n{definitions}\n\\begin{{document}}\n{body}\n\\end{{document}}\n"
    )
}

fn inline_body(definitions: &str, before: &str, after: &str) {
    let source = document(definitions, before);
    let caret = source.find("\\begin{document}").unwrap() + "\\begin{document}".len();
    let caret = caret + source[caret..].find("\\test").unwrap();
    let plan = inline(&source, caret, InlineScope::Occurrence).unwrap();
    assert_eq!(plan.preview(), document(definitions, after));
    assert_eq!(plan.occurrences(), 1);
    assert_eq!(plan.apply(&source).unwrap().source, plan.preview());
}

#[test]
fn inline_keeps_script_atom_binding_without_added_groups() {
    inline_body(
        "\\newcommand{\\test}{xy}",
        "$\\test_i + a^\\test$",
        "$xy_i + a^\\test$",
    );
    inline_body("\\newcommand{\\test}{xy}", "$a^\\test$", "$a^xy$");
    inline_body(
        "\\newcommand{\\test}[1]{#1^2}",
        "$\\test{ab}_i$",
        "$ab^2_i$",
    );
    inline_body(
        "\\newcommand{\\test}[1]{#1^2}",
        "$a^\\test{xy}$",
        "$a^xy^2$",
    );
}

#[test]
fn control_words_never_merge_and_ignored_call_whitespace_stays_ignored() {
    inline_body(
        "\\newcommand{\\test}{\\alpha}",
        "$\\test x_i$",
        "$\\alpha x_i$",
    );
    inline_body("\\newcommand{\\test}{x}", "$\\alpha\\test$", "$\\alpha x$");
    inline_body("\\newcommand{\\test}{word}", "\\test text", "wordtext");
    inline_body(
        "\\newcommand{\\test}{x}",
        "$\\test% note\n y$",
        "$x% note\n y$",
    );
}

#[test]
fn arguments_are_tokens_with_their_original_outer_braces_removed() {
    inline_body(
        "\\newcommand{\\test}[2]{#1+#2}",
        "$\\test{ab}{\\alpha}_i$",
        "$ab+\\alpha_i$",
    );
    let source = document("", "\\def\\test#1#2{#2#1}$\\test ab$");
    let at = source.rfind("\\test").unwrap();
    assert_eq!(
        inline(&source, at, InlineScope::Occurrence)
            .unwrap()
            .preview(),
        document("", "\\def\\test#1#2{#2#1}$ba$")
    );
    inline_body(
        "\\newcommand{\\test}[1]{(#1)}",
        "$\\test{{ab}}$",
        "$({ab})$",
    );
}

#[test]
fn optional_arguments_use_the_default_or_explicit_tokens() {
    inline_body("\\newcommand{\\test}[2][x]{#1+#2}", "$\\test{y}$", "$x+y$");
    inline_body(
        "\\newcommand{\\test}[2][x]{#1+#2}",
        "$\\test[z]{y}$",
        "$z+y$",
    );
}

#[test]
fn all_direct_uses_leave_definitions_and_literals_untouched() {
    let source = document(
        "\\newcommand{\\test}{xy}",
        "$\\test_i + a^\\test$ % \\test\n\\verb|\\test|",
    );
    let at = source.rfind("a^\\test").unwrap() + 2;
    let plan = inline(&source, at, InlineScope::AllOccurrences).unwrap();
    assert_eq!(plan.occurrences(), 2);
    assert_eq!(
        plan.preview(),
        document(
            "\\newcommand{\\test}{xy}",
            "$xy_i + a^xy$ % \\test\n\\verb|\\test|"
        )
    );
}

#[test]
fn reject_token_inspection_unbraced_outer_arguments_and_local_shadowing() {
    for (definitions, body) in [
        ("\\newcommand{\\test}{xy}", "\\textbf\\test"),
        ("\\newcommand{\\test}{xy}", "\\detokenize{\\test}"),
        ("\\newcommand{\\test}{xy}", "$\\frac 1\\test$"),
        ("{\\newcommand{\\test}{xy}}", "$\\test$"),
        (
            "\\newcommand{\\test}{xy}\\renewcommand{\\test}{z}",
            "$\\test$",
        ),
        ("\\providecommand{\\test}{xy}", "$\\test$"),
        ("\\DeclareRobustCommand{\\test}{xy}", "$\\test$"),
        ("\\newcommand{\\test}{2]}", "$\\sqrt[\\test]{x}$"),
    ] {
        let source = document(definitions, body);
        let at = source.rfind("\\test").unwrap();
        assert!(
            inline(&source, at, InlineScope::Occurrence).is_err(),
            "{source}"
        );
    }
}

#[test]
fn reject_dynamic_execution_and_unrepresentable_space_joins() {
    for (definitions, body) in [
        ("\\newcommand{\\test}[1]{\\LaTeX}", "\\test{x} word"),
        ("\\newcommand{\\test}{\\test}", "$\\test$"),
        ("\\newcommand{\\test}{xy}", "\\input{other} $\\test$"),
        ("\\catcode`\\_=12 \\newcommand{\\test}{xy}", "$\\test$"),
        ("\\newcommand{\\test}{xy}", "\\iftrue$\\test$\\fi"),
        ("\\def\\test#1,#2{#1#2}", "$\\test a,b$"),
        ("\\newcommand{\\test}{xy}", "% ^^5cdef\\test{z}\n$\\test$"),
        ("\\endlinechar=32 \\newcommand{\\test}{xy}", "$\\test$"),
    ] {
        let source = document(definitions, body);
        let at = source.rfind("\\test").unwrap();
        assert!(
            inline(&source, at, InlineScope::Occurrence).is_err(),
            "{source}"
        );
    }
}

#[test]
fn plans_reject_a_changed_snapshot_and_keep_unicode_byte_selections() {
    let source = document("\\newcommand{\\test}{λ}", "$\\test_i$");
    let at = source.rfind("\\test").unwrap();
    let plan = inline(&source, at, InlineScope::Occurrence).unwrap();
    let applied = plan.apply(&source).unwrap();
    assert_eq!(&applied.source[applied.selection], "λ");
    assert!(plan.apply(&(source + " ")).is_err());
}

#[test]
fn extraction_inserts_a_preamble_definition_and_preserves_scripts() {
    let source = document("", "$a^{x_i+\\alpha}+z$");
    let start = source.find("x_i+").unwrap();
    let end = source.find("}+z").unwrap();
    let plan = extract(&source, start..end, "energy").unwrap();
    assert_eq!(
        plan.preview(),
        source
            .replace(
                "\\begin{document}",
                "\\newcommand{\\energy}{x_i+\\alpha}%\n\\begin{document}"
            )
            .replace("a^{x_i+\\alpha}", "a^{\\energy}")
    );
    let applied = plan.apply(&source).unwrap();
    assert_eq!(&applied.source[applied.selection], "\\energy");
}

#[test]
fn extraction_preserves_a_following_prose_space_and_word_boundary() {
    let source = document("", "Some words here.");
    let start = source.find("Some").unwrap();
    let plan = extract(&source, start..start + 4, "sample").unwrap();
    assert!(plan.preview().contains("\\newcommand{\\sample}{Some }%"));
    assert!(plan.preview().contains("\\sample words here."));
}

#[test]
fn extraction_rejects_split_tokens_groups_math_delimiters_and_collisions() {
    let source = document("", "$a^{x_i+\\alpha}+z$");
    let alpha = source.find("\\alpha").unwrap();
    let group = source.find("{x_i").unwrap();
    for (range, name) in [
        (alpha + 2..alpha + 5, "energy"),
        (group..group + 3, "energy"),
        (alpha..alpha + 6, "alpha"),
        (alpha..alpha + 6, "with_underscore"),
        (
            source.find('$').unwrap()..source.rfind('$').unwrap() + 1,
            "energy",
        ),
    ] {
        assert!(extract(&source, range, name).is_err());
    }
}

#[test]
fn comments_discard_the_endline_and_indentation_in_bodies_and_extraction() {
    inline_body(
        "\\newcommand{\\test}{A% comment\n  B}",
        "\\test word",
        "ABword",
    );
    let source = document("", "A% comment\n  B tail");
    let start = source.find("A% comment").unwrap();
    let end = source.find(" tail").unwrap();
    assert!(extract(&source, start..end, "sample").is_err());
}

#[test]
fn unicode_boundaries_are_separated_and_engine_specific_existing_words_refused() {
    inline_body("\\newcommand{\\test}{λ}", "$\\alpha\\test$", "$\\alpha λ$");
    inline_body("\\newcommand{\\test}{\\alpha}", "$\\test λ$", "$\\alpha λ$");
    for body in ["$\\testλ$", "$\\λ+\\test$"] {
        let source = document("\\newcommand{\\test}{x}", body);
        assert!(
            inline(
                &source,
                source.rfind("\\test").unwrap(),
                InlineScope::Occurrence
            )
            .is_err()
        );
    }
}

#[test]
fn accents_and_optional_readers_cannot_change_argument_consumption() {
    inline_body(
        "\\newcommand{\\test}{\\'{a}b}",
        "\\test word",
        "\\'{a}bword",
    );
    inline_body(
        "\\newcommand{\\test}[1]{\\hat{#1}}",
        "$\\test{ab}_i$",
        "$\\hat{ab}_i$",
    );
    for body in [
        "\\'\\test",
        "\\'{\\test}",
        "\\\"{\\test}",
        "$x\\pmod\\test$",
        "\\item[\\test]",
        "\\item\\test",
        "$\\smash[\\test]{x}$",
        "$\\dots\\test$",
    ] {
        let source = document("\\newcommand{\\test}{a+b}", body);
        assert!(
            inline(
                &source,
                source.rfind("\\test").unwrap(),
                InlineScope::Occurrence
            )
            .is_err(),
            "{source}"
        );
    }
    for accent in [
        "hat",
        "widehat",
        "bar",
        "vec",
        "dot",
        "ddot",
        "dddot",
        "ddddot",
        "acute",
        "grave",
        "breve",
        "check",
        "tilde",
        "widetilde",
        "overline",
        "underline",
        "overbrace",
        "underbrace",
    ] {
        let source = document(
            "\\newcommand{\\test}{ab}",
            &format!("$\\{accent}{{\\test}}$"),
        );
        assert!(
            inline(
                &source,
                source.rfind("\\test").unwrap(),
                InlineScope::Occurrence
            )
            .is_err(),
            "{source}"
        );
        let source = document("", &format!("$\\{accent}{{x_i}}$"));
        let start = source.find("x_i").unwrap();
        assert!(
            extract(&source, start..start + 3, "energy").is_err(),
            "{source}"
        );
    }
    let source = document("", "\\'{ab}");
    let start = source.rfind("ab").unwrap();
    assert!(extract(&source, start..start + 2, "energy").is_err());
}

#[test]
fn stored_helpers_cannot_hide_redefinitions_imports_defaults_or_unknown_symbols() {
    for helper in [
        "\\newcommand{\\reset}{\\renewcommand{\\test}{y}}",
        "\\newcommand{\\reset}{\\def\\test{y}}",
        "\\newcommand{\\reset}{\\input{other}}",
        "\\newcommand{\\reset}[1][\\renewcommand{\\test}{y}]{#1}",
        "\\newcommand{\\reset}{\\?}",
        "\\newcommand{\\reset}{\\begin{equation}}",
        "\\newcommand{\\reset}{\\bibliography{other}}",
    ] {
        let source = document(&format!("\\newcommand{{\\test}}{{x}}{helper}"), "$\\test$");
        assert!(
            inline(
                &source,
                source.rfind("\\test").unwrap(),
                InlineScope::Occurrence
            )
            .is_err(),
            "{source}"
        );
    }
}

#[test]
fn other_macro_redefinitions_cannot_change_an_outer_argument_signature() {
    let source = document(
        "\\newcommand{\\take}[1]{\\textbf{#1}}\\newcommand{\\test}{ab}",
        "\\take\\test\\renewcommand{\\take}{x}",
    );
    let at = source.find("\\take\\test").unwrap() + "\\take".len();
    assert!(inline(&source, at, InlineScope::Occurrence).is_err());
}

#[test]
fn extraction_preserves_crlf_declarations_and_existing_source() {
    let source = document("", "$x_i+y$").replace('\n', "\r\n");
    let start = source.find("x_i+y").unwrap();
    let plan = extract(&source, start..start + 5, "energy").unwrap();
    assert!(
        plan.preview()
            .contains("\\newcommand{\\energy}{x_i+y}%\r\n\\begin{document}")
    );
    assert!(!plan.preview().replace("\r\n", "").contains('\n'));
}

#[test]
fn optional_field_index_uses_the_first_close_outside_brace_groups() {
    let source = document("", "$\\sqrt[{a]b}c]{x}$");
    let analysis = Analysis::new(&source).unwrap();
    let at = analysis
        .tokens
        .iter()
        .position(|token| token.kind.word() == Some("sqrt"))
        .unwrap();
    let optional = analysis.optional(at + 1).unwrap().unwrap();
    assert_eq!(
        analysis.tokens[optional.inner.end].bytes.start,
        source.find("c]").unwrap() + 1
    );
    assert_eq!(analysis.optional_closes.len(), analysis.tokens.len() + 1);
}

#[test]
fn existing_command_overrides_cannot_cross_environment_or_begin_document_hooks() {
    for definition in ["\\renewcommand{\\tag}{x}", "\\def\\tag{x}"] {
        let source = document(
            &format!("\\usepackage{{amsmath}}\n{definition}"),
            "\\begin{align}a&=b\\tag{1}\\end{align}",
        );
        let at = source.rfind("\\tag").unwrap();
        assert!(inline(&source, at, InlineScope::Occurrence).is_err());
    }
    let source = document("\\def\\test{x}", "$\\test$");
    assert!(
        inline(
            &source,
            source.rfind("\\test").unwrap(),
            InlineScope::Occurrence
        )
        .is_err()
    );
    let source = document("", "$\\newcommand{\\test}{x}$ $\\test$");
    assert!(
        inline(
            &source,
            source.rfind("\\test").unwrap(),
            InlineScope::Occurrence
        )
        .is_err()
    );
}

#[test]
fn caller_comments_and_suppression_markers_are_preserved_or_refactoring_refused() {
    let source = document(
        "\\newcommand{\\test}{x}",
        "$\\test%diag:off\n +y%diag:on\n$",
    );
    let at = source.rfind("\\test").unwrap();
    let plan = inline(&source, at, InlineScope::Occurrence).unwrap();
    assert_eq!(plan.preview(), source.replacen("$\\test", "$x", 1));
    for (definition, body) in [
        ("\\newcommand{\\test}{x}", "$\\test\n%ttt:off\n+y$"),
        ("\\newcommand{\\test}[1]{#1}", "$\\test{%fmt:off\n x}$"),
    ] {
        let source = document(definition, body);
        assert!(
            inline(
                &source,
                source.rfind("\\test").unwrap(),
                InlineScope::Occurrence
            )
            .is_err()
        );
    }
}
