//! System-wide hotkeys.
//!
//! - **Windows, macOS, X11**: the `global-hotkey` crate (`RegisterHotKey`,
//!   Carbon hot keys, `XGrabKey`).
//! - **Wayland**: no protocol lets one client watch keys meant for another,
//!   but the desktop can do it for us: the XDG desktop portal's
//!   GlobalShortcuts interface (KDE Plasma 6, GNOME 48 and later, Hyprland).
//!   Glossa asks it to bind its actions, suggesting the configured chords;
//!   the desktop may show a dialog to confirm them, and from then on the
//!   user changes them in the desktop's own settings. Where the portal does
//!   not offer the interface (Sway and other wlroots compositors, older
//!   GNOME), [`HotkeyService::new`] refuses with [`HotkeyError::Unsupported`]
//!   and the app tells the user to bind a desktop shortcut to Glossa's D-Bus
//!   service or HTTP API instead.
//!
//! The rest of the app sees one API either way: register chords per action,
//! [`poll`](HotkeyService::poll) for the actions triggered.
//!
//! **Threading (Windows):** `RegisterHotKey` binds a hotkey to the calling
//! thread's message queue, and `global-hotkey` creates a hidden window on the
//! thread that constructs the manager. Create the [`HotkeyService`] on the
//! thread that runs the GUI event loop, or the events never arrive. On macOS
//! that is the main thread as well. The portal backend has threads of its
//! own and does not care.

use std::collections::{HashMap, VecDeque};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use glossa_core::{Action, Hotkey};

/// Why a hotkey could not be set up.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyError {
    /// The session cannot have global hotkeys at all (Wayland without the
    /// GlobalShortcuts portal).
    #[error("global hotkeys are not available: {reason}")]
    Unsupported { reason: String },
    /// The key name has no equivalent on this platform's hotkey API.
    #[error("the key {0:?} cannot be used in a global hotkey")]
    UnknownKey(String),
    /// Another program (or another of our actions) already owns the chord.
    #[error("{hotkey} is already in use by another program")]
    Taken { hotkey: String },
    /// Any other refusal by the OS.
    #[error("cannot register {hotkey}: {detail}")]
    Failed { hotkey: String, detail: String },
}

impl From<HotkeyError> for glossa_core::Error {
    fn from(e: HotkeyError) -> Self {
        glossa_core::Error::Platform(e.to_string())
    }
}

/// The registered hotkeys and the actions they trigger.
pub struct HotkeyService {
    backend: Backend,
    /// Triggered actions, with the text that goes with them (a panel's
    /// `module/panel` name for [`Action::OpenPanel`]).
    queue: VecDeque<(Action, Option<String>)>,
}

enum Backend {
    Global {
        manager: GlobalHotKeyManager,
        /// global-hotkey id → (the registered chord, its action, the text
        /// that goes with it).
        registered: Registered,
        /// Set by [`HotkeyService::set_waker`]: events then arrive here
        /// instead of through the crate's global receiver.
        events: Option<std::sync::mpsc::Receiver<GlobalHotKeyEvent>>,
    },
    #[cfg(all(unix, not(target_os = "macos")))]
    Portal(portal_backend::PortalHotkeys),
}

impl std::fmt::Debug for HotkeyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HotkeyService")
            .field("bound", &self.bound_actions().len())
            .field("portal", &self.uses_portal())
            .finish()
    }
}

