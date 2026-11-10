//! Glossa — the desktop application.
//!
//! Lives in the tray; a hotkey over selected text in any program opens a
//! popup with the translation, as the classic QTranslate did. Also: a main
//! translation window, screen-region OCR, a dictionary, history, and options.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod about;
mod actions;
mod app;
mod app_menu;
mod bidi;
mod chrome;
mod code_editor;
mod dictionary;
mod dropdown;
mod fonts;
mod frametrace;
mod history;
mod icons;
mod inplace;
mod keyboard;
mod langs;
mod main_window;
mod module_actions;
mod module_editor;
mod module_panel;
mod module_settings;
mod mouse;
mod notice;
mod ocr;
mod ocr_result;
mod options;
mod popup;
mod presets;
mod preview;
mod rich;
mod screenshot;
mod scroll;
mod spell;
mod style;
mod text_menu;
mod tray;
mod watchdog;
mod welcome;
mod widgets;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use glossa_core::{Action, Paths};
use glossa_engine::Engine;

/// Command-line switches for the GUI binary.
#[derive(Default, Debug, Clone)]
pub struct Launch {
    /// Start in the tray even if the settings say otherwise.
    pub hidden: bool,
    /// Development aid: render the given window, save a screenshot, exit.
    pub screenshot: Option<(String, std::path::PathBuf)>,
    pub home: Option<std::path::PathBuf>,
    /// Text to show in the popup at start (used by screenshots and tests).
    pub popup_text: Option<String>,
}

fn parse_args() -> Launch {
    let mut l = Launch::default();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--hidden" | "--tray" => l.hidden = true,
            "--home" => l.home = it.next().map(Into::into),
            // --home=DIR, as glossa-cli takes it too.
            a if a.starts_with("--home=") => l.home = Some(a["--home=".len()..].into()),
            "--popup" => l.popup_text = it.next(),
            // For the smoke test: every window --screenshot can draw.
            "--screenshot-list" => {
                for t in screenshot::targets() {
                    println!("{t}");
                }
                std::process::exit(0);
            }
            "--screenshot" => {
                let what = it.next().unwrap_or_else(|| "main".into());
                let to = it.next().unwrap_or_else(|| format!("glossa-{what}.png"));
                l.screenshot = Some((what, to.into()));
            }
            _ => {}
        }
    }
    l
}

/// What survives while the graphics are unloaded: the engine and everything
/// that must keep working from the tray.
pub struct Shell {
    pub engine: Engine,
    pub hotkeys: Option<glossa_platform::HotkeyService>,
    /// Hotkeys that failed, kept as data and put into words when shown,
    /// so they follow the interface language.
    pub hotkey_errors: Vec<HotkeyFailure>,
    pub tray: Option<tray::Tray>,
    pub server: Option<glossa_engine::server::ServerHandle>,
    pub server_error: Option<String>,
    pub clipboard: Option<glossa_platform::Clipboard>,
    pub audio: glossa_platform::AudioPlayer,
    pub wake: Arc<Wake>,
    /// What woke the GUI up, for it to act on when it starts.
    pub pending: Vec<tray::Pending>,
    pub quit: bool,
    /// Start a fresh copy of the program once this one has shut down (a
    /// setting that only applies at start-up changed).
    pub restart: bool,
    /// Mouse mode's watcher; like the hotkeys it outlives the GUI.
    pub mouse: mouse::MouseHub,
    /// Problems met while starting, for the first GUI run to show.
    pub startup_warnings: Vec<String>,
}

/// Wakes whichever loop is running: the egui loop when the GUI is loaded,
/// the bare message loop when it is not.
pub struct Wake {
    pub ctx: parking_lot::Mutex<Option<egui::Context>>,
    pub idle: glossa_platform::idle::Waker,
}

impl Wake {
    pub fn wake(&self) {
        if let Some(ctx) = self.ctx.lock().as_ref() {
            ctx.request_repaint();
        }
        self.idle.wake();
    }
}

