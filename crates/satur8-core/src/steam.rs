//! Installed Steam games, read from the user's local Steam library files.
//!
//! Steam records every installed app in `steamapps/appmanifest_<AppID>.acf`
//! inside each library folder, and lists the library folders in
//! `steamapps/libraryfolders.vdf`. Both use Valve's text KeyValues format, so a
//! small parser here reads them without a VDF dependency. A missing or
//! malformed file is skipped, never fatal.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// An installed Steam game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamGame {
    /// Steam AppID, e.g. `4555040`.
    pub app_id: u32,
    /// The name Steam shows for the game, from its app manifest.
    pub name: String,
    /// The library folder the game is installed in (the folder that holds `steamapps/`).
    pub library: PathBuf,
}

/// The result of looking a game up by name or AppID with [`find_game`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameQuery {
    /// Exactly one game matches.
    Found(SteamGame),
    /// Several games match, sorted by name.
    Ambiguous(Vec<SteamGame>),
    /// No game matches.
    NotFound,
}

/// Steam install folders under `$HOME`, in search order: native Steam, its
/// `~/.steam` links, Flatpak Steam, Snap Steam.
const ROOTS: [&str; 5] = [
    ".local/share/Steam",
    ".steam/steam",
    ".steam/root",
    ".var/app/com.valvesoftware.Steam/.local/share/Steam",
    "snap/steam/common/.local/share/Steam",
];

/// The Steam install folders that exist for this user, in this order: native
/// Steam, its `~/.steam` links, Flatpak Steam, Snap Steam.
/// Folders that resolve to the same place (`~/.steam/steam` usually links to
/// `~/.local/share/Steam`) are listed once.
pub fn steam_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return Vec::new();
    };
    existing_unique(root_candidates(Path::new(&home)))
}

fn root_candidates(home: &Path) -> Vec<PathBuf> {
    ROOTS.into_iter().map(|root| home.join(root)).collect()
}

/// Keeps the folders that exist, dropping any that resolve to a folder
/// already kept. Order is preserved.
fn existing_unique(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| path.is_dir())
        .filter(|path| seen.insert(path.canonicalize().unwrap_or_else(|_| path.clone())))
        .collect()
}

/// The Steam library folders of one install: `root` itself plus every `"path"`
/// in its `libraryfolders.vdf`. Reads `steamapps/libraryfolders.vdf`, or
/// `config/libraryfolders.vdf` from older Steam when that is the one present.
/// Only folders that exist are returned, each once.
pub fn library_folders(root: &Path) -> Vec<PathBuf> {
    let listed = ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"]
        .into_iter()
        .map(|file| root.join(file))
        .find(|file| file.is_file())
        .and_then(|file| std::fs::read_to_string(file).ok())
        .map(|content| library_paths(&content))
        .unwrap_or_default();
    existing_unique(std::iter::once(root.to_path_buf()).chain(listed))
}

/// Every non-empty `"path"` value in a `libraryfolders.vdf`, in file order.
/// Malformed content gives no paths.
fn library_paths(content: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(pairs) = parse_keyvalues(content) {
        collect_paths(&pairs, &mut paths);
    }
    paths
}

fn collect_paths(pairs: &[(String, Value)], paths: &mut Vec<PathBuf>) {
    for (key, value) in pairs {
        match value {
            Value::Text(text) => {
                if key.eq_ignore_ascii_case("path") && !text.is_empty() {
                    paths.push(PathBuf::from(text));
                }
            }
            Value::Block(inner) => collect_paths(inner, paths),
        }
    }
}

/// The AppID and name from the content of an `appmanifest_<N>.acf`
/// (`"AppState" { "appid" "..." "name" "..." }`). `None` if the content is
/// malformed, the AppID is not a number, or the name is missing or empty.
pub fn parse_app_manifest(content: &str) -> Option<(u32, String)> {
    let top = parse_keyvalues(content)?;
    let Value::Block(state) = get(&top, "AppState")? else {
        return None;
    };
    let Value::Text(app_id) = get(state, "appid")? else {
        return None;
    };
    let Value::Text(name) = get(state, "name")? else {
        return None;
    };
    let app_id = app_id.trim().parse::<u32>().ok()?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((app_id, name.to_string()))
}

