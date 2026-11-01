//! System permissions Glossa needs, where the system asks for them.
//!
//! Only macOS gates what Glossa does behind user consent:
//!
//! - **Accessibility** to read the selection from other programs (the
//!   focused element's `AXSelectedText`) and to send the Cmd+C and Cmd+V that
//!   copying and replacing the selection fall back to.
//! - **Screen Recording** to capture a region for text recognition.
//! - **Input Monitoring** for mouse mode's event tap, which watches the
//!   mouse buttons in other programs. Accessibility covers it too, so it
//!   matters only when Accessibility is withheld.
//!
//! Global hotkeys need none of these (they are registered through Carbon).
//! On Windows and Linux every function here reports [`Status::NotNeeded`]
//! and does nothing.
//!
//! macOS ties a grant to the program's code signature, so a build signed
//! ad hoc (every local build) has to be granted again after it is rebuilt.

/// A permission the system may withhold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    Accessibility,
    ScreenRecording,
    InputMonitoring,
}

impl Permission {
    /// The permissions this system asks the user for, in the order they
    /// matter.
    pub fn all() -> &'static [Permission] {
        if cfg!(target_os = "macos") {
            &[Permission::Accessibility, Permission::ScreenRecording, Permission::InputMonitoring]
        } else {
            &[]
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Granted,
    /// Not granted (yet): denied, or never asked.
    Missing,
    /// This system does not gate it.
    NotNeeded,
}

/// Whether `p` is granted now. Cheap: fine to call every second or so.
pub fn status(p: Permission) -> Status {
    imp::status(p)
}

/// Ask for `p`: shows the system's own prompt where there is one (each
/// prompt appears at most once per launch; after a refusal the user has to
/// grant it in System Settings, see [`open_settings`]). Returns the status
/// right after asking, which for a fresh prompt is still `Missing`.
pub fn request(p: Permission) -> Status {
    imp::request(p)
}

/// Open the system's settings at the page where `p` is granted.
pub fn open_settings(p: Permission) -> crate::Result<()> {
    imp::open_settings(p)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Permission, Status};
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type Boolean = u8;

    #[repr(C)]
    struct CFDictionaryCallBacks {
        _private: [u8; 0],
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> Boolean;
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
        static kAXTrustedCheckOptionPrompt: CFTypeRef;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        // macOS 10.15 and later, which is also the oldest system Glossa runs on.
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: CFTypeRef;
        static kCFTypeDictionaryKeyCallBacks: CFDictionaryCallBacks;
        static kCFTypeDictionaryValueCallBacks: CFDictionaryCallBacks;
        fn CFDictionaryCreate(
            alloc: *const c_void,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            count: isize,
            key_callbacks: *const CFDictionaryCallBacks,
            value_callbacks: *const CFDictionaryCallBacks,
        ) -> CFDictionaryRef;
        fn CFRelease(cf: CFTypeRef);
    }

    fn granted(yes: bool) -> Status {
        if yes {
            Status::Granted
        } else {
            Status::Missing
        }
    }

    pub fn status(p: Permission) -> Status {
        // SAFETY: plain queries with no arguments.
        granted(unsafe {
            match p {
                Permission::Accessibility => AXIsProcessTrusted() != 0,
                Permission::ScreenRecording => CGPreflightScreenCaptureAccess(),
                Permission::InputMonitoring => CGPreflightListenEventAccess(),
            }
        })
    }

    pub fn request(p: Permission) -> Status {
        match p {
            Permission::Accessibility => {
                // SAFETY: a one-entry CF dictionary built from the framework's
                // own constants, released after use.
                unsafe {
                    let keys = [kAXTrustedCheckOptionPrompt];
                    let values = [kCFBooleanTrue];
                    let options = CFDictionaryCreate(
                        std::ptr::null(),
                        keys.as_ptr(),
                        values.as_ptr(),
                        1,
                        &kCFTypeDictionaryKeyCallBacks,
                        &kCFTypeDictionaryValueCallBacks,
                    );
                    if options.is_null() {
                        return status(p);
                    }
                    let trusted = AXIsProcessTrustedWithOptions(options) != 0;
                    CFRelease(options);
                    granted(trusted)
                }
            }
            // SAFETY: no arguments; shows the prompt the first time only.
            Permission::ScreenRecording => granted(unsafe { CGRequestScreenCaptureAccess() }),
            // SAFETY: as above.
            Permission::InputMonitoring => granted(unsafe { CGRequestListenEventAccess() }),
        }
    }

    pub fn open_settings(p: Permission) -> crate::Result<()> {
        let pane = match p {
            Permission::Accessibility => "Privacy_Accessibility",
            Permission::ScreenRecording => "Privacy_ScreenCapture",
            Permission::InputMonitoring => "Privacy_ListenEvent",
        };
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| crate::platform_err(format!("opening System Settings: {e}")))
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Permission, Status};

    pub fn status(_p: Permission) -> Status {
        Status::NotNeeded
    }

    pub fn request(_p: Permission) -> Status {
        Status::NotNeeded
    }

    pub fn open_settings(_p: Permission) -> crate::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_macos_gates_anything() {
        if cfg!(target_os = "macos") {
            assert_eq!(Permission::all().len(), 3);
        } else {
            assert!(Permission::all().is_empty());
            assert_eq!(status(Permission::Accessibility), Status::NotNeeded);
            assert_eq!(request(Permission::ScreenRecording), Status::NotNeeded);
        }
    }
}
