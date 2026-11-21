#!/usr/bin/env bash
# Prepare the Fedora container the autopilot's fix sessions test in, and put
# check.sh where the session can run it but not edit it. No AI in this step.
set -euo pipefail

sudo install -D -m 0755 -o root -g root .github/autopilot/check.sh /opt/autopilot/check.sh

# The repository is mounted read-write so builds land in target/, but .git is
# masked by an empty volume: code under test must not be able to touch it.
docker run -d --name satur8-dev \
    -v "$PWD:/work" -v /work/.git \
    -w /work fedora:latest sleep infinity >/dev/null

docker exec satur8-dev bash -lc '
    set -euo pipefail
    dnf -y install dnf-plugins-core rpm-build rustfmt clippy >/tmp/dnf-base.log
    dnf -y builddep packaging/satur8.spec >/tmp/dnf-builddep.log
    mkdir -p /tmp/home && chmod 1777 /tmp/home
'
echo "dev container ready"
