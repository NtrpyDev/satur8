# Satur8

Per-game digital vibrance for Linux, written in Rust.

Satur8 boosts color saturation for games like CS2 and restores your desktop when
you leave the game. It is built for Linux display stacks instead of copying the
Windows driver-panel approach. The goal is simple: make games look vivid without
injecting anything into the game process and without wasting CPU while you play.

Website: https://satur8.app

GitHub: https://github.com/NtrpyDev/satur8

Twitter / X: https://x.com/NtrpyDev

## v0.1 status

Satur8 v0.1 is the first working public version.

- KDE Plasma Wayland is implemented and tested on a real desktop.
- The GUI, CLI, daemon, KWin effect, KWin focus script, tray app, and profile
  config all work together.
- Per-game profiles auto-save from the GUI.
- The daemon reloads profile changes and reapplies the active game profile.
- Dark and bright GUI themes are included.
- Other backends are implemented behind environment detection, but KDE Plasma
  Wayland is the verified v0.1 target.

## Why Satur8 exists

Windows users have NVIDIA Digital Vibrance, and tools like VibranceGUI can turn
that setting up only while a game is running. Linux users have never had one
clean equivalent that works across modern X11 and Wayland sessions.

The hard part is Wayland. On Wayland, the compositor owns the final display
pipeline. A normal app cannot directly change the monitor color matrix. Satur8
handles that by using native compositor or display backends instead of trying to
hack around the display server.

## Why it is different

Satur8 changes the display color pipeline after the game has already rendered.
It does not inject a Vulkan layer, shared object, overlay, shader mod, or hook
inside the game process.

That matters for games like CS2. Tools such as vkBasalt and ReShade-style
injectors can be blocked by trusted or anti-cheat modes because they load into
the game. Satur8 avoids that class of problem by working outside the game.

This is not a promise about what any anti-cheat vendor will do in the future.
It is a clear statement of how Satur8 works: it adjusts compositor or scanout
color, not game memory or game rendering code.

## Why Rust

Satur8 is written in Rust because the main process is a long-lived desktop
daemon. It should be small, event-driven, and quiet.

Rust is a good fit here because:

- No garbage collector runs in the background while a game is CPU-bound.
- The daemon can sit idle until a focus change happens.
- The core saturation math is shared safely by every backend.
- The CLI, daemon, tray, and GUI can live in one workspace without a scripting
  runtime dependency.
- Native binaries are easy to ship for desktop Linux users.

The KWin effect itself is C++ because KWin effects are Qt/KWin plugins. The
control tools around it are Rust.

## Install

The installer builds and installs the binaries, KWin effect, KWin focus script,
GNOME extension, systemd user unit, and desktop entry.

```sh
packaging/install.sh
```

For a system-wide KWin effect install:

```sh
packaging/install.sh --system
```

For a per-user uninstall:

```sh
packaging/install.sh --uninstall
```

The Arch packaging scaffold is in `packaging/PKGBUILD`.

## GUI

Launch the desktop app:

```sh
satur8-gui
```

In the GUI you can:

- Add a currently running game.
- Change the saturation slider for that game.
- Let the profile auto-save immediately.
- Preview the effect on the desktop.
- Restore the desktop manually.
- Switch between bright and dark UI themes.
- Read the in-app About page with project links.

Profiles are stored at:

```sh
~/.config/satur8/profiles.toml
```

GUI settings, such as the theme, are stored at:

```sh
~/.config/satur8/gui.toml
```

## CLI quick start

Set saturation now:

```sh
satur8 on 1.75
```

Restore normal color:

```sh
satur8 off
```

Show backend and current state:

```sh
satur8 status
```

Run diagnostics:

```sh
satur8 doctor
```

Create a profile:

```sh
satur8 profile add cs2 2.0 --exe cs2 --steam-app-id 730
```

List profiles:

```sh
satur8 profile list
```

## Auto-apply per game

Satur8 has two ways to apply profiles.

### Focus daemon

