//! Process modules end to end: the Java SDK and the example Java module
//! built with the JDK found on this machine, and the Node.js example, each
//! loaded through the registry and called through the provider API.
//!
//! A test whose toolchain is missing prints why and passes: CI machines and
//! developer boxes differ, and the Rust side is covered by unit tests.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use glossa_core::ocr::ImageSource;
use glossa_core::{Capabilities, LangId, LangPair, OcrRequest, TranslateRequest};
use glossa_modules::provider::with_test_host;
use glossa_modules::runtime::{process::find_java, stdio::find_program};
use glossa_modules::{OptionValues, Registry, RuntimeContext};
use glossa_net::mock::MockHttp;
use glossa_net::Method;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn ctx() -> RuntimeContext {
    RuntimeContext { call_timeout: Duration::from_secs(60), ..Default::default() }
}

/// A JDK tool (`javac`, `jar`) that actually runs: from `JAVA_HOME`, else
/// beside the `java` Glossa would use, else on `PATH`.
fn jdk_tool(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("JAVA_HOME").filter(|h| !h.is_empty()) {
        candidates.push(PathBuf::from(home).join("bin").join(&exe));
    }
    if let Some(java) = find_java("") {
        let real = std::fs::canonicalize(&java).unwrap_or(java);
        if let Some(bin) = real.parent() {
            candidates.push(bin.join(&exe));
        }
    }
    candidates.extend(find_program(name, Path::new(".")));
    candidates.into_iter().filter(|c| c.is_file()).find(|c| {
        Command::new(c).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    })
}

fn java_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            java_sources(&p, out);
        } else if p.extension().is_some_and(|e| e == "java") {
            out.push(p);
        }
    }
}

