#!/usr/bin/env bash
# Build Satur8's KWin effect (satur8.so) inside a target system's own packages.
#
# Usage: packaging/build-kwin-effect.sh OUT_DIR FAMILY RELEASE [testing]
#        packaging/build-kwin-effect.sh --all OUT_DIR [TARGETS_FILE]
#   FAMILY RELEASE  one target, as a line of packaging/kwin-effect-targets.txt:
#                   "steamos 3.9", "steamos main", "fedora 45", "fedora 45 testing"
#   OUT_DIR         receives <key>/satur8.so and <key>/build-info.txt, where key
#                   is steamos-<kwin> or fedora<NN>-<kwin>, for example
#                   steamos-6.7.3 or fedora45-6.8.3
#   --all           builds every target in TARGETS_FILE (default:
#                   packaging/kwin-effect-targets.txt) in order. When two
#                   targets give the same key, the first one is kept.
#
# Environment, all optional:
#   ENGINE      podman or docker (default: podman if installed, else docker)
#   LIMITS      resource flags for every container
#               (default: --cpus=3 --memory=4g; set to "" for none)
#   EFFECT_SRC  effect source folder (default: assets/kwin-effect)
#   WORK_DIR    scratch folder (default: a new folder beside OUT_DIR, removed at
#               the end; a WORK_DIR you pass is kept)
#   CHANNEL     SteamOS only: update channel whose image package list must match
#               every installed package the image also ships. Default by branch:
#               3.9 -> steamdeck (Stable), 3.10 -> steamdeck-preview,
#               main -> steamdeck-main. "none" skips the comparison.
#   KEEP_IMAGE  SteamOS only: 1 keeps the imported build-root image
#
# Why per target: KWin loads an effect only when its embedded ID names KWin's
# exact x.y.z version, and Qt refuses a plugin built against a newer Qt minor.
# Building inside the target's own package set gets the KWin headers, KWin's
# build options and the Qt version exactly right.
#
# SteamOS targets use Valve's package mirror. An Arch Linux container runs
# pacman with only Valve's repositories for the branch (jupiter, holo, core,
# extra, in that order, so kwin resolves to Valve's build). The mirror's
# databases are unsigned; every package is signed. The holo keyring comes from
# holo-<branch>, checked against the database's SHA-256 and its own signature,
# and pacman then requires a valid signature on every package. pacman --root
# installs the build set into an empty root, which is imported as an image, and
# the effect is built in that image with no network.
#
# Fedora targets use Fedora's own packages in a fedora:<release> container
# (updates-testing added for "testing").
#
# Checks on every effect, run inside its build system: the embedded ID equals
# the installed KWin's version, read both here and by the installer's own
# packaging/install.sh --check-effect; ldd -r finds every library, symbol and
# symbol version; build-info.txt records the package versions, the highest Qt_6.x,
# GLIBC, GLIBCXX and CXXABI versions the effect needs, the SHA-256 of each
# KWin header the build included, the Satur8 source version and the file's
# SHA-256.
set -euo pipefail

usage() { sed -n '2,50p' "$0"; exit 2; }
say() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"

# ------------------------------------------------------------ --all driver
if [ "${1:-}" = --all ]; then
    [ "$#" -ge 2 ] && [ "$#" -le 3 ] || usage
    out_dir="$2"
    targets_file="${3:-$here/kwin-effect-targets.txt}"
    [ -f "$targets_file" ] || fail "no targets file at $targets_file"
    mkdir -p "$out_dir"
    out_dir="$(cd "$out_dir" && pwd)"
    stage_root="$(mktemp -d "$out_dir/.staging.XXXXXX")"
    trap 'rm -rf "$stage_root"' EXIT
    summary=()
    failed=0
    n=0
    while read -r -a words; do
        [ "${#words[@]}" -gt 0 ] || continue
        case "${words[0]}" in '#'*) continue ;; esac
        n=$((n + 1))
        target="${words[*]}"
        stage="$stage_root/$n"
        say "Target $n: $target"
        if ! "$0" "$stage" "${words[@]}" < /dev/null; then
            summary+=("FAIL       $target")
            failed=1
            continue
        fi
        key="$(cd "$stage" && ls)"
        if [ -e "$out_dir/$key" ]; then
            say "Key $key from '$target' was already built by an earlier target; keeping the earlier one"
            summary+=("duplicate  $target -> $key (earlier target kept)")
        else
            mv "$stage/$key" "$out_dir/$key"
            summary+=("built      $target -> $key")
        fi
    done < "$targets_file"
    echo
    say "Summary"
    printf '%s\n' "${summary[@]}"
    [ "$failed" -eq 0 ] || fail "at least one target did not build"
    exit 0