/// Whether an installed app is one of Steam's own runtimes or compatibility
/// tools (Proton, Steam Linux Runtime, redistributables) rather than a game.
pub fn is_tool(app_id: u32, name: &str) -> bool {
    app_id == 228980
        || name.starts_with("Proton ")
        || name == "Proton Experimental"
        || name.starts_with("Steam Linux Runtime")
        || name.starts_with("Steam Runtime")
        || name.starts_with("SteamVR")
        || name.contains("Proton EasyAntiCheat Runtime")
        || name.contains("Proton BattlEye Runtime")
}

/// Every installed Steam game across all installs and library folders, with
/// Steam's tools removed, each AppID once, sorted by name ignoring case.
pub fn installed_games() -> Vec<SteamGame> {
    let mut games = Vec::new();
    for root in steam_roots() {
        for library in library_folders(&root) {
            games.extend(library_games(&library));
        }
    }
    clean_game_list(games)
}

/// The apps installed in one library folder. Unreadable or malformed
/// manifests are skipped.
fn library_games(library: &Path) -> Vec<SteamGame> {
    let Ok(entries) = std::fs::read_dir(library.join("steamapps")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_manifest_file_name(path))
        .filter_map(|path| std::fs::read_to_string(&path).ok())
        .filter_map(|content| parse_app_manifest(&content))
        .map(|(app_id, name)| SteamGame {
            app_id,
            name,
            library: library.to_path_buf(),
        })
        .collect()
}

fn is_manifest_file_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("appmanifest_") && name.ends_with(".acf"))
}

/// Drops tools and repeated AppIDs (the first one seen wins), then sorts by name.
fn clean_game_list(games: Vec<SteamGame>) -> Vec<SteamGame> {
    let mut seen = HashSet::new();
    let mut games: Vec<SteamGame> = games
        .into_iter()
        .filter(|game| !is_tool(game.app_id, &game.name))
        .filter(|game| seen.insert(game.app_id))
        .collect();
    sort_by_name(&mut games);
    games
}

fn sort_by_name(games: &mut [SteamGame]) {
    games.sort_by_cached_key(|game| (game.name.to_lowercase(), game.app_id));
}

/// Looks a game up by AppID or name, in this order: the query as a number
/// equal to an AppID, then a case-insensitive exact name, then a
/// case-insensitive part of a name that only one game contains.
pub fn find_game(games: &[SteamGame], query: &str) -> GameQuery {
    let query = query.trim();
    if query.is_empty() {
        return GameQuery::NotFound;
    }
    if let Ok(app_id) = query.parse::<u32>() {
        if let Some(game) = games.iter().find(|game| game.app_id == app_id) {
            return GameQuery::Found(game.clone());
        }
    }
    let needle = query.to_lowercase();
    let exact: Vec<&SteamGame> = games
        .iter()
        .filter(|game| game.name.to_lowercase() == needle)
        .collect();
    let matches: Vec<&SteamGame> = if exact.is_empty() {
        games
            .iter()
            .filter(|game| game.name.to_lowercase().contains(needle.as_str()))
            .collect()
    } else {
        exact
    };
    let mut matches: Vec<SteamGame> = matches.into_iter().cloned().collect();
    match matches.len() {
        0 => GameQuery::NotFound,
        1 => GameQuery::Found(matches.remove(0)),
        _ => {
            sort_by_name(&mut matches);
            GameQuery::Ambiguous(matches)
        }
    }
}

/// A KeyValues value: a quoted string or a nested `{ }` block of pairs.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Text(String),
    Block(Vec<(String, Value)>),
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Text(String),
    Open,
    Close,
}

/// Deeper nesting than this is treated as malformed. Steam's files nest a few
/// levels; the cap keeps a hostile file from overflowing the stack.
const MAX_DEPTH: usize = 32;

/// Parses Valve's text KeyValues format into its top-level pairs. `None` if
/// the content is malformed: an unterminated string or block, a stray `}` or
/// `{`, a key without a value, or nesting deeper than [`MAX_DEPTH`].
fn parse_keyvalues(content: &str) -> Option<Vec<(String, Value)>> {
    let mut tokens = tokenize(content)?.into_iter();
    parse_block(&mut tokens, 0)
}

