//! Interface translations.
//!
//! Strings are looked up by a dotted key (`main.translate`,
//! `options.hotkeys.enable`). English is compiled in; a locale file supplies
//! the rest and anything it lacks falls back to English, so a half-finished
//! translation is still usable.
//!
//! Two file formats load:
//!
//! * **Native** — `{ "code": "ru", "name": "Russian", "native_name": "Русский",
//!   "strings": { "main.translate": "Перевести", ... } }`.
//! * **Classic** — the numeric-id `lang.json` files of the original program,
//!   37 of them. Every string here that has a counterpart in the classic UI
//!   carries the classic address in [`STRINGS`], so those files translate this
//!   interface without being converted.
//!
//! Some languages also ship compiled in ([`BUILTIN_LOCALES`], native files
//! under `assets/locales/`) and cover every key. Loading a language layers
//! English, then the built-in pack, then a classic file, then the user's own
//! file (see `glossa_engine::LocaleCatalog::load`), so a classic file written
//! for the old program no longer leaves Glossa's newer strings in English.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::jsonc;

/// A loaded interface language.
#[derive(Debug, Clone, Default)]
pub struct Locale {
    pub code: String,
    pub name: String,
    pub native_name: String,
    pub author: String,
    /// Read from a classic numeric-id `lang.json`.
    pub classic: bool,
    strings: HashMap<String, String>,
}

/// A language compiled into the program.
pub struct BuiltinLocale {
    pub code: &'static str,
    pub name: &'static str,
    pub native_name: &'static str,
    /// The pack in the native format.
    pub source: &'static str,
}

/// The languages that ship inside the executable. English needs no pack: it
/// is the second column of [`STRINGS`].
pub static BUILTIN_LOCALES: &[BuiltinLocale] = &[BuiltinLocale {
    code: "ru",
    name: "Russian",
    native_name: "Русский",
    source: include_str!("../../../assets/locales/ru.json"),
}];

impl Locale {
    /// English: every key, nothing loaded.
    pub fn english() -> Self {
        Locale {
            code: "en".into(),
            name: "English".into(),
            native_name: "English".into(),
            author: String::new(),
            classic: false,
            strings: HashMap::new(),
        }
    }

    /// The compiled-in pack for `code` (`ru`, `ru-RU`), if there is one.
    pub fn builtin(code: &str) -> Option<Locale> {
        let base = code.split(['-', '_']).next().unwrap_or(code);
        let pack = BUILTIN_LOCALES.iter().find(|b| b.code.eq_ignore_ascii_case(base))?;
        match Locale::parse(pack.source) {
            Ok(l) => Some(l),
            Err(e) => {
                // Only a broken build gets here; the tests parse every pack.
                tracing::error!("built-in locale {}: {e}", pack.code);
                None
            }
        }
    }

    /// Lay `over` on top of this locale: its strings win, and its name and
    /// author replace these where it has them.
    pub fn merge(&mut self, over: Locale) {
        for (field, value) in
            [(&mut self.name, over.name), (&mut self.native_name, over.native_name), (&mut self.author, over.author)]
        {
            if !value.is_empty() {
                *field = value;
            }
        }
        self.strings.extend(over.strings);
    }

    /// Whether this locale itself translates `key` (rather than falling back
    /// to English).
    pub fn has(&self, key: &str) -> bool {
        self.strings.contains_key(key)
    }

    /// The text for `key`, in this language if it has it, else in English,
    /// else the key itself (so a missing string is visible, not blank).
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        if let Some(s) = self.strings.get(key) {
            return s;
        }
        english(key).unwrap_or(key)
    }

    /// `get`, with `%s` replaced by `arg` — the classic placeholder.
    pub fn fmt(&self, key: &str, arg: &str) -> String {
        self.get(key).replacen("%s", arg, 1)
    }

    /// `get`, with each `%s` replaced by the next of `args`, in order. An
    /// argument that itself contains `%s` is left alone.
    pub fn fmt_args(&self, key: &str, args: &[&str]) -> String {
        let mut parts = self.get(key).split("%s");
        let mut out = parts.next().unwrap_or_default().to_owned();
        let mut args = args.iter();
        for part in parts {
            out.push_str(args.next().copied().unwrap_or("%s"));
            out.push_str(part);
        }
        out
    }

    /// How many of the known keys this locale actually translates.
    pub fn coverage(&self) -> (usize, usize) {
        let have = STRINGS.iter().filter(|(k, _, _)| self.strings.contains_key(*k)).count();
        (have, STRINGS.len())
    }

    /// Read a locale file of either format.
    pub fn parse(text: &str) -> Result<Locale, String> {
        let value: Value = serde_json::from_str(&jsonc::strip(text)).map_err(|e| e.to_string())?;
        if value.get("Strings").is_some() || value.get("Windows").is_some() {
            Ok(Locale::from_classic(&value))
        } else {
            #[derive(Deserialize)]
            struct Native {
                code: String,
                #[serde(default)]
                name: String,
                #[serde(default)]
                native_name: String,
                #[serde(default)]
                author: String,
                #[serde(default)]
                strings: HashMap<String, String>,
            }
            let n: Native = serde_json::from_value(value).map_err(|e| e.to_string())?;
            Ok(Locale {
                code: n.code,
                name: n.name,
                native_name: n.native_name,
                author: n.author,
                classic: false,
                strings: n.strings,
            })
        }
    }

    fn from_classic(v: &Value) -> Locale {
        let flat = flatten_classic(v);
        let mut strings = HashMap::new();
        for (key, _, addr) in STRINGS {
            if let Some(addr) = addr {
                if let Some(text) = flat.get(*addr) {
                    if !text.is_empty() {
                        strings.insert((*key).to_owned(), unmnemonic(text));
                    }
                }
            }
        }
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let mut native_name = s("LanguageNativeName");
        if native_name.is_empty() {
            native_name = s("LanguageName");
        }
        Locale {
            code: s("LanguageCode"),
            name: s("LanguageName"),
            native_name,
            author: s("Author"),
            classic: true,
            strings,
        }
    }

    /// Serialise in the native format, e.g. to convert a classic file.
    pub fn to_native_json(&self) -> String {
        let mut strings: Vec<(&String, &String)> = self.strings.iter().collect();
        strings.sort();
        let map: serde_json::Map<String, Value> =
            strings.into_iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "code": self.code,
            "name": self.name,
            "native_name": self.native_name,
            "author": self.author,
            "strings": map,
        }))
        .unwrap_or_default()
    }
}

