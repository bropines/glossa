//! `--screenshot WHAT FILE`: open one window, let it settle, save it as a
//! PNG and exit. Documentation pictures are made this way, and CI's smoke
//! test (`scripts/screenshot-smoke.sh`) renders every window with it.
//!
//! The run fails, with its own exit code, when the picture is blank (a
//! window that drew nothing, or a renderer that failed silently), when a
//! string had no text in any language (it would show its bare key), or
//! when nothing arrives in time. A crash fails it anyway.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use egui::{ColorImage, ViewportCommand, ViewportId};

use crate::app::GlossaApp;

/// Exit codes, for the smoke test to tell failures apart.
pub const EXIT_BLANK: i32 = 2;
pub const EXIT_TIMEOUT: i32 = 3;
pub const EXIT_MISSING_KEYS: i32 = 4;
pub const EXIT_WRITE: i32 = 5;
pub const EXIT_UNKNOWN: i32 = 6;

/// Frames drawn before the picture is taken: fonts for other scripts and
/// the window's own size arrive a frame or two after it opens.
const SETTLE_FRAMES: u32 = 10;
/// The frame at which `GLOSSA_SCREENSHOT_SWITCH` changes the language: the
/// window is open by then, and the rest of the settling follows it.
const SWITCH_FRAME: u32 = 3;
/// And at least this long, for a new window to be mapped and composited.
const SETTLE_TIME: Duration = Duration::from_millis(1500);
/// A run that has not finished by then is stuck; the watchdog ends it.
/// Counted from the start of the process, so a renderer that deadlocks
/// before the first frame is caught too.
const DEADLINE: Duration = Duration::from_secs(90);

/// Real windows show 145 colours or more (an empty History, the plainest,
/// on macOS); any text brings dozens of anti-aliased shades.
const MIN_COLOURS: usize = 64;
/// Below the top [`TOP_BAND`] of the picture (title bars, tab strips), at
/// least this share of pixels differs from the background. Real windows
/// measured 1.2% (an empty History) and up; a window whose body drew
/// nothing has none.
const MIN_CONTENT: f64 = 0.005;
const TOP_BAND: f64 = 0.15;

/// Options states rather than pages, as `options::open_at` takes them: the
/// font list open, the note after starting at login is turned on, and the
/// Presets tab of the Services page.
const SPECIAL_OPTIONS: [&str; 4] = ["font", "autostart", "presets", "proxy"];

/// Every window `--screenshot` can draw, as `--screenshot-list` prints them.
pub fn targets() -> Vec<String> {
    let mut all: Vec<String> = [
        "main",
        "popup",
        "dictionary",
        "history",
        "keyboard",
        "about",
        "welcome",
        "ocr-result",
        "module-editor",
        "module-editor:test",
    ]
    .map(String::from)
    .to_vec();
    all.extend(crate::options::page_names().into_iter().map(|p| format!("options:{p}")));
    all.extend(SPECIAL_OPTIONS.map(|v| format!("options:{v}")));
    // `panel:MODULE/PANEL` also works, for a loaded module's panel; it is
    // not listed, since which panels exist depends on the data folder.
    all
}

/// Called at the end of every frame while `--screenshot` is set.
pub fn flow(app: &mut GlossaApp, ctx: &egui::Context, what: &str, path: &Path) {
    static START: OnceLock<Instant> = OnceLock::new();
    static FRAMES: AtomicU32 = AtomicU32::new(0);
    static SHOT: AtomicBool = AtomicBool::new(false);

    let start = *START.get_or_init(|| {
        if !open(app, what) {
            eprintln!("glossa: --screenshot: unknown window {what:?}; known: {}", targets().join(", "));
            std::process::exit(EXIT_UNKNOWN);
        }
        Instant::now()
    });
    let Some(target) = viewport(what) else { return };
    let frames = FRAMES.fetch_add(1, Ordering::SeqCst);
    // `GLOSSA_SCREENSHOT_SWITCH=ru`: change the interface language once the
    // window is up, the way Options does, so the picture shows whether
    // everything followed without a restart.
    if frames == SWITCH_FRAME {
        if let Ok(lang) = std::env::var("GLOSSA_SCREENSHOT_SWITCH") {
            if let Err(e) = app.engine.update_config(move |c| c.general.ui_language = lang) {
                eprintln!("glossa: --screenshot: cannot switch the language: {e}");
            }
        }
    }
    // `GLOSSA_SCREENSHOT_ERROR=credential|server|files`: show that failure
    // in the window pictured, to look at the message, its buttons and the
    // window's own toast without a service that really fails.
    if frames == SWITCH_FRAME {
        if let Ok(kind) = std::env::var("GLOSSA_SCREENSHOT_ERROR") {
            show_error(app, target, &kind);
        }
    }
    if !SHOT.load(Ordering::SeqCst) && frames >= SETTLE_FRAMES && start.elapsed() >= SETTLE_TIME {
        SHOT.store(true, Ordering::SeqCst);
        let path = path.to_owned();
        ctx.send_viewport_cmd_to(
            target,
            ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |image| finish(&image, &path))),
        );
    }
    // Keep frames coming until the picture is taken: nothing else would
    // wake an idle window.
    ctx.request_repaint();
    ctx.request_repaint_of(target);
}

