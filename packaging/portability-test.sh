#!/usr/bin/env bash
# Start every Satur8 release binary on the oldest Linux systems we support.
#
# Usage: packaging/portability-test.sh SOURCE [IMAGE...]
#        packaging/portability-test.sh --negative-control [SOURCE|download] [IMAGE...]
#   SOURCE   a folder with satur8, satur8-daemon, satur8-tray, satur8-gui, or a
#            release tarball (satur8-vX.Y.Z-linux-x86_64.tar.gz)
#   IMAGE    container images to test in (default: Ubuntu 22.04, Debian 12,
#            Ubuntu 24.04, openSUSE Leap 16.0, Fedora 43)
#
#   --negative-control  proves the test can see the problem it exists for: the
#            binaries must FAIL to start, with a "GLIBC_2.xx not found" loader
#            line from satur8-gui. With no SOURCE, or SOURCE "download", it
#            downloads the published v0.3.6 tarball (built on glibc 2.43) and
#            checks it against its published SHA-256. Default image: Ubuntu 22.04.
#
# Environment, all optional:
#   ENGINE    podman or docker (default: podman if installed, else docker)
#   LIMITS    resource flags for each container
#             (default: --cpus=3 --memory=4g; set to "" for none)
#   WORK_DIR  scratch folder (default: a new folder from mktemp, removed at the
#             end; a WORK_DIR you pass is kept)
#
# Each image gets a fresh container that sees only the four binaries, read-only,
# and no repository. It installs what a desktop system has (Xvfb, D-Bus,
# fontconfig and one font, the X11 client libraries, Mesa), then:
#   - satur8 --version must print the version;
#   - satur8 doctor must not crash (a non-zero exit with a plain message is fine);
#   - satur8-daemon (it takes no options) must claim its D-Bus name;
#   - satur8-tray and satur8-gui must still be running after 10 seconds, under
#     Xvfb with SLINT_BACKEND=winit-software, a private D-Bus session and an
#     isolated XDG_CONFIG_HOME.
# Any loader error (a missing library, a missing GLIBC_ version, an undefined
# symbol) or a panic fails that binary. A font package is installed because with
# zero fonts the app panics inside its font library, a known separate bug.
set -euo pipefail

usage() { sed -n '2,35p' "$0"; exit 2; }
say() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

mode=normal
if [ "${1:-}" = --negative-control ]; then mode=negative; shift; fi
case "${1:-}" in -h|--help) usage ;; esac
source_arg="${1:-}"
[ "$#" -gt 0 ] && shift
if [ "$mode" = normal ] && [ -z "$source_arg" ]; then usage; fi
[ -n "$source_arg" ] || source_arg=download

images=("$@")
if [ "${#images[@]}" -eq 0 ]; then
    if [ "$mode" = negative ]; then
        images=(docker.io/library/ubuntu:22.04)
    else
        images=(
            docker.io/library/ubuntu:22.04
            docker.io/library/debian:12
            docker.io/library/ubuntu:24.04
            registry.opensuse.org/opensuse/leap:16.0
            registry.fedoraproject.org/fedora:43
        )
    fi
fi
read -r -a limits <<< "${LIMITS---cpus=3 --memory=4g}"
binaries="satur8 satur8-daemon satur8-tray satur8-gui"

# Same container engine choice as packaging/install-nokwin-test.sh.
engine="${ENGINE:-}"
if [ -z "$engine" ]; then
    for candidate in podman docker; do
        command -v "$candidate" >/dev/null 2>&1 && { engine="$candidate"; break; }
    done
fi
[ -n "$engine" ] || fail "needs podman or docker"

if [ -n "${WORK_DIR:-}" ]; then
    work="$WORK_DIR"; own_work=false
    mkdir -p "$work"
else
    work="$(mktemp -d)"; own_work=true
fi
work="$(cd "$work" && pwd)"
cleanup() { if [ "$own_work" = true ]; then rm -rf "$work"; fi; }
trap cleanup EXIT

