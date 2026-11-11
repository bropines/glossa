#!/usr/bin/env python3
"""The third-party notices, licence check and SBOM of a Glossa release.

    scripts/third-party-notices.py [--target TRIPLE] [-o FILE]   the notices
    scripts/third-party-notices.py --check [--target TRIPLE ...] the licence check
    scripts/third-party-notices.py --sbom FILE [--target TRIPLE] a CycloneDX SBOM
    scripts/third-party-notices.py --summary [--target TRIPLE]   counts, for a look

What ships is worked out the way cargo builds it: `cargo metadata` for one
target, then every package reachable from glossa-ui-egui and glossa-cli
through normal dependencies. Build scripts, proc macros and development
dependencies run on the build machine and are not in the binaries, so they
are left out. (Features are resolved for the whole workspace, as `cargo
metadata` does, so an optional crate another workspace member switches on
can be listed although the two programs do not link it; too many notices
rather than too few.)

For each package the notices give its licence expression and the licence
and notice files its own source ships (from the local cargo registry, so
run `cargo fetch` first on a fresh machine), each distinct text printed
once. A crate without a licence file of its own borrows the text from
another crate of the same repository, and failing that gets the standard
text of its licence from scripts/licenses/; the list says which.
`COMPONENTS` below adds what is in the release but not a crate: the fonts,
QuickJS, vendored code, adapted code and the AppImage runtime.

--check fails when a package's licence expression cannot be satisfied from
`ALLOWED`, when a package states no licence, or when a text is missing: a
new licence in the tree is a decision for a person, not something to ship
by accident. CI runs it (.github/workflows/ci.yml, `licences`), and so does
the release before anything is built.

Python 3.11 or newer, standard library only, so it runs on a CI runner as
it is. The same approach, for one module's program, is in
modules/process/paddle-ocr/third_party.py.
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tomllib
import uuid

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# The two programs every archive holds.
ROOTS = ("glossa-ui-egui", "glossa-cli")

# What .github/workflows/release.yml builds.
RELEASE_TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
)

# Licences Glossa's binaries may carry code under. All allow redistribution
# in binary form with the notices this script collects. Two need more than
# a notice, and say what:
#   MPL-2.0   file-level copyleft: the crates are used unmodified, and the
#             notices say where their source is.
#   OFL-1.1, Ubuntu-font-1.0   font licences (egui's default fonts): the
#             fonts are embedded unmodified.
# Anything else (GPL, LGPL, AGPL, SSPL, a custom text…) fails --check until
# someone has read it and added it here on purpose.
ALLOWED = {
    "0BSD",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "CC0-1.0",
    "CDLA-Permissive-2.0",
    "ISC",
    "MIT",
    "MIT-0",
    "MPL-2.0",
    "OFL-1.1",
    "Ubuntu-font-1.0",
    "Unicode-3.0",
    "Unlicense",
    "Zlib",
}

# When an expression offers a choice, the licence Glossa takes: the first of
# these the expression allows. Apache-2.0 comes first because its text is
# the same for every crate, so a crate that ships no licence file can still
# be given its exact terms.
PREFERRED = ("Apache-2.0", "MIT", "BSD-3-Clause", "BSD-2-Clause", "ISC", "Zlib", "0BSD", "Unlicense")

# The standard texts in scripts/licenses/, for crates that ship none.
STANDARD_TEXTS = {
    "Apache-2.0": "Apache-2.0.txt",
    "MPL-2.0": "MPL-2.0.txt",
    "BSL-1.0": "BSL-1.0.txt",
    # The copyright line is filled in from the crate's `authors`.
    "MIT": "MIT.txt",
}

# Licence files of repositories whose crates ship none, copied from the
# repository into scripts/licenses/ (its README.md says from where):
# repository -> licence expression -> files.
REPOSITORY_TEXTS = {
    "https://github.com/madsmtm/objc2": {
        "MIT": ("objc2-LICENSE-MIT.txt",),
        "Zlib OR Apache-2.0 OR MIT": ("objc2-LICENSE-APACHE.txt", "objc2-LICENSE-MIT.txt", "objc2-LICENSE-ZLIB.txt"),
    },
}

NOTICE_NAME = re.compile(r"^(licen[sc]e|copying|notice|copyright|unlicense|authors|patents)", re.I)
# Deeper in a crate, only files that are plainly a licence: a vendored
# library's (rquickjs-sys/quickjs/LICENSE) or a font's.
NESTED_NAME = re.compile(r"^(licen[sc]e|copying|notice)([-_.].*)?$|.*[-_]licen[sc]e\.(txt|md)$", re.I)
# Source and data files that merely have such a name (objc2's copying.rs).
NOT_A_TEXT = re.compile(r"\.(rs|c|h|cc|cpp|m|py|js|ts|sh|toml|json|ya?ml|html|svg|png)$", re.I)
# The text of a licence Glossa does not take a crate under (a crate offered
# as "Apache-2.0 OR GPL-2.0-only" ships both) has no place in the notices.
NOT_TAKEN = re.compile(r"GPL", re.I)
NESTED_SKIP = {"tests", "test", "testdata", "examples", "benches", "fuzz", "target", "docs", "doc", ".git", ".github"}
NESTED_DEPTH = 4
# Licence files deeper in a crate for things that are not in the programs.
NESTED_IGNORE = {
    # Dictionaries for its tests and examples; Glossa reads the system's.
    "spellbook": ("vendor/",),
    # A C library for its WebAssembly build only.
    "rquickjs-sys": ("vendor/wasi-libc/",),
}
# Licence files a crate keeps under a name the patterns above do not catch.
EXTRA_FILES = {
    "epaint_default_fonts": ("fonts/OFL.txt", "fonts/UFL.txt", "fonts/Hack-Regular.txt"),
}

# What a release holds besides crates. `crate` names the package the thing
# comes with (the entry is printed when that package ships); `targets` is a
# substring the target must contain. `files` are licence texts inside this
# repository. Keep it in step with NOTICE, which tells the same story for a
# reader of the source.
COMPONENTS = (
    {
        "name": "Hack, Ubuntu Light, Noto Emoji and emoji-icon-font (fonts)",
        "crate": "epaint_default_fonts",
        "licence": "MIT and Bitstream Vera (Hack), Ubuntu Font Licence 1.0 (Ubuntu Light), "
        "SIL OFL 1.1 (Noto Emoji), MIT (emoji-icon-font)",
        "source": "https://github.com/emilk/egui/tree/main/crates/epaint_default_fonts",
        "note": "egui's default fonts, embedded unmodified in the glossa program. Their licence "
        "texts are listed under the crate epaint_default_fonts below.",
    },
    {
        "name": "QuickJS-ng (C library)",
        "crate": "rquickjs-sys",
        "licence": "MIT",
        "source": "https://github.com/quickjs-ng/quickjs",
        "note": "The JavaScript engine, compiled in through the crate rquickjs-sys, which carries "
        "its source and licence (listed under that crate below).",
    },
    {
        "name": "BoringSSL and OpenSSL-derived code (C and assembly)",
        "crate": "ring",
        "licence": "Apache-2.0 AND ISC, with parts under the OpenSSL and SSLeay licences and MIT",
        "source": "https://github.com/briansmith/ring",
        "note": "Cryptography, compiled in through the crate ring (texts under that crate below).",
    },
    {
        "name": "Mozilla's CA certificate list",
        "crate": "webpki-roots",
        "licence": "CDLA-Permissive-2.0",
        "source": "https://github.com/rustls/webpki-roots",
        "note": "The root certificates TLS connections are checked against, as data in the crate "
        "webpki-roots (text under that crate below).",
    },
    {
        "name": "Chromium's lens_server_proto (protocol definitions)",
        "crate": "chrome_lens_ocr",
        "licence": "BSD-3-Clause, Copyright 2024 The Chromium Authors",
        "source": "https://chromium.googlesource.com/chromium/src/+/main/third_party/lens_server_proto",
        "note": "Vendored by the crate chrome_lens_ocr, whose source does not carry the licence "
        "file the definitions point at; the text here is Chromium's own LICENSE.",
        "files": ("scripts/licenses/chromium-LICENSE.txt",),
    },
    {
        "name": "eframe 0.37.0, vendored with local fixes (third_party/eframe)",
        "crate": "eframe",
        "licence": "MIT OR Apache-2.0, Copyright (c) 2018-2021 Emil Ernerfeldt",
        "source": "https://github.com/emilk/egui",
        "note": "Changed files are marked \"Glossa patch\"; third_party/README.md in the source "
        "describes the changes. Its default window icon (data/icon.png) is egui's, under the "
        "same terms.",
    },
    {
        "name": "fastframe-scroll (adapted code)",
        "crate": "glossa-ui-egui",
        "licence": "MIT",
        "source": "https://github.com/crmne/fastframe",
        "note": "The touchpad axis lock in crates/glossa-ui-egui/src/scroll.rs is adapted from it.",
        "files": ("scripts/licenses/fastframe-MIT.txt",),
    },
    {
        "name": "KDE Plasma's Wayland protocol definitions",
        "crate": "wayland-protocols-plasma",
        "targets": "linux",
        "licence": "LGPL-2.1-or-later for most files; BSD-3-Clause, MIT, MIT-CMU and CC0-1.0 for the rest",
        "source": "https://invent.kde.org/libraries/plasma-wayland-protocols",
        "note": "XML descriptions of KDE's Wayland protocols, carried by the crate "
        "wayland-protocols-plasma (MIT), which generates Rust bindings from them when Glossa is "
        "built; winit uses the one for window blur. The LGPL text is listed under that crate "
        "below.",
    },
    {
        "name": "AppImage type 2 runtime (the .AppImage file only)",
        "crate": "glossa-ui-egui",
        "targets": "linux",
        "licence": "MIT; statically links squashfuse (BSD-2-Clause), libfuse (LGPL-2.1), "
        "zstd (BSD-3-Clause), zlib (Zlib) and musl (MIT)",
        "source": "https://github.com/AppImage/type2-runtime",
        "note": "The small program at the start of the .AppImage file that mounts it; an "
        "unmodified release binary from that project, not linked with Glossa. Its source and "
        "the licences of its parts are at the address above. The .deb and the portable archive "
        "do not contain it.",
    },
)


def die(message):
    sys.stderr.write(f"third-party-notices: {message}\n")
    sys.exit(1)


# --- the dependency graph ----------------------------------------------------


def metadata(target):
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked", "--manifest-path", os.path.join(ROOT, "Cargo.toml")]
    if target:
        cmd += ["--filter-platform", target]
    try:
        out = subprocess.run(cmd, check=True, capture_output=True, encoding="utf-8")
    except subprocess.CalledProcessError as e:
        die(f"cargo metadata failed:\n{e.stderr}")
    return json.loads(out.stdout)


def host_target():
    out = subprocess.run(["rustc", "-vV"], check=True, capture_output=True, encoding="utf-8").stdout
    return re.search(r"^host: (.+)$", out, re.M).group(1)


def is_proc_macro(package):
    return any("proc-macro" in t["kind"] for t in package["targets"])


def shipped(meta):
    """(third-party packages in the binaries, the workspace's own, edges).

    Edges map a package id to the ids of its normal dependencies that ship.
    """
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    packages = {p["id"]: p for p in meta["packages"]}
    members = set(meta["workspace_members"])
    roots = [i for i in members if packages[i]["name"] in ROOTS]
    if len(roots) != len(ROOTS):
        die(f"the workspace does not have all of {', '.join(ROOTS)}")
    seen, todo, edges = set(), list(roots), {}
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        deps = [
            d["pkg"]
            for d in nodes[pid]["deps"]
            if any(k["kind"] is None for k in d["dep_kinds"]) and not is_proc_macro(packages[d["pkg"]])
        ]
        edges[pid] = sorted(deps)
        todo.extend(deps)
    own = [packages[i] for i in seen if i in members]
    third = [packages[i] for i in seen if i not in members]
    key = lambda p: (p["name"].lower(), p["version"])
    return sorted(third, key=key), sorted(own, key=key), edges


# --- licence expressions -----------------------------------------------------


def tokens(expression):
    # "MIT/Apache-2.0" is the old spelling of "MIT OR Apache-2.0".
    return re.findall(r"\(|\)|[^\s()]+", expression.replace("/", " OR "))


def parse_expression(expression):
    """An SPDX expression as nested tuples: ("id", name), ("and", [..]), ("or", [..])."""
    toks = tokens(expression)
    pos = 0

    def peek():
        return toks[pos] if pos < len(toks) else None

    def take():
        nonlocal pos
        pos += 1
        return toks[pos - 1]

    def atom():
        if peek() is None:
            raise ValueError("ends early")
        t = take()
        if t == "(":
            inner = either()
            if peek() != ")":
                raise ValueError("unbalanced brackets")
            take()
            return inner
        if t in (")", "AND", "OR", "WITH"):
            raise ValueError(f"unexpected {t}")
        if peek() == "WITH":
            take()
            t = f"{t} WITH {take()}"
        return ("id", t)

    def both():
        parts = [atom()]
        while peek() == "AND":
            take()
            parts.append(atom())
        return parts[0] if len(parts) == 1 else ("and", parts)

    def either():
        parts = [both()]
        while peek() == "OR":
            take()
            parts.append(both())
        return parts[0] if len(parts) == 1 else ("or", parts)

    tree = either()
    if peek() is not None:
        raise ValueError(f"unexpected {peek()}")
    return tree


def choose(tree):
    """The licences Glossa uses the package under: a list of ids from
    ALLOWED that satisfies the expression, or None when none does."""
    kind, value = tree
    if kind == "id":
        # "Apache-2.0+" and the like: the named version is one of the choices.
        name = value[:-1] if value.endswith("+") else value
        return [name] if name in ALLOWED else None
    if kind == "and":
        chosen = []
        for part in value:
            c = choose(part)
            if c is None:
                return None
            chosen += c
        return chosen
    options = [c for c in map(choose, value) if c is not None]
    if not options:
        return None
    rank = lambda c: min((PREFERRED.index(i) if i in PREFERRED else len(PREFERRED)) for i in c) if len(c) == 1 else 99
    return min(options, key=rank)


def licences_of(package):
    """(chosen licence ids or None, problem or None) for a package."""
    expression = package.get("license")
    if not expression:
        if package.get("license_file"):
            return None, f"states its licence only as a file ({package['license_file']}); read it and add an entry"
        return None, "states no licence"
    try:
        chosen = choose(parse_expression(expression))
    except (ValueError, IndexError) as e:
        return None, f"licence expression {expression!r} cannot be read ({e})"
    if chosen is None:
        return None, f"licence {expression!r} is not on the allow-list"
    return chosen, None


# --- licence texts -----------------------------------------------------------


def crate_dir(package):
    return os.path.dirname(package["manifest_path"])


def usable(name):
    return not NOT_A_TEXT.search(name) and not NOT_TAKEN.search(name)


def own_files(package):
    """[(name shown, path)] of the licence and notice files a crate ships
    for itself: those at its top, and the one its manifest names."""
    folder = crate_dir(package)
    if not os.path.isdir(folder):
        die(f"{folder} is missing; run `cargo fetch --locked` first")
    found = {}
    for name in sorted(os.listdir(folder)):
        if NOTICE_NAME.match(name) and usable(name) and os.path.isfile(os.path.join(folder, name)):
            found[name] = os.path.join(folder, name)
    if package.get("license_file"):
        path = os.path.normpath(os.path.join(folder, package["license_file"]))
        if os.path.isfile(path):
            found.setdefault(os.path.relpath(path, folder).replace(os.sep, "/"), path)
    return sorted(found.items())


def nested_files(package):
    """[(name shown, path)] of the licence files deeper in a crate: those of
    the code and data it carries from others. They come on top of the
    crate's own licence, never instead of it."""
    folder = crate_dir(package)
    found = {}
    for rel in EXTRA_FILES.get(package["name"], ()):
        path = os.path.join(folder, rel)
        if not os.path.isfile(path):
            die(f"{package['name']} no longer has {rel}; update EXTRA_FILES")
        found[rel] = path
    for base, dirs, files in os.walk(folder):
        depth = os.path.relpath(base, folder).count(os.sep) + 1 if base != folder else 0
        dirs[:] = sorted(d for d in dirs if d.lower() not in NESTED_SKIP) if depth < NESTED_DEPTH else []
        if base == folder:
            continue
        for name in sorted(files):
            if NESTED_NAME.match(name) and usable(name):
                path = os.path.join(base, name)
                rel = os.path.relpath(path, folder).replace(os.sep, "/")
                if not rel.startswith(NESTED_IGNORE.get(package["name"], ())):
                    found.setdefault(rel, path)
    return sorted(found.items())


def read_text(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = [line.rstrip() for line in f.read().replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    return "\n".join(lines).strip("\n") + "\n"


def repository_key(package):
    """The repository a crate comes from, without the path inside it some
    manifests add (github.com/emilk/egui/tree/main/crates/eframe)."""
    url = (package.get("repository") or "").strip().lower().rstrip("/")
    url = url[:-4] if url.endswith(".git") else url
    hosted = re.match(r"https?://(github\.com|gitlab\.com|codeberg\.org)/[^/]+/[^/]+", url)
    return hosted.group(0) if hosted else url


def authors_of(package):
    return ", ".join(re.sub(r"\s*<[^>]*>", "", a).strip() for a in package.get("authors") or [])


class Texts:
    """Distinct texts, numbered in the order they are first used."""

    def __init__(self):
        self.order = []
        self.by_hash = {}

    def add(self, text, user, name):
        # The same licence differs between crates in line breaks and blank
        # lines only; one copy is enough.
        digest = hashlib.sha256(" ".join(text.split()).encode()).hexdigest()
        if digest not in self.by_hash:
            self.by_hash[digest] = {"number": len(self.order) + 1, "text": text, "users": [], "names": []}
            self.order.append(digest)
        entry = self.by_hash[digest]
        if user not in entry["users"]:
            entry["users"].append(user)
        if name not in entry["names"]:
            entry["names"].append(name)
        return entry["number"]

    def __iter__(self):
        return (self.by_hash[d] for d in self.order)


def collect(third):
    """Per package: its chosen licences, where its texts come from, and any
    problem. Returns (rows, texts)."""
    texts = Texts()
    rows = []
    # A crate of a repository that keeps one licence for all its crates but
    # packs the file into only some of them: (repository, expression) ->
    # the first crate that has the files.
    with_files = {}
    for p in third:
        files = own_files(p)
        if files and repository_key(p):
            with_files.setdefault((repository_key(p), p.get("license")), (p, files))

    def sibling_of(p):
        """A crate of the same repository under the same licence, or under
        the part of this one's licence that covers its code ("(MIT OR
        Apache-2.0) AND OFL-1.1": the fonts' texts are in the crate)."""
        exact = with_files.get((repository_key(p), p.get("license")))
        if exact:
            return exact
        for (repository, expression), found in with_files.items():
            if repository == repository_key(p) and expression and f"({expression})" in (p.get("license") or ""):
                return found
        return None

    for p in third:
        label = f"{p['name']} {p['version']}"
        chosen, problem = licences_of(p)
        row = {"package": p, "label": label, "chosen": chosen, "problem": problem, "refs": [], "origin": "own"}
        files = own_files(p)
        upstream = REPOSITORY_TEXTS.get(repository_key(p), {}).get(p.get("license"))
        if not files:
            sibling = sibling_of(p)
            if sibling:
                files = sibling[1]
                row["origin"] = f"from {sibling[0]['name']} {sibling[0]['version']}, the same repository"
            elif upstream:
                files = [(name, os.path.join(ROOT, "scripts", "licenses", name)) for name in upstream]
                row["origin"] = "from its repository, kept in Glossa's scripts/licenses"
        for name, path in files:
            row["refs"].append((texts.add(read_text(path), label, os.path.basename(name)), name))
        if not files and chosen:
            row["origin"] = "standard text"
            for licence in dict.fromkeys(chosen):
                standard = STANDARD_TEXTS.get(licence)
                if not standard:
                    row["problem"] = row["problem"] or f"ships no licence file and scripts/licenses/ has no {licence} text"
                    continue
                text = read_text(os.path.join(ROOT, "scripts", "licenses", standard))
                if "@HOLDERS@" in text:
                    # A licence whose text names the holder, from a crate
                    # that published no notice: its authors are the best
                    # there is, and the summary says so for a person to check.
                    if not authors_of(p):
                        row["problem"] = row["problem"] or f"ships no licence file and names no authors for the {licence} text"
                        continue
                    text = text.replace("@HOLDERS@", authors_of(p))
                    row["origin"] = "standard text, authors from the manifest"
                row["refs"].append((texts.add(text, label, standard), f"{licence} (standard text)"))
        for name, path in nested_files(p):
            row["refs"].append((texts.add(read_text(path), label, os.path.basename(name)), name))
        rows.append(row)
    return rows, texts


# --- output ------------------------------------------------------------------


def workspace_version(own):
    return next(p["version"] for p in own if p["name"] == "glossa-cli")


def components_for(target, names):
    return [c for c in COMPONENTS if c["crate"] in names and c.get("targets", "") in target]


def write_notices(out, target, third, own, rows, texts):
    w = out.write
    version = workspace_version(own)
    names = {p["name"] for p in third} | {p["name"] for p in own}
    title = f"Third-party software in Glossa {version} ({target})"
    w(f"{title}\n{'=' * len(title)}\n\n")
    w(
        "Glossa itself is Copyright (c) bropines and Glossa contributors, under the MIT\n"
        "License or the Apache License 2.0 at your option (LICENSE-MIT, LICENSE-APACHE,\n"
        "NOTICE). This file lists the software by others that the programs glossa and\n"
        "glossa-cli built for this system contain, with the licence each is used under\n"
        "and the licence texts and notices its authors ship with it.\n\n"
        "It is generated from the build's dependency list by\n"
        "scripts/third-party-notices.py in Glossa's source,\n"
        "https://github.com/bropines/glossa. Where a licence offers a choice, \"used\n"
        "under\" names the one Glossa takes. Crates under the Mozilla Public License 2.0\n"
        "are used unmodified; the list gives the address of their source.\n\n"
    )
    components = components_for(target, names)
    extra = {
        c["name"]: [texts.add(read_text(os.path.join(ROOT, rel)), c["name"], os.path.basename(rel)) for rel in c.get("files", ())]
        for c in components
    }
    w("Contents\n--------\n\n")
    w("  1. Components that are not Rust crates\n")
    w(f"  2. Rust crates ({len(rows)})\n")
    w(f"  3. Licence texts and notices ({len(texts.order)})\n\n\n")

    w("1. Components that are not Rust crates\n--------------------------------------\n\n")
    for c in components:
        w(f"{c['name']}\n")
        for i, line in enumerate(wrap(c["licence"], 67)):
            w(f"  {'Licence:' if i == 0 else '        '} {line}\n")
        w(f"  Source:  {c['source']}\n")
        for line in wrap(c["note"], 76):
            w(f"  {line}\n")
        if extra[c["name"]]:
            w(f"  Texts:   {', '.join(f'[{n}]' for n in extra[c['name']])}\n")
        w("\n")
    w("The application icon and the pictures in the programs are Glossa's own.\n\n\n")

    w("2. Rust crates\n--------------\n\n")
    w("The numbers in brackets point at the texts in section 3.\n\n")
    for row in rows:
        p = row["package"]
        w(f"{row['label']}\n")
        w(f"  Licence:    {p.get('license') or 'see its texts'}\n")
        if row["chosen"] and tokens(p.get("license") or "") != row["chosen"]:
            w(f"  Used under: {' AND '.join(dict.fromkeys(row['chosen']))}\n")
        source = p.get("repository") or p.get("homepage") or f"https://crates.io/crates/{p['name']}"
        w(f"  Source:     {source}\n")
        if authors_of(p):
            for i, line in enumerate(wrap(authors_of(p), 64)):
                w(f"  {'Authors:   ' if i == 0 else '           '} {line}\n")
        if row["refs"]:
            listed = ", ".join(f"[{n}] {name}" for n, name in row["refs"])
            for i, line in enumerate(wrap(listed, 64)):
                w(f"  {'Texts:     ' if i == 0 else '           '} {line}\n")
            if row["origin"] == "standard text":
                w("              (the crate ships no licence file; this is the licence's standard text)\n")
            elif row["origin"].startswith("standard text"):
                w("              (the crate ships no licence file or copyright notice; this is the\n")
                w("              licence's standard text with the authors its manifest names)\n")
            elif row["origin"] != "own":
                w(f"              (the crate ships no licence file of its own; the first is\n")
                w(f"              {row['origin']})\n")
        else:
            w("  Texts:      none found\n")
        w("\n")

    w("\n3. Licence texts and notices\n----------------------------\n")
    for entry in texts:
        w("\n" + "=" * 78 + "\n")
        w(f"[{entry['number']}] {', '.join(entry['names'])}\n")
        for i, line in enumerate(wrap(", ".join(entry["users"]), 70)):
            w(f"{'Applies to:' if i == 0 else '           '} {line}\n")
        w("=" * 78 + "\n\n")
        w(entry["text"])


def wrap(text, width):
    lines, line = [], ""
    for word in text.split():
        if line and len(line) + 1 + len(word) > width:
            lines.append(line)
            line = word
        else:
            line = f"{line} {word}" if line else word
    return lines + [line] if line else lines


def purl(package):
    base = f"pkg:cargo/{package['name']}@{package['version']}"
    source = package.get("source") or ""
    if source.startswith("git+"):
        return f"{base}?vcs_url={source.split('#')[0]}"
    if not source:
        return f"{base}?source=local"
    return base


def lock_checksums():
    with open(os.path.join(ROOT, "Cargo.lock"), "rb") as f:
        lock = tomllib.load(f)
    return {(p["name"], p["version"]): p["checksum"] for p in lock["package"] if "checksum" in p}


def write_sbom(path, target, third, own, edges, rows):
    """CycloneDX 1.5 JSON: every package in the two programs for one target,
    with its licence, its crates.io checksum and what it depends on. No
    timestamp and a serial number derived from the contents, so the same
    source gives the same file."""
    checksums = lock_checksums()
    chosen = {r["package"]["id"]: r for r in rows}
    version = workspace_version(own)
    by_id = {p["id"]: p for p in third + own}

    def component(p, kind):
        c = {"type": kind, "bom-ref": purl(p), "name": p["name"], "version": p["version"], "purl": purl(p)}
        if p.get("description"):
            c["description"] = " ".join(p["description"].split())
        if p.get("license"):
            c["licenses"] = [{"expression": p["license"].replace("/", " OR ")}]
        checksum = checksums.get((p["name"], p["version"]))
        if checksum and (p.get("source") or "").startswith("registry+"):
            c["hashes"] = [{"alg": "SHA-256", "content": checksum}]
        if p.get("repository"):
            c["externalReferences"] = [{"type": "vcs", "url": p["repository"]}]
        row = chosen.get(p["id"])
        if row and row["chosen"]:
            c["properties"] = [{"name": "glossa:used-under", "value": " AND ".join(dict.fromkeys(row["chosen"]))}]
        return c

    root_ref = f"pkg:github/bropines/glossa@v{version}"
    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "tools": [{"name": "scripts/third-party-notices.py", "vendor": "Glossa"}],
            "component": {
                "type": "application",
                "bom-ref": root_ref,
                "name": "glossa",
                "version": version,
                "purl": root_ref,
                "licenses": [{"expression": "MIT OR Apache-2.0"}],
                "properties": [{"name": "glossa:target", "value": target}],
            },
        },
        "components": [component(p, "library") for p in own] + [component(p, "library") for p in third],
        "dependencies": [{"ref": root_ref, "dependsOn": sorted(purl(p) for p in own if p["name"] in ROOTS)}]
        + [{"ref": purl(by_id[i]), "dependsOn": sorted(purl(by_id[d]) for d in deps)} for i, deps in sorted(edges.items())],
    }
    body = json.dumps(bom, indent=1, sort_keys=True)
    bom["serialNumber"] = f"urn:uuid:{uuid.uuid5(uuid.NAMESPACE_URL, hashlib.sha256(body.encode()).hexdigest())}"
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(bom, indent=1, sort_keys=True) + "\n")


