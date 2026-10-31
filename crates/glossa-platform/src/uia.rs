//! Reading the selection through UI Automation, without touching the
//! clipboard or sending a keystroke.
//!
//! This is the first thing tried on Windows: it is silent, leaves the
//! clipboard alone, and works in programs that ignore synthetic Ctrl+C
//! (consoles that treat it as "interrupt", some games' chat boxes). It fails
//! — and we fall back to the clipboard — in programs that expose no
//! `TextPattern`, which includes most Electron apps until a screen reader
//! has switched their accessibility tree on.

use windows::core::Interface;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationTextPattern, UIA_TextPatternId,
};

/// UIA's own default timeout is 20 s for an unresponsive target; a hotkey
/// must not hang that long.
const TIMEOUT_MS: u32 = 1500;

/// Balances a successful `CoInitializeEx` on this thread.
struct Com {
    owned: bool,
}

impl Com {
    fn init() -> Com {
        // SAFETY: initialising COM on the current thread; S_FALSE (already
        // initialised) still needs a matching CoUninitialize, while
        // RPC_E_CHANGED_MODE (already an STA) must not get one.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Com { owned: hr.is_ok() }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: paired with the successful CoInitializeEx above, on the
            // same thread (the guard is neither Send nor stored).
            unsafe { CoUninitialize() };
        }
    }
}

/// The text selected in the focused element, if it exposes one. `None` for
/// no selection, an empty one, or an element without a text pattern.
pub fn selected_text() -> Option<String> {
    let _com = Com::init();
    // SAFETY: COM is initialised on this thread for the duration; every
    // interface pointer is reference-counted by the windows crate.
    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        if let Ok(a2) = automation.cast::<IUIAutomation2>() {
            let _ = a2.SetConnectionTimeout(TIMEOUT_MS);
            let _ = a2.SetTransactionTimeout(TIMEOUT_MS);
        }
        let element = automation.GetFocusedElement().ok()?;
        let pattern: IUIAutomationTextPattern =
            element.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
        let ranges = pattern.GetSelection().ok()?;
        let count = ranges.Length().ok()?;
        let mut parts = Vec::new();
        for i in 0..count {
            let text = ranges.GetElement(i).ok()?.GetText(-1).ok()?.to_string();
            if !text.is_empty() {
                parts.push(text);
            }
        }
        let text = parts.join("\n");
        (!text.trim().is_empty()).then_some(text)
    }
}
