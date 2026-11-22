#!/usr/bin/env python3
"""Steam library artwork for Satur8's own entry (a non-Steam shortcut).

One idea, from what the app does: the rainbow 8 of the logo, colorless on
the left and vivid on the right, with a slider's handle on the line between
them. Drawn from the brand files the app already ships, the wordmark with the
8 (crates/satur8-gui/ui/word.png) and the app icon
(crates/satur8-gui/ui/icon.png), on the app's dark background. Written to
assets/steam/, where `satur8 steam-shortcut` and the app take them from.

Sizes are Valve's library asset sizes
(https://partner.steamgames.com/doc/store/assets/libraryassets):
  capsule.png  600x900    the portrait cover in the library grid
  header.png   920x430    the wide capsule (recent games, lists)
  hero.png     1920x620   the banner behind the entry's page
  logo.png     1280x255   the name drawn over the banner, transparent
  icon.png     256x256    the small icon

Needs Pillow. Run from the repository root:
  python3 scripts/generate-steam-artwork.py [PREVIEW_DIR]
With PREVIEW_DIR it also writes preview.png there: every image on one sheet.
"""
import os
import sys

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
UI = os.path.join(ROOT, "crates", "satur8-gui", "ui")
OUT = os.path.join(ROOT, "assets", "steam")

# The app's background (Pal.bg, dark) in crates/satur8-gui/ui/theme.slint.
BG = (7, 8, 11, 255)

word = Image.open(os.path.join(UI, "word.png")).convert("RGBA")
icon = Image.open(os.path.join(UI, "icon.png")).convert("RGBA")
# The rainbow 8 alone: the right end of the wordmark, where it is largest.
# It starts at column 1400, right where the "r" ends, and the two overlap by
# a pixel of soft edge, so the light pixels of the "r" are cleared.
eight = word.crop((1399, 0, word.width, word.height))
for x in range(4):
    for y in range(eight.height):
        red, green, blue, alpha = eight.getpixel((x, y))
        if min(red, green, blue) > 200:
            eight.putpixel((x, y), (red, green, blue, 0))


def fit(image, width=None, height=None):
    """Scale to the given width or height, keeping the shape."""
    if width is None:
        width = round(image.width * height / image.height)
    if height is None:
        height = round(image.height * width / image.width)
    return image.resize((width, height), Image.LANCZOS)


def canvas(width, height):
    return Image.new("RGBA", (width, height), BG)


def paste_center(base, image, center_x, center_y):
    base.alpha_composite(image, (round(center_x - image.width / 2), round(center_y - image.height / 2)))


# The app's light ink (Pal.ink, dark) for the handle, as its sliders use.
INK = (243, 246, 251, 255)


def gray(image):
    """The image without color: each pixel at its own lightness."""
    red, green, blue, alpha = image.split()
    light = Image.merge("RGB", (red, green, blue)).convert("L")
    return Image.merge("RGBA", (light, light, light, alpha))


# Where the line sits across the 8: a third of the way, so most of the
# rainbow (orange to blue) stays in color.
SPLIT = 0.34


def split(image):
    """Colorless left of the line, as drawn right of it."""
    out = image.copy()
    line = round(image.width * SPLIT)
    out.paste(gray(image.crop((0, 0, line, image.height))), (0, 0))
    return out


def handle(base, x, top, bottom, radius):
    """A slider's handle on a vertical line at x, the handle in the middle."""
    from PIL import ImageDraw

    draw = ImageDraw.Draw(base)
    line = max(2, radius // 5)
    draw.rectangle((x - line // 2, top, x + line // 2, bottom), fill=INK)
    middle = (top + bottom) // 2
    draw.ellipse((x - radius, middle - radius, x + radius, middle + radius), fill=INK)
    dot = max(2, radius * 2 // 7)
    draw.ellipse((x - dot, middle - dot, x + dot, middle + dot), fill=BG)


def split_eight(base, height, center_x, center_y):
    """The 8, colorless left and vivid right, with the handle between."""
    mark = split(fit(eight, height=height))
    paste_center(base, mark, center_x, center_y)
    overhang = height // 9
    handle(
        base,
        round(center_x - mark.width / 2 + mark.width * SPLIT),
        center_y - height // 2 - overhang,
        center_y + height // 2 + overhang,
        max(8, height // 18),
    )


def capsule():
    image = canvas(600, 900)
    split_eight(image, 520, 300, 380)
    paste_center(image, fit(word, width=300), 300, 800)
    return image


def header():
    # The wordmark, with the same split through its own 8.
    image = canvas(920, 430)
    mark = fit(word, width=640)
    left = round(460 - mark.width / 2)
    top = round(215 - mark.height / 2)
    image.alpha_composite(mark, (left, top))
    scale = mark.width / word.width
    eight_left = left + round(1399 * scale)
    eight_width = mark.width - round(1399 * scale)
    middle = eight_left + round(eight_width * SPLIT)
    part = image.crop((eight_left, top, middle, top + mark.height))
    image.paste(gray(part.convert("RGBA")).convert("RGB").convert("RGBA"), (eight_left, top))
    handle(image, middle, top - 26, top + mark.height + 26, 13)
    return image


def hero():
    # Steam draws the logo over the left of the banner; the 8 sits right.
    image = canvas(1920, 620)
    split_eight(image, 440, 1540, 310)
    return image


def logo():
    return fit(word, width=1280)


IMAGES = {
    "capsule.png": capsule,
    "header.png": header,
    "hero.png": hero,
    "logo.png": logo,
    "icon.png": lambda: icon,
}


def main():
    os.makedirs(OUT, exist_ok=True)
    made = {}
    for name, draw in IMAGES.items():
        image = draw()
        # The banner images have no transparency; store them without alpha.
        if name in ("capsule.png", "header.png", "hero.png"):
            image = image.convert("RGB")
        image.save(os.path.join(OUT, name), optimize=True)
        made[name] = image
        print(f"{name}: {image.width}x{image.height}, {os.path.getsize(os.path.join(OUT, name))} bytes")
    if len(sys.argv) > 1:
        sheet = Image.new("RGB", (1960, 1640), (40, 44, 52))
        sheet.paste(made["hero.png"], (20, 20))
        sheet.paste(made["capsule.png"], (20, 660))
        sheet.paste(made["header.png"], (640, 660))
        logo_preview = Image.new("RGB", (1280, 255), (24, 28, 34))
        logo_preview.paste(made["logo.png"], (0, 0), made["logo.png"])
        sheet.paste(fit(logo_preview, width=920), (640, 1110))
        icon_preview = Image.new("RGB", (256, 256), (24, 28, 34))
        icon_preview.paste(made["icon.png"], (0, 0), made["icon.png"])
        sheet.paste(icon_preview, (640, 1320))
        os.makedirs(sys.argv[1], exist_ok=True)
        sheet.save(os.path.join(sys.argv[1], "preview.png"))


if __name__ == "__main__":
    main()
