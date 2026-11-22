#!/usr/bin/env bash
# Installer tests that need no container and install nothing.
#
# 1. The effect picker: `install.sh --pick-effect` against a fake effect set,
#    one fake system per row (a fake os-release through SATUR8_OS_RELEASE, a
#    stub kwin_wayland on PATH, a fake Qt library file name through
#    SATUR8_QT_LIB_DIR). Set SATUR8_REAL_EFFECT_DIR to a folder of
#    <key>/satur8.so to use real built effects in place of the fakes with the
#    same key.
# 2. --check-effect against the real kwin_wayland on this machine: accepts an
#    effect for the running KWin and rejects one for another version.
#
# Scratch files go under $TMPDIR (default /tmp).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALLER="$HERE/install.sh"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/satur8-installer-test.XXXXXX")"

fails=0

# ------------------------------------------------------------ effect picker

# A stand-in plugin: an embedded ID and Qt symbol version strings, which is all
# the picker reads.
fake_effect() { # fake_effect <file> <kwin version> <qt minor>
    mkdir -p "$(dirname "$1")"
    printf 'x%%org.kde.kwin.EffectPluginFactory%s\0Qt_6\0Qt_6.%s\0' "$2" "$3" >"$1"
}

tree="$ROOT/tree"
set_dir="$tree/prebuilt/linux-x86_64/kwin/effects"
mkdir -p "$tree/packaging"
install -m755 "$INSTALLER" "$tree/packaging/install.sh"
fake_effect "$set_dir/steamos-6.7.3/satur8.so" 6.7.3 11
fake_effect "$set_dir/steamos-6.8.2/satur8.so" 6.8.2 12
fake_effect "$set_dir/fedora45-6.8.3/satur8.so" 6.8.3 11
# Folder says 6.8.4, embedded ID says 6.8.5: must never be used.
fake_effect "$set_dir/steamos-6.8.4/satur8.so" 6.8.5 12
if [ -n "${SATUR8_REAL_EFFECT_DIR:-}" ]; then
    for real in "$SATUR8_REAL_EFFECT_DIR"/*/satur8.so; do
        [ -f "$real" ] || continue
        key="$(basename "$(dirname "$real")")"
        install -Dm644 "$real" "$set_dir/$key/satur8.so"
        echo "using the real effect $real as $key"
    done
fi
# The fake tree carries no effect source, so no row can choose a local build.

row_number=0
pick_case() { # pick_case <name> <os-release> <kwin> <Qt library file> <first line glob> <note glob, or "" for none>
    local name="$1" os="$2" kwin="$3" qtlib="$4" want="$5" want_note="$6"
    row_number=$((row_number + 1))
    local row="$ROOT/row$row_number"
    mkdir -p "$row/bin" "$row/qt" "$row/home"
    printf '%s\n' "$os" >"$row/os-release"
    printf '#!/bin/sh\necho "kwin %s"\n' "$kwin" >"$row/bin/kwin_wayland"
    chmod +x "$row/bin/kwin_wayland"
    if [ -n "$qtlib" ]; then
        : >"$row/qt/$qtlib"
    fi

    local out first note
    out="$(env HOME="$row/home" XDG_DATA_HOME="$row/home/.local/share" \
        XDG_CONFIG_HOME="$row/home/.config" XDG_STATE_HOME="$row/home/.local/state" \
        PATH="$row/bin:$PATH" SATUR8_OS_RELEASE="$row/os-release" SATUR8_QT_LIB_DIR="$row/qt" \
        bash "$tree/packaging/install.sh" --pick-effect 2>&1)" || {
        echo "  FAIL $name: --pick-effect exited non-zero: $out"
        fails=$((fails + 1))
        return
    }
    first="$(printf '%s\n' "$out" | head -n1)"
    note="$(printf '%s\n' "$out" | sed -n 's/^note: //p')"
    local ok=yes
    # shellcheck disable=SC2053 # the expectation is a glob on purpose
    [[ "$first" == $want ]] || ok=no
    if [ -z "$want_note" ]; then
        [ -z "$note" ] || ok=no
    else
        # shellcheck disable=SC2053
        [[ "$note" == $want_note ]] || ok=no
    fi
    if [ "$ok" = yes ]; then
        echo "  ok   $name"
    else
        echo "  FAIL $name"
        echo "       wanted: $want${want_note:+ / note: $want_note}"
        fails=$((fails + 1))
    fi
    printf '%s\n' "$out" | sed 's/^/       > /'
}

steamos=$'NAME="SteamOS"\nID=steamos\nID_LIKE=arch\nVERSION_ID=3.9.5\nVARIANT_ID=steamdeck'
bazzite=$'NAME="Bazzite"\nID=bazzite\nID_LIKE="fedora"\nVERSION_ID=45'
fedora=$'NAME="Fedora Linux"\nID=fedora\nVERSION_ID=45'
arch=$'NAME="Arch Linux"\nID=arch'

echo "== install.sh --pick-effect"
pick_case "SteamOS, KWin 6.7.3: its own effect" \
    "$steamos" 6.7.3 libQt6Core.so.6.11.1 "pick: steamos-6.7.3" ""
pick_case "SteamOS, KWin 6.8.2: its own effect" \
    "$steamos" 6.8.2 libQt6Core.so.6.12.0 "pick: steamos-6.8.2" ""
pick_case "Bazzite (ID_LIKE=fedora, 45), KWin 6.8.3: the Fedora 45 effect" \
    "$bazzite" 6.8.3 libQt6Core.so.6.11.2 "pick: fedora45-6.8.3" ""
pick_case "Fedora 45, KWin 6.8.3: its own effect" \
    "$fedora" 6.8.3 libQt6Core.so.6.11.2 "pick: fedora45-6.8.3" ""
pick_case "SteamOS, KWin 6.9.0: nothing, and the reason names KWin 6.9.0" \
    "$steamos" 6.9.0 libQt6Core.so.6.12.0 "skip:*KWin 6.9.0*" ""
pick_case "Arch, KWin 6.8.3, Qt 6.12, no build tools: Fedora's effect with a note" \
    "$arch" 6.8.3 libQt6Core.so.6.12.0 "pick: fedora45-6.8.3" "*another system (fedora45)*"
pick_case "Arch, KWin 6.8.3, Qt 6.10, no build tools: nothing (needs Qt 6.11)" \
    "$arch" 6.8.3 libQt6Core.so.6.10.3 "skip:*fedora45-6.8.3 needs Qt 6.11 and this system has Qt 6.10*" ""
pick_case "Arch, KWin 6.8.3, Qt unknown: nothing" \
    "$arch" 6.8.3 "" "skip:*Qt version could not be read*" ""
pick_case "Arch, KWin 6.7.3, Qt 6.11: SteamOS's effect (Qt_6.11) with a note" \
    "$arch" 6.7.3 libQt6Core.so.6.11.1 "pick: steamos-6.7.3" "*another system (steamos)*"
pick_case "Arch, KWin 6.7.3, Qt 6.10: nothing (SteamOS's effect needs Qt 6.11)" \
    "$arch" 6.7.3 libQt6Core.so.6.10.3 "skip:*steamos-6.7.3 needs Qt 6.11 and this system has Qt 6.10*" ""
pick_case "Arch, KWin 6.8.2, Qt 6.11: nothing (SteamOS's effect needs Qt 6.12)" \
    "$arch" 6.8.2 libQt6Core.so.6.11.2 "skip:*steamos-6.8.2 needs Qt 6.12 and this system has Qt 6.11*" ""
pick_case "SteamOS, KWin 6.8.4: folder steamos-6.8.4 embeds 6.8.5, never picked" \
    "$steamos" 6.8.4 libQt6Core.so.6.12.0 "skip:*KWin 6.8.4*" ""
pick_case "Arch, KWin 6.8.4: the mislabeled entry is not used across systems either" \
    "$arch" 6.8.4 libQt6Core.so.6.12.0 "skip:*KWin 6.8.4*" ""

# ------------------------------------------- --check-effect against real KWin

echo "== install.sh --check-effect"
RUNTIME_ABI="$(kwin_wayland --version | awk 'NR == 1 { print $2 }')"

printf 'org.kde.kwin.EffectPluginFactory%s\n' "$RUNTIME_ABI" >"$ROOT/matching.so"
printf 'org.kde.kwin.EffectPluginFactory0.0.0\n' >"$ROOT/mismatched.so"

"$INSTALLER" --check-effect "$ROOT/matching.so"
if "$INSTALLER" --check-effect "$ROOT/mismatched.so"; then
    echo "FAIL: installer accepted a mismatched KWin effect" >&2
    exit 1
fi
echo "PASS: installer accepts only the current KWin effect ABI"

if [ "$fails" -ne 0 ]; then
    echo "FAILED: $fails effect picker case(s); files in $ROOT" >&2
    exit 1
fi
echo "PASS: the effect picker chose as expected for every fake system"
