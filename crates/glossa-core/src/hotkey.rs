//! Actions and the key chords that trigger them.
//!
//! [`Action`] is everything a hotkey, a tray menu item or a command-line
//! invocation can ask the program to do. The first seventeen are the actions
//! the classic hotkey page listed, in its order; the rest are ours.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    // -- the classic seventeen, in their original order ---------------------
    ShowMainWindow,
    /// Translate the current selection and show it in the popup.
    ShowPopup,
    ShowDictionary,
    ShowHistory,
    ShowKeyboard,
    /// Read the selection aloud.
    ListenSelection,
    /// Translate the selection and put the result on the clipboard, no window.
    CopyTranslation,
    /// Translate the selection and paste the translation over it.
    ReplaceSelection,
    ToggleMouseMode,
    /// Translate whatever is on the clipboard, in whichever window fits.
    TranslateClipboard,
    TranslateClipboardInMain,
    TranslateClipboardInPopup,
    DictionaryFromClipboard,
    /// Retype the selection as if it had been typed in the other keyboard
    /// layout — the fix for "ghbdtn" that should have been "привет".
    ConvertLayout,
    /// Translate the selection and read the translation aloud.
    ListenTranslation,
    /// Select a screen region and read the text in it.
    TextRecognition,
    SpeechInput,

    // -- additions ----------------------------------------------------------
    /// Region OCR straight to the popup with a translation, one keystroke.
    RecognizeAndTranslate,
    /// Repeat the last OCR region without selecting it again — for a game or a
    /// video where the text keeps changing in the same spot.
    RepeatLastRecognition,
    /// Region OCR with no window at all: the text goes straight to the
    /// clipboard.
    RecognizeToClipboard,
    /// Pin a region and re-recognise it whenever its pixels change — live
    /// subtitles for a game or a video.
    WatchRegion,
    /// Swap the source and target language of the main window.
    SwapLanguages,
    /// Cycle to the next enabled translation service.
    NextService,
    ShowOptions,
    Quit,
}

impl Action {
    /// The classic actions in hotkey-page order. Index `i` here is index `i`
    /// in the classic options list.
    pub const CLASSIC: [Action; 17] = [
        Action::ShowMainWindow,
        Action::ShowPopup,
        Action::ShowDictionary,
        Action::ShowHistory,
        Action::ShowKeyboard,
        Action::ListenSelection,
        Action::CopyTranslation,
        Action::ReplaceSelection,
        Action::ToggleMouseMode,
        Action::TranslateClipboard,
        Action::TranslateClipboardInMain,
        Action::TranslateClipboardInPopup,
        Action::DictionaryFromClipboard,
        Action::ConvertLayout,
        Action::ListenTranslation,
        Action::TextRecognition,
        Action::SpeechInput,
    ];

    pub const ALL: [Action; 25] = [
        Action::ShowMainWindow,
        Action::ShowPopup,
        Action::ShowDictionary,
        Action::ShowHistory,
        Action::ShowKeyboard,
        Action::ListenSelection,
        Action::CopyTranslation,
        Action::ReplaceSelection,
        Action::ToggleMouseMode,
        Action::TranslateClipboard,
        Action::TranslateClipboardInMain,
        Action::TranslateClipboardInPopup,
        Action::DictionaryFromClipboard,
        Action::ConvertLayout,
        Action::ListenTranslation,
        Action::TextRecognition,
        Action::SpeechInput,
        Action::RecognizeAndTranslate,
        Action::RepeatLastRecognition,
        Action::RecognizeToClipboard,
        Action::WatchRegion,
        Action::SwapLanguages,
        Action::NextService,
        Action::ShowOptions,
        Action::Quit,
    ];

