#!/usr/bin/env bash
# Installer test for the bundled set of prebuilt KWin effects, on systems with
# no build tools. It unpacks the real release tarball in a clean container with
# no network, fakes the system with an os-release file (SATUR8_OS_RELEASE), a
# stub kwin_wayland and a Qt library file name (SATUR8_QT_LIB_DIR), runs the
# installer, and checks what it chose, what it printed, and that uninstall
# removes it all.
#
# Cases: SteamOS 3.9 with KWin 6.7.3; Bazzite (Fedora 45, ostree) with KWin
# 6.8.3; SteamOS with KWin 6.7.4, one patch ahead of every bundled effect.
#
# Usage: packaging/install-prebuilt-effect-test.sh [--image IMAGE] [path/to/satur8-vX.Y.Z-linux-x86_64.tar.gz]
#
# IMAGE defaults to fedora:latest. It must have no KWin and no cmake.
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

# The archive carries the app icon in the hicolor layout the installer reads.
tar -tzf "$tarball" | grep -q '^[^/]*/assets/icons/hicolor/256x256/apps/satur8\.png$' ||
    { echo "FAIL: the tarball has no assets/icons/hicolor/256x256/apps/satur8.png" >&2; exit 1; }

# The cases need these keys in the tarball's set.
set_keys="$(tar -tzf "$tarball" | sed -n 's|^[^/]*/prebuilt/linux-x86_64/kwin/effects/\([^/]*\)/satur8\.so$|\1|p')"
for key in steamos-6.7.3 fedora45-6.8.3; do
    grep -qx "$key" <<<"$set_keys" || {
        echo "FAIL: the tarball's effect set has no $key; it has: $(printf '%s' "$set_keys" | tr '\n' ' ')" >&2
        exit 1
    }
done

