//! Reviewed 2026-10-23 against pinned CLI help, installed TeX Live 2025 help,
//! and the primary sources linked with each entry. This is app-specific advice,
//! not an offline copy of upstream manuals or their complete option schemas.
use super::{Example, Reference, ReferenceId as Id, Source};

macro_rules! example {
    ($label:expr, $syntax:expr, $language:expr, $explanation:expr) => {
        Example {
            label: $label,
            syntax: $syntax,
            language: $language,
            explanation: $explanation,
        }
    };
}
macro_rules! source {
    ($title:expr, $url:expr) => {
        Source {
            title: $title,
            url: $url,
        }
    };
}
const PROCESS: &str = "Arguments are literal tokens with quoting, not a shell command: no executable prefix, $HOME, ~ or shell expansion. Environment is a JSON object of strings. Working Directory is blank for the app default, or an existing absolute directory such as /Users/me/paper. The app adds document paths and transport/output options.";

pub(super) const REFERENCES: [Reference; Id::ALL.len()] = [
    Reference {
        id: Id::Typst,
        title: "Typst",
        reviewed_version: "0.15.1",
        summary: "Compiles PDF output. Interactive Tinymist preview uses a separate compiler; keep their fonts and template inputs consistent.",
        examples: &[
            example!(
                "Arguments",
                "watch --diagnostic-format short --font-path /Users/me/fonts --input mode=draft",
                "text",
                "Add fonts and a template input while retaining incremental builds and parseable diagnostics. The document reads sys.inputs.mode."
            ),
            example!(
                "Environment",
                r#"{"TYPST_FONT_PATHS":"/Users/me/fonts","SOURCE_DATE_EPOCH":"1767225600"}"#,
                "json",
                "Additional font lookup and a fixed document creation timestamp. Explicit CLI font paths can take precedence."
            ),
        ],
        notes: &[
            "Keep watch and --diagnostic-format short. Input/output paths, project root and workspace font discovery belong to the app. Do not change the output format away from PDF. --jobs 2 can reduce CPU contention at the expense of compilation time.",
            "This exporter has no additional Problems-diagnostic preset. Keep short diagnostics. Optional --timings /tmp/typst-timings.json measures compilation and writes an additional file; it is not extra checking.",
            PROCESS,
        ],
        sources: &[
            source!(
                "Versioned CLI options",
                "https://github.com/typst/typst/blob/v0.15.1/crates/typst-cli/src/args.rs"
            ),
            source!(
                "Template inputs",
                "https://typst.app/docs/reference/foundations/sys/#definitions-inputs"
            ),
        ],
    },
    Reference {
        id: Id::Tinymist,
        title: "Tinymist",
        reviewed_version: "0.15.2",
        summary: "Provides Typst intelligence, formatting and live preview. Configure the language server, not a separate preview command.",
        examples: &[
            example!(
                "Arguments",
                "lsp --font-path /Users/me/fonts",
                "text",
                "Keep the lsp command first for the stdio connection; do not substitute preview or compile."
            ),
            example!(
                "Configuration JSON",
                r#"{"formatterMode":"typstyle","formatterPrintWidth":100,"formatterProseWrap":false,"typstExtraArgs":["--input=mode=draft"]}"#,
                "json",
                "Merge directly into Tinymist configuration, without a tinymist wrapper. In 0.15.2 prose wrapping is boolean; typstExtraArgs here accepts input values."
            ),
            example!(
                "Verbose diagnostics",
                r#"{"lint":{"enabled":true,"when":"onType"}}"#,
                "json",
                "Enable Tinymist's extra lint diagnostics while typing. Normal disables this optional linter. Arguments and process logging are unchanged; applying the configuration restarts the server."
            ),
        ],
        notes: &[
            "The app owns the designated entry, preview transport, viewport scheduling and PDF export. Leave exportPdf set to never unless extra automatic output files are wanted. JSON changes cannot add UI features the client does not implement.",
            "The embedded Typst fork is separate from the bundled Typst PDF exporter. A new Tinymist release needs its own configuration review.",
            PROCESS,
        ],
        sources: &[
            source!(
                "Versioned configuration",
                "https://github.com/Myriad-Dreamin/tinymist/blob/v0.15.2/crates/tinymist/src/config.rs"
            ),
            source!(
                "CLI and logging",
                "https://myriad-dreamin.github.io/tinymist/feature/cli.html"
            ),
        ],
    },
    Reference {
        id: Id::Tectonic,
        title: "Tectonic",
        reviewed_version: "0.17.0",
        summary: "Builds a standalone TeX document to a private PDF. The app handles exporting that result beside the source.",
        examples: &[
            example!(
                "Normal Arguments",
                "-X --chatter minimal --color never compile --untrusted --keep-logs --keep-intermediates",
                "text",
                "Root options go before compile; compile options follow it. These are the app defaults, with useful logs retained."
            ),
            example!(
                "Verbose diagnostics (Settings JSON fragment)",
                r#"{"tex":{"show_underfull_box_warnings":true}}"#,
                "json",
                "Show underfull-box warnings in Problems. Normal hides these cosmetic warnings. This display preference is shared with the external TeX engines; neither preset changes Arguments or process chatter."
            ),
            example!(
                "Environment",
                r#"{"TECTONIC_CACHE_DIR":"/Users/me/tectonic-cache"}"#,
                "json",
                "Move downloaded resources and cached formats; this does not choose the output directory."
            ),
        ],
        notes: &[
            "Keep -X compile, --untrusted, PDF output, logs and intermediates. The app supplies --outdir, input and the SyncTeX/offline-resource toggles. Only cached resources fails if required packages are missing.",
            "Do not add shell escape or deterministic mode to bypass these constraints. Project Tectonic.toml belongs to Tectonic's separate project-build command.",
            PROCESS,
        ],
        sources: &[
            source!(
                "Compile options",
                "https://tectonic-typesetting.github.io/book/latest/v2cli/compile.html"
            ),
            source!(
                "Resource cache",
                "https://tectonic-typesetting.github.io/book/latest/getting-started/first-document.html#cache"
            ),
        ],
    },
    Reference {
        id: Id::Texlab,
        title: "TexLab",
        reviewed_version: "5.26.0",
        summary: "LaTeX completion, hover and references run over stdio. The app owns builds and the chosen formatter.",
        examples: &[
            example!(
                "Verbose diagnostics",
                r#"{"texlab":{"diagnostics":{"allowedPatterns":[],"ignoredPatterns":[]}}}"#,
                "json",
                "Show diagnostics hidden by TexLab pattern filters. The Verbose preset also clears this provider's ignored diagnostic codes. It does not change Arguments, logging, builds or ChkTeX activation."
            ),
            example!(
                "Configuration JSON",
                r#"{"texlab":{"build":{"onSave":false,"forwardSearchAfter":false},"chktex":{"onEdit":false,"onOpenAndSave":false},"latexFormatter":"none","diagnosticsDelay":300}}"#,
                "json",
                "Merge under the texlab object. Keep duplicate compiler/ChkTeX/formatter work disabled unless deliberately enabling another provider."
            ),
            example!(
                "Environment",
                r#"{"RUST_BACKTRACE":"1"}"#,
                "json",
                "Include a backtrace if the Rust process crashes; it does not increase normal logging."
            ),
        ],
        notes: &[
            "Keep a stdio language server, not the inverse-search subcommand. Ignored diagnostic codes and ignoredPatterns affect TexLab only. Match codes exactly; suppressing TexLab does not suppress Badness or grammar providers.",
            "rootDirectory is not a setting in the pinned schema. Custom math environments belong in texlab.experimental.mathEnvironments.",
            PROCESS,
        ],
        sources: &[
            source!(
                "Versioned CLI",
                "https://github.com/latex-lsp/texlab/blob/v5.26.0/crates/texlab/src/main.rs"
            ),
            source!(
                "Versioned configuration",
                "https://github.com/latex-lsp/texlab/blob/v5.26.0/crates/texlab/src/server/options.rs"
            ),
        ],
    },
    Reference {
        id: Id::Badness,
        title: "Badness",
        reviewed_version: "0.24.0",
        summary: "LaTeX formatting and linting use badness lsp. Most formatting choices belong in a project badness.toml.",
        examples: &[
            example!(
                "Arguments",
                "lsp",
                "text",
                "Keep the language-server command. There is no supported general verbose switch for this integration."
            ),
            example!(
                "badness.toml",
                "[format]\nline-width = 100\nindent-width = 2\nwrap = \"preserve\"\n\n[lint]\nignore = [\"redundant-script-braces\"]",
                "toml",
                "Preserve authored prose wrapping and suppress one provider rule in the project. App Ignored diagnostic codes is a separate display filter."
            ),
            example!(
                "Configuration JSON",
                r#"{"lineWidth":100,"indentWidth":2}"#,
                "json",
                "Language-server fallback values. A discovered project TOML takes precedence."
            ),
            example!(
                "Environment",
                r#"{"BADNESS_CONFIG":"/Users/me/paper/badness.toml"}"#,
                "json",
                "Fallback configuration when no project file was found. A discovered project file wins."
            ),
        ],
        notes: &[
            "In 0.24.0, --config and --no-config do not select configuration for lsp. Discovery begins at each document's directory, so changing Working Directory is not a project-selector workaround.",
            "Normal keeps the stdio protocol unchanged and hides the twelve configured cosmetic lint codes. Verbose clears those ignored codes to show the full lint diagnostics; the lsp command and formatting settings stay unchanged. Badness has no --verbose process flag.",
            PROCESS,
        ],
        sources: &[
            source!(
                "Configuration",
                "https://badness.dev/reference/configuration.html"
            ),
            source!(
                "Versioned implementation",
                "https://github.com/jolars/badness/blob/v0.24.0/src/config.rs"
            ),
            source!("CLI reference", "https://badness.dev/reference/cli.html"),
        ],
    },
    Reference {
        id: Id::TexFmt,
        title: "tex-fmt",
        reviewed_version: "0.5.7",
        summary: "Formats LaTeX through stdin/stdout. Select tex-fmt as the TeX formatter before changing its options.",
        examples: &[
            example!(
                "Arguments",
                "--stdin --quiet --wraplen 100 --tabsize 2",
                "text",
                "Wrap at 100 columns with two-space indentation. --nowrap instead preserves long lines."
            ),
            example!(
                "Explicit configuration",
                "--stdin --quiet --config /Users/me/paper/tex-fmt.toml",
                "text",
                "Choose one file; otherwise discovery checks the working directory, repository and user configuration. CLI options take precedence."
            ),
            example!(
                "Table formatting",
                "--stdin --quiet --format-tables",
                "text",
                "Align table ampersands during formatting. This changes formatted source, not Problems diagnostics; review complex tables before using it broadly."
            ),
            example!(
                "Environment",
                r#"{"RUST_BACKTRACE":"1"}"#,
                "json",
                "Crash backtraces only. Normal operation needs no special environment."
            ),
        ],
        notes: &[
            "Retain --stdin --quiet for the app’s formatting connection. tex-fmt formats source rather than producing an extra set of Problems diagnostics, so it has no diagnostic Verbose preset. Process --verbose is unrelated to Problems checking.",
            "A line ending in % tex-fmt: skip is preserved. % tex-fmt: off and % tex-fmt: on protect a region.",
            PROCESS,
        ],
        sources: &[source!(
            "Versioned options and configuration",
            "https://github.com/WGUNDERWOOD/tex-fmt/blob/v0.5.7/README.md"
        )],
    },
    Reference {
        id: Id::Ltex,
        title: "LTeX+",
        reviewed_version: "18.7.0 / jdk-21.0.10+7",
        summary: "Optional live spelling and grammar for TeX and Typst. The bundled server and Java start on a worker after enabling LTeX+.",
        examples: &[
            example!(
                "Configuration JSON",
                r#"{"language":"en-US","checkFrequency":"manual","dictionary":{"en-US":["semigroup","Romito"]},"disabledRules":{"en-US":["TOO_LONG_SENTENCE"]},"ltex-ls":{"logLevel":"warning"}}"#,
                "json",
                "Merge into the existing object, without an ltex wrapper. Keep other dialect dictionaries and rule choices. Disable Follow document language to use this language explicitly."
            ),
            example!(
                "Environment",
                r#"{"JAVA_OPTS":"-Xmx1024m"}"#,
                "json",
                "Example Java heap limit. The bundled launcher already gets its matching Java; overriding JAVA_HOME/JAVACMD changes that choice."
            ),
            example!(
                "Verbose diagnostics",
                r#"{"disabledRules":{},"enabledRules":{},"additionalRules":{"enablePickyRules":true}}"#,
                "json",
                "Restore LanguageTool's ordinary rules and enable its optional picky checks, including passive-voice and sentence-length advice. Expect more false positives. Logging stays unchanged."
            ),
        ],
        notes: &[
            "Arguments normally stays empty. Retain standardStream transport and editor-owned checkFrequency manual: the editor debounces live checks and rejects obsolete results. Do not use TCP or endless mode.",
            "Dictionaries and rule IDs are per language. Harper's user dictionary is separate. Normal restores the quiet academic rule overrides and disables picky checks; Verbose clears rule overrides and enables picky checks. Both preserve dictionary, language, hidden false positives, manual scheduling, custom Arguments and logLevel.",
            PROCESS,
        ],
        sources: &[
            source!(
                "Server configuration",
                "https://ltex-plus.github.io/ltex-plus/settings.html"
            ),
            source!(
                "Launcher environment and transport",
                "https://ltex-plus.github.io/ltex-plus/ltex-ls-plus/server-usage.html"
            ),
            source!(
                "Pinned source",
                "https://github.com/ltex-plus/ltex-ls-plus/tree/18.7.0"
            ),
        ],
    },
    Reference {
        id: Id::Harper,
        title: "Harper",
        reviewed_version: "2.11.0",
        summary: "In-process English spelling and grammar. The app projects TeX/Typst prose and reuses unchanged paragraph results; no executable or Java is involved.",
        examples: &[
            example!(
                "Harper rule configuration",
                r#"{"SpellCheck":true,"RepeatedWords":true,"AnA":true,"LongSentences":false}"#,
                "json",
                "Use exact rule IDs and booleans. Merge with the editable academic defaults; invalid names are rejected."
            ),
            example!(
                "Harper dictionary",
                "semigroup\nnonautonomous\nRomito",
                "text",
                "One accepted word per line in the app's editable dictionary. This affects spelling, not grammar rules."
            ),
            example!(
                "Settings JSON fragment",
                r#"{"writing_language":"auto","harper_ignore_capitalized_names":true}"#,
                "json",
                "Auto follows literal document language with UK fallback. The optional capitalized-name filter changes spelling messages only."
            ),
        ],
        notes: &[
            "Normal uses the quiet academic rule overrides. Verbose clears those overrides to show Harper's curated default rules, including extra style advice; it is not process logging. Neither preset changes the dictionary, language or capitalized-name choice. There are no process arguments, environment or Working Directory.",
            "These are tiptoptyp controls over harper-core, not harper-ls configuration. They do not read harper-ls user/workspace dictionaries automatically. Old diagnostics remain visible while a fresh check is pending.",
        ],
        sources: &[
            source!(
                "Harper rule catalogue",
                "https://writewithharper.com/docs/rules"
            ),
            source!(
                "Versioned Rust library",
                "https://docs.rs/harper-core/2.11.0/harper_core/"
            ),
        ],
    },
    Reference {
        id: Id::Liquid,
        title: "Liquid",
        reviewed_version: "lfm25-encoder-350m-onnxruntime-v1@4e2f724c10985e9ec90f34189c22be5d5f2e41e0; ONNX arm64 1.31.0 / x64 1.19.2; format 3",
        summary: "Manual grammar correction of selected prose using the optional native ONNX model. Suggestions are reviewed as a diff before applying to the unchanged selection.",
        examples: &[
            example!(
                "Settings JSON fragment",
                r#"{"liquid_threads":2,"liquid_cache_directory":"","liquid_input":{"protected_content":"placeholder","placeholder":"x","formatted_text":"include"}}"#,
                "json",
                "Two CPU threads, app-managed cache, math represented by protected placeholders, and prose inside formatting included. Empty cache directory means the app data folder."
            ),
            example!(
                "Split instead of placeholder",
                r#"{"liquid_input":{"protected_content":"split","placeholder":"x","formatted_text":"skip"}}"#,
                "json",
                "Split around protected content and omit formatted text. Short fragments can lose sentence context; compare corrections before applying."
            ),
        ],
        notes: &[
            "The native helper has no editable shell arguments, environment or Working Directory. A custom cache folder stores the pinned assets; it is not a loader for arbitrary models. First use downloads matching native assets; later checks can run offline.",
            "Long input is sentence-chunked where possible; suggestions arrive incrementally. Threshold 0.5 and at most three passes are currently fixed, not Settings options. Verbose is unavailable; it must not change correction precision or auto-apply suggestions.",
        ],
        sources: &[
            source!(
                "Pinned model card",
                "https://huggingface.co/LiquidAI/LFM2.5-Encoder-350M-Spellchecker/blob/4e2f724c10985e9ec90f34189c22be5d5f2e41e0/README.md"
            ),
            source!(
                "ONNX Runtime threading",
                "https://onnxruntime.ai/docs/performance/tune-performance/threading.html"
            ),
        ],
    },
    Reference {
        id: Id::TexEngine,
        title: "pdfLaTeX / XeLaTeX / LuaLaTeX",
        reviewed_version: "TeX Live 2025: pdfTeX 1.40.27 / XeTeX 0.999997 / LuaHBTeX 1.21.0",
        summary: "External TeX engines come from your TeX distribution. The chosen engine runs directly or under finite latexmk orchestration.",
        examples: &[
            example!(
                "Optional engine Arguments",
                "-recorder",
                "text",
                "Write a dependency recorder file in private build storage. Do not add the executable or input filename."
            ),
            example!(
                "Engine Environment",
                r#"{"TEXINPUTS":"/Users/me/texmf//:","BIBINPUTS":"/Users/me/bib//:"}"#,
                "json",
                "On macOS/Unix, // searches subdirectories and the trailing colon retains default TeX search paths. This environment also reaches latexmk and its children."
            ),
        ],
        notes: &[
            "The app supplies nonstop mode, halt-on-error, file/line diagnostics, disabled shell escape, private output paths and optional SyncTeX. Normal hides underfull-box warnings; Verbose shows them, sharing this display preference with Tectonic. Both preserve custom engine Arguments and logging; do not add unsupported engine --verbose flags.",
            "External versions are unverified here. pdfLaTeX and Unicode engines have different font/package needs. Bibliography orchestration requires latexmk; direct engine passes are not a substitute for BibTeX/Biber.",
            PROCESS,
        ],
        sources: &[
            source!(
                "TeX engine options",
                "https://tug.org/texinfohtml/web2c.html#tex-invocation"
            ),
            source!(
                "Kpathsea search paths",
                "https://tug.org/texinfohtml/kpathsea.html#Path-expansion"
            ),
        ],
    },
    Reference {
        id: Id::Latexmk,
        title: "latexmk",
        reviewed_version: "4.86a (TeX Live 2025)",
        summary: "Runs finite TeX passes and chooses BibTeX/Biber as required. The app owns the selected engine and private build directory.",
        examples: &[
            example!(
                "Normal Arguments",
                "-quiet -bibtex-cond",
                "text",
                "Reduce progress output and run BibTeX when the bibliography is available."
            ),
            example!(
                "Optional build progress",
                "-verbose -bibtex-cond",
                "text",
                "Restore subprocess progress in build logs. This is a manual logging customization, not additional Problems checking; there is no diagnostic Verbose preset for latexmk."
            ),
        ],
        notes: &[
            "Keep the Build bibliography toggle authoritative. The app disables preview/watch, limits repeat passes, ignores automatic latexmk rc files and supplies engine commands/output paths. Do not add -pvc, viewer/cleanup flags, -r, -e or replacement output/engine choices.",
            "This system tool has no separate Environment or Working Directory boxes; it inherits the TeX engine environment and document directory. Unknown installed versions remain unverified. Problems presets never rewrite latexmk Arguments or bibliography orchestration.",
        ],
        sources: &[source!(
            "Official latexmk manual",
            "https://www.cantab.net/users/johncollins/latexmk/latexmk.txt"
        )],
    },
    Reference {
        id: Id::Bibtex,
        title: "BibTeX",
        reviewed_version: "0.99d (TeX Live 2025)",
        summary: "Processes classic BibTeX bibliographies when latexmk requests them. The document's bibliography commands choose its data and style.",
        examples: &[
            example!(
                "Normal Arguments",
                "-terse",
                "text",
                "Hide progress reports while retaining errors. Removing -terse manually restores progress, not extra Problems checks. BibTeX has no diagnostic Verbose preset."
            ),
            example!(
                "Optional Arguments",
                "-min-crossrefs=2",
                "text",
                "Include a cross-referenced entry after this number of citations; 2 is the upstream default."
            ),
        ],
        notes: &[
            "For console progress, manually remove latexmk -quiet as well: it forces BibTeX -terse. Problems presets never alter these Arguments. Do not supply .aux/.bib inputs or output names: latexmk supplies the job. Keep Build bibliography enabled for orchestration. BibLaTeX documents may require Biber instead.",
            "BibTeX inherits TeX engine Environment, including BIBINPUTS/BSTINPUTS. There is no independent environment/directory control for this system tool. Version shown is the reviewed local version, not a detected runtime guarantee.",
        ],
        sources: &[source!(
            "Official BibTeX documentation",
            "https://ctan.org/pkg/bibtex"
        )],
    },
    Reference {
        id: Id::Biber,
        title: "Biber",
        reviewed_version: "2.20 (TeX Live 2025)",
        summary: "BibLaTeX's bibliography processor. latexmk invokes it when the document requires it; keep Biber compatible with your installed BibLaTeX.",
        examples: &[
            example!(
                "Optional process logging",
                "--debug",
                "text",
                "Add detail to the build's .blg log without changing bibliography selection. This is manual process logging, not a Problems-diagnostic preset."
            ),
            example!(
                "Optional validation",
                "--validate-datamodel",
                "text",
                "Report data-model issues in bibliography entries. This is extra checking, not a default requirement."
            ),
        ],
        notes: &[
            "Do not add input/output filenames, output-directory flags or --tool: the app and latexmk own the job. The TeX engine environment is inherited; there is no separate environment/directory box here.",
            "latexmk -quiet adds --onlylog, suppressing Biber console output; adjust latexmk Arguments manually for console progress. Biber --debug adds .blg detail but does not implement a Problems preset. Clear Biber cache is a separate compiler-options action; diagnostic presets never clear caches or change bibliography Arguments. The installed system version remains unverified.",
        ],
        sources: &[source!(
            "Versioned Biber manual source",
            "https://github.com/plk/biber/blob/v2.20/doc/biber.tex"
        )],
    },
    Reference {
        id: Id::Latexdiff,
        title: "LaTeXdiff",
        reviewed_version: "1.3.4",
        summary: "Creates a marked-up .tex comparison through the cancellable wizard. Older/newer sources remain untouched; choose a new output file.",
        examples: &[
            example!(
                "Optional Arguments",
                "--math-markup=whole --append-textcmd=mycomment",
                "text",
                "Treat each changed formula as a unit and recognize a custom prose macro. Command names omit the leading backslash."
            ),
            example!(
                "Input encoding",
                "--encoding=utf8",
                "text",
                "Read the selected files as UTF-8. LaTeXdiff generates a comparison rather than extra Problems diagnostics; there is no diagnostic Verbose preset."
            ),
            example!(
                "Environment",
                r#"{"LC_ALL":"C"}"#,
                "json",
                "A literal locale override if needed. Input encoding is controlled separately, for example --encoding=utf8."
            ),
        ],
        notes: &[
            "Select flattening and markup in the wizard rather than duplicating --flatten/--type in Arguments. Do not paste old.tex new.tex > diff.tex: there is no shell redirection; the app captures stdout and safely publishes a new file.",
            "The selected newer file's unsaved source can be snapshotted. Cancellation stops the worker job. External LaTeXdiff versions remain unverified; Problems presets never change Arguments or either input.",
            PROCESS,
        ],
        sources: &[source!(
            "Versioned options and implementation",
            "https://github.com/ftilmann/latexdiff/blob/1.3.4/latexdiff"
        )],
    },
    Reference {
        id: Id::Texdoc,
        title: "TeXdoc",
        reviewed_version: "4.1.1 (TeX Live 2025)",
        summary: "Searches locally installed TeX manuals from File or the command palette and opens the chosen PDF in a document tab.",
        examples: &[
            example!(
                "Command-palette query",
                "amsmath",
                "text",
                "Enter the package/class/topic, not a command or URL. The app supplies list, noninteractive and machine-readable flags."
            ),
            example!(
                "User configuration",
                "lang = en",
                "text",
                "A texdoc.cnf preference for manual language. Run texdoc --files in a terminal to locate your distribution's configuration paths."
            ),
            example!(
                "Terminal diagnosis only",
                "texdoc --debug --list --nointeract --machine amsmath",
                "text",
                "Inspect manual discovery outside the app. There is no in-app Texdoc arguments/environment box or verbose preset."
            ),
        ],
        notes: &[
            "The app handles PDF opening; do not add a viewer command. Installed manuals work offline. Online manual is an explicit browser fallback and requires a connection.",
            "Texdoc is a system utility; the reviewed version does not establish your running version. texdoc.cnf choices are distribution preferences, not app Settings JSON.",
        ],
        sources: &[
            source!(
                "Official Texdoc manual",
                "https://texdoc.org/serve/texdoc/0"
            ),
            source!("Official project", "https://tug.org/texdoc/"),
        ],
    },
];
