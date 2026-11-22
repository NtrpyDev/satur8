# Satur8 Roadmap

Where Satur8 goes after v0.1. This is the living "what ships next and in what
order" document. For *why it is built this way* see [PLAN.md](PLAN.md) (design
and backend architecture); for *how to install and use it* see
[README.md](README.md).

## Where the project stands

v0.4.1 is the current release. Seven of the eight backend paths are verified on
real hardware: KDE Plasma Wayland/KWin, GNOME Wayland Shell extension, Hyprland,
NVIDIA X11 NV-CONTROL, automatic per-game switching in Steam Game Mode through
gamescope's look table (verified on a Steam Deck OLED with SteamOS 3.9.5 and
gamescope 3.16.30.1), the gamescope-native launch option for a running
full-screen gamescope compositor, and the nested gamescope fallback for
unsupported Wayland sessions. The remaining implemented-but-unverified path is
DRM/KMS. Since v0.4.1 the release tarball needs glibc 2.35 or newer (checked
by starting each program in containers of Ubuntu 22.04, Debian 12, Ubuntu
24.04, openSUSE Leap 16.0 and Fedora 43) and carries prebuilt KWin effects for
SteamOS 3.9 (KWin 6.7.3), SteamOS 3.10 (KWin 6.8.2), and Fedora 44 and 45
(KWin 6.8.3), each built in that system's own packages. `satur8 repair` installs
the matching effect from that set at login, and rebuilds it only where build
tools exist. The Steam Deck's Desktop Mode is verified with the SteamOS 3.9
effect. Distribution today is a Linux x86_64 tarball served from
[satur8.app](https://satur8.app), a tested Arch package in `packaging/`, and a
live Fedora package on COPR built for Fedora 43, 44 and 45. The AUR also has two packages, `satur8` (built from the source release)
and `satur8-bin` (prebuilt binaries), published and maintained by a community
member rather than by this project. They were at v0.3.3 when checked on
2026-11-21, so they can trail the latest release.

The north star is **v1.0 = Satur8 running well on SteamOS and Bazzite**. Both run
gamescope rather than KWin. The gamescope path was proven in v0.3, and v0.4.0
landed automatic per-game switching in Steam Game Mode and verified it on a
Steam Deck. The next work is Bazzite and other gamescope-session systems on real
hardware and the remaining Steam Deck items, then stamping 1.0 once it is
proven. Broader desktop reach
(Debian/Ubuntu, non-gaming uses) comes after 1.0.

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
- [x] Publish to the AUR. Done by an existing AUR maintainer from the
      community: `satur8` builds from the tagged source release and
      `satur8-bin` installs prebuilt binaries. This project does not maintain
      them, so they can trail the latest release (v0.3.3 when checked on
      2026-11-21).
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

- [x] **Verify the gamescope path (priority).** Done: the gamescope-native
      backend (drives the *running* compositor via its Xwayland color atoms,
      added in v0.3.0) was verified on a real full-screen gamescope session
      (NVIDIA, glxgears) where the saturation visibly changed across the range.
      The nested ReShade fallback (`satur8 run --via gamescope`) was then
      verified on KDE Wayland with AMD RX 9070 XT by wrapping `glxgears` in
      nested gamescope; a `0.0` / `4.0` screenshot pair changed mean HLS
      saturation from 0.086 to 0.797 after fixing the runtime shader install
      path to match gamescope's `$HOME/.local/share/gamescope/reshade/Shaders`
      search path. Real Steam Deck / Bazzite field-testing is the separate v0.4
      work below.
- [x] Verify the Hyprland backend. Done on PC1 (AMD RX 9070): satur8 selected
      the hyprland backend and set `decoration:screen_shader`; a headless
      Hyprland capture measured mean saturation dropping 0.975 -> 0.244 at
      satur8 0.2, confirming the screen-shader pass applies and changes output
      saturation.
- [x] Finish the DRM-CTM implementation sweep without runtime verification:
      multi-GPU active-card selection and all-active-output fan-out are
      implemented and unit-tested; the read-only probe already passes (see
      PLAN.md section 10), but real apply/off verification is deferred and the
      backend remains honestly listed as Implemented.
- [x] Verify the GNOME Wayland Shell-extension backend on real GNOME (GNOME
      50.2, NVIDIA Wayland). Required fixes: shell-version range and the
      Clutter.ShaderType enum removed in Mutter 18.
- [x] Verify the NVIDIA X11 NV-CONTROL backend on real NVIDIA hardware.
- [x] Measure KWin effect in-game cost at 1440p / 240Hz in CS2. Done on PC1
      with CS2 FPS BENCHMARK DUST2 / `de_dust2`, DP-2 at 2560x1440 @ 239.998
      Hz, KDE Plasma Wayland / KWin 6.7.0, AMD Radeon RX 9070 XT, and Satur8's
      KWin effect at 1.75. Seven alternating scored pairs were saved under
      `benchmarks/cs2-kwin-20260626-144823/`. All-run medians were OFF Avg/P1 =
      393.0/162.1 FPS and ON Avg/P1 = 399.8/162.4 FPS; clean-run sensitivity
      was OFF 409.4/166.0 FPS vs ON 402.4/165.0 FPS; paired median delta was
      -1.5% Avg / +0.038 ms/frame. Treat the result as within run-to-run noise,
      not a precise zero-cost claim.
- [x] Update the backend status table on the website. Done: satur8.app shows
      six backends Verified and DRM/KMS Implemented, matching the README table.

## v0.4: SteamOS / Bazzite support

The north star. A per-game vibrance tool is tailor-made for a Steam Deck, and
this is where that support lands and gets tested in the wild before the stable
stamp. Gated on the gamescope path being proven in v0.3. The packaging groundwork
is already done: SteamOS is Arch-based (the tested Arch package is the on-ramp)
and Bazzite is Fedora-based (the live COPR work carries over).

- [x] Build a gamescope-native backend that drives the running compositor's color
      saturation per-game at runtime (zero extra pass, no injection), instead of
      the nested-gamescope reshade fallback. Landed in v0.3.0 and verified on a
      full-screen gamescope session. It is still the `satur8 run --via
      gamescope-native` launch option and still replaces Steam's color settings
      while the game runs.
- [x] Per-game auto-apply in Game Mode, from the same Satur8 profiles used on
      the desktop. Done in v0.4.0: the focus daemon follows gamescope's focused
      app and sets gamescope's look table, so Steam's own vibrancy and color
      temperature keep working on top. Verified on a Steam Deck OLED (SteamOS
      3.9.5, gamescope 3.16.30.1) with a camera on the screen. vibrantDeck, an
      existing Decky plugin, already applies per-app saturation in Game Mode, so
      this is not unique to Satur8.
- [x] SteamOS: install story on a read-only root. Done in v0.4.0 without a Decky
      plugin: the release tarball's per-user installer puts everything in the
      home folder, which survives SteamOS updates, `--enable-daemon` turns on
      the background service, and `--system` is refused on SteamOS. The
      service starts with the Game Mode session; this was checked by switching
      the Deck to Desktop Mode and back, and a cold boot was not tested.
      Uninstall was checked on the Deck and keeps `profiles.toml`.
- [ ] Bazzite (atomic Fedora). Checked 2026-11-21 against Bazzite stable
      45.20261117.1, from source and in a container, not on a Bazzite device:
      Game Mode runs the OpenGamingCollective gamescope fork 3.17.0-ogc1 with
      the same look table, focus and server-id properties as the Deck's
      3.16.30.1, and the per-user tarball installer works unchanged. Bazzite
      Game Mode is Implemented, not Verified.
  - [x] Container check: the v0.3.5 per-user installer installs and uninstalls
        cleanly on the `bazzite-deck` image and uses the bundled KWin 6.8.3
        effect.
  - [x] Container check: Bazzite's gamescope, run headless with `--steam`,
        publishes the focus and server-id properties and applies a Satur8
        look table (grayscale and 2x confirmed in screenshots).
  - [x] Container check with v0.4.0 itself: the v0.4.0 tarball installs per
        user on the `bazzite-deck` image; the v0.4.0 background service, with
        no `DISPLAY` set, finds Bazzite's gamescope, loads a profile's table
        on the first X server only when that app takes focus, removes it at the
        home window, applies it again after being killed and restarted, and
        removes it when stopped; `satur8 doctor` prints the Game Mode report;
        uninstall leaves no table.
  - [x] Installer refuses `--system` on image-based systems (read-only `/usr`).
        Done in v0.4.0.
  - [ ] Game Mode on a real Bazzite device with an AMD GPU: the same background
        service checks that were run on the Steam Deck for v0.4.0.
  - [ ] KDE Desktop Mode: keep the KWin effect working across KWin updates on
        an image with no `cmake` or KWin headers. Partly done in v0.4.1: the
        tarball bundles `fedora45-6.8.3`, built in Fedora 45's packages for
        the KWin Bazzite stable uses, and the installer picked it in the
        Bazzite container (not run on a Bazzite desktop). After a KWin update
        the effect stays off until a newer Satur8 release includes that KWin.
        Still open, and waiting on the owner's decision: whether Satur8 may
        fetch an effect for a newer KWin when the user asks for it with a
        command. Nothing for that is built.
  - [x] GNOME images: add GNOME 51 to the Shell extension. Checked on GNOME
        Shell 51.1 headless in a Fedora 45 container with software rendering,
        not on a Bazzite device or a GNOME 51 desktop. GNOME 51 removed the
        shader API the extension used, so on GNOME 51 the effect uses a Cogl
        snippet instead. Screenshots at 2.0, 0 and off matched the saturation
        math pixel for pixel, and the same steps passed on GNOME Shell 50.6
        and 49.10.
  - [ ] Fedora 45 COPR package: the build target was added on 2026-11-22.
        Still open: test layering it on Bazzite with `rpm-ostree` and decide
        whether to document that.
  - [ ] NVIDIA and Intel Game Mode: check on hardware whether the look applies
        when gamescope scans a game out directly.
- [ ] Document clearly what works and what does not on each. SteamOS is done in
      the README's Steam Deck section (v0.4.0). The README's Bazzite section
      states what was checked and the known gaps; it stays open until a
      Bazzite device test.

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

- [x] Linear-light blend option (more correct than the gamma-encoded blend).
      Implemented for the Hyprland backend and verified on PC1 / Glorgy
      (AMD RX 9070, Hyprland 0.55.4): at saturation 0.0, gamma output sampled
      red as `(54,54,54)` while linear-light output sampled red as
      `(127,127,127)`, matching the expected sRGB transfer-function path.
- [ ] Multi-monitor and per-output profiles. Implemented on KDE Plasma, not
      yet released: a value per output, game profiles limited to named
      outputs or to the output the game is on, and reports on every other
      desktop that one value applies to every output. Checked on headless KWin
      with two virtual outputs, in pixels on KWin 6.7.3 and as the effect's
      reported state on KWin 6.8.2. Not yet run on real monitors.
- [x] Steam Deck: pick a game from a list of installed Steam games instead of
      typing its AppID, on the command line: `satur8 games` lists them and
      `satur8 profile add NAME SATURATION --steam-game NAME` adds a profile by
      the game's name. Done in v0.4.0 and run on a Steam Deck.
- [ ] Steam Deck: the same installed-games picker in the desktop app.
- [ ] Steam Deck: the desktop app in Game Mode.
- [ ] Steam Deck: click through the desktop app in Desktop Mode. In v0.4.1 it
      starts there and shows the KWin backend active; nothing more was run.
- [ ] Steam Deck: a Decky panel.
- [ ] Steam Deck: HDR games, through gamescope's separate HDR look table. The
      v0.4.0 look table only changes SDR content. First test with a game that
      presents HDR; so far only a game presenting SDR was run (ELDEN RING
      through Proton, where the look applied).
- [ ] Steam Deck: a cold boot straight into Game Mode, to confirm the service
      starts then too.
- [ ] Steam Deck: run the `steamos-6.8.2` effect on a Deck with SteamOS 3.10.
- [x] Steam Deck: make the KWin effect available on SteamOS Desktop Mode.
      Done in v0.4.1: the tarball bundles effects built against Valve's
      packages for SteamOS 3.9 (KWin 6.7.3) and SteamOS 3.10 (KWin 6.8.2), and
      the installer picks the one for the Deck. Verified on a Steam Deck OLED
      with SteamOS 3.9.5 and KWin 6.7.3: after one switch to Desktop Mode
      `satur8 on` and `off` changed the picture, per-game switching followed
      window focus with the KWin focus script on, and `satur8 repair` replaced
      a wrong effect file from the bundled set.
- [ ] Re-check whether a future KWin release exposes a client CTM path that would
      make the KDE backend zero-cost (none in KWin 6.7, see PLAN.md section 10).
- [x] `CONTRIBUTING.md`, issue and PR templates, and the dedicated
      [packager guide](PACKAGING.md).
- [x] A documented release process: `scripts/release.sh` automates the version
      bump, packaging metadata + reproducible checksum, focused commits, tag and
      push (which triggers the CI artifacts), and the COPR build. Refreshing the
      website download in the satur8-site repo is still a manual follow-up.

---

## Why not just one Flatpak?

Flatpak is the usual "package once, run everywhere including atomic distros"
answer, but it does not fit Satur8. Satur8 is a system-integration tool: it
installs a KWin compositor plugin, a systemd user unit, a KWin focus script, and
talks D-Bus to the compositor. A Flatpak sandbox cannot install a compositor
plugin where KWin scans for it. That is why native per-distro packaging is the
primary path, and it is the deeper reason the immutable targets (v0.4) are
genuinely hard rather than a quick Flatpak away.
