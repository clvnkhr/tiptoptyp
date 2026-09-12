use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver, Sender, TryIter},
};

use eframe::egui;

use crate::shortcuts::{ShortcutAction, ShortcutBindings, ShortcutChord};

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
    TypstPreview,
    InteractivePreview,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandSpec {
    pub(crate) command: AppCommand,
    pub(crate) title: &'static str,
    pub(crate) popup_title: &'static str,
    pub(crate) menu: CommandMenu,
    pub(crate) popup_section: Option<u8>,
    pub(crate) requirement: CommandRequirement,
    pub(crate) shortcut_action: ShortcutAction,
    native_id: Option<isize>,
    native_section: u8,
}

macro_rules! spec {
    ($command:ident, $title:literal, $popup:literal, $menu:ident, $section:expr, $requirement:ident, $native_id:expr) => {
        CommandSpec {
            command: AppCommand::$command,
            title: $title,
            popup_title: $popup,
            menu: CommandMenu::$menu,
            popup_section: Some($section),
            requirement: CommandRequirement::$requirement,
            shortcut_action: ShortcutAction::$command,
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
        Some(1)
    ),
    spec!(New, "New", "New", File, 0, Always, Some(100)),
    spec!(
        NewWindow,
        "New Window",
        "New Window",
        File,
        0,
        Always,
        Some(106)
    ),
    spec!(Open, "Open…", "Open…", File, 0, Always, Some(101)),
    spec!(
        OpenInNewWindow,
        "Open in New Window…",
        "Open in New Window…",
        File,
        0,
        Always,
        Some(107)
    ),
    spec!(
        ChangeWorkspaceRoot,
        "Change Workspace Root…",
        "Change Workspace Root…",
        File,
        0,
        Always,
        Some(102)
    ),
    spec!(Save, "Save", "Save", File, 1, Always, Some(103)),
    spec!(SaveAs, "Save As…", "Save As…", File, 1, Always, Some(104)),
    spec!(
        ExportPdf,
        "Export PDF…",
        "Export PDF…",
        File,
        2,
        TypstPreview,
        Some(105)
    ),
    spec!(Undo, "Undo", "Undo", Edit, 0, Undo, Some(200)),
    spec!(Redo, "Redo", "Redo", Edit, 0, Redo, Some(201)),
    spec!(Cut, "Cut", "Cut", Edit, 1, Always, Some(202)),
    spec!(Copy, "Copy", "Copy", Edit, 1, Always, Some(203)),
    spec!(Paste, "Paste", "Paste", Edit, 1, Always, Some(204)),
    spec!(
        SelectAll,
        "Select All",
        "Select All",
        Edit,
        1,
        Always,
        Some(205)
    ),
    spec!(
        ToggleComment,
        "Toggle Comment",
        "Toggle Comment",
        Edit,
        1,
        Always,
        None
    ),
    spec!(Find, "Find…", "Find…", Edit, 2, Always, Some(206)),
    spec!(
        FindReplace,
        "Find and Replace…",
        "Find and Replace…",
        Edit,
        2,
        Always,
        Some(207)
    ),
    spec!(
        Format,
        "Format Document",
        "Format Document",
        Edit,
        3,
        TypstDocument,
        Some(208)
    ),
    CommandSpec {
        popup_section: None,
        ..spec!(
            SyncPreview,
            "Sync Preview",
            "Sync Preview",
            Edit,
            3,
            InteractivePreview,
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
        Some(300)
    ),
    spec!(
        Explorer,
        "Explorer",
        "Toggle Explorer",
        View,
        0,
        Always,
        Some(301)
    ),
    spec!(Code, "Code", "Code", View, 1, Always, Some(302)),
    spec!(Split, "Split", "Split", View, 1, Always, Some(303)),
    spec!(Preview, "Preview", "Preview", View, 1, Always, Some(304)),
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
    bindings: &ShortcutBindings,
    accepts: impl Fn(AppCommand) -> bool,
) -> Option<AppCommand> {
    for specificity in (0..=4).rev() {
        for spec in COMMAND_SPECS {
            let Some(chord) = bindings.binding(spec.shortcut_action) else {
                continue;
            };
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
    receiver: Receiver<NativeMenuRequest>,
}

impl NativeMenuReceiver {
    pub(crate) fn pending(&self) -> TryIter<'_, NativeMenuRequest> {
        self.receiver.try_iter()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeMenuRequest {
    Command(AppCommand),
    Quit,
}

#[cfg(any(target_os = "macos", test))]
const NATIVE_QUIT_ID: isize = 900;

#[cfg(any(target_os = "macos", test))]
fn native_menu_request_from_tag(tag: isize) -> Option<NativeMenuRequest> {
    if tag == NATIVE_QUIT_ID {
        return Some(NativeMenuRequest::Quit);
    }
    COMMAND_SPECS
        .iter()
        .find(|spec| spec.native_id == Some(tag))
        .map(|spec| NativeMenuRequest::Command(spec.command))
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeApplicationItemRoute {
    Preserve,
    QuitThroughApp,
}

#[cfg(any(target_os = "macos", test))]
fn native_application_item_route(action: Option<&str>) -> NativeApplicationItemRoute {
    if action == Some("terminate:") {
        NativeApplicationItemRoute::QuitThroughApp
    } else {
        NativeApplicationItemRoute::Preserve
    }
}

pub(crate) fn channel() -> (Sender<NativeMenuRequest>, NativeMenuReceiver) {
    let (sender, receiver) = mpsc::channel();
    (sender, NativeMenuReceiver { receiver })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeMenuItemState {
    native_id: isize,
    command: AppCommand,
    binding: Option<ShortcutChord>,
    enabled: bool,
}

fn native_menu_item_states(
    bindings: &ShortcutBindings,
    mut enabled: impl FnMut(AppCommand) -> bool,
) -> Vec<NativeMenuItemState> {
    COMMAND_SPECS
        .iter()
        .filter_map(|spec| {
            Some(NativeMenuItemState {
                native_id: spec.native_id?,
                command: spec.command,
                binding: bindings.binding(spec.shortcut_action),
                enabled: enabled(spec.command),
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_handler(sender: Sender<NativeMenuRequest>) -> Result<(), String> {
    macos::install_handler(sender)
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_menu(
    repaint: eframe::egui::Context,
    bindings: &ShortcutBindings,
) -> Result<(), String> {
    let states = native_menu_item_states(bindings, |_| true);
    macos::install_menu(repaint, &states)
}

/// Synchronize both shortcut equivalents and availability on native menu
/// items that were installed at startup.
#[cfg(target_os = "macos")]
pub(crate) fn update_macos_menu(
    bindings: &ShortcutBindings,
    enabled: impl FnMut(AppCommand) -> bool,
) -> Result<(), String> {
    let states = native_menu_item_states(bindings, enabled);
    macos::update_menu(&states)
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

    use super::{
        AppCommand, CommandMenu, NATIVE_QUIT_ID, NativeApplicationItemRoute, NativeMenuItemState,
        NativeMenuRequest, ShortcutChord, command_spec, command_specs,
        native_application_item_route, native_menu_request_from_tag,
    };

    static COMMAND_SENDER: Mutex<Option<Sender<NativeMenuRequest>>> = Mutex::new(None);
    static REPAINT_CONTEXT: Mutex<Option<eframe::egui::Context>> = Mutex::new(None);
    static MENU_INSTALLED: AtomicBool = AtomicBool::new(false);

    fn action_selector() -> Sel {
        sel!(tiptoptypPerformMenuCommand:)
    }

    fn appkit_modifiers(chord: ShortcutChord) -> NSEventModifierFlags {
        let mut flags = NSEventModifierFlags::empty();
        if chord.primary_modifier() {
            flags |= NSEventModifierFlags::Command;
        }
        if chord.control_modifier() {
            flags |= NSEventModifierFlags::Control;
        }
        if chord.shift_modifier() {
            flags |= NSEventModifierFlags::Shift;
        }
        if chord.alt_modifier() {
            flags |= NSEventModifierFlags::Option;
        }
        flags
    }

    pub(super) fn install_handler(sender: Sender<NativeMenuRequest>) -> Result<(), String> {
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

    pub(super) fn install_menu(
        repaint: eframe::egui::Context,
        states: &[NativeMenuItemState],
    ) -> Result<(), String> {
        *REPAINT_CONTEXT
            .lock()
            .map_err(|_| "macOS menu repaint context was poisoned".to_owned())? = Some(repaint);
        if MENU_INSTALLED.swap(true, Ordering::AcqRel) {
            return update_menu(states);
        }
        let result = build_menu(states);
        if result.is_err() {
            MENU_INSTALLED.store(false, Ordering::Release);
        }
        result
    }

    fn build_menu(states: &[NativeMenuItemState]) -> Result<(), String> {
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
            command_spec(AppCommand::Settings),
            state_for_command(states, AppCommand::Settings),
            target,
        );
        app_menu.insertItem_atIndex(&settings, app_menu.numberOfItems().min(1));
        reroute_default_quit_item(&app_menu, target)?;

        add_top_level_menu(mtm, &main_menu, "File", CommandMenu::File, states, target);
        add_top_level_menu(mtm, &main_menu, "Edit", CommandMenu::Edit, states, target);
        add_top_level_menu(mtm, &main_menu, "View", CommandMenu::View, states, target);
        Ok(())
    }

    fn reroute_default_quit_item(menu: &NSMenu, target: &AnyObject) -> Result<(), String> {
        let quit = (0..menu.numberOfItems())
            .filter_map(|index| menu.itemAtIndex(index))
            .find(|item| {
                native_application_item_route(
                    item.action().and_then(|action| action.name().to_str().ok()),
                ) == NativeApplicationItemRoute::QuitThroughApp
            })
            .ok_or_else(|| {
                "the macOS application menu has no default terminate action to reroute".to_owned()
            })?;
        quit.setTag(NATIVE_QUIT_ID);
        quit.setEnabled(true);
        // SAFETY: `action_selector` was installed on the retained application
        // delegate before menu construction and accepts the sending menu item.
        unsafe {
            quit.setAction(Some(action_selector()));
            quit.setTarget(Some(target));
        }
        Ok(())
    }

    fn add_top_level_menu(
        mtm: MainThreadMarker,
        main_menu: &NSMenu,
        title: &str,
        menu: CommandMenu,
        states: &[NativeMenuItemState],
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
            submenu.addItem(&make_item(
                mtm,
                spec,
                state_for_command(states, spec.command),
                target,
            ));
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
        state: NativeMenuItemState,
        target: &AnyObject,
    ) -> Retained<NSMenuItem> {
        let title = NSString::from_str(spec.title);
        let key = NSString::from_str(
            &state
                .binding
                .map(ShortcutChord::appkit_key_equivalent)
                .unwrap_or_default(),
        );
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
        apply_item_state(&item, state);
        // SAFETY: `target` is the retained NSApplication delegate and outlives
        // every menu item attached to the application menu.
        unsafe { item.setTarget(Some(target)) };
        item
    }

    fn state_for_command(
        states: &[NativeMenuItemState],
        command: AppCommand,
    ) -> NativeMenuItemState {
        states
            .iter()
            .copied()
            .find(|state| state.command == command)
            .expect("every native command has a generated state")
    }

    fn apply_item_state(item: &NSMenuItem, state: NativeMenuItemState) {
        let key = state
            .binding
            .map(ShortcutChord::appkit_key_equivalent)
            .unwrap_or_default();
        item.setKeyEquivalent(&NSString::from_str(&key));
        item.setKeyEquivalentModifierMask(
            state
                .binding
                .map(appkit_modifiers)
                .unwrap_or_else(NSEventModifierFlags::empty),
        );
        item.setTag(state.native_id);
        item.setEnabled(state.enabled);
    }

    pub(super) fn update_menu(states: &[NativeMenuItemState]) -> Result<(), String> {
        if !MENU_INSTALLED.load(Ordering::Acquire) {
            return Err("the macOS menu has not been installed".to_owned());
        }
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "the macOS menu must be updated on the main thread".to_owned())?;
        let application = NSApplication::sharedApplication(mtm);
        let main_menu = application
            .mainMenu()
            .ok_or_else(|| "the macOS application menu is unavailable".to_owned())?;
        for &state in states {
            let item = find_item_with_tag(&main_menu, state.native_id).ok_or_else(|| {
                format!(
                    "the installed macOS menu is missing command tag {}",
                    state.native_id
                )
            })?;
            apply_item_state(&item, state);
        }
        Ok(())
    }

    fn find_item_with_tag(menu: &NSMenu, tag: NSInteger) -> Option<Retained<NSMenuItem>> {
        if let Some(item) = menu.itemWithTag(tag) {
            return Some(item);
        }
        for index in 0..menu.numberOfItems() {
            if let Some(submenu) = menu.itemAtIndex(index).and_then(|item| item.submenu())
                && let Some(item) = find_item_with_tag(&submenu, tag)
            {
                return Some(item);
            }
        }
        None
    }

    unsafe extern "C-unwind" fn perform_menu_command(
        _delegate: &AnyObject,
        _selector: Sel,
        sender: &NSMenuItem,
    ) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let Some(request) = native_menu_request_from_tag(sender.tag()) else {
                return;
            };
            if let Some(sender) = COMMAND_SENDER.lock().ok().and_then(|sender| sender.clone()) {
                let _ = sender.send(request);
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
                    native_menu_request_from_tag(spec.native_id.unwrap()),
                    Some(NativeMenuRequest::Command(spec.command))
                );
            }
            assert_eq!(
                native_menu_request_from_tag(NATIVE_QUIT_ID),
                Some(NativeMenuRequest::Quit)
            );
            assert_eq!(native_menu_request_from_tag(-1), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::shortcuts::{ShortcutOverrides, ShortcutPlatform};

    fn run_shortcut(
        shortcut: egui::KeyboardShortcut,
        bindings: &ShortcutBindings,
    ) -> Option<AppCommand> {
        let context = egui::Context::default();
        let mut result = None;
        context
            .run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: shortcut.logical_key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: shortcut.modifiers,
                    }],
                    ..Default::default()
                },
                |ui| {
                    result = Some(
                        ui.ctx()
                            .input_mut(|input| consume_shortcut(input, bindings, |_| true)),
                    );
                },
            )
            .drop_without_applying_deltas();
        result.expect("shortcut resolver should run")
    }

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
        assert_eq!(
            COMMAND_SPECS
                .iter()
                .map(|spec| spec.shortcut_action)
                .collect::<BTreeSet<_>>()
                .len(),
            COMMAND_SPECS.len()
        );
    }

    #[test]
    fn receiver_and_window_queue_preserve_command_order() {
        let (sender, receiver) = channel();
        sender
            .send(NativeMenuRequest::Command(AppCommand::Settings))
            .unwrap();
        sender.send(NativeMenuRequest::Quit).unwrap();
        sender
            .send(NativeMenuRequest::Command(AppCommand::Split))
            .unwrap();
        assert_eq!(
            receiver.pending().collect::<Vec<_>>(),
            [
                NativeMenuRequest::Command(AppCommand::Settings),
                NativeMenuRequest::Quit,
                NativeMenuRequest::Command(AppCommand::Split),
            ]
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
    fn only_the_default_terminate_item_is_rerouted() {
        assert_eq!(
            native_application_item_route(Some("terminate:")),
            NativeApplicationItemRoute::QuitThroughApp
        );
        for preserved in [
            Some("hide:"),
            Some("hideOtherApplications:"),
            Some("unhideAllApplications:"),
            Some("orderFrontStandardAboutPanel:"),
            None,
        ] {
            assert_eq!(
                native_application_item_route(preserved),
                NativeApplicationItemRoute::Preserve
            );
        }
    }

    #[test]
    fn quit_has_a_distinct_native_request_tag() {
        assert!(
            COMMAND_SPECS
                .iter()
                .all(|spec| spec.native_id != Some(NATIVE_QUIT_ID))
        );
        assert_eq!(
            native_menu_request_from_tag(NATIVE_QUIT_ID),
            Some(NativeMenuRequest::Quit)
        );
        assert_eq!(
            native_menu_request_from_tag(command_spec(AppCommand::Save).native_id.unwrap()),
            Some(NativeMenuRequest::Command(AppCommand::Save))
        );
    }

    #[test]
    fn view_descriptors_advertise_the_expected_number_shortcuts() {
        let bindings = ShortcutBindings::defaults(ShortcutPlatform::Other);
        let shortcuts = command_specs(CommandMenu::View)
            .map(|spec| {
                (
                    spec.command,
                    bindings.binding(spec.shortcut_action).unwrap().key(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            shortcuts,
            [
                (AppCommand::Problems, egui::Key::Num5),
                (AppCommand::Explorer, egui::Key::Num1),
                (AppCommand::Code, egui::Key::Num2),
                (AppCommand::Split, egui::Key::Num3),
                (AppCommand::Preview, egui::Key::Num4),
            ]
        );
    }

    #[test]
    fn native_item_state_uses_effective_bindings_and_current_availability() {
        let mut overrides = ShortcutOverrides::default();
        overrides.set(
            ShortcutAction::Save,
            Some(ShortcutChord::parse("Primary+K").unwrap()),
        );
        overrides.set(ShortcutAction::Open, None);
        let bindings = ShortcutBindings::from_overrides(ShortcutPlatform::MacOs, &overrides);
        let states = native_menu_item_states(&bindings, |command| command != AppCommand::ExportPdf);

        let save = states
            .iter()
            .find(|state| state.command == AppCommand::Save)
            .unwrap();
        assert_eq!(save.native_id, 103);
        assert_eq!(
            save.binding.map(ShortcutChord::appkit_key_equivalent),
            Some("k".to_owned())
        );
        assert!(save.enabled);

        let open = states
            .iter()
            .find(|state| state.command == AppCommand::Open)
            .unwrap();
        assert_eq!(open.binding, None);
        assert!(open.enabled);

        let export = states
            .iter()
            .find(|state| state.command == AppCommand::ExportPdf)
            .unwrap();
        assert!(!export.enabled);
    }

    #[test]
    fn consumption_uses_custom_bindings_and_ignores_displaced_defaults() {
        let mut overrides = ShortcutOverrides::default();
        let chord = ShortcutChord::parse("Primary+O").unwrap();
        overrides.assign(ShortcutAction::Save, chord, ShortcutPlatform::MacOs);
        let bindings = ShortcutBindings::from_overrides(ShortcutPlatform::MacOs, &overrides);

        assert_eq!(
            run_shortcut(chord.egui(), &bindings),
            Some(AppCommand::Save)
        );
        assert_eq!(bindings.binding(ShortcutAction::Open), None);
    }
}
