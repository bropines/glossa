//! The D-Bus service, on Linux and the BSDs.
//!
//! The control API's actions and translation, on the session bus:
//!
//! ```text
//! name       io.github.bropines.Glossa
//! object     /io/github/bropines/Glossa
//! interface  io.github.bropines.Glossa1
//!   Action(s action_id, s text)                         any action, as its hotkey would
//!   Translate(s text, s from, s to, s service) -> (s translation, s detected, s provider)
//!   ShowPopup(s text)                                   show_popup
//!   ShowMain()                                          show_main_window
//!   Recognize()                                         text_recognition
//!   Version                                             property, s
//! ```
//!
//! It is what desktop tools on Linux expect (KDE's and GNOME's shortcut
//! settings, scripts with `gdbus` or `qdbus`), and unlike the HTTP API it
//! needs no token: the session bus belongs to the user, and the bus daemon
//! lets no other user connect to it.
//!
//! Actions go through [`Engine::request_action`], exactly like
//! `/v1/action/…`, so they work while the window system is unloaded. An empty
//! `text` means none, so the action reads the selection as its hotkey would.
//!
//! The service lives on one zbus connection, whose executor runs on a thread
//! of its own; a translation runs on `blocking`'s thread pool, so a slow one
//! does not hold up an action. Only one process owns the name: a second copy
//! (another data folder) runs without it.

use glossa_core::Error;

use crate::Engine;

/// The well-known name on the session bus.
pub const BUS_NAME: &str = "io.github.bropines.Glossa";
/// The object implementing [`INTERFACE`].
pub const OBJECT_PATH: &str = "/io/github/bropines/Glossa";
/// The interface; the `1` is its version, bumped if a method ever changes.
pub const INTERFACE: &str = "io.github.bropines.Glossa1";

/// Whether this build has the service at all.
pub const SUPPORTED: bool = cfg!(all(unix, not(target_os = "macos")));

/// The running service. Dropping it gives the name up.
pub struct DbusHandle {
    #[cfg(all(unix, not(target_os = "macos")))]
    _conn: zbus::blocking::Connection,
}

impl std::fmt::Debug for DbusHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DbusHandle")
    }
}

impl Drop for DbusHandle {
    fn drop(&mut self) {
        // The connection closes with the handle, which releases the name.
        tracing::debug!("D-Bus service stopped");
    }
}

