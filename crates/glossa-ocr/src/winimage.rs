//! Decoding an image into BGRA pixels with the Windows imaging API (WIC
//! through WinRT), for the two Windows engines. PNG, JPEG, BMP, GIF and TIFF
//! need nothing installed; WebP needs the Store's WebP extension.

use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
    ExifOrientationMode,
};
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use glossa_core::{Error, Result};

use crate::layout;

/// Joins the calling thread to the multithreaded apartment for as long as it
/// lives, which WinRT calls need. Engine jobs run on short-lived worker
/// threads, so the join is undone when the job ends.
pub struct Apartment {
    joined: bool,
}

impl Apartment {
    pub fn enter() -> Apartment {
        keep_mta();
        // S_OK and S_FALSE both need a matching CoUninitialize; failure
        // (the thread is already single-threaded) needs none and still lets
        // WinRT work on this thread.
        let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        Apartment { joined }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.joined {
            unsafe { CoUninitialize() };
        }
    }
}

/// Keep the process's multithreaded apartment alive for good, once. WinRT
/// caches its class factories process-wide; when the last thread in the MTA
/// leaves it (a guard's CoUninitialize), the MTA and the objects in it go,
/// and the next call through a cached factory crashes. A permanent MTA usage
/// count keeps them valid whatever threads come and go.
pub(crate) fn keep_mta() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: no arguments to get wrong; the cookie is deliberately never
        // given back (CoDecrementMTAUsage), which is what keeps the MTA.
        if let Err(e) = unsafe { windows::Win32::System::Com::CoIncrementMTAUsage() } {
            tracing::debug!("CoIncrementMTAUsage: {e}");
        }
    });
}

/// Pixels ready for an engine: BGRA, opaque, rows packed.
pub struct Bgra {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// How much the picture was scaled for the engine (1.0 when not).
    pub scale: f64,
    /// The size of the picture as it was passed.
    pub source: (u32, u32),
}

/// Decode `bytes`, scaled as [`layout::scale_factor`] says for `max_side`
/// and `min_height`. `who` names the engine in errors.
pub fn decode(bytes: &[u8], max_side: u32, min_height: u32, who: &str) -> Result<Bgra> {
    let fail = |what: &str, e: windows::core::Error| Error::Module {
        module: who.into(),
        detail: format!("{what}: {} (0x{:08X})", e.message(), e.code().0),
    };
    let stream = InMemoryRandomAccessStream::new().map_err(|e| fail("cannot create a memory stream", e))?;
    let writer = DataWriter::new().map_err(|e| fail("cannot create a memory stream", e))?;
    writer.WriteBytes(bytes).map_err(|e| fail("cannot copy the image", e))?;
    let buffer = writer.DetachBuffer().map_err(|e| fail("cannot copy the image", e))?;
    stream.WriteAsync(&buffer).and_then(|op| op.join()).map_err(|e| fail("cannot copy the image", e))?;
    stream.Seek(0).map_err(|e| fail("cannot copy the image", e))?;
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .and_then(|op| op.join())
        .map_err(|e| fail("the image format is not one Windows can read", e))?;
    let (w, h) = (
        decoder.OrientedPixelWidth().map_err(|e| fail("cannot read the image size", e))?,
        decoder.OrientedPixelHeight().map_err(|e| fail("cannot read the image size", e))?,
    );
    if w == 0 || h == 0 {
        return Err(Error::Module { module: who.into(), detail: "the image is empty".into() });
    }
    let transform = BitmapTransform::new().map_err(|e| fail("cannot decode the image", e))?;
    let (mut width, mut height) = (w, h);
    let scale = layout::scale_factor(w, h, max_side, min_height);
    if scale != 1.0 {
        width = ((w as f64 * scale).floor() as u32).max(1);
        height = ((h as f64 * scale).floor() as u32).max(1);
        transform.SetInterpolationMode(BitmapInterpolationMode::Fant).map_err(|e| fail("cannot scale", e))?;
        // The scaled size is given in the decoder's unrotated frame.
        let (sw, sh) = if decoder.PixelWidth().ok() == Some(w) { (width, height) } else { (height, width) };
        transform
            .SetScaledWidth(sw)
            .and_then(|_| transform.SetScaledHeight(sh))
            .map_err(|e| fail("cannot scale", e))?;
    }
    let data = decoder
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Straight,
            &transform,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )
        .and_then(|op| op.join())
        .and_then(|p| p.DetachPixelData())
        .map_err(|e| fail("cannot decode the image", e))?;
    let mut pixels = data.to_vec();
    if pixels.len() != width as usize * height as usize * 4 {
        return Err(Error::Module {
            module: who.into(),
            detail: format!("the decoder returned {} bytes for {width}×{height} pixels", pixels.len()),
        });
    }
    layout::flatten_on_white(&mut pixels);
    Ok(Bgra { width, height, pixels, scale, source: (w, h) })
}
