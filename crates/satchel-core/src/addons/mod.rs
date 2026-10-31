//! Addons: the game's rules for them, and the Satchel's way of installing them safely.
//!
//! The game loads an addon from `benilla-config\AddOns\<Name>` when that folder holds
//! `<Name>.toc` (any case) saying `## Interface: 11200`. Addon code runs in the game's own Lua and
//! can't reach the PC; what a manager unpacks onto the disk is the real risk, so only addon file
//! types are ever written there, and only with the game closed.

pub mod catalog;
pub mod enabled;
pub mod forge;
pub mod github;
pub mod install;
pub mod overview;
pub mod package;
pub mod share;
pub mod store;
pub mod toc;
pub mod tree;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use toc::Toc;

/// File types an addon is made of: text, code, pictures, fonts and sounds. Anything else in a zip
/// (a program above all) is left out.
const ADDON_TYPES: &[&str] = &[
    "toc", "lua", "xml", "blp", "tga", "png", "jpg", "jpeg", "ttf", "otf", "mp3", "ogg", "wav", "txt", "md",
];

pub fn is_addon_file(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    ADDON_TYPES.iter().any(|t| t.eq_ignore_ascii_case(ext))
}

/// The game's addon folders.
#[derive(Clone, Debug)]
pub struct GameAddons {
    /// `<config>` itself (benilla-config): the game's own settings (`game_settings`).
    pub config: PathBuf,
    /// `<config>\AddOns`: one folder per addon, and the game's on/off list.
    pub addons: PathBuf,
    /// `<config>\saved`: each addon's settings, `<Addon>.lua`.
    pub saved: PathBuf,
}

impl GameAddons {
    /// `<config>` is `%BENILLA_HOME%` when set, as the game reads it; else `benilla-config`
    /// beside the game's exe.
    pub fn of(game_dir: &Path, benilla_home: Option<PathBuf>) -> GameAddons {
        let config = benilla_home.unwrap_or_else(|| game_dir.join("benilla-config"));
        GameAddons { addons: config.join("AddOns"), saved: config.join("saved"), config }
    }

    /// The game's on/off list.
    pub fn enable_list(&self) -> PathBuf {
        self.addons.join("account.txt")
    }

    /// An addon's settings file (its account-wide SavedVariables).
    pub fn settings_file(&self, addon: &str) -> PathBuf {
        self.saved.join(format!("{addon}.lua"))
    }

    /// The folder in AddOns with this name, whatever its case.
    pub fn folder(&self, name: &str) -> Option<PathBuf> {
        let exact = self.addons.join(name);
        if exact.is_dir() {
            return Some(exact);
        }
        fs::read_dir(&self.addons)
            .ok()?
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name) && e.path().is_dir())
            .map(|e| e.path())
    }
}

/// A folder found in AddOns.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub folder: String,
    /// Its `<folder>.toc`; `None` when the game won't load it (no such file).
    pub toc: Option<Toc>,
    /// Why it won't load, when it won't: in plain words, with the fix when there is one.
    pub problem: Option<String>,
}

/// The folders in AddOns, as the game sees them. Hidden folders (another tool's work folders,
/// like `.survivorsforge`) are skipped.
pub fn scan(addons_dir: &Path) -> io::Result<Vec<Found>> {
    let entries = match fs::read_dir(addons_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut found = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let folder = entry.file_name().to_string_lossy().into_owned();
        if folder.starts_with('.') || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let tocs: Vec<(String, PathBuf)> = fs::read_dir(entry.path())?
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                package::toc_stem(&name).map(|stem| (stem.to_string(), e.path()))
            })
            .collect();
        let own = tocs.iter().find(|(stem, _)| stem.eq_ignore_ascii_case(&folder));
        let (toc, problem) = match own {
            Some((_, path)) => {
                let toc = Toc::parse(&fs::read(path)?);
                let problem = (!toc.current()).then(|| {
                    format!("made for another game version (Interface {}); it loads only with \"Load out of date AddOns\"", toc.interface)
                });
                (Some(toc), problem)
            }
            None => (None, Some(wont_load(&folder, &tocs, &entry.path()))),
        };
        found.push(Found { folder, toc, problem });
    }
    found.sort_by_key(|f| f.folder.to_lowercase());
    Ok(found)
}