fi

# ------------------------------------------------------------ one target
[ "$#" -ge 3 ] && [ "$#" -le 4 ] || usage
out_dir="$1"; family="$2"; release="$3"; extra="${4:-}"
case "$release" in *[!A-Za-z0-9.]*|'') fail "bad release: $release" ;; esac
case "$family:$extra" in
    steamos:) ;;
    fedora:|fedora:testing) ;;
    *) fail "unknown target: $family $release $extra" ;;
esac
target="$family $release${extra:+ $extra}"

effect_src="${EFFECT_SRC:-$repo/assets/kwin-effect}"
read -r -a limits <<< "${LIMITS---cpus=3 --memory=4g}"
keep_image="${KEEP_IMAGE:-0}"
engine="${ENGINE:-}"
if [ -z "$engine" ]; then
    for candidate in podman docker; do
        command -v "$candidate" >/dev/null 2>&1 && { engine="$candidate"; break; }
    done
fi
[ -n "$engine" ] || fail "needs podman or docker"
effect_files="CMakeLists.txt metadata.json satur8_effect.cpp satur8_effect.h"
for f in $effect_files; do
    [ -f "$effect_src/$f" ] || fail "missing $effect_src/$f"
done
effect_src="$(cd "$effect_src" && pwd)"
for tool in curl sha256sum awk sed; do
    command -v "$tool" >/dev/null 2>&1 || fail "missing tool: $tool"
done

mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"
if [ -n "${WORK_DIR:-}" ]; then
    work="$WORK_DIR"; own_work=false
    mkdir -p "$work"
else
    work="$(mktemp -d "$(dirname "$out_dir")/.kwin-effect-work.XXXXXX")"; own_work=true
fi
work="$(cd "$work" && pwd)"
mkdir -p "$work/check" "$work/out" "$work/cache"
rm -rf "${work:?}/out/"*

# Bootstrap image for SteamOS targets: only its pacman, gpg and curl are used.
# Pinned by digest. It also deletes root-owned files in the work folder.
boot_image="docker.io/library/archlinux@sha256:9edcc183d2505745a1da7a18bf12833dde174734610c72a5978031191504af1f"
root_image="localhost/satur8-effect-root:steamos-$release"
fedora_image="registry.fedoraproject.org/fedora:$release"
# Image that removes the work folder's container-written files at the end.
helper_image="$boot_image"
[ "$family" = fedora ] && helper_image="$fedora_image"

cleanup() {
    if [ "$family" = steamos ]; then
        "$engine" run --rm --network none -v "$work:/work" "$boot_image" \
            rm -rf /work/root /work/root.tar /work/gnupg /work/bootdb >/dev/null 2>&1 || true
        if [ "$keep_image" != 1 ]; then "$engine" rmi -f "$root_image" >/dev/null 2>&1 || true; fi
    fi
    if [ "$own_work" = true ]; then
        "$engine" run --rm --network none -v "$work:/work" "$helper_image" \
            sh -c 'rm -rf /work/* /work/.[!.]*' >/dev/null 2>&1 || true
        rm -rf "$work"
    fi
}
trap cleanup EXIT

t0=$(date +%s)
say "Building the KWin effect for $target with $engine"

# ------------------------------------------------------------ in-container build
# Runs in the target's build system after its packages are installed, with the
# effect source at /src (read-only) and the results in /out. Argument: the
# package manager, pacman or rpm. Builds exactly as packaging/release-tarball.sh
# does, then records what build-info.txt needs.
cat > "$work/check/build.sh" <<'BUILD'
#!/usr/bin/env bash
set -euo pipefail
pm="$1"
so=/out/satur8.so
pkg_version() {
    case "$pm" in
        pacman) pacman -Q "$1" 2>/dev/null | awk '{ print $2 }' ;;
        rpm) rpm -q --qf '%{VERSION}-%{RELEASE}\n' "$1" 2>/dev/null | head -n1 | grep -v 'not installed' ;;
    esac
}
owner_of() {
    case "$pm" in
        pacman) pacman -Qoq "$1" 2>/dev/null ;;
        rpm) rpm -qf --qf '%{NAME}\n' "$1" 2>/dev/null | head -n1 ;;
    esac
}

