//! macOS: the menu bar Glossa shows while it is an ordinary app (a full
//! window is open; see [`crate::window::set_in_app_switcher`]).
//!
//! winit's default menu has only the application menu, with a Quit that
//! ends the process on the spot. This one has what a Mac user reaches for:
//!
//! - **Glossa**: About, Settings… (Cmd+,), Hide (Cmd+H), Hide Others, Show
//!   All, Quit (Cmd+Q). About, Settings and Quit come back to the program as
//!   [`MenuCommand`]s, so quitting saves the settings like the tray's Exit.
//! - **Edit**: Undo, Redo, Cut, Copy, Paste, Select All. egui reads these
//!   shortcuts from key presses, not from menu actions, so each item hands
//!   the key press to the focused window: the very event when the item was
//!   chosen with its shortcut, a synthetic one when it was clicked.
//! - **Window**: Minimize (Cmd+M), Close Window (Cmd+W), and the list of
//!   open windows macOS maintains.
//!
//! Elsewhere [`install`] does nothing.

/// A menu item the program acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommand {
    About,
    Settings,
    /// Close the focused window (Cmd+W).
    CloseWindow,
    Quit,
}

/// The items' titles, in the interface language.
#[derive(Debug, Clone, Default)]
pub struct MenuLabels {
    pub about: String,
    pub settings: String,
    pub hide: String,
    pub hide_others: String,
    pub show_all: String,
    pub quit: String,
    pub edit: String,
    pub undo: String,
    pub redo: String,
    pub cut: String,
    pub copy: String,
    pub paste: String,
    pub select_all: String,
    pub window: String,
    pub minimize: String,
    pub close: String,
}

