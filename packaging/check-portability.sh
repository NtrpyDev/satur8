#!/usr/bin/env bash
# Check that the four Satur8 release binaries start on an old enough Linux.
#
# Usage: packaging/check-portability.sh BIN_DIR
#   BIN_DIR  folder that holds satur8, satur8-daemon, satur8-tray, satur8-gui
#
# Environment, optional:
#   GLIBC_FLOOR  oldest glibc the binaries must run on (default: 2.35, the
#                glibc of Ubuntu 22.04, the oldest supported system we target;
#                the default lives in packaging/abi-floors.sh)
#
# Two checks per binary, both read from the file with objdump (binutils):
#   1. The highest GLIBC_x.y symbol version it asks for, weak references
#      included, must be at or below GLIBC_FLOOR. The loader refuses to start a
#      program that asks for a glibc version the system does not have.
#   2. Every NEEDED library must be on the allowed list: libc.so.6, libm.so.6
#      and libgcc_s.so.1 for all four, plus libfontconfig.so.1 for the desktop
#      app. Anything else is a library a user's system may not have; the app's
#      Wayland, X11 and GL libraries are opened at runtime, not linked.
# It prints one line per binary and one FAIL line per problem, naming the
# binary and the symbols or library, and exits 1 when anything fails.
set -euo pipefail

usage() { sed -n '2,20p' "$0"; exit 2; }
[ "$#" -eq 1 ] || usage
bin_dir="$1"
# shellcheck source=packaging/abi-floors.sh
. "$(cd "$(dirname "$0")" && pwd)/abi-floors.sh"
floor="$GLIBC_FLOOR"
case "$floor" in
    [0-9]*.[0-9]*) ;;
    *) echo "FAIL: GLIBC_FLOOR must look like 2.35, got '$floor'" >&2; exit 2 ;;
esac
command -v objdump >/dev/null 2>&1 || { echo "FAIL: needs objdump (binutils)" >&2; exit 1; }
[ -d "$bin_dir" ] || { echo "FAIL: no folder at $bin_dir" >&2; exit 1; }

binaries="satur8 satur8-daemon satur8-tray satur8-gui"
base_allowed="libc.so.6 libm.so.6 libgcc_s.so.1"
failed=0

echo "==> Checking the release binaries in $bin_dir against glibc $floor"
for name in $binaries; do
    file="$bin_dir/$name"
    if [ ! -f "$file" ]; then
        echo "FAIL: $name is missing from $bin_dir"
        failed=1
        continue
    fi
    if ! dynsyms="$(objdump -T "$file" 2>&1)"; then
        echo "FAIL: $name: objdump -T could not read it: $dynsyms"
        failed=1
        continue
    fi

    # Every glibc symbol version the binary asks for, oldest to newest. objdump
    # prints the version in the column before the symbol name, sometimes in
    # parentheses; the symbol name is always the last field.
    versions="$(printf '%s\n' "$dynsyms" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/^GLIBC_//' | sort -uV || true)"
    highest="$(printf '%s\n' "$versions" | tail -n1)"

    # GLIBC_PRIVATE is glibc's internal interface; it has no version number and
    # can change in any glibc release.
    private="$(printf '%s\n' "$dynsyms" | awk '/GLIBC_PRIVATE/ { print $NF }' | sort -u | tr '\n' ' ')"
    if [ -n "$private" ]; then
        echo "FAIL: $name uses glibc's private interface GLIBC_PRIVATE: $private"
        failed=1
    fi

    if [ -n "$highest" ] && version_above "$highest" "$floor"; then
        for v in $versions; do
            version_above "$v" "$floor" || continue
            syms="$(printf '%s\n' "$dynsyms" | awk -v tag="GLIBC_$v" '
                { for (i = 1; i < NF; i++) { t = $i; gsub(/[()]/, "", t); if (t == tag) { print $NF; break } } }' | sort -u | tr '\n' ' ')"
            echo "FAIL: $name needs GLIBC_$v, above the floor $floor, for: ${syms% }"
        done
        failed=1
    fi

    # NEEDED entries, from the dynamic section.
    needed="$(objdump -p "$file" | awk '$1 == "NEEDED" { print $2 }' | tr '\n' ' ')"
    allowed="$base_allowed"
    [ "$name" = satur8-gui ] && allowed="$allowed libfontconfig.so.1"
    for lib in $needed; do
        case " $allowed " in
            *" $lib "*) ;;
            *) echo "FAIL: $name links $lib, which is not on the allowed list ($allowed)"; failed=1 ;;
        esac
    done

    printf '%-14s highest GLIBC_%-6s NEEDED: %s\n' "$name" "${highest:-none}" "${needed% }"
done

if [ "$failed" -ne 0 ]; then
    echo "FAIL: the release binaries would not start on every system with glibc $floor or newer"
    exit 1
fi
echo "PASS: all four binaries need glibc $floor or older and link only allowed libraries"