    /// The key the classic options file used for this action's hotkey.
    pub fn legacy_option_key(self) -> Option<&'static str> {
        Some(match self {
            Action::ShowMainWindow => "HotKeyMainWindow",
            Action::ShowPopup => "HotKeyPopupWindow",
            Action::ShowDictionary => "HotKeyDictionary",
            Action::ShowHistory => "HotKeyHistory",
            Action::ShowKeyboard => "HotKeyKeyboard",
            Action::ListenSelection => "HotKeyListenText",
            Action::CopyTranslation => "HotKeyCopyTranslation",
            Action::ReplaceSelection => "HotKeyReplaceSelection",
            Action::ToggleMouseMode => "HotKeySwitchMouseMode",
            Action::TranslateClipboard => "HotKeyTranslateClipboard",
            Action::TranslateClipboardInMain => "HotKeyTranslateClipboardInMainWindow",
            Action::TranslateClipboardInPopup => "HotKeyTranslateClipboardInPopupWindow",
            Action::DictionaryFromClipboard => "HotKeyDictionaryClipboard",
            Action::ConvertLayout => "HotKeyConvertTextLayout",
            Action::ListenTranslation => "HotKeyListenTranslation",
            Action::TextRecognition => "HotKeyTextRecognition",
            Action::SpeechInput => "HotKeySpeechInput",
            _ => return None,
        })
    }

    /// Whether the action works on "the selection" and therefore needs the
    /// platform to capture it first.
    pub fn needs_selection(self) -> bool {
        matches!(
            self,
            Action::ShowPopup
                | Action::ListenSelection
                | Action::CopyTranslation
                | Action::ReplaceSelection
                | Action::ConvertLayout
                | Action::ListenTranslation
        )
    }

    /// The English label; the UI looks up a translated one by the same id.
    pub fn label(self) -> &'static str {
        match self {
            Action::ShowMainWindow => "Show main window",
            Action::ShowPopup => "Show popup window",
            Action::ShowDictionary => "Show dictionary window",
            Action::ShowHistory => "Show history window",
            Action::ShowKeyboard => "Show keyboard window",
            Action::ListenSelection => "Listen to selected text",
            Action::CopyTranslation => "Copy translation to clipboard",
            Action::ReplaceSelection => "Replace selected text",
            Action::ToggleMouseMode => "Switch mouse mode",
            Action::TranslateClipboard => "Translate clipboard content",
            Action::TranslateClipboardInMain => "Translate clipboard in main window",
            Action::TranslateClipboardInPopup => "Translate clipboard in popup window",
            Action::DictionaryFromClipboard => "Dictionary from clipboard",
            Action::ConvertLayout => "Change keyboard layout and selected text",
            Action::ListenTranslation => "Listen to selected text translation",
            Action::TextRecognition => "Text recognition",
            Action::SpeechInput => "Speech input",
            Action::RecognizeAndTranslate => "Recognise and translate a screen region",
            Action::RepeatLastRecognition => "Repeat the last recognised region",
            Action::RecognizeToClipboard => "Recognise a screen region to the clipboard",
            Action::WatchRegion => "Watch a screen region and translate changes",
            Action::SwapLanguages => "Swap translation direction",
            Action::NextService => "Switch to the next service",
            Action::ShowOptions => "Show options",
            Action::Quit => "Exit",
        }
    }

    pub fn id(self) -> &'static str {
        // The serde name, without going through serde.
        match self {
            Action::ShowMainWindow => "show_main_window",
            Action::ShowPopup => "show_popup",
            Action::ShowDictionary => "show_dictionary",
            Action::ShowHistory => "show_history",
            Action::ShowKeyboard => "show_keyboard",
            Action::ListenSelection => "listen_selection",
            Action::CopyTranslation => "copy_translation",
            Action::ReplaceSelection => "replace_selection",
            Action::ToggleMouseMode => "toggle_mouse_mode",
            Action::TranslateClipboard => "translate_clipboard",
            Action::TranslateClipboardInMain => "translate_clipboard_in_main",
            Action::TranslateClipboardInPopup => "translate_clipboard_in_popup",
            Action::DictionaryFromClipboard => "dictionary_from_clipboard",
            Action::ConvertLayout => "convert_layout",
            Action::ListenTranslation => "listen_translation",
            Action::TextRecognition => "text_recognition",
            Action::SpeechInput => "speech_input",
            Action::RecognizeAndTranslate => "recognize_and_translate",
            Action::RepeatLastRecognition => "repeat_last_recognition",
            Action::RecognizeToClipboard => "recognize_to_clipboard",
            Action::WatchRegion => "watch_region",
            Action::SwapLanguages => "swap_languages",
            Action::NextService => "next_service",
            Action::ShowOptions => "show_options",
            Action::Quit => "quit",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.id() == id)
    }
}

