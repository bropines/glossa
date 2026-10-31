//! The system clipboard, plus the save-and-restore dance that lets us borrow
//! it for a synthetic copy or paste without the user losing what they had on
//! it.
//!
//! Text and images go through `arboard` on every platform. On Windows the
//! snapshot additionally copies every memory-backed clipboard format
//! verbatim (rich text, HTML, file lists, application formats), because a
//! text-only restore would silently destroy a copied file or a spreadsheet
//! range — and the user only finds out when they paste.
//!
//! On X11 and Wayland the clipboard is served by its owner: text we put there
//! lives only as long as a [`Clipboard`] in this process does (or until a
//! clipboard manager takes it over). Keep one alive for the program's
//! lifetime.

use std::borrow::Cow;

use image::RgbaImage;

use crate::{platform_err, Result};

/// A handle to the system clipboard.
pub struct Clipboard {
    inner: arboard::Clipboard,
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Clipboard")
    }
}

/// What was on the clipboard at one moment, as far as we could read it.
#[derive(Debug, Clone, Default)]
pub struct ClipboardSnapshot {
    text: Option<String>,
    image: Option<RgbaImage>,
    /// Every memory-backed format, verbatim (Windows only).
    #[cfg(windows)]
    native: Option<Vec<(u32, Vec<u8>)>>,
}

impl ClipboardSnapshot {
    /// The text that was on the clipboard, if any.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Whether nothing restorable was captured. On Linux and macOS a
    /// clipboard holding only files or rich content also looks empty.
    pub fn is_empty(&self) -> bool {
        #[cfg(windows)]
        if self.native.as_ref().is_some_and(|n| !n.is_empty()) {
            return false;
        }
        self.text.is_none() && self.image.is_none()
    }
}

fn err(what: &str, e: arboard::Error) -> glossa_core::Error {
    platform_err(format!("clipboard: cannot {what}: {e}"))
}

impl Clipboard {
    /// Open the clipboard. Fails without a display (a TTY, WSL without
    /// WSLg).
    pub fn new() -> Result<Self> {
        arboard::Clipboard::new()
            .map(|inner| Clipboard { inner })
            .map_err(|e| err("open the clipboard", e))
    }

    /// The clipboard's text; `None` when it is empty, holds no text, or
    /// cannot be read.
    pub fn get_text(&mut self) -> Option<String> {
        match self.inner.get_text() {
            Ok(t) if !t.is_empty() => Some(t),
            Ok(_) => None,
            Err(arboard::Error::ContentNotAvailable) => None,
            Err(e) => {
                tracing::debug!("clipboard read failed: {e}");
                None
            }
        }
    }

    pub fn set_text(&mut self, text: &str) -> Result<()> {
        self.inner.set_text(text).map_err(|e| err("set text", e))
    }

    /// The clipboard's image, decoded to RGBA.
    pub fn get_image(&mut self) -> Option<RgbaImage> {
        let img = self.inner.get_image().ok()?;
        RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
    }

