//! Windows on macOS (AppKit).

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSColor, NSWindow};
use objc2_foundation::NSString;

/// One of this program's windows by its title, as a pointer to its
/// NSWindow: for the windows egui makes (immediate viewports), whose
/// handles it does not hand out. The application keeps the window alive
/// while it is open. Main thread only.
pub fn own_window_by_title(title: &str) -> Option<isize> {
    let mtm = MainThreadMarker::new()?;
    let want = NSString::from_str(title);
    NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .find(|w| w.title().isEqualToString(&want))
        .map(|w| Retained::as_ptr(&w) as isize)
}

/// `NSStatusWindowLevel`: just above the menu bar (`NSMainMenuWindowLevel`,
/// 24) and the Dock.
const ABOVE_MENU_BAR: isize = 25;

/// Lift the window titled `title` above the menu bar, so AppKit does not
/// keep it below it. Main thread only.
pub fn lift_above_menu_bar(title: &str) -> bool {
    let Some(ptr) = own_window_by_title(title) else { return false };
    // SAFETY: a window the application holds, found just now on the main
    // thread (`own_window_by_title` returns `None` elsewhere).
    let window: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
    window.setLevel(ABOVE_MENU_BAR);
    true
}

/// Round the corners of a borderless window by `radius` points, as AppKit
/// rounds its menus and popovers: the window's frame view gets a layer
/// with that corner radius masking what is drawn (the GL surface, a
/// vibrancy view), and the window turns non-opaque with a clear
/// background so the corners show what is behind. Nothing needs an alpha
/// channel for that, so it works whatever the transparency settings. The
/// shadow is redrawn to follow the new outline. Main thread only.
pub fn round_corners(window: isize, radius: f32) -> bool {
    if MainThreadMarker::new().is_none() || window == 0 {
        return false;
    }
    // SAFETY: a pointer `own_window_by_title` returned on the main thread,
    // to a window the application still holds.
    let window: &NSWindow = unsafe { &*(window as *const NSWindow) };
    let Some(content) = window.contentView() else { return false };
    // The frame view holds the content view and any vibrancy view; masking
    // it rounds both.
    let target = unsafe { content.superview() }.unwrap_or(content);
    target.setWantsLayer(true);
    // SAFETY: `layer` and the CALayer setters are AppKit/Core Animation's
    // own, called on the main thread on a layer-backed view.
    unsafe {
        let layer: *mut AnyObject = msg_send![&*target, layer];
        if layer.is_null() {
            return false;
        }
        let _: () = msg_send![layer, setCornerRadius: radius as f64];
        let _: () = msg_send![layer, setMasksToBounds: true];
    }
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.invalidateShadow();
    true
}
