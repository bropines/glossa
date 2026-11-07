//! The Python modules run with, from the engine's side: whether the managed
//! Python, `uv` and each module's environment are there, getting them in the
//! background with progress, and deleting them. The desktop app,
//! `glossa-cli python …` and the control API all come through here.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glossa_core::{Error, Result};
use glossa_modules::assets::{AssetStatus, Progress as AssetProgress};
use glossa_modules::python_env::{self, EnvState, PythonEnv, Toolchain};
use glossa_modules::CancelToken;
use parking_lot::Mutex;
use serde::Serialize;

use crate::assets::AssetJobState;
use crate::{Engine, Event};

/// The id [`Event::AssetsChanged`] carries for the shared Python itself
/// (installs not made for one module).
pub const PYTHON_JOB: &str = "python";

/// The managed Python and `uv`, and every Python module's state.
#[derive(Debug, Clone, Serialize)]
pub struct PythonReport {
    /// `[modules] python` as set.
    pub setting: String,
    /// There is a managed Python for this system.
    pub supported: bool,
    pub python_version: String,
    pub uv_version: String,
    /// The managed interpreter (downloaded or not).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python_exe: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python: Option<AssetStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uv: Option<AssetStatus>,
    pub runtimes_dir: PathBuf,
    pub venvs_dir: PathBuf,
    pub modules: Vec<ModulePython>,
    /// A background install not made for one module, running or just
    /// finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<AssetJobState>,
}

/// One Python module's interpreter and environment.
#[derive(Debug, Clone, Serialize)]
pub struct ModulePython {
    pub module: String,
    /// Its dependencies, for people, when it has any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requirements: Option<String>,
    /// Its own environment, when it has dependencies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub venv: Option<PathBuf>,
    /// The environment's folder is on disk (made, or left half made).
    pub venv_exists: bool,
    /// `ready`, `missing`, `unknown` (a system Python not searched for yet)
    /// or `impossible`.
    pub state: &'static str,
    /// What is missing, or why it cannot work.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Bytes to download for the managed Python and `uv` (packages not
    /// counted).
    pub download_size: u64,
    /// An install runs for it now.
    pub busy: bool,
    /// A background install for it, running or just finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<AssetJobState>,
}

/// A background install the engine keeps track of.
pub(crate) struct PythonJob {
    state: Arc<Mutex<AssetJobState>>,
    cancel: CancelToken,
}

/// How often a running install announces its progress.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Where the managed Python, `uv`, the modules' environments and `uv`'s
/// cache go for the data folders `paths`. The proxy is left out: [`Engine::python_dirs`]
/// adds the one the network settings call for.
pub fn python_dirs(paths: &glossa_core::Paths) -> python_env::PythonDirs {
    python_env::PythonDirs {
        runtimes: paths.runtimes_dir(),
        venvs: paths.venvs_dir(),
        uv_cache: paths.cache_dir.join("uv"),
        proxy: None,
    }
}

/// Whether a search for a system Python was started for the options page.
static SEARCHING: AtomicBool = AtomicBool::new(false);

impl Engine {
    fn toolchain(&self) -> Toolchain {
        Toolchain::new(self.python_dirs())
    }

    fn python_env_of(&self, id: &str) -> Result<Arc<PythonEnv>> {
        let registry = self.registry();
        let entry = registry.entry(id).ok_or_else(|| Error::NoSuchModule(id.to_owned()))?;
        entry.python.clone().ok_or_else(|| Error::Config(format!("module {id} is not a Python module")))
    }

    fn job_state(&self, key: &str) -> Option<AssetJobState> {
        self.inner.python_jobs.lock().get(key).map(|j| j.state.lock().clone())
    }

    /// [`Self::read_module_python`], from the cache while nothing changed.
    fn module_python_report(&self, env: &PythonEnv) -> ModulePython {
        if let Some(m) = self.inner.python_cache.lock().get(&env.module) {
            return m.clone();
        }
        let m = self.read_module_python(env);
        self.inner.python_cache.lock().insert(env.module.clone(), m.clone());
        m
    }

    /// Stop module `id`'s process, so its environment can be deleted or
    /// made again (Windows keeps a running `python.exe`'s folder locked).
    fn stop_module(&self, id: &str) {
        if let Some(p) = self.registry().get(id) {
            p.suspend();
        }
    }

