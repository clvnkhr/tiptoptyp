use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver, Sender, TryIter},
};

use eframe::egui::{self, KeyboardShortcut, Modifiers};

/// Every application action which can be invoked without a dynamic payload.
/// Native menus, egui menus and keyboard routing all consume `CommandSpec`
/// entries for this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AppCommand {
    Settings,
    New,
    NewWindow,
    Open,
    OpenInNewWindow,
    ChangeWorkspaceRoot,
    Save,
    SaveAs,
    ExportPdf,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    ToggleComment,
    Find,
    FindReplace,
    Format,
    SyncPreview,
    Problems,
    Explorer,
    Code,
    Split,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandMenu {
    Application,
    File,
    Edit,
    View,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandRequirement {
    Always,
    Undo,
    Redo,
    TypstDocument,
    InteractivePreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chord {
    key: &'static str,
    primary: bool,
    control: bool,
    shift: bool,
    alt: bool,
}

impl Chord {
    const fn primary(key: &'static str) -> Self {
        Self {
            key,
            primary: true,
            control: false,
            shift: false,
            alt: false,
        }
    }

    const fn shift(mut self) -> Self {
        self.shift = true;
        self
    }

    const fn alt(mut self) -> Self {
        self.alt = true;
        self
    }

    const fn control(key: &'static str) -> Self {
        Self {
            key,
            primary: false,
            control: true,
            shift: false,
            alt: false,
        }
    }

    fn egui(self) -> KeyboardShortcut {
        let mut modifiers = Modifiers::NONE;
        if self.primary {
            modifiers |= Modifiers::COMMAND;
        }
        if self.control {
            modifiers |= Modifiers::CTRL;
        }
        if self.shift {
            modifiers |= Modifiers::SHIFT;
        }
        if self.alt {
            modifiers |= Modifiers::ALT;
        }
        KeyboardShortcut::new(modifiers, egui_key(self.key))
    }

    fn specificity(self) -> u8 {
        u8::from(self.primary) + u8::from(self.control) + u8::from(self.shift) + u8::from(self.alt)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandShortcut {
    macos: Chord,
    other: Chord,
}

impl CommandShortcut {
    const fn same(chord: Chord) -> Self {
        Self {
            macos: chord,
            other: chord,
        }
    }

    fn platform(self) -> Chord {
        if cfg!(target_os = "macos") {
            self.macos
        } else {
            self.other
        }
    }

    pub(crate) fn egui(self) -> KeyboardShortcut {
        self.platform().egui()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandSpec {
    pub(crate) command: AppCommand,
    pub(crate) title: &'static str,
    pub(crate) popup_title: &'static str,
    pub(crate) menu: CommandMenu,
    pub(crate) popup_section: Option<u8>,
    pub(crate) requirement: CommandRequirement,
    pub(crate) shortcut: Option<CommandShortcut>,
    native_id: Option<isize>,
    native_section: u8,
}

macro_rules! spec {
    ($command:ident, $title:literal, $popup:literal, $menu:ident, $section:expr, $requirement:ident, $shortcut:expr, $native_id:expr) => {
        CommandSpec {
            command: AppCommand::$command,
            title: $title,
            popup_title: $popup,
            menu: CommandMenu::$menu,
            popup_section: Some($section),
            requirement: CommandRequirement::$requirement,
            shortcut: $shortcut,
            native_id: $native_id,
            native_section: $section,
        }
    };
}

pub(crate) const COMMAND_SPECS: &[CommandSpec] = &[
    spec!(
        Settings,
        "Settings…",
        "Settings…",
        Application,
        0,
        Always,
        Some(CommandShortcut::same(Chord::primary(","))),
        Some(1)
    ),
    spec!(
        New,
        "New",
        "New",
        File,
        0,
        Always,
        Some(CommandShortcut::same(Chord::primary("n"))),
        Some(100)
    ),
    spec!(
        NewWindow,
        "New Window",
        "New Window",
        File,
        0,
        Always,
        Some(CommandShortcut::same(Chord::primary("n").shift())),
        Some(106)
    ),
    spec!(
        Open,
        "Open…",
        "Open…",
        File,
        0,
        Always,
        Some(CommandShortcut::same(Chord::primary("o"))),
        Some(101)
    ),
    spec!(
        OpenInNewWindow,
        "Open in New Window…",
        "Open in New Window…",
        File,
        0,
        Always,
        Some(CommandShortcut::same(Chord::primary("o").alt())),
        Some(107)
    ),
    spec!(
        ChangeWorkspaceRoot,
        "Change Workspace Root…",
        "Change Workspace Root…",
        File,
        0,
        Always,
        Some(CommandShortcut::same(Chord::primary("o").shift())),
        Some(102)
    ),
    spec!(
        Save,
        "Save",
        "Save",
        File,
        1,
        Always,
        Some(CommandShortcut::same(Chord::primary("s"))),
        Some(103)
    ),
    spec!(
        SaveAs,
        "Save As…",
        "Save As…",
        File,
        1,
        Always,
        Some(CommandShortcut::same(Chord::primary("s").shift())),
        Some(104)
    ),
    spec!(
        ExportPdf,
        "Export PDF…",
        "Export PDF…",
        File,
        2,
        TypstDocument,
        Some(CommandShortcut::same(Chord::primary("e").shift())),
        Some(105)
    ),
    spec!(
        Undo,
        "Undo",
        "Undo",
        Edit,
        0,
        Undo,
        Some(CommandShortcut::same(Chord::primary("z"))),
        Some(200)
    ),
    spec!(
        Redo,
        "Redo",
        "Redo",
        Edit,
        0,
        Redo,
        Some(CommandShortcut::same(Chord::primary("z").shift())),
        Some(201)
    ),
    spec!(
        Cut,
        "Cut",
        "Cut",
        Edit,
        1,
        Always,
        Some(CommandShortcut::same(Chord::primary("x"))),
        Some(202)
    ),
    spec!(
        Copy,
        "Copy",
        "Copy",
        Edit,
        1,
        Always,
        Some(CommandShortcut::same(Chord::primary("c"))),
        Some(203)
    ),
    spec!(
        Paste,
        "Paste",
        "Paste",
        Edit,
        1,
        Always,
        Some(CommandShortcut::same(Chord::primary("v"))),
        Some(204)
    ),
    spec!(
        SelectAll,
        "Select All",
        "Select All",
        Edit,
        1,
        Always,
        Some(CommandShortcut::same(Chord::primary("a"))),
        Some(205)
    ),
    spec!(
        ToggleComment,
        "Toggle Comment",
        "Toggle Comment",
        Edit,
        1,
        Always,
        Some(CommandShortcut::same(Chord::primary("/"))),
        None
    ),
    spec!(
        Find,
        "Find…",
        "Find…",
        Edit,
        2,
        Always,
        Some(CommandShortcut::same(Chord::primary("f"))),
        Some(206)
    ),
    CommandSpec {
        shortcut: Some(CommandShortcut {
            macos: Chord::primary("f").alt(),
            other: Chord::control("h"),
        }),
        ..spec!(
            FindReplace,
            "Find and Replace…",
            "Find and Replace…",
            Edit,
            2,
            Always,
            None,
            Some(207)
        )
    },
    CommandSpec {
        shortcut: Some(CommandShortcut::same(Chord {
            key: "f",
            primary: false,
            control: false,
            shift: true,
            alt: true,
        })),
        ..spec!(
            Format,
            "Format Document",
            "Format Document",
            Edit,
            3,
            TypstDocument,
            None,
            Some(208)
        )
    },
    CommandSpec {
        popup_section: None,
        ..spec!(
            SyncPreview,
            "Sync Preview",
            "Sync Preview",
            Edit,
            3,
            InteractivePreview,
            None,
            None
        )
    },
    spec!(
        Problems,
        "Problems",
        "Toggle Problems",
        View,
        0,
        Always,
        Some(CommandShortcut::same(Chord::primary("5"))),
        Some(300)
    ),
    spec!(
        Explorer,
        "Explorer",
        "Toggle Explorer",
        View,
        0,
        Always,
        Some(CommandShortcut::same(Chord::primary("1"))),
        Some(301)
    ),
    spec!(
        Code,
        "Code",
        "Code",
        View,
        1,
        Always,
        Some(CommandShortcut::same(Chord::primary("2"))),
        Some(302)
    ),
    spec!(
        Split,
        "Split",
        "Split",
        View,
        1,
        Always,
        Some(CommandShortcut::same(Chord::primary("3"))),
        Some(303)
    ),
    spec!(
        Preview,
        "Preview",
        "Preview",
        View,
        1,
        Always,
        Some(CommandShortcut::same(Chord::primary("4"))),
        Some(304)
    ),
];

pub(crate) fn command_spec(command: AppCommand) -> &'static CommandSpec {
    COMMAND_SPECS
        .iter()
        .find(|spec| spec.command == command)
        .expect("every AppCommand has one descriptor")
}

pub(crate) fn command_specs(menu: CommandMenu) -> impl Iterator<Item = &'static CommandSpec> {
    COMMAND_SPECS.iter().filter(move |spec| spec.menu == menu)
}

/// Consume the most-specific matching chord first. egui deliberately permits
/// extra modifiers, so this ordering keeps Shift/Alt variants ahead of their
/// base commands without duplicating a hand-maintained shortcut list.
pub(crate) fn consume_shortcut(
    input: &mut egui::InputState,
    accepts: impl Fn(AppCommand) -> bool,
) -> Option<AppCommand> {
    for specificity in (0..=4).rev() {
        for spec in COMMAND_SPECS {
            let Some(shortcut) = spec.shortcut else {
                continue;
            };
            let chord = shortcut.platform();
            if chord.specificity() == specificity
                && accepts(spec.command)
                && input.consume_shortcut(&chord.egui())
            {
                return Some(spec.command);
            }
        }
    }
    None
}

fn egui_key(key: &str) -> egui::Key {
    match key {
        "," => egui::Key::Comma,
        "/" => egui::Key::Slash,
        "1" => egui::Key::Num1,
        "2" => egui::Key::Num2,
        "3" => egui::Key::Num3,
        "4" => egui::Key::Num4,
        "5" => egui::Key::Num5,
        "a" => egui::Key::A,
        "c" => egui::Key::C,
        "e" => egui::Key::E,
        "f" => egui::Key::F,
        "h" => egui::Key::H,
        "n" => egui::Key::N,
        "o" => egui::Key::O,
        "s" => egui::Key::S,
        "v" => egui::Key::V,
        "x" => egui::Key::X,
        "z" => egui::Key::Z,
        _ => unreachable!("command registry contains an unsupported key"),
    }
}

#[derive(Debug, Default)]
pub(crate) struct NativeMenuCommandQueue {
    pending: VecDeque<AppCommand>,
}

impl NativeMenuCommandQueue {
    pub(crate) fn push(&mut self, command: AppCommand) {
        self.pending.push_back(command);
    }

    pub(crate) fn pop(&mut self) -> Option<AppCommand> {
        self.pending.pop_front()
    }
}

pub(crate) struct NativeMenuReceiver {
    receiver: Receiver<AppCommand>,
}

impl NativeMenuReceiver {
    pub(crate) fn pending(&self) -> TryIter<'_, AppCommand> {
        self.receiver.try_iter()
    }
}

pub(crate) fn channel() -> (Sender<AppCommand>, NativeMenuReceiver) {
    let (sender, receiver) = mpsc::channel();
    (sender, NativeMenuReceiver { receiver })
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_handler(sender: Sender<AppCommand>) -> Result<(), String> {
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

    use super::{AppCommand, Chord, CommandMenu, command_spec, command_specs};

    static COMMAND_SENDER: Mutex<Option<Sender<AppCommand>>> = Mutex::new(None);
    static REPAINT_CONTEXT: Mutex<Option<eframe::egui::Context>> = Mutex::new(None);
    static MENU_INSTALLED: AtomicBool = AtomicBool::new(false);

    fn action_selector() -> Sel {
        sel!(tiptoptypPerformMenuCommand:)
    }

    fn appkit_modifiers(chord: Chord) -> NSEventModifierFlags {
        let mut flags = NSEventModifierFlags::empty();
        if chord.primary {
            flags |= NSEventModifierFlags::Command;
        }
        if chord.control {
            flags |= NSEventModifierFlags::Control;
        }
        if chord.shift {
            flags |= NSEventModifierFlags::Shift;
        }
        if chord.alt {
            flags |= NSEventModifierFlags::Option;
        }
        flags
    }

    pub(super) fn install_handler(sender: Sender<AppCommand>) -> Result<(), String> {
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
        // append one uniquely named action selector and leave its lifecycle
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
        let settings = make_item(mtm, command_spec(AppCommand::Settings), target);
        app_menu.insertItem_atIndex(&settings, app_menu.numberOfItems().min(1));

        add_top_level_menu(mtm, &main_menu, "File", CommandMenu::File, target);
        add_top_level_menu(mtm, &main_menu, "Edit", CommandMenu::Edit, target);
        add_top_level_menu(mtm, &main_menu, "View", CommandMenu::View, target);
        Ok(())
    }

    fn add_top_level_menu(
        mtm: MainThreadMarker,
        main_menu: &NSMenu,
        title: &str,
        menu: CommandMenu,
        target: &AnyObject,
    ) {
        let title = NSString::from_str(title);
        let submenu = NSMenu::new(mtm);
        submenu.setTitle(&title);
        submenu.setAutoenablesItems(false);
        let mut previous_section = None;
        for spec in command_specs(menu).filter(|spec| spec.native_id.is_some()) {
            if previous_section.is_some_and(|section| section != spec.native_section) {
                submenu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            submenu.addItem(&make_item(mtm, spec, target));
            previous_section = Some(spec.native_section);
        }
        let root = NSMenuItem::new(mtm);
        root.setTitle(&title);
        root.setSubmenu(Some(&submenu));
        main_menu.addItem(&root);
    }

    fn make_item(
        mtm: MainThreadMarker,
        spec: &super::CommandSpec,
        target: &AnyObject,
    ) -> Retained<NSMenuItem> {
        let shortcut = spec.shortcut.expect("native commands have shortcuts").macos;
        let title = NSString::from_str(spec.title);
        let key = NSString::from_str(shortcut.key);
        // SAFETY: the action selector is installed on `target` before menu
        // creation and accepts the sending menu item.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &title,
                Some(action_selector()),
                &key,
            )
        };
        item.setKeyEquivalentModifierMask(appkit_modifiers(shortcut));
        item.setTag(spec.native_id.expect("native command has a tag"));
        item.setEnabled(true);
        // SAFETY: `target` is the retained NSApplication delegate and outlives
        // every menu item attached to the application menu.
        unsafe { item.setTarget(Some(target)) };
        item
    }

    fn command_from_tag(tag: NSInteger) -> Option<AppCommand> {
        super::COMMAND_SPECS
            .iter()
            .find(|spec| spec.native_id == Some(tag))
            .map(|spec| spec.command)
    }

    unsafe extern "C-unwind" fn perform_menu_command(
        _delegate: &AnyObject,
        _selector: Sel,
        sender: &NSMenuItem,
    ) {
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
        fn every_native_descriptor_tag_round_trips() {
            for spec in super::super::COMMAND_SPECS
                .iter()
                .filter(|spec| spec.native_id.is_some())
            {
                assert_eq!(
                    command_from_tag(spec.native_id.unwrap()),
                    Some(spec.command)
                );
            }
            assert_eq!(command_from_tag(-1), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_exactly_one_descriptor() {
        for (index, spec) in COMMAND_SPECS.iter().enumerate() {
            assert!(
                !COMMAND_SPECS[..index]
                    .iter()
                    .any(|other| other.command == spec.command)
            );
            assert_eq!(command_spec(spec.command).command, spec.command);
        }
    }

    #[test]
    fn receiver_and_window_queue_preserve_command_order() {
        let (sender, receiver) = channel();
        sender.send(AppCommand::Settings).unwrap();
        sender.send(AppCommand::Split).unwrap();
        assert_eq!(
            receiver.pending().collect::<Vec<_>>(),
            [AppCommand::Settings, AppCommand::Split]
        );

        let mut queue = NativeMenuCommandQueue::default();
        queue.push(AppCommand::Undo);
        queue.push(AppCommand::Copy);
        queue.push(AppCommand::Preview);
        assert_eq!(
            [queue.pop(), queue.pop(), queue.pop()],
            [
                Some(AppCommand::Undo),
                Some(AppCommand::Copy),
                Some(AppCommand::Preview),
            ]
        );
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn view_descriptors_advertise_the_expected_number_shortcuts() {
        let shortcuts = command_specs(CommandMenu::View)
            .map(|spec| (spec.command, spec.shortcut.unwrap().platform().key))
            .collect::<Vec<_>>();
        assert_eq!(
            shortcuts,
            [
                (AppCommand::Problems, "5"),
                (AppCommand::Explorer, "1"),
                (AppCommand::Code, "2"),
                (AppCommand::Split, "3"),
                (AppCommand::Preview, "4"),
            ]
        );
    }
}
