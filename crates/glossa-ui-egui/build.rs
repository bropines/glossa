// The Windows executable's icon and version details. Nothing to do elsewhere.
//
// The resource script is written here rather than kept as a file: version
// strings passed to resource compilers as macros lose their quotes on the way
// through some of them (mingw's windres).
fn main() {
    let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons/glossa.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    let numeric = format!(
        "{},{},{},0",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH")
    );
    let icon_path = icon.canonicalize().unwrap_or(icon).display().to_string().replace('\\', "/");
    let rc = format!(
        r#"1 ICON "{icon_path}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "bropines and contributors"
      VALUE "FileDescription", "Glossa translator"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "glossa"
      VALUE "OriginalFilename", "glossa.exe"
      VALUE "ProductName", "Glossa"
      VALUE "ProductVersion", "{version}"
      VALUE "LegalCopyright", "MIT OR Apache-2.0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("glossa.rc");
    std::fs::write(&out, rc).expect("writing the resource script");
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().expect("embedding the Windows resources");
}
