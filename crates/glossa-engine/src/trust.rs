//! Approving modules installed by hand, from the engine's side: which
//! modules wait, what they ask for, approving, revoking and declining, the
//! policy the registry loads with, and the one-time migration that grants
//! modules installed before approvals existed. The desktop app,
//! `glossa-cli module approve|revoke|pending` and the control API's
//! `/v1/trust` all come through here. The decision itself is
//! [`glossa_modules::trust`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glossa_core::config::{Config, ModuleGrant};
use glossa_core::{Error, Result};
use glossa_modules::trust::{TrustPolicy, TrustRequest};
use glossa_modules::{Manifest, Origin};
use serde::Serialize;

use crate::Engine;

/// A module that needs the user's approval, and where it stands.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleTrust {
    pub id: String,
    pub name: String,
    /// The module's folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    /// Waiting for approval: listed, but does not run.
    pub pending: bool,
    /// Never approved before (a new module), as opposed to an update that
    /// asks for more.
    pub first_time: bool,
    /// Everything it asks for.
    pub request: TrustRequest,
    /// What it asks for beyond its current grant (all of it the first
    /// time), for the prompt to point out.
    pub new: TrustRequest,
    /// The hash of `request`; pass it to [`Engine::approve_module`] to
    /// approve exactly what was shown.
    pub hash: String,
}

impl Engine {
    /// How the registry decides which modules may run under `cfg`.
    pub(crate) fn trust_policy(&self, cfg: &Config) -> TrustPolicy {
        let user = self.inner.paths.user_module_dir();
        let mut trusted_dirs: Vec<PathBuf> = self
            .inner
            .paths
            .module_dirs()
            .into_iter()
            .filter(|d| *d != user && !cfg.modules.dirs.contains(d))
            .collect();
        if let Some(classic) = cfg.classic.services_dir() {
            trusted_dirs.push(classic);
        }
        // Before the migration has run, everything loads as it did, so the
        // migration can record what each module declares.
        let granted = cfg.modules.trust_initialised.then(|| Arc::new(cfg.modules.granted.clone()));
        TrustPolicy { granted, trusted_dirs }
    }

    /// The first time this version runs with a settings file: grant every
    /// module now loaded what it declares, so updating Glossa does not stop
    /// modules that were working. Called by `reload_modules` while the
    /// policy still approves everything.
    pub(crate) fn initialise_trust(&self) {
        let registry = self.registry();
        let snapshot = {
            let mut cfg = self.inner.config.write();
            if cfg.modules.trust_initialised {
                return;
            }
            for e in registry.entries() {
                if let (Some(request), false) = (&e.trust, e.pending) {
                    cfg.modules.granted.entry(e.info().id.clone()).or_insert_with(|| request.to_grant());
                }
            }
            cfg.modules.trust_initialised = true;
            cfg.clone()
        };
        tracing::info!(granted = snapshot.modules.granted.len(), "granted installed modules what they declare");
        if let Err(e) = self.write_config(&snapshot) {
            tracing::warn!("could not save the module approvals: {e}");
        }
    }

    /// Every module loaded that needs approval, approved or not.
    pub fn module_trust(&self) -> Vec<ModuleTrust> {
        let cfg = self.inner.config.read().modules.granted.clone();
        self.registry()
            .entries()
            .iter()
            .filter_map(|e| {
                let request = e.trust.as_deref()?.clone();
                let grant = cfg.get(&e.info().id);
                Some(ModuleTrust {
                    id: e.info().id.clone(),
                    name: e.info().name.clone(),
                    dir: match &e.origin {
                        Origin::Module { dir, .. } => Some(dir.clone()),
                        _ => None,
                    },
                    pending: e.pending,
                    first_time: grant.is_none(),
                    new: request.beyond(grant),
                    hash: request.hash(),
                    request,
                })
            })
            .collect()
    }

    /// The modules waiting for approval.
    pub fn pending_modules(&self) -> Vec<ModuleTrust> {
        self.module_trust().into_iter().filter(|m| m.pending).collect()
    }