def summary(target, third, rows, texts):
    counts = {}
    for row in rows:
        key = row["package"].get("license") or "(none stated)"
        counts[key] = counts.get(key, 0) + 1
    used = {}
    for row in rows:
        for licence in dict.fromkeys(row["chosen"] or ["(not allowed)"]):
            used[licence] = used.get(licence, 0) + 1
    print(f"{target}: {len(third)} third-party packages, {len(texts.order)} distinct texts")
    print("  licence expressions:")
    for key, n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0])):
        print(f"    {n:4}  {key}")
    print("  used under:")
    for key, n in sorted(used.items(), key=lambda kv: (-kv[1], kv[0])):
        print(f"    {n:4}  {key}")
    names = [r["label"] for r in rows if r["origin"] == "standard text"]
    print(f"  no licence file of their own, given the standard text ({len(names)}): {', '.join(names) or '-'}")
    names = [r["label"] for r in rows if r["origin"] == "standard text, authors from the manifest"]
    print(f"  no licence file or notice, standard text with the manifest's authors ({len(names)}): {', '.join(names) or '-'}")
    borrowed = [r["label"] for r in rows if r["origin"].endswith("the same repository")]
    print(f"  no licence file of their own, texts from a crate of the same repository ({len(borrowed)}): {', '.join(borrowed) or '-'}")
    kept = [r["label"] for r in rows if r["origin"].startswith("from its repository")]
    print(f"  no licence file of their own, texts copied from their repository into scripts/licenses ({len(kept)}): {', '.join(kept) or '-'}")
    missing = [r["label"] for r in rows if not r["refs"]]
    print(f"  no licence text at all ({len(missing)}): {', '.join(missing) or '-'}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--target", action="append", help="a Rust target triple (default: the host's; with --check, every release target)")
    ap.add_argument("-o", "--output", help="write the notices to FILE instead of standard output")
    ap.add_argument("--check", action="store_true", help="check the licences against the allow-list and write nothing")
    ap.add_argument("--sbom", metavar="FILE", help="also write a CycloneDX SBOM to FILE")
    ap.add_argument("--summary", action="store_true", help="print counts instead of the notices")
    args = ap.parse_args()
    # UTF-8 and Unix line ends everywhere: Windows' console encoding cannot
    # hold every name in the licence files.
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")

    if args.check:
        failures = 0
        for target in args.target or RELEASE_TARGETS:
            third, _own, _edges = shipped(metadata(target))
            rows, _texts = collect(third)
            bad = [r for r in rows if r["problem"] or not r["refs"]]
            for r in bad:
                print(f"{target}: {r['label']}: {r['problem'] or 'no licence text found'}")
            failures += len(bad)
            print(f"{target}: {len(third)} packages, {len(bad)} problems")
        if failures:
            die("licences need a decision: see ALLOWED and STANDARD_TEXTS in this script")
        return

    targets = args.target or [host_target()]
    if len(targets) != 1:
        die("one --target at a time, except with --check")
    target = targets[0]
    third, own, edges = shipped(metadata(target))
    rows, texts = collect(third)
    if args.summary:
        summary(target, third, rows, texts)
    else:
        problems = [f"{r['label']}: {r['problem'] or 'no licence text found'}" for r in rows if r["problem"] or not r["refs"]]
        if problems:
            die("cannot write complete notices:\n  " + "\n  ".join(problems))
        if args.output:
            with open(args.output, "w", encoding="utf-8", newline="\n") as f:
                write_notices(f, target, third, own, rows, texts)
        else:
            write_notices(sys.stdout, target, third, own, rows, texts)
    if args.sbom:
        write_sbom(args.sbom, target, third, own, edges, rows)


if __name__ == "__main__":
    main()
