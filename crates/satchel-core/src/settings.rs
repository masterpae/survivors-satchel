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

    #[test]
    fn unknown_or_missing_keys_do_not_fail() {
        let settings: Settings = toml::from_str("pc_name = \"Laptop\"\nfuture_key = 1\n").unwrap();
        assert_eq!(settings.pc_name, "Laptop");
        assert!(!settings.sync.enabled);
    }
}
