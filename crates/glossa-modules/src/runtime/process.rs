//! Modules as ordinary programs: `runtime = "process"` runs any command that
//! speaks the module protocol over stdio (Node, Go, C#, a shell script, a
//! compiled binary), and `runtime = "java"` is the same with the command line
//! built from a jar or a main class and Java found for the user.
//!
//! Both use the shared transport in [`super::stdio`], exactly as Python
//! modules do: same framing, same host calls, same idle stop and restart.
//! What differs is only the command line, and that the program is looked for
//! when the module loads: a module whose program is missing fails to load
//! with a message saying what to install, which `glossa-cli modules` and
//! *Options → Modules* show.
//!
//! ```toml
//! [module]
//! runtime = "process"
//! command = ["node", "main.mjs"]          # or command.windows / .macos / .linux / .unix / .default
//! args = ["--quiet"]                      # appended to the command
//! env = { NODE_NO_WARNINGS = "1" }
//! cwd = "."                               # relative to the module's folder (the default)
//! ```
//!
//! `${MODULE_DIR}` in any of these stands for the module's folder. A program
//! given with a path (`./my-module`, `bin/tool`) is found relative to the
//! module's folder; a bare name (`node`) on `PATH`. On Windows a name without
//! an extension tries `.exe`, `.cmd`, `.bat`…, so `npx` finds `npx.cmd`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::stdio::{expand, find_program, Launch, StdioConfig, StdioTransport};
use crate::manifest::{Manifest, Runtime};
use crate::protocol::ProtocolProvider;
use crate::provider::Provider;
use crate::registry::RuntimeContext;

/// What to tell a user without Java.
pub const JAVA_MISSING: &str = "Java not found. Install a Java 17 or newer runtime, for example Eclipse Temurin \
     (https://adoptium.net; `winget install EclipseAdoptium.Temurin.21.JRE` on Windows, \
     `sudo apt install openjdk-17-jre-headless` on Debian or Ubuntu, `brew install --cask temurin` on macOS), \
     then set JAVA_HOME or put java on PATH, or give the path to java in Options > Modules";

/// [`JAVA_MISSING`], naming the configured path when the setting is what
/// failed, since searching stops at an explicit setting.
fn java_missing(ctx: &RuntimeContext) -> String {
    match ctx.java.trim() {
        "" => JAVA_MISSING.to_owned(),
        configured => format!(
            "{configured} is not a Java runtime (the Java setting in Options > Modules); clear it to look in \
             JAVA_HOME and PATH. {JAVA_MISSING}"
        ),
    }
}

/// JVM options every Java module gets, before its own `jvm_args` (so those
/// win): the serial collector keeps a small heap small, and headless AWT
/// lets a module use `java.awt.image` without a window or a Dock icon.
const DEFAULT_JVM_ARGS: &[&str] = &["-XX:+UseSerialGC", "-Djava.awt.headless=true"];

pub fn load(dir: &Path, manifest: Manifest, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let transport = transport(dir, &manifest, ctx)?;
    Ok(Arc::new(ProtocolProvider::new(manifest, transport, ctx.cache.clone()).configured(ctx)))
}

/// The stdio transport for a `process` or `java` module. Nothing starts
/// until the first call.
pub fn transport(dir: &Path, manifest: &Manifest, ctx: &RuntimeContext) -> Result<StdioTransport, String> {
    let m = &manifest.module;
    let (launch, kind) = match m.runtime {
        Runtime::Java => (java_launch(dir, manifest, ctx)?, "Java"),
        _ => (process_launch(dir, manifest, ctx)?, "process"),
    };
    tracing::debug!(module = %m.id, program = %Path::new(&launch.program).display(), "{kind} module resolved");
    let cfg = StdioConfig::new(&m.id, kind, ctx.call_timeout, m.idle_timeout_secs);
    Ok(StdioTransport::new(cfg, Box::new(move || Ok(launch.clone()))))
}

