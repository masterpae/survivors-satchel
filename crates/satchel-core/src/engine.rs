//! The sync engine: checks, sends, brings in, keeps checkpoints, markers and playtime.
//!
//! Its promises: nothing on the NAS is ever overwritten (versions only accumulate); the live save
//! is only replaced after a safety copy, and only while the game is closed (the caller's part);
//! a send re-checks under the index lock, so two PCs never silently overwrite each other; an
//! empty or damaged save is never sent, checkpointed or brought in.

use crate::decide::{Decision, decide};
use crate::index::{self, IndexLock, SyncIndex, Version};
use crate::markers::{self, InUse, Other};
use crate::paths::{self, Local, Shared};
use crate::playtime::{self, Session};
use crate::settings::Settings;
use crate::state::{LocalState, Synced};
use crate::{fsx, log, names, savefile};
use chrono::{DateTime, Utc};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long to wait for another PC that is writing to the index.
pub const LOCK_WAIT: Duration = Duration::from_secs(15);
/// How long the shared folder gets to answer before it counts as unreachable.
pub const REACH_WAIT: Duration = Duration::from_secs(5);
/// The live save is read twice this far apart, and taken only when both reads agree.
pub const READ_GAP: Duration = Duration::from_millis(500);
const SAFETY_KEEP: usize = 30;
const COPIES_KEEP: usize = 30;

#[derive(Debug)]
pub enum SyncError {
    /// Sync is off, or has no folder it can use.
    NotConfigured,
    /// The shared folder doesn't answer.
    Offline,
    /// The NAS moved on while this PC was about to send: decide again.
    MovedOn,
    /// The player answered about a save or a version that is no longer the one shown: ask again.
    Changed,
    /// A save that is empty or damaged: it never travels. Says which and why.
    Damaged(String),
    /// A version or export the caller named isn't there (any more).
    Missing(String),
    Io(io::Error),
}

impl From<io::Error> for SyncError {
    fn from(e: io::Error) -> Self {
        SyncError::Io(e)
    }
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::NotConfigured => write!(f, "sync is off"),
            SyncError::Offline => write!(f, "the network folder doesn't answer"),
            SyncError::MovedOn => write!(f, "another PC sent a newer save meanwhile"),
            SyncError::Changed => write!(f, "something changed since you looked"),
            SyncError::Damaged(what) => write!(f, "{what}"),
            SyncError::Missing(what) => write!(f, "{what} is missing"),
            SyncError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SyncError {}

/// This PC's live save.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalSave {
    pub sha256: String,
    pub bytes: u64,
    pub modified: Option<DateTime<Utc>>,
    /// Why it can't be a save the game wrote (it is never sent), or `None`.
    pub problem: Option<String>,
}

/// What the player saw when asked to pick after two PCs moved on separately: this PC's save
/// (its fingerprint) and the NAS's newest version. The answer applies to exactly these.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub local: Option<String>,
    pub nas: u64,
}

impl Shown {
    /// `Changed` when this PC's save or the NAS's newest is no longer what was shown.
    fn still(&self, idx: &SyncIndex, local: Option<&[u8]>) -> Result<(), SyncError> {
        let newest = idx.newest_version().map(|v| v.n);
        let local = local.map(fsx::fingerprint);
        if newest == Some(self.nas) && local == self.local { Ok(()) } else { Err(SyncError::Changed) }
    }
}

/// Everything one check learns. Making it changes nothing.
#[derive(Clone, Debug)]
pub struct Outlook {
    pub local: Option<LocalSave>,
    pub index: SyncIndex,
    pub decision: Decision,
    pub others: Vec<Other>,
}

/// A `.sav` file outside the index: an export at the top of the shared folder, or (sync off) a
/// copy kept on this PC.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveFile {
    pub path: PathBuf,
    pub name: String,
    pub bytes: u64,
    pub modified: Option<DateTime<Utc>>,
}

