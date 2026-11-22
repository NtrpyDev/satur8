#!/usr/bin/env bash
# End-to-end test of `satur8 repair` against a HEADLESS, isolated KWin (its own
# D-Bus bus, virtual framebuffer, and throwaway HOME). Nothing on the real
# desktop is touched.
#
# It reproduces the after-a-KWin-update breakage by planting an effect built for
# a KWin version that does not exist, then checks that repair rebuilds it, that
# the rebuilt effect really applies saturation, and that a second run is a no-op.
# A second pass runs without the per-user plugin dir on QT_PLUGIN_PATH and
# expects the honest "log out and back in" answer instead of a false success.
#
# Two more passes run repair with cmake and c++ hidden from its PATH and a fake
# SteamOS os-release: one with a matching effect in the installed set (repair
# installs it from there), one without (repair fails and says so). A fake curl
# first on that PATH must never run.
#
# Usage: assets/kwin-effect/sandbox-repair-test.sh [path/to/satur8]
set -uo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
SATUR8="${1:-$repo/target/debug/satur8}"
work="$repo/target/sandbox-repair"
rm -rf "$work"
mkdir -p "$work"

# A plugin whose embedded KWin version can never match: the real build with its
# version tag rewritten in place (same length, so the file stays well-formed).
stale_build="$work/stale-build"
cmake -S "$repo/assets/kwin-effect" -B "$stale_build" -DCMAKE_BUILD_TYPE=Release >/dev/null \
    && cmake --build "$stale_build" >/dev/null \
    || { echo "FAIL: could not build the effect to make a stale copy"; exit 1; }
real_abi="$(sed -n 's/^#define KWIN_PLUGIN_VERSION_STRING "\(.*\)"/\1/p' /usr/include/kwin/config-kwin.h)"
fake_abi="$(printf '%s' "$real_abi" | tr '0-9' '9876543210')"
sed "s/EffectPluginFactory${real_abi//./\\.}/EffectPluginFactory${fake_abi}/g" \
    "$stale_build/satur8.so" > "$work/stale-satur8.so"

fails=0
expect() { # expect <label> <needle> <haystack>
    if printf '%s' "$3" | grep -qF -- "$2"; then
        echo "  ok   $1"
    else
        echo "  FAIL $1"
        echo "       wanted: $2"
        echo "       got:    $3"
        fails=$((fails + 1))
    fi
}

run_case() { # run_case <name> <with_plugin_path: yes|no>
    local name="$1" with_path="$2"
    local home="$work/$name/home"
    local plugins="$home/.local/lib/qt6/plugins"
    mkdir -p "$plugins/kwin/effects/plugins"
    install -m755 "$work/stale-satur8.so" "$plugins/kwin/effects/plugins/satur8.so"

    echo "== $name"
    local plugin_path="$plugins"
    [ "$with_path" = yes ] || plugin_path="$work/$name/not-on-the-path"
    mkdir -p "$plugin_path"

    HOME="$home" SATUR8="$SATUR8" REPO="$repo" KWIN_PLUGIN_PATH="$plugin_path" \
    WITH_PATH="$with_path" REAL_ABI="$real_abi" LOG="$work/$name/kwin.log" \
    dbus-run-session -- bash -s <<'INNER' > "$work/$name/out.txt" 2>&1
set -uo pipefail
unset XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME
export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=KDE
export SATUR8_EFFECT_SOURCE="$REPO/assets/kwin-effect"

# The per-user dir goes ahead of the system one, exactly as 60-satur8.conf does
# for a real session. The system dir stays reachable through Qt's built-in path.
env -u WAYLAND_DISPLAY -u DISPLAY QT_PLUGIN_PATH="$KWIN_PLUGIN_PATH" \
    kwin_wayland --virtual --width 1280 --height 720 >"$LOG" 2>&1 &
KWIN=$!
trap 'kill $KWIN 2>/dev/null' EXIT
for _ in $(seq 1 80); do
    qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.loadedEffects >/dev/null 2>&1 && break
    sleep 0.25
done

echo "@@supported-before=$(qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.isEffectSupported satur8)"
echo "@@doctor=$("$SATUR8" doctor 2>&1 | tr '\n' '|')"
echo "@@repair1=$("$SATUR8" repair 2>&1 | tr '\n' '|')"
echo "@@supported-after=$(qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.isEffectSupported satur8)"
echo "@@repair2=$("$SATUR8" repair 2>&1 | tr '\n' '|')"
if [ "$WITH_PATH" = yes ]; then
    echo "@@set=$("$SATUR8" set 1.5 2>&1 | tr '\n' '|')"
    echo "@@readback=$(qdbus6 org.kde.KWin /org/kde/KWin/Effect/Satur81 org.kde.kwin.Effect.Satur8.saturation 2>&1)"
    echo "@@off=$("$SATUR8" off 2>&1 | tr '\n' '|')"
    echo "@@loaded-after-off=$(qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.isEffectLoaded satur8)"
fi
echo "@@stamp=$(cat "$HOME/.local/state/satur8/kwin-effect-source-version" 2>/dev/null)"
INNER

    local out; out="$(cat "$work/$name/out.txt")"
    get() { printf '%s\n' "$out" | sed -n "s/^@@$1=//p"; }

    if [ "$system_good" = yes ]; then
        # A system effect that already matches this KWin is installed on the
        # test machine, and the sandboxed KWin can see it. The right repair is
        # then to delete the stale per-user copy that shadows it, not to build.
        if [ "$with_path" = yes ]; then
            expect "KWin rejects the stale per-user effect" "false"                    "$(get supported-before)"
            expect "repair drops the shadowing copy"        "matches the running KWin" "$(get repair1)"
            expect "KWin accepts the system effect"         "true"                     "$(get supported-after)"
            expect "saturation reads back as set"           "1.5"                      "$(get readback)"
            expect "effect unloads on off"                  "false"                    "$(get loaded-after-off)"
            if [ -e "$home/.local/lib/qt6/plugins/kwin/effects/plugins/satur8.so" ]; then
                echo "  FAIL stale per-user copy was left in place"; fails=$((fails + 1))
            else
                echo "  ok   stale per-user copy removed"
            fi
        else
            expect "nothing to do when the system effect is good" "matches the running KWin" "$(get repair1)"
        fi
    elif [ "$with_path" = yes ]; then
        expect "KWin rejects the stale effect"         "false"                     "$(get supported-before)"
        expect "doctor names the mismatch"             "running KWin rejects"      "$(get doctor)"
        expect "doctor points at repair"               "satur8 repair"             "$(get doctor)"
        expect "repair rebuilds and reports ready"     "rebuilt the KWin effect for KWin $real_abi; it is ready to use" "$(get repair1)"
        expect "KWin accepts the rebuilt effect"       "true"                      "$(get supported-after)"
        expect "second repair is a no-op"              "matches the running KWin"  "$(get repair2)"
        expect "saturation reads back as set"          "1.5"                       "$(get readback)"
        expect "effect unloads on off"                 "false"                     "$(get loaded-after-off)"
        expect "source-version stamp written"          "."                         "$(get stamp)"
    else
        expect "repair rebuilds but asks for a login"  "log out and back in once"  "$(get repair1)"
        expect "second repair does not rebuild again"  "already built for KWin $real_abi" "$(get repair2)"
    fi
}