# ------------------------------------------------------------ the binaries
if [ "$source_arg" = download ]; then
    [ "$mode" = negative ] || fail "download is only for --negative-control"
    command -v curl >/dev/null 2>&1 || fail "needs curl to download the v0.3.6 tarball"
    base="https://github.com/NtrpyDev/satur8/releases/download/v0.3.6"
    name="satur8-v0.3.6-linux-x86_64.tar.gz"
    mkdir -p "$work/download"
    say "Downloading the published v0.3.6 tarball: $base/$name"
    curl -fsSL --proto =https --proto-redir =https -o "$work/download/$name" "$base/$name"
    curl -fsSL --proto =https --proto-redir =https -o "$work/download/$name.sha256" "$base/$name.sha256"
    (cd "$work/download" && sha256sum -c "$name.sha256") || fail "the download does not match its published SHA-256"
    source_arg="$work/download/$name"
fi

if [ -d "$source_arg" ]; then
    bin_dir="$(cd "$source_arg" && pwd)"
elif [ -f "$source_arg" ]; then
    say "Unpacking $source_arg"
    rm -rf "$work/unpacked"
    mkdir -p "$work/unpacked"
    tar -xzf "$source_arg" -C "$work/unpacked"
    bin_dir="$(find "$work/unpacked" -path '*/prebuilt/linux-x86_64/bin' -type d | head -n1)"
    [ -n "$bin_dir" ] || fail "no prebuilt/linux-x86_64/bin folder in $source_arg"
else
    fail "no folder or tarball at $source_arg"
fi
for name in $binaries; do
    [ -x "$bin_dir/$name" ] || fail "$bin_dir/$name is missing or not executable"
done
say "Testing the binaries in $bin_dir"
# shellcheck disable=SC2086  # a fixed list of four names
(cd "$bin_dir" && sha256sum $binaries) | sed 's/^/    /'

# ------------------------------------------------------------ in-container files
mkdir -p "$work/test"

# A stand-in for the panel's StatusNotifierWatcher, so the tray has a host to
# register with. Without one satur8-tray exits at once with "couldn't register
# tray (no StatusNotifier host?)", which says nothing about whether it loads.
# It answers the calls ksni makes and nothing else.
cat > "$work/test/sni-watcher.py" <<'PY'
import sys
from jeepney import HeaderFields, MessageType, new_error, new_method_return
from jeepney.bus_messages import message_bus
from jeepney.io.blocking import open_dbus_connection

NAME = "org.kde.StatusNotifierWatcher"
PROPS = {
    "IsStatusNotifierHostRegistered": ("b", True),
    "ProtocolVersion": ("i", 0),
    "RegisteredStatusNotifierItems": ("as", []),
}

conn = open_dbus_connection(bus="SESSION")
reply = conn.send_and_get_reply(message_bus.RequestName(NAME, 4))
if reply.body[0] != 1:
    sys.exit("could not own " + NAME)
open(sys.argv[1], "w").close()
while True:
    msg = conn.receive()
    if msg.header.message_type != MessageType.method_call:
        continue
    member = msg.header.fields.get(HeaderFields.member)
    if member in ("RegisterStatusNotifierItem", "RegisterStatusNotifierHost"):
        conn.send(new_method_return(msg))
    elif member == "Get" and msg.body[1] in PROPS:
        conn.send(new_method_return(msg, "v", (PROPS[msg.body[1]],)))
    elif member == "GetAll":
        conn.send(new_method_return(msg, "a{sv}", (PROPS,)))
    else:
        conn.send(new_error(msg, "org.freedesktop.DBus.Error.UnknownMethod"))
PY

# Runs as root in the container. Prints RESULT lines the host reads.
cat > "$work/test/run.sh" <<'RUN'
#!/usr/bin/env bash
set -uo pipefail
bin=/satur8/bin
. /etc/os-release
echo "== system: $PRETTY_NAME, $(ldd --version 2>&1 | head -n1)"