fn main() -> eframe::Result<()> {
    let launch = parse_args();
    if launch.screenshot.is_some() {
        screenshot::watchdog();
    }
    let paths = match &launch.home {
        Some(h) => Paths::for_exe_dir(
            std::env::current_exe().ok().and_then(|p| p.parent().map(Into::into)).unwrap_or_default(),
            Some(h.clone()),
        ),
        None => Paths::discover(),
    };
    init_logging(&paths);
    frametrace::init(&paths.cache_dir);
    glossa_platform::crash::install(&paths.log_file());
    // The desktop portal's permissions to remember (Wayland) live with the data.
    glossa_platform::portal::set_state_dir(&paths.data_dir);
    // Log every panic with its frames. Panics unwind afterwards (release
    // builds too) to the `catch_unwind` around providers, engine jobs and
    // API requests, which fail that one request instead of the program.
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("unnamed");
        // A stripped build resolves no symbols; then the frames as
        // module+offset are what can be looked up later.
        let resolved = std::backtrace::Backtrace::force_capture().to_string();
        let bt = if resolved.contains("<unknown>") || resolved.trim().is_empty() {
            glossa_platform::crash::stack_trace(0)
        } else {
            format!("\n{resolved}")
        };
        tracing::error!("panic on thread '{thread}': {info}{bt}");
    }));
    crash_test();

    // One copy per user, session and data folder: a second launch hands its
    // request to the running one over a local pipe or socket (the control
    // API only as a fallback) and leaves.
    let mut instance = None;
    if launch.screenshot.is_none() {
        use glossa_platform::instance::{self, Instance, Message};
        match instance::acquire(&paths) {
            Instance::Primary(guard) => instance = Some(guard),
            Instance::Secondary => {
                let message = match &launch.popup_text {
                    Some(text) => Message { action: Action::ShowPopup, text: Some(text.clone()) },
                    None => Message::show(),
                };
                match instance::send(&paths, &message, Duration::from_secs(3)) {
                    Ok(()) => {
                        tracing::info!("another Glossa is running; handed over to it");
                        return Ok(());
                    }
                    Err(e) if hand_over_to_running_instance(&paths) => {
                        tracing::info!("another Glossa is running; handed over through the control API ({e})");
                        return Ok(());
                    }
                    Err(e) => tracing::warn!("another Glossa holds the lock but did not answer ({e}); starting anyway"),
                }
            }
        }
    }

    let engine = match Engine::start(paths.clone()) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("cannot start: {e}");
            let _ = rfd::MessageDialog::new()
                .set_title("Glossa")
                .set_description(format!("Glossa cannot start:\n\n{e}"))
                .set_level(rfd::MessageLevel::Error)
                .show();
            std::process::exit(1);
        }
    };
    // Before anything is translated: keys the settings keep in the system
    // keychain are fetched now.
    engine.attach_secret_store(glossa_platform::secrets::system_store());
    // The theme in effect may follow the system's light or dark mode.
    engine.set_system_dark(glossa_platform::appearance::system_prefers_dark());

    if let Some(guard) = &instance {
        let engine = engine.clone();
        guard.set_handler(move |m| engine.request_action(m.action, m.text));
    }

    // macOS: reading and replacing the selection need Accessibility; show
    // the system's prompt rather than failing silently later. Once per
    // build: after a refusal the prompt at every start would only nag, and
    // Options → Permissions can ask again.
    {
        use glossa_platform::permissions::{request, status, Permission, Status};
        if status(Permission::Accessibility) == Status::Missing {
            let marker = paths.cache_dir.join("asked-accessibility");
            let build = std::env::current_exe()
                .and_then(std::fs::metadata)
                .and_then(|m| m.modified())
                .map(|t| format!("{t:?}"))
                .unwrap_or_default();
            if std::fs::read_to_string(&marker).ok().as_deref() != Some(build.as_str()) {
                tracing::info!("asking for the Accessibility permission");
                request(Permission::Accessibility);
                let _ = std::fs::create_dir_all(&paths.cache_dir);
                let _ = std::fs::write(&marker, &build);
            }
        }
    }

    let pump = glossa_platform::idle::IdlePump::for_current_thread();
    let wake = Arc::new(Wake { ctx: parking_lot::Mutex::new(None), idle: pump.waker() });
    let shell = build_shell(engine.clone(), wake.clone());
    let slot: Rc<RefCell<Option<Shell>>> = Rc::new(RefCell::new(Some(shell)));
    // Engine events seen while no GUI is loaded (the control API asking for
    // an action).
    let idle_events = engine.subscribe();
    // Linux and the BSDs: the same actions on the session bus. Like the
    // instance lock it lives as long as the process, GUI loaded or not;
    // its calls arrive as engine events, as the control API's do.
    let mut dbus = None;
    if cfg_dbus(&engine) {
        match glossa_engine::dbus::start(engine.clone()) {
            Ok(handle) => dbus = Some(handle),
            Err(e) => tracing::warn!("D-Bus service not started: {e}"),
        }
    }

    let cfg = engine.config();
    let can_unload = glossa_platform::idle::IdlePump::supported() && launch.screenshot.is_none();
    // The first run shows the window once, with the note that Glossa lives
    // in the tray (`general.show_tray_hint`).
    let mut start_visible = !launch.hidden
        && (launch.screenshot.is_some()
            || cfg.general.show_tray_hint
            || cfg.general.show_welcome
            || match cfg.general.startup_action {
                glossa_core::config::StartupAction::ShowWindow
                | glossa_core::config::StartupAction::RestorePrevious => true,
                glossa_core::config::StartupAction::MinimizeToTray => {
                    slot.borrow().as_ref().is_some_and(|s| s.tray.is_none())
                }
            });
    let mut first = true;

    loop {
        // Start in the tray with nothing loaded, and come back here whenever
        // the GUI unloads itself.
        if can_unload && !(first && (start_visible || launch.popup_text.is_some())) {
            while idle_events.try_recv().is_ok() {}
            glossa_platform::idle::trim_working_set();
            tracing::debug!("graphics unloaded; waiting in the tray");
            let mut asked_to_quit = false;
            let woke = pump.run_until(|| {
                let mut guard = slot.borrow_mut();
                let Some(shell) = guard.as_mut() else { return true };
                collect_wake_events(shell, &idle_events);
                asked_to_quit = shell.quit;
                shell.quit || !shell.pending.is_empty()
            });
            if !woke || asked_to_quit {
                break;
            }
            start_visible =
                slot.borrow().as_ref().is_some_and(|s| s.pending.iter().any(tray::Pending::wants_main_window));
        }
        first = false;

        let cfg = engine.config();
        let geometry = cfg.windows.get("main").copied();
        let mut viewport = egui::ViewportBuilder::default()
            .with_title("Glossa")
            .with_app_id("glossa")
            .with_inner_size(geometry.map(|g| [g.width, g.height]).unwrap_or([680.0, 470.0]))
            .with_min_inner_size([420.0, 300.0])
            .with_decorations(!chrome::custom(&cfg))
            .with_transparent(style::needs_transparent_root(&cfg))
            .with_visible(start_visible)
            .with_window_level(if cfg.appearance.main_always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            })
            .with_icon(app_icon(64));
        if let Some(g) = geometry {
            viewport = viewport.with_position([g.x, g.y]);
            if g.maximized {
                viewport = viewport.with_maximized(true);
            }
        }
        // Fonts for other scripts and emoji are Glossa's own business
        // (`fonts::Fonts` loads them when such text first shows, used from
        // the next frame). eframe's system-font lookup is not built in; its
        // stand-in only logs "No font has …" for that first frame.
        let mut options = eframe::NativeOptions {
            viewport,
            run_and_return: can_unload,
            persist_window: false,
            system_font_fallback: false,
            ..Default::default()
        };
        choose_renderer(&mut options, cfg.appearance.renderer, style::needs_transparent_root(&cfg));
        let mut this_launch = launch.clone();
        this_launch.hidden = !start_visible;
        let app_slot = slot.clone();
        let result = eframe::run_native(
            "Glossa",
            options,
            Box::new(move |cc| Ok(Box::new(app::GlossaApp::new(cc, app_slot, this_launch)))),
        );
        if let Err(e) = result {
            tracing::error!("the window system failed: {e}");
            break;
        }
        let quit = slot.borrow().as_ref().is_none_or(|s| s.quit);
        if quit || !can_unload {
            break;
        }
        launch_once_only(&mut start_visible);
    }

    let mut restart = false;
    if let Some(shell) = slot.borrow_mut().take() {
        restart = shell.restart;
        if let Some(server) = shell.server {
            server.stop();
        }
        shell.engine.shutdown();
        // Dropping the rest releases the hotkeys and the tray icon.
    }
    if restart {
        // Only now, with the lock released: the new copy finds it and the
        // control port free and starts for real instead of handing over to
        // this one.
        drop(instance.take());
        drop(dbus.take());
        let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--hidden").collect();
        match std::env::current_exe().map(|exe| std::process::Command::new(exe).args(&args).spawn()) {
            Ok(Ok(_)) => tracing::info!("restarting"),
            Ok(Err(e)) | Err(e) => tracing::error!("could not restart: {e}"),
        }
    }
    Ok(())
}

