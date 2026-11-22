#!/usr/bin/env bash
# Build the Satur8 Decky plugin ZIP: the file Decky Loader installs.
#
# Usage: decky-plugin/build-zip.sh [OUT_DIR]
#   OUT_DIR  receives satur8-decky-plugin.zip (default: target/dist)
#
# Steps: check the plugin's version against Satur8's, install the pinned build
# tools (pnpm, from pnpm-lock.yaml), type-check and build the panel, run the
# backend's tests, then pack the ZIP with pack-zip.py. Needs node, pnpm and
# python3; nothing is built in a container.
#
# The ZIP has no version in its name, so one address always serves the newest:
#   https://github.com/NtrpyDev/satur8/releases/latest/download/satur8-decky-plugin.zip
# It must stay the only .zip on a release: DeckyHub lists a plugin whose
# release has exactly one plugin ZIP.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
out="${1:-$repo/target/dist}"

say() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

for tool in node pnpm python3; do
    command -v "$tool" >/dev/null 2>&1 || fail "needs $tool"
done

# One repository, one version: DeckyHub compares the ZIP's version with the
# release tag, and the tag is Satur8's version.
workspace_version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo/Cargo.toml" | head -n1)"
plugin_version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$here/package.json")"
[ -n "$workspace_version" ] || fail "no version in $repo/Cargo.toml"
[ "$plugin_version" = "$workspace_version" ] ||
    fail "decky-plugin/package.json says $plugin_version, Cargo.toml says $workspace_version"

say "Backend tests"
(cd "$repo" && python3 -m unittest discover -s decky-plugin/tests)

say "Installing the build tools from pnpm-lock.yaml"
(cd "$here" && pnpm install --frozen-lockfile)
say "Type-checking and building the panel"
(cd "$here" && pnpm run typecheck && pnpm run build)

mkdir -p "$out"
say "Packing"
python3 "$here/pack-zip.py" "$repo" "$out/satur8-decky-plugin.zip"
