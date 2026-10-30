//! Addons on every PC: one list of the addons every PC has, `sync\addons\addons.json`, and each
//! addon's settings as numbered versions beside it. It decides the way saves do, by content and
//! three ways: this PC now (L), what this PC and the list last agreed on (B), the list (N). Clocks
//! decide nothing, except on a PC's first run, when the settings of the PC played last are taken.
//!
//! Its promises: an addon goes from every PC only when the player removed it with the Satchel (a
//! lost record or a folder deleted by hand is put back instead); when two PCs changed one addon's
//! settings since they last agreed, both are kept as versions and the player picks; settings files
//! are written only with the game closed (the caller's part), after a backup.

use super::store::{Badge, Record};
use super::tree;
use crate::decide::{Decision, decide};
use crate::index::IndexLock;
use crate::paths::Shared;
use crate::{fsx, game_settings, gate, log, names};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use zip::ZipArchive;
use zip::write::SimpleFileOptions;

/// Settings versions kept per addon; older ones are deleted from the NAS.
pub const SETTINGS_KEEP: usize = 30;
/// The largest settings set sent or taken from the NAS.
const MAX_BUNDLE: u64 = 64 << 20;

/// A file name from the NAS's list that is only a name: nothing that could lead out of its folder.
fn plain(file: &str) -> bool {
    !file.is_empty() && file != "." && file != ".." && !file.contains(['/', '\\', ':'])
}

/// A release as the list carries it: enough for any PC to download it and check it's the file the
/// first PC installed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shipped {
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where it downloads from; empty for a zip imported from a file, which waits in the list's
    /// `files\` folder instead.
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<DateTime<Utc>>,
    #[serde(default)]
    pub size: u64,
}

impl Shipped {
    pub fn shown_version(&self) -> String {
        self.version.clone().or_else(|| self.tag.clone()).unwrap_or_else(|| "unknown version".into())
    }
}

/// One addon in the list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub badge: Badge,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The release every PC has; `None` once it's removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<Shipped>,
    /// Its folders in AddOns.
    #[serde(default)]
    pub folders: Vec<String>,
    #[serde(default)]
    pub on: bool,
    /// The addon's folders the game doesn't load, by name in lower case, sorted (since 0.5.1); `on`
    /// stays for readers from before, and is false when any folder is off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub off: Vec<String>,
    /// "Keep this version".
    #[serde(default)]
    pub pinned: bool,
    /// Every PC keeps its own settings for it.
    #[serde(default)]
    pub settings_per_pc: bool,
    /// Removed: when a PC removes it, its settings stay for a reinstall.
    #[serde(default)]
    pub keep_settings: bool,
    /// The PCs that have it through the list. When the last one keeps it to itself ("Only on this
    /// PC"), it leaves the list.
    #[serde(default)]
    pub pcs: Vec<String>,
    /// Who changed it last, and when.
    pub pc: String,
    pub at: DateTime<Utc>,
}

impl Entry {
    pub fn removed(&self) -> bool {
        self.release.is_none()
    }

    /// What has to be the same on every PC, as this entry says. One written before 0.5.1 says only
    /// on or off, and its off is every folder (`Agreed::with_folders`).
    pub fn agreed(&self) -> Agreed {
        match &self.release {
            None => Agreed::removed(),
            Some(r) => {
                let off = folder_names(self.off.clone());
                let agreed = Agreed { release: Some(r.sha256.clone()), on: self.on && off.is_empty(), off, pinned: self.pinned, settings_per_pc: self.settings_per_pc };
                agreed.with_folders(&self.folders)
            }
        }
    }
}

/// One version of an addon's settings: a zip in `settings\<addon>\`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingsVersion {
    pub n: u64,
    pub addon: String,
    pub pc: String,
    pub file: String,
    /// The settings' fingerprint (`Bundle::fingerprint`), not the zip's.
    pub sha256: String,
    pub bytes: u64,
    pub saved: DateTime<Utc>,
    /// When its newest file was written on that PC: on a first run the PC played last wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Its files, by their path in the set (since 0.4): a PC whose own set lost some of them asks
    /// before sending (`game_settings::step`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// `sync\addons\addons.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct List {
    pub format: u32,
    /// Goes up with every change: a PC sees at a glance whether anything moved.
    pub rev: u64,
    pub addons: Vec<Entry>,
    /// The highest settings version number given so far.
    #[serde(default)]
    pub settings_newest: u64,
    /// Newest first.
    #[serde(default)]
    pub settings: Vec<SettingsVersion>,
    /// The newest Satchel that wrote the list (since 0.4.1): an older one stays out (`gate`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest_satchel: Option<String>,
}

impl Default for List {
    fn default() -> Self {
        List { format: 1, rev: 0, addons: Vec::new(), settings_newest: 0, settings: Vec::new(), newest_satchel: None }
    }
}

