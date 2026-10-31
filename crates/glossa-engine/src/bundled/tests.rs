//! The bundled modules against canned answers. The fixtures are written by
//! hand in the shape each service answers with (see the comment at the top of
//! each module's `main.js`), cut down to what the module reads.

use std::sync::Arc;

use glossa_core::{Article, Capabilities, DictRequest, Error, LangId, LangPair, TranslateRequest, Translation};
use glossa_modules::host_api::ModuleCache;
use glossa_modules::provider::with_test_host;
use glossa_modules::{Manifest, OptionValues, Provider};
use glossa_net::mock::MockHttp;
use glossa_net::Method;

use super::*;

fn provider(id: &str) -> Arc<dyn Provider> {
    let b = BUNDLED.iter().find(|b| b.id == id).unwrap_or_else(|| panic!("no bundled module {id}"));
    glossa_modules::runtime::js::load_embedded(b.manifest, b.script, id, ModuleCache::new()).unwrap()
}

fn lang(code: &str) -> LangId {
    LangId::from_code(code).unwrap()
}

fn translate(id: &str, http: &MockHttp, text: &str, from: &str, to: &str) -> glossa_core::Result<Translation> {
    translate_with(&provider(id), http, text, from, to)
}

fn translate_with(
    p: &Arc<dyn Provider>,
    http: &MockHttp,
    text: &str,
    from: &str,
    to: &str,
) -> glossa_core::Result<Translation> {
    let opts = OptionValues::new();
    with_test_host(http, &opts, |host| {
        p.translate(host, &TranslateRequest::new(text, LangPair::new(lang(from), lang(to))))
    })
}

fn look_up(id: &str, http: &MockHttp, word: &str, from: &str, to: &str) -> glossa_core::Result<Article> {
    look_up_with(id, http, word, from, to, OptionValues::new())
}

fn look_up_with(
    id: &str,
    http: &MockHttp,
    word: &str,
    from: &str,
    to: &str,
    opts: OptionValues,
) -> glossa_core::Result<Article> {
    let p = provider(id);
    let req = DictRequest { text: word.into(), from: lang(from), to: lang(to), exact: false };
    with_test_host(http, &opts, |host| p.dictionary(host, &req))
}

fn body(req: &glossa_net::HttpRequest) -> String {
    String::from_utf8(req.body.clone().unwrap_or_default()).unwrap()
}

fn header<'a>(req: &'a glossa_net::HttpRequest, name: &str) -> Option<&'a str> {
    req.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

// -- every module -------------------------------------------------------------

#[test]
fn every_bundled_module_loads_with_a_sane_manifest() {
    let builtin_ids: Vec<String> = glossa_services::builtin_providers().iter().map(|p| p.info().id.clone()).collect();
    let mut ids = Vec::new();
    for b in BUNDLED {
        let m = Manifest::parse(b.manifest).unwrap_or_else(|e| panic!("{}: {e}", b.id));
        assert_eq!(m.module.id, b.id, "the folder is named after the id");
        assert_eq!(m.module.runtime, glossa_modules::Runtime::Js, "{}", b.id);
        assert_eq!(m.module.entry, "main.js", "{}", b.id);
        glossa_modules::runtime::js::engine::check_syntax(b.script, &format!("{}/main.js", b.id))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!m.module.name.is_empty() && !m.module.description.is_empty(), "{}", b.id);
        assert!(m.module.homepage.starts_with("https://"), "{}", b.id);
        assert!(m.module.max_input_chars.is_some(), "{}: says how much text it takes", b.id);
        // Everything bundled works without setup; that is why it is bundled.
        assert!(m.module.ready_to_use && m.options.iter().all(|o| !o.required), "{}", b.id);
        // Network access is limited to the service's own hosts.
        assert!(!m.permissions.network.is_empty(), "{}", b.id);
        assert!(m.permissions.network.iter().all(|h| !h.contains('*')), "{}", b.id);
        for code in m.languages.iter().chain(m.language_map.keys()) {
            assert!(LangId::from_code(code).is_some_and(|l| l.is_language()), "{}: unknown language {code}", b.id);
        }
        let info = m.provider_info();
        assert!(info.ready_to_use && !info.needs_credentials, "{}", b.id);
        assert!(!builtin_ids.contains(&info.id), "{} would hide a Rust built-in", b.id);
        // No hard-coded secrets: nothing that looks like a key or token.
        let lower = b.script.to_ascii_lowercase();
        for word in ["api_key", "apikey", "secret", "bearer "] {
            assert!(!lower.contains(word), "{}: mentions {word}", b.id);
        }
        ids.push(info.id);
    }
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(n, ids.len(), "ids are unique");
    assert_eq!(providers(&ModuleCache::new()).len(), BUNDLED.len(), "all of them load");
}