This is the normal KDE path in v0.1. The KWin focus script forwards active
window changes to the daemon. The daemon matches the focused game against your
profiles and applies the saturation automatically.

Enable the user daemon:

```sh
systemctl --user enable --now satur8-daemon
```

Enable the KWin focus script:

```sh
kwriteconfig6 --file kwinrc --group Plugins --key satur8-focusEnabled true
qdbus6 org.kde.KWin /KWin reconfigure
```

If KWin has not loaded the script yet, this can load it immediately:

```sh
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.loadScript \
  "$HOME/.local/share/kwin/scripts/satur8-focus/contents/code/main.js" satur8-focus
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.start
```

### Launch wrapper

The launch wrapper is still useful for non-KDE sessions or manual workflows:

```sh
satur8 run --profile cs2 -- %command%
```

That can be used as a Steam launch option. It applies the profile before the
game starts and restores the desktop when the command exits.

## Backends

Satur8 uses the best backend available for the current session.

| Environment | Backend | Cost | v0.1 status |
|---|---|---:|---|
| KDE Plasma Wayland | KWin saturation effect | One compositor pass | Verified |
| GNOME Wayland | GNOME Shell shader extension | One compositor pass | Implemented |
| Hyprland | hyprctl shader backend | One compositor pass | Implemented |
| X11 with NVIDIA | NV-CONTROL Digital Vibrance | Zero per-frame cost | Implemented |
| DRM/KMS capable sessions | DRM CTM | Zero per-frame cost | Implemented |
| Unsupported Wayland compositors | gamescope shader fallback | Extra nested compositor pass | Implemented |

The zero-cost paths use hardware or driver display controls. The compositor
shader paths are still cheap, but they do work per frame because the compositor
has to shade the final output.

## Repo layout

Every major folder has one job.

```text
.
|-- Cargo.toml                         Workspace and shared dependency versions
|-- README.md                          Public project readme
|-- PLAN.md                            Longer design notes and backend roadmap
|-- LICENSE                            GPL-3.0-or-later license
|-- packaging/
|   |-- install.sh                     Per-user and system installer
|   |-- PKGBUILD                       Arch package scaffold
|   |-- satur8.desktop                 Desktop launcher
|   `-- satur8-daemon.service          systemd user unit
|-- assets/
|   |-- kwin-effect/                   Native KWin compositor effect
|   |-- kwin-script/                   KWin active-window focus forwarder
|   |-- gnome-extension/               GNOME Shell shader extension
|   |-- gamescope/                     gamescope ReShade fallback shader
|   `-- gui-shot.sh                    Off-screen GUI screenshot helper
`-- crates/
    |-- satur8-core/                   Shared saturation math and profile model
    |-- satur8-cli/                    `satur8` command-line app
    |-- satur8-gui/                    Slint desktop GUI
    |-- satur8-daemon/                 Focus daemon and profile auto-apply
    |-- satur8-tray/                   StatusNotifier tray app
    `-- backends/
        |-- kwin/                      KWin D-Bus backend
        |-- gnome-shell/               GNOME Shell D-Bus backend
        |-- hyprland/                  Hyprland backend
        |-- nv-control/                NVIDIA X11 backend
        |-- drm-ctm/                   DRM color transform matrix backend
        `-- gamescope/                 gamescope fallback launcher backend
```

## Development

Build everything:

```sh
cargo build --release
```

Build only the GUI:

```sh
cargo build -p satur8-gui
```

Capture GUI screenshots off-screen:

```sh
bash assets/gui-shot.sh /tmp/satur8-profiles.png 0
SATUR8_GUI_DARK=1 bash assets/gui-shot.sh /tmp/satur8-settings-dark.png 1
```

Run CLI help:

```sh
satur8 --help
satur8 profile --help
satur8 run --help
```

## v0.1 release checklist

- Rust workspace builds.
- GUI renders in bright and dark modes.
- KDE Plasma Wayland backend verified.
- CS2 profile matching works through the focus daemon.
- Desktop restores when leaving the game.
- README, package metadata, and links point to the public project.

## License

GPL-3.0-or-later.