impl List {
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.addons.iter().find(|e| e.id == id)
    }

    /// Adds the entry, or replaces the one with its id.
    pub fn put(&mut self, entry: Entry) {
        match self.addons.iter_mut().find(|e| e.id == entry.id) {
            Some(slot) => *slot = entry,
            None => self.addons.push(entry),
        }
        self.addons.sort_by_key(|e| e.name.to_lowercase());
        self.rev += 1;
    }

    /// Records that `pc` has this addon through the list.
    pub fn join(&mut self, id: &str, pc: &str) {
        if let Some(entry) = self.addons.iter_mut().find(|e| e.id == id)
            && !entry.pcs.iter().any(|p| p == pc)
        {
            entry.pcs.push(pc.into());
            self.rev += 1;
        }
    }

    /// `pc` keeps the addon to itself or no longer has it. An addon nobody has through the list
    /// any more leaves it, so no PC removes anything.
    pub fn leave(&mut self, id: &str, pc: &str) {
        let Some(at) = self.addons.iter().position(|e| e.id == id) else { return };
        let entry = &mut self.addons[at];
        let before = entry.pcs.len();
        entry.pcs.retain(|p| p != pc);
        if entry.pcs.is_empty() && !entry.removed() {
            self.addons.remove(at);
        } else if entry.pcs.len() == before {
            return;
        }
        self.rev += 1;
    }

    pub fn newest_settings(&self, addon: &str) -> Option<&SettingsVersion> {
        self.settings.iter().filter(|v| v.addon == addon).max_by_key(|v| v.n)
    }

    pub fn settings_version(&self, n: u64) -> Option<&SettingsVersion> {
        self.settings.iter().find(|v| v.n == n)
    }

    /// The number the next settings version gets.
    fn next_settings(&self) -> u64 {
        self.settings.iter().map(|v| v.n).max().unwrap_or(0).max(self.settings_newest) + 1
    }

    /// Records a settings version just written (numbered by `next_settings`); returns the files of
    /// the versions that fell out of the `SETTINGS_KEEP` newest for that addon.
    fn push_settings(&mut self, version: SettingsVersion) -> Vec<String> {
        self.settings_newest = version.n;
        self.settings.insert(0, version.clone());
        let mut kept = 0;
        let mut dropped = Vec::new();
        self.settings.retain(|v| {
            if v.addon != version.addon {
                return true;
            }
            kept += 1;
            if kept <= SETTINGS_KEEP {
                return true;
            }
            dropped.push(v.file.clone());
            false
        });
        self.rev += 1;
        dropped
    }
}

/// What has to be the same on every PC for an addon: the release (by its zip's checksum), which of
/// its folders are off, "Keep this version", "Settings per PC"; or that it's removed. Names, dates
/// and which PCs have it don't count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agreed {
    /// The release's checksum; `None`: removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// The game loads all its folders: `off` is empty.
    #[serde(default)]
    pub on: bool,
    /// The folders the game doesn't load, by name in lower case, sorted (since 0.5.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub off: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub settings_per_pc: bool,
}

impl Agreed {
    pub fn removed() -> Agreed {
        Agreed { release: None, on: false, off: Vec::new(), pinned: false, settings_per_pc: false }
    }

    /// This PC's addon as the list would carry it; `off` names the folders the game doesn't load.
    pub fn of(record: &Record, off: Vec<String>) -> Agreed {
        let off = folder_names(off);
        Agreed { release: Some(identity(record)), on: off.is_empty(), off, pinned: record.pinned, settings_per_pc: record.settings_per_pc }
    }

    /// An agreement from before 0.5.1 (in the list, or in a PC's memory) had one switch for the
    /// whole addon, and off meant every folder off: that is how the other PCs followed it. Given
    /// the addon's folders, such an off becomes all of them; any other agreement stays as it is.
    pub fn with_folders(mut self, folders: &[String]) -> Agreed {
        if !self.is_removed() && !self.on && self.off.is_empty() {
            self.off = folder_names(folders.to_vec());
            self.on = self.off.is_empty();
        }
        self
    }

    pub fn is_removed(&self) -> bool {
        self.release.is_none()
    }
}

/// Folder names as the list compares them: in lower case (ASCII, the game's own rule for addon
/// names), sorted, each once.
fn folder_names(mut names: Vec<String>) -> Vec<String> {
    for name in &mut names {
        name.make_ascii_lowercase();
    }
    names.sort();
    names.dedup();
    names
}

/// A record's release, as the list tells releases apart: the zip's checksum.
pub fn identity(record: &Record) -> String {
    record.sha256.clone().unwrap_or_else(|| format!("tag:{}", record.shown_version()))
}

/// A settings question waiting for the player: this PC's settings and the list's newest both
/// changed since they last agreed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Split {
    /// The list's version shown.
    pub nas: u64,
    pub pc: String,
    /// This PC's settings' fingerprint shown.
    pub local: String,
}

/// What this PC remembers between runs (`addon-sync.json` in Local AppData).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    /// B for each addon: what this PC and the list last agreed on.
    pub agreed: BTreeMap<String, Agreed>,
    /// B for each addon's settings: their fingerprint when last sent or brought in.
    pub settings: BTreeMap<String, String>,
    /// Addons the player removed here, until the list has the removal: whether their settings
    /// stay.
    pub removed: BTreeMap<String, bool>,
    /// Settings questions waiting for the player, by addon id.
    pub splits: BTreeMap<String, Split>,
    /// Addons kept to this PC ("Only on this PC") and then removed here: the list's copy doesn't
    /// come back. Installing it again ends that.
    pub opted_out: BTreeSet<String>,
    /// The list's `rev` when this PC last read it.
    pub rev: u64,
    /// The game's own settings this PC had when it joined, kept on the NAS: "Use them instead"
    /// brings them back, until the player changes the settings here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_own_kept: Option<OwnKept>,
}

