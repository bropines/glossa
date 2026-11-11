// The Windows executable's icon and version details. Nothing to do elsewhere.
//
// The resource itself is described in packaging/windows/version_resource.rs,
// which glossa-cli's build script shares.
include!("../../packaging/windows/version_resource.rs");

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let icon = root.join("assets/icons/glossa.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", root.join("packaging/windows/version_resource.rs").display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = windows_resource(env!("CARGO_PKG_VERSION"), "Glossa translator", "glossa.exe", Some(&icon));
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("glossa.rc");
    std::fs::write(&out, rc).expect("writing the resource script");
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().expect("embedding the Windows resources");
}
