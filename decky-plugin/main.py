# SPDX-License-Identifier: GPL-3.0-or-later
"""The Satur8 Decky plugin's backend: install or update Satur8 for the user.

Decky Loader starts this as the Steam user, not as root, because plugin.json
does not carry the "root" flag (decky-loader, sandboxed_plugin.py: setuid to
the host user unless "root" is in the flags). It refuses to install if it
ever finds itself running as root.

It talks to one place only, Satur8's GitHub releases, and runs nothing from
a download before the download's SHA-256 has been checked. The rules are in
py_modules/satur8_logic.py; this file does the running.
"""

import asyncio
import os
import shutil
import time

import decky
import satur8_logic as logic


class Plugin:
    def __init__(self):
        self.busy = False
        self.step = 0
        self.result = ""
        self.error = ""
        self.latest = None
        self.latest_checked = 0.0
        self.task = None

    # ----------------------------------------------------------- surroundings

    def _home(self):
        return decky.DECKY_USER_HOME or os.path.expanduser("~")

    def _env(self):
        user = decky.DECKY_USER or os.environ.get("USER", "")
        return logic.user_env(self._home(), user, os.geteuid(), os.environ)

    def _log_path(self):
        return os.path.join(decky.DECKY_PLUGIN_LOG_DIR, "install.log")

    def _work_dir(self):
        return os.path.join(decky.DECKY_PLUGIN_RUNTIME_DIR, "work")

    def _clear_work_dir(self):
        # Only ever the plugin's own "work" folder inside its Decky data folder.
        if decky.DECKY_PLUGIN_RUNTIME_DIR:
            shutil.rmtree(self._work_dir(), ignore_errors=True)

    async def _run(self, command, log=None, timeout=960):
        """Run one program as the user. Returns (exit code, what it printed);
        with `log`, its output goes to that open file instead."""
        try:
            process = await asyncio.create_subprocess_exec(
                *command,
                stdin=asyncio.subprocess.DEVNULL,
                stdout=log if log is not None else asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
                env=self._env(),
                cwd=self._home(),
            )
        except OSError as error:
            return 127, str(error)
        try:
            out, _ = await asyncio.wait_for(process.communicate(), timeout)
        except asyncio.TimeoutError:
            process.kill()
            await process.wait()
            return 124, "it did not finish within %d seconds" % timeout
        return process.returncode, (out or b"").decode("utf-8", "replace")

    async def _installed(self):
        command = logic.version_command(self._home())
        if not os.path.isfile(command[0]):
            return None
        code, out = await self._run(command, timeout=10)
        return logic.parse_version(out) if code == 0 else None

    # ------------------------------------------------- called from the panel

    async def status(self):
        """Everything the panel shows. Local and quick: no network."""
        installed = await self._installed()
        return {
            "installed": installed,
            "latest": self.latest,
            "summary": logic.summary(installed, self.latest),
            "button": logic.button_label(installed, self.latest),
            "reinstall": logic.reinstall_label(installed, self.latest),
            "reinstall_question": (
                logic.reinstall_question(installed) if installed is not None else ""
            ),
            "busy": self.busy,
            "step": self.step,
            "steps": list(logic.STEPS),
            "result": self.result,
            "error": self.error,
            "log": self._log_path(),
            "app_name": logic.APP_NAME,
            "app_path": logic.app_path(self._home()),
            "app_dir": os.path.dirname(logic.app_path(self._home())),
        }

    async def check_latest(self):
        """Ask GitHub which release is the newest, at most once in ten
        minutes. Returns the version, or None when it could not be read."""
        if self.latest is not None and time.time() - self.latest_checked < 600:
            return self.latest
        code, out = await self._run(logic.latest_command(), timeout=40)
        found = logic.version_from_redirect(out.strip()) if code == 0 else None
        if found is not None:
            self.latest = found
            self.latest_checked = time.time()
        else:
            decky.logger.warning("could not read the newest Satur8 version: %s", out.strip())
        return self.latest

    async def install(self, in_steam_library=False):
        """Start the install, update or reinstall: all three are the same
        checked download and the same installer. Returns at once; `status`
        reports the step. `in_steam_library` is what Steam's own list says
        right now."""
        if self.busy:
            return
        self.busy = True
        self.step = 0
        self.result = ""
        self.error = ""
        self.task = asyncio.get_event_loop().create_task(self._install(bool(in_steam_library)))

    async def _install(self, in_steam_library):
        log_path = self._log_path()
        try:
            os.makedirs(os.path.dirname(log_path), exist_ok=True)
            with open(log_path, "ab") as log:
                self.error = await self._install_steps(in_steam_library, log, log_path) or ""
        except Exception as error:  # the panel must always get an answer
            decky.logger.exception("the install stopped unexpectedly")
            self.error = logic.failure("The install", str(error), log_path)
        finally:
            self._clear_work_dir()
            self.busy = False
            self.step = 0

    async def _install_steps(self, in_steam_library, log, log_path):
        """Returns None on success, else the message for the panel."""

        def note(text):
            log.write(("\n==> %s  %s\n" % (time.strftime("%Y-%m-%d %H:%M:%S"), text)).encode())
            log.flush()

        problem = logic.root_problem(os.geteuid())
        if problem:
            note(problem)
            return "Not installed: " + problem

        before = await self._installed()
        work = self._work_dir()
        self._clear_work_dir()
        unpacked = os.path.join(work, "unpacked")
        os.makedirs(unpacked)
        tarball = os.path.join(work, logic.TARBALL)
        checksum = os.path.join(work, logic.CHECKSUM)

        self.step = 1
        note("Downloading %s" % logic.TARBALL_URL)
        for url, dest in ((logic.CHECKSUM_URL, checksum), (logic.TARBALL_URL, tarball)):
            code, out = await self._run(logic.download_command(url, dest))
            if code != 0:
                note("curl exit %d: %s" % (code, out.strip()))
                return logic.failure(
                    "The download from GitHub",
                    logic.last_lines(out, 1) or "curl exit %d" % code,
                    log_path,
                ) + " Check the internet connection and try again."

        self.step = 2
        note("Checking the download against its SHA-256")
        with open(checksum, "r", encoding="utf-8", errors="replace") as handle:
            problem = logic.checksum_problem(tarball, handle.read())
        if problem:
            note(problem)
            return logic.failure("The check of the download", problem, log_path) + (
                " Nothing was run or installed. Try again."
            )

        self.step = 3
        note("Unpacking")
        code, out = await self._run(logic.unpack_command(tarball, unpacked))
        if code != 0:
            note("tar exit %d: %s" % (code, out.strip()))
            return logic.failure("Unpacking", logic.last_lines(out, 1), log_path)
        installer = logic.installer_in(unpacked, sorted(os.listdir(unpacked)))
        if installer is None or not os.path.isfile(installer):
            note("the release does not hold packaging/install.sh in one satur8-v*-linux-x86_64 folder")
            return logic.failure("Unpacking", "the release has no installer in it", log_path)

        self.step = 4
        command = logic.install_command(installer, add_to_steam=not in_steam_library)
        note("Running: " + " ".join(command))
        code, _ = await self._run(command, log=log)
        if code != 0:
            note("the installer stopped with exit code %d" % code)
            return logic.failure(
                "Satur8's installer", "it stopped with exit code %d" % code, log_path
            )

        after = await self._installed()
        if after is None:
            note("the installer finished, but %s does not run" % logic.version_command(self._home())[0])
            return logic.failure("Satur8's installer", "Satur8 does not start afterwards", log_path)
        self.result = logic.result_text(before, after)
        note(self.result)
        return None

    # ------------------------------------------------------ Decky lifecycle

    async def _main(self):
        decky.logger.info("Satur8 plugin started as uid %d", os.geteuid())

    async def _unload(self):
        if self.task is not None and not self.task.done():
            decky.logger.warning("unloaded while an install was running")

    async def _uninstall(self):
        # Removing the plugin leaves Satur8 itself installed; see the README.
        self._clear_work_dir()
