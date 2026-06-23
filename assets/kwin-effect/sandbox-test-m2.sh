#!/usr/bin/env bash
# M2 validation: profiles + launch wrapper, against a HEADLESS isolated KWin and
# an isolated config dir. Nothing touches the user's desktop or real config.
set -uo pipefail

VIB="${1:-$HOME/vibrance/target/debug/vibrance}"
CFG="$(mktemp -d)"

dbus-run-session -- bash -s "$VIB" "$CFG" <<'INNER'
set -uo pipefail
VIB="$1"; export XDG_CONFIG_HOME="$2"

env -u WAYLAND_DISPLAY -u DISPLAY \
    kwin_wayland --virtual --width 1920 --height 1080 >/tmp/kwin-sandbox-m2.log 2>&1 &
KWIN=$!
trap 'kill $KWIN 2>/dev/null' EXIT
for _ in $(seq 1 60); do
    qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.loadedEffects >/dev/null 2>&1 && break
    sleep 0.25
done

sat() { qdbus6 org.kde.KWin /org/kde/KWin/Effect/Vibrance1 org.kde.kwin.Effect.Vibrance.saturation 2>/dev/null; }
loaded() { qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.isEffectLoaded vibrance; }

echo "### profile add + list"
"$VIB" profile add cs2 1.6 --exe cs2 --steam-app-id 730
"$VIB" profile add dota 1.4 --exe dota2
"$VIB" profile list
echo

echo "### run --profile cs2 -- (a fake 'cs2' game = sleep). Expect 1.6 during, restored after."
"$VIB" run --profile cs2 -- sleep 3 &
RUNPID=$!
sleep 1.2
echo "during run:  loaded=$(loaded)  saturation=$(sat)"
wait $RUNPID
echo "after run:   loaded=$(loaded)  (expect false = restored)"
echo

echo "### run with auto-match by exe basename (command named 'dota2')"
ln -sf "$(command -v sleep)" "$XDG_CONFIG_HOME/dota2"
"$VIB" run -- "$XDG_CONFIG_HOME/dota2" 2 &
RUNPID=$!
sleep 1.0
echo "during run:  loaded=$(loaded)  saturation=$(sat)  (expect 1.4 from dota profile)"
wait $RUNPID
echo "after run:   loaded=$(loaded)  (expect false)"
INNER

rm -rf "$CFG"
