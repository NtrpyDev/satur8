#!/usr/bin/env bash
# Build the Linux x86_64 release archive consumed by satur8.app.
#
# Usage: packaging/release-tarball.sh [--bin-dir DIR] [--effects-dir DIR]
#
#   --bin-dir DIR      take satur8, satur8-daemon, satur8-tray and satur8-gui
#                      from DIR instead of building them here
#   --effects-dir DIR  take the KWin effect set from DIR: one folder per key
#                      (steamos-<kwin> or fedora<NN>-<kwin>) holding satur8.so
#                      and build-info.txt
#
# Without --effects-dir one effect is built here and keyed for this machine's
# family and KWin version. The effects land in
# prebuilt/linux-x86_64/kwin/effects/<key>/.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
bin_dir=""
effects_dir=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --bin-dir) bin_dir="${2:?--bin-dir requires a directory}"; shift 2 ;;
        --effects-dir) effects_dir="${2:?--effects-dir requires a directory}"; shift 2 ;;
        -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

version="$(
    sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -n1
)"

if [ -z "$version" ]; then
    echo "could not read workspace package version from Cargo.toml" >&2
    exit 1
fi

if [ "$(uname -m)" != x86_64 ]; then
    echo "release tarball is currently defined only for linux-x86_64" >&2
    exit 1
fi

name="satur8-v$version-linux-x86_64"
dist="$repo/target/dist"
stage="$dist/$name"
effect_build="$repo/target/kwin-effect-release"
bins=(satur8 satur8-daemon satur8-tray satur8-gui)
key_re='^[a-z][a-z0-9]*-[0-9]+(\.[0-9]+)+$'

# The KWin version in a plugin's embedded ID, read without binutils.
effect_id() {
    { grep -aoE 'org\.kde\.kwin\.EffectPluginFactory[0-9]+(\.[0-9]+)+' "$1" 2>/dev/null \
        | sed 's/^org\.kde\.kwin\.EffectPluginFactory//' \
        | head -n1; } || true
}

# This machine's family, the same rule packaging/install.sh uses: steamos,
# fedora<NN> for Fedora and systems built from it, else other.
host_family() {
    local id id_like version
    field() {
        local value
        value="$(sed -n "s/^$1=//p" /etc/os-release 2>/dev/null | head -n1)" || value=""
        value="${value#\"}"
        value="${value%\"}"
        printf '%s' "$value"
    }
    id="$(field ID)"
    id_like="$(field ID_LIKE)"
    version="$(field VERSION_ID)"
    if [ "$id" = steamos ]; then
        echo steamos
    elif { [ "$id" = fedora ] || [[ " $id_like " == *" fedora "* ]]; } &&
        [[ "$version" =~ ^[0-9]+$ ]]; then
        echo "fedora$version"
    else
        echo other
    fi
}

if [ -n "$bin_dir" ]; then
    [ -d "$bin_dir" ] || { echo "--bin-dir $bin_dir is not a directory" >&2; exit 1; }
    bin_src="$(cd "$bin_dir" && pwd)"
    for bin in "${bins[@]}"; do
        [ -x "$bin_src/$bin" ] || { echo "missing $bin_src/$bin" >&2; exit 1; }
    done
    echo "==> Using release binaries from $bin_src"
else
    echo "==> Building Rust release binaries"
    (cd "$repo" && cargo build --release --locked)
    bin_src="$repo/target/release"
fi

# Collect the effect set as "key<TAB>folder" lines, checking each entry.
effect_entries=()
if [ -n "$effects_dir" ]; then
    [ -d "$effects_dir" ] || { echo "--effects-dir $effects_dir is not a directory" >&2; exit 1; }
    effects_src="$(cd "$effects_dir" && pwd)"
    for dir in "$effects_src"/*/; do
        dir="${dir%/}"
        key="${dir##*/}"
        [ -d "$dir" ] || continue
        if ! [[ "$key" =~ $key_re ]]; then
            echo "effect folder $dir is not named <family>-<KWin version>" >&2
            exit 1
        fi
        for file in satur8.so build-info.txt; do
            [ -f "$dir/$file" ] || { echo "missing $dir/$file" >&2; exit 1; }
        done
        id="$(effect_id "$dir/satur8.so")"
        if [ "$id" != "${key##*-}" ]; then
            echo "$dir/satur8.so embeds KWin ${id:-no version}, but its folder says ${key##*-}" >&2
            exit 1
        fi
        effect_entries+=("$key"$'\t'"$dir")
    done
    if [ "${#effect_entries[@]}" -eq 0 ]; then
        echo "no effects found in $effects_src" >&2
        exit 1
    fi
    echo "==> Using ${#effect_entries[@]} KWin effect(s) from $effects_src"
