# SPDX-License-Identifier: GPL-3.0-or-later
"""main.py's install, start to finish, against a made-up release.

Decky is replaced by a stand-in module, GitHub by a folder of files (the
download command is swapped for `cp`), and Satur8's installer by a script
that records how it was called and leaves a `satur8` that prints a version.
Everything happens in a scratch home folder; nothing is installed.

Run from the repository root: python3 -m unittest discover -s decky-plugin/tests
"""

import asyncio
import hashlib
import importlib.util
import logging
import os
import subprocess
import sys
import tempfile
import types
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "py_modules"))

import satur8_logic as logic  # noqa: E402

FAKE_INSTALLER = """#!/usr/bin/env bash
set -eu
echo "$@" > "$HOME/installer-args.txt"
[ ! -f "$HOME/installer-must-fail" ] || { echo "no KWin here" >&2; exit 3; }
mkdir -p "$HOME/.local/bin"
printf '#!/bin/sh\\necho "satur8 %s"\\n' "__VERSION__" > "$HOME/.local/bin/satur8"
chmod +x "$HOME/.local/bin/satur8"
echo "installed __VERSION__"
"""


def make_release(folder, version, top=None):
    """A release folder: the tarball and its checksum, as GitHub serves them."""
    top = top or "satur8-v%s-linux-x86_64" % version
    stage = os.path.join(folder, "stage")
    os.makedirs(os.path.join(stage, top, "packaging"))
    with open(os.path.join(stage, top, "packaging", "install.sh"), "w") as handle:
        handle.write(FAKE_INSTALLER.replace("__VERSION__", version))
    tarball = os.path.join(folder, logic.TARBALL)
    subprocess.run(["tar", "-czf", tarball, "-C", stage, top], check=True)
    with open(tarball, "rb") as handle:
        digest = hashlib.sha256(handle.read()).hexdigest()
    with open(os.path.join(folder, logic.CHECKSUM), "w") as handle:
        handle.write("%s  %s\n" % (digest, logic.TARBALL))


