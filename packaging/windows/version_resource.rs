// The version resource of a Windows executable, shared by the build scripts
// of glossa-ui-egui and glossa-cli (`include!`d by both, and by
// crates/glossa-cli/tests/version_numbers.rs, which checks it against
// scripts/version-numbers.sh).
//
// The resource script is written out by the build script rather than kept
// as a file: version strings passed to resource compilers as macros lose
// their quotes on the way through some of them (mingw's windres).

/// Who publishes Glossa, as Windows shows it (file properties, the
/// installer, winget's `Publisher`).
pub const PUBLISHER: &str = "Bropines";

/// The copyright line, the same as in LICENSE-MIT and NOTICE.
pub const COPYRIGHT: &str = "Copyright (c) 2026 bropines and contributors";

/// A number that grows with every release of the same `major.minor.patch`,
/// for the fields that take numbers only (the fourth part of a Windows
/// file version, the end of a macOS `CFBundleVersion`): 1000 + n for
/// `-alpha.n`, 2000 + n for `-beta.n`, 3000 + n for `-rc.n`, 9000 for the
/// release itself, and n alone for any other pre-release tag. So
/// 0.1.0-beta.1 < 0.1.0-beta.2 < 0.1.0-rc.1 < 0.1.0, where cutting the
/// suffix off made them all 0.1.0.
pub fn build_number(version: &str) -> u32 {
    let Some((_, pre)) = version.split_once('-') else { return 9000 };
    let pre = pre.split('+').next().unwrap_or(pre);
    let (tag, n) = match pre.split_once('.') {
        Some((tag, n)) => (tag, n.parse::<u32>().unwrap_or(0)),
        None => (pre, 0),
    };
    let stage = match tag {
        "alpha" => 1000,
        "beta" => 2000,
        "rc" => 3000,
        _ => 0,
    };
    stage + n.min(999)
}

/// `major.minor.patch` without a pre-release or build suffix.
fn core_version(version: &str) -> &str {
    version.split(['-', '+']).next().unwrap_or(version)
}

/// `1.2.3-beta.2` as the four numbers of a Windows file version, `1.2.3.2002`.
pub fn windows_version(version: &str) -> String {
    format!("{}.{}", core_version(version), build_number(version))
}

/// The resource script for an executable: its icon when given, and the
/// version details Explorer shows. `version` is the package version in
/// full; the numeric fields get [`windows_version`].
pub fn windows_resource(version: &str, description: &str, file_name: &str, icon: Option<&std::path::Path>) -> String {
    let numeric = windows_version(version).replace('.', ",");
    let internal = file_name.trim_end_matches(".exe");
    let icon = match icon {
        Some(path) => {
            let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            format!("1 ICON \"{}\"\n\n", path.display().to_string().replace('\\', "/"))
        }
        None => String::new(),
    };
    format!(
        r#"{icon}1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "{PUBLISHER}"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{internal}"
      VALUE "OriginalFilename", "{file_name}"
      VALUE "ProductName", "Glossa"
      VALUE "ProductVersion", "{version}"
      VALUE "LegalCopyright", "{COPYRIGHT}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}
