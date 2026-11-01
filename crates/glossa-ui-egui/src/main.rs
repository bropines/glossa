//! Glossa — the desktop application.
//!
//! Lives in the tray; a hotkey over selected text in any program opens a
//! popup with the translation, as the classic QTranslate did. Also: a main
//! translation window, screen-region OCR, a dictionary, history, and options.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod about;
mod actions;
mod app;
mod chrome;
mod dictionary;
mod fonts;
mod history;
mod icons;
mod keyboard;
mod main_window;
mod mouse;
mod ocr;
mod options;
mod popup;
mod rich;
mod style;
mod tray;
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
            "--popup" => l.popup_text = it.next(),
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
    pub hotkey_errors: Vec<String>,
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
    let paths = match &launch.home {
        Some(h) => Paths::for_exe_dir(
            std::env::current_exe().ok().and_then(|p| p.parent().map(Into::into)).unwrap_or_default(),
            Some(h.clone()),
        ),
        None => Paths::discover(),
    };
    init_logging(&paths);
    // Release builds abort on panic; leave a trace in the log first.
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!(
            "panic: {info}
{bt}"
        );
    }));

    // A second launch hands over to the running instance through its control
    // API, then leaves.
    if launch.screenshot.is_none() && hand_over_to_running_instance(&paths) {
        tracing::info!("another Glossa is running; asked it to show its window");
        return Ok(());
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

    let pump = glossa_platform::idle::IdlePump::for_current_thread();
    let wake = Arc::new(Wake { ctx: parking_lot::Mutex::new(None), idle: pump.waker() });
    let shell = build_shell(engine.clone(), wake.clone());
    let slot: Rc<RefCell<Option<Shell>>> = Rc::new(RefCell::new(Some(shell)));
    // Engine events seen while no GUI is loaded (the control API asking for
    // an action).
    let idle_events = engine.subscribe();

    let cfg = engine.config();
    let can_unload = glossa_platform::idle::IdlePump::supported() && launch.screenshot.is_none();
    let mut start_visible = !launch.hidden
        && (launch.screenshot.is_some()
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
            .with_icon(app_icon(64));
        if let Some(g) = geometry {
            viewport = viewport.with_position([g.x, g.y]);
            if g.maximized {
                viewport = viewport.with_maximized(true);
            }
        }
        let mut options =
            eframe::NativeOptions { viewport, run_and_return: can_unload, persist_window: false, ..Default::default() };
        choose_renderer(&mut options, cfg.appearance.renderer);
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
        // Only now: the new copy finds the control port free and starts for
        // real instead of handing over to this one.
        let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--hidden").collect();
        match std::env::current_exe().map(|exe| std::process::Command::new(exe).args(&args).spawn()) {
            Ok(Ok(_)) => tracing::info!("restarting"),
            Ok(Err(e)) | Err(e) => tracing::error!("could not restart: {e}"),
        }
    }
    Ok(())
}

fn launch_once_only(start_visible: &mut bool) {
    *start_visible = false;
}

/// The parts that live as long as the process.
/// The tray icon in the theme's accent colour, changed while mouse mode is
/// on, with the menu's check marks.
pub fn refresh_tray_look(shell: &Shell) {
    let Some(t) = &shell.tray else { return };
    let cfg = shell.engine.config();
    t.set_look(tray::accent_of(&shell.engine), cfg.mouse_mode.enabled);
}

/// Mouse mode on or off from the tray, with no GUI needed.
fn toggle_mouse_mode(shell: &mut Shell) {
    let on = !shell.engine.config().mouse_mode.enabled;
    let _ = shell.engine.update_config(move |c| c.mouse_mode.enabled = on);
    shell.mouse.sync(on);
    refresh_tray_look(shell);
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
            hotkey_errors.push(hotkey_error_text(&engine.locale(), None, &e));
            None
        }
    };
    let tray = {
        let w = wake.clone();
        tray::Tray::new(Arc::new(move || w.wake()), &engine.locale())
            .map_err(|e| tracing::warn!("no tray icon: {e}"))
            .ok()
    };
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
    };
    register_hotkeys(&mut shell);
    shell.mouse.sync(cfg.mouse_mode.enabled);
    refresh_tray_look(&shell);
    shell
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
            let text = hotkey_error_text(&shell.engine.locale(), Some(*action), &e);
            shell.hotkey_errors.push(text);
        }
    }
}

