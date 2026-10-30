//! The game's own settings on every PC: interface options, key bindings, macros, chat, camera,
//! window positions, and the preferences among config.toml's values, as one settings set that
//! travels like an addon's (`addons::share`): sent when the game closes, brought in before it
//! starts, after a backup. Graphics, the window and the UI scale stay on each PC.
//!
//! The game writes these at the end of every run (and a second after some changes), and reads
//! them only when it starts, so they are read and written here with the game closed.

use crate::addons::share::{self, Bundle, SettingsStep, join};
use crate::{fsx, names};
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

/// The set's id among the shared list's settings versions. No addon's id starts with `_` (a
/// catalog id, `owner/repo`, `file:…`), so it can't meet one.
pub const ID: &str = "_game";
/// How the Satchel speaks of it.
pub const NAME: &str = "The game's settings";

/// Files directly in benilla-config that travel.
const TOP: [&str; 2] = ["saved-variables.lua", "once-buff-durations-on"];
/// Folders whose `.txt` files travel, each one per account or per hero.
const FOLDERS: [&str; 5] = ["bindings", "macros", "chat", "camera", "layout"];
/// The game's run-once marker: written when it comes, never removed, or the game would switch
/// Buff Durations on once more.
const KEPT: &str = "once-buff-durations-on";
/// config.toml's preferences, carried in the set as a file of their own.
pub const PREFS: &str = "config-preferences.toml";
/// The game's own HUD parts, each one "saved" (the game's addon doc); addons hide some of them to
/// draw their own.
const HUD: [&str; 10] = ["hudClock", "hudBosses", "hudTally", "hudBuild", "hudBanners", "hudLoot", "hudMeter", "hudLevels", "hudFlag", "hudRevive"];

/// A config.toml value that is the player's preference rather than this PC's: the HUD parts,
/// stored level-ups, out-of-date addons, combat text and the combat log, the camera's reach,
/// Discord. Graphics, the window, the UI scale, the realm and the account stay on each PC.
pub fn is_preference(key: &str) -> bool {
    HUD.contains(&key)
        || key.starts_with("CombatLog")
        || matches!(key, "storeLevels" | "checkAddonVersion" | "CombatDamage" | "PetMeleeDamage" | "PetSpellDamage" | "cameraDistanceMaxFactor" | "discordPresence")
}

/// Whether `path` (with `/`) is one of the set's files: what `read` takes, and all a settings zip
/// of it may hold.
pub fn is_settings_path(path: &str) -> bool {
    let plain = |p: &str| !p.is_empty() && p != "." && p != ".." && !p.contains(['\\', ':']) && names::safe(p) == p;
    match path.split('/').collect::<Vec<_>>().as_slice() {
        [file] => *file == PREFS || TOP.iter().any(|t| t.eq_ignore_ascii_case(file)),
        [folder, file] => {
            FOLDERS.iter().any(|f| f.eq_ignore_ascii_case(folder))
                && plain(file)
                && file.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && ext.eq_ignore_ascii_case("txt"))
        }
        _ => false,
    }
}

/// The game's settings decide as an addon's do (`share::settings_step`), with two cautions more,
/// because a game just installed or reset writes the newest files of all: on this PC's first look
/// the NAS's settings come in whatever their age (this PC's own are kept as a version first), so a
/// PC joining never spreads its own; and a set that lost files the NAS's newest has (a game
/// reinstalled, its folder reset) is asked about rather than sent. `local_files`: this PC's set's
/// paths, lower-case; `nas_files`: the newest version's.
pub fn step(local: Option<&str>, base: Option<&str>, nas: Option<(&str, u64)>, local_files: &BTreeSet<String>, nas_files: &[String]) -> SettingsStep {
    // `here_newer` false: a first look takes the NAS's, this PC's kept as a version.
    let step = share::settings_step(local, base, nas, || false);
    let Some((_, n)) = nas else { return step };
    match step {
        SettingsStep::Send if nas_files.iter().any(|f| !local_files.contains(&f.to_lowercase())) => SettingsStep::Ask { nas: n },
        step => step,
    }
}

