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
}

/// The window's colours: as Windows' apps are, or always light, or always dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
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
    /// switch was left. Absent, or anything else, reads as `table`, the default since 0.8.0. Not in
    /// the Settings tab.
    #[serde(default = "default_view", deserialize_with = "browse_view")]
    pub browse_view: String,
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
        AddonSettings { update_by_themselves: true, shared: true, forge: true, browse_view: default_view() }
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
            theme: Theme::System,
            sync: SyncSettings::default(),
            addons: AddonSettings::default(),
            game: GameSettings::default(),
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
    /// `None` when this PC has no settings yet (its first start).
    pub fn load(path: &Path) -> io::Result<Option<Settings>> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map(Some).map_err(|e| invalid(path, e)),
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

    #[test]
    fn settings_from_0_3_follow_windows_colours_and_share_the_game_settings() {
        let settings: Settings = toml::from_str("pc_name = 'PC 1'\n[sync]\nenabled = true\n[addons]\nshared = true\n").unwrap();
        assert_eq!(settings.theme, Theme::System);
        assert!(settings.game.shared);
        let dark = Settings { theme: Theme::Dark, ..settings };
        let text = toml::to_string_pretty(&dark).unwrap();
        assert!(text.contains("theme = \"dark\""), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), dark);
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
            let chosen = Settings { addons: AddonSettings { browse_view: view.into(), ..AddonSettings::default() }, ..Settings::default() };
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

    #[test]
    fn unknown_or_missing_keys_do_not_fail() {
        let settings: Settings = toml::from_str("pc_name = \"Laptop\"\nfuture_key = 1\n").unwrap();
        assert_eq!(settings.pc_name, "Laptop");
        assert!(!settings.sync.enabled);
    }
}
