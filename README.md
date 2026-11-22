<div align="center">

<img src="assets/readme/hero.png" alt="Satur8" width="900">


**Per-game digital vibrance for Linux gaming.**

Boost saturation when your game is focused. Restore your desktop when you leave.  
No game injection. No Vulkan layer. No overlay. Built for modern Linux display stacks.

[Website](https://satur8.app) · [Releases](https://github.com/NtrpyDev/satur8/releases) · [Roadmap](ROADMAP.md) · [Packaging](PACKAGING.md) · [Contributing](CONTRIBUTING.md) · [Issues](https://github.com/NtrpyDev/satur8/issues)

![License](https://img.shields.io/github/license/NtrpyDev/satur8?color=ff3d8a)
![Release](https://img.shields.io/github/v/release/NtrpyDev/satur8?color=ffc24b)
![Platform](https://img.shields.io/badge/platform-Linux-1fe3cb)
![Language](https://img.shields.io/badge/language-Rust-7c6cff)

</div>

## What is Satur8?

Satur8 is a Linux app for per-game saturation control.

Create a profile for a game, choose a vibrance level, and Satur8 applies it automatically when the game is focused. When you leave the game, your desktop returns to normal.

It is built for Linux gaming setups that want the punchy color controls common on Windows and NVIDIA, but through Linux-native compositor, driver, and display backends instead of a copied driver panel.

- **Per-game profiles.** Tune saturation per game instead of changing your whole desktop. Profiles match by executable name, window class, or Steam AppID.
- **Follows focus.** The profile applies when the game is focused and your desktop color comes back when you switch away.
- **Outside the game.** No injection, no overlay, no Vulkan layer. See [How it works](#how-it-works).
- **Native Linux backends.** Uses the compositor, driver, or display color path that fits your session, across Wayland and X11, and Steam Game Mode on a Steam Deck. See [Backend support](#backend-support).
- **GUI and CLI.**

## Install

### Fedora (COPR)

The Fedora package is live on [COPR](https://copr.fedorainfracloud.org/coprs/ntrpydev/satur8/), built for Fedora 43 and 44.

```sh
sudo dnf copr enable ntrpydev/satur8
sudo dnf install satur8
```

### Arch (AUR)

Two packages are on the AUR, published and maintained by a community member: [`satur8`](https://aur.archlinux.org/packages/satur8) builds from this repo's source release, and [`satur8-bin`](https://aur.archlinux.org/packages/satur8-bin) installs binaries prebuilt by the package maintainer. Install either with your AUR helper. Because they are community-maintained, they can trail the latest release.

To build the current version yourself, use the packaging files in `packaging/` from a checkout:

```sh
cd packaging
makepkg -si
```

This builds Satur8 from source, including the Rust workspace and the KWin effect.

### Release tarball

Download the latest Linux x86_64 release from the [releases page](https://github.com/NtrpyDev/satur8/releases), then run the installer:

```sh
tar -xzf satur8-*-linux-x86_64.tar.gz
cd satur8-*-linux-x86_64
packaging/install.sh
```

The installer needs no root. On KDE Plasma it places the KWin effect in your home directory and adds that directory to KWin's plugin search path, which KWin reads at login, so log out and back in once after the first install.

To use the effect without logging out, install it to the system plugin directory instead (uses `sudo`):

```sh
packaging/install.sh --system
```

To uninstall a per-user install:

```sh
packaging/install.sh --uninstall
```

If you clone the source repository instead, the same installer builds and installs Satur8 from source.

On a Steam Deck, see [Steam Deck](#steam-deck).

## Quick start

Start the focus daemon:

```sh
systemctl --user enable --now satur8-daemon
```

On KDE Plasma, turn on the KWin focus script once. It tells the daemon which window is focused, and it is off until you enable it:

```sh
kwriteconfig6 --file kwinrc --group Plugins --key satur8-focusEnabled true
qdbus6 org.kde.KWin /KWin reconfigure
```

Open the desktop app, add a game profile, and set its saturation:

```sh
satur8-gui
```

Launch the game. Satur8 applies the profile when the game is focused and restores your desktop when you switch away.

### Without the focus daemon

On non-KDE sessions, or for manual workflows, use the launch wrapper. It applies a profile, launches the game, and restores on exit. This works as a Steam launch option:

```sh
satur8 run --profile cs2 -- %command%
```

For sessions without a native backend, the gamescope strategies are available:

```sh
satur8 run --via gamescope-native -- %command%   # drive a running gamescope
satur8 run --via gamescope -- %command%          # nested gamescope fallback
```

In Steam Game Mode, use the focus daemon instead; see [Steam Deck](#steam-deck).

## Steam Deck

Satur8 switches saturation per game in Steam Game Mode. This was tested on a Steam Deck OLED with SteamOS 3.9.5 and gamescope 3.16.30.1.

Satur8 applies the color through gamescope's look table, so Steam's own Color Vibrancy and Color Temperature settings keep working on top of it. Satur8 never changes them.

### Install on the Deck

Open Konsole in Desktop Mode, or connect to the Deck over SSH. Download the latest Linux x86_64 release from the [releases page](https://github.com/NtrpyDev/satur8/releases), then run the per-user installer with the background service turned on:

```sh
tar -xzf satur8-*-linux-x86_64.tar.gz
cd satur8-*-linux-x86_64
packaging/install.sh --enable-daemon
```

Without `--enable-daemon`, the installer prints the command that turns the service on. Everything is installed in your home folder, so it survives SteamOS updates. `--system` is refused on SteamOS. If the installer says the KWin effect was not installed, that is fine: Game Mode does not use it.

The commands are installed to `~/.local/bin`. If your shell does not find `satur8`, run it as `~/.local/bin/satur8`.

### Add a game

On a Deck, profiles match a game by its Steam AppID. The AppID is the number in the game's `~/.local/share/Steam/steamapps/appmanifest_<AppID>.acf` file name, and in the game's Steam store URL.

```sh
satur8 profile add sushiamo 1.75 --steam-app-id 4555040
```

Matching by executable also works for Proton games. Use the Windows executable name, for example `--exe Sushiamo.exe`.

The Steam home screen and games without a profile get `default_saturation` from `~/.config/satur8/profiles.toml`. At 1.0, the default, Satur8 loads nothing there.

### Automatic switching

The background service does the switching. If you installed without `--enable-daemon`, turn it on with:

```sh
systemctl --user enable --now satur8-daemon
```

In Game Mode, a game's profile loads when the game takes focus and is removed when you leave or close the game. Opening Steam's overlay menu over the game keeps its profile. If the service is killed, systemd restarts it within 5 seconds. Stopping the service puts `default_saturation` back; `satur8 off` removes Satur8's table at any value.

### Check it

Over SSH while the Deck is in Game Mode:

```sh
satur8 doctor
```

It reports `Steam Game Mode (gamescope on :0)`, whether a Satur8 table is loaded and at what saturation, the focused app's AppID, whether the background service is running, and Steam's own color settings, which Satur8 leaves alone. `satur8 status` shows the same. `satur8 on`, `satur8 set` and `satur8 off` also work in Game Mode, including over SSH.

### The launch option

The `satur8 run --via gamescope-native -- %command%` launch option still works as before. It replaces Steam's color settings while the game runs, and it wins over a profile's table until the game ends. Use the background service instead, and do not use both on one game.

### Uninstall from the Deck

From the extracted release folder:

```sh
packaging/install.sh --uninstall
```

This turns the service off, runs `satur8 off`, and removes the commands, the service and Satur8's table files.

### Not tested yet

- HDR games. By gamescope's source, the look table only changes SDR content, so HDR games are expected to look unchanged.
- An external display.
- The desktop app (`satur8-gui`) on the Deck, and the Deck's Desktop Mode.
- Bazzite and other systems that run a gamescope session.
- gamescope versions other than 3.16.30.1.

## How it works

Satur8 does not modify the game itself.

Instead, it watches the active window, matches it against your saved profiles, and applies saturation through the best available display backend for your session.

<img src="assets/readme/architecture.svg" alt="Satur8 architecture diagram" width="900">

This means the game renders normally. Satur8 changes the final display color pipeline after the frame has already left the game.

<img src="assets/readme/before-after.png" alt="Default saturation compared with Satur8 boosted saturation" width="900">

The image above is a generated scene shown at normal saturation on the left and Satur8-boosted saturation on the right.

Because Satur8 does not inject into the game process or hook graphics APIs, it avoids the most common anti-cheat risk patterns associated with overlays, DLL injection, or render hooks. This is a description of how Satur8 works, not a promise about what any anti-cheat vendor will allow.

## Backend support

Satur8 chooses the best available backend for your session. Zero per-frame cost paths use hardware or driver display controls. Compositor shader paths are still cheap, but they do work per frame because the compositor shades the final output.

| Environment | Backend | Per-frame cost | Status |
|---|---|---:|---|
| KDE Plasma Wayland | KWin saturation effect | One compositor pass | Verified |
| GNOME Wayland | GNOME Shell shader extension | One compositor pass | Verified |
| X11 with NVIDIA | NV-CONTROL Digital Vibrance | Zero | Verified |
| Hyprland | `hyprctl` shader backend | One compositor pass | Verified |
| DRM/KMS sessions | DRM CTM | Zero | Implemented |
| Steam Game Mode (Steam Deck) | gamescope look table, automatic | Reuses gamescope's color pipeline | Verified on a Steam Deck OLED, SteamOS 3.9.5, gamescope 3.16.30.1 (2026-11-21) |
| gamescope (running compositor) | gamescope-native color atoms | Reuses existing gamescope | Verified |
| Unsupported Wayland | gamescope nested fallback | Extra nested compositor pass | Verified |

Status legend:

- Verified: tested on real hardware
- Implemented: built and behind environment detection, not yet independently verified

In Steam Game Mode, the focus daemon sets gamescope's look table per game, so Steam's own vibrancy and color temperature keep working on top; see [Steam Deck](#steam-deck). The gamescope-native path is a launch option that drives an already-running gamescope compositor through its Xwayland color override atoms, instead of launching a nested gamescope. While it runs, it replaces Steam's color settings.

## GUI

The screenshots below show the Satur8 GUI.

<img src="assets/readme/gui-main-v036.png" alt="Satur8 main page with desktop vibrance and game profiles" width="900">

On the Main page you can:

- Set the desktop vibrance, which is also the default when no game profile is active, and restore the desktop to normal.
- Add a currently running game.
- Set the saturation level per game.
- Enable or disable a profile.

<img src="assets/readme/gui-status-v036.png" alt="Satur8 status page with health checks and settings" width="900">

The Status page shows whether each part of Satur8 is working, with a Repair button, and the profile and saturation that are active right now. It also has the switch for automatic per-game switching, the starting saturation for newly added games, and recent activity. Copy system info and Report a bug help with bug reports; nothing is sent automatically. Profiles auto-save the moment you change them. The GUI also ships dark and bright themes and an in-app About page with project links.

Configuration is stored at:

```sh
~/.config/satur8/profiles.toml   # game profiles
~/.config/satur8/gui.toml        # GUI settings such as theme
```

## CLI

Satur8 includes a CLI for scripting, profiles, and debugging.

```sh
satur8 --help
```

Set or clear saturation now:

```sh
satur8 on 1.75      # turn on at 1.75x (1.0 = unchanged, 0 = greyscale)
satur8 off          # restore normal color
satur8 set 2.0      # set an exact value
```

Inspect the environment and backend:

```sh
satur8 status       # environment, chosen backend, and current state
satur8 doctor       # diagnose backend availability
satur8 outputs      # list the outputs the active backend can target
satur8 repair       # rebuild the KWin effect if the running KWin rejects it
```

KWin only loads an effect built for its exact version, so a KWin update used to break Satur8 until you reinstalled it. The focus daemon now runs `satur8 repair` at every login: when KWin rejects the effect, it rebuilds it from the installed source in a few seconds. That needs `cmake`, a C++ compiler, and the Qt 6 and KWin development files. `satur8 doctor` shows which KWin version each installed copy of the effect was built for.

Manage per-game profiles:

```sh
satur8 profile add cs2 1.75 --exe cs2 --steam-app-id 730
satur8 profile list
satur8 profile show cs2
satur8 profile disable cs2
satur8 profile enable cs2
satur8 profile remove cs2
satur8 profile path
```

A profile needs at least one match rule: `--exe`, `--window-class`, or `--steam-app-id`.

## Development

Satur8 is a Rust workspace. The control tools (CLI, daemon, tray, GUI, backends) are Rust; the KWin effect itself is C++ because KWin effects are Qt/KWin plugins.

Build everything:

```sh
cargo build --release
```

Build only the GUI:

```sh
cargo build -p satur8-gui
```

Run the workspace checks:

```sh
cargo check
cargo test --workspace --locked
```

Capture GUI screenshots off-screen (used for these README images):

```sh
SATUR8_GUI_DARK=1 bash assets/gui-shot.sh /tmp/satur8-main.png 0
SATUR8_GUI_DARK=1 bash assets/gui-shot.sh /tmp/satur8-status.png 1
```

Regenerate the rest of the README visuals:

```sh
python3 scripts/generate-readme-assets.py
```

Build the release and source archives:

```sh
packaging/release-tarball.sh
packaging/source-tarball.sh
```

## Roadmap

See [ROADMAP.md](ROADMAP.md) for what ships next: the remaining Steam Deck work and Bazzite support, then a stable 1.0. Distribution status and open packaging work are tracked there and in the [open issues](https://github.com/NtrpyDev/satur8/issues).

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for how to build, test, and submit changes.

Before opening a pull request, please make sure the workspace builds (`cargo build`), the checks pass (`cargo check` and `cargo test --workspace --locked`), and any README commands you touched are still accurate.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
