//! The NAS index (`sync\satchel-index.json`): numbered versions, so clocks never decide.

use crate::paths::Shared;
use crate::{fsx, gate, log, names, savefile};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub n: u64,
    pub pc: String,
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
    pub saved: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncIndex {
    pub format: u32,
    pub newest: u64,
    pub pcs: Vec<String>,
    /// Newest first.
    pub versions: Vec<Version>,
    /// The newest Satchel that wrote the index (since 0.4.1): an older one stays out (`gate`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest_satchel: Option<String>,
    /// Made from the version files because the index file was missing; not yet written back.
    #[serde(skip)]
    pub rebuilt: bool,
}

impl Default for SyncIndex {
    fn default() -> Self {
        SyncIndex { format: 1, newest: 0, pcs: Vec::new(), versions: Vec::new(), newest_satchel: None, rebuilt: false }
    }
}

impl SyncIndex {
    pub fn newest_version(&self) -> Option<&Version> {
        self.versions.iter().max_by_key(|v| v.n)
    }

    pub fn get(&self, n: u64) -> Option<&Version> {
        self.versions.iter().find(|v| v.n == n)
    }

    pub fn find_sha(&self, sha256: &str) -> Option<&Version> {
        self.versions.iter().filter(|v| v.sha256 == sha256).max_by_key(|v| v.n)
    }

    /// Records a version that was just written; returns it with its number.
    pub fn push(&mut self, mut version: Version) -> Version {
        self.newest = self.versions.iter().map(|v| v.n).max().unwrap_or(0).max(self.newest) + 1;
        version.n = self.newest;
        if !self.pcs.contains(&version.pc) {
            self.pcs.push(version.pc.clone());
        }
        self.versions.insert(0, version.clone());
        version
    }
}

/// Reads the index. A missing index with version files beside it is rebuilt from them, since a
/// folder tidied by hand must not restart the history; with none it is an empty NAS. An
/// unreadable index is an error, never silently replaced. One written by a newer Satchel is
/// refused (`gate`).
pub fn read(shared: &Shared) -> io::Result<SyncIndex> {
    read_as(shared, gate::this())
}

fn read_as(shared: &Shared, this: Option<&str>) -> io::Result<SyncIndex> {
    let path = shared.index();
    match fsx::read_optional(&path)? {
        None => rebuild(shared),
        Some(bytes) => {
            let index: SyncIndex = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))?;
            gate::check_as(this, index.newest_satchel.as_deref())?;
            Ok(index)
        }
    }
}

/// An index made from the version files in `sync\`: their names give the time, the PC and the
/// order (so numbers can differ from the lost index's), their bytes the fingerprints. A file that
/// isn't a save is left out.
fn rebuild(shared: &Shared) -> io::Result<SyncIndex> {
    let entries = match fs::read_dir(shared.sync_dir()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(SyncIndex::default()),
        Err(e) => return Err(e),
    };
    let mut found: Vec<(DateTime<Utc>, u64, String, String)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.metadata().is_ok_and(|m| m.is_file()))
        .filter_map(|entry| {
            let file = entry.file_name().to_string_lossy().into_owned();
            let (saved, pc, same_second) = names::parse_version_file(&file)?;
            Some((saved, same_second, pc, file))
        })
        .collect();
    found.sort();
    let mut index = SyncIndex::default();
    for (saved, _, pc, file) in found {
        let bytes = fs::read(shared.version(&file))?;
        if let Some(why) = savefile::problem(&bytes) {
            log!("rebuilding the index: {file} left out, it {why}");
            continue;
        }
        index.push(Version {
            n: 0,
            pc,
            sha256: fsx::fingerprint(&bytes),
            bytes: bytes.len() as u64,
            saved,
            game: None,
            note: Some("index rebuilt".into()),
            file,
        });
    }
    index.rebuilt = !index.versions.is_empty();
    Ok(index)
}

/// Writes the index, stamped with this Satchel's version when it is the newest to write it.
pub fn write(shared: &Shared, index: &SyncIndex) -> io::Result<()> {
    let stamped = SyncIndex { newest_satchel: gate::stamp(index.newest_satchel.as_deref()), ..index.clone() };
    let bytes = serde_json::to_vec_pretty(&stamped).map_err(io::Error::other)?;
    fsx::write_atomic(&shared.index(), &bytes)
}

