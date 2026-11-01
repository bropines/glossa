//! macOS: `NSSpellChecker`, the checker every Cocoa text field uses, with
//! the system's dictionaries. Words learned go to the user's dictionary,
//! shared with other apps.
//!
//! AppKit objects belong to the main thread, so every call here returns
//! nothing elsewhere ([`MAIN_THREAD_ONLY`]).
//!
//! Not yet tried on a Mac.

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSRange, NSString};

use super::{from_utf16, utf16_offsets, Misspelling};

pub const MAIN_THREAD_ONLY: bool = true;
pub const CAN_ADD: bool = true;

fn checker() -> Option<Retained<NSSpellChecker>> {
    MainThreadMarker::new()?;
    Some(NSSpellChecker::sharedSpellChecker())
}

/// AppKit names dictionaries `en_GB`; Glossa passes `en-GB`.
fn ns_language(tag: &str) -> Retained<NSString> {
    NSString::from_str(&tag.replace('-', "_"))
}

pub fn languages() -> Vec<String> {
    let Some(checker) = checker() else { return Vec::new() };
    checker.availableLanguages().to_vec().iter().map(|s| s.to_string().replace('_', "-")).collect()
}

pub fn check(text: &str, tag: &str) -> Vec<Misspelling> {
    let Some(checker) = checker() else { return Vec::new() };
    let string = NSString::from_str(text);
    let language = ns_language(tag);
    let total = string.length();
    let map = utf16_offsets(text);
    let mut found = Vec::new();
    let mut start = 0usize;
    while start < total && found.len() < 2000 {
        // SAFETY: a null word count is allowed; tag 0 is the shared
        // document.
        let r = unsafe {
            checker.checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                &string,
                start as isize,
                Some(&language),
                false,
                0,
                std::ptr::null_mut(),
            )
        };
        // NSNotFound, or nothing more: done.
        if r.length == 0 || r.location >= total || r.location < start {
            break;
        }
        if let Some(m) = from_utf16(text, &map, r.location, r.length) {
            found.push(m);
        }
        start = r.location + r.length;
    }
    found
}

pub fn suggest(word: &str, tag: &str) -> Vec<String> {
    let Some(checker) = checker() else { return Vec::new() };
    let string = NSString::from_str(word);
    let range = NSRange::new(0, string.length());
    checker
        .guessesForWordRange_inString_language_inSpellDocumentWithTag(range, &string, Some(&ns_language(tag)), 0)
        .map(|list| list.to_vec().iter().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

pub fn add(word: &str, _tag: &str) -> bool {
    let Some(checker) = checker() else { return false };
    checker.learnWord(&NSString::from_str(word));
    true
}
