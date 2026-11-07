//! Importing a classic installation.
//!
//! Point [`import_options`] and [`import_history`] at the `Data/` directory of
//! an existing classic install and the user keeps their languages, hotkeys,
//! service order, popup look, exceptions, proxy and translation history. The
//! classic files are only read, never written.

use serde_json::Value;

use crate::color::Color;
use crate::config::{
    Config, CopyAction, ExceptionMode, ExceptionRule, MouseBehaviour, PopupIcons, ProxyMode, ProxyScheme, StartupAction,
};
use crate::history::{History, HistoryEntry, HistoryResult};
use crate::hotkey::{Action, Hotkey};
use crate::jsonc;
use crate::lang::{LangId, LangPair};

/// The numeric ids classic service scripts declared in their header, and the
/// provider id each became here. Several community scripts reused a number;
/// the one found in installations is listed. Where Glossa has a keyless
/// service doing the same job (the bundled modules: `yandex-web`,
/// `deepl-web`, `lara`, the dictionaries), the classic id points at it, so an
/// imported installation gets working tabs without any setup. More than one
/// classic id may land on the same service.
pub static SERVICE_IDS: &[(u32, &str)] = &[
    (1, "google"),
    (5, "bing"),
    (10, "google-search"),
    // The classic Yandex script used the keyless web endpoint, as does the
    // bundled module; the built-in `yandex` needs a Yandex Cloud key.
    (11, "yandex-web"),
    // Scripts named "DeepL" circulated under 12 (in a folder named Promt), 31
    // and, as "DeepLX", 106; all of them were keyless.
    (12, "deepl-web"),
    (13, "babylon"),
    (14, "wikipedia"),
    (17, "multitran"),
    (18, "imtranslator"),
    (19, "wordreference"),
    (20, "babylon-dictionary"),
    (22, "reverso-context"),
    (24, "urban-dictionary"),
    (25, "lingvo-live"),
    (26, "youdao"),
    (29, "oxford-learners"),
    (30, "papago"),
    (31, "deepl-web"),
    (41, "modernmt"),
    (42, "amazon"),
    (82, "wiktionary"),
    (83, "dic-academic"),
    (84, "translate-academic"),
    (88, "chatgpt"),
    (91, "libretranslate"),
    (94, "translatero"),
    (99, "reverso"),
    (103, "openrouter"),
    (104, "lara"),
    (106, "deepl-web"),
    // A copy of the Lara script, in a folder named Baidu.
    (107, "lara"),
];

pub fn service_id(legacy: u32) -> String {
    SERVICE_IDS
        .iter()
        .find(|(n, _)| *n == legacy)
        .map(|(_, id)| (*id).to_owned())
        .unwrap_or_else(|| format!("legacy-{legacy}"))
}

