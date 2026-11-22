#!/usr/bin/env bash
# Build the four Satur8 release binaries so they run on glibc 2.35 and newer.
#
# Usage: packaging/build-portable.sh OUT_DIR
#   OUT_DIR  receives satur8, satur8-daemon, satur8-tray, satur8-gui and
#            SHA256SUMS (created if missing)
#
# Environment, all optional:
#   CARGO_BUILD_JOBS  parallel compile jobs passed to cargo (default: cargo's
#                     own default, one per CPU the container sees)
#   ENGINE            podman or docker (default: podman if installed, else docker)
#   LIMITS            resource flags for the container
#                     (default: --cpus=3 --memory=4g; set to "" for none)
#   IMAGE             build image (default: docker.io/library/ubuntu:22.04)
#   RUST_VERSION      Rust toolchain installed with rustup (default: 1.100.0)
#   GLIBC_FLOOR       passed to check-portability.sh (default there: 2.35)
#
# Why a container: a binary built on a new glibc asks for that glibc's symbol
# versions (GLIBC_2.43 for atan2f and acosf on Fedora 45), and an older system
# refuses to start it. Linking against Ubuntu 22.04's glibc 2.35 keeps every
# symbol at 2.35 or older. Ubuntu's own Rust is too old for Slint, so the
# pinned toolchain comes from rustup, whose installer is pinned by checksum.
# The pin lives here, not in a rust-toolchain.toml, so the Arch and Fedora
# package builds keep using their distribution's Rust.
#
# The repository is mounted read-only and the build happens inside the
# container, so nothing on the host changes except OUT_DIR, and no root is
# needed on the host. The build ends with packaging/check-portability.sh, run
# inside the container, which fails the build when a binary needs a newer glibc.
set -euo pipefail

usage() { sed -n '2,30p' "$0"; exit 2; }
[ "$#" -eq 1 ] || usage
out_dir="$1"

repo="$(cd "$(dirname "$0")/.." && pwd)"
image="${IMAGE:-docker.io/library/ubuntu:22.04}"
# 1.100.0 is the Rust the v0.4.0 release was built with (Fedora 45's package in
# CI). The newest minimum in Cargo.lock is 1.90 (typed-index-collections 3.5.0);
# Slint 1.16.1 needs 1.88.
rust_version="${RUST_VERSION:-1.100.0}"
rustup_version=1.29.3
rustup_sha256=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
read -r -a limits <<< "${LIMITS---cpus=3 --memory=4g}"
binaries="satur8 satur8-daemon satur8-tray satur8-gui"

say() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

# Same container engine choice as packaging/install-nokwin-test.sh.
engine="${ENGINE:-}"
if [ -z "$engine" ]; then
    for candidate in podman docker; do
        command -v "$candidate" >/dev/null 2>&1 && { engine="$candidate"; break; }
    done
fi
[ -n "$engine" ] || fail "needs podman or docker"
command -v "$engine" >/dev/null 2>&1 || fail "container engine '$engine' is not installed"
case "$rust_version" in
    *[!0-9.]*|'') fail "RUST_VERSION must look like 1.100.0, got '$rust_version'" ;;
esac
[ -f "$repo/Cargo.lock" ] || fail "no Cargo.lock in $repo"
[ -f "$repo/packaging/check-portability.sh" ] || fail "missing $repo/packaging/check-portability.sh"

mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"
for name in $binaries SHA256SUMS; do rm -f "$out_dir/$name"; done

jobs_env=()
if [ -n "${CARGO_BUILD_JOBS:-}" ]; then
    jobs_env=(-e "CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS")
fi

# Runs inside the container. Arguments: Rust version, rustup version, rustup
# checksum, binaries.
# shellcheck disable=SC2016  # expanded inside the container, not here
inner='
set -euo pipefail
rust_version="$1"; rustup_version="$2"; rustup_sha256="$3"; binaries="$4"
say() { printf "==> %s\n" "$*"; }