/// Whether to offer the D-Bus service: the setting, where there is one.
fn cfg_dbus(engine: &Engine) -> bool {
    glossa_engine::dbus::SUPPORTED && engine.config().server.dbus
}

fn launch_once_only(start_visible: &mut bool) {
    *start_visible = false;
}

/// The parts that live as long as the process.
/// The tray icon in the theme's accent colour, changed while mouse mode is
/// on, with the menu's check marks, the menus dark or light as the theme,
/// and what clicks on it do.
pub fn refresh_tray_look(shell: &Shell) {
    let Some(t) = &shell.tray else { return };
    let (accent, dark) = tray::look_of(&shell.engine);
    t.set_look(accent, dark, shell.mouse.running());
    t.set_clicks(&shell.engine.config().tray);
}

/// Mouse mode on or off from the tray, with no GUI needed. A failure to
/// start brings the GUI up to say why.
fn toggle_mouse_mode(shell: &mut Shell) {
    if !mouse::MouseHub::supported() {
        let message = shell.engine.locale().get("mouse.unavailable").to_owned();
        shell.pending.push(tray::Pending::Task(actions::Task::Done { message, error: true }));
        return;
    }
    let on = !shell.engine.config().mouse_mode.enabled;
    let _ = shell.engine.update_config(move |c| c.mouse_mode.enabled = on);
    sync_mouse_mode(shell, on);
    refresh_tray_look(shell);
}