/// How a bring-in ended: `characters.txt` now holds the version; `safety` names the safety copy
/// of what it held (`None`: there was no save).
#[derive(Clone, Debug, PartialEq)]
pub struct BroughtIn {
    pub version: Version,
    pub safety: Option<String>,
}

pub struct Engine {
    pub local: Local,
    pub settings: Settings,
    pub state: LocalState,
    /// The pause between the two reads of the live save (`READ_GAP`; tests use none).
    pub read_gap: Duration,
}

impl Engine {
    pub fn new(local: Local, settings: Settings, state: LocalState) -> Engine {
        Engine { local, settings, state, read_gap: READ_GAP }
    }

    pub fn pc(&self) -> &str {
        &self.settings.pc_name
    }

    /// The shared folder, when sync is on and its folder is one it can safely use.
    pub fn shared(&self) -> Option<Shared> {
        let folder = self.settings.sync_folder()?;
        if self.folder_problem().is_some() {
            return None;
        }
        Some(Shared { folder: folder.to_path_buf() })
    }

    /// Why the shared folder in Settings can't be used, if it can't: sync pauses until it is changed.
    pub fn folder_problem(&self) -> Option<String> {
        let folder = self.settings.sync_folder()?;
        paths::sync_folder_problem(folder, &self.local, self.settings.game_dir.as_deref())
    }

    fn reachable(&self) -> Result<Shared, SyncError> {
        let shared = self.shared().ok_or(SyncError::NotConfigured)?;
        if shared.reachable_within(REACH_WAIT) { Ok(shared) } else { Err(SyncError::Offline) }
    }

    pub fn save_state(&self) -> io::Result<()> {
        self.state.save(&self.local.state())
    }

    pub fn local_save(&self) -> io::Result<Option<LocalSave>> {
        let path = self.local.live_save();
        let Some(bytes) = fsx::read_optional(&path)? else { return Ok(None) };
        let modified = fs::metadata(&path).and_then(|m| m.modified()).ok().map(DateTime::<Utc>::from);
        Ok(Some(LocalSave {
            sha256: fsx::fingerprint(&bytes),
            bytes: bytes.len() as u64,
            modified,
            problem: savefile::problem(&bytes),
        }))
    }

    /// The live save for sending: read twice `read_gap` apart until both reads agree, since the
    /// game writes it while it runs and a read that overlaps a write can catch a torn file.
    fn read_live(&self) -> io::Result<Option<Vec<u8>>> {
        fsx::read_settled(&self.local.live_save(), self.read_gap)
    }

    /// Reads both sides and decides. Changes nothing, except that an index found missing is
    /// written back as rebuilt from the version files.
    pub fn outlook(&self, now: DateTime<Utc>) -> Result<Outlook, SyncError> {
        let shared = self.reachable()?;
        let mut index = index::read(&shared)?;
        if index.rebuilt {
            let _lock = IndexLock::acquire(&shared, self.pc(), LOCK_WAIT)?;
            index = index::read(&shared)?;
            if index.rebuilt {
                index::write(&shared, &index)?;
                log!("the NAS index was missing: rebuilt from {} version files", index.versions.len());
            }
        }
        let local = self.local_save()?;
        let decision = decide(
            local.as_ref().map(|l| l.sha256.as_str()),
            self.state.base(),
            index.newest_version().map(|v| (v.sha256.as_str(), v.n)),
        );
        let others = markers::others(&shared, self.pc(), now)?;
        Ok(Outlook { local, index, decision, others })
    }

    /// Records that this PC holds `version` (B becomes its fingerprint).
    pub fn adopt(&mut self, version: &Version, now: DateTime<Utc>) -> io::Result<()> {
        self.state.last_synced = Some(Synced { sha256: version.sha256.clone(), n: version.n, at: now });
        self.save_state()
    }