/// The command line of a `runtime = "process"` module.
pub fn process_launch(dir: &Path, manifest: &Manifest, ctx: &RuntimeContext) -> Result<Launch, String> {
    let m = &manifest.module;
    let line = m
        .command
        .as_ref()
        .and_then(|c| c.for_this_platform())
        .ok_or_else(|| format!("the manifest has no command for this platform ({})", std::env::consts::OS))?;
    let line: Vec<String> = line.iter().map(|s| expand(s, dir)).collect();
    let program = resolve(&line[0], dir, ctx)?;
    let mut args: Vec<OsString> = line[1..].iter().map(OsString::from).collect();
    args.extend(m.args.iter().map(|a| OsString::from(expand(a, dir))));
    Ok(Launch { program: program.into_os_string(), args, cwd: working_dir(dir, manifest)?, env: env(dir, manifest) })
}

/// The command line of a `runtime = "java"` module: `java [options] -jar
/// <entry> [args]`, or `java [options] -cp <classpath> <main_class> [args]`.
pub fn java_launch(dir: &Path, manifest: &Manifest, ctx: &RuntimeContext) -> Result<Launch, String> {
    let m = &manifest.module;
    let mut args: Vec<OsString> = DEFAULT_JVM_ARGS.iter().map(OsString::from).collect();
    args.extend(m.jvm_args.iter().map(|a| OsString::from(expand(a, dir))));
    if m.main_class.trim().is_empty() {
        let entry = match m.entry.trim() {
            "" => "module.jar",
            e => e,
        };
        let jar = dir.join(expand(entry, dir));
        if !jar.is_file() {
            return Err(format!("{} not found; build the module first", jar.display()));
        }
        args.push("-jar".into());
        args.push(jar.into_os_string());
    } else {
        // Without a classpath, the jar named in `entry` or the folder itself.
        let entries: Vec<String> = if !m.classpath.is_empty() {
            m.classpath.clone()
        } else if !m.entry.trim().is_empty() {
            vec![m.entry.trim().to_owned()]
        } else {
            vec![".".to_owned()]
        };
        let paths: Vec<PathBuf> = entries
            .iter()
            .map(|e| {
                let e = expand(e, dir);
                if Path::new(&e).is_absolute() {
                    PathBuf::from(e)
                } else {
                    dir.join(e)
                }
            })
            .collect();
        let classpath = std::env::join_paths(&paths).map_err(|e| format!("classpath: {e}"))?;
        args.push("-cp".into());
        args.push(classpath);
        args.push(m.main_class.trim().into());
    }
    args.extend(m.args.iter().map(|a| OsString::from(expand(a, dir))));
    let java = find_java(&ctx.java).ok_or_else(|| java_missing(ctx))?;
    Ok(Launch { program: java.into_os_string(), args, cwd: working_dir(dir, manifest)?, env: env(dir, manifest) })
}

fn working_dir(dir: &Path, manifest: &Manifest) -> Result<PathBuf, String> {
    let cwd = manifest.module.cwd.trim();
    if cwd.is_empty() {
        return Ok(dir.to_owned());
    }
    let path = dir.join(expand(cwd, dir));
    if path.is_dir() {
        Ok(path)
    } else {
        Err(format!("cwd {} is not a folder", path.display()))
    }
}

fn env(dir: &Path, manifest: &Manifest) -> Vec<(OsString, OsString)> {
    manifest.module.env.iter().map(|(k, v)| (k.into(), expand(v, dir).into())).collect()
}

/// The program at the head of a command line. `java` gets the same search
/// as Java modules, so `command = ["java", …]` finds a JDK that is not on
/// `PATH`.
fn resolve(name: &str, dir: &Path, ctx: &RuntimeContext) -> Result<PathBuf, String> {
    let stem = name.trim().trim_end_matches(".exe");
    if stem == "java" || stem == "javaw" {
        return find_java(&ctx.java).ok_or_else(|| java_missing(ctx));
    }
    find_program(name, dir).ok_or_else(|| {
        if name.contains('/') || name.contains('\\') {
            format!("{name} not found in {}", dir.display())
        } else {
            format!("{name} not found on PATH; install it, or give its full path in the manifest's command")
        }
    })
}