/// Put a sample failure of `kind` into the window `target`: as its toast,
/// and in the main window, the popup and the dictionary as the failed
/// request too.
fn show_error(app: &mut GlossaApp, target: ViewportId, kind: &str) {
    use glossa_core::Error;
    let e = match kind {
        "credential" => Error::MissingCredential { provider: "DeepL".into(), what: "api_key".into() },
        "files" => Error::AssetsMissing { module: "local-llm".into(), missing: vec!["model.gguf".into()] },
        _ => Error::HttpStatus { provider: "DeepL".into(), status: 503, body: "upstream connect error".into() },
    };
    let view = crate::errors::ErrorView::new(&app.locale, &e);
    // A job number no reply carries: the real answer, should one still
    // come, does not replace the failure.
    app.main.job = u64::MAX;
    app.main.busy = false;
    app.main.outcome = None;
    app.main.error = Some(view.clone());
    if let Some(p) = app.popup.as_mut() {
        (p.job, p.busy, p.outcome, p.error) = (u64::MAX, false, None, Some(view.clone()));
    }
    if let Some(d) = app.dictionary.as_mut() {
        (d.job, d.busy, d.article, d.error) = (u64::MAX, false, None, Some(view.clone()));
    }
    let toast = crate::toast::Toast::new(view.headline, view.detail, true, app.frame_number());
    app.toasts.insert(target, toast);
}

/// Open the window named `what`; false for a name not known.
fn open(app: &mut GlossaApp, what: &str) -> bool {
    match what {
        "main" => {
            if app.main.source.is_empty() {
                app.main.source = "Hello! This is Glossa, the resurrected QTranslate.".into();
            }
            app.translate_main();
        }
        "options" => crate::options::open(app),
        w if w.starts_with("options:") => {
            let page = &w["options:".len()..];
            if !crate::options::page_names().contains(&page) && !SPECIAL_OPTIONS.contains(&page) {
                return false;
            }
            crate::options::open_at(app, page);
            // `GLOSSA_SCREENSHOT_SERVICE=<id>`: that provider's settings on
            // the Services page; `GLOSSA_SCREENSHOT_TAB=<tabs id>=<n>`: the
            // n-th tab (from 0) of a `tabs` item on its page.
            if let Ok(id) = std::env::var("GLOSSA_SCREENSHOT_SERVICE") {
                crate::options::select_service(app, &id);
                if let Some((tabs, n)) =
                    std::env::var("GLOSSA_SCREENSHOT_TAB").ok().as_deref().and_then(|t| t.split_once('='))
                {
                    crate::module_settings::select_tab(&app.ctx, &id, tabs, n.parse().unwrap_or(0));
                }
                // `GLOSSA_SCREENSHOT_ACTION=<action id>`: press that action's
                // button when the page is first drawn, to show its result.
                if let Ok(action) = std::env::var("GLOSSA_SCREENSHOT_ACTION") {
                    crate::module_actions::press(&id, &action);
                }
            }
        }
        "dictionary" => crate::dictionary::open(app, Some("serendipity".into())),
        "history" => crate::history::open(app),
        "module-editor" | "module-editor:test" => {
            // `GLOSSA_SCREENSHOT_MODULE`: a module folder to open; else the
            // translator template, written to a scratch folder rather than
            // the user's modules.
            let dir = match std::env::var_os("GLOSSA_SCREENSHOT_MODULE") {
                Some(dir) => std::path::PathBuf::from(dir),
                None => {
                    let dir = std::env::temp_dir().join(format!("glossa-screenshot-module-{}", std::process::id()));
                    let _ = std::fs::remove_dir_all(&dir);
                    let t = glossa_engine::module_dev::Template::Translator;
                    if let Err(e) =
                        glossa_engine::module_dev::write_template(t, &dir, "screenshot-module", "Screenshot module")
                    {
                        eprintln!("glossa: --screenshot: cannot write the sample module: {e}");
                    }
                    dir
                }
            };
            crate::module_editor::open(app, crate::module_editor::Target::Dir(dir));
            // `GLOSSA_SCREENSHOT_TAB=<tabs id>=<n>`: that tab of the preview.
            if let Some((tabs, n)) =
                std::env::var("GLOSSA_SCREENSHOT_TAB").ok().as_deref().and_then(|t| t.split_once('='))
            {
                let salt = crate::module_editor::PREVIEW_SALT;
                crate::module_settings::select_tab(&app.ctx, salt, tabs, n.parse().unwrap_or(0));
            }
            // `GLOSSA_SCREENSHOT_ACTION`: press that action in the preview.
            if let Ok(action) = std::env::var("GLOSSA_SCREENSHOT_ACTION") {
                crate::module_actions::press(crate::module_editor::PREVIEW_SALT, &action);
            }
            if what.ends_with(":test") {
                if let Some(ed) = app.module_editor.as_mut() {
                    ed.test_when_ready();
                }
            }
        }
        // A module's panel; `GLOSSA_SCREENSHOT_ACTION` runs one of its
        // actions once it is open, to show what it draws.
        w if w.starts_with("panel:") => {
            let Ok(id) = glossa_engine::PanelId::parse(&w["panel:".len()..]) else { return false };
            crate::module_panel::open(app, &id);
            let Some(win) = app.panel_windows.iter_mut().find(|p| p.panel.id == id) else { return false };
            if let Ok(action) = std::env::var("GLOSSA_SCREENSHOT_ACTION") {
                win.press(&action);
            }
        }
        "about" => app.about_open = true,
        "welcome" => crate::welcome::open(app),
        "keyboard" => app.keyboard_open = true,
        "popup" => crate::popup::open(app, "The quick brown fox jumps over the lazy dog.".into(), Some((400, 300))),
        "ocr-result" => {
            let to = crate::ocr::target_language(app);
            let region = glossa_core::ocr::Rect::new(200.0, 200.0, 320.0, 120.0);
            crate::ocr_result::open(app, sample_picture(), region, glossa_core::LangId::AUTO, Some(to));
        }
        _ => return false,
    }
    true
}

