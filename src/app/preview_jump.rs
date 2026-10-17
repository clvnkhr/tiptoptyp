//! Versioned context-menu destinations, independent of caret and pointer motion.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceLocation {
    pub key: DocumentKey,
    pub char_index: usize,
}

#[derive(Debug, Clone)]
pub(super) struct WebMenuRequest {
    pub generation: crate::tinymist::Generation,
    pub token: u64,
    pub point: Pos2,
}

impl WebMenuRequest {
    pub(super) fn parse(
        value: &serde_json::Value,
        generation: crate::tinymist::Generation,
    ) -> Option<Self> {
        let token = value["token"]
            .as_u64()
            .filter(|token| *token > 0 && *token <= 9_007_199_254_740_991)?;
        let point = Pos2::new(value["x"].as_f64()? as f32, value["y"].as_f64()? as f32);
        (point.is_finite() && point.x >= 0.0 && point.y >= 0.0).then_some(Self {
            generation,
            token,
            point,
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct WebSourceLocation {
    pub generation: crate::tinymist::Generation,
    pub token: u64,
    pub path: PathBuf,
    pub url: String,
    pub window: egui::ViewportId,
}

/// Native child coordinates are logical desktop points; egui's interface zoom
/// applies once when positioning the menu in its owning window.
pub(super) fn web_menu_anchor(bounds: NativeRect, point: Pos2, zoom: f32) -> Option<Pos2> {
    (point.is_finite()
        && zoom.is_finite()
        && zoom > 0.0
        && point.x >= 0.0
        && point.y >= 0.0
        && point.x <= bounds.width()
        && point.y <= bounds.height())
    .then(|| {
        Pos2::new(
            (bounds.left() + point.x) / zoom,
            (bounds.top() + point.y) / zoom,
        )
    })
}

/// Tinymist can miss a mapped span at its leading boundary. Target the inside
/// of the clicked glyph, as preview text search does, without moving the caret.
pub(super) fn source_reveal_offset(source: &str, char_index: usize) -> usize {
    if source
        .chars()
        .nth(char_index)
        .is_some_and(|character| !character.is_whitespace())
    {
        char_index + 1
    } else {
        char_index
    }
}

impl EditorMenuAction {
    pub(super) fn at_source(self, source: Option<SourceLocation>) -> Option<Self> {
        if self == Self::Command(AppCommand::SyncPreview) {
            source.map(Self::RevealInPreview)
        } else {
            Some(self)
        }
    }
}

impl EditorApp {
    pub(super) fn execute_source_menu_jump(&mut self, source: SourceLocation) {
        if self.document().key() == source.key {
            self.jump_source_to_preview(source.char_index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_reveal_aims_inside_glyphs_without_crossing_whitespace_or_eof() {
        assert_eq!(source_reveal_offset("α word\nnext", 0), 1);
        assert_eq!(source_reveal_offset("α word\nnext", 2), 3);
        assert_eq!(source_reveal_offset("α word\nnext", 1), 1);
        assert_eq!(source_reveal_offset("α word\nnext", 6), 6);
        assert_eq!(source_reveal_offset("α word\nnext", 11), 11);
    }

    #[test]
    fn native_menu_position_accounts_for_interface_zoom_once_and_rejects_outside_points() {
        for zoom in [0.75, 1.0, 1.25, 1.5] {
            let bounds = scale_rect_from_egui_to_native(
                Rect::from_min_size(Pos2::new(500.0, 30.0), Vec2::new(600.0, 700.0)),
                Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 900.0)),
                zoom,
            )
            .unwrap();
            assert_eq!(
                web_menu_anchor(bounds, Pos2::new(20.0 * zoom, 50.0 * zoom), zoom),
                Some(Pos2::new(520.0, 80.0))
            );
            assert!(web_menu_anchor(bounds, Pos2::new(bounds.width() + 1.0, 50.0), zoom).is_none());
            assert!(web_menu_anchor(bounds, Pos2::new(-1.0, 0.0), zoom).is_none());
            assert!(web_menu_anchor(bounds, Pos2::ZERO, f32::NAN).is_none());
        }
    }

    #[test]
    fn web_context_requests_reject_invalid_tokens_and_coordinates() {
        let generation = crate::tinymist::Generation(1);
        for value in [
            serde_json::json!({"token":0,"x":1,"y":2}),
            serde_json::json!({"token":-1,"x":1,"y":2}),
            serde_json::json!({"token":9007199254740992_u64,"x":1,"y":2}),
            serde_json::json!({"token":1,"x":-1,"y":2}),
            serde_json::json!({"token":1,"x":1e100,"y":2}),
            serde_json::json!({"token":1,"x":1}),
        ] {
            assert!(WebMenuRequest::parse(&value, generation).is_none());
        }
        let request =
            WebMenuRequest::parse(&serde_json::json!({"token":3,"x":1.5,"y":2.25}), generation)
                .unwrap();
        assert_eq!(request.token, 3);
        assert_eq!(request.point, Pos2::new(1.5, 2.25));
    }

    #[test]
    fn editor_jump_uses_the_clicked_location_instead_of_the_preserved_selection_endpoint() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let mut data = EditorDerivedData::default();
            let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 1, 1);
            let popup = editor_view::editor_context_popup(
                &mut data,
                "α first\nsecond",
                kind,
                Pos2::ZERO,
                Some(3),
                key,
            );
            let AppPopup::Editor { source, .. } = popup else {
                panic!("expected source menu")
            };
            assert_eq!(source, Some(SourceLocation { key, char_index: 3 }));
            assert_eq!(
                EditorMenuAction::Command(AppCommand::SyncPreview).at_source(source),
                Some(EditorMenuAction::RevealInPreview(SourceLocation {
                    key,
                    char_index: 3
                }))
            );
        }
        assert!(
            EditorMenuAction::Command(AppCommand::SyncPreview)
                .at_source(None)
                .is_none()
        );
    }
    #[test]
    fn frontend_retirement_clears_a_queued_preview_context_action() {
        for loaded in [false, true] {
            let context = egui::Context::default();
            let directory = tempfile::tempdir().unwrap();
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            let target = WebSourceLocation {
                generation: crate::tinymist::Generation(1),
                token: 7,
                path: app.preview_document_path(),
                url: "http://localhost/old".into(),
                window: egui::ViewportId::ROOT,
            };
            app.pending_web_preview_menu = Some(WebMenuRequest {
                generation: target.generation,
                token: target.token,
                point: Pos2::new(4.0, 5.0),
            });
            app.app_popup = Some(AppPopup::PreviewSource {
                anchor: Pos2::ZERO,
                window: target.window,
                target: target.clone(),
            });
            app.pending_app_popup_action = Some(AppPopupAction::RevealPreviewSource(target));
            if loaded {
                app.handle_web_action(r#"{"type":"preview-loaded"}"#);
            } else {
                app.discard_webview();
            }
            assert!(app.pending_web_preview_menu.is_none());
            assert!(app.pending_app_popup_action.is_none());
            assert!(app.app_popup.is_none());
        }
    }
}
