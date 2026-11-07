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
/// And at least this long, for a new window to be mapped and composited.
const SETTLE_TIME: Duration = Duration::from_millis(1500);
/// A run that has not finished by then is stuck; the watchdog ends it.
const DEADLINE: Duration = Duration::from_secs(60);

/// A picture with fewer colours than this drew nothing worth the name:
/// any text brings dozens of anti-aliased shades.
const MIN_COLOURS: usize = 8;

/// Options states rather than pages, as `options::open_at` takes them: the
/// font list open, and the note after starting at login is turned on.
const SPECIAL_OPTIONS: [&str; 2] = ["font", "autostart"];

/// Every window `--screenshot` can draw, as `--screenshot-list` prints them.
pub fn targets() -> Vec<String> {
    let mut all: Vec<String> = ["main", "popup", "dictionary", "history", "keyboard", "about", "welcome", "ocr-result"]
        .map(String::from)
        .to_vec();
    all.extend(crate::options::page_names().into_iter().map(|p| format!("options:{p}")));
    all.extend(SPECIAL_OPTIONS.map(|v| format!("options:{v}")));
    all
}

/// Called at the end of every frame while `--screenshot` is set.
pub fn flow(app: &mut GlossaApp, ctx: &egui::Context, what: &str, path: &Path) {
    static START: OnceLock<Instant> = OnceLock::new();
    static FRAMES: AtomicU32 = AtomicU32::new(0);
    static SHOT: AtomicBool = AtomicBool::new(false);

    let start = *START.get_or_init(|| {
        watchdog();
        if !open(app, what) {
            eprintln!("glossa: --screenshot: unknown window {what:?}; known: {}", targets().join(", "));
            std::process::exit(EXIT_UNKNOWN);
        }
        Instant::now()
    });
    let Some(target) = viewport(what) else { return };
    let frames = FRAMES.fetch_add(1, Ordering::SeqCst);
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
        }
        "dictionary" => crate::dictionary::open(app, Some("serendipity".into())),
        "history" => crate::history::open(app),
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
        "keyboard" => viewport(KEYBOARD),
        "about" => viewport(ABOUT),
        "welcome" => viewport(WELCOME),
        "ocr-result" => viewport(crate::ocr_result::VIEWPORT),
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

/// End the run if it is stuck: a window that never opened, a renderer that
/// never delivered the picture.
fn watchdog() {
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
    let mut colours = std::collections::HashSet::new();
    for p in &image.pixels {
        colours.insert(p.to_array());
        if colours.len() >= MIN_COLOURS {
            return None;
        }
    }
    Some(match colours.len() {
        1 => "a single colour".into(),
        n => format!("only {n} colours"),
    })
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

    #[test]
    fn a_picture_with_shades_is_not_blank() {
        let mut img = filled(40, 30, Color32::WHITE);
        for (i, p) in img.pixels.iter_mut().take(64).enumerate() {
            *p = Color32::from_gray(i as u8 * 3);
        }
        assert_eq!(blank(&img), None);
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
