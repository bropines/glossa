//! Backdrops on macOS: an `NSVisualEffectView` (vibrancy) behind the
//! window's content.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSAutoresizingMaskOptions,
    NSUserInterfaceItemIdentification, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindow, NSWindowOrderingMode,
};
use objc2_foundation::NSString;

use super::{Applied, Backdrop};

const IDENTIFIER: &str = "glossa.backdrop";

/// [`apply`] to the content of a window [`crate::window::own_window_by_title`] found.
pub fn apply_window(window: isize, kind: Backdrop, dark: bool) -> Result<Applied, String> {
    if MainThreadMarker::new().is_none() {
        return Err("vibrancy must be applied on the main thread".into());
    }
    if window == 0 {
        return Err("no window".into());
    }
    // SAFETY: a pointer `own_window_by_title` returned during this frame, on
    // the main thread, to a window the application still holds.
    let window: &NSWindow = unsafe { &*(window as *const NSWindow) };
    let content = window.contentView().ok_or("the window has no content view")?;
    apply(NonNull::from(&*content).cast(), kind, dark)
}

/// The effect view sits in the content view's superview (the window's
/// frame view), below the content view: the GL or Metal surface is
/// the content view's own layer, so a subview of it would cover what we
/// draw. It covers the whole frame view, title bar included: a translucent
/// window's title bar is clear, and with the effect only behind the content
/// whatever is behind the window showed through it sharp.
pub fn apply(ns_view: NonNull<c_void>, kind: Backdrop, dark: bool) -> Result<Applied, String> {
    let mtm = MainThreadMarker::new().ok_or("vibrancy must be applied on the main thread")?;
    // SAFETY: an AppKit raw handle's `ns_view` is a live NSView, and we
    // are on the main thread, where AppKit objects may be touched.
    let content: &NSView = unsafe { ns_view.cast::<NSView>().as_ref() };
    let Some(parent) = (unsafe { content.superview() }) else {
        return Err("the window's content view has no superview".into());
    };
    // Compared as Rust strings, not with `isEqualToString:`: on macOS 15 the
    // string may be a Swift one whose BOOL return objc2's debug checks
    // reject ('B', not 'c'), aborting debug builds.
    let existing: Option<Retained<NSView>> =
        parent.subviews().iter().find(|v| v.identifier().is_some_and(|id| id.to_string() == IDENTIFIER));
    let material = match kind {
        Backdrop::None => {
            if let Some(old) = existing {
                old.removeFromSuperview();
            }
            return Ok(Applied::Cleared);
        }
        Backdrop::Mica => NSVisualEffectMaterial::WindowBackground,
        Backdrop::MicaAlt => NSVisualEffectMaterial::UnderWindowBackground,
        Backdrop::Acrylic => NSVisualEffectMaterial::Popover,
        Backdrop::Blur => NSVisualEffectMaterial::FullScreenUI,
    };
    // An effect view already there is changed in place rather than taken
    // out and put back on every theme change (see `insert_keeping_title_bar`).
    let effect = match existing.map(|v| v.downcast::<NSVisualEffectView>()) {
        Some(Ok(effect)) => effect,
        other => {
            if let Some(Err(stray)) = other {
                stray.removeFromSuperview();
            }
            let effect = NSVisualEffectView::initWithFrame(mtm.alloc(), parent.bounds());
            effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            // Stay vivid when the window is not key: the popup never is.
            effect.setState(NSVisualEffectState::Active);
            effect.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            effect.setIdentifier(Some(&NSString::from_str(IDENTIFIER)));
            insert_keeping_title_bar(&parent, &effect, content);
            effect
        }
    };
    effect.setMaterial(material);
    // SAFETY: the appearance names are AppKit's own constants.
    let name = unsafe {
        if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    };
    if let Some(appearance) = NSAppearance::appearanceNamed(name) {
        effect.setAppearance(Some(&appearance));
    }
    Ok(Applied::Vibrancy)
}

/// Put `effect` in the frame view, under the content view, and keep the
/// title bar a light window has.
///
/// macOS 27 lays a light (Aqua) window's title bar out the old way, with
/// the title at the left edge over the window buttons, once its frame view
/// has a subview it does not know. A dark window keeps its title bar, and a
/// light one that was dark through one layout pass after the subview came
/// keeps it too, through later resizes, title and theme changes. So a light
/// window is made dark for a tenth of a second once it is on screen, and
/// then gets its own appearance back. This runs from the main queue, after
/// the frame that added the view, so it also outlasts the theme winit sets
/// on the window at the end of that frame.
fn insert_keeping_title_bar(parent: &NSView, effect: &NSVisualEffectView, content: &NSView) {
    parent.addSubview_positioned_relativeTo(effect, NSWindowOrderingMode::Below, Some(content));
    if let Some(window) = content.window() {
        bounce_when_shown(window, 0);
    }
}

/// See [`insert_keeping_title_bar`]. A window that is not on screen yet
/// (egui makes a new one hidden, then shows it) has no layout pass to be
/// dark through, so it waits until it is shown, for a few seconds at most.
fn bounce_when_shown(window: Retained<NSWindow>, tries: u32) {
    run_on_main(if tries == 0 { 0 } else { 50 }, move || {
        if !window.isVisible() {
            if tries < 100 {
                bounce_when_shown(window, tries + 1);
            }
            return;
        }
        if is_dark(&window) {
            return;
        }
        let own = window.appearance();
        // SAFETY: AppKit's own constant.
        let dark = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua });
        window.setAppearance(dark.as_deref());
        tracing::debug!(title = %window.title(), "vibrancy: kept the light title bar");
        // One main-queue turn is not always a layout pass; a tenth of a
        // second is a few.
        run_on_main(100, move || window.setAppearance(own.as_deref()));
    });
}

fn is_dark(window: &NSWindow) -> bool {
    // SAFETY: AppKit's own constant.
    // As Rust strings: see `apply` on `isEqualToString:`.
    window.effectiveAppearance().name().to_string() == unsafe { NSAppearanceNameDarkAqua }.to_string()
}

/// Run `f` on the main thread, at the main queue's next turn or after
/// `delay_ms`.
fn run_on_main(delay_ms: u64, f: impl FnOnce() + 'static) {
    extern "C" {
        static _dispatch_main_q: c_void;
        fn dispatch_time(when: u64, delta: i64) -> u64;
        fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
        fn dispatch_after_f(when: u64, queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    }
    extern "C" fn trampoline(context: *mut c_void) {
        // SAFETY: the box `run_on_main` leaked, handed back exactly once.
        let f = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce()>) };
        f();
    }
    let boxed: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    let context = Box::into_raw(boxed) as *mut c_void;
    // SAFETY: libdispatch's main queue; the context is a leaked box the
    // trampoline takes back. The closure may hold main-thread-only AppKit
    // objects: the main queue runs on the main thread, where it was made.
    unsafe {
        if delay_ms == 0 {
            dispatch_async_f(&_dispatch_main_q, context, trampoline);
        } else {
            let when = dispatch_time(0, (delay_ms * 1_000_000) as i64);
            dispatch_after_f(when, &_dispatch_main_q, context, trampoline);
        }
    }
}
