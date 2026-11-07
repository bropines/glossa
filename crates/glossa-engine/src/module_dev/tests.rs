use super::*;
use glossa_core::{Config, Paths};

fn engine(name: &str) -> (Engine, PathBuf) {
    let dir = std::env::temp_dir().join(format!("glossa-moddev-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
    let e = Engine::with_config(paths, Config::default()).unwrap();
    e.reload_modules();
    (e, dir)
}

fn errors(problems: &[Problem]) -> Vec<&Problem> {
    problems.iter().filter(|p| p.severity == Severity::Error).collect()
}

#[test]
fn every_template_checks_clean_and_passes_its_own_tests() {
    let (e, root) = engine("templates");
    for t in Template::ALL {
        let dir = e.new_module(t, &format!("My {} module", t.name())).unwrap();
        let id = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(id, format!("my-{}-module", t.name()));
        assert!(e.registry().get(&id).is_some(), "{id} is loaded at once");
        assert_eq!(e.module_source(&id), Some(ModuleSource::User(dir.clone())));
        let problems = e.check_module(&dir);
        assert!(errors(&problems).is_empty(), "{id}: {problems:?}");
        let report = e.test_module(&dir);
        assert_eq!(report.status, TestStatus::Pass, "{}", report.render());
        assert!(report.cases.len() >= 2, "{}", report.render());
    }
    // The same name again gets a folder of its own.
    let again = e.new_module(Template::Translator, "My translator module").unwrap();
    assert!(again.ends_with("my-translator-module-2"), "{}", again.display());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn the_edit_check_reload_test_loop() {
    let (e, root) = engine("loop");
    let dir = e.new_module(Template::Translator, "Loop").unwrap();

    // A syntax error is placed in the script.
    let good = read_module_file(&dir, "main.js").unwrap();
    let broken = good.replacen("var key = host.options.api_key;", "var key = host.options.api_key +;", 1);
    save_module_file(&dir, "main.js", &broken).unwrap();
    let problems = e.check_module(&dir);
    let p = errors(&problems).into_iter().find(|p| p.file == "main.js").unwrap_or_else(|| panic!("{problems:?}"));
    assert_eq!(p.line, 9, "{p:?}");
    assert!(p.message.contains("SyntaxError"), "{p:?}");
    // Reloading a broken module keeps the old one running and says why.
    assert!(e.reload_module(&dir).is_err());
    assert!(e.registry().get("loop").is_some());
    assert!(e.module_status().failed.iter().any(|(d, _)| d.ends_with("loop")));

    // A mistake that only shows when it runs: the failing case names the line.
    let runtime = good.replacen("var data = r.json();", "var data = r.json();\n  data = undefinedThing.x;", 1);
    save_module_file(&dir, "main.js", &runtime).unwrap();
    assert!(errors(&e.check_module(&dir)).is_empty());
    let report = e.test_module(&dir);
    assert_eq!(report.status, TestStatus::Fail);
    let failed = report.cases.iter().find(|c| c.status == TestStatus::Fail).unwrap();
    let place = failed.problems.iter().find_map(|p| find_place(p)).unwrap_or_else(|| panic!("{failed:?}"));
    assert_eq!((place.0.as_str(), place.1), ("main.js", 23), "{failed:?}");

    // Fixed, renamed and reloaded: only this provider changes.
    save_module_file(&dir, "main.js", &good).unwrap();
    let manifest = read_module_file(&dir, "module.toml").unwrap().replace("name = \"Loop\"", "name = \"Loop two\"");
    save_module_file(&dir, "module.toml", &manifest).unwrap();
    let google_before = e.registry().get("google").map(|p| std::sync::Arc::as_ptr(&p) as *const () as usize);
    assert_eq!(e.reload_module(&dir).unwrap(), ReloadOutcome::Reloaded { id: "loop".into() });
    assert_eq!(e.registry().get("loop").unwrap().info().name, "Loop two");
    let google_after = e.registry().get("google").map(|p| std::sync::Arc::as_ptr(&p) as *const () as usize);
    assert_eq!(google_before, google_after, "other providers are kept, not rebuilt");
    assert!(e.module_status().failed.is_empty());
    // What the module logs while its tests run is kept for the editor.
    let log = e.module_log("loop");
    assert_eq!(e.test_module(&dir).status, TestStatus::Pass);
    assert!(log.lines().iter().any(|l| l.message.contains("translated 12 characters")), "{:?}", log.lines());

    // A changed id replaces the provider from the same folder.
    let renamed = manifest.replace("id = \"loop\"", "id = \"loop-renamed\"");
    save_module_file(&dir, "module.toml", &renamed).unwrap();
    e.reload_module(&dir).unwrap();
    assert!(e.registry().get("loop").is_none());
    assert!(e.registry().get("loop-renamed").is_some());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn manifest_problems_have_places() {
    let (e, root) = engine("manifest");
    let dir = e.new_module(Template::Dictionary, "Places").unwrap();
    let text = read_module_file(&dir, "module.toml").unwrap();

    save_module_file(&dir, "module.toml", &text.replace("version = \"0.1.0\"", "version = 0.1.0\"")).unwrap();
    let problems = e.check_module(&dir);
    let p = &errors(&problems)[0];
    assert_eq!((p.file.as_str(), p.line), ("module.toml", 9), "{problems:?}");
    assert!(p.column > 0);

    save_module_file(&dir, "module.toml", &text.replace("entry = \"main.js\"", "entyr = \"main.js\"")).unwrap();
    let problems = e.check_module(&dir);
    let p = errors(&problems).into_iter().find(|p| p.message.contains("entyr")).expect("unknown key");
    assert_eq!((p.file.as_str(), p.line), ("module.toml", 11), "{p:?}");

    save_module_file(&dir, "module.toml", &text).unwrap();
    save_module_file(&dir, "tests/basic.toml", "[[case]\nname = 1").unwrap();
    let problems = e.check_module(&dir);
    assert!(problems.iter().any(|p| p.file == "tests/basic.toml" && p.line == 1), "{problems:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn bundled_modules_are_copied_out_to_be_edited() {
    let (e, root) = engine("copy");
    assert_eq!(e.module_source("wikipedia"), Some(ModuleSource::Bundled));
    assert_eq!(e.module_source("google"), None, "a built-in is not listed");
    let dir = e.copy_module_to_user("wikipedia").unwrap();
    assert!(dir.join("module.toml").is_file() && dir.join("main.js").is_file());
    assert_eq!(e.module_source("wikipedia"), Some(ModuleSource::User(dir.clone())), "the copy replaced it");
    assert_eq!(e.copy_module_to_user("wikipedia").unwrap(), dir, "already the user's");
    let files = module_files(&dir);
    assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["main.js", "module.toml"]);
    assert!(files.iter().all(|f| f.text));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn saving_stays_inside_the_folder_and_leaves_no_temporary_file() {
    let dir = std::env::temp_dir().join(format!("glossa-moddev-save-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    save_module_file(&dir, "tests/a.toml", "x = 1").unwrap();
    save_module_file(&dir, "tests/a.toml", "x = 2").unwrap();
    assert_eq!(read_module_file(&dir, "tests/a.toml").unwrap(), "x = 2");
    assert_eq!(std::fs::read_dir(dir.join("tests")).unwrap().count(), 1);
    for bad in ["../evil.js", "/etc/passwd", "", "a/../../b"] {
        assert!(save_module_file(&dir, bad, "x").is_err(), "{bad}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn small_helpers() {
    assert_eq!(module_id_for("  My Translator!  "), "my-translator");
    assert_eq!(module_id_for("Яндекс"), "my-module");
    assert_eq!(find_place("TypeError: x (at main.js:14:9)"), Some(("main.js".into(), 14, 9)));
    assert_eq!(find_place("main.js:3: bad"), Some(("main.js".into(), 3, 0)));
    assert_eq!(find_place("took 12:30 hours"), None);
    assert_eq!(line_col("ab\ncdé\nf", 7), (2, 4));
    assert_eq!(line_col("x", 0), (1, 1));
    let p = Problem::new("main.js", 3, 0, Severity::Error, "x");
    assert_eq!(p.place(), "main.js:3");
}
