# Satur8 for Linux - Design Notes

> A VibranceGUI-style digital vibrance tool for Linux. Per-game saturation
> boost, applied **outside** the game process so it cannot trip anti-cheat,
> with as close to **zero CPU cost** as the hardware allows.

Status: v0.4.1 is the current release. KDE Plasma Wayland (including the
Steam Deck's Desktop Mode on SteamOS 3.9.5 with KWin 6.7.3), GNOME Wayland,
Hyprland, NVIDIA X11 NV-CONTROL, automatic per-game switching in Steam Game Mode
(verified on a Steam Deck OLED with SteamOS 3.9.5 and gamescope 3.16.30.1),
gamescope-native, and the nested gamescope fallback are verified. DRM CTM is
verified too: on a Steam Deck OLED's AMD GPU with no display server running,
apply, per-CRTC apply and off changed the picture as expected. The release tarball's programs need glibc 2.35 or
newer, and it carries prebuilt KWin effects built in each target system's own
packages (B1); `satur8 repair` installs the matching one at login, and rebuilds
the effect only where build tools exist. Distribution is a Linux x86_64
release tarball, the Arch package in `packaging/`, a live Fedora package on COPR
built for Fedora 43, 44 and 45, and two community-maintained AUR packages
(`satur8` and `satur8-bin`). A tagged-release GitHub Actions workflow builds the source
and Linux tarballs with checksums so the packaging sources and the website
download stay in sync. This file keeps the longer design notes and backend
roadmap; the user-facing setup guide lives in `README.md`.

---

## 1. The problem, stated precisely

NVIDIA Windows users get "Digital Vibrance" in the driver, and VibranceGUI
automates it per-game (boost saturation when CS2 launches, restore the desktop
when it closes). Linux has no clean equivalent, and the situation splits hard by
display session:

- **X11**: `libvibrant` / `vibrantLinux` work. They set the GPU's **CTM** (Color
  Transformation Matrix) via the X RandR `CTM` output property. AMD, Intel, and
  nouveau expose this through the open DRM drivers. This is the good path, but
  it only works on X11 because on X11 the X server owns the display.

- **Wayland**: the compositor exclusively owns the display (DRM master). A
  third-party app *cannot* just set the CTM. You must go through the compositor.
  Each compositor is different, which is why no universal tool exists yet.

