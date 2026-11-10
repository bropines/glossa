//! Where things live on disk.
//!
//! Two layouts:
//!
//! * **Installed** — the platform's usual directories (`%APPDATA%\Glossa`,
//!   `~/.config/glossa`, `~/Library/Application Support/Glossa`).
//! * **Portable** — everything next to the executable, under `data/`, the way
//!   the classic program kept `Data/` beside its `.exe`. Chosen when a file
//!   named `portable` (any extension) or a `data/` directory sits next to the
//!   executable, or when `GLOSSA_HOME` points somewhere.
//!
//! Read-only resources (bundled themes, locales, modules) are looked up next
//! to the executable in both layouts, then in the user's own directories, so a
//! user's file with the same name wins.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    /// The data is not in the user's profile: beside the program, or in a
    /// folder given with `--home` / `GLOSSA_HOME` (see `home_override`).
    pub portable: bool,
    /// The data folder was given with `--home` or `GLOSSA_HOME`, rather than
    /// found beside the program or in the profile.
    pub home_override: bool,
    /// Where the executable is.
    pub exe_dir: PathBuf,
    /// User data: config, history, user themes and modules.
    pub data_dir: PathBuf,
    /// Throwaway: downloaded audio, module caches.
    pub cache_dir: PathBuf,
}

impl Paths {
    /// Work out the layout for the running executable.
    pub fn discover() -> Paths {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        Paths::for_exe_dir(exe_dir, std::env::var_os("GLOSSA_HOME").map(PathBuf::from))
    }

    /// The same decision, with the inputs made explicit for testing.
    pub fn for_exe_dir(exe_dir: PathBuf, home_override: Option<PathBuf>) -> Paths {
        if let Some(home) = home_override.filter(|p| !p.as_os_str().is_empty()) {
            return Paths {
                portable: true,
                home_override: true,
                cache_dir: home.join("cache"),
                data_dir: home,
                exe_dir,
            };
        }
        let marker = std::fs::read_dir(&exe_dir)
            .map(|rd| {
                rd.flatten().any(|e| {
                    let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                    name == "portable" || name.starts_with("portable.")
                })
            })
            .unwrap_or(false);
        if marker || exe_dir.join("data").is_dir() {
            let data = exe_dir.join("data");
            return Paths {
                portable: true,
                home_override: false,
                cache_dir: data.join("cache"),
                data_dir: data,
                exe_dir,
            };
        }
        let data_dir = dirs::config_dir().map(|d| d.join(app_dir_name())).unwrap_or_else(|| exe_dir.join("data"));
        let cache_dir = dirs::cache_dir().map(|d| d.join(app_dir_name())).unwrap_or_else(|| data_dir.join("cache"));
        Paths { portable: false, home_override: false, exe_dir, data_dir, cache_dir }
    }

    pub fn config_file(&self) -> PathBuf {
        self.data_dir.join("config.toml")
    }

    pub fn history_file(&self) -> PathBuf {
        self.data_dir.join("history.json")
    }

