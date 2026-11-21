#!/usr/bin/env bash
# Render-level regression test for the KWin effect. It starts a private D-Bus
# session and virtual KWin output, so it never touches the live desktop.
set -euo pipefail

SATUR8_BIN="${1:-$PWD/target/debug/satur8}"
PLUGIN="${2:-/usr/lib/qt6/plugins/kwin/effects/plugins/satur8.so}"
ROOT="$(mktemp -d /tmp/satur8-render-test.XXXXXX)"

if [ ! -x "$SATUR8_BIN" ]; then
    echo "FAIL: satur8 binary not executable: $SATUR8_BIN" >&2
    exit 1
fi
if [ ! -f "$PLUGIN" ]; then
    echo "FAIL: KWin effect not found: $PLUGIN" >&2
    exit 1
fi

mkdir -p "$ROOT/plugins/kwin/effects/plugins" "$ROOT/config" "$ROOT/data"
install -m755 "$PLUGIN" "$ROOT/plugins/kwin/effects/plugins/satur8.so"

dbus-run-session -- bash -s "$ROOT" "$SATUR8_BIN" <<'INNER'
set -euo pipefail
ROOT="$1"
SATUR8_BIN="$2"
SOCKET="satur8-render-test-$$"

export WAYLAND_DISPLAY="$SOCKET"
export QT_QPA_PLATFORM=wayland
export QT_PLUGIN_PATH="$ROOT/plugins"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_DATA_HOME="$ROOT/data"

kwin_wayland --virtual --width 640 --height 480 --socket "$SOCKET" \
    --no-lockscreen --no-global-shortcuts --no-kactivities \
    >"$ROOT/kwin.log" 2>&1 &
KWIN_PID=$!
cleanup() { kill "$KWIN_PID" "${CLIENT_PID:-}" 2>/dev/null || true; }
trap cleanup EXIT

ready=false
for _ in $(seq 1 80); do
    if qdbus6 org.kde.KWin /Effects \
        org.kde.kwin.Effects.isEffectLoaded satur8 >/dev/null 2>&1; then
        ready=true
        break
    fi
    sleep 0.1
done
if [ "$ready" != true ]; then
    echo "FAIL: isolated KWin did not start" >&2
    tail -n 40 "$ROOT/kwin.log" >&2
    exit 1
fi

konsole -e bash -lc \
    'printf "\033[41m                    \033[0m\nSATUR8 RENDER TEST\n"; sleep 20' \
    >"$ROOT/client.log" 2>&1 &
CLIENT_PID=$!
sleep 2

spectacle -b -n -f -o "$ROOT/before.png"
"$SATUR8_BIN" set 1.0 >"$ROOT/satur8.log" 2>&1
sleep 1
if qdbus6 org.kde.KWin /Effects org.freedesktop.DBus.Properties.Get \
    org.kde.kwin.Effects activeEffects | grep -qw satur8; then
    echo "FAIL: identity saturation left the KWin effect active" >&2
    exit 1
fi
spectacle -b -n -f -o "$ROOT/identity.png"
"$SATUR8_BIN" set 0.0 >>"$ROOT/satur8.log" 2>&1
sleep 1
if ! qdbus6 org.kde.KWin /Effects org.freedesktop.DBus.Properties.Get \
    org.kde.kwin.Effects activeEffects | grep -qw satur8; then
    echo "FAIL: non-identity saturation did not activate the KWin effect" >&2
    exit 1
fi
spectacle -b -n -f -o "$ROOT/grayscale.png"
"$SATUR8_BIN" set 2.0 >>"$ROOT/satur8.log" 2>&1
sleep 1
spectacle -b -n -f -o "$ROOT/boosted.png"
"$SATUR8_BIN" off >>"$ROOT/satur8.log" 2>&1
sleep 1
spectacle -b -n -f -o "$ROOT/restored.png"

python3 - "$ROOT/before.png" "$ROOT/identity.png" "$ROOT/grayscale.png" \
    "$ROOT/boosted.png" "$ROOT/restored.png" <<'PY'
import sys
from PIL import Image, ImageChops, ImageStat

before = Image.open(sys.argv[1]).convert("RGB")
identity = Image.open(sys.argv[2]).convert("RGB")
grayscale = Image.open(sys.argv[3]).convert("RGB")
boosted = Image.open(sys.argv[4]).convert("RGB")
restored = Image.open(sys.argv[5]).convert("RGB")

def mean_chroma(image):
    pixels = image.getdata()
    return sum(max(pixel) - min(pixel) for pixel in pixels) / (image.width * image.height)

before_mean = sum(ImageStat.Stat(before).mean) / 3
identity_mean = sum(ImageStat.Stat(identity).mean) / 3
diff_mean = sum(ImageStat.Stat(ImageChops.difference(before, identity)).mean) / 3
restore_diff = sum(ImageStat.Stat(ImageChops.difference(before, restored)).mean) / 3
before_chroma = mean_chroma(before)
grayscale_chroma = mean_chroma(grayscale)
boosted_chroma = mean_chroma(boosted)

print(f"before_mean={before_mean:.3f}")
print(f"identity_mean={identity_mean:.3f}")
print(f"identity_diff_mean={diff_mean:.3f}")
print(f"before_chroma={before_chroma:.3f}")
print(f"grayscale_chroma={grayscale_chroma:.3f}")
print(f"boosted_chroma={boosted_chroma:.3f}")
print(f"restore_diff_mean={restore_diff:.3f}")

if before_mean < 5:
    raise SystemExit("FAIL: control screenshot is unexpectedly black")
if identity_mean < before_mean * 0.75:
    raise SystemExit("FAIL: identity saturation darkened the virtual output")
if diff_mean > 8:
    raise SystemExit("FAIL: identity saturation materially changed the rendered output")
if grayscale_chroma > max(1.0, before_chroma * 0.15):
    raise SystemExit("FAIL: zero saturation did not produce grayscale output")
if boosted_chroma <= before_chroma * 1.05:
    raise SystemExit("FAIL: boosted saturation did not increase rendered chroma")
if restore_diff > 8:
    raise SystemExit("FAIL: turning Satur8 off did not restore the rendered output")
PY

echo "PASS: identity, grayscale, boost, and restore rendering"
INNER

echo "artifacts: $ROOT"
