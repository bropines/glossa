//! `glossa-cli doctor`: what works on this machine and what does not, in
//! one report for the user and for bug reports.
//!
//! Everything is looked up, nothing is changed: a `config.toml` that does
//! not parse is reported, not moved aside as the desktop program would.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use glossa_core::{Config, Paths};
use glossa_engine::Engine;
use glossa_platform::permissions::{self, Permission, Status};
use glossa_platform::{Compositor, DisplayServer, Session};
use serde_json::{json, Value};

/// The whole report as JSON; [`print`] turns it into text.
pub fn report(paths: &Paths) -> Value {
    let session = Session::detect();
    let config = Config::load(&paths.config_file());
    let cfg = config.as_ref().cloned().unwrap_or_default();

    let display = match &session.display {
        DisplayServer::Windows => "Windows".to_owned(),
        DisplayServer::MacOs => "macOS".to_owned(),
        DisplayServer::X11 => "X11".to_owned(),
        DisplayServer::Wayland { compositor: Compositor::Unknown } => "Wayland (unknown compositor)".to_owned(),
        DisplayServer::Wayland { compositor } => format!("Wayland ({compositor})"),
        DisplayServer::Unknown => "none".to_owned(),
    };
    let features: Vec<Value> = session
        .features()
        .iter()
        .map(|f| json!({ "feature": f.key, "support": f.support.as_str(), "note": f.note }))
        .collect();
    let limitations: Vec<Value> =
        session.limitations().iter().map(|l| json!({ "key": l.key, "text": l.english() })).collect();
    let permissions: Vec<Value> = Permission::all()
        .iter()
        .map(|p| {
            let status = match permissions::status(*p) {
                Status::Granted => "granted",
                Status::Missing => "missing",
                Status::NotNeeded => "not needed",
            };
            json!({ "permission": format!("{p:?}"), "status": status })
        })
        .collect();

    let ocr: Vec<Value> = glossa_ocr::engine_status(&cfg.ocr)
        .into_iter()
        .map(|s| json!({ "engine": s.id, "available": s.available, "detail": s.detail }))
        .collect();

    let (config_state, modules) = match &config {
        Err(e) => (json!({ "ok": false, "error": e.to_string() }), Value::Null),
        Ok(c) => {
            let modules = match Engine::with_config(paths.clone(), c.clone()) {
                Ok(e) => {
                    let s = e.module_status();
                    json!({
                        "loaded": s.loaded.len(),
                        "failed": s.failed.iter().map(|(p, w)| json!({ "path": p, "error": w })).collect::<Vec<_>>(),
                        "skipped": s.skipped.iter().map(|(p, w)| json!({ "path": p, "reason": w })).collect::<Vec<_>>(),
                    })
                }
                Err(e) => json!({ "error": e.to_string() }),
            };
            (json!({ "ok": true }), modules)
        }
    };

    let proxy_mode = format!("{:?}", cfg.network.proxy.mode).to_lowercase();
    json!({
        "version": glossa_core::VERSION,
        "system": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "display": display,
            "desktop": session.desktop,
        },
        "features": features,
        "limitations": limitations,
        "permissions": permissions,
        "ocr": ocr,
        "modules": modules,
        "paths": {
            "config": paths.config_file(),
            "config_state": config_state,
            "data": paths.data_dir,
            "cache": paths.cache_dir,
            "log": paths.log_file(),
            "portable": paths.portable,
        },
        "network": {
            "proxy_mode": proxy_mode,
            "proxy": glossa_net::proxy_in_use(&cfg.network),
            "timeout_secs": cfg.network.timeout_secs,
            "certificates": if glossa_net::uses_system_certificates() { "system store" } else { "bundled Mozilla roots" },
        },
        "control_api": control_api(&cfg),
        "dbus": dbus(&cfg),
        "portal": portal(&session),
        "running": glossa_platform::instance::is_running(paths),
    })
}

/// The D-Bus service (Linux and the BSDs): switched on, and whether some
/// program owns its name now. `null` elsewhere.
fn dbus(cfg: &Config) -> Value {
    if !glossa_engine::dbus::SUPPORTED {
        return Value::Null;
    }
    let name = glossa_engine::dbus::BUS_NAME;
    let owner = match glossa_platform::portal::session_name_has_owner(name) {
        None => "no session bus",
        Some(true) => "owned (Glossa is running)",
        Some(false) => "nobody owns it",
    };
    json!({ "enabled": cfg.server.dbus, "name": name, "owner": owner })
}

