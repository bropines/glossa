//! The XDG desktop portals Glossa uses on Wayland: global shortcuts and
//! screenshots.
//!
//! A portal is a D-Bus service of the desktop (`org.freedesktop.portal.Desktop`
//! on the session bus, implemented by `xdg-desktop-portal` with a backend per
//! desktop) that does on an application's behalf what Wayland does not let it
//! do itself, asking the user where the desktop thinks it should:
//!
//! - **GlobalShortcuts** (KDE Plasma 6, GNOME 48 and later, Hyprland):
//!   shortcuts the desktop watches for the application. Used by
//!   [`crate::hotkeys`].
//! - **Screenshot** (GNOME, KDE, wlroots with `xdg-desktop-portal-wlr`): a
//!   picture of the whole screen, saved to a file. Used by screen capture
//!   when the session is Wayland. GNOME asks the user once whether Glossa may
//!   take screenshots.
//!
//! The calls follow the portal's request pattern: a method returns at once
//! with the path of a `Request` object, and the answer arrives later as that
//! object's `Response` signal, after whatever dialog the desktop shows. The
//! signal is subscribed to before the call (the request path is predictable
//! from the `handle_token` option), so a fast answer is never missed.
//!
//! One connection serves every portal call of the process; it is opened on
//! first use and registers Glossa's application id with the host portal
//! registry (`xdg-desktop-portal` 1.19 and later), which KDE needs to
//! remember the shortcuts per application.
//!
//! Elsewhere (Windows, macOS) this module has only [`probe`], which answers
//! that there are no portals.

/// What the session's portal offers, for `glossa-cli doctor`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortalSupport {
    /// Whether `org.freedesktop.portal.Desktop` answered at all.
    pub reachable: bool,
    /// The GlobalShortcuts interface's version, if offered.
    pub global_shortcuts: Option<u32>,
    /// The Screenshot interface's version, if offered.
    pub screenshot: Option<u32>,
    /// Why the portal could not be asked, when it could not.
    pub error: Option<String>,
}

/// Ask the session's portal which of the interfaces Glossa uses it offers.
/// May start the portal service (D-Bus activation), so it can take a moment;
/// call it from a worker or the command line.
pub fn probe() -> PortalSupport {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        imp::probe()
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        PortalSupport { error: Some("desktop portals are a Linux and BSD feature".into()), ..Default::default() }
    }
}

/// Whether some program owns `name` on the session bus (`None`: no session
/// bus, or not Linux or a BSD). `glossa-cli doctor` asks about Glossa's
/// own D-Bus name.
pub fn session_name_has_owner(name: &str) -> Option<bool> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        imp::name_has_owner(name)
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = name;
        None
    }
}