// -- Yandex -------------------------------------------------------------------

#[test]
fn yandex_translates_with_a_detected_source_and_a_dictionary_entry() {
    let http = MockHttp::new()
        .on(
            Method::Post,
            "https://translate.yandex.net/api/v1/tr.json/translate",
            200,
            r#"{"code":200,"lang":"en-ru","text":["дом"]}"#,
        )
        .on(
            Method::Get,
            "https://translate.yandex.net/dicservice.json/lookup",
            200,
            r#"{"head":{},"def":[{"text":"house","pos":"noun","ts":"haʊs","tr":[
                {"text":"дом","pos":"noun","mean":[{"text":"home"},{"text":"building"}]},
                {"text":"здание","pos":"noun"}]}]}"#,
        );
    let t = translate("yandex-web", &http, "house", "auto", "ru").unwrap();
    assert_eq!(t.text, "дом");
    assert_eq!(t.detected_from, Some(LangId::ENGLISH));
    assert_eq!(t.parts_of_speech.len(), 1);
    assert_eq!(t.parts_of_speech[0].part_of_speech, "noun");
    assert_eq!(t.parts_of_speech[0].senses[0].word, "дом");
    assert_eq!(t.parts_of_speech[0].senses[0].back_translations, ["home", "building"]);

    let reqs = http.requests();
    assert!(reqs[0].url.contains("srv=browser_video_translation"), "{}", reqs[0].url);
    assert!(reqs[0].url.ends_with("&lang=ru"), "an unknown source is left to Yandex: {}", reqs[0].url);
    assert_eq!(body(&reqs[0]), "text=house");
    assert!(reqs[1].url.contains("lang=en-ru") && reqs[1].url.contains("text=house"), "{}", reqs[1].url);
}

#[test]
fn yandex_cuts_long_text_and_keeps_the_gaps() {
    let http = MockHttp::new().on(
        Method::Post,
        "https://translate.yandex.net/api/v1/tr.json/translate",
        200,
        r#"{"code":200,"lang":"en-ru","text":["X"]}"#,
    );
    // A 2800-character paragraph, a blank line, a 5600-character one: cut at
    // the paragraph break first, then at a sentence end inside the second.
    let short = "Lorem ipsum dolor sit amet. ".repeat(100);
    let long = "Lorem ipsum dolor sit amet. ".repeat(200);
    let text = format!("{}\n\n{}", short.trim_end(), long.trim_end());
    let t = translate("yandex-web", &http, &text, "en", "ru").unwrap();
    let reqs = http.requests();
    assert_eq!(reqs.len(), 3, "sent in three pieces");
    assert!(reqs.iter().all(|r| body(r).len() <= 3 * 4000 + 5), "each piece fits");
    assert!(reqs.iter().all(|r| !body(r).contains("%0A")), "no piece carries the break");
    assert!(reqs[0].url.contains("lang=en-ru"));
    assert_eq!(t.text, "X\n\nX X", "the paragraph break and the space at the cuts are kept");
    assert!(t.parts_of_speech.is_empty(), "no dictionary lookup for a long text");
}

#[test]
fn yandex_reports_unsupported_directions_and_bad_answers() {
    let http = MockHttp::new().on(
        Method::Post,
        "https://translate.yandex.net/api/v1/tr.json/translate",
        400,
        r#"{"code":501,"message":"The specified translation direction is not supported"}"#,
    );
    let e = translate("yandex-web", &http, "a long sentence here", "en", "ru").unwrap_err();
    assert!(matches!(e, Error::TargetLanguageUnsupported { .. }), "{e:?}");

    let http = MockHttp::new().on(
        Method::Post,
        "https://translate.yandex.net/api/v1/tr.json/translate",
        200,
        "<html>captcha</html>",
    );
    let e = translate("yandex-web", &http, "a long sentence here", "en", "ru").unwrap_err();
    assert!(matches!(e, Error::BadResponse { .. }), "{e:?}");
}

