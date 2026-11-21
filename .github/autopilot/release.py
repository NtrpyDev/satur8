#!/usr/bin/env python3
"""Autopilot release: turn the fixes waiting on main into a new patch version.

No AI. It does what scripts/release.sh does by hand: bump the version, date
the changelog section, update the Arch and Fedora package files with the
checksum of the reproducible source archive, commit one described commit per
top-level entry, and tag. The tag's artifacts are built by release.yml.

Fixes are batched: nothing is released until MIN_HOURS have passed since the
previous version, so a burst of small fixes becomes one version, not five.
"""

import datetime
import json
import os
import pathlib
import re
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parent
MIN_HOURS = float(os.environ.get("AUTOPILOT_RELEASE_MIN_HOURS", "6"))
GIT = ["git", "-c", "core.hooksPath=/dev/null", "-c", "user.name=satur8-autopilot",
       "-c", "user.email=41898282+github-actions[bot]@users.noreply.github.com"]


def run(cmd, check=True):
    result = subprocess.run(cmd, check=False, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if check and result.returncode != 0:
        sys.exit(f"command failed: {' '.join(cmd)}\n{result.stderr}")
    return result.stdout


def out(name, value):
    target = os.environ.get("GITHUB_OUTPUT")
    if target:
        with open(target, "a", encoding="utf-8") as handle:
            handle.write(f"{name}={value}\n")
    print(f"{name}={value}")


def replace_once(path, pattern, replacement):
    text = pathlib.Path(path).read_text(encoding="utf-8")
    new, count = re.subn(pattern, replacement, text, count=1, flags=re.M)
    if count != 1:
        sys.exit(f"{path}: expected exactly one match for {pattern!r}")
    pathlib.Path(path).write_text(new, encoding="utf-8")


def source_sha(version):
    run(["bash", "packaging/source-tarball.sh"])
    return pathlib.Path(f"target/dist/satur8-v{version}-source.tar.gz.sha256").read_text().split()[0]


def main():
    descriptions = json.loads((ROOT / "descriptions.json").read_text())
    changelog = pathlib.Path("CHANGELOG.md").read_text(encoding="utf-8")
    section = re.search(r"^## Unreleased\n(.*?)(?=^## )", changelog, re.S | re.M)
    if not section or not re.search(r"^- ", section.group(1), re.M):
        out("released", "false")
        print("Nothing is waiting to be released.")
        return

    cargo = pathlib.Path("Cargo.toml").read_text(encoding="utf-8")
    old = re.search(r'^version = "(\d+)\.(\d+)\.(\d+)"', cargo, re.M)
    if not old:
        sys.exit("could not read the workspace version from Cargo.toml")
    old_version = ".".join(old.groups())
    new_version = f"{old.group(1)}.{old.group(2)}.{int(old.group(3)) + 1}"

    tagged = run([*GIT, "log", "-1", "--format=%ct", f"v{old_version}"], check=False).strip()
    if tagged and time.time() - int(tagged) < MIN_HOURS * 3600 and os.environ.get("AUTOPILOT_RELEASE_NOW") != "true":
        out("released", "false")
        print(f"v{old_version} is less than {MIN_HOURS:g} hours old; batching fixes for now.")
        return

    today = datetime.date.today().isoformat()
    replace_once("CHANGELOG.md", r"^## Unreleased\n", f"## v{new_version} - {today}\n")
    replace_once("Cargo.toml", rf'^version = "{re.escape(old_version)}"', f'version = "{new_version}"')

    members = [package["name"] for package in
               json.loads(run(["cargo", "metadata", "--no-deps", "--format-version", "1"]))["packages"]]
    lock = pathlib.Path("Cargo.lock").read_text(encoding="utf-8")
    for name in members:
        lock, count = re.subn(
            rf'(\[\[package\]\]\nname = "{re.escape(name)}"\nversion = "){re.escape(old_version)}(")',
            rf"\g<1>{new_version}\g<2>", lock)
        if count != 1:
            sys.exit(f"Cargo.lock: expected one entry for {name} at {old_version}, found {count}")
    pathlib.Path("Cargo.lock").write_text(lock, encoding="utf-8")

    replace_once("packaging/PKGBUILD", r"^pkgver=.*$", f"pkgver={new_version}")
    replace_once("packaging/satur8.spec", r"^Version:.*$", f"Version:        {new_version}")

    def commit(path):
        run([*GIT, "add", path])
        run([*GIT, "commit", "-q", "-m", descriptions[path.split("/", 1)[0]]])

    commit("Cargo.toml")
    commit("Cargo.lock")
    commit("CHANGELOG.md")

    sha = source_sha(new_version)
    replace_once("packaging/PKGBUILD", r"^sha256sums=.*$", f"sha256sums=('{sha}')")
    srcinfo = pathlib.Path("packaging/.SRCINFO")
    text = srcinfo.read_text(encoding="utf-8").replace(old_version, new_version)
    text, count = re.subn(r"^(\s*sha256sums = ).*$", rf"\g<1>{sha}", text, flags=re.M)
    if count != 1:
        sys.exit("packaging/.SRCINFO: expected exactly one sha256sums line")
    srcinfo.write_text(text, encoding="utf-8")
    run([*GIT, "add", "packaging"])
    run([*GIT, "commit", "-q", "-m", descriptions["packaging"]])

    if source_sha(new_version) != sha:
        sys.exit("the source archive is not reproducible; refusing to release")

    run([*GIT, "tag", f"v{new_version}"])
    if os.environ.get("AUTOPILOT_DRY_RUN") != "true":
        run([*GIT, "push", "-q", "origin", "HEAD:main"])
        run([*GIT, "push", "-q", "origin", f"v{new_version}"])
    out("released", "true")
    out("tag", f"v{new_version}")
    print(f"Released v{new_version}.")


if __name__ == "__main__":
    main()