    fn read_module_python(&self, env: &PythonEnv) -> ModulePython {
        // The job before the state, as for assets: an install finishing in
        // between must not show as finished with things still missing.
        let job = self.job_state(&env.module);
        let (state, missing) = match env.state_now() {
            EnvState::Ready => ("ready", Vec::new()),
            EnvState::Missing(m) => ("missing", m),
            EnvState::Unknown => {
                self.search_system_python();
                ("unknown", Vec::new())
            }
            EnvState::Impossible(why) => ("impossible", vec![why]),
        };
        ModulePython {
            module: env.module.clone(),
            requirements: env.requirements.as_ref().map(|r| r.describe()),
            venv_exists: env.venv_dir().is_some_and(|v| v.exists()),
            venv: env.venv_dir(),
            state,
            missing,
            download_size: env.download_size(),
            busy: env.is_busy() || job.as_ref().is_some_and(|j| j.running),
            job,
        }
    }

    /// Search for a system Python on a worker, once, and tell the front end
    /// when it is done: deciding whether a module needs the managed Python
    /// runs a program, which the UI thread must not wait for.
    fn search_system_python(&self) {
        if SEARCHING.swap(true, Ordering::SeqCst) {
            return;
        }
        let engine = self.clone();
        self.spawn("python-search", move || {
            let found = python_env::system_python();
            tracing::debug!(found = ?found.as_ref().map(|f| &f.0), "searched for a system Python");
            engine.emit(Event::AssetsChanged { module: PYTHON_JOB.into() });
        });
    }

    /// The managed Python, `uv` and every Python module. Cheap: runs
    /// nothing (a system Python not searched for yet shows as `unknown`, and
    /// a search starts on a worker).
    pub fn python_status(&self) -> PythonReport {
        let tc = self.toolchain();
        let registry = self.registry();
        let modules = registry
            .entries()
            .iter()
            .filter_map(|e| e.python.as_ref())
            .map(|env| self.module_python_report(env))
            .collect();
        PythonReport {
            setting: self.config().modules.python.clone(),
            supported: tc.supported(),
            python_version: python_env::PYTHON_VERSION.into(),
            uv_version: python_env::UV_VERSION.into(),
            python_exe: tc.python_exe(),
            python: tc.python_status(),
            uv: tc.uv_status(),
            runtimes_dir: tc.dirs.runtimes.clone(),
            venvs_dir: tc.dirs.venvs.clone(),
            modules,
            job: self.job_state(PYTHON_JOB),
        }
    }

    /// Python module `id`'s state; `None` when it is not a Python module.
    /// Cheap, like [`Engine::python_status`].
    pub fn module_python(&self, id: &str) -> Option<ModulePython> {
        let env = self.python_env_of(id).ok()?;
        Some(self.module_python_report(&env))
    }

    /// A Python 3.8+ on this system, if any. Runs programs the first time:
    /// call it from a worker.
    pub fn system_python(&self) -> Option<PathBuf> {
        python_env::system_python().map(|(p, _)| PathBuf::from(p))
    }

    /// Get what module `id` needs (the managed Python and `uv` if it uses
    /// them, its environment) or, with no module, the managed Python and
    /// `uv` themselves, and wait (the CLI). `progress` is called as bytes
    /// arrive.
    pub fn install_python(
        &self,
        module: Option<&str>,
        cancel: &CancelToken,
        progress: &dyn Fn(&AssetProgress),
    ) -> Result<PythonReport> {
        let http = self.inner.http.read().clone();
        match module {
            Some(id) => {
                let env = self.python_env_of(id)?;
                self.stop_module(id);
                env.install(http.as_ref(), cancel, progress)?;
            }
            None => self.toolchain().download(true, true, http.as_ref(), cancel, progress)?,
        }
        self.emit(Event::AssetsChanged { module: module.unwrap_or(PYTHON_JOB).to_owned() });
        Ok(self.python_status())
    }

    /// [`Engine::install_python`] on a worker. [`Event::AssetsChanged`]
    /// (with the module's id, or [`PYTHON_JOB`]) reports progress and the
    /// end. Starting one while it runs does nothing.
    pub fn start_python_install(&self, module: Option<&str>) -> Result<()> {
        let env = module.map(|id| self.python_env_of(id)).transpose()?;
        let key = module.unwrap_or(PYTHON_JOB).to_owned();
        let state = Arc::new(Mutex::new(AssetJobState { running: true, ..Default::default() }));
        let cancel = CancelToken::new();
        {
            let mut jobs = self.inner.python_jobs.lock();
            if jobs.get(&key).is_some_and(|j| j.state.lock().running) {
                return Ok(());
            }
            jobs.insert(key.clone(), PythonJob { state: state.clone(), cancel: cancel.clone() });
        }
        self.emit(Event::AssetsChanged { module: key.clone() });
        let engine = self.clone();
        self.spawn("python", move || {
            let http = engine.inner.http.read().clone();
            let last = Mutex::new(Instant::now());
            let progress = |p: &AssetProgress| {
                state.lock().progress = p.clone();
                let mut last = last.lock();
                if last.elapsed() >= PROGRESS_EVERY || p.total == 0 {
                    *last = Instant::now();
                    engine.emit(Event::AssetsChanged { module: key.clone() });
                }
            };
            let result = match &env {
                Some(env) => {
                    engine.stop_module(&env.module);
                    env.install(http.as_ref(), &cancel, &progress)
                }
                None => engine.toolchain().download(true, true, http.as_ref(), &cancel, &progress),
            };
            {
                let mut s = state.lock();
                s.running = false;
                match &result {
                    Ok(()) => tracing::info!(job = %key, "Python ready"),
                    Err(Error::Cancelled) => s.error = engine.tr("assets.cancelled"),
                    Err(e) => {
                        tracing::warn!(job = %key, "getting Python failed: {e}");
                        s.error = e.to_string();
                    }
                }
            }
            engine.emit(Event::AssetsChanged { module: key });
        });
        Ok(())
    }

