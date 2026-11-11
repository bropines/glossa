//! OneOCR: the recogniser inside Windows 11's Snipping Tool, used through
//! the C functions `oneocr.dll` exports.
//!
//! Nothing of Microsoft's is shipped with Glossa. The engine is three files
//! in the user's own Snipping Tool package
//! (`…\WindowsApps\Microsoft.ScreenSketch_<version>_x64__8wekyb3d8bbwe\SnippingTool\`):
//! `oneocr.dll`, the `onnxruntime.dll` it runs on, and the encrypted
//! `oneocr.onemodel`. The package folder is readable but Windows refuses to
//! load a library from it into another program, so the two DLLs are copied
//! once into Glossa's cache folder (`cache/oneocr/<version>/`, about 55 MB,
//! replaced when the Snipping Tool updates); the model is read where it is.
//! A folder set in the options is used as it is instead, with all three
//! files in it.
//!
//! The pipeline is created on first use with its models loaded lazily (a
//! few hundred milliseconds, some 50 MB), recognition runs one image at a
//! time, and the pipeline is released again after a couple of idle minutes
//! so the tray-resident program gives the memory back.
//!
//! The function signatures follow the public reimplementations of this
//! interface (b1tg/win11-oneocr and the projects derived from it; the
//! model key is the one they publish).

use std::path::{Path, PathBuf};

use glossa_core::{Capabilities, Error, OcrRequest, OcrResult, ProviderInfo, Result};
use glossa_modules::{Host, Provider};

pub const ONEOCR_ID: &str = "oneocr";
const NAME: &str = "OneOCR (Snipping Tool)";

/// The Snipping Tool's package family.
pub const PACKAGE_FAMILY: &str = "Microsoft.ScreenSketch_8wekyb3d8bbwe";
/// The engine's files inside the package.
pub const PACKAGE_SUBDIR: &str = "SnippingTool";
pub const DLL: &str = "oneocr.dll";
pub const RUNTIME_DLL: &str = "onnxruntime.dll";
pub const MODEL: &str = "oneocr.onemodel";

pub struct OneOcr {
    info: ProviderInfo,
    /// The user's folder holding the three files; empty finds the Snipping
    /// Tool's.
    folder: String,
    /// Where the DLLs are copied to.
    cache_dir: PathBuf,
}

impl OneOcr {
    pub fn new(folder: impl Into<String>, cache_dir: impl Into<PathBuf>) -> Self {
        let mut info = ProviderInfo::new(ONEOCR_ID, NAME, Capabilities::OCR | Capabilities::OFFLINE);
        info.description = "The offline text recognition of the Windows 11 Snipping Tool, loaded from your own \
                            installation. Reads many scripts without choosing a language."
            .into();
        info.homepage = "https://github.com/bropines/oneocr-onnx-python".into();
        Self { info, folder: folder.into(), cache_dir: cache_dir.into() }
    }
}

impl Provider for OneOcr {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn recognize(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        if host.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        #[cfg(windows)]
        {
            let files = sys::locate(self.folder.trim(), &self.cache_dir)?;
            sys::recognize(&files, req)
        }
        #[cfg(not(windows))]
        {
            let _ = (req, &self.folder, &self.cache_dir);
            Err(Error::Unsupported { provider: NAME.into(), capability: "text recognition outside Windows" })
        }
    }
}

/// Where the engine's files are for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineFiles {
    /// The folder `oneocr.dll` and `onnxruntime.dll` are loaded from.
    pub dll_dir: PathBuf,
    pub model: PathBuf,
}

