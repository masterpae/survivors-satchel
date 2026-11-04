//! This PC's settings (`settings.toml`), and the optional defaults file beside the exe.

use crate::fsx;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub game_dir: Option<PathBuf>,
    pub pc_name: String,
    pub start_with_windows: bool,
    pub desktop_shortcut: bool,
    pub notifications: bool,
    /// Esc goes to the game when it stops being the window in front, so it pauses (an
    /// experiment, off unless turned on).
    pub pause_when_away: bool,
    pub theme: Theme,
    pub sync: SyncSettings,
    pub addons: AddonSettings,
    pub game: GameSettings,
    /// The window as it was left (since 0.9.0).
    pub window: WindowSettings,
}

/// The window's colours: as Windows' apps are, or always light, or always dark. Dark for a new
/// settings file (since 0.9.0); a file that names its theme keeps it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    #[default]
    Dark,
}

/// What the window remembers of itself: only whether it was maximised when it closed to the tray
/// or exited, so it reopens maximised. Nothing else about the window is kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    pub maximised: bool,
}

/// The game's own settings (`game_settings`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameSettings {
    /// With sync on: the game's interface options, key bindings, macros, chat, camera, window
    /// positions and HUD switches on every PC.
    pub shared: bool,
}

impl Default for GameSettings {
    fn default() -> Self {
        GameSettings { shared: true }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    pub enabled: bool,
    pub folder: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AddonSettings {
    /// Verified addons update by themselves before Play; off, they ask like the others.
    pub update_by_themselves: bool,
    /// With sync on: the same addons and settings on every PC.
    pub shared: bool,
    /// SurvivorsForge: more addons, descriptions and trust levels from its registry. Off, nothing
    /// is asked of it and the cache stays unused.
    pub forge: bool,
    /// How Browse shows the addons on this PC: `list`, `cards` or `table` (`BROWSE_VIEWS`), as its
    /// switch was left. Absent, or anything else, reads as `table`, the default since 0.8.0; so does
    /// a `cards` the player never picked (`browse_view_picked`, `Settings::load`). Not in the
    /// Settings tab.
    #[serde(default = "default_view", deserialize_with = "browse_view")]
    pub browse_view: String,
    /// The player picked `browse_view` with Browse's own switch (`pick_view`). Every settings.toml
    /// 0.7.2 wrote holds `browse_view = "cards"` without this key, its Save writing every field: a
    /// cards nobody picked, which loads as the table. Kept by Save, as every field is; absent in
    /// older files, it is false.
    #[serde(default)]
    pub browse_view_picked: bool,
}

/// Browse's views, as `browse_view` names them.
pub const BROWSE_VIEWS: [&str; 3] = ["list", "cards", "table"];

/// The view a new or unreadable `settings.toml` opens Browse in.
pub const DEFAULT_VIEW: &str = "table";

fn default_view() -> String {
    DEFAULT_VIEW.into()
}

/// One of Browse's views by its name, else the default (`DEFAULT_VIEW`): a name a later Satchel
/// adds, or a value of another type, reads as the default instead of failing the whole file.
pub fn browse_view_or_default(name: &str) -> String {
    BROWSE_VIEWS.iter().find(|v| **v == name.trim()).map_or_else(default_view, |v| v.to_string())
}

fn browse_view<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match toml::Value::deserialize(d)? {
        toml::Value::String(name) => browse_view_or_default(&name),
        _ => default_view(),
    })
}

impl Default for AddonSettings {
    fn default() -> Self {
        AddonSettings { update_by_themselves: true, shared: true, forge: true, browse_view: default_view(), browse_view_picked: false }
    }
}

impl AddonSettings {
    /// Browse's switch: the view the player picked, kept from now on, the cards too.
    pub fn pick_view(&mut self, name: &str) {
        self.browse_view = browse_view_or_default(name);
        self.browse_view_picked = true;
    }

