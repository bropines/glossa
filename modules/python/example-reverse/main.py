"""The smallest useful Glossa module: it writes the text backwards."""

from glossa_module import Module

mod = Module()


@mod.translate
def translate(req, host):
    # req["from"] / req["to"] are BCP 47 codes ("auto" for the source when
    # unknown); req["options"] holds this module's settings.
    return {"text": req["text"][::-1]}


mod.run()