/// What the desktop portal offers, asked only on Wayland, where Glossa
/// relies on it for global shortcuts and screen capture. `null` elsewhere.
fn portal(session: &Session) -> Value {
    if !session.is_wayland() {
        return Value::Null;
    }
    let p = glossa_platform::portal::probe();
    json!({
        "reachable": p.reachable,
        "global_shortcuts": p.global_shortcuts,
        "screenshot": p.screenshot,
        "error": p.error,
    })
}

/// Whether the control API is switched on and what answers at its address.
fn control_api(cfg: &Config) -> Value {
    let addr = glossa_engine::server::connect_addr(&cfg.server.bind);
    let answer = match addr.to_socket_addrs().ok().and_then(|mut a| a.next()) {
        None => "the address does not resolve".to_owned(),
        Some(sock) => probe(sock),
    };
    json!({
        "enabled": cfg.server.enabled,
        "bind": cfg.server.bind,
        "pot_compat_without_token": cfg.server.pot_compat_without_token,
        "answer": answer,
    })
}

/// What listens at `addr`: Glossa (its `/health`), something else, or
/// nothing.
fn probe(addr: SocketAddr) -> String {
    if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err() {
        return "nothing is listening".to_owned();
    }
    let settings = glossa_core::config::Network {
        timeout_secs: 2,
        max_retries: 0,
        proxy: glossa_core::config::Proxy { mode: glossa_core::config::ProxyMode::None, ..Default::default() },
        ..Default::default()
    };
    let health = glossa_net::HttpClient::new(&settings).ok().and_then(|http| {
        let req = glossa_net::HttpRequest::get(format!("http://{addr}/health")).timeout(Duration::from_secs(2));
        glossa_net::Http::send(&http, req).ok()
    });
    match health.and_then(|r| serde_json::from_slice::<Value>(&r.body).ok()) {
        Some(v) if v["name"] == "glossa" => format!("Glossa {} is running", v["version"].as_str().unwrap_or("?")),
        _ => "another program has the port".to_owned(),
    }
}

