#!/usr/bin/env bash
# Satur8 installer.
#
# Default is a per-user install (no root): binaries to ~/.local/bin, the KWin
# effect/script and GNOME extension to the user data dirs, and a user systemd
# unit for the focus daemon. The KWin *effect* is a Qt plugin, and KWin only
# scans the plugin path it had at startup, so the installer also drops an
# environment.d file that puts the per-user plugin dir on that path. It takes
# effect at the next login. --system installs the effect to the system plugin
# dir instead, which works immediately.
#
# Either way the effect source is installed too, so `satur8 repair` (run by the
# daemon at every session start) can rebuild the effect after a KWin update.
#
# Usage:
#   packaging/install.sh                 # per-user install
#   packaging/install.sh --system        # system install (uses sudo)
#   packaging/install.sh --uninstall     # remove a per-user install
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
prebuilt="$repo/prebuilt/linux-x86_64"
mode="user"
action="install"
effect_to_check=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --system) mode="system"; shift ;;
        --uninstall) action="uninstall"; shift ;;
        --check-effect)
            action="check-effect"
            effect_to_check="${2:?--check-effect requires a plugin path}"
            shift 2
            ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

bindir="$HOME/.local/bin"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
qt_plugin_user="$HOME/.local/lib/qt6/plugins/kwin/effects/plugins"
qt_plugin_root=""
if command -v qmake6 >/dev/null 2>&1; then
    qt_plugin_root="$(qmake6 -query QT_INSTALL_PLUGINS 2>/dev/null || true)"
elif command -v qtpaths6 >/dev/null 2>&1; then
    qt_plugin_root="$(qtpaths6 --query QT_INSTALL_PLUGINS 2>/dev/null || true)"
fi
qt_plugin_sys="${qt_plugin_root:-/usr/lib/qt6/plugins}/kwin/effects/plugins"
kwin_script_dir="$data/kwin/scripts/satur8-focus"
gnome_ext_dir="$data/gnome-shell/extensions/satur8@satur8.github.io"
unit_dir="$HOME/.config/systemd/user"
effect_src_dir="$data/satur8/kwin-effect"
env_dir="${XDG_CONFIG_HOME:-$HOME/.config}/environment.d"
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/satur8"

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }

daemon_was_active=false

quiesce_runtime() {
    if command -v systemctl >/dev/null 2>&1 \
        && systemctl --user is-active --quiet satur8-daemon.service 2>/dev/null; then
        daemon_was_active=true
        systemctl --user stop satur8-daemon.service
    fi
    if command -v qdbus6 >/dev/null 2>&1; then
        qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.unloadEffect satur8 \
            >/dev/null 2>&1 || true
    fi
}

resume_runtime() {
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload >/dev/null 2>&1 || true
        if [ "$daemon_was_active" = true ]; then
            systemctl --user start satur8-daemon.service
        fi
    fi
}

runtime_kwin_abi() {
    if command -v kwin_wayland >/dev/null 2>&1; then
        kwin_wayland --version 2>/dev/null | sed -n '1s/.* //p'
        return
    fi
    sed -n 's/^#define KWIN_PLUGIN_VERSION_STRING "\(.*\)"/\1/p' \
        /usr/include/kwin/config-kwin.h 2>/dev/null | head -n1
}

effect_kwin_abi() {
    { strings "$1" 2>/dev/null \
        | grep -oE 'org\.kde\.kwin\.EffectPluginFactory[0-9]+(\.[0-9]+)+' \
        | sed 's/^org\.kde\.kwin\.EffectPluginFactory//' \
        | head -n1; } || true
}

check_effect_abi() {
    local effect="$1" runtime effect_abi
    runtime="$(runtime_kwin_abi)"
    effect_abi="$(effect_kwin_abi "$effect")"
    if [ -z "$runtime" ]; then
        echo "could not determine the installed KWin version" >&2
        return 1
    fi
    if [ -z "$effect_abi" ]; then
        echo "could not determine the KWin ABI of $effect" >&2
        return 1
    fi
    if [ "$effect_abi" != "$runtime" ]; then
        echo "KWin effect ABI mismatch: plugin $effect_abi, runtime $runtime" >&2
        return 1
    fi
    echo "KWin effect ABI $effect_abi matches the runtime"
}

has_prebuilt_binaries() {
    [ "$(uname -m)" = x86_64 ] &&
    [ -x "$prebuilt/bin/satur8" ] &&
    [ -x "$prebuilt/bin/satur8-daemon" ] &&
    [ -x "$prebuilt/bin/satur8-gui" ]
}

build_binaries() {
    say "Building release binaries"
    (cd "$repo" && cargo build --release)
    bin_src="$repo/target/release"
}

