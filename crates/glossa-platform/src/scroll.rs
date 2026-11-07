//! How far the system scrolls for a mouse-wheel notch.
//!
//! Window systems report a wheel notch as one "line" (Windows sends 120 of
//! `WHEEL_DELTA`, X11 and Wayland one click, macOS one tick of
//! `scrollingDeltaY`), and every system means a different distance by it.
//! [`wheel_step`] answers in points, the way native programs on that
//! system scroll, so the interface can move by the same amount. Touchpads
//! that report exact distances (macOS, Wayland, most of X11) are not
//! scaled: their deltas are already in points.
//!
//! * **Windows**: the user's "lines to scroll at a time"
//!   (`SPI_GETWHEELSCROLLLINES`, 3 by default, or a whole page) and
//!   characters for a tilt (`SPI_GETWHEELSCROLLCHARS`), at Chromium's
//!   100/3 points a line: 100 points a notch by default, as Edge and
//!   Chrome scroll.
//! * **macOS**: 40 points a tick, as AppKit's web views (WebKit's
//!   `pixelsPerLineStep`) and Chromium (`kScrollbarPixelsPerCocoaTick`)
//!   scroll. The system applies the user's scrolling speed
//!   (`com.apple.scrollwheel.scaling`) to the tick count before a program
//!   sees it, so reading it here would apply it twice.
//! * **Linux**: no desktop-wide setting to read; 53 points a click, as
//!   Chromium scrolls on X11 and Wayland, close to Firefox's three lines.

#[cfg(not(any(windows, target_os = "macos")))]
#[path = "scroll/linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "scroll/mac.rs"]
mod imp;
#[cfg(windows)]
#[path = "scroll/win.rs"]
mod imp;

/// How far one wheel "line" scrolls along one axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelStep {
    /// This many points (logical pixels).
    Points(f32),
    /// The height (or width) of the view: Windows' "one screen at a time".
    Page,
}

/// The wheel steps for vertical scrolling and for horizontal scrolling (a
/// tilting wheel, or Shift with the wheel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelSteps {
    pub vertical: WheelStep,
    pub horizontal: WheelStep,
}

/// The system's wheel steps, read now: the user may change them at any
/// time, so callers read them again now and then rather than once. Cheap
/// (no I/O; one `SystemParametersInfoW` per axis on Windows), but not free
/// enough for every frame. Never fails: a setting that cannot be read gives
/// the system's default.
pub fn wheel_step() -> WheelSteps {
    imp::wheel_step()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_sensible() {
        let s = wheel_step();
        for step in [s.vertical, s.horizontal] {
            if let WheelStep::Points(p) = step {
                assert!((0.0..10_000.0).contains(&p), "{p}");
            }
        }
    }
}
