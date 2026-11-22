# Shared by check-portability.sh, build-kwin-effect.sh and release-tarball.sh:
# the oldest C and C++ runtime the release tarball supports, and the check that
# a bundled KWin effect needs no more than that. Source it; it is not a program.
#
# The floors are those of Ubuntu 22.04, the oldest supported system. Each can be
# overridden from the environment.
#   GLIBC_FLOOR    2.35    Ubuntu 22.04's glibc (libc6 2.35-0ubuntu3.x)
#   GLIBCXX_FLOOR  3.4.30  libstdc++ from GCC 12 (libstdc++.so.6.0.30), in both
#                          22.04's release-day package (12-20220319-1ubuntu1)
#                          and its current one (12.3.0-1ubuntu1~22.04.3)
#   CXXABI_FLOOR   1.3.13  the same libstdc++ files
# GCC's ABI table agrees: "GCC 12.1.0: GLIBCXX_3.4.30, CXXABI_1.3.13"
# (https://gcc.gnu.org/onlinedocs/libstdc++/manual/abi.html).
#
# Every bundled effect is built with a gate on these floors and records what it
# needs in build-info.txt, so an effect never needs a newer C or C++ runtime
# than the tarball's own binaries. That leaves Qt as the only host-dependent
# requirement when the installer or `satur8 repair` uses an effect built for
# another system.

GLIBC_FLOOR="${GLIBC_FLOOR:-2.35}"
GLIBCXX_FLOOR="${GLIBCXX_FLOOR:-3.4.30}"
CXXABI_FLOOR="${CXXABI_FLOOR:-1.3.13}"

# True when version $1 is above version $2 (sort -V orders 2.9 before 2.10).
version_above() {
    [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n1)" = "$1" ]
}

# One ABI_<family>= value from a build-info.txt; empty when the key is missing.
build_info_value() { # build_info_value <file> <GLIBC|GLIBCXX|CXXABI>
    sed -n "s/^ABI_$2=//p" "$1" 2>/dev/null | head -n1
}

# Check a bundled effect's build-info.txt against the floors. Prints one line
# per problem, naming the effect, the symbol version and the floor, and returns
# non-zero when there is any. A value of "none" means the effect needs nothing
# from that library.
effect_abi_problems() { # effect_abi_problems <build-info.txt> <effect name>
    local info="$1" name="$2" family floor value bad=0
    for family in GLIBC GLIBCXX CXXABI; do
        case "$family" in
            GLIBC) floor="$GLIBC_FLOOR" ;;
            GLIBCXX) floor="$GLIBCXX_FLOOR" ;;
            CXXABI) floor="$CXXABI_FLOOR" ;;
        esac
        value="$(build_info_value "$info" "$family")"
        if [ -z "$value" ]; then
            echo "effect $name: build-info.txt has no ABI_$family line, so what it needs from the system is unknown"
            bad=1
        elif [ "$value" = none ]; then
            :
        elif ! [[ "$value" =~ ^[0-9]+(\.[0-9]+)+$ ]]; then
            echo "effect $name: build-info.txt has ABI_$family=$value, which is not a version"
            bad=1
        elif version_above "$value" "$floor"; then
            echo "effect $name needs ${family}_$value, above the floor ${family}_$floor"
            bad=1
        fi
    done
    return "$bad"
}