#[test]
fn yandex_detects_and_looks_up() {
    let http = MockHttp::new()
        .on(Method::Get, "https://translate.yandex.net/api/v1/tr.json/detect", 200, r#"{"code":200,"lang":"de"}"#)
        .on(
            Method::Get,
            "https://translate.yandex.net/dicservice.json/lookup",
            200,
            r#"{"def":[{"text":"Haus","pos":"noun","ts":"haʊ̯s","tr":[
                {"text":"дом","pos":"noun","syn":[{"text":"здание"}],"mean":[{"text":"Gebäude"}],
                 "ex":[{"text":"ein großes Haus","tr":[{"text":"большой дом"}]}]}]}]}"#,
        );
    let p = provider("yandex-web");
    let opts = OptionValues::new();
    let d = with_test_host(&http, &opts, |host| p.detect(host, "Guten Morgen")).unwrap();
    assert_eq!(d.lang, lang("de"));

    let a = look_up("yandex-web", &http, "Haus", "auto", "ru").unwrap();
    assert_eq!(a.headword, "Haus");
    assert_eq!(a.pronunciations[0].symbol, "haʊ̯s");
    assert_eq!(a.explanations[0].part_of_speech, "noun");
    assert_eq!(a.explanations[0].definitions, ["дом, здание (Gebäude)"]);
    assert_eq!(a.examples[0].source, "ein großes Haus");
    assert_eq!(a.examples[0].target, "большой дом");
    let lookup = http.requests().into_iter().find(|r| r.url.contains("dicservice")).unwrap();
    assert!(lookup.url.contains("lang=de-ru"), "the detected language is used: {}", lookup.url);

    let empty =
        MockHttp::new().on(Method::Get, "https://translate.yandex.net/dicservice.json/lookup", 200, r#"{"def":[]}"#);
    let e = look_up("yandex-web", &empty, "qwzxv", "en", "ru").unwrap_err();
    assert!(e.to_string().contains("qwzxv"), "{e}");
}

// -- DeepL --------------------------------------------------------------------

#[test]
fn deepl_sends_the_app_request_and_reads_the_detected_language() {
    let http = MockHttp::new().on(
        Method::Post,
        "https://oneshot-free.www.deepl.com/v1/translate",
        200,
        r#"{"translations":[{"detected_source_language":"EN","text":"Привет, мир"}]}"#,
    );
    let deepl = provider("deepl-web");
    let t = translate_with(&deepl, &http, "Hello, world", "auto", "ru").unwrap();
    assert_eq!(t.text, "Привет, мир");
    assert_eq!(t.detected_from, Some(LangId::ENGLISH));
    let req = &http.requests()[0];
    let sent: serde_json::Value = serde_json::from_str(&body(req)).unwrap();
    assert_eq!(sent["text"][0], "Hello, world");
    assert_eq!(sent["target_lang"], "ru");
    assert!(sent.get("source_lang").is_none(), "auto leaves the source out");
    assert!(header(req, "User-Agent").is_some_and(|ua| ua.starts_with("DeepL/")));

    translate_with(&deepl, &http, "你好", "zh-Hans", "en").unwrap();
    let sent: serde_json::Value = serde_json::from_str(&body(&http.requests()[1])).unwrap();
    assert_eq!(sent["target_lang"], "en-US", "English needs a variant as a target");
    assert_eq!(sent["source_lang"], "zh", "a source has no variant");
    let a: serde_json::Value = serde_json::from_str(&body(&http.requests()[0])).unwrap();
    assert_eq!(
        a["app_information"]["instance_id"], sent["app_information"]["instance_id"],
        "one instance id per run, kept in the cache"
    );
}

