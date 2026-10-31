//! Glossa — the desktop application.
//!
//! Lives in the tray; a hotkey over selected text in any program opens a
//! popup with the translation, as the classic QTranslate did. Also: a main
//! translation window, screen-region OCR, a dictionary, history, and options.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod app;
mod dictionary;
mod fonts;
mod history;
mod icons;
mod main_window;
mod ocr;
mod options;
mod popup;
mod rich;
mod style;
mod tray;
mod widgets;

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

fn main() -> eframe::Result<()> {
    let launch = parse_args();
    let paths = match &launch.home {
        Some(h) => Paths::for_exe_dir(std::env::current_exe().ok().and_then(|p| p.parent().map(Into::into)).unwrap_or_default(), Some(h.clone())),
        None => Paths::discover(),
    };
    init_logging(&paths);

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

    let cfg = engine.config();
    let geometry = cfg.windows.get("main").copied();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Glossa")
        .with_app_id("glossa")
        .with_inner_size(geometry.map(|g| [g.width, g.height]).unwrap_or([680.0, 470.0]))
        .with_min_inner_size([420.0, 300.0])
        .with_decorations(!cfg.appearance.custom_frame)
        .with_icon(app_icon(64));
    if let Some(g) = geometry {
        viewport = viewport.with_position([g.x, g.y]);
        if g.maximized {
            viewport = viewport.with_maximized(true);
        }
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        run_and_return: false,
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "Glossa",
        options,
        Box::new(move |cc| Ok(Box::new(app::GlossaApp::new(cc, engine, launch)))),
    )
}

fn init_logging(paths: &Paths) {
    let filter = tracing_subscriber::EnvFilter::try_from_env("GLOSSA_LOG").unwrap_or_else(|_| "info,glossa=debug".into());
    let _ = std::fs::create_dir_all(&paths.cache_dir);
    match std::fs::OpenOptions::new().create(true).append(true).open(paths.log_file()) {
        Ok(file) => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(std::sync::Mutex::new(file)).try_init();
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
    let url = format!("http://{}/v1/action/{}?token={}", cfg.server.bind, Action::ShowMainWindow.id(), cfg.server.token);
    let http = glossa_net::HttpClient::plain();
    let req = glossa_net::HttpRequest::get(url).timeout(Duration::from_millis(600));
    matches!(glossa_net::Http::send(&http, req), Ok(r) if r.status == 202)
}

/// The application icon, drawn rather than shipped: two overlapping speech
/// bubbles on a rounded blue square.
pub fn app_icon(size: u32) -> egui::IconData {
    let n = size as f32;
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
            over([0.16 - 0.06 * t, 0.55 - 0.12 * t, 0.89 - 0.10 * t], 0.5 - d);
            // Back bubble (light), offset up-left, with a tail.
            let d1 = rounded(x, y, n * 0.14, n * 0.17, n * 0.62, n * 0.52, n * 0.08)
                .min(tail(x, y, n * 0.22, n * 0.50, n * 0.32, n * 0.50, n * 0.20, n * 0.62));
            over([0.85, 0.93, 1.0], 0.5 - d1);
            // Front bubble (white), offset down-right.
            let d2 = rounded(x, y, n * 0.38, n * 0.42, n * 0.86, n * 0.77, n * 0.08)
                .min(tail(x, y, n * 0.68, n * 0.75, n * 0.78, n * 0.75, n * 0.80, n * 0.88));
            over([1.0, 1.0, 1.0], 0.5 - d2);
            // Two "text" bars on each bubble.
            for (bx0, by, bx1) in [(0.22, 0.29, 0.52), (0.22, 0.38, 0.44), (0.46, 0.55, 0.78), (0.46, 0.64, 0.70)] {
                let db = rounded(x, y, n * bx0, n * by - n * 0.025, n * bx1, n * by + n * 0.025, n * 0.025);
                over([0.16, 0.50, 0.85], 0.5 - db);
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
