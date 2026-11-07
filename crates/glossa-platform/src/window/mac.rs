//! Windows on macOS (AppKit).

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSColor, NSWindow, NSWindowCollectionBehavior};

/// One of this program's windows by its title, as a pointer to its
/// NSWindow: for the windows egui makes (immediate viewports), whose
/// handles it does not hand out. The application keeps the window alive
/// while it is open. Main thread only.
pub fn own_window_by_title(title: &str) -> Option<isize> {
    let mtm = MainThreadMarker::new()?;
    // Compared as Rust strings, not with `isEqualToString:`: on macOS 15 a
    // title may be a Swift string whose BOOL return objc2's debug checks
    // reject ('B', not 'c'), aborting debug builds.
    NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .find(|w| w.title().to_string() == title)
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

/// `NSScreenSaverWindowLevel`: over the menu bar, the Dock and what the
/// system shows at the screen's edges (the Stage Manager strip, hot
/// corners), as screenshot tools put their selection overlays.
const OVER_EVERYTHING: isize = 1000;

/// Lift the window titled `title` over everything else on screen. Main
/// thread only.
pub fn lift_over_everything(title: &str) -> bool {
    let Some(ptr) = own_window_by_title(title) else { return false };
    // SAFETY: as in `lift_above_menu_bar`.
    let window: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
    window.setLevel(OVER_EVERYTHING);
    true
}

/// Take the window titled `title` out of window management: it shows on
/// whichever Space and over full-screen apps, is skipped by Cmd-`, and is
/// an auxiliary window to Stage Manager, which then neither makes a stage
/// for it nor switches stages to show it. For the overlays, popups and
/// notices that appear over what the user is doing. Main thread only.
pub fn float_free(title: &str) -> bool {
    let Some(ptr) = own_window_by_title(title) else { return false };
    // SAFETY: as in `lift_above_menu_bar`.
    let window: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::Auxiliary,
    );
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

/// Hand the focus back to the program the user was working in when
/// clicking one of this program's windows made it the active application:
/// the popup's Replace button pastes with a keystroke, which goes to the
/// active application. That program owns the frontmost ordinary window
/// that is not ours. Waits up to `timeout` for it to become active, and
/// returns whether it did (or this program was not active in the first
/// place). Any thread.
pub fn give_focus_back(timeout: std::time::Duration) -> bool {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

    if !NSRunningApplication::currentApplication().isActive() {
        return true;
    }
    let Some(pid) = frontmost_other_pid() else {
        tracing::debug!("no other program's window to give the focus back to");
        return false;
    };
    let Some(target) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
        return false;
    };
    target.activateWithOptions(NSApplicationActivationOptions(0));
    let until = std::time::Instant::now() + timeout;
    while !target.isActive() {
        if std::time::Instant::now() >= until {
            tracing::debug!(pid, "the program did not take the focus back in time");
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Activation is reported before the window is key again; give it a
    // moment so the keystroke reaches the text field.
    std::thread::sleep(std::time::Duration::from_millis(60));
    true
}

/// The process owning the frontmost on-screen window of the normal level
/// (layer 0) that is not this process's. Window owners and layers are
/// readable without the Screen Recording permission.
fn frontmost_other_pid() -> Option<i32> {
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;
    const ON_SCREEN_ONLY: u32 = 1 << 0; // kCGWindowListOptionOnScreenOnly
    const EXCLUDE_DESKTOP: u32 = 1 << 4; // kCGWindowListExcludeDesktopElements
    const CF_NUMBER_SINT64: isize = 4; // kCFNumberSInt64Type

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFTypeRef;
        static kCGWindowOwnerPID: CFTypeRef;
        static kCGWindowLayer: CFTypeRef;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
        fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFNumberGetValue(number: CFTypeRef, kind: isize, out: *mut c_void) -> u8;
    }

    let own = std::process::id() as i64;
    // SAFETY: the copied array is ours and released below; the values read
    // from it are borrowed from it while it lives.
    unsafe {
        let number = |dict: CFTypeRef, key: CFTypeRef| -> Option<i64> {
            let n = CFDictionaryGetValue(dict, key);
            let mut v: i64 = 0;
            (!n.is_null() && CFNumberGetValue(n, CF_NUMBER_SINT64, (&mut v as *mut i64).cast()) != 0).then_some(v)
        };
        let list = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0);
        if list.is_null() {
            return None;
        }
        // Front to back.
        let found = (0..CFArrayGetCount(list)).find_map(|i| {
            let info = CFArrayGetValueAtIndex(list, i);
            let pid = number(info, kCGWindowOwnerPID)?;
            (number(info, kCGWindowLayer) == Some(0) && pid != own).then_some(pid as i32)
        });
        CFRelease(list);
        found
    }
}
