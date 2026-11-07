//! Module assets from the engine's side: listing what each module needs,
//! downloading it in the background with progress and cancelling, checking
//! and deleting it. The desktop app, `glossa-cli module-assets` and the
//! control API all come through here.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glossa_core::{Error, Result};
use glossa_modules::assets::{AssetStatus, ModuleAssets, Progress as AssetProgress};
use glossa_modules::CancelToken;
use parking_lot::Mutex;
use serde::Serialize;

use crate::{Engine, Event};

/// One module's assets and where they stand.
#[derive(Debug, Clone, Serialize)]
pub struct AssetReport {
    pub module: String,
    /// The folder they are downloaded into.
    pub dir: PathBuf,
    pub assets: Vec<AssetStatus>,
    /// Bytes in all.
    pub total_size: u64,
    /// Every asset is downloaded and verified.
    pub ready: bool,
    /// A background download, running or just finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<AssetJobState>,
}

/// A background download as the front end sees it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AssetJobState {
    pub running: bool,
    pub progress: AssetProgress,
    /// Why the last download failed (empty when it worked or runs).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// A background download the engine keeps track of.
pub(crate) struct AssetJob {
    state: Arc<Mutex<AssetJobState>>,
    cancel: CancelToken,
}

/// How often a running download announces its progress.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

impl Engine {
    fn assets_of(&self, id: &str) -> Result<Arc<ModuleAssets>> {
        let registry = self.registry();
        let entry = registry.entry(id).ok_or_else(|| Error::NoSuchModule(id.to_owned()))?;
        entry.assets.clone().ok_or_else(|| Error::Config(format!("module {id} declares no files to download")))
    }

    fn report(&self, assets: &ModuleAssets) -> AssetReport {
        let statuses = assets.status();
        AssetReport {
            module: assets.module.clone(),
            dir: assets.root.clone(),
            ready: assets.ready(),
            assets: statuses,
            total_size: assets.total_size(),
            job: self.inner.asset_jobs.lock().get(&assets.module).map(|j| j.state.lock().clone()),
        }
    }

    /// The files module `id` declares for this system and where they stand.
    /// Cheap: no hashing.
    pub fn module_assets(&self, id: &str) -> Result<AssetReport> {
        let assets = self.assets_of(id)?;
        Ok(self.report(&assets))
    }

    /// Every module that declares files, with their state.
    pub fn modules_with_assets(&self) -> Vec<AssetReport> {
        let registry = self.registry();
        registry.entries().iter().filter_map(|e| e.assets.as_ref()).map(|a| self.report(a)).collect()
    }

    /// Download module `id`'s missing files and wait for them (the CLI):
    /// those `names` names, or every required one when it is empty.
    /// `progress` is called as bytes arrive. Does nothing when all are
    /// there.
    pub fn download_module_assets(
        &self,
        id: &str,
        names: &[String],
        cancel: &CancelToken,
        progress: &dyn Fn(&AssetProgress),
    ) -> Result<AssetReport> {
        let assets = self.assets_of(id)?;
        let http = self.inner.http.read().clone();
        assets.download_some(names, http.as_ref(), cancel, progress)?;
        Ok(self.report(&assets))
    }

    /// Start downloading module `id`'s missing files on a worker thread:
    /// those `names` names, or every required one when it is empty.
    /// [`Event::AssetsChanged`] reports progress and the end;
    /// [`Engine::module_assets`] has the state. Starting a download while
    /// one runs for the module does nothing.
    pub fn start_asset_download(&self, id: &str, names: &[String]) -> Result<()> {
        let assets = self.assets_of(id)?;
        let total = assets.chosen(names)?.iter().map(|s| s.size).sum();
        let names = names.to_vec();
        let state = Arc::new(Mutex::new(AssetJobState {
            running: true,
            progress: AssetProgress { asset: String::new(), done: 0, total },
            error: String::new(),
        }));
        let cancel = CancelToken::new();
        {
            let mut jobs = self.inner.asset_jobs.lock();
            if jobs.get(id).is_some_and(|j| j.state.lock().running) {
                return Ok(());
            }
            jobs.insert(id.to_owned(), AssetJob { state: state.clone(), cancel: cancel.clone() });
        }
        self.emit(Event::AssetsChanged { module: id.to_owned() });
        let engine = self.clone();
        let module = id.to_owned();
        self.spawn("assets", move || {
            let http = engine.inner.http.read().clone();
            let last = Mutex::new(Instant::now());
            let result = assets.download_some(&names, http.as_ref(), &cancel, &|p| {
                state.lock().progress = p.clone();
                let mut last = last.lock();
                if last.elapsed() >= PROGRESS_EVERY {
                    *last = Instant::now();
                    engine.emit(Event::AssetsChanged { module: module.clone() });
                }
            });
            {
                let mut s = state.lock();
                s.running = false;
                match &result {
                    Ok(()) => tracing::info!(module = %module, "module files downloaded"),
                    Err(Error::Cancelled) => s.error = engine.tr("assets.cancelled"),
                    Err(e) => {
                        tracing::warn!(module = %module, "module files failed to download: {e}");
                        s.error = e.to_string();
                    }
                }
            }
            engine.emit(Event::AssetsChanged { module });
        });
        Ok(())
    }

