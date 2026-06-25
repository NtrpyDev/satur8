# Changelog

All notable changes to Satur8, newest first. Each entry is also published as the
GitHub release notes for that tag.

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
