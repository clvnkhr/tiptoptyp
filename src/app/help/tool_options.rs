//! The Settings help reference is compiled in; no guide file or network is needed.
use crate::{settings::AppSettings, toolchain::ToolKind};
use eframe::egui::{self, RichText};

struct ToolOption {
    example: &'static str,
    purpose: &'static str,
}
struct ToolHelp {
    kind: ToolKind,
    summary: &'static str,
    options: [ToolOption; 10],
    notes: &'static [&'static str],
    links: &'static [(&'static str, &'static str)],
}

pub(super) fn show(ui: &mut egui::Ui, settings: &AppSettings) {
    let section = egui::CollapsingHeader::new("Tool options")
        .id_salt("help-tool-options")
        .show(ui, |ui| {
            ui.label("Reference opens a local summary with its reviewed version. A warning marks a bundled version bump that still needs review. Online sources remain optional. Normal keeps the current diagnostic filters; Verbose shows more warnings and pedantic advice in Problems. The Problems filter controls can switch all providers together; individual tools can also be set separately. Presets change the visible diagnostic settings and preserve command arguments, executables, environment, dictionaries and language. Manual diagnostic changes are kept until you choose a preset.");
            ui.label("Ten useful choices for each bundled tool. These are project preferences, not ten options everyone should enable. Arguments shows real defaults: append optional flags there; the app supplies document paths and runtime options separately.");
            ui.label("Arguments uses shell-style quoting, without shell execution or variable expansion. Environment is a JSON object of strings. Working Directory is blank or an existing absolute directory. Server configuration JSON and project TOML are separate from process arguments.");
            for tool in &TOOLS {
                let section = egui::CollapsingHeader::new(format!("{} options", tool.kind.label()))
                    .id_salt(("help-tool", tool.kind.binary_name()))
                    .show(ui, |ui| show_tool(ui, settings, tool));
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                if ui.clip_rect().contains_rect(section.header_response.rect) {
                    crate::desktop_test::observe(
                        &format!("settings.help.tool.{}", tool.kind.binary_name()),
                        &section.header_response,
                    );
                }
                #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
                let _ = section;
            }
            ui.label("Harper and PDFium are linked libraries; use their app settings rather than command arguments.");
        });
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if ui.clip_rect().contains_rect(section.header_response.rect) {
        crate::desktop_test::observe("settings.help.tools", &section.header_response);
    }
    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
    let _ = section;
}

fn show_tool(ui: &mut egui::Ui, settings: &AppSettings, tool: &ToolHelp) {
    ui.label(format!(
        "Bundled {} {}",
        tool.kind.label(),
        tool.kind.bundled_version()
    ));
    ui.label(tool.summary);
    let arguments = match tool.kind {
        ToolKind::Typst => &settings.typst.command.arguments,
        ToolKind::Tinymist => &settings.tinymist.command.arguments,
        ToolKind::Tectonic => &settings.tex.tectonic.command.arguments,
        ToolKind::Texlab => &settings.tex.texlab.command.arguments,
        ToolKind::Badness => &settings.tex.badness.command.arguments,
        ToolKind::TexFmt => &settings.tex.tex_fmt.command.arguments,
    };
    ui.label(RichText::new("Current Arguments").strong());
    ui.add(
        egui::Label::new(
            RichText::new(if arguments.is_empty() {
                "(empty)"
            } else {
                arguments
            })
            .monospace(),
        )
        .wrap(),
    );
    for (index, option) in tool.options.iter().enumerate() {
        ui.separator();
        let example = ui.add(
            egui::Label::new(
                RichText::new(format!("{}. {}", index + 1, option.example)).monospace(),
            )
            .wrap(),
        );
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        if ui.clip_rect().contains_rect(example.rect) {
            crate::desktop_test::observe(
                &format!("settings.help.option.{}.{}", tool.kind.binary_name(), index),
                &example,
            );
        }
        #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
        let _ = example;
        ui.label(option.purpose);
    }
    for note in tool.notes {
        ui.label(*note);
    }
    ui.horizontal_wrapped(|ui| {
        for &(label, url) in tool.links {
            super::super::settings_controls::documentation_link(ui, label, url);
        }
    });
}

