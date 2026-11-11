// glossa-cli.exe's icon and version details on Windows, the same as the
// desktop program's (packaging/windows/version_resource.rs), so Explorer,
// SmartScreen and an inventory tool can tell what the file is and which
// release it belongs to. Nothing to do elsewhere.
include!("../../packaging/windows/version_resource.rs");

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let icon = root.join("assets/icons/glossa.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", root.join("packaging/windows/version_resource.rs").display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = windows_resource(env!("CARGO_PKG_VERSION"), "Glossa command line", "glossa-cli.exe", Some(&icon));
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("glossa-cli.rc");
    std::fs::write(&out, rc).expect("writing the resource script");
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().expect("embedding the Windows resources");
}
