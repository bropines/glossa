//! Synthetic keyboard input: the copy and paste keystrokes behind selection
//! capture and "replace selection", and typing text directly.
//!
//! Windows uses `SendInput`; Linux (X11) and macOS go through `enigo`
//! (XTest and CGEvent). Wayland forbids one client from typing into another,
//! so every function here returns an error there, as it does without a
//! graphical session.
//!
//! Every Windows event we inject carries [`INJECTED_TAG`] in `dwExtraInfo`,
//! so the program's own low-level hooks (mouse mode, layout indicator) can
//! recognise and ignore it.

use glossa_core::config::CopyAction;

use crate::Result;

/// The `dwExtraInfo` value on every keystroke we inject on Windows ("GLOS").
pub const INJECTED_TAG: usize = 0x474C_4F53;

/// Release modifier keys that are still held down — typically the user's
/// fingers still on the hotkey. Without this our synthetic Ctrl+C arrives as
/// Ctrl+Shift+C (or Ctrl+Alt+C) and does something else entirely.
///
/// On Windows only the keys `GetAsyncKeyState` reports down are released,
/// and a neutral key is tapped first when Alt or Win is among them, so their
/// release does not open the menu bar or the Start menu. Elsewhere all
/// modifiers are released unconditionally, which is harmless.
pub fn release_modifiers() -> Result<()> {
    imp::release_modifiers()
}

/// Send the copy keystroke chosen in the options: Ctrl+C or Ctrl+Insert
/// (Cmd+C on macOS for both). [`CopyAction::AccessibilityOnly`] sends
/// nothing and succeeds.
pub fn send_copy(action: CopyAction) -> Result<()> {
    if action == CopyAction::AccessibilityOnly {
        return Ok(());
    }
    imp::send_copy(action)
}

/// Send the paste keystroke (Ctrl+V, Cmd+V on macOS).
pub fn send_paste() -> Result<()> {
    imp::send_paste()
}

/// Type `text` into the focused window as if typed on a keyboard, whatever
/// the active layout. Newlines become Enter.
pub fn type_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    imp::type_text(text)
}

/// Refuse up front where synthetic input cannot work, with the reason.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn ensure_supported() -> Result<()> {
    let session = crate::Session::detect();
    if session.supports_synthetic_input() {
        Ok(())
    } else if session.is_wayland() {
        Err(crate::platform_err("Wayland does not allow sending keystrokes to other programs"))
    } else {
        Err(crate::platform_err("synthetic input needs a graphical session"))
    }
}

