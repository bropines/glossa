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
    pub portable: bool,
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
            return Paths { portable: true, cache_dir: data.join("cache"), data_dir: data, exe_dir };
        }
        let data_dir = dirs::config_dir()
            .map(|d| d.join(app_dir_name()))
            .unwrap_or_else(|| exe_dir.join("data"));
        let cache_dir = dirs::cache_dir()
            .map(|d| d.join(app_dir_name()))
            .unwrap_or_else(|| data_dir.join("cache"));
        Paths { portable: false, exe_dir, data_dir, cache_dir }
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

    /// Where the user's own modules go when installed from the UI.
    pub fn user_module_dir(&self) -> PathBuf {
        self.data_dir.join("modules")
    }

    pub fn user_theme_dir(&self) -> PathBuf {
        self.data_dir.join("themes")
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
        assert!(p.portable);
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