The target user (this project's author) is on **CachyOS + KDE Plasma Wayland +
AMD RX 9070 XT**, which is exactly the case with no good answer today.

### The anti-cheat constraint (this is the whole point)

CS2 runs **Trusted Mode** by default: it blocks third-party code from being
**injected into the CS2 process**. That is precisely what `vkBasalt` does - it
is a Vulkan post-processing *layer loaded inside the game process*. Same for
ReShade injected into the game. Recommending those for CS2 is the mistake.

The safe rule, which this project is built around:

> **Never touch the game process. Only adjust the display/compositor color path.**

The GPU scanout color matrix, a compositor shader, or a nested compositor all
operate on pixels *after* the game has already rendered and handed them off.
The CS2 process is never read, written, or hooked. This is the same category as
turning up saturation on your monitor's OSD - VAC has nothing to see.

---

## 2. Design principles

1. **Outside the game, always.** No Vulkan layers in the game, no DLL/`.so`
   injection, no memory hooks. Compositor/scanout only.
2. **Lowest possible hardware tax**, CPU first. CS2 is CPU-bound; the satur8
   tool must not steal cycles from it. Two consequences:
   - Prefer the **hardware CTM** (zero per-frame cost) over per-frame shaders.
   - **No busy polling.** Triggers are event-driven or a launch wrapper.
3. **Hardware/compositor agnostic by abstraction.** One core, many backends. The
   tool detects the environment and picks the best available backend.
4. **Per-game profiles** with sane global default, like VibranceGUI.
5. **Degrade gracefully and honestly.** If the only available backend has a real
   cost or caveat (added latency, GPU pass), say so in the UI rather than hiding
   it.

---

## 3. Why the cost differs per backend (and why CTM wins)

| Backend | Where it runs | Per-frame CPU | Per-frame GPU | Latency added | Anti-cheat |
|---|---|---|---|---|---|
| **DRM CTM** (hardware) | display scanout block | **0** | **0** | **0** | safe |
| **KWin GLSL effect** | KWin compositor | ~0 | 1 fullscreen pass | ~0 (already compositing) | safe |
| **gamescope + reshade** | nested compositor | low | extra composite pass | small, measurable | safe |
| ~~vkBasalt / ReShade~~ | **inside game** | n/a | n/a | n/a | **risky - excluded** |

The CTM is a 3x3 matrix the GPU's display hardware multiplies every output pixel
by, during scanout. Setting it is a one-time `ioctl`; after that it is literally
free. This is why it is the preferred backend wherever we can reach it.

The catch is *reachability* on Wayland (section 5).

---

## 4. The math (shared by every backend)

Every backend applies the same saturation transform; they differ only in how
they hand the GPU the matrix. Saturation `s` (1.0 = unchanged, >1 = more vivid,
matching VibranceGUI's range conceptually; libvibrant uses 0.0-4.0):

```
out = luma + s * (in - luma)
luma = 0.2126*R + 0.7152*G + 0.0722*B   (Rec.709)
```

As a 3x3 matrix (identity when s = 1), with `w = 1 - s`:

```
| w*Lr + s   w*Lg       w*Lb      |
| w*Lr       w*Lg + s   w*Lb      |
| w*Lr       w*Lg       w*Lb + s  |
```

This lives in `satur8-core` once. The DRM backend converts it to the
S31.32 fixed-point the kernel CTM property expects; the shader backends feed it
to a `mat3` uniform. (Optional later refinement: do the blend in linear light
rather than gamma-encoded sRGB for a more correct result; default to matching
VibranceGUI's perceptual behavior so numbers feel familiar.)

---

## 5. Backends (priority = best cost + coverage first)

### B1. KWin effect backend - **MVP, the author's stack**
KDE Plasma Wayland. KWin owns DRM master, and there is no public API to set the
CTM from a client, so on KDE the realistic path is a **compositor GLSL effect**.
Prior art: `kevinlekiller/kwin-effect-shaders` (archived) explicitly exists as a
VAC-safe alternative to vkBasalt/ReShade and applies GLSL post-processing in the
compositor. We ship our own minimal, single-purpose **saturation** effect
(not a general shader loader) so it is tiny and fast:

- A KWin/Effect package in `assets/kwin-effect/` (metadata + QSB/GLSL fragment
  shader that applies the `mat3`).
- Toggle + set saturation via **D-Bus** to the effect (no polling).
- Trade-off vs CTM: one fullscreen GPU pass per frame. On a 9070 XT this is
  sub-millisecond and CPU-free, but we document it.
- Investigate whether recent KWin (6.x) exposes a per-output color/CTM path we
  can use instead of a shader - if so, KDE moves to zero-cost too. Track this.

**A different value per output (since v0.5.0).** Implemented; checked on
headless KWin with two virtual outputs and software rendering (pixels on
6.7.3, 6.8.2 and 6.8.3), and on a real KWin 6.8.3 session with three monitors
the focus script reported the window's monitor as it moved. A different value
on each of several real monitors has not been seen yet.

- *File.* `profile_outputs` (`"all"`, the default, or `"focused"`) and
  `[output."NAME"]` tables with their own `default_saturation`, written only
  when used. Outputs are named by port, as KWin names them.
- *Resolver.* `crates/satur8-core/src/levels.rs` turns the file, the matched
  profile, the focused output and whether the backend can target one output
  into the levels every output shows, plus notes for what could not be done.
  All rules are pure and table-tested there.
- *Backends.* `Backend::per_output()` and `apply_levels()`; the defaults keep
  one value on every output. KWin is per-output when the loaded effect
  answers the new slots, and DRM CTM sets each CRTC. GNOME, Hyprland,
  NV-CONTROL and gamescope stay one value for every output, and the reports
  say so when the file asks for more.
- *Effect.* A base value plus a table from output name to value, read per
  window from `EffectWindow::screen()` at draw time, so a monitor plugged in
  later gets its value with no daemon work. D-Bus: `setOutputSaturation(sd)`,
  `setSaturations(da{sv})`, `outputSaturations() -> a{sv}`,
  `outputs() -> a{sv}`. `setSaturation` keeps its meaning and clears the
  table. An effect without the new slots answers
  `org.freedesktop.DBus.Error.UnknownMethod`, and the backend falls back to one
  value with a note.
- *Focus.* The focus script sends `WindowActivatedOn(class, caption, pid,
  output)` and reports again when the focused window changes output; until
  the daemon answers that call it also sends `WindowActivatedPid` first, so an
  older daemon still follows focus. The daemon handles focus calls in the order
  they arrive.

**How the effect reaches systems without build tools (since v0.4.1).**

- *The compatibility key.* KWin loads an effect only when its embedded plugin
  ID names KWin's exact x.y.z version, and Qt refuses a plugin built against a
  newer Qt minor than the one installed (the effect requires one `Qt_6.x`
  symbol version). So each effect is built inside the target system's own
  package set, which gets the KWin headers, KWin's build options and the Qt
  version exactly right. The key is the system family plus the KWin version:
  `steamos-6.7.3`, `fedora45-6.8.3`.
- *The bundled set.* `packaging/kwin-effect-targets.txt` lists the targets and
  `packaging/build-kwin-effect.sh` builds each one: SteamOS branches against
  Valve's package mirror, Fedora releases in a Fedora container. v0.4.1 carries
  `steamos-6.7.3` (SteamOS 3.9), `steamos-6.8.2` (SteamOS 3.10), and
  `fedora44-6.8.3` and `fedora45-6.8.3`. When two targets give the same key,
  the first in the list wins, so the effect is built against the oldest Qt that
  KWin version meets. The effect is built and tested for KWin 6.7 and 6.8;
  KWin 6.5 and older use a different `drawWindow` signature and are not
  covered.
- *The picker order.* The installer (`packaging/install.sh`, which prints its
  choice with `--pick-effect`) and `satur8 repair` decide the same way: the
  set's effect for this family and KWin version; else a local build when the
  effect source, `cmake`, a C++ compiler and the KWin headers are present;
  else another family's effect for the same KWin version whose `Qt_6.x`
  requirement is not above the installed Qt, with a printed note; else skip,
  with the reason. An entry whose embedded ID disagrees with its folder name is
  never used. Nothing is downloaded.
- *Repair from the set.* The installer copies the whole set to
  `~/.local/share/satur8/kwin-effects`, so `satur8 repair`, run by the daemon's
  unit at every session start, can install the right effect after an OS update
  with no network and no build tools. Repair also checks the per-user effect
  file on disk against the running KWin, so a wrong file is replaced even while
  KWin still uses the copy it loaded earlier. The new file is copied beside the
  destination and renamed over it, never written in place: the running KWin
  may have that file loaded, and on a Steam Deck overwriting a loaded effect
  crashed KWin. `satur8 doctor` names where the loaded effect came
  from, and the desktop app's Status page follows the same decision.

### B2. DRM CTM backend - **zero-cost, the ideal**
Direct libdrm. Sets the `CTM` property on the CRTC. Works when we can hold or
share DRM master:
- **X11** sessions (any GPU exposing CTM: AMD, Intel, nouveau) via the RandR
  `CTM` property - same mechanism as libvibrant.
- **TTY / standalone** (we are the only DRM client).
- Foundation for compositor-cooperative paths below.

Zero per-frame cost. This is the gold standard; the Wayland problem is purely
"who is allowed to set it."

### B3. Hyprland backend - **zero-cost on Wayland, today**
Hyprland implemented a custom Wayland protocol that lets a client set the CTM
per-window (prior art: `hyprland-ctm-vibrance`, and its successor `hyprvibr`
which uses a plugin so stock Hyprland works). Non-NVIDIA GPUs that honor the DRM
CTM. This is proof that compositor-cooperative CTM is the right long-term shape.

### The one thing that matters: it's all DRM/KMS underneath
The saturation matrix lives in *one* place in hardware: the GPU scanout block
(the CTM), in the kernel's DRM/KMS layer. X11 and Wayland are not two color
systems; they are two answers to "who is allowed to talk to DRM":
- **X11**: the X server talks to DRM -> it (or NV-CONTROL) sets the CTM for us.
- **Wayland**: the compositor talks to DRM -> *it* must set the CTM for us.
- **Bare KMS (no display server)**: *we* talk to DRM -> we set the CTM ourselves.

One room, three doors. Doing it right = getting a key to that room (native
backend, zero cost). gamescope is "lazy" because instead of getting a key it
builds a second compositor in front of yours and does the work there - hence the
extra composite pass and latency. It needs zero cooperation, which is why it is
the universal fallback and also why it is the wrong default. Prefer native.

### B4. GNOME Shell extension backend - **GNOME Wayland, native, any GPU**
GNOME/Mutter exposes no client CTM API, but a GNOME Shell extension can apply a
GLSL effect at the shell level (same category as the KWin effect). Because it is
just a compositor shader, it is GPU-agnostic and **works on NVIDIA Wayland too**.
This is the native GNOME path; GNOME users do not need gamescope.

### B5. NV-CONTROL backend - **NVIDIA on X11, native**
The NVIDIA proprietary driver exposes a real "Digital Vibrance" control via the
NV-CONTROL X extension (`nvidia-settings -a "[gpu:0]/DigitalVibrance=N"`, range
roughly -1024..1023). This is the exact feature VibranceGUI drives on Windows,
available on Linux X11. Native path for NVIDIA on X11 (the DRM CTM in B2 is
unreliable on the proprietary driver). NVIDIA on Wayland is covered natively by
the compositor shader paths (B1 KWin / B4 GNOME / Hyprland's screen-shader hook),
*not* gamescope.

### B6. gamescope backends - **Steam Game Mode, a running gamescope, and nested as the last resort**
The gamescope crate (`crates/backends/gamescope`) has one automatic backend and
two run strategies. The Game Mode backend is part of the automatic backend
selector (`select_backend` in `crates/satur8-backend/src/lib.rs`), so
`satur8 on`/`set`/`off`, the daemon, and the GUI (not tested on a Deck) use it
in Steam Game Mode. The two run strategies are launch-only: they wrap one game
launch with `satur8 run --via ...` and restore when it exits, and the selector
never picks them.

**Steam Game Mode (automatic, the SteamOS path; `look.rs`, `session.rs`,
`backend.rs`).** Since v0.4.0. Verified on a Steam Deck OLED with SteamOS 3.9.5
and gamescope 3.16.30.1.

- *Color.* Satur8 sets gamescope's look table, the `GAMESCOPE_COLOR_LOOK_G22`
  root property, to the path of a 17x17x17 `.cube` file built from the shared
  saturation math. gamescope applies the look to each app's gamma-encoded sRGB
  values before its own color pipeline, so Steam's vibrancy (gamut wideness),
  color temperature and night mode still run on top of it, and Satur8 never
  writes those properties. The G22 look only changes SDR content; HDR content
  is untouched (by gamescope's source; not tested).
- *One file per value.* gamescope reads the file only when the property
  changes, so a file is never rewritten in place. Each saturation gets its own
  file, named by its value (`look-g22-s1750.cube` for 1.75), written once under
  a temporary name and renamed into `~/.local/state/satur8/gamescope/` (or
  `$XDG_STATE_HOME/satur8/gamescope/`). Applying a value writes its file if it
  is missing and sets the property to its path; older Satur8 files are then
  deleted. At 1.0 the property is removed. A look something else had set is
  remembered in memory and put back on reset.
- *Server 0's root only.* The look is one global inside gamescope, whichever
  Xwayland root set it, and deleting it on any root clears it. Satur8 reads and
  writes only on the root of gamescope's first Xwayland server.
- *The probe.* `DISPLAY` is not trusted, because the session scripts unset it
  for systemd units and an SSH shell has none. Satur8 tries `$DISPLAY`, then
  each `/tmp/.X11-unix/X<n>` socket, and accepts the root that has
  `GAMESCOPE_PID`, `GAMESCOPE_XWAYLAND_SERVER_ID` = 0, and
  `GAMESCOPE_FOCUSED_APP_GFX`. gamescope writes that last one only with
  `--steam`, so a gamescope nested in a desktop does not count. The backend
  keeps no X connection between calls: each apply or reset connects, and
  re-probes when the remembered display is no longer gamescope's.
- *Focus.* A thread in the daemon waits for `PropertyNotify` on that root for
  `GAMESCOPE_FOCUSED_APP_GFX` (the AppID of the window being drawn; 769 is the
  Steam UI), `GAMESCOPE_FOCUSED_WINDOW` and `GAMESCOPE_FOCUSABLE_WINDOWS`. It
  reports the focus to the daemon as window class `steam_app_<AppID>`, which
  profiles already match by `steam_app_id`, with the pid from the `[window,
  appid, pid]` triplet of the focused window, which resolves a Proton game to
  its Windows executable. No polling while connected. On every new connection
  it resyncs: it applies the focused game's profile, or puts the default back,
  which also clears a table a crashed daemon left behind. When the connection
  drops it re-probes. It acts only while the selected backend is gamescope's,
  so it never competes with a desktop's focus script.

**Bazzite.** Bazzite's Game Mode (the `bazzite-deck` images) runs the
OpenGamingCollective gamescope fork, packaged by Terra as `terra-gamescope`
3.17.0-ogc1 in stable 45.20261117.1, started by OpenGamingCollective's
`gamescope-session-plus@.service` with `--steam` and two Xwayland servers.
The fork's color, look and focus code matches upstream 3.17.0, so the look
table path applies unchanged. Desktop Mode is KDE Plasma 6.8 or GNOME 51 and
uses the KWin or GNOME backend as on any Fedora desktop. Checked from source and
in a container only: the v0.4.0 installer and background service ran against
Bazzite's own gamescope, run headless with `--steam`, which applied and removed
the look table as focus moved. Satur8 has not been run on a Bazzite device, so
Bazzite Game Mode is Implemented, not Verified.

**gamescope-native (running compositor, launch option).**
`satur8 run --via gamescope-native -- %command%` is for a session that already
runs inside gamescope, such as Steam Game Mode, where Steam launch options run
with `DISPLAY` pointed at gamescope's embedded Xwayland. It connects to that X
server, refuses to continue unless the root window carries `GAMESCOPE_PID`,
writes a shaper LUT and a 3D LUT generated from the shared saturation matrix,
and points the running gamescope at them through the
`GAMESCOPE_COLOR_SHAPERLUT_OVERRIDE` and `GAMESCOPE_COLOR_3DLUT_OVERRIDE`
root-window atoms, holding `GAMESCOPE_COLOR_MANAGEMENT_DISABLE` at 0 while they
are set. It saves the previous values of all three atoms first and restores them
when the game exits (or rolls them back if applying fails). No second
compositor is launched; the saturation goes through gamescope's own color
pipeline. The override replaces gamescope's computed color tables, so Steam's
vibrancy and color temperature do not apply while it is set, and it wins over a
Satur8 look table until the game ends (measured on a Steam Deck). It is
unchanged in v0.4.0; in Game Mode the automatic backend above is the
recommended path, and a game should use one or the other, not both.

**Nested gamescope (the last resort).** gamescope is a nested compositor that
runs under *any* session and supports `--reshade-effect <path>` applied to its
**own** composited output (not injected into the game). So
`satur8 run --via gamescope -- %command%` launches the game inside a new
gamescope with a satur8 effect. It needs no cooperation from the host
compositor, which is exactly why it is the floor nobody falls through - and why
it costs an extra composite pass + latency. It is the answer ONLY for Wayland
compositors with no native hook at all (Sway / minimal wlroots / niche), where
`wlr-gamma-control` is the only color API and cannot do saturation (per-channel
gamma LUTs can't mix channels; saturation is a 3x3 matrix that must). The
proper long-term fix for those is upstreaming a CTM protocol to wlroots.

### Backend selection order at runtime
```
KDE Wayland         -> B1 (KWin effect)
Hyprland            -> B3 (Hyprland CTM; screen-shader on NVIDIA)
GNOME Wayland       -> B4 (Shell extension shader, any GPU incl. NVIDIA)
X11 + AMD/Intel/nv  -> B2 (DRM CTM)
X11 + NVIDIA prop   -> B5 (NV-CONTROL Digital Vibrance)
Bare KMS            -> B2 (DRM CTM, we own DRM master)
Steam Game Mode     -> B6 Game Mode (gamescope look table)
Inside gamescope    -> B6 native (`satur8 run --via gamescope-native`, opt-in)
Sway / other wlr    -> B6 nested (`satur8 run --via gamescope`, opt-in)
                       [no native hook; upstream CTM = TODO]
```
The first seven rows are picked automatically. `select_backend` tries them in
this order: KWin, GNOME, Hyprland, NV-CONTROL, then stops with a `satur8 repair`
hint if KWin is running but rejects the effect, then Steam Game Mode, then DRM
CTM. Game Mode comes after every desktop backend, so desktops behave as before,
and before DRM CTM, which needs the DRM master that gamescope holds as the
session compositor. `satur8 doctor` and `satur8 status` show the Game Mode
report only when no desktop backend comes first. The last two gamescope rows are not
automatic: the user chooses them per launch with `--via`, because both wrap a
single game launch instead of applying and resetting on demand.

### Who can actually run this (the "all Linux users" answer)
Three independent axes; only the compositor one is hard:

- **Distro** (Ubuntu/Debian/Mint/Manjaro/Arch/Fedora/...): **all of them, no code
  difference.** A distro is packaging + kernel version. Ship one Rust binary as
  distro packages *and* a Flatpak/AppImage. This axis is essentially free.
- **GPU/driver**: AMD/Intel/nouveau via CTM; NVIDIA via NV-CONTROL (X11) or a
  compositor shader (Wayland). All covered, all native.
- **Display system**: X11, Wayland, and bare KMS are the only real ones. Mir is a
  Wayland compositor underneath. XWayland is just X11 apps on a Wayland
  compositor, so the compositor backend still owns the final pixels. fbdev /
  VNC / RDP have no real color pipeline and are not gaming-relevant.

| Environment | Backend | Coverage |
|---|---|---|
| Any X11 (AMD/Intel/nouveau) | DRM CTM | native, zero-cost |
| Any X11 (NVIDIA) | NV-CONTROL | native, zero-cost |
| Bare KMS (any GPU) | DRM CTM | native, zero-cost |
| KDE Plasma Wayland (any GPU) | KWin effect | native, ~free |
| GNOME Wayland (any GPU) | Shell extension | native, ~free |
| Hyprland (any GPU) | CTM / screen shader | native, ~free |
| Steam Game Mode (gamescope `--steam`) | gamescope look table | native, in gamescope's own pipeline |
| Sway / minimal wlroots | gamescope | fallback, small cost |

**The guarantee:** every major environment gets a *native* backend - gamescope is
no longer the answer for GNOME or NVIDIA-on-Wayland (those use compositor
shaders). Steam Game Mode, where gamescope is the session compositor, gets its
own native path through gamescope's look table. Nested gamescope survives only
as the last-resort floor for niche Wayland compositors that expose no hook at
all, and even they get working per-game
satur8. The one genuine gap: desktop-wide *always-on* satur8 on those niche
compositors (per-game still works via gamescope).

---

## 6. Triggering: per-game, "any game", and CPU-cheap

VibranceGUI watches the process list. Polling the process table at 60Hz is
exactly the CPU waste we want to avoid. Two better paths, both near-zero CPU:

1. **Launch wrapper (primary, zero polling).** Steam launch options:
   `satur8 run --profile cs2 -- %command%`. Applies the profile, `exec`s the
   game, restores on exit. No watcher process at all during play. This also
   covers non-Steam launchers (Lutris, Heroic, bare command).
2. **Event-driven watcher (optional, for always-on / focus-based).**
   - KDE: subscribe to KWin's active-window-changed D-Bus signal. React only on
     change. Zero idle cost.
   - Hyprland: read its IPC event socket (blocking read, wakes on event).
   - Steam Game Mode: wait for `PropertyNotify` on gamescope's root for its
     focused-app properties (blocking wait, wakes on change).
   - Generic X11: `XRandR`/X events.
   - Last resort generic: a slow (e.g. 2s) process check, clearly labeled as the
     fallback for environments with no event source.

Profiles match by executable name, window class/title, or Steam AppID, and store
the target saturation + which output(s) to affect.

---

## 7. Language choice - recommendation: **Rust**

You asked whether Rust or something else fits. Rust, clearly, and here is the
honest reasoning rather than fashion:

**Why Rust fits this problem specifically**
- **Lowest idle/runtime footprint with no GC.** This is a long-lived daemon that
  must not compete with CS2 for CPU. No garbage collector means no background
  collection pauses or heap churn while you game. An event-driven Rust daemon
  idles at effectively 0% CPU.
- **Single static binary**, trivial for users to install and for distros/AUR to
  package. No runtime to ship.
- **The ecosystem already exists for every backend:** `drm`/`drm-rs` for CTM,
  `zbus` for KWin D-Bus, `hyprland-rs` for Hyprland IPC, `wayland-client` for
  protocols. The closest prior art (`hyprland-ctm-vibrance`, `hyprvibr`) and a
  `libvibrant` crate are already Rust.
- **FFI to libdrm/C is first-class**, which we need for the CTM ioctls.
- **Memory safety** matters when poking ioctls and parsing compositor IPC; this
  is a tool other people will run as a daemon.

**Alternatives considered**
- **C++/Qt** - what VibranceGUI and vibrantLinux use. Viable, best-in-class if we
  want a deeply native KDE GUI, but more boilerplate and manual memory for the
  daemon. We can still write the *GUI* in Qt/QML later and keep the core in Rust.
- **Go** - easy and fast to write, but GC and larger idle footprint cut against
  the "don't tax the CPU" goal, and libdrm FFI is clunkier (cgo).
- **C** - closest to libdrm, but no safety net and slow to build the higher-level
  daemon/profile/IPC layers.

**Conclusion:** Rust for the core + daemon + CLI. GUI is a separate, later
component and can be Rust (egui/slint) or Qt/QML for a native KDE feel - it must
stay out of the gaming hot path (don't run a heavy GUI process while playing).

---

## 8. Architecture / repo layout

```
satur8/
├─ PLAN.md                 (this file)
├─ README.md
├─ LICENSE                 (GPL-3.0 - matches the libvibrant/KWin ecosystem)
├─ Cargo.toml              (workspace)
├─ crates/
│  ├─ satur8-core/       saturation matrix, Backend trait, Profile model, env detect
│  ├─ satur8-daemon/     event-driven watcher, applies/restores profiles
│  ├─ satur8-cli/        `satur8` command (run/set/profile/status/doctor)
│  └─ backends/
│     ├─ kwin/             B1  D-Bus control of the shipped KWin effect
│     ├─ drm-ctm/          B2  libdrm CTM (X11 / TTY)
│     ├─ hyprland/         B3  Hyprland CTM protocol / screen shader
│     ├─ gnome-shell/      B4  GNOME Shell extension (GLSL, any GPU)
│     ├─ nv-control/       B5  NVIDIA X11 Digital Vibrance (NV-CONTROL)
│     └─ gamescope/        B6  Steam Game Mode look table, launch wrappers, reshade effect
└─ assets/
   └─ kwin-effect/         the GLSL saturation effect package (shipped)
```

`Backend` trait (in core) is roughly:

```rust
pub trait Backend {
    fn name(&self) -> &str;
    fn detect() -> Option<Self> where Self: Sized;   // available in this env?
    fn apply(&mut self, output: &Output, sat: Saturation) -> Result<()>;
    fn reset(&mut self, output: &Output) -> Result<()>;
    fn cost_note(&self) -> CostNote;                 // honest perf disclosure
}
```

---

## 9. Roadmap / milestones

> These M0-M7 entries are the original *design* milestones and are kept as
> history; they are largely complete through v0.1. Forward-looking, trackable
> work (packaging, backend verification, Steam Deck) now lives in
> [ROADMAP.md](ROADMAP.md).

- **M0 - scaffold** (this commit): repo, workspace, core math + traits, plan.
- **M1 - MVP for the author's box:** KWin effect backend + saturation that
  visibly works on KDE Wayland, set via CLI. "I can boost CS2 and restore on
  exit on my machine."
- **M2 - launch wrapper:** `satur8 run -- %command%`, Steam launch-option
  workflow, profiles file. Any game, zero-poll trigger.
- **M3 - DRM CTM backend:** X11/TTY zero-cost path; broadens hardware coverage.
- **M4 - event-driven watcher:** D-Bus focus watcher for always-on KDE use.
- **M5 - native coverage for the rest:** GNOME Shell extension, Hyprland, and
  NV-CONTROL backends, then gamescope as the last-resort fallback. After this
  every major environment has a *native* path.
- **M6 - GUI + packaging:** profile editor, system tray; AUR + Flatpak; docs.
- **M7 - polish:** linear-light option, multi-monitor, per-output profiles,
  investigate native KWin CTM path to make KDE zero-cost.

## 10. Open questions (findings as of the M1-M7 build, verified on the box)
- **Does current KWin (6.x) expose a per-output color/CTM API a client can use?**
  **No (KWin 6.7.0).** The only color D-Bus surface is `org.kde.KWin.NightLight`,
  which exposes *colour temperature* only (no CTM, no saturation, read-mostly).
  There is no generic client shader/CTM path. So the compositor effect
  (`OffscreenEffect` + GLSL, what we ship) remains the realistic KDE path; KWin
  cannot be moved to zero-cost CTM today. Re-check on future KWin releases.
- **Does the AMD RX 9070 XT honor DRM CTM on this kernel?** **Yes.** A read-only
  probe (`satur8 doctor` / `satur8-drm-ctm::probe_ctm`) finds 4 CTM-capable
  CRTCs on the amdgpu card. Setting it still requires DRM master, i.e. a TTY /
  bare-KMS session (on Wayland KWin owns master); the zero-cost path is real
  there.
- gamescope reshade satur8 quality + measured latency on a 240Hz panel.
  Quality is verified: `satur8 run --via gamescope` wrapped `glxgears` in
  nested gamescope on KDE Wayland / AMD RX 9070 XT, and a `0.0` / `4.0`
  screenshot pair changed mean HLS saturation from 0.086 to 0.797 after fixing
  the shader install path. Latency/perf is still not measured.
- Confirm KWin effect cost is actually negligible at 1440p/240Hz in CS2.
  Measured on PC1 with CS2 FPS BENCHMARK DUST2 / `de_dust2`, DP-2 at
  2560x1440 @ 239.998 Hz, KDE Plasma Wayland / KWin 6.7.0, AMD Radeon RX
  9070 XT, and Satur8's KWin effect at 1.75. Dataset:
  `benchmarks/cs2-kwin-20260626-144823/` (raw VProf logs, CSV/JSONL, summary,
  and telemetry snapshots). Seven alternating scored pairs gave all-run medians
  of OFF Avg/P1 = 393.0/162.1 FPS and ON Avg/P1 = 399.8/162.4 FPS, which is
  +1.7% Avg and -0.043 ms/frame for ON. Because several runs had VProf excluded
  frames, the clean-run sensitivity check (`frames_excluded == 0`) was OFF
  409.4/166.0 FPS vs ON 402.4/165.0 FPS, or -1.7% Avg and +0.042 ms/frame.
  Paired median delta was -1.5% Avg / +0.038 ms/frame. Conclusion: no large
  measurable KWin cost on this setup; the observed effect is inside run-to-run
  noise, so do not claim literal zero cost from this benchmark.

## 11. Anti-cheat note for the README (user-facing, must be accurate)
State plainly: this tool only changes the **display color pipeline** (compositor
or GPU scanout), never the game. It does not inject into, read, or modify CS2 or
any game process, so it is outside the scope of VAC Trusted Mode - the same
category as your monitor's saturation setting. Do **not** add a disclaimer
promising "no ban" as a guarantee; describe *what it does and does not do* and
let that stand. Never recommend vkBasalt/ReShade-into-the-game for VAC titles.

## Naming
Working name `satur8` (binary `satur8`). Check GitHub + crates.io before
publishing; alternates if taken: `vivid`, `chroma-cli`, `satur`, `vibra`.
```