/// One PC at a time changes the index. The lock is a file created exclusively, which is
/// atomic on a network share too; a lock older than a minute was left by a PC that stopped.
/// Such a lock is moved aside under a name of this PC's own, then deleted there: of two PCs that
/// both find it stale only one can move it, and the other then finds that one's new lock and
/// waits. Deleted where it lies, the second PC could delete the first one's new lock, and both
/// would write.
pub struct IndexLock {
    path: PathBuf,
}

pub const LOCK_STALE_AFTER: Duration = Duration::from_secs(60);

impl IndexLock {
    pub fn acquire(shared: &Shared, pc: &str, wait: Duration) -> io::Result<IndexLock> {
        IndexLock::acquire_at(shared.lock(), pc, wait)
    }

    /// The same kind of lock on another file (the shared addon list has its own).
    pub fn acquire_at(path: PathBuf, pc: &str, wait: Duration) -> io::Result<IndexLock> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let deadline = Instant::now() + wait;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{pc} {}", Utc::now().to_rfc3339());
                    return Ok(IndexLock { path });
                }
                // Windows refuses to create a file another PC is deleting at that very moment
                // (access denied): that is a lock being let go, so it is waited for like one in use.
                Err(e) if matches!(e.kind(), io::ErrorKind::AlreadyExists | io::ErrorKind::PermissionDenied) => {
                    let denied = e.kind() == io::ErrorKind::PermissionDenied;
                    if stale(&path) && break_stale(&path, pc) {
                        continue;
                    }
                    if Instant::now() >= deadline {
                        // Denied to the end: a folder this PC can't write, not a lock being let go.
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            if denied { "the network folder can't be written (access denied)" } else { "another PC is writing to the network folder" },
                        ));
                    }
                    thread::sleep(Duration::from_millis(200));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Whether the lock at `path` is older than `LOCK_STALE_AFTER`; one that can't be read isn't.
/// The file's time comes from the network folder's clock and is held against this PC's: a PC
/// running more than a minute ahead would break a lock in use, so the clocks have to agree, as
/// Windows keeps them.
fn stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().unwrap_or_default() > LOCK_STALE_AFTER)
        .unwrap_or(false)
}