echo "+ cmake -S /src -B /build -DCMAKE_BUILD_TYPE=Release"
cmake -S /src -B /build -DCMAKE_BUILD_TYPE=Release
echo "+ cmake --build /build"
cmake --build /build
install -m755 /build/satur8.so "$so"
sha256sum "$so" | cut -d' ' -f1 > /out/sha256-in-container.txt

# The KWin version: the package that owns config-kwin.h, and the version string
# that header gives the effect's ID.
kwin_header=/usr/include/kwin/config-kwin.h
kwin_pkg="$(owner_of "$kwin_header")"
kwin_pkgver="$(pkg_version "$kwin_pkg")"
echo "$kwin_pkg ${kwin_pkgver%%-*} $kwin_pkgver" > /out/kwin.txt
sed -n 's/.*KWIN_PLUGIN_VERSION_STRING *"\([^"]*\)".*/\1/p' "$kwin_header" > /out/kwin-header-version.txt
qtcore="$(readlink -f "$(ls /usr/lib/libQt6Core.so.6 /usr/lib64/libQt6Core.so.6 2>/dev/null | head -n1)")"
qt_pkg="$(owner_of "$qtcore")"

# Package versions that matter, as "name version".
case "$pm" in
    pacman) wanted="kwin qt6-base qt6-declarative kcoreaddons kconfig libepoxy kdecoration libplasma kwindowsystem kglobalaccel glibc gcc gcc-libs binutils cmake make" ;;
    rpm) wanted="kwin-devel kwin-wayland kwin-libs qt6-qtbase qt6-qtbase-devel qt6-qtdeclarative kf6-kcoreaddons kf6-kconfig-core libepoxy glibc gcc-c++ libstdc++ binutils cmake make" ;;
esac
: > /out/packages.txt
for p in $wanted; do
    v="$(pkg_version "$p" || true)"
    [ -n "$v" ] && printf '%s %s\n' "$p" "$v" >> /out/packages.txt
done
echo "qt-core-owner $qt_pkg $(pkg_version "$qt_pkg") ($qtcore)" >> /out/packages.txt
case "$pm" in pacman) pacman -Q > /out/all-packages.txt ;; rpm) rpm -qa --qf '%{NAME} %{VERSION}-%{RELEASE}\n' | sort > /out/all-packages.txt ;; esac

# Every KWin header the compiler read, from the dependency files the build
# wrote, with its SHA-256. A distribution that patches one of these without
# changing the KWin version changes this list.
find /build -name '*.d' -type f -exec cat {} + | tr ' \\' '\n\n' | grep '^/usr/include/kwin/' \
    | xargs -r realpath | sort -u | xargs -r sha256sum > /out/kwin-headers.txt

