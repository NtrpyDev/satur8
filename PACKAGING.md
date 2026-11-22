# Satur8 Packager Guide

This guide is for people maintaining Satur8 packages or adding a new package
channel. User install commands live in [README.md](README.md); this file
explains what the packages must contain, how release artifacts are produced, and
which claims are honest today.

## Current Package Status

- Fedora: live on COPR as `ntrpydev/satur8`, built for Fedora 43, 44 and 45.
- Arch: `packaging/PKGBUILD` and generated `.SRCINFO` are ready and tested from
  a checkout.
- AUR: two packages are published and maintained by a community member, not by
  this project: `satur8` builds from the source release and `satur8-bin`
  installs prebuilt binaries. They can trail the latest release; both were at
  v0.3.3 when checked on 2026-11-21.
- Release tarball: GitHub release artifact for Linux x86_64, installed with
  `packaging/install.sh`. Since v0.4.0 this is also the install path on SteamOS
  (tested on a Steam Deck in Game Mode).
- Debian/Ubuntu, SteamOS, and Bazzite packages are not shipped yet. On
  Bazzite the release tarball's per-user installer is the intended path; the
  v0.4.0 installer and background service were run in a Bazzite container, not
  on a Bazzite device.

Use `vX.Y.Z` for public Satur8 releases. Only use Arch-style `X.Y.Z-1` when
talking about the Arch package build number.

## What A Package Must Install

Every native package should install the same runtime surface unless the target
platform cannot support part of it:

- `satur8`, `satur8-daemon`, `satur8-tray`, and `satur8-gui` binaries.
- KWin effect plugin, built against the target distro's KWin/Qt ABI.
- KWin effect source (`CMakeLists.txt`, `metadata.json`, `satur8_effect.cpp`,
  `satur8_effect.h`) in `/usr/share/satur8/kwin-effect/`, so `satur8 repair`
  can rebuild the effect after a KWin update.
- `packaging/60-satur8.conf` in `/usr/lib/environment.d/`, which puts the
  per-user Qt plugin directory on KWin's plugin search path.
- KWin focus-forwarder script from `assets/kwin-script/`.
- GNOME Shell extension from `assets/gnome-extension/`.
- gamescope ReShade fallback shader from `assets/gamescope/Satur8.fx`.
- systemd user unit from `packaging/satur8-daemon.service`, with both
  `%h/.local/bin/` paths rewritten to the packaged binary directory.
- desktop launcher from `packaging/satur8.desktop`.
- GPL license text.

The KWin effect ABI is not stable. KWin loads an effect only when its embedded
ID names KWin's exact x.y.z version, and Qt refuses one built against a newer
Qt minor than the installed one. So build the effect against the target's own
KWin and Qt packages, in the package build. Using an effect built on another
system is only a guarded fallback, as the release tarball's installer does it:
the KWin version must be equal and the installed Qt at least as new as the one
the effect needs.

The packaged `.so` goes stale the moment the distro updates KWin. The daemon's
unit runs `satur8 repair` at session start, which asks the running KWin whether
it accepts the effect and, when it does not, installs the matching effect from
the release tarball's set if one is installed (`~/.local/share/satur8/kwin-effects`),
else rebuilds it from the installed source into `~/.local/lib/qt6/plugins`. A package should therefore pull in the
effect's build toolchain at runtime as well: a hard dependency where the distro
has no weak ones (Arch), `Recommends` where it does (Fedora). The end-to-end
check for this path is `assets/kwin-effect/sandbox-repair-test.sh`.

The KWin effect is optional at install time. `packaging/install.sh` must finish
on a machine with no KWin at all (GNOME, Hyprland, NVIDIA X11, a gamescope
session) and simply leave the effect out. Before tagging a release, run
`packaging/install-nokwin-test.sh` on the release tarball: it runs the
installer in a clean container with no KWin, no cmake and no network.

## Release Artifacts

There are two tarballs, and they serve different jobs:

- `satur8-vX.Y.Z-source.tar.gz`: deterministic source archive used by Arch and
  Fedora packaging. Built by `packaging/source-tarball.sh`.