/// The viewport that shows `what`.
fn viewport(what: &str) -> Option<ViewportId> {
    use crate::chrome::{viewport, ABOUT, DICTIONARY, HISTORY, KEYBOARD, OPTIONS, WELCOME};
    Some(match what {
        "main" => ViewportId::ROOT,
        "popup" => crate::popup::viewport_id(),
        w if w == "options" || w.starts_with("options:") => viewport(OPTIONS),
        "dictionary" => viewport(DICTIONARY),
        "history" => viewport(HISTORY),
        "module-editor" | "module-editor:test" => viewport(crate::module_editor::VIEWPORT),
        "keyboard" => viewport(KEYBOARD),
        "about" => viewport(ABOUT),
        "welcome" => viewport(WELCOME),
        "ocr-result" => viewport(crate::ocr_result::VIEWPORT),
        w if w.starts_with("panel:") => {
            crate::module_panel::viewport_of(&glossa_engine::PanelId::parse(&w["panel:".len()..]).ok()?)
        }
        _ => return None,
    })
}

/// A stand-in for a captured region: dark bars on light paper, roughly
/// where lines of text would be. Whatever the OCR engine makes of it, the
/// window has a picture to show.
fn sample_picture() -> image::RgbaImage {
    image::RgbaImage::from_fn(320, 120, |x, y| {
        let line = (y / 20) % 2 == 1 && (20..300).contains(&x) && (x / 14) % 5 != 4;
        if line {
            image::Rgba([30, 30, 30, 255])
        } else {
            image::Rgba([245, 245, 240, 255])
        }
    })
}

/// End the run if it is stuck: a renderer that never drew a frame, a window
/// that never opened, a picture that never arrived. Started as soon as
/// `--screenshot` is seen, before any window or graphics context exists.
pub fn watchdog() {
    std::thread::spawn(|| {
        std::thread::sleep(DEADLINE);
        eprintln!("glossa: --screenshot: no picture after {}s", DEADLINE.as_secs());
        std::process::exit(EXIT_TIMEOUT);
    });
}

