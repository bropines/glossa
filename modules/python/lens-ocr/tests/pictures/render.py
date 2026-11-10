"""Draws hello.png, the picture the live case reads.

Run it with any Python that has Pillow 10.1 or newer (this module's own
environment has it):

    python render.py

Pillow's built-in font is used, so the picture is the same everywhere.
"""

import pathlib

from PIL import Image, ImageDraw, ImageFont

image = Image.new("L", (480, 120), 255)
draw = ImageDraw.Draw(image)
draw.text((24, 28), "Hello world", fill=0, font=ImageFont.load_default(size=56))
image.save(pathlib.Path(__file__).with_name("hello.png"), optimize=True)