- `satur8-vX.Y.Z-linux-x86_64.tar.gz`: convenience binary tarball served by the
  website and GitHub release. Built by `packaging/release-tarball.sh`.

The source archive intentionally excludes downstream package metadata:
`packaging/PKGBUILD`, `packaging/.SRCINFO`, and `packaging/satur8.spec`. That
keeps package metadata edits from changing the source checksum they consume.

## Release Flow

For normal Satur8 releases, use:

```sh
scripts/release.sh X.Y.Z
```

Before running it:

- The working tree should be clean except for the new `CHANGELOG.md` entry.
- `CHANGELOG.md` must contain a `## vX.Y.Z - DATE` section.
- You need `makepkg` available for regenerating `.SRCINFO`.
- You need `copr-cli` configured if you want the script to queue the COPR build.

The script does the release mechanics in order:

1. Bumps the workspace version in `Cargo.toml`.
2. Runs `cargo check --workspace` to refresh `Cargo.lock` package versions.
3. Builds the deterministic source archive and reads its SHA-256.
4. Updates `packaging/PKGBUILD`, `packaging/.SRCINFO`, and
   `packaging/satur8.spec`.
5. Rebuilds the source archive and confirms the checksum is reproducible.
6. Commits the manifest, lockfile, packaging metadata, and changelog as focused
   commits.
7. Tags `vX.Y.Z` and pushes `main` and the tag.
8. Queues the Fedora COPR build from the tag.

The tag push triggers `.github/workflows/release.yml`, which creates or updates
the GitHub release and attaches the source archive, binary tarball, and SHA-256
files.

After the workflow finishes, update the website download in the `satur8-site`
repo so satur8.app points at the new binary tarball.

## Arch Package

Arch packaging lives in `packaging/PKGBUILD` and `packaging/.SRCINFO`.

Local smoke test:

```sh
cd packaging
makepkg -si
```

Stronger validation, when an Arch clean chroot is available:

```sh
extra-x86_64-build
```

Keep these points in sync:

- `pkgver` matches the Satur8 release version without the leading `v`.
- `pkgrel` starts at `1` for each new Satur8 version.
- `source` points at the GitHub `vX.Y.Z` source archive.
- `sha256sums` matches the generated source archive checksum.
- `.SRCINFO` is regenerated after any PKGBUILD metadata change.

Arch package status wording must stay precise: "Arch package ready/tested" means
the package files build and install locally or in a clean chroot. "AUR
published" means it is live on aur.archlinux.org.

## Fedora COPR Package

Fedora packaging lives in `packaging/satur8.spec`; the COPR source RPM entry
point is `.copr/Makefile`.

COPR builds from the release tag with the `make_srpm` method. The makefile
builds the same deterministic source archive, then uses the spec to produce the
source RPM. COPR's Fedora chroots have network access, so the Rust crates are
fetched during the build.

For official Fedora packaging later, expect extra work: Fedora Koji builds are
offline, so Rust crates would need to be vendored or packaged through Fedora's
Rust packaging process.

When changing the spec:

- Keep `Version:` aligned with the Satur8 release.
- Keep `Source0` pointed at the GitHub source archive.
- Keep `BuildRequires` in sync with what the Rust workspace, Slint GUI, and KWin
  effect need in Fedora chroots.
- Keep weak dependencies honest: desktop-specific backends should not become
  hard dependencies unless Satur8 cannot run without them.
- Add changelog entries only for package-relevant changes.

## Release Tarball

The binary release tarball is for users who want a direct download rather than a
distro package. It includes prebuilt Linux x86_64 binaries, a set of prebuilt
KWin effects, the installer, and the runtime assets.