    /// Sends this PC's save as the next version. Under the index lock it decides again, so if
    /// another PC moved on meanwhile nothing is sent and the answer is `MovedOn`. With `kept`
    /// the player has chosen this PC's save over the NAS's: it is sent anyway, provided both
    /// are still what was shown (else `Changed`).
    pub fn send(
        &mut self,
        now: DateTime<Utc>,
        game: Option<String>,
        note: Option<String>,
        kept: Option<&Shown>,
    ) -> Result<Option<Version>, SyncError> {
        let shared = self.reachable()?;
        let Some(bytes) = self.read_live()? else { return Ok(None) };
        if let Some(why) = savefile::problem(&bytes) {
            return Err(SyncError::Damaged(format!("this PC's save {why}")));
        }
        let sha = fsx::fingerprint(&bytes);
        let _lock = IndexLock::acquire(&shared, self.pc(), LOCK_WAIT)?;
        let mut idx = index::read(&shared)?;
        if let Some(shown) = kept {
            shown.still(&idx, Some(&bytes))?;
        }
        match decide(Some(&sha), self.state.base(), idx.newest_version().map(|v| (v.sha256.as_str(), v.n))) {
            Decision::Send => {}
            Decision::InSync { adopt } => {
                if let Some(version) = adopt.and_then(|n| idx.get(n)).cloned() {
                    self.state.last_synced = Some(Synced { sha256: version.sha256, n: version.n, at: now });
                }
                self.state.send_pending = false;
                self.save_state()?;
                return Ok(None);
            }
            Decision::BringIn(_) | Decision::Split { .. } if kept.is_none() => return Err(SyncError::MovedOn),
            Decision::BringIn(_) | Decision::Split { .. } => {}
        }
        let pc = self.pc().to_string();
        let version = write_version(&shared, &mut idx, &pc, &bytes, now, game, note)?;
        self.state.last_synced = Some(Synced { sha256: sha, n: version.n, at: now });
        self.state.send_pending = false;
        self.save_state()?;
        log!("sent version {} ({} bytes)", version.n, version.bytes);
        Ok(Some(version))
    }

    /// Brings version `n` in: fetched and checked against its fingerprint, a safety copy of this
    /// PC's save made, then swapped in one move. The game must be closed.
    pub fn bring_in(&mut self, n: u64, now: DateTime<Utc>) -> Result<BroughtIn, SyncError> {
        let shared = self.reachable()?;
        let idx = index::read(&shared)?;
        let version = idx.get(n).cloned().ok_or_else(|| SyncError::Missing(format!("version {n}")))?;
        let bytes = fsx::read_verified(&shared.version(&version.file), &version.sha256)?;
        // Only a save this PC already synced is replaced without asking: if it changed since
        // the decision, the player must choose.
        if let Some(live) = fsx::read_optional(&self.local.live_save())? {
            let sha = fsx::fingerprint(&live);
            if self.state.base() != Some(sha.as_str()) && sha != version.sha256 {
                return Err(SyncError::MovedOn);
            }
        }
        self.place(bytes, &version, now)
    }

    /// The player picked the NAS's save after both sides moved on: this PC's save is first kept
    /// as a version, then the chosen one is made the newest and brought in. Both must still be
    /// what was shown (else `Changed`).
    pub fn use_nas(&mut self, shown: &Shown, now: DateTime<Utc>) -> Result<BroughtIn, SyncError> {
        let shared = self.reachable()?;
        let pc = self.pc().to_string();
        let n = shown.nas;
        let (chosen, bytes) = {
            let _lock = IndexLock::acquire(&shared, &pc, LOCK_WAIT)?;
            let mut idx = index::read(&shared)?;
            let mine = self.read_live()?;
            shown.still(&idx, mine.as_deref())?;
            let picked = idx.get(n).cloned().ok_or_else(|| SyncError::Missing(format!("version {n}")))?;
            let bytes = fsx::read_verified(&shared.version(&picked.file), &picked.sha256)?;
            if let Some(why) = savefile::problem(&bytes) {
                return Err(SyncError::Damaged(format!("version {n} {why}")));
            }
            // An empty or damaged save here isn't kept as a version; the safety copy keeps it.
            if let Some(mine) = mine
                && savefile::problem(&mine).is_none()
                && idx.find_sha(&fsx::fingerprint(&mine)).is_none()
            {
                let note = Some(format!("{pc}'s progress, kept when version {n} was chosen"));
                write_version(&shared, &mut idx, &pc, &mine, now, None, note)?;
            }
            let chosen = if idx.newest_version().map(|v| v.n) == Some(picked.n) {
                picked
            } else {
                let note = Some(format!("version {} from {}, chosen", picked.n, picked.pc));
                write_version(&shared, &mut idx, &pc, &bytes, now, picked.game.clone(), note)?
            };
            (chosen, bytes)
        };
        self.place(bytes, &chosen, now)
    }

