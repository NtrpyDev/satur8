# Satur8 Roadmap

Where Satur8 goes after v0.1. This is the living "what ships next and in what
order" document. For *why it is built this way* see [PLAN.md](PLAN.md) (design
and backend architecture); for *how to install and use it* see
[README.md](README.md).

## Where the project stands

v0.2.2 is shipped and working. KDE Plasma Wayland is the verified target, with
the CLI, GUI, daemon, tray, KWin saturation effect, and KWin focus script all
working together. NVIDIA X11 NV-CONTROL and the GNOME Wayland Shell-extension
backend are also verified on real hardware (the latter on GNOME 50.2, NVIDIA
Wayland). Distribution today is a Linux x86_64 tarball served from
[satur8.app](https://satur8.app), a tested Arch package, and a live Fedora
package on COPR built for Fedora 43 and 44. AUR publication is prepared but
blocked while new AUR account registration is disabled upstream, unless an
existing AUR maintainer publishes the package. The remaining backends
(Hyprland, DRM/KMS, gamescope) are implemented behind environment detection but
not yet independently verified.

The north star is **v1.0 = Satur8 running well on SteamOS and Bazzite**. Both run
gamescope rather than KWin, so everything between here and 1.0 is sequenced to
de-risk and reach that: prove the gamescope path (v0.3), land and field-test
Deck/Bazzite support (v0.4), then stamp 1.0 once it is proven. Broader desktop
reach (Debian/Ubuntu, non-gaming uses) comes after 1.0.

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
- [x] COPR project live with auto-builds, publishing `satur8` for Fedora 43 and
      44 (https://copr.fedorainfracloud.org/coprs/ntrpydev/satur8/). The first
      build also confirmed the `BuildRequires` list on a real Fedora chroot.
- [x] Visually verified the KWin saturation effect in a live Fedora KDE Plasma
      Wayland session on real hardware (Fedora 44 KDE): COPR install, `satur8
      doctor` detection, KWin over D-Bus, and `satur8 on 1.75` boosting all
      outputs via the kwin backend. Note: on a pre-release Fedora the prebuilt
      effect needs the system fully updated first, because Fedora bumped Qt
      (6.11.0 -> 6.11.1) out from under it; the unstable KWin effect ABI means
      the effect must match the running KWin/Qt.
- [x] GitHub Actions: on a `v*` tag, build the source and Linux tarballs with
      `.sha256` so packaging sources stay trustworthy and the site download
      stays in sync (`.github/workflows/release.yml`). Verified on the first
      tagged run.
- [x] Docs: "Install on Fedora (COPR)" in the README.

## v0.3: Backend verification sweep

Turn the backend table's "implemented" rows into "verified". A credibility
milestone, and the place we de-risk the 1.0 north star: SteamOS and Bazzite both
run gamescope, so proving the gamescope path here is the priority, ahead of the
other backends.

- [ ] **Verify the gamescope path (priority).** Confirm satur8 can drive a live
      gamescope session's color saturation. This is the load-bearing assumption
      for the SteamOS/Bazzite 1.0 target, so it gets verified first. The current
      gamescope backend wraps a *nested* gamescope and bakes the value at launch;
      the Deck needs driving the *running* compositor's native saturation at
      runtime.
- [ ] Verify the Hyprland backend.
- [ ] Verify DRM-CTM on X11 / TTY for AMD and Intel (the read-only probe already
      passes, see PLAN.md section 10). Also relevant to the Deck, an AMD APU.
- [x] Verify the GNOME Wayland Shell-extension backend on real GNOME (GNOME
      50.2, NVIDIA Wayland). Required fixes: shell-version range and the
      Clutter.ShaderType enum removed in Mutter 18.
- [x] Verify the NVIDIA X11 NV-CONTROL backend on real NVIDIA hardware.
- [ ] Measure KWin effect in-game cost at 1440p / 240Hz in CS2 (PLAN.md open
      item).
- [ ] Update the backend status table in the README and on the website to match
      reality.

## v0.4: SteamOS / Bazzite support

The north star. A per-game vibrance tool is tailor-made for a Steam Deck, and
this is where that support lands and gets tested in the wild before the stable
stamp. Gated on the gamescope path being proven in v0.3. The packaging groundwork
is already done: SteamOS is Arch-based (the tested Arch package is the on-ramp)
and Bazzite is Fedora-based (the live COPR work carries over).

- [ ] Build a gamescope-native backend that drives the running compositor's color
      saturation per-game at runtime (zero extra pass, no injection), instead of
      the nested-gamescope reshade fallback.
- [ ] Per-game auto-apply in Game Mode. This is the differentiator versus the
      existing global-saturation tools (e.g. vibrantDeck).
- [ ] SteamOS: install story on a read-only root (a Decky plugin is the likely
      path); ship only honest, tested docs.
- [ ] Bazzite (atomic Fedora): evaluate rpm-ostree layering versus a Decky/
      Flatpak split.
- [ ] Document clearly what works and what does not on each.

## v1.0: Stable on the north star

1.0 is a stability stamp, not a feature drop. It means the SteamOS and Bazzite
support from v0.4 has been proven on real hardware and the bugs found there are
fixed. Shipping Deck/Bazzite as a pre-1.0 version first is deliberate: it shakes
out the hard target before the 1.0 label goes on it.

- [ ] Shake out the v0.4 Deck/Bazzite bugs on real hardware.
- [ ] Lock the supported backend matrix and only the install stories that are
      actually tested.
- [ ] Tag v1.0.0.

## Post-1.0: broaden beyond gamers

Once the beachhead is solid, widen reach. Per-app vibrance has uses beyond
gaming, but this is demand-pull: do it when the proven Deck/enthusiast audience
is served and there is real pull for more, not on speculation.

- [ ] Debian / Ubuntu: `.deb` packaging plus a Launchpad PPA; handle the older
      Qt6 / KWin skew on Ubuntu LTS; pick a minimum supported release; Mint and
      Pop!_OS notes.
- [ ] Evaluate non-gaming use cases (photo/video editing, accessibility) if the
      demand shows up.

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