impl HotkeyService {
    /// Create the service. On Windows, call this on the event-loop thread
    /// (see the module docs). On Wayland this asks the desktop portal
    /// whether it offers global shortcuts, waiting up to two seconds for it.
    pub fn new() -> Result<Self, HotkeyError> {
        let session = crate::Session::detect();
        if session.supports_global_hotkeys() {
            let manager = GlobalHotKeyManager::new().map_err(|e| HotkeyError::Unsupported { reason: e.to_string() })?;
            let backend = Backend::Global { manager, registered: HashMap::new(), events: None };
            return Ok(HotkeyService { backend, queue: VecDeque::new() });
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        if session.is_wayland() {
            return match portal_backend::PortalHotkeys::start(None) {
                Ok(p) => {
                    tracing::info!("global hotkeys through the desktop portal's GlobalShortcuts");
                    Ok(HotkeyService { backend: Backend::Portal(p), queue: VecDeque::new() })
                }
                Err(why) => Err(HotkeyError::Unsupported {
                    reason: format!(
                        "this Wayland desktop offers no global shortcuts portal ({why}); bind a shortcut in \
                         its settings that runs Glossa's D-Bus action or calls its HTTP API instead"
                    ),
                }),
            };
        }
        let reason = if session.is_wayland() {
            "Wayland does not let programs register global hotkeys; bind a shortcut in your compositor's \
             settings that calls Glossa's HTTP API instead"
        } else {
            "no graphical session"
        };
        Err(HotkeyError::Unsupported { reason: reason.into() })
    }

    /// Whether the desktop portal binds the hotkeys (Wayland). The desktop
    /// then owns the chords: it may ask the user to confirm them, and they
    /// are changed in its settings.
    pub fn uses_portal(&self) -> bool {
        match &self.backend {
            Backend::Global { .. } => false,
            #[cfg(all(unix, not(target_os = "macos")))]
            Backend::Portal(_) => true,
        }
    }

    /// Bind `hotkey` to `action`, replacing any hotkey the action had.
    ///
    /// Through the portal the binding happens a moment later, in one batch
    /// with the others (the desktop may show one dialog for all of them);
    /// a refusal there is logged rather than returned.
    pub fn register(&mut self, action: Action, hotkey: &Hotkey) -> Result<(), HotkeyError> {
        match &mut self.backend {
            Backend::Global { manager, registered, .. } => register_global(manager, registered, action, None, hotkey),
            #[cfg(all(unix, not(target_os = "macos")))]
            Backend::Portal(p) => {
                let trigger = portal_trigger(hotkey)?;
                p.set(action, trigger).map_err(|_| HotkeyError::Taken { hotkey: hotkey.to_string() })
            }
        }
    }

    /// Bind `hotkey` to opening the module panel `panel` (`module/panel`):
    /// it triggers [`Action::OpenPanel`] with the name as its text. Several
    /// panels may each have one. Not through the desktop portal, whose
    /// shortcuts are named by action.
    pub fn register_panel(&mut self, panel: &str, hotkey: &Hotkey) -> Result<(), HotkeyError> {
        match &mut self.backend {
            Backend::Global { manager, registered, .. } => {
                register_global(manager, registered, Action::OpenPanel, Some(panel.to_owned()), hotkey)
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            Backend::Portal(_) => {
                let _ = (panel, hotkey);
                Err(HotkeyError::Unsupported {
                    reason: "panel hotkeys are not offered through the desktop portal; bind a shortcut that runs \
                             `glossa-cli action open_panel MODULE/PANEL` instead"
                        .into(),
                })
            }
        }
    }

    /// Remove the hotkey bound to `action`, if any.
    pub fn unregister(&mut self, action: Action) {
        match &mut self.backend {
            Backend::Global { manager, registered, .. } => unregister_global(manager, registered, action, None),
            #[cfg(all(unix, not(target_os = "macos")))]
            Backend::Portal(p) => p.remove(action),
        }
    }

    /// Remove every hotkey this service registered.
    pub fn unregister_all(&mut self) {
        match &mut self.backend {
            Backend::Global { manager, registered, .. } => {
                let keys: Vec<HotKey> = registered.drain().map(|(_, (hk, _, _))| hk).collect();
                if let Err(e) = manager.unregister_all(&keys) {
                    tracing::debug!("unregistering hotkeys failed: {e}");
                }
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            Backend::Portal(p) => p.clear(),
        }
        self.queue.clear();
    }

    /// The actions currently bound.
    pub fn bound_actions(&self) -> Vec<Action> {
        let mut v: Vec<Action> = match &self.backend {
            Backend::Global { registered, .. } => {
                registered.values().filter(|(_, _, text)| text.is_none()).map(|(_, a, _)| *a).collect()
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            Backend::Portal(p) => p.actions(),
        };
        v.sort();
        v
    }

    /// Call `waker` on every hotkey event, from whatever thread the OS
    /// delivers it on, so a GUI whose window is hidden can wake its loop
    /// instead of polling. Events are still collected by [`poll`](Self::poll).
    pub fn set_waker(&mut self, waker: impl Fn() + Send + Sync + 'static) {
        match &mut self.backend {
            Backend::Global { events, .. } => {
                let (tx, rx) = std::sync::mpsc::channel();
                GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                    let _ = tx.send(event);
                    waker();
                }));
                *events = Some(rx);
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            Backend::Portal(p) => p.set_waker(std::sync::Arc::new(waker)),
        }
    }

    /// The next triggered action, if any. Drains the event channel (key
    /// presses only; releases are ignored) without blocking; call it from
    /// the event loop on every wake-up.
    ///
    /// Without [`set_waker`](Self::set_waker) the global backend reads
    /// `GlobalHotKeyEvent::receiver()`, so nothing else in the process may
    /// install `GlobalHotKeyEvent::set_event_handler`.
    pub fn poll(&mut self) -> Option<Action> {
        self.poll_with_text().map(|(a, _)| a)
    }

    /// [`poll`](Self::poll), with the text that goes with the action (a
    /// panel's name for [`Action::OpenPanel`]).
    pub fn poll_with_text(&mut self) -> Option<(Action, Option<String>)> {
        match &mut self.backend {
            Backend::Global { registered, events, .. } => loop {
                let event = match events {
                    Some(rx) => rx.try_recv().ok(),
                    None => GlobalHotKeyEvent::receiver().try_recv().ok(),
                };
                let Some(event) = event else { break };
                if event.state() != HotKeyState::Pressed {
                    continue;
                }
                if let Some((_, action, text)) = registered.get(&event.id()) {
                    self.queue.push_back((*action, text.clone()));
                }
            },
            #[cfg(all(unix, not(target_os = "macos")))]
            Backend::Portal(p) => self.queue.extend(p.drain().into_iter().map(|a| (a, None))),
        }
        self.queue.pop_front()
    }
}

type Registered = HashMap<u32, (HotKey, Action, Option<String>)>;

/// Bind `hotkey` to `action` with `text`, replacing what that pair had.
fn register_global(
    manager: &GlobalHotKeyManager,
    registered: &mut Registered,
    action: Action,
    text: Option<String>,
    hotkey: &Hotkey,
) -> Result<(), HotkeyError> {
    let hk = to_global(hotkey)?;
    if let Some((_, other, other_text)) = registered.get(&hk.id()) {
        if *other == action && *other_text == text {
            return Ok(());
        }
        return Err(HotkeyError::Taken { hotkey: hotkey.to_string() });
    }
    unregister_global(manager, registered, action, text.as_deref());
    manager.register(hk).map_err(|e| match e {
        global_hotkey::Error::AlreadyRegistered(_) => HotkeyError::Taken { hotkey: hotkey.to_string() },
        other => HotkeyError::Failed { hotkey: hotkey.to_string(), detail: other.to_string() },
    })?;
    registered.insert(hk.id(), (hk, action, text));
    Ok(())
}

fn unregister_global(manager: &GlobalHotKeyManager, registered: &mut Registered, action: Action, text: Option<&str>) {
    let ids: Vec<u32> =
        registered.iter().filter(|(_, (_, a, t))| *a == action && t.as_deref() == text).map(|(id, _)| *id).collect();
    for id in ids {
        if let Some((hk, _, _)) = registered.remove(&id) {
            if let Err(e) = manager.unregister(hk) {
                tracing::debug!("unregistering hotkey failed: {e}");
            }
        }
    }
}

impl Drop for HotkeyService {
    fn drop(&mut self) {
        self.unregister_all();
    }
}

/// `hotkey` as a trigger in the XDG shortcut syntax, which the
/// GlobalShortcuts portal takes as a preferred trigger: modifiers `CTRL`,
/// `ALT`, `SHIFT`, `LOGO` joined by `+`, then the key's XKB keysym name,
/// letters in lower case (`CTRL+SHIFT+q`, `LOGO+ALT+Print`).
pub fn portal_trigger(hotkey: &Hotkey) -> Result<String, HotkeyError> {
    let key = keysym(&hotkey.key.0).ok_or_else(|| HotkeyError::UnknownKey(hotkey.key.0.clone()))?;
    let mut out = String::new();
    for (on, name) in
        [(hotkey.mods.ctrl, "CTRL"), (hotkey.mods.alt, "ALT"), (hotkey.mods.shift, "SHIFT"), (hotkey.mods.meta, "LOGO")]
    {
        if on {
            out.push_str(name);
            out.push('+');
        }
    }
    out.push_str(&key);
    Ok(out)
}

/// The XKB keysym name for a canonical glossa key name.
fn keysym(name: &str) -> Option<String> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            // Letters are their lower-case keysyms (`q`, not `Q`, which is
            // Shift+q); digits are themselves.
            return Some(c.to_ascii_lowercase().to_string());
        }
    }
    let sym = match name {
        "Backspace" => "BackSpace",
        "Tab" => "Tab",
        "Enter" => "Return",
        "Pause" => "Pause",
        "CapsLock" => "Caps_Lock",
        "Escape" => "Escape",
        "Space" => "space",
        "PageUp" => "Page_Up",
        "PageDown" => "Page_Down",
        "End" => "End",
        "Home" => "Home",
        "Left" => "Left",
        "Up" => "Up",
        "Right" => "Right",
        "Down" => "Down",
        "PrintScreen" => "Print",
        "Insert" => "Insert",
        "Delete" => "Delete",
        "NumpadMultiply" => "KP_Multiply",
        "NumpadAdd" => "KP_Add",
        "NumpadSubtract" => "KP_Subtract",
        "NumpadDecimal" => "KP_Decimal",
        "NumpadDivide" => "KP_Divide",
        "ScrollLock" => "Scroll_Lock",
        ";" => "semicolon",
        "=" => "equal",
        "," => "comma",
        "-" => "minus",
        "." => "period",
        "/" => "slash",
        "`" => "grave",
        "[" => "bracketleft",
        "\\" => "backslash",
        "]" => "bracketright",
        "'" => "apostrophe",
        other => {
            if let Some(n) = other.strip_prefix("Numpad").filter(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit()) {
                return Some(format!("KP_{n}"));
            }
            if let Some(n) = other.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=24).contains(n))
            {
                return Some(format!("F{n}"));
            }
            return None;
        }
    };
    Some(sym.to_owned())
}