    /// Makes an older version, or an export, this PC's progress again. It becomes the newest
    /// version first, so every PC follows. Safety copy first; the game must be closed.
    pub fn restore(&mut self, bytes: Vec<u8>, note: String, game: Option<String>, now: DateTime<Utc>) -> Result<BroughtIn, SyncError> {
        if let Some(why) = savefile::problem(&bytes) {
            return Err(SyncError::Damaged(format!("that save {why}")));
        }
        let shared = self.reachable()?;
        let pc = self.pc().to_string();
        let version = {
            let _lock = IndexLock::acquire(&shared, &pc, LOCK_WAIT)?;
            let mut idx = index::read(&shared)?;
            write_version(&shared, &mut idx, &pc, &bytes, now, game, Some(note))?
        };
        self.place(bytes, &version, now)
    }

    /// Sync off: puts a kept copy back as this PC's progress, after a safety copy.
    pub fn restore_copy(&mut self, path: &Path, now: DateTime<Utc>) -> Result<Option<String>, SyncError> {
        let bytes = fs::read(path)?;
        if let Some(why) = savefile::problem(&bytes) {
            return Err(SyncError::Damaged(format!("that copy {why}")));
        }
        let safety = self.safety_copy(now)?;
        fsx::write_atomic(&self.local.live_save(), &bytes)?;
        log!("restored {} (safety copy {})", path.display(), safety.as_deref().unwrap_or("none: there was no save"));
        Ok(safety)
    }

    /// The bytes of version `n`, checked.
    pub fn version_bytes(&self, n: u64) -> Result<(Version, Vec<u8>), SyncError> {
        let shared = self.reachable()?;
        let idx = index::read(&shared)?;
        let version = idx.get(n).cloned().ok_or_else(|| SyncError::Missing(format!("version {n}")))?;
        let bytes = fsx::read_verified(&shared.version(&version.file), &version.sha256)?;
        Ok((version, bytes))
    }

    fn place(&mut self, bytes: Vec<u8>, version: &Version, now: DateTime<Utc>) -> Result<BroughtIn, SyncError> {
        if let Some(why) = savefile::problem(&bytes) {
            return Err(SyncError::Damaged(format!("version {} {why}", version.n)));
        }
        let safety = self.safety_copy(now)?;
        fsx::write_atomic(&self.local.live_save(), &bytes)?;
        self.state.last_synced = Some(Synced { sha256: version.sha256.clone(), n: version.n, at: now });
        self.save_state()?;
        log!("brought in version {} from {} (safety copy {})", version.n, version.pc, safety.as_deref().unwrap_or("none: there was no save"));
        Ok(BroughtIn { version: version.clone(), safety })
    }

    /// Copies this PC's save into the game's Saves folder (the game's own Import can bring it
    /// back) and into the Satchel's safety folder; an empty or damaged one goes only to the
    /// latter. `None` when there is no save yet.
    fn safety_copy(&self, now: DateTime<Utc>) -> io::Result<Option<String>> {
        let Some(bytes) = fsx::read_optional(&self.local.live_save())? else { return Ok(None) };
        let file = names::safety_file(now);
        if savefile::problem(&bytes).is_none() {
            fsx::write_atomic(&self.local.game_saves().join(&file), &bytes)?;
        }
        fsx::write_atomic(&self.local.safety_dir().join(&file), &bytes)?;
        fsx::prune(&self.local.safety_dir(), SAFETY_KEEP)?;
        Ok(Some(file))
    }

