//! Windows on macOS (AppKit).

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSWindow};
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
