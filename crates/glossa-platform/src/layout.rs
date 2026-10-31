//! The keyboard-layout fix: text typed with the wrong layout active
//! ("ghbdtn" that should have been "привет", or "руддщ" that should have been
//! "hello") retyped as if the other layout had been on.
//!
//! The conversion is a key-for-key mapping between the US QWERTY layout and
//! the standard Windows Russian (ЙЦУКЕН) or Ukrainian layout, including the
//! shifted characters and the punctuation that moves between them
//! (`?` ↔ `,`, `&` ↔ `?`, `@` ↔ `"` …), so a round trip gives back exactly
//! what was typed.
//!
//! Which way to go is guessed from the script that dominates the text. The
//! Russian and Ukrainian layouts differ in only five keys (`s ] ' ` \`), so
//! for Latin input the choice between them is made from letter context —
//! `ї` straight after a vowel is Ukrainian, `ъ` there is impossible in Russian
//! — and falls back to a preference the caller supplies (Russian by default).

/// A conversion direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Conversion {
    /// Latin typed, Russian meant.
    EnToRu,
    /// Russian typed, Latin meant.
    RuToEn,
    /// Latin typed, Ukrainian meant.
    EnToUk,
    /// Ukrainian typed, Latin meant.
    UkToEn,
}

impl Conversion {
    /// The conversion that undoes this one.
    pub fn reverse(self) -> Conversion {
        match self {
            Conversion::EnToRu => Conversion::RuToEn,
            Conversion::RuToEn => Conversion::EnToRu,
            Conversion::EnToUk => Conversion::UkToEn,
            Conversion::UkToEn => Conversion::EnToUk,
        }
    }

    /// The Cyrillic layout involved.
    pub fn cyrillic(self) -> Cyrillic {
        match self {
            Conversion::EnToRu | Conversion::RuToEn => Cyrillic::Russian,
            Conversion::EnToUk | Conversion::UkToEn => Cyrillic::Ukrainian,
        }
    }
}

/// The Cyrillic layout to prefer when the text does not decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Cyrillic {
    #[default]
    Russian,
    Ukrainian,
}

// Each table is a pair of strings of equal length in chars: the US key and
// what the same key produces in the Cyrillic layout. Characters a key
// produces identically in both (digits, `!`, `%`, `*`, `(`, `)`, `-`, `=`,
// `_`, `+`, space) are left out, so they pass through unchanged.

const RU_EN: &str = concat!("`qwertyuiop[]asdfghjkl;'zxcvbnm,./", "~QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>?", "@#$^&|",);
const RU_RU: &str = concat!("ёйцукенгшщзхъфывапролджэячсмитьбю.", "ЁЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ,", "\"№;:?/",);

const UK_EN: &str = concat!("`qwertyuiop[]asdfghjkl;'zxcvbnm,./\\", "~QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>?|", "@#$^&",);
const UK_UK: &str = concat!("'йцукенгшщзхїфівапролджєячсмитьбю.ґ", "₴ЙЦУКЕНГШЩЗХЇФІВАПРОЛДЖЄЯЧСМИТЬБЮ,Ґ", "\"№;:?",);

fn table(cyr: Cyrillic) -> (&'static str, &'static str) {
    match cyr {
        Cyrillic::Russian => (RU_EN, RU_RU),
        Cyrillic::Ukrainian => (UK_EN, UK_UK),
    }
}

/// Map one character through a table, `from` → `to`.
fn map_char(c: char, from: &str, to: &str) -> char {
    match from.chars().position(|f| f == c) {
        Some(i) => to.chars().nth(i).unwrap_or(c),
        None => c,
    }
}

/// Convert `text` in the given direction. Characters the source layout does
/// not produce (digits, spaces, other scripts) pass through unchanged.
pub fn convert(text: &str, conversion: Conversion) -> String {
    let (en, cyr) = table(conversion.cyrillic());
    let (from, to) = match conversion {
        Conversion::EnToRu | Conversion::EnToUk => (en, cyr),
        Conversion::RuToEn | Conversion::UkToEn => (cyr, en),
    };
    text.chars().map(|c| map_char(c, from, to)).collect()
}