say "Build system: $(. /etc/os-release && echo "$PRETTY_NAME"), $(ldd --version | head -n1)"
say "Installing the build packages with apt"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
# build-essential: gcc, the linker and glibc 2.35 headers. libfontconfig1-dev:
# the one C library the desktop app links. Nothing from Qt, so Slint does not
# pick up a Qt backend.
apt-get install -y -qq --no-install-recommends \
    build-essential pkg-config libfontconfig1-dev curl ca-certificates >/dev/null

say "Installing Rust $rust_version with rustup $rustup_version"
url="https://static.rust-lang.org/rustup/archive/$rustup_version/x86_64-unknown-linux-gnu/rustup-init"
curl -fsS --proto =https --tlsv1.2 -o /root/rustup-init "$url"
echo "$rustup_sha256  /root/rustup-init" | sha256sum -c --quiet
chmod +x /root/rustup-init
/root/rustup-init -y -q --no-modify-path --profile minimal --default-toolchain "$rust_version"
export PATH="/root/.cargo/bin:$PATH"
rustc --version
cargo --version

say "Building: cargo build --release --locked (jobs: ${CARGO_BUILD_JOBS:-cargo default})"
cd /src
start=$(date +%s)
CARGO_TARGET_DIR=/build/target cargo build --release --locked
say "Rust build took $(( $(date +%s) - start )) s"

for name in $binaries; do install -m755 "/build/target/release/$name" "/out/$name"; done
(cd /out && sha256sum $binaries > SHA256SUMS)
cat /out/SHA256SUMS

rc=0
bash /src/packaging/check-portability.sh /out || rc=$?
if [ -n "${HOST_UID:-}" ]; then chown -R "$HOST_UID:$HOST_GID" /out; fi
exit "$rc"
'

say "Building the Satur8 release binaries in $image with $engine"
"$engine" pull -q "$image" >/dev/null 2>&1 || true
digest="$("$engine" image inspect --format '{{index .RepoDigests 0}}' "$image" 2>/dev/null || echo unknown)"
say "Image digest: $digest"
say "Source: $repo ($(git -C "$repo" describe --always --dirty 2>/dev/null || echo 'not a git checkout'))"

# Rootful docker (a GitHub runner) writes the outputs as root, so they are
# handed back to the caller with chown. Rootless podman and rootless docker
# already map the container's root to the caller, and a chown there would hand
# the files to a sub-UID instead. Which case applies is tested with one file,
# not guessed from the engine's name.
owner_env=()
probe="$out_dir/.owner-probe"
rm -rf "$probe"
mkdir -p "$probe"
"$engine" run --rm --network none -v "$probe:/probe" "$image" touch /probe/file \
    || fail "could not start a container from $image"
if [ "$(stat -c %u "$probe/file")" != "$(id -u)" ]; then
    owner_env=(-e "HOST_UID=$(id -u)" -e "HOST_GID=$(id -g)")
    say "The container writes files as another user; its outputs will be handed back with chown"
fi
rm -rf "$probe"

t0=$(date +%s)
"$engine" run --rm --pull=missing "${limits[@]}" "${owner_env[@]}" "${jobs_env[@]}" \
    -e "GLIBC_FLOOR=${GLIBC_FLOOR:-2.35}" \
    -v "$repo:/src:ro" -v "$out_dir:/out" \
    "$image" bash -c "$inner" inner "$rust_version" "$rustup_version" "$rustup_sha256" "$binaries" \
    || fail "the build or the portability check failed (see the lines above)"

# The checksums were written inside the container; reading them back on the
# host catches a file that changed on its way out (this build box has had
# memory faults).
say "Checking the checksums on the host"
(cd "$out_dir" && sha256sum -c --quiet SHA256SUMS) || fail "a binary changed between the container and $out_dir"
say "Done in $(( $(date +%s) - t0 )) s: $out_dir"
