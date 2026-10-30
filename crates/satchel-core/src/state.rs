//! What this PC remembers between runs (`state.json`): above all B, its last sync.

use crate::fsx;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalState {
    /// B: the save this PC last sent or brought in.
    pub last_synced: Option<Synced>,
    /// A session ended while the NAS was unreachable; its save still has to go out.
    pub send_pending: bool,
    /// Fingerprint of the last checkpoint sent while playing.
    pub last_checkpoint: Option<String>,
    /// First start finished on this PC.
    pub setup_done: bool,
    /// The Satchel version whose What's new the player has opened, or that was the first to run
    /// here: a newer one shows "Updated to …" until its What's new is opened.
    pub seen_version: Option<String>,
    /// Every name this PC has had in Settings, so a name it used before isn't refused as another PC's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pc_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Synced {
    pub sha256: String,
    pub n: u64,
    pub at: DateTime<Utc>,
}

impl LocalState {
    pub fn load(path: &Path) -> io::Result<LocalState> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(LocalState::default()),
            Err(e) => Err(e),
        }
    }

    /// Loads from `path`; when nothing is there yet but `old` exists (where an earlier Satchel
    /// kept it), that memory moves to `path` and `old` is renamed `….moved`, never read again.
    pub fn load_or_move(path: &Path, old: &Path) -> io::Result<LocalState> {
        if !path.exists() && old.exists() {
            let state = LocalState::load(old)?;
            state.save(path)?;
            fs::rename(old, old.with_extension("json.moved"))?;
            return Ok(state);
        }
        LocalState::load(path)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fsx::write_atomic(path, &bytes)
    }

    pub fn base(&self) -> Option<&str> {
        self.last_synced.as_ref().map(|s| s.sha256.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_moves_from_roaming_to_local_once() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("Roaming").join("SurvivorsSatchel").join("state.json");
        let new = dir.path().join("Local").join("SurvivorsSatchel").join("state.json");
        assert_eq!(LocalState::load_or_move(&new, &old).unwrap(), LocalState::default());
        assert!(!new.exists(), "nothing to move: nothing written");

        let mut state = LocalState { setup_done: true, ..LocalState::default() };
        state.last_synced = Some(Synced { sha256: "b".into(), n: 3, at: Utc::now() });
        state.save(&old).unwrap();
        assert_eq!(LocalState::load_or_move(&new, &old).unwrap(), state);
        assert!(new.exists());
        assert!(!old.exists());
        assert!(old.with_extension("json.moved").exists(), "the old file is kept, renamed");

        // From now on only the new place counts.
        let mut later = state.clone();
        later.send_pending = true;
        later.save(&new).unwrap();
        assert_eq!(LocalState::load_or_move(&new, &old).unwrap(), later);
    }

    #[test]
    fn a_memory_from_the_first_test_builds_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let json = br#"{"setup_done":true,"checks":{"export_format_matches":true,"writes_during_play":true,"swap_works":true,"swap_question_due":false}}"#;
        fs::write(&path, json).unwrap();
        assert_eq!(LocalState::load(&path).unwrap(), LocalState { setup_done: true, ..LocalState::default() });

        // The names this PC has had are kept; none, nothing is written.
        let named = LocalState { pc_names: vec!["PC 1".into(), "Laptop".into()], ..LocalState::default() };
        named.save(&path).unwrap();
        assert_eq!(LocalState::load(&path).unwrap(), named);
        LocalState::default().save(&path).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("pc_names"));
    }
}