#[test]
fn deepl_rate_limits_are_typed() {
    let http = MockHttp::new().on(
        Method::Post,
        "https://oneshot-free.www.deepl.com/v1/translate",
        429,
        r#"{ "code": 429, "message": "Too many requests. Please slow down and try again later." }"#,
    );
    let e = translate("deepl-web", &http, "Hello", "en", "ru").unwrap_err();
    assert!(matches!(e, Error::RateLimited { .. }), "{e:?}");
}

// -- Lara ---------------------------------------------------------------------

#[test]
fn lara_translates_and_names_unsupported_languages() {
    let http = MockHttp::new().on(
        Method::Post,
        "https://webapi.laratranslate.com/translate",
        200,
        r#"{"status":200,"content":{"source_language":"en","translation":"Привет, мир"}}"#,
    );
    let t = translate("lara", &http, "Hello, world", "auto", "ru").unwrap();
    assert_eq!(t.text, "Привет, мир");
    assert_eq!(t.detected_from, Some(LangId::ENGLISH));
    let req = &http.requests()[0];
    let sent: serde_json::Value = serde_json::from_str(&body(req)).unwrap();
    assert_eq!(sent, serde_json::json!({ "q": "Hello, world", "source": "", "target": "ru", "instructions": [] }));
    assert_eq!(header(req, "Origin"), Some("https://laratranslate.com"));

    let http = MockHttp::new().on(
        Method::Post,
        "https://webapi.laratranslate.com/translate",
        400,
        r#"{"status":400,"error":{"type":"UnsupportedLanguageError","message":"Unsupported language: tt"}}"#,
    );
    let e = translate("lara", &http, "Hello", "en", "tt").unwrap_err();
    assert!(matches!(e, Error::TargetLanguageUnsupported { .. }), "{e:?}");
}

// -- Multitran ----------------------------------------------------------------

const MULTITRAN_PAGE: &str = r#"<html><body><table width="100%">
<tr><td colspan="2" class="orig11"><table width="100%"><tr><td valign='top'>
<a name="noun"></a>
<h1><a href="/m.exe?s=lamp&amp;l1=1&amp;l2=2">lamp</a></h1> <span style="color:gray">[læmp]</span> <em><span style="color:gray">n</span></em> <audio id="a"></audio></td><td>stresses</td></tr></table></td></tr>
<tr><td class="subj" width="1%"></td>
<td class="trans1" width="100%">&#8658; <a href="/m.exe?s=table+lamp">table lamp</a>; <a href="/m.exe?s=lava+lamp">lava lamp</a></td></tr>
<tr><td class="subj" width="1%"><a href="/m.exe?a=110&amp;sc=0" title="General"><nobr>gen.</nobr></a></td>
<td class="trans1" width="100%"><a href="/m.exe?s=x">лампа</a> <span style="color:gray">(a device for giving light)</span>; <a href="/m.exe?s=y">светильник</a></td></tr>
<tr><td class="subj" width="1%"><a href="/m.exe?a=110&amp;sc=1" title="Mining"><nobr>mining.</nobr></a></td>
<td class="trans1" width="100%"><a href="/m.exe?s=z">шахтёрская лампа</a></td></tr>
<tr><td colspan="2" class="orig11"><table width="100%"><tr><td valign='top'>
<a name="verb"></a>
<h1><a href="/m.exe?s=lamp">lamp</a></h1> <em><span style="color:gray">v</span></em></td></tr></table></td></tr>
<tr><td class="subj" width="1%"><a href="/m.exe?sc=2" title="Slang"><nobr>sl.</nobr></a></td>
<td class="trans1" width="100%"><a href="/m.exe?s=w">отдыхать</a></td></tr>
</table></body></html>"#;

#[test]
fn multitran_reads_parts_of_speech_and_subjects() {
    let http = MockHttp::new().on(Method::Get, "https://www.multitran.com/m.exe", 200, MULTITRAN_PAGE);
    let a = look_up("multitran", &http, "lamp", "en", "ru").unwrap();
    assert_eq!(a.headword, "lamp");
    assert_eq!(a.pronunciations[0].symbol, "læmp");
    assert_eq!(a.explanations.len(), 2);
    assert_eq!(a.explanations[0].part_of_speech, "lamp n");
    assert_eq!(
        a.explanations[0].definitions,
        ["General: лампа (a device for giving light); светильник", "Mining: шахтёрская лампа"]
    );
    assert_eq!(a.explanations[1].part_of_speech, "lamp v");
    assert_eq!(a.explanations[1].definitions, ["Slang: отдыхать"]);
    assert_eq!(a.associations, ["table lamp", "lava lamp"]);
    let url = &http.requests()[0].url;
    assert!(url.contains("l1=1&l2=2&s=lamp&SHL=1"), "languages go out as Multitran's numbers: {url}");
}

