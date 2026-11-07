use super::{WheelStep, WheelSteps};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_ACTION,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

/// Points per line, as Chromium scrolls on Windows.
const LINE: f32 = 100.0 / 3.0;
/// Windows' own default for both settings.
const DEFAULT_LINES: u32 = 3;
/// `WHEEL_PAGESCROLL`: "one screen at a time".
const PAGE: u32 = u32::MAX;

pub fn wheel_step() -> WheelSteps {
    WheelSteps { vertical: step(read(SPI_GETWHEELSCROLLLINES)), horizontal: step(read(SPI_GETWHEELSCROLLCHARS)) }
}

fn step(lines: u32) -> WheelStep {
    if lines == PAGE {
        WheelStep::Page
    } else {
        // Zero is a valid setting: the wheel does not scroll at all.
        WheelStep::Points(lines.min(100) as f32 * LINE)
    }
}

fn read(action: SYSTEM_PARAMETERS_INFO_ACTION) -> u32 {
    let mut v: u32 = DEFAULT_LINES;
    // SAFETY: both actions write one UINT through the pointer, which points
    // at a live u32 for the duration of the call.
    let read = unsafe {
        SystemParametersInfoW(action, 0, Some((&mut v as *mut u32).cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
    };
    if read.is_err() {
        return DEFAULT_LINES;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_and_lines() {
        assert_eq!(step(PAGE), WheelStep::Page);
        assert_eq!(step(3), WheelStep::Points(100.0));
        assert_eq!(step(0), WheelStep::Points(0.0));
    }
}
