//! Language names as the interface shows them: in the interface language
//! where its locale names the language (the built-in Russian pack names
//! them all), else in English, with the language's own name beside it in
//! lists.
//!
//! The names follow the current locale, which the app sets here whenever
//! it loads one, so pickers and labels need no locale handed down to them.
//! Everything runs on the UI thread.

use std::cell::RefCell;
use std::sync::Arc;

use glossa_core::{LangId, Locale};

thread_local! {
    static LOCALE: RefCell<Option<Arc<Locale>>> = const { RefCell::new(None) };
}

/// Use `locale`'s names from now on.
pub fn set_locale(locale: Arc<Locale>) {
    LOCALE.with(|l| *l.borrow_mut() = Some(locale));
}

/// The language's name in the interface language.
pub fn name(lang: LangId) -> String {
    LOCALE.with(|l| match l.borrow().as_ref() {
        Some(locale) => locale.language_name(lang).to_owned(),
        None => lang.english_name().to_owned(),
    })
}

/// A list entry: the name, and the language's own name after it when that
/// differs ("Немецкий · Deutsch", "German · Deutsch").
pub fn label(lang: LangId) -> String {
    let (name, native) = (name(lang), lang.native_name());
    if native.is_empty() || native == name {
        name
    } else {
        format!("{name}  ·  {native}")
    }
}

/// Whether a search for `needle` (already lower case) finds `lang`: by its
/// name here, its English name, its own name, or its tag.
pub fn matches(lang: LangId, needle: &str) -> bool {
    needle.is_empty()
        || name(lang).to_lowercase().contains(needle)
        || lang.english_name().to_lowercase().contains(needle)
        || lang.native_name().to_lowercase().contains(needle)
        || lang.bcp47().eq_ignore_ascii_case(needle)
}

/// Sort languages by their names here.
pub fn sort(langs: &mut [LangId]) {
    langs.sort_by_cached_key(|l| name(*l).to_lowercase());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_locale() {
        set_locale(Arc::new(Locale::english()));
        assert_eq!(name(LangId::ENGLISH), "English");
        assert_eq!(label(LangId::RUSSIAN), "Russian  ·  Русский");
        set_locale(Arc::new(Locale::builtin("ru").unwrap()));
        assert_eq!(name(LangId::ENGLISH), "Английский");
        // The language's own name, the same as the Russian one, is not repeated.
        assert_eq!(label(LangId::RUSSIAN), "Русский");
        // Found by any of its names.
        assert!(matches(LangId::ENGLISH, "англ"));
        assert!(matches(LangId::ENGLISH, "engl"));
        assert!(matches(LangId::ENGLISH, "en"));
        let mut l = vec![LangId::ENGLISH, LangId::RUSSIAN, LangId(23)];
        sort(&mut l);
        assert_eq!(l, [LangId::ENGLISH, LangId(23), LangId::RUSSIAN], "Английский, Немецкий, Русский");
    }
}
