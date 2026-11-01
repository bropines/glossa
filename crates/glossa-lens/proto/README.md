# Lens protos

`lens.fds` is a `FileDescriptorSet` of Chromium's
`third_party/lens_server_proto/*.proto` (45 files, all but
`aim_communication.proto`, which imports protos from outside that directory),
at the Chromium commit in `CHROMIUM_COMMIT`. `build.rs` turns it into Rust with
prost-build; no protoc is needed to build Glossa.

The descriptors are the ones chrome-lens-py compiled, taken from its generated
`*_pb2.py` modules, so both clients speak the same version of the protocol.

## Moving to newer protos

1. In a chrome-lens-py checkout, run its `scripts/update_protos.py` at the
   Chromium commit wanted. It resolves the ref, downloads the `.proto` files,
   compiles them with a protoc new enough for their editions syntax and fails
   on any file it cannot compile or any import it cannot resolve.
2. From the Glossa repository:

   ```
   python scripts/lens-descriptors.py <chrome-lens-py checkout> <chromium commit>
   ```

   It rewrites `lens.fds` and `CHROMIUM_COMMIT`, and fails if an import does
   not resolve inside the set.
3. `cargo test -p glossa-lens` and a real recognition (`glossa-cli ocr
   picture.png -e lens`) before committing.
