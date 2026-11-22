# Satur8 for Decky Loader

A [Decky Loader](https://decky.xyz) plugin that installs, updates and opens
[Satur8](https://github.com/NtrpyDev/satur8) on a Steam Deck without leaving
Game Mode. Satur8 itself, the app where you set each game's color saturation,
is the main thing; this plugin gets it onto the Deck and keeps it current.

## What the panel does

Open the Quick Access Menu, then Decky, then Satur8.

- The first line says whether Satur8 is installed and which version.
- **Install Satur8**, or **Update to v…** when a newer release is out,
  downloads the release, checks it, and installs it for your user with the
  background service turned on. The panel shows the step it is on and, at the
  end, what it did. If something fails it says what failed and where the log
  is.
- **Open Satur8** starts Satur8's entry in the Steam library.
- **Add Satur8 to the Steam library** appears only when Satur8 is installed
  and Steam does not list it.

Removing the plugin leaves Satur8 installed. To remove Satur8 itself, see the
main README.

## What it touches

- Network: only `https://github.com/NtrpyDev/satur8/releases/...`, over HTTPS.
  It asks which release is the newest when the panel opens (at most once in
  ten minutes) and downloads `satur8-linux-x86_64.tar.gz` and its `.sha256`
  when you press the button.
- The download's SHA-256 is checked before anything in it is unpacked or run.
  The checksum comes from the same release, so it catches a damaged or cut-off
  download; HTTPS to GitHub is what vouches for where the files came from.
- It runs as your user, never as root: `plugin.json` does not carry Decky's
  `root` flag, and the backend refuses to install if it ever finds itself
  running as root. It then runs Satur8's own installer
  (`packaging/install.sh --enable-daemon`), which installs under `~/.local`.
- Files: the download is unpacked in Decky's data folder for this plugin and
  removed afterwards; the install log is `install.log` in Decky's log folder
  for this plugin (the panel shows the full path when something fails).

## Installing the plugin

- DeckyHub: find Satur8 on the Discover tab.
- Decky Loader: Settings, Developer, Install Plugin from URL, with
  `https://github.com/NtrpyDev/satur8/releases/latest/download/satur8-decky-plugin.zip`

## Building

```sh
decky-plugin/build-zip.sh        # writes target/dist/satur8-decky-plugin.zip
python3 -m unittest discover -s decky-plugin/tests
```

Needs node, pnpm and python3. `build-zip.sh` checks that `package.json` has
the same version as Satur8, runs the tests, type-checks and builds the panel,
and packs the ZIP. The release workflow attaches that ZIP to every Satur8
release; it is the only `.zip` there, which DeckyHub requires.

- `main.py`: the backend Decky Loader runs. It does the downloading and
  installing.
- `py_modules/satur8_logic.py`: every rule (versions, the checksum, the
  commands) as plain functions, tested in `tests/`.
- `src/index.tsx`: the panel.

## License

GPL-3.0-or-later, like Satur8. The build setup started from
[decky-plugin-template](https://github.com/SteamDeckHomebrew/decky-plugin-template)
(BSD 3-Clause); its license is kept in `LICENSE.decky-plugin-template` and at
the end of the ZIP's `LICENSE`.