/// Check and save the picture, then exit.
fn finish(image: &ColorImage, path: &Path) {
    let missing = glossa_core::locale::missing_keys();
    if let Some(why) = blank(image) {
        eprintln!("glossa: --screenshot: the picture is blank ({why})");
        // Saved anyway, for whoever looks into it.
        let _ = save(image, path);
        std::process::exit(EXIT_BLANK);
    }
    if let Err(e) = save(image, path) {
        eprintln!("glossa: --screenshot: cannot write {}: {e}", path.display());
        std::process::exit(EXIT_WRITE);
    }
    if !missing.is_empty() {
        eprintln!("glossa: --screenshot: strings with no text in any language: {}", missing.join(", "));
        std::process::exit(EXIT_MISSING_KEYS);
    }
    eprintln!("glossa: --screenshot: saved {} ({}x{})", path.display(), image.width(), image.height());
    std::process::exit(0);
}

fn save(image: &ColorImage, path: &Path) -> image::ImageResult<()> {
    let [w, h] = image.size;
    image::save_buffer(path, image.as_raw(), w as u32, h as u32, image::ColorType::Rgba8)
}

/// Why `image` shows nothing, or `None` when it shows something.
fn blank(image: &ColorImage) -> Option<String> {
    let [w, h] = image.size;
    if w == 0 || h == 0 {
        return Some("no pixels".into());
    }
    let mut counts = std::collections::HashMap::new();
    for p in &image.pixels {
        *counts.entry(*p).or_insert(0usize) += 1;
    }
    match counts.len() {
        1 => return Some("a single colour".into()),
        n if n < MIN_COLOURS => return Some(format!("only {n} colours")),
        _ => {}
    }
    let background = counts.into_iter().max_by_key(|&(_, n)| n).map(|(c, _)| c)?;
    let body = &image.pixels[((h as f64 * TOP_BAND) as usize) * w..];
    let content = body.iter().filter(|&&p| p != background).count() as f64 / body.len().max(1) as f64;
    if content < MIN_CONTENT {
        return Some(format!("{:.2}% of the body differs from the background", content * 100.0));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    fn filled(w: usize, h: usize, colour: Color32) -> ColorImage {
        ColorImage::new([w, h], vec![colour; w * h])
    }

    #[test]
    fn empty_and_flat_pictures_are_blank() {
        assert_eq!(blank(&filled(0, 0, Color32::BLACK)).as_deref(), Some("no pixels"));
        assert_eq!(blank(&filled(40, 30, Color32::from_gray(0x20))).as_deref(), Some("a single colour"));
        let mut two = filled(40, 30, Color32::WHITE);
        two.pixels[5] = Color32::RED;
        assert_eq!(blank(&two).as_deref(), Some("only 2 colours"));
    }

    /// Shades of grey drawn as text would be, in rows starting at `top`.
    fn with_text(mut img: ColorImage, top: usize, rows: usize) -> ColorImage {
        let w = img.width();
        for y in top..top + rows {
            for x in 0..w / 2 {
                img.pixels[y * w + x] = Color32::from_gray(((x * 7 + y) % 200) as u8);
            }
        }
        img
    }

    #[test]
    fn a_picture_with_text_in_its_body_is_not_blank() {
        let img = with_text(filled(200, 100, Color32::WHITE), 50, 4);
        assert_eq!(blank(&img), None);
    }

    #[test]
    fn few_colours_or_an_empty_body_are_blank() {
        // A dozen shades on the background: a frame and a border, no text.
        let mut img = filled(200, 100, Color32::WHITE);
        for (i, p) in img.pixels.iter_mut().take(12).enumerate() {
            *p = Color32::from_gray(i as u8 * 10);
        }
        assert_eq!(blank(&img).as_deref(), Some("only 13 colours"));
        // Plenty of colours, but all in the title bar.
        let img = with_text(filled(200, 100, Color32::WHITE), 0, 10);
        assert!(blank(&img).is_some_and(|why| why.contains("body")), "{:?}", blank(&img));
        // Black with a few stray pixels is still blank.
        let mut img = filled(200, 100, Color32::BLACK);
        img.pixels[9_000] = Color32::RED;
        assert!(blank(&img).is_some());
    }

    #[test]
    fn every_target_has_a_viewport() {
        for t in targets() {
            assert!(viewport(&t).is_some(), "{t}");
        }
        assert!(viewport("nonsense").is_none());
        assert!(targets().iter().any(|t| t == "options:appearance"));
    }

    #[test]
    fn the_sample_picture_has_ink_and_paper() {
        let img = sample_picture();
        let dark = img.pixels().filter(|p| p.0[0] < 100).count();
        assert!(dark > 1000 && dark < img.pixels().count() / 2);
    }
}
