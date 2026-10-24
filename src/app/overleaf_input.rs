//! The connection window owns its keyboard, including native WebKit responders.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Elsewhere,
    Hide,
    Address,
    BrowserEdit,
    WidgetEdit,
    Swallow,
}

fn route(command: AppCommand, visible: bool, focused: bool, browser_focused: bool) -> Route {
    if !visible || !focused {
        return Route::Elsewhere;
    }
    match command {
        AppCommand::CloseTab => Route::Hide,
        AppCommand::Find | AppCommand::FindReplace | AppCommand::Overleaf => Route::Address,
        command if settings_window::is_widget_edit_command(command) => {
            if browser_focused {
                Route::BrowserEdit
            } else {
                Route::WidgetEdit
            }
        }
        _ => Route::Swallow,
    }
}

impl EditorApp {
    pub(super) fn route_overleaf_command(
        &mut self,
        command: AppCommand,
        context: &egui::Context,
    ) -> bool {
        let viewport = scoped_child_viewport_id(context, overleaf::SALT);
        let focused = focused_input_viewport(context) == viewport;
        let browser_focused = focused
            && self
                .overleaf
                .browser
                .as_ref()
                .is_some_and(crate::overleaf_browser::Browser::owns_keyboard);
        match route(command, self.overleaf.visible, focused, browser_focused) {
            Route::Elsewhere => return false,
            Route::Hide => self.hide_overleaf(context),
            Route::Address => {
                if let Some(browser) = &self.overleaf.browser {
                    browser.focus_parent();
                }
                self.begin_overleaf(context);
                context.request_repaint_of(viewport);
            }
            Route::BrowserEdit => {
                if let Some(browser) = &self.overleaf.browser {
                    browser.edit_command(command);
                }
                // An unsupported native edit must not fall through into a
                // previously focused URL field or the document source.
            }
            Route::WidgetEdit => {
                let event = match command {
                    AppCommand::Cut => Some(egui::Event::Cut),
                    AppCommand::Copy => Some(egui::Event::Copy),
                    AppCommand::Undo | AppCommand::Redo | AppCommand::SelectAll => {
                        standard_text_edit_shortcut(command).map(|shortcut| egui::Event::Key {
                            key: shortcut.logical_key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: shortcut.modifiers,
                        })
                    }
                    AppCommand::Paste => {
                        self.request_widget_paste(context, viewport);
                        None
                    }
                    _ => None,
                };
                if let Some(event) = event {
                    self.overleaf.edit_events.push(event);
                }
                context.request_repaint_of(viewport);
            }
            Route::Swallow => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_or_unfocused_connection_window_never_intercepts_document_commands() {
        for spec in crate::native_menu::COMMAND_SPECS {
            for (visible, focused) in [(false, false), (false, true), (true, false)] {
                for browser_focused in [false, true] {
                    assert_eq!(
                        route(spec.command, visible, focused, browser_focused),
                        Route::Elsewhere
                    );
                }
            }
        }
    }

    #[test]
    fn native_browser_focus_overrides_stale_egui_text_focus() {
        for command in [
            AppCommand::Copy,
            AppCommand::Cut,
            AppCommand::Paste,
            AppCommand::SelectAll,
            AppCommand::Undo,
            AppCommand::Redo,
        ] {
            assert_eq!(route(command, true, true, true), Route::BrowserEdit);
            assert_eq!(route(command, true, true, false), Route::WidgetEdit);
        }
    }

    #[test]
    fn connection_commands_are_local_and_source_actions_cannot_escape() {
        assert_eq!(route(AppCommand::CloseTab, true, true, true), Route::Hide);
        assert_eq!(route(AppCommand::Find, true, true, true), Route::Address);
        for command in [
            AppCommand::Save,
            AppCommand::SaveAs,
            AppCommand::Rename,
            AppCommand::RevertTo,
            AppCommand::Format,
            AppCommand::ToggleComment,
            AppCommand::NewTable,
            AppCommand::EditTable,
            AppCommand::SyncPreview,
            AppCommand::Explorer,
        ] {
            for browser_focused in [false, true] {
                assert_eq!(route(command, true, true, browser_focused), Route::Swallow);
            }
        }
    }
}