/// This PC's own game settings from before it joined.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnKept {
    /// The version that holds them.
    pub n: u64,
    /// The set's fingerprint as placed at the join (this PC's own heroes' files and the run-once
    /// marker stay, so it may differ from the version taken); a set sent that differs from it was
    /// changed by the player, and the offer ends.
    pub placed: Option<String>,
}

impl Memory {
    pub fn load(path: &Path) -> io::Result<Memory> {
        match fsx::read_optional(path)? {
            None => Ok(Memory::default()),
            Some(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        fsx::write_atomic(path, &serde_json::to_vec_pretty(self).map_err(io::Error::other)?)
    }
}

// The decisions --------------------------------------------------------------------------------

/// An addon on this PC, as far as the list goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Here {
    /// Installed, and shared.
    Has(Agreed),
    /// The player removed it here with the Satchel; the list doesn't know yet.
    Removed,
    /// Not here: never installed, or lost (a record gone, a folder deleted by hand). It follows
    /// the list.
    Missing,
}

/// What this PC does about one addon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// This PC and the list agree.
    InStep,
    /// What this PC has (or that it removed it) goes into the list.
    Send,
    /// This PC follows the list's entry: install, update, removal or a switch, with the game
    /// closed.
    Follow,
    /// Neither here nor in the list.
    Nothing,
}

/// L against B against N for one addon. `here_wins` settles the case where both moved on (or this
/// PC never agreed with the list on it): `true` keeps this PC's.
pub fn step(here: &Here, base: Option<&Agreed>, nas: Option<&Agreed>, here_wins: impl FnOnce() -> bool) -> Step {
    match (here, nas) {
        (Here::Missing | Here::Removed, None) => Step::Nothing,
        (Here::Has(_), None) => Step::Send,
        (Here::Missing, Some(n)) => {
            if n.is_removed() {
                Step::Nothing
            } else {
                Step::Follow
            }
        }
        (Here::Removed, Some(n)) if n.is_removed() => Step::InStep,
        // Removed here; the list still has what this PC removed: the removal goes up. If another
        // PC changed it meanwhile, that wins: an install beats a removal.
        (Here::Removed, Some(n)) => {
            if base == Some(n) {
                Step::Send
            } else {
                Step::Follow
            }
        }
        (Here::Has(h), Some(n)) if h == n => Step::InStep,
        (Here::Has(h), Some(n)) => {
            if base == Some(n) {
                Step::Send
            } else if base == Some(h) {
                Step::Follow
            } else if n.is_removed() || here_wins() {
                // Changed here and removed elsewhere, or a first run: it stays.
                Step::Send
            } else {
                Step::Follow
            }
        }
    }
}

/// Which side keeps its release when both moved on: `Some(true)` this PC's. On a first run the
/// newer release wins; later "Keep this version" on one side wins, then the newer one. Same
/// release (only a switch differs), or no way to tell: the list's.
pub fn here_wins(
    here: &Agreed,
    nas: &Agreed,
    first_run: bool,
    newer: impl FnOnce() -> Option<std::cmp::Ordering>,
) -> bool {
    if here.release == nas.release {
        return false;
    }
    if !first_run && here.pinned != nas.pinned {
        return here.pinned;
    }
    newer() == Some(std::cmp::Ordering::Greater)
}

/// What this PC does about one addon's settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsStep {
    /// The same here and in the list. `adopt`: B catches up to the list's version.
    InStep { adopt: Option<u64> },
    /// This PC's settings go up as the next version.
    Send,
    /// The list's version `n` comes here (game closed). With `keep_mine`, this PC's settings go up
    /// first as a version of their own, so nothing is lost.
    BringIn { n: u64, keep_mine: bool },
    /// Both changed since they agreed: the player picks; both are kept.
    Ask { nas: u64 },
}

/// Saves' rules for one addon's settings (`decide`). On this PC's first look at an addon whose
/// settings differ from the list's, the PC played last wins (`here_newer`), and the other's are
/// kept as a version.
pub fn settings_step(local: Option<&str>, base: Option<&str>, nas: Option<(&str, u64)>, here_newer: impl FnOnce() -> bool) -> SettingsStep {
    match decide(local, base, nas) {
        Decision::InSync { adopt } => SettingsStep::InStep { adopt },
        Decision::Send => SettingsStep::Send,
        Decision::BringIn(n) => SettingsStep::BringIn { n, keep_mine: false },
        Decision::Split { nas } if base.is_none() => {
            if here_newer() {
                SettingsStep::Send
            } else {
                SettingsStep::BringIn { n: nas, keep_mine: true }
            }
        }
        Decision::Split { nas } => SettingsStep::Ask { nas },
    }
}

// Settings files ---------------------------------------------------------------------------------

/// An addon's settings on a PC: its SavedVariables under `saved\`, account-wide (`<Addon>.lua`)
/// and per character (`<Realm>-<Character>\<Addon>.lua`), by their path there with `/`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bundle {
    pub files: BTreeMap<String, Vec<u8>>,
    /// The newest file's time: on a first run the PC played last gives the settings.
    pub newest: Option<DateTime<Utc>>,
}

