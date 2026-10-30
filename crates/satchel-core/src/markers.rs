//! "In use" markers: one small file per PC while it plays, refreshed by a heartbeat.
//! They warn, they never block.

use crate::fsx;
use crate::paths::Shared;
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;

/// A marker without a heartbeat for this long was left by a PC that crashed or was switched off.
pub const STALE_AFTER_SECS: i64 = 180;
/// How often a playing PC refreshes its marker.
pub const HEARTBEAT_SECS: i64 = 60;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InUse {
    pub pc: String,
    pub started: DateTime<Utc>,
    pub heartbeat: DateTime<Utc>,
    /// Fingerprint of the checkpoint in `sync\live\<PC>.sav`, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// When that checkpoint was taken (from 0.4.2 on). The heartbeat goes on after it, through
    /// the minutes a session takes to close, so only this time says how old the checkpoint is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpointed: Option<DateTime<Utc>>,
}

/// Another PC's marker.
#[derive(Clone, Debug, PartialEq)]
pub struct Other {
    pub marker: InUse,
    /// No heartbeat for 3 minutes: that PC stopped during a session.
    pub stale: bool,
}

pub fn write(shared: &Shared, marker: &InUse) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(marker).map_err(io::Error::other)?;
    fsx::write_atomic(&shared.in_use(&marker.pc), &bytes)
}

pub fn clear(shared: &Shared, pc: &str) -> io::Result<()> {
    fsx::remove_if_present(&shared.in_use(pc))
}

/// The other PCs' markers; unreadable marker files are skipped. A marker whose PC differs from this
/// one only in case is this PC's own, from before its name changed case: Windows names files
/// without case.
pub fn others(shared: &Shared, pc: &str, now: DateTime<Utc>) -> io::Result<Vec<Other>> {
    let entries = match fs::read_dir(shared.in_use_dir()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let own = shared.in_use(pc);
    let mut found = Vec::new();
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if path == own || path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(marker) = serde_json::from_slice::<InUse>(&bytes) else { continue };
        if marker.pc.eq_ignore_ascii_case(pc) {
            continue;
        }
        let stale = now - marker.heartbeat > TimeDelta::seconds(STALE_AFTER_SECS);
        found.push(Other { marker, stale });
    }
    found.sort_by(|a, b| a.marker.pc.cmp(&b.marker.pc));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_other_pcs_and_tells_stale_from_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        let now = Utc::now();
        let marker = |pc: &str, age: i64| InUse {
            pc: pc.into(),
            started: now - TimeDelta::seconds(3600),
            heartbeat: now - TimeDelta::seconds(age),
            checkpoint: None,
            checkpointed: None,
        };
        write(&shared, &marker("PC 1", 10)).unwrap();
        write(&shared, &marker("PC 2", 30)).unwrap();
        write(&shared, &marker("Laptop", 600)).unwrap();
        // This PC's own, left while its name was written in another case.
        write(&shared, &marker("pc 1", 20)).unwrap();
        let others = others(&shared, "PC 1", now).unwrap();
        let summary: Vec<(&str, bool)> = others.iter().map(|o| (o.marker.pc.as_str(), o.stale)).collect();
        assert_eq!(summary, [("Laptop", true), ("PC 2", false)]);
        clear(&shared, "PC 2").unwrap();
        clear(&shared, "PC 2").unwrap();
        assert_eq!(super::others(&shared, "PC 1", now).unwrap().len(), 1);
    }

    #[test]
    fn reads_a_marker_written_before_checkpoints_had_a_time() {
        let json = br#"{"pc":"PC 1","started":"2026-10-30T10:00:00Z","heartbeat":"2026-10-30T10:05:00Z","checkpoint":"ab"}"#;
        let marker: InUse = serde_json::from_slice(json).unwrap();
        assert_eq!(marker.checkpoint.as_deref(), Some("ab"));
        assert_eq!(marker.checkpointed, None);
    }
}