    /// While the game runs: when the live save changed since the last checkpoint, copies it to
    /// `sync\live\<PC>.sav` and refreshes this PC's marker. Returns whether it wrote one.
    pub fn checkpoint(&mut self, marker: &mut InUse, now: DateTime<Utc>) -> Result<bool, SyncError> {
        let shared = self.reachable()?;
        let Some(bytes) = self.read_live()? else { return Ok(false) };
        if let Some(why) = savefile::problem(&bytes) {
            return Err(SyncError::Damaged(format!("this PC's save {why}")));
        }
        let sha = fsx::fingerprint(&bytes);
        if self.state.last_checkpoint.as_deref() == Some(sha.as_str()) {
            return Ok(false);
        }
        fsx::write_atomic(&shared.live(self.pc()), &bytes)?;
        marker.checkpoint = Some(sha.clone());
        marker.checkpointed = Some(now);
        marker.heartbeat = now;
        markers::write(&shared, marker)?;
        self.state.last_checkpoint = Some(sha);
        self.save_state()?;
        Ok(true)
    }

    pub fn write_marker(&self, marker: &InUse) -> Result<(), SyncError> {
        let shared = self.reachable()?;
        Ok(markers::write(&shared, marker)?)
    }

    pub fn clear_marker(&mut self) -> Result<(), SyncError> {
        let shared = self.reachable()?;
        markers::clear(&shared, self.pc())?;
        self.state.last_checkpoint = None;
        self.save_state()?;
        Ok(())
    }

    /// At a look, before this PC's save is sent: a power cut can take the game's last write with it
    /// and leave a save on disk older than this PC's own last checkpoint (`sync\live\<PC>.sav`, named
    /// by this PC's marker). Sent first, that save would become the newest version, and the
    /// checkpoint, older than it, would then go with the marker. So a checkpoint that isn't a version
    /// yet, isn't the save on disk, and was taken after that save was written (both by this PC's
    /// clock; with no save, or no time for it, the checkpoint counts as newer) becomes a version
    /// first, as another PC would promote it (`take_over_checkpoint`, which also removes the marker).
    /// The player then gets the pick between it and the save on disk, and both are kept. Otherwise
    /// `None`, and nothing changes.
    pub fn promote_own_checkpoint(&mut self, outlook: &Outlook, now: DateTime<Utc>) -> Result<Option<Version>, SyncError> {
        let shared = self.shared().ok_or(SyncError::NotConfigured)?;
        let Some(bytes) = fsx::read_optional(&shared.in_use(self.pc()))? else { return Ok(None) };
        let Ok(marker) = serde_json::from_slice::<InUse>(&bytes) else { return Ok(None) };
        let taken = marker.checkpointed.unwrap_or(marker.started);
        let local = outlook.local.as_ref();
        let lost = marker.checkpoint.as_deref().is_some_and(|sha| {
            outlook.index.find_sha(sha).is_none()
                && local.is_none_or(|l| l.sha256 != sha)
                && local.and_then(|l| l.modified).is_none_or(|written| taken > written)
        });
        if !lost {
            return Ok(None);
        }
        self.take_over_checkpoint(&Other { marker, stale: true }, now)
    }

