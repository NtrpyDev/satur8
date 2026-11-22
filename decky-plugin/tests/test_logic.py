# SPDX-License-Identifier: GPL-3.0-or-later
"""The plugin's rules: versions, the checksum, and the commands it builds.

Run from the repository root: python3 -m unittest discover -s decky-plugin/tests
"""

import hashlib
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "py_modules"))

import satur8_logic as logic  # noqa: E402


class Versions(unittest.TestCase):
    def test_reads_the_version_the_cli_prints(self):
        self.assertEqual(logic.parse_version("satur8 0.5.1\n"), "0.5.1")
        self.assertEqual(logic.parse_version("v0.5.1"), "0.5.1")
        self.assertEqual(logic.parse_version("satur8 1.12.0\nmore"), "1.12.0")

    def test_no_version_in_nothing_or_noise(self):
        for text in (None, "", "\n", "satur8: command not found", "satur8 8"):
            self.assertIsNone(logic.parse_version(text), text)

    def test_newer_compares_numbers_not_text(self):
        self.assertTrue(logic.is_newer("0.10.0", "0.9.9"))
        self.assertTrue(logic.is_newer("0.5.2", "0.5.1"))
        self.assertTrue(logic.is_newer("1.0", "0.99.99"))
        self.assertFalse(logic.is_newer("0.5.1", "0.5.1"))
        self.assertFalse(logic.is_newer("0.5.0", "0.5.1"))
        self.assertFalse(logic.is_newer("0.5", "0.5.0"))

    def test_latest_version_comes_from_githubs_redirect(self):
        self.assertEqual(
            logic.version_from_redirect(
                "https://github.com/NtrpyDev/satur8/releases/download/v0.5.0/"
                "satur8-linux-x86_64.tar.gz.sha256"
            ),
            "0.5.0",
        )

    def test_a_redirect_to_anywhere_else_is_no_version(self):
        for url in (
            "",
            None,
            "https://github.com/login",
            "https://github.com/Someone/satur8/releases/download/v9.9.9/x",
            "http://github.com/NtrpyDev/satur8/releases/download/v9.9.9/x",
            "https://github.com/NtrpyDev/satur8/releases/download/nightly/x",
        ):
            self.assertIsNone(logic.version_from_redirect(url), url)


class TheButton(unittest.TestCase):
    def test_not_installed_offers_the_install_even_offline(self):
        self.assertEqual(logic.button_label(None, "0.5.1"), "Install Satur8")
        self.assertEqual(logic.button_label(None, None), "Install Satur8")

    def test_an_older_install_offers_the_update_by_version(self):
        self.assertEqual(logic.button_label("0.5.0", "0.5.1"), "Update to v0.5.1")

    def test_up_to_date_or_unknown_has_no_button(self):
        self.assertIsNone(logic.button_label("0.5.1", "0.5.1"))
        self.assertIsNone(logic.button_label("0.5.1", None))
        self.assertIsNone(logic.button_label("0.6.0", "0.5.1"))

    def test_summary_lines(self):
        self.assertEqual(logic.summary(None, "0.5.1"), "Satur8 is not installed")
        self.assertEqual(logic.summary("0.5.1", None), "Satur8 v0.5.1 is installed")
        self.assertEqual(
            logic.summary("0.5.0", "0.5.1"), "Satur8 v0.5.0 is installed; v0.5.1 is out"
        )
        self.assertEqual(
            logic.summary("0.5.1", "0.5.1"), "Satur8 v0.5.1 is installed and up to date"
        )

    def test_result_says_installed_or_updated(self):
        self.assertEqual(logic.result_text(None, "0.5.1"), "Installed Satur8 v0.5.1")
        self.assertEqual(logic.result_text("0.5.0", "0.5.1"), "Updated to v0.5.1")
        self.assertEqual(logic.result_text("0.5.1", "0.5.1"), "Satur8 v0.5.1 was installed again")


