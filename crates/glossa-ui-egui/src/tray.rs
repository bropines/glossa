//! The tray icon and its menu.
//!
//! What a left click, a double click and a middle click do is set in
//! `[tray]` (by default a click toggles mouse mode, as the classic program
//! did, and a double click shows the main window; on macOS a click opens
//! the menu, as menu-bar items do). Right click opens the classic menu
//! (text recognition, dictionary, history, mouse mode and global hotkeys
//! on/off, options, about, exit), with copy
//! mode, in-place translation and the other recognition actions beside text
//! recognition. The icon is drawn in the theme's accent colour, another hue
//! while mouse mode is on, and on Windows the menu is dark or light with the
//! theme (`glossa_platform::appearance::set_menu_theme`). Events reach the
//! app through channels and wake its loop, so they work while every window
//! is hidden.
//!
//! The system's own double-click events are not used: Windows sends the
//! single click first and macOS sends none, so a small thread of our own
//! counts the button releases instead, waiting out the double-click time
//! only while a double click has something to do.
//!
//! On Linux the indicator needs GTK running on the thread that owns it, so
//! the icon lives on a thread of its own there. AppIndicator reports no
//! clicks at all: a click there opens the menu, whatever the settings say.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;

use glossa_core::config::{TrayAction, TrayConfig};
use glossa_core::{Action, Locale};
use glossa_platform::window::FocusMark;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::app::GlossaApp;

pub enum TrayEvent {
    ShowMain,
    /// A click on the icon, single and double told apart, with the window
    /// that had the focus before it (Windows; see `focus_mark`).
    Click(ClickKind, Option<FocusMark>),
    ToggleMouseMode,
    Action(Action),
    ToggleHotkeys,
    About,
    /// Open the page where the project can be supported.
    Support,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickKind {
    Single,
    Double,
    Middle,
}

/// What a click comes to, decided in one place for both the tray-only loop
/// (GUI unloaded) and the GUI; each only carries it out its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickPlan {
    Nothing,
    Menu,
    Quit,
    ToggleMouseMode,
    ShowMain,
    HideMain,
    /// Read the selection, from the window marked at the click, for this
    /// action.
    Selection(Action),
    Perform(Action),
}

/// Decide what a click does, by the settings. Mouse mode where it cannot
/// run falls back to the main window, and a single click set to show the
/// main window hides it again while it is showing, as the classic click
/// did.
pub fn plan_click(cfg: &TrayConfig, kind: ClickKind, main_visible: bool, mouse_supported: bool) -> ClickPlan {
    let chosen = match kind {
        ClickKind::Single => cfg.click,
        ClickKind::Double => cfg.double_click,
        ClickKind::Middle => cfg.middle_click,
    };
    match chosen {
        TrayAction::Nothing => ClickPlan::Nothing,
        TrayAction::Menu => ClickPlan::Menu,
        TrayAction::Run(Action::Quit) => ClickPlan::Quit,
        TrayAction::Run(Action::ToggleMouseMode) if mouse_supported => ClickPlan::ToggleMouseMode,
        TrayAction::Run(Action::ShowMainWindow | Action::ToggleMouseMode) => {
            if kind == ClickKind::Single && main_visible {
                ClickPlan::HideMain
            } else {
                ClickPlan::ShowMain
            }
        }
        TrayAction::Run(a) if crate::actions::needs_selection(a) => ClickPlan::Selection(a),
        TrayAction::Run(a) => ClickPlan::Perform(a),
    }
}

/// What the click thread needs to know of the settings.
#[derive(Default)]
struct ClickSettings {
    /// The left click opens the menu, which the icon does by itself.
    left_is_menu: AtomicBool,
    /// A double click does something, so a single one waits it out.
    double: AtomicBool,
}