{
    echo "### gcc --version / cmake --version"
    gcc --version | head -n1
    cmake --version | head -n1
    echo
    echo "### embedded plugin ID (grep -a satur8.so)"
    grep -aoE 'org\.kde\.kwin\.EffectPluginFactory[0-9]+(\.[0-9]+)+' "$so" | sort -u || echo "(no match)"
    echo
    echo "### KWIN_PLUGIN_VERSION_STRING in $kwin_header ($kwin_pkg $kwin_pkgver)"
    grep KWIN_PLUGIN_VERSION_STRING "$kwin_header"
    echo
    echo "### highest version required, per family (objdump -T)"
    for fam in Qt_6 GLIBC_ GLIBCXX_ CXXABI_; do
        all="$(objdump -T "$so" | grep -oE "\b${fam}[A-Za-z0-9_.]*" | grep -E "^${fam}" | sort -uV || true)"
        top="$(printf '%s\n' "$all" | tail -n1)"
        all="$(printf '%s' "$all" | tr '\n' ' ')"
        printf '%-8s highest %-14s all: %s\n' "$fam" "${top:-none}" "$all"
    done
    echo
    echo "### objdump -p satur8.so (version references)"
    objdump -p "$so" | sed -n '/Version References/,$p'
    echo
    echo "### NEEDED libraries, with the owning package"
    objdump -p "$so" | awk '$1 == "NEEDED" { print $2 }' | while read -r lib; do
        path="$(ldd "$so" | awk -v l="$lib" '$1 == l { print $3 }')"
        owner="$(owner_of "$path" || true)"
        printf '%-26s -> %-40s %s %s\n' "$lib" "${path:-NOT FOUND}" "${owner:-NOT OWNED}" "$(pkg_version "${owner:-none}" || true)"
    done
    echo
    # The installer's own check, so the build and the installer read the
    # embedded ID the same way: it compares the ID with the installed KWin.
    echo "### bash packaging/install.sh --check-effect satur8.so"
    rc=0
    bash /check/install.sh --check-effect "$so" 2>&1 || rc=$?
    echo "CHECK_EFFECT_RESULT: exit $rc"
    echo
    echo "### ldd -r satur8.so"
    ldd -r "$so" 2>&1
    echo
    if ldd -r "$so" 2>&1 | grep -qE 'not found|undefined symbol'; then
        echo "LDD_RESULT: FAIL"
    else
        echo "LDD_RESULT: clean"
    fi
} > /out/verify.txt 2>&1
cat /out/verify.txt
BUILD

# The installer, for its --check-effect gate inside the build system.
[ -f "$repo/packaging/install.sh" ] || fail "missing $repo/packaging/install.sh"
install -m644 "$repo/packaging/install.sh" "$work/check/install.sh"

downloads="unknown"
manifest_report=""
source_lines=()

