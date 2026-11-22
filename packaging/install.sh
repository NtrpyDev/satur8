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
# A release archive carries prebuilt effects, one per system and KWin version,
# in prebuilt/linux-x86_64/kwin/effects/<key>/. The installer uses the one built
# for this system; else builds one; else uses one built for another system with
# the same KWin when this system's Qt is new enough. On a machine with KWin it
# copies the whole set to ~/.local/share/satur8/kwin-effects for `satur8 repair`.
#
# The effect source is installed too, so `satur8 repair` (run by the daemon at
# every session start) can rebuild the effect after a KWin update.
#
# Usage:
#   packaging/install.sh                 # per-user install
#   packaging/install.sh --system        # system install (uses sudo)
#   packaging/install.sh --enable-daemon # also turn on the background service
#   packaging/install.sh --uninstall     # remove a per-user install
#   packaging/install.sh --pick-effect   # print which KWin effect fits; change nothing
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
prebuilt="$repo/prebuilt/linux-x86_64"
# The bundled effects, one folder per key: <family>-<KWin version>, such as
# steamos-6.7.3 or fedora45-6.8.3.
effect_set="$prebuilt/kwin/effects"
# Names the system family. Tests point it at a fake file.
os_release="${SATUR8_OS_RELEASE:-/etc/os-release}"
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
        --pick-effect) action="pick-effect"; shift ;;
        -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

# One os-release field without its quotes; empty when the file or field is
# missing.
os_field() {
    local value
    value="$(sed -n "s/^$1=//p" "$os_release" 2>/dev/null | head -n1)" || value=""
    value="${value#\"}"
    value="${value%\"}"
    value="${value#\'}"
    value="${value%\'}"
    printf '%s' "$value"
}

is_steamos() {
    [ "$(os_field ID)" = steamos ]
}

# The family half of an effect key: steamos; fedora<NN> for Fedora and systems
# built from it, such as Bazzite (ID=bazzite, ID_LIKE="fedora"); else other.
system_family() {
    local id id_like version
    id="$(os_field ID)"
    id_like="$(os_field ID_LIKE)"
    version="$(os_field VERSION_ID)"
    if [ "$id" = steamos ]; then
        echo steamos
    elif { [ "$id" = fedora ] || [[ " $id_like " == *" fedora "* ]]; } &&
        [[ "$version" =~ ^[0-9]+$ ]]; then
        echo "fedora$version"
    else
        echo other
    fi
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
effect_set_dir="$data/satur8/kwin-effects"
env_dir="${XDG_CONFIG_HOME:-$HOME/.config}/environment.d"
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/satur8"
# The app icon, named satur8 as satur8.desktop's Icon= says, in the
# freedesktop hicolor theme at its real size (256x256). A release archive
# carries it in that layout; a source checkout has it beside the GUI's files.
icon_rel="icons/hicolor/256x256/apps/satur8.png"
app_icon=""
for candidate in "$repo/assets/$icon_rel" "$repo/crates/satur8-gui/ui/icon.png"; do
    if [ -f "$candidate" ]; then
        app_icon="$candidate"
        break
    fi
done

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }

# Refresh GTK's icon cache for a hicolor folder, but only where a cache
# already exists: a new per-user cache that later goes stale would hide other
# apps' new icons. The tool is optional (KDE does not use the cache), so its
# absence or failure never stops the install.
refresh_icon_cache() { # refresh_icon_cache <hicolor dir> [sudo]
    local dir="$1" run=()
    [ "${2:-}" = sudo ] && run=(sudo)
    if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f "$dir/icon-theme.cache" ]; then
        "${run[@]}" gtk-update-icon-cache -q -t "$dir" >/dev/null 2>&1 || true
    fi
}

# Qt 6's D-Bus command-line tool. Arch and SteamOS name it qdbus6; Fedora and
# Bazzite ship it as qdbus-qt6 (package qt6-qttools) with no qdbus6. Empty
# when neither is installed.
qdbus_tool=""
for candidate in qdbus6 qdbus-qt6; do
    if command -v "$candidate" >/dev/null 2>&1; then
        qdbus_tool="$candidate"
        break
    fi
done

daemon_was_active=false