/// A button released over the icon, with the focus mark of the moment.
enum Release {
    Left(Option<FocusMark>),
    Middle(Option<FocusMark>),
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
    clicks: Arc<ClickSettings>,
    #[cfg(not(target_os = "linux"))]
    icon: tray_icon::TrayIcon,
    #[cfg(not(target_os = "linux"))]
    hotkeys_item: CheckMenuItem,
    #[cfg(not(target_os = "linux"))]
    mouse_item: CheckMenuItem,
    /// New icons and menus for the GTK thread that owns the indicator.
    #[cfg(target_os = "linux")]
    looks: mpsc::Sender<LinuxUpdate>,
    /// The interface language the menu was built in.
    locale: Arc<Locale>,
}

/// What the GTK thread is asked to change.
#[cfg(target_os = "linux")]
enum LinuxUpdate {
    Icon(tray_icon::Icon),
    Menu(Arc<Locale>),
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
    // Greyed out where mouse mode cannot work at all.
    let mouse = CheckMenuItem::with_id(
        "mouse",
        locale.get("tray.mouse_mode"),
        crate::mouse::MouseHub::supported(),
        false,
        None,
    );
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
        Box::new(action(Action::TranslateScreenInPlace, "ocr.menu.screen_in_place")),
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
        &item("support", "tray.support"),
        &item("about", "tray.about"),
        &PredefinedMenuItem::separator(),
        &item("exit", "tray.exit"),
    ]);
    (menu, hotkeys, mouse)
}

fn install_handlers(wake: WakeFn, clicks: Arc<ClickSettings>) -> Receiver<TrayEvent> {
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
            "support" => Some(TrayEvent::Support),
            "exit" => Some(TrayEvent::Action(Action::Quit)),
            other => other.strip_prefix("action:").and_then(Action::from_id).map(TrayEvent::Action),
        };
        if let Some(ev) = ev {
            let _ = tx.send(ev);
            c1();
        }
    }));
    let releases = spawn_click_thread(tx2, c2, clicks.clone());
    // The window the user was in, taken at the click itself: the taskbar
    // or the overflow flyout is in front by then (Windows), and the
    // platform remembers what was before it.
    glossa_platform::window::track_foreground();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        let release = match e {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
                if !clicks.left_is_menu.load(Ordering::Relaxed) =>
            {
                Some(Release::Left(glossa_platform::window::focus_mark()))
            }
            TrayIconEvent::Click { button: MouseButton::Middle, button_state: MouseButtonState::Up, .. } => {
                Some(Release::Middle(glossa_platform::window::focus_mark()))
            }
            _ => None,
        };
        if let (Some(r), Some(releases)) = (release, releases.as_ref()) {
            let _ = releases.send(r);
        }
    }));
    rx
}

