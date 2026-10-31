//! System-wide hotkeys, over the `global-hotkey` crate.
//!
//! Works on Windows (`RegisterHotKey`), macOS (Carbon hot keys) and X11
//! (`XGrabKey`). Wayland has no protocol for one client to watch keys meant
//! for another, so [`HotkeyService::new`] refuses there with
//! [`HotkeyError::Unsupported`]; the app should then tell the user to bind a
//! compositor shortcut that calls the local HTTP API (`curl …`).
//!
//! **Threading (Windows):** `RegisterHotKey` binds a hotkey to the calling
//! thread's message queue, and `global-hotkey` creates a hidden window on the
//! thread that constructs the manager. Create the [`HotkeyService`] on the
//! thread that runs the GUI event loop, or the events never arrive. On macOS
//! that is the main thread as well.

use std::collections::{HashMap, VecDeque};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use glossa_core::{Action, Hotkey};

/// Why a hotkey could not be set up.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyError {
    /// The session cannot have global hotkeys at all (Wayland).
    #[error("global hotkeys are not available: {reason}")]
    Unsupported { reason: String },
    /// The key name has no equivalent on this platform's hotkey API.
    #[error("the key {0:?} cannot be used in a global hotkey")]
    UnknownKey(String),
    /// Another program (or another of our actions) already owns the chord.
    #[error("{hotkey} is already in use by another program")]
    Taken { hotkey: String },
    /// Any other refusal by the OS.
    #[error("cannot register {hotkey}: {detail}")]
    Failed { hotkey: String, detail: String },
}

impl From<HotkeyError> for glossa_core::Error {
    fn from(e: HotkeyError) -> Self {
        glossa_core::Error::Platform(e.to_string())
    }
}

/// The registered hotkeys and the actions they trigger.
pub struct HotkeyService {
    manager: GlobalHotKeyManager,
    /// global-hotkey id → (the registered chord, its action).
    registered: HashMap<u32, (HotKey, Action)>,
    queue: VecDeque<Action>,
    /// Set by [`HotkeyService::set_waker`]: events then arrive here instead
    /// of through the crate's global receiver.
    events: Option<std::sync::mpsc::Receiver<GlobalHotKeyEvent>>,
}

impl std::fmt::Debug for HotkeyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HotkeyService").field("registered", &self.registered.len()).finish()
    }
}

impl HotkeyService {
    /// Create the service. On Windows, call this on the event-loop thread
    /// (see the module docs).
    pub fn new() -> Result<Self, HotkeyError> {
        let session = crate::Session::detect();
        if !session.supports_global_hotkeys() {
            let reason = if session.is_wayland() {
                "Wayland does not let programs register global hotkeys; bind a shortcut in \
                 your compositor's settings that calls Glossa's HTTP API instead"
            } else {
                "no graphical session"
            };
            return Err(HotkeyError::Unsupported { reason: reason.into() });
        }
        let manager = GlobalHotKeyManager::new().map_err(|e| HotkeyError::Unsupported { reason: e.to_string() })?;
        Ok(HotkeyService { manager, registered: HashMap::new(), queue: VecDeque::new(), events: None })
    }

    /// Bind `hotkey` to `action`, replacing any hotkey the action had.
    pub fn register(&mut self, action: Action, hotkey: &Hotkey) -> Result<(), HotkeyError> {
        let hk = to_global(hotkey)?;
        if let Some((_, other)) = self.registered.get(&hk.id()) {
            if *other == action {
                return Ok(());
            }
            return Err(HotkeyError::Taken { hotkey: hotkey.to_string() });
        }
        self.unregister(action);
        self.manager.register(hk).map_err(|e| match e {
            global_hotkey::Error::AlreadyRegistered(_) => HotkeyError::Taken { hotkey: hotkey.to_string() },
            other => HotkeyError::Failed { hotkey: hotkey.to_string(), detail: other.to_string() },
        })?;
        self.registered.insert(hk.id(), (hk, action));
        Ok(())
    }

