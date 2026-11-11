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

/// The file name of the third-party notices in a release, written by
/// `scripts/third-party-notices.py` when the release is built.
pub const THIRD_PARTY_NOTICES: &str = "THIRD-PARTY-NOTICES.txt";

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

    /// The third-party notices a release ships
    /// ([`THIRD_PARTY_NOTICES`]): every crate and bundled component in
    /// the programs with its licence text, far too long to build in.
    /// Beside the program (Windows, the portable archives), in the app
    /// bundle's resources (macOS) or in the package's doc folder (Linux).
    /// `None` in a build that was not packaged, such as a development one.
    pub fn third_party_notices(&self) -> Option<PathBuf> {
        let mut places = vec![self.exe_dir.join(THIRD_PARTY_NOTICES)];
        if let Some(prefix) = self.exe_dir.parent() {
            places.push(prefix.join("Resources").join(THIRD_PARTY_NOTICES));
            places.push(prefix.join("share").join("doc").join("glossa").join(THIRD_PARTY_NOTICES));
        }
        places.into_iter().find(|p| p.is_file())
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
        std::fs::create_dir_all(&target)?;
        for file in ["config.toml", "history.json", "dictionary-history.json"] {
            let from = self.data_dir.join(file);
            if from.is_file() {
                std::fs::copy(&from, target.join(file))?;
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