/// Merge a classic `Options.json` into `config`. Settings the file does not
/// mention keep whatever `config` had. Returns notes on anything skipped.
pub fn import_options(text: &str, config: &mut Config) -> Result<Vec<String>, String> {
    let v: Value = serde_json::from_str(&jsonc::strip(text)).map_err(|e| e.to_string())?;
    let mut notes = Vec::new();

    let b = |path: &[&str]| get(&v, path).and_then(Value::as_bool);
    let n = |path: &[&str]| get(&v, path).and_then(Value::as_i64);
    let s = |path: &[&str]| get(&v, path).and_then(Value::as_str).map(str::to_owned);
    let lang = |path: &[&str]| n(path).map(|x| LangId(x.max(0) as u16));

    // -- General ----------------------------------------------------------
    let g = &mut config.general;
    if let Some(x) = lang(&["General", "LanguageFrom"]) {
        g.pair.from = x;
    }
    if let Some(x) = lang(&["General", "LanguageTo"]) {
        g.pair.to = x;
    }
    if let Some(x) = lang(&["AutoDetection", "LanguageFirst"]) {
        g.first_language = x;
    }
    if let Some(x) = lang(&["AutoDetection", "LanguageSecond"]) {
        g.second_language = x;
    }
    if let Some(x) = lang(&["AutoDetection", "LanguageSpeechRecognition"]) {
        g.speech_language = x;
    }
    macro_rules! flag {
        ($field:expr, $($path:literal),+) => {
            if let Some(x) = b(&[$($path),+]) { $field = x; }
        };
    }
    flag!(g.smart_direction, "General", "UseSmartDetection");
    flag!(g.always_detect_language, "General", "AlwaysDetectLanguage");
    flag!(g.instant_translation, "General", "InstantTranslation");
    flag!(g.spell_checking, "General", "SpellChecking");
    flag!(g.back_translation, "General", "BackTranslation");
    flag!(g.read_phonetically, "General", "ReadPhonetically");
    flag!(g.clear_on_drag_drop, "General", "ClearOnDragDrop");
    flag!(g.auto_cleanup_translation, "General", "AutoCleanupOfTranslation");
    flag!(g.show_top_pane, "General", "ShowTopPane");
    flag!(g.show_middle_pane, "General", "ShowMiddlePane");
    flag!(g.show_services_pane, "General", "ShowServicesPane");
    flag!(g.minimize_to_tray_on_minimize, "General", "MinimizeToTrayOnMinimize");
    flag!(g.minimize_to_tray_on_close, "General", "MinimizeToTrayOnClose");
    flag!(g.save_contents_on_exit, "Contents", "SaveOnExit");
    if let Some(x) = n(&["General", "MainWindowStartupAction"]) {
        g.startup_action = match x {
            0 => StartupAction::ShowWindow,
            1 => StartupAction::MinimizeToTray,
            _ => StartupAction::RestorePrevious,
        };
    }
    if let Some(pairs) = get(&v, &["LanguagePairs"]).and_then(Value::as_array) {
        let parsed: Vec<LangPair> = pairs
            .iter()
            .filter_map(|p| {
                let a = p.as_array()?;
                Some(LangPair::new(LangId(a.first()?.as_u64()? as u16), LangId(a.get(1)?.as_u64()? as u16)))
            })
            .collect();
        if !parsed.is_empty() {
            g.recent_pairs = parsed;
        }
    }
    if let Some(folder) = s(&["General", "LocaleFoderName"]) {
        // The classic key is misspelt; it names a locale folder, e.g.
        // "Russian". The UI resolves a folder name as well as a code.
        g.ui_language = folder;
    }
    if let Some(font) = s(&["General", "FontName"]).filter(|f| !f.is_empty()) {
        config.appearance.font_family = font;
    }
    if let Some(size) = n(&["General", "TextSize"]) {
        config.appearance.text_size = size as f32 + 2.0; // classic sizes ran small
    }

    // -- History ------------------------------------------------------------
    flag!(config.history.enabled, "General", "EnableHistory");
    flag!(config.history.clear_on_exit, "General", "ClearHistoryOnExit");
    flag!(config.history.expand_items, "General", "ExpandHistoryItems");

    // -- Hotkeys --------------------------------------------------------------
    flag!(config.hotkeys.enabled, "HotKeys", "EnableHotKeys");
    if get(&v, &["HotKeys"]).is_some() {
        for action in Action::CLASSIC {
            let key = action.legacy_option_key().expect("classic action");
            if let Some(raw) = n(&["HotKeys", key]) {
                match Hotkey::from_legacy(raw as u32) {
                    Some(hk) => {
                        config.hotkeys.bindings.insert(action, hk);
                    }
                    None => {
                        config.hotkeys.bindings.remove(&action);
                        if raw != 0 {
                            notes.push(format!("hotkey {key}={raw} uses a key we do not know"));
                        }
                    }
                }
            }
        }
    }

    // -- Internet -----------------------------------------------------------------
    if let Some(ms) = n(&["Internet", "Timeout"]) {
        config.network.timeout_secs = (ms as u64 / 1000).max(1);
    }
    if let Some(p) = get(&v, &["Proxy"]) {
        let proxy = &mut config.network.proxy;
        if let Some(t) = p.get("ProxyType").and_then(Value::as_i64) {
            proxy.mode = match t {
                0 => ProxyMode::System,
                1 => ProxyMode::None,
                2 => ProxyMode::Manual,
                _ => ProxyMode::Auto,
            };
        }
        if let Some(sch) = p.get("Scheme").and_then(Value::as_i64) {
            // The classic drop-down: HTTP, HTTPS, SOCKS4, SOCKS4A, SOCKS5,
            // SOCKS5 (hostname). SOCKS variants collapse to what we support.
            proxy.scheme = match sch {
                1 => ProxyScheme::Https,
                2 | 3 => ProxyScheme::Socks4,
                4 | 5 => ProxyScheme::Socks5,
                _ => ProxyScheme::Http,
            };
        }
        if let Some(h) = p.get("Host").and_then(Value::as_str) {
            proxy.host = h.to_owned();
        }
        if let Some(port) = p.get("Port").and_then(Value::as_u64) {
            proxy.port = port as u16;
        }
        if let Some(u) = p.get("Username").and_then(Value::as_str) {
            proxy.username = u.to_owned();
        }
        if let Some(pw) = p.get("Password").and_then(Value::as_str) {
            proxy.password = pw.to_owned();
        }
    }

    // -- Services ---------------------------------------------------------------------
    let numbers = |path: &[&str]| -> Option<Vec<u32>> {
        get(&v, path)?.as_array().map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as u32).collect())
    };
    let ids = |path: &[&str]| -> Option<Vec<String>> { numbers(path).map(|n| n.into_iter().map(service_id).collect()) };
    // An order and its disabled list → (order, enabled) in Glossa ids. Several
    // classic ids can map to one service: it keeps the first one's place, and
    // is on when any of them was.
    let lists = |order: Vec<u32>, disabled: &[u32]| -> (Vec<String>, Vec<String>) {
        let (mut ordered, mut enabled) = (Vec::<String>::new(), Vec::<String>::new());
        for n in order {
            let id = service_id(n);
            if !disabled.contains(&n) && !enabled.contains(&id) {
                enabled.push(id.clone());
            }
            if !ordered.contains(&id) {
                ordered.push(id);
            }
        }
        // Keep the enabled list in the order's order.
        enabled.sort_by_key(|id| ordered.iter().position(|o| o == id));
        (ordered, enabled)
    };
    if let Some(order) = numbers(&["ServicesOrder"]) {
        let (order, enabled) = lists(order, &numbers(&["DisabledServices"]).unwrap_or_default());
        config.services.enabled = enabled;
        // The classic settings already decided about these; do not switch
        // any of them back on as "new".
        config.services.seen.extend(order.iter().cloned());
        config.services.order = order;
    }
    if let Some(active) = ids(&["General", "ActiveServices"]).and_then(|a| a.into_iter().next()) {
        config.services.active = active;
    }
    if let Some(order) = numbers(&["DictionariesOrder"]) {
        let (order, enabled) = lists(order, &numbers(&["DisabledDictionaries"]).unwrap_or_default());
        config.services.dictionary_enabled = enabled;
        config.services.seen.extend(order.iter().cloned());
        config.services.dictionary_order = order;
    }
    if let Some(active) = ids(&["General", "ActiveDictionaryServices"]).and_then(|a| a.into_iter().next()) {
        config.services.dictionary_active = active;
    }
    if let Some(langs) = get(&v, &["DisabledLanguages"]).and_then(Value::as_array) {
        config.languages.disabled = langs.iter().filter_map(Value::as_u64).map(|x| LangId(x as u16)).collect();
    }

    // -- Appearance ---------------------------------------------------------------------
    if let Some(theme) = s(&["Appearance", "ThemeName"]) {
        config.appearance.theme = theme;
        // The classic program had one theme; keep it whatever the system does.
        config.appearance.follow_system_theme = false;
    }
    flag!(config.appearance.custom_frame, "Appearance", "EnableWindowStyle");
    let p = &mut config.appearance.popup;
    flag!(p.auto_size, "Appearance", "PopupAutoSize");
    flag!(p.auto_position, "Appearance", "PopupAutoPos");
    flag!(p.activate_on_open, "Appearance", "PopupAutoFocus");
    flag!(p.pin_when_dragging, "Appearance", "PopupPinWhenDragging");
    if let Some(t) = n(&["Appearance", "PopupTimeout"]) {
        p.auto_hide_secs = t.max(0) as u32;
    }
    if let Some(alpha) = n(&["Appearance", "Transparency"]) {
        // Stored as the window alpha (0-255); the slider showed percent
        // transparent.
        let alpha = alpha.clamp(0, 255) as f32;
        p.transparency = ((1.0 - alpha / 255.0) * 100.0).round().clamp(0.0, 90.0) as u8;
    }
    if let Some(t) = n(&["Appearance", "PopupWindowFrameThickness"]) {
        p.frame_thickness = t as f32;
    }
    if let Some(mask) = n(&["Appearance", "PopupIcons"]) {
        p.icons = PopupIcons::from_legacy(mask as u32);
    }
    let colour = |key: &str| n(&["Appearance", key]).map(|c| Color::from_colorref(c as u32));
    p.back = colour("ColorBack").or(p.back);
    p.text = colour("ColorText").or(p.text);
    p.frame = colour("ColorFrame").or(p.frame);

    // -- Exceptions --------------------------------------------------------------------------
    if let Some(ex) = get(&v, &["Exceptions"]) {
        let disabled_mode = ex.get("DisabledMode").and_then(Value::as_bool).unwrap_or(true);
        config.exceptions.mode = if disabled_mode { ExceptionMode::DisabledFor } else { ExceptionMode::EnabledFor };
        let list = if disabled_mode { "Disabled" } else { "Enabled" };
        if let Some(rules) = ex.get(list).and_then(Value::as_array) {
            config.exceptions.rules = rules
                .iter()
                .filter_map(|r| {
                    let a = r.as_array()?;
                    Some(ExceptionRule {
                        program: a.first()?.as_str()?.to_owned(),
                        window_class: a.get(1)?.as_str()?.to_owned(),
                    })
                })
                .collect();
        }
    }

    // -- Advanced -------------------------------------------------------------------------------
    let a = &mut config.advanced;
    flag!(a.slower_listen_repeat, "Advanced", "EnableSlowerListening");
    flag!(a.gui_translation, "Advanced", "EnableGuiTranslation");
    flag!(a.remove_line_breaks, "Advanced", "RemoveLineBreaks");
    if let Some(d) = s(&["Advanced", "PreferredDomain"]).filter(|d| !d.is_empty()) {
        a.preferred_domain = d;
    }
    if let Some(c) = n(&["Advanced", "CopyAction"]) {
        a.copy_action = if c == 1 { CopyAction::CtrlInsert } else { CopyAction::CtrlC };
    }
    if let Some(br) = s(&["Advanced", "DefaultBrowserId"]) {
        a.browser = br;
    }
    flag!(config.mouse_mode.ctrl_only, "Advanced", "EnableMouseModeOnCtrl");
    if let Some(on) = b(&["Advanced", "SwitchMouseModeOnTrayClick"]) {
        config.tray.adopt_tray_click_toggles(Some(on), crate::config::Os::current());
    }
    flag!(config.mouse_mode.enabled, "General", "MouseModeOn");
    if let Some(m) = n(&["General", "MouseMode"]) {
        config.mouse_mode.behaviour = match m {
            1 => MouseBehaviour::ShowTranslation,
            2 => MouseBehaviour::ShowTranslationAndRead,
            _ => MouseBehaviour::ShowIcon,
        };
    }

    // -- OCR, dictionary, updates, contents -------------------------------------------------------
    if let Some(x) = lang(&["Ocr", "OcrLanguage"]) {
        config.ocr.language = x;
    }
    if let Some(dir) = s(&["Ocr", "SaveImagePath"]).filter(|d| !d.is_empty()) {
        config.ocr.save_image_dir = Some(dir.into());
    }
    if s(&["Advanced", "OcrApiKey"]).is_some_and(|k| !k.is_empty()) {
        notes.push("the classic OCR API key (OCR.space) is not used; OCR goes through chrome-lens".into());
    }
    flag!(config.dictionary.show_services_pane, "Dictionary", "ShowServicesPane");
    flag!(config.dictionary.exact_search, "Dictionary", "DictionaryExactSearch");
    if let Some(z) = n(&["Dictionary", "DictionaryZoom"]) {
        config.dictionary.zoom = z.max(0) as u32;
    }
    if let Some(x) = lang(&["General", "LanguageDictionary"]) {
        config.dictionary.language = x;
    }
    flag!(config.updates.check_automatically, "Update", "CheckForUpdates");
    if let Some(src) = s(&["Contents", "EditSource"]) {
        config.contents.source = src;
    }
    if let Some(t) = s(&["Contents", "EditTranslation"]) {
        config.contents.translation = t;
    }
    if let Some(t) = s(&["Contents", "EditBackTranslation"]) {
        config.contents.back_translation = t;
    }

    config.sanitise();
    Ok(notes)
}

