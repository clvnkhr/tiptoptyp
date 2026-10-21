use super::bibtex::parse_entry;
use super::*;
use std::{fs, sync::atomic::AtomicBool};

const FIRST: &str = "@article{Smith2024,\n title = {A {Nested} Title}, author = {Smith, A.}, doi = {10.1000/FIRST}, year = 2024\n}";
const SECOND: &str =
    "@misc{Jones2025, title = {Another title}, eprint = {2401.01234v2}, archivePrefix = {arXiv}}";
fn running() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn identifiers_accept_official_urls_encoded_punctuation_and_both_arxiv_schemes() {
    for input in [
        "10.1000/ABC",
        "doi:10.1000/ABC",
        "https://doi.org/10.1000/ABC",
        "http://dx.doi.org/10.1000/ABC",
    ] {
        assert_eq!(
            Identifier::parse(input).unwrap(),
            Identifier::Doi("10.1000/abc".into())
        );
    }
    assert_eq!(
        Identifier::parse("https://doi.org/10.1000/a%3Fb%23c")
            .unwrap()
            .value(),
        "10.1000/a?b#c"
    );
    for input in [
        "arXiv:2401.01234v2",
        "2401.01234v2",
        "https://arxiv.org/abs/2401.01234v2",
        "https://arxiv.org/pdf/2401.01234v2.pdf?download=1",
    ] {
        assert_eq!(
            Identifier::parse(input).unwrap(),
            Identifier::Arxiv("2401.01234v2".into())
        );
    }
    assert_eq!(
        Identifier::parse("hep-th/9901001v3").unwrap().value(),
        "hep-th/9901001v3"
    );
    assert_eq!(
        Identifier::parse("https://arxiv.org/abs/math.GT/0301234")
            .unwrap()
            .value(),
        "math.GT/0301234"
    );
}

#[test]
fn identifiers_reject_arbitrary_urls_credentials_queries_and_invalid_months() {
    for input in [
        "",
        "https://localhost/10.1000/x",
        "https://doi.org.evil/10.1000/x",
        "file:///tmp/x",
        "https://u@doi.org/10.1000/x",
        "https://doi.org:8080/10.1000/x",
        "https://doi.org/10.1000/x?q=y",
        "10.100/a",
        "10.1000/a b",
        "10.1000/",
        "2400.01234",
        "2413.01234",
        "math/0313001",
        "2401.12345v0",
        "https://arxiv.org/list/math",
        "https://doi.org/10.1000/%00",
        "https://doi.org/10.1000/%Q1",
    ] {
        assert!(Identifier::parse(input).is_err(), "accepted {input}");
    }
}

#[test]
fn parser_handles_nested_and_quoted_braces_comments_and_strings_without_rewriting() {
    let source = concat!(
        "% a comment @fake{x}\n@string{prefix = \"10.1000/\"}\n",
        "@comment{anything {nested}}\n@preamble{\"hello\"}\n",
        "@article(Key, title = \"A {quoted} title with \\\"quotes\\\"\",\n",
        "% inline comment\ndoi = prefix # {mixed}, month = jan, year = 2024,)\n"
    );
    let entries = bibtex::parse_database(source).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "Key");
    assert_eq!(entries[0].doi().as_deref(), Some("10.1000/mixed"));
    assert!(entries[0].bibtex.starts_with("@article(Key,"));
    assert_eq!(parse_entry(FIRST).unwrap().bibtex, FIRST);
    assert_eq!(
        parse_entry(&format!("\u{feff}{FIRST}")).unwrap().key,
        "Smith2024"
    );
}

#[test]
fn parser_rejects_html_plain_text_incomplete_duplicates_and_unresolved_import_macros() {
    for source in [
        "<html>error</html>",
        "not bibtex",
        "@article{x,title={broken}",
        "@article{x,title=\"broken}",
        "@article{x,title={one},TITLE={two}}",
        "@article{x,title={a}} @article{y,title={b}}",
        "@article{x,title=undefined}",
        "@string{x={a}} @article{y,title=x}",
        "@article{x,title={a\0b}}",
    ] {
        assert!(parse_entry(source).is_err(), "accepted {source:?}");
    }
    assert!(
        bibtex::parse_database("% comments only\n")
            .unwrap()
            .is_empty()
    );
    let nested = format!("@misc{{x,title={}{}}}", "{".repeat(65), "}".repeat(65));
    assert!(parse_entry(&nested).is_err());
}

#[test]
fn rekey_changes_only_the_actual_key_and_requires_portable_key() {
    let changed = rekey(FIRST, "My-Key:2024").unwrap();
    assert_eq!(changed, FIRST.replacen("Smith2024", "My-Key:2024", 1));
    for key in ["", "with space", "x,y", "x}", "#a", "\\input"] {
        assert!(rekey(FIRST, key).is_err());
    }
}