fn parse_block(
    tokens: &mut std::vec::IntoIter<Token>,
    depth: usize,
) -> Option<Vec<(String, Value)>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut pairs = Vec::new();
    loop {
        let key = match tokens.next() {
            None if depth == 0 => return Some(pairs),
            Some(Token::Close) if depth > 0 => return Some(pairs),
            Some(Token::Text(key)) => key,
            _ => return None,
        };
        let value = match tokens.next()? {
            Token::Text(text) => Value::Text(text),
            Token::Open => Value::Block(parse_block(tokens, depth + 1)?),
            Token::Close => return None,
        };
        pairs.push((key, value));
    }
}

/// Splits KeyValues text into strings and braces. Quoted strings honor `\\`
/// and `\"`; any other backslash is kept as written. `//` starts a comment
/// that runs to the end of the line. Bare words count as strings. `None` if
/// a quoted string is not closed.
fn tokenize(content: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = content.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        match c {
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '"' => {
                let mut text = String::new();
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            '\\' => text.push('\\'),
                            '"' => text.push('"'),
                            other => {
                                text.push('\\');
                                text.push(other);
                            }
                        },
                        other => text.push(other),
                    }
                }
                tokens.push(Token::Text(text));
            }
            _ => {
                if c == '/' && chars.peek() == Some(&'/') {
                    for skipped in chars.by_ref() {
                        if skipped == '\n' {
                            break;
                        }
                    }
                    continue;
                }
                let mut text = String::from(c);
                while let Some(&next) = chars.peek() {
                    if next.is_whitespace() || matches!(next, '{' | '}' | '"') {
                        break;
                    }
                    text.push(next);
                    chars.next();
                }
                tokens.push(Token::Text(text));
            }
        }
    }
    Some(tokens)
}

/// The first value under `key`, compared ignoring ASCII case as Steam does.
fn get<'a>(pairs: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    pairs
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The installed apps listed on a Steam Deck, tools included.
    const DECK: [(u32, &str); 18] = [
        (105600, "Terraria"),
        (1070560, "Steam Linux Runtime 1.0 (scout)"),
        (1086940, "Baldur's Gate 3"),
        (1245620, "ELDEN RING"),
        (1313140, "Cult of the Lamb"),
        (1391110, "Steam Linux Runtime 2.0 (soldier)"),
        (1493710, "Proton Experimental"),
        (1623730, "Palworld"),
        (1628350, "Steam Linux Runtime 3.0 (sniper)"),
        (228980, "Steamworks Common Redistributables"),
        (2379780, "Balatro"),
        (2868840, "Slay the Spire 2"),
        (3658110, "Proton 10.0"),
        (413150, "Stardew Valley"),
        (4183110, "Steam Linux Runtime 4.0"),
        (4421010, "Bills Must Be Paid"),
        (4555040, "Sushiamo"),
        (4628710, "Proton 11.0"),
    ];

    fn game(app_id: u32, name: &str) -> SteamGame {
        SteamGame {
            app_id,
            name: name.to_string(),
            library: PathBuf::from("/home/deck/.local/share/Steam"),
        }
    }

    fn deck_games() -> Vec<SteamGame> {
        clean_game_list(
            DECK.iter()
                .map(|&(app_id, name)| game(app_id, name))
                .collect(),
        )
    }

    fn names(games: &[SteamGame]) -> Vec<&str> {
        games.iter().map(|game| game.name.as_str()).collect()
    }

    const MANIFEST: &str = r#"