/// Find a `java` executable: the configured one (a file, a Java home or a
/// command name), then `$JAVA_HOME/bin`, then `PATH`, then the folders the
/// usual installers use.
pub fn find_java(configured: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let configured = configured.trim().trim_matches('"');
    if !configured.is_empty() {
        let p = Path::new(configured);
        if p.is_dir() {
            return [p.join("bin").join(exe), p.join(exe)].into_iter().find(|c| c.is_file());
        }
        if p.is_file() {
            return Some(p.to_owned());
        }
        return find_program(configured, Path::new("."));
    }
    if let Some(home) = std::env::var_os("JAVA_HOME").filter(|h| !h.is_empty()) {
        let candidate = PathBuf::from(home).join("bin").join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Some(found) = find_program("java", Path::new(".")) {
        return Some(found);
    }
    java_install_dirs().into_iter().map(|home| home.join("bin").join(exe)).find(|c| c.is_file())
}

/// Java homes under the folders installers use, newest-looking name first
/// within each folder.
fn java_install_dirs() -> Vec<PathBuf> {
    let mut parents: Vec<PathBuf> = Vec::new();
    let mut nested_home = false;
    if cfg!(windows) {
        let roots = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .chain(std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Programs")));
        for root in roots {
            for vendor in
                ["Eclipse Adoptium", "Java", "Microsoft", "Zulu", "BellSoft", "Amazon Corretto", "Semeru", "OpenJDK"]
            {
                parents.push(root.join(vendor));
            }
        }
    } else if cfg!(target_os = "macos") {
        nested_home = true;
        parents.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
        if let Some(home) = std::env::var_os("HOME") {
            parents.push(PathBuf::from(home).join("Library/Java/JavaVirtualMachines"));
        }
    } else {
        parents.push(PathBuf::from("/usr/lib/jvm"));
        if let Some(home) = std::env::var_os("HOME") {
            parents.push(PathBuf::from(home).join(".sdkman/candidates/java"));
        }
    }
    let mut out = Vec::new();
    for parent in parents {
        let Ok(read) = std::fs::read_dir(&parent) else { continue };
        let mut homes: Vec<PathBuf> = read.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        homes.sort();
        homes.reverse();
        out.extend(homes.into_iter().map(|h| if nested_home { h.join("Contents").join("Home") } else { h }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct TempModule {
        dir: PathBuf,
        manifest: Manifest,
    }

    impl TempModule {
        fn new(name: &str, module_lines: &str, files: &[(&str, &str)]) -> Self {
            let dir = std::env::temp_dir().join(format!("glossa-proc-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let toml = format!(
                "capabilities = [\"translate\"]\n[module]\nid = \"{name}\"\nname = \"{name}\"\n{module_lines}\n"
            );
            std::fs::write(dir.join("module.toml"), &toml).unwrap();
            for (file, body) in files {
                std::fs::write(dir.join(file), body).unwrap();
            }
            let manifest = Manifest::parse(&toml).unwrap();
            Self { dir, manifest }
        }
    }

    impl Drop for TempModule {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn ctx() -> RuntimeContext {
        RuntimeContext { call_timeout: Duration::from_secs(20), ..Default::default() }
    }

    #[test]
    fn the_command_line_is_built_from_the_manifest() {
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        let m = TempModule::new(
            "cmdline",
            &format!(
                "runtime = \"process\"\ncommand = [\"{shell}\", \"${{MODULE_DIR}}/x\"]\nargs = [\"--flag\"]\nenv = {{ A = \"${{MODULE_DIR}}\" }}"
            ),
            &[],
        );
        let launch = process_launch(&m.dir, &m.manifest, &ctx()).unwrap();
        assert!(Path::new(&launch.program).is_absolute(), "{launch:?}");
        let dir = m.dir.to_string_lossy().into_owned();
        assert_eq!(launch.args, [OsString::from(format!("{dir}/x")), OsString::from("--flag")]);
        assert_eq!(launch.env, [(OsString::from("A"), OsString::from(dir))]);
        assert_eq!(launch.cwd, m.dir);
    }

    #[test]
    fn a_missing_program_fails_the_load_with_a_reason() {
        let m = TempModule::new("absent", "runtime = \"process\"\ncommand = [\"glossa-no-such-tool\", \"x\"]", &[]);
        let err = transport(&m.dir, &m.manifest, &ctx()).err().unwrap();
        assert!(err.contains("glossa-no-such-tool not found on PATH"), "{err}");
        let m = TempModule::new("absent-rel", "runtime = \"process\"\ncommand = \"./bin/tool\"", &[]);
        assert!(transport(&m.dir, &m.manifest, &ctx()).err().unwrap().contains("not found in"));
        let m = TempModule::new("badcwd", "runtime = \"process\"\ncommand = [\"sh\"]\ncwd = \"nope\"", &[]);
        if find_program("sh", &m.dir).is_some() {
            assert!(transport(&m.dir, &m.manifest, &ctx()).err().unwrap().contains("not a folder"));
        }
    }

    #[test]
    fn java_modules_need_their_jar_and_say_how_to_get_java() {
        let m = TempModule::new("nojar", "runtime = \"java\"\nentry = \"missing.jar\"", &[]);
        let err = java_launch(&m.dir, &m.manifest, &ctx()).unwrap_err();
        assert!(err.contains("missing.jar not found"), "{err}");

        let m = TempModule::new(
            "withjar",
            "runtime = \"java\"\njvm_args = [\"-Xmx64m\"]\nargs = [\"x\"]",
            &[("module.jar", "")],
        );
        let no_java = RuntimeContext { java: "/definitely/not/java".into(), ..ctx() };
        let err = java_launch(&m.dir, &m.manifest, &no_java).unwrap_err();
        assert!(err.starts_with("/definitely/not/java is not a Java runtime") && err.ends_with(JAVA_MISSING), "{err}");
        if let Some(java) = find_java("") {
            let launch = java_launch(&m.dir, &m.manifest, &ctx()).unwrap();
            assert_eq!(launch.program, java.into_os_string());
            let args: Vec<String> = launch.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            let jar = m.dir.join("module.jar").to_string_lossy().into_owned();
            assert_eq!(args, ["-XX:+UseSerialGC", "-Djava.awt.headless=true", "-Xmx64m", "-jar", jar.as_str(), "x"]);
        }
    }

    #[test]
    fn java_main_classes_get_a_classpath() {
        let m = TempModule::new(
            "maincls",
            "runtime = \"java\"\nmain_class = \"com.example.Main\"\nclasspath = [\"classes\", \"lib/*\"]",
            &[],
        );
        let Some(java) = find_java("") else {
            eprintln!("note: no Java found; skipping the classpath check");
            return;
        };
        let launch =
            java_launch(&m.dir, &m.manifest, &RuntimeContext { java: java.to_string_lossy().into(), ..ctx() }).unwrap();
        let args: Vec<String> = launch.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let sep = if cfg!(windows) { ";" } else { ":" };
        let cp = format!("{}{sep}{}", m.dir.join("classes").display(), m.dir.join("lib/*").display());
        assert_eq!(&args[2..], ["-cp", cp.as_str(), "com.example.Main"]);
    }

    /// A module with no interpreter at all: a POSIX shell loop that answers
    /// every request, proving the transport needs nothing but stdio.
    #[cfg(unix)]
    #[test]
    fn a_shell_script_is_a_module() {
        use crate::protocol::Transport;
        use glossa_module_sdk::wire::{ModuleError, Reply};
        use serde_json::Value;

        fn no_host(method: &str, _: Value) -> Reply {
            Reply::err(ModuleError::other(format!("unexpected host call {method}")))
        }

        let script = r#"
echo "starting" >&2
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed 's/^{"id":\([0-9]*\).*/\1/')
  printf 'not a frame\n'
  printf '{"id":%s,"result":{"text":"from sh %s"}}\n' "$id" "$GREETING"
done
"#;
        let m = TempModule::new(
            "shell",
            "runtime = \"process\"\ncommand = [\"sh\", \"main.sh\"]\nenv = { GREETING = \"hi\" }",
            &[("main.sh", script)],
        );
        let t = transport(&m.dir, &m.manifest, &ctx()).unwrap();
        assert!(!t.is_running());
        let params = serde_json::json!({"text": "x", "from": "auto", "to": "en", "options": {}});
        assert_eq!(t.call("translate", params.clone(), &no_host).unwrap()["text"], "from sh hi");
        assert_eq!(t.call("translate", params, &no_host).unwrap()["text"], "from sh hi");
        assert!(t.is_running());
        t.suspend();
        assert!(!t.is_running());
    }
}
