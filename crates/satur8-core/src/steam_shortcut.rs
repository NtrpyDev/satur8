//! Satur8's own entry in the Steam library.
//!
//! Steam Game Mode shows only programs Steam started, so the desktop app is
//! reached there through a "non-Steam game" shortcut. Steam keeps those in
//! `userdata/<user>/config/shortcuts.vdf`, a binary file it rewrites while it
//! runs and when it exits. Nothing here writes that file: this module only
//! reads it, to tell whether the entry exists and which AppID Steam gave it,
//! and puts the entry's artwork where Steam looks for it,
//! `userdata/<user>/config/grid/`, named by that AppID.

use std::path::{Path, PathBuf};

/// One non-Steam shortcut, as far as this module needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    /// The AppID Steam gave the shortcut (stored signed; this is unsigned,
    /// the way Steam shows it and names artwork files).
    pub app_id: u32,
    pub name: String,
    /// The program, without the quotes Steam stores around it.
    pub exe: String,
    pub launch_options: String,
}

/// What a shortcut is to Satur8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// It starts the desktop app.
    App,
    /// A leftover that cannot start anything: Steam could not read the
    /// launcher entry of Satur8 v0.5.0 and made a shortcut for the entry file
    /// itself, named `satur8.desktop`.
    Broken,
}

/// The values of one binary VDF map that this module reads.
#[derive(Debug, Default)]
struct Fields {
    app_id: Option<u32>,
    name: Option<String>,
    exe: Option<String>,
    launch_options: Option<String>,
}

/// A cursor over binary VDF.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.at)?;
        self.at += 1;
        Some(byte)
    }

    /// A NUL-terminated string.
    fn text(&mut self) -> Option<String> {
        let rest = self.data.get(self.at..)?;
        let end = rest.iter().position(|byte| *byte == 0)?;
        self.at += end + 1;
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    }

    fn skip(&mut self, bytes: usize) -> Option<&'a [u8]> {
        let slice = self.data.get(self.at..self.at.checked_add(bytes)?)?;
        self.at += bytes;
        Some(slice)
    }

    /// Read a map's entries up to its end marker. `shortcuts` collects every
    /// map that has an `exe`; `depth` stops a malformed file from recursing
    /// without end.
    fn map(&mut self, shortcuts: &mut Vec<Shortcut>, depth: u32) -> Option<Fields> {
        if depth > 16 {
            return None;
        }
        let mut fields = Fields::default();
        loop {
            match self.byte()? {
                // End of this map.
                0x08 => break,
                // A nested map.
                0x00 => {
                    self.text()?;
                    let inner = self.map(shortcuts, depth + 1)?;
                    if let Some(exe) = inner.exe {
                        shortcuts.push(Shortcut {
                            app_id: inner.app_id.unwrap_or(0),
                            name: inner.name.unwrap_or_default(),
                            exe: unquote(&exe).to_string(),
                            launch_options: inner.launch_options.unwrap_or_default(),
                        });
                    }
                }
                // A string.
                0x01 => {
                    let key = self.text()?.to_ascii_lowercase();
                    let value = self.text()?;
                    match key.as_str() {
                        "appname" => fields.name = Some(value),
                        "exe" => fields.exe = Some(value),
                        "launchoptions" => fields.launch_options = Some(value),
                        _ => {}
                    }
                }
                // A 32-bit integer, little endian.
                0x02 => {
                    let key = self.text()?.to_ascii_lowercase();
                    let bytes = self.skip(4)?;
                    if key == "appid" {
                        fields.app_id =
                            Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
                    }
                }
                // A 32-bit float.
                0x03 => {
                    self.text()?;
                    self.skip(4)?;
                }
                // A 64-bit integer.
                0x07 | 0x0a => {
                    self.text()?;
                    self.skip(8)?;
                }
                _ => return None,
            }
        }
        Some(fields)
    }
}

fn unquote(text: &str) -> &str {
    let text = text.trim();
    text.strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(text)
}

/// The shortcuts in the content of a `shortcuts.vdf`, in file order. `None`
/// if the content is not binary VDF this reader understands.
pub fn parse_shortcuts(data: &[u8]) -> Option<Vec<Shortcut>> {
    let mut reader = Reader { data, at: 0 };
    let mut shortcuts = Vec::new();
    // The file is one map named "shortcuts" holding one map per shortcut.
    if reader.byte()? != 0x00 {
        return None;
    }
    reader.text()?;
    reader.map(&mut shortcuts, 0)?;
    Some(shortcuts)
}

