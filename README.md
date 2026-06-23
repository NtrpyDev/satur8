# Vibrance (working name)

Per-game **digital vibrance / saturation** for Linux - the thing VibranceGUI
does on Windows, built for Wayland and X11.

> **Status: early scaffold.** See [`PLAN.md`](PLAN.md) for the full design.

## What it is

Boost color saturation when you launch a game (e.g. CS2) and restore your normal
desktop colors when it closes. Per-game profiles, hardware/compositor agnostic
by design.

## How it stays safe with anti-cheat

Vibrance **only changes the display color pipeline** - either the GPU's hardware
scanout color matrix or the compositor - *after* the game has rendered its
frame. It never injects into, reads, or modifies the game process. That is the
same category as turning up saturation on your monitor's OSD.

This is the key difference from tools like **vkBasalt** or in-game **ReShade**,
which load *inside the game process* and are exactly what CS2's Trusted Mode is
built to block. Vibrance deliberately does not do that.

This is a description of what the tool does and does not do - not a guarantee
about any anti-cheat's future behavior.

## Lowest possible cost

Where the environment allows it (X11, Hyprland, TTY), Vibrance sets the GPU's
**hardware Color Transformation Matrix**: a one-time setup with **zero**
per-frame CPU or GPU cost - it matters because CS2 is CPU-bound. On KDE Wayland
(the first supported target) it uses a tiny single-purpose compositor shader; the
roadmap moves KDE to the zero-cost path if/when KWin exposes it.

## Supported environments (planned)

| Environment | Backend | Cost |
|---|---|---|
| KDE Plasma Wayland | KWin saturation effect | ~free (one GPU pass) |
| X11 (AMD/Intel/nouveau) | DRM CTM | **zero** |
| Hyprland | Hyprland CTM protocol | **zero** |
| Anything (fallback) | gamescope + reshade | small |

## License

GPL-3.0-or-later.
