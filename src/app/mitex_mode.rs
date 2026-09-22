//! The user-facing boundary for the optional displayed-source representation.
use super::*;

impl EditorApp {
    pub(super) fn tex_mode_available(&mut self) -> bool {
        if self.document().config().is_some() {
            return true;
        }
        if !self.document().kind().is_typst() {
            return false;
        }
        self.prepare_editor_source_data();
        self.editor_data
            .mitex_compatible(&self.settings.mitex_version)
    }
    fn tex_config(&self) -> tiptoptyp::mitex_projection::Config {
        tiptoptyp::mitex_projection::Config {
            package: format!("@preview/mitex:{}", self.settings.mitex_version.trim()),
        }
    }
    fn tex_error(&mut self, error: tiptoptyp::mitex_document::Error) {
        let message = if let tiptoptyp::mitex_document::Error::Translation(
            tiptoptyp::mitex_projection::Error::NativeMath { byte },
        ) = error
        {
            let line = self.document().source()[..byte.min(self.document().source().len())]
                .bytes()
                .filter(|&b| b == b'\n')
                .count()
                + 1;
            format!(
                "Cannot enable TeX dollar notation: native Typst math on line {line}. Remove or explicitly convert that math first."
            )
        } else {
            error.to_string()
        };
        self.notice = Some(Notice {
            message,
            kind: NoticeKind::Error,
        });
    }
    pub(super) fn activate_preferred_tex(&mut self) -> bool {
        if !self.tex_mode_available() {
            return false;
        }
        let config = self.tex_config();
        match self.document_mut().enable(config) {
            Ok(_) => true,
            Err(error) => {
                self.tex_error(error);
                false
            }
        }
    }
    pub(super) fn set_tex_mode(&mut self, enabled: bool, context: &egui::Context) -> bool {
        if self.table_editor.is_some() {
            return false;
        }
        if enabled == self.document().config().is_some() {
            return true;
        }
        let cursor = self.editor_snapshot(context).cursor;
        let result = if enabled {
            let config = self.tex_config();
            self.document_mut().enable(config)
        } else {
            self.document_mut().disable()
        };
        let map = match result {
            Ok(Some(map)) => map,
            Ok(None) => return true,
            Err(error) => {
                self.tex_error(error);
                return false;
            }
        };
        let remap = |cursor: CCursor| {
            let byte = map
                .input()
                .char_indices()
                .nth(cursor.index.0)
                .map_or(map.input().len(), |(byte, _)| byte);
            let byte = map.input_to_output(byte).unwrap_or(map.output().len());
            CCursor::new(map.output()[..byte].chars().count())
        };
        let mut state = egui::text_edit::TextEditState::load(context, source_editor_id(context))
            .unwrap_or_default();
        state.clear_undoer();
        state.cursor.set_char_range(Some(CCursorRange {
            primary: remap(cursor.primary),
            secondary: remap(cursor.secondary),
            h_pos: cursor.h_pos,
        }));
        state.store(context, source_editor_id(context));
        self.document_mut().set_history_reset(false);
        self.pending_editor_selection = None;
        self.editor_attention = None;
        self.find_bar.search.clear();
        self.mark_edited();
        self.prepare_editor_source_data();
        self.reset_document_services();
        self.schedule_compile_now();
        self.schedule_autosave_if_needed();
        self.notice = Some(Notice {
            message: if enabled {
                "TeX dollar notation enabled — saves standard MiTeX calls"
            } else {
                "TeX dollar notation disabled — showing saved Typst syntax"
            }
            .into(),
            kind: NoticeKind::Success,
        });
        true
    }
}
