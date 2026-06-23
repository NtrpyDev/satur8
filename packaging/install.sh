#!/usr/bin/env bash
# Satur8 installer.
#
# Default is a per-user install (no root): binaries to ~/.local/bin, the KWin
# effect/script and GNOME extension to the user data dirs, and a user systemd
# unit for the focus daemon. The one piece that benefits from root is the KWin
# *effect* (a Qt plugin): KWin only scans its plugin path discovered at startup,
# which on most distros is the system dir. We install it to the user Qt plugin
# path and tell you how to make KWin see it, or you can run with --system.
#
# Usage:
#   packaging/install.sh                 # per-user install
#   packaging/install.sh --system        # system install (uses sudo)
#   packaging/install.sh --uninstall     # remove a per-user install
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
mode="user"
action="install"
for arg in "$@"; do
    case "$arg" in
        --system) mode="system" ;;
        --uninstall) action="uninstall" ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) echo "unknown arg: $arg" >&2; exit 1 ;;
    esac
done

bindir="$HOME/.local/bin"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
qt_plugin_user="$HOME/.local/lib/qt6/plugins/kwin/effects/plugins"
qt_plugin_sys="/usr/lib/qt6/plugins/kwin/effects/plugins"
kwin_script_dir="$data/kwin/scripts/satur8-focus"
gnome_ext_dir="$data/gnome-shell/extensions/satur8@satur8.github.io"
unit_dir="$HOME/.config/systemd/user"

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }

build() {
    say "Building release binaries"
    (cd "$repo" && cargo build --release)
    say "Building the KWin effect"
    cmake -S "$repo/assets/kwin-effect" -B "$repo/assets/kwin-effect/build" \
        -DCMAKE_BUILD_TYPE=Release >/dev/null
    cmake --build "$repo/assets/kwin-effect/build" >/dev/null
}

install_all() {
    build
    say "Installing binaries to $bindir"
    mkdir -p "$bindir"
    install -m755 "$repo/target/release/satur8" "$bindir/satur8"
    install -m755 "$repo/target/release/satur8-daemon" "$bindir/satur8-daemon"
    [ -f "$repo/target/release/satur8-tray" ] && \
        install -m755 "$repo/target/release/satur8-tray" "$bindir/satur8-tray"
    [ -f "$repo/target/release/satur8-gui" ] && \
        install -m755 "$repo/target/release/satur8-gui" "$bindir/satur8-gui"

    local so="$repo/assets/kwin-effect/build/satur8.so"
    if [ "$mode" = system ]; then
        say "Installing KWin effect to $qt_plugin_sys (sudo)"
        sudo install -Dm755 "$so" "$qt_plugin_sys/satur8.so"
    else
        say "Installing KWin effect to $qt_plugin_user"
        install -Dm755 "$so" "$qt_plugin_user/satur8.so"
        echo "    NOTE: for KWin to find a user-path effect, ensure QT_PLUGIN_PATH"
        echo "    includes ~/.local/lib/qt6/plugins at login, or re-run with --system."
    fi

    say "Installing KWin focus script to $kwin_script_dir"
    mkdir -p "$kwin_script_dir"
    cp -r "$repo/assets/kwin-script/." "$kwin_script_dir/"

    say "Installing GNOME extension to $gnome_ext_dir"
    mkdir -p "$gnome_ext_dir"
    cp -r "$repo/assets/gnome-extension/satur8@satur8.github.io/." "$gnome_ext_dir/"

    say "Installing systemd user unit"
    mkdir -p "$unit_dir"
    install -m644 "$repo/packaging/satur8-daemon.service" "$unit_dir/satur8-daemon.service"

    say "Installing desktop entry"
    install -Dm644 "$repo/packaging/satur8.desktop" "$data/applications/satur8.desktop"

    cat <<EOF

Installed. Next steps:
  * Effect:   satur8 on 1.5      (load + boost)   /   satur8 off
  * Per-game: satur8 run --profile cs2 -- %command%   (Steam launch option)
  * Always-on focus mode:
      systemctl --user enable --now satur8-daemon
      kwriteconfig6 --file kwinrc --group Plugins --key satur8-focusEnabled true
      qdbus6 org.kde.KWin /KWin reconfigure
EOF
}

uninstall_all() {
    say "Removing per-user install"
    rm -f "$bindir/satur8" "$bindir/satur8-daemon" "$bindir/satur8-tray" "$bindir/satur8-gui"
    rm -f "$qt_plugin_user/satur8.so"
    rm -rf "$kwin_script_dir" "$gnome_ext_dir"
    rm -f "$unit_dir/satur8-daemon.service" "$data/applications/satur8.desktop"
    echo "    (system-installed effect at $qt_plugin_sys left alone; remove with sudo if needed)"
}

case "$action" in
    install) install_all ;;
    uninstall) uninstall_all ;;
esac