/// Start or stop mouse mode's watcher; a failure is queued for the GUI to
/// show.
fn sync_mouse_mode(shell: &mut Shell, on: bool) {
    if let Some(e) = shell.mouse.sync(on) {
        let message = shell.engine.locale().fmt("mouse.failed", &e);
        shell.pending.push(tray::Pending::Task(actions::Task::Done { message, error: true }));
    }
}

fn build_shell(engine: Engine, wake: Arc<Wake>) -> Shell {
    let wake_for_mouse = wake.clone();
    {
        let w = wake.clone();
        engine.set_waker(move || w.wake());
    }
    let cfg = engine.config();
    let mut hotkey_errors = Vec::new();
    let hotkeys = match glossa_platform::HotkeyService::new() {
        Ok(mut h) => {
            let w = wake.clone();
            h.set_waker(move || w.wake());
            Some(h)
        }
        Err(e) => {
            hotkey_errors.push((None, None, e));
            None
        }
    };
    let tray = {
        let w = wake.clone();
        tray::Tray::new(Arc::new(move || w.wake()), &engine.locale(), &cfg.tray)
            .map_err(|e| tracing::warn!("no tray icon: {e}"))
            .ok()
    };
    let mut tray = tray;
    if let Some(t) = tray.as_mut() {
        t.set_panels(tray_panels(&engine));
    }
    refresh_autostart(&cfg);
    let (server, server_error) = if cfg.server.enabled {
        match glossa_engine::server::start(engine.clone()) {
            Ok(h) => (Some(h), None),
            Err(e) => {
                tracing::warn!("control API not started: {e}");
                (None, Some(e.to_string()))
            }
        }
    } else {
        (None, None)
    };
    let mut shell = Shell {
        engine,
        hotkeys,
        hotkey_errors,
        tray,
        server,
        server_error,
        clipboard: glossa_platform::Clipboard::new().map_err(|e| tracing::warn!("no clipboard: {e}")).ok(),
        audio: glossa_platform::AudioPlayer::new(),
        wake,
        pending: Vec::new(),
        quit: false,
        restart: false,
        mouse: mouse::MouseHub::new({
            let w = wake_for_mouse.clone();
            Arc::new(move || w.wake())
        }),
        startup_warnings: Vec::new(),
    };
    shell.startup_warnings = engine_startup_warnings(&shell.engine);
    register_hotkeys(&mut shell);
    sync_mouse_mode(&mut shell, cfg.mouse_mode.enabled);
    refresh_tray_look(&shell);
    shell
}

/// Problems the engine met while starting (a configuration it could not
/// read and set aside, say), in the interface language, for the user to see
/// once.
fn engine_startup_warnings(engine: &Engine) -> Vec<String> {
    let locale = engine.locale();
    engine.startup_warnings().iter().map(|w| w.message(&locale)).collect()
}

/// Bind the configured hotkeys; failures are kept for the options page.
pub fn register_hotkeys(shell: &mut Shell) {
    let cfg = shell.engine.config();
    let Some(h) = shell.hotkeys.as_mut() else { return };
    h.unregister_all();
    shell.hotkey_errors.clear();
    if !cfg.hotkeys.enabled {
        return;
    }
    for (action, hotkey) in &cfg.hotkeys.bindings {
        if let Err(e) = h.register(*action, hotkey) {
            tracing::warn!("hotkey {hotkey} for {}: {e}", action.id());
            shell.hotkey_errors.push((Some(*action), None, e));
        }
    }
    register_panel_hotkeys(h, &shell.engine, &cfg, &mut shell.hotkey_errors);
}

/// The `[hotkeys.panels]` bindings: each opens one module panel.
pub fn register_panel_hotkeys(
    h: &mut glossa_platform::HotkeyService,
    engine: &glossa_engine::Engine,
    cfg: &glossa_core::Config,
    errors: &mut Vec<HotkeyFailure>,
) {
    // Only for modules that may run: one waiting for approval (or not
    // loaded) gets no hotkey until it is approved and loaded.
    let registry = engine.registry();
    let allowed = |panel: &str| {
        let module = panel.split_once('/').map(|(m, _)| m).unwrap_or(panel);
        registry.entry(module).is_some_and(|e| !e.pending)
    };
    for (panel, hotkey) in cfg.hotkeys.panels.iter().filter(|(p, _)| allowed(p)) {
        if let Err(e) = h.register_panel(panel, hotkey) {
            tracing::warn!("hotkey {hotkey} for panel {panel}: {e}");
            errors.push((Some(Action::OpenPanel), Some(panel.clone()), e));
        }
    }
}

/// The panels the tray's Modules submenu offers: `(module/panel, title)`.
pub fn tray_panels(engine: &glossa_engine::Engine) -> Vec<(String, String)> {
    engine.panels().into_iter().filter(|p| p.open_from.iter().any(|o| o == "tray")).map(|p| (p.id, p.title)).collect()
}

