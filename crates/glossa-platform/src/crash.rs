//! Last words for a crashing process.
//!
//! Release builds are stripped and abort on panic, so without help a crash
//! leaves nothing usable behind: the panic hook's backtrace is a column of
//! `<unknown>`, and a native fault (an access violation in a driver, a stack
//! overflow) never reaches Rust code at all. Two things fill the gap:
//!
//! * [`stack_trace`] lists the caller's frames as `module+offset`, which can
//!   be resolved later against an unstripped build of the same commit
//!   (`addr2line -e glossa.exe 0x<image base + offset>`). The panic hook logs
//!   it.
//! * [`install`] (Windows) registers an unhandled-exception filter that
//!   appends the exception code, the faulting address as `module+offset`, the
//!   thread and the frames to the log file, and a `SIGABRT` handler for C
//!   code that calls `abort()`. They write straight to the file: no
//!   allocation, no locks, nothing that could itself fail on a broken heap or
//!   an exhausted stack.
//!
//! A Rust `abort` (what a panic becomes in a release build) is a fail-fast
//! that no handler sees; the panic hook has run by then.

use std::path::Path;

/// Start logging native crashes to `log_file` (appended). Call once, early.
/// Does nothing outside Windows.
pub fn install(log_file: &Path) {
    imp::install(log_file);
}

/// The current thread's frames, innermost first, as `module+0xoffset`, one per
/// line, skipping `skip` frames above the caller.
pub fn stack_trace(skip: usize) -> String {
    imp::stack_trace(skip)
}

/// Append `module+0xoffset` for `addr` to `out`, or the bare address when no
/// module contains it.
#[cfg_attr(not(windows), allow(dead_code))]
fn write_frame(out: &mut impl core::fmt::Write, addr: usize, module: Option<(&str, usize)>) -> core::fmt::Result {
    match module {
        Some((name, base)) => write!(out, "{name}+{:#x}", addr - base),
        None => write!(out, "{addr:#x}"),
    }
}

