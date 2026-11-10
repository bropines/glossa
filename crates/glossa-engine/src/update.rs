//! Checking whether a newer Glossa has been released.
//!
//! One request to GitHub's releases list for the project, sent through the
//! engine's HTTP client so the user's proxy applies. Nothing is downloaded
//! or installed: the answer is "up to date" or "version X is available" with
//! the address of its release page, which the front end opens in the
//! browser.
//!
//! The repository also publishes things that are not the program (module
//! bundles such as `paddle-ocr-v0.1.0`), so only tags of the form `v1.2.3`
//! count, and versions are compared as semantic versions: `0.1.0-beta.2` is
//! older than `0.1.0` and newer than `0.1.0-beta.1`.
//!
//! The last check (when, and what it found) is kept in `update-check.json`
//! in the data folder, so the automatic check runs at most once a day across
//! restarts and the options page can say when the last one was.

use std::cmp::Ordering;
use std::sync::atomic::Ordering as Atomic;
use std::sync::Arc;
use std::time::Duration;

use glossa_core::{Error, Result};
use glossa_net::{Http, HttpRequest};
use serde::{Deserialize, Serialize};

use crate::{Engine, Event};

/// The releases of the project, newest first. Thirty is GitHub's default
/// page and far more than the releases between two checks.
pub const RELEASES_API: &str = "https://api.github.com/repos/bropines/glossa/releases?per_page=30";

/// Where releases are shown to people; every address handed to a browser
/// starts with this.
pub const RELEASES_PAGE: &str = "https://github.com/bropines/glossa/releases";

/// The name errors carry for this request.
const PROVIDER: &str = "GitHub";

/// An automatic check is made at most this often.
const CHECK_EVERY_SECS: i64 = 24 * 60 * 60;

/// After a check that failed (no network at start, say) the automatic one
/// tries again this much later rather than a whole day on.
const RETRY_AFTER_SECS: i64 = 60 * 60;

/// How long after the start the first automatic check waits, so it never
/// competes with the first translation for the network or the processor.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);

/// How often the schedule looks at the clock afterwards. The program stays
/// in the tray for weeks, so one look at start would not be enough.
const SCHEDULE_TICK: Duration = Duration::from_secs(30 * 60);

/// A semantic version: `1.2.3`, optionally with a pre-release
/// (`1.2.3-beta.2`). Build metadata (`+abc`) is accepted and ignored, as
/// the specification says it has no part in precedence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pre: Vec<Identifier>,
}

/// One dot-separated part of a pre-release.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Identifier {
    Number(u64),
    Text(String),
}