#[cfg(windows)]
mod imp {
    use glossa_core::config::CopyAction;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_C, VK_DELETE,
        VK_DOWN, VK_END, VK_HOME, VK_INSERT, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_NEXT, VK_PRIOR,
        VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_TAB, VK_UP, VK_V,
    };

    use super::INJECTED_TAG;
    use crate::{platform_err, Result};

    /// An unassigned virtual key: tapping it between Alt/Win down and up
    /// tells Windows the modifier was "used", so its release is not a menu
    /// or Start-menu activation.
    const MASK_KEY: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);

    fn is_extended(vk: VIRTUAL_KEY) -> bool {
        matches!(
            vk,
            VK_INSERT
                | VK_DELETE
                | VK_HOME
                | VK_END
                | VK_PRIOR
                | VK_NEXT
                | VK_LEFT
                | VK_RIGHT
                | VK_UP
                | VK_DOWN
                | VK_RCONTROL
                | VK_RMENU
                | VK_LWIN
                | VK_RWIN
        )
    }

    fn keyboard(ki: KEYBDINPUT) -> INPUT {
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }
    }

    /// A virtual-key event with its scan code filled in: some programs
    /// (games, remote-desktop clients, Java) read only the scan code.
    fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        // SAFETY: a pure table lookup.
        let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } as u16;
        let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
        if is_extended(vk) {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        keyboard(KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: INJECTED_TAG })
    }

    fn unicode(unit: u16, up: bool) -> INPUT {
        let mut flags = KEYEVENTF_UNICODE;
        if up {
            flags |= KEYEVENTF_KEYUP;
        }
        keyboard(KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: INJECTED_TAG })
    }

    fn send(inputs: &[INPUT]) -> Result<()> {
        if inputs.is_empty() {
            return Ok(());
        }
        // SAFETY: the slice and the element size describe valid INPUTs.
        let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() {
            Ok(())
        } else {
            // Blocked by UIPI (target runs elevated) or by the secure desktop.
            Err(platform_err(format!(
                "SendInput delivered {sent} of {} events; the target window may run as administrator",
                inputs.len()
            )))
        }
    }

    fn chord(modifier: VIRTUAL_KEY, k: VIRTUAL_KEY) -> [INPUT; 4] {
        [key(modifier, false), key(k, false), key(k, true), key(modifier, true)]
    }

    pub fn release_modifiers() -> Result<()> {
        const MODS: [VIRTUAL_KEY; 8] =
            [VK_LCONTROL, VK_RCONTROL, VK_LSHIFT, VK_RSHIFT, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN];
        // SAFETY: GetAsyncKeyState has no preconditions; the sign bit means
        // "down now".
        let held: Vec<VIRTUAL_KEY> =
            MODS.into_iter().filter(|vk| unsafe { GetAsyncKeyState(vk.0 as i32) } < 0).collect();
        if held.is_empty() {
            return Ok(());
        }
        let mut inputs = Vec::with_capacity(held.len() + 2);
        if held.iter().any(|vk| matches!(*vk, VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN)) {
            inputs.push(key(MASK_KEY, false));
            inputs.push(key(MASK_KEY, true));
        }
        inputs.extend(held.iter().map(|vk| key(*vk, true)));
        send(&inputs)
    }

    pub fn send_copy(action: CopyAction) -> Result<()> {
        let k = if action == CopyAction::CtrlInsert { VK_INSERT } else { VK_C };
        send(&chord(VK_LCONTROL, k))
    }

    pub fn send_paste() -> Result<()> {
        send(&chord(VK_LCONTROL, VK_V))
    }

    pub fn type_text(text: &str) -> Result<()> {
        let mut inputs = Vec::with_capacity(text.len() * 2);
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                // "\r\n" and a lone "\r" or "\n" are each one Enter; a
                // KEYEVENTF_UNICODE newline is ignored by many programs.
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    inputs.extend([key(VK_RETURN, false), key(VK_RETURN, true)]);
                }
                '\n' => inputs.extend([key(VK_RETURN, false), key(VK_RETURN, true)]),
                '\t' => inputs.extend([key(VK_TAB, false), key(VK_TAB, true)]),
                _ => {
                    let mut buf = [0u16; 2];
                    for unit in c.encode_utf16(&mut buf) {
                        inputs.extend([unicode(*unit, false), unicode(*unit, true)]);
                    }
                }
            }
        }
        // Moderate batches: a single huge SendInput can overrun the target's
        // input queue and lose characters.
        for batch in inputs.chunks(256) {
            send(batch)?;
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod imp {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    use glossa_core::config::CopyAction;

    use crate::{platform_err, Result};

    /// Ctrl on Linux, Command on macOS.
    const PRIMARY: Key = if cfg!(target_os = "macos") { Key::Meta } else { Key::Control };

    fn enigo() -> Result<Enigo> {
        super::ensure_supported()?;
        Enigo::new(&Settings::default()).map_err(|e| platform_err(format!("cannot simulate input: {e}")))
    }

    fn input_err(e: enigo::InputError) -> glossa_core::Error {
        platform_err(format!("synthetic input failed: {e}"))
    }

    fn chord(modifier: Key, k: Key) -> Result<()> {
        let mut en = enigo()?;
        en.key(modifier, Direction::Press).map_err(input_err)?;
        let clicked = en.key(k, Direction::Click);
        // Release the modifier even if the click failed, or it stays stuck.
        let released = en.key(modifier, Direction::Release);
        clicked.and(released).map_err(input_err)
    }

    pub fn release_modifiers() -> Result<()> {
        let mut en = enigo()?;
        for k in [Key::Control, Key::Shift, Key::Alt, Key::Meta] {
            en.key(k, Direction::Release).map_err(input_err)?;
        }
        Ok(())
    }

    /// The C and V keys. On macOS by virtual key code (`kVK_ANSI_C`,
    /// `kVK_ANSI_V`): looking a character up in the keyboard layout calls
    /// the Text Input Sources API, which macOS only allows on the main thread
    /// and kills the process for elsewhere, and copying runs on a worker.
    /// Command shortcuts follow the key's Latin meaning on non-Latin layouts
    /// (Russian included), so the physical key is right there too.
    #[cfg(target_os = "macos")]
    const KEY_C: Key = Key::Other(0x08);
    #[cfg(target_os = "macos")]
    const KEY_V: Key = Key::Other(0x09);
    #[cfg(not(target_os = "macos"))]
    const KEY_C: Key = Key::Unicode('c');
    #[cfg(not(target_os = "macos"))]
    const KEY_V: Key = Key::Unicode('v');

    pub fn send_copy(action: CopyAction) -> Result<()> {
        #[cfg(not(target_os = "macos"))]
        if action == CopyAction::CtrlInsert {
            return chord(PRIMARY, Key::Insert);
        }
        // macOS has no Insert key; Cmd+C is the only copy there.
        let _ = action;
        chord(PRIMARY, KEY_C)
    }

    pub fn send_paste() -> Result<()> {
        chord(PRIMARY, KEY_V)
    }

    pub fn type_text(text: &str) -> Result<()> {
        enigo()?.text(text).map_err(input_err)
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    use glossa_core::config::CopyAction;

    use crate::{platform_err, Result};

    fn unsupported<T>() -> Result<T> {
        Err(platform_err("synthetic input is not implemented on this platform"))
    }

    pub fn release_modifiers() -> Result<()> {
        unsupported()
    }

    pub fn send_copy(_: CopyAction) -> Result<()> {
        unsupported()
    }

    pub fn send_paste() -> Result<()> {
        unsupported()
    }

    pub fn type_text(_: &str) -> Result<()> {
        unsupported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessibility_only_sends_nothing() {
        // Must succeed even without a display: nothing is sent.
        assert!(send_copy(CopyAction::AccessibilityOnly).is_ok());
        assert!(type_text("").is_ok());
    }

    #[test]
    fn tag_spells_glos() {
        assert_eq!((INJECTED_TAG as u32).to_be_bytes(), *b"GLOS");
    }
}