/// Whether `file` is one of the addon's settings files: `<folder>.lua` for one of its folders
/// (`<folder>.lua.bak`, the game's own leftover, isn't).
fn is_settings_of(file: &str, folders: &[String]) -> bool {
    file.rsplit_once('.')
        .is_some_and(|(stem, ext)| ext.eq_ignore_ascii_case("lua") && folders.iter().any(|f| f.eq_ignore_ascii_case(stem)))
}

impl Bundle {
    /// The addon's settings files in `saved` (the game's `benilla-config\saved`).
    pub fn read(saved: &Path, folders: &[String]) -> io::Result<Bundle> {
        let mut bundle = Bundle::default();
        let entries = match fs::read_dir(saved) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(bundle),
            Err(e) => return Err(e),
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = entry.file_type()?;
            if kind.is_file() && is_settings_of(&name, folders) {
                bundle.take(&entry.path(), name)?;
            } else if kind.is_dir() {
                for inner in fs::read_dir(entry.path())?.filter_map(Result::ok) {
                    let file = inner.file_name().to_string_lossy().into_owned();
                    if inner.file_type()?.is_file() && is_settings_of(&file, folders) {
                        bundle.take(&inner.path(), format!("{name}/{file}"))?;
                    }
                }
            }
        }
        Ok(bundle)
    }

    pub(crate) fn take(&mut self, path: &Path, rel: String) -> io::Result<()> {
        let bytes = fs::read(path)?;
        let modified = fs::metadata(path).and_then(|m| m.modified()).ok().map(DateTime::<Utc>::from);
        self.insert(rel, bytes, modified);
        Ok(())
    }

    /// A file of the set that isn't a file on disk as it is (the game's preferences, cut out of
    /// its config.toml), written at `modified`.
    pub(crate) fn insert(&mut self, rel: String, bytes: Vec<u8>, modified: Option<DateTime<Utc>>) {
        self.newest = self.newest.max(modified);
        self.files.insert(rel, bytes);
    }

    /// One fingerprint over the files' paths and contents (paths without case); `None` when there
    /// are no settings.
    pub fn fingerprint(&self) -> Option<String> {
        (!self.files.is_empty()).then(|| tree::combine(self.files.iter().map(|(p, b)| (p.clone(), fsx::fingerprint(b))).collect()))
    }

    pub fn to_zip(&self) -> io::Result<Vec<u8>> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (path, bytes) in &self.files {
            zip.start_file(path.as_str(), options).map_err(io::Error::other)?;
            zip.write_all(bytes)?;
        }
        Ok(zip.finish().map_err(io::Error::other)?.into_inner())
    }

    /// An addon's settings zip from the NAS. Only `.lua` files at most one folder deep, by safe
    /// names, are taken; anything else makes it unreadable.
    pub fn from_zip(bytes: &[u8]) -> io::Result<Bundle> {
        Bundle::from_zip_with(bytes, |name| {
            let parts: Vec<&str> = name.split('/').collect();
            parts.len() <= 2
                && name.to_ascii_lowercase().ends_with(".lua")
                && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && !p.contains(['\\', ':']) && names::safe(p) == *p)
        })
    }

    /// The settings zip of the set `id`: an addon's, or the game's own (its files and nothing
    /// else, `game_settings::is_settings_path`).
    pub fn from_zip_for(bytes: &[u8], id: &str) -> io::Result<Bundle> {
        if id == game_settings::ID { Bundle::from_zip_with(bytes, game_settings::is_settings_path) } else { Bundle::from_zip(bytes) }
    }

    fn from_zip_with(bytes: &[u8], allowed: impl Fn(&str) -> bool) -> io::Result<Bundle> {
        let bad = |why: String| io::Error::new(io::ErrorKind::InvalidData, why);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|e| bad(e.to_string()))?;
        let mut bundle = Bundle::default();
        let mut total = 0u64;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| bad(e.to_string()))?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().to_string();
            if !allowed(&name) {
                return Err(bad(format!("a settings zip holds {name:?}")));
            }
            // The sizes a zip states can lie: what is read counts.
            let mut content = Vec::new();
            (&mut file).take(MAX_BUNDLE + 1 - total).read_to_end(&mut content)?;
            total += content.len() as u64;
            if total > MAX_BUNDLE {
                return Err(bad("a settings zip unpacks to far too much".into()));
            }
            bundle.files.insert(name, content);
        }
        Ok(bundle)
    }

    /// Puts these settings in place of the addon's own on this PC: the current files go to
    /// `backup` first, then these are written, then files of the addon that these don't have are
    /// removed (they are in the backup). The game must be closed.
    pub fn place(&self, saved: &Path, folders: &[String], backup: &Path) -> io::Result<()> {
        let current = Bundle::read(saved, folders)?;
        for (rel, bytes) in &current.files {
            fsx::write_atomic(&join(backup, rel), bytes)?;
        }
        for (rel, bytes) in &self.files {
            fsx::write_atomic(&join(saved, rel), bytes)?;
        }
        let incoming: BTreeSet<String> = self.files.keys().map(|k| k.to_lowercase()).collect();
        for rel in current.files.keys().filter(|rel| !incoming.contains(&rel.to_lowercase())) {
            fsx::remove_if_present(&join(saved, rel))?;
        }
        Ok(())
    }
}

/// `rel` (with `/`) under `dir`.
pub(crate) fn join(dir: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(dir.to_path_buf(), |path, part| path.join(part))
}