/// The report as text.
pub fn print(r: &Value) -> String {
    use std::fmt::Write as _;
    let mut o = String::new();
    let s = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".to_owned(),
        other => other.to_string(),
    };
    let _ = writeln!(o, "Glossa {}", s(&r["version"]));
    let sys = &r["system"];
    let desktop =
        sys["desktop"].as_str().filter(|d| !d.is_empty()).map(|d| format!(", desktop {d}")).unwrap_or_default();
    let _ = writeln!(o, "\nSystem\n  {} {}, display: {}{desktop}", s(&sys["os"]), s(&sys["arch"]), s(&sys["display"]));

    let _ = writeln!(o, "\nDesktop features");
    for f in r["features"].as_array().into_iter().flatten() {
        let _ = writeln!(o, "  {:16} {:7} {}", s(&f["feature"]), s(&f["support"]), s(&f["note"]));
    }
    if let Some(l) = r["limitations"].as_array().filter(|l| !l.is_empty()) {
        let _ = writeln!(o, "\nLimitations");
        for x in l {
            let _ = writeln!(o, "  - {}", s(&x["text"]));
        }
    }
    if let Some(p) = r["permissions"].as_array().filter(|p| !p.is_empty()) {
        let _ = writeln!(o, "\nPermissions");
        for x in p {
            let _ = writeln!(o, "  {:16} {}", s(&x["permission"]), s(&x["status"]));
        }
    }
    let _ = writeln!(o, "\nOCR engines");
    for e in r["ocr"].as_array().into_iter().flatten() {
        let mark = if e["available"] == true { "yes" } else { "no" };
        let _ = writeln!(o, "  {:16} {:7} {}", s(&e["engine"]), mark, s(&e["detail"]));
    }
    let m = &r["modules"];
    let _ = writeln!(o, "\nModules");
    if m.is_null() {
        let _ = writeln!(o, "  not loaded: the config file does not parse");
    } else if let Some(e) = m.get("error") {
        let _ = writeln!(o, "  the engine did not start: {}", s(e));
    } else {
        let _ = writeln!(o, "  loaded: {}", s(&m["loaded"]));
        for f in m["failed"].as_array().into_iter().flatten() {
            let _ = writeln!(o, "  failed: {}: {}", s(&f["path"]), s(&f["error"]));
        }
        for f in m["skipped"].as_array().into_iter().flatten() {
            let _ = writeln!(o, "  skipped: {}: {}", s(&f["path"]), s(&f["reason"]));
        }
    }
    let p = &r["paths"];
    let _ = writeln!(o, "\nFiles");
    let state = if p["config_state"]["ok"] == true {
        "ok".to_owned()
    } else {
        format!("does not parse: {}", s(&p["config_state"]["error"]))
    };
    let _ = writeln!(o, "  config   {} ({state})", s(&p["config"]));
    let _ = writeln!(o, "  data     {}{}", s(&p["data"]), if p["portable"] == true { " (portable)" } else { "" });
    let _ = writeln!(o, "  cache    {}", s(&p["cache"]));
    let _ = writeln!(o, "  log      {}", s(&p["log"]));
    let n = &r["network"];
    let _ = writeln!(o, "\nNetwork");
    let _ = writeln!(o, "  proxy    {} (mode {})", s(&n["proxy"]), s(&n["proxy_mode"]));
    let _ = writeln!(o, "  timeout  {} s", s(&n["timeout_secs"]));
    let _ = writeln!(o, "  TLS      {}", s(&n["certificates"]));
    let c = &r["control_api"];
    let _ = writeln!(o, "\nControl API");
    let _ = writeln!(
        o,
        "  {} on {}: {}",
        if c["enabled"] == true { "enabled" } else { "disabled" },
        s(&c["bind"]),
        s(&c["answer"])
    );
    let d = &r["dbus"];
    if !d.is_null() {
        let _ = writeln!(o, "\nD-Bus service");
        let state = if d["enabled"] == true { "enabled" } else { "disabled" };
        let _ = writeln!(o, "  {state}, {}: {}", s(&d["name"]), s(&d["owner"]));
    }
    let p = &r["portal"];
    if !p.is_null() {
        let _ = writeln!(o, "\nDesktop portal");
        let version = |v: &Value| v.as_u64().map(|v| format!("yes (version {v})")).unwrap_or_else(|| "no".into());
        if p["reachable"] == true {
            let _ = writeln!(o, "  global shortcuts {}", version(&p["global_shortcuts"]));
            let _ = writeln!(o, "  screenshot       {}", version(&p["screenshot"]));
        } else {
            let _ = writeln!(o, "  not reachable: {}", s(&p["error"]));
        }
    }
    let _ = writeln!(
        o,
        "\nDesktop program for this data folder: {}",
        if r["running"] == true { "running" } else { "not running" }
    );
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_covers_every_section_and_reads_as_text() {
        let dir = std::env::temp_dir().join(format!("glossa-doctor-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        let mut cfg = Config::default();
        cfg.server.bind = "127.0.0.1:9".into(); // discard: nothing listens
        cfg.server.token = "t".into();
        cfg.save(&paths.config_file()).unwrap();
        let r = report(&paths);
        for key in
            ["system", "features", "limitations", "permissions", "ocr", "modules", "paths", "network", "control_api"]
        {
            assert!(!r[key].is_null(), "{key} missing");
        }
        assert_eq!(r["paths"]["config_state"]["ok"], true);
        assert_eq!(r["running"], false);
        assert!(r["limitations"].as_array().unwrap().iter().all(|l| l["key"].is_string() && l["text"].is_string()));
        let text = print(&r);
        for heading in ["System", "Desktop features", "OCR engines", "Modules", "Files", "Network", "Control API"] {
            assert!(text.contains(heading), "{heading} missing from\n{text}");
        }
        if glossa_engine::dbus::SUPPORTED {
            assert_eq!(r["dbus"]["name"], "io.github.bropines.Glossa");
            assert!(text.contains("D-Bus service"), "{text}");
        } else {
            assert!(r["dbus"].is_null());
        }

        // A broken config is reported, and left where it is.
        std::fs::write(paths.config_file(), "[general\nbroken").unwrap();
        let r = report(&paths);
        assert_eq!(r["paths"]["config_state"]["ok"], false);
        assert!(print(&r).contains("does not parse"));
        assert!(paths.config_file().exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