    /// A cards nobody picked (every settings.toml 0.7.2 wrote has one) is the table, the default
    /// since 0.8.0, and is written back so with the next save. A view the player picked stays, and
    /// so does a list or a table.
    fn settle_view(&mut self) {
        if !self.browse_view_picked && self.browse_view == "cards" {
            self.browse_view = default_view();
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            game_dir: None,
            pc_name: String::new(),
            // Opt-in: the Satchel starts with Windows only when the player asks (Settings).
            start_with_windows: false,
            desktop_shortcut: true,
            notifications: true,
            pause_when_away: false,
            theme: Theme::default(),
            sync: SyncSettings::default(),
            addons: AddonSettings::default(),
            game: GameSettings::default(),
            window: WindowSettings::default(),
        }
    }
}

/// `SurvivorsSatchel.defaults.toml` beside the exe: read only when a PC has no settings yet.
/// The public release ships without it, so everyone else starts in one-PC mode.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub sync: Option<SyncSettings>,
    pub start_with_windows: Option<bool>,
    pub desktop_shortcut: Option<bool>,
    pub notifications: Option<bool>,
}

fn invalid(path: &Path, e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))
}

impl Settings {
    /// `None` when this PC has no settings yet (its first start). A cards view nobody picked loads
    /// as the table (`AddonSettings::settle_view`).
    pub fn load(path: &Path) -> io::Result<Option<Settings>> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Settings>(&text)
                .map(|mut settings| {
                    settings.addons.settle_view();
                    Some(settings)
                })
                .map_err(|e| invalid(path, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(|e| invalid(path, e))?;
        fsx::write_atomic(path, text.as_bytes())
    }

    pub fn apply(&mut self, defaults: &Defaults) {
        if let Some(sync) = &defaults.sync {
            self.sync = sync.clone();
        }
        if let Some(v) = defaults.start_with_windows {
            self.start_with_windows = v;
        }
        if let Some(v) = defaults.desktop_shortcut {
            self.desktop_shortcut = v;
        }
        if let Some(v) = defaults.notifications {
            self.notifications = v;
        }
    }

    /// Sync is on and has a folder to sync with.
    pub fn sync_folder(&self) -> Option<&Path> {
        if self.sync.enabled { self.sync.folder.as_deref() } else { None }
    }
}

impl Defaults {
    pub fn load(path: &Path) -> io::Result<Option<Defaults>> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map(Some).map_err(|e| invalid(path, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// One change from a Settings row ------------------------------------------------------------------

/// Why a change from a Settings row wasn't applied: the row (`key`), the text typed there, and the
/// reason the row shows in red. The settings in force stay as they were.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub key: String,
    pub typed: String,
    pub why: String,
}

/// What a change is checked against from outside this pure step: the file system, the network
/// folder's answer and the names the other PCs use. The app answers from Windows and its worker,
/// the tests from a table.
pub trait Checks {
    /// Why `folder` can't be the network folder with the game in `game_dir` (`paths::sync_folder_problem`:
    /// a whole drive, inside or around the game's folders or the Satchel's own).
    fn folder_problem(&self, folder: &Path, game_dir: Option<&Path>) -> Option<String>;
    /// The folder is there and answers now.
    fn folder_answers(&self, folder: &Path) -> bool;
    /// How another PC syncing here spells `name`, when one already uses it.
    fn name_taken(&self, name: &str) -> Option<String>;
    /// The game is in `dir`.
    fn game_found(&self, dir: &Path) -> bool;
}

/// One change from a Settings row, applied to the settings in force (`saved`): the settings to
/// write, or why not. `key` names the row as the window does: `start-with-windows`,
/// `desktop-shortcut`, `notifications`, `theme`, `sync`, `folder`, `pc-name`, `game-shared`,
/// `addons-auto`, `addons-shared`, `addons-forge`, `game-dir`, `pause-when-away`. `value` is `true`
/// or `false` for a switch, `system`, `light` or `dark` for the theme, the text for a field (the
/// network folder already in its network form, an emptied name already this PC's Windows name).
/// Only that row's value is checked, so a refused field never holds another row back, and nothing
/// typed in another field rides along:
/// - the network folder: emptied, it is cleared; else it mustn't be a risky place, and must answer;
/// - the sync switch turned on: a network folder already set must pass the same check;
/// - this PC's name: while sync is on, no other PC syncing there may use it;
/// - the game's folder: emptied, it is "Not set"; else the game must be in it, and it must stay
///   apart from the network folder.
pub fn apply(saved: &Settings, key: &str, value: &str, checks: &impl Checks) -> Result<Settings, Refusal> {
    let refuse = |why: String| Refusal { key: key.to_string(), typed: value.to_string(), why };
    let switch = || match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(refuse(format!("{other:?} is neither on nor off"))),
    };
    let mut next = saved.clone();
    match key {
        "start-with-windows" => next.start_with_windows = switch()?,
        "desktop-shortcut" => next.desktop_shortcut = switch()?,
        "notifications" => next.notifications = switch()?,
        "pause-when-away" => next.pause_when_away = switch()?,
        "addons-auto" => next.addons.update_by_themselves = switch()?,
        "addons-shared" => next.addons.shared = switch()?,
        "addons-forge" => next.addons.forge = switch()?,
        "game-shared" => next.game.shared = switch()?,
        "theme" => {
            next.theme = match value.trim() {
                "system" => Theme::System,
                "light" => Theme::Light,
                "dark" => Theme::Dark,
                other => return Err(refuse(format!("{other:?} isn't a theme"))),
            }
        }
        "sync" => {
            let on = switch()?;
            if on && !saved.sync.enabled
                && let Some(folder) = saved.sync.folder.as_deref()
            {
                folder_check(folder, saved.game_dir.as_deref(), checks).map_err(refuse)?;
            }
            next.sync.enabled = on;
        }
        "folder" => {
            let typed = value.trim();
            next.sync.folder = if typed.is_empty() {
                None
            } else {
                let folder = PathBuf::from(typed);
                folder_check(&folder, saved.game_dir.as_deref(), checks).map_err(refuse)?;
                Some(folder)
            };
        }
        "pc-name" => {
            let name = value.trim();
            if name.is_empty() {
                return Err(refuse("Give this PC a name".into()));
            }
            if saved.sync_folder().is_some()
                && let Some(taken) = checks.name_taken(name)
            {
                return Err(refuse(format!("Another PC syncing here is already called {taken}; give this one a name of its own")));
            }
            next.pc_name = name.to_string();
        }
        "game-dir" => {
            let typed = value.trim();
            next.game_dir = if typed.is_empty() {
                None
            } else {
                let dir = PathBuf::from(typed);
                if !checks.game_found(&dir) {
                    return Err(refuse("The game isn't in this folder".into()));
                }
                if let Some(folder) = saved.sync_folder()
                    && let Some(why) = checks.folder_problem(folder, Some(&dir))
                {
                    return Err(refuse(format!("It can't go with the network folder: {why}")));
                }
                Some(dir)
            };
        }
        other => return Err(refuse(format!("there is no setting called {other:?}"))),
    }
    Ok(next)
}

/// The network folder's check: not a risky place, and it answers.
fn folder_check(folder: &Path, game_dir: Option<&Path>, checks: &impl Checks) -> Result<(), String> {
    if let Some(why) = checks.folder_problem(folder, game_dir) {
        return Err(format!("Can't be used: {why}"));
    }
    if !checks.folder_answers(folder) {
        return Err("It doesn't answer: check the path, and that the PC or the drive that shares it is on".into());
    }
    Ok(())
}

/// What a change from a Settings row answers: the settings in force after it, the row whose
/// "✓ Saved" lights (only when its value changed and applied), and the refusal when it didn't.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub settings: Settings,
    pub applied: Option<String>,
    pub refused: Option<Refusal>,
}

/// One change, as the worker answers it (`apply`): a value that changed applies and lights its
/// row's "✓ Saved"; one that didn't change applies nothing and lights nothing; a refused one leaves
/// the settings as they were.
pub fn change(saved: &Settings, key: &str, value: &str, checks: &impl Checks) -> Answer {
    match apply(saved, key, value, checks) {
        Ok(next) => {
            let applied = (next != *saved).then(|| key.to_string());
            Answer { settings: next, applied, refused: None }
        }
        Err(refusal) => Answer { settings: saved.clone(), applied: None, refused: Some(refusal) },
    }
}

/// Restore defaults: the switches and the theme as a new settings file has them, except the sync
/// switch and the two sharing switches (the same addons on every PC, the game's own settings on
/// every PC). The game's folder, the network folder and this PC's name stay, as do Browse's view
/// and the window's state.
pub fn restore_defaults(saved: &Settings) -> Settings {
    let fresh = Settings::default();
    Settings {
        start_with_windows: fresh.start_with_windows,
        desktop_shortcut: fresh.desktop_shortcut,
        notifications: fresh.notifications,
        pause_when_away: fresh.pause_when_away,
        theme: fresh.theme,
        addons: AddonSettings { update_by_themselves: fresh.addons.update_by_themselves, forge: fresh.addons.forge, ..saved.addons.clone() },
        ..saved.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_windows_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let settings = Settings {
            game_dir: Some(PathBuf::from(r"C:\games\AzerothSurvivors")),
            pc_name: "PC 1".into(),
            sync: SyncSettings {
                enabled: true,
                folder: Some(PathBuf::from(r"\\homenas\games\Azeroth Survivors Saves")),
            },
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), Some(settings));
    }