// The list on the NAS ----------------------------------------------------------------------------

/// How long to wait for another PC that is writing the list.
pub const LOCK_WAIT: Duration = Duration::from_secs(15);

/// Reads the list; a missing one is an empty list. An unreadable list is an error, never replaced
/// by itself.
/// Reads the list; one written by a newer Satchel is refused (`gate`).
pub fn read(shared: &Shared) -> io::Result<List> {
    read_as(shared, gate::this())
}

fn read_as(shared: &Shared, this: Option<&str>) -> io::Result<List> {
    let path = shared.addons_list();
    match fsx::read_optional(&path)? {
        None => Ok(List::default()),
        Some(bytes) => {
            let list: List = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))?;
            gate::check_as(this, list.newest_satchel.as_deref())?;
            Ok(list)
        }
    }
}

/// Writes the list, stamped with this Satchel's version when it is the newest to write it.
pub fn write(shared: &Shared, list: &List) -> io::Result<()> {
    let stamped = List { newest_satchel: gate::stamp(list.newest_satchel.as_deref()), ..list.clone() };
    fsx::write_atomic(&shared.addons_list(), &serde_json::to_vec_pretty(&stamped).map_err(io::Error::other)?)
}

/// One PC at a time changes the list.
pub fn lock(shared: &Shared, pc: &str) -> io::Result<IndexLock> {
    IndexLock::acquire_at(shared.addons_lock(), pc, LOCK_WAIT)
}

/// Writes these settings as the addon's next version: the zip, then the list (the caller holds the
/// lock and has read `list` under it). Versions past the newest `SETTINGS_KEEP` are deleted.
pub fn send_settings(
    shared: &Shared,
    list: &mut List,
    addon: &str,
    pc: &str,
    bundle: &Bundle,
    note: Option<String>,
    now: DateTime<Utc>,
) -> io::Result<SettingsVersion> {
    let sha256 = bundle.fingerprint().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no settings to send"))?;
    if bundle.files.values().map(|b| b.len() as u64).sum::<u64>() > MAX_BUNDLE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "its settings are larger than any PC would take in"));
    }
    let zip = bundle.to_zip()?;
    let n = list.next_settings();
    let file = format!("{} ({}) v{n}.zip", names::stamp(now), names::safe(pc));
    fsx::write_atomic(&shared.settings_version(addon, &file), &zip)?;
    let files = bundle.files.keys().cloned().collect();
    let version = SettingsVersion {
        n,
        addon: addon.into(),
        pc: pc.into(),
        file,
        sha256,
        bytes: zip.len() as u64,
        saved: now,
        written: bundle.newest,
        note,
        files,
    };
    let dropped = list.push_settings(version.clone());
    write(shared, list)?;
    for old in dropped.into_iter().filter(|f| plain(f)) {
        if let Err(e) = fsx::remove_if_present(&shared.settings_version(addon, &old)) {
            log!("old settings version {old} of {addon} not deleted: {e}");
        }
    }
    Ok(version)
}

