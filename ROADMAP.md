# Satur8 Roadmap

Where Satur8 goes after v0.1. This is the living "what ships next and in what
order" document. For *why it is built this way* see [PLAN.md](PLAN.md) (design
and backend architecture); for *how to install and use it* see
[README.md](README.md).

## Where v0.1 stands

v0.1.0 is shipped and working. KDE Plasma Wayland is the verified target, with
the CLI, GUI, daemon, tray, KWin saturation effect, and KWin focus script all
working together. Distribution today is a single Linux x86_64 tarball served
from [satur8.app](https://satur8.app), plus a tested Arch package. AUR
publication is prepared but blocked while new AUR account registration is
disabled upstream, unless an existing AUR maintainer publishes the package.
v0.2 adds Fedora/RPM packaging for COPR and a tagged-release CI workflow (see
below). Every other backend (GNOME, Hyprland, NVIDIA X11, DRM/KMS, gamescope) is
implemented behind environment detection but not yet independently verified.

## Guiding principle

Ship where it is cheap and where the audience already is, widen reach as the
packaging matures, and save the integration-heavy and immutable targets for when
the backend they depend on is actually proven. Credibility is the scarce
resource for a young niche tool: never document an install path that has not been
tested honestly.

Version labels below are target tags, not dated commitments.

---

## v0.1.x: Arch / AUR

Cheapest channel to ship, and the best audience fit (Arch and its derivatives,
including CachyOS, skew heavily toward enthusiast Linux gamers).

- [x] Finish `packaging/PKGBUILD`: replace `sha256sums=('SKIP')` with the real
      checksum of the release source archive; reconfirm `makedepends` /
      `optdepends`.
- [x] Generate `.SRCINFO`.
- [x] Clean-chroot build test (`extra-x86_64-build` / `makechroot`).
- [ ] Publish to the AUR as the tagged `satur8` package once new AUR account
      registration reopens or an existing AUR maintainer can push it.
- [ ] Consider a `satur8-git` variant for bleeding-edge users.
- [x] Add an "Install on Arch" section to the README.
- [x] Release tarball + checksum, R2 download worker, and website (done in v0.1).

## v0.2: Fedora / RPM + release CI

Second-biggest desktop and gaming audience. COPR is the practical path (an
auto-building hosted repo, the Fedora equivalent of the AUR).

- [x] Write the RPM `.spec` (mirrors the PKGBUILD build: `cargo build --release`
      plus the cmake KWin effect, then installs the same components into the
      Fedora system paths). Lives in `packaging/satur8.spec`.
- [x] COPR build entry point (`.copr/Makefile`, "make srpm" method) and setup
      notes (`packaging/copr/README.md`) so creating the project is one step.
- [ ] Create the COPR project under a Fedora account and run the first build.
      Blocked on a Fedora Account System login, the same shape of blocker as the
      AUR. The first build is also what verifies the `BuildRequires` list on a
      real Fedora chroot.
- [ ] Test on Fedora Workstation and the KDE spin (needs real Fedora hardware).
- [x] GitHub Actions: on a `v*` tag, build the source and Linux tarballs with
      `.sha256` so packaging sources stay trustworthy and the site download
      stays in sync (`.github/workflows/release.yml`). Verified on the first
      tagged run.
- [x] Docs: "Install on Fedora (COPR)" in the README.

## v0.3: Backend verification sweep

Turn the backend table's "implemented" rows into "verified". This is a
credibility milestone and a prerequisite for the Steam Deck work. It can run in
parallel with v0.4.

- [ ] Verify the GNOME Wayland Shell-extension backend on real GNOME.
- [ ] Verify the Hyprland backend.
- [ ] Verify the NVIDIA X11 NV-CONTROL backend (needs NVIDIA hardware).
- [ ] Verify DRM-CTM on X11 / TTY for AMD and Intel (the read-only probe already
      passes, see PLAN.md section 10).
- [ ] Measure gamescope fallback quality and added latency (PLAN.md open item).
- [ ] Measure KWin effect in-game cost at 1440p / 240Hz in CS2 (PLAN.md open
      item).
- [ ] Update the backend status table in the README and on the website to match
      reality.

## v0.4: Debian / Ubuntu

Largest raw install base, more packaging friction and older libraries.

- [ ] `.deb` packaging plus a Launchpad PPA.
- [ ] Handle older Qt6 / KWin library skew on Ubuntu LTS; pick the minimum
      supported release.
- [ ] Test on Ubuntu LTS with KDE.
- [ ] Add Mint and Pop!_OS notes.

## v0.5: SteamOS / Bazzite

Highest-value target conceptually (a per-game vibrance tool is tailor-made for a
Steam Deck) and the hardest. Gated on v0.3, because SteamOS runs gamescope, not
KWin, so the gamescope backend must be verified first.

- [ ] SteamOS: work out the install story on a read-only root (a Decky plugin or
      a layered install); ship only honest, tested docs.
- [ ] Bazzite (atomic Fedora): evaluate rpm-ostree layering versus Flatpak for
      the parts that can live in a Flatpak.
- [ ] Document clearly what works and what does not on each.

## Ongoing: features and project health

Not blocking the packaging spine; picked up as time allows.

- [ ] Linear-light blend option (more correct than the gamma-encoded blend).
- [ ] Multi-monitor and per-output profiles.
- [ ] Re-check whether a future KWin release exposes a client CTM path that would
      make the KDE backend zero-cost (none in KWin 6.7, see PLAN.md section 10).
- [ ] `CONTRIBUTING.md`, issue and PR templates, and a packager guide.
- [ ] A documented release process: tag, CI artifacts, update AUR and COPR,
      refresh the site download.

---

## Why not just one Flatpak?

Flatpak is the usual "package once, run everywhere including atomic distros"
answer, but it does not fit Satur8. Satur8 is a system-integration tool: it
installs a KWin compositor plugin, a systemd user unit, a KWin focus script, and
talks D-Bus to the compositor. A Flatpak sandbox cannot install a compositor
plugin where KWin scans for it. That is why native per-distro packaging is the
primary path, and it is the deeper reason the immutable targets (v0.5) are
genuinely hard rather than a quick Flatpak away.
