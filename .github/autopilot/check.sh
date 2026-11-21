#!/usr/bin/env bash
# Run Satur8's checks the way CI does, inside the Fedora container the
# autopilot prepared (see dev-container.sh). Installed read-only at
# /opt/autopilot/check.sh so the session that calls it cannot rewrite it.
#
# The container has the repository mounted but none of the job's secrets, and
# no .git directory, so code under test cannot reach credentials or plant hooks.
set -euo pipefail

what="${1:-all}"
filter="${2:-}"
if [ -n "$filter" ] && ! printf '%s' "$filter" | grep -Eq '^[A-Za-z0-9_:.-]{1,120}$'; then
    echo "check.sh: the test filter may only contain letters, digits, '_', ':', '.', '-'" >&2
    exit 2
fi

run() {
    docker exec \
        -u "$(id -u):$(id -g)" \
        -e HOME=/tmp/home -e CARGO_HOME=/tmp/home/.cargo -e CARGO_TERM_COLOR=never \
        -w /work satur8-dev bash -lc "$1"
}

case "$what" in
    all)    run "cargo fmt --all --check && cargo test --workspace --locked" ;;
    test)   run "cargo test --workspace --locked $filter" ;;
    lint)   run "cargo fmt --all --check && cargo clippy --workspace --all-targets --locked" ;;
    effect) run "cmake -S assets/kwin-effect -B target/kwin-effect-check -DCMAKE_BUILD_TYPE=Release >/dev/null && cmake --build target/kwin-effect-check" ;;
    *)      echo "usage: check.sh [all | test <filter> | lint | effect]" >&2; exit 2 ;;
esac