class Backend(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        root = self.scratch.name
        self.home = os.path.join(root, "home")
        self.release = os.path.join(root, "release")
        os.makedirs(self.home)
        os.makedirs(self.release)

        decky = types.ModuleType("decky")
        decky.DECKY_USER_HOME = self.home
        decky.DECKY_USER = "deck"
        decky.DECKY_PLUGIN_LOG_DIR = os.path.join(root, "logs")
        decky.DECKY_PLUGIN_RUNTIME_DIR = os.path.join(root, "data")
        decky.logger = logging.getLogger("satur8-plugin-test")
        decky.logger.addHandler(logging.NullHandler())
        decky.logger.propagate = False
        sys.modules["decky"] = decky

        spec = importlib.util.spec_from_file_location("satur8_main", os.path.join(HERE, "..", "main.py"))
        self.main = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.main)
        self.plugin = self.main.Plugin()

        # "GitHub" is the release folder; everything else is the real command.
        self.real_download = logic.download_command

        def download(url, dest):
            self.real_download(url, dest)  # still refuses any other address
            return ["cp", os.path.join(self.release, url.rsplit("/", 1)[1]), dest]

        logic.download_command = download

    def tearDown(self):
        logic.download_command = self.real_download
        sys.modules.pop("decky", None)
        self.scratch.cleanup()

    def install(self, in_steam_library=False):
        async def run():
            await self.plugin.install(in_steam_library)
            await self.plugin.task
            return await self.plugin.status()

        return asyncio.run(run())

    def installer_args(self):
        path = os.path.join(self.home, "installer-args.txt")
        if not os.path.exists(path):
            return None
        with open(path) as handle:
            return handle.read().strip()

    def log(self):
        with open(os.path.join(self.scratch.name, "logs", "install.log")) as handle:
            return handle.read()

    def test_fresh_install_then_update(self):
        before = asyncio.run(self.plugin.status())
        self.assertIsNone(before["installed"])
        self.assertEqual(before["button"], "Install Satur8")
        self.assertEqual(before["summary"], "Satur8 is not installed")

        make_release(self.release, "0.5.1")
        status = self.install()
        self.assertEqual(status["error"], "")
        self.assertEqual(status["result"], "Installed Satur8 v0.5.1")
        self.assertEqual(status["installed"], "0.5.1")
        self.assertFalse(status["busy"])
        self.assertEqual(self.installer_args(), "--enable-daemon")
        self.assertIn("installed 0.5.1", self.log())
        self.assertFalse(os.path.exists(os.path.join(self.scratch.name, "data", "work")))

        release2 = os.path.join(self.scratch.name, "release2")
        os.makedirs(release2)
        make_release(release2, "0.5.2")
        self.release = release2
        status = self.install(in_steam_library=True)
        self.assertEqual(status["error"], "")
        self.assertEqual(status["result"], "Updated to v0.5.2")
        self.assertEqual(self.installer_args(), "--enable-daemon --no-steam-entry")

    def test_reinstall_of_the_current_version_runs_the_same_install(self):
        make_release(self.release, "0.5.1")
        first = self.install()
        self.assertEqual(first["result"], "Installed Satur8 v0.5.1")

        # Installed and current: no main button, the reinstall one instead.
        self.plugin.latest = "0.5.1"
        status = asyncio.run(self.plugin.status())
        self.assertIsNone(status["button"])
        self.assertEqual(status["reinstall"], "Reinstall Satur8")
        self.assertIn("install it over v0.5.1", status["reinstall_question"])

        # Something broke the install; the user's own files are beside it.
        os.remove(os.path.join(self.home, "installer-args.txt"))
        settings = os.path.join(self.home, ".config", "satur8")
        os.makedirs(settings)
        with open(os.path.join(settings, "profiles.toml"), "w") as handle:
            handle.write("default_saturation = 1.3\n")

        status = self.install(in_steam_library=True)
        self.assertEqual(status["error"], "")
        self.assertEqual(status["result"], "Satur8 v0.5.1 was installed again")
        self.assertEqual(self.installer_args(), "--enable-daemon --no-steam-entry")
        self.assertEqual(status["reinstall"], "Reinstall Satur8")
        self.assertIn("Checking the download against its SHA-256", self.log())
        with open(os.path.join(settings, "profiles.toml")) as handle:
            self.assertEqual(handle.read(), "default_saturation = 1.3\n")

    def test_reinstall_is_not_offered_before_satur8_is_installed(self):
        status = asyncio.run(self.plugin.status())
        self.assertIsNone(status["reinstall"])
        self.assertEqual(status["reinstall_question"], "")

    def test_a_reinstall_that_fails_its_checksum_leaves_the_install_alone(self):
        make_release(self.release, "0.5.1")
        self.install()
        os.remove(os.path.join(self.home, "installer-args.txt"))
        with open(os.path.join(self.release, logic.TARBALL), "ab") as handle:
            handle.write(b"tampered")
        status = self.install()
        self.assertIn("does not match its checksum", status["error"])
        self.assertIsNone(self.installer_args())
        self.assertEqual(status["installed"], "0.5.1")
        self.assertEqual(status["reinstall"], "Reinstall Satur8")

    def test_a_download_that_does_not_match_its_checksum_runs_nothing(self):
        make_release(self.release, "0.5.1")
        with open(os.path.join(self.release, logic.TARBALL), "ab") as handle:
            handle.write(b"tampered")
        status = self.install()
        self.assertIn("does not match its checksum", status["error"])
        self.assertIn("Nothing was run or installed", status["error"])
        self.assertIn(os.path.join(self.scratch.name, "logs", "install.log"), status["error"])
        self.assertEqual(status["result"], "")
        self.assertIsNone(status["installed"])
        self.assertIsNone(self.installer_args())
        self.assertNotIn("Unpacking", self.log())
        self.assertFalse(os.path.exists(os.path.join(self.scratch.name, "data", "work")))

    def test_a_failed_download_says_so(self):
        status = self.install()  # the release folder is empty
        self.assertIn("The download from GitHub failed", status["error"])
        self.assertIn("Check the internet connection", status["error"])
        self.assertIsNone(self.installer_args())

    def test_an_archive_that_is_not_a_satur8_release_is_refused(self):
        make_release(self.release, "0.5.1", top="surprise")
        status = self.install()
        self.assertIn("the release has no installer in it", status["error"])
        self.assertIsNone(self.installer_args())

    def test_a_failing_installer_is_reported_with_the_log(self):
        make_release(self.release, "0.5.1")
        open(os.path.join(self.home, "installer-must-fail"), "w").close()
        status = self.install()
        self.assertIn("Satur8's installer failed: it stopped with exit code 3", status["error"])
        self.assertIn("no KWin here", self.log())
        self.assertIsNone(status["installed"])

    def test_root_installs_nothing(self):
        make_release(self.release, "0.5.1")
        real = os.geteuid
        self.main.os.geteuid = lambda: 0
        try:
            status = self.install()
        finally:
            self.main.os.geteuid = real
        self.assertIn("running as root", status["error"])
        self.assertIsNone(self.installer_args())
        self.assertIsNone(status["installed"])

    def test_a_second_press_while_busy_starts_nothing(self):
        make_release(self.release, "0.5.1")

        async def run():
            await self.plugin.install(False)
            first = self.plugin.task
            await self.plugin.install(False)
            self.assertIs(self.plugin.task, first)
            self.assertTrue((await self.plugin.status())["busy"])
            await first

        asyncio.run(run())


if __name__ == "__main__":
    unittest.main()