/// Why a folder without its own `.toc` won't load, with the usual causes named.
fn wont_load(folder: &str, tocs: &[(String, PathBuf)], dir: &Path) -> String {
    if let Some(stem) = folder.strip_suffix(".disabled") {
        return format!("turned off by another tool (renamed from {stem})");
    }
    if let [(stem, _)] = tocs {
        return format!("won't load: its .toc is {stem}.toc, so the folder should be named {stem}");
    }
    let inner = fs::read_dir(dir).ok().and_then(|mut entries| {
        entries.find_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            e.path().join(format!("{name}.toc")).is_file().then_some(name)
        })
    });
    match inner {
        Some(name) => format!("won't load: an extra folder level; the addon is {folder}\\{name}"),
        None => "won't load: no .toc named like the folder".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addon_file_types() {
        for name in ["A.toc", "x.LUA", "frame.xml", "Icon.blp", "a.TGA", "readme.md", "font.ttf", "s.ogg"] {
            assert!(is_addon_file(name), "{name}");
        }
        for name in ["tool.exe", "hook.dll", "run.ps1", "a.bat", "LICENSE", ".gitattributes", "x.js", "docs.json"] {
            assert!(!is_addon_file(name), "{name}");
        }
    }

    #[test]
    fn scans_addons_as_the_game_does() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameAddons::of(Path::new("unused"), Some(dir.path().to_path_buf()));
        let addons = game.addons.as_path();
        let make = |folder: &str, toc: Option<(&str, &str)>| {
            fs::create_dir_all(addons.join(folder)).unwrap();
            if let Some((file, text)) = toc {
                fs::write(addons.join(folder).join(file), text).unwrap();
            }
        };
        make("LuckyOdds", Some(("luckyodds.TOC", "## Interface: 11200\n## Version: 0.6.0\n")));
        make("OldOne", Some(("OldOne.toc", "## Interface: 30300\n")));
        make("BigCursor-main", Some(("BigCursor.toc", "## Interface: 11200\n")));
        make("Meter-v1", None);
        make("Meter-v1/Meter", Some(("Meter.toc", "## Interface: 11200\n")));
        make("Plates.disabled", Some(("Plates.toc", "## Interface: 11200\n")));
        make(".survivorsforge", None);
        fs::write(addons.join("account.txt"), "LuckyOdds: disabled\n").unwrap();

        let found = scan(addons).unwrap();
        let problems: Vec<(&str, Option<&str>)> = found.iter().map(|f| (f.folder.as_str(), f.problem.as_deref())).collect();
        assert_eq!(
            problems,
            [
                ("BigCursor-main", Some("won't load: its .toc is BigCursor.toc, so the folder should be named BigCursor")),
                ("LuckyOdds", None),
                ("Meter-v1", Some("won't load: an extra folder level; the addon is Meter-v1\\Meter")),
                ("OldOne", Some("made for another game version (Interface 30300); it loads only with \"Load out of date AddOns\"")),
                ("Plates.disabled", Some("turned off by another tool (renamed from Plates)")),
            ]
        );
        assert_eq!(found[1].toc.as_ref().unwrap().version.as_deref(), Some("0.6.0"));
        // A folder is found whatever the case it is asked for in.
        assert_eq!(game.folder("luckyodds"), Some(addons.join("LuckyOdds")));
        assert_eq!(game.folder("Missing"), None);
    }

    #[test]
    fn the_config_folder_follows_benilla_home() {
        let game = GameAddons::of(Path::new(r"C:\games\AzerothSurvivors"), None);
        assert_eq!(game.addons, Path::new(r"C:\games\AzerothSurvivors").join("benilla-config").join("AddOns"));
        assert_eq!(game.settings_file("LuckyOdds"), Path::new(r"C:\games\AzerothSurvivors").join("benilla-config").join("saved").join("LuckyOdds.lua"));
        let moved = GameAddons::of(Path::new("unused"), Some(PathBuf::from("/elsewhere")));
        assert_eq!(moved.enable_list(), Path::new("/elsewhere").join("AddOns").join("account.txt"));
    }
}