build_effect() {
    if [ ! -f "$repo/assets/kwin-effect/CMakeLists.txt" ]; then
        echo "the bundled KWin effect is incompatible and this archive lacks its source" >&2
        echo "install Satur8 from a distro package or use a release archive that can rebuild for this KWin version" >&2
        exit 1
    fi
    for command in cmake c++; do
        if ! command -v "$command" >/dev/null 2>&1; then
            echo "cannot rebuild the KWin effect: missing $command" >&2
            exit 1
        fi
    done
    if [ ! -f /usr/include/kwin/config-kwin.h ]; then
        echo "cannot rebuild the KWin effect: KWin development headers are not installed" >&2
        exit 1
    fi
    say "Building the KWin effect"
    local effect_build="$repo/target/kwin-effect-install"
    cmake --fresh -S "$repo/assets/kwin-effect" -B "$effect_build" \
        -DCMAKE_BUILD_TYPE=Release >/dev/null
    cmake --build "$effect_build" >/dev/null
    effect_so="$effect_build/satur8.so"
    check_effect_abi "$effect_so" >/dev/null
}

prepare_artifacts() {
    if has_prebuilt_binaries; then
        say "Using bundled linux-x86_64 release binaries"
        bin_src="$prebuilt/bin"
    else
        build_binaries
    fi

    local prebuilt_effect="$prebuilt/kwin/satur8.so"
    if [ -f "$prebuilt_effect" ] && check_effect_abi "$prebuilt_effect" >/dev/null 2>&1; then
        say "Using bundled KWin effect for ABI $(runtime_kwin_abi)"
        effect_so="$prebuilt_effect"
    else
        if [ -f "$prebuilt_effect" ]; then
            say "Bundled KWin effect does not match KWin $(runtime_kwin_abi); rebuilding locally"
        fi
        build_effect
    fi
}

install_all() {
    prepare_artifacts
    satur8_version="$("$bin_src/satur8" --version 2>/dev/null | awk '{print $NF}')"
    quiesce_runtime
    trap resume_runtime EXIT
    say "Installing binaries to $bindir"
    mkdir -p "$bindir"
    install -m755 "$bin_src/satur8" "$bindir/satur8"
    install -m755 "$bin_src/satur8-daemon" "$bindir/satur8-daemon"
    [ -f "$bin_src/satur8-tray" ] && \
        install -m755 "$bin_src/satur8-tray" "$bindir/satur8-tray"
    [ -f "$bin_src/satur8-gui" ] && \
        install -m755 "$bin_src/satur8-gui" "$bindir/satur8-gui"

    if [ "$mode" = system ]; then
        say "Installing KWin effect to $qt_plugin_sys (sudo)"
        sudo install -Dm755 "$effect_so" "$qt_plugin_sys/satur8.so"
        if [ -f "$qt_plugin_user/satur8.so" ]; then
            say "Removing stale per-user KWin effect so it cannot shadow the system copy"
            rm -f "$qt_plugin_user/satur8.so" "$state_dir/kwin-effect-source-version"
        fi
    else
        say "Installing KWin effect to $qt_plugin_user"
        install -Dm755 "$effect_so" "$qt_plugin_user/satur8.so"
        mkdir -p "$state_dir"
        printf '%s' "$satur8_version" > "$state_dir/kwin-effect-source-version"
        echo "    NOTE: KWin picks up the per-user effect at your next login."
        echo "    Log out and back in once, or re-run with --system to use it now."
    fi

    # Everything `satur8 repair` needs to rebuild the effect after a KWin update.
    if [ -f "$repo/assets/kwin-effect/CMakeLists.txt" ]; then
        say "Installing KWin effect source to $effect_src_dir"
        mkdir -p "$effect_src_dir"
        for f in CMakeLists.txt metadata.json satur8_effect.cpp satur8_effect.h; do
            install -m644 "$repo/assets/kwin-effect/$f" "$effect_src_dir/$f"
        done
    fi
    say "Adding the per-user Qt plugin dir to the session environment"
    install -Dm644 "$repo/packaging/60-satur8.conf" "$env_dir/60-satur8.conf"

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
  * GUI:      satur8-gui
  * Manual:   satur8 on 1.5      (load + boost)   /   satur8 off
  * Per-game focus mode:
      systemctl --user enable --now satur8-daemon
      kwriteconfig6 --file kwinrc --group Plugins --key satur8-focusEnabled true
      qdbus6 org.kde.KWin /KWin reconfigure
EOF

    resume_runtime
    trap - EXIT
}

uninstall_all() {
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now satur8-daemon.service >/dev/null 2>&1 || true
    fi
    if command -v qdbus6 >/dev/null 2>&1; then
        qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.unloadEffect satur8 \
            >/dev/null 2>&1 || true
    fi
    say "Removing per-user install"
    rm -f "$bindir/satur8" "$bindir/satur8-daemon" "$bindir/satur8-tray" "$bindir/satur8-gui"
    rm -f "$qt_plugin_user/satur8.so" "$env_dir/60-satur8.conf"
    rm -rf "$kwin_script_dir" "$gnome_ext_dir" "$effect_src_dir" "$state_dir"
    rm -f "$unit_dir/satur8-daemon.service" "$data/applications/satur8.desktop"
    echo "    (system-installed effect at $qt_plugin_sys left alone; remove with sudo if needed)"
}

case "$action" in
    install) install_all ;;
    uninstall) uninstall_all ;;
    check-effect) check_effect_abi "$effect_to_check" ;;
esac