/// The thread that tells a single click from a double one. A single click
/// is held for the system's double-click time only while a double click
/// has something to do; otherwise it goes straight through.
fn spawn_click_thread(events: Sender<TrayEvent>, wake: WakeFn, clicks: Arc<ClickSettings>) -> Option<Sender<Release>> {
    if !glossa_platform::shell::SHELL.tray_clicks {
        return None;
    }
    let (tx, rx) = mpsc::channel::<Release>();
    let emit = move |kind: ClickKind, mark: Option<FocusMark>| {
        let _ = events.send(TrayEvent::Click(kind, mark));
        wake();
    };
    std::thread::Builder::new()
        .name("glossa-tray-clicks".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                let mark = match first {
                    Release::Middle(m) => {
                        emit(ClickKind::Middle, m);
                        continue;
                    }
                    Release::Left(m) => m,
                };
                if !clicks.double.load(Ordering::Relaxed) {
                    emit(ClickKind::Single, mark);
                    continue;
                }
                match rx.recv_timeout(glossa_platform::window::double_click_time()) {
                    Ok(Release::Left(_)) => emit(ClickKind::Double, mark),
                    Ok(Release::Middle(m)) => {
                        emit(ClickKind::Single, mark);
                        emit(ClickKind::Middle, m);
                    }
                    Err(RecvTimeoutError::Timeout) => emit(ClickKind::Single, mark),
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(|e| tracing::warn!("no tray click thread: {e}"))
        .ok()?;
    Some(tx)
}

impl Tray {
    #[cfg(not(target_os = "linux"))]
    pub fn new(wake: WakeFn, locale: &Arc<Locale>, clicks_cfg: &TrayConfig) -> Result<Tray, String> {
        let clicks = Arc::new(ClickSettings::default());
        let events = install_handlers(wake, clicks.clone());
        let (menu, hotkeys_item, mouse_item) = build_menu(locale);
        let mut b = TrayIconBuilder::new()
            .with_tooltip("Glossa")
            .with_menu(Box::new(menu))
            // The icon opens its menu on a left click by itself when that
            // is the setting (`set_clicks` keeps it in step).
            .with_menu_on_left_click(clicks_cfg.click == TrayAction::Menu);
        if let Some(i) = tray_icon_image([0.16, 0.55, 0.89], false) {
            // macOS paints a template icon to suit the menu bar.
            #[cfg(target_os = "macos")]
            {
                b = b.with_icon_templated(i);
            }
            #[cfg(not(target_os = "macos"))]
            {
                b = b.with_icon(i);
            }
        }
        let icon = b.build().map_err(|e| e.to_string())?;
        let tray = Tray { events, clicks, icon, hotkeys_item, mouse_item, locale: locale.clone() };
        tray.set_clicks(clicks_cfg);
        Ok(tray)
    }

    #[cfg(target_os = "linux")]
    pub fn new(wake: WakeFn, locale: &Arc<Locale>, clicks_cfg: &TrayConfig) -> Result<Tray, String> {
        // The indicator reports no clicks; the settings have nothing to do.
        let _ = clicks_cfg;
        let clicks = Arc::new(ClickSettings::default());
        let events = install_handlers(wake, clicks.clone());
        let built_for = locale.clone();
        let locale = locale.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (looks, new_looks) = mpsc::channel::<LinuxUpdate>();
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
                        // New icons and menus arrive on a channel; the
                        // indicator may only be touched from this thread.
                        let icon = std::rc::Rc::new(icon);
                        let held = icon.clone();
                        gtk::glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
                            for update in new_looks.try_iter() {
                                match update {
                                    LinuxUpdate::Icon(i) => {
                                        let _ = held.set_icon(Some(i));
                                    }
                                    LinuxUpdate::Menu(locale) => {
                                        let (menu, _hotkeys, _mouse) = build_menu(&locale);
                                        held.set_menu(Some(Box::new(menu)));
                                    }
                                }
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
        Ok(Tray { events, clicks, looks, locale: built_for })
    }

    /// Follow the click settings: whether the icon opens its menu on a
    /// left click by itself, and whether a single click waits for a second.
    pub fn set_clicks(&self, cfg: &TrayConfig) {
        let left_is_menu = cfg.click == TrayAction::Menu;
        self.clicks.left_is_menu.store(left_is_menu, Ordering::Relaxed);
        self.clicks.double.store(!left_is_menu && cfg.double_click != TrayAction::Nothing, Ordering::Relaxed);
        #[cfg(not(target_os = "linux"))]
        self.icon.set_show_menu_on_left_click(left_is_menu);
    }

    /// Open the tray menu at the icon, for a click set to do that. Nothing
    /// on Linux, where only the indicator itself opens it.
    pub fn show_menu(&self) {
        #[cfg(not(target_os = "linux"))]
        self.icon.show_menu();
    }

    pub fn poll(&self) -> Vec<TrayEvent> {
        self.events.try_iter().collect()
    }

    /// Recolour the icon for the theme's accent and mouse mode's state
    /// (running, not merely switched on), and make the menu dark or light to
    /// match the theme.
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
        let _ = self.looks.send(LinuxUpdate::Icon(image));
    }

    /// Rebuild the menu in `locale`'s language, keeping the check marks.
    /// Nothing happens when the menu is already in that language.
    pub fn set_locale(&mut self, locale: &Arc<Locale>) {
        if Arc::ptr_eq(locale, &self.locale) {
            return;
        }
        self.locale = locale.clone();
        #[cfg(not(target_os = "linux"))]
        {
            let (menu, hotkeys_item, mouse_item) = build_menu(locale);
            hotkeys_item.set_checked(self.hotkeys_item.is_checked());
            mouse_item.set_checked(self.mouse_item.is_checked());
            self.icon.set_menu(Some(Box::new(menu)));
            self.hotkeys_item = hotkeys_item;
            self.mouse_item = mouse_item;
        }
        #[cfg(target_os = "linux")]
        let _ = self.looks.send(LinuxUpdate::Menu(locale.clone()));
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
        TrayEvent::Click(kind, mark) => {
            match plan_click(&app.cfg.tray, kind, app.main_visible, crate::mouse::MouseHub::supported()) {
                ClickPlan::Nothing => {}
                ClickPlan::Menu => {
                    if let Some(t) = &app.tray {
                        t.show_menu();
                    }
                }
                ClickPlan::Quit => app.perform(Action::Quit, None),
                ClickPlan::ToggleMouseMode => app.perform(Action::ToggleMouseMode, None),
                ClickPlan::ShowMain => app.show_main(),
                ClickPlan::HideMain => app.hide_main(),
                ClickPlan::Selection(a) => crate::actions::capture_after_click(app, a, mark),
                ClickPlan::Perform(a) => app.perform(a, None),
            }
        }
        TrayEvent::ToggleMouseMode => app.perform(Action::ToggleMouseMode, None),
        TrayEvent::Action(a) => app.perform(a, None),
        TrayEvent::ToggleHotkeys => {
            let on = !app.cfg.hotkeys.enabled;
            let _ = app.engine.update_config(move |c| c.hotkeys.enabled = on);
        }
        TrayEvent::About => app.open_about(),
        TrayEvent::Support => app.open_url(glossa_core::SUPPORT_URL),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::config::Os;

    fn cfg(click: TrayAction, double_click: TrayAction, middle_click: TrayAction) -> TrayConfig {
        TrayConfig { click, double_click, middle_click }
    }

    #[test]
    fn each_click_reads_its_own_setting() {
        let c =
            cfg(TrayAction::Run(Action::TextRecognition), TrayAction::Run(Action::ShowMainWindow), TrayAction::Menu);
        assert_eq!(plan_click(&c, ClickKind::Single, false, true), ClickPlan::Perform(Action::TextRecognition));
        assert_eq!(plan_click(&c, ClickKind::Double, false, true), ClickPlan::ShowMain);
        assert_eq!(plan_click(&c, ClickKind::Middle, false, true), ClickPlan::Menu);
    }

    #[test]
    fn the_main_window_click_hides_a_showing_window_on_a_single_click_only() {
        let c =
            cfg(TrayAction::Run(Action::ShowMainWindow), TrayAction::Run(Action::ShowMainWindow), TrayAction::Nothing);
        assert_eq!(plan_click(&c, ClickKind::Single, true, true), ClickPlan::HideMain);
        assert_eq!(plan_click(&c, ClickKind::Single, false, true), ClickPlan::ShowMain);
        assert_eq!(plan_click(&c, ClickKind::Double, true, true), ClickPlan::ShowMain);
        assert_eq!(plan_click(&c, ClickKind::Middle, false, true), ClickPlan::Nothing);
    }

    #[test]
    fn mouse_mode_falls_back_to_the_main_window_where_it_cannot_run() {
        let c = TrayConfig::for_os(Os::Windows);
        assert_eq!(plan_click(&c, ClickKind::Single, false, true), ClickPlan::ToggleMouseMode);
        assert_eq!(plan_click(&c, ClickKind::Single, false, false), ClickPlan::ShowMain);
        assert_eq!(plan_click(&c, ClickKind::Single, true, false), ClickPlan::HideMain);
    }

    #[test]
    fn selection_actions_and_quit_get_their_own_plans() {
        let c = cfg(TrayAction::Run(Action::ShowPopup), TrayAction::Run(Action::Quit), TrayAction::Nothing);
        assert_eq!(plan_click(&c, ClickKind::Single, false, true), ClickPlan::Selection(Action::ShowPopup));
        assert_eq!(plan_click(&c, ClickKind::Double, false, true), ClickPlan::Quit);
    }
}
