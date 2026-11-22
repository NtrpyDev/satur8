# Changelog

All notable changes to Satur8, newest first. Each entry is also published as the
GitHub release notes for that tag.

## v0.4.1 - 2026-11-22

Release binaries that run on older systems, and a working KWin effect on systems without build tools, including the Steam Deck's Desktop Mode.

- The release tarball's four programs are now built on Ubuntu 22.04 and need
  glibc 2.35 or newer. Before, the desktop app needed glibc 2.43 and the other
  programs 2.39, so the app did not start on Fedora 43, Ubuntu 24.04 or Debian.
  Every build is checked for a higher requirement, and each program is started
  in containers of Ubuntu 22.04, Debian 12, Ubuntu 24.04, openSUSE Leap 16.0
  and Fedora 43, the desktop app under a virtual display. The v0.3.6 tarball
  fails that same check with `GLIBC_2.43 not found` and `GLIBC_2.39 not found`.
- The tarball carries prebuilt KWin effects, each built in that system's own
  packages: `steamos-6.7.3` (SteamOS 3.9, the Stable channel, built against
  Valve's packages), `steamos-6.8.2` (SteamOS 3.10, Preview and Main),
  `fedora44-6.8.3` and `fedora45-6.8.3`. KWin only loads an effect built for
  its exact version, and the effect also needs a Qt at least as new as the one
  it was built with. The installer uses the effect built for your system and
  KWin version; if there is none it builds one when build tools are installed;
  otherwise it may use another system's effect for the same KWin version when
  your Qt is new enough, and says so; otherwise it skips the effect with the
  reason. `packaging/install.sh --pick-effect` prints the choice. Nothing is
  downloaded.
- Steam Deck Desktop Mode now works. On SteamOS 3.9 the installer uses the
  `steamos-6.7.3` effect; after switching to Desktop Mode once, KWin loads it
  with no further logout. `satur8 on` and `off` change the desktop, and
  per-game switching follows window focus after turning on the KWin focus
  script with the three commands the installer prints. Verified on a Steam Deck
  OLED with SteamOS 3.9.5 and KWin 6.7.3, with a camera on the screen.
  Switching between Game Mode and Desktop Mode several times left Steam's color
  settings unchanged.
- Bazzite KDE Desktop Mode gets the `fedora45-6.8.3` effect, built in Fedora
  45's packages for the KWin that Bazzite stable uses. In the Bazzite
  container, not on a Bazzite device, the installer picked it and installed it
  per user; with a stand-in KWin one patch newer it skipped the effect with the reason
  and installed everything else. Game Mode against Bazzite's gamescope passed
  again in the container with this build.
- The installer copies the whole effect set to
  `~/.local/share/satur8/kwin-effects`, and `satur8 repair` installs the
  matching effect from there before it tries a local build, with no network.
  Repair also checks the effect file on disk, so a wrong file is replaced even
  while KWin still uses a copy it loaded earlier. On the Deck, a wrong effect
  file was reported by `satur8 doctor` and replaced by `satur8 repair` from the
  set while KWin kept running. The effect file is always replaced as a new
  file, never written over in place.
- `satur8 doctor` on KDE names where the effect came from, and repair, doctor
  and the desktop app's Status page no longer promise a rebuild on systems that
  cannot build.
- The installer's closing message for Game Mode shows `satur8 games` and
  `satur8 profile add NAME 1.5 --steam-game NAME`.
- The release pipeline on GitHub builds the portable binaries and the effect
  set and runs the installer tests before it publishes the Linux tarball.
- Not covered: KWin 6.5 and older. On a system with no build tools, a KWin
  newer than the bundled effects keeps the effect off until a newer Satur8
  release includes that version.
- Not tested yet: HDR games (a game presenting SDR does get the look: ELDEN
  RING through Proton on the Deck), the `steamos-6.8.2` effect on a Deck
  running SteamOS 3.10, the Fedora effects on a real Fedora KDE or Bazzite
  desktop, the desktop app on the Deck beyond starting it, and a cold boot of
  the Deck.

## v0.4.0 - 2026-11-22

Automatic per-game saturation in Steam Game Mode, tested on a Steam Deck.

- Per-game profiles now switch automatically in Steam Game Mode. With the
  background service running, a game's profile loads when the game takes focus
  and is removed when you leave or close the game. Opening Steam's overlay menu
  over a game keeps the game's profile. The service starts with the Game Mode
  session; this was checked by switching the Deck to Desktop Mode and back, and
  a cold boot was not tested. A profile added or changed while the service runs
  takes effect right away, also for the game that is on screen.
- On every desktop, saving a profile for the window that has focus now applies
  it at once. Before, the daemon only matched profiles again at the next focus
  change, so a new profile for the game on screen waited until you switched
  away and back.
- The Steam home screen and games without a profile get `default_saturation`
  from `profiles.toml`. At 1.0, the default, Satur8 loads nothing there.
- `satur8 games` lists the installed Steam games with each game's AppID and
  profile, and `satur8 profile add NAME SATURATION --steam-game NAME` adds a
  profile by the game's name, part of it, or its AppID, so you do not have to
  look the AppID up. Steam's own runtimes and Proton are left out of the list.
- Profiles match a game by Steam AppID (`satur8 profile add NAME SATURATION
  --steam-app-id N`) or by executable, including the Windows executable name of
  a Proton game such as `Sushiamo.exe`.
- Satur8 applies the color through gamescope's look table, so Steam's own Color
  Vibrancy and Color Temperature keep working on top of it. Satur8 never
  changes them.
- If the background service is killed, systemd restarts it within 5 seconds and
  the game's profile stays loaded. Stopping the service puts
  `default_saturation` back; `satur8 off` removes Satur8's table at any value.
- `satur8 on`, `satur8 set` and `satur8 off` work in Game Mode, including over
  SSH.
- `satur8 doctor` and `satur8 status` recognize Steam Game Mode. Doctor reports
  "Steam Game Mode (gamescope on :0)", the loaded table and its saturation, the
  focused app, whether the background service is running, and Steam's own color
  settings, instead of KWin and DRM CTM sections that do not apply there. On
  desktops their output is unchanged.
- Installer: `packaging/install.sh --enable-daemon` turns on the background
  service; without it the installer prints the command. `--system` is refused
  on SteamOS and on image-based systems such as Bazzite, where `/usr` is
  read-only, and the installer no longer suggests it there. Everything is
  installed in your home folder, so it survives SteamOS updates. On a machine
  with a Game Mode session where the KWin effect cannot be installed, the
  installer now says Game Mode works without it, and uninstall runs `satur8
  off` before it removes the commands. Uninstall keeps `profiles.toml`, so
  profiles survive a reinstall; on the Deck it was checked with a game running
  and its table loaded. The installer prints a hint when `~/.local/bin` is not
  on PATH, which is the case in an SSH login on SteamOS.
- `satur8 repair` reads the installed KWin version from `kwin_wayland` when the
  KWin development headers are missing, so a matching per-user effect waits for
  the next login instead of reporting an error, and it explains that build
  tools cannot simply be installed on image-based systems.
- Unchanged: the `satur8 run --via gamescope-native` launch option works as
  before. It still replaces Steam's color settings while the game runs, and it
  wins over a profile's table until the game ends. Use the background service
  or the launch option for a game, not both.
- Tested on a Steam Deck OLED with SteamOS 3.9.5 and gamescope 3.16.30.1 in Game
  Mode, with a camera pointed at the screen. Saturation from grayscale to 2x
  responds in order.
- Bazzite: Game Mode support is implemented but Satur8 has not been run on a
  Bazzite device. The v0.4.0 installer and background service were run against
  Bazzite's own gamescope build (3.17.0-ogc1, in Bazzite stable 45.20261117.1)
  in a container: the service found gamescope, loaded a profile's table when
  that app took focus, removed it at the home window and when stopped, and
  uninstall left no table. Install from the release tarball without `--system`. Known gaps:
  the KDE desktop effect cannot be rebuilt on Bazzite after a KWin update
  because the image has no build tools, the GNOME images run GNOME 51, which
  the GNOME Shell extension does not list yet, and layering the COPR package
  with `rpm-ostree` is untested (a Fedora 45 build target, what current Bazzite
  is based on, was added to the COPR project on 2026-11-22).
- The Deck's Desktop Mode does not work yet. It runs KDE Plasma with KWin
  6.7.3, the KWin effect bundled in the release tarball is built for a
  different KWin, and SteamOS has no build tools to rebuild it, so the service
  starts there but cannot apply anything. Game Mode is not affected.
- Not tested yet: a cold boot of the Deck, HDR games (by gamescope's source the
  look table does not change HDR content), an external display, the desktop
  app on the Deck, a Bazzite device and other systems that run a gamescope
  session, and gamescope versions other than 3.16.30.1.
- The README has a new Steam Deck section, and ROADMAP.md and PLAN.md describe
  the Game Mode support and the Deck work that remains.

## v0.3.6 - 2026-11-22

Redesigned desktop app with a new Status tab, plus documentation fixes.

- Redesign the desktop app. The window now has three tabs, Main, Status and
  About, in the dark teal look of satur8.app, with a light mode on the theme
  button. Fonts and icons are bundled, so the app looks the same on every
  desktop.
- Main puts desktop vibrance in a compact strip above the game cards. Dragging
  the desktop slider changes the desktop live and saves once when you let go,
  instead of rewriting `profiles.toml` on every step of the drag.
- Game cards show each game's real Steam icon from the local Steam cache,
  including the newer cache layout where Steam keeps art in subfolders. Games
  without Steam art get a lettered tile.
- New Status tab:
  - Health checks for the color backend, the KWin effect, the background
    service and game detection, in plain words, with Repair and Check again.
    Repair does what `satur8 repair` does, and also starts the background
    service and game detection when those are meant to be on but are not
    running. Rows that only apply to KDE Plasma show as not applicable on other
    desktops instead of as failures.
  - A Right now view of the active profile, the saturation the compositor is
    applying, and the last window the daemon saw focused.
  - An Automatic per-game switching toggle that shows the real system state
    and does the three setup steps the installer used to print, plus starting
    game detection in the running session. It is turned off with a reason on
    sessions where per-game switching does not work yet.
  - A Starting saturation for new games setting (default +40%, the old fixed
    value), saved in `gui.toml`.
  - Copy system info, which leaves out your home folder, user name and
    computer name, and Report a bug, which copies that information and opens
    a new GitHub issue in your browser for you to paste it into. Nothing is
    sent until you submit the issue.
  - The activity log moved here from the main window.
- The daemon exposes a new read-only `LastFocused` D-Bus property for the
  Status tab. The app still works with an older daemon that lacks it.
- Closing the window hides it to the tray only when the tray icon belongs to
  you and runs in the same session. A tray started by another user, or by your
  other login, no longer keeps a hidden window around with no way to reopen it.
- Remove the Live Preview panel and the per-game Preview button. The app no
  longer changes the desktop just to preview a profile.
- ROADMAP.md and PLAN.md now name the current release and describe the
  backends and distribution channels as they are today (#22).
- The roadmap and packager guide no longer say AUR publication is blocked.
  They now match the README: `satur8` and `satur8-bin` are on the AUR,
  published and maintained by a community member.
- Roadmap cleanups: the website backend table item is marked done, the Game
  Mode item no longer claims per-game saturation is unique to Satur8
  (vibrantDeck already does per-app saturation), and the immutable targets are
  labeled v0.4 like everywhere else.
- PLAN.md now describes the gamescope-native path (`satur8 run --via
  gamescope-native`) alongside the nested gamescope fallback, instead of
  calling gamescope only a last resort, and lists both in the backend
  selection order as opt-in launch strategies.

## v0.3.5 - 2026-11-21

Installer fix for machines without KDE.

- Fix the release installer quitting on GNOME, Hyprland, NVIDIA X11 and
  gamescope-only machines. The v0.3.4 installer tried to rebuild the KWin
  effect even where KWin is not installed, and stopped before installing
  anything when it could not. The KWin effect is now optional: with no KWin it
  is skipped, and on KDE Plasma without build tools the rest of Satur8 still
  installs and `satur8 repair` builds the effect once the tools are there.
- Add an installer test that runs the release tarball in a clean container
  with no KWin, so this cannot ship broken again.

## v0.3.4 - 2026-11-21

Full reliability repair for the KDE app and its shared profile workflows.

- Satur8 no longer breaks on every KWin update. KWin only loads an effect built
  for its exact version, so each update left the installed effect rejected
  until a manual reinstall. The focus daemon now runs the new `satur8 repair`
  at every login: when the running KWin rejects the effect, it is rebuilt from
  the installed source in a few seconds, with no root. Packages and the
  installer now ship the effect source and an `environment.d` file that lets
  KWin find the per-user rebuild.
- The per-user installer works without the manual `QT_PLUGIN_PATH` step it used
  to print; log out and back in once after the first install.
- Fix per-game switching never starting on Plasma 6. The KWin focus script's
  metadata lacked `KPackageStructure`, so KWin skipped it at login even when it
  was enabled (#11, #13).
- Fix Proton and Wine games in the GUI's running-game list. They all showed up
  as `wine64-preloader`; the list now shows the real game executable, and the
  daemon matches the focused window by that same executable, so non-Steam
  games launched through umu, which all share one window class, each get their
  own profile (#12, #14).
- A profile added as `Game.exe` now also matches a process reported as `Game`.
- Build the KWin effect against KWin 6.8.
- When KWin rejects the effect, `satur8 doctor` and the daemon log now say so
  and show which KWin version each installed copy was built for, instead of
  suggesting the gamescope fallback.
- `satur8 doctor`, `status` and `outputs` no longer panic when piped into a
  command that stops reading early.

- Fix the KWin effect's black-screen failure. Identity saturation previously
  redirected every window through a shader whose custom uniforms were still
  zero, producing fully black outputs. The effect now initializes every draw
  and remains inactive until a non-identity value is requested. Setting
  saturation to exactly normal now switches the effect off instead of leaving
  it loaded (#10, #15).
- Add an isolated virtual-KWin render test that checks identity output,
  grayscale, boosted saturation, restoration, and zero-cost inactive state
  without touching the user's real desktop.
- Detect exact KWin effect ABI compatibility at runtime and during install.
  Release archives now include the effect source and rebuild it locally when
  their bundled plugin does not match the installed KWin version.
- Make installer upgrades unload Satur8 before replacing compositor code and
  remove stale per-user plugins that could shadow the system copy.
- Make profile enable/disable real and persistent across the GUI, CLI, core
  matcher, and daemon. Disabling an active profile restores the desktop
  immediately; CLI profile edits notify the running daemon.
- Write profile configuration atomically so an interrupted save cannot leave a
  truncated file.
- Restore display state before the tray exits, including SIGTERM and SIGHUP,
  instead of terminating with saturation still applied.
- Repair the isolated GUI and KWin integration harnesses and exercise CLI,
  launch-wrapper, daemon focus, profile toggling, and GUI rendering paths.

Thanks to @NasirA1991 for the fixes in #13, #14 and #15, and to @Ferziellee for
the black-screen report.

## v0.3.3 - 2026-06-26

Backend sweep plus a full code audit of the stateful paths.

- The daemon now follows focus on every backend, not just KWin, and restores
  your desktop colors when it is stopped or restarted. A failed apply or a
  compositor hiccup no longer leaves it convinced a profile is active when it
  isn't, and if no backend is ready at startup it keeps retrying instead of
  staying dead for the session.
- A broken `profiles.toml` no longer silently wipes your profiles: the daemon
  and GUI log the parse error and keep the last good set.
- On NVIDIA, leaving a game restores the Digital Vibrance you actually had
  (say, from nvidia-settings autostart) instead of resetting it to zero.
- Per-profile `outputs` now work: a profile listing specific monitors applies
  only to those. Applying to several monitors reports exactly which ones
  failed instead of claiming success if any single one worked.
- Closing the GUI window exits the app unless the tray is running to bring it
  back, instead of leaving an invisible process behind. Activity-log
  timestamps are local time now, not UTC.
- Profiles no longer match windows by profile name, so a profile called
  "Steam" can't recolor the Steam client itself.
- Plumbing with no user-visible surface: one shared backend selector across
  CLI/GUI/tray/daemon, a restore-on-drop guard on every apply path, a fixed
  DRM kernel-blob leak, gamescope property rollback on partial failure, and
  NaN-proof saturation parsing everywhere input comes in.
- Fix the nested gamescope fallback so its per-launch ReShade shader is written
  to the directory gamescope actually searches,
  `$HOME/.local/share/gamescope/reshade/Shaders`, instead of following
  `XDG_DATA_HOME`.
- Verify `satur8 run --via gamescope` on KDE Wayland / AMD RX 9070 XT by
  wrapping `glxgears` in nested gamescope and comparing `0.0` vs `4.0`
  screenshots.
- Improve the DRM-CTM backend's multi-GPU behavior by preferring a card with an
  active CTM-capable CRTC and by applying the shared `all` output target to all
  active CTM-capable CRTCs.
- Add DRM-CTM unit coverage for active CRTC detection, target resolution, and
  partial apply success/failure.
- Keep DRM/KMS listed as Implemented rather than Verified; runtime apply/off
  verification remains deferred.

## v0.3.2 - 2026-06-25

Hyprland linear-light release.

- Make `satur8 set --linear` apply before the Hyprland shader is written, so
  the requested linear-light mode affects the current command.
- Add a Hyprland linear-light shader path using the standard sRGB transfer
  functions instead of a simple gamma approximation.
- Keep the default Hyprland gamma-encoded shader path unchanged unless
  `--linear` is requested.
- Verify on AMD RX 9070 / Hyprland 0.55.4 that linear-light output differs from
  the gamma path as expected.

## v0.3.1 - 2026-06-25

GUI visual refresh to match satur8.app.

- Restyle the desktop app to the website's dark theme: ink background, cyan
  accent, and the spectrum gradient on the brand mark and primary buttons.
- Replace the old green and blue widgets with custom cyan saturation sliders and
  toggles.
- Add a spectrum accent line under the header and align card, text, and status
  colors to the site palette.
- No backend, daemon, or CLI behavior changes.

## v0.3.0 - 2026-06-25

Backend-verification work for gamescope, aimed at SteamOS and Bazzite.

- Add `satur8 run --via gamescope-native`, which drives the already-running
  gamescope compositor through its Xwayland root-window color atoms instead of
  launching a nested gamescope.
- Generate gamescope-format shaper and 3D LUT override files using Satur8's
  shared saturation matrix, so this path can represent the full Satur8 range
  including desaturation.
- Save and restore the previous gamescope LUT override/color-management
  properties around the launched game.
- Keep the existing nested `--via gamescope` ReShade fallback unchanged.

## v0.2.2 - 2026-06-24

Bug-fix release for the GNOME Wayland backend.

The GNOME Shell saturation extension was broken on GNOME 50: it loaded but never
changed the screen. This fixes it and marks GNOME Wayland verified on real
hardware.

- Fix the GNOME Shell backend for GNOME 50 / Mutter 18. The extension's
  shell-version range was too old, and Clutter dropped the `ShaderType` enum the
  effect relied on, so the shader was constructed wrong and never applied.
- Force the effect to repaint so a saturation change shows immediately on an
  idle desktop instead of waiting for the next redraw.
- Verified on GNOME Shell 50.2 (NVIDIA, Wayland) across the full saturation
  range.
- No changes to the Rust app, CLI, daemon, or KWin effect.

Install: extract `satur8-v0.2.2-linux-x86_64.tar.gz` below and run
`packaging/install.sh`, or use the Arch package / Fedora COPR (see the README).

## v0.2.1 - 2026-06-24

Bug-fix release for the NVIDIA X11 backend.

- Fix the NVIDIA NV-CONTROL backend to set the driver's real `DigitalVibrance`
  attribute. A rename had broken the attribute name, so the backend silently did
  nothing on NVIDIA + X11.
- Mark the NVIDIA X11 backend verified on real NVIDIA hardware.
- No changes to the other backends or the app itself.

Install: extract `satur8-v0.2.1-linux-x86_64.tar.gz` below and run
`packaging/install.sh`, or use the Arch package / Fedora COPR (see the README).

## v0.2.0 - 2026-06-24

Distribution release: Satur8 widens beyond Arch and hardens the release process.

- Add Fedora/RPM packaging (`packaging/satur8.spec`) and ship it live on COPR,
  built for Fedora 43 and 44.
- Add a tagged-release GitHub Actions workflow that builds the source and Linux
  tarballs with checksums, so the packaging sources and the website download
  stay in sync.
- Verify the KWin saturation effect in a live Fedora 44 KDE Plasma Wayland
  session on real hardware.
- Bump workspace and package metadata to v0.2.0.

Install: extract `satur8-v0.2.0-linux-x86_64.tar.gz` below and run
`packaging/install.sh`, or use the Arch package / Fedora COPR (see the README).

## v0.1.3 - 2026-06-24

The clean, Arch-ready packaging release.

- Publish a stable, reproducible v0.1.3 source archive for Arch packaging, plus
  the Linux x86_64 tarball and checksum.
- Update the Arch PKGBUILD and generated `.SRCINFO` to track the v0.1.3 source.
- Verify the Arch package with makepkg source verification and a clean chroot
  build.
- Document the honest package status: the Arch package is ready and tested; AUR
  publication is pending while new AUR account registration is disabled upstream,
  unless an existing AUR maintainer publishes it.
- Bump workspace and package metadata to v0.1.3.

Install: extract `satur8-v0.1.3-linux-x86_64.tar.gz` below and run
`packaging/install.sh`, or build the Arch package from `packaging/`.

## v0.1.2 - 2026-06-24

A small polish and packaging-validation release.

- Fix the release notes so GitHub renders proper Markdown bullets instead of
  escaped `\n` text.
- Split repo updates into focused commits so the GitHub file list is readable.
- Update public version labels to v0.1.2.

Install: extract `satur8-v0.1.2-linux-x86_64.tar.gz` below and run
`packaging/install.sh`, or build the Arch package from `packaging/`.

## v0.1.1 - 2026-06-24

A packaging-readiness patch on top of the first release.

- Add a scripted Linux x86_64 release tarball builder.
- Complete the Arch PKGBUILD metadata and generated `.SRCINFO` for the tagged
  source archive.
- Document the Arch install flow and refresh the roadmap and plan.
- Bump workspace crate versions and the GUI version label to v0.1.1.

Install: extract `satur8-v0.1.1-linux-x86_64.tar.gz` below and run
`packaging/install.sh`, or build the Arch package from `packaging/`.

## v0.1.0 - 2026-06-23

First public release of Satur8: per-game digital vibrance for Linux that boosts
color saturation while a game is focused and restores your desktop when you tab
out, without injecting anything into the game.

- CLI (`satur8 on/off/status/doctor`), a desktop GUI, a background daemon, and a
  system tray app.
- KDE Plasma Wayland backend via a KWin saturation effect, with a KWin focus
  script that auto-applies per-game profiles.
- Per-game profiles saved to `~/.config/satur8` and reloaded live by the daemon.
- Ships as a Linux x86_64 tarball with a one-command installer.

Install: extract `satur8-v0.1.0-linux-x86_64.tar.gz` below and run
`packaging/install.sh`.