const TOOLS: [ToolHelp; 6] = [
    ToolHelp {
        kind: ToolKind::Typst,
        summary: "PDF export uses watch, compact diagnostics, local font discovery and a private source/PDF pair.",
        options: [
            ToolOption {
                example: "Arguments: --input mode=draft",
                purpose: "Template variables; none supplied by default. The document must read them.",
            },
            ToolOption {
                example: "Arguments: --font-path /Users/me/fonts",
                purpose: "Additional fonts; workspace fonts are already detected.",
            },
            ToolOption {
                example: "Arguments: --ignore-system-fonts",
                purpose: "Reproducible font selection; system fonts normally enabled. Can change layout.",
            },
            ToolOption {
                example: "Arguments: --package-path /Users/me/packages",
                purpose: "Private/local package collection; default is platform storage.",
            },
            ToolOption {
                example: "Arguments: --package-cache-path /Users/me/typst-cache",
                purpose: "Downloaded-package cache location; default is platform storage.",
            },
            ToolOption {
                example: "Environment: {\"SOURCE_DATE_EPOCH\":\"1767225600\"}",
                purpose: "Reproducible creation date; normally current time. Choose the intended timestamp.",
            },
            ToolOption {
                example: "Arguments: --pdf-standard a-2u",
                purpose: "Archival output; default PDF 1.7. Conformance can reject content.",
            },
            ToolOption {
                example: "Arguments: --jobs 2",
                purpose: "Reduce CPU contention; default uses available CPUs. May compile slower.",
            },
            ToolOption {
                example: "Arguments: --timings /tmp/typst-timings.json",
                purpose: "Compilation profiling; normally off. Writes an extra file.",
            },
            ToolOption {
                example: "Arguments: --deps /tmp/typst-deps.json",
                purpose: "Inspect imports/assets used; normally off. Writes an extra file.",
            },
        ],
        notes: &[
            "Keep PDF output and accessibility tags. --no-pdf-tags reduces accessibility; --pages produces a partial document.",
            "Tinymist preview and Typst PDF export use separate compilers. Match inputs, fonts and package choices in both tools.",
        ],
        links: &[
            (
                "CLI options",
                "https://github.com/typst/typst/blob/v0.15.1/crates/typst-cli/src/args.rs",
            ),
            (
                "Template inputs",
                "https://typst.app/docs/reference/foundations/sys/#definitions-inputs",
            ),
            ("PDF choices", "https://typst.app/docs/reference/pdf/"),
        ],
    },
    ToolHelp {
        kind: ToolKind::Tinymist,
        summary: "Interactive Typst preview and editor intelligence use the lsp process. Formatter, input, lint and export choices belong in Tinymist configuration JSON.",
        options: [
            ToolOption {
                example: "Arguments: --font-path /Users/me/fonts",
                purpose: "Additional fonts; workspace fonts are already detected.",
            },
            ToolOption {
                example: "Configuration JSON: \"systemFonts\": false",
                purpose: "Reproducible fonts; normally enabled. Supply required fonts explicitly.",
            },
            ToolOption {
                example: "Configuration JSON: \"typstExtraArgs\": [\"--input=mode=draft\"]",
                purpose: "Template variables; none by default. The app retains its designated entry.",
            },
            ToolOption {
                example: "Configuration JSON: \"formatterMode\": \"typstyle\"",
                purpose: "Default formatter; alternatives are \"typstfmt\" and \"disable\".",
            },
            ToolOption {
                example: "Configuration JSON: \"formatterPrintWidth\": 100",
                purpose: "Narrower formatting; editable default is 120.",
            },
            ToolOption {
                example: "Configuration JSON: \"formatterIndentSize\": 2",
                purpose: "Formatter indentation; editable default is 2 spaces.",
            },
            ToolOption {
                example: "Configuration JSON: \"formatterProseWrap\": true",
                purpose: "Reflow prose; editable default false preserves authored wrapping.",
            },
            ToolOption {
                example: "Configuration JSON: \"lint\": {\"enabled\": true, \"when\": \"onType\"}",
                purpose: "Additional linting; disabled by default. Adds work/warnings.",
            },
            ToolOption {
                example: "Configuration JSON: \"exportPdf\": \"never\"",
                purpose: "Default keeps app-controlled PDF writes; automatic export can create additional files.",
            },
            ToolOption {
                example: "Environment: {\"TINYMIST_LOG\":\"warn\"}",
                purpose: "Quieter logs; normal LSP level is INFO. Less troubleshooting detail.",
            },
        ],
        notes: &[
            "JSON examples go directly in the object. Merge entries; do not wrap them in a tinymist object. Tinymist 0.15.2 embeds a Typst 0.15.0 fork, separate from the Typst exporter.",
            "For this bundled version, formatterProseWrap is boolean. typstExtraArgs accepts --input options only; use --input=key=value. The app owns preview transport, scheduling, compile status and document navigation.",
            "Lint/export policies are never, onSave and onType. The script policy is unfinished. Do not pass preview/compile flags to lsp.",
            "Inputs, fonts and packages must match the separate Typst exporter when saving the same document as PDF.",
        ],
        links: &[
            (
                "Configuration schema",
                "https://github.com/Myriad-Dreamin/tinymist/blob/v0.15.2/crates/tinymist/src/config.rs",
            ),
            (
                "Compiler settings",
                "https://myriad-dreamin.github.io/tinymist/feature/compiler-settings.html",
            ),
            (
                "Logging",
                "https://myriad-dreamin.github.io/tinymist/feature/cli.html#logging",
            ),
            (
                "Lint policy",
                "https://myriad-dreamin.github.io/tinymist/feature/linting.html",
            ),
        ],
    },
    ToolHelp {
        kind: ToolKind::Tectonic,
        summary: "TeX builds use -X compile, generate a PDF in private storage and export it beside the source.",
        options: [
            ToolOption {
                example: "Settings: SyncTeX; CLI --synctex",
                purpose: "Source/PDF navigation; on by default. Small extra artifact.",
            },
            ToolOption {
                example: "Arguments: --keep-logs",
                purpose: "Preserve detailed engine logs; already on.",
            },
            ToolOption {
                example: "Arguments: --keep-intermediates",
                purpose: "Retain reference/bibliography artifacts in private storage; already on.",
            },
            ToolOption {
                example: "Arguments: --untrusted",
                purpose: "Restrict unsafe TeX features; already on.",
            },
            ToolOption {
                example: "Settings: Only cached resources; CLI --only-cached",
                purpose: "Offline builds; default off. Uncached resources cause failure.",
            },
            ToolOption {
                example: "Arguments: --bundle /Users/me/tex-bundle.tar",
                purpose: "Pin resources; no custom bundle by default. Must suit the project.",
            },
            ToolOption {
                example: "Environment: {\"TECTONIC_CACHE_DIR\":\"/Users/me/tectonic-cache\"}",
                purpose: "Resource/format cache location; default is platform storage.",
            },
            ToolOption {
                example: "Environment: {\"SOURCE_DATE_EPOCH\":\"1767225600\"}",
                purpose: "Reproducible dates; optional. Changes timestamp semantics.",
            },
            ToolOption {
                example: "Arguments: --print",
                purpose: "Full engine chatter; normally off. More output.",
            },
            ToolOption {
                example: "Arguments: --reruns 2",
                purpose: "Force additional passes; normally automatic convergence. May waste work or miss required passes.",
            },
        ],
        notes: &[
            "The app supplies the input and private --outdir. SyncTeX and offline-resource flags follow their dedicated Settings toggles.",
            "Default root options --chatter minimal --color never precede compile. They reduce progress output while retaining warning/error logs. Underfull box warnings stay in raw output; enable Show underfull box warnings to also list them in Problems.",
            "Tectonic.toml belongs to the separate project build command, not this standalone compilation. -Z deterministic-mode breaks SyncTeX. Search-path unstable flags are ineffective in untrusted mode.",
        ],
        links: &[
            (
                "Compile options",
                "https://tectonic-typesetting.github.io/book/latest/v2cli/compile.html",
            ),
            (
                "Resource cache",
                "https://tectonic-typesetting.github.io/book/latest/getting-started/first-document.html#cache",
            ),
        ],
    },
    ToolHelp {
        kind: ToolKind::Texlab,
        summary: "TeX editor intelligence uses a stdio language server. Put these JSON entries inside the texlab object in TexLab configuration JSON.",
        options: [
            ToolOption {
                example: "\"build\": {\"onSave\": false, \"forwardSearchAfter\": false}",
                purpose: "Default avoids duplicate builds; app owns compilation.",
            },
            ToolOption {
                example: "\"chktex\": {\"onEdit\": false, \"onOpenAndSave\": false}",
                purpose: "Default avoids duplicate linting; Badness supplies it.",
            },
            ToolOption {
                example: "\"latexFormatter\": \"none\"",
                purpose: "Default leaves formatting to the selected app formatter.",
            },
            ToolOption {
                example: "\"hover\": {\"symbols\": \"glyph\"}",
                purpose: "Default compact symbol hovers.",
            },
            ToolOption {
                example: "\"completion\": {\"matcher\": \"fuzzy-ignore-case\"}",
                purpose: "Explicit upstream default; forgiving name completion.",
            },
            ToolOption {
                example: "\"diagnosticsDelay\": 300",
                purpose: "Explicit upstream default in milliseconds; lower values add work.",
            },
            ToolOption {
                example: "\"diagnostics\": {\"ignoredPatterns\": [\"^Unused label$\", \"^Unused entry$\"]}",
                purpose: "Defaults hide unused labels and unused bibliography entries. Keep undefined references and citations visible.",
            },
            ToolOption {
                example: "\"experimental\": {\"mathEnvironments\": [\"myalign\"]}",
                purpose: "Recognize custom math environments; no custom names by default.",
            },
            ToolOption {
                example: "\"experimental\": {\"verbatimEnvironments\": [\"mycode\"]}",
                purpose: "Protect custom literal/code environments; no custom names by default.",
            },
            ToolOption {
                example: "\"experimental\": {\"labelReferenceCommands\": [\"myref\"]}",
                purpose: "Recognize custom references; no custom names by default.",
            },
        ],
        notes: &[
            "Merge the experimental examples into one object. The app owns builds and the selected formatter; leave duplicate build/checker roles disabled unless intended.",
            "For process debugging, Arguments can be -vv --log-file /tmp/texlab.log. The log is truncated on startup. Do not use --quiet by default.",
            "rootDirectory is absent from the bundled schema. Symbol/inlay-hint settings cannot add UI features the app does not request.",
        ],
        links: &[
            (
                "Configuration reference",
                "https://github.com/latex-lsp/texlab/wiki/Configuration",
            ),
            (
                "Bundled schema",
                "https://github.com/latex-lsp/texlab/blob/v5.26.0/crates/texlab/src/server/options.rs",
            ),
            (
                "CLI logging",
                "https://github.com/latex-lsp/texlab/blob/v5.26.0/crates/texlab/src/main.rs",
            ),
        ],
    },
    ToolHelp {
        kind: ToolKind::Badness,
        summary: "TeX formatting and linting use badness lsp. Most preferences belong in project badness.toml; the server JSON accepts a small subset.",
        options: [
            ToolOption {
                example: "Configuration JSON: \"lineWidth\": 100; TOML [format]: line-width = 100",
                purpose: "Wider formula/prose lines; default 80. Project configuration wins.",
            },
            ToolOption {
                example: "Configuration JSON: \"indentWidth\": 4; TOML [format]: indent-width = 4",
                purpose: "Four-space formatting; default 2. Project preference.",
            },
            ToolOption {
                example: "TOML [format]: wrap = \"stable\"",
                purpose: "Reduce reflow churn; default \"reflow\".",
            },
            ToolOption {
                example: "TOML [format]: math-wrap = \"preserve\"",
                purpose: "Preserve display-formula breaks; default \"auto\". Alignment grids unaffected.",
            },
            ToolOption {
                example: "TOML [lint]: ignore = [\"redundant-script-braces\"]",
                purpose: "Project lint selection. The app separately hides twelve advisory rules by default; edit Badness Ignored diagnostic codes to restore them.",
            },
            ToolOption {
                example: "Configuration JSON: \"texmf\": {\"roots\": [\"/Users/me/texmf\"]}",
                purpose: "Additional package trees; none by default. Machine-specific paths.",
            },
            ToolOption {
                example: "TOML [commands.eqrefs]: like = \"cref\"",
                purpose: "Recognize a custom reference macro; none by default. Match real semantics.",
            },
            ToolOption {
                example: "TOML [environments.mycode]: like = \"lstlisting\"",
                purpose: "Protect custom code bodies; none by default. Use only for literal content.",
            },
            ToolOption {
                example: "TOML [format.no-break-abbreviations]: default = [\"Prop.\", \"Thm.\"]",
                purpose: "Avoid sentence breaks after abbreviations; normally built-in English list.",
            },
            ToolOption {
                example: "TOML top level: extend-exclude = [\"build/\", \"vendor/\"]",
                purpose: "Exclude sibling discovery; default adds nothing beyond built-in exclusions. Open files still processed.",
            },
        ],
        notes: &[
            "JSON defaults are {\"lineWidth\":80,\"indentWidth\":2}. Discovered project/TOML configuration wins over these fallback values.",
            "For TOML examples, use the indicated section and put top-level entries before all section headers. Abbreviation additions affect sentence/semantic wrapping.",
            "Badness Ignored diagnostic codes hides a rule in this app. TOML [lint] suppression changes the provider itself.",
            "Environment can choose a fallback file: {\"BADNESS_CONFIG\":\"/Users/me/paper/badness.toml\"}. A discovered project file wins; LSP discovery starts from each document parent. Changing Working Directory does not choose a project file. --config and --no-config do not affect this LSP command.",
        ],
        links: &[
            (
                "Configuration",
                "https://badness.dev/reference/configuration.html",
            ),
            (
                "Package discovery",
                "https://badness.dev/guide/editor-setup.html#texmf-discovery",
            ),
            (
                "Bundled implementation",
                "https://github.com/jolars/badness/blob/v0.24.0/src/config.rs",
            ),
        ],
    },
    ToolHelp {
        kind: ToolKind::TexFmt,
        summary: "Select tex-fmt as the TeX formatter first. --stdin --quiet keeps formatting output separate from chatter; configuration discovery starts in the document directory.",
        options: [
            ToolOption {
                example: "Arguments: --nowrap",
                purpose: "Preserve long authored lines; wrapping normally enabled.",
            },
            ToolOption {
                example: "Arguments: --wraplen 100",
                purpose: "Wrap at 100 columns; default 80.",
            },
            ToolOption {
                example: "Arguments: --tabsize 4",
                purpose: "Four-space indentation; default 2.",
            },
            ToolOption {
                example: "Arguments: --format-tables",
                purpose: "Align table ampersands; default off. Inspect complex/custom tables.",
            },
            ToolOption {
                example: "Arguments: --usetabs",
                purpose: "Tab indentation; default spaces. Project preference.",
            },
            ToolOption {
                example: "Arguments: --config /Users/me/paper/tex-fmt.toml",
                purpose: "Explicit existing configuration; normally directory/git-root/user discovery.",
            },
            ToolOption {
                example: "tex-fmt.toml: wrapmin = 90",
                purpose: "Minimum wrapping preference; normally wraplen - 10. Keep below/equal to wrap length.",
            },
            ToolOption {
                example: "tex-fmt.toml: lists = [\"myitemize\"]",
                purpose: "Add custom list indentation; none by default. Extends built-ins.",
            },
            ToolOption {
                example: "tex-fmt.toml: verbatims = [\"mycode\"]",
                purpose: "Preserve custom literal bodies; none by default. Extends built-ins.",
            },
            ToolOption {
                example: "tex-fmt.toml: no-indent-envs = [\"mydocument\"]",
                purpose: "Avoid indentation in a custom outer environment; none by default. Built-in document already covered.",
            },
        ],
        notes: &[
            "The last four choices are tex-fmt.toml entries, not CLI flags. Lists, verbatims and no-indent-envs extend the built-in names.",
            "Working Directory changes where this formatter discovers configuration. Choose wrapping and indentation to match your project.",
        ],
        links: &[
            (
                "CLI options",
                "https://github.com/WGUNDERWOOD/tex-fmt/blob/v0.5.7/README.md#command-line-options",
            ),
            (
                "Configuration file",
                "https://github.com/WGUNDERWOOD/tex-fmt/blob/v0.5.7/README.md#configuration-file-options",
            ),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

    fn harness(settings: AppSettings) -> Harness<'static, AppSettings> {
        let mut harness = Harness::builder()
            .with_size(egui::Vec2::new(800.0, 3000.0))
            .build_ui_state(
                |ui, settings: &mut AppSettings| show(ui, settings),
                settings,
            );
        harness.run();
        harness
    }

    #[test]
    fn options_stay_compact_until_opened_and_do_not_edit_settings() {
        let defaults = AppSettings::default();
        let mut harness = harness(defaults.clone());
        assert!(harness.query_by_label("Typst options").is_none());
        harness.get_by_label("Tool options").click();
        harness.run();
        for (name, first, last) in [
            (
                "Typst",
                "1. Arguments: --input mode=draft",
                "10. Arguments: --deps /tmp/typst-deps.json",
            ),
            (
                "Tinymist",
                "1. Arguments: --font-path /Users/me/fonts",
                "10. Environment: {\"TINYMIST_LOG\":\"warn\"}",
            ),
            (
                "Tectonic",
                "1. Settings: SyncTeX; CLI --synctex",
                "10. Arguments: --reruns 2",
            ),
            (
                "TexLab",
                "1. \"build\": {\"onSave\": false, \"forwardSearchAfter\": false}",
                "10. \"experimental\": {\"labelReferenceCommands\": [\"myref\"]}",
            ),
            (
                "Badness",
                "1. Configuration JSON: \"lineWidth\": 100; TOML [format]: line-width = 100",
                "10. TOML top level: extend-exclude = [\"build/\", \"vendor/\"]",
            ),
            (
                "tex-fmt",
                "1. Arguments: --nowrap",
                "10. tex-fmt.toml: no-indent-envs = [\"mydocument\"]",
            ),
        ] {
            let label = format!("{name} options");
            assert!(harness.query_by_label(first).is_none());
            harness.get_by_label(&label).click();
            harness.run();
            harness.get_by_label(first);
            harness.get_by_label(last);
            harness.get_by_label("Current Arguments");
            harness.get_by_label(&label).click();
            harness.run();
            assert!(harness.query_by_label(first).is_none());
        }
        assert_eq!(harness.state(), &defaults);
        harness.get_by_label("Tool options").click();
        harness.run();
        assert!(harness.query_by_label("tex-fmt options").is_none());
    }

    #[test]
    fn help_reflects_custom_arguments_and_its_links_open_the_specific_reference() {
        let mut settings = AppSettings::default();
        settings.tinymist.command.arguments = "lsp --font-path /Users/me/fonts".into();
        let mut harness = harness(settings.clone());
        harness.get_by_label("Tool options").click();
        harness.run();
        harness.get_by_label("Tinymist options").click();
        harness.run();
        harness.get_by_label("lsp --font-path /Users/me/fonts");
        harness.get_by_label("Configuration schema").click();
        harness.step();
        let id = egui::Id::new(("settings-documentation", harness.ctx.viewport_id()));
        assert_eq!(
            harness
                .ctx
                .data_mut(|data| data.remove_temp::<String>(id))
                .as_deref(),
            Some(
                "https://github.com/Myriad-Dreamin/tinymist/blob/v0.15.2/crates/tinymist/src/config.rs"
            )
        );
        assert!(
            harness.output().platform_output.commands.is_empty(),
            "Help links must use the app's background launcher, not eframe's disabled native links"
        );
        assert_eq!(harness.state(), &settings);
    }
}