/// A fixed buffer that formats without allocating; output past its end is
/// dropped.
#[cfg_attr(not(windows), allow(dead_code))]
struct StackBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl<const N: usize> StackBuf<N> {
    fn new() -> Self {
        Self { buf: [0; N], len: 0 }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl<const N: usize> core::fmt::Write for StackBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let room = N - self.len;
        let take = s.len().min(room);
        self.buf[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use core::fmt::Write as _;
    use std::fs::File;
    use std::io::Write as _;
    use std::path::Path;
    use std::sync::OnceLock;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::Diagnostics::Debug::{
        RtlCaptureStackBackTrace, SetUnhandledExceptionFilter, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    };
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    use windows::Win32::System::SystemInformation::GetSystemTime;
    use windows::Win32::System::Threading::GetCurrentThreadId;

    use super::{write_frame, StackBuf};

    /// Opened at install time: by the time something crashes, opening a file
    /// may no longer work.
    static LOG: OnceLock<File> = OnceLock::new();

    const SIGABRT: i32 = 22;

    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }

    pub fn install(log_file: &Path) {
        let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(log_file) else { return };
        if LOG.set(file).is_err() {
            return; // installed already
        }
        // SAFETY: both handlers only read their arguments and write to the
        // pre-opened file; they are valid for the life of the process.
        unsafe {
            SetUnhandledExceptionFilter(Some(on_exception));
            signal(SIGABRT, on_abort as *const () as usize);
        }
    }

    /// The module containing `addr`: its file name (without the folder) in
    /// `name`, and its base address.
    fn module_of(addr: usize, name: &mut [u8; 64]) -> Option<(usize, usize)> {
        let mut module = HMODULE::default();
        // SAFETY: FROM_ADDRESS takes the address in place of a name pointer;
        // UNCHANGED_REFCOUNT means nothing needs releasing.
        unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                PCWSTR(addr as *const u16),
                &mut module,
            )
            .ok()?;
        }
        let mut wide = [0u16; 260];
        // SAFETY: the buffer is ours and its length is passed along.
        let n = unsafe { GetModuleFileNameW(Some(module), &mut wide) } as usize;
        let path = &wide[..n.min(wide.len())];
        let file = path.rsplit(|c| *c == u16::from(b'\\') || *c == u16::from(b'/')).next().unwrap_or(path);
        // Module names are ASCII in practice; anything else becomes '?'.
        let mut len = 0;
        for c in file.iter().take(name.len()) {
            name[len] = if *c < 0x80 { *c as u8 } else { b'?' };
            len += 1;
        }
        Some((module.0 as usize, len))
    }

    fn frame(out: &mut impl core::fmt::Write, addr: usize) -> core::fmt::Result {
        let mut name = [0u8; 64];
        match module_of(addr, &mut name) {
            Some((base, len)) => {
                write_frame(out, addr, Some((core::str::from_utf8(&name[..len]).unwrap_or("?"), base)))
            }
            None => write_frame(out, addr, None),
        }
    }

    fn frames(out: &mut impl core::fmt::Write, skip: u32) {
        let mut addrs = [core::ptr::null_mut(); 48];
        // SAFETY: the buffer is ours; the function fills at most its length.
        let n = unsafe { RtlCaptureStackBackTrace(skip + 1, &mut addrs, None) } as usize;
        for (i, a) in addrs[..n].iter().enumerate() {
            let _ = write!(out, "\n  {i:2}: ");
            let _ = frame(out, *a as usize);
        }
    }

    pub fn stack_trace(skip: usize) -> String {
        let mut out = String::new();
        frames(&mut out, skip as u32 + 1);
        out
    }

    /// The start of a log line in the subscriber's format, so the entry sorts
    /// and greps with the rest.
    fn header(out: &mut impl core::fmt::Write) {
        // SAFETY: no preconditions.
        let t = unsafe { GetSystemTime() };
        let _ = write!(
            out,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}000Z ERROR glossa_platform::crash: ",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        );
    }

    fn emit(buf: &StackBuf<8192>) {
        if let Some(mut f) = LOG.get() {
            let _ = f.write_all(buf.as_bytes());
            let _ = f.flush();
        }
    }

    fn describe(code: u32) -> &'static str {
        match code {
            0xC000_0005 => "access violation",
            0xC000_00FD => "stack overflow",
            0xC000_001D => "illegal instruction",
            0xC000_0094 => "integer division by zero",
            0xC000_0409 => "fail-fast (stack buffer overrun or abort)",
            0xC000_0374 => "heap corruption",
            0xC000_0006 => "in-page error",
            0x8000_0003 => "breakpoint",
            0xE06D_7363 => "C++ exception",
            _ => "exception",
        }
    }

    unsafe extern "system" fn on_exception(info: *const EXCEPTION_POINTERS) -> i32 {
        let mut buf = StackBuf::<8192>::new();
        header(&mut buf);
        // SAFETY: Windows passes valid exception pointers to the filter.
        let record = unsafe { info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) };
        match record {
            Some(r) => {
                let code = r.ExceptionCode.0 as u32;
                let _ = write!(buf, "native crash: {} ({code:#010x}) at ", describe(code));
                let _ = frame(&mut buf, r.ExceptionAddress as usize);
                if code == 0xC000_0005 && r.NumberParameters >= 2 {
                    let what = match r.ExceptionInformation[0] {
                        0 => "reading",
                        1 => "writing",
                        8 => "executing",
                        _ => "touching",
                    };
                    let _ = write!(buf, ", {what} {:#x}", r.ExceptionInformation[1]);
                }
            }
            None => {
                let _ = write!(buf, "native crash");
            }
        }
        // SAFETY: no preconditions.
        let _ = write!(buf, " on thread {}", unsafe { GetCurrentThreadId() });
        frames(&mut buf, 0);
        let _ = buf.write_str("\n");
        emit(&buf);
        // Let Windows Error Reporting carry on and write its dump.
        EXCEPTION_CONTINUE_SEARCH
    }

    extern "C" fn on_abort(_sig: i32) {
        let mut buf = StackBuf::<8192>::new();
        header(&mut buf);
        // SAFETY: no preconditions.
        let _ = write!(buf, "abort() called on thread {}", unsafe { GetCurrentThreadId() });
        frames(&mut buf, 0);
        let _ = buf.write_str("\n");
        emit(&buf);
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    pub fn install(_log_file: &Path) {}

    pub fn stack_trace(_skip: usize) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write as _;

    #[test]
    fn frames_are_module_relative() {
        let mut s = String::new();
        write_frame(&mut s, 0x1234_5678, Some(("glossa.exe", 0x1230_0000))).unwrap();
        assert_eq!(s, "glossa.exe+0x45678");
        s.clear();
        write_frame(&mut s, 0xdead, None).unwrap();
        assert_eq!(s, "0xdead");
    }

    #[test]
    fn the_stack_buffer_truncates_instead_of_failing() {
        let mut b = StackBuf::<8>::new();
        b.write_str("0123456789").unwrap();
        assert_eq!(b.as_bytes(), b"01234567");
    }

    #[cfg(windows)]
    #[test]
    fn a_trace_names_this_test_binary() {
        let t = stack_trace(0);
        assert!(t.contains(".exe+0x"), "{t}");
    }
}
