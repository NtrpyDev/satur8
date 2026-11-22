#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Pack the built Satur8 Decky plugin into the ZIP Decky Loader installs.

Usage: pack-zip.py REPO OUT.zip   (build-zip.sh runs this after the build)

Layout, from decky-plugin-template's README: one folder holding plugin.json,
package.json, main.py, dist/index.js, LICENSE and README.md, plus py_modules,
which Decky Loader puts on the backend's import path. Decky Loader refuses a
ZIP with no plugin.json one folder deep, or with more than one.

The same input gives the same bytes: entries are sorted, and carry one fixed
date and mode. The ZIP is read back and checked before this exits.
"""

import json
import os
import sys
import zipfile

FOLDER = "Satur8"
DATE = (2026, 1, 1, 0, 0, 0)
FILES = (
    "plugin.json",
    "package.json",
    "main.py",
    "py_modules/satur8_logic.py",
    "dist/index.js",
    "README.md",
)


def license_text(repo, plugin):
    """The plugin's notice, the GPL it names, then the template's license."""
    parts = []
    for path in (
        os.path.join(plugin, "LICENSE"),
        os.path.join(repo, "LICENSE"),
        os.path.join(plugin, "LICENSE.decky-plugin-template"),
    ):
        with open(path, "r", encoding="utf-8") as handle:
            parts.append(handle.read().strip("\n"))
    rule = "\n\n" + "=" * 78 + "\n\n"
    return (rule.join(parts) + "\n").encode("utf-8")


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    repo, out = os.path.abspath(sys.argv[1]), sys.argv[2]
    plugin = os.path.join(repo, "decky-plugin")

    entries = {}
    for name in FILES:
        path = os.path.join(plugin, name)
        if not os.path.isfile(path):
            sys.exit("FAIL: missing %s (dist/index.js is built by build-zip.sh)" % path)
        with open(path, "rb") as handle:
            entries[name] = handle.read()
    entries["LICENSE"] = license_text(repo, plugin)

    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name in sorted(entries):
            info = zipfile.ZipInfo(FOLDER + "/" + name, DATE)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, entries[name])

    # Read it back the way Decky Loader and DeckyHub do.
    with zipfile.ZipFile(out) as archive:
        names = archive.namelist()
        manifests = [n for n in names if n.endswith("/plugin.json") and n.count("/") == 1]
        if manifests != [FOLDER + "/plugin.json"]:
            sys.exit("FAIL: the ZIP must hold exactly one plugin.json, one folder deep: %s" % manifests)
        manifest = json.loads(archive.read(manifests[0]))
        package = json.loads(archive.read(FOLDER + "/package.json"))
        if "root" in manifest["flags"]:
            sys.exit("FAIL: plugin.json has the root flag; Satur8 installs as the user")
    print(
        "%s: plugin %r version %s, %d files, %d bytes"
        % (out, manifest["name"], package["version"], len(names), os.path.getsize(out))
    )


if __name__ == "__main__":
    main()