/// Read a classic `History.json`: an array, newest first, of
/// `[source, [[service, from, to, translation], ...], favourite]`.
pub fn import_history(text: &str, history: &mut History, now: i64) -> Result<usize, String> {
    let v: Value = serde_json::from_str(&jsonc::strip(text)).map_err(|e| e.to_string())?;
    let items = v.as_array().ok_or("history file is not an array")?;
    let mut imported = 0;
    for item in items {
        let Some(arr) = item.as_array() else { continue };
        let Some(source) = arr.first().and_then(Value::as_str) else { continue };
        let results: Vec<HistoryResult> = arr
            .get(1)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let r = r.as_array()?;
                Some(HistoryResult {
                    provider_id: service_id(r.first()?.as_u64()? as u32),
                    from: LangId(r.get(1)?.as_u64()? as u16),
                    to: LangId(r.get(2)?.as_u64()? as u16),
                    text: r.get(3)?.as_str()?.replace("\r\n", "\n"),
                })
            })
            .collect();
        if results.is_empty() {
            continue;
        }
        let favorite = arr.get(2).and_then(Value::as_bool).unwrap_or(false);
        history.push_oldest(HistoryEntry {
            id: 0,
            source: source.replace("\r\n", "\n"),
            results,
            favorite,
            created: now,
        });
        imported += 1;
    }
    Ok(imported)
}

