//! Language detection on this computer: nothing is sent anywhere.
//!
//! Two layers. The script settles what it can alone: kana means Japanese,
//! Hangul Korean, Han without kana Chinese (simplified or traditional by
//! the characters only one of them uses). Everything else goes to
//! `whatlang`'s trigram tables, which know seventy languages.
//!
//! Trigrams are unsure about short text (Russian against Bulgarian on two
//! words). The answer says so ([`Guess::sure`]), and the pipeline may then
//! ask an enabled service instead (`[general] detect`). Where it may not,
//! the user's own languages break the tie: for someone who reads Russian
//! and English, a few Cyrillic words are Russian.

use glossa_core::{Detection, LangId};
use whatlang::{Detector, Lang, Script};

/// What the built-in detector made of a text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guess {
    pub detection: Detection,
    /// Sure enough to act on without a second opinion. An unsure guess is
    /// still the best one available offline.
    pub sure: bool,
}

/// No more than this much of a text is looked at: the answer does not
/// change after a few sentences, and the time should not grow with a book.
const SAMPLE_CHARS: usize = 2000;

/// The confidence given to a guess that the user's languages settled: just
/// usable ([`Detection::is_usable`]), and visibly not a measurement.
const HINTED: f32 = 0.5;

/// Fewer letters than this is "a few words": too little for trigrams, so
/// the user's own languages decide.
const SHORT_LETTERS: usize = 30;

const SIMPLIFIED: LangId = LangId(11);
const TRADITIONAL: LangId = LangId(12);

/// whatlang's ISO 639-3 codes as the codes Glossa's language table knows.
const CODES: [(&str, &str); 70] = [
    ("epo", "eo"),
    ("eng", "en"),
    ("rus", "ru"),
    ("cmn", "zh-CN"),
    ("spa", "es"),
    ("por", "pt"),
    ("ita", "it"),
    ("ben", "bn"),
    ("fra", "fr"),
    ("deu", "de"),
    ("ukr", "uk"),
    ("kat", "ka"),
    ("ara", "ar"),
    ("hin", "hi"),
    ("jpn", "ja"),
    ("heb", "he"),
    ("yid", "yi"),
    ("pol", "pl"),
    ("amh", "am"),
    ("jav", "jv"),
    ("kor", "ko"),
    ("nob", "no"),
    ("dan", "da"),
    ("swe", "sv"),
    ("fin", "fi"),
    ("tur", "tr"),
    ("nld", "nl"),
    ("hun", "hu"),
    ("ces", "cs"),
    ("ell", "el"),
    ("bul", "bg"),
    ("bel", "be"),
    ("mar", "mr"),
    ("kan", "kn"),
    ("ron", "ro"),
    ("slv", "sl"),
    ("hrv", "hr"),
    ("srp", "sr"),
    ("mkd", "mk"),
    ("lit", "lt"),
    ("lav", "lv"),
    ("est", "et"),
    ("tam", "ta"),
    ("vie", "vi"),
    ("urd", "ur"),
    ("tha", "th"),
    ("guj", "gu"),
    ("uzb", "uz"),
    ("pan", "pa"),
    ("aze", "az"),
    ("ind", "id"),
    ("tel", "te"),
    ("pes", "fa"),
    ("mal", "ml"),
    ("ori", "or"),
    ("mya", "my"),
    ("nep", "ne"),
    ("sin", "si"),
    ("khm", "km"),
    ("tuk", "tk"),
    ("aka", "ak"),
    ("zul", "zu"),
    ("sna", "sn"),
    ("afr", "af"),
    ("lat", "la"),
    ("slk", "sk"),
    ("cat", "ca"),
    ("tgl", "tl"),
    ("hye", "hy"),
    ("cym", "cy"),
];

/// A whatlang language as Glossa's, when the table has it.
fn to_lang_id(lang: Lang) -> Option<LangId> {
    let code = lang.code();
    let (_, ours) = CODES.iter().find(|(theirs, _)| *theirs == code)?;
    LangId::from_code(ours).filter(|l| l.is_language())
}