- **Binaries.** `packaging/build-portable.sh OUT_DIR` builds the four programs
  in an Ubuntu 22.04 container with a pinned Rust toolchain, so they need glibc
  2.35 or newer. It ends with `packaging/check-portability.sh`, which fails
  when a program asks for a newer glibc symbol version or links a library
  outside the allowed list. `packaging/portability-test.sh` then starts every
  program in containers of Ubuntu 22.04, Debian 12, Ubuntu 24.04, openSUSE Leap
  16.0 and Fedora 43, the desktop app under a virtual display; with
  `--negative-control` it shows the check catches the problem, using the
  v0.3.6 tarball. These are container checks, not tests on hardware.
- **KWin effects.** `packaging/build-kwin-effect.sh` builds one effect per line
  of `packaging/kwin-effect-targets.txt`, inside that system's own packages:
  SteamOS branches against Valve's package mirror, Fedora releases in a Fedora
  container. Each lands in `<key>/satur8.so` with a `build-info.txt`, where the
  key is `steamos-<kwin>` or `fedora<NN>-<kwin>`; when two targets give the
  same key, the first wins. The tarball carries them in
  `prebuilt/linux-x86_64/kwin/effects/<key>/`.
- **Assembly.** `packaging/release-tarball.sh --bin-dir DIR --effects-dir DIR`
  puts the portable binaries and the effect set into the archive. Without
  `--effects-dir` it builds one effect on this machine, keyed for its family and
  KWin version.
- **Installer tests.** `packaging/install-prebuilt-effect-test.sh` checks
  which effect the installer picks on faked SteamOS and Bazzite systems with no
  build tools, and `packaging/install-nokwin-test.sh` checks an install on a
  system with no KWin.

To rebuild locally on Linux x86_64 (needs podman or docker):

```sh
packaging/build-portable.sh target/portable-bin
packaging/build-kwin-effect.sh --all target/effects
packaging/release-tarball.sh --bin-dir target/portable-bin --effects-dir target/effects
packaging/portability-test.sh target/dist/satur8-vX.Y.Z-linux-x86_64.tar.gz
```

On GitHub, `.github/workflows/build-tarball.yml` builds the portable binaries
and the effect set, assembles the tarball, and runs the installer and
portability tests on it; `release.yml` publishes the result.

`install-prebuilt-effect-test.sh` names the keys `steamos-6.7.3` and
`fedora45-6.8.3`. When those targets move to a new KWin version, update the
test's keys, KWin versions and Qt library names with them.

## Verification Expectations

Before claiming a package channel works:

- Build the package or artifact from a clean checkout or clean chroot.
- Install it through the package manager or installer users will actually run.
- Run `satur8 doctor`.
- Verify at least one backend that the package claims as supported.
- Confirm uninstall or package removal leaves no surprising user-facing pieces.

Do not document an install path as done until users can install from it. For
example, the Arch package files in `packaging/` can be ready/tested before any
AUR package for that version exists.

## Adding A New Package Channel

Start by matching the existing native packages. The package should build from
the deterministic source archive, install the same runtime files, and document
any target-specific limitations in plain language.

For immutable or gaming-focused distros, do not promise support before hardware
testing, because they depend on gamescope behavior on real systems, not just on
the package format. SteamOS is supported since v0.4.0 through the release
tarball's per-user installer, tested on a Steam Deck in Game Mode. Bazzite has
not been tested on real hardware.

Bazzite specifics (stable 45.20261117.1, checked 2026-11-21 from source and in a
container, not on a Bazzite device):

- It is Fedora 45 based. The COPR project builds Fedora 45 too. Layering that
  package on Bazzite has not been tested, and Bazzite's documentation calls
  `rpm-ostree` layering a last resort.
- `/usr` is read-only, so `install.sh --system` cannot work there and the
  installer refuses it; the per-user install writes only under `$HOME`, which
  persists.
- The images ship no `cmake` and no KWin development headers, so the KWin
  effect cannot be rebuilt on the machine after a KWin update. Since v0.4.1 the
  tarball carries `fedora45-6.8.3`, built in Fedora 45's packages, and the
  installer picked it in the Bazzite container. After a KWin update the effect
  stays off until a newer Satur8 release includes that KWin version.
- The GNOME images run GNOME 51, which the GNOME Shell extension's metadata does
  not list yet.