    /// Stop module `id`'s background download. What arrived so far is kept,
    /// and the next download resumes from it.
    pub fn cancel_asset_download(&self, id: &str) {
        if let Some(job) = self.inner.asset_jobs.lock().get(id) {
            job.cancel.cancel();
        }
    }

    /// Re-hash module `id`'s files. Damaged ones are deleted. Blocks for as
    /// long as reading the files takes: call it from a worker thread.
    pub fn verify_module_assets(&self, id: &str) -> Result<AssetReport> {
        let assets = self.assets_of(id)?;
        let checked = assets.verify()?;
        let mut report = self.report(&assets);
        report.assets = checked;
        self.emit(Event::AssetsChanged { module: id.to_owned() });
        Ok(report)
    }

    /// Delete module `id`'s downloaded files. The modules are reloaded, so a
    /// module that was running waits for its files again.
    pub fn remove_module_assets(&self, id: &str) -> Result<()> {
        let assets = self.assets_of(id)?;
        self.cancel_asset_download(id);
        assets.remove()?;
        self.inner.asset_jobs.lock().remove(id);
        self.reload_modules();
        self.emit(Event::AssetsChanged { module: id.to_owned() });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::{Config, Paths};
    use std::io::{BufRead, BufReader, Write};

    const BODY: &[u8] = b"pretend this is an ONNX model";

    /// The SHA-256 of [`BODY`].
    const BODY_SHA: &str = "15cc5ff591adfaf386c000633f8e53105175afe7b5687085ef06c828c113125c";

    /// A file server honouring `Range: bytes=N-`, answering `count` requests.
    /// Returns its port and the ranges it was asked for.
    fn file_server(count: usize) -> (u16, Arc<Mutex<Vec<Option<u64>>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for _ in 0..count {
                let Ok((stream, _)) = listener.accept() else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut from = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        from = v.trim().trim_end_matches('-').parse::<u64>().ok();
                    }
                }
                log.lock().push(from);
                let mut out = stream;
                let (status, body) = match from {
                    Some(n) => ("206 Partial Content", &BODY[n as usize..]),
                    None => ("200 OK", BODY),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = out.write_all(head.as_bytes());
                let _ = out.write_all(body);
            }
        });
        (port, seen)
    }

    fn engine(tag: &str, port: u16) -> (Engine, PathBuf) {
        let dir = std::env::temp_dir().join(format!("glossa-engine-assets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let module = dir.join("home").join("modules").join("offline-model");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(
            module.join("module.toml"),
            format!(
                "capabilities = [\"translate\"]\n[module]\nid = \"offline-model\"\nname = \"Offline\"\nruntime = \"js\"\n\
                 [[assets]]\nname = \"model.onnx\"\nurl = \"http://127.0.0.1:{port}/model.onnx\"\nsha256 = \"{}\"\nsize = {}\n",
                BODY_SHA,
                BODY.len()
            ),
        )
        .unwrap();
        std::fs::write(module.join("main.js"), "function translate(req) { return host.assetPath('model.onnx'); }")
            .unwrap();
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        (Engine::with_config(paths, Config::default()).unwrap(), dir)
    }

    #[test]
    fn the_engine_lists_downloads_resumes_and_deletes_module_files() {
        let (port, seen) = file_server(2);
        let (e, dir) = engine("blocking", port);
        let all = e.modules_with_assets();
        assert_eq!(all.len(), 1);
        let r = e.module_assets("offline-model").unwrap();
        assert!(!r.ready);
        assert_eq!(r.dir, dir.join("home").join("models").join("offline-model"));
        assert!(matches!(e.module_assets("nope"), Err(Error::NoSuchModule(_))));

        // Half a file from an earlier attempt: the download resumes it.
        let state = r.dir.join(".glossa");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("model.onnx.part"), &BODY[..8]).unwrap();
        let r = e.download_module_assets("offline-model", &[], &CancelToken::new(), &|_| {}).unwrap();
        assert!(r.ready, "{r:?}");
        assert_eq!(std::fs::read(r.dir.join("model.onnx")).unwrap(), BODY);
        assert_eq!(*seen.lock(), vec![Some(8)]);
        assert!(e.verify_module_assets("offline-model").unwrap().ready);

        e.remove_module_assets("offline-model").unwrap();
        assert!(!e.module_assets("offline-model").unwrap().ready);
        assert!(!r.dir.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_background_download_reports_through_events() {
        let (port, _) = file_server(1);
        let (e, dir) = engine("background", port);
        let rx = e.subscribe();
        e.start_asset_download("offline-model", &[]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(Event::AssetsChanged { module }) => {
                    assert_eq!(module, "offline-model");
                    let r = e.module_assets("offline-model").unwrap();
                    if r.job.as_ref().is_some_and(|j| !j.running) {
                        assert!(r.ready, "{r:?}");
                        assert!(r.job.unwrap().error.is_empty());
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => panic!("the download did not finish"),
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