fn run(cmd: &mut Command) {
    let out = cmd.output().unwrap_or_else(|e| panic!("cannot run {cmd:?}: {e}"));
    assert!(
        out.status.success(),
        "{cmd:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Compile `src_dirs` together with the SDK into `classes`.
fn javac(javac: &Path, src_dirs: &[PathBuf], classes: &Path) {
    let mut sources = Vec::new();
    java_sources(&repo().join("sdk").join("java").join("src"), &mut sources);
    for d in src_dirs {
        java_sources(d, &mut sources);
    }
    std::fs::create_dir_all(classes).unwrap();
    run(Command::new(javac).args(["--release", "17", "-encoding", "UTF-8", "-d"]).arg(classes).args(&sources));
}

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glossa-procmod-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_java_example_module_builds_and_runs() {
    let (Some(javac_bin), Some(jar_bin)) = (jdk_tool("javac"), jdk_tool("jar")) else {
        eprintln!("note: no JDK (javac and jar) found; skipping the Java module test");
        return;
    };
    if find_java("").is_none() {
        eprintln!("note: javac found but no java for Glossa to run; skipping the Java module test");
        return;
    }
    // Build in a copy, so the repository stays free of build output.
    let tmp = Temp::new("java");
    let module = tmp.0.join("example-upper");
    std::fs::create_dir_all(&module).unwrap();
    let example = repo().join("modules").join("java").join("example-upper");
    std::fs::copy(example.join("module.toml"), module.join("module.toml")).unwrap();
    let classes = tmp.0.join("classes");
    javac(&javac_bin, &[example.join("src")], &classes);
    run(Command::new(&jar_bin)
        .args(["--create", "--file"])
        .arg(module.join("module.jar"))
        .args(["--main-class", "example.ExampleUpper", "-C"])
        .arg(&classes)
        .arg("."));

    let mut reg = Registry::new();
    let report = reg.load_dir(&tmp.0, &ctx());
    assert_eq!(report.loaded, ["example-upper"], "{report:?}");
    let entry = reg.entry("example-upper").unwrap();
    assert!(entry.info().capabilities.contains(Capabilities::TRANSLATE | Capabilities::OCR));
    let provider = entry.provider.clone();
    let http = MockHttp::new();
    let opts = OptionValues::new();
    with_test_host(&http, &opts, |host| {
        let pair = LangPair::new(LangId::AUTO, LangId::RUSSIAN);
        let t = provider.translate(host, &TranslateRequest::new("Hello, мир", pair)).unwrap();
        assert_eq!(t.text, "HELLO, МИР");
        assert_eq!(t.detected_from, Some(LangId::ENGLISH));
        // The same process answers again.
        assert_eq!(provider.translate(host, &TranslateRequest::new("ß and i", pair)).unwrap().text, "SS AND I");
        let ocr = provider.recognize(host, &OcrRequest::new(ImageSource::Bytes(vec![1, 2, 3, 4]))).unwrap();
        assert_eq!(ocr.text, "Recognised by Java (4 bytes)");
    });
    provider.suspend();
}

/// The SDK's `Host` against Glossa's real host API: HTTP under the
/// manifest's permissions, the cache, logging, a host call by name, and a
/// module started from `main_class` + `classpath` instead of a jar.
#[test]
fn the_java_sdk_reaches_every_host_call() {
    let Some(javac_bin) = jdk_tool("javac") else {
        eprintln!("note: no javac found; skipping the Java host-call test");
        return;
    };
    if find_java("").is_none() {
        eprintln!("note: no java found; skipping the Java host-call test");
        return;
    }
    let tmp = Temp::new("probe");
    let module = tmp.0.join("probe");
    std::fs::create_dir_all(module.join("src")).unwrap();
    std::fs::write(
        module.join("src").join("Probe.java"),
        r#"
import io.github.bropines.glossa.*;

public final class Probe implements GlossaModule {
    public Object translate(Request req, Host host) throws Exception {
        Host.HttpResponse r = host.fetch("POST", "https://api.test/echo", java.util.Map.of("X-A", "1"), "q=" + req.text());
        Object data = host.fetch("https://api.test/json").json();
        host.cacheSet("k", Json.obj("n", 7), 60);
        Object cached = host.cacheGet("k");
        host.info("probe ran");
        String denied;
        try { host.fetch("https://denied.test/"); denied = "no"; } catch (ModuleError e) { denied = e.kind + (e.message.contains("not in the module's permissions") ? "+perm" : ""); }
        String unknown;
        try { host.call("no_such_host_call", null); unknown = "no"; } catch (ModuleError e) { unknown = e.kind; }
        return r.status() + "|" + r.body() + "|" + Json.asMap(data).get("v") + "|" + Json.asMap(cached).get("n")
            + "|" + denied + "|" + unknown + "|" + req.option("flavour", "none");
    }
    public static void main(String[] args) { Glossa.run(new Probe()); }
}
"#,
    )
    .unwrap();
    javac(&javac_bin, &[module.join("src")], &module.join("classes"));
    std::fs::write(
        module.join("module.toml"),
        "capabilities = [\"translate\"]\n[module]\nid = \"probe\"\nname = \"Probe\"\nruntime = \"java\"\nmain_class = \"Probe\"\nclasspath = [\"classes\"]\n[permissions]\nnetwork = [\"api.test\"]\n[[options]]\nkey = \"flavour\"\ntype = \"text\"\n",
    )
    .unwrap();

    let mut reg = Registry::new();
    let report = reg.load_dir(&tmp.0, &ctx());
    assert_eq!(report.loaded, ["probe"], "{report:?}");
    let provider = reg.get("probe").unwrap();
    let http = MockHttp::new().on(Method::Post, "https://api.test/echo", 201, "echoed").on(
        Method::Get,
        "https://api.test/json",
        200,
        "{\"v\": 42}",
    );
    let mut opts = OptionValues::new();
    opts.insert("flavour".into(), serde_json::json!("vanilla"));
    with_test_host(&http, &opts, |host| {
        let t = provider.translate(host, &TranslateRequest::new("hi", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)));
        assert_eq!(t.unwrap().text, "201|echoed|42|7|other+perm|unsupported|vanilla");
    });
    let seen = http.requests();
    assert_eq!(seen.len(), 2, "the denied request never left: {seen:?}");
    provider.suspend();
}

/// The SDK's own tests (JSON codec, protocol loop), so `cargo test` covers
/// them wherever a JDK is around.
#[test]
fn the_java_sdk_tests_pass() {
    let (Some(javac_bin), Some(java)) = (jdk_tool("javac"), find_java("")) else {
        eprintln!("note: no JDK found; skipping the Java SDK's tests");
        return;
    };
    let tmp = Temp::new("sdktest");
    let classes = tmp.0.join("classes");
    javac(&javac_bin, &[repo().join("sdk").join("java").join("test")], &classes);
    run(Command::new(java).arg("-cp").arg(&classes).arg("io.github.bropines.glossa.SdkTest"));
}

#[test]
fn the_node_example_module_runs() {
    if find_program("node", Path::new(".")).is_none() {
        eprintln!("note: node not found on PATH; skipping the Node.js module test");
        return;
    }
    let mut reg = Registry::new();
    let report = reg.load_dir(&repo().join("modules").join("process"), &ctx());
    assert!(report.loaded.contains(&"example-node".to_owned()), "{report:?}");
    let provider = reg.get("example-node").unwrap();
    let http = MockHttp::new();
    let opts = OptionValues::new();
    with_test_host(&http, &opts, |host| {
        let pair = LangPair::new(LangId::AUTO, LangId::ENGLISH);
        let t = provider.translate(host, &TranslateRequest::new("one two  three мир", pair)).unwrap();
        assert_eq!(t.text, "мир three two one");
        assert_eq!(provider.translate(host, &TranslateRequest::new("a b", pair)).unwrap().text, "b a");
        let d = provider.detect(host, "whatever").unwrap();
        assert_eq!((d.lang, d.confidence), (LangId::ENGLISH, 0.5));
    });
    provider.suspend();
}

#[test]
fn a_java_module_without_java_fails_to_load_with_install_advice() {
    let tmp = Temp::new("nojava");
    let module = tmp.0.join("needs-java");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("module.jar"), b"").unwrap();
    std::fs::write(
        module.join("module.toml"),
        "capabilities = [\"translate\"]\n[module]\nid = \"needs-java\"\nname = \"x\"\nruntime = \"java\"\n",
    )
    .unwrap();
    let ctx = RuntimeContext { java: "/no/such/java".into(), ..ctx() };
    let report = Registry::new().load_dir(&tmp.0, &ctx);
    assert!(report.loaded.is_empty());
    let (_, why) = &report.failed[0];
    assert!(why.contains("Java not found") && why.contains("adoptium.net"), "{why}");
}
