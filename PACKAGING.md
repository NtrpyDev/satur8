# Satur8 Packager Guide

This guide is for people maintaining Satur8 packages or adding a new package
channel. User install commands live in [README.md](README.md); this file
explains what the packages must contain, how release artifacts are produced, and
which claims are honest today.

## Current Package Status

- Fedora: live on COPR as `ntrpydev/satur8`, built for Fedora 43 and 44.
- Arch: `packaging/PKGBUILD` and generated `.SRCINFO` are ready and tested from
  a checkout. AUR publication is pending because new AUR account registration is
  disabled upstream, unless an existing AUR maintainer publishes it.
- Release tarball: GitHub release artifact for Linux x86_64, installed with
  `packaging/install.sh`.
- Debian/Ubuntu, SteamOS, and Bazzite packages are not shipped yet.

Use `vX.Y.Z` for public Satur8 releases. Only use Arch-style `X.Y.Z-1` when
talking about the Arch package build number.

## What A Package Must Install

Every native package should install the same runtime surface unless the target
platform cannot support part of it:

- `satur8`, `satur8-daemon`, `satur8-tray`, and `satur8-gui` binaries.
- KWin effect plugin, built against the target distro's KWin/Qt ABI.
- KWin focus-forwarder script from `assets/kwin-script/`.
- GNOME Shell extension from `assets/gnome-extension/`.
- gamescope ReShade fallback shader from `assets/gamescope/Satur8.fx`.
- systemd user unit from `packaging/satur8-daemon.service`, rewritten to point
  at the packaged binary path.
- desktop launcher from `packaging/satur8.desktop`.
- GPL license text.

The KWin effect ABI is not stable. Do not reuse a KWin `.so` built on another
distro or another incompatible KWin/Qt version; build it in the package build.

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
distro package. It includes prebuilt Linux x86_64 binaries, the KWin effect, the
installer, and the runtime assets.

Build locally on Linux x86_64:

```sh
packaging/release-tarball.sh
```

The CI workflow builds the same tarball inside Fedora so dependencies are
repeatable and close to the Fedora package environment.

## Verification Expectations

Before claiming a package channel works:

- Build the package or artifact from a clean checkout or clean chroot.
- Install it through the package manager or installer users will actually run.
- Run `satur8 doctor`.
- Verify at least one backend that the package claims as supported.
- Confirm uninstall or package removal leaves no surprising user-facing pieces.

Do not document an install path as done until users can install from it. For
example, the Arch package can be ready/tested while AUR publication remains
blocked.

## Adding A New Package Channel

Start by matching the existing native packages. The package should build from
the deterministic source archive, install the same runtime files, and document
any target-specific limitations in plain language.

For immutable or gaming-focused distros, do not promise support before hardware
testing. SteamOS and Bazzite are the v0.4 target because they depend on
gamescope behavior on real systems, not just on the package format.
