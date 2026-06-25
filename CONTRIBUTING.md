# Contributing to Satur8

Thanks for your interest. Satur8 is a Rust workspace plus a C++ KWin effect, and
it values one thing above features: **never document an install path or backend
as working unless it has been tested.** Credibility is the scarce resource for a
small niche tool.

## Building

```sh
cargo build                 # debug build of the whole workspace
cargo build --release       # optimized build (LTO; slower to compile)
cargo build -p satur8-gui   # just the GUI
```

The KWin effect is a separate C++/Qt plugin (needs the KDE/KWin SDK):

```sh
cmake -S assets/kwin-effect -B assets/kwin-effect/build -DCMAKE_BUILD_TYPE=Release
cmake --build assets/kwin-effect/build
```

For a full local install (binaries, KWin effect/script, GNOME extension, systemd
user unit) without root:

```sh
packaging/install.sh            # per-user install to ~/.local
packaging/install.sh --uninstall
```

## Checks before a PR

```sh
cargo check
cargo test --workspace --locked
```

Please make sure:

- The workspace builds and the checks pass.
- Any command you add to the README or docs has actually been run.
- Backend status changes in the README/website table reflect real testing. Use
  "Implemented" for code that exists but is unverified and "Verified" only after
  it has been confirmed on real hardware.
- Commits are focused: one concern per commit with a clear message. The GitHub
  file list shows each file's most recent commit message, so a tidy message reads
  as that file's description.

## Project layout

See the repo layout section in the [README](README.md). Design and backend
rationale live in [PLAN.md](PLAN.md); what ships next and in what order lives in
[ROADMAP.md](ROADMAP.md).

## Backends

Each backend lives in `crates/backends/`. A new backend should implement the
shared backend trait in `satur8-core`, be selected by environment detection, and
ship with an honest status: do not mark it Verified in user-facing docs until it
has been confirmed working on real hardware for that environment.

## License

By contributing you agree your contributions are licensed under GPL-3.0-or-later,
the same license as the project.
