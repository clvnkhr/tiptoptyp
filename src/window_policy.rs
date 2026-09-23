//! Native surface roles. Document windows never inherit popup alpha or controls.
use eframe::egui;

pub(crate) fn document() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_transparent(false)
        .with_decorations(true)
        .with_has_shadow(true)
        .with_close_button(true)
        .with_minimize_button(true)
        .with_maximize_button(true)
        .with_fullsize_content_view(true)
        .with_title_shown(false)
        .with_titlebar_shown(false)
}

pub(crate) fn popup(title: impl Into<String>) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_resizable(false)
        .with_transparent(true)
        .with_decorations(false)
        .with_taskbar(false)
        .with_close_button(false)
        .with_minimize_button(false)
        .with_maximize_button(false)
        .with_has_shadow(false)
        .with_always_on_top()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_and_popup_native_policies_are_explicit_and_disjoint() {
        let document = document();
        let popup = popup("Popup");
        for (builder, opaque) in [(document, true), (popup, false)] {
            assert_eq!(builder.transparent, Some(!opaque));
            assert_eq!(builder.has_shadow, Some(opaque));
            assert_eq!(builder.decorations, Some(opaque));
            assert_eq!(builder.close_button, Some(opaque));
            assert_eq!(builder.minimize_button, Some(opaque));
            assert_eq!(builder.maximize_button, Some(opaque));
            assert_eq!(builder.active, None, "painting must not activate windows");
        }
    }
}
