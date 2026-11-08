"""Write the licence notices of every crate compiled into paddle-ocr.

    python3 third_party.py [TARGET] > THIRD-PARTY-NOTICES.txt

Reads `cargo metadata` for this crate (TARGET, a Rust target triple,
narrows platform-specific dependencies; the host's by default), follows the
normal dependencies from paddle-ocr, and prints each crate's name, version,
licence expression and the licence and notice files it ships, from the
local cargo registry. package.sh runs it for the archive. Standard library
only, so it runs on any CI runner with Python 3.
"""

import json
import os
import subprocess
import sys

NOTICE_PREFIXES = ("license", "licence", "copying", "notice", "copyright", "unlicense", "authors")


def metadata(target):
    here = os.path.dirname(os.path.abspath(__file__))
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked", "--manifest-path", os.path.join(here, "Cargo.toml")]
    if target:
        cmd += ["--filter-platform", target]
    return json.loads(subprocess.run(cmd, check=True, capture_output=True, encoding="utf-8").stdout)


def compiled_in(meta):
    """Packages reachable from the root through normal (not dev or build)
    dependencies: what ends up in the binary. Proc macros run at build time
    only and are left out."""
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    packages = {p["id"]: p for p in meta["packages"]}
    root = meta["resolve"]["root"]
    seen, todo = set(), [root]
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]):
                todo.append(dep["pkg"])
    seen.discard(root)
    out = []
    for pid in seen:
        p = packages[pid]
        if any("proc-macro" in t["kind"] for t in p["targets"]):
            continue
        out.append(p)
    return sorted(out, key=lambda p: (p["name"], p["version"]))


def notice_files(package):
    folder = os.path.dirname(package["manifest_path"])
    names = sorted(
        f for f in os.listdir(folder) if f.lower().startswith(NOTICE_PREFIXES) and os.path.isfile(os.path.join(folder, f))
    )
    if package.get("license_file"):
        extra = os.path.normpath(os.path.join(folder, package["license_file"]))
        if os.path.isfile(extra) and os.path.basename(extra) not in names:
            names.append(os.path.relpath(extra, folder))
    return [(n, os.path.join(folder, n)) for n in names]


def main():
    # UTF-8 and Unix line ends everywhere: Windows' console encoding cannot
    # hold every name in the licence files.
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    target = sys.argv[1] if len(sys.argv) > 1 else None
    packages = compiled_in(metadata(target))
    w = sys.stdout.write
    w("Third-party software in paddle-ocr\n")
    w("==================================\n\n")
    w("paddle-ocr is MIT OR Apache-2.0 (LICENSE-MIT.txt, LICENSE-APACHE.txt).\n")
    w("ONNX Runtime, shipped beside it, has its own notices\n")
    w("(LICENSE-onnxruntime.txt, ThirdPartyNotices-onnxruntime.txt).\n")
    w(f"These {len(packages)} Rust crates are compiled into the program:\n\n")
    for p in packages:
        w(f"  {p['name']} {p['version']}  {p.get('license') or 'see its files'}\n")
    for p in packages:
        w("\n" + "-" * 78 + "\n")
        w(f"{p['name']} {p['version']}\n")
        w(f"License: {p.get('license') or 'see below'}\n")
        if p.get("repository"):
            w(f"Source: {p['repository']}\n")
        files = notice_files(p)
        if not files:
            w("(The crate ships no licence file; its licence is the expression above.)\n")
        for name, path in files:
            w(f"\n--- {name} ---\n\n")
            with open(path, encoding="utf-8", errors="replace") as f:
                w(f.read().rstrip() + "\n")


if __name__ == "__main__":
    main()
