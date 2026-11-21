#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALLER="$HERE/install.sh"
ROOT="$(mktemp -d /tmp/satur8-installer-test.XXXXXX)"
RUNTIME_ABI="$(kwin_wayland --version | awk 'NR == 1 { print $2 }')"

printf 'org.kde.kwin.EffectPluginFactory%s\n' "$RUNTIME_ABI" >"$ROOT/matching.so"
printf 'org.kde.kwin.EffectPluginFactory0.0.0\n' >"$ROOT/mismatched.so"

"$INSTALLER" --check-effect "$ROOT/matching.so"
if "$INSTALLER" --check-effect "$ROOT/mismatched.so"; then
    echo "FAIL: installer accepted a mismatched KWin effect" >&2
    exit 1
fi

echo "PASS: installer accepts only the current KWin effect ABI"
