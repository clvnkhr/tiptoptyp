//! Explorer mutations are authenticated server operations. Mirror files are
//! never edited directly, and an unknown outcome is never automatically retried.
use super::*;
use crate::overleaf::FileAction;

#[derive(Default)]
pub(super) struct State {
    access: FileAccess,
    next_request: u64,
    operation: Operation,
}
#[derive(Default)]
enum FileAccess {
    #[default]
    Locked,
    Writable {
        epoch: u64,
    },
}
#[derive(Default)]
enum Operation {
    #[default]
    Idle,
    Editing(Dialog),
    Submitting {
        dialog: Dialog,
        request: Pending,
    },
    Uncertain {
        dialog: Dialog,
        request: Pending,
    },
}
impl State {
    fn dialog(&self) -> Option<&Dialog> {
        match &self.operation {
            Operation::Idle => None,
            Operation::Editing(dialog)
            | Operation::Submitting { dialog, .. }
            | Operation::Uncertain { dialog, .. } => Some(dialog),
        }
    }
    fn dialog_mut(&mut self) -> Option<&mut Dialog> {
        match &mut self.operation {
            Operation::Idle => None,
            Operation::Editing(dialog)
            | Operation::Submitting { dialog, .. }
            | Operation::Uncertain { dialog, .. } => Some(dialog),
        }
    }
    fn pending(&self) -> Option<&Pending> {
        match &self.operation {
            Operation::Submitting { request, .. } => Some(request),
            _ => None,
        }
    }
    fn submit(&mut self, request: Pending) {
        let Operation::Editing(dialog) = std::mem::take(&mut self.operation) else {
            panic!("Overleaf file submission requires an editable dialog");
        };
        self.operation = Operation::Submitting { dialog, request };
    }
    fn complete(&mut self) -> Option<Pending> {
        match std::mem::take(&mut self.operation) {
            Operation::Submitting { dialog, request } => {
                self.operation = Operation::Editing(dialog);
                Some(request)
            }
            state => {
                self.operation = state;
                None
            }
        }
    }
    fn expire(&mut self) {
        self.operation = match std::mem::take(&mut self.operation) {
            Operation::Submitting {
                mut dialog,
                request,
            } => {
                dialog.error = Some("Overleaf has not confirmed this operation. Reconnect to inspect the project before retrying.".into());
                self.access = FileAccess::Locked;
                Operation::Uncertain { dialog, request }
            }
            state => state,
        };
    }
    fn grant(&mut self, epoch: u64, writable: bool) {
        self.access = if writable {
            FileAccess::Writable { epoch }
        } else {
            FileAccess::Locked
        };
        // Only a fresh authenticated capability can release an uncertain form.
        if writable {
            self.operation = match std::mem::take(&mut self.operation) {
                Operation::Uncertain { dialog, request } if epoch >= request.epoch => {
                    Operation::Editing(dialog)
                }
                state => state,
            };
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    fn phase(&self) -> &'static str {
        match self.operation {
            Operation::Idle => "idle",
            Operation::Editing(_) => "editing",
            Operation::Submitting { .. } => "submitting",
            Operation::Uncertain { .. } => "uncertain",
        }
    }
}
struct Pending {
    epoch: u64,
    request: u64,
    action: FileAction,
    started: Instant,
}
#[derive(Clone)]
struct Dialog {
    epoch: u64,
    action: FileAction,
    id: Option<String>,
    path: String,
    name: String,
    parent: String,
    folders: Vec<String>,
    focus: bool,
    error: Option<String>,
    edit_events: Vec<egui::Event>,
}
#[derive(Clone, Debug)]
pub(in crate::app) enum Action {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Duplicate(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Move(PathBuf),
}
impl Action {
    fn path(&self) -> &Path {
        match self {
            Self::NewFile(path)
            | Self::NewFolder(path)
            | Self::Duplicate(path)
            | Self::Rename(path)
            | Self::Delete(path)
            | Self::Move(path) => path,
        }
    }
    fn kind(&self) -> FileAction {
        match self {
            Self::NewFile(_) => FileAction::CreateFile,
            Self::NewFolder(_) => FileAction::CreateFolder,
            Self::Duplicate(_) => FileAction::Duplicate,
            Self::Rename(_) => FileAction::Rename,
            Self::Delete(_) => FileAction::Delete,
            Self::Move(_) => FileAction::Move,
        }
    }
}
impl Dialog {
    fn title(&self) -> &'static str {
        match self.action {
            FileAction::CreateFile => "New Overleaf file",
            FileAction::CreateFolder => "New Overleaf folder",
            FileAction::Duplicate => "Duplicate Overleaf document",
            FileAction::Rename => "Rename Overleaf entry",
            FileAction::Delete => "Delete from Overleaf",
            FileAction::Move => "Move Overleaf entry",
        }
    }
    fn primary(&self) -> &'static str {
        match self.action {
            FileAction::CreateFile | FileAction::CreateFolder => "Create",
            FileAction::Duplicate => "Duplicate",
            FileAction::Rename => "Rename",
            FileAction::Delete => "Delete",
            FileAction::Move => "Move",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Submit,
    Cancel,
}

impl EditorApp {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(in crate::app) fn overleaf_files_inspection(&self) -> serde_json::Value {
        let dialog = self.overleaf.files.dialog();
        serde_json::json!({
            "dialog": dialog.is_some(),
            "phase": self.overleaf.files.phase(),
            "pending": self.overleaf_file_operation_pending(),
            "action": dialog.map(|dialog| dialog.action),
            "name_fingerprint": dialog.map(|dialog| crate::desktop_test::fingerprint(&dialog.name)),
            "parent_fingerprint": dialog.map(|dialog| crate::desktop_test::fingerprint(&dialog.parent)),
            "error": dialog.and_then(|dialog| dialog.error.as_deref()),
            "writable": self.overleaf_files_writable(),
        })
    }
    pub(in crate::app) fn route_overleaf_file_command(
        &mut self,
        command: AppCommand,
        context: &egui::Context,
    ) -> bool {
        if !self.overleaf_file_dialog_open() {
            return false;
        }
        if command == AppCommand::CloseTab {
            if !self.overleaf_file_operation_pending() {
                self.overleaf.files.operation = Operation::Idle;
                crate::window_host::return_focus(context);
            }
        } else {
            self.route_edit_command_to_focused_widget(
                command,
                context,
                scoped_child_viewport_id(context, "tiptoptyp-overleaf-file-overlay"),
            );
        }
        true
    }
    pub(in crate::app) fn queue_overleaf_file_edit_event(&mut self, event: egui::Event) {
        if self.overleaf_file_operation_pending() {
            return;
        }
        if let Some(dialog) = self.overleaf.files.dialog_mut() {
            dialog.edit_events.push(event);
        }
    }
    pub(in crate::app) fn overleaf_file_dialog_open(&self) -> bool {
        self.overleaf.files.dialog().is_some()
    }
    pub(in crate::app) fn overleaf_file_operation_pending(&self) -> bool {
        self.overleaf.files.pending().is_some()
    }
    fn overleaf_files_writable(&self) -> bool {
        matches!(self.overleaf.files.access, FileAccess::Writable { epoch } if epoch == self.overleaf.epoch)
            && self
                .overleaf
                .connection
                .as_ref()
                .is_some_and(|connection| connection.online() && !connection.hydration.pending())
    }
    pub(in crate::app) fn show_overleaf_file_menu(
        &self,
        ui: &mut egui::Ui,
        path: &Path,
        is_file: bool,
        action: &mut Option<AppPopupAction>,
    ) {
        let entry = self.overleaf_entry(path);
        let mutable = self.overleaf_files_writable()
            && !self.overleaf_file_operation_pending()
            && !self.document_flow_busy();
        let directory = if is_file {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        file_menu(
            ui,
            path,
            directory,
            entry.map(|entry| entry.kind),
            mutable,
            action,
        );
    }
    pub(in crate::app) fn begin_overleaf_file_action(&mut self, action: Action) {
        if self.document_flow_busy() || self.overleaf_file_operation_pending() {
            return;
        }
        if !self.overleaf_files_writable() {
            self.show_file_error("Wait for an editable Overleaf project to finish syncing.".into());
            return;
        }
        let connection = self.overleaf.connection.as_ref().unwrap();
        let Some(relative) = action
            .path()
            .strip_prefix(&connection.root)
            .ok()
            .and_then(Path::to_str)
        else {
            self.show_file_error("This entry does not belong to the Overleaf project.".into());
            return;
        };
        let kind = action.kind();
        let creating = matches!(kind, FileAction::CreateFile | FileAction::CreateFolder);
        let tree = connection.model.tree().unwrap();
        let entry = tree.id_for_path(relative).and_then(|id| tree.entry(id));
        if (!creating && entry.is_none())
            || (creating
                && !relative.is_empty()
                && entry.is_none_or(|entry| entry.kind != EntryKind::Folder))
        {
            self.show_file_error("This Overleaf entry changed. Reopen its menu.".into());
            return;
        }
        if kind == FileAction::Duplicate && entry.is_none_or(|entry| entry.kind != EntryKind::Doc) {
            self.show_file_error(
                "Only editable Overleaf documents can be duplicated safely in the app.".into(),
            );
            return;
        }
        if matches!(kind, FileAction::Duplicate | FileAction::Delete)
            && self.tabs.ids().any(|tab| {
                self.document_for_tab(tab).is_some_and(|document| {
                    document.is_dirty()
                        && document.path().as_ref().is_some_and(|path| {
                            path == action.path() || path.starts_with(action.path())
                        })
                })
            })
        {
            self.show_file_error(
                "Save the affected Overleaf drafts before duplicating or deleting them.".into(),
            );
            return;
        }
        let parent = if creating {
            relative
        } else {
            relative.rsplit_once('/').map_or("", |(parent, _)| parent)
        }
        .to_owned();
        let name = match kind {
            FileAction::CreateFile => available_name(tree, &parent, "untitled.tex", false),
            FileAction::CreateFolder => available_name(tree, &parent, "New folder", false),
            FileAction::Duplicate => available_name(
                tree,
                &parent,
                relative.rsplit('/').next().unwrap_or("untitled.tex"),
                true,
            ),
            _ => relative.rsplit('/').next().unwrap_or_default().to_owned(),
        };
        let mut folders = vec![String::new()];
        folders.extend(
            tree.entries()
                .iter()
                .filter(|candidate| {
                    candidate.kind == EntryKind::Folder
                        && candidate.path != relative
                        && !candidate.path.starts_with(&format!("{relative}/"))
                })
                .map(|entry| entry.path.clone()),
        );
        self.overleaf.files.operation = Operation::Editing(Dialog {
            epoch: self.overleaf.epoch,
            action: kind,
            id: entry.map(|entry| entry.id.clone()),
            path: relative.into(),
            name,
            parent,
            folders,
            focus: true,
            error: None,
            edit_events: Vec::new(),
        });
    }
    fn submit_overleaf_file_action(&mut self) {
        let Some(dialog) = self.overleaf.files.dialog().cloned() else {
            return;
        };
        if self.overleaf.files.pending().is_some() {
            return;
        }
        let validation = (|| {
            if dialog.epoch != self.overleaf.epoch || !self.overleaf_files_writable() {
                return Err(
                    "The Overleaf connection changed. Reopen the file menu after syncing.".into(),
                );
            }
            let tree = self
                .overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .tree()
                .unwrap();
            validate_dialog(&dialog, tree)?;
            self.overleaf.files.next_request = self
                .overleaf
                .files
                .next_request
                .checked_add(1)
                .filter(|value| *value <= 9_007_199_254_740_991)
                .ok_or("Reconnect before making more file changes.")?;
            let request = self.overleaf.files.next_request;
            self.overleaf.send(BrowserCommand::FileAction {
                epoch: dialog.epoch,
                request,
                action: dialog.action,
                id: dialog.id.clone(),
                path: dialog.id.as_ref().map(|_| dialog.path.clone()),
                parent: matches!(
                    dialog.action,
                    FileAction::CreateFile | FileAction::CreateFolder | FileAction::Move
                )
                .then(|| dialog.parent.clone()),
                name: (!matches!(dialog.action, FileAction::Delete | FileAction::Move))
                    .then(|| dialog.name.clone()),
            })?;
            self.overleaf.files.submit(Pending {
                epoch: dialog.epoch,
                request,
                action: dialog.action,
                started: Instant::now(),
            });
            Ok::<_, String>(())
        })();
        if let Err(error) = validation {
            self.overleaf.files.dialog_mut().unwrap().error = Some(error);
        }
    }
    pub(super) fn receive_overleaf_file_event(
        &mut self,
        event: &BrowserEvent,
        context: &egui::Context,
    ) -> Result<bool, String> {
        match event {
            BrowserEvent::FileAccess { epoch, writable } => {
                self.overleaf.files.grant(*epoch, *writable);
                Ok(true)
            }
            BrowserEvent::FileAction {
                epoch,
                request,
                id,
                path,
                error,
            } => {
                if error.as_ref().is_some_and(|error| error.len() > 4096)
                    || id.as_ref().is_some_and(|id| {
                        id.len() != 24 || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                    || path
                        .as_ref()
                        .is_some_and(|path| crate::overleaf::validate_path(path).is_err())
                {
                    return Err("Overleaf returned an invalid file-operation receipt.".into());
                }
                if self
                    .overleaf
                    .files
                    .pending()
                    .is_none_or(|pending| pending.epoch != *epoch || pending.request != *request)
                {
                    return Ok(true);
                }
                let pending = self.overleaf.files.complete().unwrap();
                if let Some(error) = error {
                    if let Some(dialog) = self.overleaf.files.dialog_mut() {
                        dialog.error = Some(error.clone());
                        dialog.focus = true;
                    }
                    return Ok(true);
                }
                if matches!(
                    pending.action,
                    FileAction::CreateFile | FileAction::CreateFolder | FileAction::Duplicate
                ) && !id.as_ref().zip(path.as_ref()).is_some_and(|(id, path)| {
                    self.overleaf
                        .connection
                        .as_ref()
                        .and_then(|connection| connection.model.tree())
                        .and_then(|tree| tree.entry(id))
                        .is_some_and(|entry| &entry.path == path)
                }) {
                    return Err("Overleaf did not confirm the new entry in its file tree.".into());
                }
                self.overleaf.files.operation = Operation::Idle;
                self.notice = Some(Notice {
                    message: "Overleaf file changes saved".into(),
                    kind: NoticeKind::Success,
                });
                if let Some(path) = path
                    && let Some(connection) = &self.overleaf.connection
                {
                    let path = connection.root.join(path);
                    self.explorer.select_path(path.clone());
                    if matches!(
                        pending.action,
                        FileAction::CreateFile | FileAction::Duplicate
                    ) {
                        self.open_overleaf_path(&path);
                    }
                }
                crate::window_host::return_focus(context);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
    pub(in crate::app) fn show_overleaf_file_dialog(&mut self, context: &egui::Context) {
        if self.overleaf.files.pending().is_some_and(|pending| {
            pending.epoch != self.overleaf.epoch
                || self
                    .overleaf
                    .connection
                    .as_ref()
                    .is_none_or(|connection| !connection.online())
                || pending.started.elapsed() > Duration::from_secs(50)
        }) {
            self.overleaf.files.expire();
        }
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let busy = self.overleaf.files.pending().is_some();
        let Some(dialog) = self.overleaf.files.dialog_mut() else {
            return;
        };
        let title = dialog.title();
        let style = context.style_of(context.theme());
        let shortcuts = self.settings.effective_shortcuts();
        let captures = self.captures.clone();
        let child = scoped_child_viewport_id(context, "tiptoptyp-overleaf-file-overlay");
        let admitted = self
            .pending_widget_paste
            .is_some_and(|pending| pending.admits(child, context.cumulative_frame_nr()));
        let mut paste_requested = false;
        let mut paste_admitted = false;
        let mut choice = None;
        let spec = ChildViewSpec::modal(
            "tiptoptyp-overleaf-file-overlay",
            title,
            window_rect,
            "overleaf-file",
        );
        ChildViewHost::show(
            context,
            &captures,
            spec,
            context.theme(),
            &style,
            |ui, input| {
                handle_window_management_shortcuts(ui.ctx(), ui.ctx().viewport_id(), &shortcuts);
                let edits = prepare_child_widget_edit_events(ui.ctx(), &shortcuts, admitted);
                paste_requested |= edits.paste_requested;
                paste_admitted |= edits.requested_paste_admitted;
                let _queued =
                    crate::child_view::begin_queued_edit_events(ui.ctx(), &mut dialog.edit_events);
                if !busy
                    && (input.close_requested
                        || input.escape_pressed
                        || [ShortcutAction::CloseWindow, ShortcutAction::CloseTab]
                            .into_iter()
                            .any(|action| {
                                shortcuts.egui(action).is_some_and(|shortcut| {
                                    ui.input_mut(|input| input.consume_shortcut(&shortcut))
                                })
                            }))
                {
                    choice = Some(Choice::Cancel);
                }
                egui::Area::new(viewport_scoped_id(ui.ctx(), "overleaf-file-form"))
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::dialog_card_frame(&style).show(ui, |ui| {
                            choice = file_form(ui, dialog, busy).or(choice);
                        });
                    });
            },
        );
        if paste_admitted {
            self.pending_widget_paste = None;
        }
        if paste_requested {
            self.request_widget_paste(context, child);
        }
        if busy {
            context.request_repaint_after(Duration::from_millis(100));
        }
        match choice {
            Some(Choice::Cancel) => {
                self.overleaf.files.operation = Operation::Idle;
                crate::window_host::return_focus(context);
            }
            Some(Choice::Submit) => self.submit_overleaf_file_action(),
            None => {}
        }
    }
}

fn available_name(tree: &Tree, parent: &str, original: &str, copy: bool) -> String {
    use unicode_normalization::UnicodeNormalization as _;
    let occupied: BTreeSet<_> = tree
        .entries()
        .iter()
        .map(|entry| entry.path.nfc().collect::<String>().to_lowercase())
        .collect();
    let path = Path::new(original);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(original);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    for index in 0..=crate::overleaf::MAX_ENTRIES {
        let name = if !copy && index == 0 {
            original.into()
        } else {
            let copy_index = if copy { index + 1 } else { index };
            let suffix = if copy_index == 1 {
                " copy".into()
            } else {
                format!(" copy {copy_index}")
            };
            format!("{stem}{suffix}{extension}")
        };
        let candidate = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/{name}")
        };
        if !occupied.contains(&candidate.nfc().collect::<String>().to_lowercase()) {
            return name;
        }
    }
    original.into()
}
fn validate_name(name: &str) -> Result<(), String> {
    crate::overleaf::validate_path(name)?;
    if name.contains('/')
        || name.trim() != name
        || name.encode_utf16().count() >= 150
        || name.contains('*')
        || name
            .chars()
            .any(|value| ('\u{7f}'..='\u{9f}').contains(&value))
        || [
            "prototype",
            "constructor",
            "toString",
            "toLocaleString",
            "valueOf",
            "hasOwnProperty",
            "isPrototypeOf",
            "propertyIsEnumerable",
            "__defineGetter__",
            "__lookupGetter__",
            "__defineSetter__",
            "__lookupSetter__",
            "__proto__",
        ]
        .contains(&name)
    {
        return Err("Use a file name of 1–149 characters without slashes, control characters or surrounding spaces.".into());
    }
    Ok(())
}
fn validate_dialog(dialog: &Dialog, tree: &Tree) -> Result<(), String> {
    use unicode_normalization::UnicodeNormalization as _;
    if let Some(id) = &dialog.id
        && tree.entry(id).is_none_or(|entry| entry.path != dialog.path)
    {
        return Err("This Overleaf entry changed. Reopen its menu.".into());
    }
    if dialog.action == FileAction::Delete {
        return Ok(());
    }
    let parent = if matches!(dialog.action, FileAction::Rename | FileAction::Duplicate) {
        dialog
            .path
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent)
    } else {
        &dialog.parent
    };
    if !parent.is_empty()
        && tree
            .id_for_path(parent)
            .and_then(|id| tree.entry(id))
            .is_none_or(|entry| entry.kind != EntryKind::Folder)
    {
        return Err("This Overleaf folder is no longer available.".into());
    }
    if dialog.action != FileAction::Move {
        validate_name(&dialog.name)?;
    }
    if dialog.action == FileAction::Move
        && tree
            .entry(dialog.id.as_deref().unwrap_or_default())
            .is_some_and(|entry| {
                entry.kind == EntryKind::Folder
                    && (parent == dialog.path || parent.starts_with(&format!("{}/", dialog.path)))
            })
    {
        return Err("A folder cannot be moved inside itself.".into());
    }
    let name = if dialog.action == FileAction::Move {
        dialog.path.rsplit('/').next().unwrap_or_default()
    } else {
        &dialog.name
    };
    let path = if parent.is_empty() {
        name.into()
    } else {
        format!("{parent}/{name}")
    };
    let normalized = |value: &str| value.nfc().collect::<String>().to_lowercase();
    let folded = normalized(&path);
    if tree.entries().iter().any(|entry| {
        Some(&entry.id)
            != dialog
                .id
                .as_ref()
                .filter(|_| dialog.action != FileAction::Duplicate)
            && normalized(&entry.path) == folded
    }) {
        return Err("An Overleaf file or folder with this name already exists.".into());
    }
    Ok(())
}
fn file_form(ui: &mut egui::Ui, dialog: &mut Dialog, busy: bool) -> Option<Choice> {
    theme::apply_compact_tool_spacing(ui);
    ui.set_width((ui.ctx().content_rect().width() - 32.0).clamp(200.0, 420.0));
    ui.strong(dialog.title());
    let mut enter = false;
    if dialog.action == FileAction::Delete {
        ui.label(format!(
            "Delete {} and its contents from Overleaf? This cannot be undone here.",
            dialog.name
        ));
    } else if dialog.action == FileAction::Move {
        ui.label(&dialog.name);
        let response = ui
            .add_enabled_ui(!busy, |ui| {
                egui::ComboBox::from_id_salt("overleaf-file-parent")
                    .selected_text(if dialog.parent.is_empty() {
                        "Project root"
                    } else {
                        &dialog.parent
                    })
                    .show_ui(ui, |ui| {
                        for folder in &dialog.folders {
                            let response = ui.selectable_value(
                                &mut dialog.parent,
                                folder.clone(),
                                if folder.is_empty() {
                                    "Project root"
                                } else {
                                    folder
                                },
                            );
                            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                            crate::desktop_test::observe(
                                &format!(
                                    "overleaf.files.folder.{}",
                                    if folder.is_empty() { "root" } else { folder }
                                ),
                                &response,
                            );
                            let _ = response;
                        }
                    })
                    .response
            })
            .inner;
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("overleaf.files.parent", &response);
        let _ = response;
    } else {
        let response = ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut dialog.name)
                .id_salt("overleaf-file-name")
                .hint_text("File name")
                .desired_width(f32::INFINITY),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, !busy, "Overleaf file name")
        });
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("overleaf.files.name", &response);
        if response.changed() {
            dialog.error = None;
        }
        if dialog.focus && !busy {
            response.request_focus();
            crate::window_host::focus(
                ui.ctx(),
                ui.ctx().viewport_id(),
                crate::window_host::FocusCause::UserAction,
            );
            dialog.focus = false;
        }
        enter =
            !busy && response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    }
    if let Some(error) = &dialog.error {
        ui.colored_label(error_color(ui.ctx()), error);
    }
    let mut choice = None;
    ui.horizontal(|ui| {
        if busy {
            ui.spinner();
            ui.weak("Saving on Overleaf…");
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let primary = ui.add_enabled(
                !busy,
                egui::Button::new(dialog.primary()).fill(ui.visuals().selection.bg_fill),
            );
            let cancel = ui.add_enabled(!busy, egui::Button::new("Cancel"));
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            {
                crate::desktop_test::observe("overleaf.files.submit", &primary);
                crate::desktop_test::observe("overleaf.files.cancel", &cancel);
            }
            if primary.clicked() || enter {
                choice = Some(Choice::Submit);
            }
            if cancel.clicked() {
                choice = Some(Choice::Cancel);
            }
        });
    });
    choice
}
fn file_menu(
    ui: &mut egui::Ui,
    path: &Path,
    directory: &Path,
    kind: Option<EntryKind>,
    mutable: bool,
    action: &mut Option<AppPopupAction>,
) {
    let file = kind.is_some_and(|kind| kind != EntryKind::Folder);
    if menu_item_enabled(ui, file, "Open", None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Open(
            path.into(),
        )));
    }
    let source = file && preview_source_path(path);
    if menu_item_enabled(ui, source, "Use for preview", None).clicked() {
        *action = Some(AppPopupAction::Workspace(
            WorkspaceMenuAction::TogglePreview(path.into()),
        ));
    }
    ui.separator();
    for (label, command, enabled, observation) in [
        (
            "New file…",
            Action::NewFile(directory.into()),
            mutable,
            "menu.overleaf.new_file",
        ),
        (
            "New folder…",
            Action::NewFolder(directory.into()),
            mutable,
            "menu.overleaf.new_folder",
        ),
        (
            "Duplicate…",
            Action::Duplicate(path.into()),
            mutable && kind == Some(EntryKind::Doc),
            "menu.overleaf.duplicate",
        ),
        (
            "Rename…",
            Action::Rename(path.into()),
            mutable && kind.is_some(),
            "menu.overleaf.rename",
        ),
        (
            "Move…",
            Action::Move(path.into()),
            mutable && kind.is_some(),
            "menu.overleaf.move",
        ),
        (
            "Delete…",
            Action::Delete(path.into()),
            mutable && kind.is_some(),
            "menu.overleaf.delete",
        ),
    ] {
        let response = menu_item_enabled(ui, enabled, label, None);
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe(observation, &response);
        let _ = observation;
        if response.clicked() {
            *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Overleaf(
                command,
            )));
        }
    }
    ui.separator();
    if menu_item(ui, "Copy Relative Path", None).clicked() {
        *action = Some(AppPopupAction::Workspace(WorkspaceMenuAction::Copy {
            path: path.into(),
            kind: WorkspaceCopyKind::RelativePath,
        }));
    }
}

fn preview_source_path(path: &Path) -> bool {
    crate::document::is_tex_path(path)
        || path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("typ"))
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