/// Modifier keys, as a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Windows key, Command on macOS, Super on Linux.
    pub meta: bool,
}

impl Modifiers {
    pub fn is_empty(&self) -> bool {
        !(self.ctrl || self.alt || self.shift || self.meta)
    }
}

/// A key, by name. Kept as a name rather than a platform scan code so a
/// config file moves between operating systems.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key(pub String);

/// A key chord: modifiers plus one key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: Modifiers,
    pub key: Key,
}

impl Hotkey {
    pub fn new(mods: Modifiers, key: impl Into<String>) -> Self {
        Self { mods, key: Key(key.into()) }
    }

    /// Decode the integer the classic options file stored: the low byte is
    /// the Win32 virtual-key code and the next byte the `MOD_*` flags
    /// (`MOD_ALT` 1, `MOD_CONTROL` 2, `MOD_SHIFT` 4, `MOD_WIN` 8). `0` means
    /// "no hotkey".
    pub fn from_legacy(value: u32) -> Option<Hotkey> {
        if value == 0 {
            return None;
        }
        let vk = (value & 0xff) as u8;
        let flags = (value >> 8) & 0xff;
        let mods = Modifiers {
            alt: flags & 1 != 0,
            ctrl: flags & 2 != 0,
            shift: flags & 4 != 0,
            meta: flags & 8 != 0,
        };
        Some(Hotkey { mods, key: Key(vk_name(vk)?.to_owned()) })
    }

    /// The inverse of [`from_legacy`](Self::from_legacy), for writing a
    /// classic options file back out.
    pub fn to_legacy(&self) -> Option<u32> {
        let vk = vk_code(&self.key.0)? as u32;
        let flags = (self.mods.alt as u32)
            | (self.mods.ctrl as u32) << 1
            | (self.mods.shift as u32) << 2
            | (self.mods.meta as u32) << 3;
        Some(flags << 8 | vk)
    }
}

impl fmt::Display for Hotkey {
    /// `Ctrl+Shift+Q`, in the conventional order.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.mods.alt {
            f.write_str("Alt+")?;
        }
        if self.mods.shift {
            f.write_str("Shift+")?;
        }
        if self.mods.meta {
            f.write_str("Meta+")?;
        }
        f.write_str(&self.key.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyParseError {
    #[error("empty hotkey")]
    Empty,
    #[error("unknown key {0:?}")]
    UnknownKey(String),
    #[error("hotkey {0:?} has modifiers but no key")]
    NoKey(String),
}

impl FromStr for Hotkey {
    type Err = HotkeyParseError;

    /// Accepts `Ctrl+Shift+Q`, `ctrl + shift + q`, `Control+Alt+F1`,
    /// `Super+Space`, `Cmd+E`. Modifiers in any order.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(HotkeyParseError::Empty);
        }
        let mut mods = Modifiers::default();
        let mut key: Option<String> = None;
        // Split on '+', but a trailing "+" means the plus key itself.
        let parts: Vec<&str> = if let Some(stripped) = s.strip_suffix("++") {
            stripped.split('+').map(str::trim).chain(std::iter::once("+")).collect()
        } else {
            s.split('+').map(str::trim).collect()
        };
        for part in parts {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "ctl" => mods.ctrl = true,
                "alt" | "option" | "opt" => mods.alt = true,
                "shift" => mods.shift = true,
                "meta" | "win" | "super" | "cmd" | "command" | "logo" => mods.meta = true,
                "" => {}
                _ => {
                    let canonical = canonical_key(part)
                        .ok_or_else(|| HotkeyParseError::UnknownKey(part.to_owned()))?;
                    key = Some(canonical.to_owned());
                }
            }
        }
        let key = key.ok_or_else(|| HotkeyParseError::NoKey(s.to_owned()))?;
        Ok(Hotkey { mods, key: Key(key) })
    }
}

