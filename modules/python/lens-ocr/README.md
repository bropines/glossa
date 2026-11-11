# Google Lens (Python module) (`lens-ocr-python`)

**How it gets its answers: an unofficial endpoint, through a library.**

Text recognition by Google Lens, through the
[chrome-lens-py](https://github.com/bropines/chrome-lens-py) package, which
talks to the endpoint Chrome's "Search with Google Lens" uses. Google does
not document or promise that endpoint for other clients: it can change or
close without notice, and then the module waits for a new chrome-lens-py.

Glossa also has a built-in Lens engine (`lens`) that needs no Python. This
module is the alternative that keeps one warm Python process with the
library loaded, and an example of a Python module with dependencies.

- **Key or account:** none.
- **What leaves your machine:** the picture you capture, to Google
  (`lensfrontend-pa.googleapis.com`), with the language you asked for. The
  connection is chrome-lens-py's own, not Glossa's: Glossa's proxy setting
  does not apply to it (the module has a *Proxy for Lens* setting of its
  own), and the module's network permissions list is empty for the same
  reason. Do not capture what Google should not see.
- **Limits:** none published. Google answers with an error after many
  requests in a short time; the module reports it as a rate limit.
- **Cost:** free.
- **Content:** Google's terms apply to the service.
- **Code:** MIT. chrome-lens-py is MIT as well.

## Setting it up

1. Copy this folder into your modules folder (*Options → Modules → Open
   folder*), or into `<GLOSSA_HOME>/modules/lens-ocr`.
2. *Options → Modules → Reload modules*, and approve the module: it runs a
   program with your rights and installs one package from PyPI.
3. *Options → Services → Google Lens (Python module) → Download*, or
   `glossa-cli python install lens-ocr-python`. Glossa downloads a Python
   of its own when it needs one and installs `chrome-lens-py` (pinned in
   `module.toml`) with its dependencies into an environment that belongs
   to this module alone: about 24 MB, from wheels only. Nothing is
   installed into your system Python, and there is no `pip` step.

It needs a Glossa newer than 0.1.0-beta.2 (the first with Python packages
for modules comes after it).

## Testing

```bash
glossa-cli module test --download modules/python/lens-ocr         # offline: starts, imports, maps a failed connection
glossa-cli module test --download --live modules/python/lens-ocr  # reads tests/pictures/hello.png through Google
```

The library's traffic does not pass through Glossa, so it cannot be
recorded and replayed: recognition is tested live only.
