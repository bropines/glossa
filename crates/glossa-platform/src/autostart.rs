//! Starting with the user's session.
//!
//! - **Windows**: a `Glossa` value under
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Task Manager's
//!   "Startup apps" switch is honoured: it records a disabled entry under
//!   `…\Explorer\StartupApproved\Run`, which we read, and clear when the
//!   user turns autostart back on from our options.
//! - **Linux**: `~/.config/autostart/glossa.desktop` (XDG autostart, honoured
//!   by every major desktop).
//! - **macOS**: `~/Library/LaunchAgents/app.glossa.plist` with `RunAtLoad`.
//!
//! The file contents are built by pure functions so they can be tested.

use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;

use crate::Result;

/// The name under which we register, everywhere.
pub const APP_NAME: &str = "Glossa";
/// The macOS launchd label.
pub const LAUNCH_AGENT_LABEL: &str = "app.glossa";

/// Turn autostart on (registering `exe_path args…`) or off.
pub fn set_autostart(enabled: bool, exe_path: &Path, args: &[String]) -> Result<()> {
    #[cfg(windows)]
    {
        win::set(enabled, &windows_command_line(exe_path, args))
    }
    #[cfg(not(windows))]
    {
        let file = autostart_file()?;
        let contents = if cfg!(target_os = "macos") {
            launch_agent_plist(exe_path, args)
        } else {
            desktop_entry(exe_path, args)
        };
        set_file(&file, enabled.then_some(contents.as_str()))
    }
}

/// Whether autostart is currently on.
pub fn is_autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        win::is_enabled()
    }
    #[cfg(not(windows))]
    {
        autostart_file().map(|f| file_enables(&f)).unwrap_or(false)
    }
}

/// Where the autostart file lives on this platform.
#[cfg(not(windows))]
fn autostart_file() -> Result<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = dirs::home_dir().ok_or_else(|| crate::platform_err("no home directory"))?;
        Ok(home.join("Library/LaunchAgents").join(format!("{LAUNCH_AGENT_LABEL}.plist")))
    } else {
        let config =
            dirs::config_dir().ok_or_else(|| crate::platform_err("no configuration directory"))?;
        Ok(config.join("autostart").join("glossa.desktop"))
    }
}

/// Write the file (creating its directory) or remove it.
#[cfg_attr(windows, allow(dead_code))]
fn set_file(file: &Path, contents: Option<&str>) -> Result<()> {
    match contents {
        Some(text) => {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(file, text)?;
        }
        None => match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        },
    }
    Ok(())
}

/// Whether an existing autostart file is in force. A desktop entry can be
/// present yet switched off by the desktop's own startup settings.
#[cfg_attr(windows, allow(dead_code))]
fn file_enables(file: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else { return false };
    !text.lines().map(str::trim).any(|l| {
        l.eq_ignore_ascii_case("Hidden=true")
            || l.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
    })
}

/// The `.desktop` file for XDG autostart.
pub fn desktop_entry(exe_path: &Path, args: &[String]) -> String {
    let mut exec = desktop_exec_arg(&exe_path.to_string_lossy());
    for a in args {
        exec.push(' ');
        exec.push_str(&desktop_exec_arg(a));
    }
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={APP_NAME}\n\
         Comment=Popup translator\n\
         Exec={exec}\n\
         Icon=glossa\n\
         Terminal=false\n\
         Categories=Utility;\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// Quote one `Exec=` argument per the Desktop Entry specification: quote
/// when it contains reserved characters, backslash-escape `"`, `` ` ``, `$`
/// and `\` inside quotes, double `%` (field codes) — and then escape every
/// backslash once more, because the whole value is itself a string whose
/// `\` is an escape character.
fn desktop_exec_arg(arg: &str) -> String {
    const RESERVED: &[char] = &[
        ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(',
        ')', '`',
    ];
    let arg = arg.replace('%', "%%");
    let quoted = if arg.is_empty() || arg.contains(RESERVED) {
        let mut q = String::from("\"");
        for c in arg.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                q.push('\\');
            }
            q.push(c);
        }
        q.push('"');
        q
    } else {
        arg
    };
    quoted.replace('\\', "\\\\")
}

