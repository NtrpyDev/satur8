#!/usr/bin/env bash
# Build the source archive consumed by the Arch package.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
version="$(
    sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -n1
)"

if [ -z "$version" ]; then
    echo "could not read workspace package version from Cargo.toml" >&2
    exit 1
fi

name="satur8-$version"
archive_name="satur8-v$version-source.tar.gz"
dist="$repo/target/dist"
manifest="$dist/source-files.txt"

mkdir -p "$dist"

# Leave the downstream packaging metadata out of the source archive: the Arch
# PKGBUILD/.SRCINFO and the Fedora spec live in their own packaging repos and
# would otherwise let an edit to those files change the archive's checksum.
git -C "$repo" ls-files -z \
    | grep -zv -E '^(packaging/PKGBUILD|packaging/\.SRCINFO|packaging/satur8\.spec)$' \
    > "$manifest"

tar -C "$repo" \
    --null -T "$manifest" \
    --transform "s#^#$name/#" \
    --sort=name \
    --mtime="@0" \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -czf "$dist/$archive_name"

(cd "$dist" && sha256sum "$archive_name" > "$archive_name.sha256")

echo "$dist/$archive_name"
echo "$dist/$archive_name.sha256"