#[test]
fn multitran_guesses_the_pair_and_reports_unknown_words() {
    let http = MockHttp::new().on(Method::Get, "https://www.multitran.com/m.exe", 200, "<html>nothing here</html>");
    let mut opts = OptionValues::new();
    opts.insert("labels".into(), serde_json::json!("ru"));
    let e = look_up_with("multitran", &http, "дом", "auto", "ru", opts).unwrap_err();
    assert!(e.to_string().contains("дом"), "{e}");
    let url = &http.requests()[0].url;
    assert!(url.contains("l1=2&l2=1") && url.ends_with("SHL=2"), "Cyrillic is Russian, looked up into English: {url}");
}

// -- ABBYY Lingvo Live --------------------------------------------------------

#[test]
fn lingvo_groups_cards_by_dictionary() {
    let http = MockHttp::new().on(
        Method::Get,
        "https://api.lingvolive.com/Translation/tutor-cards",
        200,
        r#"[{"dictionaryName":"Universal (En-Ru)","heading":"lamp","transcription":"læmp","translations":"лампа; светильник",
             "examples":"table lamp — настольная лампа \r\nstreet lamp — фонарь"},
            {"dictionaryName":"Universal (En-Ru)","heading":"lamp","transcription":"læmp","translations":"светоч","examples":""},
            {"dictionaryName":"Engineering (En-Ru)","heading":"lamp","transcription":"","translations":"осветительный прибор","examples":""}]"#,
    );
    let a = look_up("lingvo-live", &http, "lamp", "en", "ru").unwrap();
    assert_eq!(a.headword, "lamp");
    assert_eq!(a.pronunciations.len(), 1);
    assert_eq!(a.explanations.len(), 2);
    assert_eq!(a.explanations[0].part_of_speech, "Universal (En-Ru)");
    assert_eq!(a.explanations[0].definitions, ["лампа; светильник", "светоч"]);
    assert_eq!(a.examples.len(), 2);
    assert_eq!(a.examples[1].source, "street lamp");
    assert_eq!(a.examples[1].target, "фонарь");
    let url = &http.requests()[0].url;
    assert!(url.ends_with("text=lamp&srcLang=1033&dstLang=1049"), "{url}");

    let none = MockHttp::new().on(Method::Get, "https://api.lingvolive.com/", 200, "null");
    assert!(look_up("lingvo-live", &none, "qwzxv", "en", "ru").unwrap_err().to_string().contains("qwzxv"));
    let broken =
        MockHttp::new().on(Method::Get, "https://api.lingvolive.com/", 500, r#"{"message":"An error has occurred."}"#);
    let e = look_up("lingvo-live", &broken, "lamp", "fr", "es").unwrap_err();
    assert!(matches!(e, Error::SourceLanguageUnsupported { .. }), "{e:?}");
}

// -- Oxford -------------------------------------------------------------------