/// The launchd agent for macOS.
pub fn launch_agent_plist(exe_path: &Path, args: &[String]) -> String {
    let mut program_args =
        format!("        <string>{}</string>\n", xml_escape(&exe_path.to_string_lossy()));
    for a in args {
        program_args.push_str(&format!("        <string>{}</string>\n", xml_escape(a)));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20   <key>Label</key>\n\
         \x20   <string>{LAUNCH_AGENT_LABEL}</string>\n\
         \x20   <key>ProgramArguments</key>\n\
         \x20   <array>\n\
         {program_args}\
         \x20   </array>\n\
         \x20   <key>RunAtLoad</key>\n\
         \x20   <true/>\n\
         \x20   <key>ProcessType</key>\n\
         \x20   <string>Interactive</string>\n\
         </dict>\n\
         </plist>\n"
    )
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// The command line stored in the Run key: the executable always quoted
/// (paths under `C:\Program Files` have spaces), arguments quoted by the
/// rules `CommandLineToArgvW` undoes.
pub fn windows_command_line(exe_path: &Path, args: &[String]) -> String {
    let mut line = format!("\"{}\"", exe_path.to_string_lossy());
    for a in args {
        line.push(' ');
        line.push_str(&windows_quote_arg(a));
    }
    line
}

fn windows_quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Backslashes before a quote are doubled, and the quote escaped.
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    // Backslashes before the closing quote are doubled too.
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

#[cfg(windows)]
mod win {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
        RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
    };

    use crate::{platform_err, Result};

    const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    const APPROVED: PCWSTR =
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");
    const VALUE: PCWSTR = w!("Glossa");

    pub fn set(enabled: bool, command: &str) -> Result<()> {
        if enabled {
            let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: the data pointer and byte length describe `wide`,
            // NUL included, as REG_SZ requires.
            let rc = unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    RUN,
                    VALUE,
                    REG_SZ.0,
                    Some(wide.as_ptr().cast()),
                    (wide.len() * 2) as u32,
                )
            };
            if rc != ERROR_SUCCESS {
                return Err(platform_err(format!("cannot write the Run key: error {}", rc.0)));
            }
            // A "disabled in Task Manager" marker would override us; the user
            // just asked for autostart, so drop it.
            // SAFETY: constant, NUL-terminated key and value names.
            let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, APPROVED, VALUE) };
        } else {
            // SAFETY: as above.
            let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, VALUE) };
            if rc != ERROR_SUCCESS && rc.0 != 2 {
                // 2 = ERROR_FILE_NOT_FOUND: already off.
                return Err(platform_err(format!("cannot remove the Run key: error {}", rc.0)));
            }
        }
        Ok(())
    }

    pub fn is_enabled() -> bool {
        // SAFETY: a size-only query; no buffer is written.
        let present =
            unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, VALUE, RRF_RT_REG_SZ, None, None, None) }
                == ERROR_SUCCESS;
        if !present {
            return false;
        }
        let mut data = [0u8; 12];
        let mut len = data.len() as u32;
        // SAFETY: `data` and `len` describe a valid 12-byte buffer.
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                APPROVED,
                VALUE,
                RRF_RT_REG_BINARY,
                None,
                Some(data.as_mut_ptr().cast()),
                Some(&mut len),
            )
        };
        // Task Manager writes 02/06 for enabled and 03/07 for disabled in the
        // first byte; no entry means enabled.
        rc != ERROR_SUCCESS || len == 0 || data[0] & 1 == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn desktop_entry_plain() {
        let text = desktop_entry(Path::new("/usr/bin/glossa"), &s(&["--minimized"]));
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("\nType=Application\n"));
        assert!(text.contains("\nName=Glossa\n"));
        assert!(text.contains("\nExec=/usr/bin/glossa --minimized\n"));
        assert!(text.contains("\nTerminal=false\n"));
        assert!(text.ends_with("X-GNOME-Autostart-enabled=true\n"));
    }

    #[test]
    fn desktop_exec_quoting() {
        let text = desktop_entry(
            Path::new("/opt/My Apps/glossa"),
            &s(&["--name", "50% off", "say \"hi\"", "a\\b", ""]),
        );
        let exec = text.lines().find(|l| l.starts_with("Exec=")).unwrap();
        assert_eq!(
            exec,
            r#"Exec="/opt/My Apps/glossa" --name "50%% off" "say \\"hi\\"" "a\\\\b" """#
        );
    }

    #[test]
    fn plist_contents() {
        let text = launch_agent_plist(
            Path::new("/Applications/Glossa.app/Contents/MacOS/glossa"),
            &s(&["--minimized", "a&b"]),
        );
        assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        assert!(text.contains("<key>Label</key>\n    <string>app.glossa</string>"));
        assert!(text.contains(
            "    <array>\n        <string>/Applications/Glossa.app/Contents/MacOS/glossa</string>\n        <string>--minimized</string>\n        <string>a&amp;b</string>\n    </array>"
        ));
        assert!(text.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(text.trim_end().ends_with("</plist>"));
    }

    #[test]
    fn windows_command_lines() {
        assert_eq!(
            windows_command_line(
                Path::new(r"C:\Program Files\Glossa\glossa.exe"),
                &s(&["--minimized"])
            ),
            r#""C:\Program Files\Glossa\glossa.exe" --minimized"#
        );
        assert_eq!(
            windows_command_line(
                Path::new(r"C:\g.exe"),
                &s(&["a b", r#"say "hi""#, r"C:\my dir\", r"C:\dir\", ""])
            ),
            r#""C:\g.exe" "a b" "say \"hi\"" "C:\my dir\\" C:\dir\ """#
        );
    }

    #[test]
    fn file_set_and_unset_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("glossa-autostart-test-{}", std::process::id()));
        let file = dir.join("autostart").join("glossa.desktop");
        let entry = desktop_entry(Path::new("/usr/bin/glossa"), &[]);
        set_file(&file, Some(&entry)).unwrap();
        assert!(file_enables(&file));
        std::fs::write(&file, entry.replace("enabled=true", "enabled=false")).unwrap();
        assert!(!file_enables(&file), "switched off by the desktop");
        set_file(&file, None).unwrap();
        assert!(!file.exists());
        assert!(!file_enables(&file));
        set_file(&file, None).unwrap(); // removing twice is fine
        let _ = std::fs::remove_dir_all(&dir);
    }
}
