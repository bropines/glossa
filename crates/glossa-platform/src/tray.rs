//! Whether the system can show a tray icon at all.
//!
//! On Linux the icon is an AppIndicator, a library `tray-icon` opens at run
//! time; when the system has none, its loader panics on the tray thread.
//! Asking first turns that into one line in the log. Windows and macOS
//! always have a tray (the notification area, the menu bar).

/// Why there can be no tray icon here, or `None` when there can be.
pub fn unavailable() -> Option<String> {
    imp::unavailable()
}

#[cfg(target_os = "linux")]
mod imp {
    /// The names `libappindicator-sys` tries, in its order.
    const NAMES: [&str; 4] = [
        "libayatana-appindicator3.so.1",
        "libappindicator3.so.1",
        "libayatana-appindicator3.so",
        "libappindicator3.so",
    ];

    pub fn unavailable() -> Option<String> {
        // SAFETY: these libraries' initialisers only register GObject types;
        // the handle is dropped at once and the indicator loads its own.
        let found = NAMES.iter().any(|name| unsafe { libloading::Library::new(name) }.is_ok());
        (!found).then(|| {
            "no AppIndicator library (libayatana-appindicator3 or libappindicator3) on this system; install one \
             for the tray icon"
                .to_owned()
        })
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    pub fn unavailable() -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_and_macos_always_have_a_tray() {
        if cfg!(any(windows, target_os = "macos")) {
            assert_eq!(super::unavailable(), None);
        }
    }
}
