#!/usr/bin/env bash
# Print the CHANGELOG.md section for a given version, for use as GitHub release
# notes. Usage: release-notes.sh 0.2.2   (with or without a leading "v")
set -euo pipefail

version="${1:?usage: release-notes.sh X.Y.Z}"
version="${version#v}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
changelog="$repo/CHANGELOG.md"

# Print everything between this version's "## vX.Y.Z" heading and the next
# "## " heading, trimming leading/trailing blank lines.
awk -v ver="$version" '
  $0 ~ ("^## v" ver "([^0-9]|$)") { found = 1; next }
  found && /^## / { exit }
  found { print }
' "$changelog" | sed -e '/./,$!d' | tac | sed -e '/./,$!d' | tac
