//! Help stays in the Settings viewport and reflects effective shortcut bindings.
use crate::{
    settings::AppSettings,
    shortcuts::{ShortcutAction as A, ShortcutBindings},
};
use eframe::egui::{self, RichText};

pub(super) fn show(ui: &mut egui::Ui, settings: &AppSettings) {
    let bindings = ShortcutBindings::current(&settings.shortcut_overrides);
    egui::ScrollArea::vertical().id_salt("settings-help-scroll").show(ui, |ui| {
        ui.heading("Help & tips");
        ui.label("Shortcuts below reflect your custom bindings. Preview shortcuts require preview focus; its border shows focus. Escape returns to the editor.");
        ui.heading("10 useful shortcuts");
        shortcut_table(ui, "help-main-shortcuts", &bindings, &[A::Open, A::Save, A::Find, A::FindReplace, A::Undo, A::Redo, A::Compile, A::Format, A::ToggleComment, A::ToggleLineWrap]);
        ui.heading("Preview shortcuts");
        shortcut_table(ui, "help-preview-shortcuts", &bindings, &[A::PreviewPreviousPage, A::PreviewNextPage, A::PreviewFitWidth, A::PreviewZoomIn, A::PreviewZoomOut, A::PreviewZoomReset, A::SyncPreview, A::ComfyPreview]);
        ui.monospace("t   Switch Tinymist preview appearance (preview focused)\nEscape   Return focus to source");
        ui.label("Tinymist also accepts Cmd/Ctrl with + / − / 0 for zoom while focused. Use the floating preview controls for Find, Outline and Back/Forward after internal links.");
        ui.heading("Leave part of your code untouched");
        ui.label("Directives apply to the named tool, not every service. Select your TeX formatter in Settings → TeX. Compiler errors cannot be disabled with formatter comments.");
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
        ui.heading("Try these");
        for tip in [
            "Select text, then type an opening bracket, quote or $ to surround it. Undo reverses it in one step.",
            "Focus an Explorer subpanel and use Find to search just that panel. Its empty search bar scrolls away.",
            "Draw a symbol in Explorer; choose TeX or Typst insertion preferences in Settings.",
            "Use the Activity panel to see which services are stale, working or failing; hover an indicator for detail.",
            "Use the eye on a tab to choose the preview document independently of the tab you edit.",
            "Customize templates and snippets in Settings. Tab advances snippet placeholders.",
            "Indentation guides, definition highlighting and literal text highlights are under Editor. Their styling never changes saved source.",
            "The {} title-bar button edits all settings as validated JSON. Changes apply only after Save.",
        ] { ui.label(format!("• {tip}")); }
    });
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