# ------------------------------------------------------------ SteamOS
if [ "$family" = steamos ]; then
    mirror="https://steamdeck-packages.steamos.cloud/archlinux-mirror"
    meta_base="https://steamdeck-atomupd.steamos.cloud/meta/steamos/amd64/snapshot"
    images_base="https://steamdeck-images.steamos.cloud/steamdeck"
    # What the effect build needs, plus the tools the checks use.
    packages="bash coreutils grep sed findutils binutils pacman gcc make cmake kwin qt6-base qt6-declarative kcoreaddons kconfig libepoxy"
    if [ -z "${CHANNEL:-}" ]; then
        case "$release" in
            3.9) channel=steamdeck ;;
            3.10) channel=steamdeck-preview ;;
            main) channel=steamdeck-main ;;
            *) fail "no default CHANNEL for SteamOS branch $release; set CHANNEL" ;;
        esac
    else
        channel="$CHANNEL"
    fi
    cache="$work/cache"
    cache_before=$(du -sb "$cache" | cut -f1)

    if [ "$channel" != none ]; then
        say "Reading the $channel image package list"
        curl -fsS --proto =https -o "$work/meta.json" "$meta_base/$channel.json"
        buildid="$(sed -n 's/.*"buildid": *"\([^"]*\)".*/\1/p' "$work/meta.json" | head -n1)"
        osver="$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$work/meta.json" | head -n1)"
        [ -n "$buildid" ] && [ -n "$osver" ] || fail "cannot read $meta_base/$channel.json"
        manifest_url="$images_base/$buildid/steamdeck-$buildid-$osver.manifest.pacman"
        curl -fsS --proto =https -o "$work/manifest.pacman" "$manifest_url"
    fi

    cat > "$work/bootstrap.sh" <<'STAGE1'
#!/usr/bin/env bash
set -euo pipefail
branch="$1"; mirror="$2"; packages="$3"
conf=/work/pacman.conf
{
    printf '[options]\nArchitecture = x86_64\nSigLevel = Required DatabaseOptional\n'
    printf 'LocalFileSigLevel = Optional\nParallelDownloads = 5\n'
    printf 'DisableSandboxFilesystem\nDisableSandboxSyscalls\n'
    for repo in jupiter holo core extra; do
        printf '\n[%s-%s]\nServer = %s/$repo/os/$arch\n' "$repo" "$branch" "$mirror"
    done
} > "$conf"
rm -rf /work/root /work/bootdb /work/gnupg /work/kr
mkdir -p /work/root/var/lib/pacman /work/root/var/log /work/root/var/cache/pacman/pkg \
    /work/root/etc/pacman.d /work/root/dev /work/root/run /work/bootdb /work/kr
mkdir -m 1777 -p /work/root/tmp
mkdir -m 0555 -p /work/root/sys /work/root/proc

echo "--- repository databases"
pacman --config "$conf" --dbpath /work/bootdb -Sy --noconfirm
sha256sum /work/bootdb/sync/*.db

echo "--- holo keyring"
desc="$(bsdtar -xOf "/work/bootdb/sync/holo-$branch.db" 'holo-keyring-*/desc')"
fn="$(printf '%s\n' "$desc" | awk '/^%FILENAME%$/{getline; print; exit}')"
sum="$(printf '%s\n' "$desc" | awk '/^%SHA256SUM%$/{getline; print; exit}')"
[ -n "$fn" ] && [ -n "$sum" ] || { echo "holo-keyring not found in holo-$branch" >&2; exit 1; }
curl -fsS --proto =https -o "/work/kr/$fn" "$mirror/holo-$branch/os/x86_64/$fn"
curl -fsS --proto =https -o "/work/kr/$fn.sig" "$mirror/holo-$branch/os/x86_64/$fn.sig"
echo "$sum  /work/kr/$fn" | sha256sum -c
bsdtar -xf "/work/kr/$fn" -C /work/kr usr/share/pacman/keyrings
cp /work/kr/usr/share/pacman/keyrings/holo* /usr/share/pacman/keyrings/
pacman-key --gpgdir /work/gnupg --init >/dev/null 2>&1
pacman-key --gpgdir /work/gnupg --populate holo
pacman-key --gpgdir /work/gnupg --verify "/work/kr/$fn.sig"
echo "holo-keyring: $fn sha256 $sum"

echo "--- install into the empty root"
echo "+ pacman --config $conf --root /work/root --dbpath /work/root/var/lib/pacman --cachedir /cache --gpgdir /work/gnupg -Sy --noconfirm $packages"
# shellcheck disable=SC2086
pacman --config "$conf" --root /work/root --dbpath /work/root/var/lib/pacman \
    --cachedir /cache --gpgdir /work/gnupg -Sy --noconfirm $packages
cp "$conf" /work/root/etc/pacman.conf.satur8-build
tar --numeric-owner -C /work/root -cf /work/root.tar .
STAGE1

    say "Stage 1: installing the SteamOS $release package set (bootstrap $boot_image)"
    "$engine" run --rm --pull=missing "${limits[@]}" \
        -v "$work:/work" -v "$cache:/cache" "$boot_image" \
        bash /work/bootstrap.sh "$release" "$mirror" "$packages" 2>&1 | tee "$work/stage1.log"

    say "Importing the build root as $root_image"
    "$engine" rmi -f "$root_image" >/dev/null 2>&1 || true
    "$engine" import \
        --change 'ENV PATH=/usr/local/sbin:/usr/local/bin:/usr/bin' \
        --change 'ENV LANG=C.UTF-8' \
        "$work/root.tar" "$root_image" >/dev/null
    "$engine" run --rm --network none -v "$work:/work" "$boot_image" rm -rf /work/root /work/root.tar

    say "Stage 2: building the effect in $root_image (no network)"
    "$engine" run --rm --network none "${limits[@]}" \
        -v "$effect_src:/src:ro" -v "$work/check:/check:ro" -v "$work/out:/out" \
        "$root_image" bash /check/build.sh pacman 2>&1 | tee "$work/build.log"

    downloads="$(( ($(du -sb "$cache" | cut -f1) - cache_before) / 1048576 )) MiB of packages"
    source_lines+=("SteamOS repository branch: $release")
    for r in jupiter holo core extra; do source_lines+=("  [$r-$release] $mirror/$r-$release/os/x86_64/"); done
    source_lines+=("  (that order; databases unsigned on the mirror, every package signature required)")
    source_lines+=("Bootstrap image: $boot_image")
    source_lines+=("Signature trust:")
    while IFS= read -r line; do source_lines+=("  $line"); done < <(
        grep -E '^(holo-keyring:|gpg: (Signature made|Good signature)|gpg: +using RSA key)' "$work/stage1.log" || true)
    source_lines+=("Commands:")
    while IFS= read -r line; do source_lines+=("  stage 1: ${line#+ }"); done < <(grep -E '^\+ pacman ' "$work/stage1.log" || true)
    source_lines+=("  stage 2 (image $root_image, --network none): cmake -S <src> -B /build -DCMAKE_BUILD_TYPE=Release; cmake --build /build")

    if [ "$channel" != none ]; then
        manifest_report="$(awk '
            NR == FNR { if ($1 !~ /^#/) { n = $1; sub(/:.*/, "", n); img[n] = $2 }; next }
            { if ($1 in img) { if (img[$1] == $2) same++; else { diff++; d = d "\n  DIFFERENT " $1 " installed " $2 " image " img[$1] } }
              else { extra = extra " " $1 } }
            END {
                printf "%d installed packages equal the image version; %d differ.%s\n", same, diff, d
                printf "Not in the image (build-only):%s\n", extra
                if (diff > 0) exit 3
            }' "$work/manifest.pacman" "$work/out/all-packages.txt")" || {
            echo "$manifest_report" >&2
            fail "installed packages differ from the $channel image package list"
        }
        manifest_report="Image compared: channel $channel, SteamOS $osver build $buildid
  $meta_base/$channel.json
  $manifest_url
$(printf '%s\n' "$manifest_report" | sed 's/^/  /')"
    fi
fi

# ------------------------------------------------------------ Fedora
if [ "$family" = fedora ]; then
    image="$fedora_image"
    repo_flag=""
    [ "$extra" = testing ] && repo_flag="--enablerepo=updates-testing"
    # The effect's build requirements, as in packaging/satur8.spec, plus the
    # tools the checks use.
    packages="gcc-c++ make cmake extra-cmake-modules qt6-qtbase-devel qt6-qtdeclarative-devel kf6-kcoreaddons-devel kf6-kconfig-devel kwin-devel libepoxy-devel binutils findutils"
    say "Pulling $image"
    "$engine" pull -q "$image" >/dev/null
    digest="$("$engine" image inspect --format '{{index .RepoDigests 0}}' "$image" 2>/dev/null || echo unknown)"
    cat > "$work/check/fedora.sh" <<'FEDORA'
#!/usr/bin/env bash
set -euo pipefail
repo_flag="$1"; packages="$2"
echo "+ dnf -y --setopt=install_weak_deps=False $repo_flag install $packages"
# shellcheck disable=SC2086
dnf -y --setopt=install_weak_deps=False $repo_flag install $packages > /out/dnf.log 2>&1 || { tail -n 40 /out/dnf.log; exit 1; }
grep -iE 'Total size of inbound packages|Total download size' /out/dnf.log || true
bash /check/build.sh rpm
FEDORA
    say "Installing Fedora $release${extra:+ ($extra)} packages and building the effect in $image"
    "$engine" run --rm "${limits[@]}" \
        -v "$effect_src:/src:ro" -v "$work/check:/check:ro" -v "$work/out:/out" \
        "$image" bash /check/fedora.sh "$repo_flag" "$packages" 2>&1 | tee "$work/build.log"
    downloads="$(grep -m1 -iE 'Total size of inbound packages|Total download size' "$work/out/dnf.log" || echo unknown)"
    source_lines+=("Fedora $release${extra:+ with updates-testing}, image $image")
    source_lines+=("  digest $digest")
    source_lines+=("Commands:")
    source_lines+=("  $(grep -m1 '^+ dnf ' "$work/build.log" | sed 's/^+ //')")
    source_lines+=("  cmake -S <src> -B /build -DCMAKE_BUILD_TYPE=Release; cmake --build /build")
fi

# ------------------------------------------------------------ checks
so="$work/out/satur8.so"
[ -f "$so" ] || fail "the build produced no satur8.so"
sum_host="$(sha256sum "$so" | cut -d' ' -f1)"
sum_cont="$(cat "$work/out/sha256-in-container.txt")"
[ "$sum_host" = "$sum_cont" ] || fail "sha256 changed between container and host ($sum_cont vs $sum_host): possible hardware fault"
grep -q '^LDD_RESULT: clean' "$work/out/verify.txt" || fail "ldd -r is not clean inside the $target build system (see verify output above)"
grep -q '^CHECK_EFFECT_RESULT: exit 0$' "$work/out/verify.txt" || fail "packaging/install.sh --check-effect rejected the effect inside the $target build system (see verify output above)"

read -r kwin_pkg kwin_ver kwin_pkgver < "$work/out/kwin.txt"
header_ver="$(cat "$work/out/kwin-header-version.txt")"
case "$kwin_ver" in [0-9]*.[0-9]*.[0-9]*) ;; *) fail "cannot read the installed KWin version (got '$kwin_ver')" ;; esac
[ "$header_ver" = "$kwin_ver" ] || fail "config-kwin.h says KWin $header_ver but package $kwin_pkg is $kwin_pkgver"
ids="$(grep -aoE 'org\.kde\.kwin\.EffectPluginFactory[0-9]+(\.[0-9]+)+' "$so" | sort -u)"
[ "$ids" = "org.kde.kwin.EffectPluginFactory$kwin_ver" ] || fail "embedded ID '$ids' does not match the installed KWin $kwin_ver"
qt_tag="$(awk '$1 == "Qt_6" { print $3 }' "$work/out/verify.txt" | head -n1)"
case "$qt_tag" in Qt_6.[0-9]*) ;; *) fail "the effect records no Qt_6.x version (got '$qt_tag')" ;; esac

