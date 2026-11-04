//! Installing, updating, going back and removing, each all-or-nothing and only with the game
//! closed (the caller's part): either the new version is in place and checked, or the old one is
//! exactly as it was.
//!
//! An install: the zip's checksum is checked against the one its source published; the zip is
//! judged before anything is unpacked; two addons never share a folder; it is unpacked outside
//! AddOns; what it replaces (folders and the addon's settings) goes to the backups; then the swap.
//! If any step fails, the backups go back.

use super::package::{self, PackageError};
use super::store::{Badge, Gone, Kept, Record, Store};
use super::GameAddons;
use crate::paths::Local;
use crate::{fsx, log, names};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Earlier versions kept per addon, for going back.
const KEEP_EARLIER: usize = 3;

#[derive(Debug)]
pub enum AddonError {
    /// The file isn't the one its source published.
    Checksum { expected: String, got: String },
    Package(PackageError),
    /// Another addon already owns that folder.
    Clash { folder: String, owner: String },
    /// No addon with that id is installed.
    NotInstalled(String),
    /// No earlier version is kept for it.
    NothingEarlier(String),
    /// The earlier version asked for isn't kept any more.
    NotKept(String),
    /// The zip's addon folders aren't the ones its listing names.
    Folders { listed: Vec<String>, found: Vec<String> },
    Io(io::Error),
}

impl From<io::Error> for AddonError {
    fn from(e: io::Error) -> Self {
        AddonError::Io(e)
    }
}

impl From<PackageError> for AddonError {
    fn from(e: PackageError) -> Self {
        AddonError::Package(e)
    }
}

impl fmt::Display for AddonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddonError::Checksum { .. } => write!(f, "the file isn't the one its author published (its checksum differs), so it wasn't installed"),
            AddonError::Package(e) => write!(f, "{e}"),
            AddonError::Clash { folder, owner } => write!(f, "{owner} already uses the folder {folder}; two addons can't share one"),
            AddonError::NotInstalled(id) => write!(f, "{id} isn't installed"),
            AddonError::NothingEarlier(name) => write!(f, "no earlier version of {name} is kept"),
            AddonError::NotKept(name) => write!(f, "that earlier version of {name} isn't kept any more"),
            AddonError::Folders { listed, found } => {
                write!(f, "its zip holds {}, not the folders it's listed with ({}), so it wasn't installed", found.join(", "), listed.join(", "))
            }
            AddonError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AddonError {}

/// An addon zip on its way in, and what is known about it.
#[derive(Clone, Debug)]
pub struct Incoming {
    pub zip: Vec<u8>,
    /// The checksum its source published (GitHub, the catalog); `None` for an imported file.
    pub sha256: Option<String>,
    pub id: String,
    pub name: String,
    pub badge: Badge,
    pub repo: Option<String>,
    pub tag: Option<String>,
    /// The release file's name.
    pub file: Option<String>,
    pub version: Option<String>,
    /// When its release was published, when that's known.
    pub released: Option<DateTime<Utc>>,
    /// Where it was downloaded from; `None` for an imported file.
    pub url: Option<String>,
    /// Installed from SurvivorsForge alone: its slug there, which the record keeps.
    pub forge: Option<String>,
    /// The addon folders the zip must hold, exactly (names without case): SurvivorsForge lists
    /// every folder an addon installs, and a zip that holds others isn't its file. `None`: any.
    pub expected_folders: Option<Vec<String>>,
    /// Kept at this version once in ("Keep this version"): a version picked in Versions…, kept the
    /// way Go back keeps one, so nothing updates it by itself until the player lets it.
    pub pinned: bool,
}

/// What an install did.
#[derive(Clone, Debug, PartialEq)]
pub struct Done {
    pub record: Record,
    /// Files in the zip that weren't written (other types, or outside any addon folder).
    pub left_out: usize,
    /// AddOns already held exactly these files as this addon: nothing was changed.
    pub already_installed: bool,
}

/// Everything an install, a going back or a removal works with.
pub struct Addons<'a> {
    pub game: &'a GameAddons,
    pub local: &'a Local,
    pub store: &'a mut Store,
}