    #[test]
    fn settings_from_0_1_have_addon_updates_on() {
        let settings: Settings = toml::from_str("pc_name = 'PC 1'\n[sync]\nenabled = true\n").unwrap();
        assert!(settings.addons.update_by_themselves);
        assert!(toml::to_string_pretty(&settings).unwrap().contains("[addons]\nupdate_by_themselves = true"));
    }

    #[test]
    fn missing_file_means_first_start() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(&dir.path().join("settings.toml")).unwrap(), None);
    }

    #[test]
    fn defaults_file_turns_sync_on() {
        let text = "[sync]\nenabled = true\nfolder = '\\\\homenas\\games\\Azeroth Survivors Saves'\n";
        let defaults: Defaults = toml::from_str(text).unwrap();
        let mut settings = Settings::default();
        settings.apply(&defaults);
        assert!(settings.sync.enabled);
        assert_eq!(
            settings.sync_folder(),
            Some(Path::new(r"\\homenas\games\Azeroth Survivors Saves"))
        );
        assert!(!settings.start_with_windows, "untouched keys keep their defaults: no start with Windows");
        assert!(settings.desktop_shortcut, "untouched keys keep their defaults");
    }

    /// Since 0.9.0 a new settings file is dark; a file that names its theme keeps it (every file
    /// 0.3 and later wrote names it, as Save writes every field), and one from before 0.3 is dark
    /// too, as a new one.
    #[test]
    fn a_new_settings_file_is_dark_and_an_existing_one_keeps_its_theme() {
        assert_eq!(Settings::default().theme, Theme::Dark);
        assert_eq!(Theme::default(), Theme::Dark);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        Settings::default().save(&path).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("theme = \"dark\""));
        for (line, theme) in [("theme = 'system'", Theme::System), ("theme = 'light'", Theme::Light), ("theme = 'dark'", Theme::Dark)] {
            let settings: Settings = toml::from_str(&format!("pc_name = 'PC 1'\n{line}\n[sync]\nenabled = true\n")).unwrap();
            assert_eq!(settings.theme, theme, "{line}");
            settings.save(&path).unwrap();
            assert_eq!(Settings::load(&path).unwrap().unwrap().theme, theme, "{line} kept");
        }
        // A file from before 0.3, which names no theme: as a new one.
        let old: Settings = toml::from_str("pc_name = 'PC 1'\n[sync]\nenabled = true\n[addons]\nshared = true\n").unwrap();
        assert_eq!(old.theme, Theme::Dark);
        assert!(old.game.shared);
    }

    /// The window remembers only whether it was maximised; a file without the key reads as not.
    #[test]
    fn the_window_remembers_only_that_it_was_maximised() {
        let from_0_8: Settings = toml::from_str("pc_name = 'PC 1'\ntheme = 'light'\n").unwrap();
        assert!(!from_0_8.window.maximised);
        assert!(!Settings::default().window.maximised);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let maximised = Settings { window: WindowSettings { maximised: true }, ..Settings::default() };
        maximised.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[window]\nmaximised = true"), "{text}");
        assert_eq!(Settings::load(&path).unwrap(), Some(maximised));
        // Written by a later Satchel with more under [window]: the rest stands.
        let later: Settings = toml::from_str("[window]\nmaximised = true\nleft = 10\n").unwrap();
        assert!(later.window.maximised);
    }

    #[test]
    fn settings_from_0_7_0_have_survivorsforge_on() {
        let settings: Settings = toml::from_str("pc_name = 'PC 1'\n[addons]\nupdate_by_themselves = true\nshared = true\n").unwrap();
        assert!(settings.addons.forge);
        let off = Settings { addons: AddonSettings { forge: false, ..settings.addons.clone() }, ..settings };
        let text = toml::to_string_pretty(&off).unwrap();
        assert!(text.contains("forge = false"), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), off);
    }

    #[test]
    fn browses_view_is_kept_and_reads_as_the_table_when_absent_or_unknown() {
        // Absent, as in every settings.toml before 0.7.2, and in a new one: the table.
        let before: Settings = toml::from_str("pc_name = 'PC 1'\n[addons]\nupdate_by_themselves = true\n").unwrap();
        assert_eq!(before.addons.browse_view, "table");
        assert_eq!(Settings::default().addons.browse_view, "table");
        assert_eq!(DEFAULT_VIEW, "table");
        // The player's choice is kept: written, and read back.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        for view in ["cards", "list", "table"] {
            let mut chosen = Settings::default();
            chosen.addons.pick_view(view);
            chosen.save(&path).unwrap();
            assert!(fs::read_to_string(&path).unwrap().contains(&format!("browse_view = \"{view}\"")));
            assert_eq!(Settings::load(&path).unwrap(), Some(chosen), "{view}");
        }
        // A view a later Satchel adds, or a value of another type: the table, and the rest stands.
        for line in ["browse_view = \"gallery\"", "browse_view = 3", "browse_view = \"\"", "browse_view = [\"list\"]"] {
            let read: Settings = toml::from_str(&format!("[addons]\nforge = false\n{line}\n")).unwrap();
            assert_eq!((read.addons.browse_view.as_str(), read.addons.forge), ("table", false), "{line}");
        }
        assert_eq!((browse_view_or_default(" list "), browse_view_or_default("Cards")), ("list".to_string(), "table".to_string()));
    }

    /// Every settings.toml 0.7.2 wrote holds the cards, its Save writing every field: a cards
    /// nobody picked loads as the table, and is written back so; one picked with Browse's switch
    /// stays, as does a list.
    #[test]
    fn a_cards_nobody_picked_loads_as_the_table_and_a_pick_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        // As 0.7.2 wrote it: every field, browse_view = "cards", no browse_view_picked.
        let from_0_7_2 = "game_dir = 'C:\\games\\AzerothSurvivors'\npc_name = 'PC 1'\nstart_with_windows = false\ndesktop_shortcut = true\nnotifications = true\npause_when_away = false\ntheme = 'system'\n\n[sync]\nenabled = true\nfolder = '\\\\homenas\\games\\Saves'\n\n[addons]\nupdate_by_themselves = true\nshared = true\nforge = true\nbrowse_view = 'cards'\n\n[game]\nshared = true\n";
        fs::write(&path, from_0_7_2).unwrap();
        let updated = Settings::load(&path).unwrap().unwrap();
        assert_eq!((updated.addons.browse_view.as_str(), updated.addons.browse_view_picked), ("table", false));
        assert_eq!((updated.pc_name.as_str(), updated.sync.enabled), ("PC 1", true), "the rest stands");
        // Written back with the next save, and read so again.
        updated.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("browse_view = \"table\"") && text.contains("browse_view_picked = false"), "{text}");
        assert_eq!(Settings::load(&path).unwrap().unwrap().addons.browse_view, "table");

        // Picked with the switch: the cards stay, from now on.
        fs::write(&path, "[addons]\nbrowse_view = 'cards'\nbrowse_view_picked = true\n").unwrap();
        let picked = Settings::load(&path).unwrap().unwrap();
        assert_eq!((picked.addons.browse_view.as_str(), picked.addons.browse_view_picked), ("cards", true));

        // A list from 0.7.2, which only the switch could have set: it stays, picked key or not.
        fs::write(&path, "[addons]\nbrowse_view = 'list'\n").unwrap();
        assert_eq!(Settings::load(&path).unwrap().unwrap().addons.browse_view, "list");

        // The switch: the view it names, picked; the cards survive a save and a load.
        let mut settings = updated.clone();
        settings.addons.pick_view("cards");
        assert_eq!((settings.addons.browse_view.as_str(), settings.addons.browse_view_picked), ("cards", true));
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), Some(settings.clone()));
        // A name it doesn't know picks the default, the table.
        settings.addons.pick_view("gallery");
        assert_eq!((settings.addons.browse_view.as_str(), settings.addons.browse_view_picked), ("table", true));
        // A new settings.toml has picked nothing.
        assert!(!Settings::default().addons.browse_view_picked);
    }

    const GAME: &str = r"C:\games\AzerothSurvivors";
    const SHARE: &str = r"\\server\share\Saves";

    /// The app's answers as a table: one risky folder, one that doesn't answer, a name another
    /// PC uses, the folders that hold the game.
    struct Table;

    impl Checks for Table {
        fn folder_problem(&self, folder: &Path, game_dir: Option<&Path>) -> Option<String> {
            let folder = folder.to_string_lossy();
            if folder == r"\\server\share\Saves\inside" {
                return Some("the network folder is inside the game's progress folder".into());
            }
            game_dir.filter(|dir| dir.to_string_lossy().starts_with(&*folder)).map(|_| "the game's folder is inside the network folder".into())
        }
        fn folder_answers(&self, folder: &Path) -> bool {
            folder.to_string_lossy() != r"\\down\share"
        }
        fn name_taken(&self, name: &str) -> Option<String> {
            name.eq_ignore_ascii_case("Desktop").then(|| "Desktop".to_string())
        }
        fn game_found(&self, dir: &Path) -> bool {
            [GAME, r"D:\Games\AzerothSurvivors", r"\\server\share\Saves\game"].contains(&&*dir.to_string_lossy())
        }
    }

    fn synced() -> Settings {
        Settings {
            game_dir: Some(PathBuf::from(GAME)),
            pc_name: "Laptop".into(),
            sync: SyncSettings { enabled: true, folder: Some(PathBuf::from(SHARE)) },
            ..Settings::default()
        }
    }

    /// Each change is checked alone against the settings in force: a refused folder, name or game
    /// folder keeps the saved value, and the switches changed after it apply all the same; only
    /// the rows that applied light their "✓ Saved".
    #[test]
    fn a_refused_field_keeps_the_saved_value_and_never_holds_another_row_back() {
        let mut settings = synced();
        let changes = [
            ("folder", r"\\server\share\Saves\inside"),
            ("notifications", "false"),
            ("folder", r"\\down\share"),
            ("pc-name", "desktop"),
            ("theme", "light"),
            ("game-dir", r"C:\games\Azeroth"),
            ("pause-when-away", "true"),
            ("pc-name", "  Study PC "),
        ];
        let mut applied = Vec::new();
        let mut refused = Vec::new();
        for (key, value) in changes {
            let answer = change(&settings, key, value, &Table);
            applied.push(answer.applied.clone());
            refused.extend(answer.refused.clone());
            // What a refusal answers is the settings as they were.
            if answer.refused.is_some() {
                assert_eq!(answer.settings, settings, "{key} refused: nothing changes");
            }
            settings = answer.settings;
        }
        assert_eq!(
            applied,
            [None, Some("notifications".to_string()), None, None, Some("theme".to_string()), None, Some("pause-when-away".to_string()), Some("pc-name".to_string())]
        );
        assert_eq!(settings.sync.folder.as_deref(), Some(Path::new(SHARE)), "the saved folder stays in force");
        assert_eq!(settings.game_dir.as_deref(), Some(Path::new(GAME)), "the saved game folder stays in force");
        assert_eq!(settings.pc_name, "Study PC");
        assert!(!settings.notifications && settings.pause_when_away && settings.sync.enabled);
        assert_eq!(settings.theme, Theme::Light);
        let said: Vec<(&str, &str, &str)> = refused.iter().map(|r| (r.key.as_str(), r.typed.as_str(), r.why.as_str())).collect();
        assert_eq!(
            said,
            [
                ("folder", r"\\server\share\Saves\inside", "Can't be used: the network folder is inside the game's progress folder"),
                ("folder", r"\\down\share", "It doesn't answer: check the path, and that the PC or the drive that shares it is on"),
                ("pc-name", "desktop", "Another PC syncing here is already called Desktop; give this one a name of its own"),
                ("game-dir", r"C:\games\Azeroth", "The game isn't in this folder"),
            ]
        );
    }

    /// The game's folder must hold the game and stay apart from the network folder; emptied, it
    /// is "Not set".
    #[test]
    fn the_game_folder_must_hold_the_game_and_may_be_emptied() {
        let saved = synced();
        assert_eq!(apply(&saved, "game-dir", r"D:\Games\AzerothSurvivors", &Table).unwrap().game_dir.as_deref(), Some(Path::new(r"D:\Games\AzerothSurvivors")));
        assert_eq!(apply(&saved, "game-dir", "  ", &Table).unwrap().game_dir, None);
        let typo = apply(&saved, "game-dir", r"C:\games\AzerothSurvivor", &Table).unwrap_err();
        assert_eq!((typo.key.as_str(), typo.why.as_str()), ("game-dir", "The game isn't in this folder"));
        let around = apply(&saved, "game-dir", r"\\server\share\Saves\game", &Table).unwrap_err();
        assert_eq!(around.why, "It can't go with the network folder: the game's folder is inside the network folder");
        // With sync off, the network folder doesn't count.
        let off = Settings { sync: SyncSettings { enabled: false, ..saved.sync.clone() }, ..saved };
        assert!(apply(&off, "game-dir", r"\\server\share\Saves\game", &Table).is_ok());
    }

    /// The network folder is checked when it is typed, and again when sync is turned on; sync
    /// turns on without a folder too (it then waits for one). A name is checked against the other
    /// PCs only while sync is on.
    #[test]
    fn the_network_folder_is_checked_and_sync_turns_on_only_with_one_that_answers() {
        let off = Settings { sync: SyncSettings { enabled: false, folder: Some(PathBuf::from(r"\\down\share")) }, ..synced() };
        let refused = apply(&off, "sync", "true", &Table).unwrap_err();
        assert_eq!((refused.key.as_str(), refused.typed.as_str()), ("sync", "true"));
        assert!(refused.why.starts_with("It doesn't answer"), "{}", refused.why);
        assert!(!change(&off, "sync", "true", &Table).settings.sync.enabled, "sync stays off");
        let none = Settings { sync: SyncSettings { enabled: false, folder: None }, ..synced() };
        assert!(apply(&none, "sync", "true", &Table).unwrap().sync.enabled, "on, waiting for a folder");
        assert!(!apply(&off, "sync", "false", &Table).unwrap().sync.enabled);
        assert_eq!(apply(&synced(), "folder", "", &Table).unwrap().sync.folder, None, "emptied: cleared");
        assert_eq!(apply(&synced(), "folder", r" \\server\share\Other ", &Table).unwrap().sync.folder.as_deref(), Some(Path::new(r"\\server\share\Other")));
        // The name: refused while sync is on, free while it is off; never empty.
        assert!(apply(&synced(), "pc-name", "Desktop", &Table).is_err());
        assert_eq!(apply(&none, "pc-name", "Desktop", &Table).unwrap().pc_name, "Desktop");
        assert_eq!(apply(&none, "pc-name", " ", &Table).unwrap_err().why, "Give this PC a name");
    }

    /// A value that didn't change applies nothing and lights nothing; values the rows never send
    /// are refused.
    #[test]
    fn a_value_that_didnt_change_lights_nothing_and_strange_values_are_refused() {
        let saved = synced();
        assert_eq!(change(&saved, "theme", "dark", &Table), Answer { settings: saved.clone(), applied: None, refused: None });
        assert_eq!(change(&saved, "notifications", "true", &Table).applied, None);
        assert_eq!(change(&saved, "theme", "system", &Table).applied.as_deref(), Some("theme"));
        assert!(apply(&saved, "notifications", "maybe", &Table).is_err());
        assert!(apply(&saved, "theme", "blue", &Table).is_err());
        assert_eq!(apply(&saved, "colour", "red", &Table).unwrap_err().key, "colour");
        // Every switch reaches its own setting: one of its two values changes the settings.
        for key in ["start-with-windows", "desktop-shortcut", "notifications", "pause-when-away", "addons-auto", "addons-shared", "addons-forge", "game-shared", "sync"] {
            let (on, off) = (apply(&saved, key, "true", &Table).unwrap(), apply(&saved, key, "false", &Table).unwrap());
            assert_ne!(on, off, "{key} changes something");
        }
    }

    /// Restore defaults resets the switches and the theme, except sync and sharing; the folders,
    /// the name, Browse's view and the window stay.
    #[test]
    fn restore_defaults_keeps_sync_sharing_the_folders_and_the_name() {
        let mut saved = synced();
        saved.start_with_windows = true;
        saved.desktop_shortcut = false;
        saved.notifications = false;
        saved.pause_when_away = true;
        saved.theme = Theme::Light;
        saved.addons.update_by_themselves = false;
        saved.addons.forge = false;
        saved.addons.shared = false;
        saved.game.shared = false;
        saved.addons.pick_view("list");
        saved.window.maximised = true;
        let restored = restore_defaults(&saved);
        let fresh = Settings::default();
        assert_eq!(
            (restored.start_with_windows, restored.desktop_shortcut, restored.notifications, restored.pause_when_away, restored.theme),
            (fresh.start_with_windows, fresh.desktop_shortcut, fresh.notifications, fresh.pause_when_away, fresh.theme)
        );
        assert_eq!((restored.addons.update_by_themselves, restored.addons.forge), (true, true));
        assert_eq!((restored.addons.shared, restored.game.shared, restored.sync.clone()), (false, false, saved.sync.clone()), "sync and sharing stay");
        assert_eq!((restored.game_dir.clone(), restored.pc_name.clone()), (saved.game_dir.clone(), saved.pc_name.clone()));
        assert_eq!((restored.addons.browse_view.as_str(), restored.window.maximised), ("list", true));
    }

    #[test]
    fn unknown_or_missing_keys_do_not_fail() {
        let settings: Settings = toml::from_str("pc_name = \"Laptop\"\nfuture_key = 1\n").unwrap();
        assert_eq!(settings.pc_name, "Laptop");
        assert!(!settings.sync.enabled);
    }
}