    /// Remove the hotkey bound to `action`, if any.
    pub fn unregister(&mut self, action: Action) {
        let ids: Vec<u32> = self.registered.iter().filter(|(_, (_, a))| *a == action).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some((hk, _)) = self.registered.remove(&id) {
                if let Err(e) = self.manager.unregister(hk) {
                    tracing::debug!("unregistering hotkey failed: {e}");
                }
            }
        }
    }

    /// Remove every hotkey this service registered.
    pub fn unregister_all(&mut self) {
        let keys: Vec<HotKey> = self.registered.drain().map(|(_, (hk, _))| hk).collect();
        if let Err(e) = self.manager.unregister_all(&keys) {
            tracing::debug!("unregistering hotkeys failed: {e}");
        }
        self.queue.clear();
    }

    /// The actions currently bound.
    pub fn bound_actions(&self) -> Vec<Action> {
        let mut v: Vec<Action> = self.registered.values().map(|(_, a)| *a).collect();
        v.sort();
        v
    }

    /// Call `waker` on every hotkey event, from whatever thread the OS
    /// delivers it on, so a GUI whose window is hidden can wake its loop
    /// instead of polling. Events are still collected by [`poll`](Self::poll).
    pub fn set_waker(&mut self, waker: impl Fn() + Send + Sync + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            let _ = tx.send(event);
            waker();
        }));
        self.events = Some(rx);
    }

    /// The next triggered action, if any. Drains the global event channel
    /// (key presses only; releases are ignored) without blocking; call it
    /// from the event loop on every wake-up.
    ///
    /// Uses `GlobalHotKeyEvent::receiver()`, so nothing else in the process
    /// may install `GlobalHotKeyEvent::set_event_handler`.
    pub fn poll(&mut self) -> Option<Action> {
        loop {
            let event = match &self.events {
                Some(rx) => rx.try_recv().ok(),
                None => GlobalHotKeyEvent::receiver().try_recv().ok(),
            };
            let Some(event) = event else { break };
            if event.state() != HotKeyState::Pressed {
                continue;
            }
            if let Some((_, action)) = self.registered.get(&event.id()) {
                self.queue.push_back(*action);
            }
        }
        self.queue.pop_front()
    }
}

impl Drop for HotkeyService {
    fn drop(&mut self) {
        self.unregister_all();
    }
}

/// Convert our hotkey to global-hotkey's.
pub fn to_global(hotkey: &Hotkey) -> Result<HotKey, HotkeyError> {
    let code = key_code(&hotkey.key.0).ok_or_else(|| HotkeyError::UnknownKey(hotkey.key.0.clone()))?;
    let mut mods = Modifiers::empty();
    if hotkey.mods.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if hotkey.mods.alt {
        mods |= Modifiers::ALT;
    }
    if hotkey.mods.shift {
        mods |= Modifiers::SHIFT;
    }
    if hotkey.mods.meta {
        mods |= Modifiers::META;
    }
    Ok(HotKey::new(Some(mods), code))
}

