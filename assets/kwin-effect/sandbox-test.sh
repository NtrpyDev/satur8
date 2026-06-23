#!/usr/bin/env bash
# Validate the vibrance CLI <-> KWin backend end to end against a HEADLESS,
# fully isolated KWin instance (its own D-Bus bus + virtual framebuffer). This
# touches nothing on the user's real desktop.
set -uo pipefail

VIB="${1:-$HOME/vibrance/target/debug/vibrance}"

dbus-run-session -- bash -s "$VIB" <<'INNER'
set -uo pipefail
VIB="$1"

# Pure headless: ignore any parent display, render to a virtual framebuffer.
env -u WAYLAND_DISPLAY -u DISPLAY \
    kwin_wayland --virtual --width 1920 --height 1080 >/tmp/kwin-sandbox.log 2>&1 &
KWIN=$!
trap 'kill $KWIN 2>/dev/null' EXIT

# Wait for KWin to own org.kde.KWin on this private bus.
ok=0
for _ in $(seq 1 60); do
    if qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.loadedEffects >/dev/null 2>&1; then
        ok=1; break
    fi
    sleep 0.25
done
if [ "$ok" != 1 ]; then
    echo "FAIL: headless KWin never came up on the private bus"
    tail -5 /tmp/kwin-sandbox.log
    exit 1
fi
echo "headless KWin up (pid $KWIN), private bus = ${DBUS_SESSION_BUS_ADDRESS%%,*}"
echo

echo "### vibrance status (before)"; "$VIB" status; echo
echo "### vibrance set 1.8";        "$VIB" set 1.8; echo
echo "### read back saturation";    qdbus6 org.kde.KWin /org/kde/KWin/Effect/Vibrance1 org.kde.kwin.Effect.Vibrance.saturation
echo "### vibrance status (on)";    "$VIB" status; echo
echo "### vibrance off";            "$VIB" off; echo
echo "### effect loaded now?";      qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.isEffectLoaded vibrance
INNER
