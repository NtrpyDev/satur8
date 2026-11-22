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
#   packaging/install.sh --enable-daemon # also turn on the background service
#   packaging/install.sh --uninstall     # remove a per-user install
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
prebuilt="$repo/prebuilt/linux-x86_64"
mode="user"
action="install"
effect_to_check=""
enable_daemon=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --system) mode="system"; shift ;;
        --enable-daemon) enable_daemon=true; shift ;;
        --uninstall) action="uninstall"; shift ;;
        --check-effect)
            action="check-effect"
            effect_to_check="${2:?--check-effect requires a plugin path}"
            shift 2
            ;;
        -h|--help) sed -n '2,21p' "$0"; exit 0 ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

is_steamos() {
    grep -Eq '^ID="?steamos"?$' /etc/os-release 2>/dev/null
}

# Booted from an ostree image (Bazzite, Fedora Atomic): /usr is a read-only
# mount, so the system plugin dir cannot be written. ostree creates this file
# at boot.
is_ostree() {
    [ -e /run/ostree-booted ]
}

# SteamOS and ostree image-based systems support only the per-user install:
# everything it writes is under $HOME, which their OS updates leave alone.
per_user_only() {
    is_steamos || is_ostree
}

if [ "$action" = install ] && [ "$mode" = system ] && per_user_only; then
    echo "--system is not supported on SteamOS or on image-based systems such as Bazzite." >&2
    echo "Run packaging/install.sh without --system for a per-user install." >&2
    exit 1
fi

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

# Whether this machine has KWin at all. The effect only matters on KDE Plasma;
# GNOME, Hyprland, NVIDIA X11 and gamescope sessions use other backends.
kwin_present() {
    command -v kwin_wayland >/dev/null 2>&1 ||
    command -v kwin_x11 >/dev/null 2>&1 ||
    [ -f /usr/include/kwin/config-kwin.h ]
}

# Whether this machine has a Steam Game Mode (gamescope) session to log into:
# SteamOS's session files, or the gamescope-session-plus unit that ChimeraOS
# and Bazzite use.
has_gamescope_session() {
    [ -f /usr/share/wayland-sessions/gamescope-wayland.desktop ] ||
    [ -f /usr/lib/systemd/user/gamescope-session.service ] ||
    [ -f /usr/lib/systemd/user/gamescope-session-plus@.service ]
}

# Build the effect for the installed KWin. Returns non-zero, with the reason in
# effect_skip_reason, when it cannot: a missing effect must never stop the rest
# of Satur8 from installing. (This runs inside an `if`, where `set -e` is off,
# so every step checks its own result.)
build_effect() {
    effect_skip_reason=""
    if [ ! -f "$repo/assets/kwin-effect/CMakeLists.txt" ]; then
        effect_skip_reason="the bundled effect does not match this KWin and this archive lacks the effect source"
        return 1
    fi
    for command in cmake c++; do
        if ! command -v "$command" >/dev/null 2>&1; then
            effect_skip_reason="missing $command"
            return 1
        fi
    done
    if [ ! -f /usr/include/kwin/config-kwin.h ]; then
        effect_skip_reason="the KWin development headers are not installed"
        return 1
    fi
    say "Building the KWin effect"
    local effect_build="$repo/target/kwin-effect-install"
    if ! cmake --fresh -S "$repo/assets/kwin-effect" -B "$effect_build" \
        -DCMAKE_BUILD_TYPE=Release >/dev/null ||
        ! cmake --build "$effect_build" >/dev/null; then
        effect_skip_reason="the effect failed to compile against the installed KWin"
        return 1
    fi
    if ! check_effect_abi "$effect_build/satur8.so" >/dev/null 2>&1; then
        effect_skip_reason="the rebuilt effect does not match the installed KWin"
        return 1
    fi
    effect_so="$effect_build/satur8.so"
}

