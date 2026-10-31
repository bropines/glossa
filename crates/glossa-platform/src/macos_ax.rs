//! Reading the selection through the macOS Accessibility API
//! (`AXSelectedText` of the focused element), without the clipboard.
//!
//! Declared by hand against ApplicationServices and CoreFoundation: four
//! functions do not justify a binding crate. Needs the Accessibility
//! permission; without it `AXIsProcessTrusted` is false and we return `None`
//! so the caller falls back to Cmd+C (which needs the same permission, but
//! enigo will prompt for it).

use std::ffi::{c_char, c_void, CStr};

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type AXUIElementRef = *const c_void;
type CFIndex = isize;
type Boolean = u8;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_AX_ERROR_SUCCESS: i32 = 0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> Boolean;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: *mut CFTypeRef) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithBytes(
        alloc: *const c_void,
        bytes: *const u8,
        len: CFIndex,
        encoding: u32,
        is_external: Boolean,
    ) -> CFStringRef;
    fn CFStringGetLength(s: CFStringRef) -> CFIndex;
    fn CFStringGetMaximumSizeForEncoding(len: CFIndex, encoding: u32) -> CFIndex;
    fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: CFIndex, encoding: u32) -> Boolean;
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFRelease(cf: CFTypeRef);
}

/// An owned CF object, released on drop.
struct Owned(CFTypeRef);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we hold the one reference a Create/Copy call gave us.
            unsafe { CFRelease(self.0) };
        }
    }
}

fn cfstr(s: &str) -> Owned {
    // SAFETY: the bytes are valid UTF-8 for the given length; the result is
    // owned by the returned guard.
    Owned(unsafe {
        CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as CFIndex, K_CF_STRING_ENCODING_UTF8, 0)
    })
}

fn copy_attribute(element: CFTypeRef, name: &str) -> Option<Owned> {
    let attr = cfstr(name);
    if attr.0.is_null() {
        return None;
    }
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: `element` is a live AXUIElement, `attr` a live CFString and
    // `value` a valid out-pointer; on success we own `value`.
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr.0, &mut value) };
    (err == K_AX_ERROR_SUCCESS && !value.is_null()).then_some(Owned(value))
}

fn to_string(s: CFStringRef) -> Option<String> {
    // SAFETY: `s` is a live CFString; the buffer is sized by CF's own
    // worst-case estimate and NUL-terminated on success.
    unsafe {
        let len = CFStringGetLength(s);
        let cap = CFStringGetMaximumSizeForEncoding(len, K_CF_STRING_ENCODING_UTF8) + 1;
        let mut buf = vec![0 as c_char; cap.max(1) as usize];
        if CFStringGetCString(s, buf.as_mut_ptr(), cap, K_CF_STRING_ENCODING_UTF8) == 0 {
            return None;
        }
        Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

/// The focused element's selected text, or `None`.
pub fn selected_text() -> Option<String> {
    // SAFETY: no arguments; reads the process's trust state.
    if unsafe { AXIsProcessTrusted() } == 0 {
        return None;
    }
    // SAFETY: returns a new reference we own.
    let system = Owned(unsafe { AXUIElementCreateSystemWide() });
    if system.0.is_null() {
        return None;
    }
    let focused = copy_attribute(system.0, "AXFocusedUIElement")?;
    let text = copy_attribute(focused.0, "AXSelectedText")?;
    // SAFETY: both calls only inspect a live CF object.
    if unsafe { CFGetTypeID(text.0) != CFStringGetTypeID() } {
        return None;
    }
    to_string(text.0).filter(|t| !t.trim().is_empty())
}