/// A hotkey that could not be set up, for which action (none when the
/// hotkey service itself failed) and, for `open_panel`, which panel.
pub type HotkeyFailure = (Option<Action>, Option<String>, glossa_platform::HotkeyError);

/// A hotkey failure in the interface language, after the action's name when
/// there is one; the options page lists these.
pub fn hotkey_error_text(
    locale: &glossa_core::Locale,
    action: Option<Action>,
    panel: Option<&str>,
    e: &glossa_platform::HotkeyError,
) -> String {
    use glossa_platform::HotkeyError as E;
    let text = match e {
        E::Unsupported { reason } => locale.fmt("options.hotkeys.unsupported", reason),
        E::UnknownKey(key) => locale.fmt("options.hotkeys.unknown_key", key),
        E::Taken { hotkey } => locale.fmt("options.hotkeys.taken", hotkey),
        E::Failed { hotkey, detail } => locale.fmt_args("options.hotkeys.failed", &[hotkey, detail]),
    };
    match (action, panel) {
        (_, Some(p)) => format!("{} {p}: {text}", locale.get("action.open_panel")),
        (Some(a), None) => format!("{}: {text}", locale.get(&format!("action.{}", a.id()))),
        (None, None) => text,
    }
}

/// Gather what should wake the GUI while it is unloaded.
fn collect_wake_events(shell: &mut Shell, events: &std::sync::mpsc::Receiver<glossa_engine::Event>) {
    if let Some(h) = shell.hotkeys.as_mut() {
        while let Some((a, text)) = h.poll_with_text() {
            if text.is_some() {
                shell.pending.push(tray::Pending::Action(a, text));
            } else if actions::needs_selection(a) {
                // Read it now, before the windows exist (see `read_selection`).
                if let Some(task) = actions::read_selection(&shell.engine.config(), a) {
                    shell.pending.push(tray::Pending::Task(task));
                }
            } else {
                shell.pending.push(tray::Pending::Action(a, None));
            }
        }
    }
    if let Some(t) = &shell.tray {
        for e in t.poll() {
            match e {
                tray::TrayEvent::Action(Action::Quit) => shell.quit = true,
                tray::TrayEvent::ToggleMouseMode => toggle_mouse_mode(shell),
                tray::TrayEvent::Click(kind, mark) => {
                    use tray::ClickPlan;
                    let cfg = shell.engine.config();
                    // No GUI, so no main window showing.
                    match tray::plan_click(&cfg.tray, kind, false, mouse::MouseHub::supported()) {
                        ClickPlan::Nothing | ClickPlan::HideMain => {}
                        ClickPlan::Menu => {
                            if let Some(t) = &shell.tray {
                                t.show_menu();
                            }
                        }
                        ClickPlan::Quit => shell.quit = true,
                        ClickPlan::ToggleMouseMode => toggle_mouse_mode(shell),
                        ClickPlan::ShowMain => shell.pending.push(tray::Pending::Tray(tray::TrayEvent::ShowMain)),
                        // Read now, before the windows exist (see `read_selection`).
                        ClickPlan::Selection(a) => {
                            if let Some(task) = actions::read_selection_after_click(&cfg, a, mark) {
                                shell.pending.push(tray::Pending::Task(task));
                            }
                        }
                        ClickPlan::Perform(a) => shell.pending.push(tray::Pending::Action(a, None)),
                    }
                }
                tray::TrayEvent::ToggleHotkeys => {
                    let on = !shell.engine.config().hotkeys.enabled;
                    let _ = shell.engine.update_config(move |c| c.hotkeys.enabled = on);
                    register_hotkeys(shell);
                    if let Some(t) = &shell.tray {
                        t.set_hotkeys_checked(on);
                    }
                }
                other => shell.pending.push(tray::Pending::Tray(other)),
            }
        }
    }
    for g in shell.mouse.poll() {
        // Read now, before any window exists (see `read_selection`).
        if let Some(task) = mouse::selection_task(&shell.engine.config(), g) {
            shell.pending.push(tray::Pending::Task(task));
        }
    }
    while let Ok(ev) = events.try_recv() {
        match ev {
            glossa_engine::Event::Action { action, text } => {
                if action == Action::Quit {
                    shell.quit = true;
                } else {
                    shell.pending.push(tray::Pending::Action(action, text));
                }
            }
            // The tray's menu is up while the windows are not: it follows a
            // language chosen from the command line or the control API.
            glossa_engine::Event::ConfigChanged | glossa_engine::Event::ModulesReloaded => {
                if let Some(t) = shell.tray.as_mut() {
                    t.set_locale(&shell.engine.locale());
                    t.set_panels(tray_panels(&shell.engine));
                }
                if matches!(ev, glossa_engine::Event::ModulesReloaded) {
                    // Panel hotkeys follow which modules may run.
                    register_hotkeys(shell);
                }
            }
            _ => {}
        }
    }
}

