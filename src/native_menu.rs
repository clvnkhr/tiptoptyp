use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver, Sender, TryIter},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeMenuCommand {
    Application(ApplicationCommand),
    File(FileCommand),
    Edit(EditCommand),
    View(ViewCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplicationCommand {
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileCommand {
    New,
    NewWindow,
    Open,
    OpenInNewWindow,
    ChangeWorkspaceRoot,
    Save,
    SaveAs,
    ExportPdf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    FindReplace,
    Format,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewCommand {
    Problems,
    Explorer,
    Code,
    Split,
    Preview,
}

/// Commands already assigned to a document window but not yet executed.
///
/// Keeping this queue on `EditorApp` is important for multi-window editing:
/// egui text state is scoped to the viewport context, so a process-wide menu
/// callback may choose the target session, but only that session's next `ui`
/// pass may execute the command.
#[derive(Debug, Default)]
pub(crate) struct NativeMenuCommandQueue {
    pending: VecDeque<NativeMenuCommand>,
}

impl NativeMenuCommandQueue {
    pub(crate) fn push(&mut self, command: NativeMenuCommand) {
        self.pending.push_back(command);
    }

    pub(crate) fn extend(&mut self, commands: impl IntoIterator<Item = NativeMenuCommand>) {
        self.pending.extend(commands);
    }

    pub(crate) fn pop(&mut self) -> Option<NativeMenuCommand> {
        self.pending.pop_front()
    }
}

/// Process-wide native menu commands. The application shell drains this once
/// and assigns each command to the focused document session's local queue.
pub(crate) struct NativeMenuReceiver {
    receiver: Receiver<NativeMenuCommand>,
}

impl NativeMenuReceiver {
    pub(crate) fn pending(&self) -> TryIter<'_, NativeMenuCommand> {
        self.receiver.try_iter()
    }
}

pub(crate) fn channel() -> (Sender<NativeMenuCommand>, NativeMenuReceiver) {
    let (sender, receiver) = mpsc::channel();
    (sender, NativeMenuReceiver { receiver })
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_handler(sender: Sender<NativeMenuCommand>) -> Result<(), String> {
    macos::install_handler(sender)
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_menu(repaint: eframe::egui::Context) -> Result<(), String> {
    macos::install_menu(repaint)
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc::Sender,
        },
    };

    use objc2::{
        MainThreadMarker, ffi,
        rc::Retained,
        runtime::{AnyClass, AnyObject, Imp, Sel},
        sel,
    };
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
    use objc2_foundation::{NSInteger, NSString};

    use super::{ApplicationCommand, EditCommand, FileCommand, NativeMenuCommand, ViewCommand};

    static COMMAND_SENDER: Mutex<Option<Sender<NativeMenuCommand>>> = Mutex::new(None);
    static REPAINT_CONTEXT: Mutex<Option<eframe::egui::Context>> = Mutex::new(None);
    static MENU_INSTALLED: AtomicBool = AtomicBool::new(false);

    fn action_selector() -> Sel {
        sel!(tiptoptypPerformMenuCommand:)
    }

    #[derive(Clone, Copy)]
    struct Modifiers(u8);

    impl Modifiers {
        const COMMAND: Self = Self(1 << 0);
        const SHIFT: Self = Self(1 << 1);
        const OPTION: Self = Self(1 << 2);

        const fn with(self, other: Self) -> Self {
            Self(self.0 | other.0)
        }

        fn appkit(self) -> NSEventModifierFlags {
            let mut flags = NSEventModifierFlags::empty();
            if self.0 & Self::COMMAND.0 != 0 {
                flags |= NSEventModifierFlags::Command;
            }
            if self.0 & Self::SHIFT.0 != 0 {
                flags |= NSEventModifierFlags::Shift;
            }
            if self.0 & Self::OPTION.0 != 0 {
                flags |= NSEventModifierFlags::Option;
            }
            flags
        }
    }

    #[derive(Clone, Copy)]
    enum ItemSpec {
        Command {
            title: &'static str,
            key: &'static str,
            modifiers: Modifiers,
            command: NativeMenuCommand,
        },
        Separator,
    }

    const FILE_ITEMS: &[ItemSpec] = &[
        command(
            "New",
            "n",
            Modifiers::COMMAND,
            NativeMenuCommand::File(FileCommand::New),
        ),
        command(
            "New Window",
            "n",
            Modifiers::COMMAND.with(Modifiers::SHIFT),
            NativeMenuCommand::File(FileCommand::NewWindow),
        ),
        command(
            "Open…",
            "o",
            Modifiers::COMMAND,
            NativeMenuCommand::File(FileCommand::Open),
        ),
        command(
            "Open in New Window…",
            "o",
            Modifiers::COMMAND.with(Modifiers::OPTION),
            NativeMenuCommand::File(FileCommand::OpenInNewWindow),
        ),
        command(
            "Change Workspace Root…",
            "o",
            Modifiers::COMMAND.with(Modifiers::SHIFT),
            NativeMenuCommand::File(FileCommand::ChangeWorkspaceRoot),
        ),
        ItemSpec::Separator,
        command(
            "Save",
            "s",
            Modifiers::COMMAND,
            NativeMenuCommand::File(FileCommand::Save),
        ),
        command(
            "Save As…",
            "s",
            Modifiers::COMMAND.with(Modifiers::SHIFT),
            NativeMenuCommand::File(FileCommand::SaveAs),
        ),
        command(
            "Export PDF…",
            "e",
            Modifiers::COMMAND.with(Modifiers::SHIFT),
            NativeMenuCommand::File(FileCommand::ExportPdf),
        ),
    ];

    const EDIT_ITEMS: &[ItemSpec] = &[
        command(
            "Undo",
            "z",
            Modifiers::COMMAND,
            NativeMenuCommand::Edit(EditCommand::Undo),
        ),
        command(
            "Redo",
            "z",
            Modifiers::COMMAND.with(Modifiers::SHIFT),
            NativeMenuCommand::Edit(EditCommand::Redo),
        ),
        ItemSpec::Separator,
        command(
            "Cut",
            "x",
            Modifiers::COMMAND,
            NativeMenuCommand::Edit(EditCommand::Cut),
        ),
        command(
            "Copy",
            "c",
            Modifiers::COMMAND,
            NativeMenuCommand::Edit(EditCommand::Copy),
        ),
        command(
            "Paste",
            "v",
            Modifiers::COMMAND,
            NativeMenuCommand::Edit(EditCommand::Paste),
        ),
        command(
            "Select All",
            "a",
            Modifiers::COMMAND,
            NativeMenuCommand::Edit(EditCommand::SelectAll),
        ),
        ItemSpec::Separator,
        command(
            "Find…",
            "f",
            Modifiers::COMMAND,
            NativeMenuCommand::Edit(EditCommand::Find),
        ),
        command(
            "Find and Replace…",
            "f",
            Modifiers::COMMAND.with(Modifiers::OPTION),
            NativeMenuCommand::Edit(EditCommand::FindReplace),
        ),
        ItemSpec::Separator,
        command(
            "Format Document",
            "f",
            Modifiers::OPTION.with(Modifiers::SHIFT),
            NativeMenuCommand::Edit(EditCommand::Format),
        ),
    ];

    const VIEW_ITEMS: &[ItemSpec] = &[
        command(
            "Problems",
            "",
            Modifiers(0),
            NativeMenuCommand::View(ViewCommand::Problems),
        ),
        command(
            "Explorer",
            "",
            Modifiers(0),
            NativeMenuCommand::View(ViewCommand::Explorer),
        ),
        ItemSpec::Separator,
        command(
            "Code",
            "",
            Modifiers(0),
            NativeMenuCommand::View(ViewCommand::Code),
        ),
        command(
            "Split",
            "",
            Modifiers(0),
            NativeMenuCommand::View(ViewCommand::Split),
        ),
        command(
            "Preview",
            "",
            Modifiers(0),
            NativeMenuCommand::View(ViewCommand::Preview),
        ),
    ];

    const fn command(
        title: &'static str,
        key: &'static str,
        modifiers: Modifiers,
        command: NativeMenuCommand,
    ) -> ItemSpec {
        ItemSpec::Command {
            title,
            key,
            modifiers,
            command,
        }
    }

    pub(super) fn install_handler(sender: Sender<NativeMenuCommand>) -> Result<(), String> {
        *COMMAND_SENDER
            .lock()
            .map_err(|_| "macOS menu command channel was poisoned".to_owned())? = Some(sender);

        let class = AnyClass::get(c"WinitApplicationDelegate")
            .ok_or_else(|| "winit's macOS application delegate is unavailable".to_owned())?;
        if class.responds_to(action_selector()) {
            return Ok(());
        }

        let implementation =
            perform_menu_command as unsafe extern "C-unwind" fn(&AnyObject, Sel, &NSMenuItem);
        // SAFETY: Objective-C erases IMP argument types. This callback and the
        // encoding below both describe `void self selector object`.
        let implementation: Imp = unsafe { std::mem::transmute(implementation) };
        // SAFETY: winit's registered delegate class lives for the process. We
        // only append a uniquely named action selector and leave its lifecycle
        // methods and ivars untouched.
        let added = unsafe {
            ffi::class_addMethod(
                class as *const AnyClass as *mut AnyClass,
                action_selector(),
                implementation,
                c"v@:@".as_ptr(),
            )
        };
        added
            .as_bool()
            .then_some(())
            .ok_or_else(|| "could not register macOS menu command handling".to_owned())
    }

    pub(super) fn install_menu(repaint: eframe::egui::Context) -> Result<(), String> {
        *REPAINT_CONTEXT
            .lock()
            .map_err(|_| "macOS menu repaint context was poisoned".to_owned())? = Some(repaint);
        if MENU_INSTALLED.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        let result = build_menu();
        if result.is_err() {
            MENU_INSTALLED.store(false, Ordering::Release);
        }
        result
    }

    fn build_menu() -> Result<(), String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "the macOS menu must be installed on the main thread".to_owned())?;
        let application = NSApplication::sharedApplication(mtm);
        let main_menu = application
            .mainMenu()
            .ok_or_else(|| "the macOS application menu is unavailable".to_owned())?;
        let delegate = application
            .delegate()
            .ok_or_else(|| "the macOS application delegate is unavailable".to_owned())?;
        let target: &AnyObject = AsRef::<AnyObject>::as_ref(&*delegate);

        let app_menu = main_menu
            .itemAtIndex(0)
            .and_then(|item| item.submenu())
            .ok_or_else(|| "the tiptoptyp application menu is unavailable".to_owned())?;
        app_menu.setAutoenablesItems(false);
        let settings = make_item(
            mtm,
            "Settings…",
            ",",
            Modifiers::COMMAND,
            NativeMenuCommand::Application(ApplicationCommand::Settings),
            target,
        );
        app_menu.insertItem_atIndex(&settings, app_menu.numberOfItems().min(1));

        add_top_level_menu(mtm, &main_menu, "File", FILE_ITEMS, target);
        add_top_level_menu(mtm, &main_menu, "Edit", EDIT_ITEMS, target);
        add_top_level_menu(mtm, &main_menu, "View", VIEW_ITEMS, target);
        Ok(())
    }

    fn add_top_level_menu(
        mtm: MainThreadMarker,
        main_menu: &NSMenu,
        title: &str,
        specs: &[ItemSpec],
        target: &AnyObject,
    ) {
        let title = NSString::from_str(title);
        let submenu = NSMenu::new(mtm);
        submenu.setTitle(&title);
        submenu.setAutoenablesItems(false);
        for spec in specs {
            match *spec {
                ItemSpec::Command {
                    title,
                    key,
                    modifiers,
                    command,
                } => submenu.addItem(&make_item(mtm, title, key, modifiers, command, target)),
                ItemSpec::Separator => submenu.addItem(&NSMenuItem::separatorItem(mtm)),
            }
        }
        let root = NSMenuItem::new(mtm);
        root.setTitle(&title);
        root.setSubmenu(Some(&submenu));
        main_menu.addItem(&root);
    }

    fn make_item(
        mtm: MainThreadMarker,
        title: &str,
        key: &str,
        modifiers: Modifiers,
        command: NativeMenuCommand,
        target: &AnyObject,
    ) -> Retained<NSMenuItem> {
        let title = NSString::from_str(title);
        let key = NSString::from_str(key);
        // SAFETY: ACTION_SELECTOR is installed on `target` before the AppKit
        // menu is created, and its signature accepts the sending menu item.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &title,
                Some(action_selector()),
                &key,
            )
        };
        item.setKeyEquivalentModifierMask(modifiers.appkit());
        item.setTag(command_tag(command));
        item.setEnabled(true);
        // SAFETY: `target` is the retained NSApplication delegate, which
        // outlives every menu item attached to the application menu.
        unsafe { item.setTarget(Some(target)) };
        item
    }

    const fn command_tag(command: NativeMenuCommand) -> NSInteger {
        match command {
            NativeMenuCommand::Application(ApplicationCommand::Settings) => 1,
            NativeMenuCommand::File(FileCommand::New) => 100,
            NativeMenuCommand::File(FileCommand::Open) => 101,
            NativeMenuCommand::File(FileCommand::ChangeWorkspaceRoot) => 102,
            NativeMenuCommand::File(FileCommand::Save) => 103,
            NativeMenuCommand::File(FileCommand::SaveAs) => 104,
            NativeMenuCommand::File(FileCommand::ExportPdf) => 105,
            NativeMenuCommand::File(FileCommand::NewWindow) => 106,
            NativeMenuCommand::File(FileCommand::OpenInNewWindow) => 107,
            NativeMenuCommand::Edit(EditCommand::Undo) => 200,
            NativeMenuCommand::Edit(EditCommand::Redo) => 201,
            NativeMenuCommand::Edit(EditCommand::Cut) => 202,
            NativeMenuCommand::Edit(EditCommand::Copy) => 203,
            NativeMenuCommand::Edit(EditCommand::Paste) => 204,
            NativeMenuCommand::Edit(EditCommand::SelectAll) => 205,
            NativeMenuCommand::Edit(EditCommand::Find) => 206,
            NativeMenuCommand::Edit(EditCommand::FindReplace) => 207,
            NativeMenuCommand::Edit(EditCommand::Format) => 208,
            NativeMenuCommand::View(ViewCommand::Problems) => 300,
            NativeMenuCommand::View(ViewCommand::Explorer) => 301,
            NativeMenuCommand::View(ViewCommand::Code) => 302,
            NativeMenuCommand::View(ViewCommand::Split) => 303,
            NativeMenuCommand::View(ViewCommand::Preview) => 304,
        }
    }

    const fn command_from_tag(tag: NSInteger) -> Option<NativeMenuCommand> {
        Some(match tag {
            1 => NativeMenuCommand::Application(ApplicationCommand::Settings),
            100 => NativeMenuCommand::File(FileCommand::New),
            101 => NativeMenuCommand::File(FileCommand::Open),
            102 => NativeMenuCommand::File(FileCommand::ChangeWorkspaceRoot),
            103 => NativeMenuCommand::File(FileCommand::Save),
            104 => NativeMenuCommand::File(FileCommand::SaveAs),
            105 => NativeMenuCommand::File(FileCommand::ExportPdf),
            106 => NativeMenuCommand::File(FileCommand::NewWindow),
            107 => NativeMenuCommand::File(FileCommand::OpenInNewWindow),
            200 => NativeMenuCommand::Edit(EditCommand::Undo),
            201 => NativeMenuCommand::Edit(EditCommand::Redo),
            202 => NativeMenuCommand::Edit(EditCommand::Cut),
            203 => NativeMenuCommand::Edit(EditCommand::Copy),
            204 => NativeMenuCommand::Edit(EditCommand::Paste),
            205 => NativeMenuCommand::Edit(EditCommand::SelectAll),
            206 => NativeMenuCommand::Edit(EditCommand::Find),
            207 => NativeMenuCommand::Edit(EditCommand::FindReplace),
            208 => NativeMenuCommand::Edit(EditCommand::Format),
            300 => NativeMenuCommand::View(ViewCommand::Problems),
            301 => NativeMenuCommand::View(ViewCommand::Explorer),
            302 => NativeMenuCommand::View(ViewCommand::Code),
            303 => NativeMenuCommand::View(ViewCommand::Split),
            304 => NativeMenuCommand::View(ViewCommand::Preview),
            _ => return None,
        })
    }

    unsafe extern "C-unwind" fn perform_menu_command(
        _delegate: &AnyObject,
        _selector: Sel,
        sender: &NSMenuItem,
    ) {
        // Never allow a Rust panic to unwind into AppKit.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let Some(command) = command_from_tag(sender.tag()) else {
                return;
            };
            if let Some(sender) = COMMAND_SENDER.lock().ok().and_then(|sender| sender.clone()) {
                let _ = sender.send(command);
            }
            if let Some(context) = REPAINT_CONTEXT
                .lock()
                .ok()
                .and_then(|context| context.clone())
            {
                context.request_repaint();
            }
        }));
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_native_item_tag_round_trips_to_its_typed_command() {
            for spec in FILE_ITEMS.iter().chain(EDIT_ITEMS).chain(VIEW_ITEMS) {
                if let ItemSpec::Command { command, .. } = spec {
                    assert_eq!(command_from_tag(command_tag(*command)), Some(*command));
                }
            }
            let settings = NativeMenuCommand::Application(ApplicationCommand::Settings);
            assert_eq!(command_from_tag(command_tag(settings)), Some(settings));
            assert_eq!(command_from_tag(-1), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receiver_drains_typed_commands_in_order() {
        let (sender, receiver) = channel();
        sender
            .send(NativeMenuCommand::Application(ApplicationCommand::Settings))
            .unwrap();
        sender
            .send(NativeMenuCommand::View(ViewCommand::Split))
            .unwrap();
        assert_eq!(
            receiver.pending().collect::<Vec<_>>(),
            [
                NativeMenuCommand::Application(ApplicationCommand::Settings),
                NativeMenuCommand::View(ViewCommand::Split),
            ]
        );
        assert!(receiver.pending().next().is_none());
    }

    #[test]
    fn assigned_window_queue_preserves_command_order() {
        let mut queue = NativeMenuCommandQueue::default();
        queue.push(NativeMenuCommand::Edit(EditCommand::Undo));
        queue.extend([
            NativeMenuCommand::Edit(EditCommand::Copy),
            NativeMenuCommand::View(ViewCommand::Preview),
        ]);

        assert_eq!(
            [queue.pop(), queue.pop(), queue.pop()],
            [
                Some(NativeMenuCommand::Edit(EditCommand::Undo)),
                Some(NativeMenuCommand::Edit(EditCommand::Copy)),
                Some(NativeMenuCommand::View(ViewCommand::Preview)),
            ]
        );
        assert_eq!(queue.pop(), None);
    }
}