#[test]
fn atomic_creation_and_append_preserve_all_existing_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.bib");
    let snapshot = BibliographySnapshot::read(&path).unwrap();
    assert!(!snapshot.exists);
    let result = snapshot.import(FIRST, &running()).unwrap();
    assert!(result.added);
    assert_eq!(result.key, "Smith2024");
    let original = fs::read(&path).unwrap();
    assert_eq!(original, format!("{FIRST}\n").as_bytes());
    let snapshot = BibliographySnapshot::read(&path).unwrap();
    assert!(snapshot.exists);
    assert!(snapshot.import(SECOND, &running()).unwrap().added);
    assert_eq!(
        fs::read(&path).unwrap(),
        format!("{FIRST}\n\n{SECOND}\n").as_bytes()
    );
}

#[test]
fn doi_duplicate_returns_existing_key_and_never_modifies_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("r.bib");
    fs::write(&path, format!("% keep formatting\r\n{FIRST}")).unwrap();
    let before = fs::read(&path).unwrap();
    let result = BibliographySnapshot::read(&path).unwrap().import("@article{other, doi={https://doi.org/10.1000/first}, title={Different provider formatting}}", &running()).unwrap();
    assert!(!result.added);
    assert_eq!(result.key, "Smith2024");
    assert_eq!(result.bibtex, FIRST);
    assert_eq!(fs::read(&path).unwrap(), before);
    let escaped = "@article{another,doi={{10.1000/FIRST}}}";
    assert!(
        !BibliographySnapshot::read(&path)
            .unwrap()
            .import(escaped, &running())
            .unwrap()
            .added
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn arxiv_duplicate_ignores_version_and_matches_arxiv_doi() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("r.bib");
    fs::write(&path, SECOND).unwrap();
    let snapshot = BibliographySnapshot::read(&path).unwrap();
    for entry in [
        "@misc{newkey,eprint={arXiv:2401.01234v4}}",
        "@article{newkey,doi={10.48550/arXiv.2401.01234}}",
        "@article{newkey,url={https://arxiv.org/abs/2401.01234v3}}",
    ] {
        let result = snapshot.import(entry, &running()).unwrap();
        assert!(!result.added);
        assert_eq!(result.key, "Jones2025");
    }
    assert_eq!(fs::read_to_string(path).unwrap(), SECOND);
}

#[test]
fn conflicting_key_is_an_error_instead_of_citing_a_different_paper() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("r.bib");
    fs::write(&path, FIRST).unwrap();
    let snapshot = BibliographySnapshot::read(&path).unwrap();
    assert!(
        snapshot
            .import("@article{smith2024,doi={10.1000/other}}", &running())
            .unwrap_err()
            .contains("different entry")
    );
    assert_eq!(fs::read_to_string(path).unwrap(), FIRST);
}

#[test]
fn externally_modified_replaced_or_new_destination_is_never_clobbered() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("r.bib");
    let missing = BibliographySnapshot::read(&path).unwrap();
    fs::write(&path, FIRST).unwrap();
    assert!(missing.import(SECOND, &running()).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), FIRST);
    let snapshot = BibliographySnapshot::read(&path).unwrap();
    fs::write(&path, format!("{FIRST}\n% external")).unwrap();
    assert!(snapshot.import(SECOND, &running()).is_err());
    assert!(fs::read_to_string(&path).unwrap().ends_with("% external"));
    let snapshot = BibliographySnapshot::read(&path).unwrap();
    let replacement = directory.path().join("replacement.bib");
    fs::write(&replacement, fs::read(&path).unwrap()).unwrap();
    fs::rename(replacement, &path).unwrap();
    assert!(snapshot.import(SECOND, &running()).is_err());
}

#[test]
fn invalid_bibliographies_nonbib_paths_symlinks_and_cancellation_have_no_side_effects() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("r.bib");
    for source in ["<html>wrong</html>", "@misc{x,title={broken}"] {
        fs::write(&path, source).unwrap();
        assert!(BibliographySnapshot::read(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
    }
    assert!(BibliographySnapshot::read(&directory.path().join("r.tex")).is_err());
    assert!(BibliographySnapshot::read(std::path::Path::new("relative.bib")).is_err());
    fs::remove_file(&path).unwrap();
    let snapshot = BibliographySnapshot::read(&path).unwrap();
    assert!(snapshot.import(FIRST, &AtomicBool::new(true)).is_err());
    assert!(!path.exists());
    assert!(lookup("10.1000/first", &AtomicBool::new(true)).is_err());
    #[cfg(unix)]
    {
        let target = directory.path().join("target.bib");
        fs::write(&target, FIRST).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(BibliographySnapshot::read(&path).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), FIRST);
    }
}
