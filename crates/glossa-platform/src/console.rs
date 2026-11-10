//! The terminal a windowed program was started from.
//!
//! A release `glossa.exe` is a Windows GUI program: it has no console, so
//! what it prints goes nowhere even when it was started from one. Before
//! answering `--help`, `--version` or a mistyped flag it borrows the
//! console of the program that started it. Elsewhere a program started from
//! a terminal already writes to it.

/// Attach to the parent's console, when it has one and this program has
/// none. Does nothing where that is not needed, or when this program has a
/// console already (a debug build).
pub fn attach_parent() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        // Fails when there is no parent console (started from Explorer) or
        // this process has a console already; both are fine as they are.
        // SAFETY: a plain system call with a constant argument.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}