/// What this shortcut is to Satur8, if anything. `Exe` is either the program
/// itself or, for a desktop entry Steam could read, sometimes a launcher with
/// the program in its options, so both are looked at.
pub fn kind(shortcut: &Shortcut) -> Option<Kind> {
    let file_name = |path: &str| {
        Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    };
    let exe = file_name(&shortcut.exe);
    if exe == "satur8.desktop" || shortcut.name.eq_ignore_ascii_case("satur8.desktop") {
        return Some(Kind::Broken);
    }
    let in_options = shortcut
        .launch_options
        .split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .any(|word| file_name(word) == "satur8-gui");
    (exe == "satur8-gui" || in_options).then_some(Kind::App)
}

/// The Steam user folders of one Steam install: `userdata/<number>`.
pub fn user_dirs(steam_root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(steam_root.join("userdata"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name != "0" && name.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    dirs.sort();
    dirs
}

/// The shortcuts of one Steam user. Empty when the user has none or the file
/// cannot be read.
pub fn read_shortcuts(user_dir: &Path) -> Vec<Shortcut> {
    std::fs::read(user_dir.join("config/shortcuts.vdf"))
        .ok()
        .and_then(|data| parse_shortcuts(&data))
        .unwrap_or_default()
}

/// What was found about Satur8's entry across every Steam user here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    /// Entries that start the app: the Steam user folder and the AppID.
    pub apps: Vec<(PathBuf, u32)>,
    /// Broken leftovers: the Steam user folder and the entry's name.
    pub broken: Vec<(PathBuf, String)>,
    /// How many Steam user folders were looked at.
    pub users: usize,
}

/// Look through every Steam install and user for Satur8's entries.
pub fn find() -> Found {
    let mut found = Found::default();
    for root in crate::steam::steam_roots() {
        for user in user_dirs(&root) {
            found.users += 1;
            for shortcut in read_shortcuts(&user) {
                match kind(&shortcut) {
                    Some(Kind::App) => found.apps.push((user.clone(), shortcut.app_id)),
                    Some(Kind::Broken) => found.broken.push((user.clone(), shortcut.name)),
                    None => {}
                }
            }
        }
    }
    found
}

/// The Steam user folders an entry with this AppID belongs to: those whose
/// shortcuts file lists it. Steam writes that file in its own time, so when
/// none lists it yet and there is only one Steam user here, that one.
pub fn users_with(app_id: u32, users: &[(PathBuf, Vec<Shortcut>)]) -> Vec<PathBuf> {
    let listed: Vec<PathBuf> = users
        .iter()
        .filter(|(_, shortcuts)| shortcuts.iter().any(|shortcut| shortcut.app_id == app_id))
        .map(|(user, _)| user.clone())
        .collect();
    match (listed.is_empty(), users) {
        (true, [(only, _)]) => vec![only.clone()],
        _ => listed,
    }
}

/// Put the artwork in place for the entry the running app was started from
/// (`SteamAppId` in its environment). Returns how many files were written.
pub fn place_artwork_for_running_app(app_id: u32) -> usize {
    let users: Vec<(PathBuf, Vec<Shortcut>)> = crate::steam::steam_roots()
        .iter()
        .flat_map(|root| user_dirs(root))
        .map(|user| {
            let shortcuts = read_shortcuts(&user);
            (user, shortcuts)
        })
        .collect();
    users_with(app_id, &users)
        .iter()
        .map(|user| place_artwork(user, app_id).unwrap_or(0))
        .sum()
}

/// The artwork of Satur8's library entry, drawn by
/// `scripts/generate-steam-artwork.py`: the suffix Steam expects after the
/// AppID in `config/grid/`, and the image.
pub const ARTWORK: [(&str, &[u8]); 5] = [
    // The portrait cover in the library grid, 600x900.
    ("p.png", include_bytes!("../../../assets/steam/capsule.png")),
    // The wide capsule, 920x430.
    (".png", include_bytes!("../../../assets/steam/header.png")),
    // The banner behind the entry's page, 1920x620.
    (
        "_hero.png",
        include_bytes!("../../../assets/steam/hero.png"),
    ),
    // The name over the banner.
    (
        "_logo.png",
        include_bytes!("../../../assets/steam/logo.png"),
    ),
    // The small icon.
    (
        "_icon.png",
        include_bytes!("../../../assets/steam/icon.png"),
    ),
];

/// The artwork files for an entry with this AppID, in `user_dir`.
pub fn artwork_paths(user_dir: &Path, app_id: u32) -> Vec<(PathBuf, &'static [u8])> {
    ARTWORK
        .iter()
        .map(|(suffix, image)| {
            (
                user_dir
                    .join("config/grid")
                    .join(format!("{app_id}{suffix}")),
                *image,
            )
        })
        .collect()
}

/// Put the entry's artwork in place. A file that is already there is left
/// alone, whatever it holds: it may be artwork the user chose in Steam.
/// Returns how many files were written.
pub fn place_artwork(user_dir: &Path, app_id: u32) -> std::io::Result<usize> {
    let mut written = 0;
    for (path, image) in artwork_paths(user_dir, app_id) {
        if path.exists() {
            continue;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, image)?;
        written += 1;
    }
    Ok(written)
}

/// Take the entry's artwork away again: only files that still hold exactly
/// what [`place_artwork`] wrote. Returns how many files were removed.
pub fn remove_artwork(user_dir: &Path, app_id: u32) -> usize {
    artwork_paths(user_dir, app_id)
        .into_iter()
        .filter(|(path, image)| std::fs::read(path).is_ok_and(|content| content == *image))
        .filter(|(path, _)| std::fs::remove_file(path).is_ok())
        .count()
}

/// Whether a Steam client is running for this user: a process named `steam`.
pub fn steam_is_running() -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(uid) = std::fs::metadata("/proc/self").map(|meta| meta.uid()) else {
        return false;
    };
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            let path = entry.path();
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
                && std::fs::read_to_string(path.join("comm"))
                    .is_ok_and(|comm| comm.trim() == "steam")
                && std::fs::metadata(&path).is_ok_and(|meta| meta.uid() == uid)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Binary VDF for a shortcuts file with these entries: name, quoted
    /// program, AppID. Keys are written the way current Steam writes them.
    fn vdf(entries: &[(&str, &str, u32, &str)]) -> Vec<u8> {
        let mut data = Vec::new();
        let text = |data: &mut Vec<u8>, key: &str, value: &str| {
            data.push(0x01);
            data.extend_from_slice(key.as_bytes());
            data.push(0);
            data.extend_from_slice(value.as_bytes());
            data.push(0);
        };
        let int = |data: &mut Vec<u8>, key: &str, value: u32| {
            data.push(0x02);
            data.extend_from_slice(key.as_bytes());
            data.push(0);
            data.extend_from_slice(&value.to_le_bytes());
        };
        data.push(0x00);
        data.extend_from_slice(b"shortcuts\0");
        for (index, (name, exe, app_id, options)) in entries.iter().enumerate() {
            data.push(0x00);
            data.extend_from_slice(index.to_string().as_bytes());
            data.push(0);
            int(&mut data, "appid", *app_id);
            text(&mut data, "AppName", name);
            text(&mut data, "Exe", exe);
            text(&mut data, "StartDir", "\"/home/deck/\"");
            text(&mut data, "icon", "");
            text(&mut data, "ShortcutPath", "");
            text(&mut data, "LaunchOptions", options);
            int(&mut data, "IsHidden", 0);
            int(&mut data, "AllowDesktopConfig", 1);
            int(&mut data, "AllowOverlay", 1);
            int(&mut data, "OpenVR", 0);
            int(&mut data, "Devkit", 0);
            text(&mut data, "DevkitGameID", "");
            int(&mut data, "DevkitOverrideAppID", 0);
            int(&mut data, "LastPlayTime", 1795300000);
            text(&mut data, "FlatpakAppID", "");
            // The tags map: one tag, then its end.
            data.push(0x00);
            data.extend_from_slice(b"tags\0");
            text(&mut data, "0", "favorite");
            data.push(0x08);
            data.push(0x08);
        }
        data.extend_from_slice(&[0x08, 0x08]);
        data
    }

    /// The owner's Deck on 2026-11-22: the entry Steam made for the app
    /// (AppID 3367991943) and the broken one from the first attempt.
    fn deck() -> Vec<u8> {
        vdf(&[
            (
                "Satur8",
                "\"/home/deck/.local/bin/satur8-gui\"",
                3367991943,
                "",
            ),
            (
                "satur8.desktop",
                "\"/home/deck/.local/share/applications/satur8.desktop\"",
                2903917283,
                "",
            ),
            ("Heroic", "\"/usr/bin/heroic\"", 2789367651, ""),
        ])
    }

    #[test]
    fn shortcuts_are_read_with_their_unsigned_app_ids() {
        let shortcuts = parse_shortcuts(&deck()).unwrap();
        assert_eq!(shortcuts.len(), 3);
        assert_eq!(
            shortcuts[0],
            Shortcut {
                app_id: 3367991943,
                name: "Satur8".into(),
                exe: "/home/deck/.local/bin/satur8-gui".into(),
                launch_options: String::new(),
            }
        );
        assert_eq!(shortcuts[2].name, "Heroic");
    }

    #[test]
    fn the_apps_entry_and_the_broken_one_are_told_apart() {
        let shortcuts = parse_shortcuts(&deck()).unwrap();
        let kinds: Vec<Option<Kind>> = shortcuts.iter().map(kind).collect();
        assert_eq!(kinds, vec![Some(Kind::App), Some(Kind::Broken), None]);
    }

    #[test]
    fn a_launcher_with_the_app_in_its_options_counts() {
        let data = vdf(&[(
            "Satur8",
            "\"/usr/bin/env\"",
            7,
            "\"/home/deck/.local/bin/satur8-gui\" --something",
        )]);
        let shortcuts = parse_shortcuts(&data).unwrap();
        assert_eq!(kind(&shortcuts[0]), Some(Kind::App));
        // Another program whose name only contains ours does not.
        let other = vdf(&[("X", "\"/opt/not-satur8-gui-helper\"", 8, "")]);
        assert_eq!(kind(&parse_shortcuts(&other).unwrap()[0]), None);
    }

    #[test]
    fn older_lowercase_keys_are_read_too() {
        let mut data = Vec::new();
        data.extend_from_slice(b"\0shortcuts\0\x000\0");
        data.extend_from_slice(b"\x01appname\0Satur8\0\x01exe\0/usr/bin/satur8-gui\0");
        data.extend_from_slice(&[0x08, 0x08, 0x08]);
        let shortcuts = parse_shortcuts(&data).unwrap();
        assert_eq!(shortcuts[0].exe, "/usr/bin/satur8-gui");
        // No AppID stored: 0, which nothing is placed for.
        assert_eq!(shortcuts[0].app_id, 0);
        assert_eq!(kind(&shortcuts[0]), Some(Kind::App));
    }

    #[test]
    fn an_empty_file_has_no_shortcuts_and_garbage_is_refused() {
        assert_eq!(parse_shortcuts(b"\0shortcuts\0\x08\x08"), Some(vec![]));
        assert_eq!(parse_shortcuts(b""), None);
        assert_eq!(parse_shortcuts(b"\"shortcuts\" { }"), None);
        // Cut off in the middle of a value.
        let mut cut = deck();
        cut.truncate(40);
        assert_eq!(parse_shortcuts(&cut), None);
    }

    #[test]
    fn artwork_is_named_by_the_app_id_the_way_steam_expects() {
        let names: Vec<String> = artwork_paths(Path::new("/s/userdata/55"), 3367991943)
            .into_iter()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                "/s/userdata/55/config/grid/3367991943p.png",
                "/s/userdata/55/config/grid/3367991943.png",
                "/s/userdata/55/config/grid/3367991943_hero.png",
                "/s/userdata/55/config/grid/3367991943_logo.png",
                "/s/userdata/55/config/grid/3367991943_icon.png",
            ]
        );
        // Every image is a PNG.
        for (_, image) in ARTWORK {
            assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
        }
    }

    #[test]
    fn artwork_is_placed_once_and_only_our_own_files_are_removed() {
        let user = std::env::temp_dir().join(format!(
            "satur8-shortcut-test-{}/userdata/55",
            std::process::id()
        ));
        std::fs::create_dir_all(user.join("config/grid")).unwrap();
        // The user already chose a cover in Steam.
        let cover = user.join("config/grid/77p.png");
        std::fs::write(&cover, b"the user's own cover").unwrap();
        assert_eq!(place_artwork(&user, 77).unwrap(), 4);
        assert_eq!(std::fs::read(&cover).unwrap(), b"the user's own cover");
        // A second run writes nothing.
        assert_eq!(place_artwork(&user, 77).unwrap(), 0);
        // Removing leaves the user's cover.
        assert_eq!(remove_artwork(&user, 77), 4);
        assert!(cover.exists());
        assert!(!user.join("config/grid/77_hero.png").exists());
        std::fs::remove_file(&cover).unwrap();
    }

    #[test]
    fn artwork_goes_to_the_user_whose_library_lists_the_entry() {
        let listing = |user: &str, ids: &[u32]| {
            (
                PathBuf::from(user),
                ids.iter()
                    .map(|id| Shortcut {
                        app_id: *id,
                        name: "Satur8".into(),
                        exe: "/usr/bin/satur8-gui".into(),
                        launch_options: String::new(),
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let two = [listing("/u/1", &[5]), listing("/u/2", &[9])];
        assert_eq!(users_with(9, &two), vec![PathBuf::from("/u/2")]);
        // Listed nowhere with two users here: nowhere, rather than a guess.
        assert!(users_with(7, &two).is_empty());
        // Not written to the file yet, one user: that user.
        let one = [listing("/u/1", &[])];
        assert_eq!(users_with(7, &one), vec![PathBuf::from("/u/1")]);
        assert!(users_with(7, &[]).is_empty());
    }

    #[test]
    fn only_numbered_user_folders_count() {
        let root = std::env::temp_dir().join(format!("satur8-users-test-{}", std::process::id()));
        for name in ["55", "1234567", "0", "anonymous", "ac"] {
            std::fs::create_dir_all(root.join("userdata").join(name)).unwrap();
        }
        let users: Vec<String> = user_dirs(&root)
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(users, vec!["1234567", "55"]);
        assert!(user_dirs(Path::new("/nonexistent")).is_empty());
    }
}