/// Version `n` of an addon's settings, read and checked against its fingerprint.
pub fn settings_bytes(shared: &Shared, version: &SettingsVersion) -> io::Result<Bundle> {
    if !plain(&version.file) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("settings version {} has an odd file name", version.n)));
    }
    let zip = fs::read(shared.settings_version(&version.addon, &version.file))?;
    let bundle = Bundle::from_zip_for(&zip, &version.addon)?;
    if bundle.fingerprint().as_deref() != Some(version.sha256.as_str()) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("settings version {} isn't what was sent", version.n)));
    }
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    fn agreed(release: &str) -> Agreed {
        Agreed { release: Some(release.into()), on: true, off: Vec::new(), pinned: false, settings_per_pc: false }
    }

    fn never() -> bool {
        panic!("no tie to settle here")
    }

    #[test]
    fn three_ways_like_saves() {
        let (a, b) = (agreed("a"), agreed("b"));
        // Same everywhere.
        assert_eq!(step(&Here::Has(a.clone()), Some(&a), Some(&a), never), Step::InStep);
        // Changed here only: it goes up. Changed in the list only: this PC follows.
        assert_eq!(step(&Here::Has(b.clone()), Some(&a), Some(&a), never), Step::Send);
        assert_eq!(step(&Here::Has(a.clone()), Some(&a), Some(&b), never), Step::Follow);
        // A switch is a change too.
        let off = Agreed { on: false, ..a.clone() };
        assert_eq!(step(&Here::Has(off.clone()), Some(&a), Some(&a), never), Step::Send);
        // New here, or the list was emptied: this PC seeds it.
        assert_eq!(step(&Here::Has(a.clone()), None, None, never), Step::Send);
        assert_eq!(step(&Here::Has(a.clone()), Some(&a), None, never), Step::Send);
        // New in the list: this PC installs it.
        assert_eq!(step(&Here::Missing, None, Some(&a), never), Step::Follow);
        assert_eq!(step(&Here::Missing, None, None, never), Step::Nothing);
    }

    #[test]
    fn removals_follow_only_when_the_player_removed_it() {
        let a = agreed("a");
        let gone = Agreed::removed();
        // Removed here with the Satchel: the removal goes up, then they agree.
        assert_eq!(step(&Here::Removed, Some(&a), Some(&a), never), Step::Send);
        assert_eq!(step(&Here::Removed, Some(&a), Some(&gone), never), Step::InStep);
        // Removed on another PC: this PC removes it too.
        assert_eq!(step(&Here::Has(a.clone()), Some(&a), Some(&gone), never), Step::Follow);
        // Lost here (a record gone, a folder deleted by hand): it comes back from the list.
        assert_eq!(step(&Here::Missing, Some(&a), Some(&a), never), Step::Follow);
        // Removed here while another PC updated it: the update wins.
        assert_eq!(step(&Here::Removed, Some(&a), Some(&agreed("b")), never), Step::Follow);
        // Updated here while another PC removed it: it stays, and goes up.
        assert_eq!(step(&Here::Has(agreed("b")), Some(&a), Some(&gone), never), Step::Send);
        // A removal this PC never had: nothing to do.
        assert_eq!(step(&Here::Missing, None, Some(&gone), never), Step::Nothing);
    }

    #[test]
    fn both_moved_on_the_tie_breaker_decides() {
        let (a, b, c) = (agreed("a"), agreed("b"), agreed("c"));
        assert_eq!(step(&Here::Has(b.clone()), Some(&a), Some(&c), || true), Step::Send);
        assert_eq!(step(&Here::Has(b.clone()), Some(&a), Some(&c), || false), Step::Follow);
        // First run: both have it, no B.
        assert_eq!(step(&Here::Has(b.clone()), None, Some(&c), || false), Step::Follow);
        // First run: removed elsewhere before this PC joined: every addon any PC has stays.
        assert_eq!(step(&Here::Has(b), None, Some(&Agreed::removed()), never), Step::Send);
    }

    #[test]
    fn the_tie_breaker() {
        let older = agreed("old");
        let newer = agreed("new");
        let kept_older = Agreed { pinned: true, ..older.clone() };
        // First run: the newer release wins, kept or not.
        assert!(here_wins(&newer, &older, true, || Some(Ordering::Greater)));
        assert!(!here_wins(&kept_older, &newer, true, || Some(Ordering::Less)));
        // Later: "Keep this version" wins, then the newer release.
        assert!(here_wins(&kept_older, &newer, false, || Some(Ordering::Less)));
        assert!(!here_wins(&newer, &kept_older, false, || Some(Ordering::Greater)));
        assert!(here_wins(&newer, &older, false, || Some(Ordering::Greater)));
        // No way to tell, or the same release with a switch changed on both: the list's.
        assert!(!here_wins(&newer, &older, false, || None));
        let off = Agreed { on: false, ..older.clone() };
        assert!(!here_wins(&off, &older, false, || panic!("not asked")));
    }

    #[test]
    fn settings_follow_the_saves_rules() {
        let here_newer = || true;
        let there_newer = || false;
        assert_eq!(settings_step(Some("a"), Some("a"), Some(("a", 1)), never), SettingsStep::InStep { adopt: None });
        assert_eq!(settings_step(Some("b"), Some("a"), Some(("a", 1)), never), SettingsStep::Send);
        assert_eq!(settings_step(Some("a"), Some("a"), Some(("c", 2)), never), SettingsStep::BringIn { n: 2, keep_mine: false });
        // Both changed since they agreed: the player picks.
        assert_eq!(settings_step(Some("b"), Some("a"), Some(("c", 2)), never), SettingsStep::Ask { nas: 2 });
        // A first look with different settings: the PC played last wins, the other is kept.
        assert_eq!(settings_step(Some("b"), None, Some(("c", 2)), here_newer), SettingsStep::Send);
        assert_eq!(settings_step(Some("b"), None, Some(("c", 2)), there_newer), SettingsStep::BringIn { n: 2, keep_mine: true });
        // No settings here yet (a fresh install): the list's come in. None anywhere: nothing.
        assert_eq!(settings_step(None, None, Some(("c", 2)), never), SettingsStep::BringIn { n: 2, keep_mine: false });
        assert_eq!(settings_step(None, None, None, never), SettingsStep::InStep { adopt: None });
        assert_eq!(settings_step(Some("b"), None, None, never), SettingsStep::Send);
    }

    fn entry(id: &str, pcs: &[&str]) -> Entry {
        Entry {
            id: id.into(),
            name: id.into(),
            badge: Badge::Verified,
            repo: None,
            release: Some(Shipped {
                sha256: "z".into(),
                tag: Some("v1".into()),
                file: None,
                version: Some("1".into()),
                url: "https://example.invalid/z.zip".into(),
                released: None,
                size: 1,
            }),
            folders: vec![id.into()],
            on: true,
            off: Vec::new(),
            pinned: false,
            settings_per_pc: false,
            keep_settings: false,
            pcs: pcs.iter().map(|p| p.to_string()).collect(),
            pc: pcs.first().unwrap_or(&"PC 1").to_string(),
            at: Utc::now(),
        }
    }

    /// Survivor UI as installed on a PC: two folders, the release `entry` carries.
    fn survivor_ui() -> Record {
        Record {
            id: "survivor-ui".into(),
            name: "Survivor UI".into(),
            badge: Badge::Verified,
            repo: None,
            tag: Some("v1".into()),
            file: None,
            version: Some("1".into()),
            sha256: Some("z".into()),
            folders: [("SurvivorUI".to_string(), "p1".to_string()), ("SurvivorPlates".to_string(), "p2".to_string())].into(),
            installed: Utc::now(),
            released: None,
            pinned: false,
            ask: false,
            url: None,
            local_only: false,
            settings_per_pc: false,
            earlier: Vec::new(),
        }
    }

    #[test]
    fn one_folder_off_is_its_own_state() {
        let record = survivor_ui();
        let plates_off = Agreed::of(&record, vec!["SurvivorPlates".into()]);
        assert!(!plates_off.on);
        assert_eq!(plates_off.off, ["survivorplates"]);
        let all_on = Agreed::of(&record, Vec::new());
        let all_off = Agreed::of(&record, vec!["SurvivorUI".into(), "SurvivorPlates".into()]);
        assert!(all_on.on && !all_off.on);
        assert_ne!(plates_off, all_on);
        assert_ne!(plates_off, all_off);

        // In the list: written by 0.5.1, it reads back the same.
        let folders = vec!["SurvivorUI".to_string(), "SurvivorPlates".to_string()];
        let listed = |on: bool, off: &[&str]| Entry {
            folders: folders.clone(),
            on,
            off: off.iter().map(|f| f.to_string()).collect(),
            ..entry("survivor-ui", &["PC 1"])
        };
        assert_eq!(listed(false, &["survivorplates"]).agreed(), plates_off);
        // Written before 0.5.1: one switch, and off was every folder off.
        assert_eq!(listed(false, &[]).agreed().off, ["survivorplates", "survivorui"]);
        assert_eq!(listed(false, &[]).agreed(), all_off);
        assert_eq!(listed(true, &[]).agreed().off, Vec::<String>::new());
        assert_eq!(listed(true, &[]).agreed(), all_on);
    }

    #[test]
    fn an_off_from_before_is_every_folder() {
        let record = survivor_ui();
        let folders: Vec<String> = record.folders.keys().cloned().collect();
        let plates_off = Agreed::of(&record, vec!["SurvivorPlates".into()]);
        let all_off = Agreed::of(&record, folders.clone());
        // What a Satchel from before 0.5.1 kept in its memory, or wrote in the list, for an addon
        // with a folder off.
        let before = Agreed { on: false, off: Vec::new(), ..plates_off.clone() };
        assert_eq!(before.clone().with_folders(&folders), all_off);
        for unchanged in [plates_off.clone(), all_off.clone(), Agreed::of(&record, Vec::new()), Agreed::removed()] {
            assert_eq!(unchanged.clone().with_folders(&folders), unchanged, "only an off from before changes");
        }
        // The PC that turned only Survivor Plates off before 0.5.1 (the list then turned both off
        // on the other PCs): read in folders, its own state goes up, and the others follow it.
        let base = before.with_folders(&folders);
        assert_eq!(step(&Here::Has(plates_off), Some(&base), Some(&all_off), never), Step::Send);
    }

    #[test]
    fn an_addon_nobody_shares_leaves_the_list() {
        let mut list = List::default();
        list.put(entry("Solo", &["PC 1"]));
        list.put(entry("Both", &["PC 1"]));
        list.join("Both", "PC 2");
        list.join("Both", "PC 2");
        assert_eq!(list.get("Both").unwrap().pcs, ["PC 1", "PC 2"]);
        // PC 1 keeps Solo to itself before any other PC took it: it leaves the list, removed nowhere.
        list.leave("Solo", "PC 1");
        assert!(list.get("Solo").is_none());
        // PC 2 has Both too: it stays for PC 2.
        list.leave("Both", "PC 1");
        assert_eq!(list.get("Both").unwrap().pcs, ["PC 2"]);
        let rev = list.rev;
        list.leave("Both", "PC 3");
        assert_eq!(list.rev, rev, "nothing changed, no new rev");
    }

    #[test]
    fn settings_files_by_addon_and_character() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("saved");
        fs::create_dir_all(saved.join("Realm-Hero")).unwrap();
        fs::write(saved.join("LuckyOdds.lua"), "LuckyOddsDB = {}").unwrap();
        fs::write(saved.join("Realm-Hero").join("LuckyOdds.lua"), "LuckyOddsChar = {}").unwrap();
        fs::write(saved.join("Other.lua"), "x").unwrap();
        fs::write(saved.join("LuckyOdds.lua.bak"), "old").unwrap();
        let folders = vec!["LuckyOdds".to_string()];

        let bundle = Bundle::read(&saved, &folders).unwrap();
        assert_eq!(bundle.files.keys().collect::<Vec<_>>(), ["LuckyOdds.lua", "Realm-Hero/LuckyOdds.lua"]);
        assert!(bundle.newest.is_some());
        let print = bundle.fingerprint().unwrap();
        assert_eq!(Bundle::read(&saved, &folders).unwrap().fingerprint().unwrap(), print, "the same files, the same fingerprint");
        assert_eq!(Bundle::read(&saved, &["Missing".to_string()]).unwrap().fingerprint(), None);

        // Through a zip and back, unchanged.
        let back = Bundle::from_zip(&bundle.to_zip().unwrap()).unwrap();
        assert_eq!(back.files, bundle.files);
        assert_eq!(back.fingerprint().unwrap(), print);

        // Placed on another PC: its own files go to the backup first, a character it alone had
        // goes (it's in the backup), the rest is left alone.
        let other = dir.path().join("other");
        fs::create_dir_all(other.join("Realm-Alt")).unwrap();
        fs::write(other.join("LuckyOdds.lua"), "mine").unwrap();
        fs::write(other.join("Realm-Alt").join("LuckyOdds.lua"), "alt").unwrap();
        fs::write(other.join("Other.lua"), "untouched").unwrap();
        let backup = dir.path().join("backup");
        bundle.place(&other, &folders, &backup).unwrap();
        assert_eq!(Bundle::read(&other, &folders).unwrap().fingerprint().unwrap(), print);
        assert!(!other.join("Realm-Alt").join("LuckyOdds.lua").exists());
        assert_eq!(fs::read_to_string(other.join("Other.lua")).unwrap(), "untouched");
        assert_eq!(fs::read_to_string(backup.join("LuckyOdds.lua")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(backup.join("Realm-Alt").join("LuckyOdds.lua")).unwrap(), "alt");
    }

    #[test]
    fn a_settings_zip_with_anything_else_is_refused() {
        let zip_with = |name: &str| {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap().into_inner()
        };
        for name in ["../evil.lua", "a/b/c.lua", "tool.exe", "Meter.txt", "x/../y.lua", "C:/x.lua"] {
            assert!(Bundle::from_zip(&zip_with(name)).is_err(), "{name}");
        }
        assert!(Bundle::from_zip(&zip_with("Realm-Hero/Meter.lua")).is_ok());
    }

    #[test]
    fn settings_versions_on_the_nas() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        let mut bundle = Bundle::default();
        bundle.files.insert("Meter.lua".into(), b"MeterDB = { 1 }".to_vec());
        let now = Utc::now();
        let mut list = read(&shared).unwrap();
        let first = send_settings(&shared, &mut list, "meter", "PC 1", &bundle, None, now).unwrap();
        let second = send_settings(&shared, &mut list, "meter", "PC 1", &bundle, None, now).unwrap();
        assert_eq!((first.n, second.n), (1, 2));
        assert_ne!(first.file, second.file, "two in the same second get their own files");
        let list = read(&shared).unwrap();
        assert_eq!(list.newest_settings("meter").unwrap().n, 2);
        assert_eq!(settings_bytes(&shared, &first).unwrap().files, bundle.files);

        // A version file changed on the NAS is refused.
        fs::write(shared.settings_version("meter", &first.file), Bundle { files: BTreeMap::from([("Meter.lua".into(), b"x".to_vec())]), newest: None }.to_zip().unwrap()).unwrap();
        assert!(settings_bytes(&shared, &first).is_err());
    }

    #[test]
    fn only_the_newest_settings_versions_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        let mut list = List::default();
        let mut first_file = String::new();
        for i in 0..(SETTINGS_KEEP + 2) {
            let mut bundle = Bundle::default();
            bundle.files.insert("Meter.lua".into(), format!("MeterDB = {{ {i} }}").into_bytes());
            let version = send_settings(&shared, &mut list, "meter", "PC 1", &bundle, None, Utc::now()).unwrap();
            if i == 0 {
                first_file = version.file;
            }
        }
        let mut other = Bundle::default();
        other.files.insert("Clock.lua".into(), b"ClockDB = {}".to_vec());
        send_settings(&shared, &mut list, "clock", "PC 1", &other, None, Utc::now()).unwrap();
        assert_eq!(list.settings.iter().filter(|v| v.addon == "meter").count(), SETTINGS_KEEP);
        assert_eq!(list.settings.iter().filter(|v| v.addon == "clock").count(), 1);
        assert!(!shared.settings_version("meter", &first_file).exists());
    }

    #[test]
    fn a_version_file_name_that_leads_elsewhere_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        let odd = SettingsVersion {
            n: 1,
            addon: "meter".into(),
            pc: "PC 1".into(),
            file: "../../addons.json".into(),
            sha256: "x".into(),
            bytes: 1,
            saved: Utc::now(),
            written: None,
            note: None,
            files: Vec::new(),
        };
        assert_eq!(settings_bytes(&shared, &odd).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(plain("2026-10-30 08-00-00 (PC 1) v3.zip"));
        for name in ["", ".", "..", "a/b.zip", "a\\b.zip", "C:x.zip"] {
            assert!(!plain(name), "{name}");
        }
    }

    #[test]
    fn a_list_from_a_newer_satchel_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        fs::create_dir_all(shared.addons_dir()).unwrap();
        let list = List { newest_satchel: Some("0.5.0".into()), ..List::default() };
        fs::write(shared.addons_list(), serde_json::to_vec(&list).unwrap()).unwrap();
        assert!(read_as(&shared, Some("0.4.1")).is_err_and(|e| gate::newer_satchel(&e).is_some()));
        assert_eq!(read_as(&shared, Some("0.5.0")).unwrap(), list);
        // A list from before the gate (no stamp) reads everywhere.
        fs::write(shared.addons_list(), serde_json::to_vec(&List::default()).unwrap()).unwrap();
        assert_eq!(read_as(&shared, Some("0.4.1")).unwrap(), List::default());
    }

    #[test]
    fn an_unreadable_list_is_an_error_and_a_missing_one_empty() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        assert_eq!(read(&shared).unwrap(), List::default());
        fs::create_dir_all(shared.addons_dir()).unwrap();
        fs::write(shared.addons_list(), b"{ not json").unwrap();
        assert_eq!(read(&shared).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