quiesce_runtime() {
    if command -v systemctl >/dev/null 2>&1 \
        && systemctl --user is-active --quiet satur8-daemon.service 2>/dev/null; then
        daemon_was_active=true
        systemctl --user stop satur8-daemon.service
    fi
    if [ -n "$qdbus_tool" ]; then
        "$qdbus_tool" org.kde.KWin /Effects org.kde.kwin.Effects.unloadEffect satur8 \
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

# The KWin version in a plugin's embedded ID. grep -a reads the binary
# directly, so this needs no binutils (`strings` is missing on many systems).
effect_kwin_abi() {
    { grep -aoE 'org\.kde\.kwin\.EffectPluginFactory[0-9]+(\.[0-9]+)+' "$1" 2>/dev/null \
        | sed 's/^org\.kde\.kwin\.EffectPluginFactory//' \
        | head -n1; } || true
}

# The highest Qt_6.x symbol version a plugin needs, as x. A Qt older than
# that cannot load the plugin.
effect_qt_minor() {
    { grep -aoE 'Qt_6\.[0-9]+' "$1" 2>/dev/null \
        | sed 's/^Qt_6\.//' \
        | sort -n \
        | tail -n1; } || true
}

# The installed Qt 6 minor version, from the Qt core library's file name
# (libQt6Core.so.6.11.1 gives 11). It looks in /usr/lib64 (Fedora), /usr/lib
# (Arch, SteamOS) and /usr/lib/x86_64-linux-gnu (Debian, Ubuntu), and the first
# folder that has one decides. SATUR8_QT_LIB_DIR replaces that list for tests.
installed_qt_minor() {
    local dirs=(/usr/lib64 /usr/lib /usr/lib/x86_64-linux-gnu) dir file minor best=""
    if [ -n "${SATUR8_QT_LIB_DIR:-}" ]; then
        dirs=("$SATUR8_QT_LIB_DIR")
    fi
    for dir in "${dirs[@]}"; do
        for file in "$dir"/libQt6Core.so.6.*; do
            [ -e "$file" ] || continue
            minor="${file##*/libQt6Core.so.6.}"
            minor="${minor%%.*}"
            [[ "$minor" =~ ^[0-9]+$ ]] || continue
            if [ -z "$best" ] || [ "$minor" -gt "$best" ]; then
                best="$minor"
            fi
        done
        if [ -n "$best" ]; then
            break
        fi
    done
    printf '%s' "$best"
}

# Whether $1 is a usable entry of the bundled set: its folder name is
# <family>-<KWin version> and the plugin inside embeds exactly that version.
# An entry whose embedded ID disagrees with its folder name is never used.
set_entry_ok() {
    local dir="$1" key key_re='^[a-z][a-z0-9]*-[0-9]+(\.[0-9]+)+$'
    key="${dir##*/}"
    [[ "$key" =~ $key_re ]] || return 1
    [ -f "$dir/satur8.so" ] && [ -r "$dir/satur8.so" ] || return 1
    [ "$(effect_kwin_abi "$dir/satur8.so")" = "${key##*-}" ]
}

has_effect_set() {
    compgen -G "$effect_set/*/satur8.so" >/dev/null
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

# Whether this machine can build the effect: the effect source, cmake, a C++
# compiler and the KWin development headers. Sets build_blocked_by when not.
can_build_effect() {
    build_blocked_by=""
    if [ ! -f "$repo/assets/kwin-effect/CMakeLists.txt" ]; then
        build_blocked_by="this archive lacks the effect source"
        return 1
    fi
    local tool
    for tool in cmake c++; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            build_blocked_by="missing $tool"
            return 1
        fi
    done
    if [ ! -f /usr/include/kwin/config-kwin.h ]; then
        build_blocked_by="the KWin development headers are not installed"
        return 1
    fi
}

# Decide which KWin effect this machine gets, changing nothing. In order: the
# bundled effect built for this family and KWin version; a local build; another
# family's effect for the same KWin whose Qt_6.x tag is not above the installed
# Qt. Sets effect_pick to "set" (effect_pick_key names the folder; with
# effect_pick_note when it was built for another system), "build", or "" with
# the reason in effect_skip_reason.
choose_effect() {
    effect_pick=""
    effect_pick_key=""
    effect_pick_note=""
    effect_skip_reason=""
    effect_kwin="$(runtime_kwin_abi)"
    if [ -z "$effect_kwin" ]; then
        effect_skip_reason="could not determine the installed KWin version"
        return
    fi
    local family qt dir key need qt_note=""
    family="$(system_family)"
    if [ "$family" != other ] && set_entry_ok "$effect_set/$family-$effect_kwin"; then
        effect_pick=set
        effect_pick_key="$family-$effect_kwin"
        return
    fi
    if can_build_effect; then
        effect_pick=build
        return
    fi
    # Another system's effect: Qt is the only requirement checked against this
    # host. The build gate in packaging/build-kwin-effect.sh (floors in
    # packaging/abi-floors.sh) guarantees every bundled effect needs no newer
    # GLIBC, GLIBCXX or CXXABI version than the tarball's own binaries do.
    qt="$(installed_qt_minor)"
    for dir in "$effect_set"/*/; do
        dir="${dir%/}"
        key="${dir##*/}"
        [ "${key##*-}" = "$effect_kwin" ] || continue
        [ "${key%-*}" != "$family" ] || continue
        set_entry_ok "$dir" || continue
        need="$(effect_qt_minor "$dir/satur8.so")"
        if [ -z "$need" ]; then
            qt_note="$key does not name the Qt version it needs"
            continue
        fi
        if [ -z "$qt" ]; then
            qt_note="the installed Qt version could not be read, so $key built for another system was not used"
            continue
        fi
        if [ "$need" -gt "$qt" ]; then
            qt_note="$key needs Qt 6.$need and this system has Qt 6.$qt"
            continue
        fi
        effect_pick=set
        effect_pick_key="$key"
        effect_pick_note="this effect was built for another system (${key%-*}) with the same KWin $effect_kwin; this system's Qt 6.$qt is new enough for it"
        return
    done
    effect_skip_reason="this Satur8 has no KWin effect for KWin $effect_kwin built for this system, and cannot build one here ($build_blocked_by)"
    if [ -n "$qt_note" ]; then
        effect_skip_reason="$effect_skip_reason; $qt_note"
    fi
}

# Build the effect for the installed KWin. Returns non-zero, with the reason in
# effect_skip_reason, when it cannot: a missing effect must never stop the rest
# of Satur8 from installing. (This runs inside an `if`, where `set -e` is off,
# so every step checks its own result.)
build_effect() {
    effect_skip_reason=""
    if ! can_build_effect; then
        effect_skip_reason="$build_blocked_by"
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

    # The KWin effect is optional: no KWin, or no effect that fits and no way
    # to build one, skips it and installs everything else.
    effect_so=""
    effect_skip_reason=""
    if ! kwin_present; then
        say "KWin not found: skipping the KWin effect (only KDE Plasma uses it)"
        return
    fi
    choose_effect
    case "$effect_pick" in
        set)
            say "Using bundled KWin effect for ABI $effect_kwin ($effect_pick_key)"
            if [ -n "$effect_pick_note" ]; then
                echo "    NOTE: $effect_pick_note."
            fi
            effect_so="$effect_set/$effect_pick_key/satur8.so"
            ;;
        build)
            if has_effect_set; then
                say "Bundled KWin effects do not include KWin $effect_kwin for this system; rebuilding locally"
            fi
            if ! build_effect; then
                effect_so=""
                say "Skipping the KWin effect: $effect_skip_reason"
            fi
            ;;
        *)
            say "Skipping the KWin effect: $effect_skip_reason"
            ;;
    esac
}