impl Version {
    /// Read `1.2.3` or `1.2.3-beta.2`. Anything else is not a version.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.split_once('+').map_or(text, |(v, _)| v);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = core.split('.');
        let mut next = || -> Option<u64> {
            let n = numbers.next()?;
            // `parse` alone would take "+1"; a version number is digits only.
            (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.parse().ok())?
        };
        let (major, minor, patch) = (next()?, next()?, next()?);
        if numbers.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|part| {
                    let valid = !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
                    valid.then(|| match part.parse::<u64>() {
                        Ok(n) if part.bytes().all(|b| b.is_ascii_digit()) => Identifier::Number(n),
                        _ => Identifier::Text(part.to_owned()),
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Version { major, minor, patch, pre })
    }

    /// The version a release tag names: `v0.1.0-beta.2`. Tags of other
    /// things the repository releases (`paddle-ocr-v0.1.0`) give `None`.
    pub fn from_tag(tag: &str) -> Option<Version> {
        Version::parse(tag.trim().strip_prefix('v')?)
    }

    /// Whether this is a pre-release (a beta, a release candidate).
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (i, part) in self.pre.iter().enumerate() {
            f.write_str(if i == 0 { "-" } else { "." })?;
            match part {
                Identifier::Number(n) => write!(f, "{n}")?,
                Identifier::Text(t) => f.write_str(t)?,
            }
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| {
            // A release is newer than any of its pre-releases; between two
            // pre-releases the parts compare in turn, numbers before words,
            // and the longer one wins when the shorter is its prefix.
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Identifier {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Identifier::Number(a), Identifier::Number(b)) => a.cmp(b),
            (Identifier::Number(_), Identifier::Text(_)) => Ordering::Less,
            (Identifier::Text(_), Identifier::Number(_)) => Ordering::Greater,
            (Identifier::Text(a), Identifier::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Identifier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A released version newer than the running one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// `0.2.0`, without the tag's `v`.
    pub version: String,
    /// The release's page, for a browser.
    pub url: String,
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    Available(Release),
}

/// The last check, as kept in `update-check.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateState {
    /// When a check last got an answer (Unix seconds); 0 when none has.
    pub checked_at: i64,
    /// When a check was last tried, answered or not.
    pub attempted_at: i64,
    /// The newer version the last answered check found, if any.
    pub available: Option<Release>,
    /// Why the last try failed; `None` when it was answered.
    pub error: Option<String>,
    /// The version the user has already been told about, so an automatic
    /// check announces each new version once rather than daily.
    pub announced: String,
}

impl UpdateState {
    /// The newer version to offer to a program of version `current`. A
    /// version found before the user upgraded is no longer news.
    pub fn newer_than(&self, current: &str) -> Option<&Release> {
        let current = Version::parse(current)?;
        self.available.as_ref().filter(|r| Version::parse(&r.version).is_some_and(|v| v > current))
    }

    /// Whether an automatic check should run at `now`.
    pub fn due(&self, now: i64) -> bool {
        // A clock set back must not postpone the check for ever, hence the
        // absolute differences.
        (now - self.checked_at).abs() >= CHECK_EVERY_SECS && (now - self.attempted_at).abs() >= RETRY_AFTER_SECS
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// The newest release in GitHub's list that a program of version `current`
/// should hear about, or `None` when it runs the newest.
///
/// Pre-releases are offered only to someone already running one: a user on
/// a stable version asked for stable versions.
pub fn newest_release(json: &str, current: &Version) -> Result<Option<Release>> {
    let list: Vec<ApiRelease> = serde_json::from_str(json)
        .map_err(|e| Error::BadResponse { provider: PROVIDER.into(), detail: e.to_string() })?;
    let newest = list
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let version = Version::from_tag(&r.tag_name)?;
            let wanted = current.is_prerelease() || !(r.prerelease || version.is_prerelease());
            wanted.then_some((version, r))
        })
        .max_by(|a, b| a.0.cmp(&b.0));
    Ok(newest.filter(|(v, _)| v > current).map(|(version, r)| {
        // The address goes to a browser, so it is taken from the reply only
        // when it is where releases live; anything else is rebuilt from the
        // tag, which was just parsed and holds nothing but a version.
        let url = if r.html_url.starts_with(&format!("{RELEASES_PAGE}/")) {
            r.html_url
        } else {
            format!("{RELEASES_PAGE}/tag/{}", r.tag_name.trim())
        };
        Release { version: version.to_string(), url }
    }))
}

/// Ask GitHub whether a version newer than `current` has been released.
pub fn check(http: &dyn Http, current: &str) -> Result<UpdateStatus> {
    let current =
        Version::parse(current).ok_or_else(|| Error::Config(format!("{current:?} is not a version number")))?;
    let request = HttpRequest::get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .provider(PROVIDER)
        .timeout(Duration::from_secs(15));
    let response = http.send(request)?.error_for_status(PROVIDER)?;
    Ok(match newest_release(&response.text(), &current)? {
        Some(release) => UpdateStatus::Available(release),
        None => UpdateStatus::UpToDate,
    })
}

impl Engine {
    /// The last update check: when, what it found, why it failed.
    pub fn update_state(&self) -> UpdateState {
        self.inner.update.lock().clone()
    }

    /// Whether a check is on its way ([`Engine::check_for_updates_async`]).
    pub fn update_check_running(&self) -> bool {
        self.inner.update_busy.load(Atomic::SeqCst)
    }

    /// Check for a newer version now, blocking the caller, and remember the
    /// answer. Nothing is downloaded.
    pub fn check_for_updates(&self) -> Result<UpdateStatus> {
        let http = self.inner.http.read().clone();
        let result = check(http.as_ref(), glossa_core::VERSION);
        self.record_update_check(&result, glossa_core::now_unix());
        result
    }

    /// The same on a worker; the answer arrives as [`Event::UpdateChecked`].
    /// A check already running is not doubled. `automatic` marks one nobody
    /// asked for, which a front end reports only when it found something.
    pub fn check_for_updates_async(&self, automatic: bool) {
        if self.inner.update_busy.swap(true, Atomic::SeqCst) {
            return;
        }
        let me = self.clone();
        self.spawn("update-check", move || {
            let result = glossa_core::error::catch_panic("update check", || me.check_for_updates());
            me.inner.update_busy.store(false, Atomic::SeqCst);
            me.emit(Event::UpdateChecked { automatic, result: result.map_err(Arc::new) });
        });
    }

    /// Note that the user has been told about `version`, so the automatic
    /// check does not announce it again.
    pub fn mark_update_announced(&self, version: &str) {
        let state = {
            let mut state = self.inner.update.lock();
            if state.announced == version {
                return;
            }
            state.announced = version.to_owned();
            state.clone()
        };
        // Front ends call this from their interface thread, which must not
        // wait for the disk.
        let me = self.clone();
        self.spawn("update-state", move || me.save_update_state(&state));
    }

    /// Run the automatic check for as long as the engine lives: a little
    /// after the start and then whenever a day has passed, when
    /// `[updates] check_automatically` is on. The desktop program calls
    /// this once; the command line never does.
    pub fn start_update_schedule(&self) {
        // The thread must not keep the engine alive, so it holds a weak
        // reference and ends when the engine is gone.
        let weak = Arc::downgrade(&self.inner);
        let spawned = std::thread::Builder::new().name("glossa-update-schedule".into()).spawn(move || {
            std::thread::sleep(FIRST_CHECK_DELAY);
            loop {
                let Some(inner) = weak.upgrade() else { return };
                let engine = Engine { inner };
                if engine.config().updates.check_automatically && engine.update_state().due(glossa_core::now_unix()) {
                    engine.check_for_updates_async(true);
                }
                drop(engine);
                std::thread::sleep(SCHEDULE_TICK);
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("could not start the update schedule: {e}");
        }
    }

    fn record_update_check(&self, result: &Result<UpdateStatus>, now: i64) {
        let state = {
            let mut state = self.inner.update.lock();
            state.attempted_at = now;
            match result {
                Ok(status) => {
                    state.checked_at = now;
                    state.error = None;
                    state.available = match status {
                        UpdateStatus::Available(r) => Some(r.clone()),
                        UpdateStatus::UpToDate => None,
                    };
                }
                Err(e) => {
                    tracing::warn!("update check failed: {e}");
                    state.error = Some(e.to_string());
                }
            }
            state.clone()
        };
        self.save_update_state(&state);
    }

    fn save_update_state(&self, state: &UpdateState) {
        let path = self.inner.paths.update_state_file();
        let written = serde_json::to_string_pretty(state)
            .map_err(std::io::Error::other)
            .and_then(|text| std::fs::write(&path, text));
        if let Err(e) = written {
            // Not worth a message to the user: the cost is one more check
            // after the next start.
            tracing::warn!("could not write {}: {e}", path.display());
        }
    }
}

/// Read `update-check.json`; a missing or unreadable file is "never checked".
pub(crate) fn load_state(path: &std::path::Path) -> UpdateState {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} should parse"))
    }

    #[test]
    fn versions_parse_and_print() {
        assert_eq!(v("1.2.3").to_string(), "1.2.3");
        assert_eq!(v("0.1.0-beta.2").to_string(), "0.1.0-beta.2");
        assert_eq!(v(" 1.0.0-rc.1+build.5 ").to_string(), "1.0.0-rc.1");
        assert!(v("0.1.0-beta.2").is_prerelease());
        assert!(!v("0.1.0").is_prerelease());
        for bad in ["", "1", "1.2", "1.2.3.4", "1.2.x", "v1.2.3", "1.2.3-", "1.2.3-a..b", "1.+2.3", "1.2.3-β"] {
            assert!(Version::parse(bad).is_none(), "{bad:?} is not a version");
        }
    }

    #[test]
    fn versions_compare_as_semantic_versions() {
        // The order the specification gives as its example.
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "1.10.0",
            "2.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} should be older than {}", pair[0], pair[1]);
            assert!(v(pair[1]) > v(pair[0]));
        }
        assert_eq!(v("0.1.0-beta.2"), v("0.1.0-beta.2"));
        assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), Ordering::Equal);
        // Numbers compare as numbers, not as text.
        assert!(v("0.1.0-beta.2") < v("0.1.0-beta.10"));
        assert!(v("0.9.0") < v("0.10.0"));
    }

    #[test]
    fn only_program_tags_are_versions() {
        assert_eq!(Version::from_tag("v0.1.0-beta.2"), Some(v("0.1.0-beta.2")));
        assert_eq!(Version::from_tag("v1.0.0"), Some(v("1.0.0")));
        for other in ["paddle-ocr-v0.1.0", "0.1.0", "vnext", "v1.0", "nightly", ""] {
            assert!(Version::from_tag(other).is_none(), "{other:?} is not a program release");
        }
    }

    fn releases(list: &[(&str, bool, bool)]) -> String {
        let items: Vec<serde_json::Value> = list
            .iter()
            .map(|(tag, prerelease, draft)| {
                serde_json::json!({
                    "tag_name": tag,
                    "html_url": format!("{RELEASES_PAGE}/tag/{tag}"),
                    "prerelease": prerelease,
                    "draft": draft,
                    "name": tag,
                })
            })
            .collect();
        serde_json::Value::Array(items).to_string()
    }

    #[test]
    fn the_newest_program_release_is_offered() {
        let json = releases(&[
            // A module bundle with a higher number must not be mistaken for
            // the program, wherever it sits in the list.
            ("paddle-ocr-v9.0.0", false, false),
            ("v0.1.0-beta.2", true, false),
            ("v0.1.0-beta.3", true, false),
            ("v0.1.0-beta.1", true, false),
        ]);
        let found = newest_release(&json, &v("0.1.0-beta.2")).unwrap().expect("beta.3 is newer");
        assert_eq!(found.version, "0.1.0-beta.3");
        assert_eq!(found.url, format!("{RELEASES_PAGE}/tag/v0.1.0-beta.3"));
        assert_eq!(newest_release(&json, &v("0.1.0-beta.3")).unwrap(), None);
        assert_eq!(newest_release(&json, &v("0.1.0")).unwrap(), None, "a release is newer than its betas");
        assert_eq!(newest_release("[]", &v("0.1.0")).unwrap(), None);
    }

    #[test]
    fn prereleases_and_drafts_are_offered_only_where_they_belong() {
        let json = releases(&[("v0.3.0", false, true), ("v0.2.0-beta.1", true, false), ("v0.1.1", false, false)]);
        // A stable version hears about stable versions only.
        assert_eq!(newest_release(&json, &v("0.1.0")).unwrap().unwrap().version, "0.1.1");
        // Someone on a beta hears about the next beta; nobody about a draft.
        assert_eq!(newest_release(&json, &v("0.1.0-beta.2")).unwrap().unwrap().version, "0.2.0-beta.1");
        // A pre-release by its number counts as one even when the flag was
        // forgotten on the release.
        let unflagged = releases(&[("v0.2.0-rc.1", false, false)]);
        assert_eq!(newest_release(&unflagged, &v("0.1.0")).unwrap(), None);
    }

    #[test]
    fn an_address_from_elsewhere_is_not_handed_to_the_browser() {
        let json = r#"[{"tag_name":"v2.0.0","html_url":"https://example.test/evil","prerelease":false,"draft":false}]"#;
        let found = newest_release(json, &v("1.0.0")).unwrap().unwrap();
        assert_eq!(found.url, format!("{RELEASES_PAGE}/tag/v2.0.0"));
    }

    #[test]
    fn a_check_asks_github_once_and_reads_the_answer() {
        let http = MockHttp::new().on(
            Method::Get,
            RELEASES_API,
            200,
            &releases(&[("paddle-ocr-v0.1.0", false, false), ("v0.2.0", false, false), ("v0.1.0", false, false)]),
        );
        let found = check(&http, "0.1.0").unwrap();
        assert_eq!(
            found,
            UpdateStatus::Available(Release { version: "0.2.0".into(), url: format!("{RELEASES_PAGE}/tag/v0.2.0") })
        );
        assert_eq!(check(&http, "0.2.0").unwrap(), UpdateStatus::UpToDate);
        let seen = http.requests();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].url, RELEASES_API);
        assert!(seen[0].body.is_none(), "nothing about the user is sent");
        assert!(seen[0].headers.iter().any(|(k, v)| k == "Accept" && v == "application/vnd.github+json"));
    }

    #[test]
    fn a_check_that_cannot_be_answered_is_an_error() {
        let refused = MockHttp::new().on(Method::Get, RELEASES_API, 500, "oops");
        assert!(matches!(check(&refused, "0.1.0"), Err(Error::HttpStatus { status: 500, .. })));
        let garbage = MockHttp::new().on(Method::Get, RELEASES_API, 200, "<html>");
        assert!(matches!(check(&garbage, "0.1.0"), Err(Error::BadResponse { .. })));
        let offline = MockHttp::new();
        assert!(matches!(check(&offline, "0.1.0"), Err(Error::Network(_))));
    }

    #[test]
    fn the_automatic_check_runs_once_a_day_and_retries_sooner_after_a_failure() {
        let now = 1_800_000_000;
        assert!(UpdateState::default().due(now), "never checked");
        let fresh = UpdateState { checked_at: now - 3600, attempted_at: now - 3600, ..Default::default() };
        assert!(!fresh.due(now));
        let old = UpdateState { checked_at: now - 25 * 3600, attempted_at: now - 25 * 3600, ..Default::default() };
        assert!(old.due(now));
        let just_failed = UpdateState { checked_at: now - 25 * 3600, attempted_at: now - 60, ..Default::default() };
        assert!(!just_failed.due(now), "a failure is not retried at once");
        let failed_a_while_ago = UpdateState { attempted_at: now - 2 * 3600, ..just_failed };
        assert!(failed_a_while_ago.due(now));
    }

    #[test]
    fn the_last_check_is_remembered_and_an_installed_version_is_no_longer_news() {
        let (engine, dir) = crate::tests::engine("update-state");
        assert_eq!(engine.update_state(), UpdateState::default());
        let release = Release { version: "99.0.0".into(), url: format!("{RELEASES_PAGE}/tag/v99.0.0") };
        engine.record_update_check(&Ok(UpdateStatus::Available(release.clone())), 1000);
        engine.record_update_check(&Err(Error::Network("down".into())), 2000);
        engine.mark_update_announced("99.0.0");

        // The announcement is written on a worker.
        let file = dir.join("home").join("update-check.json");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while load_state(&file).announced.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let saved = load_state(&file);
        assert_eq!(saved, engine.update_state());
        assert_eq!((saved.checked_at, saved.attempted_at), (1000, 2000));
        assert_eq!(saved.error.as_deref(), Some("network error: down"));
        assert_eq!(saved.announced, "99.0.0");
        // The failure did not forget what the answered check found.
        assert_eq!(saved.newer_than("0.1.0"), Some(&release));
        assert_eq!(saved.newer_than("99.0.0"), None);
        assert_eq!(saved.newer_than("100.0.0"), None);

        engine.record_update_check(&Ok(UpdateStatus::UpToDate), 3000);
        let state = engine.update_state();
        assert_eq!((state.available, state.error), (None, None));
    }
}