const OXFORD_ENTRY: &str = r#"<html><body><div id="entryContent" class="oald"><div class="entry">
<h1 class="headword" id="lamp_h_1">lamp</h1> <span class="pos" hclass="pos">noun</span>
<span class="phonetics"><div class="phons_br" geo="br"><div class="sound" data-src-mp3="https://www.oxfordlearnersdictionaries.com/media/uk/lamp.mp3"></div><span class="phon">/læmp/</span></div>
<div class="phons_n_am" geo="n_am"><div class="sound" data-src-mp3="https://www.oxfordlearnersdictionaries.com/media/us/lamp.mp3"></div><span class="phon">/læmp/</span></div></span>
<ol class="senses_multiple">
<li class="sense" id="lamp_sng_1"><span class="def" hclass="def">a device that uses electricity, oil or gas to <span class="dh">produce</span> light</span>
<ul class="examples"><li><span class="x">to switch on a lamp</span></li><li><span class="x">a table lamp</span></li><li><span class="x">a third example</span></li></ul></li>
<li class="sense" id="lamp_sng_2"><span class="def">an electrical device that produces rays of heat</span></li>
</ol>
<div class="idioms"><li class="sense"><span class="def">an idiom that is not wanted</span></li></div>
</div></div>
<div id="relatedentries"><dl><dd><ul>
<li><a href="https://www.oxfordlearnersdictionaries.com/definition/english/lamp_2" title="lamp verb"><span class='arl1'>lamp <pos-g><pos>verb</pos></pos-g></span></a></li>
<li><a href="https://www.oxfordlearnersdictionaries.com/definition/english/lamplight" title="lamplight"><span class='arl2'>lamplight <pos-g><pos>noun</pos></pos-g></span></a></li>
</ul></dd></dl></div>
<div id="rightcolumn"></div></body></html>"#;

const OXFORD_VERB: &str = r#"<div id="entryContent"><h1 class="headword">lamp</h1> <span class="pos">verb</span>
<li class="sense"><span class="def">to hit somebody hard</span><span class="x">He lamped him.</span></li></div>
<div id="relatedentries"></div>"#;

#[test]
fn oxford_reads_an_entry_and_the_same_word_in_another_part_of_speech() {
    let http = MockHttp::new()
        .on(Method::Get, "https://www.oxfordlearnersdictionaries.com/search/english/?q=lamp", 200, OXFORD_ENTRY)
        .on(Method::Get, "https://www.oxfordlearnersdictionaries.com/definition/english/lamp_2", 200, OXFORD_VERB);
    let a = look_up("oxford-learners", &http, "lamp", "en", "ru").unwrap();
    assert_eq!(a.headword, "lamp");
    assert_eq!(a.pronunciations.len(), 2);
    assert_eq!(a.pronunciations[0].region, "UK");
    assert_eq!(a.pronunciations[0].symbol, "/læmp/");
    assert_eq!(
        a.pronunciations[1].audio_url.as_deref(),
        Some("https://www.oxfordlearnersdictionaries.com/media/us/lamp.mp3")
    );
    assert_eq!(a.explanations.len(), 2);
    assert_eq!(a.explanations[0].part_of_speech, "noun");
    assert_eq!(
        a.explanations[0].definitions,
        [
            "a device that uses electricity, oil or gas to produce light",
            "an electrical device that produces rays of heat"
        ],
        "nested spans are read whole, idioms are left out"
    );
    assert_eq!(a.explanations[1].part_of_speech, "verb");
    assert_eq!(a.explanations[1].definitions, ["to hit somebody hard"]);
    let sources: Vec<&str> = a.examples.iter().map(|e| e.source.as_str()).collect();
    assert_eq!(sources, ["to switch on a lamp", "a table lamp", "He lamped him."], "two examples per sense by default");
    assert_eq!(a.associations, ["lamplight"]);
}

#[test]
fn oxford_spelling_suggestions_mean_not_found() {
    let http = MockHttp::new().on(
        Method::Get,
        "https://www.oxfordlearnersdictionaries.com/",
        200,
        "<html><div id=\"spellcheck\">Did you mean: <a>lamb</a></div></html>",
    );
    let e = look_up("oxford-learners", &http, "lammp", "en", "ru").unwrap_err();
    assert!(e.to_string().contains("lammp"), "{e}");
}

// -- Wiktionary ---------------------------------------------------------------