# The sandboxed KWin still sees the machine's real system plugin dir, so which
# repair is correct depends on whether a matching system effect is installed.
system_good=no
for root in "$(qtpaths6 --query QT_INSTALL_PLUGINS 2>/dev/null)" /usr/lib/qt6/plugins /usr/lib64/qt6/plugins; do
    f="$root/kwin/effects/plugins/satur8.so"
    if [ -f "$f" ] && grep -qa "EffectPluginFactory${real_abi//./\\.}" "$f"; then
        system_good=yes
    fi
done
echo "system effect matches KWin $real_abi: $system_good"

run_case with-plugin-path yes
run_case without-plugin-path no

# ------------------------------------------- repair from the set, no build tools

# satur8 repair runs only kwin_wayland, qtpaths6 or qmake6, and cmake. This
# PATH has the first three and a curl that records being run.
nobuild="$work/nobuild-bin"
mkdir -p "$nobuild"
for tool in kwin_wayland qtpaths6 qmake6; do
    tool_path="$(command -v "$tool" 2>/dev/null)" && ln -sf "$tool_path" "$nobuild/$tool"
done
curl_marker="$work/curl-was-run"
printf '#!/bin/sh\ntouch "%s"\nexit 1\n' "$curl_marker" > "$nobuild/curl"
chmod +x "$nobuild/curl"

