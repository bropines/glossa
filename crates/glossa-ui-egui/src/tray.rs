//! The tray icon and its menu.
//!
//! A click toggles mouse mode, as the classic program did (or shows the main
//! window, if so configured); a double click shows the main window; right
//! click opens the classic menu (text recognition, dictionary, history,
//! mouse mode and global hotkeys on/off, options, about, exit), with copy
//! mode, in-place translation and the other recognition actions beside text
//! recognition. The icon is drawn in the theme's accent colour, another hue
//! while mouse mode is on, and on Windows the menu is dark or light with the
//! theme (`glossa_platform::appearance::set_menu_theme`). Events reach the
//! app through channels and wake its loop, so they work while every window
//! is hidden.
//!
//! On Linux the indicator needs GTK running on the thread that owns it, so
//! the icon lives on a thread of its own there.

use std::sync::mpsc::{self, Receiver};

use glossa_core::{Action, Locale};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::app::GlossaApp;

pub enum TrayEvent {
    ShowMain,
    /// A single left click: mouse mode, or the main window.
    Click,
    ToggleMouseMode,
    Action(Action),
    ToggleHotkeys,
    About,
}

/// Something that woke the GUI while it was unloaded.
pub enum Pending {
    Action(Action, Option<String>),
    Tray(TrayEvent),
    /// A step already done before the GUI came up (the selection, read
    /// while the windows did not exist yet).
    Task(crate::actions::Task),
}

impl Pending {
    /// Whether the GUI should come up with the main window showing.
    pub fn wants_main_window(&self) -> bool {
        matches!(self, Pending::Action(Action::ShowMainWindow, _) | Pending::Tray(TrayEvent::ShowMain))
    }
}

pub type WakeFn = std::sync::Arc<dyn Fn() + Send + Sync>;

pub struct Tray {
    events: Receiver<TrayEvent>,
    #[cfg(not(target_os = "linux"))]
    icon: tray_icon::TrayIcon,
    #[cfg(not(target_os = "linux"))]
    hotkeys_item: CheckMenuItem,
    #[cfg(not(target_os = "linux"))]
    mouse_item: CheckMenuItem,
    /// New icons for the GTK thread that owns the indicator.
    #[cfg(target_os = "linux")]
    looks: mpsc::Sender<tray_icon::Icon>,
}

/// The accent of the current theme, as the icon's base colour, and whether
/// the theme is dark, for the menu.
pub fn look_of(engine: &glossa_engine::Engine) -> ([f32; 3], bool) {
    let mut theme = engine.current_theme();
    theme.resolve_system_accent(glossa_platform::appearance::system_accent().as_ref());
    let c = theme.accent;
    ([c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0], theme.dark)
}

/// The tray icon for an accent colour; mouse mode turns the hue well away
/// from it, so the state shows at a glance.
fn tray_icon_image(accent: [f32; 3], mouse_mode: bool) -> Option<tray_icon::Icon> {
    if cfg!(target_os = "macos") {
        // A template image, 18 pt at 2x: macOS colours it for the menu bar.
        let data = crate::template_icon(36);
        return tray_icon::Icon::from_rgba(data.rgba, data.width, data.height).ok();
    }
    let base = if mouse_mode {
        let hsva = egui::ecolor::Hsva::from_rgb(accent);
        let turned = egui::ecolor::Hsva::new((hsva.h + 0.42) % 1.0, hsva.s.max(0.6), hsva.v.max(0.75), 1.0);
        turned.to_rgb()
    } else {
        accent
    };
    let data = crate::tinted_icon(32, base);
    tray_icon::Icon::from_rgba(data.rgba, data.width, data.height).ok()
}