else
    echo "==> Building KWin effect"
    rm -rf "$effect_build"
    cmake -S "$repo/assets/kwin-effect" -B "$effect_build" -DCMAKE_BUILD_TYPE=Release >/dev/null
    cmake --build "$effect_build" >/dev/null
    id="$(effect_id "$effect_build/satur8.so")"
    if [ -z "$id" ]; then
        echo "the built effect has no KWin plugin ID" >&2
        exit 1
    fi
    key="$(host_family)-$id"
    qt_tag="$({ grep -aoE 'Qt_6\.[0-9]+' "$effect_build/satur8.so" | sort -t. -k2 -n | tail -n1; } || true)"
    {
        echo "Satur8 KWin effect built by packaging/release-tarball.sh on the build machine"
        echo
        echo "Satur8 version: $version"
        echo "Plugin ID:      org.kde.kwin.EffectPluginFactory$id"
        echo "Highest Qt tag: ${qt_tag:-none found}"
        echo "Build system:   $(sed -n 's/^PRETTY_NAME=//p' /etc/os-release 2>/dev/null | tr -d '"')"
        echo "sha256:         $(sha256sum "$effect_build/satur8.so" | awk '{print $1}')"
    } > "$effect_build/build-info.txt"
    effect_entries+=("$key"$'\t'"$effect_build")
    echo "==> Keyed the KWin effect as $key"
fi

echo "==> Staging $name"
rm -rf "$stage"
mkdir -p "$stage/prebuilt/linux-x86_64/bin" "$stage/prebuilt/linux-x86_64/kwin/effects"

for bin in "${bins[@]}"; do
    install -m755 "$bin_src/$bin" "$stage/prebuilt/linux-x86_64/bin/$bin"
done
for entry in "${effect_entries[@]}"; do
    key="${entry%%$'\t'*}"
    dir="${entry#*$'\t'}"
    install -Dm755 "$dir/satur8.so" "$stage/prebuilt/linux-x86_64/kwin/effects/$key/satur8.so"
    install -Dm644 "$dir/build-info.txt" "$stage/prebuilt/linux-x86_64/kwin/effects/$key/build-info.txt"
done

mkdir -p "$stage/packaging" "$stage/assets/kwin-effect"
install -m755 "$repo/packaging/install.sh" "$stage/packaging/install.sh"
install -m644 "$repo/packaging/satur8-daemon.service" "$stage/packaging/satur8-daemon.service"
install -m644 "$repo/packaging/satur8.desktop" "$stage/packaging/satur8.desktop"
install -m644 "$repo/packaging/60-satur8.conf" "$stage/packaging/60-satur8.conf"
cp -R "$repo/assets/kwin-script" "$stage/assets/kwin-script"
cp -R "$repo/assets/gnome-extension" "$stage/assets/gnome-extension"
cp -R "$repo/assets/gamescope" "$stage/assets/gamescope"
install -m644 "$repo/assets/kwin-effect/CMakeLists.txt" "$stage/assets/kwin-effect/CMakeLists.txt"
install -m644 "$repo/assets/kwin-effect/metadata.json" "$stage/assets/kwin-effect/metadata.json"
install -m644 "$repo/assets/kwin-effect/satur8_effect.cpp" "$stage/assets/kwin-effect/satur8_effect.cpp"
install -m644 "$repo/assets/kwin-effect/satur8_effect.h" "$stage/assets/kwin-effect/satur8_effect.h"
install -m644 "$repo/README.md" "$stage/README.md"
install -m644 "$repo/LICENSE" "$stage/LICENSE"

echo "==> Writing archive and checksum"
mkdir -p "$dist"
tar -C "$dist" -czf "$dist/$name.tar.gz" "$name"
(cd "$dist" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")

echo "$dist/$name.tar.gz"
echo "$dist/$name.tar.gz.sha256"
