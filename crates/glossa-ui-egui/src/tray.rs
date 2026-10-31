//! The tray icon and its menu.
//!
//! Left click shows the main window; right click opens the classic menu (text
//! recognition, dictionary, history, global hotkeys on/off, options, about,
//! exit). Events reach the app through channels and wake its loop, so they
//! work while every window is hidden.
//!
//! On Linux the indicator needs GTK running on the thread that owns it, so
//! the icon lives on a thread of its own there.

use std::sync::mpsc::{self, Receiver};

use glossa_core::{Action, Locale};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::app::GlossaApp;

pub enum TrayEvent {
    ShowMain,
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
    _icon: tray_icon::TrayIcon,
    #[cfg(not(target_os = "linux"))]
    hotkeys_item: CheckMenuItem,
}

fn build_menu(locale: &Locale) -> (Menu, CheckMenuItem) {
    let menu = Menu::new();
    let item = |id: &str, key: &str| MenuItem::with_id(id, locale.get(key), true, None);
    let hotkeys = CheckMenuItem::with_id("hotkeys", locale.get("tray.hotkeys"), true, true, None);
    let _ = menu.append_items(&[
        &item("show", "tray.show"),
        &PredefinedMenuItem::separator(),
        &item("ocr", "tray.text_recognition"),
        &item("dictionary", "tray.dictionary"),
        &item("history", "tray.history"),
        &item("keyboard", "tray.keyboard"),
        &PredefinedMenuItem::separator(),
        &hotkeys,
        &item("options", "tray.options"),
        &item("about", "tray.about"),
        &PredefinedMenuItem::separator(),
        &item("exit", "tray.exit"),
    ]);
    (menu, hotkeys)
}

fn icon() -> Option<tray_icon::Icon> {
    let data = crate::app_icon(32);
    tray_icon::Icon::from_rgba(data.rgba, data.width, data.height).ok()
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
            "options" => Some(TrayEvent::Action(Action::ShowOptions)),
            "about" => Some(TrayEvent::About),
            "exit" => Some(TrayEvent::Action(Action::Quit)),
            _ => None,
        };
        if let Some(ev) = ev {
            let _ = tx.send(ev);
            c1();
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            let _ = tx2.send(TrayEvent::ShowMain);
            c2();
        }
    }));
    rx
}

impl Tray {
    #[cfg(not(target_os = "linux"))]
    pub fn new(wake: WakeFn, locale: &Locale) -> Result<Tray, String> {
        let events = install_handlers(wake);
        let (menu, hotkeys_item) = build_menu(locale);
        let mut b = TrayIconBuilder::new().with_tooltip("Glossa").with_menu(Box::new(menu)).with_menu_on_left_click(false);
        if let Some(i) = icon() {
            b = b.with_icon(i);
        }
        let icon = b.build().map_err(|e| e.to_string())?;
        Ok(Tray { events, _icon: icon, hotkeys_item })
    }

    #[cfg(target_os = "linux")]
    pub fn new(wake: WakeFn, locale: &Locale) -> Result<Tray, String> {
        let events = install_handlers(wake);
        let locale = locale.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("glossa-tray".into())
            .spawn(move || {
                if let Err(e) = gtk::init() {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
                let (menu, _hotkeys) = build_menu(&locale);
                let mut b = TrayIconBuilder::new().with_tooltip("Glossa").with_menu(Box::new(menu));
                if let Some(i) = icon() {
                    b = b.with_icon(i);
                }
                match b.build() {
                    Ok(icon) => {
                        let _ = ready_tx.send(Ok(()));
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
        Ok(Tray { events })
    }

    pub fn poll(&self) -> Vec<TrayEvent> {
        self.events.try_iter().collect()
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
        TrayEvent::ShowMain => {
            if app.cfg.mouse_mode.tray_click_toggles && app.cfg.mouse_mode.enabled && app.main_visible {
                app.perform(Action::ToggleMouseMode, None);
            } else if app.main_visible {
                app.hide_main();
            } else {
                app.show_main();
            }
        }
        TrayEvent::Action(a) => app.perform(a, None),
        TrayEvent::ToggleHotkeys => {
            let on = !app.cfg.hotkeys.enabled;
            let _ = app.engine.update_config(move |c| c.hotkeys.enabled = on);
        }
        TrayEvent::About => app.about_open = true,
    }
}