run_store_case() { # run_store_case <name> <set has a match: yes|no>
    local name="$1" match="$2"
    local home="$work/$name/home"
    local plugins="$home/.local/lib/qt6/plugins"
    local user_effect="$plugins/kwin/effects/plugins/satur8.so"
    mkdir -p "$plugins/kwin/effects/plugins"
    install -m755 "$work/stale-satur8.so" "$user_effect"
    printf 'NAME="SteamOS"\nID=steamos\nVERSION_ID=3.9.5\n' > "$work/$name/os-release"
    local store_override=""
    if [ "$match" = yes ]; then
        # Where the installer leaves the set.
        install -Dm755 "$stale_build/satur8.so" \
            "$home/.local/share/satur8/kwin-effects/steamos-$real_abi/satur8.so"
    else
        # Through SATUR8_EFFECT_STORE, holding only an effect for another KWin.
        store_override="$work/$name/store"
        install -Dm755 "$work/stale-satur8.so" "$store_override/steamos-$fake_abi/satur8.so"
    fi
    rm -f "$curl_marker"

    echo "== $name"
    HOME="$home" SATUR8="$SATUR8" REPO="$repo" KWIN_PLUGIN_PATH="$plugins" \
    NOBUILD="$nobuild" STORE="$store_override" OS_RELEASE="$work/$name/os-release" \
    LOG="$work/$name/kwin.log" \
    dbus-run-session -- bash -s <<'INNER' > "$work/$name/out.txt" 2>&1
set -uo pipefail
unset XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME
export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=KDE
export SATUR8_EFFECT_SOURCE="$REPO/assets/kwin-effect" SATUR8_OS_RELEASE="$OS_RELEASE"
if [ -n "$STORE" ]; then
    export SATUR8_EFFECT_STORE="$STORE"
fi

env -u WAYLAND_DISPLAY -u DISPLAY QT_PLUGIN_PATH="$KWIN_PLUGIN_PATH" \
    kwin_wayland --virtual --width 1280 --height 720 >"$LOG" 2>&1 &
KWIN=$!
trap 'kill $KWIN 2>/dev/null' EXIT
for _ in $(seq 1 80); do
    qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.loadedEffects >/dev/null 2>&1 && break
    sleep 0.25
done

satur8() { PATH="$NOBUILD" "$SATUR8" "$@" 2>&1; }
echo "@@supported-before=$(qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.isEffectSupported satur8)"
out="$(satur8 repair)"; rc=$?
echo "@@repair1=$(printf '%s' "$out" | tr '\n' '|')"
echo "@@rc1=$rc"
echo "@@supported-after=$(qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.isEffectSupported satur8)"
echo "@@repair2=$(satur8 repair | tr '\n' '|')"
echo "@@doctor=$(satur8 doctor | tr '\n' '|')"
echo "@@set=$(satur8 set 1.5 | tr '\n' '|')"
echo "@@readback=$(qdbus6 org.kde.KWin /org/kde/KWin/Effect/Satur81 org.kde.kwin.Effect.Satur8.saturation 2>&1)"
echo "@@off=$(satur8 off | tr '\n' '|')"
echo "@@stamp=$(cat "$HOME/.local/state/satur8/kwin-effect-source-version" 2>/dev/null)"
INNER

    local out; out="$(cat "$work/$name/out.txt")"
    get() { printf '%s\n' "$out" | sed -n "s/^@@$1=//p"; }

    if [ -e "$curl_marker" ]; then
        echo "  FAIL repair ran curl"; fails=$((fails + 1))
    else
        echo "  ok   repair did not run curl"
    fi

    if [ "$system_good" = yes ]; then
        # The sandboxed KWin sees this machine's matching system effect, so the
        # right repair drops the stale per-user copy and the set is not used.
        echo "  note a matching system effect is installed here, so the set path is not exercised"
        expect "repair drops the shadowing copy" "matches the running KWin" "$(get repair1)"
    elif [ "$match" = yes ]; then
        expect "KWin rejects the stale effect"      "false" "$(get supported-before)"
        expect "repair installs it from the set"    "installed the KWin effect for KWin $real_abi from the set that came with Satur8; it is ready to use" "$(get repair1)"
        expect "repair exits 0"                     "0"     "$(get rc1)"
        expect "KWin accepts the effect from the set" "true" "$(get supported-after)"
        expect "second repair is a no-op"           "matches the running KWin" "$(get repair2)"
        expect "doctor names the set entry"         "from the set that came with Satur8 (steamos-$real_abi)" "$(get doctor)"
        expect "saturation reads back as set"       "1.5"   "$(get readback)"
        expect "source-version stamp written"       "."     "$(get stamp)"
        if cmp -s "$user_effect" "$stale_build/satur8.so"; then
            echo "  ok   the per-user effect is the set's file"
        else
            echo "  FAIL the per-user effect is not the set's file"; fails=$((fails + 1))
        fi
    else
        expect "repair says there is no effect here" "has no KWin effect for KWin $real_abi" "$(get repair1)"
        expect "repair points at a newer release"    "Install a newer Satur8 release" "$(get repair1)"
        if [ "$(get rc1)" != 0 ]; then
            echo "  ok   repair exits non-zero"
        else
            echo "  FAIL repair exited 0 with nothing it could do"; fails=$((fails + 1))
        fi
        if printf '%s' "$(get repair1)" | grep -qi "install cmake"; then
            echo "  FAIL SteamOS was told to install cmake"; fails=$((fails + 1))
        else
            echo "  ok   SteamOS is not told to install cmake"
        fi
        if cmp -s "$user_effect" "$work/stale-satur8.so"; then
            echo "  ok   the per-user effect was left alone"
        else
            echo "  FAIL the per-user effect was changed"; fails=$((fails + 1))
        fi
    fi
}

run_store_case set-has-match yes
run_store_case set-has-no-match no

echo
if [ "$fails" -eq 0 ]; then
    echo "PASS: all checks passed"
else
    echo "FAILED: $fails check(s); see $work/*/out.txt and kwin.log"
    exit 1
fi