/// The GlobalShortcuts portal behind [`HotkeyService`] on Wayland.
///
/// One thread binds: it waits for changes to the wanted chords, lets them
/// settle for a moment (the app re-registers everything at once), and then
/// asks the portal for a session with exactly those shortcuts, closing the
/// previous one. An unchanged set is not bound again, so applying the
/// settings does not bring the desktop's dialog back. Another thread
/// forwards the `Activated` signals of the current session as actions.
#[cfg(all(unix, not(target_os = "macos")))]
mod portal_backend {
    use std::collections::BTreeMap;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures_lite::StreamExt;
    use glossa_core::Action;
    use zbus::zvariant::OwnedObjectPath;

    use crate::portal::{self, Portal, PortalError, Shortcut};

    type Waker = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

    /// How long the wanted chords must stay unchanged before binding.
    const SETTLE: Duration = Duration::from_millis(300);
    /// How long [`PortalHotkeys::start`] waits for the portal to say whether
    /// it has the interface. Past that it is assumed to (and the binding
    /// thread logs it if not).
    const PROBE_WAIT: Duration = Duration::from_secs(2);
    /// The desktop may show a dialog for the user to confirm the shortcuts.
    const BIND_TIMEOUT: Duration = Duration::from_secs(300);

    pub struct PortalHotkeys {
        wanted: Arc<Mutex<BTreeMap<Action, String>>>,
        changed: mpsc::Sender<()>,
        events: mpsc::Receiver<Action>,
        waker: Waker,
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }

