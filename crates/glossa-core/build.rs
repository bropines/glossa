// The commit the program is built from, for the About window, `glossa-cli
// --version` and the control API's /health: `git rev-parse --short HEAD`,
// with "-dirty" when tracked files have uncommitted changes. A build from a
// source archive has no git: set GLOSSA_COMMIT to say which commit it is,
// or the version stands alone.
//
// The script runs again only when HEAD, the branch it points to or the
// index change, so a commit costs one rebuild of the crates above this one
// and an ordinary edit costs nothing.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=GLOSSA_COMMIT");
    let commit = match std::env::var("GLOSSA_COMMIT").ok().filter(|c| !c.trim().is_empty()) {
        Some(given) => given.trim().to_owned(),
        None => match git(&["rev-parse", "--short=8", "HEAD"]) {
            Some(hash) => {
                for path in ["HEAD", "index"] {
                    if let Some(p) = git(&["rev-parse", "--git-path", path]) {
                        println!("cargo:rerun-if-changed={p}");
                    }
                }
                if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
                    if let Some(p) = git(&["rev-parse", "--git-path", &branch]) {
                        println!("cargo:rerun-if-changed={p}");
                    }
                }
                let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
                if dirty {
                    format!("{hash}-dirty")
                } else {
                    hash
                }
            }
            None => String::new(),
        },
    };
    println!("cargo:rustc-env=GLOSSA_COMMIT={commit}");
}
