#!/usr/bin/env python3
"""Generate the Satur8 README visuals.

Outputs, all written under assets/readme/:
  hero.png            1200x630 project hero with wordmark and a before/after panel
  before-after.png    1200x600 split scene, normal vs Satur8-boosted saturation
  demo.gif            60fps loop showing detect -> apply -> restore
  architecture.svg    focus -> match -> backend -> output -> restore diagram

The scene is synthetic so no copyrighted game art is used. Saturation changes
use Pillow's ImageEnhance.Color, the same kind of per-channel saturation scaling
Satur8 applies through the display backend, so the before/after is honest about
what the tool does.

The wordmark uses Clash Display (bundled in scripts/fonts/, converted from the
website font) with the spectrum-gradient "8" from the site brand, so the README
matches satur8.app. The demo GIF is encoded with ffmpeg. No network access is
required.
"""

import math
import os
import shutil
import subprocess
import tempfile

from PIL import Image, ImageChops, ImageDraw, ImageEnhance, ImageFilter, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "assets", "readme")
FONT_LOCAL = os.path.join(ROOT, "scripts", "fonts")
os.makedirs(OUT, exist_ok=True)

# Satur8 brand palette, taken from the website stylesheet.
INK = (7, 8, 11)
INK_2 = (12, 14, 19)
PANEL = (16, 19, 26)
CYAN = (31, 227, 203)
CYAN_2 = (108, 245, 230)
MAGENTA = (255, 61, 138)
CORAL = (255, 106, 61)
AMBER = (255, 194, 75)
VIOLET = (124, 108, 255)
TEXT = (243, 246, 251)
MUTED = (130, 140, 157)

# The site --spectrum gradient: 100deg, magenta -> coral -> amber -> cyan -> violet.
SPECTRUM = [(0.0, MAGENTA), (0.26, CORAL), (0.48, AMBER), (0.74, CYAN), (1.0, VIOLET)]
SPECTRUM_ANGLE = 100

NOTO = "/usr/share/fonts/noto"


def font(path, size):
    return ImageFont.truetype(path, size)


def F_BLACK(s):
    return font(os.path.join(NOTO, "NotoSans-Black.ttf"), s)


def F_BOLD(s):
    return font(os.path.join(NOTO, "NotoSans-Bold.ttf"), s)


def F_MED(s):
    return font(os.path.join(NOTO, "NotoSans-Medium.ttf"), s)


def F_REG(s):
    return font(os.path.join(NOTO, "NotoSans-Regular.ttf"), s)


def F_MONO_B(s):
    return font(os.path.join(NOTO, "NotoSansMono-Bold.ttf"), s)


def F_CLASH(s):
    """Clash Display Bold (site wordmark font), falling back to Noto Black."""
    p = os.path.join(FONT_LOCAL, "ClashDisplay-Bold.ttf")
    return font(p if os.path.exists(p) else os.path.join(NOTO, "NotoSans-Black.ttf"), s)


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def color_at(stops, t):
    t = max(0.0, min(1.0, t))
    for i in range(len(stops) - 1):
        p0, c0 = stops[i]
        p1, c1 = stops[i + 1]
        if p0 <= t <= p1:
            lt = 0 if p1 == p0 else (t - p0) / (p1 - p0)
            return lerp(c0, c1, lt)
    return stops[-1][1]


def gradient_fill(w, h, stops, angle_deg):
    a = math.radians(angle_deg)
    ux, uy = math.sin(a), -math.cos(a)
    corners = [(0, 0), (w, 0), (0, h), (w, h)]
    projs = [cx * ux + cy * uy for cx, cy in corners]
    pmin, pmax = min(projs), max(projs)
    rng = (pmax - pmin) or 1
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            px[x, y] = color_at(stops, ((x * ux + y * uy) - pmin) / rng)
    return img