    pub fn set_image(&mut self, image: &RgbaImage) -> Result<()> {
        let data = arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: Cow::Borrowed(image.as_raw()),
        };
        self.inner.set_image(data).map_err(|e| err("set an image", e))
    }

    pub fn clear(&mut self) -> Result<()> {
        self.inner.clear().map_err(|e| err("clear", e))
    }

    /// Capture what is on the clipboard now, best effort, for
    /// [`restore`](Self::restore) later.
    pub fn snapshot(&mut self) -> ClipboardSnapshot {
        #[cfg(windows)]
        if let Some(native) = win::snapshot() {
            let text = native
                .iter()
                .find(|(f, _)| *f == win::CF_UNICODETEXT)
                .map(|(_, b)| win::utf16_bytes_to_string(b))
                .filter(|t| !t.is_empty());
            return ClipboardSnapshot { text, image: None, native: Some(native) };
        }
        let text = self.get_text();
        // Decoding a screenshot is expensive; only bother without text.
        let image = if text.is_none() { self.get_image() } else { None };
        ClipboardSnapshot {
            text,
            image,
            #[cfg(windows)]
            native: None,
        }
    }

    /// Put a snapshot back. An empty snapshot clears the clipboard, since an
    /// empty clipboard is what it recorded.
    pub fn restore(&mut self, snapshot: &ClipboardSnapshot) -> Result<()> {
        #[cfg(windows)]
        if let Some(native) = &snapshot.native {
            match win::restore(native) {
                Ok(()) => return Ok(()),
                Err(e) => tracing::debug!("native clipboard restore failed, falling back: {e}"),
            }
        }
        if let Some(text) = &snapshot.text {
            self.set_text(text)
        } else if let Some(image) = &snapshot.image {
            self.set_image(image)
        } else {
            self.clear()
        }
    }

    /// The X11/Wayland PRIMARY selection: the most recently highlighted
    /// text, readable without any keystroke. On Wayland this needs a
    /// compositor with the data-control protocol. Always `None` on Windows
    /// and macOS, which have no such selection.
    pub fn get_primary(&mut self) -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            use arboard::{GetExtLinux, LinuxClipboardKind};
            match self.inner.get().clipboard(LinuxClipboardKind::Primary).text() {
                Ok(t) if !t.is_empty() => Some(t),
                Ok(_) => None,
                Err(e) => {
                    tracing::debug!("PRIMARY selection unavailable: {e}");
                    None
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// The Windows clipboard sequence number, which changes on every
    /// clipboard update — the reliable way to tell that a synthetic Ctrl+C
    /// actually copied something, even when the text is the same as before.
    /// `None` on other platforms.
    pub fn sequence_number() -> Option<u32> {
        #[cfg(windows)]
        {
            // SAFETY: no arguments, no preconditions.
            Some(unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() })
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

#[cfg(windows)]
mod win {
    use std::time::Duration;

    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard,
        SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
    };

    pub const CF_UNICODETEXT: u32 = 13;

    /// Snapshots larger than this keep only the formats that fit; a 4K
    /// screenshot as a DIB is ~33 MB.
    const LIMIT: usize = 64 << 20;

    /// Formats whose handle is not a plain HGLOBAL (GDI objects, metafile
    /// handles wrapped in memory, owner-display and private formats) cannot
    /// be copied byte for byte.
    fn copyable(format: u32) -> bool {
        !matches!(format, 2 | 3 | 9 | 14 | 0x80 | 0x82 | 0x83 | 0x8E | 0x200..=0x3FF)
    }

    pub fn utf16_bytes_to_string(bytes: &[u8]) -> String {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16_lossy(&units)
    }

    /// Closes the clipboard when dropped, so no early return leaves it open
    /// and locked for every other program.
    struct Open;

    impl Drop for Open {
        fn drop(&mut self) {
            // SAFETY: only constructed after OpenClipboard succeeded.
            let _ = unsafe { CloseClipboard() };
        }
    }

    /// Another program may hold the clipboard for a moment; retry briefly.
    fn open() -> Option<Open> {
        for _ in 0..20 {
            // SAFETY: a null owner is allowed; we close it via `Open`.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Open);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    pub fn snapshot() -> Option<Vec<(u32, Vec<u8>)>> {
        let _open = open()?;
        let mut out = Vec::new();
        let mut total = 0usize;
        let mut format = 0u32;
        loop {
            // SAFETY: the clipboard is open on this thread.
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            if !copyable(format) {
                continue;
            }
            // SAFETY: the clipboard is open; the handle stays owned by the
            // clipboard and is only read between GlobalLock/GlobalUnlock.
            unsafe {
                let Ok(handle) = GetClipboardData(format) else { continue };
                let mem = HGLOBAL(handle.0);
                let size = GlobalSize(mem);
                if size == 0 || total + size > LIMIT {
                    continue;
                }
                let ptr = GlobalLock(mem);
                if ptr.is_null() {
                    continue;
                }
                let bytes = std::slice::from_raw_parts(ptr as *const u8, size).to_vec();
                let _ = GlobalUnlock(mem);
                total += size;
                out.push((format, bytes));
            }
        }
        Some(out)
    }

    pub fn restore(formats: &[(u32, Vec<u8>)]) -> Result<(), String> {
        let _open = open().ok_or("the clipboard is busy")?;
        // SAFETY: the clipboard is open on this thread.
        unsafe { EmptyClipboard() }.map_err(|e| e.to_string())?;
        for (format, bytes) in formats {
            // SAFETY: the allocation is sized for `bytes`, written while
            // locked, and either handed to the clipboard (which then owns it)
            // or freed here.
            unsafe {
                let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else { continue };
                let ptr = GlobalLock(mem);
                if ptr.is_null() {
                    let _ = GlobalFree(Some(mem));
                    continue;
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
                let _ = GlobalUnlock(mem);
                if SetClipboardData(*format, Some(HANDLE(mem.0))).is_err() {
                    let _ = GlobalFree(Some(mem));
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn gdi_formats_are_skipped() {
            assert!(copyable(CF_UNICODETEXT));
            assert!(copyable(0xC0F0));
            assert!(!copyable(2));
            assert!(!copyable(0x250));
        }

        #[test]
        fn utf16_decoding_stops_at_nul() {
            let bytes: Vec<u8> = "hé\0junk".encode_utf16().flat_map(u16::to_le_bytes).collect();
            assert_eq!(utf16_bytes_to_string(&bytes), "hé");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_emptiness() {
        assert!(ClipboardSnapshot::default().is_empty());
        let s = ClipboardSnapshot { text: Some("x".into()), ..Default::default() };
        assert!(!s.is_empty());
        assert_eq!(s.text(), Some("x"));
        let s = ClipboardSnapshot { image: Some(RgbaImage::new(1, 1)), ..Default::default() };
        assert!(!s.is_empty());
    }

    #[test]
    #[ignore = "needs a display and touches the real clipboard"]
    fn text_round_trip_and_restore() {
        let mut cb = Clipboard::new().unwrap();
        let before = cb.snapshot();
        cb.set_text("glossa clipboard test").unwrap();
        assert_eq!(cb.get_text().as_deref(), Some("glossa clipboard test"));
        cb.restore(&before).unwrap();
        assert_eq!(cb.get_text().as_deref(), before.text());
    }

    #[test]
    #[ignore = "needs a display and touches the real clipboard"]
    fn image_round_trip() {
        let mut cb = Clipboard::new().unwrap();
        let img = RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        cb.set_image(&img).unwrap();
        assert_eq!(cb.get_image().unwrap().dimensions(), (3, 2));
    }
}
