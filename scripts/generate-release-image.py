#!/usr/bin/env python3
import importlib.util, os
from PIL import Image, ImageDraw

spec = importlib.util.spec_from_file_location("g", "/home/glorg/satur8/scripts/generate-readme-assets.py")
g = importlib.util.module_from_spec(spec); spec.loader.exec_module(g)

W, H = 1600, 900
# Background: ink base + the site color mesh only (no grid).
img = Image.new("RGBA", (W, H), g.INK + (255,))
for center, rad, color, alpha in [
    ((int(W * 0.78), int(-H * 0.06)), (int(W * 0.33), int(W * 0.33)), g.VIOLET, 34),
    ((int(W * 0.06), int(H * 0.08)), (int(W * 0.29), int(W * 0.29)), g.MAGENTA, 24),
    ((int(W * 0.50), int(H * 1.16)), (int(W * 0.37), int(W * 0.37)), g.CYAN, 32),
]:
    img.alpha_composite(g.radial_glow((W, H), center, rad, color, alpha))
d = ImageDraw.Draw(img, "RGBA")


def grad_text(text, font, cx, cy):
    l, t, r, b = (int(v) for v in d.textbbox((cx, cy), text, font=font, anchor="mm"))
    w, h = max(1, r - l), max(1, b - t)
    grad = g.gradient_fill(w, h, g.SPECTRUM, g.SPECTRUM_ANGLE).convert("RGBA")
    mask = Image.new("L", (w, h), 0)
    ImageDraw.Draw(mask).text((cx - l, cy - t), text, font=font, anchor="mm", fill=255)
    grad.putalpha(mask)
    img.alpha_composite(grad, (l, t))


def center_text(text, font, cx, cy, fill):
    d.text((cx, cy), text, font=font, anchor="mm", fill=fill)


cx = W // 2

# Brand wordmark: "Satur" white + spectrum "8", centered.
wf = g.F_CLASH(78)
ww = d.textlength("Satur8", font=wf)
g.draw_wordmark(img, int(cx - ww / 2), 150, 78)
d2 = ImageDraw.Draw(img, "RGBA")

# Hero line.
grad_text("v0.3 is out", g.F_CLASH(170), cx, 430)

# SteamOS + Bazzite soon, plain text underneath.
center_text("SteamOS + Bazzite soon", g.F_MED(44), cx, 575, g.CYAN_2)

out = "/home/glorg/Downloads/satur8-v0.3-release.png"
img.convert("RGB").save(out)
print("saved", out, img.size)
