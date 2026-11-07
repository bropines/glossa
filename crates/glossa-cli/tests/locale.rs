//! `glossa-cli locale check` and `locale template`, run as the real binary:
//! exit codes, the JSON report, and a template that sees the user's packs.

use std::path::PathBuf;
use std::process::{Command, Output};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("glossa-cli-locale-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cli(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_glossa-cli"))
        .arg("--home")
        .arg(home)
        .args(args)
        .env_remove("GLOSSA_LANG")
        .output()
        .expect("glossa-cli runs")
}

const GOOD: &str = r#"{ "code": "eo", "native_name": "Esperanto",
    "strings": { "main.translate": "Traduki", "no.such.key": "?" } }"#;
const BAD: &str = r#"{ "code": "eo", "strings": { "main.translate": "Traduki", "msg.ms": "ms" } }"#;

#[test]
fn check_exits_zero_with_only_warnings_and_one_with_errors() {
    let home = temp("check");
    let good = home.join("good.json");
    let bad = home.join("bad.json");
    std::fs::write(&good, GOOD).unwrap();
    std::fs::write(&bad, BAD).unwrap();

    let out = cli(&home, &["locale", "check", good.to_str().unwrap()]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "warnings alone pass: {text}");
    assert!(text.contains("Esperanto (eo): 1 of "), "{text}");
    assert!(text.contains("warning: no.such.key: unknown key, ignored"), "{text}");

    let out = cli(&home, &["locale", "check", bad.to_str().unwrap()]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("error: msg.ms: 0 placeholders where English has 1"), "{text}");

    let out = cli(&home, &["locale", "check", home.join("missing.json").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "an unreadable file fails");
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn check_reports_json() {
    let home = temp("json");
    let bad = home.join("bad.json");
    std::fs::write(&bad, BAD).unwrap();
    let out = cli(&home, &["--json", "locale", "check", bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one JSON document");
    assert_eq!(report["code"], "eo");
    assert_eq!(report["translated"], 1, "the skipped string does not count");
    assert!(report["total"].as_u64().unwrap() > 100);
    assert!(report["missing"].as_array().unwrap().iter().any(|k| k == "msg.ms"));
    let issues = report["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0]["key"], "msg.ms");
    assert_eq!(issues[0]["severity"], "error");
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn template_for_a_language_includes_the_users_files() {
    let home = temp("template");
    std::fs::create_dir_all(home.join("locales")).unwrap();
    std::fs::write(
        home.join("locales").join("ru.json"),
        r#"{ "code": "ru", "strings": { "main.translate": "Перевести!!" } }"#,
    )
    .unwrap();
    std::fs::write(home.join("locales").join("eo.json"), GOOD).unwrap();

    let out = cli(&home, &["locale", "template", "ru"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(r#""main.translate": "Перевести!!""#), "the user's override");
    assert!(text.contains(r#""options.server": "API управления""#), "the built-in pack under it");

    let out = cli(&home, &["locale", "template", "eo"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(r#""code": "eo""#) && text.contains(r#""main.translate": "Traduki""#));
    assert!(text.contains(r#""options.server": """#), "untranslated keys stay empty");

    let out = cli(&home, &["locale", "template", "xx"]);
    assert_eq!(out.status.code(), Some(64), "an unknown language is a usage error");
    std::fs::remove_dir_all(&home).ok();
}
