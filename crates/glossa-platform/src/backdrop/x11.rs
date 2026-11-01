//! Backdrops on Linux under X11: KWin's blur-behind property.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
use x11rb::wrapper::ConnectionExt as _;

use super::{Applied, Backdrop};

/// KWin blurs whatever is behind a window's translucent pixels when the
/// window carries this property; an empty region means "all of it".
/// Other window managers ignore it, and since KWin creates the atom,
/// its absence means nobody is listening.
pub fn apply(window: u32, kind: Backdrop) -> Result<Applied, String> {
    let (conn, _) = x11rb::connect(None).map_err(|e| format!("X11: {e}"))?;
    let atom = conn
        .intern_atom(true, b"_KDE_NET_WM_BLUR_BEHIND_REGION")
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom;
    if kind == Backdrop::None {
        if atom != 0 {
            let _ = conn.delete_property(window, atom);
            let _ = conn.flush();
        }
        return Ok(Applied::Cleared);
    }
    if atom == 0 {
        return Err("this window manager offers no blur (KWin's blur-behind is not available)".into());
    }
    conn.change_property32(PropMode::REPLACE, window, atom, AtomEnum::CARDINAL, &[]).map_err(|e| e.to_string())?;
    conn.flush().map_err(|e| e.to_string())?;
    Ok(Applied::KdeBlur)
}
