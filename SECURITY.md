# Security

## Reporting a problem

Please report security problems privately, through
[GitHub's private vulnerability reporting](https://github.com/bropines/glossa/security/advisories/new),
not in a public issue. Say what an attacker can do, under which conditions,
and how to reproduce it; a proof of concept helps. You will get an answer
within a week, and credit in the release notes unless you prefer otherwise.

<!-- Maintainer: the link above works only while "Private vulnerability
reporting" is switched on in the repository's Settings → Code security.
It was off when this page was last revised; switch it on, then delete this
comment. There is no security e-mail address: do not add one that is not
read. -->

If that page does not let you open a report, private reporting is not
switched on for the repository at the moment. Open an ordinary
[issue](https://github.com/bropines/glossa/issues/new/choose) that says
only that you have a security report, with no details of the problem, so
that the maintainer can open a private channel.

## Supported versions

Glossa is in beta. Fixes go into the next release only; there are no
maintained older branches and no backports.

| version | security fixes |
|---|---|
| the latest release (currently the `0.1.0-beta` series) | yes, in the next release |
| anything older | no: upgrade to the latest |

The module programs released separately (`paddle-ocr-v*`, `local-llm-v*`)
are covered the same way: the latest release of each.

The downloads are not signed with a publisher certificate yet (Windows)
and are signed ad hoc, not notarised (macOS). Check a download against the
release's `SHA256SUMS` if you need to know it is the file that was built.

## What Glossa protects, and what it does not

Some things are deliberate limits rather than bugs. Knowing them helps
decide what to report. [Privacy and data](docs/privacy.md) says what is
sent where and what is stored; [Deployment](docs/deployment.md) lists the
settings that narrow all of this.

**Modules run only after you approve them, and the approval covers the
manifest, not the code.** A module installed by hand (a folder or a
`.potext` in your modules folder or one you added) is listed but does not
run, download or install anything until you approve what its `module.toml`
asks for: the hosts, whether it runs a program or a native library, the
files it downloads, the Python packages, its actions and panels. The
approval is kept as `[modules.granted.<id>]` in `config.toml`, tied to the
module's id and folder. An update that asks for more waits again; one that
asks for the same or less keeps running. Not asked about: the modules
compiled into Glossa, the folders an installer writes, the `Service.js`
scripts of a classic QTranslate installation you pointed Glossa at, and
modules you create in the module editor
([details](docs/modules/README.md#permissions-and-trust)).

What this does not do:

- The module's scripts, programs and libraries are **not hashed**. An
  update that changes `main.js` or the executable but declares nothing new
  runs without a question. The approval says what a module is allowed to
  ask Glossa for, not that its code is unchanged or harmless.
- Anything that can write `config.toml` can write an approval into it, and
  anything that holds the control API token can approve a module through
  `/v1/trust`. Both already mean running as you.
- On the first run of a version with approvals, modules that were already
  installed are approved for what they declare at that moment, so that an
  upgrade does not stop them.

A module that runs without approval, or that gets more than was approved
through Glossa's own host calls, is a vulnerability.

**Module permissions cover host calls only.** The `[permissions] network`
list in a `module.toml` limits the HTTP requests a module makes *through
Glossa* (`host.fetch`, `host.http`): a request to any other host never
leaves. A Glossa JavaScript module has no other way out (no files, no
programs, a memory and time limit per call), so for it the list is a
fence; reaching a host it did not declare is a bug worth reporting.
Python, Java and process modules are ordinary processes running as the
user and can open their own connections and files; the list does not
restrict them, and the approval prompt says so. Classic QTranslate scripts
and pot-app plugins have no permission list and may reach any host, as
they could in the programs they come from (classic scripts can be switched
off with `[modules] allow_legacy_scripts`).

**Native modules run inside Glossa**, in the same process and with all of
its privileges: no sandbox, no check on what they do themselves, and a crash
in one takes Glossa down. Install native modules only from people you trust,
or switch them off (`[modules] allow_native`).

**Downloads are pinned.** Files a module declares, the managed Python and
`uv` are checked against a SHA-256 from the module's manifest or from
Glossa before they are used, and a placeholder hash is refused. Python
packages are installed by `uv` from PyPI (or the index the module
declares) from wheels only; they are not pinned by hash.

**The local control API requires its token.** It listens on loopback
(`127.0.0.1:60828`) by default and is switched on by default
(`[server] enabled`). Every request except `GET /health` must carry the
token (`Authorization: Bearer …` or `?token=…`); a token is generated when
none is set and cannot be left empty. Two more checks apply even with the
token: a request carrying a header only browsers send (`Origin`,
`Sec-Fetch-*`, `Referer`) is refused, so a web page cannot reach the API
from the user's browser, and the `Host` header must be an address or
`localhost`, which defeats DNS rebinding.

- One exception to the token: pot-app's compatibility routes are answered
  without it for requests from this machine, while
  `[server] pot_compat_without_token` is on (the default). They can open
  windows and never return data. Set it to `false` to require the token
  there too.
- The token grants everything the API does: translating with your keys,
  typing into the foreground program, quitting Glossa, downloading module
  files, and approving modules (`/v1/trust`). Treat it like a password for
  your Glossa.
- The token is always stored in plain text in `config.toml`, also when
  keys are kept in the system keychain. Anything that can read that file
  can use the API; that is the boundary.
- Binding beyond loopback is refused without a token, and the traffic is
  plain HTTP: the token and the text cross the network unencrypted.
- On Linux, the D-Bus service on the session bus (`[server] dbus`) has no
  token: the session bus is the user's own.

A way to reach the API without the token (other than the pot-app routes as
described), or from a web page, is a vulnerability.

**Secrets in the settings.** By default API keys, the proxy password and
the Lens daemon token live in plain text in `config.toml` in the user's
data folder, like the classic program's `Options.json`.
With `[general] secret_storage = "system"` (*Options → Basics*, or the
welcome guide) they are kept in the system keychain instead: Windows
Credential Manager, the macOS Keychain, or the Secret Service on Linux;
where no keychain answers, they stay in the file. In memory they are held
in the clear either way. Glossa sends a key only to the service it belongs
to; a preset never contains keys, and an imported preset cannot change the
address a key is sent to.

**Text you translate goes to the services you choose.** Online services
receive the text, and a screen region goes to the OCR engine: by default
Google Lens, through Glossa's built-in client (or through a
[chrome-lens-py](https://github.com/bropines/chrome-lens-py) daemon when
one is running on the machine). Offline dictionaries, the system's OCR
engines and voices, and modules that work locally keep it on the machine.
Some fallbacks can send text to a service other than the selected one;
they are listed in
[Privacy and data](docs/privacy.md#requests-you-might-not-expect).

**No machine-wide policy.** Every setting above is in the user's own
`config.toml` and can be changed by that user. There is no administrator
lock yet ([Deployment](docs/deployment.md#no-policy-layer)).

**Elevation.** Glossa runs without administrator rights and never asks for
them. Windows does not let it read from or type into programs running as
administrator; that is the system's protection, not a bug.
