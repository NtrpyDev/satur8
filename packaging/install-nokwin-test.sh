#!/usr/bin/env bash
# Installer regression test for machines without KDE: GNOME, Hyprland, NVIDIA
# X11, a Steam Deck in Game Mode. It unpacks a release tarball in a clean
# container that has no KWin, no cmake and no network, runs the installer, and
# checks that Satur8 installs anyway and simply leaves the KWin effect out.
#
# v0.3.4 shipped an installer that aborted here; this is the check that would
# have caught it. Run it on the release tarball before tagging.
#
# Usage: packaging/install-nokwin-test.sh [--image IMAGE] [path/to/satur8-vX.Y.Z-linux-x86_64.tar.gz]
#
# IMAGE defaults to fedora:latest. Run it for ubuntu:22.04 too: there it is
# also a glibc floor test, because satur8 --version and satur8 repair run on
# glibc 2.35.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -n1)"
image="fedora:latest"
tarball=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --image) image="${2:?--image requires an image name}"; shift 2 ;;
        *) tarball="$1"; shift ;;
    esac
done
tarball="${tarball:-$repo/target/dist/satur8-v$version-linux-x86_64.tar.gz}"
[ -f "$tarball" ] || { echo "FAIL: no release tarball at $tarball (run packaging/release-tarball.sh)" >&2; exit 1; }

# ENGINE picks the container engine (CI sets docker); otherwise podman if
# installed, else docker.
runtime="${ENGINE:-}"
if [ -z "$runtime" ]; then
    for candidate in podman docker; do
        command -v "$candidate" >/dev/null 2>&1 && { runtime="$candidate"; break; }
    done
fi
[ -n "$runtime" ] || { echo "FAIL: needs podman or docker" >&2; exit 1; }

echo "==> $image"
"$runtime" run --rm --network none \
    -v "$(realpath "$tarball"):/satur8.tar.gz:ro" "$image" bash -c '
set -euo pipefail
if command -v kwin_wayland cmake >/dev/null 2>&1 || [ -e /usr/include/kwin ]; then
    echo "FAIL: the test container has KWin or cmake, so it proves nothing"; exit 1
fi
cd /root && tar -xzf /satur8.tar.gz && cd satur8-v*-linux-x86_64
packaging/install.sh > /root/install.log 2>&1 || { cat /root/install.log; echo "FAIL: installer exited non-zero without KWin"; exit 1; }
grep -q "KWin not found: skipping the KWin effect" /root/install.log || { cat /root/install.log; echo "FAIL: installer did not report skipping the effect"; exit 1; }
for bin in satur8 satur8-daemon satur8-gui; do
    [ -x "$HOME/.local/bin/$bin" ] || { echo "FAIL: $bin was not installed"; exit 1; }
done
[ ! -e "$HOME/.local/lib/qt6/plugins/kwin/effects/plugins/satur8.so" ] || { echo "FAIL: an effect was installed with no KWin present"; exit 1; }
[ ! -e "$HOME/.local/share/satur8/kwin-effects" ] || { echo "FAIL: the KWin effect set was copied with no KWin present"; exit 1; }
[ -f "$HOME/.config/systemd/user/satur8-daemon.service" ] || { echo "FAIL: the daemon unit was not installed"; exit 1; }
"$HOME/.local/bin/satur8" --version
"$HOME/.local/bin/satur8" repair
echo "PASS: installs without KWin and leaves the KWin effect out"
'
