# SPDX-License-Identifier: GPL-3.0-or-later
"""What the Satur8 Decky plugin decides, as plain functions.

Nothing here touches the network, starts a program or imports Decky, so
every rule can be tested on any machine: `python3 -m unittest discover -s
decky-plugin/tests`. `main.py` does the running.

Only modules Decky Loader's own program already carries are imported (it is a
PyInstaller bundle, which holds just the standard-library modules it uses).
"""

import hashlib
import os
import re

# The only places this plugin talks to: Satur8's own GitHub releases.
REPO_URL = "https://github.com/NtrpyDev/satur8"
LATEST_URL = REPO_URL + "/releases/latest/download"
TARBALL = "satur8-linux-x86_64.tar.gz"
CHECKSUM = TARBALL + ".sha256"
TARBALL_URL = LATEST_URL + "/" + TARBALL
CHECKSUM_URL = LATEST_URL + "/" + CHECKSUM
# GitHub answers a release download with a redirect to its file servers.
ALLOWED_URL_PREFIX = REPO_URL + "/releases/"

# The name of Satur8's entry in the Steam library, and the program it starts.
APP_NAME = "Satur8"
APP_PROGRAM = "satur8-gui"

STEPS = ("Downloading", "Checking the download", "Unpacking", "Installing")

_VERSION = re.compile(r"(?<![\d.])v?(\d+(?:\.\d+){1,3})(?![\d.])")
_REDIRECT = re.compile(
    r"^https://github\.com/NtrpyDev/satur8/releases/download/v(\d+(?:\.\d+){1,3})/"
)
_SHA256 = re.compile(r"^[0-9a-f]{64}$")
_TOP_FOLDER = re.compile(r"^satur8-v\d+(?:\.\d+){1,3}-linux-x86_64$")


def parse_version(text):
    """The first version number in `text` ("satur8 0.5.1", "v0.5.1") as a
    string without the "v", or None."""
    if not text:
        return None
    found = _VERSION.search(text.strip().splitlines()[0] if text.strip() else "")
    return found.group(1) if found else None


def version_key(version):
    """A version string as numbers that compare the way versions do."""
    return tuple(int(part) for part in version.split("."))


def is_newer(candidate, than):
    """Whether `candidate` is a later version than `than`. Shorter versions
    count as padded with zeros, so 0.5 equals 0.5.0."""
    a, b = version_key(candidate), version_key(than)
    width = max(len(a), len(b))
    return a + (0,) * (width - len(a)) > b + (0,) * (width - len(b))


def version_from_redirect(url):
    """The release a "latest" download redirects to: GitHub answers
    .../releases/latest/download/NAME with .../releases/download/v0.5.1/NAME."""
    found = _REDIRECT.match(url or "")
    return found.group(1) if found else None


def action(installed, latest):
    """What the one button does: "install", "update", or None for no button.

    Not installed always offers the install, even when the newest version
    could not be read: the attempt then says what went wrong."""
    if installed is None:
        return "install"
    if latest is not None and is_newer(latest, installed):
        return "update"
    return None


def button_label(installed, latest):
    what = action(installed, latest)
    if what == "install":
        return "Install Satur8"
    if what == "update":
        return "Update to v" + latest
    return None


def reinstall_label(installed, latest):
    """The smaller second button, the repair path: Satur8 is installed and
    the main button offers nothing. With an update on offer the main button
    already does the same download and install, so there is no second one."""
    if installed is None or action(installed, latest) is not None:
        return None
    return "Reinstall Satur8"


def reinstall_question(installed):
    """The one line the panel asks before a reinstall. It says "newest"
    because the download is always the newest release, which is a later
    version when the check for one could not be made. Satur8's installer does
    not touch the saved games and settings."""
    return (
        "Download the newest Satur8 and install it over v%s? "
        "Your games and their settings are kept." % installed
    )


def summary(installed, latest):
    """One line on what is installed."""
    if installed is None:
        return "Satur8 is not installed"
    if latest is None:
        return "Satur8 v%s is installed" % installed
    if is_newer(latest, installed):
        return "Satur8 v%s is installed; v%s is out" % (installed, latest)
    return "Satur8 v%s is installed and up to date" % installed


def result_text(before, after):
    """What a finished install says."""
    if before is None:
        return "Installed Satur8 v%s" % after
    if before == after:
        return "Satur8 v%s was installed again" % after
    return "Updated to v%s" % after


