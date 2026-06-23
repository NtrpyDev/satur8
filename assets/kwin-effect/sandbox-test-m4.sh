#!/usr/bin/env bash
# M4 validation: the focus daemon applies/restores profiles in response to the
# WindowActivated calls the KWin script would make. Headless KWin + isolated
# config + private bus. Nothing touches the user's desktop.
set -uo pipefail

VIB="${1:-$HOME/vibrance/target/debug/vibrance}"
DAEMON="${2:-$HOME/vibrance/target/debug/vibrance-daemon}"
CFG="$(mktemp -d)"

dbus-run-session -- bash -s "$VIB" "$DAEMON" "$CFG" <<'INNER'
set -uo pipefail
VIB="$1"; DAEMON="$2"; export XDG_CONFIG_HOME="$3"

env -u WAYLAND_DISPLAY -u DISPLAY \
    kwin_wayland --virtual --width 1920 --height 1080 >/tmp/kwin-sandbox-m4.log 2>&1 &
KWIN=$!
trap 'kill $KWIN $DPID 2>/dev/null' EXIT
for _ in $(seq 1 60); do
    qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.loadedEffects >/dev/null 2>&1 && break
    sleep 0.25
done

sat() { qdbus6 org.kde.KWin /org/kde/KWin/Effect/Vibrance1 org.kde.kwin.Effect.Vibrance.saturation 2>/dev/null; }
loaded() { qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.isEffectLoaded vibrance; }
activate() { qdbus6 org.vibrance.Daemon /org/vibrance/Daemon org.vibrance.Daemon.WindowActivated "$1" "$2"; }
active_profile() { qdbus6 org.vibrance.Daemon /org/vibrance/Daemon org.vibrance.Daemon.activeProfile 2>/dev/null; }

# A game profile keyed by window class.
"$VIB" profile add cs2 1.6 --window-class cs2 >/dev/null

# Start the daemon (it reads the same profiles file).
"$DAEMON" >/tmp/vibrance-daemon-m4.log 2>&1 &
DPID=$!
for _ in $(seq 1 40); do
    qdbus6 org.vibrance.Daemon /org/vibrance/Daemon org.vibrance.Daemon.activeProfile >/dev/null 2>&1 && break
    sleep 0.25
done

echo "### focus the game window (class cs2)"
activate "cs2" "Counter-Strike 2"
sleep 0.4
echo "loaded=$(loaded)  saturation=$(sat)  active_profile=$(active_profile)  (expect true / 1.6 / cs2)"
echo

echo "### focus a non-game window (class firefox)"
activate "firefox" "Mozilla Firefox"
sleep 0.4
echo "loaded=$(loaded)  active_profile='$(active_profile)'  (expect false / empty = restored)"
echo

echo "### focus the game again"
activate "cs2" "Counter-Strike 2"
sleep 0.4
echo "loaded=$(loaded)  saturation=$(sat)  active_profile=$(active_profile)  (expect true / 1.6 / cs2)"
echo

echo "### daemon log:"; sed 's/^/    /' /tmp/vibrance-daemon-m4.log
INNER

rm -rf "$CFG"
