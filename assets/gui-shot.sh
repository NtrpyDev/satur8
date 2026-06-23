#!/usr/bin/env bash
# Render satur8-gui on a throwaway virtual X display and screenshot it, so it
# never appears on the real desktop. Intended to run as a background command
# (foreground sleep is blocked in this environment).
#   assets/gui-shot.sh [out.png] [nav]
set -uo pipefail

OUT="${1:-/tmp/gui_shot.png}"
NAV="${2:-0}"
DISP=":97"

# CRITICAL: unset WAYLAND_DISPLAY so winit cannot fall back to the real Wayland
# session. With only DISPLAY set + the X11 backend forced, the GUI can ONLY
# connect to the throwaway Xvfb below - it never reaches the real desktop.
unset WAYLAND_DISPLAY
export DISPLAY="$DISP"
Xvfb "$DISP" -screen 0 1280x880x24 >/tmp/xvfb.log 2>&1 &
XPID=$!
sleep 2

# Software renderer = no GL needed on the virtual display. VIBRANCE_GUI_NAV lets
# us screenshot a specific page.
WINIT_UNIX_BACKEND=x11 SLINT_BACKEND=winit-software VIBRANCE_GUI_NAV="$NAV" \
    "$HOME/satur8/target/debug/satur8-gui" >/tmp/gui-x.log 2>&1 &
GPID=$!
sleep 5

import -window root "$OUT" 2>/tmp/import.log \
    || (xwd -root -silent | convert xwd:- "$OUT")

kill "$GPID" "$XPID" 2>/dev/null
wait 2>/dev/null
echo "saved $OUT ($(identify -format '%wx%h' "$OUT" 2>/dev/null))"