/// Install the menu bar, replacing whatever was there (winit's default, or
/// an earlier call's). `on_command` runs on the main thread from inside the
/// event loop: hand the command on and wake the UI. Must be called on the
/// main thread; elsewhere, and on other systems, it does nothing.
pub fn install(labels: &MenuLabels, on_command: impl Fn(MenuCommand) + 'static) {
    #[cfg(target_os = "macos")]
    imp::install(labels, Box::new(on_command));
    #[cfg(not(target_os = "macos"))]
    let _ = (labels, on_command);
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem};
    use objc2_foundation::{NSPoint, NSString};

    use super::{MenuCommand, MenuLabels};

    type Sink = Box<dyn Fn(MenuCommand)>;

    /// The Edit items by tag: key equivalent, with Shift, and the virtual
    /// key code (ANSI layout) of the key press handed to the window.
    const EDIT_KEYS: [(&str, bool, u16); 6] =
        [("z", false, 6), ("z", true, 6), ("x", false, 7), ("c", false, 8), ("v", false, 9), ("a", false, 0)];

    const TAG_ABOUT: isize = 1;
    const TAG_SETTINGS: isize = 2;
    const TAG_CLOSE: isize = 3;
    const TAG_QUIT: isize = 4;

    pub struct TargetIvars {
        sink: Sink,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and the class
        // implements no `Drop`.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "GlossaMenuTarget"]
        #[ivars = TargetIvars]
        struct MenuTarget;

        impl MenuTarget {
            #[unsafe(method(glossaCommand:))]
            fn command(&self, sender: Option<&NSMenuItem>) {
                let command = match sender.map(|s| s.tag()) {
                    Some(TAG_ABOUT) => MenuCommand::About,
                    Some(TAG_SETTINGS) => MenuCommand::Settings,
                    Some(TAG_CLOSE) => MenuCommand::CloseWindow,
                    Some(TAG_QUIT) => MenuCommand::Quit,
                    _ => return,
                };
                (self.ivars().sink)(command);
            }

            #[unsafe(method(glossaEdit:))]
            fn edit(&self, sender: Option<&NSMenuItem>) {
                let Some(&key) = sender.and_then(|s| usize::try_from(s.tag()).ok()).and_then(|t| EDIT_KEYS.get(t))
                else {
                    return;
                };
                press_in_key_window(self.mtm(), key);
            }
        }

        unsafe impl NSObjectProtocol for MenuTarget {}
    );

    impl MenuTarget {
        fn new(mtm: MainThreadMarker, sink: Sink) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(TargetIvars { sink });
            // SAFETY: NSObject's designated initialiser.
            unsafe { msg_send![super(this), init] }
        }
    }

    thread_local! {
        // Menu items hold their target weakly; this keeps it alive.
        static TARGET: RefCell<Option<Retained<MenuTarget>>> = const { RefCell::new(None) };
    }

    /// Give the key window the key press behind an Edit item.
    fn press_in_key_window(mtm: MainThreadMarker, (key, shift, code): (&str, bool, u16)) {
        let app = NSApplication::sharedApplication(mtm);
        let Some(window) = app.keyWindow() else { return };
        let mut flags = NSEventModifierFlags::Command;
        if shift {
            flags |= NSEventModifierFlags::Shift;
        }
        // Chosen with its shortcut: pass that very event on, so the window
        // sees exactly what was pressed. (It was not, when the menu was
        // worked from the keyboard and Return chose the item.)
        if let Some(event) = app.currentEvent() {
            let pressed = event.r#type() == NSEventType::KeyDown
                && event.modifierFlags().contains(NSEventModifierFlags::Command)
                && event.charactersIgnoringModifiers().is_some_and(|c| c.to_string().eq_ignore_ascii_case(key));
            if pressed {
                window.sendEvent(&event);
                return;
            }
        }
        // Clicked: press the key for it.
        let chars = NSString::from_str(&if shift { key.to_ascii_uppercase() } else { key.to_owned() });
        for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                kind,
                NSPoint::new(0.0, 0.0),
                flags,
                0.0,
                window.windowNumber(),
                None,
                &chars,
                &chars,
                false,
                code,
            );
            if let Some(event) = event {
                window.sendEvent(&event);
            }
        }
    }

    fn item(
        mtm: MainThreadMarker,
        title: &str,
        action: Sel,
        key: &str,
        modifiers: NSEventModifierFlags,
        target: Option<(&MenuTarget, isize)>,
    ) -> Retained<NSMenuItem> {
        // SAFETY: `action` is one of AppKit's standard actions (sent along
        // the responder chain) or a method of `target`.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        if !key.is_empty() {
            item.setKeyEquivalentModifierMask(modifiers);
        }
        if let Some((target, tag)) = target {
            let target: &AnyObject = target;
            // SAFETY: the target implements `action`, and is kept alive in
            // `TARGET` for as long as the menu uses it.
            unsafe { item.setTarget(Some(target)) };
            item.setTag(tag);
        }
        item
    }

    fn submenu(mtm: MainThreadMarker, bar: &NSMenu, title: &str, items: Vec<Retained<NSMenuItem>>) -> Retained<NSMenu> {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
        for i in &items {
            menu.addItem(i);
        }
        let holder = NSMenuItem::new(mtm);
        holder.setSubmenu(Some(&menu));
        bar.addItem(&holder);
        menu
    }

    pub fn install(l: &MenuLabels, sink: Sink) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let target = MenuTarget::new(mtm, sink);
        let t = &*target;
        let cmd = NSEventModifierFlags::Command;
        let none = NSEventModifierFlags::empty();
        let sep = || NSMenuItem::separatorItem(mtm);
        let bar = NSMenu::new(mtm);

        // The first menu's title is replaced by the app's name.
        submenu(
            mtm,
            &bar,
            "Glossa",
            vec![
                item(mtm, &l.about, sel!(glossaCommand:), "", none, Some((t, TAG_ABOUT))),
                sep(),
                item(mtm, &l.settings, sel!(glossaCommand:), ",", cmd, Some((t, TAG_SETTINGS))),
                sep(),
                item(mtm, &l.hide, sel!(hide:), "h", cmd, None),
                item(mtm, &l.hide_others, sel!(hideOtherApplications:), "h", cmd | NSEventModifierFlags::Option, None),
                item(mtm, &l.show_all, sel!(unhideAllApplications:), "", none, None),
                sep(),
                item(mtm, &l.quit, sel!(glossaCommand:), "q", cmd, Some((t, TAG_QUIT))),
            ],
        );
        let edit = |title: &str, tag: usize| {
            let (key, shift, _) = EDIT_KEYS[tag];
            let modifiers = if shift { cmd | NSEventModifierFlags::Shift } else { cmd };
            item(mtm, title, sel!(glossaEdit:), key, modifiers, Some((t, tag as isize)))
        };
        submenu(
            mtm,
            &bar,
            &l.edit,
            vec![
                edit(&l.undo, 0),
                edit(&l.redo, 1),
                sep(),
                edit(&l.cut, 2),
                edit(&l.copy, 3),
                edit(&l.paste, 4),
                edit(&l.select_all, 5),
            ],
        );
        let window = submenu(
            mtm,
            &bar,
            &l.window,
            vec![
                item(mtm, &l.minimize, sel!(performMiniaturize:), "m", cmd, None),
                item(mtm, &l.close, sel!(glossaCommand:), "w", cmd, Some((t, TAG_CLOSE))),
            ],
        );

        let app = NSApplication::sharedApplication(mtm);
        app.setMainMenu(Some(&bar));
        app.setWindowsMenu(Some(&window));
        TARGET.with(|slot| *slot.borrow_mut() = Some(target));
        tracing::debug!("app menu installed");
    }
}