/// Pick how windows are drawn.
///
/// The settings choose (Appearance → renderer); `GLOSSA_RENDERER` overrides
/// for one run. Measured on Windows with an NVIDIA card, OpenGL was the
/// lightest (~75 MB private), the GPU path through wgpu the heaviest
/// (~385 MB), and WARP software rendering in between in working set but
/// worse in commit (~340 MB) — which is why OpenGL is the default, and why the
/// GUI unloads entirely while it sits in the tray.
///
/// `transparent` (a backdrop or window transparency is on) matters to wgpu
/// on Windows: a swapchain made for the window's HWND, which Vulkan and
/// wgpu's DX12 default both use, is opaque, so DWM can show nothing behind
/// it. DX12 can instead present to a DirectComposition visual over the
/// window, which supports premultiplied alpha, and that is what lets Mica,
/// Acrylic and the transparency slider through.
fn choose_renderer(options: &mut eframe::NativeOptions, configured: glossa_core::config::Renderer, transparent: bool) {
    use eframe::{egui_wgpu, wgpu};
    let composited = |setup: &mut egui_wgpu::WgpuSetupCreateNew| {
        if cfg!(windows) && transparent {
            setup.instance_descriptor.backends = wgpu::Backends::DX12;
            setup.instance_descriptor.backend_options.dx12.presentation_system =
                wgpu::Dx12SwapchainKind::DxgiFromVisual;
        }
    };
    use glossa_core::config::Renderer;
    // The variable takes every spelling the config file does (`open_gl`,
    // `glow`, `gpu`, `wgpu`, `software`…); an unknown one is ignored.
    let choice = std::env::var("GLOSSA_RENDERER").ok().and_then(|v| Renderer::from_name(&v)).unwrap_or(configured);
    match choice {
        Renderer::OpenGl => options.renderer = eframe::Renderer::Glow,
        Renderer::Gpu => {
            options.renderer = eframe::Renderer::Wgpu;
            if let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
                composited(setup);
            }
        }
        _ => {
            options.renderer = eframe::Renderer::Wgpu;
            let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
            if cfg!(windows) {
                // Enumerating adapters through DXGI does not load the GPU's
                // driver; a Vulkan or GL instance would.
                setup.instance_descriptor.backends = wgpu::Backends::DX12;
            }
            setup.power_preference = wgpu::PowerPreference::LowPower;
            setup.native_adapter_selector =
                Some(std::sync::Arc::new(|adapters: &[wgpu::Adapter], _surface: Option<&wgpu::Surface<'_>>| {
                    let pick = |t: wgpu::DeviceType| adapters.iter().find(|a| a.get_info().device_type == t);
                    pick(wgpu::DeviceType::Cpu)
                        .or_else(|| pick(wgpu::DeviceType::IntegratedGpu))
                        .or(adapters.first())
                        .cloned()
                        .ok_or_else(|| "no graphics adapter".to_owned())
                }));
            composited(&mut setup);
            options.wgpu_options.wgpu_setup = egui_wgpu::WgpuSetup::CreateNew(setup);
        }
    }
}

/// Development aid: `GLOSSA_CRASH_TEST=panic|native` crashes on purpose, to
/// check what the crash loggers leave in the log.
fn crash_test() {
    match std::env::var("GLOSSA_CRASH_TEST").as_deref() {
        Ok("panic") => {
            let max = std::hint::black_box(2.0f32);
            let _ = std::hint::black_box(1.0f32).clamp(20.0, max);
        }
        Ok("native") => {
            // SAFETY: deliberately not safe; this is the crash being tested.
            unsafe { std::ptr::write_volatile(std::hint::black_box(16usize) as *mut u32, 1) };
        }
        _ => {}
    }
}

fn init_logging(paths: &Paths) {
    let filter =
        tracing_subscriber::EnvFilter::try_from_env("GLOSSA_LOG").unwrap_or_else(|_| "info,glossa=debug".into());
    let _ = std::fs::create_dir_all(&paths.cache_dir);
    match std::fs::OpenOptions::new().create(true).append(true).open(paths.log_file()) {
        Ok(file) => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .try_init();
        }
        Err(_) => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
        }
    }
}

/// With "start with the system" on, point the autostart entry at this copy
/// of the program, so it survives the program being moved or updated in a
/// new folder. An entry the user removed elsewhere is not added back, and a
/// throwaway data folder (`GLOSSA_HOME`) leaves the system alone.
fn refresh_autostart(cfg: &glossa_core::Config) {
    use glossa_platform::autostart;
    if !cfg.general.start_with_system || std::env::var_os("GLOSSA_HOME").is_some() || !autostart::is_autostart_enabled()
    {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Err(e) = autostart::set_autostart(true, &exe, &["--hidden".to_owned()]) {
            tracing::debug!("autostart entry not refreshed: {e}");
        }
    }
}

