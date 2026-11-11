//! Getting the text the user has selected in another program, and pasting a
//! replacement over it.
//!
//! There is no portable "give me the selection" call, so each platform gets
//! the least intrusive strategy that works there:
//!
//! - **Windows**: UI Automation first (silent, clipboard untouched); then a
//!   synthetic copy — release the user's still-held hotkey modifiers, save
//!   the clipboard, send Ctrl+C (or Ctrl+Insert), watch
//!   `GetClipboardSequenceNumber` until the target program has copied or the
//!   timeout passes, read the text, restore the clipboard. In a terminal
//!   (`[advanced] terminals`), where Ctrl+C interrupts the running command,
//!   the terminal's own copy action applies instead: none by default.
//! - **X11**: the PRIMARY selection, which is exactly "what is highlighted";
//!   no keystroke at all, so a terminal never receives a Ctrl+C.
//! - **Wayland**: PRIMARY where the compositor offers data-control (KDE,
//!   wlroots, Hyprland; GNOME bridges it for XWayland), otherwise Ctrl+C
//!   through the RemoteDesktop portal. With neither, the selection cannot
//!   be read: the capture fails with a message saying so, unless the user
//!   chose to have the clipboard stand in for it
//!   (`[advanced] clipboard_as_selection`, off by default; the clipboard
//!   may hold anything, and what is captured goes to a translation
//!   service).
//! - **macOS**: Accessibility `AXSelectedText`, then a synthetic Cmd+C with
//!   the clipboard saved and restored. In web content (browsers, Electron
//!   apps) Cmd+C comes first: Chromium's `AXSelectedText` loses the line
//!   breaks between paragraphs and list items.

use std::time::{Duration, Instant};

use glossa_core::config::{is_terminal, Advanced, CopyAction, ExceptionRule};

use crate::Result;
#[cfg(any(windows, target_os = "macos"))]
use crate::{clipboard::Clipboard, input};

/// How long the program we pasted into gets to read the clipboard before we
/// put the user's own content back. Pasting is asynchronous: the target
/// reads the clipboard whenever it gets round to processing the keystroke,
/// and Office or a busy browser can take a few hundred milliseconds.
pub const REPLACE_RESTORE_DELAY: Duration = Duration::from_millis(600);

/// How selection capture should behave; usually built from the options with
/// [`CaptureOptions::from_config`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureOptions {
    /// The keystroke for the synthetic copy, or none at all.
    pub copy_action: CopyAction,
    /// How long to wait for the target program to put the selection on the
    /// clipboard after the synthetic copy.
    pub wait_ms: u64,
    /// Put the user's clipboard back afterwards. Off leaves the copied
    /// selection on the clipboard, as a manual Ctrl+C would.
    pub restore_clipboard: bool,
    /// Terminal windows (`[advanced] terminals`), and the copy action used
    /// there instead of `copy_action`.
    pub terminals: Vec<ExceptionRule>,
    pub terminal_copy_action: CopyAction,
    /// Where the selection cannot be read at all (Wayland without PRIMARY
    /// and without the RemoteDesktop portal), return the clipboard's text
    /// instead of failing (`[advanced] clipboard_as_selection`).
    pub clipboard_as_selection: bool,
}

/// What the user is told where the selection cannot be read and the
/// clipboard was not allowed to stand in for it.
pub const NO_SELECTION_ACCESS: &str = "this desktop gives no way to read the selected text (no PRIMARY selection and \
     no RemoteDesktop portal). Copy the text and use \"Translate clipboard\", or set [advanced] \
     clipboard_as_selection = true to have the selection hotkeys take the clipboard";

/// Without PRIMARY and without the portal: the clipboard's text when the
/// user allowed that, else an error that says what to do. Never the
/// clipboard unasked: it may hold a password or a private message, and
/// whatever this returns is sent to a translation service.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn last_resort(allowed: bool, clipboard: impl FnOnce() -> Option<String>) -> Result<Option<String>> {
    if !allowed {
        return Err(crate::platform_err(NO_SELECTION_ACCESS));
    }
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "the selection cannot be read on this desktop; taking the clipboard instead, as [advanced] \
             clipboard_as_selection asks (copy the text with Ctrl+C before translating)"
        );
    });
    Ok(clipboard())
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self::from_config(&Advanced::default())
    }
}