fn build_menu(locale: &Locale) -> (Menu, CheckMenuItem, CheckMenuItem) {
    let menu = Menu::new();
    let item = |id: &str, key: &str| MenuItem::with_id(id, locale.get(key), true, None);
    let hotkeys = CheckMenuItem::with_id("hotkeys", locale.get("tray.hotkeys"), true, true, None);
    let mouse = CheckMenuItem::with_id("mouse", locale.get("tray.mouse_mode"), true, false, None);
    // Recognition actions go by their action id; the classic item stays
    // first, with copy mode and in-place translation next to it where they
    // are seen, and the rest a level down.
    let action = |a: Action, key: &str| MenuItem::with_id(format!("action:{}", a.id()), locale.get(key), true, None);
    let more = Submenu::with_items(
        locale.get("ocr.menu.more"),
        true,
        &[
            &action(Action::RecognizeAndTranslate, "ocr.menu.recognize_translate"),
            &action(Action::WatchRegion, "ocr.menu.watch"),
            &action(Action::RepeatLastRecognition, "ocr.menu.repeat"),
        ],
    );
    let more = more.ok();
    let mut ocr_items: Vec<Box<dyn tray_icon::menu::IsMenuItem>> = vec![
        Box::new(item("ocr", "tray.text_recognition")),
        Box::new(action(Action::RecognizeToClipboard, "ocr.menu.copy")),
        Box::new(action(Action::TranslateRegionInPlace, "ocr.menu.in_place")),
    ];
    if let Some(m) = more {
        ocr_items.push(Box::new(m));
    }
    let _ = menu.append_items(&[&item("show", "tray.show"), &PredefinedMenuItem::separator()]);
    let _ = menu.append_items(&ocr_items.iter().map(|b| b.as_ref()).collect::<Vec<_>>());
    let _ = menu.append_items(&[
        &item("dictionary", "tray.dictionary"),
        &item("history", "tray.history"),
        &item("keyboard", "tray.keyboard"),
        &PredefinedMenuItem::separator(),
        &mouse,
        &hotkeys,
        &item("options", "tray.options"),
        &item("about", "tray.about"),
        &PredefinedMenuItem::separator(),
        &item("exit", "tray.exit"),
    ]);
    (menu, hotkeys, mouse)
}

fn install_handlers(wake: WakeFn) -> Receiver<TrayEvent> {
    let (tx, rx) = mpsc::channel();
    let (tx2, c1, c2) = (tx.clone(), wake.clone(), wake);
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let ev = match e.id.as_ref() {
            "show" => Some(TrayEvent::ShowMain),
            "ocr" => Some(TrayEvent::Action(Action::TextRecognition)),
            "dictionary" => Some(TrayEvent::Action(Action::ShowDictionary)),
            "history" => Some(TrayEvent::Action(Action::ShowHistory)),
            "keyboard" => Some(TrayEvent::Action(Action::ShowKeyboard)),
            "hotkeys" => Some(TrayEvent::ToggleHotkeys),
            "mouse" => Some(TrayEvent::ToggleMouseMode),
            "options" => Some(TrayEvent::Action(Action::ShowOptions)),
            "about" => Some(TrayEvent::About),
            "exit" => Some(TrayEvent::Action(Action::Quit)),
            other => other.strip_prefix("action:").and_then(Action::from_id).map(TrayEvent::Action),
        };
        if let Some(ev) = ev {
            let _ = tx.send(ev);
            c1();
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        let ev = match e {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
                if !cfg!(target_os = "macos") =>
            {
                Some(TrayEvent::Click)
            }
            TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => Some(TrayEvent::ShowMain),
            _ => None,
        };
        if let Some(ev) = ev {
            let _ = tx2.send(ev);
            c2();
        }
    }));
    rx
}