    /// Right after an `outlook` (it read the NAS just now), with the game closed and no session
    /// open: a marker this PC left behind (the Satchel closed, updated or shut down while a session
    /// was still closing, or a crash) is dealt with once this PC's save is sound and already a
    /// version. Then the marker goes, so the other PCs stop seeing a session here; but a checkpoint
    /// of it newer than every version, and not one of them, holds progress this save lost (a power
    /// cut), and is first promoted as another PC would. `true` when nothing of it is left to do;
    /// `false` while it stays (this PC's progress isn't on the NAS yet: the send comes first).
    pub fn clear_own_leftover(&mut self, outlook: &Outlook, now: DateTime<Utc>) -> Result<bool, SyncError> {
        let shared = self.shared().ok_or(SyncError::NotConfigured)?;
        let Some(bytes) = fsx::read_optional(&shared.in_use(self.pc()))? else { return Ok(true) };
        let sent = outlook.local.as_ref().is_some_and(|l| l.problem.is_none() && outlook.index.find_sha(&l.sha256).is_some());
        if !sent {
            return Ok(false);
        }
        if let Ok(marker) = serde_json::from_slice::<InUse>(&bytes) {
            let taken = marker.checkpointed.unwrap_or(marker.started);
            let lost = marker.checkpoint.as_deref().is_some_and(|sha| outlook.index.find_sha(sha).is_none())
                && outlook.index.newest_version().is_none_or(|v| taken > v.saved);
            if lost {
                self.take_over_checkpoint(&Other { marker, stale: true }, now)?;
            }
        }
        markers::clear(&shared, self.pc())?;
        self.state.last_checkpoint = None;
        self.save_state()?;
        Ok(true)
    }

    /// A PC that stopped during a session (crash, power cut) left a checkpoint. When that
    /// checkpoint was taken after the newest version was saved, it becomes the newest version,
    /// under that PC's name. The stale marker is removed either way.
    ///
    /// The checkpoint's own time decides, never the heartbeat: a Satchel closed while its session
    /// was still closing leaves a marker whose heartbeat is newer than the progress it sent when
    /// the game closed, beside a checkpoint older than that progress. A marker from before 0.4.2
    /// has no checkpoint time, and its session's start stands in: the checkpoint came after it.
    pub fn take_over_checkpoint(&mut self, other: &Other, now: DateTime<Utc>) -> Result<Option<Version>, SyncError> {
        let shared = self.reachable()?;
        let pc = self.pc().to_string();
        let _lock = IndexLock::acquire(&shared, &pc, LOCK_WAIT)?;
        let mut idx = index::read(&shared)?;
        let taken = other.marker.checkpointed.unwrap_or(other.marker.started);
        let newer_than_newest = idx.newest_version().is_none_or(|v| taken > v.saved);
        let checkpoint = fsx::read_optional(&shared.live(&other.marker.pc))?;
        let promoted = match (other.marker.checkpoint.as_deref(), checkpoint) {
            (Some(sha), Some(bytes))
                if newer_than_newest
                    && fsx::fingerprint(&bytes) == sha
                    && idx.find_sha(sha).is_none()
                    && savefile::problem(&bytes).is_none() =>
            {
                let note = Some(format!("checkpoint: {} stopped during a session", other.marker.pc));
                Some(write_version(&shared, &mut idx, &other.marker.pc, &bytes, now, None, note)?)
            }
            _ => None,
        };
        markers::clear(&shared, &other.marker.pc)?;
        log!("{} stopped during a session; checkpoint promoted: {:?}", other.marker.pc, promoted.as_ref().map(|v| v.n));
        Ok(promoted)
    }

    /// Adds a finished session to the playtime: on the NAS when it answers (with any sessions
    /// that waited), otherwise kept on this PC until it does. Sync off: this PC's file.
    pub fn record_session(&self, session: &Session) -> io::Result<()> {
        let Some(shared) = self.shared() else {
            return playtime::append(&self.local.playtime(), std::slice::from_ref(session));
        };
        if !shared.reachable_within(REACH_WAIT) {
            return playtime::append(&self.local.playtime_pending(), std::slice::from_ref(session));
        }
        let mut batch = playtime::read_file(&self.local.playtime_pending())?;
        batch.push(session.clone());
        playtime::append(&shared.playtime(self.pc()), &batch)?;
        fsx::remove_if_present(&self.local.playtime_pending())
    }