#[test]
fn wiktionary_shows_the_source_language_section() {
    let http = MockHttp::new()
        .on(Method::Get, "https://en.wiktionary.org/api/rest_v1/page/definition/Lamp", 404, r#"{"status":404}"#)
        .on(
            Method::Get,
            "https://en.wiktionary.org/api/rest_v1/page/definition/lamp",
            200,
            r#"{"en":[{"partOfSpeech":"Noun","language":"English","definitions":[
                    {"definition":""},
                    {"definition":"A device producing <a href=\"/wiki/light\">light</a>.<style>.x{}</style><ol><li>A sub-sense</li></ol>",
                     "examples":["Turn on the <b>lamp</b>."]}]}],
                "de":[{"partOfSpeech":"Noun","language":"German","definitions":[{"definition":"lamb (in some dialects)"}]}]}"#,
        );
    let a = look_up("wiktionary", &http, "Lamp", "en", "ru").unwrap();
    assert_eq!(a.headword, "lamp", "the lower-case page answered");
    assert_eq!(a.explanations.len(), 1, "only the English section");
    assert_eq!(a.explanations[0].part_of_speech, "noun");
    assert_eq!(a.explanations[0].definitions, ["A device producing light."]);
    assert_eq!(a.examples[0].source, "Turn on the lamp.");
    assert_eq!(a.link.as_deref(), Some("https://en.wiktionary.org/wiki/lamp"));
    assert!(header(&http.requests()[0], "Api-User-Agent").is_some(), "Wikimedia asks clients to identify themselves");

    // An unknown source shows every language, labelled.
    let a = look_up("wiktionary", &http, "lamp", "auto", "ru").unwrap();
    let labels: Vec<&str> = a.explanations.iter().map(|e| e.part_of_speech.as_str()).collect();
    assert_eq!(labels, ["English, noun", "German, noun"]);

    let missing = MockHttp::new().on(Method::Get, "https://en.wiktionary.org/api/rest_v1/page/definition/", 404, "{}");
    assert!(look_up("wiktionary", &missing, "qwzxv", "en", "ru").unwrap_err().to_string().contains("qwzxv"));
}

// -- Youdao -------------------------------------------------------------------

#[test]
fn youdao_reads_english_and_chinese_entries() {
    let http = MockHttp::new()
        .on(
            Method::Get,
            "https://dict.youdao.com/jsonapi?q=lamp",
            200,
            r#"{"ec":{"word":[{"ukphone":"læmp","ukspeech":"lamp&type=1","usphone":"læmp","usspeech":"lamp&type=2",
                 "trs":[{"tr":[{"l":{"i":["n. 灯；台灯"]}}]},{"tr":[{"l":{"i":["v. 照亮"]}}]}],
                 "wfs":[{"wf":{"name":"复数","value":"lamps"}}],
                 "return-phrase":{"l":{"i":"lamp"}}}]},
               "blng_sents_part":{"sentence-pair":[{"sentence":"Turn on the <b>lamp</b>.","sentence-translation":"开灯。"}]}}"#,
        )
        .on(
            Method::Get,
            "https://dict.youdao.com/jsonapi?q=%E7%81%AF",
            200,
            r##"{"ce":{"word":[{"phone":"dēng","trs":[{"tr":[{"l":{"i":["",{"#text":"lamp","@action":"link"}],"#tran":"灯，台灯"}}]}]}]}}"##,
        );
    let a = look_up("youdao", &http, "lamp", "en", "zh-Hans").unwrap();
    assert_eq!(a.pronunciations.len(), 2);
    assert_eq!(a.pronunciations[0].audio_url.as_deref(), Some("https://dict.youdao.com/dictvoice?audio=lamp&type=1"));
    assert_eq!(a.explanations[0].part_of_speech, "n.");
    assert_eq!(a.explanations[0].definitions, ["灯", "台灯"]);
    assert_eq!(a.explanations[1].part_of_speech, "v.");
    assert_eq!(a.associations, ["lamps"]);
    assert_eq!(a.examples[0].source, "Turn on the lamp.");
    assert_eq!(a.examples[0].target, "开灯。");

    let a = look_up("youdao", &http, "灯", "zh-Hans", "en").unwrap();
    assert_eq!(a.pronunciations[0].symbol, "dēng");
    assert_eq!(a.explanations[0].definitions, ["lamp (灯，台灯)"]);

    let unknown = MockHttp::new().on(Method::Get, "https://dict.youdao.com/", 200, r#"{"input":"qwzxv","meta":{}}"#);
    assert!(look_up("youdao", &unknown, "qwzxv", "en", "zh-Hans").unwrap_err().to_string().contains("qwzxv"));
}

// -- Academic -----------------------------------------------------------------

