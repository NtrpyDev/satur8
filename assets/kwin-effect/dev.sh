#!/usr/bin/env bash
# Build, install and (re)load the Satur8 KWin effect on a live KDE Wayland
# session. Usage:
#   ./dev.sh build              # configure + compile only
#   ./dev.sh install            # system install via pkexec (KWin scans it now)
#   ./dev.sh install-user       # ~/.local install (needs QT_PLUGIN_PATH at login)
#   ./dev.sh load | unload | reload
#   ./dev.sh set <0.0..4.0>     # live saturation
#   ./dev.sh all <sat>          # build + install + reload + set
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
build_dir="$here/build"
so="$build_dir/satur8.so"
effect_id="satur8"

# KWin already has this on its Qt plugin search path, so a system install is
# picked up by loadEffect immediately - no compositor restart, no relogin.
sys_dir="/usr/lib/qt6/plugins/kwin/effects/plugins"
user_dir="$HOME/.local/lib/qt6/plugins/kwin/effects/plugins"

dbus_effects() { qdbus6 org.kde.KWin /Effects "org.kde.kwin.Effects.$1" "${@:2}"; }

cmd_build() {
    cmake -S "$here" -B "$build_dir" -DCMAKE_BUILD_TYPE=Release >/dev/null
    cmake --build "$build_dir"
}

cmd_install() {
    # Unattended root install. Primes sudo from the stored password if present,
    # otherwise falls back to an interactive prompt.
    local pass="$HOME/.config/satur8/sudo-pass"
    if [ -r "$pass" ]; then
        sudo -S -v < "$pass" 2>/dev/null
    fi
    sudo sh -c "mkdir -p '$sys_dir' && cp '$so' '$sys_dir/satur8.so' && chmod 0755 '$sys_dir/satur8.so'"
    echo "installed -> $sys_dir"
}

cmd_install_user() {
    mkdir -p "$user_dir"
    cp "$so" "$user_dir/satur8.so"
    echo "installed -> $user_dir (add this dir's root to QT_PLUGIN_PATH and re-login)"
}

cmd_load()   { dbus_effects loadEffect "$effect_id"; }
cmd_unload() { dbus_effects unloadEffect "$effect_id"; }
cmd_reload() { dbus_effects unloadEffect "$effect_id" || true; dbus_effects loadEffect "$effect_id"; }
cmd_loaded() { dbus_effects isEffectLoaded "$effect_id"; }
cmd_set()    { qdbus6 org.kde.KWin /org/kde/KWin/Effect/Satur81 org.kde.kwin.Effect.Satur8.setSaturation "$1"; }

case "${1:-}" in
    build)        cmd_build ;;
    install)      cmd_install ;;
    install-user) cmd_install_user ;;
    load)         cmd_load ;;
    unload)       cmd_unload ;;
    reload)       cmd_reload ;;
    loaded)       cmd_loaded ;;
    set)          cmd_set "${2:?usage: dev.sh set <0.0..4.0>}" ;;
    all)          cmd_build; cmd_install; cmd_reload; cmd_set "${2:-1.5}" ;;
    *) echo "usage: $0 {build|install|install-user|load|unload|reload|loaded|set <s>|all <s>}" >&2; exit 1 ;;
esac
