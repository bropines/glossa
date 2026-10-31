# Security

## Reporting a problem

Please report security problems privately, through
[GitHub's private vulnerability reporting](https://github.com/bropines/glossa/security/advisories/new),
not in a public issue. Say what an attacker can do, under which conditions,
and how to reproduce it; a proof of concept helps. You will get an answer
within a week, and credit in the release notes unless you prefer otherwise.

Fixes go into the latest release only; there are no maintained older
branches.

## What Glossa protects, and what it does not

Some things are deliberate limits rather than bugs. Knowing them helps
decide what to report.

**Module permissions cover host calls only.** The `[permissions] network`
list in a `module.toml` limits the HTTP requests a module makes *through
Glossa* (`host.fetch`, `host.http`): a request to any other host never
leaves. A Glossa JavaScript module has no other way out, so for it the list
is a fence; reaching a host it did not declare is a bug worth reporting.
Python modules are ordinary processes running as the user and can open their
own connections and files; the list does not restrict them. Classic
QTranslate scripts and pot-app plugins have no permission list and may reach
any host, as they could in the programs they come from (classic scripts can
be switched off with `allow_legacy_scripts`).

**Native modules run inside Glossa**, in the same process and with all of
its privileges: no sandbox, no check on what they do themselves, and a crash
in one takes Glossa down. Install native modules only from people you trust,
or switch them off (`allow_native`).

**The local control API requires its token.** It listens on loopback
(`127.0.0.1:60828`) by default, and every request except `GET /health` must
carry the token (`Authorization: Bearer …` or `?token=…`). A token is
generated when none is set. Requests carrying a browser `Origin` header are
refused, so a web page cannot reach the API from the user's browser. Binding
beyond loopback is refused without a token. Anything that can read the
user's `config.toml` can read the token; that is the boundary. A way to
reach the API without the token, or from a web page, is a vulnerability.

**Secrets in the settings.** API keys live in plain text in `config.toml`
in the user's data folder, readable by the user's own programs, like the
classic program's `Options.json`. Glossa does not send them anywhere except
to the service they belong to.

**Text you translate goes to the services you choose.** Online services
receive the text, and screen regions go to the OCR engine (Google Lens
through chrome-lens-py by default). Offline dictionaries and modules that
work locally keep it on the machine.