    impl PortalHotkeys {
        /// Start the binding thread on `portal` (the shared session one when
        /// `None`). Fails when there is no session bus or the portal has no
        /// GlobalShortcuts interface.
        pub fn start(portal: Option<Portal>) -> Result<PortalHotkeys, String> {
            let wanted = Arc::new(Mutex::new(BTreeMap::new()));
            let waker: Waker = Arc::new(Mutex::new(None));
            let (changed, changes) = mpsc::channel();
            let (events_tx, events) = mpsc::channel();
            let (ready_tx, ready) = mpsc::sync_channel(1);
            let shared = Shared { wanted: wanted.clone(), waker: waker.clone(), events: events_tx };
            std::thread::Builder::new()
                .name("glossa-shortcuts".into())
                .stack_size(512 * 1024)
                .spawn(move || {
                    let portal = match portal.map(Ok).unwrap_or_else(portal::shared) {
                        Ok(p) => p,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                            return;
                        }
                    };
                    match async_io::block_on(portal.version(portal::GLOBAL_SHORTCUTS)) {
                        Ok(v) => {
                            tracing::debug!("GlobalShortcuts portal, version {v}");
                            let _ = ready_tx.send(Ok(()));
                        }
                        Err(e) => {
                            tracing::info!("no global shortcuts portal: {e}");
                            let _ = ready_tx.send(Err(e.to_string()));
                            return;
                        }
                    }
                    run(portal, changes, shared);
                })
                .map_err(|e| e.to_string())?;
            match ready.recv_timeout(PROBE_WAIT) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e),
                Err(RecvTimeoutError::Timeout) => {
                    tracing::warn!("the desktop portal is slow to answer; assuming it has global shortcuts")
                }
                Err(RecvTimeoutError::Disconnected) => return Err("the portal thread stopped".into()),
            }
            Ok(PortalHotkeys { wanted, changed, events, waker })
        }

        /// Want `action` on `trigger`. Refused when another action already
        /// has the same trigger.
        pub fn set(&self, action: Action, trigger: String) -> Result<(), ()> {
            let mut wanted = lock(&self.wanted);
            if wanted.iter().any(|(a, t)| *a != action && *t == trigger) {
                return Err(());
            }
            wanted.insert(action, trigger);
            drop(wanted);
            let _ = self.changed.send(());
            Ok(())
        }

        pub fn remove(&self, action: Action) {
            if lock(&self.wanted).remove(&action).is_some() {
                let _ = self.changed.send(());
            }
        }

        pub fn clear(&self) {
            lock(&self.wanted).clear();
            let _ = self.changed.send(());
        }

        pub fn actions(&self) -> Vec<Action> {
            lock(&self.wanted).keys().copied().collect()
        }

        pub fn set_waker(&self, waker: Arc<dyn Fn() + Send + Sync>) {
            *lock(&self.waker) = Some(waker);
        }

        /// The actions activated since the last call.
        pub fn drain(&self) -> Vec<Action> {
            self.events.try_iter().collect()
        }
    }

    /// What the binding thread shares with the service.
    struct Shared {
        wanted: Arc<Mutex<BTreeMap<Action, String>>>,
        waker: Waker,
        events: mpsc::Sender<Action>,
    }

    /// The shortcuts to bind for `wanted`, described in English (the
    /// desktop shows the descriptions in its dialog and settings).
    pub(crate) fn shortcuts(wanted: &BTreeMap<Action, String>) -> Vec<Shortcut> {
        let english = glossa_core::Locale::english();
        wanted
            .iter()
            .map(|(action, trigger)| Shortcut {
                id: action.id().to_owned(),
                description: english.get(&format!("action.{}", action.id())).to_owned(),
                preferred_trigger: Some(trigger.clone()),
            })
            .collect()
    }

    fn run(portal: Portal, changes: mpsc::Receiver<()>, shared: Shared) {
        // The session whose activations count; older sessions' are ignored.
        let current: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        match async_io::block_on(portal.activations()) {
            Ok(stream) => {
                let (current, waker, events) = (current.clone(), shared.waker.clone(), shared.events.clone());
                let spawned = std::thread::Builder::new()
                    .name("glossa-shortcut-events".into())
                    .stack_size(256 * 1024)
                    .spawn(move || forward(stream, current, waker, events));
                if let Err(e) = spawned {
                    tracing::warn!("global shortcuts: {e}");
                    return;
                }
            }
            Err(e) => {
                tracing::warn!("global shortcuts: cannot listen to the portal: {e}");
                return;
            }
        }
        let mut bound: Option<(OwnedObjectPath, BTreeMap<Action, String>)> = None;
        let close = |bound: &mut Option<(OwnedObjectPath, BTreeMap<Action, String>)>| {
            if let Some((session, _)) = bound.take() {
                *lock(&current) = None;
                async_io::block_on(portal.close_session(&session.as_ref()));
            }
        };
        loop {
            if changes.recv().is_err() {
                break;
            }
            // Let a burst of changes (unregister all, register each) settle.
            loop {
                match changes.recv_timeout(SETTLE) {
                    Ok(()) => continue,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        close(&mut bound);
                        return;
                    }
                }
            }
            let want = lock(&shared.wanted).clone();
            if bound.as_ref().map(|(_, b)| b) == Some(&want) {
                continue;
            }
            close(&mut bound);
            if want.is_empty() {
                continue;
            }
            match async_io::block_on(bind(&portal, &want)) {
                Ok((session, triggers)) => {
                    let list: Vec<String> = triggers.iter().map(|(id, t)| format!("{id} = {t:?}")).collect();
                    tracing::info!("global shortcuts bound by the desktop: {}", list.join(", "));
                    *lock(&current) = Some(session.as_str().to_owned());
                    bound = Some((session, want));
                }
                Err(PortalError::Cancelled) => {
                    tracing::warn!("global shortcuts: the desktop's dialog was cancelled; none are bound")
                }
                Err(e) => tracing::warn!("global shortcuts: {e}"),
            }
        }
        close(&mut bound);
    }

    async fn bind(
        portal: &Portal,
        want: &BTreeMap<Action, String>,
    ) -> Result<(OwnedObjectPath, Vec<(String, String)>), PortalError> {
        let session = portal.create_shortcuts_session().await?;
        match portal.bind_shortcuts(&session.as_ref(), &shortcuts(want), BIND_TIMEOUT).await {
            Ok(triggers) => Ok((session, triggers)),
            Err(e) => {
                portal.close_session(&session.as_ref()).await;
                Err(e)
            }
        }
    }

    fn forward(
        mut stream: zbus::MessageStream,
        current: Arc<Mutex<Option<String>>>,
        waker: Waker,
        events: mpsc::Sender<Action>,
    ) {
        async_io::block_on(async {
            while let Some(message) = stream.next().await {
                let Ok(message) = message else { continue };
                let Some((session, id)) = portal::activation(&message) else { continue };
                if lock(&current).as_deref() != Some(session.as_str()) {
                    continue;
                }
                let Some(action) = Action::from_id(&id) else { continue };
                if events.send(action).is_err() {
                    break;
                }
                let wake = lock(&waker).clone();
                if let Some(wake) = wake {
                    wake();
                }
            }
        });
    }
}

