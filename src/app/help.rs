//! Help stays in the Settings viewport and reflects effective shortcut bindings.
use crate::{
    settings::AppSettings,
    shortcuts::{ShortcutAction as A, ShortcutBindings},
};
use eframe::egui::{self, RichText};

mod tool_options;

pub(super) fn show(ui: &mut egui::Ui, settings: &AppSettings) {
    let bindings = ShortcutBindings::current(&settings.shortcut_overrides);
    let scroll = egui::ScrollArea::vertical().id_salt("settings-help-scroll").show(ui, |ui| {
        ui.heading("Help & tips");
        ui.label("Shortcuts below reflect your custom bindings. Preview shortcuts require preview focus; its border shows focus. Escape returns to the editor.");
        tool_options::show(ui, settings);
        ui.heading("10 useful shortcuts");
        shortcut_table(ui, "help-main-shortcuts", &bindings, &[A::Open, A::Save, A::Find, A::FindReplace, A::Undo, A::Redo, A::Compile, A::Format, A::ToggleComment, A::ToggleLineWrap]);
        ui.heading("Preview shortcuts");
        ui.label("The engine gear refreshes the live preview. PDF (Compile PDF shortcut) creates a file beside the previewed source; Export PDF lets you choose another destination. A green check confirms a save in this window, yellow means the PDF needs updating, and red means creation failed. Preview loading and private PDF caches do not save that file.");
        shortcut_table(ui, "help-preview-shortcuts", &bindings, &[A::PreviewPreviousPage, A::PreviewNextPage, A::PreviewFitWidth, A::PreviewZoomIn, A::PreviewZoomOut, A::PreviewZoomReset, A::SyncPreview, A::ComfyPreview]);
        ui.monospace("t   Switch Tinymist preview appearance (preview focused)\nEscape   Return focus to source");
        ui.label("Tinymist also accepts Cmd/Ctrl with + / − / 0 for zoom while focused. Use the floating preview controls for Find, Outline and Back/Forward after internal links.");
        ui.label("Typst defaults to Tinymist partial (experimental), keeping nearby pages ready and filling distant pages as you navigate. Select Tinymist (Typst only) for the complete renderer. This choice changes only the live viewer, not saved PDFs.");
        ui.label("On macOS, Smooth Tinymist zoom keeps a higher-resolution viewport image over the preview while it redraws. Toggle it in Settings → Preview; it works with both Tinymist viewers.");
        ui.heading("Leave part of your code untouched");
        ui.label("Directives apply to the named tool, not every service. Select your TeX formatter in Settings → Tools → TeX tools. Compiler errors cannot be disabled with formatter comments.");
        ui.label(RichText::new("Typst formatter (Typstyle through Tinymist)").strong());
        ui.monospace("// @typstyle off\n#let carefully_aligned = (a: 1, b: 2)");
        ui.label("Protects the next syntax node, such as a definition. It is not an off/on region toggle.");
        ui.hyperlink_to("Typstyle escape hatch", "https://typstyle-rs.github.io/typstyle/escape-hatch.html");
        ui.label(RichText::new("Badness: formatting only").strong());
        ui.monospace("% badness-format off\n% your hand-aligned code\n% badness-format on");
        ui.label("Use % badness-format skip before one construct, or skip-file for the entire file. Without -format, % badness off/on disables formatting and linting together.");
        ui.hyperlink_to("Badness formatting directives", "https://badness.dev/guide/formatting.html");
        ui.label(RichText::new("Badness: suppress one lint").strong());
        ui.monospace("% badness-lint skip redundant-script-braces\n$x^{2}$");
        ui.label("For document-wide preferences use Badness ignored diagnostic codes in Settings. TexLab has its own list; suppressing a provider's code does not affect other providers.");
        ui.hyperlink_to("Badness lint rules and suppressions", "https://badness.dev/reference/linter-rules.html");
        ui.label(RichText::new("tex-fmt").strong());
        ui.monospace("% tex-fmt: off\n% your hand-aligned code\n% tex-fmt: on\nOne preserved line % tex-fmt: skip");
        ui.hyperlink_to("tex-fmt directives", "https://github.com/WGUNDERWOOD/tex-fmt#disabling-the-formatter");
        ui.label("Harper and TexLab do not share these region directives. Harper already excludes math and technical arguments; turn writing checks off in Settings when needed. Tool-specific capabilities depend on your installed version.");
        ui.label("Writing checks wait briefly after typing, then reuse unchanged paragraphs. They still parse the whole source to track code and math boundaries, so sentences can continue across display equations. Old diagnostics remain visible until a fresh result arrives. Apostrophes stay part of possessives; name evidence from author lists and mathematical attributions applies throughout the document without turning off grammar checks.");
        ui.heading("Snippets");
        ui.label("Type a prefix such as frac inside an equation, choose the document-symbol suggestion, and press Enter. Suggestions appear with other completions; they never replace text automatically.");
        shortcut_table(ui, "help-snippet-shortcuts", &bindings, &[A::Complete]);
        ui.monospace("Tab / Shift+Tab   Next / previous editable field\nEscape            Finish linked editing\nUndo / Redo       Restore all linked copies together");
        ui.label("Try mk for inline math, dm for display math, frac / sqrt / sum / int / pmat / cases inside equations, and env / sec in text. Typst also has fig / table. TeX and Typst get their own syntax; auto-miTeX dollar math uses TeX snippets. Some block templates appear only at the start of a line; indentation is allowed. Comments and raw/verbatim text are excluded.");
        ui.label("Settings → Snippets lists every suggested and custom template, with search, filters and per-row enable, Edit and Delete actions. Edit opens that entry in its own resizable source window. Edit snippets opens the full editor for adding or restoring entries. Uncheck Enabled to disable a snippet temporarily. Duplicate copies an entry; Remove deletes it. Save snippets or Cmd/Ctrl+S applies changes; Revert changes restores the saved collection. Cmd/Ctrl+F searches this window; Cmd/Ctrl+W closes only the snippet editor and keeps its unsaved draft for reopening. Restore suggestions resets the suggested library while keeping your own entries. An enabled, eligible custom snippet overrides a suggestion with the same prefix.");
        ui.label("Write ordinary Typst or TeX. Both languages use the same placeholders, which are removed before insertion:");
        ui.monospace("{{1:x}}   Field 1 starts with x\n{{1}}     Repeat field 1\n{{2:y}}   Next field\n{{0}}     Final cursor position");
        ui.label("Fields are visited in numerical order. Repeating a number links its text. Backslashes, dollars and document commands remain literal. Set Opening delimiter and Closing delimiter per snippet if {{ and }} clash with your source. Rewrite markers converts existing fields; Keep source changes the delimiters without rewriting text.");
        ui.label("The TeX env template keeps its begin/end names in sync:");
        ui.monospace("\\begin{{{1:equation}}}\n  {{2:x = y}}\n\\end{{{1}}}\n{{0}}");
        ui.label("Multiline insertion keeps your line's indentation. Recognized TeX dependencies are imported automatically; Undo removes insertion and imports together. Keep fields flat: use separate numbered fields rather than nesting them. Custom templates do not execute Lua, regex triggers, transformations or choice menus.");
        ui.horizontal_wrapped(|ui| {
            ui.hyperlink_to("TeX LuaSnip examples", "https://github.com/iurimateus/luasnip-latex-snippets.nvim");
            ui.hyperlink_to("Typst LuaSnip examples", "https://github.com/pseudofractal/typst-notes.nvim/blob/main/snippets/typst.lua");
        });
        ui.heading("Try these");
        for tip in [
            "Right-click code and choose Reveal in Preview; right-click the preview and choose Reveal in Source. Typst uses Tinymist; TeX needs its compiled SyncTeX map.",
            "Appearance → Effects lets you choose separate source and preview jump cues: Rings, Pulse or Spotlight, with duration and size. Either cue can be turned off. Editor → Typing particles adds optional sparks while typing, with Low, Medium or High density.",
            "Cmd/Ctrl+Shift+P opens the command palette. Type a command, choose with Up/Down, then Enter; Escape cancels. The palette shows your current shortcut bindings.",
            "Select text, then type an opening bracket, quote or $ to surround it. Undo reverses it in one step.",
            "Layout in the title bar groups Code, Split, Preview, Explorer and the bottom panel. Their keyboard shortcuts still work, and toggling a panel keeps its chosen size. Turn off Appearance → Compact layout controls to restore the separate title-bar buttons.",
            "For literal Chinese and Japanese punctuation such as 。 and ！, leave Convert CJK punctuation to ASCII when typing off in Settings → Writing. This option changes committed keyboard input; pasted text is always preserved.",
            "Focus an Explorer subpanel and use Find to search just that panel. Its empty search bar scrolls away.",
            "Edit → LaTeXdiff compares two TeX files or the current editor against its saved version. Generate a marked-up .tex file, then Save and open to build its PDF. Options are also under TeX tools.",
            "Edit → Edit Table opens an editable grid. Variable cells display locked compiler samples; generated Typst tables open as a read-only sampled grid. Use Source for deliberate expression changes. TeX tables, math conversion, snippets and document templates add missing package imports to the preamble automatically; Undo restores both imports and inserted code.",
            "Draw a symbol in Explorer; choose TeX or Typst insertion preferences in Settings. TeX symbols automatically add their package imports.",
            "Use the Activity panel to see which services are stale, working or failing; hover an indicator for detail.",
            "Use the eye on a tab to choose the preview document independently of the tab you edit.",
            "Autocomplete symbols identify language-server, snippet and built-in suggestions. Font names appear in their own font. See Snippets above for editable templates and linked fields.",
            "Indentation guides, highlighting uses of local definitions and literal text highlights are under Editor. Their styling never changes saved source.",
            "Appearance includes a translucent background on macOS. Blur radius and Background opacity each have independent directions and Start/End values. The chosen edge uses End, the opposite edge uses Start; either value can be higher. Opacity 100% is opaque and 0% is fully transparent. Uniform uses Start everywhere. Fullscreen disables both effects. PDF page colors stay unchanged.",
            "Settings search covers every category. Choose a result to switch pages and highlight its control. Categories keep their scroll positions and drafts; narrow windows use a selector beside search. The {} title-bar button edits validated JSON, applied only after Save.",
        ] { ui.label(format!("• {tip}")); }
    });
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe_scroll(
        "settings.help",
        ui.ctx(),
        scroll.inner_rect,
        scroll.content_size.y,
        scroll.state.offset.y,
        false,
    );
    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
    let _ = scroll;
}
fn shortcut_table(ui: &mut egui::Ui, id: &str, bindings: &ShortcutBindings, actions: &[A]) {
    egui::Grid::new(id).striped(true).show(ui, |ui| {
        for &action in actions {
            ui.monospace(bindings.display(action).unwrap_or_else(|| "Unbound".into()));
            ui.label(action.label());
            ui.end_row();
        }
    });
}