/// Guess the direction and convert; text with no letters, or with exactly as
/// many Latin as Cyrillic letters, comes back unchanged.
pub fn convert_layout(text: &str) -> String {
    convert_layout_prefer(text, Cyrillic::Russian)
}

/// [`convert_layout`] with an explicit tie-break between Russian and
/// Ukrainian — the caller may know which one the user has installed.
pub fn convert_layout_prefer(text: &str, preferred: Cyrillic) -> String {
    match guess_direction_prefer(text, preferred) {
        Some(c) => convert(text, c),
        None => text.to_owned(),
    }
}

/// Which way `text` should be converted, preferring Russian when the text
/// gives no hint. `None` when there is nothing to decide on.
pub fn guess_direction(text: &str) -> Option<Conversion> {
    guess_direction_prefer(text, Cyrillic::Russian)
}

/// [`guess_direction`] with an explicit Russian/Ukrainian preference.
pub fn guess_direction_prefer(text: &str, preferred: Cyrillic) -> Option<Conversion> {
    // Keys like `[`, `;` and `,` are letters in the Cyrillic layouts
    // (х, ж, б), so they count for the Latin side: "]]" is "її" mistyped.
    let latin = text
        .chars()
        .filter(|&c| c.is_ascii_alphabetic() || (c.is_ascii() && is_cyrillic(map_char(c, RU_EN, RU_RU))))
        .count();
    let cyrillic = text.chars().filter(|c| is_cyrillic(*c)).count();
    if latin == cyrillic {
        return None;
    }
    if cyrillic > latin {
        // The letters themselves say which layout produced them.
        let uk = text.chars().filter(|c| "іїєґІЇЄҐ".contains(*c)).count();
        let ru = text.chars().filter(|c| "ыэъёЫЭЪЁ".contains(*c)).count();
        let cyr = match uk.cmp(&ru) {
            std::cmp::Ordering::Greater => Cyrillic::Ukrainian,
            std::cmp::Ordering::Less => Cyrillic::Russian,
            std::cmp::Ordering::Equal => preferred,
        };
        return Some(match cyr {
            Cyrillic::Russian => Conversion::RuToEn,
            Cyrillic::Ukrainian => Conversion::UkToEn,
        });
    }
    let (ru_votes, uk_votes) = vote_ru_uk(text);
    let cyr = match uk_votes.cmp(&ru_votes) {
        std::cmp::Ordering::Greater => Cyrillic::Ukrainian,
        std::cmp::Ordering::Less => Cyrillic::Russian,
        std::cmp::Ordering::Equal => preferred,
    };
    Some(match cyr {
        Cyrillic::Russian => Conversion::EnToRu,
        Cyrillic::Ukrainian => Conversion::EnToUk,
    })
}

fn is_cyrillic(c: char) -> bool {
    ('\u{0400}'..='\u{04FF}').contains(&c)
}

fn is_cyr_vowel(c: char) -> bool {
    "аеёиоуыэюяєіїАЕЁИОУЫЭЮЯЄІЇ".contains(c)
}

fn is_cyr_consonant(c: char) -> bool {
    is_cyrillic(c) && !is_cyr_vowel(c) && !"ьъйЬЪЙ".contains(c)
}