/// Turn a `file://` URI (what the Screenshot portal answers) into a path:
/// the host must be empty or `localhost`, and percent escapes are decoded
/// to raw bytes, which on Unix are the path's bytes. `None` for anything
/// else.
pub fn file_uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://").or_else(|| {
        // The scheme is case-insensitive.
        uri.get(..7).filter(|s| s.eq_ignore_ascii_case("file://")).map(|_| &uri[7..])
    })?;
    let path = match rest.find('/') {
        Some(0) => rest,
        Some(i) if rest[..i].eq_ignore_ascii_case("localhost") => &rest[i..],
        _ => return None,
    };
    // A query or fragment is not part of the path.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut bytes = Vec::with_capacity(path.len());
    let raw = path.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = raw.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes).ok().map(std::path::PathBuf::from)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) use imp::*;

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use futures_lite::StreamExt;
    use zbus::message::Type as MessageType;
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
    use zbus::{Connection, MatchRule, MessageStream};

    use super::PortalSupport;

    pub const DESTINATION: &str = "org.freedesktop.portal.Desktop";
    pub const PATH: &str = "/org/freedesktop/portal/desktop";
    pub const GLOBAL_SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
    pub const SCREENSHOT: &str = "org.freedesktop.portal.Screenshot";
    const REQUEST: &str = "org.freedesktop.portal.Request";
    const SESSION: &str = "org.freedesktop.portal.Session";
    const REGISTRY: &str = "org.freedesktop.host.portal.Registry";
    /// Glossa's application id, the same as its D-Bus name.
    pub const APP_ID: &str = "io.github.bropines.Glossa";
    /// How long a plain call (not a request with a dialog) may take,
    /// including starting the portal service.
    const CALL_TIMEOUT: Duration = Duration::from_secs(10);

    /// Why a portal call did not give an answer.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PortalError {
        /// No session bus, no portal service, or no such interface: try
        /// something else, and do not ask again.
        Unavailable(String),
        /// The user said no, or closed the dialog.
        Cancelled,
        /// The portal answered with a failure, or not in time.
        Failed(String),
    }

    impl std::fmt::Display for PortalError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                PortalError::Unavailable(why) => write!(f, "the desktop portal is not available: {why}"),
                PortalError::Cancelled => f.write_str("the request was cancelled in the desktop's dialog"),
                PortalError::Failed(why) => write!(f, "the desktop portal failed: {why}"),
            }
        }
    }

    impl From<PortalError> for glossa_core::Error {
        fn from(e: PortalError) -> Self {
            glossa_core::Error::Platform(e.to_string())
        }
    }

    fn failed(e: impl std::fmt::Display) -> PortalError {
        PortalError::Failed(e.to_string())
    }

    /// Whether a method call's error means the interface or service is not
    /// there (as opposed to a call that failed).
    fn unavailable_or_failed(e: zbus::Error) -> PortalError {
        let text = e.to_string();
        let missing = [
            "ServiceUnknown",
            "UnknownMethod",
            "UnknownInterface",
            "UnknownObject",
            "NameHasNoOwner",
            "No such interface",
        ];
        if missing.iter().any(|m| text.contains(m)) {
            PortalError::Unavailable(text)
        } else {
            PortalError::Failed(text)
        }
    }

    /// A connection to the session bus and the portal service on it. The
    /// destination is configurable so tests can stand up a fake portal under
    /// a name of their own.
    #[derive(Clone, Debug)]
    pub struct Portal {
        pub(crate) conn: Connection,
        pub(crate) destination: String,
    }

    static SHARED: Mutex<Option<Portal>> = Mutex::new(None);

    /// The process's portal connection, opened (and Glossa registered with
    /// the host registry) on first use. A failure is not remembered, so a
    /// session bus that appears later is used.
    pub fn shared() -> Result<Portal, PortalError> {
        let mut slot = SHARED.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(p) = slot.as_ref() {
            return Ok(p.clone());
        }
        let portal = async_io::block_on(Portal::connect())?;
        *slot = Some(portal.clone());
        Ok(portal)
    }

    impl Portal {
        /// Connect to the session bus and register Glossa's app id.
        pub async fn connect() -> Result<Portal, PortalError> {
            let conn = zbus::connection::Builder::session()
                .map(|b| b.method_timeout(CALL_TIMEOUT))
                .map_err(|e| PortalError::Unavailable(format!("no session bus: {e}")))?
                .build()
                .await
                .map_err(|e| PortalError::Unavailable(format!("no session bus: {e}")))?;
            let portal = Portal { conn, destination: DESTINATION.to_owned() };
            portal.register_app_id().await;
            Ok(portal)
        }

        /// A portal over an existing connection, for tests with a fake one.
        #[cfg(test)]
        pub fn with(conn: Connection, destination: &str) -> Portal {
            Portal { conn, destination: destination.to_owned() }
        }

        /// Tell the host registry which application this is, so portals that
        /// keep state per application (KDE's shortcuts) can. Only possible
        /// before any other portal call on the connection; older portals do
        /// not have the registry, which is fine.
        async fn register_app_id(&self) {
            let options: HashMap<&str, Value<'_>> = HashMap::new();
            let result = self
                .conn
                .call_method(Some(self.destination.as_str()), PATH, Some(REGISTRY), "Register", &(APP_ID, options))
                .await;
            match result {
                Ok(_) => tracing::debug!("registered {APP_ID} with the portal"),
                Err(e) => tracing::debug!("portal registry not used: {e}"),
            }
        }

        /// The `version` property of a portal interface: `None` when the
        /// portal or the interface is missing.
        pub async fn version(&self, interface: &str) -> Result<u32, PortalError> {
            let reply = self
                .conn
                .call_method(
                    Some(self.destination.as_str()),
                    PATH,
                    Some("org.freedesktop.DBus.Properties"),
                    "Get",
                    &(interface, "version"),
                )
                .await
                .map_err(|e| {
                    // A missing interface answers InvalidArgs ("No such
                    // interface") on Properties.Get.
                    let text = e.to_string();
                    if text.contains("InvalidArgs") || text.contains("No such") {
                        PortalError::Unavailable(format!("no {interface}: {text}"))
                    } else {
                        unavailable_or_failed(e)
                    }
                })?;
            let value: OwnedValue = reply.body().deserialize().map_err(failed)?;
            u32::try_from(value).map_err(failed)
        }

        /// Where the portal will put the `Request` object for `token`:
        /// `/org/freedesktop/portal/desktop/request/SENDER/TOKEN`, the
        /// sender being our unique name without its colon, dots made
        /// underscores.
        pub fn request_path(&self, token: &str) -> String {
            let sender = self.conn.unique_name().map(|n| n.as_str().to_owned()).unwrap_or_default();
            request_path_for(&sender, token)
        }

        async fn response_stream(&self, path: &str) -> Result<MessageStream, PortalError> {
            let rule = MatchRule::builder()
                .msg_type(MessageType::Signal)
                .interface(REQUEST)
                .and_then(|r| r.member("Response"))
                .and_then(|r| r.path(path.to_owned()))
                .map_err(failed)?
                .build();
            MessageStream::for_match_rule(rule, &self.conn, Some(4)).await.map_err(failed)
        }

        /// Call a method that answers through a `Request` and wait up to
        /// `timeout` for the answer. `token` must be the `handle_token`
        /// option in `body`.
        pub async fn request<B>(
            &self,
            interface: &str,
            method: &str,
            body: &B,
            token: &str,
            timeout: Duration,
        ) -> Result<HashMap<String, OwnedValue>, PortalError>
        where
            B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
        {
            let expected = self.request_path(token);
            let mut responses = self.response_stream(&expected).await?;
            let reply = self
                .conn
                .call_method(Some(self.destination.as_str()), PATH, Some(interface), method, body)
                .await
                .map_err(unavailable_or_failed)?;
            let handle: OwnedObjectPath = reply.body().deserialize().map_err(failed)?;
            if handle.as_str() != expected {
                // Portals before 0.9 chose the path themselves.
                responses = self.response_stream(handle.as_str()).await?;
            }
            let answer = futures_lite::future::or(async { responses.next().await.map(Some) }, async {
                async_io::Timer::after(timeout).await;
                None
            })
            .await;
            let message = match answer {
                Some(Some(Ok(m))) => m,
                Some(Some(Err(e))) => return Err(failed(e)),
                Some(None) => return Err(failed("the bus connection closed")),
                None => {
                    // Withdraw the dialog, if there is one.
                    let _ = self
                        .conn
                        .call_method(Some(self.destination.as_str()), handle.as_str(), Some(REQUEST), "Close", &())
                        .await;
                    return Err(failed(format!("no answer to {method} in {} s", timeout.as_secs())));
                }
            };
            let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize().map_err(failed)?;
            match code {
                0 => Ok(results),
                1 => Err(PortalError::Cancelled),
                _ => Err(PortalError::Failed(format!("{method} was refused"))),
            }
        }

        /// A full-screen screenshot without a dialog (`interactive` false);
        /// the file the portal saved it to.
        pub async fn screenshot(&self, timeout: Duration) -> Result<std::path::PathBuf, PortalError> {
            let token = new_token();
            let body = screenshot_body(&token);
            let mut results = self.request(SCREENSHOT, "Screenshot", &body, &token, timeout).await?;
            let uri = results
                .remove("uri")
                .and_then(|v| String::try_from(v).ok())
                .ok_or_else(|| failed("the screenshot answer has no uri"))?;
            super::file_uri_to_path(&uri).ok_or_else(|| failed(format!("not a local file: {uri}")))
        }

        /// Start a GlobalShortcuts session; its handle.
        pub async fn create_shortcuts_session(&self) -> Result<OwnedObjectPath, PortalError> {
            let (token, session_token) = (new_token(), new_token());
            let body = create_session_body(&token, &session_token);
            let mut results = self.request(GLOBAL_SHORTCUTS, "CreateSession", &body, &token, CALL_TIMEOUT).await?;
            // The spec types it as a string; accept an object path too.
            let handle = results.remove("session_handle").ok_or_else(|| failed("no session_handle"))?;
            let handle = match String::try_from(handle.try_clone().map_err(failed)?) {
                Ok(s) => OwnedObjectPath::try_from(s).map_err(failed)?,
                Err(_) => OwnedObjectPath::try_from(handle).map_err(failed)?,
            };
            Ok(handle)
        }

        /// Ask the desktop to bind `shortcuts` in `session`. The desktop may
        /// show a dialog first, hence the long timeout. Answers the triggers
        /// as the desktop describes them, per id.
        pub async fn bind_shortcuts(
            &self,
            session: &ObjectPath<'_>,
            shortcuts: &[Shortcut],
            timeout: Duration,
        ) -> Result<Vec<(String, String)>, PortalError> {
            let token = new_token();
            let body = bind_body(session, shortcuts, &token);
            let mut results = self.request(GLOBAL_SHORTCUTS, "BindShortcuts", &body, &token, timeout).await?;
            Ok(results.remove("shortcuts").map(bound_triggers).unwrap_or_default())
        }

        /// End a portal session; its shortcuts stop firing.
        pub async fn close_session(&self, session: &ObjectPath<'_>) {
            let result =
                self.conn.call_method(Some(self.destination.as_str()), session, Some(SESSION), "Close", &()).await;
            if let Err(e) = result {
                tracing::debug!("closing portal session {session}: {e}");
            }
        }

        /// The stream of `Activated` signals from the GlobalShortcuts portal.
        pub async fn activations(&self) -> Result<MessageStream, PortalError> {
            let rule = MatchRule::builder()
                .msg_type(MessageType::Signal)
                .interface(GLOBAL_SHORTCUTS)
                .and_then(|r| r.member("Activated"))
                .and_then(|r| r.path(PATH))
                .map_err(failed)?
                .build();
            MessageStream::for_match_rule(rule, &self.conn, Some(16)).await.map_err(failed)
        }
    }

    /// One shortcut to bind: its id, what it does in words, and the trigger
    /// to suggest, in the XDG shortcut syntax (`CTRL+SHIFT+q`).
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Shortcut {
        pub id: String,
        pub description: String,
        pub preferred_trigger: Option<String>,
    }

    pub(crate) fn request_path_for(unique_name: &str, token: &str) -> String {
        let sender = unique_name.trim_start_matches(':').replace('.', "_");
        format!("{PATH}/request/{sender}/{token}")
    }

    /// A fresh `handle_token`: letters, digits and underscores only, as
    /// an object path element must be, and unique within the process.
    pub(crate) fn new_token() -> String {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        format!("glossa_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
    }

    type Options<'a> = HashMap<&'static str, Value<'a>>;

    /// `Screenshot(s parent_window, a{sv} options)`.
    pub(crate) fn screenshot_body(token: &str) -> (&'static str, Options<'_>) {
        let mut options = Options::new();
        options.insert("handle_token", Value::from(token));
        options.insert("interactive", Value::from(false));
        ("", options)
    }

    /// `CreateSession(a{sv} options)`.
    pub(crate) fn create_session_body<'a>(token: &'a str, session_token: &'a str) -> (Options<'a>,) {
        let mut options = Options::new();
        options.insert("handle_token", Value::from(token));
        options.insert("session_handle_token", Value::from(session_token));
        (options,)
    }

    /// `BindShortcuts(o session_handle, a(sa{sv}) shortcuts, s parent_window, a{sv} options)`.
    #[allow(clippy::type_complexity)]
    pub(crate) fn bind_body<'a>(
        session: &'a ObjectPath<'a>,
        shortcuts: &'a [Shortcut],
        token: &'a str,
    ) -> (&'a ObjectPath<'a>, Vec<(&'a str, Options<'a>)>, &'static str, Options<'a>) {
        let list = shortcuts
            .iter()
            .map(|s| {
                let mut o = Options::new();
                o.insert("description", Value::from(s.description.as_str()));
                if let Some(t) = &s.preferred_trigger {
                    o.insert("preferred_trigger", Value::from(t.as_str()));
                }
                (s.id.as_str(), o)
            })
            .collect();
        let mut options = Options::new();
        options.insert("handle_token", Value::from(token));
        (session, list, "", options)
    }

    /// The `shortcuts` result of BindShortcuts (`a(sa{sv})`): each id with
    /// its `trigger_description`, empty when the desktop gives none.
    pub(crate) fn bound_triggers(value: OwnedValue) -> Vec<(String, String)> {
        let Ok(list) = Vec::<(String, HashMap<String, OwnedValue>)>::try_from(value) else { return Vec::new() };
        list.into_iter()
            .map(|(id, mut props)| {
                let trigger =
                    props.remove("trigger_description").and_then(|v| String::try_from(v).ok()).unwrap_or_default();
                (id, trigger)
            })
            .collect()
    }

    /// An `Activated(o session_handle, s shortcut_id, t timestamp, a{sv} options)`
    /// signal's session and shortcut id.
    pub(crate) fn activation(message: &zbus::Message) -> Option<(String, String)> {
        let (session, id, _ts, _opts): (OwnedObjectPath, String, u64, HashMap<String, OwnedValue>) =
            message.body().deserialize().ok()?;
        Some((session.as_str().to_owned(), id))
    }

    pub fn probe() -> PortalSupport {
        let portal = match shared() {
            Ok(p) => p,
            Err(e) => return PortalSupport { error: Some(e.to_string()), ..Default::default() },
        };
        async_io::block_on(async {
            let gs = portal.version(GLOBAL_SHORTCUTS).await;
            let shot = portal.version(SCREENSHOT).await;
            let unreachable = [&gs, &shot].iter().find_map(|r| match r {
                Err(PortalError::Unavailable(why)) if !why.starts_with("no org.freedesktop.portal") => {
                    Some(why.clone())
                }
                Err(PortalError::Failed(why)) => Some(why.clone()),
                _ => None,
            });
            PortalSupport {
                reachable: unreachable.is_none(),
                global_shortcuts: gs.ok(),
                screenshot: shot.ok(),
                error: unreachable,
            }
        })
    }

    pub fn name_has_owner(name: &str) -> Option<bool> {
        let conn = zbus::blocking::Connection::session().ok()?;
        let reply = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &(name,),
            )
            .ok()?;
        reply.body().deserialize::<bool>().ok()
    }

    /// Take a screenshot through the portal and read it. The portal saves
    /// it to a file for us (GNOME in the Pictures folder); the file is
    /// removed once read, so OCR captures do not pile up there.
    pub fn screenshot_image() -> Result<image::RgbaImage, PortalError> {
        screenshot_image_from(&shared()?)
    }

    pub(crate) fn screenshot_image_from(portal: &Portal) -> Result<image::RgbaImage, PortalError> {
        // Long enough for GNOME's one-time permission dialog.
        let path = async_io::block_on(portal.screenshot(Duration::from_secs(60)))?;
        let image = image::open(&path).map(|i| i.to_rgba8());
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::debug!("could not remove the portal's screenshot {}: {e}", path.display());
        }
        image.map_err(|e| failed(format!("unreadable screenshot {}: {e}", path.display())))
    }

    /// A stand-in for `xdg-desktop-portal` on a session bus, under a name
    /// of its own: GlobalShortcuts and Screenshot as the spec describes
    /// them, answering every request at once (as a desktop whose user
    /// confirms instantly would). Tests run it under `dbus-run-session`.
    #[cfg(test)]
    pub(crate) mod fake {
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        use zbus::message::Header;
        use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
        use zbus::{fdo, Connection};

        use super::{request_path_for, GLOBAL_SHORTCUTS, PATH, REQUEST};

        /// A shortcut as BindShortcuts received it: id, description,
        /// preferred trigger.
        pub type Bound = (String, String, Option<String>);

        #[derive(Default, Debug)]
        pub struct State {
            /// Each BindShortcuts call: the session and the shortcuts
            /// (id, description, preferred trigger).
            pub binds: Vec<(String, Vec<Bound>)>,
            pub closed: Vec<String>,
            pub sessions: Vec<String>,
            /// Where the next screenshot is "saved".
            pub screenshot_file: Option<std::path::PathBuf>,
        }

        pub struct FakePortal {
            pub conn: zbus::blocking::Connection,
            pub name: String,
            pub state: Arc<Mutex<State>>,
        }

        impl FakePortal {
            /// `None` when there is no session bus.
            pub fn start(tag: &str) -> Option<FakePortal> {
                std::env::var_os("DBUS_SESSION_BUS_ADDRESS")?;
                let name = format!("io.github.bropines.GlossaTest.Portal.{tag}.p{}", std::process::id());
                let state = Arc::new(Mutex::new(State::default()));
                let conn = zbus::blocking::connection::Builder::session()
                    .ok()?
                    .serve_at(PATH, Shortcuts { state: state.clone() })
                    .ok()?
                    .serve_at(PATH, Screenshot { state: state.clone() })
                    .ok()?
                    .name(name.as_str())
                    .ok()?
                    .build()
                    .ok()?;
                Some(FakePortal { conn, name, state })
            }

            /// Press a bound shortcut.
            pub fn activate(&self, session: &str, id: &str) {
                let session = OwnedObjectPath::try_from(session.to_owned()).unwrap();
                let opts: HashMap<&str, Value<'_>> = HashMap::new();
                self.conn
                    .emit_signal(None::<&str>, PATH, GLOBAL_SHORTCUTS, "Activated", &(session, id, 0u64, opts))
                    .unwrap();
            }
        }

        fn token(options: &HashMap<String, OwnedValue>, key: &str) -> String {
            options.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
        }

        /// Answer a request: emit its Response to the caller and return its path.
        async fn respond(
            conn: &Connection,
            header: &Header<'_>,
            options: &HashMap<String, OwnedValue>,
            results: HashMap<&str, Value<'_>>,
        ) -> fdo::Result<OwnedObjectPath> {
            let sender = header.sender().map(|s| s.as_str().to_owned()).unwrap_or_default();
            let path = request_path_for(&sender, &token(options, "handle_token"));
            conn.emit_signal(Some(sender.as_str()), path.as_str(), REQUEST, "Response", &(0u32, results))
                .await
                .map_err(|e| fdo::Error::Failed(e.to_string()))?;
            OwnedObjectPath::try_from(path).map_err(|e| fdo::Error::Failed(e.to_string()))
        }

        struct Shortcuts {
            state: Arc<Mutex<State>>,
        }

        #[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
        impl Shortcuts {
            async fn create_session(
                &self,
                options: HashMap<String, OwnedValue>,
                #[zbus(header)] header: Header<'_>,
                #[zbus(connection)] conn: &Connection,
                #[zbus(object_server)] server: &zbus::ObjectServer,
            ) -> fdo::Result<OwnedObjectPath> {
                let sender = header.sender().map(|s| s.as_str().to_owned()).unwrap_or_default();
                let session = format!(
                    "{PATH}/session/{}/{}",
                    sender.trim_start_matches(':').replace('.', "_"),
                    token(&options, "session_handle_token")
                );
                server
                    .at(session.as_str(), Session { state: self.state.clone(), path: session.clone() })
                    .await
                    .map_err(|e| fdo::Error::Failed(e.to_string()))?;
                self.state.lock().unwrap().sessions.push(session.clone());
                let mut results = HashMap::new();
                results.insert("session_handle", Value::from(session));
                respond(conn, &header, &options, results).await
            }

            async fn bind_shortcuts(
                &self,
                session: OwnedObjectPath,
                shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
                _parent_window: String,
                options: HashMap<String, OwnedValue>,
                #[zbus(header)] header: Header<'_>,
                #[zbus(connection)] conn: &Connection,
            ) -> fdo::Result<OwnedObjectPath> {
                let list: Vec<Bound> = shortcuts
                    .iter()
                    .map(|(id, o)| {
                        let trigger = o.get("preferred_trigger").map(|_| token(o, "preferred_trigger"));
                        (id.clone(), token(o, "description"), trigger)
                    })
                    .collect();
                self.state.lock().unwrap().binds.push((session.as_str().to_owned(), list.clone()));
                let answered: Vec<(String, HashMap<&str, Value<'_>>)> = list
                    .into_iter()
                    .map(|(id, description, trigger)| {
                        let mut o = HashMap::new();
                        o.insert("description", Value::from(description));
                        o.insert("trigger_description", Value::from(trigger.unwrap_or_default()));
                        (id, o)
                    })
                    .collect();
                let mut results = HashMap::new();
                results.insert("shortcuts", Value::from(answered));
                respond(conn, &header, &options, results).await
            }

            #[zbus(property, name = "version")]
            fn version(&self) -> u32 {
                1
            }
        }

        struct Session {
            state: Arc<Mutex<State>>,
            path: String,
        }

        #[zbus::interface(name = "org.freedesktop.portal.Session")]
        impl Session {
            async fn close(&self, #[zbus(object_server)] server: &zbus::ObjectServer) {
                self.state.lock().unwrap().closed.push(self.path.clone());
                let _ = server.remove::<Session, _>(self.path.as_str()).await;
            }
        }

        struct Screenshot {
            state: Arc<Mutex<State>>,
        }

        #[zbus::interface(name = "org.freedesktop.portal.Screenshot")]
        impl Screenshot {
            async fn screenshot(
                &self,
                _parent_window: String,
                options: HashMap<String, OwnedValue>,
                #[zbus(header)] header: Header<'_>,
                #[zbus(connection)] conn: &Connection,
            ) -> fdo::Result<OwnedObjectPath> {
                let interactive = options.get("interactive").and_then(|v| bool::try_from(v).ok());
                if interactive != Some(false) {
                    return Err(fdo::Error::InvalidArgs("expected interactive = false".into()));
                }
                let file = self.state.lock().unwrap().screenshot_file.clone();
                let file = file.ok_or_else(|| fdo::Error::Failed("no screenshot".into()))?;
                // As GNOME names them: with a space, so the URI has escapes.
                let uri = format!("file://{}", file.display()).replace(' ', "%20");
                let mut results = HashMap::new();
                results.insert("uri", Value::from(uri));
                respond(conn, &header, &options, results).await
            }

            #[zbus(property, name = "version")]
            fn version(&self) -> u32 {
                1
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The fake portal's client side, `None` without a session bus.
        fn fake(tag: &str) -> Option<(fake::FakePortal, Portal)> {
            let Some(f) = fake::FakePortal::start(tag) else {
                eprintln!("no session bus; skipped (run under dbus-run-session)");
                return None;
            };
            let conn = async_io::block_on(zbus::Connection::session()).unwrap();
            let portal = Portal::with(conn, &f.name);
            Some((f, portal))
        }

        #[test]
        fn screenshots_come_back_as_images_and_the_file_goes() {
            let Some((f, portal)) = fake("shot") else { return };
            let dir = std::env::temp_dir().join(format!("glossa portal {}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("Screenshot from today.png");
            let mut img = image::RgbaImage::new(4, 3);
            img.put_pixel(1, 1, image::Rgba([9, 8, 7, 255]));
            img.save(&file).unwrap();
            f.state.lock().unwrap().screenshot_file = Some(file.clone());

            assert_eq!(async_io::block_on(portal.version(SCREENSHOT)), Ok(1));
            let shot = screenshot_image_from(&portal).unwrap();
            assert_eq!(shot.dimensions(), (4, 3));
            assert_eq!(shot.get_pixel(1, 1).0, [9, 8, 7, 255]);
            assert!(!file.exists(), "the portal's file is removed once read");
            std::fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn a_missing_interface_is_unavailable() {
            let Some((f, _)) = fake("missing") else { return };
            let conn = async_io::block_on(zbus::Connection::session()).unwrap();
            // A name nobody owns: no portal at all.
            let none = Portal::with(conn.clone(), "io.github.bropines.GlossaTest.Nobody");
            assert!(matches!(async_io::block_on(none.version(SCREENSHOT)), Err(PortalError::Unavailable(_))));
            // The fake has no RemoteDesktop.
            let fake = Portal::with(conn, &f.name);
            let v = async_io::block_on(fake.version("org.freedesktop.portal.RemoteDesktop"));
            assert!(matches!(v, Err(PortalError::Unavailable(_))), "{v:?}");
        }

        /// Against the session's real portal: on GNOME the first run asks
        /// for permission. `cargo test -p glossa-platform live_portal -- --ignored --nocapture`
        #[test]
        #[ignore = "needs a desktop portal with the Screenshot interface"]
        fn live_portal_screenshot() {
            let support = probe();
            eprintln!("portal: {support:?}");
            let image = screenshot_image().expect("a screenshot through the portal");
            eprintln!("screenshot {}x{}", image.width(), image.height());
            assert!(image.width() > 0 && image.height() > 0);
        }

        /// Against the session's real portal: binds Ctrl+Alt+Shift+F11 to
        /// the popup and waits a minute for it to be pressed. Confirm the
        /// desktop's dialog, then press the chord.
        #[test]
        #[ignore = "needs a Wayland desktop with the GlobalShortcuts portal and a person at the keyboard"]
        fn live_portal_global_shortcuts() {
            let mut hotkeys = crate::HotkeyService::new().expect("a hotkey service");
            assert!(hotkeys.uses_portal(), "not a Wayland session with the portal");
            hotkeys.register(glossa_core::Action::ShowPopup, &"Ctrl+Alt+Shift+F11".parse().unwrap()).unwrap();
            eprintln!("press Ctrl+Alt+Shift+F11 (or the trigger you chose in the dialog) within 60 s");
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_secs(60) {
                if let Some(action) = hotkeys.poll() {
                    eprintln!("activated: {}", action.id());
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            panic!("no activation within 60 s");
        }

        fn signature<B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType>(
            method: &str,
            body: &B,
        ) -> String {
            let msg = zbus::Message::method_call(PATH, method)
                .unwrap()
                .destination(DESTINATION)
                .unwrap()
                .interface(GLOBAL_SHORTCUTS)
                .unwrap()
                .build(body)
                .unwrap();
            msg.body().signature().to_string_no_parens()
        }

        #[test]
        fn request_paths_follow_the_spec() {
            assert_eq!(
                request_path_for(":1.42", "glossa_7_0"),
                "/org/freedesktop/portal/desktop/request/1_42/glossa_7_0"
            );
            let t = new_token();
            assert!(t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "{t}");
            assert_ne!(new_token(), new_token());
        }

        #[test]
        fn message_bodies_have_the_portal_signatures() {
            assert_eq!(signature("Screenshot", &screenshot_body("t")), "sa{sv}");
            assert_eq!(signature("CreateSession", &create_session_body("t", "s")), "a{sv}");
            let session = ObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_42/s").unwrap();
            let shortcuts = vec![
                Shortcut {
                    id: "show_popup".into(),
                    description: "Translate selection".into(),
                    preferred_trigger: Some("CTRL+SHIFT+q".into()),
                },
                Shortcut {
                    id: "text_recognition".into(),
                    description: "Text recognition".into(),
                    preferred_trigger: None,
                },
            ];
            let body = bind_body(&session, &shortcuts, "t");
            assert_eq!(signature("BindShortcuts", &body), "oa(sa{sv})sa{sv}");
            assert_eq!(body.1.len(), 2);
            assert_eq!(body.1[0].1.get("preferred_trigger"), Some(&Value::from("CTRL+SHIFT+q")));
            assert!(!body.1[1].1.contains_key("preferred_trigger"), "no trigger is left out, not sent empty");
            let (_, opts) = screenshot_body("tok");
            assert_eq!(opts.get("interactive"), Some(&Value::from(false)));
            assert_eq!(opts.get("handle_token"), Some(&Value::from("tok")));
        }

        #[test]
        fn activations_and_bind_results_are_read() {
            let session = ObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_42/s").unwrap();
            let opts: HashMap<&str, Value<'_>> = HashMap::new();
            let msg = zbus::Message::signal(PATH, GLOBAL_SHORTCUTS, "Activated")
                .unwrap()
                .build(&(&session, "show_popup", 1234u64, opts))
                .unwrap();
            assert_eq!(activation(&msg), Some((session.to_string(), "show_popup".to_owned())));

            let mut props: HashMap<&str, Value<'_>> = HashMap::new();
            props.insert("description", Value::from("Translate selection"));
            props.insert("trigger_description", Value::from("Ctrl+Shift+Q"));
            let list = vec![("show_popup", props)];
            let value = OwnedValue::try_from(Value::from(list)).unwrap();
            assert_eq!(bound_triggers(value), vec![("show_popup".to_owned(), "Ctrl+Shift+Q".to_owned())]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn file_uris_become_paths() {
        assert_eq!(file_uri_to_path("file:///tmp/a.png"), Some(PathBuf::from("/tmp/a.png")));
        assert_eq!(
            file_uri_to_path("file:///home/u/Pictures/Screenshot%20from%202026-11-01.png"),
            Some(PathBuf::from("/home/u/Pictures/Screenshot from 2026-11-01.png"))
        );
        // UTF-8 escapes (a Russian Pictures folder).
        assert_eq!(
            file_uri_to_path("file:///home/u/%D0%98%D0%B7%D0%BE%D0%B1%D1%80%D0%B0%D0%B6%D0%B5%D0%BD%D0%B8%D1%8F/s.png"),
            Some(PathBuf::from("/home/u/Изображения/s.png"))
        );
        assert_eq!(file_uri_to_path("file://localhost/tmp/b.png"), Some(PathBuf::from("/tmp/b.png")));
        assert_eq!(file_uri_to_path("FILE:///tmp/c.png"), Some(PathBuf::from("/tmp/c.png")));
        assert_eq!(file_uri_to_path("file:///tmp/d.png?x=1"), Some(PathBuf::from("/tmp/d.png")));
    }

    #[test]
    fn other_uris_are_refused() {
        assert_eq!(file_uri_to_path("https://example.com/a.png"), None);
        assert_eq!(file_uri_to_path("file://otherhost/tmp/a.png"), None, "a remote host");
        assert_eq!(file_uri_to_path("file:///tmp/%zz.png"), None, "a broken escape");
        assert_eq!(file_uri_to_path("file:///tmp/a%2"), None, "a cut escape");
        assert_eq!(file_uri_to_path("/tmp/a.png"), None);
        assert_eq!(file_uri_to_path(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn escapes_decode_to_raw_bytes() {
        use std::os::unix::ffi::OsStrExt;
        // Not UTF-8: a Latin-1 file name is still a valid Unix path.
        let p = file_uri_to_path("file:///tmp/caf%E9.png").unwrap();
        assert_eq!(p.as_os_str().as_bytes(), b"/tmp/caf\xe9.png");
    }
}