fn get<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for key in path {
        cur = cur.get(*key)?;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real classic `Options.json`.
    const OPTIONS: &str = r#"{
       "Exceptions": {
          "Disabled": [["", "SysListView32"], ["mstsc.exe", ""]],
          "Enabled": [], "DisabledMode": true
       },
       "Contents": { "SaveOnExit": true, "EditSource": "Do you know any YOLO module?", "EditTranslation": "Знаете ли вы?" },
       "Advanced": { "EnableSlowerListening": true, "CopyAction": 0, "RemoveLineBreaks": false, "PreferredDomain": "com", "OcrApiKey": "" },
       "Appearance": {
          "EnableWindowStyle": true, "PopupAutoSize": true, "ColorFrame": 8023133, "PopupPinWhenDragging": true,
          "PopupWindowFrameThickness": 2, "Transparency": 217, "PopupIcons": 30, "ColorText": 0,
          "PopupAutoFocus": false, "ThemeName": "Flat Dark", "PopupAutoPos": true, "PopupTimeout": 5, "ColorBack": 15790320
       },
       "Internet": { "Timeout": 10000 },
       "HotKeys": { "EnableHotKeys": true, "HotKeyListenText": 581, "HotKeyPopupWindow": 1617, "HotKeyMainWindow": 0 },
       "Ocr": { "OcrLanguage": 17, "SaveImagePath": "" },
       "DisabledServices": [12, 13, 91],
       "Proxy": { "Password": "", "Scheme": 5, "Host": "127.0.0.1", "ProxyType": 2, "Username": "", "Port": 2080 },
       "DictionariesOrder": [10, 19, 17],
       "Dictionary": { "ShowServicesPane": true, "DictionaryZoom": 100, "DictionaryExactSearch": true },
       "LanguagePairs": [[46, 17], [17, 46]],
       "General": {
          "InstantTranslation": false, "LanguageDictionary": 46, "UseSmartDetection": true,
          "ActiveServices": [104], "LanguageTo": 1, "LanguageFrom": 1, "LocaleFoderName": "Russian",
          "MinimizeToTrayOnClose": true, "TextSize": 9, "MainWindowStartupAction": 0, "MouseModeOn": false
       },
       "ServicesOrder": [1, 5, 12, 13, 11, 104, 88, 91],
       "AutoDetection": { "LanguageFirst": 46, "LanguageSecond": 17, "LanguageSpeechRecognition": 46 }
    }"#;

    #[test]
    fn a_real_options_file_imports() {
        let mut c = Config::default();
        let notes = import_options(OPTIONS, &mut c).unwrap();
        assert!(notes.is_empty(), "{notes:?}");

        assert_eq!(c.general.first_language, LangId::RUSSIAN);
        assert_eq!(c.general.second_language, LangId::ENGLISH);
        assert_eq!(c.general.pair, LangPair::new(LangId::AUTO, LangId::AUTO));
        assert_eq!(c.general.recent_pairs[0], LangPair::new(LangId::RUSSIAN, LangId::ENGLISH));
        assert_eq!(c.general.ui_language, "Russian");
        assert_eq!(c.general.startup_action, StartupAction::ShowWindow);

        assert_eq!(c.hotkeys.bindings[&Action::ShowPopup].to_string(), "Ctrl+Shift+Q");
        assert_eq!(c.hotkeys.bindings[&Action::ListenSelection].to_string(), "Ctrl+E");
        assert!(!c.hotkeys.bindings.contains_key(&Action::ShowMainWindow), "0 clears a binding");

        assert_eq!(c.network.timeout_secs, 10);
        assert_eq!(c.network.proxy.mode, ProxyMode::Manual);
        assert_eq!(c.network.proxy.scheme, ProxyScheme::Socks5);
        assert_eq!(c.network.proxy.url().as_deref(), Some("socks5://127.0.0.1:2080"));

        assert_eq!(&c.services.order[..5], ["google", "bing", "deepl-web", "babylon", "yandex-web"]);
        assert!(!c.services.enabled.contains(&"deepl-web".to_string()), "disabled services are off");
        assert_eq!(c.services.enabled, ["google", "bing", "yandex-web", "lara", "chatgpt"]);
        assert_eq!(c.services.active, "lara");
        assert_eq!(c.services.dictionary_order, ["google-search", "wordreference", "multitran"]);

        assert_eq!(c.appearance.theme, "Flat Dark");
        assert_eq!(c.appearance.popup.transparency, 15, "alpha 217 is 15% transparent");
        assert_eq!(c.appearance.popup.frame, Some(Color::rgb(0x5d, 0x6c, 0x7a)));
        assert_eq!(c.appearance.popup.back, Some(Color::grey(0xf0)));
        assert_eq!(c.appearance.popup.text, Some(Color::BLACK));
        assert_eq!(c.appearance.popup.icons, PopupIcons::default());

        assert_eq!(c.exceptions.rules.len(), 2);
        assert!(!c.exceptions.allows("mstsc.exe", "x"));
        assert_eq!(c.ocr.language, LangId::ENGLISH);
        assert_eq!(c.contents.source, "Do you know any YOLO module?");
    }

    #[test]
    fn a_history_file_imports_newest_first() {
        let text = "\u{feff}[[\"Привет\",[[11,46,17,\"Hello\"],[1,46,17,\"Hi\"]],true],[\"oh\\r\\nok\",[[104,17,46,\"о\\nок\"]],false],[\"empty\",[],false]]";
        let mut h = History::new(100);
        let n = import_history(text, &mut h, 0).unwrap();
        assert_eq!(n, 2, "entries without results are skipped");
        let entries: Vec<_> = h.iter().collect();
        assert_eq!(entries[0].source, "Привет");
        assert!(entries[0].favorite);
        assert_eq!(entries[0].results[0].provider_id, "yandex-web");
        assert_eq!(entries[0].results[1].provider_id, "google");
        assert_eq!(entries[1].source, "oh\nok", "CRLF normalised");
        assert_eq!(entries[1].results[0].provider_id, "lara");
    }

    #[test]
    fn unknown_service_numbers_keep_their_number() {
        assert_eq!(service_id(1), "google");
        assert_eq!(service_id(9999), "legacy-9999");
    }

    #[test]
    fn classic_services_land_on_the_keyless_glossa_ones() {
        assert_eq!(service_id(11), "yandex-web", "not the built-in that needs a Yandex Cloud key");
        for n in [12, 31, 106] {
            assert_eq!(service_id(n), "deepl-web", "{n}");
        }
        assert_eq!(service_id(107), "lara");
        assert_eq!(service_id(25), "lingvo-live");
        let mut ids: Vec<u32> = SERVICE_IDS.iter().map(|(n, _)| *n).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SERVICE_IDS.len(), "each classic number is listed once");
    }

    #[test]
    fn classic_ids_sharing_a_service_merge() {
        // Three DeepL scripts: the first one off, the other two on.
        let mut c = Config::default();
        import_options(
            r#"{ "ServicesOrder": [1, 12, 31, 106, 11], "DisabledServices": [12],
                 "DictionariesOrder": [17, 26], "DisabledDictionaries": [] }"#,
            &mut c,
        )
        .unwrap();
        assert_eq!(c.services.order, ["google", "deepl-web", "yandex-web"], "one entry, at the first one's place");
        assert_eq!(c.services.enabled, ["google", "deepl-web", "yandex-web"], "on because another one was");
        assert_eq!(c.services.dictionary_enabled, ["multitran", "youdao"]);

        let mut c = Config::default();
        import_options(r#"{ "ServicesOrder": [12, 31, 1], "DisabledServices": [12, 31] }"#, &mut c).unwrap();
        assert_eq!(c.services.enabled, ["google"], "off when every one of them was");
    }
}