echo "== installing the desktop runtime packages"
case " $ID ${ID_LIKE:-} " in
    *" debian "*|*" ubuntu "*)
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq > /root/pkg.log 2>&1 &&
        apt-get install -y -qq --no-install-recommends \
            xvfb dbus fontconfig fonts-dejavu-core libxkbcommon0 libxkbcommon-x11-0 \
            libx11-6 libx11-xcb1 libxcursor1 libxi6 libxrandr2 libxrender1 libxcb1 \
            libgl1 libegl1 libgl1-mesa-dri procps python3 python3-jeepney >> /root/pkg.log 2>&1 ;;
    *" fedora "*)
        dnf -y -q --setopt=install_weak_deps=False install \
            xorg-x11-server-Xvfb dbus-daemon fontconfig dejavu-sans-fonts libxkbcommon \
            libxkbcommon-x11 libX11 libX11-xcb libXcursor libXi libXrandr libXrender libxcb \
            mesa-dri-drivers mesa-libGL mesa-libEGL procps-ng python3 python3-jeepney > /root/pkg.log 2>&1 ;;
    *" suse "*|*" opensuse "*)
        zypper -n -q install --no-recommends \
            xorg-x11-server-Xvfb dbus-1-daemon fontconfig dejavu-fonts libxkbcommon0 libxkbcommon-x11-0 \
            libX11-6 libX11-xcb1 libXcursor1 libXi6 libXrandr2 libXrender1 libxcb1 \
            Mesa-dri Mesa-libGL1 Mesa-libEGL1 procps python3 python3-jeepney > /root/pkg.log 2>&1 ;;
    *) echo "RESULT setup FAIL no package recipe for ID=$ID"; exit 1 ;;
esac
pkg_rc=$?
if [ "$pkg_rc" -ne 0 ]; then
    tail -n 30 /root/pkg.log
    echo "RESULT setup FAIL package install exited $pkg_rc"
    exit 1
fi
for tool in Xvfb dbus-run-session python3 pgrep; do
    command -v "$tool" >/dev/null 2>&1 || { echo "RESULT setup FAIL $tool is missing after the install"; exit 1; }
done
echo "== fonts: $(fc-list | wc -l); repository present: $(ls -d /src /work /satur8/src 2>/dev/null || echo none)"

loader_errors='error while loading shared libraries|version .GLIBC_[0-9.]+. not found|symbol lookup error|undefined symbol|cannot open shared object file|panicked at'

# report NAME LOG RC_OK_TEXT: PASS when the log has no loader error or panic.
report() {
    local name="$1" log="$2" note="$3" bad
    bad="$(grep -E "$loader_errors" "$log" | head -n3)"
    if [ -n "$bad" ]; then
        echo "RESULT $name FAIL $(echo "$bad" | tr '\n' ' ')"
    else
        echo "RESULT $name PASS $note"
    fi
}

mkdir -p /root/iso/config /root/iso/run /root/logs
chmod 700 /root/iso/run
export XDG_CONFIG_HOME=/root/iso/config XDG_RUNTIME_DIR=/root/iso/run
unset WAYLAND_DISPLAY DISPLAY

echo "== satur8 --version"
"$bin/satur8" --version > /root/logs/version.log 2>&1; rc=$?
cat /root/logs/version.log
if [ "$rc" -eq 0 ] && grep -q '^satur8 ' /root/logs/version.log; then
    report satur8--version /root/logs/version.log "printed $(head -n1 /root/logs/version.log)"
else
    echo "RESULT satur8--version FAIL exit $rc: $(head -n2 /root/logs/version.log | tr '\n' ' ')"
fi

echo "== satur8 doctor"
"$bin/satur8" doctor > /root/logs/doctor.log 2>&1; rc=$?
head -n 40 /root/logs/doctor.log
if [ "$rc" -ge 128 ]; then
    echo "RESULT satur8-doctor FAIL crashed (exit $rc, signal $((rc - 128)))"
else
    report satur8-doctor /root/logs/doctor.log "exit $rc, no crash"
fi

# The rest needs a session bus and a display: a private bus per container, and
# Xvfb on :99.
cat > /root/session.sh <<'SESSION'
set -uo pipefail
bin=/satur8/bin
# alive NAME LOG SECONDS CMD...: start CMD, wait, require it is still running.
alive() {
    local name="$1" log="$2" secs="$3" pid
    shift 3
    "$@" > "$log" 2>&1 &
    pid=$!
    sleep "$secs"
    if kill -0 "$pid" 2>/dev/null; then
        echo "== $name still running after $secs s"
        kill -TERM "$pid" 2>/dev/null; sleep 1; kill -KILL "$pid" 2>/dev/null
        wait "$pid" 2>/dev/null
        return 0
    fi
    wait "$pid"; local rc=$?
    echo "== $name exited within $secs s with status $rc"
    return 1
}
loader_errors='error while loading shared libraries|version .GLIBC_[0-9.]+. not found|symbol lookup error|undefined symbol|cannot open shared object file|panicked at'
verdict() {
    local name="$1" log="$2" up="$3" bad
    echo "-- $name log:"; tail -n 15 "$log"
    bad="$(grep -E "$loader_errors" "$log" | head -n3)"
    if [ -n "$bad" ]; then echo "RESULT $name FAIL $(echo "$bad" | tr '\n' ' ')"
    elif [ "$up" -ne 0 ]; then echo "RESULT $name FAIL did not stay running: $(tail -n2 "$log" | tr '\n' ' ')"
    else echo "RESULT $name PASS $4"
    fi
}