prepare_artifacts() {
    if has_prebuilt_binaries; then
        say "Using bundled linux-x86_64 release binaries"
        bin_src="$prebuilt/bin"
    else
        build_binaries
    fi

    # The KWin effect is optional: no KWin, or no way to build a matching
    # effect, skips it and installs everything else.
    effect_so=""
    effect_skip_reason=""
    local prebuilt_effect="$prebuilt/kwin/satur8.so"
    if ! kwin_present; then
        say "KWin not found: skipping the KWin effect (only KDE Plasma uses it)"
    elif [ -f "$prebuilt_effect" ] && check_effect_abi "$prebuilt_effect" >/dev/null 2>&1; then
        say "Using bundled KWin effect for ABI $(runtime_kwin_abi)"
        effect_so="$prebuilt_effect"
    else
        if [ -f "$prebuilt_effect" ]; then
            say "Bundled KWin effect does not match KWin $(runtime_kwin_abi); rebuilding locally"
        fi
        if ! build_effect; then
            effect_so=""
            say "Skipping the KWin effect: $effect_skip_reason"
        fi
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

    if [ -z "$effect_so" ]; then
        : # no effect to install on this machine; see prepare_artifacts
    elif [ "$mode" = system ]; then
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
        if per_user_only; then
            echo "    Log out and back in once."
        else
            echo "    Log out and back in once, or re-run with --system to use it now."
        fi
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
EOF
    # Per-game switching is set up differently in Steam Game Mode and on KDE
    # Plasma; a Steam Deck has both sessions, so it gets both blocks.
    if has_gamescope_session; then
        echo "  * Per-game switching in Steam Game Mode:"
        if [ "$enable_daemon" != true ]; then
            echo "      systemctl --user enable --now satur8-daemon"
        fi
        echo "      satur8 profile add NAME 1.5 --steam-app-id APPID"
    fi
    if kwin_present || ! has_gamescope_session; then
        cat <<EOF
  * Per-game focus mode on KDE Plasma:
      systemctl --user enable --now satur8-daemon
      kwriteconfig6 --file kwinrc --group Plugins --key satur8-focusEnabled true
      qdbus6 org.kde.KWin /KWin reconfigure
EOF
    fi
    case ":$PATH:" in
        *":$bindir:"*) ;;
        *) echo "  $bindir is not on your PATH here; run the commands as $bindir/satur8." ;;
    esac

    if [ -n "$effect_skip_reason" ] && has_gamescope_session; then
        echo
        echo "NOTE: the KWin effect was not installed ($effect_skip_reason)."
        echo "  Everything else is installed. Steam Game Mode does not use the KWin"
        echo "  effect, so Satur8 works there without it."
    elif [ -n "$effect_skip_reason" ] && is_ostree; then
        echo
        echo "NOTE: the KWin effect was not installed ($effect_skip_reason)."
        echo "  Everything else is installed. /usr is read-only on this system, so the"
        echo "  build tools could only be added by layering packages with rpm-ostree."
        echo "  A Satur8 release whose bundled effect matches your KWin installs it"
        echo "  without them."
    elif [ -n "$effect_skip_reason" ]; then
        echo
        echo "NOTE: the KWin effect was not installed ($effect_skip_reason)."
        echo "  Everything else is installed. On KDE Plasma, install cmake, a C++"
        echo "  compiler, and the Qt 6 and KWin development files, then run: satur8 repair"
    fi

    resume_runtime
    trap - EXIT

    if [ "$enable_daemon" = true ]; then
        say "Turning on the background service"
        systemctl --user enable --now satur8-daemon.service
    fi
}

uninstall_all() {
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now satur8-daemon.service >/dev/null 2>&1 || true
    fi
    # Put the colors back while the binary still exists. Stopping the daemon
    # restores too, but only when it was running.
    if [ -x "$bindir/satur8" ]; then
        "$bindir/satur8" off >/dev/null 2>&1 || true
    fi
    if command -v qdbus6 >/dev/null 2>&1; then
        qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.unloadEffect satur8 \
            >/dev/null 2>&1 || true
    fi
    say "Removing per-user install"
    rm -f "$bindir/satur8" "$bindir/satur8-daemon" "$bindir/satur8-tray" "$bindir/satur8-gui"
    rm -f "$qt_plugin_user/satur8.so" "$env_dir/60-satur8.conf"
    rm -rf "$kwin_script_dir" "$gnome_ext_dir" "$effect_src_dir" "$state_dir"
    # The effect source was the only thing in this folder; rmdir keeps it if
    # anything else is there.
    rmdir "$data/satur8" 2>/dev/null || true
    rm -f "$unit_dir/satur8-daemon.service" "$data/applications/satur8.desktop"
    if [ -e "$qt_plugin_sys/satur8.so" ]; then
        echo "    (system-installed effect at $qt_plugin_sys left alone; remove with sudo if needed)"
    fi
}

case "$action" in
    install) install_all ;;
    uninstall) uninstall_all ;;
    check-effect) check_effect_abi "$effect_to_check" ;;
esac
