"""Internationalised domain names to punycode and back, with the idna package.

The package comes from `requirements` in module.toml: Glossa installs it into
this module's own environment before the first call.
"""

import idna

from glossa_module import Module, ModuleError

mod = Module()


@mod.translate
def translate(req, host):
    text = req["text"].strip()
    try:
        if text.startswith("xn--") or ".xn--" in text:
            return {"text": idna.decode(text)}
        return {"text": idna.encode(text).decode("ascii")}
    except idna.IDNAError as e:
        raise ModuleError("other", "not a valid domain name: %s" % e)


mod.run()