class Checksum(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.file = os.path.join(self.dir.name, "satur8-linux-x86_64.tar.gz")
        with open(self.file, "wb") as handle:
            handle.write(b"the release")
        self.digest = hashlib.sha256(b"the release").hexdigest()

    def tearDown(self):
        self.dir.cleanup()

    def test_reads_a_sha256sum_line(self):
        line = self.digest + "  satur8-linux-x86_64.tar.gz\n"
        self.assertEqual(logic.parse_checksum(line), self.digest)
        self.assertEqual(logic.parse_checksum(self.digest.upper()), self.digest)

    def test_rejects_what_is_not_a_sha256(self):
        for text in ("", None, "Not Found", "abc123  file", self.digest[:-1], "<html>"):
            self.assertIsNone(logic.parse_checksum(text), text)

    def test_a_matching_file_has_no_problem(self):
        self.assertIsNone(logic.checksum_problem(self.file, self.digest + "  name\n"))

    def test_a_changed_file_is_a_problem(self):
        with open(self.file, "ab") as handle:
            handle.write(b"!")
        problem = logic.checksum_problem(self.file, self.digest + "  name\n")
        self.assertIn("does not match its checksum", problem)

    def test_a_checksum_file_that_is_a_web_page_is_a_problem(self):
        problem = logic.checksum_problem(self.file, "<html>Not Found</html>")
        self.assertIn("not a SHA-256 checksum", problem)


class Commands(unittest.TestCase):
    def test_the_addresses_are_satur8s_releases(self):
        self.assertEqual(
            logic.TARBALL_URL,
            "https://github.com/NtrpyDev/satur8/releases/latest/download/satur8-linux-x86_64.tar.gz",
        )
        self.assertEqual(logic.CHECKSUM_URL, logic.TARBALL_URL + ".sha256")

    def test_download_is_https_only_and_fails_on_an_error_page(self):
        command = logic.download_command(logic.TARBALL_URL, "/work/file")
        self.assertEqual(command[0], "curl")
        self.assertIn("--fail", command)
        self.assertEqual(command[command.index("--proto") + 1], "=https")
        self.assertEqual(command[command.index("--proto-redir") + 1], "=https")
        self.assertEqual(command[command.index("--output") + 1], "/work/file")
        self.assertEqual(command[-1], logic.TARBALL_URL)
        self.assertNotIn("--insecure", command)
        self.assertNotIn("-k", command)

    def test_no_command_is_built_for_another_address(self):
        for url in (
            "http://github.com/NtrpyDev/satur8/releases/latest/download/x",
            "https://example.com/satur8.tar.gz",
            "https://github.com/Someone/satur8/releases/latest/download/x",
        ):
            with self.assertRaises(ValueError):
                logic.download_command(url, "/work/file")
            with self.assertRaises(ValueError):
                logic.latest_command(url)

    def test_latest_asks_for_the_redirect_and_downloads_nothing(self):
        command = logic.latest_command()
        self.assertNotIn("--location", command)
        self.assertEqual(command[command.index("--output") + 1], "/dev/null")
        self.assertEqual(command[command.index("--write-out") + 1], "%{redirect_url}")
        self.assertEqual(command[-1], logic.CHECKSUM_URL)

    def test_unpack_goes_into_the_given_folder(self):
        command = logic.unpack_command("/work/a.tar.gz", "/work/unpacked")
        self.assertEqual(command[0], "tar")
        self.assertEqual(command[command.index("--file") + 1], "/work/a.tar.gz")
        self.assertEqual(command[command.index("--directory") + 1], "/work/unpacked")

    def test_the_installer_is_found_in_the_one_release_folder(self):
        self.assertEqual(
            logic.installer_in("/work/unpacked", ["satur8-v0.5.1-linux-x86_64"]),
            "/work/unpacked/satur8-v0.5.1-linux-x86_64/packaging/install.sh",
        )

    def test_anything_else_in_the_archive_is_refused(self):
        for names in (
            [],
            ["something-else"],
            ["satur8-v0.5.1-linux-x86_64", "extra"],
            ["satur8-v0.5.1-linux-x86_64", "satur8-v0.5.0-linux-x86_64"],
            ["satur8-vX-linux-x86_64"],
        ):
            self.assertIsNone(logic.installer_in("/work/unpacked", names), names)

    def test_install_turns_the_service_on_and_never_asks_for_root(self):
        command = logic.install_command("/w/packaging/install.sh", add_to_steam=True)
        self.assertEqual(command, ["bash", "/w/packaging/install.sh", "--enable-daemon"])
        self.assertNotIn("sudo", command)
        self.assertNotIn("--system", command)

    def test_install_leaves_the_library_alone_when_steam_lists_satur8(self):
        command = logic.install_command("/w/packaging/install.sh", add_to_steam=False)
        self.assertEqual(
            command, ["bash", "/w/packaging/install.sh", "--enable-daemon", "--no-steam-entry"]
        )

    def test_programs_are_looked_for_in_the_users_install(self):
        self.assertEqual(
            logic.version_command("/home/deck"), ["/home/deck/.local/bin/satur8", "--version"]
        )
        self.assertEqual(logic.app_path("/home/deck"), "/home/deck/.local/bin/satur8-gui")


class Surroundings(unittest.TestCase):
    def test_the_user_session_is_named_and_decky_libraries_are_left_out(self):
        env = logic.user_env(
            "/home/deck",
            "deck",
            1000,
            {"LD_LIBRARY_PATH": "/tmp/_MEI123", "DISPLAY": ":0", "SECRET": "x", "LANG": ""},
        )
        self.assertEqual(env["HOME"], "/home/deck")
        self.assertEqual(env["USER"], "deck")
        self.assertEqual(env["XDG_RUNTIME_DIR"], "/run/user/1000")
        self.assertEqual(env["DBUS_SESSION_BUS_ADDRESS"], "unix:path=/run/user/1000/bus")
        self.assertTrue(env["PATH"].startswith("/home/deck/.local/bin:"))
        self.assertEqual(env["DISPLAY"], ":0")
        self.assertNotIn("LD_LIBRARY_PATH", env)
        self.assertNotIn("SECRET", env)
        self.assertNotIn("LANG", env)

    def test_root_is_refused(self):
        self.assertIn("running as root", logic.root_problem(0))
        self.assertIsNone(logic.root_problem(1000))

    def test_a_failure_says_what_failed_and_where_the_log_is(self):
        text = logic.failure("The download from GitHub", "could not resolve host", "/logs/install.log")
        self.assertEqual(
            text,
            "The download from GitHub failed: could not resolve host. Log: /logs/install.log",
        )

    def test_last_lines_skips_blanks(self):
        self.assertEqual(logic.last_lines("a\n\nb\n c \n", 2), "b / c")
        self.assertEqual(logic.last_lines(None), "")


if __name__ == "__main__":
    unittest.main()