    /// Approve what module `id` asks for now and load it. With `hash`, only
    /// if the request is still the one with that hash (what the user was
    /// shown), so a manifest changed in the meantime is not approved
    /// unseen.
    pub fn approve_module(&self, id: &str, hash: Option<&str>) -> Result<()> {
        let request = self.request_of(id)?;
        if let Some(h) = hash.filter(|h| !h.trim().is_empty()) {
            if h.trim() != request.hash() {
                return Err(Error::Config(format!(
                    "module {id} changed since it was shown for approval; look at what it asks for again"
                )));
            }
        }
        self.update_config(|c| {
            c.modules.granted.insert(id.to_owned(), request.to_grant());
        })
    }

    /// Withdraw module `id`'s approval: it stays listed, waiting for
    /// approval, until approved again.
    pub fn revoke_module(&self, id: &str) -> Result<()> {
        if !self.config().modules.granted.contains_key(id) {
            self.request_of(id)?;
            return Err(Error::Config(format!("module {id} has no approval to revoke")));
        }
        self.update_config(|c| {
            c.modules.granted.remove(id);
        })
    }

    /// Say no to module `id`: switch it off, so it neither runs nor asks
    /// again until switched back on.
    pub fn decline_module(&self, id: &str) -> Result<()> {
        self.request_of(id)?;
        self.update_config(|c| {
            if !c.modules.disabled.iter().any(|d| d == id) {
                c.modules.disabled.push(id.to_owned());
            }
        })
    }

    fn request_of(&self, id: &str) -> Result<TrustRequest> {
        let registry = self.registry();
        let entry = registry.entry(id).ok_or_else(|| Error::NoSuchModule(id.to_owned()))?;
        entry
            .trust
            .as_deref()
            .cloned()
            .ok_or_else(|| Error::Config(format!("{id} comes with Glossa and needs no approval")))
    }

    /// Approve what the module in `dir` declares without reloading
    /// anything: for modules the user writes in the module editor, whose
    /// author approves by definition. A folder Glossa trusts anyway, or
    /// without a `module.toml`, is left alone.
    pub(crate) fn approve_dir(&self, dir: &Path) {
        let cfg = self.config();
        if self.trust_policy(&cfg).is_trusted(dir) {
            return;
        }
        let Ok(manifest) = Manifest::load(&dir.join("module.toml")) else { return };
        let grant: ModuleGrant = TrustRequest::for_manifest(&manifest, dir).to_grant();
        if cfg.modules.granted.get(&manifest.module.id) == Some(&grant) {
            return;
        }
        let snapshot = {
            let mut cfg = self.inner.config.write();
            cfg.modules.granted.insert(manifest.module.id.clone(), grant);
            cfg.clone()
        };
        if let Err(e) = self.write_config(&snapshot) {
            tracing::warn!("could not save the approval of {}: {e}", manifest.module.id);
        }
    }

    /// [`Engine::reload_module`] for the module editor: the author's save
    /// approves what the module now declares.
    pub fn reload_module_approved(&self, dir: &Path) -> Result<crate::module_dev::ReloadOutcome> {
        self.approve_dir(dir);
        self.reload_module(dir)
    }
}

#[cfg(all(test, feature = "js"))]
mod tests {
    use super::*;
    use glossa_core::Paths;

