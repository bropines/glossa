//! Windows on macOS (AppKit).

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
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