echo "== private session bus: $DBUS_SESSION_BUS_ADDRESS"

up=0; alive satur8-daemon /root/logs/daemon.log 5 "$bin/satur8-daemon" || up=1
if [ "$up" -eq 0 ] && ! grep -q 'listening on org.satur8.Daemon' /root/logs/daemon.log; then up=1; fi
verdict satur8-daemon /root/logs/daemon.log "$up" "claimed org.satur8.Daemon on the session bus"

python3 /satur8/test/sni-watcher.py /root/iso/watcher-ready > /root/logs/watcher.log 2>&1 &
for _ in $(seq 50); do [ -e /root/iso/watcher-ready ] && break; sleep 0.2; done
[ -e /root/iso/watcher-ready ] || { cat /root/logs/watcher.log; echo "RESULT setup FAIL the stand-in tray host did not start"; }
up=0; alive satur8-tray /root/logs/tray.log 10 "$bin/satur8-tray" || up=1
verdict satur8-tray /root/logs/tray.log "$up" "running after 10 s, registered with the stand-in tray host"

Xvfb :99 -screen 0 1280x800x24 -nolisten tcp > /root/logs/xvfb.log 2>&1 &
for _ in $(seq 50); do [ -S /tmp/.X11-unix/X99 ] && break; sleep 0.2; done
[ -S /tmp/.X11-unix/X99 ] || { cat /root/logs/xvfb.log; echo "RESULT setup FAIL Xvfb did not start"; }
up=0; DISPLAY=:99 SLINT_BACKEND=winit-software alive satur8-gui /root/logs/gui.log 10 "$bin/satur8-gui" || up=1
verdict satur8-gui /root/logs/gui.log "$up" "running after 10 s under Xvfb with winit-software"
SESSION
dbus-run-session -- bash /root/session.sh
echo "== files written under the isolated XDG_CONFIG_HOME: $(find /root/iso/config -type f | tr '\n' ' ')"
RUN

# ------------------------------------------------------------ per image
summary=()
overall=0
for image in "${images[@]}"; do
    say "Image $image"
    "$engine" pull -q "$image" >/dev/null 2>&1 || true
    digest="$("$engine" image inspect --format '{{index .RepoDigests 0}}' "$image" 2>/dev/null || echo unknown)"
    say "Digest: $digest"
    log="$work/run-$(echo "$image" | tr '/:' '__').log"
    "$engine" run --rm --pull=missing "${limits[@]}" \
        -v "$bin_dir:/satur8/bin:ro" -v "$work/test:/satur8/test:ro" \
        "$image" bash /satur8/test/run.sh 2>&1 | tee "$log" || true

    results="$(grep '^RESULT ' "$log" || true)"
    if [ "$mode" = normal ]; then
        n_pass="$(printf '%s\n' "$results" | grep -c '^RESULT [^ ]* PASS' || true)"
        if [ "$n_pass" -eq 5 ] && ! printf '%s\n' "$results" | grep -q '^RESULT [^ ]* FAIL'; then
            summary+=("PASS  $image: all 5 checks passed")
        else
            summary+=("FAIL  $image: $(printf '%s\n' "$results" | grep ' FAIL' | cut -d' ' -f2 | tr '\n' ' ')(passed $n_pass of 5)")
            overall=1
        fi
    else
        gui_line="$(printf '%s\n' "$results" | grep '^RESULT satur8-gui ' || true)"
        if printf '%s\n' "$gui_line" | grep -qE "FAIL .*GLIBC_2\.[0-9]+'? not found"; then
            summary+=("PASS  $image: the old satur8-gui failed as expected: ${gui_line#RESULT satur8-gui FAIL }")
        else
            summary+=("FAIL  $image: the old satur8-gui did not fail with a GLIBC loader error, so this test cannot see the problem: ${gui_line:-no result}")
            overall=1
        fi
    fi
done

echo
if [ "$mode" = normal ]; then say "Summary (start test)"; else say "Summary (negative control)"; fi
printf '%s\n' "${summary[@]}"
exit "$overall"
