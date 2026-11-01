//! Windows: the Spell Checking API (`ISpellCheckerFactory`, Windows 8+),
//! with the dictionaries of the installed language packs. Words added go to
//! the user's Windows dictionary, which other programs share.
//!
//! The factory and one checker per language live in a thread-local, so a
//! worker thread pays for loading a language once.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Globalization::{
    ISpellChecker, ISpellCheckerFactory, SpellCheckerFactory, CORRECTIVE_ACTION_GET_SUGGESTIONS,
    CORRECTIVE_ACTION_REPLACE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IEnumString, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};

use super::{from_utf16, utf16_offsets, Misspelling};

pub const MAIN_THREAD_ONLY: bool = false;
pub const CAN_ADD: bool = true;

/// Balances a successful `CoInitializeEx` on this thread.
struct Com {
    owned: bool,
}

impl Com {
    fn init() -> Com {
        crate::uia::keep_mta();
        // SAFETY: initialising COM on the current thread; S_FALSE (already
        // initialised) still needs a matching CoUninitialize, while
        // RPC_E_CHANGED_MODE (already an STA) must not get one.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Com { owned: hr.is_ok() }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: paired with the CoInitializeEx above, on the same
            // thread (the state holding it is thread-local).
            unsafe { CoUninitialize() };
        }
    }
}

struct State {
    // Fields drop in order: the COM objects before COM itself.
    checkers: HashMap<String, Option<ISpellChecker>>,
    factory: Option<ISpellCheckerFactory>,
    _com: Com,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        let state = slot.get_or_insert_with(|| {
            let com = Com::init();
            // SAFETY: COM is initialised on this thread.
            let factory = unsafe { CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| tracing::debug!("spell: no ISpellCheckerFactory: {e}"))
                .ok();
            State { checkers: HashMap::new(), factory, _com: com }
        });
        f(state)
    })
}

fn checker(state: &mut State, tag: &str) -> Option<ISpellChecker> {
    if let Some(c) = state.checkers.get(tag) {
        return c.clone();
    }
    let factory = state.factory.as_ref()?;
    let htag = HSTRING::from(tag);
    // SAFETY: a live factory on the thread that created it.
    let made = unsafe {
        match factory.IsSupported(&htag) {
            Ok(b) if b.as_bool() => factory.CreateSpellChecker(&htag).ok(),
            _ => None,
        }
    };
    state.checkers.insert(tag.to_owned(), made.clone());
    made
}

/// Take a string the API allocated.
///
/// # Safety
/// `p` must be null or a string from `CoTaskMemAlloc`, not used after this.
unsafe fn take(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: as the caller promised.
    let s = unsafe { p.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

/// Every string of an `IEnumString`.
///
/// # Safety
/// COM must be initialised on this thread.
unsafe fn strings(list: &IEnumString) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let mut one = [PWSTR::null()];
        let mut got = 0u32;
        // SAFETY: one slot, filled with a CoTaskMemAlloc'ed string or left
        // null; `take` frees it.
        let hr = unsafe { list.Next(&mut one, Some(&mut got)) };
        if hr.is_err() || got == 0 {
            break;
        }
        if let Some(s) = unsafe { take(one[0]) } {
            out.push(s);
        }
        if out.len() > 1000 {
            break;
        }
    }
    out
}

pub fn languages() -> Vec<String> {
    with_state(|state| {
        let Some(factory) = state.factory.as_ref() else { return Vec::new() };
        // SAFETY: a live factory on its own thread.
        unsafe { factory.SupportedLanguages().map(|l| strings(&l)).unwrap_or_default() }
    })
}

pub fn check(text: &str, tag: &str) -> Vec<Misspelling> {
    with_state(|state| {
        let Some(checker) = checker(state, tag) else { return Vec::new() };
        let map = utf16_offsets(text);
        let mut found = Vec::new();
        // SAFETY: a live checker on its own thread; every returned
        // interface is reference-counted by the windows crate.
        unsafe {
            let Ok(errors) = checker.Check(&HSTRING::from(text)) else { return found };
            loop {
                let mut error = None;
                if errors.Next(&mut error).is_err() {
                    break;
                }
                let Some(error) = error else { break };
                // Repeated words ("the the") come as "delete"; only words
                // the dictionary does not know are wanted here.
                let action = error.CorrectiveAction().unwrap_or_default();
                if action != CORRECTIVE_ACTION_GET_SUGGESTIONS && action != CORRECTIVE_ACTION_REPLACE {
                    continue;
                }
                let (Ok(start), Ok(len)) = (error.StartIndex(), error.Length()) else { continue };
                if let Some(m) = from_utf16(text, &map, start as usize, len as usize) {
                    found.push(m);
                }
                if found.len() >= 2000 {
                    break;
                }
            }
        }
        found
    })
}

pub fn suggest(word: &str, tag: &str) -> Vec<String> {
    with_state(|state| {
        let Some(checker) = checker(state, tag) else { return Vec::new() };
        // SAFETY: a live checker on its own thread.
        unsafe { checker.Suggest(&HSTRING::from(word)).map(|l| strings(&l)).unwrap_or_default() }
    })
}

pub fn add(word: &str, tag: &str) -> bool {
    with_state(|state| {
        let Some(checker) = checker(state, tag) else { return false };
        // SAFETY: a live checker on its own thread.
        unsafe { checker.Add(&HSTRING::from(word)) }.is_ok()
    })
}