/// The hero a per-hero file is for (`layout/Elwynn_Forest-Hero.txt` → `elwynn_forest-hero`);
/// `None` for an account file or one directly in benilla-config.
fn hero_of(rel: &str) -> Option<String> {
    let (folder, file) = rel.split_once('/')?;
    let stem = file.rsplit_once('.')?.0.to_lowercase();
    (FOLDERS.iter().any(|f| f.eq_ignore_ascii_case(folder)) && stem != "account").then_some(stem)
}

fn odd(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.into())
}

/// The set as it is in `config` (the game's benilla-config). An error when config.toml isn't in
/// the shape the game writes: its preferences can't be told apart, so nothing travels until the
/// game writes it again.
pub fn read(config: &Path) -> io::Result<Bundle> {
    let mut bundle = Bundle::default();
    for name in TOP {
        let path = config.join(name);
        if path.is_file() {
            bundle.take(&path, name.to_string())?;
        }
    }
    for folder in FOLDERS {
        let entries = match fs::read_dir(config.join(folder)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for entry in entries.filter_map(Result::ok) {
            let rel = format!("{folder}/{}", entry.file_name().to_string_lossy());
            if entry.file_type()?.is_file() && is_settings_path(&rel) {
                bundle.take(&entry.path(), rel)?;
            }
        }
    }
    let toml = config.join("config.toml");
    if let Some(bytes) = fsx::read_optional(&toml)? {
        let text = String::from_utf8(bytes).map_err(|_| odd("config.toml isn't UTF-8"))?;
        if let Some(prefs) = preferences(&text).map_err(odd)? {
            let modified = fs::metadata(&toml).and_then(|m| m.modified()).ok().map(DateTime::<Utc>::from);
            bundle.insert(PREFS.into(), prefs.into_bytes(), modified);
        }
    }
    Ok(bundle)
}

/// Puts `set` in place of this PC's game settings (the game closed): this PC's files and its
/// config.toml go to `backup` first; then the set's files are written and its preferences merged
/// into config.toml; then the files the set doesn't have are removed (they are in the backup).
/// Kept: the run-once marker, and the files of heroes the set knows nothing of (heroes played
/// only here). When a write fails half way, this PC's files are put back as they were.
pub fn place(set: &Bundle, config: &Path, backup: &Path) -> io::Result<()> {
    let current = read(config)?;
    let toml = config.join("config.toml");
    let old_toml = fsx::read_optional(&toml)?;
    // What could fail goes first, before a file is touched.
    let text = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| odd("config.toml isn't UTF-8"));
    let old_text = old_toml.as_deref().map(text).transpose()?;
    let prefs = set.files.get(PREFS).map(|b| text(b)).transpose()?;
    let merged = merge_preferences(old_text.as_deref(), prefs.as_deref()).map_err(odd)?;
    for (rel, bytes) in current.files.iter().filter(|(rel, _)| *rel != PREFS) {
        fsx::write_atomic(&join(backup, rel), bytes)?;
    }
    if let Some(bytes) = &old_toml {
        fsx::write_atomic(&backup.join("config.toml"), bytes)?;
    }
    let incoming: BTreeSet<String> = set.files.keys().map(|k| k.to_lowercase()).collect();
    let heroes: BTreeSet<String> = set.files.keys().filter_map(|k| hero_of(k)).collect();
    let gone: Vec<&String> = current
        .files
        .keys()
        .filter(|rel| *rel != PREFS && !rel.eq_ignore_ascii_case(KEPT) && !incoming.contains(&rel.to_lowercase()))
        .filter(|rel| hero_of(rel).is_none_or(|hero| heroes.contains(&hero)))
        .collect();
    let applied = (|| -> io::Result<()> {
        for (rel, bytes) in set.files.iter().filter(|(rel, _)| *rel != PREFS) {
            fsx::write_atomic(&join(config, rel), bytes)?;
        }
        if let Some(merged) = &merged {
            fsx::write_atomic(&toml, merged.as_bytes())?;
        }
        for rel in &gone {
            fsx::remove_if_present(&join(config, rel))?;
        }
        Ok(())
    })();
    if let Err(e) = applied {
        // Half of one set over half of another is worse than either: this PC's go back.
        let had: BTreeSet<String> = current.files.keys().map(|k| k.to_lowercase()).collect();
        for (rel, bytes) in current.files.iter().filter(|(rel, _)| *rel != PREFS) {
            let _ = fsx::write_atomic(&join(config, rel), bytes);
        }
        for rel in set.files.keys().filter(|rel| *rel != PREFS && !had.contains(&rel.to_lowercase())) {
            let _ = fsx::remove_if_present(&join(config, rel));
        }
        match &old_toml {
            Some(bytes) => {
                let _ = fsx::write_atomic(&toml, bytes);
            }
            None if merged.is_some() => {
                let _ = fsx::remove_if_present(&toml);
            }
            None => {}
        }
        return Err(e);
    }
    Ok(())
}