/// A hotkey failure in the interface language, after the action's name when
/// there is one; the options page lists these.
pub fn hotkey_error_text(
    locale: &glossa_core::Locale,
    action: Option<Action>,
    e: &glossa_platform::HotkeyError,
) -> String {
    use glossa_platform::HotkeyError as E;
    let text = match e {
        E::Unsupported { reason } => locale.fmt("options.hotkeys.unsupported", reason),
        E::UnknownKey(key) => locale.fmt("options.hotkeys.unknown_key", key),
        E::Taken { hotkey } => locale.fmt("options.hotkeys.taken", hotkey),
        E::Failed { hotkey, detail } => locale.fmt_args("options.hotkeys.failed", &[hotkey, detail]),
    };
    match action {
        Some(a) => format!("{}: {text}", locale.get(&format!("action.{}", a.id()))),
        None => text,
    }
}

/// Gather what should wake the GUI while it is unloaded.
fn collect_wake_events(shell: &mut Shell, events: &std::sync::mpsc::Receiver<glossa_engine::Event>) {
    if let Some(h) = shell.hotkeys.as_mut() {
        while let Some(a) = h.poll() {
            if actions::needs_selection(a) {
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
                tray::TrayEvent::Click => {
                    if shell.engine.config().mouse_mode.tray_click_toggles {
                        toggle_mouse_mode(shell);
                    } else {
                        shell.pending.push(tray::Pending::Tray(tray::TrayEvent::ShowMain));
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
        if let glossa_engine::Event::Action { action, text } = ev {
            if action == Action::Quit {
                shell.quit = true;
            } else {
                shell.pending.push(tray::Pending::Action(action, text));
            }
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
fn choose_renderer(options: &mut eframe::NativeOptions, configured: glossa_core::config::Renderer) {
    use eframe::{egui_wgpu, wgpu};
    let choice = std::env::var("GLOSSA_RENDERER").unwrap_or_else(|_| configured.as_str().into());
    match choice.as_str() {
        "glow" | "opengl" | "gl" => options.renderer = eframe::Renderer::Glow,
        "gpu" | "wgpu" => options.renderer = eframe::Renderer::Wgpu,
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
            options.wgpu_options.wgpu_setup = egui_wgpu::WgpuSetup::CreateNew(setup);
        }
    }
    // A DXGI swap chain made from the window handle is always opaque, so
    // under wgpu a translucent window came out solid. One made for a
    // DirectComposition visual over the window carries alpha.
    if cfg!(windows) && options.renderer == eframe::Renderer::Wgpu && options.viewport.transparent == Some(true) {
        if let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
            setup.instance_descriptor.backends = wgpu::Backends::DX12;
            setup.instance_descriptor.backend_options.dx12.presentation_system =
                wgpu::Dx12SwapchainKind::DxgiFromVisual;
        }
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

/// Ask an already running Glossa to show its main window. True if one
/// answered.
fn hand_over_to_running_instance(paths: &Paths) -> bool {
    let Ok(cfg) = glossa_core::Config::load(&paths.config_file()) else { return false };
    if !cfg.server.enabled || cfg.server.token.is_empty() {
        return false;
    }
    // Is anything listening at all? Binding the port answers at once, where a
    // connection to a closed port on Windows waits for its timeout.
    if std::net::TcpListener::bind(&cfg.server.bind).is_ok() {
        return false;
    }
    let url =
        format!("http://{}/v1/action/{}?token={}", cfg.server.bind, Action::ShowMainWindow.id(), cfg.server.token);
    let settings = glossa_core::config::Network {
        timeout_secs: 1,
        max_retries: 0,
        proxy: glossa_core::config::Proxy { mode: glossa_core::config::ProxyMode::None, ..Default::default() },
        ..Default::default()
    };
    let Ok(http) = glossa_net::HttpClient::new(&settings) else { return false };
    let req = glossa_net::HttpRequest::get(url).timeout(Duration::from_millis(600));
    matches!(glossa_net::Http::send(&http, req), Ok(r) if r.status == 202)
}

/// The application icon, drawn rather than shipped: two overlapping speech
/// bubbles on a rounded blue square.
pub fn app_icon(size: u32) -> egui::IconData {
    tinted_icon(size, [0.16, 0.55, 0.89])
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