/// One of Glossa's languages as whatlang's, when it knows it.
fn to_whatlang(lang: LangId) -> Option<Lang> {
    if lang == SIMPLIFIED || lang == TRADITIONAL {
        return Some(Lang::Cmn);
    }
    CODES
        .iter()
        .find(|(_, ours)| LangId::from_code(ours) == Some(lang))
        .and_then(|(theirs, _)| Lang::from_code(*theirs))
}

/// Detect the language of `text`. `hints` are the languages the user works
/// with (the first and second language): they decide between languages the
/// text alone cannot tell apart. `None` when there are no letters, or the
/// language is not one Glossa has.
pub fn detect(text: &str, hints: &[LangId]) -> Option<Guess> {
    let sample: String = text.chars().take(SAMPLE_CHARS).collect();
    if let Some(guess) = by_script(&sample, hints) {
        return Some(guess);
    }
    let script = whatlang::detect_script(&sample)?;
    let info = whatlang::detect(&sample);
    let mut hinted: Vec<(LangId, Lang)> = hints
        .iter()
        .filter_map(|h| Some((*h, to_whatlang(*h)?)))
        .filter(|(_, lang)| script.langs().contains(lang))
        .collect();
    hinted.dedup();
    let reliable = info.as_ref().is_some_and(|i| i.is_reliable());
    let measured = info.as_ref().map_or(0.0, |i| i.confidence() as f32).clamp(0.0, 1.0);
    // The user's languages settle only what the text cannot: a few words,
    // or a guess that is already one of them. A paragraph of German is not
    // English because the reader knows English.
    let top = info.as_ref().map(|i| i.lang());
    let short = sample.chars().filter(|c| c.is_alphabetic()).count() < SHORT_LETTERS;
    let hints_decide = !hinted.is_empty() && (short || hinted.iter().any(|(_, lang)| Some(*lang) == top));
    if reliable || !hints_decide {
        let lang = to_lang_id(info?.lang())?;
        return Some(Guess { detection: Detection { lang, confidence: measured }, sure: reliable });
    }
    // Unsure, and the user reads at least one language written this way:
    // choose among those.
    let lang = match hinted.as_slice() {
        [(only, _)] => *only,
        several => {
            let allowed = several.iter().map(|(_, lang)| *lang).collect();
            let narrowed = Detector::with_allowlist(allowed).detect(&sample).map(|i| i.lang());
            several.iter().find(|(_, lang)| Some(*lang) == narrowed).map_or(several[0].0, |(id, _)| *id)
        }
    };
    // Chinese among the hints matched a Han text by_script let through.
    let lang = if script == Script::Mandarin { chinese_variant(&sample, lang) } else { lang };
    Some(Guess { detection: Detection { lang, confidence: measured.clamp(HINTED, 0.89) }, sure: false })
}

/// What the writing system alone decides: Japanese, Korean, Chinese.
fn by_script(sample: &str, hints: &[LangId]) -> Option<Guess> {
    let (mut letters, mut kana, mut han, mut hangul) = (0usize, 0usize, 0usize, 0usize);
    for c in sample.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => kana += 1,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F => han += 1,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => hangul += 1,
            _ => {}
        }
    }
    if letters == 0 {
        return None;
    }
    let sure =
        |code: &str| LangId::from_code(code).map(|lang| Guess { detection: Detection::certain(lang), sure: true });
    // Japanese mixes kana with Han characters, and a product name or a
    // quotation in Latin letters does not make it less Japanese.
    if kana > 0 && (kana + han) * 2 >= letters {
        return sure("ja");
    }
    if hangul > 0 && (hangul + han) * 2 >= letters {
        return sure("ko");
    }
    if han * 2 >= letters {
        // Han alone is Chinese, unless the user reads Japanese and not
        // Chinese: then a short run of kanji (a name, a heading) is theirs.
        let japanese = LangId::from_code("ja");
        let reads = |l: LangId| hints.contains(&l);
        if japanese.is_some_and(reads) && !reads(SIMPLIFIED) && !reads(TRADITIONAL) {
            return japanese.map(|lang| Guess { detection: Detection { lang, confidence: HINTED }, sure: false });
        }
        let lang = chinese_variant(sample, SIMPLIFIED);
        return Some(Guess { detection: Detection::certain(lang), sure: true });
    }
    None
}