    pub fn dictionary_history_file(&self) -> PathBuf {
        self.data_dir.join("dictionary-history.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.cache_dir.join("glossa.log")
    }

    /// Where temporary files go (a screenshot for an OCR program, speech
    /// from the system voice): a folder of Glossa's own, private, instead
    /// of the temporary folder every user of the machine shares.
    pub fn temp_dir(&self) -> PathBuf {
        self.cache_dir.join("tmp")
    }

    /// Make what is already on disk the user's alone, and clear out what a
    /// killed process left behind. Files written by earlier versions were
    /// readable by every user of the machine, the settings with their API
    /// keys among them; this runs at every start, so they are private from
    /// the first start of a version that knows better. Nothing is created,
    /// and on Windows nothing is changed (see [`crate::fsio`]).
    pub fn secure(&self) {
        use crate::fsio::{make_private, sweep_stale};
        use std::time::Duration;
        for dir in [&self.data_dir, &self.cache_dir] {
            if !dir.is_dir() {
                continue;
            }
            make_private(dir);
            // The files directly inside: settings, their backups and kept
            // copies, the histories, the portal token, the logs. Modules,
            // themes and downloads in the folders below hold no secrets.
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|t| t.is_file()) {
                    make_private(&entry.path());
                }
            }
            // Ten minutes: far longer than any write, so another running
            // copy's file in the making is never taken from under it.
            sweep_stale(dir, Duration::from_secs(600), false);
        }
        let tmp = self.temp_dir();
        if tmp.is_dir() {
            make_private(&tmp);
            // An hour: longer than any OCR or speech call still running in
            // another copy of the program.
            sweep_stale(&tmp, Duration::from_secs(3600), true);
        }
    }

    /// Directories to search for themes, lowest priority first.
    pub fn theme_dirs(&self) -> Vec<PathBuf> {
        self.resource_dirs("themes")
    }

    pub fn locale_dirs(&self) -> Vec<PathBuf> {
        self.resource_dirs("locales")
    }

    pub fn module_dirs(&self) -> Vec<PathBuf> {
        self.resource_dirs("modules")
    }

    /// The module folders whose modules count as shipped with Glossa and
    /// need no approval: narrower than [`Self::module_dirs`], whose
    /// relative guesses (`<exe>/../share/…`, a development checkout) can
    /// land in folders anyone can write to (`C:\modules`, `Downloads\…`).
    /// Only the folder beside the program, the system share folders on
    /// Linux and the BSDs, the app bundle's resources on macOS, and in
    /// debug builds a development checkout. Not necessarily existing.
    pub fn shipped_module_dirs(&self) -> Vec<PathBuf> {
        // In a Windows release build nothing is pushed after this.
        #[allow(unused_mut)]
        let mut out = vec![self.exe_dir.join("modules")];
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            out.push(PathBuf::from("/usr/share/glossa/modules"));
            out.push(PathBuf::from("/usr/local/share/glossa/modules"));
        }
        #[cfg(target_os = "macos")]
        if self.exe_dir.ends_with("Contents/MacOS") {
            if let Some(contents) = self.exe_dir.parent() {
                out.push(contents.join("Resources").join("modules"));
            }
        }
        #[cfg(debug_assertions)]
        if let Some(repo) = self.exe_dir.ancestors().nth(2) {
            out.push(repo.join("assets").join("modules"));
            out.push(repo.join("modules"));
        }
        out
    }

    /// Where the user's own modules go when installed from the UI.
    pub fn user_module_dir(&self) -> PathBuf {
        self.data_dir.join("modules")
    }

    /// Where module assets (models, dictionaries) are downloaded, one
    /// folder per module id. Beside the rest of the data in a portable
    /// layout; otherwise in the local (not roaming) data folder, since a
    /// model can be hundreds of megabytes and need not follow the user
    /// between machines.
    pub fn models_dir(&self) -> PathBuf {
        self.local_data("models")
    }

    /// Where the managed Python and `uv` are downloaded
    /// (`runtimes/python-<version>/`), shared by every Python module. Local
    /// data, like [`Self::models_dir`].
    pub fn runtimes_dir(&self) -> PathBuf {
        self.local_data("runtimes")
    }

    /// Where Python modules with dependencies get their own environment
    /// (`venvs/<module-id>/`).
    pub fn venvs_dir(&self) -> PathBuf {
        self.local_data("venvs")
    }

    /// `name` beside the data in a portable layout, else in the local (not
    /// roaming) data folder.
    fn local_data(&self, name: &str) -> PathBuf {
        if self.portable {
            return self.data_dir.join(name);
        }
        dirs::data_local_dir().map(|d| d.join(app_dir_name()).join(name)).unwrap_or_else(|| self.data_dir.join(name))
    }

    /// Where the user's own interface languages and rewordings go. It is
    /// searched last, so its files win over everything else.
    pub fn user_locale_dir(&self) -> PathBuf {
        self.data_dir.join("locales")
    }

    pub fn user_theme_dir(&self) -> PathBuf {
        self.data_dir.join("themes")
    }

    /// Whether the program's own folder takes new files: where a portable
    /// copy would keep its data.
    pub fn exe_dir_writable(&self) -> bool {
        let probe = self.exe_dir.join(format!(".glossa-write-test-{}", std::process::id()));
        let ok = std::fs::write(&probe, b"").is_ok();
        let _ = std::fs::remove_file(&probe);
        ok
    }

    /// Start keeping the data beside the program: copy the settings, the
    /// history and the user's modules and themes into `<exe>/data`, which
    /// makes the next start portable. The old folder is left as it was.
    /// Returns the new data folder.
    pub fn make_portable(&self) -> std::io::Result<PathBuf> {
        let target = self.exe_dir.join("data");
        crate::fsio::create_private_dir(&target)?;
        for file in ["config.toml", "history.json", "dictionary-history.json"] {
            let from = self.data_dir.join(file);
            if from.is_file() {
                std::fs::copy(&from, target.join(file))?;
                crate::fsio::make_private(&target.join(file));
            }
        }
        for dir in ["modules", "themes"] {
            copy_tree(&self.data_dir.join(dir), &target.join(dir))?;
        }
        Ok(target)
    }

    /// `<exe>/<name>`, then `<exe>/../share/glossa/<name>` for a Unix install,
    /// then `<data>/<name>`. Only directories that exist are returned.
    fn resource_dirs(&self, name: &str) -> Vec<PathBuf> {
        let mut out = vec![self.exe_dir.join(name)];
        if let Some(prefix) = self.exe_dir.parent() {
            out.push(prefix.join("share").join("glossa").join(name));
        }
        // A development checkout: target/debug/glossa -> repo/assets/<name>.
        if let Some(repo) = self.exe_dir.ancestors().nth(2) {
            out.push(repo.join("assets").join(name));
            if name == "modules" {
                out.push(repo.join("modules"));
            }
        }
        out.push(self.data_dir.join(name));
        let mut seen = Vec::new();
        out.retain(|p| {
            if !p.is_dir() || seen.contains(p) {
                return false;
            }
            seen.push(p.clone());
            true
        });
        out
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            copy_tree(&path, &to.join(entry.file_name()))?;
        } else {
            std::fs::copy(&path, to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn app_dir_name() -> &'static str {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        "Glossa"
    } else {
        "glossa"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("glossa-paths-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&p).ok();
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// The audit's finding: an installation from before, with the settings
    /// (API keys, the proxy password, the control token) readable by
    /// every user of the machine.
    #[cfg(unix)]
    #[test]
    fn an_existing_installation_is_made_private_at_start() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = temp("secure");
        let paths = Paths::for_exe_dir(dir.join("app"), Some(dir.join("home")));
        std::fs::create_dir_all(paths.data_dir.join("modules")).unwrap();
        std::fs::create_dir_all(paths.temp_dir()).unwrap();
        let files = [
            paths.config_file(),
            paths.data_dir.join("config.toml.broken-1700000000"),
            paths.data_dir.join("config.toml.bak"),
            paths.history_file(),
            paths.dictionary_history_file(),
            paths.log_file(),
        ];
        let leftovers = [paths.data_dir.join("config.toml.tmp"), paths.temp_dir().join("glossa-ocr-1-0-0.png")];
        let module = paths.data_dir.join("modules").join("module.toml");
        for file in files.iter().chain(&leftovers).chain([&module]) {
            std::fs::write(file, "x").unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        for folder in [&paths.data_dir, &paths.cache_dir] {
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // Left by a process killed long ago.
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400);
        for file in &leftovers {
            std::fs::File::options().write(true).open(file).unwrap().set_modified(long_ago).unwrap();
        }
        paths.secure();
        assert_eq!(mode(&paths.data_dir), 0o700);
        assert_eq!(mode(&paths.cache_dir), 0o700);
        for file in &files {
            assert_eq!(mode(file), 0o600, "{}", file.display());
        }
        assert_eq!(mode(&module), 0o644, "a module's files are not secrets and stay as they were");
        for file in &leftovers {
            assert!(!file.exists(), "{} is swept", file.display());
        }
        // A fresh installation: nothing is created by looking.
        let fresh = Paths::for_exe_dir(dir.join("app"), Some(dir.join("fresh")));
        fresh.secure();
        assert!(!fresh.data_dir.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn making_a_copy_portable_moves_its_data_beside_it() {
        let dir = temp("make-portable");
        let exe = dir.join("app");
        std::fs::create_dir_all(&exe).unwrap();
        let profile = Paths {
            portable: false,
            home_override: false,
            exe_dir: exe.clone(),
            data_dir: dir.join("profile"),
            cache_dir: dir.join("cache"),
        };
        std::fs::create_dir_all(profile.data_dir.join("modules/mine")).unwrap();
        std::fs::write(
            profile.config_file(),
            "[general]
",
        )
        .unwrap();
        std::fs::write(profile.data_dir.join("modules/mine/module.toml"), "id = 'mine'").unwrap();
        assert!(profile.exe_dir_writable());
        let data = profile.make_portable().unwrap();
        assert_eq!(
            std::fs::read_to_string(data.join("config.toml")).unwrap(),
            "[general]
"
        );
        assert!(data.join("modules/mine/module.toml").is_file());
        // The next start finds it.
        assert!(Paths::for_exe_dir(exe, None).portable);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_portable_marker_keeps_data_beside_the_exe() {
        let dir = temp("marker");
        std::fs::write(dir.join("portable.txt"), "").unwrap();
        let p = Paths::for_exe_dir(dir.clone(), None);
        assert!(p.portable);
        assert_eq!(p.data_dir, dir.join("data"));
        assert_eq!(p.config_file(), dir.join("data").join("config.toml"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_data_directory_alone_also_means_portable() {
        let dir = temp("datadir");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        assert!(Paths::for_exe_dir(dir.clone(), None).portable);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn glossa_home_wins() {
        let dir = temp("home");
        let home = dir.join("elsewhere");
        let p = Paths::for_exe_dir(dir.clone(), Some(home.clone()));
        assert!(p.portable && p.home_override);
        assert_eq!(p.data_dir, home);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn without_markers_the_platform_directories_are_used() {
        let dir = temp("installed");
        let p = Paths::for_exe_dir(dir.clone(), None);
        assert!(!p.portable);
        assert!(!p.data_dir.starts_with(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resource_dirs_list_only_existing_directories_in_priority_order() {
        let dir = temp("resources");
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::create_dir_all(dir.join("data").join("themes")).unwrap();
        let p = Paths::for_exe_dir(dir.clone(), None);
        let dirs = p.theme_dirs();
        assert_eq!(dirs.first(), Some(&dir.join("themes")));
        assert_eq!(dirs.last(), Some(&dir.join("data").join("themes")), "user dir comes last, wins");
        assert!(dirs.iter().all(|d| d.is_dir()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
