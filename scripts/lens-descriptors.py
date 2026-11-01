#!/usr/bin/env python3
"""Rebuild crates/glossa-lens/proto/lens.fds from a chrome-lens-py checkout.

Glossa's built-in Lens client is generated (prost-build, in the crate's
build.rs) from Chromium's own lens_server_proto files. Rather than vendoring
the .proto sources and needing protoc 33 or newer to read their editions
syntax, it takes the descriptors chrome-lens-py already compiled: every
generated *_pb2.py module carries its file's serialized FileDescriptorProto.
This script collects them into one FileDescriptorSet, checks that every
import resolves inside the set, and fails on any problem instead of skipping
a file quietly.

    python scripts/lens-descriptors.py <chrome-lens-py checkout> <chromium commit>

To move to newer protos: run chrome-lens-py's scripts/update_protos.py at the
Chromium commit wanted (it pins and validates the .proto files), then this
script with the same commit. Needs nothing beyond the Python standard
library.
"""
import ast
import pathlib
import sys

# Never compiled by chrome-lens-py either: it imports protos from outside
# lens_server_proto (omnibox_proto, optimization_guide).
EXCLUDED = {"aim_communication.proto"}


def varint(n: int) -> bytes:
    out = bytearray()
    while True:
        byte = n & 0x7F
        n >>= 7
        if n:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def read_varint(data: bytes, i: int):
    shift = result = 0
    while True:
        b = data[i]
        i += 1
        result |= (b & 0x7F) << shift
        if not b & 0x80:
            return result, i
        shift += 7


def fields(data: bytes):
    """(field number, wire type, value) for the top level of a message."""
    i = 0
    while i < len(data):
        key, i = read_varint(data, i)
        number, wire = key >> 3, key & 7
        if wire == 0:
            value, i = read_varint(data, i)
        elif wire == 2:
            length, i = read_varint(data, i)
            value, i = data[i:i + length], i + length
        elif wire == 1:
            value, i = data[i:i + 8], i + 8
        elif wire == 5:
            value, i = data[i:i + 4], i + 4
        else:
            raise ValueError(f"unexpected wire type {wire}")
        yield number, wire, value


def serialized_file(module: pathlib.Path) -> bytes:
    """The bytes passed to AddSerializedFile in a generated module."""
    tree = ast.parse(module.read_text(encoding="utf-8"))
    for node in ast.walk(tree):
        if (
            isinstance(node, ast.Call)
            and getattr(node.func, "attr", None) == "AddSerializedFile"
            and node.args
            and isinstance(node.args[0], ast.Constant)
            and isinstance(node.args[0].value, bytes)
        ):
            return node.args[0].value
    raise SystemExit(f"{module.name}: no serialized descriptor found")


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    checkout, commit = pathlib.Path(sys.argv[1]), sys.argv[2].strip()
    modules = sorted((checkout / "src/chrome_lens_py/utils/protobufs").glob("*_pb2.py"))
    if not modules:
        raise SystemExit(f"no generated modules under {checkout}")
    files = {}
    for module in modules:
        data = serialized_file(module)
        name = next((v.decode() for n, w, v in fields(data) if n == 1 and w == 2), None)
        if not name:
            raise SystemExit(f"{module.name}: the descriptor has no file name")
        if name in EXCLUDED:
            continue
        deps = [v.decode() for n, w, v in fields(data) if n == 3 and w == 2]
        files[name] = (data, deps)
    problems = [f"{name} imports {dep}, which is not in the set" for name, (_, deps) in files.items()
                for dep in deps if dep not in files]
    if problems:
        raise SystemExit("unresolved imports:\n  " + "\n  ".join(problems))
    # FileDescriptorSet { repeated FileDescriptorProto file = 1; }, imports
    # before their importers.
    ordered, seen = [], set()

    def visit(name: str) -> None:
        if name in seen:
            return
        seen.add(name)
        for dep in files[name][1]:
            visit(dep)
        ordered.append(name)

    for name in sorted(files):
        visit(name)
    out = b"".join(b"\x0a" + varint(len(files[n][0])) + files[n][0] for n in ordered)
    proto_dir = pathlib.Path(__file__).resolve().parent.parent / "crates/glossa-lens/proto"
    proto_dir.mkdir(parents=True, exist_ok=True)
    (proto_dir / "lens.fds").write_bytes(out)
    (proto_dir / "CHROMIUM_COMMIT").write_text(commit + "\n", encoding="utf-8")
    print(f"{len(ordered)} files from chromium@{commit[:12]}, {len(out)} bytes -> {proto_dir / 'lens.fds'}")


if __name__ == "__main__":
    main()