    fn module(dir: &Path, id: &str, hosts: &str) {
        let m = dir.join(id);
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("module.toml"),
            format!(
                "capabilities = [\"translate\"]\n[module]\nid = \"{id}\"\nname = \"{id}\"\nruntime = \"js\"\n\
                 [permissions]\nnetwork = [{hosts}]\n"
            ),
        )
        .unwrap();
        std::fs::write(m.join("main.js"), "function translate(req) { return 'ok:' + req.text; }").unwrap();
    }

    fn home(tag: &str) -> (PathBuf, Paths) {
        let root = std::env::temp_dir().join(format!("glossa-trust-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths::for_exe_dir(root.join("exe"), Some(root.join("home")));
        (root, paths)
    }

    fn initialised() -> Config {
        let mut c = Config::default();
        c.modules.trust_initialised = true;
        c
    }

    /// Calls the provider itself, without the pipeline failing over to
    /// another service.
    fn translate(e: &Engine, id: &str) -> Result<String> {
        e.test_provider(id, None).map(|(t, _)| t.split(':').next().unwrap_or_default().to_owned())
    }

    #[test]
    fn a_new_module_waits_then_runs_once_approved_and_asks_again_when_widened() {
        let (root, paths) = home("flow");
        let user = paths.user_module_dir();
        module(&user, "fresh", "\"api.test\"");
        let e = Engine::with_config(paths, initialised()).unwrap();

        let pending = e.pending_modules();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].first_time && pending[0].pending);
        let err = translate(&e, "fresh").unwrap_err();
        assert!(matches!(&err, Error::NeedsApproval { module } if module == "fresh"), "{err:?}");
        assert!(e.approve_module("fresh", Some("not the hash")).is_err(), "approves only what was shown");

        e.approve_module("fresh", Some(&pending[0].hash)).unwrap();
        assert!(e.pending_modules().is_empty());
        assert_eq!(translate(&e, "fresh").unwrap(), "ok");
        assert!(e.config().modules.granted.contains_key("fresh"));

        // An update with fewer hosts runs; one with more asks again.
        module(&user, "fresh", "");
        e.reload_modules();
        assert!(e.pending_modules().is_empty(), "narrowing does not ask");
        module(&user, "fresh", "\"api.test\", \"more.test\"");
        e.reload_modules();
        let again = e.pending_modules();
        assert_eq!(again.len(), 1);
        assert!(!again[0].first_time);
        assert_eq!(again[0].new.network, ["more.test"], "points out what is new");

        // Revoking puts it back to waiting; declining switches it off.
        e.approve_module("fresh", None).unwrap();
        e.revoke_module("fresh").unwrap();
        assert_eq!(e.pending_modules().len(), 1);
        e.decline_module("fresh").unwrap();
        assert!(e.pending_modules().is_empty());
        assert!(e.config().modules.disabled.contains(&"fresh".to_owned()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn modules_installed_before_approvals_keep_working() {
        let (root, paths) = home("migrate");
        module(&paths.user_module_dir(), "old", "\"api.test\"");
        // An older settings file has no `trust_initialised`.
        let cfg: Config = toml::from_str("[modules]\ndisabled = []\n").unwrap();
        assert!(!cfg.modules.trust_initialised);
        let e = Engine::with_config(paths.clone(), cfg).unwrap();
        assert!(e.pending_modules().is_empty());
        assert_eq!(translate(&e, "old").unwrap(), "ok");
        let saved = Config::load(&paths.config_file()).unwrap();
        assert!(saved.modules.trust_initialised);
        assert_eq!(saved.modules.granted["old"].network, ["api.test"]);

        // A module added after the migration waits.
        module(&paths.user_module_dir(), "later", "\"api.test\"");
        e.reload_modules();
        assert_eq!(e.pending_modules().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["later"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_and_shipped_modules_need_no_approval() {
        let (root, paths) = home("bundled");
        // Beside the program: shipped with Glossa.
        module(&root.join("exe").join("modules"), "shipped", "\"api.test\"");
        let e = Engine::with_config(paths, initialised()).unwrap();
        assert!(e.pending_modules().is_empty());
        assert_eq!(translate(&e, "shipped").unwrap(), "ok");
        let bundled = e.registry().entries().iter().find(|x| x.origin == Origin::Bundled).map(|x| x.info().id.clone());
        if let Some(id) = bundled {
            assert!(e.registry().entry(&id).unwrap().trust.is_none());
            assert!(e.approve_module(&id, None).is_err(), "nothing to approve");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn modules_made_in_the_editor_are_approved_by_their_author() {
        let (root, paths) = home("editor");
        let e = Engine::with_config(paths, initialised()).unwrap();
        let dir = e.new_module(crate::module_dev::Template::ALL[0], "Hand made").unwrap();
        let id = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(e.pending_modules().is_empty(), "{:?}", e.pending_modules());
        assert!(e.config().modules.granted.contains_key(&id));
        std::fs::remove_dir_all(&root).ok();
    }
}