/// Ask an already running Glossa to show its main window. True if one
/// answered.
fn hand_over_to_running_instance(paths: &Paths) -> bool {
    // Only the Glossa that owns this data folder, and only once it has
    // proved to be that one: the port in the settings may be anybody's, and
    // whoever listens there must not be handed the token.
    let Some(running) = glossa_engine::server::find_running(paths).running() else { return false };
    let url = format!("http://{}/v1/action/{}", running.addr, Action::ShowMainWindow.id());
    let settings = glossa_core::config::Network {
        timeout_secs: 1,
        max_retries: 0,
        proxy: glossa_core::config::Proxy { mode: glossa_core::config::ProxyMode::None, ..Default::default() },
        ..Default::default()
    };
    let Ok(http) = glossa_net::HttpClient::new(&settings) else { return false };
    let req = glossa_net::HttpRequest::post(url)
        .header("Authorization", format!("Bearer {}", running.token))
        .timeout(Duration::from_millis(600));
    matches!(glossa_net::Http::send(&http, req), Ok(r) if r.status == 202)
}

/// The application icon, drawn rather than shipped: two overlapping speech
/// bubbles on a rounded blue square.
pub fn app_icon(size: u32) -> egui::IconData {
    tinted_icon(size, [0.16, 0.55, 0.89])
}

/// The child windows' icon, drawn once. egui tells an icon change by the
/// pointer, so a fresh copy per frame (as `chrome::builder` made) had it
/// redraw the icon and hand it to the system on every frame of every
/// open window.
pub fn window_icon() -> std::sync::Arc<egui::IconData> {
    static ICON: std::sync::OnceLock<std::sync::Arc<egui::IconData>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| std::sync::Arc::new(app_icon(32))).clone()
}

/// The icon with another base colour (the tray follows the theme's accent).
pub fn tinted_icon(size: u32, base: [f32; 3]) -> egui::IconData {
    let n = size as f32;
    let shade = |k: f32| [base[0] * k, base[1] * k, base[2] * k];
    // The back bubble is white with a hint of the base colour.
    let pale = [0.85 + base[0] * 0.15, 0.85 + base[1] * 0.15, 0.85 + base[2] * 0.15];
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let rounded = |x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32| -> f32 {
        // Signed distance to a rounded rectangle (negative inside).
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r
    };
    for py in 0..size {
        for px in 0..size {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            let mut out = [0f32; 4];
            let mut over = |col: [f32; 3], alpha: f32| {
                let a = alpha.clamp(0.0, 1.0);
                for i in 0..3 {
                    out[i] = out[i] * (1.0 - a) + col[i] * a;
                }
                out[3] = out[3] + a * (1.0 - out[3]);
            };
            // Background square, vertical gradient.
            let d = rounded(x, y, 0.0, 0.0, n, n, n * 0.22);
            let t = y / n;
            over(shade(1.0 - 0.18 * t), 0.5 - d);
            // Back bubble (light), offset up-left, with a tail.
            let d1 = rounded(x, y, n * 0.14, n * 0.17, n * 0.62, n * 0.52, n * 0.08).min(tail(
                x,
                y,
                n * 0.22,
                n * 0.50,
                n * 0.32,
                n * 0.50,
                n * 0.20,
                n * 0.62,
            ));
            over(pale, 0.5 - d1);
            // Front bubble (white), offset down-right.
            let d2 = rounded(x, y, n * 0.38, n * 0.42, n * 0.86, n * 0.77, n * 0.08).min(tail(
                x,
                y,
                n * 0.68,
                n * 0.75,
                n * 0.78,
                n * 0.75,
                n * 0.80,
                n * 0.88,
            ));
            over([1.0, 1.0, 1.0], 0.5 - d2);
            // Two "text" bars on each bubble.
            for (bx0, by, bx1) in [(0.22, 0.29, 0.52), (0.22, 0.38, 0.44), (0.46, 0.55, 0.78), (0.46, 0.64, 0.70)] {
                let db = rounded(x, y, n * bx0, n * by - n * 0.025, n * bx1, n * by + n * 0.025, n * 0.025);
                over(shade(0.92), 0.5 - db);
            }
            let i = ((py * size + px) * 4) as usize;
            rgba[i] = (out[0] * 255.0) as u8;
            rgba[i + 1] = (out[1] * 255.0) as u8;
            rgba[i + 2] = (out[2] * 255.0) as u8;
            rgba[i + 3] = (out[3] * 255.0) as u8;
        }
    }
    egui::IconData { rgba, width: size, height: size }
}

