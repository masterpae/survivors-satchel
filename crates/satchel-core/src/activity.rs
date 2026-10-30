//! "Recent on this PC": a short local log of sessions, sends and bring-ins.

use crate::fsx;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

const KEEP: usize = 200;

/// What an entry is about; the list shows an icon for each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Sync,
    Addon,
    Game,
    Playtime,
    Warning,
    Satchel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub at: DateTime<Utc>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
}

impl Entry {
    /// Its kind. Entries written before 0.2 carry none: a session is "Played …", the rest sync.
    pub fn shown_kind(&self) -> Kind {
        self.kind.unwrap_or(if self.text.starts_with("Played") { Kind::Game } else { Kind::Sync })
    }
}

pub fn add(path: &Path, at: DateTime<Utc>, kind: Kind, text: impl Into<String>) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let line = serde_json::to_string(&Entry { at, text: text.into(), kind: Some(kind) }).map_err(io::Error::other)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    drop(file);
    // Keep the file small: past 2 × KEEP lines, only the newest KEEP stay.
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > 2 * KEEP {
        let kept = lines[lines.len() - KEEP..].join("\n") + "\n";
        fsx::write_atomic(path, kept.as_bytes())?;
    }
    Ok(())
}

/// The newest `count` entries, newest first.
pub fn recent(path: &Path, count: usize) -> io::Result<Vec<Entry>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(text.lines().rev().filter_map(|l| serde_json::from_str(l).ok()).take(count).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_and_stays_small() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("activity.jsonl");
        for i in 0..(2 * KEEP + 1) {
            add(&path, Utc::now(), Kind::Sync, format!("entry {i}")).unwrap();
        }
        let newest = recent(&path, 3).unwrap();
        let texts: Vec<&str> = newest.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["entry 400", "entry 399", "entry 398"]);
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), KEEP);
    }

    #[test]
    fn entries_from_before_kinds_are_told_by_their_words() {
        let old: Entry = serde_json::from_str(r#"{"at":"2026-10-29T19:54:52Z","text":"Played · session 29 m"}"#).unwrap();
        assert_eq!(old.shown_kind(), Kind::Game);
        let old: Entry = serde_json::from_str(r#"{"at":"2026-10-29T19:54:52Z","text":"Sent version 9 · session 29 m"}"#).unwrap();
        assert_eq!(old.shown_kind(), Kind::Sync);
        let new: Entry = serde_json::from_str(r#"{"at":"2026-10-29T19:54:52Z","text":"AZS Spell Meter 1.34 installed","kind":"addon"}"#).unwrap();
        assert_eq!(new.shown_kind(), Kind::Addon);
    }
}