// config.toml ----------------------------------------------------------------------------------

/// One line of config.toml as the game writes it.
enum Line<'a> {
    /// A comment, a blank line, or before `[cvars]`.
    Other(&'a str),
    Table,
    /// `Name = "value"` in `[cvars]`.
    Value { key: &'a str, value: &'a str, line: &'a str },
}

/// `Name = "value"`, the only shape of the game's values; `None` for anything else.
fn value_of(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    let (key, value) = (key.trim(), value.trim());
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let key_ok = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (key_ok && !inner.contains(['"', '\\'])).then_some((key, value))
}

/// config.toml's lines: comments and blank lines, `[cvars]` once, then only values (and blank
/// lines). Anything else is a file the game didn't write that way.
fn lines_of(text: &str) -> Result<Vec<Line<'_>>, String> {
    let mut lines = Vec::new();
    let mut in_cvars = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "[cvars]" && !in_cvars {
            in_cvars = true;
            lines.push(Line::Table);
        } else if trimmed.is_empty() || (!in_cvars && trimmed.starts_with('#')) {
            lines.push(Line::Other(line));
        } else if let (true, Some((key, value))) = (in_cvars, value_of(trimmed)) {
            lines.push(Line::Value { key, value, line });
        } else {
            return Err(format!("config.toml has a line the game doesn't write: {trimmed:?}"));
        }
    }
    Ok(lines)
}

/// The preferences in config.toml, one `Name = "value"` line each, sorted by name; `None` when it
/// holds none (all at the game's defaults).
pub fn preferences(config: &str) -> Result<Option<String>, String> {
    let text = config.strip_prefix('\u{feff}').unwrap_or(config);
    let mut prefs: Vec<(&str, &str)> = lines_of(text)?
        .into_iter()
        .filter_map(|l| match l {
            Line::Value { key, value, .. } if is_preference(key) => Some((key, value)),
            _ => None,
        })
        .collect();
    prefs.sort();
    Ok((!prefs.is_empty()).then(|| prefs.iter().map(|(k, v)| format!("{k} = {v}\n")).collect()))
}