/// The classic files mark menu accelerators with `&` and write a literal
/// ampersand as `&&`.
fn unmnemonic(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                chars.next();
                out.push('&');
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Turn a classic locale file into `address -> text`. Addresses:
///
/// * `s:<id>` — a global string
/// * `w:<window>:caption` — a window title
/// * `w:<window>:<control>` — a control
/// * `w:<window>:<control>:<item>` — an item of a list control
/// * `w:<window>:s:<id>` — a string scoped to a window
/// * `m:<menu>:<item>` — a menu item
pub fn flatten_classic(v: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let pair = |item: &Value| -> Option<(i64, Value)> {
        let arr = item.as_array()?;
        Some((arr.first()?.as_i64()?, arr.get(1)?.clone()))
    };
    if let Some(list) = v.get("Strings").and_then(Value::as_array) {
        for item in list {
            if let Some((id, Value::String(text))) = pair(item) {
                out.insert(format!("s:{id}"), text);
            }
        }
    }
    if let Some(windows) = v.get("Windows").and_then(Value::as_array) {
        for w in windows {
            let Some(wid) = w.get("Id").and_then(Value::as_i64) else { continue };
            if let Some(cap) = w.get("Caption").and_then(Value::as_str) {
                out.insert(format!("w:{wid}:caption"), cap.to_owned());
            }
            for item in w.get("Controls").and_then(Value::as_array).into_iter().flatten() {
                match pair(item) {
                    Some((cid, Value::String(text))) => {
                        out.insert(format!("w:{wid}:{cid}"), text);
                    }
                    Some((cid, Value::Array(sub))) => {
                        for s in &sub {
                            if let Some((sid, Value::String(text))) = pair(s) {
                                out.insert(format!("w:{wid}:{cid}:{sid}"), text);
                            }
                        }
                    }
                    _ => {}
                }
            }
            for item in w.get("Strings").and_then(Value::as_array).into_iter().flatten() {
                if let Some((sid, Value::String(text))) = pair(item) {
                    out.insert(format!("w:{wid}:s:{sid}"), text);
                }
            }
        }
    }
    if let Some(menus) = v.get("Menus").and_then(Value::as_array) {
        for m in menus {
            let Some(mid) = m.get("Id").and_then(Value::as_i64) else { continue };
            for item in m.get("Items").and_then(Value::as_array).into_iter().flatten() {
                if let Some((iid, Value::String(text))) = pair(item) {
                    out.insert(format!("m:{mid}:{iid}"), text);
                }
            }
        }
    }
    out
}

fn english(key: &str) -> Option<&'static str> {
    STRINGS.iter().find(|(k, _, _)| *k == key).map(|(_, en, _)| *en)
}

