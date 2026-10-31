//! The backends behind monitor enumeration and screen capture.
//!
//! - **Windows, macOS**: `xcap`.
//! - **Linux, X11**: our own x11rb code — RandR for the monitors (with the
//!   work area from `_NET_WORKAREA`), `GetImage` on the root window for the
//!   pixels, EWMH properties for the active window. It needs no system
//!   libraries beyond libxcb, unlike xcap, which links PipeWire.
//! - **Linux, Wayland**: `xcap` when built with the `wayland-capture`
//!   feature; otherwise the `grim` command (wlroots compositors, Hyprland),
//!   and an explanatory error where that is missing too.

use glossa_core::ocr::Rect;
#[cfg(target_os = "linux")]
use image::RgbaImage;

use crate::capture::MonitorShot;
use crate::window::Monitor;
use crate::Result;

/// Whether two rectangles overlap (touching edges do not count).
pub(crate) fn intersects(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

/// Every monitor, from the capture backend's point of view. (Windows has
/// its own richer enumeration in `window`.)
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn monitors() -> Result<Vec<Monitor>> {
    #[cfg(target_os = "linux")]
    {
        if !crate::Session::detect().is_wayland() {
            return x11::monitors();
        }
        #[cfg(feature = "wayland-capture")]
        {
            xcap_backend::monitors()
        }
        #[cfg(not(feature = "wayland-capture"))]
        {
            Ok(Vec::new())
        }
    }
    #[cfg(any(windows, target_os = "macos"))]
    {
        xcap_backend::monitors()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Err(crate::platform_err("monitor enumeration is not implemented on this platform"))
    }
}

/// Capture the monitors that overlap `within` (all of them for `None`).
pub(crate) fn capture(within: Option<Rect>) -> Result<Vec<MonitorShot>> {
    #[cfg(target_os = "linux")]
    {
        if !crate::Session::detect().is_wayland() {
            return x11::capture(within);
        }
        #[cfg(feature = "wayland-capture")]
        {
            xcap_backend::capture(within)
        }
        #[cfg(not(feature = "wayland-capture"))]
        {
            let _ = within;
            let image = grim::capture(None)?;
            let (w, h) = image.dimensions();
            let monitor = Monitor {
                x: 0,
                y: 0,
                width: w,
                height: h,
                scale: 1.0,
                work_area: Rect::new(0.0, 0.0, w as f32, h as f32),
                is_primary: true,
            };
            Ok(vec![MonitorShot { monitor, image }])
        }
    }
    #[cfg(any(windows, target_os = "macos"))]
    {
        xcap_backend::capture(within)
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = within;
        Err(crate::platform_err("screen capture is not implemented on this platform"))
    }
}

/// A backend that can grab an arbitrary rectangle directly, cheaper than
/// capturing whole monitors and cropping. `None` when there is none.
#[cfg(target_os = "linux")]
pub(crate) fn capture_rect_direct(rect: Rect) -> Option<Result<RgbaImage>> {
    if !crate::Session::detect().is_wayland() {
        return Some(x11::capture_rect(rect));
    }
    #[cfg(not(feature = "wayland-capture"))]
    {
        Some(grim::capture(Some(rect)))
    }
    #[cfg(feature = "wayland-capture")]
    {
        None
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn capture_rect_direct(_rect: Rect) -> Option<Result<image::RgbaImage>> {
    None
}

#[cfg(any(windows, target_os = "macos", all(target_os = "linux", feature = "wayland-capture")))]
mod xcap_backend {
    use glossa_core::ocr::Rect;

    use crate::capture::MonitorShot;
    use crate::window::Monitor;
    use crate::{platform_err, Result};

    fn err(e: xcap::XCapError) -> glossa_core::Error {
        platform_err(format!("screen capture failed: {e}"))
    }

    /// xcap does not know the work area, so it is the full bounds.
    fn info(m: &xcap::Monitor) -> Monitor {
        let (x, y) = (m.x().unwrap_or(0), m.y().unwrap_or(0));
        let (width, height) = (m.width().unwrap_or(0), m.height().unwrap_or(0));
        Monitor {
            x,
            y,
            width,
            height,
            scale: m.scale_factor().unwrap_or(1.0),
            work_area: Rect::new(x as f32, y as f32, width as f32, height as f32),
            is_primary: m.is_primary().unwrap_or(false),
        }
    }

    #[cfg_attr(windows, allow(dead_code))]
    pub fn monitors() -> Result<Vec<Monitor>> {
        Ok(xcap::Monitor::all().map_err(err)?.iter().map(info).collect())
    }

    pub fn capture(within: Option<Rect>) -> Result<Vec<MonitorShot>> {
        let mut shots = Vec::new();
        for m in xcap::Monitor::all().map_err(err)? {
            let monitor = info(&m);
            if within.is_none_or(|r| super::intersects(monitor.bounds(), r)) {
                shots.push(MonitorShot { monitor, image: m.capture_image().map_err(err)? });
            }
        }
        Ok(shots)
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod x11 {
    use glossa_core::ocr::Rect;
    use image::RgbaImage;
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, Window};
    use x11rb::rust_connection::RustConnection;

    use crate::capture::MonitorShot;
    use crate::window::{Monitor, WindowInfo};
    use crate::{platform_err, Result};

    fn x_err(e: impl std::fmt::Display) -> glossa_core::Error {
        platform_err(format!("X11: {e}"))
    }

    fn connect() -> Result<(RustConnection, Window)> {
        let (conn, screen) = x11rb::connect(None).map_err(x_err)?;
        let root =
            conn.setup().roots.get(screen).map(|s| s.root).ok_or_else(|| x_err("no screen"))?;
        Ok((conn, root))
    }

    fn atom(conn: &RustConnection, name: &str) -> Option<u32> {
        conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom)
    }

    fn prop32(
        conn: &RustConnection,
        win: Window,
        prop: u32,
        ty: u32,
        len: u32,
    ) -> Option<Vec<u32>> {
        let reply = conn.get_property(false, win, prop, ty, 0, len).ok()?.reply().ok()?;
        let values: Vec<u32> = reply.value32()?.collect();
        Some(values)
    }

    fn prop_bytes(conn: &RustConnection, win: Window, prop: u32, ty: u32) -> Option<Vec<u8>> {
        let reply = conn.get_property(false, win, prop, ty, 0, 4096).ok()?.reply().ok()?;
        (!reply.value.is_empty()).then_some(reply.value)
    }

    pub fn monitors() -> Result<Vec<Monitor>> {
        let (conn, root) = connect()?;
        let mut out: Vec<Monitor> = match conn.randr_get_monitors(root, true).map(|c| c.reply()) {
            Ok(Ok(reply)) => reply
                .monitors
                .iter()
                .map(|m| {
                    bounds_monitor(
                        m.x as i32,
                        m.y as i32,
                        m.width as u32,
                        m.height as u32,
                        m.primary,
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        if out.is_empty() {
            // No RandR 1.5: the whole screen is one monitor.
            let s = &conn.setup().roots[0];
            out.push(bounds_monitor(
                0,
                0,
                s.width_in_pixels as u32,
                s.height_in_pixels as u32,
                true,
            ));
        }
        // _NET_WORKAREA is one rectangle per desktop for the whole screen;
        // its overlap with each monitor is that monitor's work area.
        if let Some(net_workarea) = atom(&conn, "_NET_WORKAREA") {
            if let Some(v) = prop32(&conn, root, net_workarea, AtomEnum::CARDINAL.into(), 4) {
                if let [x, y, w, h] = v[..] {
                    let wa = Rect::new(x as f32, y as f32, w as f32, h as f32);
                    for m in &mut out {
                        if let Some(r) = intersection(m.bounds(), wa) {
                            m.work_area = r;
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    fn bounds_monitor(x: i32, y: i32, width: u32, height: u32, primary: bool) -> Monitor {
        Monitor {
            x,
            y,
            width,
            height,
            scale: 1.0,
            work_area: Rect::new(x as f32, y as f32, width as f32, height as f32),
            is_primary: primary,
        }
    }

    pub(crate) fn intersection(a: Rect, b: Rect) -> Option<Rect> {
        let x0 = a.x.max(b.x);
        let y0 = a.y.max(b.y);
        let x1 = (a.x + a.width).min(b.x + b.width);
        let y1 = (a.y + a.height).min(b.y + b.height);
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    pub fn capture(within: Option<Rect>) -> Result<Vec<MonitorShot>> {
        let mut shots = Vec::new();
        for monitor in monitors()? {
            if within.is_none_or(|r| super::intersects(monitor.bounds(), r)) {
                let image = capture_rect(monitor.bounds())?;
                shots.push(MonitorShot { monitor, image });
            }
        }
        Ok(shots)
    }

    /// Grab a rectangle of the root window.
    pub fn capture_rect(rect: Rect) -> Result<RgbaImage> {
        let (conn, root) = connect()?;
        let clamp16 = |v: f32| v.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        let size16 = |v: f32| v.round().clamp(1.0, u16::MAX as f32) as u16;
        let (w, h) = (size16(rect.width), size16(rect.height));
        let reply = conn
            .get_image(ImageFormat::Z_PIXMAP, root, clamp16(rect.x), clamp16(rect.y), w, h, !0)
            .map_err(x_err)?
            .reply()
            .map_err(x_err)?;
        let setup = conn.setup();
        let bpp = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == reply.depth)
            .map(|f| f.bits_per_pixel)
            .unwrap_or(0);
        zpixmap_to_rgba(
            &reply.data,
            w as u32,
            h as u32,
            bpp,
            setup.image_byte_order == ImageOrder::LSB_FIRST,
        )
        .ok_or_else(|| {
            x_err(format!("unsupported screen format: depth {} at {bpp} bpp", reply.depth))
        })
    }

    /// Convert a 32-bit-per-pixel ZPixmap (the format of every 24- and
    /// 32-bit TrueColor screen) to RGBA.
    pub(crate) fn zpixmap_to_rgba(
        data: &[u8],
        w: u32,
        h: u32,
        bpp: u8,
        lsb_first: bool,
    ) -> Option<RgbaImage> {
        if bpp != 32 || data.len() < (w * h * 4) as usize {
            return None;
        }
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for px in data[..(w * h * 4) as usize].chunks_exact(4) {
            let (r, g, b) = if lsb_first { (px[2], px[1], px[0]) } else { (px[1], px[2], px[3]) };
            out.extend_from_slice(&[r, g, b, 255]);
        }
        RgbaImage::from_raw(w, h, out)
    }

    /// The EWMH active window with its class, title and process.
    pub fn active_window() -> Option<WindowInfo> {
        let (conn, root) = connect().ok()?;
        let active = atom(&conn, "_NET_ACTIVE_WINDOW")?;
        let win = *prop32(&conn, root, active, AtomEnum::WINDOW.into(), 1)?.first()?;
        if win == 0 {
            return None;
        }
        let pid = atom(&conn, "_NET_WM_PID")
            .and_then(|a| prop32(&conn, win, a, AtomEnum::CARDINAL.into(), 1))
            .and_then(|v| v.first().copied())
            .unwrap_or(0);
        let utf8 = atom(&conn, "UTF8_STRING");
        let title = atom(&conn, "_NET_WM_NAME")
            .zip(utf8)
            .and_then(|(name, utf8)| prop_bytes(&conn, win, name, utf8))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .or_else(|| {
                // WM_NAME is Latin-1.
                prop_bytes(&conn, win, AtomEnum::WM_NAME.into(), AtomEnum::STRING.into())
                    .map(|b| b.iter().map(|&c| c as char).collect())
            })
            .unwrap_or_default();
        let class_name = prop_bytes(&conn, win, AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into())
            .map(|b| wm_class(&b))
            .unwrap_or_default();
        Some(WindowInfo {
            process_name: process_name(pid).unwrap_or_default(),
            class_name,
            focus_class: String::new(),
            title,
            pid,
        })
    }

    /// WM_CLASS is "instance\0class\0"; the class is the stable part.
    pub(crate) fn wm_class(raw: &[u8]) -> String {
        let mut parts = raw.split(|&b| b == 0).filter(|p| !p.is_empty());
        let instance = parts.next();
        let class = parts.next().or(instance).unwrap_or_default();
        String::from_utf8_lossy(class).into_owned()
    }

    fn process_name(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .or_else(|| {
                std::fs::read_to_string(format!("/proc/{pid}/comm"))
                    .ok()
                    .map(|s| s.trim().to_owned())
            })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn zpixmap_conversion() {
            // One pixel, BGRX little-endian: B=1 G=2 R=3.
            let img = zpixmap_to_rgba(&[1, 2, 3, 0], 1, 1, 32, true).unwrap();
            assert_eq!(img.get_pixel(0, 0).0, [3, 2, 1, 255]);
            // Big-endian XRGB.
            let img = zpixmap_to_rgba(&[0, 3, 2, 1], 1, 1, 32, false).unwrap();
            assert_eq!(img.get_pixel(0, 0).0, [3, 2, 1, 255]);
            assert!(zpixmap_to_rgba(&[0; 3], 1, 1, 24, true).is_none());
            assert!(zpixmap_to_rgba(&[0; 4], 2, 1, 32, true).is_none(), "short data");
        }

        #[test]
        fn wm_class_takes_the_class_part() {
            assert_eq!(wm_class(b"navigator\0firefox\0"), "firefox");
            assert_eq!(wm_class(b"xterm\0"), "xterm");
            assert_eq!(wm_class(b""), "");
        }

        #[test]
        fn work_area_intersection() {
            let m = Rect::new(1920.0, 0.0, 1920.0, 1080.0);
            let wa = Rect::new(0.0, 32.0, 3840.0, 1048.0);
            assert_eq!(intersection(m, wa), Some(Rect::new(1920.0, 32.0, 1920.0, 1048.0)));
            assert_eq!(intersection(m, Rect::new(0.0, 0.0, 100.0, 100.0)), None);
        }
    }
}

/// Wayland without xcap: the `grim` screenshot tool (wlroots-based
/// compositors and Hyprland implement the protocol it uses).
#[cfg(all(target_os = "linux", not(feature = "wayland-capture")))]
mod grim {
    use glossa_core::ocr::Rect;
    use image::RgbaImage;

    use crate::{platform_err, Result};

    pub fn capture(rect: Option<Rect>) -> Result<RgbaImage> {
        let mut cmd = std::process::Command::new("grim");
        cmd.args(["-t", "png"]);
        if let Some(r) = rect {
            cmd.args(["-g", &geometry(r)]);
        }
        cmd.arg("-");
        let out = cmd.output().map_err(|e| {
            platform_err(format!(
                "screen capture on Wayland needs the `grim` tool or a build with the \
                 `wayland-capture` feature ({e})"
            ))
        })?;
        if !out.status.success() {
            return Err(platform_err(format!(
                "grim failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        image::load_from_memory_with_format(&out.stdout, image::ImageFormat::Png)
            .map(|i| i.to_rgba8())
            .map_err(|e| platform_err(format!("grim returned an unreadable image: {e}")))
    }

    /// grim's `-g` syntax: "x,y wxh".
    pub(crate) fn geometry(r: Rect) -> String {
        format!(
            "{},{} {}x{}",
            r.x.round() as i64,
            r.y.round() as i64,
            r.width.round().max(1.0) as i64,
            r.height.round().max(1.0) as i64
        )
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn geometry_syntax() {
            let g = super::geometry(glossa_core::ocr::Rect::new(-10.4, 20.6, 300.0, 0.2));
            assert_eq!(g, "-10,21 300x1");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersects_excludes_touching_edges() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(intersects(a, Rect::new(5.0, 5.0, 10.0, 10.0)));
        assert!(!intersects(a, Rect::new(10.0, 0.0, 10.0, 10.0)));
        assert!(intersects(a, Rect::new(-5.0, -5.0, 6.0, 6.0)));
    }
}