"AppState"
{
	"appid"		"4555040"
	"universe"		"1"
	"LauncherPath"		"C:\\Program Files (x86)\\Steam\\steam.exe"
	"name"		"Sushiamo"
	"StateFlags"		"4"
	"installdir"		"Sushiamo"
	"InstalledDepots"
	{
		"4555041"
		{
			"manifest"		"7193038312480317245"
			"size"		"412345678"
		}
	}
	"UserConfig"
	{
		"language"		"english"
	}
}
"#;

    #[test]
    fn manifest_gives_app_id_and_name() {
        assert_eq!(
            parse_app_manifest(MANIFEST),
            Some((4555040, "Sushiamo".to_string()))
        );
    }

    #[test]
    fn manifest_keys_ignore_case() {
        let content = r#""appstate" { "AppID" "730" "Name" "Counter-Strike 2" }"#;
        assert_eq!(
            parse_app_manifest(content),
            Some((730, "Counter-Strike 2".to_string()))
        );
    }

    #[test]
    fn broken_manifests_give_none() {
        for content in [
            "",
            "garbage",
            r#""AppState" { "appid" "730" "name" "Unclosed"#,
            r#""AppState" { "appid" "730" "name" "No closing brace""#,
            r#""AppState" { "appid" "not-a-number" "name" "Game" }"#,
            r#""AppState" { "appid" "730" }"#,
            r#""AppState" { "name" "Game" }"#,
            r#""AppState" { "appid" "730" "name" "  " }"#,
            r#""AppState" "flat""#,
            r#""Other" { "appid" "730" "name" "Game" }"#,
            r#""AppState" { "appid" { } "name" "Game" }"#,
            r#""AppState" { "appid" "730" "name" "Game" } }"#,
            r#"{ "appid" "730" }"#,
        ] {
            assert_eq!(parse_app_manifest(content), None, "{content:?}");
        }
    }

    #[test]
    fn quoted_strings_unescape_backslash_and_quote() {
        let content = r#""AppState" { "appid" "1" "name" "Say \"hi\" to C:\\Games\n" }"#;
        assert_eq!(
            parse_app_manifest(content),
            Some((1, r#"Say "hi" to C:\Games\n"#.to_string()))
        );
    }

    #[test]
    fn comments_and_bare_words_are_handled() {
        let content = "// written by Steam\n\
                       \"AppState\" // the app\n\
                       {\n\
                       \tappid 42 // bare words\n\
                       \t\"name\" \"A // not a comment\"\n\
                       }\n";
        assert_eq!(
            parse_app_manifest(content),
            Some((42, "A // not a comment".to_string()))
        );
    }

    #[test]
    fn very_deep_nesting_is_rejected_without_overflowing() {
        let content = "\"a\" {".repeat(100_000);
        assert_eq!(parse_keyvalues(&content), None);
    }

    #[test]
    fn library_paths_come_from_every_path_value() {
        let content = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/deck/.local/share/Steam"
		"label"		""
		"contentid"		"5012345678901234567"
		"totalsize"		"0"
		"apps"
		{
			"228980"		"201123456"
			"4555040"		"412345678"
		}
	}
	"1"
	{
		"path"		"/run/media/deck/SD card \"games\""
		"label"		"SD"
		"apps"
		{
		}
	}
}
"#;
        assert_eq!(
            library_paths(content),
            vec![
                PathBuf::from("/home/deck/.local/share/Steam"),
                PathBuf::from("/run/media/deck/SD card \"games\""),
            ]
        );
    }

    #[test]
    fn malformed_library_file_gives_no_paths() {
        assert!(library_paths("").is_empty());
        assert!(library_paths(r#""libraryfolders" { "0" { "path" "/a" "#).is_empty());
        assert!(library_paths(r#""libraryfolders" { "0" { "path" "" } }"#).is_empty());
    }

    #[test]
    fn roots_are_searched_native_first_then_flatpak_then_snap() {
        assert_eq!(
            root_candidates(Path::new("/home/deck")),
            vec![
                PathBuf::from("/home/deck/.local/share/Steam"),
                PathBuf::from("/home/deck/.steam/steam"),
                PathBuf::from("/home/deck/.steam/root"),
                PathBuf::from("/home/deck/.var/app/com.valvesoftware.Steam/.local/share/Steam"),
                PathBuf::from("/home/deck/snap/steam/common/.local/share/Steam"),
            ]
        );
    }

    #[test]
    fn missing_folders_are_dropped() {
        let missing = PathBuf::from("/nonexistent/satur8/steam/library");
        assert!(existing_unique([missing]).is_empty());
    }

    #[test]
    fn manifest_file_names_are_recognised() {
        assert!(is_manifest_file_name(Path::new(
            "/x/steamapps/appmanifest_4555040.acf"
        )));
        assert!(!is_manifest_file_name(Path::new(
            "/x/steamapps/libraryfolders.vdf"
        )));
        assert!(!is_manifest_file_name(Path::new(
            "/x/steamapps/appmanifest_4555040.acf.tmp"
        )));
    }

    #[test]
    fn deck_tools_are_removed_leaving_the_ten_games() {
        assert_eq!(
            names(&deck_games()),
            vec![
                "Balatro",
                "Baldur's Gate 3",
                "Bills Must Be Paid",
                "Cult of the Lamb",
                "ELDEN RING",
                "Palworld",
                "Slay the Spire 2",
                "Stardew Valley",
                "Sushiamo",
                "Terraria",
            ]
        );
    }

    #[test]
    fn other_tools_are_recognised() {
        assert!(is_tool(1, "Proton EasyAntiCheat Runtime"));
        assert!(is_tool(1, "Proton BattlEye Runtime"));
        assert!(is_tool(1, "Steam Runtime"));
        assert!(is_tool(1, "SteamVR"));
        assert!(!is_tool(1, "Protonaut"));
        assert!(!is_tool(1, "Steamworld Dig"));
    }

    #[test]
    fn repeated_app_ids_are_listed_once_and_names_sort_ignoring_case() {
        let mut second_library = game(4555040, "Sushiamo");
        second_library.library = PathBuf::from("/run/media/deck/SD");
        let games = clean_game_list(vec![
            game(2, "beta"),
            game(4555040, "Sushiamo"),
            game(1, "Alpha"),
            second_library,
        ]);
        assert_eq!(names(&games), vec!["Alpha", "beta", "Sushiamo"]);
        assert_eq!(
            games[2].library,
            PathBuf::from("/home/deck/.local/share/Steam")
        );
    }

    #[test]
    fn query_by_app_id_finds_the_game() {
        assert_eq!(
            find_game(&deck_games(), "4555040"),
            GameQuery::Found(game(4555040, "Sushiamo"))
        );
    }

    #[test]
    fn unique_part_of_a_name_finds_the_game() {
        assert_eq!(
            find_game(&deck_games(), "slay"),
            GameQuery::Found(game(2868840, "Slay the Spire 2"))
        );
        assert_eq!(
            find_game(&deck_games(), "SUSHI"),
            GameQuery::Found(game(4555040, "Sushiamo"))
        );
    }

    #[test]
    fn tools_never_match_because_they_are_not_listed() {
        assert_eq!(find_game(&deck_games(), "Proton"), GameQuery::NotFound);
        assert_eq!(find_game(&deck_games(), "1493710"), GameQuery::NotFound);
    }

    #[test]
    fn shared_part_of_several_names_is_ambiguous_and_sorted() {
        let GameQuery::Ambiguous(candidates) = find_game(&deck_games(), "e") else {
            panic!("expected an ambiguous match");
        };
        assert_eq!(
            names(&candidates),
            vec![
                "Baldur's Gate 3",
                "Bills Must Be Paid",
                "Cult of the Lamb",
                "ELDEN RING",
                "Slay the Spire 2",
                "Stardew Valley",
                "Terraria",
            ]
        );
    }

    #[test]
    fn exact_name_beats_a_longer_name_containing_it() {
        let games = clean_game_list(vec![game(620, "Portal 2"), game(400, "Portal")]);
        assert_eq!(
            find_game(&games, "portal"),
            GameQuery::Found(game(400, "Portal"))
        );
        assert_eq!(
            find_game(&games, "portal 2"),
            GameQuery::Found(game(620, "Portal 2"))
        );
    }

    #[test]
    fn unknown_or_empty_queries_find_nothing() {
        assert_eq!(find_game(&deck_games(), "zelda"), GameQuery::NotFound);
        assert_eq!(find_game(&deck_games(), "   "), GameQuery::NotFound);
        assert_eq!(find_game(&[], "sushiamo"), GameQuery::NotFound);
    }
}