/// Connect to the session bus, serve the interface and take [`BUS_NAME`].
/// Fails when there is no session bus or another process owns the name.
pub fn start(engine: Engine) -> Result<DbusHandle, Error> {
    start_named(engine, BUS_NAME)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn start_named(engine: Engine, name: &str) -> Result<DbusHandle, Error> {
    use zbus::fdo::{RequestNameFlags, RequestNameReply};

    let err = |e: zbus::Error| Error::Platform(format!("D-Bus: {e}"));
    let conn = zbus::blocking::connection::Builder::session()
        .map_err(err)?
        .serve_at(OBJECT_PATH, imp::Service { engine })
        .map_err(err)?
        .build()
        .map_err(err)?;
    // Not queued: a second copy should know at once that it has no name,
    // rather than silently take it over when the first one quits.
    let taken = || Error::Platform(format!("D-Bus: another program owns {name} on the session bus"));
    match conn.request_name_with_flags(name, RequestNameFlags::DoNotQueue.into()) {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
            tracing::info!("D-Bus service {name} on the session bus");
            Ok(DbusHandle { _conn: conn })
        }
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(zbus::Error::NameTaken) => Err(taken()),
        Err(e) => Err(err(e)),
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn start_named(_engine: Engine, _name: &str) -> Result<DbusHandle, Error> {
    Err(Error::Platform("the D-Bus service is for Linux and the BSDs".into()))
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use glossa_core::{Action, Capabilities, LangId, LangPair};
    use zbus::fdo;

    use crate::{Engine, TranslateOptions};

    /// What an empty string argument means: none.
    pub(super) fn non_empty(s: &str) -> Option<String> {
        (!s.is_empty()).then(|| s.to_owned())
    }

    pub(super) struct Service {
        pub engine: Engine,
    }

    #[zbus::interface(name = "io.github.bropines.Glossa1")]
    impl Service {
        /// Perform an action by its id (`show_popup`, `text_recognition`…),
        /// with `text` instead of the selection when not empty.
        fn action(&self, action: &str, text: &str) -> fdo::Result<()> {
            let action = parse_action(action)?;
            self.engine.request_action(action, non_empty(text));
            Ok(())
        }

        /// Translate `text`. Empty `from`, `to` and `service` mean the
        /// defaults: detect, the settings' direction, the active service
        /// (with fail-over). Returns the translation, the source language
        /// used and the service that answered.
        #[zbus(out_args("translation", "detected", "service"))]
        async fn translate(
            &self,
            text: String,
            from: String,
            to: String,
            service: String,
        ) -> fdo::Result<(String, String, String)> {
            let engine = self.engine.clone();
            blocking::unblock(move || {
                // A bug behind one call answers an error instead of ending
                // the program, as in the HTTP API.
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    translate(&engine, &text, &from, &to, &service)
                }))
                .unwrap_or_else(|payload| {
                    let msg = glossa_core::error::panic_message(payload.as_ref());
                    tracing::error!("D-Bus Translate panicked: {msg}");
                    Err(fdo::Error::Failed(format!("internal error: {msg}")))
                })
            })
            .await
        }

        /// `show_popup`: translate `text` (the selection when empty) in the popup.
        fn show_popup(&self, text: &str) {
            self.engine.request_action(Action::ShowPopup, non_empty(text));
        }

        /// `show_main_window`.
        fn show_main(&self) {
            self.engine.request_action(Action::ShowMainWindow, None);
        }

        /// `text_recognition`: select a screen region and recognise it.
        fn recognize(&self) {
            self.engine.request_action(Action::TextRecognition, None);
        }

        #[zbus(property)]
        fn version(&self) -> &str {
            glossa_core::VERSION
        }
    }

    pub(super) fn parse_action(id: &str) -> fdo::Result<Action> {
        Action::from_id(id.trim()).ok_or_else(|| {
            fdo::Error::InvalidArgs(format!(
                "no action {id:?}; the ids are {}",
                Action::ALL.iter().map(|a| a.id()).collect::<Vec<_>>().join(", ")
            ))
        })
    }

    fn lang(code: &str, what: &str) -> fdo::Result<LangId> {
        let code = code.trim();
        if code.is_empty() {
            return Ok(LangId::AUTO);
        }
        LangId::from_code(code).ok_or_else(|| {
            fdo::Error::InvalidArgs(format!("unknown language {code:?} for {what} (codes like en, ru, zh-CN, auto)"))
        })
    }

    pub(super) fn translate(
        engine: &Engine,
        text: &str,
        from: &str,
        to: &str,
        service: &str,
    ) -> fdo::Result<(String, String, String)> {
        let pair = LangPair::new(lang(from, "from")?, lang(to, "to")?);
        let provider = match service.trim() {
            "" => None,
            id => {
                engine.require_provider(id, Capabilities::TRANSLATE).map_err(fdo::Error::InvalidArgs)?;
                Some(id.to_owned())
            }
        };
        let opts = TranslateOptions { provider, ..Default::default() };
        match engine.translate(text, pair, &opts) {
            Ok(out) => {
                let r = out.report();
                Ok((r.text, r.from, r.service))
            }
            Err(e) => Err(error(&e)),
        }
    }

    /// Usage errors are the caller's (`InvalidArgs`), the rest `Failed`,
    /// with Glossa's message either way.
    pub(super) fn error(e: &glossa_core::Error) -> fdo::Error {
        use glossa_core::Error as E;
        match e {
            E::EmptyInput | E::SameLanguage | E::Config(_) => fdo::Error::InvalidArgs(e.to_string()),
            _ => fdo::Error::Failed(e.to_string()),
        }
    }
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use std::time::Duration;

    use glossa_core::{Action, Config, Paths};
    use zbus::fdo;

    use super::*;
    use crate::Event;

    fn engine() -> (Engine, std::path::PathBuf) {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let dir = std::env::temp_dir().join(format!("glossa-dbus-{}-{n}", std::process::id()));
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        let mut cfg = Config::default();
        cfg.server.enabled = false;
        (Engine::with_config(paths, cfg).unwrap(), dir)
    }

    /// A translator that needs no network.
    fn add_echo(e: &Engine) {
        let m = e.paths().user_module_dir().join("echo");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("module.toml"),
            "capabilities = [\"translate\"]\n[module]\nid = \"echo\"\nname = \"Echo\"\nruntime = \"js\"\n",
        )
        .unwrap();
        std::fs::write(m.join("main.js"), "function translate(req) { return 'echo: ' + req.text; }").unwrap();
        e.reload_modules();
        // Dropped in after the start, it waits for approval.
        e.approve_module("echo", None).unwrap();
    }

    #[test]
    fn action_ids_are_checked() {
        assert_eq!(imp::parse_action("show_popup").unwrap(), Action::ShowPopup);
        assert_eq!(imp::parse_action(" text_recognition ").unwrap(), Action::TextRecognition);
        match imp::parse_action("self_destruct") {
            Err(fdo::Error::InvalidArgs(m)) => assert!(m.contains("show_popup"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(imp::non_empty(""), None);
        assert_eq!(imp::non_empty("x").as_deref(), Some("x"));
    }

    #[test]
    fn translate_checks_its_arguments_and_answers_three_strings() {
        let (e, dir) = engine();
        add_echo(&e);
        let (text, from, service) = imp::translate(&e, "hi", "en", "ru", "echo").unwrap();
        assert_eq!((text.as_str(), from.as_str(), service.as_str()), ("echo: hi", "en", "echo"));
        assert!(matches!(imp::translate(&e, "hi", "klingon", "ru", ""), Err(fdo::Error::InvalidArgs(_))));
        assert!(matches!(imp::translate(&e, "hi", "en", "ru", "gogle"), Err(fdo::Error::InvalidArgs(_))));
        assert!(matches!(imp::translate(&e, "hi", "en", "en", "echo"), Err(fdo::Error::InvalidArgs(_))));
        assert!(e.history().is_empty(), "not recorded");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The whole round trip over a real session bus, under a name of its
    /// own so a running Glossa is not disturbed. Skipped where there is no
    /// session bus (`dbus-run-session cargo test` provides one).
    #[test]
    fn the_service_answers_on_the_session_bus() {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            eprintln!("no session bus; skipped");
            return;
        }
        let (e, dir) = engine();
        add_echo(&e);
        let rx = e.subscribe();
        let name = format!("io.github.bropines.GlossaTest.p{}", std::process::id());
        let _service = match start_named(e.clone(), &name) {
            Ok(h) => h,
            Err(err) => {
                eprintln!("session bus not usable ({err}); skipped");
                return;
            }
        };
        let client = zbus::blocking::Connection::session().unwrap();
        let call = |method: &str, body: &dyn erased::Body| body.call(&client, &name, method);

        call("ShowPopup", &("some text",)).unwrap();
        call("Action", &("text_recognition", "")).unwrap();
        call("ShowMain", &()).unwrap();
        let mut got = Vec::new();
        while got.len() < 3 {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Event::Action { action, text }) => got.push((action, text)),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert_eq!(
            got,
            vec![
                (Action::ShowPopup, Some("some text".into())),
                (Action::TextRecognition, None),
                (Action::ShowMainWindow, None),
            ]
        );
        let err = call("Action", &("self_destruct", "")).unwrap_err();
        assert!(err.to_string().contains("InvalidArgs"), "{err}");

        let reply = client
            .call_method(Some(name.as_str()), OBJECT_PATH, Some(INTERFACE), "Translate", &("hi", "en", "ru", "echo"))
            .unwrap();
        let out: (String, String, String) = reply.body().deserialize().unwrap();
        assert_eq!(out, ("echo: hi".into(), "en".into(), "echo".into()));

        let props = zbus::blocking::fdo::PropertiesProxy::builder(&client)
            .destination(name.as_str())
            .unwrap()
            .path(OBJECT_PATH)
            .unwrap()
            .build()
            .unwrap();
        let version = props.get(INTERFACE.try_into().unwrap(), "Version").unwrap();
        assert_eq!(String::try_from(version).unwrap(), glossa_core::VERSION);

        // A second copy cannot take the name.
        assert!(start_named(e.clone(), &name).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Lets the test pass bodies of different types through one closure.
    mod erased {
        pub trait Body {
            fn call(&self, c: &zbus::blocking::Connection, dest: &str, method: &str) -> zbus::Result<()>;
        }
        impl<T: serde::Serialize + zbus::zvariant::DynamicType> Body for T {
            fn call(&self, c: &zbus::blocking::Connection, dest: &str, method: &str) -> zbus::Result<()> {
                c.call_method(Some(dest), super::OBJECT_PATH, Some(super::INTERFACE), method, self).map(|_| ())
            }
        }
    }
}
