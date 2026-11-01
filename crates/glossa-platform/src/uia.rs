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
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationTextPattern, UIA_TextPatternId,
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
        keep_mta();
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

/// Keep the process's multithreaded apartment alive for good, once. WinRT
/// caches its class factories process-wide; when the last thread in the MTA
/// leaves it (a guard's CoUninitialize), the MTA and the objects in it go,
/// and the next call through a cached factory crashes. A permanent MTA usage
/// count keeps them valid whatever threads come and go.
pub(crate) fn keep_mta() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: no arguments to get wrong; the cookie is deliberately never
        // given back (CoDecrementMTAUsage), which is what keeps the MTA.
        if let Err(e) = unsafe { windows::Win32::System::Com::CoIncrementMTAUsage() } {
            tracing::debug!("CoIncrementMTAUsage: {e}");
        }
    });
}

/// What UI Automation says about the focused element: its class (a Win32
/// class, or in Chromium and Electron the HTML `class` attribute, such as
/// xterm.js's `xterm-helper-textarea`) and its selected text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Focused {
    pub class: String,
    /// `None` for no selection, an empty one, or an element without a text
    /// pattern.
    pub selection: Option<String>,
}

/// The focused element's class and selection, in one round of UI
/// Automation calls. `None` when there is no focused element at all.
pub fn focused() -> Option<Focused> {
    let _com = Com::init();
    // SAFETY: COM is initialised on this thread for the duration; every
    // interface pointer is reference-counted by the windows crate.
    unsafe {
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        if let Ok(a2) = automation.cast::<IUIAutomation2>() {
            let _ = a2.SetConnectionTimeout(TIMEOUT_MS);
            let _ = a2.SetTransactionTimeout(TIMEOUT_MS);
        }
        let element = automation.GetFocusedElement().ok()?;
        let class = element.CurrentClassName().map(|b| b.to_string()).unwrap_or_default();
        Some(Focused { class, selection: selection_of(&element) })
    }
}

/// The selected text of `element`, through its text pattern.
///
/// # Safety
/// COM must be initialised on this thread.
unsafe fn selection_of(element: &IUIAutomationElement) -> Option<String> {
    // SAFETY: the caller guarantees COM; the interfaces are reference-counted.
    unsafe {
        let pattern: IUIAutomationTextPattern = element.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
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