/// Simplified or traditional, by the common characters only one of them
/// uses; `fallback` when the text has none of either (or as many of both).
fn chinese_variant(sample: &str, fallback: LangId) -> LangId {
    const SIMPLIFIED_ONLY: &str =
        "这个们来时为说国过对会学经还点实现发体关样长间题当种么机电开无与应东车书见门头问后语话华万义";
    const TRADITIONAL_ONLY: &str =
        "這個們來時為說國過對會學經還點實現發體關樣長間題當種麼機電開無與應東車書見門頭問後語話華萬義";
    let simplified = sample.chars().filter(|c| SIMPLIFIED_ONLY.contains(*c)).count();
    let traditional = sample.chars().filter(|c| TRADITIONAL_ONLY.contains(*c)).count();
    match simplified.cmp(&traditional) {
        std::cmp::Ordering::Greater => SIMPLIFIED,
        std::cmp::Ordering::Less => TRADITIONAL,
        std::cmp::Ordering::Equal if fallback == TRADITIONAL => TRADITIONAL,
        std::cmp::Ordering::Equal => SIMPLIFIED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(code: &str) -> LangId {
        LangId::from_code(code).unwrap_or_else(|| panic!("{code} is not in the table"))
    }

    fn lang(text: &str, hints: &[&str]) -> Option<LangId> {
        let hints: Vec<LangId> = hints.iter().map(|h| id(h)).collect();
        detect(text, &hints).map(|g| g.detection.lang)
    }

    #[test]
    fn every_whatlang_code_is_mapped_and_most_are_languages_glossa_has() {
        let mut unknown = Vec::new();
        for l in Lang::all() {
            assert!(CODES.iter().any(|(theirs, _)| *theirs == l.code()), "{} has no mapping", l.code());
            match to_lang_id(*l) {
                Some(ours) => assert_eq!(to_whatlang(ours), Some(*l), "{} does not round-trip", l.code()),
                None => unknown.push(l.code()),
            }
        }
        // A language missing from Glossa's table is "not detected", never
        // another language. Listed, so that adding one to the table (or a
        // whatlang upgrade) is noticed here.
        assert_eq!(unknown, ["amh", "jav", "guj", "pan", "mal", "ori", "mya", "nep", "tuk", "aka", "zul", "sna"]);
        assert_eq!(detect("ይህ በአማርኛ የተጻፈ ዓረፍተ ነገር ነው", &[]), None, "Amharic is not in the table");
        for (theirs, ours) in [("rus", "ru"), ("ukr", "uk"), ("bul", "bg"), ("srp", "sr"), ("heb", "he"), ("pes", "fa")]
        {
            assert_eq!(to_lang_id(Lang::from_code(theirs).unwrap()), Some(id(ours)));
        }
        assert_eq!(to_lang_id(Lang::Cmn), Some(SIMPLIFIED));
        assert_eq!(SIMPLIFIED, id("zh-CN"));
        assert_eq!(TRADITIONAL, id("zh-TW"));
    }

    #[test]
    fn sentences_are_detected_and_sure() {
        for (text, code) in [
            ("The quick brown fox jumps over the lazy dog and runs away into the forest.", "en"),
            ("Съешь же ещё этих мягких французских булок, да выпей чаю, пока он не остыл.", "ru"),
            ("Реве та стогне Дніпр широкий, сердитий вітер завива, додолу верби гне високі.", "uk"),
            ("Това е изречение на български език, което трябва да бъде разпознато правилно.", "bg"),
            ("Ово је реченица на српском језику коју треба препознати без икакве грешке.", "sr"),
            ("Der schnelle braune Fuchs springt über den faulen Hund und läuft in den Wald.", "de"),
            ("Portez ce vieux whisky au juge blond qui fume une cigarette dans le jardin.", "fr"),
            ("El veloz murciélago hindú comía feliz cardillo y kiwi mientras la cigüeña tocaba.", "es"),
            ("هذا نص باللغة العربية يجب أن يتم التعرف عليه بشكل صحيح ودون أي خطأ.", "ar"),
            ("Αυτό είναι ένα κείμενο στα ελληνικά.", "el"),
            ("यह हिंदी भाषा में लिखा गया एक वाक्य है जिसे सही पहचाना जाना चाहिए।", "hi"),
            ("นี่คือข้อความภาษาไทย", "th"),
        ] {
            let g = detect(text, &[]).unwrap_or_else(|| panic!("nothing for {code}"));
            assert_eq!(g.detection.lang, id(code), "{text}");
            assert!(g.sure && g.detection.is_usable(), "{code}: {g:?}");
        }
    }

    #[test]
    fn cjk_is_told_apart_by_script() {
        assert_eq!(lang("これは日本語の文章です。", &[]), Some(id("ja")));
        // Mostly kanji with one particle is still Japanese, not Chinese.
        assert_eq!(lang("東京大学の学生", &[]), Some(id("ja")));
        assert_eq!(lang("カタカナ", &[]), Some(id("ja")));
        assert_eq!(lang("한국어 문장입니다", &[]), Some(id("ko")));
        assert_eq!(lang("这是一个用简体中文写的句子。", &[]), Some(SIMPLIFIED));
        assert_eq!(lang("這是一個用繁體中文寫的句子。", &[]), Some(TRADITIONAL));
        assert!(detect("这是中文", &[]).unwrap().sure);
        // Kanji alone: Chinese, unless the user reads Japanese and no Chinese.
        assert_eq!(lang("日本", &[]), Some(SIMPLIFIED));
        let g = detect("日本", &[id("ja"), id("en")]).unwrap();
        assert_eq!((g.detection.lang, g.sure), (id("ja"), false));
        assert_eq!(lang("日本", &["ja", "zh-CN"]), Some(SIMPLIFIED));
    }

    #[test]
    fn mixed_scripts_go_to_the_one_most_of_the_text_is_in() {
        assert_eq!(lang("Скачал новую версию Windows и установил её на свой старый компьютер", &[]), Some(id("ru")));
        assert_eq!(lang("iPhoneの新しいモデルが発売されました", &[]), Some(id("ja")));
        assert_eq!(lang("I learned the word привет yesterday from my Russian friend at school", &[]), Some(id("en")));
        assert_eq!(lang("我今天买了一个新的 iPhone 手机", &[]), Some(SIMPLIFIED));
    }

    /// One or two words are where trigram tables guess: the guess is marked
    /// unsure (so a service may be asked), and the user's languages decide.
    #[test]
    fn short_text_is_settled_by_the_users_languages_and_marked_unsure() {
        for (text, hints, want) in [
            ("привет", ["ru", "en"], "ru"),
            ("hello", ["ru", "en"], "en"),
            ("доброго ранку", ["uk", "en"], "uk"),
            ("благодаря", ["bg", "en"], "bg"),
            ("хвала", ["sr", "en"], "sr"),
            ("ok", ["ru", "en"], "en"),
            ("bonjour", ["fr", "ru"], "fr"),
        ] {
            let hints: Vec<LangId> = hints.iter().map(|h| id(h)).collect();
            let g = detect(text, &hints).unwrap_or_else(|| panic!("nothing for {text}"));
            assert_eq!(g.detection.lang, id(want), "{text}");
            assert!(g.detection.is_usable(), "{text}: {g:?}");
        }
        // Two languages in one script among the hints: the text decides.
        assert_eq!(lang("Це речення написане українською мовою", &["ru", "uk"]), Some(id("uk")));
        assert_eq!(lang("Это предложение написано на русском языке", &["ru", "uk"]), Some(id("ru")));
        // A script none of the user's languages uses is still detected.
        assert_eq!(lang("ελληνικά", &["ru", "en"]), Some(id("el")));
        // A single word without hints is a guess and says so.
        let alone = detect("привет", &[]);
        assert!(alone.is_none_or(|g| !g.sure), "{alone:?}");
    }

    #[test]
    fn text_without_letters_is_not_detected() {
        assert_eq!(detect("", &[]), None);
        assert_eq!(detect("12345 !!! ... 42%", &[id("ru")]), None);
        assert_eq!(detect("   \n\t", &[]), None);
    }

    #[test]
    fn a_long_text_costs_no_more_than_its_start() {
        let long = "Это длинный русский текст. ".repeat(20_000);
        let started = std::time::Instant::now();
        assert_eq!(lang(&long, &[]), Some(id("ru")));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}
