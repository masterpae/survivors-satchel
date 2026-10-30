//! What the Satchel installed on this PC and where each addon came from (`addons.json` in Local
//! AppData): the release, the zip's checksum, each folder's fingerprint at install time (so a
//! later change on disk shows), and the earlier versions kept for going back.

use crate::fsx;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// Who vouches for an addon; always shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Badge {
    /// In the catalog of addons the game's devs reviewed.
    Verified,
    /// Added by the player from a GitHub link: nobody reviewed it.
    Added,
    /// Imported from a zip file: no source to update from.
    File,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The catalog id for a verified addon, `owner/repo` for one added by link, `file:<folder>`
    /// (its main folder) for an import, so a newer zip of the same addon replaces it.
    pub id: String,
    pub name: String,
    pub badge: Badge,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The release file it came from. For an addon added by link, the next release's file is the
    /// one named like it, its numbers aside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// SHA-256 of the zip it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Its folders in AddOns, each with its fingerprint when installed.
    pub folders: BTreeMap<String, String>,
    pub installed: DateTime<Utc>,
    /// When its release was published, when that's known: a release seen only on GitHub's page,
    /// or a zip from elsewhere, has no true date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<DateTime<Utc>>,
    /// "Keep this version": no updates until lifted.
    #[serde(default)]
    pub pinned: bool,
    /// Updates ask first.
    #[serde(default)]
    pub ask: bool,
    /// Where its release file downloads from, for the other PCs; `None` for an imported zip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// "Only on this PC": this PC's copy isn't shared, and the other PCs' changes don't touch it.
    #[serde(default)]
    pub local_only: bool,
    /// "Settings per PC": every PC keeps its own settings for it (a switch shared by every PC).
    #[serde(default)]
    pub settings_per_pc: bool,
    /// Earlier versions in the backups, newest first.
    #[serde(default)]
    pub earlier: Vec<Kept>,
}

/// An earlier version kept in the backups.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<DateTime<Utc>>,
    /// Where it downloads from, for the other PCs once it's back in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub folders: BTreeMap<String, String>,
    /// Its folder in the Satchel's addon backups.
    pub backup: String,
    pub kept: DateTime<Utc>,
}

impl Record {
    /// The version to show: the release's, else the `.toc`'s.
    pub fn shown_version(&self) -> String {
        self.version.clone().or_else(|| self.tag.clone()).unwrap_or_else(|| "unknown version".into())
    }
}

/// Removals kept for the Removed list, newest first.
pub const GONE_KEEP: usize = 30;

/// An addon removed on this PC, for the Removed list: as it was, and where its settings went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gone {
    pub record: Record,
    pub at: DateTime<Utc>,
    /// Its settings stayed in the game's folder; else they went with it (a copy is in `backup`).
    pub settings_kept: bool,
    /// Its folder in the Satchel's addon backups, which holds a copy of its settings.
    pub backup: String,
    /// The PC it was removed on, when this PC followed another one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub format: u32,
    pub addons: Vec<Record>,
    /// Addons removed here, newest first: one each, the newest `GONE_KEEP`.
    #[serde(default)]
    pub removed: Vec<Gone>,
}

impl Default for Store {
    fn default() -> Self {
        Store { format: 1, addons: Vec::new(), removed: Vec::new() }
    }
}

impl Store {
    pub fn load(path: &Path) -> io::Result<Store> {
        match fsx::read_optional(path)? {
            None => Ok(Store::default()),
            Some(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        fsx::write_atomic(path, &serde_json::to_vec_pretty(self).map_err(io::Error::other)?)
    }

    pub fn get(&self, id: &str) -> Option<&Record> {
        self.addons.iter().find(|r| r.id == id)
    }

    /// Records a removal: the newest one per addon, the newest `GONE_KEEP` in all.
    pub fn gone(&mut self, gone: Gone) {
        self.removed.retain(|g| g.record.id != gone.record.id);
        self.removed.insert(0, gone);
        self.removed.truncate(GONE_KEEP);
    }

    /// A removal by its addon's id.
    pub fn removal(&self, id: &str) -> Option<&Gone> {
        self.removed.iter().find(|g| g.record.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Record> {
        self.addons.iter_mut().find(|r| r.id == id)
    }

    /// The addon that owns a folder in AddOns (folder names compare without case).
    pub fn owner_of(&self, folder: &str) -> Option<&Record> {
        self.addons.iter().find(|r| r.folders.keys().any(|f| f.eq_ignore_ascii_case(folder)))
    }

    /// Adds the record, or replaces the one with its id.
    pub fn put(&mut self, record: Record) {
        match self.addons.iter_mut().find(|r| r.id == record.id) {
            Some(slot) => *slot = record,
            None => self.addons.push(record),
        }
        self.addons.sort_by_key(|r| r.name.to_lowercase());
    }

    pub fn remove(&mut self, id: &str) -> Option<Record> {
        let at = self.addons.iter().position(|r| r.id == id)?;
        Some(self.addons.remove(at))
    }
}
