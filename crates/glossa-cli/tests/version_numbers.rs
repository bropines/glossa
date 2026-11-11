//! The numbers-only forms of the version (a Windows file version, a macOS
//! build number) are worked out twice: in Rust for the build scripts
//! (`packaging/windows/version_resource.rs`) and in shell for the packaging
//! (`scripts/version-numbers.sh`). They must agree, and a pre-release must
//! sort before its release.

#[allow(dead_code)]
mod resource {
    include!("../../../packaging/windows/version_resource.rs");
}

const VERSIONS: &[(&str, u32, &str)] = &[
    ("0.1.0-alpha.3", 1003, "0.1.0.1003"),
    ("0.1.0-beta.1", 2001, "0.1.0.2001"),
    ("0.1.0-beta.2", 2002, "0.1.0.2002"),
    ("0.1.0-rc.1", 3001, "0.1.0.3001"),
    ("0.1.0", 9000, "0.1.0.9000"),
    ("0.1.1-dev", 0, "0.1.1.0"),
    ("1.12.3-beta.10+build5", 2010, "1.12.3.2010"),
    ("2.0.0+meta", 9000, "2.0.0.9000"),
];

#[test]
fn the_build_number_keeps_the_pre_release_and_its_order() {
    for (version, build, windows) in VERSIONS {
        assert_eq!(resource::build_number(version), *build, "{version}");
        assert_eq!(resource::windows_version(version), *windows, "{version}");
    }
    let order = ["0.1.0-alpha.3", "0.1.0-beta.1", "0.1.0-beta.2", "0.1.0-rc.1", "0.1.0"];
    assert!(order.windows(2).all(|w| resource::build_number(w[0]) < resource::build_number(w[1])));
}

#[test]
fn the_resource_names_the_file_the_full_version_and_the_copyright() {
    let rc = resource::windows_resource("0.1.0-beta.2", "Glossa command line", "glossa-cli.exe", None);
    assert!(rc.contains("FILEVERSION 0,1,0,2002") && rc.contains("PRODUCTVERSION 0,1,0,2002"), "{rc}");
    assert!(rc.contains(r#"VALUE "FileVersion", "0.1.0-beta.2""#), "{rc}");
    assert!(rc.contains(r#"VALUE "OriginalFilename", "glossa-cli.exe""#), "{rc}");
    assert!(rc.contains(r#"VALUE "InternalName", "glossa-cli""#), "{rc}");
    assert!(rc.contains(resource::COPYRIGHT) && !rc.contains("ICON"), "{rc}");
    // The line every licence file carries.
    let mit = include_str!("../../../LICENSE-MIT");
    assert!(mit.contains(resource::COPYRIGHT), "LICENSE-MIT and the version resource disagree on the copyright line");
}

/// The shell script the release workflow and the macOS bundle use.
#[test]
#[cfg(unix)]
fn the_shell_script_agrees() {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/version-numbers.sh");
    let run = |version: &str| {
        let out = std::process::Command::new("sh").arg(script).arg(version).output().expect("running sh");
        assert!(out.status.success(), "{version}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    for (version, build, windows) in VERSIONS {
        let out = run(version);
        assert!(out.contains(&format!("build={build}\n")), "{version}: {out}");
        assert!(out.contains(&format!("windows={windows}\n")), "{version}: {out}");
    }
    assert!(run("0.1.0-beta.2").contains("macos=1002002\n"));
    assert!(run("0.1.0").contains("macos=1009000\n"));
    assert!(run("1.12.3").contains("macos=112039000\n"));
    // The workspace's own version, read from Cargo.toml.
    let own = std::process::Command::new("sh").arg(script).output().unwrap();
    assert!(String::from_utf8_lossy(&own.stdout).contains(&format!("version={}\n", env!("CARGO_PKG_VERSION"))));
    let bad = std::process::Command::new("sh").arg(script).arg("beta").output().unwrap();
    assert!(!bad.status.success());
}