impl Tray {
    #[cfg(not(target_os = "linux"))]
    pub fn new(wake: WakeFn, locale: &Locale) -> Result<Tray, String> {
        let events = install_handlers(wake);
        let (menu, hotkeys_item, mouse_item) = build_menu(locale);
        let mut b = TrayIconBuilder::new()
            .with_tooltip("Glossa")
            .with_menu(Box::new(menu))
            // A menu bar item opens its menu on click on macOS; elsewhere a
            // click toggles mouse mode or shows the window.
            .with_menu_on_left_click(cfg!(target_os = "macos"));
        if let Some(i) = tray_icon_image([0.16, 0.55, 0.89], false) {
            // macOS paints a template icon to suit the menu bar.
            b = if cfg!(target_os = "macos") { b.with_icon_templated(i) } else { b.with_icon(i) };
        }
        let icon = b.build().map_err(|e| e.to_string())?;
        Ok(Tray { events, icon, hotkeys_item, mouse_item })
    }

    #[cfg(target_os = "linux")]
    pub fn new(wake: WakeFn, locale: &Locale) -> Result<Tray, String> {
        let events = install_handlers(wake);
        let locale = locale.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (looks, new_looks) = mpsc::channel::<tray_icon::Icon>();
        std::thread::Builder::new()
            .name("glossa-tray".into())
            .spawn(move || {
                if let Err(e) = gtk::init() {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
                let (menu, _hotkeys, _mouse) = build_menu(&locale);
                let mut b = TrayIconBuilder::new().with_tooltip("Glossa").with_menu(Box::new(menu));
                if let Some(i) = tray_icon_image([0.16, 0.55, 0.89], false) {
                    b = b.with_icon(i);
                }
                match b.build() {
                    Ok(icon) => {
                        let _ = ready_tx.send(Ok(()));
                        // New icons arrive on a channel; the indicator may
                        // only be touched from this thread.
                        let icon = std::rc::Rc::new(icon);
                        let held = icon.clone();
                        gtk::glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
                            if let Some(i) = new_looks.try_iter().last() {
                                let _ = held.set_icon(Some(i));
                            }
                            gtk::glib::ControlFlow::Continue
                        });
                        gtk::main();
                        drop(icon);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        ready_rx.recv_timeout(std::time::Duration::from_secs(3)).map_err(|e| e.to_string())??;
        Ok(Tray { events, looks })
    }

    pub fn poll(&self) -> Vec<TrayEvent> {
        self.events.try_iter().collect()
    }

    /// Recolour the icon for the theme's accent and mouse mode's state, and
    /// make the menu dark or light to match the theme.
    pub fn set_look(&self, accent: [f32; 3], dark: bool, mouse_mode: bool) {
        // The menu is drawn by the system (TrackPopupMenu on Windows), which
        // follows the process-wide menu theme rather than any window's.
        glossa_platform::appearance::set_menu_theme(Some(dark));
        let Some(image) = tray_icon_image(accent, mouse_mode) else { return };
        #[cfg(not(target_os = "linux"))]
        {
            // The macOS template icon has no colour to change.
            if !cfg!(target_os = "macos") {
                let _ = self.icon.set_icon(Some(image));
            }
            self.mouse_item.set_checked(mouse_mode);
        }
        #[cfg(target_os = "linux")]
        let _ = self.looks.send(image);
    }

    pub fn set_hotkeys_checked(&self, on: bool) {
        #[cfg(not(target_os = "linux"))]
        self.hotkeys_item.set_checked(on);
        #[cfg(target_os = "linux")]
        let _ = on;
    }
}

pub fn handle(app: &mut GlossaApp, e: TrayEvent) {
    match e {
        TrayEvent::ShowMain => app.show_main(),
        TrayEvent::Click => {
            if app.cfg.mouse_mode.tray_click_toggles {
                app.perform(Action::ToggleMouseMode, None);
            } else if app.main_visible {
                app.hide_main();
            } else {
                app.show_main();
            }
        }
        TrayEvent::ToggleMouseMode => app.perform(Action::ToggleMouseMode, None),
        TrayEvent::Action(a) => app.perform(a, None),
        TrayEvent::ToggleHotkeys => {
            let on = !app.cfg.hotkeys.enabled;
            let _ = app.engine.update_config(move |c| c.hotkeys.enabled = on);
        }
        TrayEvent::About => app.open_about(),
    }
}