/// The menu-bar icon on macOS: the two bubbles as a black mask (the text
/// bars cut out), which macOS paints to suit a light or dark menu bar.
pub fn template_icon(size: u32) -> egui::IconData {
    let n = size as f32;
    let rounded = |x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32| -> f32 {
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r
    };
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for px in 0..size {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            // The same layout as the app icon, spread over the whole square.
            let back = rounded(x, y, n * 0.02, n * 0.06, n * 0.64, n * 0.52, n * 0.1).min(tail(
                x,
                y,
                n * 0.12,
                n * 0.50,
                n * 0.26,
                n * 0.50,
                n * 0.10,
                n * 0.66,
            ));
            let front = rounded(x, y, n * 0.34, n * 0.38, n * 0.98, n * 0.84, n * 0.1).min(tail(
                x,
                y,
                n * 0.74,
                n * 0.82,
                n * 0.88,
                n * 0.82,
                n * 0.90,
                n * 0.98,
            ));
            let mut a = (0.5 - front).clamp(0.0, 1.0);
            // The back bubble shows only outside the front one, a little lighter.
            a = a.max((0.5 - back).clamp(0.0, 1.0) * 0.6 * (1.0 - (0.5 - front + 1.5).clamp(0.0, 1.0)));
            for (bx0, by, bx1) in [(0.44, 0.55, 0.86), (0.44, 0.67, 0.72)] {
                let bar = rounded(x, y, n * bx0, n * by - n * 0.035, n * bx1, n * by + n * 0.035, n * 0.035);
                a *= 1.0 - (0.5 - bar).clamp(0.0, 1.0);
            }
            rgba[((py * size + px) * 4 + 3) as usize] = (a * 255.0) as u8;
        }
    }
    egui::IconData { rgba, width: size, height: size }
}

/// Signed distance to a triangle (a bubble's tail), approximately.
#[allow(clippy::too_many_arguments)] // three points, spelled out
fn tail(x: f32, y: f32, ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32) -> f32 {
    let edge = |px: f32, py: f32, qx: f32, qy: f32| {
        let (ex, ey) = (qx - px, qy - py);
        let len = (ex * ex + ey * ey).sqrt().max(1e-3);
        ((x - px) * ey - (y - py) * ex) / len
    };
    let d = edge(ax, ay, bx, by).max(edge(bx, by, cx, cy)).max(edge(cx, cy, ax, ay));
    // Orientation-agnostic: flip if the triangle winds the other way.
    let area = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
    if area < 0.0 {
        -edge(ax, ay, bx, by).min(-edge(bx, by, cx, cy)).min(-edge(cx, cy, ax, ay))
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    /// Writes the application icon as a PNG when `GLOSSA_ICON_OUT` names a
    /// file, for the README and installers:
    /// `GLOSSA_ICON_OUT=assets/icons/glossa.png cargo test -p glossa-ui-egui export_icon -- --ignored`
    #[test]
    fn template_icon_is_a_mask_with_the_bars_cut_out() {
        let icon = super::template_icon(36);
        let alpha = |x: u32, y: u32| icon.rgba[((y * 36 + x) * 4 + 3) as usize];
        assert!(icon.rgba.chunks(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0), "black only");
        assert_eq!(alpha(0, 35), 0, "corner is clear");
        assert!(alpha(16, 16) > 200, "front bubble is solid");
        assert!(alpha(25, 20) < 60, "text bar is cut out");
        // Written out for a look when GLOSSA_TEMPLATE_OUT names a file.
        if let Some(out) = std::env::var_os("GLOSSA_TEMPLATE_OUT") {
            let big = super::template_icon(256);
            image::RgbaImage::from_raw(256, 256, big.rgba).unwrap().save(out).unwrap();
        }
    }

    #[test]
    #[ignore]
    fn export_icon() {
        let Some(out) = std::env::var_os("GLOSSA_ICON_OUT") else { return };
        let out = std::path::PathBuf::from(out);
        let png = |size: u32| {
            let icon = super::app_icon(size);
            let mut bytes = Vec::new();
            image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba)
                .unwrap()
                .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
                .unwrap();
            bytes
        };
        std::fs::write(&out, png(256)).unwrap();
        // The same as a Windows .ico (PNG frames), for the executable's
        // resources (build.rs).
        let sizes = [16u32, 24, 32, 48, 64, 128, 256];
        let frames: Vec<Vec<u8>> = sizes.iter().map(|&s| png(s)).collect();
        let mut ico = Vec::new();
        ico.extend_from_slice(&[0, 0, 1, 0]);
        ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
        let mut offset = 6 + 16 * sizes.len() as u32;
        for (size, frame) in sizes.iter().zip(&frames) {
            let side = if *size >= 256 { 0 } else { *size as u8 };
            ico.extend_from_slice(&[side, side, 0, 0, 1, 0, 32, 0]);
            ico.extend_from_slice(&(frame.len() as u32).to_le_bytes());
            ico.extend_from_slice(&offset.to_le_bytes());
            offset += frame.len() as u32;
        }
        for frame in &frames {
            ico.extend_from_slice(frame);
        }
        std::fs::write(out.with_extension("ico"), ico).unwrap();
    }
}