impl Serialize for Hotkey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Hotkey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

/// Every key name we accept, with the virtual-key code it maps to on
/// Windows. The names are the canonical spellings used in config files.
const KEYS: &[(&str, u8)] = &[
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0d),
    ("Pause", 0x13),
    ("CapsLock", 0x14),
    ("Escape", 0x1b),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2c),
    ("Insert", 0x2d),
    ("Delete", 0x2e),
    ("0", 0x30),
    ("1", 0x31),
    ("2", 0x32),
    ("3", 0x33),
    ("4", 0x34),
    ("5", 0x35),
    ("6", 0x36),
    ("7", 0x37),
    ("8", 0x38),
    ("9", 0x39),
    ("A", 0x41),
    ("B", 0x42),
    ("C", 0x43),
    ("D", 0x44),
    ("E", 0x45),
    ("F", 0x46),
    ("G", 0x47),
    ("H", 0x48),
    ("I", 0x49),
    ("J", 0x4a),
    ("K", 0x4b),
    ("L", 0x4c),
    ("M", 0x4d),
    ("N", 0x4e),
    ("O", 0x4f),
    ("P", 0x50),
    ("Q", 0x51),
    ("R", 0x52),
    ("S", 0x53),
    ("T", 0x54),
    ("U", 0x55),
    ("V", 0x56),
    ("W", 0x57),
    ("X", 0x58),
    ("Y", 0x59),
    ("Z", 0x5a),
    ("Numpad0", 0x60),
    ("Numpad1", 0x61),
    ("Numpad2", 0x62),
    ("Numpad3", 0x63),
    ("Numpad4", 0x64),
    ("Numpad5", 0x65),
    ("Numpad6", 0x66),
    ("Numpad7", 0x67),
    ("Numpad8", 0x68),
    ("Numpad9", 0x69),
    ("NumpadMultiply", 0x6a),
    ("NumpadAdd", 0x6b),
    ("NumpadSubtract", 0x6d),
    ("NumpadDecimal", 0x6e),
    ("NumpadDivide", 0x6f),
    ("F1", 0x70),
    ("F2", 0x71),
    ("F3", 0x72),
    ("F4", 0x73),
    ("F5", 0x74),
    ("F6", 0x75),
    ("F7", 0x76),
    ("F8", 0x77),
    ("F9", 0x78),
    ("F10", 0x79),
    ("F11", 0x7a),
    ("F12", 0x7b),
    ("F13", 0x7c),
    ("F14", 0x7d),
    ("F15", 0x7e),
    ("F16", 0x7f),
    ("F17", 0x80),
    ("F18", 0x81),
    ("F19", 0x82),
    ("F20", 0x83),
    ("F21", 0x84),
    ("F22", 0x85),
    ("F23", 0x86),
    ("F24", 0x87),
    ("ScrollLock", 0x91),
    (";", 0xba),
    ("=", 0xbb),
    (",", 0xbc),
    ("-", 0xbd),
    (".", 0xbe),
    ("/", 0xbf),
    ("`", 0xc0),
    ("[", 0xdb),
    ("\\", 0xdc),
    ("]", 0xdd),
    ("'", 0xde),
];

fn vk_name(vk: u8) -> Option<&'static str> {
    KEYS.iter().find(|(_, code)| *code == vk).map(|(name, _)| *name)
}

fn vk_code(name: &str) -> Option<u8> {
    KEYS.iter().find(|(n, _)| *n == name).map(|(_, code)| *code)
}

