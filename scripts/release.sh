#!/usr/bin/env bash
# Satur8 release helper. Prepares and ships a tagged release end to end:
#
#   1. bump the workspace version and refresh Cargo.lock
#   2. build the reproducible source archive and update the Arch PKGBUILD/.SRCINFO
#      and the Fedora spec with its checksum
#   3. commit (one focused commit per file so the GitHub file list reads cleanly)
#   4. tag and push -> the release CI builds the source + Linux tarballs and
#      creates the GitHub release
#   5. trigger the Fedora COPR build
#
# A CHANGELOG.md entry for the new version must already exist (the only file the
# working tree may have uncommitted when you run this).
#
# After this finishes: wait for CI to attach the binary tarball, then update the
# website download in the satur8-site repo. AUR stays blocked upstream.
#
# Usage: scripts/release.sh X.Y.Z
set -euo pipefail

ver="${1:?usage: scripts/release.sh X.Y.Z}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"

dirty="$(git status --porcelain | grep -vE '(^| )CHANGELOG\.md$' || true)"
[ -z "$dirty" ] || { echo "working tree has changes other than CHANGELOG.md; commit or stash them first" >&2; exit 1; }
grep -q "^## v$ver " CHANGELOG.md || { echo "add a '## v$ver - DATE' section to CHANGELOG.md first" >&2; exit 1; }

echo "==> bumping workspace version to $ver"
sed -i "s/^version = \".*\"/version = \"$ver\"/" Cargo.toml
cargo check --workspace --quiet   # refresh Cargo.lock version entries

echo "==> building reproducible source archive"
bash packaging/source-tarball.sh >/dev/null
sha="$(awk '{print $1}' "target/dist/satur8-v$ver-source.tar.gz.sha256")"
echo "    sha256 = $sha"

echo "==> updating packaging metadata"
sed -i "s/^pkgver=.*/pkgver=$ver/; s/^sha256sums=.*/sha256sums=('$sha')/" packaging/PKGBUILD
sed -i "s/^Version:.*/Version:        $ver/" packaging/satur8.spec
( cd packaging && makepkg --printsrcinfo > .SRCINFO )

echo "==> verifying the source archive is reproducible"
bash packaging/source-tarball.sh >/dev/null
got="$(awk '{print $1}' "target/dist/satur8-v$ver-source.tar.gz.sha256")"
[ "$got" = "$sha" ] || { echo "source archive not reproducible ($got != $sha); aborting" >&2; exit 1; }

git --no-pager diff --stat
read -r -p "Commit, tag v$ver, push, and trigger COPR? [y/N] " ok
[ "$ok" = y ] || { echo "stopped before publishing; changes left in the working tree"; exit 0; }

T="Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
commit() { git add "$1"; git commit -q -m "$2" -m "$T"; }
commit Cargo.toml   "Rust workspace manifest listing the crates and shared dependencies"
commit Cargo.lock   "Exact pinned versions of every dependency for reproducible builds"
git add packaging/PKGBUILD packaging/.SRCINFO packaging/satur8.spec
git commit -q -m "Installer plus Arch, Fedora, and release-tarball packaging scripts" -m "$T"
commit CHANGELOG.md "Release notes for every Satur8 version, newest first"

git tag "v$ver"
git push origin main
git push origin "v$ver"
copr-cli buildscm --clone-url https://github.com/NtrpyDev/satur8.git \
  --commit "v$ver" --method make_srpm --nowait ntrpydev/satur8

cat <<EOF

v$ver tagged and pushed.
  * GitHub Actions builds the source + Linux tarballs and creates the release.
  * COPR build queued for Fedora 43/44.
  * Next: once CI attaches satur8-v$ver-linux-x86_64.tar.gz to the release,
    update the website download in the satur8-site repo.
  * AUR: still blocked on upstream account registration.
EOF