    /// Stop a background install (module `id`, or [`PYTHON_JOB`]). What was
    /// downloaded so far is kept and resumed next time.
    pub fn cancel_python_install(&self, key: &str) {
        if let Some(job) = self.inner.python_jobs.lock().get(key) {
            job.cancel.cancel();
        }
    }

    /// Delete module `id`'s environment; the shared Python stays.
    pub fn remove_module_python(&self, id: &str) -> Result<()> {
        let env = self.python_env_of(id)?;
        self.stop_module(id);
        env.remove_venv()?;
        self.inner.python_jobs.lock().remove(id);
        self.emit(Event::AssetsChanged { module: id.to_owned() });
        Ok(())
    }

    /// Delete the managed Python, `uv`, every module's environment and
    /// `uv`'s cache. The modules are stopped and reloaded first, so none
    /// keeps running from the deleted files. Blocks: call it from a worker.
    pub fn remove_python(&self) -> Result<()> {
        for job in self.inner.python_jobs.lock().values() {
            job.cancel.cancel();
        }
        self.registry().suspend_all();
        self.toolchain().remove_all()?;
        self.inner.python_jobs.lock().clear();
        self.reload_modules();
        self.emit(Event::AssetsChanged { module: PYTHON_JOB.into() });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::{Config, LangId, LangPair, Paths, TranslateRequest};

    fn engine(tag: &str, python: &str, requirements: &str) -> (Engine, PathBuf) {
        let dir = std::env::temp_dir().join(format!("glossa-engine-python-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let module = dir.join("home").join("modules").join("py-deps");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(
            module.join("module.toml"),
            format!(
                "capabilities = [\"translate\"]\n[module]\nid = \"py-deps\"\nname = \"Py\"\nruntime = \"python\"\n{requirements}\n"
            ),
        )
        .unwrap();
        std::fs::write(module.join("main.py"), "raise SystemExit(1)\n").unwrap();
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        let mut cfg = Config::default();
        cfg.modules.python = python.into();
        (Engine::with_config(paths, cfg).unwrap(), dir)
    }

    #[test]
    fn a_module_with_dependencies_asks_for_the_managed_python_first() {
        let (e, dir) = engine("deps", "auto", "requirements = [\"idna\"]");
        let report = e.python_status();
        assert_eq!(report.runtimes_dir, dir.join("home").join("runtimes"));
        assert_eq!(report.venvs_dir, dir.join("home").join("venvs"));
        let m = e.module_python("py-deps").expect("a Python module");
        assert_eq!(m.venv, Some(dir.join("home").join("venvs").join("py-deps")));
        if report.supported {
            assert_eq!(m.state, "missing", "{m:?}");
            assert!(m.download_size > 10_000_000, "{m:?}");
            let req = TranslateRequest::new("hi", LangPair::new(LangId::AUTO, LangId::ENGLISH));
            let p = e.registry().get("py-deps").unwrap();
            let http = glossa_net::mock::MockHttp::new();
            let err = glossa_modules::provider::with_test_host(&http, &Default::default(), |h| p.translate(h, &req))
                .unwrap_err();
            match err {
                Error::RuntimeMissing { module, what } => {
                    assert_eq!(module, "py-deps");
                    assert!(what.contains("Python") && what.contains("uv") && what.contains("idna"), "{what}");
                }
                other => panic!("expected RuntimeMissing, got {other:?}"),
            }
            assert!(http.requests().is_empty(), "nothing is downloaded without asking");
        }
        assert!(e.module_python("nope").is_none());
        e.remove_python().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_module_without_dependencies_under_managed_needs_only_python() {
        let (e, dir) = engine("managed", "managed", "");
        let m = e.module_python("py-deps").unwrap();
        if e.python_status().supported {
            assert_eq!(m.state, "missing");
            assert_eq!(m.missing, [format!("Python {}", python_env::PYTHON_VERSION)]);
            assert_eq!(m.venv, None);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
