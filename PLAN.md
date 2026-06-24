# Satur8 for Linux - Design Notes

> A VibranceGUI-style digital vibrance tool for Linux. Per-game saturation
> boost, applied **outside** the game process so it cannot trip anti-cheat,
> with as close to **zero CPU cost** as the hardware allows.

Status: v0.2.1 is working. KDE Plasma Wayland is the verified target, with the
GUI, CLI, daemon, KWin effect, KWin focus script, tray app, and profile config
working together. NVIDIA X11 NV-CONTROL is also verified on real NVIDIA
hardware. v0.2 widened distribution: a live Fedora package on COPR alongside
the Arch package, and a tagged-release GitHub Actions workflow that builds the
source and Linux tarballs with checksums so the packaging sources and the
website download stay in sync. This file keeps the longer design notes and
backend roadmap; the user-facing setup guide lives in `README.md`.

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

### B6. gamescope backend - **true last resort only**
gamescope is a nested compositor that runs under *any* session and supports
`--reshade-effect <path>` applied to its **own** composited output (not injected
into the game). So `satur8 run -- %command%` launches the game inside gamescope
with a satur8 effect. It needs no cooperation from the host compositor, which
is exactly why it is the floor nobody falls through - and why it costs an extra
composite pass + latency. It is the answer ONLY for Wayland compositors with no
native hook at all (Sway / minimal wlroots / niche), where `wlr-gamma-control`
is the only color API and cannot do saturation (per-channel gamma LUTs can't mix
channels; saturation is a 3x3 matrix that must). The proper long-term fix for
those is upstreaming a CTM protocol to wlroots.

### Backend selection order at runtime
```
KDE Wayland         -> B1 (KWin effect)
Hyprland            -> B3 (Hyprland CTM; screen-shader on NVIDIA)
GNOME Wayland       -> B4 (Shell extension shader, any GPU incl. NVIDIA)
X11 + AMD/Intel/nv  -> B2 (DRM CTM)
X11 + NVIDIA prop   -> B5 (NV-CONTROL Digital Vibrance)
Bare KMS            -> B2 (DRM CTM, we own DRM master)
Sway / other wlr    -> B6 (gamescope)   [no native hook; upstream CTM = TODO]
```

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
| Sway / minimal wlroots | gamescope | fallback, small cost |

**The guarantee:** every major environment gets a *native* backend - gamescope is
no longer the answer for GNOME or NVIDIA-on-Wayland (those use compositor
shaders). gamescope survives only as the last-resort floor for niche Wayland
compositors that expose no hook at all, and even they get working per-game
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
│     └─ gamescope/        B6  last-resort launch wrapper + reshade effect
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
- gamescope reshade satur8 quality + measured latency on a 240Hz panel. (Open;
  the fallback shader is implemented and loads, perf not yet measured.)
- Confirm KWin effect cost is actually negligible at 1440p/240Hz in CS2. (Open;
  needs an in-game measurement pass.)

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