/// Convert our hotkey to global-hotkey's.
pub fn to_global(hotkey: &Hotkey) -> Result<HotKey, HotkeyError> {
    let code = key_code(&hotkey.key.0).ok_or_else(|| HotkeyError::UnknownKey(hotkey.key.0.clone()))?;
    let mut mods = Modifiers::empty();
    if hotkey.mods.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if hotkey.mods.alt {
        mods |= Modifiers::ALT;
    }
    if hotkey.mods.shift {
        mods |= Modifiers::SHIFT;
    }
    if hotkey.mods.meta {
        mods |= Modifiers::META;
    }
    Ok(HotKey::new(Some(mods), code))
}

/// Map a canonical glossa key name (as `glossa_core::hotkey` spells them)
/// to a physical key code.
pub fn key_code(name: &str) -> Option<Code> {
    use Code::*;
    let code = match name {
        "Backspace" => Backspace,
        "Tab" => Tab,
        "Enter" => Enter,
        "Pause" => Pause,
        "CapsLock" => CapsLock,
        "Escape" => Escape,
        "Space" => Space,
        "PageUp" => PageUp,
        "PageDown" => PageDown,
        "End" => End,
        "Home" => Home,
        "Left" => ArrowLeft,
        "Up" => ArrowUp,
        "Right" => ArrowRight,
        "Down" => ArrowDown,
        "PrintScreen" => PrintScreen,
        "Insert" => Insert,
        "Delete" => Delete,
        "0" => Digit0,
        "1" => Digit1,
        "2" => Digit2,
        "3" => Digit3,
        "4" => Digit4,
        "5" => Digit5,
        "6" => Digit6,
        "7" => Digit7,
        "8" => Digit8,
        "9" => Digit9,
        "A" => KeyA,
        "B" => KeyB,
        "C" => KeyC,
        "D" => KeyD,
        "E" => KeyE,
        "F" => KeyF,
        "G" => KeyG,
        "H" => KeyH,
        "I" => KeyI,
        "J" => KeyJ,
        "K" => KeyK,
        "L" => KeyL,
        "M" => KeyM,
        "N" => KeyN,
        "O" => KeyO,
        "P" => KeyP,
        "Q" => KeyQ,
        "R" => KeyR,
        "S" => KeyS,
        "T" => KeyT,
        "U" => KeyU,
        "V" => KeyV,
        "W" => KeyW,
        "X" => KeyX,
        "Y" => KeyY,
        "Z" => KeyZ,
        "Numpad0" => Numpad0,
        "Numpad1" => Numpad1,
        "Numpad2" => Numpad2,
        "Numpad3" => Numpad3,
        "Numpad4" => Numpad4,
        "Numpad5" => Numpad5,
        "Numpad6" => Numpad6,
        "Numpad7" => Numpad7,
        "Numpad8" => Numpad8,
        "Numpad9" => Numpad9,
        "NumpadMultiply" => NumpadMultiply,
        "NumpadAdd" => NumpadAdd,
        "NumpadSubtract" => NumpadSubtract,
        "NumpadDecimal" => NumpadDecimal,
        "NumpadDivide" => NumpadDivide,
        "F1" => F1,
        "F2" => F2,
        "F3" => F3,
        "F4" => F4,
        "F5" => F5,
        "F6" => F6,
        "F7" => F7,
        "F8" => F8,
        "F9" => F9,
        "F10" => F10,
        "F11" => F11,
        "F12" => F12,
        "F13" => F13,
        "F14" => F14,
        "F15" => F15,
        "F16" => F16,
        "F17" => F17,
        "F18" => F18,
        "F19" => F19,
        "F20" => F20,
        "F21" => F21,
        "F22" => F22,
        "F23" => F23,
        "F24" => F24,
        "ScrollLock" => ScrollLock,
        ";" => Semicolon,
        "=" => Equal,
        "," => Comma,
        "-" => Minus,
        "." => Period,
        "/" => Slash,
        "`" => Backquote,
        "[" => BracketLeft,
        "\\" => Backslash,
        "]" => BracketRight,
        "'" => Quote,
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::hotkey::Modifiers as Mods;

    /// Every key name glossa-core accepts, enumerated through its public
    /// legacy decoder (each name has a virtual-key code), so a key added to
    /// the core table without a mapping here fails this test.
    fn all_core_keys() -> Vec<String> {
        (1u32..=255).filter_map(Hotkey::from_legacy).map(|h| h.key.0).collect()
    }

    #[test]
    fn every_core_key_has_a_code() {
        let keys = all_core_keys();
        assert!(keys.len() > 100, "expected the full key table, got {}", keys.len());
        for name in &keys {
            assert!(key_code(name).is_some(), "no global-hotkey code for {name:?}");
        }
    }

    #[test]
    fn the_mapping_is_one_to_one() {
        let mut codes: Vec<Code> = all_core_keys().iter().filter_map(|k| key_code(k)).collect();
        let n = codes.len();
        codes.sort_by_key(|c| *c as u32);
        codes.dedup();
        assert_eq!(codes.len(), n, "two key names map to the same code");
    }

    #[test]
    fn unknown_keys_are_reported() {
        let hk = Hotkey::new(Mods::default(), "Banana");
        assert_eq!(to_global(&hk), Err(HotkeyError::UnknownKey("Banana".into())));
    }

    #[test]
    fn modifiers_carry_over() {
        let hk: Hotkey = "Ctrl+Shift+Q".parse().unwrap();
        let g = to_global(&hk).unwrap();
        assert_eq!(g.key, Code::KeyQ);
        assert_eq!(g.mods, Modifiers::CONTROL | Modifiers::SHIFT);
        let hk: Hotkey = "Alt+Meta+F12".parse().unwrap();
        let g = to_global(&hk).unwrap();
        assert_eq!(g.mods, Modifiers::ALT | Modifiers::META);
        assert_eq!(g.key, Code::F12);
        // Distinct chords get distinct ids, which is what poll() relies on.
        let a = to_global(&"Ctrl+E".parse().unwrap()).unwrap();
        let b = to_global(&"Ctrl+Shift+E".parse().unwrap()).unwrap();
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn errors_convert_to_the_pipeline_error() {
        let e: glossa_core::Error = HotkeyError::Unsupported { reason: "Wayland".into() }.into();
        assert!(matches!(e, glossa_core::Error::Platform(ref m) if m.contains("Wayland")));
    }

    #[test]
    #[ignore = "needs a display; registers a real hotkey"]
    fn register_and_unregister() {
        let mut s = HotkeyService::new().unwrap();
        s.register(Action::ShowPopup, &"Ctrl+Alt+Shift+F11".parse().unwrap()).unwrap();
        assert_eq!(s.bound_actions(), vec![Action::ShowPopup]);
        assert_eq!(s.poll(), None);
        s.unregister_all();
        assert!(s.bound_actions().is_empty());
    }

    #[test]
    fn portal_triggers_use_the_xdg_shortcut_syntax() {
        let t = |s: &str| portal_trigger(&s.parse().unwrap()).unwrap();
        assert_eq!(t("Ctrl+Shift+Q"), "CTRL+SHIFT+q");
        assert_eq!(t("Alt+Meta+F12"), "ALT+LOGO+F12");
        assert_eq!(t("Meta+Alt+PrintScreen"), "ALT+LOGO+Print", "modifiers in a fixed order");
        assert_eq!(t("Ctrl+Alt+Shift+Meta+Enter"), "CTRL+ALT+SHIFT+LOGO+Return");
        assert_eq!(t("Ctrl+1"), "CTRL+1");
        assert_eq!(t("Ctrl+Numpad7"), "CTRL+KP_7");
        assert_eq!(t("Ctrl+NumpadAdd"), "CTRL+KP_Add");
        assert_eq!(t("Ctrl+Space"), "CTRL+space");
        assert_eq!(t("Ctrl+PageDown"), "CTRL+Page_Down");
        assert_eq!(t("Ctrl+Backspace"), "CTRL+BackSpace");
        assert_eq!(t("F24"), "F24");
        assert_eq!(t("Ctrl+'"), "CTRL+apostrophe");
        assert_eq!(t("Ctrl+\\"), "CTRL+backslash");
        assert_eq!(
            portal_trigger(&Hotkey::new(Mods::default(), "Banana")),
            Err(HotkeyError::UnknownKey("Banana".into()))
        );
    }

    #[test]
    fn every_core_key_has_a_keysym() {
        let mut seen = std::collections::HashSet::new();
        for name in all_core_keys() {
            let sym = keysym(&name).unwrap_or_else(|| panic!("no keysym for {name:?}"));
            assert!(sym.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "{name:?} -> {sym:?}");
            assert!(seen.insert(sym.clone()), "two keys map to {sym}");
        }
    }

    /// The portal backend against a fake portal on a session bus: one
    /// session with the configured chords, rebinding only on change, the
    /// old session closed, activations arriving as actions.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn hotkeys_through_a_fake_global_shortcuts_portal() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        use crate::portal::fake::FakePortal;
        use crate::portal::Portal;

        let Some(fake) = FakePortal::start("keys") else {
            eprintln!("no session bus; skipped (run under dbus-run-session)");
            return;
        };
        let conn = async_io::block_on(zbus::Connection::session()).unwrap();
        let backend = portal_backend::PortalHotkeys::start(Some(Portal::with(conn, &fake.name))).unwrap();
        let mut s = HotkeyService { backend: Backend::Portal(backend), queue: VecDeque::new() };
        assert!(s.uses_portal());
        let woken = std::sync::Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        s.set_waker(move || {
            w.fetch_add(1, Ordering::SeqCst);
        });

        let wait_for = |what: &str, done: &dyn Fn() -> bool| {
            let start = Instant::now();
            while !done() {
                assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };

        s.unregister_all();
        s.register(Action::ShowPopup, &"Ctrl+Shift+Q".parse().unwrap()).unwrap();
        s.register(Action::TextRecognition, &"Ctrl+Shift+O".parse().unwrap()).unwrap();
        assert_eq!(
            s.register(Action::ShowMainWindow, &"Ctrl+Shift+Q".parse().unwrap()),
            Err(HotkeyError::Taken { hotkey: "Ctrl+Shift+Q".into() })
        );
        assert_eq!(s.bound_actions(), vec![Action::ShowPopup, Action::TextRecognition]);
        wait_for("the first bind", &|| fake.state.lock().unwrap().binds.len() == 1);
        let (session, list) = fake.state.lock().unwrap().binds[0].clone();
        assert_eq!(
            list,
            vec![
                ("show_popup".into(), "Translate selection".into(), Some("CTRL+SHIFT+q".into())),
                ("text_recognition".into(), "Text recognition".into(), Some("CTRL+SHIFT+o".into())),
            ]
        );

        // Re-registering the same chords (applying the settings) binds nothing new.
        s.unregister_all();
        s.register(Action::ShowPopup, &"Ctrl+Shift+Q".parse().unwrap()).unwrap();
        s.register(Action::TextRecognition, &"Ctrl+Shift+O".parse().unwrap()).unwrap();
        std::thread::sleep(Duration::from_millis(700));
        assert_eq!(fake.state.lock().unwrap().binds.len(), 1, "an unchanged set is not bound again");

        fake.activate(&session, "text_recognition");
        fake.activate(&session, "no_such_action");
        wait_for("the activation", &|| woken.load(Ordering::SeqCst) > 0);
        assert_eq!(s.poll(), Some(Action::TextRecognition));
        assert_eq!(s.poll(), None);

        // A change: a new session, the old one closed and deaf.
        s.register(Action::ShowPopup, &"Ctrl+Alt+T".parse().unwrap()).unwrap();
        wait_for("the second bind", &|| fake.state.lock().unwrap().binds.len() == 2);
        let (new_session, list) = fake.state.lock().unwrap().binds[1].clone();
        assert_ne!(new_session, session);
        assert_eq!(list[0].2.as_deref(), Some("CTRL+ALT+t"));
        wait_for("the old session to close", &|| fake.state.lock().unwrap().closed.contains(&session));
        fake.activate(&session, "show_popup");
        fake.activate(&new_session, "show_popup");
        wait_for("the second activation", &|| woken.load(Ordering::SeqCst) > 1);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(s.poll(), Some(Action::ShowPopup));
        assert_eq!(s.poll(), None, "the closed session's activation is ignored");

        // Dropping the service closes its session.
        drop(s);
        wait_for("the last session to close", &|| fake.state.lock().unwrap().closed.contains(&new_session));
    }
}