impl Addons<'_> {
    /// Installs or updates an addon. The game must be closed.
    pub fn install(&mut self, incoming: Incoming, now: DateTime<Utc>) -> Result<Done, AddonError> {
        let got = fsx::fingerprint(&incoming.zip);
        if let Some(expected) = &incoming.sha256
            && *expected != got
        {
            return Err(AddonError::Checksum { expected: expected.clone(), got });
        }
        let package = package::inspect(&incoming.zip)?;
        if let Some(listed) = &incoming.expected_folders {
            let found: Vec<String> = package.folders.iter().map(|f| f.name.clone()).collect();
            let names = |names: &[String]| names.iter().map(|n| n.to_ascii_lowercase()).collect::<BTreeSet<String>>();
            if names(listed) != names(&found) {
                return Err(AddonError::Folders { listed: listed.clone(), found });
            }
        }
        for folder in &package.folders {
            if let Some(owner) = self.store.owner_of(&folder.name)
                && owner.id != incoming.id
            {
                return Err(AddonError::Clash { folder: folder.name.clone(), owner: owner.name.clone() });
            }
        }
        // Exactly what this addon has in AddOns now: nothing to do, and no backup of the same files.
        let previous = self.store.get(&incoming.id).cloned();
        if let Some(previous) = &previous
            && self.holds(previous, &package::fingerprints(&incoming.zip, &package)?)
        {
            log!("addon {} {} is already installed, exactly", previous.name, previous.shown_version());
            return Ok(Done { record: previous.clone(), left_out: package.left_out.len(), already_installed: true });
        }

        let stamp = names::stamp(now);
        let stage = self.local.addon_staging().join(&stamp);
        fsx::remove_dir_if_present(&stage)?;
        let unpacked = package::unpack(&incoming.zip, &package, &stage);
        let prints = match unpacked {
            Ok(prints) => prints,
            Err(e) => {
                let _ = fsx::remove_dir_if_present(&stage);
                return Err(e.into());
            }
        };

        // What goes out: the folders the new version brings, and any the old version had that it
        // doesn't.
        let mut going: Vec<String> = package.folders.iter().map(|f| f.name.clone()).collect();
        for old in previous.iter().flat_map(|p| p.folders.keys()) {
            if !going.iter().any(|g| g.eq_ignore_ascii_case(old)) {
                going.push(old.clone());
            }
        }
        let backup = format!("{stamp} {}", names::safe(&incoming.id));
        let incoming_folders: Vec<String> = package.folders.iter().map(|f| f.name.clone()).collect();
        let swapped = self.swap(&going, &incoming_folders, &stage, &backup);
        let _ = fsx::remove_dir_if_present(&stage);
        let replaced = swapped?;

        let mut earlier = previous.as_ref().map(|p| p.earlier.clone()).unwrap_or_default();
        if !replaced.is_empty() {
            earlier.insert(
                0,
                Kept {
                    tag: previous.as_ref().and_then(|p| p.tag.clone()),
                    file: previous.as_ref().and_then(|p| p.file.clone()),
                    version: previous.as_ref().and_then(|p| p.version.clone()),
                    sha256: previous.as_ref().and_then(|p| p.sha256.clone()),
                    released: previous.as_ref().and_then(|p| p.released),
                    url: previous.as_ref().and_then(|p| p.url.clone()),
                    folders: previous.as_ref().map(|p| p.folders.clone()).unwrap_or_default(),
                    backup: backup.clone(),
                    kept: now,
                },
            );
        }
        self.prune(&mut earlier);
        let record = Record {
            id: incoming.id,
            name: incoming.name,
            badge: incoming.badge,
            repo: incoming.repo,
            tag: incoming.tag,
            file: incoming.file,
            version: incoming.version.or_else(|| package.folders.iter().find_map(|f| f.toc.version.clone())),
            sha256: Some(got),
            folders: prints,
            installed: now,
            released: incoming.released,
            pinned: incoming.pinned,
            // A first install asks first, unless it is from our list or SurvivorsForge's own (whose
            // verdict decides); later the record keeps what it had.
            ask: previous.as_ref().map_or(incoming.badge != Badge::Verified && incoming.forge.is_none(), |p| p.ask),
            url: incoming.url,
            local_only: previous.as_ref().is_some_and(|p| p.local_only),
            settings_per_pc: previous.as_ref().is_some_and(|p| p.settings_per_pc),
            earlier,
            // An update that doesn't say keeps where the addon came from.
            forge: incoming.forge.or_else(|| previous.as_ref().and_then(|p| p.forge.clone())),
        };
        let left_out = match package.left_out.len() {
            0 => "nothing left out".to_string(),
            1 => "1 file left out".into(),
            n => format!("{n} files left out"),
        };
        log!("addon {} {} installed ({left_out})", record.name, record.shown_version());
        // Installed again: it leaves the Removed list.
        self.store.removed.retain(|g| g.record.id != record.id);
        self.store.put(record.clone());
        self.store.save(&self.local.addon_store())?;
        Ok(Done { record, left_out: package.left_out.len(), already_installed: false })
    }

    /// Whether AddOns holds `record`'s folders, unchanged since it was installed, and they are
    /// exactly `prints` (folder names compare without case).
    fn holds(&self, record: &Record, prints: &BTreeMap<String, String>) -> bool {
        let lower = |m: &BTreeMap<String, String>| m.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect::<BTreeMap<_, _>>();
        lower(&record.folders) == lower(prints)
            && record.folders.iter().all(|(name, print)| {
                self.game.folder(name).and_then(|dir| super::tree::fingerprint_dir(&dir).ok()).as_ref() == Some(print)
            })
    }

    /// Moves the folders in `going` (those present) to the backup named `backup`, with their
    /// settings, then the staged `incoming` folders into AddOns. All or nothing. Returns the
    /// folders that were replaced.
    fn swap(&self, going: &[String], incoming: &[String], stage: &Path, backup: &str) -> Result<Vec<String>, AddonError> {
        let backup_dir = self.local.addon_backups().join(backup);
        let mut out: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut placed: Vec<PathBuf> = Vec::new();
        let result = (|| -> io::Result<()> {
            for name in going {
                let Some(existing) = self.game.folder(name) else { continue };
                if let Some(settings) = fsx::read_optional(&self.game.settings_file(name))? {
                    fsx::write_atomic(&backup_dir.join("saved").join(format!("{name}.lua")), &settings)?;
                }
                let kept = backup_dir.join(existing.file_name().unwrap_or_default());
                fsx::move_dir(&existing, &kept)?;
                out.push((existing, kept));
            }
            for name in incoming {
                let target = self.game.addons.join(name);
                fsx::move_dir(&stage.join(name), &target)?;
                placed.push(target);
            }
            Ok(())
        })();
        if let Err(e) = result {
            for target in placed.iter().rev() {
                let _ = fsx::remove_dir_if_present(target);
            }
            for (original, kept) in out.iter().rev() {
                if let Err(back) = fsx::move_dir(kept, original) {
                    log!("could not put {} back from the backups: {back}", original.display());
                }
            }
            return Err(e.into());
        }
        Ok(out.iter().filter_map(|(original, _)| original.file_name().map(|n| n.to_string_lossy().into_owned())).collect())
    }

    /// Keeps the newest `KEEP_EARLIER` earlier versions; older backups are deleted.
    fn prune(&self, earlier: &mut Vec<Kept>) {
        for old in earlier.drain(KEEP_EARLIER.min(earlier.len())..) {
            let _ = fsx::remove_dir_if_present(&self.local.addon_backups().join(&old.backup));
        }
    }

    /// Goes back to an earlier version kept, `target` (its backup's name, as `Kept::backup` has it;
    /// `None`: the newest kept), and keeps it ("Keep this version") until the player lifts that.
    /// The version it replaces becomes the newest kept one; the others stay as they were. Settings
    /// stay as they are. The game must be closed.
    pub fn go_back(&mut self, id: &str, target: Option<&str>, now: DateTime<Utc>) -> Result<Record, AddonError> {
        let current = self.store.get(id).cloned().ok_or_else(|| AddonError::NotInstalled(id.into()))?;
        let at = match target {
            None if current.earlier.is_empty() => return Err(AddonError::NothingEarlier(current.name)),
            None => 0,
            Some(backup) => current.earlier.iter().position(|k| k.backup == backup).ok_or_else(|| AddonError::NotKept(current.name.clone()))?,
        };
        let target = current.earlier[at].clone();
        let stage = self.local.addon_backups().join(&target.backup);
        let mut going: Vec<String> = current.folders.keys().cloned().collect();
        let coming: Vec<String> = fs::read_dir(&stage)?
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir() && e.file_name() != "saved")
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        for name in &coming {
            if !going.iter().any(|g| g.eq_ignore_ascii_case(name)) {
                going.push(name.clone());
            }
        }
        let backup = format!("{} {}", names::stamp(now), names::safe(id));
        self.swap(&going, &coming, &stage, &backup)?;
        // The settings stay as they are now (and a copy of them is in the new backup); the copy
        // taken back then goes with the rest of that backup.
        let _ = fsx::remove_dir_if_present(&stage);

        let mut earlier: Vec<Kept> = current.earlier.iter().enumerate().filter(|(i, _)| *i != at).map(|(_, kept)| kept.clone()).collect();
        earlier.insert(
            0,
            Kept {
                tag: current.tag.clone(),
                file: current.file.clone(),
                version: current.version.clone(),
                sha256: current.sha256.clone(),
                released: current.released,
                url: current.url.clone(),
                folders: current.folders.clone(),
                backup,
                kept: now,
            },
        );
        self.prune(&mut earlier);
        let record = Record {
            tag: target.tag,
            file: target.file,
            version: target.version,
            sha256: target.sha256,
            folders: if target.folders.is_empty() { self.prints(&coming)? } else { target.folders },
            installed: now,
            released: target.released,
            url: target.url,
            pinned: true,
            earlier,
            ..current
        };
        log!("addon {} went back to {}", record.name, record.shown_version());
        self.store.put(record.clone());
        self.store.save(&self.local.addon_store())?;
        Ok(record)
    }

    fn prints(&self, folders: &[String]) -> io::Result<BTreeMap<String, String>> {
        folders
            .iter()
            .map(|name| Ok((name.clone(), super::tree::fingerprint_dir(&self.game.addons.join(name))?)))
            .collect()
    }

    /// Removes an addon: its folders go to `recycle` (the Recycle Bin, in the app), its settings
    /// too unless they are kept for a later reinstall; a copy of the settings stays in the backups
    /// either way. The game must be closed.
    pub fn remove(
        &mut self,
        id: &str,
        keep_settings: bool,
        from: Option<&str>,
        now: DateTime<Utc>,
        mut recycle: impl FnMut(&Path) -> io::Result<()>,
    ) -> Result<Record, AddonError> {
        let record = self.store.get(id).cloned().ok_or_else(|| AddonError::NotInstalled(id.into()))?;
        let backup_name = format!("{} {} removed", names::stamp(now), names::safe(id));
        let backup = self.local.addon_backups().join(&backup_name);
        for name in record.folders.keys() {
            let settings = self.game.settings_file(name);
            if let Some(bytes) = fsx::read_optional(&settings)? {
                fsx::write_atomic(&backup.join("saved").join(format!("{name}.lua")), &bytes)?;
                if !keep_settings {
                    recycle(&settings)?;
                }
            }
            if let Some(folder) = self.game.folder(name) {
                recycle(&folder)?;
            }
        }
        for kept in &record.earlier {
            let _ = fsx::remove_dir_if_present(&self.local.addon_backups().join(&kept.backup));
        }
        let mut list = super::enabled::EnableList::read(&self.game.enable_list())?;
        let before = list.clone();
        for name in record.folders.keys() {
            list.forget(name);
        }
        if list != before {
            list.write(&self.game.enable_list())?;
        }
        log!("addon {} removed{}", record.name, if keep_settings { ", its settings kept" } else { "" });
        self.store.remove(id);
        // Its earlier versions' backups went above: the Removed list keeps only the release.
        self.store.gone(Gone {
            record: Record { earlier: Vec::new(), ..record.clone() },
            at: now,
            settings_kept: keep_settings,
            backup: backup_name,
            from: from.map(str::to_string),
        });
        self.store.save(&self.local.addon_store())?;
        Ok(record)
    }

    /// "Keep this version", or lifts it.
    pub fn pin(&mut self, id: &str, pinned: bool) -> Result<(), AddonError> {
        let record = self.store.get_mut(id).ok_or_else(|| AddonError::NotInstalled(id.into()))?;
        record.pinned = pinned;
        self.store.save(&self.local.addon_store())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::enabled::EnableList;
    use crate::addons::package::tests::zip_of;
    use chrono::TimeZone;

    struct Pc {
        _dir: tempfile::TempDir,
        game: GameAddons,
        local: Local,
        store: Store,
    }

    fn pc() -> Pc {
        let dir = tempfile::tempdir().unwrap();
        let game = GameAddons::of(&dir.path().join("game"), None);
        fs::create_dir_all(&game.addons).unwrap();
        let local = Local {
            save_dir: dir.path().join("Roaming").join("wenilla-rogue"),
            app_dir: dir.path().join("Roaming").join("SurvivorsSatchel"),
            local_dir: dir.path().join("Local").join("SurvivorsSatchel"),
        };
        Pc { _dir: dir, game, local, store: Store::default() }
    }

    impl Pc {
        fn addons(&mut self) -> Addons<'_> {
            Addons { game: &self.game, local: &self.local, store: &mut self.store }
        }
    }

    fn t(minutes: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 30, 18, 0, 0).unwrap() + chrono::TimeDelta::minutes(minutes)
    }

    fn lucky(version: &str) -> Incoming {
        let toc = format!("## Interface: 11200\n## Title: LuckyOdds\n## Version: {version}\n");
        let zip = zip_of(&[("LuckyOdds/LuckyOdds.toc", toc.as_bytes()), ("LuckyOdds/core.lua", version.as_bytes()), ("LuckyOdds/x.exe", b"MZ")]);
        Incoming {
            sha256: Some(fsx::fingerprint(&zip)),
            zip,
            id: "lucky-odds".into(),
            name: "LuckyOdds".into(),
            badge: Badge::Verified,
            repo: Some("cadoky/LuckyOdds".into()),
            tag: Some(format!("v{version}")),
            file: Some(format!("LuckyOdds-v{version}.zip")),
            version: Some(version.into()),
            released: None,
            url: None,
            forge: None,
            expected_folders: None,
            pinned: false,
        }
    }

    /// An addon from SurvivorsForge alone, with these folders in its zip; SurvivorsForge lists one.
    fn totems(folders: &[&str], version: &str) -> Incoming {
        let files: Vec<(String, String)> =
            folders.iter().map(|f| (format!("{f}/{f}.toc"), format!("## Interface: 11200\n## Version: {version}\n"))).collect();
        let entries: Vec<(&str, &[u8])> = files.iter().map(|(path, text)| (path.as_str(), text.as_bytes())).collect();
        let zip = zip_of(&entries);
        Incoming {
            sha256: Some(fsx::fingerprint(&zip)),
            zip,
            id: "forge:totems".into(),
            name: "Totems".into(),
            badge: Badge::Added,
            repo: Some("sam/Totems".into()),
            tag: Some(format!("v{version}")),
            file: Some("Totems.zip".into()),
            version: None,
            released: None,
            url: Some(format!("https://github.com/sam/Totems/releases/download/v{version}/Totems.zip")),
            forge: Some("totems".into()),
            expected_folders: Some(vec!["totems".into()]),
            pinned: false,
        }
    }

    fn core_lua(pc: &Pc) -> String {
        fs::read_to_string(pc.game.addons.join("LuckyOdds").join("core.lua")).unwrap()
    }

    #[test]
    fn install_then_update_keeps_the_old_version_and_its_settings() {
        let mut pc = pc();
        let done = pc.addons().install(lucky("0.6.0"), t(0)).unwrap();
        assert_eq!(done.left_out, 1, "the program in the zip");
        assert!(!pc.game.addons.join("LuckyOdds").join("x.exe").exists());
        assert_eq!(core_lua(&pc), "0.6.0");
        assert_eq!(done.record.folders["LuckyOdds"], super::super::tree::fingerprint_dir(&pc.game.addons.join("LuckyOdds")).unwrap());
        assert!(!done.record.ask, "verified addons update by themselves");

        fs::create_dir_all(&pc.game.saved).unwrap();
        fs::write(pc.game.settings_file("LuckyOdds"), "LuckyOddsDB = {}").unwrap();
        let done = pc.addons().install(lucky("0.7.0"), t(10)).unwrap();
        assert_eq!(core_lua(&pc), "0.7.0");
        assert_eq!(done.record.earlier.len(), 1);
        let kept = pc.local.addon_backups().join(&done.record.earlier[0].backup);
        assert_eq!(fs::read_to_string(kept.join("LuckyOdds").join("core.lua")).unwrap(), "0.6.0");
        assert_eq!(fs::read_to_string(kept.join("saved").join("LuckyOdds.lua")).unwrap(), "LuckyOddsDB = {}");
        assert!(!pc.local.addon_staging().join(names::stamp(t(10))).exists(), "no staging left");
        assert_eq!(Store::load(&pc.local.addon_store()).unwrap(), pc.store, "the record is on disk");
    }

    #[test]
    fn a_survivorsforge_install_holds_exactly_its_listed_folders() {
        let mut pc = pc();
        // Two folders where SurvivorsForge lists one: refused, and nothing written.
        let err = pc.addons().install(totems(&["Totems", "Extra"], "1.1"), t(0)).unwrap_err();
        assert!(matches!(err, AddonError::Folders { .. }), "{err}");
        assert!(!pc.game.addons.join("Totems").exists() && pc.store.addons.is_empty());
        // Its one folder (names compare without case): in, and it updates by itself, as SurvivorsForge's.
        let done = pc.addons().install(totems(&["Totems"], "1.1"), t(1)).unwrap();
        assert_eq!((done.record.forge.as_deref(), done.record.badge, done.record.ask), (Some("totems"), Badge::Added, false));
        assert_eq!(done.record.url.as_deref(), Some("https://github.com/sam/Totems/releases/download/v1.1/Totems.zip"));
        // An update that doesn't name it keeps where it came from.
        let mut update = totems(&["Totems"], "1.2");
        (update.forge, update.expected_folders) = (None, None);
        let done = pc.addons().install(update, t(2)).unwrap();
        assert_eq!((done.record.forge.as_deref(), done.record.ask, done.record.version.as_deref()), (Some("totems"), false, Some("1.2")));
        // Added by link: asks first, as before.
        let mut linked = lucky("0.6.0");
        (linked.id, linked.badge) = ("cadoky/LuckyOdds".into(), Badge::Added);
        assert!(pc.addons().install(linked, t(3)).unwrap().record.ask);
    }

    #[test]
    fn a_tag_zips_wrapper_folder_installs_under_its_tocs_name() {
        let mut pc = pc();
        // GitHub's zip of a tag wraps the addon in "<repo>-<tag>/"; SurvivorsForge lists "KMA".
        let zip = zip_of(&[("KMA-1/KMA.toc", b"## Interface: 11200\n## Title: KMA\n"), ("KMA-1/KMA.lua", b"-- the arrow")]);
        let incoming = Incoming {
            sha256: Some(fsx::fingerprint(&zip)),
            zip,
            id: "forge:kma".into(),
            name: "KMA".into(),
            badge: Badge::Added,
            repo: Some("sam/KMA".into()),
            tag: Some("1".into()),
            file: Some("KMA-1.zip".into()),
            version: None,
            released: None,
            url: Some("https://codeload.github.com/sam/KMA/zip/refs/tags/1".into()),
            forge: Some("kma".into()),
            expected_folders: Some(vec!["KMA".into()]),
            pinned: false,
        };
        let done = pc.addons().install(incoming, t(0)).unwrap();
        assert_eq!(done.record.folders.keys().collect::<Vec<_>>(), ["KMA"]);
        assert!(pc.game.addons.join("KMA").join("KMA.lua").is_file() && !pc.game.addons.join("KMA-1").exists());
    }

    #[test]
    fn the_same_version_again_changes_nothing_unless_its_files_changed() {
        let mut pc = pc();
        let first = pc.addons().install(lucky("0.6.0"), t(0)).unwrap();
        let again = pc.addons().install(lucky("0.6.0"), t(5)).unwrap();
        assert!(again.already_installed);
        assert_eq!(again.record, first.record, "the record stays as it was");
        assert!(!pc.local.addon_backups().exists(), "no backup of the same files");
        assert!(!first.already_installed);

        // A copy edited since is put right by the same version, and the edited one is kept.
        fs::write(pc.game.addons.join("LuckyOdds").join("core.lua"), "edited").unwrap();
        let repaired = pc.addons().install(lucky("0.6.0"), t(10)).unwrap();
        assert!(!repaired.already_installed);
        assert_eq!(core_lua(&pc), "0.6.0");
        assert_eq!(repaired.record.earlier.len(), 1);
    }

    #[test]
    fn release_dates_travel_with_their_versions() {
        let mut pc = pc();
        let day = |d: u32| Utc.with_ymd_and_hms(2026, 10, d, 9, 0, 0).unwrap();
        let dated = |version: &str, d: u32| Incoming { released: Some(day(d)), ..lucky(version) };
        pc.addons().install(dated("0.6.0", 20), t(0)).unwrap();
        let done = pc.addons().install(dated("0.7.0", 27), t(10)).unwrap();
        assert_eq!((done.record.released, done.record.earlier[0].released), (Some(day(27)), Some(day(20))));
        let back = pc.addons().go_back("lucky-odds", None, t(20)).unwrap();
        assert_eq!((back.released, back.earlier[0].released), (Some(day(20)), Some(day(27))));
    }

    #[test]
    fn a_wrong_checksum_or_a_clash_changes_nothing() {
        let mut pc = pc();
        pc.addons().install(lucky("0.6.0"), t(0)).unwrap();
        let mut tampered = lucky("0.7.0");
        tampered.sha256 = Some(fsx::fingerprint(b"what the author published"));
        assert!(matches!(pc.addons().install(tampered, t(5)), Err(AddonError::Checksum { .. })));
        let mut other = lucky("9.9.9");
        other.id = "someone-else/LuckyOdds".into();
        other.name = "Another LuckyOdds".into();
        let err = pc.addons().install(other, t(6)).unwrap_err();
        assert_eq!(err.to_string(), "LuckyOdds already uses the folder LuckyOdds; two addons can't share one");
        assert_eq!(core_lua(&pc), "0.6.0");
        assert_eq!(pc.store.addons.len(), 1);
    }

    #[test]
    fn a_swap_that_fails_puts_the_old_version_back() {
        let mut pc = pc();
        pc.addons().install(lucky("0.6.0"), t(0)).unwrap();
        // 0.7.0 brings a second folder whose place is taken by a file (on Windows: something held
        // open): the old LuckyOdds is already in the backups and the new one in place when that
        // move fails, so both steps are undone.
        fs::write(pc.game.addons.join("Zone"), "in the way").unwrap();
        let mut update = lucky("0.7.0");
        update.zip = zip_of(&[
            ("LuckyOdds/LuckyOdds.toc", b"## Interface: 11200\n## Version: 0.7.0\n"),
            ("LuckyOdds/core.lua", b"0.7.0"),
            ("Zone/Zone.toc", b"## Interface: 11200\n"),
        ]);
        update.sha256 = Some(fsx::fingerprint(&update.zip));
        assert!(pc.addons().install(update, t(10)).is_err());
        assert_eq!(core_lua(&pc), "0.6.0");
        assert_eq!(fs::read_to_string(pc.game.addons.join("Zone")).unwrap(), "in the way");
        assert_eq!(pc.store.get("lucky-odds").unwrap().version.as_deref(), Some("0.6.0"));
        let backup = pc.local.addon_backups().join(format!("{} lucky-odds", names::stamp(t(10))));
        assert!(!backup.join("LuckyOdds").exists(), "the backup went back into AddOns");
    }

    #[test]
    fn going_back_restores_and_keeps_that_version() {
        let mut pc = pc();
        pc.addons().install(lucky("0.6.0"), t(0)).unwrap();
        pc.addons().install(lucky("0.7.0"), t(10)).unwrap();
        let back = pc.addons().go_back("lucky-odds", None, t(20)).unwrap();
        assert_eq!(core_lua(&pc), "0.6.0");
        assert_eq!(back.version.as_deref(), Some("0.6.0"));
        assert!(back.pinned, "going back keeps that version");
        assert_eq!(back.earlier[0].version.as_deref(), Some("0.7.0"), "and 0.7.0 is kept to return to");
        assert_eq!(back.folders["LuckyOdds"], super::super::tree::fingerprint_dir(&pc.game.addons.join("LuckyOdds")).unwrap());
        pc.addons().pin("lucky-odds", false).unwrap();
        assert!(!pc.store.get("lucky-odds").unwrap().pinned);
        let forward = pc.addons().go_back("lucky-odds", None, t(30)).unwrap();
        assert_eq!(core_lua(&pc), "0.7.0");
        assert_eq!(forward.version.as_deref(), Some("0.7.0"));
    }

    #[test]
    fn going_back_to_a_chosen_kept_version() {
        let mut pc = pc();
        for (i, v) in ["0.5.0", "0.6.0", "0.7.0"].iter().enumerate() {
            pc.addons().install(lucky(v), t(i as i64 * 10)).unwrap();
        }
        let record = pc.store.get("lucky-odds").unwrap().clone();
        let kept: Vec<_> = record.earlier.iter().map(|k| k.version.clone().unwrap()).collect();
        assert_eq!(kept, ["0.6.0", "0.5.0"]);
        // Versions…' Go back to this, on the older one: that one comes back, kept at it.
        let oldest = record.earlier[1].backup.clone();
        let back = pc.addons().go_back("lucky-odds", Some(&oldest), t(30)).unwrap();
        assert_eq!(core_lua(&pc), "0.5.0");
        assert!(back.pinned, "kept at that version, as Go back keeps one");
        let kept: Vec<_> = back.earlier.iter().map(|k| k.version.clone().unwrap()).collect();
        assert_eq!(kept, ["0.7.0", "0.6.0"], "the version it replaced first, the other as it was");
        assert!(!pc.local.addon_backups().join(&oldest).exists(), "its backup is what went back in");
        assert_eq!(Store::load(&pc.local.addon_store()).unwrap(), pc.store, "the record is on disk");
        // A version no longer kept: refused, and nothing changes.
        let err = pc.addons().go_back("lucky-odds", Some(&oldest), t(40)).unwrap_err();
        assert!(matches!(err, AddonError::NotKept(_)), "{err}");
        assert_eq!(core_lua(&pc), "0.5.0");
        // Without a target: the newest kept, as before.
        let forward = pc.addons().go_back("lucky-odds", None, t(50)).unwrap();
        assert_eq!((forward.version.as_deref(), core_lua(&pc).as_str()), (Some("0.7.0"), "0.7.0"));
    }

    #[test]
    fn a_version_picked_in_versions_is_kept_until_it_may_update() {
        let mut pc = pc();
        pc.addons().install(lucky("0.7.0"), t(0)).unwrap();
        // An older release installed from Versions…: pinned, the way Go back pins.
        let picked = pc.addons().install(Incoming { pinned: true, ..lucky("0.6.0") }, t(10)).unwrap();
        assert_eq!((core_lua(&pc), picked.record.pinned), ("0.6.0".to_string(), true));
        assert_eq!(picked.record.earlier[0].version.as_deref(), Some("0.7.0"), "the version it replaced is kept");
        assert!(Store::load(&pc.local.addon_store()).unwrap().get("lucky-odds").unwrap().pinned, "on disk");
        // Letting it update again unpins it, and an ordinary install isn't pinned.
        pc.addons().pin("lucky-odds", false).unwrap();
        assert!(!pc.store.get("lucky-odds").unwrap().pinned);
        assert!(!pc.addons().install(lucky("0.7.0"), t(20)).unwrap().record.pinned);
    }

    #[test]
    fn only_three_earlier_versions_are_kept() {
        let mut pc = pc();
        for (i, v) in ["0.1.0", "0.2.0", "0.3.0", "0.4.0", "0.5.0"].iter().enumerate() {
            pc.addons().install(lucky(v), t(i as i64 * 10)).unwrap();
        }
        let record = pc.store.get("lucky-odds").unwrap();
        let kept: Vec<_> = record.earlier.iter().map(|k| k.version.clone().unwrap()).collect();
        assert_eq!(kept, ["0.4.0", "0.3.0", "0.2.0"]);
        assert_eq!(fs::read_dir(pc.local.addon_backups()).unwrap().count(), 3, "the oldest backup is gone");
    }

    #[test]
    fn removing_recycles_and_tidies() {
        let mut pc = pc();
        pc.addons().install(lucky("0.6.0"), t(0)).unwrap();
        fs::create_dir_all(&pc.game.saved).unwrap();
        fs::write(pc.game.settings_file("LuckyOdds"), "LuckyOddsDB = {}").unwrap();
        let mut list = EnableList::read(&pc.game.enable_list()).unwrap();
        list.set("LuckyOdds", false);
        list.set("Other", false);
        list.write(&pc.game.enable_list()).unwrap();

        let mut recycled = Vec::new();
        pc.addons()
            .remove("lucky-odds", true, None, t(5), |path| {
                recycled.push(path.file_name().unwrap().to_string_lossy().into_owned());
                fsx::remove_dir_if_present(path)
            })
            .unwrap();
        assert_eq!(recycled, ["LuckyOdds"], "settings kept for a reinstall");
        assert!(pc.game.settings_file("LuckyOdds").exists());
        assert!(!pc.game.addons.join("LuckyOdds").exists());
        assert!(pc.store.addons.is_empty());
        assert_eq!(fs::read_to_string(pc.game.enable_list()).unwrap(), "Other: disabled\n");
    }
}