def parse_checksum(text):
    """The SHA-256 from a `sha256sum` line ("<64 hex>  name"), or None."""
    words = (text or "").split()
    if not words:
        return None
    digest = words[0].lower()
    return digest if _SHA256.match(digest) else None


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while True:
            block = handle.read(1 << 20)
            if not block:
                return digest.hexdigest()
            digest.update(block)


def checksum_problem(tarball_path, checksum_text):
    """None when the file matches its checksum, else what is wrong."""
    wanted = parse_checksum(checksum_text)
    if wanted is None:
        return "the checksum file from GitHub is not a SHA-256 checksum"
    got = sha256_of(tarball_path)
    if got != wanted:
        return "the download does not match its checksum (expected %s, got %s)" % (
            wanted,
            got,
        )
    return None


def _checked_url(url):
    if not url.startswith(ALLOWED_URL_PREFIX):
        raise ValueError("not a Satur8 release address: " + url)
    return url


def download_command(url, dest):
    """curl, HTTPS only (redirects too), failing on an HTTP error. The system's
    curl is used, not Python's own HTTPS, because Decky Loader's bundled
    Python does not know where the system keeps its certificates."""
    return [
        "curl",
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--connect-timeout",
        "30",
        "--max-time",
        "900",
        "--output",
        dest,
        _checked_url(url),
    ]


def latest_command(url=CHECKSUM_URL):
    """curl that asks where the newest release's file is and prints the
    address GitHub redirects to, without downloading anything."""
    return [
        "curl",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--tlsv1.2",
        "--connect-timeout",
        "15",
        "--max-time",
        "30",
        "--output",
        "/dev/null",
        "--write-out",
        "%{redirect_url}",
        _checked_url(url),
    ]


def unpack_command(tarball, dest):
    return ["tar", "--extract", "--gzip", "--no-same-owner", "--file", tarball, "--directory", dest]


def installer_in(unpacked, names):
    """The installer inside the unpacked release: the one folder named
    satur8-v<version>-linux-x86_64, then packaging/install.sh. `names` is
    what the unpack folder holds. None when it is not exactly that."""
    tops = [name for name in names if _TOP_FOLDER.match(name)]
    if len(tops) != 1 or len(names) != 1:
        return None
    return os.path.join(unpacked, tops[0], "packaging", "install.sh")


def install_command(installer, add_to_steam):
    """Satur8's own installer, per user, with the background service on. When
    Steam already lists Satur8, the installer is told to leave the library
    alone, so an update can never add a second entry."""
    command = ["bash", installer, "--enable-daemon"]
    if not add_to_steam:
        command.append("--no-steam-entry")
    return command


def version_command(home):
    return [os.path.join(home, ".local", "bin", "satur8"), "--version"]


def app_path(home):
    return os.path.join(home, ".local", "bin", APP_PROGRAM)


# What a program started as the user is told about its surroundings, passed
# through from the plugin's own surroundings when set there.
_PASS_THROUGH = ("LANG", "LC_ALL", "DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY")


def user_env(home, user, uid, current):
    """The environment for everything this plugin starts.

    Decky Loader runs as a system service, so the plugin's own environment has
    no session in it; the user's runtime folder and session bus are named here
    so `systemctl --user` reaches the user's service manager. LD_LIBRARY_PATH
    is left out on purpose: Decky Loader's bundle points it at its own
    libraries, which system programs must not load.
    """
    runtime = "/run/user/%d" % uid
    env = {
        "HOME": home,
        "USER": user,
        "LOGNAME": user,
        "PATH": os.path.join(home, ".local", "bin") + ":/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "XDG_RUNTIME_DIR": runtime,
        "DBUS_SESSION_BUS_ADDRESS": "unix:path=%s/bus" % runtime,
    }
    for name in _PASS_THROUGH:
        if current.get(name):
            env[name] = current[name]
    return env


def root_problem(euid):
    """Satur8 installs for the user. As root it would land in root's home."""
    if euid == 0:
        return (
            "this plugin is running as root, and Satur8 must be installed as your user. "
            "Nothing was installed."
        )
    return None


def failure(step, detail, log_path):
    """What the panel shows when a step fails: what failed, and where the log is."""
    return "%s failed: %s. Log: %s" % (step, detail, log_path)


def last_lines(text, count=3):
    lines = [line.strip() for line in (text or "").splitlines() if line.strip()]
    return " / ".join(lines[-count:])
