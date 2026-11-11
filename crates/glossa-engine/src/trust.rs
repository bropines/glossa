//! Approving modules installed by hand, from the engine's side: which
//! modules wait, what they ask for, approving, revoking and declining, the
//! policy the registry loads with, and the one-time migration that grants
//! the modules an earlier version is known to have run. A settings file
//! without approvals approves nothing by itself: on a fresh install every
//! module found in the user's folders waits. The desktop app,
//! `glossa-cli module approve|revoke|pending` and the control API's
//! `/v1/trust` all come through here. The decision itself is
//! [`glossa_modules::trust`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glossa_core::config::{Config, ModuleGrant};
use glossa_core::{Error, Result};
use glossa_modules::trust::{canonical, TrustPolicy, TrustRequest};
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
    /// Waiting only because its files are not the ones approved: it asks
    /// for nothing new, so `new` is empty.
    pub changed: bool,
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
        let paths = &self.inner.paths;
        let user = canonical(&paths.user_module_dir());
        let extra: Vec<PathBuf> = cfg.modules.dirs.iter().map(|d| canonical(d)).collect();
        // Only folders an installer writes to count as shipped; the user's
        // own folders never do, even where the two coincide.
        let trusted_dirs = paths
            .shipped_module_dirs()
            .into_iter()
            .filter(|d| d.is_dir())
            .map(|d| canonical(&d))
            .filter(|d| *d != user && !extra.contains(d))
            .collect();
        let trusted_script_dirs = cfg.classic.services_dir().into_iter().collect();
        // Before the migration has run, only the modules an earlier version
        // recorded as seen (`[services] seen`) load without a grant, so the
        // migration can record what each declares. A missing or empty
        // settings file names none: a fresh install approves nothing, and
        // neither does deleting the file. An id Glossa itself provides is
        // never one of them, so a folder cannot claim a built-in's id here.
        let legacy = if cfg.modules.trust_initialised {
            Vec::new()
        } else {
            let own = self.own_provider_ids(cfg);
            cfg.services.seen.iter().filter(|id| !own.contains(id)).cloned().collect()
        };
        TrustPolicy { granted: Some(Arc::new(cfg.modules.granted.clone())), legacy, trusted_dirs, trusted_script_dirs }
    }

    /// The ids of the providers compiled into the program or made from the
    /// settings: none of them is a module installed by hand.
    fn own_provider_ids(&self, cfg: &Config) -> Vec<String> {
        let ocr = self.inner.ocr_builtins.lock().1.clone();
        self.inner
            .builtins
            .iter()
            .chain(&self.inner.bundled)
            .chain(&ocr)
            .map(|p| p.info().id.clone())
            .chain(cfg.services.ai_endpoints.iter().map(|e| e.id.clone()))
            .collect()
    }

    /// The first time this version runs with a settings file: grant the
    /// modules an earlier version had running what they declare, with the
    /// files they hold now, so updating Glossa does not stop modules that
    /// were working. Called by `reload_modules` after a load under the
    /// policy that lets exactly those through; everything else found in
    /// the folders is listed as waiting.
    pub(crate) fn initialise_trust(&self) {
        let registry = self.registry();
        let snapshot = {
            let mut cfg = self.inner.config.write();
            if cfg.modules.trust_initialised {
                return;
            }
            for e in registry.entries() {
                if let (Some(request), false, Origin::Module { dir, .. }) = (&e.trust, e.pending, &e.origin) {
                    cfg.modules.granted.entry(e.info().id.clone()).or_insert_with(|| request.to_grant(dir));
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

    /// Bind approvals from before files were hashed to the files their
    /// modules hold now: such a grant let its module load, and from here on
    /// a change to those files asks again. Called by `reload_modules`.
    pub(crate) fn bind_grants(&self) {
        let registry = self.registry();
        let unbound: Vec<(String, PathBuf)> = {
            let cfg = self.inner.config.read();
            registry
                .entries()
                .iter()
                .filter_map(|e| match (&e.trust, e.pending, &e.origin) {
                    (Some(_), false, Origin::Module { dir, .. }) => {
                        let id = &e.info().id;
                        cfg.modules
                            .granted
                            .get(id)
                            .is_some_and(|g| g.content.is_empty())
                            .then(|| (id.clone(), dir.clone()))
                    }
                    _ => None,
                })
                .collect()
        };
        if unbound.is_empty() {
            return;
        }
        // Hashed outside the lock: a module folder can be large.
        let hashed: Vec<(String, String)> =
            unbound.into_iter().map(|(id, dir)| (id, glossa_modules::trust::content_hash(&dir))).collect();
        let snapshot = {
            let mut cfg = self.inner.config.write();
            for (id, content) in hashed {
                if let Some(g) = cfg.modules.granted.get_mut(&id).filter(|g| g.content.is_empty()) {
                    g.content = content;
                }
            }
            cfg.clone()
        };
        if let Err(e) = self.write_config(&snapshot) {
            tracing::warn!("could not save the module approvals: {e}");
        }
    }

    /// Every module loaded that needs approval, approved or not, with those
    /// held back because a trusted provider has their id.
    pub fn module_trust(&self) -> Vec<ModuleTrust> {
        let cfg = self.inner.config.read().modules.granted.clone();
        let registry = self.registry();
        registry
            .entries()
            .iter()
            .chain(registry.held())
            .filter_map(|e| {
                let request = e.trust.as_deref()?.clone();
                let grant = cfg.get(&e.info().id);
                let dir = match &e.origin {
                    Origin::Module { dir, .. } => Some(dir.as_path()),
                    _ => None,
                };
                // Waiting although the grant covers the request and the
                // folder: only the files can be the reason.
                let changed = e.pending
                    && grant.is_some_and(|g| {
                        request.within(g)
                            && !g.content.is_empty()
                            && dir.is_some_and(|d| g.dir.is_empty() || Path::new(&g.dir) == canonical(d))
                    });
                Some(ModuleTrust {
                    id: e.info().id.clone(),
                    name: e.info().name.clone(),
                    dir: match &e.origin {
                        Origin::Module { dir, .. } => Some(dir.clone()),
                        _ => None,
                    },
                    pending: e.pending,
                    first_time: grant.is_none(),
                    changed,
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

    /// Approve what module `id` asks for now, with the files it holds now,
    /// and load it. With `hash`, only
    /// if the request is still the one with that hash (what the user was
    /// shown), so a manifest changed in the meantime is not approved
    /// unseen.
    pub fn approve_module(&self, id: &str, hash: Option<&str>) -> Result<()> {
        let (request, dir) = self.request_of(id)?;
        if let Some(h) = hash.filter(|h| !h.trim().is_empty()) {
            if h.trim() != request.hash() {
                return Err(Error::Config(format!(
                    "module {id} changed since it was shown for approval; look at what it asks for again"
                )));
            }
        }
        self.update_config(|c| {
            c.modules.granted.insert(id.to_owned(), request.to_grant(&dir));
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

    /// What module `id` asks for and its folder. A module waiting for
    /// approval comes before an approved one with the same id: that is the
    /// one being asked about.
    fn request_of(&self, id: &str) -> Result<(TrustRequest, PathBuf)> {
        let registry = self.registry();
        let mut found = registry.held().iter().chain(registry.entries()).filter(|e| e.info().id == id).peekable();
        if found.peek().is_none() {
            return Err(Error::NoSuchModule(id.to_owned()));
        }
        found
            .find_map(|e| match (&e.trust, &e.origin) {
                (Some(r), Origin::Module { dir, .. }) => Some(((**r).clone(), dir.clone())),
                _ => None,
            })
            .ok_or_else(|| Error::Config(format!("{id} comes with Glossa and needs no approval")))
    }

    /// Whether the module in `dir` would wait for approval if loaded now
    /// (it is waiting, or its manifest asks for more than it was granted).
    pub fn waits_for_approval(&self, dir: &Path) -> bool {
        if self.waits_for_approval_listed(dir) {
            return true;
        }
        let policy = self.trust_policy(&self.config());
        if policy.is_trusted(dir) {
            return false;
        }
        match Manifest::load(&dir.join("module.toml")) {
            Ok(m) => !policy.allows(&m.module.id, dir, &TrustRequest::for_manifest(&m, dir)),
            // A script without a manifest: approved only if listed so.
            Err(_) => dir.join("Service.js").is_file() || dir.join("info.json").is_file(),
        }
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
        let grant: ModuleGrant = TrustRequest::for_manifest(&manifest, dir).to_grant(dir);
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

    /// Remember that the user created or copied the module in `dir` in the
    /// editor, and approve it: its saves approve it from now on.
    pub(crate) fn approve_authored(&self, dir: &Path) {
        self.inner.authored.lock().insert(canonical(dir));
        self.approve_dir(dir);
    }

    /// [`Engine::reload_module`] for the module editor. A save approves
    /// what the module now declares when the user is its author here (it
    /// was created or copied in the editor in this session) or had already
    /// approved it; a module waiting for approval stays waiting, so opening
    /// someone else's module in the editor and saving it approves nothing.
    pub fn reload_module_approved(&self, dir: &Path) -> Result<crate::module_dev::ReloadOutcome> {
        let authored = self.inner.authored.lock().contains(&canonical(dir));
        let approved = !self.waits_for_approval_listed(dir)
            && self.registry().entries().iter().any(|e| {
                e.trust.is_some()
                    && !e.pending
                    && matches!(&e.origin, Origin::Module { dir: d, .. } if canonical(d) == canonical(dir))
            });
        if authored || approved {
            self.approve_dir(dir);
        }
        self.reload_module(dir)
    }

    /// An engine for tests of what approved modules do: every module found
    /// waiting at the start is approved as the user would.
    #[cfg(test)]
    pub(crate) fn approving(paths: glossa_core::Paths, cfg: Config) -> Engine {
        let e = Engine::with_config(paths, cfg).unwrap();
        for m in e.pending_modules() {
            e.approve_module(&m.id, Some(&m.hash)).unwrap();
        }
        e
    }

    /// Whether the module loaded from `dir` is listed as waiting.
    fn waits_for_approval_listed(&self, dir: &Path) -> bool {
        let registry = self.registry();
        registry
            .entries()
            .iter()
            .chain(registry.held())
            .any(|e| e.pending && matches!(&e.origin, Origin::Module { dir: d, .. } if canonical(d) == canonical(dir)))
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

        // An update asks again, whatever it asks for: its files changed.
        module(&user, "fresh", "");
        e.reload_modules();
        let narrowed = e.pending_modules();
        assert!(narrowed[0].changed, "the files are not the approved ones");
        assert!(narrowed[0].new.network.is_empty(), "though it asks for nothing new");
        // Put back as approved, it runs again without asking.
        module(&user, "fresh", "\"api.test\"");
        e.reload_modules();
        assert!(e.pending_modules().is_empty());
        module(&user, "fresh", "\"api.test\", \"more.test\"");
        e.reload_modules();
        let again = e.pending_modules();
        assert_eq!(again.len(), 1);
        assert!(!again[0].first_time && !again[0].changed);
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
        // Dropped into the folder since the old version last ran: it never
        // ran, so the upgrade does not approve it.
        module(&paths.user_module_dir(), "planted", "\"evil.test\"");
        // An older settings file has no `trust_initialised`, and lists the
        // modules that version had running under `[services] seen`.
        let cfg: Config = toml::from_str("[modules]\ndisabled = []\n[services]\nseen = [\"old\"]\n").unwrap();
        assert!(!cfg.modules.trust_initialised);
        let e = Engine::with_config(paths.clone(), cfg).unwrap();
        assert_eq!(e.pending_modules().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["planted"]);
        assert_eq!(translate(&e, "old").unwrap(), "ok");
        assert!(matches!(translate(&e, "planted"), Err(Error::NeedsApproval { .. })));
        let saved = Config::load(&paths.config_file()).unwrap();
        assert!(saved.modules.trust_initialised);
        assert_eq!(saved.modules.granted["old"].network, ["api.test"]);
        assert!(!saved.modules.granted.contains_key("planted"));
        let dir = paths.user_module_dir().join("old");
        assert_eq!(saved.modules.granted["old"].content, glossa_modules::trust::content_hash(&dir));
        e.decline_module("planted").unwrap();

        // A module added after the migration waits.
        module(&paths.user_module_dir(), "later", "\"api.test\"");
        e.reload_modules();
        assert_eq!(e.pending_modules().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["later"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_fresh_install_approves_nothing_it_finds() {
        // No settings file at all, and a module already in the user's
        // folder (unpacked there by something else before the first start).
        let (root, paths) = home("fresh-install");
        module(&paths.user_module_dir(), "planted", "\"api.test\"");
        assert!(!paths.config_file().exists());
        let e = Engine::start(paths.clone()).unwrap();
        assert_eq!(e.pending_modules().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["planted"]);
        let err = translate(&e, "planted").unwrap_err();
        assert!(matches!(&err, Error::NeedsApproval { module } if module == "planted"), "{err:?}");
        assert!(e.config().modules.granted.is_empty());
        assert!(e.config().modules.trust_initialised, "the migration is over, having granted nothing");
        assert!(!e.config().services.enabled.contains(&"planted".to_owned()), "nor switched on");
        // What ships inside the build runs all the same.
        assert!(e.registry().entries().iter().any(|x| x.origin == Origin::Bundled && !x.pending));

        // Nor does an existing file with no approvals in it, whatever else
        // it holds, or the same Glossa started again.
        drop(e);
        let (root2, paths2) = home("fresh-install-2");
        module(&paths2.user_module_dir(), "planted", "\"api.test\"");
        let e = Engine::with_config(paths2, Config::default()).unwrap();
        assert_eq!(e.pending_modules().len(), 1);
        e.reload_modules();
        assert_eq!(e.pending_modules().len(), 1);
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&root2).ok();
    }

    #[test]
    fn the_upgrade_does_not_let_a_folder_take_a_built_in_id() {
        let (root, paths) = home("upgrade-builtin");
        let probe = Engine::with_config(paths.clone(), initialised()).unwrap();
        let Some(id) =
            probe.registry().entries().iter().find(|x| x.origin == Origin::Bundled).map(|x| x.info().id.clone())
        else {
            return;
        };
        drop(probe);
        std::fs::remove_file(paths.config_file()).ok();
        // Every built-in is in `seen` in an older file; a folder with that
        // id must not ride on it.
        module(&paths.user_module_dir(), &id, "\"evil.test\"");
        let mut cfg = Config::default();
        cfg.services.seen = vec![id.clone()];
        let e = Engine::with_config(paths, cfg).unwrap();
        assert_eq!(e.registry().entry(&id).unwrap().origin, Origin::Bundled, "the built-in keeps its place");
        assert_eq!(e.pending_modules().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
        assert!(!e.config().modules.granted.contains_key(&id));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn changed_files_ask_again_with_the_same_manifest() {
        let (root, paths) = home("content");
        let user = paths.user_module_dir();
        module(&user, "mod", "\"api.test\"");
        let e = Engine::with_config(paths.clone(), initialised()).unwrap();
        e.approve_module("mod", None).unwrap();
        assert_eq!(translate(&e, "mod").unwrap(), "ok");
        e.reload_modules();
        assert!(e.pending_modules().is_empty(), "unchanged files stay approved");

        // Same `module.toml`, other code.
        std::fs::write(user.join("mod").join("main.js"), "function translate(req) { return 'evil:' + req.text; }")
            .unwrap();
        e.reload_modules();
        let waiting = e.pending_modules();
        assert_eq!(waiting.len(), 1);
        assert!(waiting[0].changed && !waiting[0].first_time);
        assert!(waiting[0].new.network.is_empty(), "it asks for nothing new");
        assert!(matches!(translate(&e, "mod"), Err(Error::NeedsApproval { .. })));
        // Reloading the one module is no way round it.
        e.reload_module(&user.join("mod")).unwrap();
        assert!(matches!(translate(&e, "mod"), Err(Error::NeedsApproval { .. })));
        assert!(e.waits_for_approval(&user.join("mod")));

        // A file added beside the code counts too; approving takes it in.
        e.approve_module("mod", Some(&waiting[0].hash)).unwrap();
        assert_eq!(translate(&e, "mod").unwrap(), "evil");
        std::fs::write(user.join("mod").join("helper.js"), "1").unwrap();
        e.reload_modules();
        assert!(e.pending_modules()[0].changed);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn approvals_from_before_files_were_hashed_keep_working_and_are_bound() {
        let (root, paths) = home("unbound");
        let user = paths.user_module_dir();
        module(&user, "kept", "\"api.test\"");
        // As an earlier version wrote it: no `content`.
        let text =
            "[modules]\ntrust_initialised = true\n[modules.granted.kept]\nruntime = \"js\"\nnetwork = [\"api.test\"]\n";
        let cfg: Config = toml::from_str(text).unwrap();
        assert!(cfg.modules.granted["kept"].content.is_empty());
        let e = Engine::with_config(paths.clone(), cfg).unwrap();
        assert!(e.pending_modules().is_empty());
        assert_eq!(translate(&e, "kept").unwrap(), "ok");
        let bound = e.config().modules.granted["kept"].content.clone();
        assert_eq!(bound, glossa_modules::trust::content_hash(&user.join("kept")));
        assert_eq!(Config::load(&paths.config_file()).unwrap().modules.granted["kept"].content, bound);

        std::fs::write(user.join("kept").join("main.js"), "function translate() { return 'x'; }").unwrap();
        e.reload_modules();
        assert!(e.pending_modules()[0].changed, "from then on a change asks");
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

        // The author's saves approve what the module now declares.
        let toml = std::fs::read_to_string(dir.join("module.toml")).unwrap();
        let wider = toml.replace("[permissions]\nnetwork = [", "[permissions]\nnetwork = [\"more.test\", ");
        assert_ne!(toml, wider, "the template declares hosts");
        std::fs::write(dir.join("module.toml"), wider).unwrap();
        e.reload_module_approved(&dir).unwrap();
        assert!(e.pending_modules().is_empty());
        assert!(e.config().modules.granted[&id].network.contains(&"more.test".to_owned()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn opening_a_waiting_module_in_the_editor_approves_nothing() {
        let (root, paths) = home("editor-waiting");
        let user = paths.user_module_dir();
        module(&user, "foreign", "\"api.test\"");
        let e = Engine::with_config(paths, initialised()).unwrap();
        let dir = user.join("foreign");
        assert!(e.waits_for_approval(&dir));
        e.reload_module_approved(&dir).unwrap();
        assert_eq!(e.pending_modules().len(), 1, "a save does not approve someone else's module");
        assert!(e.config().modules.granted.is_empty());

        // Nor do the editor's tests run it.
        let report = e.test_module(&dir);
        assert_eq!(report.status, glossa_module_test::Status::Fail);
        assert!(report.findings[0].message.contains("waiting for your approval"), "{:?}", report.findings);

        // Once approved, its saves keep it approved as it changes.
        e.approve_module("foreign", None).unwrap();
        module(&user, "foreign", "\"api.test\", \"more.test\"");
        e.reload_module_approved(&dir).unwrap();
        assert!(e.pending_modules().is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_fixed_folders_count_as_shipped() {
        let (root, paths) = home("shipped");
        // `<exe>/../share/glossa/modules` is a module folder, but anyone may
        // be able to write there (`C:\share\…`, `Downloads\share\…`).
        module(&root.join("share").join("glossa").join("modules"), "near", "\"api.test\"");
        module(&root.join("exe").join("modules"), "beside", "\"api.test\"");
        let e = Engine::with_config(paths, initialised()).unwrap();
        let waiting: Vec<_> = e.pending_modules().into_iter().map(|m| m.id).collect();
        assert_eq!(waiting, ["near"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_recovered_settings_file_does_not_approve_everything() {
        let (root, paths) = home("recovered");
        module(&paths.user_module_dir(), "declined", "\"api.test\"");
        std::fs::create_dir_all(paths.config_file().parent().unwrap()).unwrap();
        std::fs::write(paths.config_file(), "this is [not toml").unwrap();
        let e = Engine::start(paths.clone()).unwrap();
        assert!(!e.startup_warnings().is_empty(), "the file was set aside");
        assert_eq!(e.pending_modules().len(), 1, "waits instead of being granted by the migration");
        assert!(e.config().modules.granted.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}