/// The version in a package full name such as
/// `Microsoft.ScreenSketch_11.2608.41.0_x64__8wekyb3d8bbwe`.
pub fn package_version(full_name: &str) -> Option<[u32; 4]> {
    let v = full_name.split('_').nth(1)?;
    let mut out = [0u32; 4];
    let mut parts = v.split('.');
    for slot in &mut out {
        *slot = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// The newest x64 (or neutral) package among `full_names`.
pub fn newest_package<'a>(full_names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    full_names
        .into_iter()
        .filter(|n| {
            let arch = n.split('_').nth(2).unwrap_or("");
            arch == "x64" || arch == "neutral"
        })
        .filter_map(|n| package_version(n).map(|v| (v, n)))
        .max_by_key(|(v, _)| *v)
        .map(|(_, n)| n)
}

/// Which of the engine's files are missing from `dir`.
pub fn missing_files(dir: &Path, names: &[&'static str]) -> Vec<&'static str> {
    names.iter().copied().filter(|n| !dir.join(n).is_file()).collect()
}

/// Copy `names` from `src` into `dst` unless a file of the same size is
/// already there. Each file is written under a temporary name and renamed,
/// so a second Glossa process never loads a half-written library.
pub fn copy_if_changed(src: &Path, dst: &Path, names: &[&str]) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for name in names {
        let from = src.join(name);
        let to = dst.join(name);
        let len = std::fs::metadata(&from)?.len();
        if std::fs::metadata(&to).map(|m| m.len()).ok() == Some(len) {
            continue;
        }
        let tmp = dst.join(format!("{name}.{}.part", std::process::id()));
        std::fs::copy(&from, &tmp)?;
        if let Err(e) = std::fs::rename(&tmp, &to) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    Ok(())
}

/// Remove the copies made for other Snipping Tool versions. A copy still in
/// use by another Glossa process stays until a later run.
pub fn remove_stale_copies(root: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy() != keep && e.path().is_dir() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(windows)]
mod sys {
    use super::*;
    use crate::layout::{self, RawLine, RawWord, Shape};
    use crate::winimage::{self, Apartment};
    use std::ffi::{c_char, CStr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, MutexGuard};
    use std::time::{Duration, Instant};
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HMODULE};
    use windows::Win32::Storage::Packaging::Appx::{GetPackagePathByFullName, GetPackagesByPackageFamily};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_WITH_ALTERED_SEARCH_PATH};

    /// The key `CreateOcrPipeline` decrypts the model with.
    const MODEL_KEY: &[u8] = b"kj)TGtrK>f]b[Piow.gU+nC@s\"\"\"\"\"\"4\0";
    /// Release the pipeline after this long without a recognition.
    const IDLE: Duration = Duration::from_secs(120);
    /// `RunOcrPipeline` reads at most this many lines (its default is lower).
    const MAX_LINES: i64 = 1000;
    /// Pictures smaller than this on a side are padded: the detector rejects
    /// very small images.
    const MIN_SIDE: u32 = 50;

    fn err(detail: impl Into<String>) -> Error {
        Error::Module { module: ONEOCR_ID.into(), detail: detail.into() }
    }

    // -- finding the files ---------------------------------------------------------

    /// The installed Snipping Tool packages' full names.
    fn package_full_names() -> Vec<String> {
        let family = HSTRING::from(PACKAGE_FAMILY);
        let (mut count, mut len) = (0u32, 0u32);
        let rc = unsafe { GetPackagesByPackageFamily(&family, &mut count, None, &mut len, None) };
        if rc != ERROR_INSUFFICIENT_BUFFER || count == 0 {
            return Vec::new();
        }
        let mut names = vec![PWSTR::null(); count as usize];
        let mut buf = vec![0u16; len as usize];
        let rc = unsafe {
            GetPackagesByPackageFamily(
                &family,
                &mut count,
                Some(names.as_mut_ptr()),
                &mut len,
                Some(PWSTR(buf.as_mut_ptr())),
            )
        };
        if rc != ERROR_SUCCESS {
            return Vec::new();
        }
        names
            .into_iter()
            .take(count as usize)
            .filter(|p| !p.is_null())
            .filter_map(|p| unsafe { p.to_string() }.ok())
            .collect()
    }

    fn package_path(full_name: &str) -> Option<PathBuf> {
        let name = HSTRING::from(full_name);
        let mut len = 0u32;
        let rc = unsafe { GetPackagePathByFullName(&name, &mut len, None) };
        if rc != ERROR_INSUFFICIENT_BUFFER || len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        let rc = unsafe { GetPackagePathByFullName(&name, &mut len, Some(PWSTR(buf.as_mut_ptr()))) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..end])))
    }

    pub(super) fn locate(folder: &str, cache_dir: &Path) -> Result<EngineFiles> {
        if !folder.is_empty() {
            let dir = PathBuf::from(folder);
            let missing = missing_files(&dir, &[DLL, RUNTIME_DLL, MODEL]);
            if !missing.is_empty() {
                return Err(Error::Config(format!(
                    "OneOCR: the folder {} lacks {} (it needs oneocr.dll, onnxruntime.dll and oneocr.onemodel \
                     from the Snipping Tool; leave the folder empty to find them automatically)",
                    dir.display(),
                    missing.join(", ")
                )));
            }
            return Ok(EngineFiles { model: dir.join(MODEL), dll_dir: dir });
        }
        let names = package_full_names();
        let newest = newest_package(names.iter().map(String::as_str)).ok_or_else(|| {
            err("the Snipping Tool is not installed. OneOCR is part of the Windows 11 Snipping Tool \
                 (Microsoft Store, version 11.2409 or newer); install or update it, or set a folder holding \
                 oneocr.dll, onnxruntime.dll and oneocr.onemodel in the OCR options")
        })?;
        let root = package_path(newest).ok_or_else(|| err(format!("cannot find where {newest} is installed")))?;
        let src = root.join(PACKAGE_SUBDIR);
        let missing = missing_files(&src, &[DLL, RUNTIME_DLL, MODEL]);
        if !missing.is_empty() {
            return Err(err(format!(
                "this Snipping Tool ({newest}) has no OCR engine ({} missing from {}); update it from the \
                 Microsoft Store",
                missing.join(", "),
                src.display()
            )));
        }
        let version = package_version(newest).map(|v| v.map(|n| n.to_string()).join(".")).unwrap_or_default();
        let copies = cache_dir.join("oneocr");
        let dll_dir = copies.join(&version);
        copy_if_changed(&src, &dll_dir, &[RUNTIME_DLL, DLL])
            .map_err(|e| err(format!("cannot copy the engine from {} to {}: {e}", src.display(), dll_dir.display())))?;
        remove_stale_copies(&copies, &version);
        Ok(EngineFiles { dll_dir, model: src.join(MODEL) })
    }

    // -- the library ---------------------------------------------------------------

    #[repr(C)]
    struct Image {
        kind: i32,
        width: i32,
        height: i32,
        reserved: i32,
        step: i64,
        data: *const u8,
    }

    #[repr(C)]
    struct BoundingBox {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x3: f32,
        y3: f32,
        x4: f32,
        y4: f32,
    }

    /// Pixel layout of [`Image::kind`]: 8-bit BGRA.
    const IMAGE_BGRA: i32 = 3;

    type Handle = i64;
    type Status = i64;

    /// The exported functions. The library is never unloaded: a pipeline's
    /// threads may outlive it, and the second load costs nothing.
    struct Api {
        create_init_options: unsafe extern "C" fn(*mut Handle) -> Status,
        init_options_set_delay_load: unsafe extern "C" fn(Handle, c_char) -> Status,
        create_pipeline: unsafe extern "C" fn(*const c_char, *const c_char, Handle, *mut Handle) -> Status,
        create_process_options: unsafe extern "C" fn(*mut Handle) -> Status,
        process_options_set_max_lines: unsafe extern "C" fn(Handle, i64) -> Status,
        run_pipeline: unsafe extern "C" fn(Handle, *const Image, Handle, *mut Handle) -> Status,
        line_count: unsafe extern "C" fn(Handle, *mut i64) -> Status,
        line: unsafe extern "C" fn(Handle, i64, *mut Handle) -> Status,
        line_content: unsafe extern "C" fn(Handle, *mut *const c_char) -> Status,
        line_box: unsafe extern "C" fn(Handle, *mut *const BoundingBox) -> Status,
        word_count: unsafe extern "C" fn(Handle, *mut i64) -> Status,
        word: unsafe extern "C" fn(Handle, i64, *mut Handle) -> Status,
        word_content: unsafe extern "C" fn(Handle, *mut *const c_char) -> Status,
        word_box: unsafe extern "C" fn(Handle, *mut *const BoundingBox) -> Status,
        /// Optional: a version without it still reads text, just without
        /// confidences.
        word_confidence: Option<unsafe extern "C" fn(Handle, *mut f32) -> Status>,
        release_init_options: unsafe extern "C" fn(Handle) -> Status,
        release_pipeline: unsafe extern "C" fn(Handle) -> Status,
        release_process_options: unsafe extern "C" fn(Handle) -> Status,
        release_result: unsafe extern "C" fn(Handle) -> Status,
    }

    fn load_library(path: &Path) -> Result<HMODULE> {
        let wide = HSTRING::from(path.as_os_str());
        unsafe { LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_WITH_ALTERED_SEARCH_PATH) }.map_err(|e| {
            err(format!(
                "cannot load {}: {} (0x{:08X}){}",
                path.display(),
                e.message(),
                e.code().0,
                if e.code().0 as u32 == 0x8007_0005 {
                    "; Windows does not let programs load libraries from the WindowsApps folder: leave the \
                     OneOCR folder empty so Glossa copies them, or copy the three files somewhere else"
                } else {
                    ""
                }
            ))
        })
    }

    /// Reinterpret an exported symbol as the function type `F`.
    ///
    /// # Safety
    /// `F` must be a function pointer type matching the symbol's real
    /// signature.
    unsafe fn fn_cast<F: Copy>(f: unsafe extern "system" fn() -> isize) -> F {
        const { assert!(std::mem::size_of::<F>() == std::mem::size_of::<usize>()) };
        unsafe { std::mem::transmute_copy(&f) }
    }

    impl Api {
        fn load(dir: &Path) -> Result<Api> {
            // The runtime first, from the same folder, so oneocr.dll binds to
            // this copy and not to some other onnxruntime.dll on the path.
            load_library(&dir.join(RUNTIME_DLL))?;
            let module = load_library(&dir.join(DLL))?;
            macro_rules! sym {
                ($name:literal) => {{
                    let f = unsafe { GetProcAddress(module, windows::core::s!($name)) }.ok_or_else(|| {
                        err(format!("{} has no function {} (an unsupported Snipping Tool version?)", DLL, $name))
                    })?;
                    // SAFETY: the signature is the one the library exports
                    // under this name; on x64 there is one calling convention.
                    unsafe { fn_cast(f) }
                }};
            }
            Ok(Api {
                create_init_options: sym!("CreateOcrInitOptions"),
                init_options_set_delay_load: sym!("OcrInitOptionsSetUseModelDelayLoad"),
                create_pipeline: sym!("CreateOcrPipeline"),
                create_process_options: sym!("CreateOcrProcessOptions"),
                process_options_set_max_lines: sym!("OcrProcessOptionsSetMaxRecognitionLineCount"),
                run_pipeline: sym!("RunOcrPipeline"),
                line_count: sym!("GetOcrLineCount"),
                line: sym!("GetOcrLine"),
                line_content: sym!("GetOcrLineContent"),
                line_box: sym!("GetOcrLineBoundingBox"),
                word_count: sym!("GetOcrLineWordCount"),
                word: sym!("GetOcrWord"),
                word_content: sym!("GetOcrWordContent"),
                word_box: sym!("GetOcrWordBoundingBox"),
                // SAFETY: as for `sym!`; the signature is
                // `int64 GetOcrWordConfidence(int64 word, float *out)`.
                word_confidence: unsafe { GetProcAddress(module, windows::core::s!("GetOcrWordConfidence")) }
                    .map(|f| unsafe { fn_cast(f) }),
                release_init_options: sym!("ReleaseOcrInitOptions"),
                release_pipeline: sym!("ReleaseOcrPipeline"),
                release_process_options: sym!("ReleaseOcrProcessOptions"),
                release_result: sym!("ReleaseOcrResult"),
            })
        }
    }

    /// A created pipeline and the options it runs with.
    struct Pipeline {
        pipeline: Handle,
        options: Handle,
    }

    struct Runtime {
        dll_dir: PathBuf,
        api: &'static Api,
        model: PathBuf,
        pipeline: Option<Pipeline>,
        last_used: Instant,
    }

    // SAFETY: the handles are plain integers owned by the library, used only
    // under the mutex.
    unsafe impl Send for Runtime {}

    static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);
    static REAPER: AtomicBool = AtomicBool::new(false);

    fn lock() -> MutexGuard<'static, Option<Runtime>> {
        RUNTIME.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl Runtime {
        fn pipeline(&mut self) -> Result<&Pipeline> {
            if self.pipeline.is_none() {
                let api = self.api;
                let model = std::ffi::CString::new(self.model.to_string_lossy().as_bytes())
                    .map_err(|_| err("the model path contains a NUL"))?;
                let mut init = 0;
                let mut pipeline = 0;
                let mut options = 0;
                unsafe {
                    if (api.create_init_options)(&mut init) != 0 {
                        return Err(err("CreateOcrInitOptions failed"));
                    }
                    // Load each script's recogniser when first needed: a
                    // fraction of the memory and start-up time.
                    (api.init_options_set_delay_load)(init, 1);
                    let rc = (api.create_pipeline)(model.as_ptr(), MODEL_KEY.as_ptr().cast(), init, &mut pipeline);
                    (api.release_init_options)(init);
                    if rc != 0 {
                        return Err(err(format!(
                            "cannot open the model {} (error {rc}); the Snipping Tool may be too old or too new \
                             for this interface",
                            self.model.display()
                        )));
                    }
                    if (api.create_process_options)(&mut options) != 0 {
                        (api.release_pipeline)(pipeline);
                        return Err(err("CreateOcrProcessOptions failed"));
                    }
                    (api.process_options_set_max_lines)(options, MAX_LINES);
                }
                tracing::debug!(model = %self.model.display(), "OneOCR pipeline created");
                self.pipeline = Some(Pipeline { pipeline, options });
            }
            Ok(self.pipeline.as_ref().expect("just created"))
        }

        fn release(&mut self) {
            if let Some(p) = self.pipeline.take() {
                unsafe {
                    (self.api.release_process_options)(p.options);
                    (self.api.release_pipeline)(p.pipeline);
                }
                tracing::debug!("OneOCR pipeline released after being idle");
            }
        }
    }

    /// Release the pipeline once it has been idle for [`IDLE`].
    fn start_reaper() {
        if REAPER.swap(true, Ordering::SeqCst) {
            return;
        }
        let spawned = std::thread::Builder::new().name("oneocr-idle".into()).spawn(|| loop {
            std::thread::sleep(Duration::from_secs(15));
            let mut guard = lock();
            if let Some(rt) = guard.as_mut() {
                if rt.pipeline.is_some() && rt.last_used.elapsed() >= IDLE {
                    rt.release();
                }
            }
        });
        if spawned.is_err() {
            REAPER.store(false, Ordering::SeqCst);
        }
    }

    unsafe fn text_of(ptr: *const c_char) -> String {
        if ptr.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
        }
    }

    unsafe fn shape_of(b: *const BoundingBox) -> Option<Shape> {
        let b = unsafe { b.as_ref() }?;
        Some(Shape::Quad([b.x1, b.y1, b.x2, b.y2, b.x3, b.y3, b.x4, b.y4]))
    }

    /// Read every line and word out of a result.
    unsafe fn read_result(api: &Api, result: Handle) -> Vec<RawLine> {
        let mut lines = Vec::new();
        let mut count = 0i64;
        if unsafe { (api.line_count)(result, &mut count) } != 0 {
            return lines;
        }
        for i in 0..count {
            let mut line = 0;
            if unsafe { (api.line)(result, i, &mut line) } != 0 || line == 0 {
                continue;
            }
            let mut text = std::ptr::null();
            unsafe { (api.line_content)(line, &mut text) };
            let mut bbox = std::ptr::null();
            let shape = if unsafe { (api.line_box)(line, &mut bbox) } == 0 { unsafe { shape_of(bbox) } } else { None };
            let mut words = Vec::new();
            let mut n = 0i64;
            if unsafe { (api.word_count)(line, &mut n) } == 0 {
                for j in 0..n {
                    let mut word = 0;
                    if unsafe { (api.word)(line, j, &mut word) } != 0 || word == 0 {
                        continue;
                    }
                    let mut wtext = std::ptr::null();
                    unsafe { (api.word_content)(word, &mut wtext) };
                    let mut wbox = std::ptr::null();
                    let wshape =
                        if unsafe { (api.word_box)(word, &mut wbox) } == 0 { unsafe { shape_of(wbox) } } else { None };
                    let confidence = api.word_confidence.and_then(|f| {
                        let mut c = 0f32;
                        (unsafe { f(word, &mut c) } == 0 && c.is_finite()).then_some(c.clamp(0.0, 1.0))
                    });
                    words.push(RawWord { text: unsafe { text_of(wtext) }, shape: wshape, confidence });
                }
            }
            lines.push(RawLine { text: unsafe { text_of(text) }, shape, words });
        }
        lines
    }

    pub(super) fn recognize(files: &EngineFiles, req: &OcrRequest) -> Result<OcrResult> {
        let bytes = req.image.to_bytes()?;
        let img = {
            let _apartment = Apartment::enter();
            winimage::decode(&bytes, 0, 0, ONEOCR_ID)?
        };
        let (width, height, pixels) = layout::pad_to_min(img.width, img.height, img.pixels, MIN_SIDE);

        let mut guard = lock();
        if guard.as_ref().is_some_and(|rt| rt.dll_dir != files.dll_dir || rt.model != files.model) {
            // Another folder or a Snipping Tool update: start over. The old
            // library stays loaded; a new folder loads its own.
            if let Some(mut old) = guard.take() {
                old.release();
            }
        }
        if guard.is_none() {
            let api: &'static Api = Box::leak(Box::new(Api::load(&files.dll_dir)?));
            *guard = Some(Runtime {
                dll_dir: files.dll_dir.clone(),
                api,
                model: files.model.clone(),
                pipeline: None,
                last_used: Instant::now(),
            });
        }
        let rt = guard.as_mut().expect("just set");
        rt.last_used = Instant::now();
        let api = rt.api;
        let (pipeline, options) = {
            let p = rt.pipeline()?;
            (p.pipeline, p.options)
        };
        let image = Image {
            kind: IMAGE_BGRA,
            width: width as i32,
            height: height as i32,
            reserved: 0,
            step: width as i64 * 4,
            data: pixels.as_ptr(),
        };
        let mut result = 0;
        let rc = unsafe { (api.run_pipeline)(pipeline, &image, options, &mut result) };
        if rc != 0 || result == 0 {
            return Err(err(format!("the engine could not read the picture (error {rc})")));
        }
        let lines = unsafe { read_result(api, result) };
        unsafe { (api.release_result)(result) };
        rt.last_used = Instant::now();
        drop(guard);
        start_reaper();
        Ok(layout::build_result(&lines, req, (img.width, img.height)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_out_of_a_package_name() {
        assert_eq!(package_version("Microsoft.ScreenSketch_11.2608.41.0_x64__8wekyb3d8bbwe"), Some([11, 2608, 41, 0]));
        assert_eq!(package_version("Microsoft.ScreenSketch_8wekyb3d8bbwe"), None);
        assert_eq!(package_version("Microsoft.ScreenSketch_11.2_x64__8wekyb3d8bbwe"), None);
    }

    #[test]
    fn picks_the_newest_x64_package() {
        let names = [
            "Microsoft.ScreenSketch_11.2409.25.0_x64__8wekyb3d8bbwe",
            "Microsoft.ScreenSketch_11.2608.41.0_arm64__8wekyb3d8bbwe",
            "Microsoft.ScreenSketch_11.2510.3.0_x64__8wekyb3d8bbwe",
            "garbage",
        ];
        assert_eq!(newest_package(names), Some("Microsoft.ScreenSketch_11.2510.3.0_x64__8wekyb3d8bbwe"));
        assert_eq!(newest_package([]), None);
    }

    #[test]
    fn copies_once_and_cleans_up_other_versions() {
        let base = std::env::temp_dir().join(format!("glossa-oneocr-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(DLL), b"dll").unwrap();
        std::fs::write(src.join(RUNTIME_DLL), b"runtime").unwrap();
        assert_eq!(missing_files(&src, &[DLL, RUNTIME_DLL, MODEL]), [MODEL]);

        let copies = base.join("copies");
        let dst = copies.join("2");
        std::fs::create_dir_all(copies.join("1")).unwrap();
        copy_if_changed(&src, &dst, &[RUNTIME_DLL, DLL]).unwrap();
        assert_eq!(std::fs::read(dst.join(DLL)).unwrap(), b"dll");
        // Same size: left alone (a loaded library cannot be replaced anyway).
        std::fs::write(src.join(DLL), b"DLL").unwrap();
        copy_if_changed(&src, &dst, &[DLL]).unwrap();
        assert_eq!(std::fs::read(dst.join(DLL)).unwrap(), b"dll");
        // Another size: replaced, with no temporary file left behind.
        std::fs::write(src.join(DLL), b"newer dll").unwrap();
        copy_if_changed(&src, &dst, &[DLL]).unwrap();
        assert_eq!(std::fs::read(dst.join(DLL)).unwrap(), b"newer dll");
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 2);

        remove_stale_copies(&copies, "2");
        assert!(!copies.join("1").exists() && dst.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn describes_itself_as_an_ocr_provider() {
        let p = OneOcr::new("", std::env::temp_dir());
        assert_eq!(p.info().id, ONEOCR_ID);
        assert!(p.info().capabilities.contains(Capabilities::OCR));
    }

    /// `GLOSSA_OCR_SAMPLE=picture.png`: recognise it twice, cold then warm.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs Windows 11 with the Snipping Tool and GLOSSA_OCR_SAMPLE"]
    fn recognises_a_sample_live() {
        use glossa_core::ocr::{ImageSource, OcrGranularity};
        let path = std::env::var_os("GLOSSA_OCR_SAMPLE").expect("set GLOSSA_OCR_SAMPLE");
        let cache = std::env::temp_dir().join("glossa-oneocr-live");
        let p = OneOcr::new("", &cache);
        let mut req = OcrRequest::new(ImageSource::Path(path.into()));
        req.granularity = OcrGranularity::Words;
        let http = glossa_net::mock::MockHttp::new();
        let opts = glossa_modules::OptionValues::new();
        for round in ["cold", "warm"] {
            let started = std::time::Instant::now();
            let r = glossa_modules::provider::with_test_host(&http, &opts, |h| p.recognize(h, &req)).unwrap();
            println!("{round}: {} ms\n{}", started.elapsed().as_millis(), r.text);
            for line in &r.blocks {
                println!("  {:?} angle {:.3} words {}", line.bounds, line.angle, line.children.len());
            }
            assert!(!r.text.trim().is_empty());
            assert!(r.blocks.iter().all(|b| b.bounds.is_some() && !b.children.is_empty()));
        }
    }
}