    /// Sends the sessions that waited for the NAS.
    pub fn flush_playtime(&self) -> io::Result<()> {
        let Some(shared) = self.shared() else { return Ok(()) };
        let waiting = playtime::read_file(&self.local.playtime_pending())?;
        if waiting.is_empty() || !shared.reachable_within(REACH_WAIT) {
            return Ok(());
        }
        playtime::append(&shared.playtime(self.pc()), &waiting)?;
        fsx::remove_if_present(&self.local.playtime_pending())
    }

    /// Every PC's sessions with sync on (plus this PC's waiting ones); this PC's with sync off.
    pub fn sessions(&self) -> io::Result<Vec<Session>> {
        let mut sessions = match self.shared() {
            None => playtime::read_file(&self.local.playtime())?,
            Some(shared) => {
                let mut all = if shared.reachable_within(REACH_WAIT) {
                    playtime::read_dir(&shared.playtime_dir())?
                } else {
                    Vec::new()
                };
                all.extend(playtime::read_file(&self.local.playtime_pending())?);
                all
            }
        };
        sessions.sort_by_key(|s| std::cmp::Reverse(s.start));
        Ok(sessions)
    }

    /// Sync off: keeps a copy of the save after a session (skipped when unchanged).
    pub fn keep_copy(&self, now: DateTime<Utc>) -> io::Result<Option<String>> {
        let Some(bytes) = fsx::read_optional(&self.local.live_save())? else { return Ok(None) };
        if let Some(why) = savefile::problem(&bytes) {
            // A damaged copy would only push a good one out of the 30 kept.
            log!("no copy kept: this PC's save {why}");
            return Ok(None);
        }
        let dir = self.local.copies_dir();
        if let Some(newest) = list_saves(&dir)?.first()
            && fs::read(&newest.path)? == bytes
        {
            return Ok(None);
        }
        let file = format!("AzerothSurvivors {}.sav", names::stamp(now));
        fsx::write_atomic(&dir.join(&file), &bytes)?;
        fsx::prune(&dir, COPIES_KEEP)?;
        Ok(Some(file))
    }

    /// Sync off: the copies kept on this PC, newest first.
    pub fn copies(&self) -> io::Result<Vec<SaveFile>> {
        list_saves(&self.local.copies_dir())
    }

    /// The exports at the top of the shared folder (made by hand in the game), newest first.
    pub fn exports(&self) -> Result<Vec<SaveFile>, SyncError> {
        let shared = self.reachable()?;
        Ok(list_saves(&shared.folder)?)
    }
}

/// Writes `bytes` as a new version file, then records it in the index. The caller holds the lock
/// and has checked the bytes are a save; this refuses anything else all the same.
fn write_version(
    shared: &Shared,
    idx: &mut SyncIndex,
    pc: &str,
    bytes: &[u8],
    now: DateTime<Utc>,
    game: Option<String>,
    note: Option<String>,
) -> io::Result<Version> {
    if let Some(why) = savefile::problem(bytes) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("not sent: the save {why}")));
    }
    let mut file = names::version_file(now, pc);
    if shared.version(&file).exists() {
        // Two versions from one PC within the same second: the version number tells them apart.
        file = format!("{} v{}.sav", file.trim_end_matches(".sav"), idx.newest + 1);
    }
    fsx::write_atomic(&shared.version(&file), bytes)?;
    let version = idx.push(Version {
        n: 0,
        pc: pc.into(),
        file,
        sha256: fsx::fingerprint(bytes),
        bytes: bytes.len() as u64,
        saved: now,
        game,
        note,
    });
    index::write(shared, idx)?;
    Ok(version)
}

/// The `.sav` files directly in `dir`, newest first.
fn list_saves(dir: &Path) -> io::Result<Vec<SaveFile>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut saves: Vec<SaveFile> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_save = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("sav"));
            let meta = entry.metadata().ok().filter(|m| m.is_file() && is_save)?;
            Some(SaveFile {
                name: entry.file_name().to_string_lossy().into_owned(),
                bytes: meta.len(),
                modified: meta.modified().ok().map(DateTime::<Utc>::from),
                path,
            })
        })
        .collect();
    saves.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.name.cmp(&a.name)));
    Ok(saves)
}
