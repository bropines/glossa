//! The module protocol as Glossa speaks it, replayed against the built
//! binary over stdin and stdout: a request, the `assets` host call and its
//! answer, the reply. None of these reach ONNX Runtime or a model, so they
//! run anywhere, CI included. Recognition itself is covered by the
//! fixtures in basic.toml and the ignored `live_recognition` test.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

/// Feed `input` (one JSON value per line) and return what came out.
fn exchange(input: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_paddle-ocr"))
        .env_remove("GLOSSA_ASSETS_DIR")
        .env("GLOSSA_MODULE_ID", "paddle-ocr")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start paddle-ocr");
    {
        let mut stdin = child.stdin.take().unwrap();
        for v in input {
            writeln!(stdin, "{v}").unwrap();
        }
    } // closing stdin ends the module
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "exit {:?}", out.status);
    String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).expect(l)).collect()
}

fn error_kind(v: &Value) -> &str {
    v["error"]["kind"].as_str().unwrap_or("")
}

#[test]
fn other_methods_are_unsupported() {
    let out = exchange(&[json!({"id": 1, "method": "translate", "params": {"text": "hi", "from": "en", "to": "de"}})]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["id"], 1);
    assert_eq!(error_kind(&out[0]), "unsupported");
}

#[test]
fn recognize_asks_for_its_files_first() {
    let out = exchange(&[
        json!({"id": 7, "method": "recognize", "params": {"image_base64": "", "lang": "auto", "options": {}}}),
        json!({"id": "h1", "result": {"dir": "", "files": {}}}),
    ]);
    assert_eq!(out[0], json!({"id": "h1", "method": "assets", "params": null}));
    assert_eq!(out[1]["id"], 7);
    assert!(out[1]["error"]["message"].as_str().unwrap().contains("det.onnx"), "{}", out[1]);
}

#[test]
fn a_language_without_its_recogniser_names_the_download() {
    let out = exchange(&[
        json!({"id": 2, "method": "recognize", "params": {"image_base64": "", "lang": "ko", "options": {}}}),
        json!({"id": "h1", "result": {"files": {"det.onnx": "/nowhere/det.onnx", "rec-latin.onnx": "/nowhere/rec-latin.onnx"}}}),
    ]);
    assert_eq!(error_kind(&out[1]), "source_language", "{}", out[1]);
    let msg = out[1]["error"]["message"].as_str().unwrap();
    assert!(msg.contains("glossa-cli assets download paddle-ocr rec-korean.onnx"), "{msg}");
}

#[test]
fn auto_with_every_script_switched_off_says_so() {
    let options = json!({"script_latin": false, "script_eslav": false, "script_cyrillic": false, "script_ch": false, "script_korean": false});
    let out = exchange(&[
        json!({"id": 3, "method": "recognize", "params": {"image_base64": "", "lang": "auto", "options": options}}),
        json!({"id": "h1", "result": {"files": {"det.onnx": "/nowhere/det.onnx", "rec-latin.onnx": "/nowhere/rec-latin.onnx"}}}),
    ]);
    assert_eq!(error_kind(&out[1]), "other");
    assert!(out[1]["error"]["message"].as_str().unwrap().contains("switched on"), "{}", out[1]);
}

#[test]
fn a_broken_picture_is_a_bad_request_and_the_module_carries_on() {
    let files = json!({"files": {"det.onnx": "/nowhere/det.onnx", "rec-latin.onnx": "/nowhere/rec-latin.onnx"}});
    let out = exchange(&[
        json!({"id": 4, "method": "recognize", "params": {"image_base64": "bm90IGEgcGljdHVyZQ==", "lang": "en", "options": {}}}),
        json!({"id": "h1", "result": files}),
        json!({"id": 5, "method": "speak", "params": {}}),
    ]);
    assert_eq!(error_kind(&out[1]), "bad_request", "{}", out[1]);
    assert_eq!(out[2]["id"], 5);
    assert_eq!(error_kind(&out[2]), "unsupported");
}

#[test]
fn lines_that_are_not_requests_are_skipped() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_paddle-ocr"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"hello\n\n{\"id\":9,\"method\":\"detect\",\"params\":{}}\n").unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains("\"id\":9") && text.contains("unsupported"), "{text}");
}
