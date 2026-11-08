//! The harness over every module in the repository's `modules/` folder, the
//! bundled ones and the examples, as `cargo test` runs it everywhere.
//!
//! A module that cannot run on this machine (no Python, no Node, no JDK, a
//! native example not built) is skipped, not failed: machines differ. CI's
//! Linux job builds the examples and runs `glossa-cli module test
//! --require-runtimes modules` as well, where nothing may be skipped.

use std::path::Path;

use glossa_module_test::{find_modules, test_module, Settings, Status};

/// Modules whose program is not released yet: their `bin` asset still has
/// the all-zero placeholder hash, which the harness refuses
/// (each module's README.md, "Releasing the binary").
const UNRELEASED: &[&str] = &["process/local-llm"];

#[test]
fn every_module_in_the_repository_passes_its_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
    let dirs = find_modules(&root);
    assert!(dirs.len() >= 20, "found only {} modules under {}", dirs.len(), root.display());
    let settings = Settings { strict: true, ..Settings::default() };
    let mut failed = Vec::new();
    for dir in &dirs {
        let report = test_module(dir, &settings);
        eprint!("{}", report.render());
        if UNRELEASED.iter().any(|m| dir.ends_with(m)) {
            // Must fail, and only for its placeholder hash: once the binary
            // is released and the hash filled in, this assertion fails and
            // the entry has to go.
            let placeholder = report.findings.iter().any(|f| f.message.contains("all zeros, a placeholder"));
            assert!(report.status == Status::Fail && placeholder, "{}: remove it from UNRELEASED", dir.display());
            continue;
        }
        if report.status == Status::Fail {
            failed.push(report.id.clone());
        }
        if dir.starts_with(root.join("bundled")) {
            // Bundled modules are plain JavaScript with recorded answers:
            // nothing may excuse them.
            assert_ne!(report.status, Status::Skip, "{}", report.render());
        }
    }
    assert!(failed.is_empty(), "modules failing their fixtures: {failed:?}");
}