/// For Latin input, count the keys whose Russian and Ukrainian readings
/// differ and where the context makes one of them implausible.
fn vote_ru_uk(latin: &str) -> (u32, u32) {
    let ru: Vec<char> = convert(latin, Conversion::EnToRu).chars().collect();
    let uk: Vec<char> = convert(latin, Conversion::EnToUk).chars().collect();
    let (mut ru_votes, mut uk_votes) = (0, 0);
    for (i, (&r, &u)) in ru.iter().zip(&uk).enumerate() {
        if r == u {
            continue;
        }
        let prev = if i == 0 { None } else { Some(uk[i - 1]) };
        let after_vowel = prev.is_some_and(|p| is_cyr_vowel(p) || p == '\'');
        let at_start = prev.is_none_or(|p| !p.is_alphabetic() && p != '\'');
        let after_consonant = prev.is_some_and(is_cyr_consonant);
        match u.to_lowercase().next().unwrap_or(u) {
            // ы/і: "ы" cannot start a word or follow a vowel in Russian.
            'і' if after_vowel || at_start => uk_votes += 1,
            // ъ/ї: "ъ" only ever follows a consonant; "ї" never does.
            'ї' if after_consonant => ru_votes += 1,
            'ї' => uk_votes += 1,
            // э/є: "моє", "своє" — є after a vowel is everyday Ukrainian.
            'є' if after_vowel => uk_votes += 1,
            // ё/': the Ukrainian apostrophe sits before я, ю, є, ї.
            '\'' if uk.get(i + 1).is_some_and(|n| "яюєїЯЮЄЇ".contains(*n)) => uk_votes += 1,
            _ => {}
        }
    }
    (ru_votes, uk_votes)
}