# --pick-effect: print the decision and change nothing.
report_effect_pick() {
    if ! kwin_present; then
        echo "skip: KWin not found (only KDE Plasma uses it)"
        return
    fi
    choose_effect
    case "$effect_pick" in
        set)
            echo "pick: $effect_pick_key"
            if [ -n "$effect_pick_note" ]; then
                echo "note: $effect_pick_note"
            fi
            ;;
        build) echo "build: no bundled effect for KWin $effect_kwin on this system; it would be built here" ;;
        *) echo "skip: $effect_skip_reason" ;;
    esac
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
        if is_steamos; then
            echo "    Switch to Game Mode and back to Desktop Mode, or log out and back in, once."
        elif per_user_only; then
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
    # The whole bundled set, so `satur8 repair` can switch to the right effect
    # after an OS update with no network and no build tools. A set from an
    # earlier install is replaced, never mixed in. Only KWin uses it.
    rm -rf "$effect_set_dir"
    if kwin_present && has_effect_set; then
        say "Installing the bundled KWin effects to $effect_set_dir"
        mkdir -p "$effect_set_dir"
        cp -R "$effect_set/." "$effect_set_dir/"
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
    # The entry names the app by its full path. A launcher only finds a bare
    # program name on the session's PATH, and SteamOS (among others) does not
    # put ~/.local/bin there: the launcher said "Could not find the program
    # 'satur8-gui'", and Steam's "Add to Steam" could not read the entry.
    # A path with spaces or special characters is quoted as the Desktop Entry
    # specification requires.
    exec_path="$bindir/satur8-gui"
    case "$exec_path" in
        *[[:space:]\"\`\$\\]*)
            exec_line="Exec=\"$(printf '%s' "$exec_path" | sed 's/[\\"`$]/\\&/g')\"" ;;
        *)
            exec_line="Exec=$exec_path" ;;
    esac
    mkdir -p "$data/applications"
    EXEC_LINE="$exec_line" awk '/^Exec=/ { print ENVIRON["EXEC_LINE"]; next } { print }' \
        "$repo/packaging/satur8.desktop" > "$data/applications/satur8.desktop.new"
    chmod 644 "$data/applications/satur8.desktop.new"
    mv -f "$data/applications/satur8.desktop.new" "$data/applications/satur8.desktop"
    if [ -z "$app_icon" ]; then
        say "No app icon in this archive: the desktop entry will show a generic icon"
    elif [ "$mode" = system ]; then
        say "Installing app icon to /usr/share/$icon_rel (sudo)"
        sudo install -Dm644 "$app_icon" "/usr/share/$icon_rel"
        refresh_icon_cache /usr/share/icons/hicolor sudo
    else
        say "Installing app icon to $data/$icon_rel"
        install -Dm644 "$app_icon" "$data/$icon_rel"
        refresh_icon_cache "$data/icons/hicolor"
    fi

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
        echo "      satur8 games"
        echo "      satur8 profile add NAME 1.5 --steam-game NAME"
    fi
    if kwin_present || ! has_gamescope_session; then
        cat <<EOF
  * Per-game focus mode on KDE Plasma:
      systemctl --user enable --now satur8-daemon
      kwriteconfig6 --file kwinrc --group Plugins --key satur8-focusEnabled true
      ${qdbus_tool:-qdbus6} org.kde.KWin /KWin reconfigure
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
    if [ -n "$qdbus_tool" ]; then
        "$qdbus_tool" org.kde.KWin /Effects org.kde.kwin.Effects.unloadEffect satur8 \
            >/dev/null 2>&1 || true
    fi
    say "Removing per-user install"
    rm -f "$bindir/satur8" "$bindir/satur8-daemon" "$bindir/satur8-tray" "$bindir/satur8-gui"
    rm -f "$qt_plugin_user/satur8.so" "$env_dir/60-satur8.conf"
    rm -rf "$kwin_script_dir" "$gnome_ext_dir" "$effect_src_dir" "$effect_set_dir" "$state_dir"
    # The effect source and the effect set were the only things in this
    # folder; rmdir keeps it if anything else is there.
    rmdir "$data/satur8" 2>/dev/null || true
    rm -f "$unit_dir/satur8-daemon.service" "$data/applications/satur8.desktop"
    if [ -e "$data/$icon_rel" ]; then
        rm -f "$data/$icon_rel"
        refresh_icon_cache "$data/icons/hicolor"
    fi
    if [ -e "$qt_plugin_sys/satur8.so" ]; then
        echo "    (system-installed effect at $qt_plugin_sys left alone; remove with sudo if needed)"
    fi
    if [ -e "/usr/share/$icon_rel" ]; then
        echo "    (system-installed icon at /usr/share/$icon_rel left alone; remove with sudo if needed)"
    fi
}

case "$action" in
    install) install_all ;;
    uninstall) uninstall_all ;;
    check-effect) check_effect_abi "$effect_to_check" ;;
    pick-effect) report_effect_pick ;;
esac