fails=0
run_case() { # run_case <name> <os-release, \n-separated> <kwin> <Qt lib file> <ostree: yes|no> <gamescope: yes|no> <expected key, or "" for none>
    local name="$1"
    echo "== $name"
    if "$runtime" run --rm --network none \
        -v "$(realpath "$tarball"):/satur8.tar.gz:ro" \
        -e OS_RELEASE="$2" -e KWIN="$3" -e QT_LIB="$4" -e OSTREE="$5" -e GAMESCOPE="$6" \
        -e EXPECT_KEY="$7" \
        "$image" bash -c '
set -euo pipefail
fail() { echo "FAIL: $*"; for f in /root/pick.txt /root/install.log /root/uninstall.log; do [ -f "$f" ] && { echo "--- $f"; cat "$f"; }; done; exit 1; }
for tool in kwin_wayland cmake c++; do
    if command -v "$tool" >/dev/null 2>&1; then
        fail "the test container has $tool, so it proves nothing"
    fi
done
[ ! -e /usr/include/kwin ] || fail "the test container has KWin headers, so it proves nothing"

# The fake system.
printf "%b\n" "$OS_RELEASE" > /root/os-release
printf "#!/bin/sh\necho \"kwin %s\"\n" "$KWIN" > /usr/local/bin/kwin_wayland
chmod +x /usr/local/bin/kwin_wayland
mkdir -p /root/qt && : > "/root/qt/$QT_LIB"
if [ "$OSTREE" = yes ]; then mkdir -p /run && touch /run/ostree-booted; fi
if [ "$GAMESCOPE" = yes ]; then
    mkdir -p /usr/share/wayland-sessions && : > /usr/share/wayland-sessions/gamescope-wayland.desktop
fi
export SATUR8_OS_RELEASE=/root/os-release SATUR8_QT_LIB_DIR=/root/qt

cd /root && tar -xzf /satur8.tar.gz && cd satur8-v*-linux-x86_64
set_src=prebuilt/linux-x86_64/kwin/effects
effect="$HOME/.local/lib/qt6/plugins/kwin/effects/plugins/satur8.so"
set_dst="$HOME/.local/share/satur8/kwin-effects"
conf="$HOME/.config/environment.d/60-satur8.conf"
icon="$HOME/.local/share/icons/hicolor/256x256/apps/satur8.png"

packaging/install.sh --pick-effect > /root/pick.txt 2>&1 || fail "--pick-effect exited non-zero"
packaging/install.sh > /root/install.log 2>&1 || fail "the installer exited non-zero"
! grep -q "rebuilding locally" /root/install.log || fail "the installer said it would rebuild with nothing to build with"

if [ -n "$EXPECT_KEY" ]; then
    grep -qx "pick: $EXPECT_KEY" /root/pick.txt || fail "--pick-effect did not choose $EXPECT_KEY"
    [ -f "$effect" ] || fail "no per-user effect was installed"
    grep -aqF "org.kde.kwin.EffectPluginFactory$KWIN" "$effect" || fail "the installed effect is not for KWin $KWIN"
    [ "$(sha256sum < "$effect")" = "$(sha256sum < "$set_src/$EXPECT_KEY/satur8.so")" ] ||
        fail "the installed effect is not the bundled $EXPECT_KEY"
    grep -qF "Using bundled KWin effect for ABI $KWIN ($EXPECT_KEY)" /root/install.log ||
        fail "the installer did not name the bundled effect it used"
else
    grep -q "^skip: .*KWin $KWIN" /root/pick.txt || fail "--pick-effect did not skip with the KWin version"
    [ ! -e "$effect" ] || fail "an effect was installed although none fits KWin $KWIN"
    grep -q "Skipping the KWin effect: .*KWin $KWIN" /root/install.log ||
        fail "the installer did not name the reason for skipping"
fi
if grep -q "^ID=steamos" /root/os-release && [ -n "$EXPECT_KEY" ]; then
    grep -q "Switch to Game Mode and back to Desktop Mode" /root/install.log ||
        fail "SteamOS was not told to switch to Game Mode and back"
fi
if grep -q "^ID=steamos" /root/os-release || [ "$OSTREE" = yes ]; then
    ! grep -qi "install cmake" /root/install.log || fail "an image-based system was told to install cmake"
fi
[ -d "$set_dst" ] || fail "the effect set was not copied to $set_dst"
[ "$(cd "$set_src" && ls)" = "$(cd "$set_dst" && ls)" ] || fail "the copied effect set differs from the bundled one"
[ -f "$conf" ] || fail "60-satur8.conf was not installed"
cmp -s "$icon" assets/icons/hicolor/256x256/apps/satur8.png || fail "the app icon was not installed at $icon"

packaging/install.sh --uninstall > /root/uninstall.log 2>&1 || fail "uninstall exited non-zero"
for path in "$effect" "$set_dst" "$conf" "$icon" "$HOME/.local/bin/satur8"; do
    [ ! -e "$path" ] || fail "uninstall left $path"
done
echo "--- pick: $(tr "\n" " " < /root/pick.txt)"
echo "PASS"
'; then
        :
    else
        fails=$((fails + 1))
    fi
}

steamos='NAME="SteamOS"\nID=steamos\nID_LIKE=arch\nVERSION_ID=3.9.5\nVARIANT_ID=steamdeck'
bazzite='NAME="Bazzite"\nID=bazzite\nID_LIKE="fedora"\nVERSION_ID=45'

run_case "SteamOS 3.9, KWin 6.7.3" "$steamos" 6.7.3 libQt6Core.so.6.11.1 no yes steamos-6.7.3
run_case "Bazzite, KWin 6.8.3" "$bazzite" 6.8.3 libQt6Core.so.6.11.2 yes no fedora45-6.8.3
run_case "SteamOS, KWin 6.7.4 (one patch ahead)" "$steamos" 6.7.4 libQt6Core.so.6.11.1 no yes ""

echo
if [ "$fails" -eq 0 ]; then
    echo "PASS: the installer picks the bundled effect, or skips it with the reason, with no build tools and no network"
else
    echo "FAILED: $fails case(s)"
    exit 1
fi