/// Switch the keyboard layout of the foreground window to the next installed
/// one, as the user would with Alt+Shift — the second half of "convert the
/// selection", so they can keep typing in the right language. Returns the
/// language identifier (`LANGID`, e.g. `0x0419` for Russian) of the layout
/// that was requested.
///
/// Windows only; elsewhere the desktop owns layout switching and this returns
/// an error.
pub fn switch_keyboard_layout_of_foreground() -> crate::Result<u16> {
    #[cfg(windows)]
    {
        win::switch_next()
    }
    #[cfg(not(windows))]
    {
        Err(crate::platform_err("switching the keyboard layout is only supported on Windows"))
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, HKL};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, WM_INPUTLANGCHANGEREQUEST,
    };

    pub fn switch_next() -> crate::Result<u16> {
        // SAFETY: plain Win32 queries with no pointers beyond the out-buffer,
        // which is sized from the count the first call returned.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() {
                return Err(crate::platform_err("no foreground window"));
            }
            let thread = GetWindowThreadProcessId(hwnd, None);
            let current = GetKeyboardLayout(thread);
            let count = GetKeyboardLayoutList(None);
            if count <= 0 {
                return Err(crate::platform_err("no keyboard layouts are installed"));
            }
            let mut list = vec![HKL::default(); count as usize];
            let got = GetKeyboardLayoutList(Some(&mut list));
            list.truncate(got.max(0) as usize);
            if list.len() < 2 {
                return Err(crate::platform_err("only one keyboard layout is installed"));
            }
            let idx = list.iter().position(|h| h.0 == current.0).unwrap_or(0);
            let next = list[(idx + 1) % list.len()];
            // The window, not the system, owns its input language; asking it
            // via WM_INPUTLANGCHANGEREQUEST is what the language bar does.
            PostMessageW(Some(hwnd), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(next.0 as isize))
                .map_err(|e| crate::platform_err(format!("cannot switch layout: {e}")))?;
            Ok((next.0 as usize & 0xffff) as u16)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_classic_examples() {
        assert_eq!(convert_layout("ghbdtn"), "привет");
        assert_eq!(convert_layout("привет"), "ghbdtn");
        assert_eq!(convert_layout("Ghbdtn? rfr ltkf&"), "Привет, как дела?");
        assert_eq!(convert_layout("Привет, как дела?"), "Ghbdtn? rfr ltkf&");
        assert_eq!(convert_layout("руддщ цщкдв"), "hello world");
        assert_eq!(convert_layout("hello"), "руддщ");
    }

    #[test]
    fn shifted_keys_and_punctuation() {
        assert_eq!(convert("Ntcn^ {jhjij? b ~", Conversion::EnToRu), "Тест: Хорошо, и Ё");
        assert_eq!(convert("@#$%^&*()", Conversion::EnToRu), "\"№;%:?*()");
        assert_eq!(convert("\"№;%:?*()", Conversion::RuToEn), "@#$%^&*()");
        assert_eq!(convert("ytn/", Conversion::EnToRu), "нет.");
        assert_eq!(convert("Ok|", Conversion::EnToRu), "Щл/");
        assert_eq!(convert("`~", Conversion::EnToRu), "ёЁ");
    }

    #[test]
    fn untouched_characters_pass_through() {
        assert_eq!(convert("123 -=_+!", Conversion::EnToRu), "123 -=_+!");
        assert_eq!(convert("日本 ghbdtn", Conversion::EnToRu), "日本 привет");
        assert_eq!(convert_layout("12345"), "12345");
        assert_eq!(convert_layout(""), "");
        // A dead heat between scripts is left alone rather than guessed.
        assert_eq!(convert_layout("abc где"), "abc где");
    }

    #[test]
    fn every_table_round_trips() {
        for cyr in [Cyrillic::Russian, Cyrillic::Ukrainian] {
            let (en, local) = table(cyr);
            assert_eq!(en.chars().count(), local.chars().count(), "{cyr:?} table lengths");
            let fwd = match cyr {
                Cyrillic::Russian => Conversion::EnToRu,
                Cyrillic::Ukrainian => Conversion::EnToUk,
            };
            // Both sides must be free of duplicates, or the inverse is lossy.
            for side in [en, local] {
                let mut seen: Vec<char> = side.chars().collect();
                seen.sort_unstable();
                let n = seen.len();
                seen.dedup();
                assert_eq!(seen.len(), n, "{cyr:?}: duplicate in {side:?}");
            }
            assert_eq!(convert(en, fwd), local);
            assert_eq!(convert(local, fwd.reverse()), en);
            assert_eq!(convert(&convert(en, fwd), fwd.reverse()), en);
        }
    }

    #[test]
    fn ukrainian_letters() {
        assert_eq!(convert("ghbdsn", Conversion::EnToUk), "привіт");
        assert_eq!(convert("привіт", Conversion::UkToEn), "ghbdsn");
        assert_eq!(convert("v`zcj", Conversion::EnToUk), "м'ясо");
        assert_eq!(convert("|fyjr", Conversion::EnToUk), "Ґанок");
        assert_eq!(convert("Є Ї І ₴", Conversion::UkToEn), "\" } S ~");
        // Cyrillic input names its own layout.
        assert_eq!(guess_direction("привіт"), Some(Conversion::UkToEn));
        assert_eq!(convert_layout("м'ясо і хліб"), "v`zcj s [ks,");
        assert_eq!(guess_direction("съешь"), Some(Conversion::RuToEn));
    }

    #[test]
    fn latin_input_picks_ukrainian_from_context() {
        // "Україна": ї right after а.
        assert_eq!(guess_direction("Erhf]yf"), Some(Conversion::EnToUk));
        assert_eq!(convert_layout("Erhf]yf"), "Україна");
        // "її": ї at the start of a word and after a vowel.
        assert_eq!(convert_layout("]]"), "її");
        // "і" as a word on its own.
        assert_eq!(convert_layout("nb s z"), "ти і я");
        // "моє".
        assert_eq!(convert_layout("vj'"), "моє");
        // "п'ять": apostrophe before я.
        assert_eq!(convert_layout("g`znm"), "п'ять");
        // "объём": ъ after a consonant is Russian.
        assert_eq!(convert_layout("j,]`v"), "объём");
        // "мы": no hint either way, so the preference decides.
        assert_eq!(convert_layout("vs"), "мы");
        assert_eq!(convert_layout_prefer("vs", Cyrillic::Ukrainian), "мі");
    }

    #[test]
    fn reverse_is_an_involution() {
        for c in [Conversion::EnToRu, Conversion::RuToEn, Conversion::EnToUk, Conversion::UkToEn] {
            assert_eq!(c.reverse().reverse(), c);
            assert_eq!(c.reverse().cyrillic(), c.cyrillic());
        }
    }
}