impl CaptureOptions {
    pub fn from_config(advanced: &Advanced) -> Self {
        Self {
            copy_action: advanced.copy_action,
            wait_ms: advanced.copy_wait_ms,
            restore_clipboard: true,
            terminals: advanced.terminals.clone(),
            terminal_copy_action: advanced.terminal_copy_action,
            clipboard_as_selection: advanced.clipboard_as_selection,
        }
    }

    /// The copy action for a window: the terminal one when the program or
    /// any of `classes` (window, focused control, focused UI Automation
    /// element) is a terminal, else the usual one.
    pub fn copy_action_for(&self, program: &str, classes: &[&str]) -> CopyAction {
        if self.copy_action == CopyAction::AccessibilityOnly {
            return CopyAction::AccessibilityOnly;
        }
        if is_terminal(&self.terminals, program, classes) {
            self.terminal_copy_action
        } else {
            self.copy_action
        }
    }
}

/// The selected text in the foreground program, or `None` when nothing is
/// selected (or it could not be read). Blocks for up to about
/// `opts.wait_ms`; call it off the UI thread.
pub fn capture_selection(opts: &CaptureOptions) -> Result<Option<String>> {
    #[cfg(windows)]
    {
        let focused = crate::uia::focused().unwrap_or_default();
        if let Some(text) = focused.selection {
            tracing::debug!("selection via UI Automation");
            return Ok(Some(text));
        }
        if opts.copy_action == CopyAction::AccessibilityOnly {
            return Ok(None);
        }
        let win = crate::window::foreground_window().unwrap_or_default();
        let action = opts.copy_action_for(&win.process_name, &[&win.class_name, &win.focus_class, &focused.class]);
        if action == CopyAction::AccessibilityOnly {
            tracing::debug!(program = %win.process_name, class = %win.class_name, uia_class = %focused.class,
                "a terminal: no copy keystroke (Ctrl+C would interrupt it)");
            return Ok(None);
        }
        copy_via_clipboard(&CaptureOptions { copy_action: action, ..opts.clone() })
    }
    #[cfg(target_os = "macos")]
    {
        let ax = crate::macos_ax::selected_text();
        let copy_allowed = opts.copy_action != CopyAction::AccessibilityOnly;
        match ax {
            // Web content's AXSelectedText runs paragraphs and list items
            // together; Cmd+C keeps the line breaks. The AX text stays the
            // fallback when copying gives nothing.
            Some(sel) if sel.web && copy_allowed => match copy_via_clipboard(opts) {
                Ok(Some(text)) => {
                    tracing::debug!("selection via Cmd+C (web content)");
                    Ok(Some(text))
                }
                other => {
                    tracing::debug!(?other, "Cmd+C gave nothing in web content; using AXSelectedText");
                    Ok(Some(sel.text))
                }
            },
            Some(sel) => {
                tracing::debug!("selection via AXSelectedText");
                Ok(Some(sel.text))
            }
            None if copy_allowed => copy_via_clipboard(opts),
            None => Ok(None),
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        unix_capture(opts)
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn unix_capture(opts: &CaptureOptions) -> Result<Option<String>> {
    use crate::{clipboard::Clipboard, DisplayServer, Session};

    let session = Session::detect();
    match session.display {
        DisplayServer::X11 => Ok(Clipboard::new()?.get_primary()),
        DisplayServer::Wayland { .. } => {
            let mut cb = Clipboard::new()?;
            if let Some(text) = cb.get_primary() {
                return Ok(Some(text));
            }
            // No readable PRIMARY: copy through the RemoteDesktop portal.
            if opts.copy_action != CopyAction::AccessibilityOnly {
                match wayland_copy(&mut cb, opts) {
                    Ok(text) => return Ok(text),
                    Err(e) => tracing::debug!("no copy through the desktop portal: {e}"),
                }
            }
            // Nothing can read the selection here. The clipboard stands in
            // only when the user asked for that; otherwise say so instead
            // of quietly translating whatever was copied last.
            last_resort(opts.clipboard_as_selection, || cb.get_text())
        }
        _ => Err(crate::platform_err("reading the selection needs a graphical session")),
    }
}

/// Copy the selection with Ctrl+C sent through the RemoteDesktop portal and
/// read it from the clipboard, putting back what was there. An error means
/// the portal could not send the keys (nothing was changed then).
#[cfg(not(any(windows, target_os = "macos")))]
fn wayland_copy(cb: &mut crate::clipboard::Clipboard, opts: &CaptureOptions) -> Result<Option<String>> {
    let snapshot = cb.snapshot();
    let before = cb.get_text();
    crate::input::send_copy(opts.copy_action)?;
    // The clipboard has no change counter here: wait for its text to change.
    // A selection equal to what was already copied reads as that text.
    let deadline = Instant::now() + Duration::from_millis(opts.wait_ms);
    let mut text = None;
    wait_until(deadline, || {
        text = cb.get_text().filter(|t| Some(t) != before.as_ref());
        text.is_some()
    });
    let text = text.or(before);
    if opts.restore_clipboard {
        if let Err(e) = cb.restore(&snapshot) {
            tracing::warn!("could not restore the clipboard after copying: {e}");
        }
    }
    Ok(text)
}

/// The synthetic-copy strategy shared by Windows and macOS.
#[cfg(any(windows, target_os = "macos"))]
fn copy_via_clipboard(opts: &CaptureOptions) -> Result<Option<String>> {
    let mut cb = Clipboard::new()?;
    let snapshot = cb.snapshot();
    input::release_modifiers()?;

    // Arm a change detector. Windows has the clipboard sequence number; on
    // macOS we clear the clipboard and wait for text to appear, which also
    // works when the selection equals what was already there.
    #[cfg(windows)]
    let before = Clipboard::sequence_number();
    #[cfg(not(windows))]
    cb.clear()?;

    let sent = input::send_copy(opts.copy_action);
    #[cfg(not(windows))]
    if sent.is_err() {
        let _ = cb.restore(&snapshot); // we emptied it above
    }
    sent?;

    let deadline = Instant::now() + Duration::from_millis(opts.wait_ms);
    #[cfg(windows)]
    let changed = wait_until(deadline, || Clipboard::sequence_number() != before);
    #[cfg(not(windows))]
    let changed = true; // we emptied it, so restoring is always due

    let text = if changed {
        // The sequence number moves on EmptyClipboard, before the text is
        // set, and the source may still hold the clipboard open: retry the
        // read briefly past the deadline.
        let grace = deadline.max(Instant::now() + Duration::from_millis(100));
        let mut text = None;
        wait_until(grace, || {
            text = cb.get_text();
            text.is_some()
        });
        text
    } else {
        None
    };

    let keep_copy = text.is_some() && !opts.restore_clipboard;
    if changed && !keep_copy {
        if let Err(e) = cb.restore(&snapshot) {
            tracing::warn!("could not restore the clipboard: {e}");
        }
    }
    Ok(text)
}

/// Poll `done` every few milliseconds until it is true or `deadline`
/// passes; returns whether it became true.
fn wait_until(deadline: Instant, mut done: impl FnMut() -> bool) -> bool {
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Replace the selection in the foreground program with `text`: put it on
/// the clipboard, send the paste keystroke, and restore the user's clipboard
/// after [`REPLACE_RESTORE_DELAY`] on a background thread (only if it still
/// holds our text — the user may have copied something new meanwhile).
///
/// Where synthetic input is impossible (Wayland) the text is left on the
/// clipboard and an error says so, so the UI can tell the user to paste it.
pub fn replace_selection(text: &str) -> Result<()> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        // The popup's Replace button activated Glossa on macOS; the paste
        // must go to the program the text was selected in.
        crate::window::give_focus_back(Duration::from_millis(500));
        let mut cb = Clipboard::new()?;
        let snapshot = cb.snapshot();
        cb.set_text(text)?;
        let ours = Clipboard::sequence_number();
        // Let the clipboard settle before the target asks for it.
        std::thread::sleep(Duration::from_millis(20));
        input::release_modifiers()?;
        input::send_paste()?;
        let pasted = text.to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(REPLACE_RESTORE_DELAY);
            let still_ours = match (ours, Clipboard::sequence_number()) {
                (Some(a), Some(b)) => a == b,
                _ => cb.get_text().as_deref() == Some(pasted.as_str()),
            };
            if still_ours {
                if let Err(e) = cb.restore(&snapshot) {
                    tracing::warn!("could not restore the clipboard after pasting: {e}");
                }
            }
        });
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        unix_replace(text)
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn unix_replace(text: &str) -> Result<()> {
    use crate::{clipboard::Clipboard, input, Session};

    let session = Session::detect();
    let mut cb = Clipboard::new()?;
    let snapshot = cb.snapshot();
    let pasted = if session.supports_synthetic_input() {
        None
    } else if session.is_wayland() {
        // Through the desktop's RemoteDesktop portal.
        cb.set_text(text)?;
        std::thread::sleep(Duration::from_millis(20));
        Some(input::send_paste())
    } else {
        Some(Err(crate::platform_err("synthetic input needs a graphical session")))
    };
    if let Some(Err(e)) = &pasted {
        tracing::debug!("no paste into the other program: {e}");
        cb.set_text(text)?;
        // Keep the clipboard object (and so, on X11/XWayland, the data)
        // alive long enough for a clipboard manager to take it over.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(30));
            drop(cb);
        });
        return Err(crate::platform_err(
            "this session does not allow pasting into other programs; the translation is on \
             the clipboard — paste it with Ctrl+V",
        ));
    }
    if pasted.is_none() {
        cb.set_text(text)?;
        std::thread::sleep(Duration::from_millis(20));
        input::release_modifiers()?;
        input::send_paste()?;
    }
    let pasted = text.to_owned();
    std::thread::spawn(move || {
        // On X11 the clipboard is served from this process: `cb` must live
        // until the target has fetched the text.
        std::thread::sleep(REPLACE_RESTORE_DELAY);
        if cb.get_text().as_deref() == Some(pasted.as_str()) {
            if let Err(e) = cb.restore(&snapshot) {
                tracing::warn!("could not restore the clipboard after pasting: {e}");
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_follow_the_config() {
        let mut adv = glossa_core::config::Advanced::default();
        assert_eq!(CaptureOptions::from_config(&adv), CaptureOptions::default());
        adv.copy_action = CopyAction::CtrlInsert;
        adv.copy_wait_ms = 900;
        let o = CaptureOptions::from_config(&adv);
        assert_eq!(o.copy_action, CopyAction::CtrlInsert);
        assert_eq!(o.wait_ms, 900);
        assert!(o.restore_clipboard);
    }

    #[test]
    fn the_clipboard_stands_in_for_the_selection_only_when_asked() {
        assert!(!CaptureOptions::default().clipboard_as_selection, "off unless the user turns it on");
        let adv = Advanced { clipboard_as_selection: true, ..Advanced::default() };
        assert!(CaptureOptions::from_config(&adv).clipboard_as_selection);

        // Not asked: the clipboard is not even looked at, and the error
        // says what to do instead.
        let err = last_resort(false, || panic!("the clipboard must not be read")).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("Translate clipboard") && text.contains("clipboard_as_selection"), "{text}");
        // Asked: what was copied, or nothing.
        assert_eq!(last_resort(true, || Some("copied".into())).unwrap().as_deref(), Some("copied"));
        assert_eq!(last_resort(true, || None).unwrap(), None);
    }

    #[test]
    fn terminals_never_get_ctrl_c() {
        let o = CaptureOptions::default();
        assert_eq!(o.copy_action_for("notepad.exe", &["Notepad", "Edit", ""]), CopyAction::CtrlC);
        for (program, classes) in [
            ("WindowsTerminal.exe", ["CASCADIA_HOSTING_WINDOW_CLASS", "", ""]),
            ("mintty.exe", ["mintty", "", ""]),
            ("Code.exe", ["Chrome_WidgetWin_1", "Chrome_RenderWidgetHostHWND", "xterm-helper-textarea"]),
        ] {
            assert_eq!(o.copy_action_for(program, &classes), CopyAction::AccessibilityOnly, "{program}");
        }
        let mut adv = Advanced { terminal_copy_action: CopyAction::CtrlInsert, ..Advanced::default() };
        let o = CaptureOptions::from_config(&adv);
        assert_eq!(o.copy_action_for("mintty.exe", &[]), CopyAction::CtrlInsert);
        // Accessibility only everywhere means accessibility only in terminals too.
        adv.copy_action = CopyAction::AccessibilityOnly;
        let o = CaptureOptions::from_config(&adv);
        assert_eq!(o.copy_action_for("mintty.exe", &[]), CopyAction::AccessibilityOnly);
    }

    #[test]
    fn wait_until_respects_the_deadline() {
        let start = Instant::now();
        assert!(!wait_until(start + Duration::from_millis(30), || false));
        assert!(start.elapsed() >= Duration::from_millis(30));
        let mut n = 0;
        assert!(wait_until(start + Duration::from_secs(5), || {
            n += 1;
            n == 3
        }));
    }

    #[test]
    #[ignore = "needs a display and a selection in another window"]
    fn capture_from_the_real_desktop() {
        let text = capture_selection(&CaptureOptions::default()).unwrap();
        println!("selection: {text:?}");
    }
}