/// Map a loosely written key name to its canonical spelling.
fn canonical_key(part: &str) -> Option<&'static str> {
    let lower = part.to_ascii_lowercase();
    let alias = match lower.as_str() {
        "esc" => "Escape",
        "return" => "Enter",
        "del" => "Delete",
        "ins" => "Insert",
        "pgup" | "prior" => "PageUp",
        "pgdn" | "next" => "PageDown",
        "prtsc" | "printscr" | "print" | "snapshot" => "PrintScreen",
        "spacebar" => "Space",
        "plus" => "=",
        "minus" => "-",
        "comma" => ",",
        "period" | "dot" => ".",
        "slash" => "/",
        "backslash" => "\\",
        "semicolon" => ";",
        "quote" | "apostrophe" => "'",
        "backquote" | "grave" | "tilde" => "`",
        "arrowleft" => "Left",
        "arrowright" => "Right",
        "arrowup" => "Up",
        "arrowdown" => "Down",
        _ => "",
    };
    if !alias.is_empty() {
        return Some(alias);
    }
    KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(part)).map(|(n, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_integers_decode_like_the_screenshot_shows() {
        // From a real options file, matched against the hotkey page:
        // popup window "Ctrl + Shift + Q", listen "Ctrl + E".
        assert_eq!(Hotkey::from_legacy(1617).unwrap().to_string(), "Ctrl+Shift+Q");
        assert_eq!(Hotkey::from_legacy(581).unwrap().to_string(), "Ctrl+E");
        assert_eq!(Hotkey::from_legacy(0), None);
    }

    #[test]
    fn legacy_round_trip() {
        for value in [1617u32, 581, 0x0170, 0x0841, 0x0720] {
            let hk = Hotkey::from_legacy(value).unwrap();
            assert_eq!(hk.to_legacy(), Some(value), "{hk}");
        }
    }

    #[test]
    fn parsing_is_forgiving_about_spelling_and_order() {
        let a: Hotkey = "Ctrl+Shift+Q".parse().unwrap();
        let b: Hotkey = "shift + control + q".parse().unwrap();
        assert_eq!(a, b);
        assert_eq!("Super+Space".parse::<Hotkey>().unwrap().to_string(), "Meta+Space");
        assert_eq!("cmd+e".parse::<Hotkey>().unwrap().to_string(), "Meta+E");
        assert_eq!("Ctrl+Esc".parse::<Hotkey>().unwrap().key.0, "Escape");
        assert_eq!("Alt+PrtSc".parse::<Hotkey>().unwrap().key.0, "PrintScreen");
        // Bare function keys need no modifier.
        assert!("F9".parse::<Hotkey>().unwrap().mods.is_empty());
    }

    #[test]
    fn parsing_rejects_nonsense() {
        assert_eq!("".parse::<Hotkey>(), Err(HotkeyParseError::Empty));
        assert!(matches!("Ctrl+Shift".parse::<Hotkey>(), Err(HotkeyParseError::NoKey(_))));
        assert!(matches!("Ctrl+Banana".parse::<Hotkey>(), Err(HotkeyParseError::UnknownKey(_))));
    }

    #[test]
    fn display_round_trips_through_parse() {
        for s in ["Ctrl+Alt+Shift+Meta+F12", "Alt+`", "Ctrl+Numpad5", "Shift+PageDown"] {
            let hk: Hotkey = s.parse().unwrap();
            assert_eq!(hk.to_string(), s);
            assert_eq!(hk.to_string().parse::<Hotkey>().unwrap(), hk);
        }
    }

    #[test]
    fn every_classic_action_has_a_legacy_key_and_ids_are_unique() {
        for a in Action::CLASSIC {
            assert!(a.legacy_option_key().is_some(), "{a:?}");
        }
        let mut ids: Vec<&str> = Action::ALL.iter().map(|a| a.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Action::ALL.len());
        for a in Action::ALL {
            assert_eq!(Action::from_id(a.id()), Some(a));
            // The id matches what serde writes.
            assert_eq!(serde_json::to_string(&a).unwrap(), format!("\"{}\"", a.id()));
        }
    }
}