case "$family" in
    steamos) key="steamos-$kwin_ver" ;;
    fedora) key="fedora$release-$kwin_ver" ;;
esac
dest="$out_dir/$key"
rm -rf "$dest"
mkdir -p "$dest"
install -m755 "$so" "$dest/satur8.so"
[ "$(sha256sum "$dest/satur8.so" | cut -d' ' -f1)" = "$sum_host" ] || fail "sha256 changed on copy to $dest: possible hardware fault"

size="$(wc -c < "$dest/satur8.so" | tr -d ' ')"
satur8_version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -n1)"
satur8_commit="$(git -C "$repo" describe --always --dirty 2>/dev/null || echo 'not a git checkout')"
n_headers="$(wc -l < "$work/out/kwin-headers.txt" | tr -d ' ')"
headers_sum="$(cut -d' ' -f1 "$work/out/kwin-headers.txt" | sha256sum | cut -d' ' -f1)"
t1=$(date +%s)

{
    echo "Satur8 KWin effect $key"
    echo
    echo "File:       satur8.so, $size bytes"
    echo "sha256:     $sum_host"
    echo "Plugin ID:  $ids"
    echo "Target:     $target"
    echo "KWin:       $kwin_pkg $kwin_pkgver"
    echo "Needs:      $(awk '$1 ~ /^(Qt_6|GLIBC_|GLIBCXX_|CXXABI_)$/ { printf "%s%s", sep, $3; sep = ", " }' "$work/out/verify.txt")"
    echo "Satur8:     source version $satur8_version, commit $satur8_commit"
    echo "Built:      $(date -u +%Y-%m-%dT%H:%M:%SZ) in $((t1 - t0)) s; downloads this run: $downloads"
    echo "Builder:    $(uname -n), $("$engine" --version)"
    echo
    printf '%s\n' "${source_lines[@]}"
    [ -z "$manifest_report" ] || printf '%s\n' "$manifest_report"
    echo
    echo "Installed versions that matter:"
    awk '{ printf "  %-28s %s\n", $1, substr($0, length($1) + 2) }' "$work/out/packages.txt"
    echo
    echo "Effect source: $effect_src"
    for f in $effect_files; do echo "  $(sha256sum "$effect_src/$f" | cut -d' ' -f1)  $f"; done
    echo
    echo "KWin headers the build included: $n_headers, combined sha256 $headers_sum"
    echo "  (sha256 of the list of per-header sha256 values below, in this order)"
    sed 's/^/  /' "$work/out/kwin-headers.txt"
    echo
    echo "Checks (run inside the build system):"
    sed 's/^/  /' "$work/out/verify.txt"
} > "$dest/build-info.txt"

say "Done: $dest/satur8.so ($ids, $qt_tag, $size bytes, sha256 $sum_host)"
