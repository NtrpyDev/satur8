# Satur8 (working name)

Per-game **digital vibrance / saturation** for Linux - the thing VibranceGUI
does on Windows, built for Wayland and X11.

> **Status: working.** KDE Plasma Wayland is fully implemented and verified;
> other backends are implemented and gated by environment. See [`PLAN.md`](PLAN.md)
> for the full design.

## Install

```sh
packaging/install.sh            # per-user install (no root)
packaging/install.sh --system   # system install (KWin effect to the system Qt plugin dir)
```

This builds the Rust binaries (`satur8`, `satur8-daemon`, `satur8-tray`),
the KWin effect, and installs the KWin focus script, GNOME extension, a systemd
user unit, and a desktop entry. An Arch [`PKGBUILD`](packaging/PKGBUILD) is
provided too.

## Usage

```sh
satur8 on 1.5                         # boost saturation now
satur8 off                            # restore, release all per-frame cost
satur8 status                         # environment + backend + current state
satur8 doctor                         # diagnose backends (incl. DRM CTM probe)

# Per-game, as a Steam launch option (apply on launch, restore on exit):
satur8 run --profile cs2 -- %command%

# Profiles:
satur8 profile add cs2 1.6 --exe cs2 --steam-app-id 730
satur8 profile list

# Universal fallback for niche compositors:
satur8 run --via gamescope --saturation 1.5 -- <game>
```

Always-on, follow-focus mode (KDE): enable the daemon + the KWin focus script:

```sh
systemctl --user enable --now satur8-daemon
kwriteconfig6 --file kwinrc --group Plugins --key satur8-focusEnabled true
qdbus6 org.kde.KWin /KWin reconfigure
```

## What it is

Boost color saturation when you launch a game (e.g. CS2) and restore your normal
desktop colors when it closes. Per-game profiles, hardware/compositor agnostic
by design.

## How it stays safe with anti-cheat

Satur8 **only changes the display color pipeline** - either the GPU's hardware
scanout color matrix or the compositor - *after* the game has rendered its
frame. It never injects into, reads, or modifies the game process. That is the
same category as turning up saturation on your monitor's OSD.

This is the key difference from tools like **vkBasalt** or in-game **ReShade**,
which load *inside the game process* and are exactly what CS2's Trusted Mode is
built to block. Satur8 deliberately does not do that.

This is a description of what the tool does and does not do - not a guarantee
about any anti-cheat's future behavior.

## Lowest possible cost

Where the environment allows it (X11, Hyprland, TTY), Satur8 sets the GPU's
**hardware Color Transformation Matrix**: a one-time setup with **zero**
per-frame CPU or GPU cost - it matters because CS2 is CPU-bound. On KDE Wayland
(the first supported target) it uses a tiny single-purpose compositor shader; the
roadmap moves KDE to the zero-cost path if/when KWin exposes it.

## Supported environments

| Environment | Backend | Cost | Status |
|---|---|---|---|
| KDE Plasma Wayland | KWin saturation effect | ~free (one GPU pass) | implemented, verified |
| GNOME Wayland (any GPU) | Shell extension shader | ~free | implemented |
| Hyprland | screen shader via hyprctl | ~free | implemented |
| X11 + NVIDIA | NV-CONTROL Digital Vibrance | **zero** | implemented |
| Bare KMS / TTY | DRM CTM | **zero** | implemented |
| Niche wlroots (Sway, ...) | gamescope + reshade | small | implemented (fallback) |

"Verified" means exercised end-to-end on real hardware; the others are
implemented against each platform's documented interface and gated by
environment detection.

## License

GPL-3.0-or-later.