/// config.toml with its preferences replaced by `prefs` (`None`: none, the game's defaults);
/// every other line stays as it is, and the values stay sorted by name, as the game writes them.
/// `Ok(None)`: nothing changes. `Err`: config.toml (or `prefs`) isn't in the shape the game writes.
pub fn merge_preferences(config: Option<&str>, prefs: Option<&str>) -> Result<Option<String>, String> {
    let mut incoming: Vec<(String, String)> = Vec::new();
    for line in prefs.unwrap_or_default().lines().filter(|l| !l.trim().is_empty()) {
        match value_of(line.trim()) {
            Some((key, value)) if is_preference(key) => incoming.push((key.into(), value.into())),
            _ => return Err(format!("the preferences that came hold {line:?}")),
        }
    }
    let Some(config) = config else {
        if incoming.is_empty() {
            return Ok(None);
        }
        incoming.sort();
        let values: String = incoming.iter().map(|(k, v)| format!("{k} = {v}\n")).collect();
        return Ok(Some(format!("[cvars]\n{values}")));
    };
    let (bom, text) = match config.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", config),
    };
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut head: Vec<&str> = Vec::new();
    let mut values: Vec<(String, String)> = Vec::new();
    let mut has_table = false;
    for line in lines_of(text)? {
        match line {
            Line::Other(l) if !has_table => head.push(l),
            Line::Other(_) => {}
            Line::Table => has_table = true,
            Line::Value { key, line, .. } if !is_preference(key) => values.push((key.into(), line.trim().into())),
            Line::Value { .. } => {}
        }
    }
    values.extend(incoming.into_iter().map(|(k, v)| {
        let line = format!("{k} = {v}");
        (k, line)
    }));
    values.sort();
    let mut out = String::from(bom);
    for line in head {
        out.push_str(line);
        out.push_str(newline);
    }
    if has_table || !values.is_empty() {
        out.push_str("[cvars]");
        out.push_str(newline);
    }
    for (_, line) in &values {
        out.push_str(line);
        out.push_str(newline);
    }
    Ok((out != config).then_some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of a real config.toml (values from a player's PC, 30 Oct 2026).
    const CONFIG: &str = "# benilla local config \u{2014} CVar values that moved off their defaults.\n# Managed by the client; hand edits are read on next launch and preserved on save.\n[cvars]\nCombatDamage = \"0\"\nCombatLogRangeParty = \"200\"\nSmallCull = \"0.070000\"\ndiscordPresence = \"0\"\ngamma = \"0.700000\"\ngxVSync = \"0\"\nhudMeter = \"0\"\nhudTally = \"0\"\nrealmName = \"Elwynn Forest\"\nstoreLevels = \"1\"\n";

    #[test]
    fn preferences_are_told_from_this_pcs_values() {
        assert!(is_preference("hudMeter") && is_preference("CombatLogRangeParty") && is_preference("storeLevels"));
        for machine in ["gxVSync", "gamma", "uiScale", "gxResolution", "realmName", "accountName", "SmallCull", "gameTip"] {
            assert!(!is_preference(machine), "{machine}");
        }
        assert_eq!(
            preferences(CONFIG).unwrap().unwrap(),
            "CombatDamage = \"0\"\nCombatLogRangeParty = \"200\"\ndiscordPresence = \"0\"\nhudMeter = \"0\"\nhudTally = \"0\"\nstoreLevels = \"1\"\n"
        );
        assert_eq!(preferences("[cvars]\ngamma = \"1\"\n").unwrap(), None);
        assert_eq!(preferences("").unwrap(), None);
    }

    #[test]
    fn preferences_merge_into_this_pcs_config() {
        // Another PC shows the meter and hides the clock; this PC's graphics stay.
        let theirs = "hudClock = \"0\"\nhudMeter = \"1\"\n";
        let merged = merge_preferences(Some(CONFIG), Some(theirs)).unwrap().unwrap();
        assert_eq!(
            merged,
            "# benilla local config \u{2014} CVar values that moved off their defaults.\n# Managed by the client; hand edits are read on next launch and preserved on save.\n[cvars]\nSmallCull = \"0.070000\"\ngamma = \"0.700000\"\ngxVSync = \"0\"\nhudClock = \"0\"\nhudMeter = \"1\"\nrealmName = \"Elwynn Forest\"\n"
        );
        assert_eq!(preferences(&merged).unwrap().as_deref(), Some(theirs));
        // The same preferences: nothing to write.
        let mine = preferences(CONFIG).unwrap();
        assert_eq!(merge_preferences(Some(CONFIG), mine.as_deref()).unwrap(), None);
        // None at all: back to the game's defaults, the rest kept.
        let defaults = merge_preferences(Some(CONFIG), None).unwrap().unwrap();
        assert_eq!(preferences(&defaults).unwrap(), None);
        assert!(defaults.contains("gamma = \"0.700000\"") && defaults.contains("realmName"));
        // No config.toml yet: one is written with only the preferences.
        assert_eq!(merge_preferences(None, Some(theirs)).unwrap().unwrap(), format!("[cvars]\n{theirs}"));
        assert_eq!(merge_preferences(None, None).unwrap(), None);
        // Windows line ends stay.
        let crlf = CONFIG.replace('\n', "\r\n");
        assert!(merge_preferences(Some(&crlf), Some(theirs)).unwrap().unwrap().contains("hudClock = \"0\"\r\n"));
    }

    #[test]
    fn a_config_in_another_shape_is_left_alone() {
        for odd in ["[cvars]\nhudMeter = 0\n", "[other]\nx = \"1\"\n", "hudMeter = \"0\"\n", "[cvars]\n# note\nhudMeter = \"0\"\n", "[cvars]\nname = \"a\\\"b\"\n"] {
            assert!(preferences(odd).is_err(), "{odd:?}");
            assert!(merge_preferences(Some(odd), Some("hudMeter = \"1\"\n")).is_err(), "{odd:?}");
        }
        // Preferences that aren't preferences are refused too.
        assert!(merge_preferences(Some(CONFIG), Some("gxVSync = \"1\"\n")).is_err());
    }

    #[test]
    fn the_paths_a_set_may_hold() {
        for ok in ["saved-variables.lua", "once-buff-durations-on", PREFS, "bindings/account.txt", "layout/Elwynn_Forest-Hero.txt", "Chat/Realm-Hero.TXT"] {
            assert!(is_settings_path(ok), "{ok}");
        }
        for bad in ["config.toml", "cache/Elwynn_Forest.tsv", "saved/LuckyOdds.lua", "layout/x.tmp", "layout/x.txt.bak", "bindings/x.wtf", "layout/a/b.txt", "../layout/x.txt", "layout/.txt", "layout/..", "macros/C:x.txt", "Logs/log.txt"] {
            assert!(!is_settings_path(bad), "{bad}");
        }
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = join(dir, rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn a_new_or_reset_game_never_spreads_its_own() {
        let files = |names: &[&str]| -> BTreeSet<String> { names.iter().map(|n| n.to_lowercase()).collect() };
        let all = vec!["bindings/account.txt".to_string(), "layout/R-Hero.txt".into(), PREFS.into()];
        let mine = files(&["bindings/account.txt", "layout/R-Hero.txt", PREFS]);
        // The first PC seeds the NAS.
        assert_eq!(step(Some("a"), None, None, &mine, &[]), SettingsStep::Send);
        // A first look with other settings than the NAS's: the NAS's come, whichever is newer, and
        // this PC's are kept as a version.
        assert_eq!(step(Some("a"), None, Some(("b", 4)), &mine, &all), SettingsStep::BringIn { n: 4, keep_mine: true });
        // A first look with the same, or with nothing here yet: no question.
        assert_eq!(step(Some("b"), None, Some(("b", 4)), &mine, &all), SettingsStep::InStep { adopt: Some(4) });
        assert_eq!(step(None, None, Some(("b", 4)), &files(&[]), &all), SettingsStep::BringIn { n: 4, keep_mine: false });
        // Changed here, the NAS as agreed: sent, unless the set lost files the NAS's has.
        assert_eq!(step(Some("c"), Some("b"), Some(("b", 4)), &mine, &all), SettingsStep::Send);
        assert_eq!(step(Some("c"), Some("b"), Some(("b", 4)), &files(&["bindings/account.txt"]), &all), SettingsStep::Ask { nas: 4 });
        // A version from before the files were listed: sent as before.
        assert_eq!(step(Some("c"), Some("b"), Some(("b", 4)), &files(&[]), &[]), SettingsStep::Send);
        // Changed elsewhere only: it comes. Both changed: asked.
        assert_eq!(step(Some("b"), Some("b"), Some(("d", 5)), &mine, &all), SettingsStep::BringIn { n: 5, keep_mine: false });
        assert_eq!(step(Some("c"), Some("b"), Some(("d", 5)), &mine, &all), SettingsStep::Ask { nas: 5 });
    }

    #[test]
    fn read_takes_the_settings_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path();
        for (rel, text) in [
            ("saved-variables.lua", "SHOW_BUFF_DURATIONS = 1"),
            ("once-buff-durations-on", "1\n"),
            ("bindings/account.txt", "bind A"),
            ("layout/Realm-Hero.txt", "Frame: AZSSpellMeterFrame"),
            ("chat/Realm-Hero.txt", "chat"),
            ("camera/Realm-Hero.txt", "cameraDistance 5"),
            ("layout/Realm-Hero.tmp", "half"),
            ("cache/Realm.tsv", "names"),
            ("saved/LuckyOdds.lua", "addon"),
            ("AddOns/account.txt", "LuckyOdds: enabled"),
            ("config.toml", CONFIG),
        ] {
            write(config, rel, text);
        }
        let set = read(config).unwrap();
        assert_eq!(
            set.files.keys().map(String::as_str).collect::<Vec<_>>(),
            ["bindings/account.txt", "camera/Realm-Hero.txt", "chat/Realm-Hero.txt", PREFS, "layout/Realm-Hero.txt", "once-buff-durations-on", "saved-variables.lua"]
        );
        assert!(set.newest.is_some());
        // Graphics changed here: the set is the same.
        let print = set.fingerprint().unwrap();
        write(config, "config.toml", &CONFIG.replace("gamma = \"0.700000\"", "gamma = \"0.900000\""));
        assert_eq!(read(config).unwrap().fingerprint().unwrap(), print);
        // A preference changed: it isn't.
        write(config, "config.toml", &CONFIG.replace("hudTally = \"0\"", "hudTally = \"1\""));
        assert_ne!(read(config).unwrap().fingerprint().unwrap(), print);
        // config.toml in a shape the game doesn't write: nothing is read.
        write(config, "config.toml", "[cvars]\nhudMeter = 0\n");
        assert!(read(config).is_err());
        // Through a zip and back, unchanged.
        let back = Bundle::from_zip_for(&set.to_zip().unwrap(), ID).unwrap();
        assert_eq!(back.files, set.files);
    }

    #[test]
    fn placed_on_another_pc() {
        let dir = tempfile::tempdir().unwrap();
        let (mine, theirs, backup) = (dir.path().join("mine"), dir.path().join("theirs"), dir.path().join("backup"));
        write(&theirs, "saved-variables.lua", "AUTO_QUEST_WATCH = 0");
        write(&theirs, "layout/Realm-Hero.txt", "Frame: LuckyOddsFrame\nX: 8");
        write(&theirs, "bindings/account.txt", "bind B");
        write(&theirs, "config.toml", "[cvars]\nhudMeter = \"1\"\n");
        let set = read(&theirs).unwrap();

        write(&mine, "saved-variables.lua", "AUTO_QUEST_WATCH = 1");
        write(&mine, "once-buff-durations-on", "1\n");
        write(&mine, "layout/Realm-Hero.txt", "Frame: LuckyOddsFrame\nX: 400");
        write(&mine, "bindings/Realm-Hero.txt", "bind C");
        write(&mine, "chat/Realm-Hero.txt", "chat");
        // A hero played only here: the other PC's set knows nothing of it.
        write(&mine, "layout/Realm-Alt.txt", "Frame: AltFrame");
        write(&mine, "saved/LuckyOdds.lua", "addon");
        write(&mine, "config.toml", CONFIG);
        place(&set, &mine, &backup).unwrap();

        // The set's files are here, its preferences in config.toml, this PC's graphics kept.
        let mut expected = set.files.clone();
        expected.insert(KEPT.into(), b"1\n".to_vec());
        expected.insert("layout/Realm-Alt.txt".into(), b"Frame: AltFrame".to_vec());
        assert_eq!(read(&mine).unwrap().files, expected);
        let config = fs::read_to_string(mine.join("config.toml")).unwrap();
        assert!(config.contains("hudMeter = \"1\"") && !config.contains("hudTally") && config.contains("gamma = \"0.700000\""));
        // What the set doesn't have is gone (into the backup), the run-once marker and the
        // addons' own settings excepted.
        assert!(!mine.join("bindings/Realm-Hero.txt").exists() && !mine.join("chat/Realm-Hero.txt").exists());
        assert!(mine.join("once-buff-durations-on").exists() && mine.join("saved/LuckyOdds.lua").exists());
        assert_eq!(fs::read_to_string(backup.join("bindings/Realm-Hero.txt")).unwrap(), "bind C");
        assert_eq!(fs::read_to_string(backup.join("layout/Realm-Hero.txt")).unwrap(), "Frame: LuckyOddsFrame\nX: 400");
        assert_eq!(fs::read_to_string(backup.join("config.toml")).unwrap(), CONFIG);

        // A config.toml in another shape stops it before anything is written.
        write(&mine, "config.toml", "[cvars]\nhudMeter = 1\n");
        write(&mine, "chat/Realm-Hero.txt", "chat again");
        assert!(place(&set, &mine, &dir.path().join("backup2")).is_err());
        assert!(mine.join("chat/Realm-Hero.txt").exists());
    }
}