/// Every interface string: key, English text, classic address (if the
/// classic UI had this string).
pub static STRINGS: &[(&str, &str, Option<&str>)] = &[
    // -- messages ---------------------------------------------------------
    ("msg.clear", "Clear current translation (Ctrl+N)", Some("s:171")),
    ("msg.translate_tip", "Translate (Ctrl+Enter)", Some("s:172")),
    ("msg.swap_tip", "Swap translation direction (Ctrl+I)", Some("s:173")),
    ("msg.listen_tip", "Listen to the text", Some("s:174")),
    ("msg.back_tip", "Go back (Alt+Left Arrow)", Some("s:175")),
    ("msg.forward_tip", "Go forward (Alt+Right Arrow)", Some("s:184")),
    ("msg.cannot_detect", "Cannot detect language. Please choose it manually.", Some("s:176")),
    ("msg.source_unsupported", "Service doesn't support source language.", Some("s:177")),
    ("msg.target_unsupported", "Service doesn't support destination language.", Some("s:198")),
    ("msg.about", "About Glossa", Some("s:178")),
    ("msg.version", "Version ", Some("s:179")),
    ("msg.update_unavailable", "Update server is not available", Some("s:180")),
    ("msg.update_available", "New version (%s) is available", Some("s:181")),
    ("msg.up_to_date", "%s is up to date", Some("s:182")),
    ("msg.checking_updates", "Checking for updates...", Some("s:183")),
    ("msg.update_failed", "Update failed", Some("s:185")),
    ("msg.romanization", "Romanization: ", Some("s:186")),
    ("msg.always_on_top", "Always on top", Some("s:187")),
    ("msg.keyboard", "Keyboard", Some("s:188")),
    ("msg.no_internet", "No Internet connection.", Some("s:189")),
    ("msg.timeout", "No data returned (timeout while sending data).", Some("s:190")),
    ("msg.no_data", "No data returned.", Some("s:191")),
    ("msg.nothing_to_translate", "No data to translate.", Some("s:192")),
    ("msg.capability_unsupported", "Service doesn't support requested capability.", Some("s:193")),
    ("msg.cannot_play", "Cannot play audio stream.", Some("s:194")),
    ("msg.no_audio", "Service doesn't support audio for this language, or text is too long.", Some("s:195")),
    ("msg.script_error", "Script service error.", Some("s:196")),
    ("msg.unexpected", "Unexpected error.", Some("s:197")),
    ("msg.file_not_found", "Cannot find file: ", Some("s:199")),
    ("msg.downloading_update", "Downloading update...", Some("s:200")),
    ("msg.add_favorite", "Add to favorites", Some("s:201")),
    ("msg.remove_favorite", "Remove from favorites", Some("s:202")),
    ("msg.no_microphone", "Make sure the microphone is connected to your computer.", Some("s:203")),
    ("msg.no_connection", "Unable to establish Internet connection", Some("s:204")),
    ("msg.speech_on", "Turn on speech input (%s)", Some("s:205")),
    ("msg.speech_off", "Turn off speech input", Some("s:206")),
    ("msg.no_text_in_image", "No text found in the selected area.", None),
    ("msg.ocr_unavailable", "OCR is not available: %s", None),
    ("msg.copied", "Copied", None),
    ("msg.translating", "Translating…", None),
    ("msg.recognizing", "Recognising…", None),
    ("msg.yes", "Yes", None),
    ("msg.no", "No", None),
    ("msg.ms", "%s ms", None),
    ("msg.watching", "Watching the region — press the hotkey again to stop.", None),
    ("msg.watch_stopped", "Stopped watching the region.", None),
    ("msg.not_available", "%s — not available in this version.", None),
    // -- main window --------------------------------------------------------
    ("main.title", "Glossa", Some("w:1:caption")),
    ("main.translate", "Translate", Some("w:1:1004")),
    ("main.auto_detect", "Auto-detect", None),
    ("main.auto_detect_as", "Auto-detect (%s)", None),
    ("main.source_placeholder", "Type, paste or drop text here", None),
    ("main.tried_first", "Tried first: %s", None),
    ("lang.search", "Search…", None),
    // -- on-screen keyboard -------------------------------------------------
    ("keyboard.space", "Space", None),
    ("keyboard.backspace", "Backspace", None),
    ("keyboard.enter", "Enter", None),
    ("keyboard.shift", "Shift", None),
    // -- about window -------------------------------------------------------
    ("about.tagline", "A clean-room reincarnation of QTranslate.", None),
    ("about.summary", "Translation, dictionaries and screen OCR — Rust, egui, no web engine.", None),
    ("about.ocr_credit", "OCR: chrome-lens-py by Bropines (MIT)", None),
    ("about.scripting", "Scripting: Boa (JavaScript), CPython modules, native modules", None),
    ("about.compat", "Compatible with classic QTranslate services, themes, locales, and pot-app plugins", None),
    ("about.status", "%s providers · %s modules loaded · %s failed", None),
    // -- popup --------------------------------------------------------------
    ("popup.dictionary", "Dictionary search for", Some("w:6:10:20")),
    ("popup.listen", "Listen to selected text", Some("w:6:10:30")),
    ("popup.copy", "Copy translation to clipboard", Some("w:6:10:40")),
    ("popup.title_direction", "%s to %s (%s)", None),
    ("popup.reset_size", "Back to automatic size", None),
    ("mouse.translate", "Translate the selection", None),
    ("options.appearance.backdrop", "Window backdrop:", None),
    ("options.appearance.popup_backdrop", "Popup backdrop:", None),
    ("options.appearance.popup_shade", "Shade when not blurred (0 - 90):", None),
    ("options.backdrop.none", "None", None),
    ("options.backdrop.blur", "Blur", None),
    ("options.backdrop.needs_opengl", "Backdrops need Windows or macOS and the OpenGL engine.", None),
    ("popup.close", "Close", None),
    ("popup.pin", "Keep it open", None),
    ("popup.unpin", "Let it hide again", None),
    ("options.services.needs_key", "needs a key", None),
    ("options.services.pick_one", "Select a service in the list to see its settings.", None),
    ("options.services.capabilities", "Capabilities:", None),
    ("options.services.no_settings", "No settings.", None),
    ("options.services.show_keys", "Show keys", None),
    ("options.restart_needed", "Some changes take effect after a restart.", None),
    ("options.restart_now", "Restart now", None),
    ("options.restart_later", "Later", None),
    ("options.appearance.icon_scale", "Icon size:", None),
    ("options.appearance.show_border", "Show the popup border", None),
    ("popup.replace", "Replace selected text", Some("w:6:10:50")),
    // -- dictionary ---------------------------------------------------------
    ("dict.title", "Dictionary", Some("w:5:caption")),
    ("dict.no_results", "No results found for '%s'.", Some("w:5:s:10")),
    // -- history ------------------------------------------------------------
    ("history.title", "History", Some("w:3:caption")),
    ("history.save_as", "Save as...", Some("w:3:1160")),
    ("history.clear", "Clear", Some("w:3:1067")),
    ("history.search", "Search history", None),
    ("history.favorites_only", "Favourites only", None),
    ("history.confirm_clear", "Clear all non-favourite entries?", None),
    ("history.format_text", "Text file", None),
    ("history.format_csv", "CSV file", None),
    ("history.format_html", "HTML file", None),
    ("history.format_json", "JSON file", None),
    // -- options frame --------------------------------------------------------
    ("options.title", "Options", Some("w:2:caption")),
    ("options.ok", "OK", Some("w:2:1")),
    ("options.cancel", "Cancel", Some("w:2:2")),
    ("options.apply", "Apply", Some("w:2:1049")),
    ("options.saved", "Saved.", None),
    ("options.remove", "Remove", None),
    // -- options: basics --------------------------------------------------------
    ("options.basics", "Basics", Some("w:10:caption")),
    ("options.basics.start_with_system", "Start with Windows", Some("w:10:1045")),
    ("options.basics.ui_language", "Interface language:", Some("w:10:1070")),
    ("options.basics.font", "Font name:", Some("w:10:1075")),
    ("options.basics.text_size", "Text size:", Some("w:10:1074")),
    ("options.basics.first_language", "First language:", Some("w:10:1068")),
    ("options.basics.second_language", "Second language:", Some("w:10:1069")),
    ("options.basics.speech_input", "Speech input:", Some("w:10:1071")),
    ("options.basics.auto_detect_group", "Auto-detect languages", Some("w:10:1063")),
    ("options.basics.general_group", "General", Some("w:10:1046")),
    ("options.basics.history_group", "History", Some("w:10:1064")),
    ("options.basics.enable_history", "Enable history", Some("w:10:1122")),
    ("options.basics.clear_history_on_exit", "Clear history on exit", Some("w:10:1124")),
    ("options.basics.expand_items", "Expand items", Some("w:10:1123")),
    ("options.basics.ui_language_system", "System", None),
    ("options.basics.font_default", "--- Default ---", None),
    (
        "options.basics.smart_direction",
        "Smart direction: the first language goes to the second, everything else to the first",
        None,
    ),
    // -- options: internet ------------------------------------------------------
    ("options.internet", "Internet", Some("w:11:caption")),
    ("options.internet.connection", "Connection", Some("w:11:1049")),
    ("options.internet.timeout", "Timeout, sec:", Some("w:11:1084")),
    ("options.internet.proxy", "Proxy", Some("w:11:1048")),
    ("options.internet.host", "Host", Some("w:11:1034")),
    ("options.internet.port", "Port", Some("w:11:1035")),
    ("options.internet.username", "Username", Some("w:11:1036")),
    ("options.internet.password", "Password", Some("w:11:1037")),
    ("options.internet.proxy_system", "Use system proxy settings", Some("w:11:1135:0")),
    ("options.internet.proxy_none", "No proxy", Some("w:11:1135:1")),
    ("options.internet.proxy_manual", "Manual proxy configuration", Some("w:11:1135:2")),
    ("options.internet.proxy_auto", "Auto-detect proxy settings", Some("w:11:1135:3")),
    ("options.internet.retries", "Retries:", None),
    ("options.internet.password_plain", "The password is stored in the settings file in plain text.", None),
    // -- options: services --------------------------------------------------------
    ("options.services", "Services", Some("w:12:caption")),
    ("options.services.add_dictionary", "Add dictionary...", Some("w:12:1177")),
    ("options.services.remove_dictionary", "Remove dictionary", Some("w:12:1180")),
    ("options.services.translation", "Translation", None),
    ("options.services.dictionaries", "Dictionaries", None),
    ("options.services.speech", "Speech", None),
    ("options.services.ocr", "Text recognition", None),
    ("options.services.settings", "Settings", None),
    ("options.services.fail_over", "If a service fails, try the next one", None),
    ("options.services.origin_builtin", "Built-in", None),
    ("options.services.origin_bundled", "Bundled", None),
    ("capability.translate", "translation", None),
    ("capability.detect_language", "language detection", None),
    ("capability.listen", "speech", None),
    ("capability.dictionary", "dictionary", None),
    ("capability.ocr", "text recognition", None),
    ("capability.speech_input", "speech input", None),
    ("capability.batch", "batches", None),
    ("capability.alternatives", "alternatives", None),
    ("capability.promptable", "custom prompt", None),
    ("capability.offline", "offline", None),
    // -- options: AI endpoints (on the Services page) ---------------------------
    ("options.ai.title", "AI endpoints (OpenAI-compatible)", None),
    (
        "options.ai.hint",
        "Each endpoint is a translator of its own: OpenAI, OpenRouter, DeepSeek, Gemini, a local Ollama… \
         Select it in the list above for its settings.",
        None,
    ),
    ("options.ai.add", "Add endpoint", None),
    ("options.ai.load_models", "Load models", None),
    ("options.ai.test", "Test", None),
    ("options.ai.test_tip", "Translate a short sample", None),
    ("options.ai.test_ok", "OK, %s ms: %s", None),
    ("options.ai.models", "Models (%s)", None),
    ("options.ai.models_loaded", "%s models", None),
    ("options.ai.pick_model", "Pick a model", None),
    // -- options: languages --------------------------------------------------------
    ("options.languages", "Languages", Some("w:13:caption")),
    ("options.languages.toggle_all", "Check / Uncheck all", Some("w:13:1099")),
    // -- options: appearance ---------------------------------------------------------
    ("options.appearance", "Appearance", Some("w:14:caption")),
    ("options.appearance.general", "General", Some("w:14:1076")),
    ("options.appearance.window_style", "Enable window style", Some("w:14:1123")),
    ("options.appearance.theme", "Theme:", Some("w:14:1169")),
    ("options.appearance.auto_size", "Enable auto size", Some("w:14:1158")),
    ("options.appearance.auto_position", "Enable auto position", Some("w:14:1160")),
    ("options.appearance.activate", "Activate when opened", Some("w:14:1159")),
    ("options.appearance.pin_when_dragging", "Pin when dragging", Some("w:14:1161")),
    ("options.appearance.popup_group", "Popup window", Some("w:14:1124")),
    ("options.appearance.back_color", "Background color:", Some("w:14:1092")),
    ("options.appearance.text_color", "Text color:", Some("w:14:1091")),
    ("options.appearance.frame_color", "Frame color:", Some("w:14:1090")),
    ("options.appearance.transparency", "Transparency (0 - 90):", Some("w:14:1145")),
    ("options.appearance.frame_thickness", "Frame thickness:", Some("w:14:1146")),
    ("options.appearance.auto_hide", "Auto-hide delay:", Some("w:14:1147")),
    ("options.appearance.seconds", "seconds", Some("w:14:1194")),
    ("options.appearance.icons", "Icons:", Some("w:14:1137")),
    ("options.appearance.icon_dictionary", "Show open dictionary icon", Some("w:14:1138")),
    ("options.appearance.icon_listen", "Show listen to selected text icon", Some("w:14:1139")),
    ("options.appearance.icon_copy", "Show copy text to clipboard icon", Some("w:14:1140")),
    ("options.appearance.icon_replace", "Show replace selected text icon", Some("w:14:1141")),
    ("options.appearance.ui_scale", "Interface scale:", None),
    ("options.appearance.edit_theme", "Edit theme file", None),
    ("options.appearance.reload_themes", "Reload themes", None),
    ("options.appearance.classic_theme", "%s  (classic)", None),
    ("options.appearance.window_transparency", "Window transparency (0 - 90):", None),
    ("options.appearance.renderer", "Graphics engine (after restart):", None),
    ("options.appearance.renderer_opengl", "OpenGL — lightest", None),
    ("options.appearance.renderer_gpu", "GPU — Direct3D 12 / Vulkan / Metal", None),
    ("options.appearance.renderer_software", "Software — no GPU (WARP)", None),
    ("options.appearance.unload_after", "Unload graphics after hiding, s (0 = never):", None),
    (
        "options.appearance.unload_hint",
        "While everything is hidden in the tray, Glossa frees its graphics and keeps only the tray, hotkeys \
         and engine — a few MB. The first window afterwards takes a moment longer to appear.",
        None,
    ),
    ("options.appearance.popup_width", "Width:", None),
    ("options.appearance.width_max", "max ", None),
    ("options.appearance.width_min", "min ", None),
    // -- options: exceptions -----------------------------------------------------------
    ("options.exceptions", "Exceptions", Some("w:15:caption")),
    ("options.exceptions.enabled_for", "Enabled for these programs:", Some("w:15:1162:0")),
    ("options.exceptions.disabled_for", "Disabled for these programs:", Some("w:15:1162:1")),
    ("options.exceptions.add", "Add", Some("w:15:1101")),
    ("options.exceptions.modify", "Modify", Some("w:15:1103")),
    ("options.exceptions.delete", "Delete", Some("w:15:1102")),
    ("options.exceptions.smart", "Enable smart detection", Some("w:15:1113")),
    ("options.exceptions.program", "Program name", None),
    ("options.exceptions.window_class", "Window class name", None),
    ("options.exceptions.any_program", "* All Programs *", None),
    ("options.exceptions.any_window", "* All Windows *", None),
    ("options.exceptions.from_active", "From the active window", None),
    ("options.exceptions.from_active_tip", "Fill from the window that was active before this one", None),
    // -- options: hotkeys ----------------------------------------------------------------
    ("options.hotkeys", "Hotkeys", Some("w:16:caption")),
    ("options.hotkeys.enable", "Enable hotkeys", Some("w:16:1118")),
    ("options.hotkeys.action", "Action", None),
    ("options.hotkeys.hotkey", "Hotkey", None),
    ("options.hotkeys.press_keys", "Press a key combination… (Esc cancels, Backspace clears)", None),
    ("options.hotkeys.conflict", "Also bound to: %s", None),
    (
        "options.hotkeys.wayland",
        "Global hotkeys are not available in this session (Wayland). Bind a shortcut in your desktop's \
         keyboard settings to:",
        None,
    ),
    ("action.show_main_window", "Show main window", Some("w:16:1165:0")),
    ("action.show_popup", "Show popup window", Some("w:16:1165:1")),
    ("action.show_dictionary", "Show dictionary window", Some("w:16:1165:2")),
    ("action.show_history", "Show history window", Some("w:16:1165:3")),
    ("action.show_keyboard", "Show keyboard window", Some("w:16:1165:4")),
    ("action.listen_selection", "Listen to selected text", Some("w:16:1165:5")),
    ("action.copy_translation", "Copy translation to clipboard", Some("w:16:1165:6")),
    ("action.replace_selection", "Replace selected text", Some("w:16:1165:7")),
    ("action.toggle_mouse_mode", "Switch mouse mode", Some("w:16:1165:8")),
    ("action.translate_clipboard", "Translate clipboard content", Some("w:16:1165:9")),
    ("action.translate_clipboard_in_main", "Translate clipboard in main window", Some("w:16:1165:10")),
    ("action.translate_clipboard_in_popup", "Translate clipboard in popup window", Some("w:16:1165:11")),
    ("action.dictionary_from_clipboard", "Dictionary from clipboard", Some("w:16:1165:12")),
    ("action.convert_layout", "Change keyboard layout and selected text", Some("w:16:1165:13")),
    ("action.listen_translation", "Listen to selected text translation", Some("w:16:1165:14")),
    ("action.text_recognition", "Text recognition", Some("w:16:1165:15")),
    ("action.speech_input", "Speech input", Some("w:16:1165:16")),
    ("action.recognize_and_translate", "Recognise and translate a screen region", None),
    ("action.repeat_last_recognition", "Repeat the last recognised region", None),
    ("action.recognize_to_clipboard", "Recognise a screen region to the clipboard", None),
    ("action.watch_region", "Watch a screen region and translate changes", None),
    ("action.swap_languages", "Swap translation direction", None),
    ("action.next_service", "Switch to the next service", None),
    ("action.show_options", "Show options", None),
    ("action.quit", "Exit", None),
    // -- options: advanced ---------------------------------------------------------------
    ("options.advanced", "Advanced", Some("w:17:caption")),
    ("options.advanced.group", "Advanced", Some("w:17:1146")),
    ("options.advanced.browser", "Default browser:", Some("w:17:1151")),
    ("options.advanced.copy_action", "Copy action:", Some("w:17:1147")),
    ("options.advanced.layout_indicator", "Keyboard layout indicator:", Some("w:17:1149")),
    ("options.advanced.preferred_domain", "Preferred domain:", Some("w:17:1157")),
    ("options.advanced.gui_translation", "Enable GUI translation", Some("w:17:1154")),
    ("options.advanced.slower_listen", "Enable listening to the same text at a slower speed", Some("w:17:1159")),
    ("options.advanced.remove_line_breaks", "Remove line breaks", Some("w:17:1160")),
    ("options.advanced.mouse_mode_group", "Mouse mode", Some("w:17:1186")),
    ("options.advanced.mouse_ctrl_only", "Enable mouse mode only when Ctrl is pressed", Some("w:17:1155")),
    ("options.advanced.tray_toggles_mouse", "Click on system tray icon switches mouse mode", Some("w:17:1156")),
    ("options.advanced.api_keys", "API keys", Some("w:17:1158")),
    ("options.advanced.ocr_key", "OCR API key:", Some("w:17:1195")),
    ("options.advanced.browser_default", "system default", None),
    ("options.advanced.copy_accessibility", "Accessibility only", None),
    ("options.advanced.copy_wait", "Selection copy wait, ms:", None),
    (
        "options.advanced.mouse_mode_note",
        "Mouse mode (translate on mouse selection, without a hotkey) is saved but not active in this version.",
        None,
    ),
    // -- options: updates ------------------------------------------------------------------
    ("options.updates", "Updates", Some("w:18:caption")),
    ("options.updates.auto", "Automatically check for updates", Some("w:18:1188")),
    ("options.updates.check_now", "Check now", Some("w:18:1190")),
    ("options.updates.install", "Install", Some("w:18:1191")),
    (
        "options.updates.no_server",
        "Glossa has no update server yet; new versions are published as releases of the project.",
        None,
    ),
    // -- options: new pages ---------------------------------------------------------------
    ("options.ocr", "Text recognition", None),
    ("options.modules", "Modules", None),
    ("options.server", "Control API", None),
    // -- options: text recognition ----------------------------------------------------------
    ("options.ocr.provider", "Provider:", None),
    ("options.ocr.translate_after", "Translate the recognised text", None),
    ("options.ocr.keep_line_breaks", "Keep line breaks", None),
    ("options.ocr.watch_interval", "Watched region check, ms:", None),
    ("options.ocr.save_dir", "Save captured regions to:", None),
    ("options.ocr.lens_auto", "Auto", None),
    ("options.ocr.lens_daemon", "Daemon only", None),
    ("options.ocr.lens_cli", "CLI only", None),
    ("options.ocr.lens_endpoint", "Daemon URL:", None),
    ("options.ocr.lens_endpoint_hint", "discovered automatically", None),
    ("options.ocr.lens_cli_path", "lens_scan path:", None),
    ("options.ocr.lens_cli_hint", "found on PATH", None),
    (
        "options.ocr.lens_install",
        "Install: pip install \"chrome-lens-py[clipboard]\" — the warm daemon makes a scan take ~0.4 s.",
        None,
    ),
    // -- options: modules ----------------------------------------------------------------------
    ("options.modules.runtimes", "Runtimes", None),
    ("options.modules.legacy_scripts", "Load classic QTranslate service scripts", None),
    ("options.modules.native", "Load native modules (they run with Glossa's full privileges)", None),
    ("options.modules.python", "Python interpreter:", None),
    ("options.modules.python_hint", "python3 / py -3 from PATH", None),
    ("options.modules.call_timeout", "Call timeout, ms:", None),
    ("options.modules.classic", "Classic QTranslate installation", None),
    ("options.modules.choose", "Choose…", None),
    ("options.modules.use_services", "services", None),
    ("options.modules.use_themes", "themes", None),
    ("options.modules.use_locales", "interface languages", None),
    ("options.modules.import", "Import its settings and history", None),
    ("options.modules.imported", "Imported: settings %s, %s history entries", None),
    ("options.modules.folders", "Module folders", None),
    ("options.modules.open_folder", "Open my modules folder", None),
    ("options.modules.install", "Install module…", None),
    ("options.modules.install_tip", "A module folder, or a pot-app .potext file", None),
    ("options.modules.potext_filter", "pot-app plugin", None),
    ("options.modules.installed", "Installed. Enable it on the Services page.", None),
    ("options.modules.not_a_module", "That folder has no module.toml, Service.js or info.json.", None),
    ("options.modules.reload", "Reload modules", None),
    ("options.modules.add_folder", "Add a folder…", None),
    ("options.modules.status", "Status: %s loaded, %s failed, %s skipped", None),
    ("options.modules.failed_item", "failed  %s: %s", None),
    // -- options: control API ------------------------------------------------------------------
    ("options.server.enable", "Enable the local control API (restart to apply)", None),
    ("options.server.listen_on", "Listen on:", None),
    ("options.server.token", "Token:", None),
    ("options.server.regenerate", "Regenerate", None),
    ("options.server.show_token", "Show token", None),
    ("options.server.running", "Running.", None),
    ("options.server.not_running", "Not running: %s", None),
    (
        "options.server.bind_hint",
        "Bind any of these to a key in your desktop or launcher (essential on Wayland):",
        None,
    ),
    ("options.server.cmd_popup", "Translate the selection in the popup", None),
    ("options.server.cmd_ocr", "Recognise a screen region", None),
    ("options.server.cmd_ocr_translate", "Recognise and translate", None),
    ("options.server.cmd_main", "Show the main window", None),
    // -- main menu (the "⋮" button) -------------------------------------------------------
    ("menu.spell_checking", "Spell checking", Some("m:1:10")),
    ("menu.instant_translation", "Instant translation", Some("m:1:20")),
    ("menu.back_translation", "Backward translation", Some("m:1:30")),
    ("menu.extended", "Extended", Some("m:1:40")),
    ("menu.read_phonetically", "Read phonetically", Some("m:1:50")),
    ("menu.clear_on_drop", "Clear input on Drag & Drop", Some("m:1:60")),
    ("menu.auto_cleanup", "Auto-cleanup of translation", Some("m:1:61")),
    ("menu.save_on_exit", "Save contents on exit", Some("m:1:62")),
    ("menu.show_top_pane", "Show top pane", Some("m:1:65")),
    ("menu.show_middle_pane", "Show middle pane", Some("m:1:70")),
    ("menu.show_services_pane", "Show services pane", Some("m:1:80")),
    ("menu.tray_on_minimize", "Minimize to system tray on minimize", Some("m:1:81")),
    ("menu.tray_on_close", "Minimize to system tray on close", Some("m:1:82")),
    ("menu.on_startup", "On startup", Some("m:1:85")),
    ("menu.startup_show", "Show window", Some("m:1:86")),
    ("menu.startup_tray", "Minimize to tray", Some("m:1:87")),
    ("menu.startup_restore", "Restore previous state", Some("m:1:88")),
    ("menu.dictionary", "Dictionary", Some("m:1:90")),
    ("menu.history", "History", Some("m:1:100")),
    ("menu.keyboard", "Keyboard", Some("m:1:110")),
    ("menu.options", "Options", Some("m:1:120")),
    ("menu.about", "About Glossa", Some("m:1:125")),
    ("menu.help", "Help", Some("m:1:130")),
    ("menu.exit", "Exit", Some("m:1:140")),
    ("menu.stacked_results", "Show all services at once", None),
    // -- text field context menu -------------------------------------------------------------
    ("edit.undo", "Undo", Some("m:2:10")),
    ("edit.cut", "Cut", Some("m:2:20")),
    ("edit.cut_all", "Cut all", Some("m:2:30")),
    ("edit.copy", "Copy", Some("m:2:40")),
    ("edit.copy_all", "Copy all", Some("m:2:50")),
    ("edit.paste", "Paste", Some("m:2:60")),
    ("edit.select_all", "Select all", Some("m:2:70")),
    ("edit.translate", "Translate", Some("m:2:75")),
    ("edit.dictionary", "Dictionary search for", Some("m:2:80")),
    ("edit.listen", "Listen", Some("m:2:90")),
    // -- history item menu ---------------------------------------------------------------------
    ("history.open", "Open", Some("m:3:10")),
    ("history.copy_text", "Copy text", Some("m:3:15")),
    ("history.copy_translation", "Copy translation", Some("m:3:20")),
    ("history.delete", "Delete", Some("m:3:30")),
    ("history.listen", "Listen", Some("m:3:40")),
    // -- language menu ---------------------------------------------------------------------------
    ("lang.reset", "Reset", Some("m:4:10")),
    ("lang.edit", "Edit...", Some("m:4:20")),
    ("lang.always_detect", "Always detect language", Some("m:4:30")),
    // -- tray menu --------------------------------------------------------------------------------
    ("tray.text_recognition", "Text recognition", Some("m:5:5")),
    ("tray.dictionary", "Dictionary", Some("m:5:10")),
    ("tray.history", "History", Some("m:5:20")),
    ("tray.keyboard", "Keyboard", Some("m:5:30")),
    ("tray.hotkeys", "Enable global hotkeys", Some("m:5:36")),
    ("tray.mouse_mode", "Mouse mode", Some("m:5:40")),
    ("tray.mouse_enabled", "Enabled", Some("m:5:50")),
    ("tray.mouse_icon", "Show icon", Some("m:5:60")),
    ("tray.mouse_translation", "Show translation", Some("m:5:70")),
    ("tray.mouse_translation_read", "Show translation and read", Some("m:5:71")),
    ("tray.options", "Options", Some("m:5:80")),
    ("tray.about", "About Glossa", Some("m:5:90")),
    ("tray.exit", "Exit", Some("m:5:100")),
    ("tray.show", "Show Glossa", None),
    // -- dictionary article menu --------------------------------------------------------------------
    ("article.select_all", "Select all", Some("m:6:10")),
    ("article.copy", "Copy", Some("m:6:20")),
    ("article.print", "Print...", Some("m:6:30")),
    ("article.translate", "Translate", Some("m:6:40")),
    ("article.search", "Search", Some("m:6:50")),
    ("article.listen", "Listen", Some("m:6:60")),
    ("history.show_full", "Show Full History", Some("m:7:10")),
    ("dict.show_info", "Show info", Some("m:8:10")),
    ("dict.remove", "Remove dictionary", Some("m:8:20")),
    ("dict.clear_history", "Clear history", Some("m:9:10")),
    ("dict.show_services", "Show services pane", Some("m:9:20")),
    ("dict.exact_search", "Exact search", Some("m:9:30")),
    ("dict.options", "Options", Some("m:9:40")),
    // -- OCR window ---------------------------------------------------------------------------------
    ("ocr.language", "OCR language", Some("m:10:110")),
    ("ocr.translate", "Translate", Some("m:10:120")),
    ("ocr.copy_image", "Copy image", Some("m:10:130")),
    ("ocr.copy_text", "Copy text", Some("m:10:140")),
    ("ocr.save_image", "Save image", Some("m:10:150")),
    ("ocr.edit_image", "Edit image", Some("m:10:160")),
    ("ocr.select_all", "Select all", Some("m:10:170")),
    ("ocr.clear_selection", "Clear selection", Some("m:10:180")),
    ("ocr.close", "Close", Some("m:10:190")),
    ("ocr.hint", "Drag to select an area. Esc cancels. Right-click for options.", None),
];

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSIC_RU: &str = r#"{
        "Author": "someone",
        "LanguageName": "Russian",
        "LanguageNativeName": "Русский",
        "LanguageCode": "ru",
        "Strings": [ [172, "Перевести (Ctrl+Enter)"], [181, "Доступна новая версия (%s)"] ],
        "Windows": [
            { "Id": 1, "Caption": "QTranslate", "Controls": [ [1004, "Перевести"] ] },
            { "Id": 16, "Caption": "Горячие клавиши", "Controls": [
                [1118, "Использовать горячие клавиши"],
                [1165, [ [0, "Показать главное окно"], [1, "Показать всплывающее окно"] ]]
            ]},
            { "Id": 5, "Caption": "Словарь", "Controls": [], "Strings": [ [10, "Нет результатов для '%s'."] ] },
        ],
        "Menus": [ { "Id": 1, "Items": [ [60, "Очищать при Drag && Drop"], [120, "&Настройки"] ] } ]
    }"#;

    #[test]
    fn english_is_built_in_and_missing_keys_show_themselves() {
        let en = Locale::english();
        assert_eq!(en.get("main.translate"), "Translate");
        assert_eq!(en.get("no.such.key"), "no.such.key");
    }

    #[test]
    fn classic_locale_files_translate_this_interface() {
        let ru = Locale::parse(CLASSIC_RU).unwrap();
        assert_eq!(ru.code, "ru");
        assert_eq!(ru.native_name, "Русский");
        assert_eq!(ru.get("main.translate"), "Перевести");
        assert_eq!(ru.get("msg.translate_tip"), "Перевести (Ctrl+Enter)");
        assert_eq!(ru.get("options.hotkeys"), "Горячие клавиши");
        assert_eq!(ru.get("options.hotkeys.enable"), "Использовать горячие клавиши");
        assert_eq!(ru.get("action.show_popup"), "Показать всплывающее окно");
        assert_eq!(ru.get("dict.title"), "Словарь");
        // Keys the file lacks fall back to English.
        assert_eq!(ru.get("history.title"), "History");
    }

    #[test]
    fn mnemonics_are_stripped_and_double_ampersands_kept() {
        let ru = Locale::parse(CLASSIC_RU).unwrap();
        assert_eq!(ru.get("menu.options"), "Настройки");
        assert_eq!(ru.get("menu.clear_on_drop"), "Очищать при Drag & Drop");
    }

    #[test]
    fn placeholders_format() {
        let ru = Locale::parse(CLASSIC_RU).unwrap();
        assert_eq!(ru.fmt("msg.update_available", "2.0"), "Доступна новая версия (2.0)");
        assert_eq!(ru.fmt("dict.no_results", "xyz"), "Нет результатов для 'xyz'.");
    }

    #[test]
    fn native_round_trip() {
        let ru = Locale::parse(CLASSIC_RU).unwrap();
        let native = ru.to_native_json();
        let back = Locale::parse(&native).unwrap();
        assert_eq!(back.code, "ru");
        assert_eq!(back.get("main.translate"), "Перевести");
        assert_eq!(back.coverage(), ru.coverage());
    }

    #[test]
    fn keys_and_addresses_are_unique() {
        let mut keys: Vec<&str> = STRINGS.iter().map(|(k, _, _)| *k).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate key in STRINGS");

        let mut addrs: Vec<&str> = STRINGS.iter().filter_map(|(_, _, a)| *a).collect();
        addrs.sort_unstable();
        let n = addrs.len();
        addrs.dedup();
        assert_eq!(addrs.len(), n, "two keys claim the same classic address");
    }

    #[test]
    fn every_action_has_a_label_key() {
        for a in crate::hotkey::Action::ALL {
            let key = format!("action.{}", a.id());
            assert!(english(&key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn every_capability_has_a_label_key() {
        for name in crate::Capabilities(u32::MAX).names() {
            let key = format!("capability.{name}");
            assert!(english(&key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn several_placeholders_fill_in_order() {
        let en = Locale::english();
        assert_eq!(en.fmt_args("options.ai.test_ok", &["12", "Hallo"]), "OK, 12 ms: Hallo");
        // Too few arguments leave the rest visible; an argument holding a
        // placeholder is not expanded again.
        assert_eq!(en.fmt_args("options.ai.test_ok", &["%s"]), "OK, %s ms: %s");
        assert_eq!(en.fmt_args("msg.ms", &["5", "extra"]), "5 ms");
    }

    #[test]
    fn merging_lays_strings_over_and_keeps_the_rest() {
        let mut ru = Locale::builtin("ru").unwrap();
        let built_in = ru.get("options.modules").to_owned();
        ru.merge(Locale::parse(CLASSIC_RU).unwrap());
        assert_eq!(ru.get("options.hotkeys"), "Горячие клавиши", "the classic file wins");
        assert_eq!(ru.get("main.translate"), "Перевести");
        assert_eq!(ru.get("options.modules"), built_in, "what the classic file lacks stays from the pack");
        assert_eq!(ru.author, "someone");
    }

    /// Every compiled-in pack parses, translates every key and nothing else,
    /// and keeps the English placeholders.
    #[test]
    fn built_in_packs_cover_every_key() {
        let known: std::collections::HashSet<&str> = STRINGS.iter().map(|(k, _, _)| *k).collect();
        for pack in BUILTIN_LOCALES {
            let l = Locale::parse(pack.source).unwrap_or_else(|e| panic!("{}: {e}", pack.code));
            assert_eq!(l.code, pack.code);
            assert_eq!(l.native_name, pack.native_name);
            let missing: Vec<&str> = STRINGS.iter().map(|(k, _, _)| *k).filter(|k| !l.has(k)).collect();
            assert!(missing.is_empty(), "{}: untranslated keys {missing:?}", pack.code);
            let mut stale: Vec<&String> = l.strings.keys().filter(|k| !known.contains(k.as_str())).collect();
            stale.sort();
            assert!(stale.is_empty(), "{}: keys that no longer exist {stale:?}", pack.code);
            for (key, en, _) in STRINGS {
                let text = l.get(key);
                assert!(!text.trim().is_empty(), "{}: {key} is blank", pack.code);
                assert_eq!(
                    text.matches("%s").count(),
                    en.matches("%s").count(),
                    "{}: {key} has a different number of %s than English",
                    pack.code
                );
            }
        }
        assert!(Locale::builtin("ru-RU").is_some(), "a regional code finds its base pack");
        assert!(Locale::builtin("xx").is_none());
    }
}