def radial_glow(size, center, radius, color, max_alpha):
    """A soft elliptical glow, built small and upscaled for speed."""
    W, H = size
    sw, sh = max(1, W // 4), max(1, H // 4)
    cx, cy = center[0] / 4.0, center[1] / 4.0
    rx, ry = max(1.0, radius[0] / 4.0), max(1.0, radius[1] / 4.0)
    layer = Image.new("RGBA", (sw, sh), (0, 0, 0, 0))
    px = layer.load()
    r, g, b = color
    for y in range(sh):
        for x in range(sw):
            dx, dy = (x - cx) / rx, (y - cy) / ry
            d = math.hypot(dx, dy)
            if d < 1.0:
                a = int(max_alpha * (1.0 - d) ** 1.4)
                if a > 0:
                    px[x, y] = (r, g, b, a)
    return layer.resize((W, H), Image.BICUBIC)


def site_background(W, H):
    """The satur8.app background: ink base, three-color mesh, faint top grid."""
    img = Image.new("RGBA", (W, H), INK + (255,))
    # Mesh glows: violet top-right, magenta top-left, cyan bottom-center.
    for center, rad, color, alpha in [
        ((int(W * 0.78), int(-H * 0.06)), (int(W * 0.33), int(W * 0.33)), VIOLET, 34),
        ((int(W * 0.06), int(H * 0.08)), (int(W * 0.29), int(W * 0.29)), MAGENTA, 24),
        ((int(W * 0.50), int(H * 1.16)), (int(W * 0.37), int(W * 0.37)), CYAN, 32),
    ]:
        img.alpha_composite(radial_glow((W, H), center, rad, color, alpha))

    # Faint line grid, 88px cells, fading out toward the bottom (mask from top).
    grid = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    gd = ImageDraw.Draw(grid)
    step, line = 88, (233, 240, 255, 22)
    for x in range(0, W + 1, step):
        gd.line([(x, 0), (x, H)], fill=line, width=1)
    for y in range(0, H + 1, step):
        gd.line([(0, y), (W, y)], fill=line, width=1)
    fade = Image.new("L", (W, H), 0)
    fp = fade.load()
    span = H * 0.82
    for y in range(H):
        a = max(0, int(255 * (1 - y / span)))
        for x in range(W):
            fp[x, y] = a
    grid.putalpha(ImageChops.multiply(grid.getchannel("A"), fade))
    img.alpha_composite(grid)
    return img


def vgrad(w, h, top, bottom):
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        c = lerp(top, bottom, y / max(1, h - 1))
        for x in range(w):
            px[x, y] = c
    return img


def draw_wordmark(img, x, y, size):
    """Draw 'Satur' in white and a spectrum-gradient '8', matching the site."""
    clash = F_CLASH(size)
    d = ImageDraw.Draw(img)
    d.text((x, y), "Satur", font=clash, fill=TEXT)
    x8 = x + d.textlength("Satur", font=clash)
    l, t, r, b = (int(v) for v in d.textbbox((x8, y), "8", font=clash))
    gw, gh = max(1, r - l), max(1, b - t)
    grad = gradient_fill(gw, gh, SPECTRUM, SPECTRUM_ANGLE).convert("RGBA")
    mask = Image.new("L", (gw, gh), 0)
    ImageDraw.Draw(mask).text((x8 - l, y - t), "8", font=clash, fill=255)
    grad.putalpha(mask)
    img.alpha_composite(grad, (int(l), int(t)))
    return x8 + (r - x8)  # right edge of the wordmark


def base_scene(w, h):
    """A synthwave vista: gradient sky, banded sun, ridgelines, perspective grid.

    Rendered with strong hues so a saturation change is obvious.
    """
    img = vgrad(w, h, (26, 18, 48), (58, 30, 70))
    horizon = int(h * 0.62)

    warm = vgrad(w, horizon, (58, 30, 70), (255, 120, 90))
    img.paste(warm, (0, 0))

    # Sun: vertical gradient disc with scanline gaps in the lower half.
    sun_r = int(h * 0.30)
    cx, cy = int(w * 0.5), int(horizon - sun_r * 0.18)
    sun = Image.new("RGBA", (sun_r * 2, sun_r * 2), (0, 0, 0, 0))
    sd = ImageDraw.Draw(sun)
    for yy in range(sun_r * 2):
        col = lerp(AMBER, MAGENTA, yy / (sun_r * 2 - 1))
        sd.line([(0, yy), (sun_r * 2, yy)], fill=col + (255,))
    mask = Image.new("L", (sun_r * 2, sun_r * 2), 0)
    ImageDraw.Draw(mask).ellipse([0, 0, sun_r * 2 - 1, sun_r * 2 - 1], fill=255)
    md = ImageDraw.Draw(mask)
    gap = max(4, sun_r // 9)
    for i in range(6):
        y0 = int(sun_r * 0.95) + i * gap
        md.rectangle([0, y0, sun_r * 2, y0 + max(2, gap // 2)], fill=0)
    img.paste(sun, (cx - sun_r, cy - sun_r), mask)

    glow = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse(
        [cx - sun_r * 2, cy - sun_r * 2, cx + sun_r * 2, cy + sun_r * 2],
        fill=MAGENTA + (60,),
    )
    glow = glow.filter(ImageFilter.GaussianBlur(sun_r * 0.45))
    img = Image.alpha_composite(img.convert("RGBA"), glow).convert("RGB")
    d = ImageDraw.Draw(img, "RGBA")

    def ridge(base_y, amp, step, color):
        pts = [(0, h)]
        x = 0
        while x <= w:
            yy = base_y - int(amp * (0.5 + 0.5 * math.sin(x / step + base_y)))
            pts.append((x, yy))
            x += step // 3
        # Anchor the right edge so the silhouette has no notch.
        yy = base_y - int(amp * (0.5 + 0.5 * math.sin(w / step + base_y)))
        pts.append((w, yy))
        pts.append((w, h))
        d.polygon(pts, fill=color)

    ridge(horizon, int(h * 0.05), 70, lerp(VIOLET, INK, 0.45) + (255,))
    ridge(int(horizon + h * 0.03), int(h * 0.07), 95, lerp(VIOLET, INK, 0.65) + (255,))

    # Foreground perspective grid below the horizon.
    grid_top = horizon
    grid = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    gd = ImageDraw.Draw(grid)
    vp = (w // 2, grid_top)
    for gx in range(-12, 13):
        bx = w // 2 + gx * (w // 8)
        gd.line([vp, (bx, h)], fill=CYAN + (90,), width=2)
    rows = 16
    for r in range(1, rows + 1):
        t = r / rows
        yy = grid_top + int((h - grid_top) * (t * t))
        gd.line([(0, yy), (w, yy)], fill=CYAN + (110,), width=2)
    fmask = Image.new("L", (w, h - grid_top))
    fp = fmask.load()
    for y in range(h - grid_top):
        a = int(255 * (1 - y / (h - grid_top)))
        for x in range(w):
            fp[x, y] = a
    grid.paste((0, 0, 0, 0), (0, grid_top), fmask)
    img = Image.alpha_composite(img.convert("RGBA"), grid).convert("RGB")

    return img


def satur(img, factor):
    return ImageEnhance.Color(img).enhance(factor)


def rounded(img, radius):
    mask = Image.new("L", img.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, img.size[0] - 1, img.size[1] - 1], radius=radius, fill=255
    )
    out = Image.new("RGBA", img.size, (0, 0, 0, 0))
    out.paste(img, (0, 0), mask)
    return out


def slider(draw, x, y, w, t, label, accent=CYAN):
    """A saturation slider track, filled portion, knob, and value label."""
    h = 6
    cy = y + h // 2
    draw.rounded_rectangle([x, y, x + w, y + h], radius=h // 2, fill=(40, 46, 58))
    fillw = int(w * t)
    draw.rounded_rectangle([x, y, x + fillw, y + h], radius=h // 2, fill=accent)
    knob, kx = 11, x + fillw
    draw.ellipse([kx - knob, cy - knob, kx + knob, cy + knob], fill=TEXT)
    draw.ellipse([kx - 4, cy - 4, kx + 4, cy + 4], fill=accent)
    if label:
        draw.text((x + w + 18, cy), label, font=F_MONO_B(18), fill=accent, anchor="lm")


def pill(draw, x, y, h, text, fnt, dot=CYAN, fg=TEXT):
    """A rounded chip with a leading dot and vertically centered text."""
    cy = y + h // 2
    tw = draw.textlength(text, font=fnt)
    dot_r = 4
    pad_l, gap, pad_r = 18, 12, 18
    box_w = pad_l + dot_r * 2 + gap + tw + pad_r
    draw.rounded_rectangle(
        [x, y, x + box_w, y + h], radius=h // 2,
        outline=(60, 68, 82), width=1, fill=(16, 19, 26, 220),
    )
    dcx = x + pad_l + dot_r
    draw.ellipse([dcx - dot_r, cy - dot_r, dcx + dot_r, cy + dot_r], fill=dot)
    draw.text((dcx + dot_r + gap, cy), text, font=fnt, fill=fg, anchor="lm")
    return box_w


def label_box(draw, text, cx_or_edge, cy, anchor, fnt, fg=TEXT):
    """A dark rounded label with the text centered both ways."""
    tw = draw.textlength(text, font=fnt)
    asc, desc = fnt.getmetrics()
    th = asc + desc
    pad_x, pad_y = 16, 9
    box_w, box_h = tw + pad_x * 2, th + pad_y * 2
    if anchor == "l":
        bx = cx_or_edge
    else:  # right edge
        bx = cx_or_edge - box_w
    by = cy - box_h // 2
    draw.rounded_rectangle([bx, by, bx + box_w, by + box_h], radius=10, fill=(7, 8, 11, 210))
    draw.text((bx + box_w // 2, by + box_h // 2), text, font=fnt, fill=fg, anchor="mm")


# ---------------------------------------------------------------------------
# before-after.png
# ---------------------------------------------------------------------------
def make_before_after():
    W, H = 1200, 600
    scene = base_scene(W, H)
    left = satur(scene, 1.0)
    right = satur(scene, 1.75)
    img = Image.new("RGB", (W, H), INK)
    img.paste(left.crop((0, 0, W // 2, H)), (0, 0))
    img.paste(right.crop((W // 2, 0, W, H)), (W // 2, 0))

    d = ImageDraw.Draw(img, "RGBA")
    d.rectangle([W // 2 - 1, 0, W // 2 + 1, H], fill=CYAN + (255,))

    f = F_BOLD(26)
    label_box(d, "Default", 36, H - 44, "l", f, TEXT)
    label_box(d, "Satur8", W - 36, H - 44, "r", f, CYAN_2)
    return rounded(img, 18)


# ---------------------------------------------------------------------------
# hero.png
# ---------------------------------------------------------------------------
def make_hero():
    W, H = 1200, 630
    img = site_background(W, H)
    d = ImageDraw.Draw(img, "RGBA")

    pad = 72
    draw_wordmark(img, pad, 92, 116)
    d = ImageDraw.Draw(img, "RGBA")

    d.text((pad + 4, 244), "Per-game digital vibrance for Linux",
           font=F_MED(30), fill=CYAN_2)
    d.text((pad + 4, 304), "Boost saturation when your game is focused.",
           font=F_REG(23), fill=TEXT)
    d.text((pad + 4, 338), "Restore your desktop when you leave.",
           font=F_REG(23), fill=MUTED)

    d.text((pad + 4, 410), "SATURATION", font=F_MONO_B(15), fill=MUTED)
    slider(d, pad + 4, 450, 300, 0.78, "1.75x")

    cx, cy, ch = pad + 4, 512, 40
    for c in ["No injection", "No overlay", "No Vulkan layer"]:
        w = pill(d, cx, cy, ch, c, F_MED(17))
        cx += w + 16

    # Right-side before/after scene panel.
    pw, ph = 470, 360
    px, py = W - pw - 64, (H - ph) // 2 - 6
    scene = base_scene(pw, ph)
    left = satur(scene, 1.0).crop((0, 0, pw // 2, ph))
    right = satur(scene, 1.75).crop((pw // 2, 0, pw, ph))
    panel = Image.new("RGB", (pw, ph), INK)
    panel.paste(left, (0, 0))
    panel.paste(right, (pw // 2, 0))
    pdv = ImageDraw.Draw(panel, "RGBA")
    pdv.rectangle([pw // 2 - 1, 0, pw // 2 + 1, ph], fill=CYAN + (255,))
    label_box(pdv, "Default", 16, ph - 28, "l", F_BOLD(18), TEXT)
    label_box(pdv, "Satur8", pw - 16, ph - 28, "r", F_BOLD(18), CYAN_2)
    panel = rounded(panel, 16)
    border = Image.new("RGBA", (pw, ph), (0, 0, 0, 0))
    ImageDraw.Draw(border).rounded_rectangle(
        [0, 0, pw - 1, ph - 1], radius=16, outline=(60, 68, 82), width=2)
    panel = Image.alpha_composite(panel, border)
    img.alpha_composite(panel, (px, py))

    return img.convert("RGB")


# ---------------------------------------------------------------------------
# demo.gif (60fps via ffmpeg)
# ---------------------------------------------------------------------------
def hud(scene, status, sat_t, sat_label, dot=CYAN):
    W, H = scene.size
    img = scene.convert("RGBA")
    d = ImageDraw.Draw(img, "RGBA")
    bar_h = 88
    bar_cy = H - bar_h // 2
    d.rectangle([0, H - bar_h, W, H], fill=(7, 8, 11, 215))
    d.rectangle([0, H - bar_h, W, H - bar_h + 2], fill=CYAN + (180,))
    d.ellipse([24, bar_cy - 7, 38, bar_cy + 7], fill=dot)
    d.text((52, bar_cy), status, font=F_BOLD(22), fill=TEXT, anchor="lm")
    slider(d, W - 360, bar_cy - 3, 240, sat_t, sat_label)
    # Wordmark watermark.
    d.text((22, 30), "Satur", font=F_BOLD(26), fill=(243, 246, 251, 235), anchor="lm")
    sw = d.textlength("Satur", font=F_BOLD(26))
    d.text((22 + sw, 30), "8", font=F_BOLD(26), fill=CYAN + (255,), anchor="lm")
    return img.convert("RGB")


def make_demo():
    W, H = 800, 440
    scene = base_scene(W, H)
    fps = 60

    # Saturation keyframes (time seconds, factor) with eased ramps.
    kf = [(0.0, 1.0), (1.3, 1.0), (2.0, 1.75), (3.0, 1.75),
          (3.5, 1.75), (4.2, 1.0), (4.8, 1.0)]
    spans = [
        (0.0, 0.7, "Desktop idle", MUTED),
        (0.7, 1.3, "cs2 focused, profile matched", CYAN),
        (1.3, 2.0, "Applying saturation", CYAN),
        (2.0, 3.0, "Saturation applied", CYAN),
        (3.0, 3.5, "Switched away from game", AMBER),
        (3.5, 4.2, "Restoring desktop", CYAN),
        (4.2, 4.8, "Desktop restored", CYAN),
    ]
    dur = kf[-1][0]
    nframes = int(round(dur * fps))

    def factor_at(t):
        for i in range(len(kf) - 1):
            t0, f0 = kf[i]
            t1, f1 = kf[i + 1]
            if t0 <= t <= t1:
                lt = 0 if t1 == t0 else (t - t0) / (t1 - t0)
                lt = lt * lt * (3 - 2 * lt)  # smoothstep
                return f0 + (f1 - f0) * lt
        return kf[-1][1]

    def span_at(t):
        for s in spans:
            if s[0] <= t < s[1]:
                return s[2], s[3]
        return spans[-1][2], spans[-1][3]

    def slider_t(v):
        return max(0.0, min(1.0, (v - 0.8) / (2.0 - 0.8)))

    # Cache rendered frames by quantized factor so we do not re-enhance 288x.
    cache = {}
    tmp = tempfile.mkdtemp(prefix="satur8demo_")
    try:
        for i in range(nframes):
            t = i / fps
            fac = round(factor_at(t), 3)
            status, dot = span_at(t)
            key = (fac, status, dot)
            if key not in cache:
                cache[key] = hud(satur(scene, fac), status, slider_t(fac), f"{fac:.2f}x", dot)
            cache[key].save(os.path.join(tmp, f"f{i:04d}.png"))
        out = os.path.join(OUT, "demo.gif")
        subprocess.run([
            "ffmpeg", "-y", "-loglevel", "error", "-framerate", str(fps),
            "-i", os.path.join(tmp, "f%04d.png"),
            "-filter_complex",
            "split[s0][s1];[s0]palettegen=stats_mode=diff[p];"
            "[s1][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle",
            "-loop", "0", out,
        ], check=True)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


# ---------------------------------------------------------------------------
# architecture.svg
# ---------------------------------------------------------------------------
def make_architecture():
    boxes = [
        ("Focused game", "active window"),
        ("Focus detection", "KWin script / poll"),
        ("Profile match", "exe, class, AppID"),
        ("Display backend", "compositor / driver"),
        ("Monitor output", "adjusted color"),
    ]
    bw, bh, gap = 200, 86, 56
    x0, y0 = 30, 60
    total_w = len(boxes) * bw + (len(boxes) - 1) * gap + 60
    H = 320

    def esc(s):
        return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")

    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{total_w}" height="{H}" '
        f'viewBox="0 0 {total_w} {H}" font-family="DejaVu Sans, Noto Sans, Arial, sans-serif">',
        '<defs>'
        '<marker id="arrow" markerWidth="10" markerHeight="10" refX="8" refY="3" '
        'orient="auto" markerUnits="strokeWidth">'
        '<path d="M0,0 L8,3 L0,6 Z" fill="#1fe3cb"/></marker>'
        '<marker id="arrow-spec" markerWidth="10" markerHeight="10" refX="8" refY="3" '
        'orient="auto" markerUnits="strokeWidth">'
        '<path d="M0,0 L8,3 L0,6 Z" fill="#ff3d8a"/></marker>'
        '<linearGradient id="box" x1="0" y1="0" x2="0" y2="1">'
        '<stop offset="0" stop-color="#141821"/><stop offset="1" stop-color="#0c0e13"/>'
        '</linearGradient>'
        '<linearGradient id="spectrum" x1="0" y1="0" x2="1" y2="0">'
        '<stop offset="0" stop-color="#ff3d8a"/><stop offset="0.26" stop-color="#ff6a3d"/>'
        '<stop offset="0.48" stop-color="#ffc24b"/><stop offset="0.74" stop-color="#1fe3cb"/>'
        '<stop offset="1" stop-color="#7c6cff"/></linearGradient></defs>',
        f'<rect x="0" y="0" width="{total_w}" height="{H}" rx="16" fill="#07080b"/>',
    ]

    cy = y0 + bh // 2
    centers = []
    for i, (title, sub) in enumerate(boxes):
        x = x0 + i * (bw + gap)
        centers.append((x, x + bw))
        parts.append(
            f'<rect x="{x}" y="{y0}" width="{bw}" height="{bh}" rx="12" '
            f'fill="url(#box)" stroke="#1fe3cb" stroke-width="1.5"/>'
        )
        parts.append(
            f'<text x="{x + bw // 2}" y="{y0 + 34}" fill="#f3f6fb" font-size="18" '
            f'font-weight="700" text-anchor="middle">{esc(title)}</text>'
        )
        parts.append(
            f'<text x="{x + bw // 2}" y="{y0 + 60}" fill="#828c9d" font-size="13" '
            f'text-anchor="middle">{esc(sub)}</text>'
        )
        if i > 0:
            px = centers[i - 1][1]
            parts.append(
                f'<line x1="{px + 6}" y1="{cy}" x2="{x - 6}" y2="{cy}" '
                f'stroke="#1fe3cb" stroke-width="2.5" marker-end="url(#arrow)"/>'
            )

    last_cx = centers[-1][0] + bw // 2
    first_cx = centers[0][0] + bw // 2
    fb_y = y0 + bh + 56
    parts.append(
        f'<path d="M{last_cx},{y0 + bh} L{last_cx},{fb_y} L{first_cx},{fb_y} '
        f'L{first_cx},{y0 + bh}" fill="none" stroke="url(#spectrum)" stroke-width="2.5" '
        f'stroke-dasharray="7 6" marker-end="url(#arrow-spec)"/>'
    )
    parts.append(
        f'<text x="{(first_cx + last_cx) // 2}" y="{fb_y + 22}" fill="url(#spectrum)" '
        f'font-size="14" font-weight="700" text-anchor="middle">'
        f'Restore desktop when focus changes</text>'
    )
    parts.append('</svg>')

    with open(os.path.join(OUT, "architecture.svg"), "w") as f:
        f.write("\n".join(parts))


def _wrap(draw, text, font, max_w):
    words, lines, cur = text.split(), [], ""
    for w in words:
        t = (cur + " " + w).strip()
        if draw.textlength(t, font=font) <= max_w:
            cur = t
        else:
            if cur:
                lines.append(cur)
            cur = w
    if cur:
        lines.append(cur)
    return lines


def _icon_sliders(d, x, y, s, c):
    for i, frac in enumerate([0.7, 0.4, 0.85]):
        yy = y + i * (s // 2)
        d.line([(x, yy), (x + s, yy)], fill=c + (90,), width=3)
        kx = x + int(s * frac)
        d.ellipse([kx - 5, yy - 5, kx + 5, yy + 5], fill=c + (255,))


def _icon_shield(d, x, y, s, c):
    cx = x + s // 2
    pts = [(cx, y), (x + s, y + s * 0.22), (x + s, y + s * 0.55),
           (cx, y + s), (x, y + s * 0.55), (x, y + s * 0.22)]
    d.polygon(pts, outline=c + (255,), width=3)
    d.line([(cx - s * 0.22, y + s * 0.5), (cx - s * 0.04, y + s * 0.66),
            (cx + s * 0.26, y + s * 0.32)], fill=c + (255,), width=3, joint="curve")


def _icon_chip(d, x, y, s, c):
    d.rounded_rectangle([x, y, x + s, y + s], radius=6, outline=c + (255,), width=3)
    inset = int(s * 0.28)
    d.rounded_rectangle([x + inset, y + inset, x + s - inset, y + s - inset],
                        radius=3, outline=c + (200,), width=2)
    for i in range(3):
        px = x + int(s * (0.3 + 0.2 * i))
        d.line([(px, y - 6), (px, y)], fill=c + (255,), width=3)
        d.line([(px, y + s), (px, y + s + 6)], fill=c + (255,), width=3)
        d.line([(x - 6, px), (x, px)], fill=c + (255,), width=3)
        d.line([(x + s, px), (x + s + 6, px)], fill=c + (255,), width=3)


def make_why():
    W, H = 1200, 360
    img = site_background(W, H)
    d = ImageDraw.Draw(img, "RGBA")
    cards = [
        ("Per-game profiles",
         "Tune saturation per game instead of changing your whole desktop every "
         "time. Profiles match by executable name, window class, or Steam AppID.",
         _icon_sliders),
        ("Game-safe approach",
         "Satur8 works outside the game process. It does not inject code, hook "
         "rendering APIs, or require an overlay. It changes the display pipeline "
         "after the game has rendered.",
         _icon_shield),
        ("Native Linux backends",
         "Uses the compositor, driver, or display color path that fits your "
         "session, across both Wayland and X11.",
         _icon_chip),
    ]
    margin, gap = 40, 24
    cw = (W - margin * 2 - gap * 2) // 3
    ch = H - margin * 2
    title_f, body_f = F_BOLD(27), F_REG(18)
    for i, (title, body, icon) in enumerate(cards):
        x = margin + i * (cw + gap)
        y = margin
        d.rounded_rectangle([x, y, x + cw, y + ch], radius=14,
                            fill=(14, 17, 23, 235), outline=(30, 37, 50, 255), width=1)
        pad = 26
        icon(d, x + pad, y + pad + 4, 30, VIOLET)
        d.text((x + pad, y + pad + 58), title, font=title_f, fill=TEXT)
        ty = y + pad + 100
        for ln in _wrap(d, body, body_f, cw - pad * 2):
            d.text((x + pad, ty), ln, font=body_f, fill=MUTED)
            ty += 27
    return img.convert("RGB")


def main():
    make_hero().save(os.path.join(OUT, "hero.png"))
    make_before_after().convert("RGB").save(os.path.join(OUT, "before-after.png"))
    make_why().save(os.path.join(OUT, "why-satur8.png"))
    make_demo()
    make_architecture()
    print("wrote:", ", ".join(sorted(os.listdir(OUT))))


if __name__ == "__main__":
    main()