/// Breaks the stale lock at `path`: moves it beside itself, under a name of this PC's own, and
/// deletes it there. What was moved is looked at once more first, now that no other PC can touch
/// it: a fresh lock means another PC broke the stale one and took the lock in the moment since this
/// PC looked, and it is put back. One case stays open: a third PC that takes the lock in the moment
/// it is aside has its own lock replaced by the one put back, and two PCs write, which takes three
/// PCs within the same few milliseconds. `true` when the lock is gone (broken here, or by another
/// PC first): try again at once. `false`: wait, as for any lock in use.
fn break_stale(path: &Path, pc: &str) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let aside = path.with_file_name(format!("{name}.broken-{}-{nanos}", names::safe(pc)));
    match fs::rename(path, &aside) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return true,
        // Something holds it in place: waiting ends in time, where trying again at once never would.
        Err(_) => return false,
    }
    if !stale(&aside) {
        if let Err(e) = fs::rename(&aside, path) {
            log!("another PC's lock, moved aside as stale, couldn't be put back from {}: {e}", aside.display());
        }
        return false;
    }
    if let Err(e) = fs::remove_file(&aside) {
        log!("a stale lock was broken but {} couldn't be deleted: {e}", aside.display());
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::time::SystemTime;

    fn version(pc: &str) -> Version {
        Version {
            n: 0,
            pc: pc.into(),
            file: format!("{pc}.sav"),
            sha256: "x".into(),
            bytes: 1,
            saved: Utc::now(),
            game: None,
            note: None,
        }
    }

    #[test]
    fn missing_index_is_empty_and_corrupt_one_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        assert_eq!(read(&shared).unwrap(), SyncIndex::default());
        fs::create_dir_all(shared.sync_dir()).unwrap();
        fs::write(shared.index(), b"{ not json").unwrap();
        assert_eq!(read(&shared).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn an_index_from_a_newer_satchel_is_refused_and_an_older_one_reads_on() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        fs::create_dir_all(shared.sync_dir()).unwrap();
        let written = |stamp: &str| {
            let index = SyncIndex { newest_satchel: Some(stamp.into()), ..SyncIndex::default() };
            fs::write(shared.index(), serde_json::to_vec(&index).unwrap()).unwrap();
        };
        written("0.4.2");
        let refused = read_as(&shared, Some("0.4.1")).unwrap_err();
        assert!(gate::newer_satchel(&refused).is_some_and(|n| n.newer == "0.4.2"));
        assert!(read_as(&shared, Some("0.4.2")).is_ok());
        written("0.4.1");
        assert_eq!(read_as(&shared, Some("0.4.2")).unwrap().newest_satchel.as_deref(), Some("0.4.1"));
        // An index from before the gate (no stamp) reads everywhere.
        fs::write(shared.index(), br#"{"format":1,"newest":0,"pcs":[],"versions":[]}"#).unwrap();
        assert_eq!(read_as(&shared, Some("0.4.1")).unwrap(), SyncIndex::default());
    }

    #[test]
    fn push_numbers_versions_and_lists_pcs() {
        let mut index = SyncIndex::default();
        assert_eq!(index.push(version("PC 1")).n, 1);
        assert_eq!(index.push(version("PC 2")).n, 2);
        assert_eq!(index.push(version("PC 1")).n, 3);
        assert_eq!(index.newest, 3);
        assert_eq!(index.pcs, ["PC 1", "PC 2"]);
        assert_eq!(index.newest_version().unwrap().n, 3);
        assert_eq!(index.versions[0].n, 3, "newest first");
    }

    #[test]
    fn concurrent_writers_never_lose_a_version() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        let barrier = Arc::new(Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let (shared, barrier) = (shared.clone(), barrier.clone());
                thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..5 {
                        let _lock = IndexLock::acquire(&shared, &format!("PC {i}"), Duration::from_secs(20)).unwrap();
                        let mut index = read(&shared).unwrap();
                        index.push(version(&format!("PC {i}")));
                        write(&shared, &index).unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let index = read(&shared).unwrap();
        assert_eq!(index.versions.len(), 20);
        assert_eq!(index.newest, 20);
        assert!(!shared.lock().exists());
    }

    #[test]
    fn busy_lock_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        let _held = IndexLock::acquire(&shared, "PC 1", Duration::from_secs(1)).unwrap();
        let err = IndexLock::acquire(&shared, "PC 2", Duration::from_millis(300)).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    }

    /// The names in `dir`, sorted.
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn a_stale_lock_is_broken_and_nothing_is_left_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        fs::create_dir_all(shared.sync_dir()).unwrap();
        // Left by a PC that stopped two minutes ago.
        let mut left = fs::File::create(shared.lock()).unwrap();
        writeln!(left, "PC 1 2026-10-30T10:00:00+00:00").unwrap();
        left.set_modified(SystemTime::now() - Duration::from_secs(120)).unwrap();
        drop(left);

        let _lock = IndexLock::acquire(&shared, "PC 2", Duration::from_millis(300)).unwrap();
        assert!(fs::read_to_string(shared.lock()).unwrap().starts_with("PC 2 "), "the lock is PC 2's now");
        assert_eq!(names_in(&shared.sync_dir()), ["satchel-index.lock"], "the broken lock isn't left beside it");
    }

    #[test]
    fn a_fresh_lock_moved_aside_too_late_is_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared { folder: dir.path().into() };
        // PC 1 broke a stale lock and took its own; PC 2 found the stale one a moment before, and
        // moves the lock aside only now.
        let held = IndexLock::acquire(&shared, "PC 1", Duration::from_secs(1)).unwrap();
        let before = fs::read_to_string(shared.lock()).unwrap();
        assert!(!break_stale(&shared.lock(), "PC 2"), "PC 2 waits");
        assert_eq!(fs::read_to_string(shared.lock()).unwrap(), before, "PC 1's lock is back as it was");
        assert_eq!(names_in(&shared.sync_dir()), ["satchel-index.lock"]);

        // A lock another PC broke first is simply gone: PC 2 tries again at once.
        drop(held);
        assert!(break_stale(&shared.lock(), "PC 2"));
        assert!(names_in(&shared.sync_dir()).is_empty());
    }
}
