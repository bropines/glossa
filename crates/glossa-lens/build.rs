// Rust types for Chromium's lens_server_proto, from the descriptors in
// proto/lens.fds (see scripts/lens-descriptors.py and proto/README.md). No
// protoc is needed: the descriptors were compiled once, by chrome-lens-py's
// pinned protoc, and are checked in.
//
// 28 of the files use the editions syntax, which prost-build does not read.
// Edition 2023's defaults (explicit field presence) are proto2's, and the
// wire format is the same, so those files are handed over as proto2.
use prost::Message;

fn main() {
    let path = "proto/lens.fds";
    println!("cargo:rerun-if-changed={path}");
    let bytes = std::fs::read(path).expect("proto/lens.fds");
    let mut set = prost_types::FileDescriptorSet::decode(bytes.as_slice()).expect("a FileDescriptorSet");
    for file in &mut set.file {
        if file.syntax.as_deref() == Some("editions") {
            file.syntax = Some("proto2".into());
        }
    }
    prost_build::Config::new().compile_fds(set).expect("generating the Lens protos");
}