/// Map a canonical glossa key name (as `glossa_core::hotkey` spells them)
/// to a physical key code.
pub fn key_code(name: &str) -> Option<Code> {
    use Code::*;
    let code = match name {
        "Backspace" => Backspace,
        "Tab" => Tab,
        "Enter" => Enter,
        "Pause" => Pause,
        "CapsLock" => CapsLock,
        "Escape" => Escape,
        "Space" => Space,
        "PageUp" => PageUp,
        "PageDown" => PageDown,
        "End" => End,
        "Home" => Home,
        "Left" => ArrowLeft,
        "Up" => ArrowUp,
        "Right" => ArrowRight,
        "Down" => ArrowDown,
        "PrintScreen" => PrintScreen,
        "Insert" => Insert,
        "Delete" => Delete,
        "0" => Digit0,
        "1" => Digit1,
        "2" => Digit2,
        "3" => Digit3,
        "4" => Digit4,
        "5" => Digit5,
        "6" => Digit6,
        "7" => Digit7,
        "8" => Digit8,
        "9" => Digit9,
        "A" => KeyA,
        "B" => KeyB,
        "C" => KeyC,
        "D" => KeyD,
        "E" => KeyE,
        "F" => KeyF,
        "G" => KeyG,
        "H" => KeyH,
        "I" => KeyI,
        "J" => KeyJ,
        "K" => KeyK,
        "L" => KeyL,
        "M" => KeyM,
        "N" => KeyN,
        "O" => KeyO,
        "P" => KeyP,
        "Q" => KeyQ,
        "R" => KeyR,
        "S" => KeyS,
        "T" => KeyT,
        "U" => KeyU,
        "V" => KeyV,
        "W" => KeyW,
        "X" => KeyX,
        "Y" => KeyY,
        "Z" => KeyZ,
        "Numpad0" => Numpad0,
        "Numpad1" => Numpad1,
        "Numpad2" => Numpad2,
        "Numpad3" => Numpad3,
        "Numpad4" => Numpad4,
        "Numpad5" => Numpad5,
        "Numpad6" => Numpad6,
        "Numpad7" => Numpad7,
        "Numpad8" => Numpad8,
        "Numpad9" => Numpad9,
        "NumpadMultiply" => NumpadMultiply,
        "NumpadAdd" => NumpadAdd,
        "NumpadSubtract" => NumpadSubtract,
        "NumpadDecimal" => NumpadDecimal,
        "NumpadDivide" => NumpadDivide,
        "F1" => F1,
        "F2" => F2,
        "F3" => F3,
        "F4" => F4,
        "F5" => F5,
        "F6" => F6,
        "F7" => F7,
        "F8" => F8,
        "F9" => F9,
        "F10" => F10,
        "F11" => F11,
        "F12" => F12,
        "F13" => F13,
        "F14" => F14,
        "F15" => F15,
        "F16" => F16,
        "F17" => F17,
        "F18" => F18,
        "F19" => F19,
        "F20" => F20,
        "F21" => F21,
        "F22" => F22,
        "F23" => F23,
        "F24" => F24,
        "ScrollLock" => ScrollLock,
        ";" => Semicolon,
        "=" => Equal,
        "," => Comma,
        "-" => Minus,
        "." => Period,
        "/" => Slash,
        "`" => Backquote,
        "[" => BracketLeft,
        "\\" => Backslash,
        "]" => BracketRight,
        "'" => Quote,
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::hotkey::Modifiers as Mods;

    /// Every key name glossa-core accepts, enumerated through its public
    /// legacy decoder (each name has a virtual-key code), so a key added to
    /// the core table without a mapping here fails this test.
    fn all_core_keys() -> Vec<String> {
        (1u32..=255).filter_map(Hotkey::from_legacy).map(|h| h.key.0).collect()
    }

    #[test]
    fn every_core_key_has_a_code() {
        let keys = all_core_keys();
        assert!(keys.len() > 100, "expected the full key table, got {}", keys.len());
        for name in &keys {
            assert!(key_code(name).is_some(), "no global-hotkey code for {name:?}");
        }
    }

    #[test]
    fn the_mapping_is_one_to_one() {
        let mut codes: Vec<Code> = all_core_keys().iter().filter_map(|k| key_code(k)).collect();
        let n = codes.len();
        codes.sort_by_key(|c| *c as u32);
        codes.dedup();
        assert_eq!(codes.len(), n, "two key names map to the same code");
    }

    #[test]
    fn unknown_keys_are_reported() {
        let hk = Hotkey::new(Mods::default(), "Banana");
        assert_eq!(to_global(&hk), Err(HotkeyError::UnknownKey("Banana".into())));
    }

    #[test]
    fn modifiers_carry_over() {
        let hk: Hotkey = "Ctrl+Shift+Q".parse().unwrap();
        let g = to_global(&hk).unwrap();
        assert_eq!(g.key, Code::KeyQ);
        assert_eq!(g.mods, Modifiers::CONTROL | Modifiers::SHIFT);
        let hk: Hotkey = "Alt+Meta+F12".parse().unwrap();
        let g = to_global(&hk).unwrap();
        assert_eq!(g.mods, Modifiers::ALT | Modifiers::META);
        assert_eq!(g.key, Code::F12);
        // Distinct chords get distinct ids, which is what poll() relies on.
        let a = to_global(&"Ctrl+E".parse().unwrap()).unwrap();
        let b = to_global(&"Ctrl+Shift+E".parse().unwrap()).unwrap();
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn errors_convert_to_the_pipeline_error() {
        let e: glossa_core::Error = HotkeyError::Unsupported { reason: "Wayland".into() }.into();
        assert!(matches!(e, glossa_core::Error::Platform(ref m) if m.contains("Wayland")));
    }

    #[test]
    #[ignore = "needs a display; registers a real hotkey"]
    fn register_and_unregister() {
        let mut s = HotkeyService::new().unwrap();
        s.register(Action::ShowPopup, &"Ctrl+Alt+Shift+F11".parse().unwrap()).unwrap();
        assert_eq!(s.bound_actions(), vec![Action::ShowPopup]);
        assert_eq!(s.poll(), None);
        s.unregister_all();
        assert!(s.bound_actions().is_empty());
    }
}
