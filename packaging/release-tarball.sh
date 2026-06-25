#!/usr/bin/env bash
# Build the Linux x86_64 release archive consumed by satur8.app.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
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

echo "==> Building Rust release binaries"
(cd "$repo" && cargo build --release --locked)

echo "==> Building KWin effect"
rm -rf "$effect_build"
cmake -S "$repo/assets/kwin-effect" -B "$effect_build" -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$effect_build" >/dev/null

echo "==> Staging $name"
rm -rf "$stage"
mkdir -p "$stage/prebuilt/linux-x86_64/bin" "$stage/prebuilt/linux-x86_64/kwin"

install -m755 "$repo/target/release/satur8" "$stage/prebuilt/linux-x86_64/bin/satur8"
install -m755 "$repo/target/release/satur8-daemon" "$stage/prebuilt/linux-x86_64/bin/satur8-daemon"
install -m755 "$repo/target/release/satur8-tray" "$stage/prebuilt/linux-x86_64/bin/satur8-tray"
install -m755 "$repo/target/release/satur8-gui" "$stage/prebuilt/linux-x86_64/bin/satur8-gui"
install -m755 "$effect_build/satur8.so" "$stage/prebuilt/linux-x86_64/kwin/satur8.so"

mkdir -p "$stage/packaging" "$stage/assets"
install -m755 "$repo/packaging/install.sh" "$stage/packaging/install.sh"
install -m644 "$repo/packaging/satur8-daemon.service" "$stage/packaging/satur8-daemon.service"
install -m644 "$repo/packaging/satur8.desktop" "$stage/packaging/satur8.desktop"
cp -R "$repo/assets/kwin-script" "$stage/assets/kwin-script"
cp -R "$repo/assets/gnome-extension" "$stage/assets/gnome-extension"
cp -R "$repo/assets/gamescope" "$stage/assets/gamescope"
install -m644 "$repo/README.md" "$stage/README.md"
install -m644 "$repo/LICENSE" "$stage/LICENSE"

echo "==> Writing archive and checksum"
mkdir -p "$dist"
tar -C "$dist" -czf "$dist/$name.tar.gz" "$name"
(cd "$dist" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")

echo "$dist/$name.tar.gz"
echo "$dist/$name.tar.gz.sha256"
