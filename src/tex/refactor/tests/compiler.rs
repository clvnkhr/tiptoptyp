//! Opt-in proof against the packaged XeTeX engine, independent of our lexer.
use super::*;
use std::{fs, path::Path, process::Command};

/// Requires the pinned Tectonic executable and Poppler. It compares actual TeX
/// expansion strings where fully expandable, hbox metrics, and every RGB pixel
/// on the resulting page. PDF metadata and timestamps are deliberately ignored.
#[test]
#[ignore = "requires the pinned Tectonic bundle/cache and pdftoppm"]
fn pinned_tex_expansion_metrics_and_pixels_survive_refactoring() {
    let engine = std::env::var_os("TIPTOPTYP_REFACTOR_TECTONIC")
        .expect("Set TIPTOPTYP_REFACTOR_TECTONIC to the pinned Tectonic executable");
    let rasterizer = std::env::var_os("TIPTOPTYP_REFACTOR_PDFTOPPM")
        .expect("Set TIPTOPTYP_REFACTOR_PDFTOPPM to pdftoppm");
    // This exact argument-reader case previously failed the independent TeX
    // proof: \'{macro->ab} emitted ab plus a combining accent, whereas \'{ab}
    // emitted just á. The production planner must refuse that transformation.
    for body in ["\\'{\\tttLetters}", "$\\hat{\\tttLetters}$"] {
        let source = document("\\newcommand{\\tttLetters}{ab}", body);
        assert!(
            inline(
                &source,
                source.rfind("\\tttLetters").unwrap(),
                InlineScope::Occurrence
            )
            .is_err()
        );
    }
    let cases = [
        (
            document(
                "\\newcommand{\\tttSquare}[1]{#1^2}\n\\newcommand{\\tttWord}{ab}\n\\newcommand{\\tttSym}{\\alpha}",
                "$\\tttSquare{ab}_i+c^\\tttSquare{xy}+d^{\\tttSquare{x}}$\n$\\tttWord_i+c^\\tttWord+\\alpha\\tttWord+\\tttSym x$",
            ),
            vec!["tttSquare", "tttWord", "tttSym"],
            true,
        ),
        (
            document(
                "\\newcommand{\\tttPair}[2][x]{#1+#2}\n\\newcommand{\\tttJoined}{A% comment\n  B}",
                "$\\tttPair{b}_i+\\tttPair[z]{d}^2$\n\\tttJoined word",
            ),
            vec!["tttPair", "tttJoined"],
            false,
        ),
        (
            document(
                "\\newcommand{\\tttLetters}{\\'{a}b}\n\\newcommand{\\tttHat}[1]{\\hat{#1}}",
                "[\\tttLetters]\n$\\tttHat{ab}_i$",
            ),
            vec!["tttLetters", "tttHat"],
            true,
        ),
    ];
    let workspace = tempfile::tempdir().unwrap();
    for (index, (before, names, compare_tokens)) in cases.into_iter().enumerate() {
        let mut after = before.clone();
        for name in names {
            let body_start = after.find("\\begin{document}").unwrap();
            let at = body_start + after[body_start..].find(&format!("\\{name}")).unwrap();
            after = inline(&after, at, InlineScope::AllOccurrences)
                .unwrap()
                .preview()
                .into();
        }
        compare(
            &engine,
            &rasterizer,
            workspace.path(),
            index,
            &before,
            &after,
            compare_tokens,
        );
    }
    let before = document("", "$a^{x_i+\\alpha}+z$");
    let start = before.find("x_i+").unwrap();
    let end = before.find("}+z").unwrap();
    let after = extract(&before, start..end, "tttEnergy").unwrap();
    compare(
        &engine,
        &rasterizer,
        workspace.path(),
        3,
        &before,
        after.preview(),
        true,
    );
}

fn compare(
    engine: &std::ffi::OsStr,
    rasterizer: &std::ffi::OsStr,
    root: &Path,
    index: usize,
    before: &str,
    after: &str,
    compare_tokens: bool,
) {
    let mut artifacts = Vec::new();
    for (label, source) in [("before", before), ("after", after)] {
        let directory = root.join(format!("{index}-{label}"));
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join("document.tex"),
            instrument(source, compare_tokens),
        )
        .unwrap();
        let compiled = Command::new(engine)
            .args([
                "-X",
                "compile",
                "--only-cached",
                "--untrusted",
                "--keep-logs",
                "--keep-intermediates",
                "--reruns",
                "0",
                "document.tex",
            ])
            .current_dir(&directory)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "Tectonic case {index}-{label}: {}\n{}",
            String::from_utf8_lossy(&compiled.stdout),
            String::from_utf8_lossy(&compiled.stderr)
        );
        let rendered = Command::new(rasterizer)
            .args([
                "-r",
                "120",
                "-f",
                "1",
                "-l",
                "1",
                "-singlefile",
                "document.pdf",
                "page",
            ])
            .current_dir(&directory)
            .output()
            .unwrap();
        assert!(
            rendered.status.success(),
            "pdftoppm: {}",
            String::from_utf8_lossy(&rendered.stderr)
        );
        artifacts.push((
            fs::read(directory.join("metrics.txt")).unwrap(),
            fs::read(directory.join("page.ppm")).unwrap(),
        ));
    }
    assert_eq!(
        artifacts[0].0, artifacts[1].0,
        "case {index}: TeX expansion/box metrics changed"
    );
    assert_eq!(
        artifacts[0].1, artifacts[1].1,
        "case {index}: rendered glyph pixels changed"
    );
}

fn instrument(source: &str, compare_tokens: bool) -> String {
    let begin = source.find("\\begin{document}").unwrap() + "\\begin{document}".len();
    let end = source.rfind("\\end{document}").unwrap();
    let body = source[begin..end].trim();
    let mut measured =
        String::from("\n\\newwrite\\tttMetrics\n\\immediate\\openout\\tttMetrics=metrics.txt\n");
    if compare_tokens {
        measured.push_str(&format!("\\edef\\tttTokens{{{body}}}\n\\immediate\\write\\tttMetrics{{TOKENS:\\meaning\\tttTokens}}\n"));
    }
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        measured.push_str(&format!("\\setbox0=\\hbox{{{line}}}\n\\immediate\\write\\tttMetrics{{BOX:\\the\\wd0,\\the\\ht0,\\the\\dp0}}\n\\noindent\\box0\\par\n"));
    }
    measured.push_str("\\immediate\\closeout\\tttMetrics\n");
    format!("{}{measured}{}", &source[..begin], &source[end..])
}
