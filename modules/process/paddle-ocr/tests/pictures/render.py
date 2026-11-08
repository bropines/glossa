# Draws mixed.png, the fixture picture: English, Russian and Japanese lines
# in two sizes, with macOS system fonts. Needs Pillow.
#   python3 render.py mixed.png
import sys
from PIL import Image, ImageDraw, ImageFont

out = sys.argv[1]
lines = [
    ("/System/Library/Fonts/Helvetica.ttc", 28, "The quick brown fox jumps over the lazy dog."),
    ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 22, "Settings  Open file  Save as PDF  Help"),
    ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 28, "Съешь же ещё этих мягких французских булок."),
    ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 20, "Настройки   Открыть файл   Сохранить"),
    ("/System/Library/Fonts/Hiragino Sans GB.ttc", 28, "今日はいい天気ですね。日本語の文章を読みます。"),
    ("/System/Library/Fonts/Hiragino Sans GB.ttc", 20, "設定  ファイルを開く  保存"),
]
img = Image.new("RGB", (760, 330), "white")
d = ImageDraw.Draw(img)
y = 18
for path, size, text in lines:
    d.text((20, y), text, font=ImageFont.truetype(path, size), fill=(20, 20, 20))
    y += size + 26
img.save(out)
print(out, img.size)
