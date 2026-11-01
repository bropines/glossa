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
/// frame view), just below the content view: the GL or Metal surface is
/// the content view's own layer, so a subview of it would cover what we
/// draw.
pub fn apply(ns_view: NonNull<c_void>, kind: Backdrop, dark: bool) -> Result<Applied, String> {
    let mtm = MainThreadMarker::new().ok_or("vibrancy must be applied on the main thread")?;
    // SAFETY: an AppKit raw handle's `ns_view` is a live NSView, and we
    // are on the main thread, where AppKit objects may be touched.
    let content: &NSView = unsafe { ns_view.cast::<NSView>().as_ref() };
    let Some(parent) = (unsafe { content.superview() }) else {
        return Err("the window's content view has no superview".into());
    };
    let identifier = NSString::from_str(IDENTIFIER);
    let existing: Option<Retained<NSView>> =
        parent.subviews().iter().find(|v| v.identifier().is_some_and(|id| id.isEqualToString(&identifier)));
    if let Some(old) = existing {
        old.removeFromSuperview();
    }
    let material = match kind {
        Backdrop::None => return Ok(Applied::Cleared),
        Backdrop::Mica => NSVisualEffectMaterial::WindowBackground,
        Backdrop::MicaAlt => NSVisualEffectMaterial::UnderWindowBackground,
        Backdrop::Acrylic => NSVisualEffectMaterial::Popover,
        Backdrop::Blur => NSVisualEffectMaterial::FullScreenUI,
    };
    let effect = NSVisualEffectView::initWithFrame(mtm.alloc(), content.frame());
    effect.setMaterial(material);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    // Stay vivid when the window is not key: the popup never is.
    effect.setState(NSVisualEffectState::Active);
    effect.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    effect.setIdentifier(Some(&identifier));
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
    parent.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, Some(content));
    Ok(Applied::Vibrancy)
}