#[test]
fn dic_academic_groups_results_by_source() {
    let page = r#"<html><div id="search_results"><ul class="terms-list" id="found_articles">
<li><p><span class="item">1</span><strong><a href="//dal.academic.ru/1/ДОМ">ДОМ</a></strong> — муж. строение для житья; &amp;#160;избa …</p>
<p class="src"><a href="//dic.academic.ru/contents.nsf/enc2p/">Толковый словарь Даля</a></p></li>
<li><p><span class="item">2</span><strong><a href="//x">дом</a></strong> — жилое здание …</p>
<p class="src"><a href="//y">Толковый словарь Ушакова</a></p></li>
<li><p><span class="item">3</span><strong><a href="//z">Дом</a></strong> — семья, хозяйство …</p>
<p class="src"><a href="//dic.academic.ru/contents.nsf/enc2p/">Толковый словарь Даля</a></p></li>
</ul></div></html>"#;
    let http = MockHttp::new().on(Method::Get, "https://dic.academic.ru/searchall.php", 200, page);
    let a = look_up("dic-academic", &http, "дом", "ru", "en").unwrap();
    assert_eq!(a.explanations.len(), 2);
    assert_eq!(a.explanations[0].part_of_speech, "Толковый словарь Даля");
    assert_eq!(
        a.explanations[0].definitions,
        ["ДОМ — муж. строение для житья; избa …", "Дом — семья, хозяйство …"],
        "the twice-escaped no-break space is decoded and folded like any space"
    );
    assert!(http.requests()[0].url.contains("&from=ru&"), "{}", http.requests()[0].url);

    let none = MockHttp::new().on(Method::Get, "https://dic.academic.ru/", 200, "<html>Ничего не найдено</html>");
    assert!(look_up("dic-academic", &none, "qwzxv", "ru", "en").unwrap_err().to_string().contains("qwzxv"));
}

#[test]
fn translate_academic_passes_entries_on_as_html() {
    let page = r#"<html><div id="article"><ul class="terms-list">
<li><p><span class="item">1</span><strong>lamp</strong><span class="sound" data-term="lamp" onClick="x()"></span></p>
<div class="translate_definition"><div><strong>1.</strong> лампа; <a href="/light/en/ru/">light</a></div></div>
<p class="src"><a href="https://eng_rus.academic.ru">Англо-русский словарь</a></p></li>
<li><p><span class="item">2</span><strong>lamp</strong></p>
<div class="translate_definition"><div>светильник</div></div><p class="src"><a href="//other.academic.ru">Другой словарь</a></p></li>
</ul></div><script>ads()</script></html>"#;
    let http = MockHttp::new().on(Method::Get, "https://translate.academic.ru/lamp/en/ru/", 200, page);
    let a = look_up("translate-academic", &http, "lamp", "en", "ru").unwrap();
    let html = a.html.as_deref().unwrap();
    assert!(html.contains("лампа") && html.contains("светильник"), "{html}");
    assert!(html.contains("href=\"https://translate.academic.ru/light/en/ru/\""), "links made absolute: {html}");
    assert!(html.contains("href=\"https://other.academic.ru\""), "{html}");
    assert!(!html.contains("onClick") && !html.contains("ads()"), "no scripts or sound buttons: {html}");
    assert!(!a.is_empty());

    let none = MockHttp::new().on(Method::Get, "https://translate.academic.ru/", 200, "<html>Ничего не найдено</html>");
    assert!(look_up("translate-academic", &none, "qwzxv", "en", "ru").unwrap_err().to_string().contains("qwzxv"));
}

// -- in the engine ------------------------------------------------------------

#[test]
fn bundled_capabilities_match_their_roles() {
    let caps = |id: &str| provider(id).info().capabilities;
    assert!(caps("yandex-web").contains(Capabilities::TRANSLATE | Capabilities::DICTIONARY));
    assert!(caps("deepl-web").contains(Capabilities::TRANSLATE));
    for id in
        ["multitran", "lingvo-live", "oxford-learners", "wiktionary", "youdao", "dic-academic", "translate-academic"]
    {
        assert!(caps(id).contains(Capabilities::DICTIONARY), "{id}");
        assert!(!caps(id).contains(Capabilities::TRANSLATE), "{id}");
    }
}
