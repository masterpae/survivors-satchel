//! Progress sync, the worker's side: the session (the game watched from start to end, the in-use
//! marker, checkpoints while it runs, the send when it closes), the network folder's outlook (the
//! versions, the other PCs, the decision), the gate a newer Satchel keeps, the checks before Play,
//! Sync now, the "Needs you" choice when two PCs moved on separately, bringing a version back,
//! and the lists the window shows of all that. The core does the file work; nothing here runs
//! while the game writes.

use super::feedback::NOTICE_FOR;
use super::{Worker, ago, clock, due, playtime, self_update, size, when};
use crate::about;
use crate::view::{PcLine, SaveLine};
use crate::win;
use chrono::{DateTime, Local, Utc};
use satchel_core::activity::Kind;
use satchel_core::decide::Decision;
use satchel_core::engine::{BroughtIn, LocalSave, Outlook, Shown, SyncError};
use satchel_core::fsx;
use satchel_core::gate;
use satchel_core::log;
use satchel_core::markers::{HEARTBEAT_SECS, InUse, Other};
use satchel_core::playtime::{Session, format_minutes};
use satchel_core::session::{Event, Tracker};
use satchel_core::shell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const RETRY_EVERY: Duration = Duration::from_secs(180);
/// A write to the live save is checkpointed once it has been quiet this long.
const SETTLE: Duration = Duration::from_secs(3);

/// What a row of Progress versions stands for.
#[derive(Clone)]
enum Row {
    Version(u64),
    File(PathBuf),
}

/// What the window shows of the progress: Settings' Progress versions and Home's Your PCs.
#[derive(Default)]
struct ProgressLists {
    saves: Vec<SaveLine>,
    saves_summary: String,
    rows: Vec<Row>,
    /// The PCs of Home's Your PCs card, this PC first.
    pcs: Vec<PcLine>,
    last_copy: Option<DateTime<Utc>>,
    /// The last successful send or bring-in: the newest version this PC sent or has.
    last_sync: Option<DateTime<Utc>>,
    pc_count: usize,
}

#[derive(Default)]
pub(super) struct SyncState {
    tracker: Tracker,
    // The session.
    marker: Option<InUse>,
    sent: Option<u64>,
    launched: bool,
    live_seen: Option<SystemTime>,
    live_changed: Option<Instant>,
    writes: u32,
    files_stamp: Option<SystemTime>,
    last_heartbeat: Option<Instant>,
    // The PC and the NAS.
    outlook: Option<Outlook>,
    offline: bool,
    offline_told: bool,
    split: Option<u64>,
    needs_you_open: bool,
    /// Why the progress question's pick didn't go through (the game running, the folder not
    /// answering, …), under its buttons while it is open.
    needs_note: Option<(String, Instant)>,
    /// What the "Needs you" choice shows on screen: an answer applies to exactly that.
    shown: Option<Shown>,
    // Play.
    pending_play: bool,
    play_anyway: bool,
    /// This PC's own in-use marker from an earlier run has been dealt with (or there was none).
    leftover_done: bool,
    gated: Option<String>,
    last_retry: Option<Instant>,
    lists: ProgressLists,
}

impl SyncState {
    /// The session's step for this tick: the game running or not, and whether its files changed
    /// while it was closed (an update), which keeps a session open through the restart.
    pub(super) fn session_tick(&mut self, now_s: i64, running: bool, dir: Option<&Path>) -> Option<Event> {
        let changed = !running && self.tracker.in_session() && dir.and_then(super::game_files_stamp).is_some_and(|stamp| Some(stamp) > self.files_stamp);
        self.tracker.tick(now_s, running, changed)
    }

    pub(super) fn game_running(&self) -> bool {
        self.tracker.game_running()
    }

    pub(super) fn in_session(&self) -> bool {
        self.tracker.in_session()
    }

    /// The game updated itself during this session.
    pub(super) fn updated(&self) -> bool {
        self.tracker.updated
    }

    /// Writes to the save while playing, this session.
    pub(super) fn writes(&self) -> u32 {
        self.writes
    }

    /// The game was started by Play.
    pub(super) fn launched(&mut self) {
        self.launched = true;
    }

    /// The network folder isn't answering, as of the last try.
    pub(super) fn offline(&self) -> bool {
        self.offline
    }

    /// The newer Satchel whose files keep this one out of the network folder, when one does.
    pub(super) fn gated(&self) -> Option<&str> {
        self.gated.as_deref()
    }

    /// A newer Satchel's files were met in the network folder (the shared addon list's reading).
    pub(super) fn set_gated(&mut self, newer: String) {
        self.gated = Some(newer);
    }

    /// Two PCs moved on separately: the network folder's version to pick against this PC's.
    pub(super) fn split(&self) -> Option<u64> {
        self.split
    }

    /// The PC whose version the split is with, when the index names it.
    pub(super) fn split_pc(&self) -> Option<String> {
        self.split.and_then(|n| self.outlook.as_ref().and_then(|o| o.index.get(n)).map(|v| v.pc.clone()))
    }

    /// The version sent when the game closed, this session.
    pub(super) fn sent(&self) -> Option<u64> {
        self.sent
    }

    /// A Play waits for an answer (the card's question, the "Needs you" choice).
    pub(super) fn pending_play(&self) -> bool {
        self.pending_play
    }

    pub(super) fn set_pending_play(&mut self, pending: bool) {
        self.pending_play = pending;
    }

    /// The network folder as last looked at, when sync is on and it answered.
    pub(super) fn outlook(&self) -> Option<&Outlook> {
        self.outlook.as_ref()
    }

    /// The PCs the versions' index names.
    pub(super) fn pc_names(&self) -> impl Iterator<Item = &String> {
        self.outlook.iter().flat_map(|o| &o.index.pcs)
    }

    /// What the "Needs you" choice showed last: an answer applies to exactly that.
    pub(super) fn set_shown(&mut self, shown: Option<Shown>) {
        self.shown = shown;
    }

    /// Whether the retry of an unsent save is due.
    pub(super) fn retry_due(&self) -> bool {
        due(self.last_retry, RETRY_EVERY)
    }

    pub(super) fn saves(&self) -> &[SaveLine] {
        &self.lists.saves
    }

    pub(super) fn saves_summary(&self) -> &str {
        &self.lists.saves_summary
    }

    pub(super) fn pcs(&self) -> &[PcLine] {
        &self.lists.pcs
    }

    pub(super) fn pc_count(&self) -> usize {
        self.lists.pc_count
    }

    pub(super) fn last_copy(&self) -> Option<DateTime<Utc>> {
        self.lists.last_copy
    }

    pub(super) fn last_sync(&self) -> Option<DateTime<Utc>> {
        self.lists.last_sync
    }

    /// The "Needs you" choice as the window shows it, while it is open: this PC's save, the
    /// other PC's version and that PC; and what the choice applies to.
    pub(super) fn needs_you_view(&self, local: Option<&LocalSave>) -> (Option<[String; 3]>, Option<Shown>) {
        let mut shown = None;
        let needs_you = match (self.needs_you_open, self.split) {
            (true, Some(n)) => {
                shown = Some(Shown { local: local.map(|l| l.sha256.clone()), nas: n });
                let version = self.outlook.as_ref().and_then(|o| o.index.get(n));
                let here = local.map(|l| format!("{} · {}", when_opt(l.modified), size(l.bytes))).unwrap_or_default();
                let there = version.map(|v| format!("{} · {}", when(v.saved), size(v.bytes))).unwrap_or_default();
                let from = version.map(|v| v.pc.clone()).unwrap_or_else(|| "the other PC".into());
                Some([here, there, from])
            }
            _ => None,
        };
        (needs_you, shown)
    }

    /// The pick's refusal, under the overlay's buttons while it is open, for its minute.
    pub(super) fn needs_note(&self, open: bool) -> String {
        match (open, &self.needs_note) {
            (true, Some((why, at))) if at.elapsed() < NOTICE_FOR => why.clone(),
            _ => String::new(),
        }
    }
}

fn when_opt(at: Option<DateTime<Utc>>) -> String {
    at.map(when).unwrap_or_else(|| "–".into())
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Worker {
    pub(super) fn session_started(&mut self, now: DateTime<Utc>) {
        log!("game started{}", if self.sync.launched { " by Play" } else { " without Play" });
        self.playtime.started(now);
        self.sync.sent = None;
        self.sync.writes = 0;
        self.sync.live_seen = fs::metadata(self.engine.local.live_save()).and_then(|m| m.modified()).ok();
        self.sync.live_changed = None;
        self.sync.files_stamp = self.game_dir().and_then(|d| super::game_files_stamp(&d));
        if self.engine.shared().is_none() {
            return;
        }
        let marker = InUse { pc: self.engine.pc().into(), started: now, heartbeat: now, checkpoint: None, checkpointed: None };
        match self.engine.write_marker(&marker) {
            Ok(()) => {}
            Err(SyncError::Offline) => self.sync.offline = true,
            Err(e) => log!("in-use marker not written: {e}"),
        }
        self.sync.marker = Some(marker);
        self.sync.last_heartbeat = Some(Instant::now());
        if self.sync.launched {
            return;
        }
        // Started the old way: the Satchel can't swap a save the game has already loaded.
        if let Ok(outlook) = self.engine.outlook(now) {
            if let Decision::BringIn(n) | Decision::Split { nas: n } = outlook.decision {
                let from = outlook.index.get(n).map(|v| v.pc.clone()).unwrap_or_default();
                self.feedback.warn(format!("{from} has newer progress: close the game and press Play to bring it in."));
                self.notify(
                    "Newer progress in the network folder",
                    &format!("{from} has newer progress. Close the game and press Play to bring it in; if you keep playing, you'll pick one at the end."),
                );
            }
            for other in outlook.others.iter().filter(|o| !o.stale) {
                self.notify(
                    &format!("Careful: the game is also open on {}", other.marker.pc),
                    &format!("Started {}. If you keep playing here, you'll pick one progress at the end.", clock(other.marker.started)),
                );
            }
            self.sync.outlook = Some(outlook);
        }
    }

    /// While the game runs: every write to the live save is noted, and once it settles a
    /// checkpoint goes to the NAS.
    pub(super) fn watch_save(&mut self) {
        let modified = fs::metadata(self.engine.local.live_save()).and_then(|m| m.modified()).ok();
        if modified != self.sync.live_seen {
            self.sync.live_seen = modified;
            self.sync.live_changed = Some(Instant::now());
            self.sync.writes += 1;
        }
        if self.sync.live_changed.is_some_and(|t| t.elapsed() >= SETTLE) {
            self.sync.live_changed = None;
            let Some(mut marker) = self.sync.marker.take() else { return };
            match self.engine.checkpoint(&mut marker, Utc::now()) {
                Ok(true) => {
                    log!("checkpoint sent");
                    self.sync.offline = false;
                    self.sync.last_heartbeat = Some(Instant::now());
                }
                Ok(false) => {}
                Err(SyncError::Offline) => self.sync.offline = true,
                Err(e) => log!("checkpoint failed: {e}"),
            }
            self.sync.marker = Some(marker);
        }
    }

    pub(super) fn heartbeat(&mut self) {
        if !due(self.sync.last_heartbeat, Duration::from_secs(HEARTBEAT_SECS as u64)) {
            return;
        }
        self.sync.last_heartbeat = Some(Instant::now());
        // An update restart can be quicker than one look at the processes, and then goes unseen.
        // Re-reading the game's files while it runs keeps such an update from making the next
        // ordinary close look like one.
        if self.sync.tracker.game_running() {
            let now = self.game_dir().and_then(|d| super::game_files_stamp(&d));
            self.sync.files_stamp = self.sync.files_stamp.max(now);
        }
        if let Some(marker) = self.sync.marker.as_mut() {
            marker.heartbeat = Utc::now();
            match self.engine.write_marker(marker) {
                Ok(()) => self.sync.offline = false,
                Err(SyncError::Offline) => self.sync.offline = true,
                Err(e) => log!("heartbeat failed: {e}"),
            }
        }
    }

    /// The game closed: the save on disk is complete and goes out now, while Windows is held
    /// from shutting down.
    pub(super) fn send_after_close(&mut self) {
        let _hold = win::Hold::start();
        self.busy_push("Sending your progress…".into());
        let result = self.engine.send(Utc::now(), None, None, None);
        self.feedback.clear_busy();
        match result {
            Ok(Some(version)) => {
                self.sync.offline = false;
                self.sync.sent = Some(version.n);
                self.marker_names(&version);
            }
            Ok(None) => {}
            Err(SyncError::Offline) => self.unsent_offline(),
            Err(SyncError::MovedOn) => {
                log!("another PC sent newer progress meanwhile: both are kept and the player picks");
                self.sync.split = self.newest();
            }
            // Nothing worth sending: no retry until the game writes a save again.
            Err(SyncError::Damaged(why)) => log!("not sent: {why}"),
            Err(e) => {
                log!("send failed: {e}");
                self.engine.state.send_pending = true;
                let _ = self.engine.save_state();
                self.feedback.set_problem(format!("Sending failed: {e}. Your progress is safe on this PC."));
            }
        }
        // The cards show what was just sent, not what the NAS held before.
        self.refresh();
    }

    /// The marker names the version just sent as its checkpoint: a Satchel of any version reading
    /// it later (this PC closed before the session's end, say) finds it among the versions, and
    /// never takes an older checkpoint over it.
    fn marker_names(&mut self, version: &satchel_core::index::Version) {
        let Some(marker) = self.sync.marker.as_mut() else { return };
        marker.checkpoint = Some(version.sha256.clone());
        marker.checkpointed = Some(version.saved);
        if let Err(e) = self.engine.write_marker(marker) {
            log!("in-use marker not updated after the send: {e}");
        }
        self.engine.state.last_checkpoint = Some(version.sha256.clone());
        if let Err(e) = self.engine.save_state() {
            log!("state not saved: {e}");
        }
    }

    /// This PC's own marker left by an earlier run (closed, updated or shut down while a session
    /// was still closing), dealt with after the first outlook that can: game closed, no session.
    fn leftover(&mut self, outlook: &Outlook) {
        if self.sync.leftover_done || self.sync.tracker.in_session() || self.game_dir().is_none_or(|d| self.scanner.game_running(&d)) {
            return;
        }
        match self.engine.clear_own_leftover(outlook, Utc::now()) {
            Ok(done) => self.sync.leftover_done = done,
            Err(e) => log!("this PC's in-use marker from an earlier run not dealt with: {e}"),
        }
    }

    fn unsent_offline(&mut self) {
        self.sync.offline = true;
        self.engine.state.send_pending = true;
        let _ = self.engine.save_state();
        log!("NAS unreachable: the save waits on this PC");
        if !self.sync.offline_told {
            self.sync.offline_told = true;
            self.notify("Network folder unreachable", "Your progress is safe on this PC and goes up when the network folder is back.");
        }
    }

    fn newest(&self) -> Option<u64> {
        self.engine.outlook(Utc::now()).ok().and_then(|o| o.index.newest_version().map(|v| v.n))
    }

    /// The sync side of a session's end: with sync on, the marker goes and what was sent is said;
    /// with sync off, a copy is kept. Two PCs that moved on separately meanwhile are put to the
    /// player.
    pub(super) fn session_over_sync(&mut self, now: DateTime<Utc>, session: &Session) {
        let mut line = format!("session {}", format_minutes(session.minutes));
        if session.updated {
            line.push_str(" · the game updated itself, still one session");
        }
        if self.engine.shared().is_some() {
            match self.engine.clear_marker() {
                Ok(()) => {}
                Err(SyncError::Offline) => self.sync.offline = true,
                Err(e) => log!("in-use marker not removed: {e}"),
            }
            match self.sync.sent.take() {
                Some(n) => {
                    self.notify(
                        "Progress sent to the network folder",
                        &format!("Version {n}, from {} · session {}.", self.engine.pc(), format_minutes(session.minutes)),
                    );
                    line = format!("Sent version {n} · {line}");
                }
                None => line = format!("Played · {line}"),
            }
        } else {
            match self.engine.keep_copy(now) {
                Ok(Some(file)) => log!("copy kept: {file}"),
                Ok(None) => {}
                Err(e) => log!("copy not kept: {e}"),
            }
            line = format!("Played · {line}");
        }
        self.activity(if line.starts_with("Sent") { Kind::Sync } else { Kind::Game }, &line);
        self.sync.marker = None;
        self.sync.launched = false;
        if self.sync.split.is_some() {
            self.sync.needs_you_open = true;
            self.ui.show(Some(0));
        }
    }

    /// The NAS side of Play. `true`: go on and start the game.
    pub(super) fn check_before_play(&mut self, now: DateTime<Utc>) -> bool {
        let mut outlook = match self.engine.outlook(now) {
            Ok(outlook) => outlook,
            Err(SyncError::Offline) => {
                self.sync.offline = true;
                self.feedback.card_line("The network folder doesn't answer: you play with this PC's progress, sent when it's back", false, true);
                return true;
            }
            Err(SyncError::Io(e)) if gate::newer_satchel(&e).is_some() => {
                let newer = gate::newer_satchel(&e).map(|n| n.newer.clone()).unwrap_or_default();
                self.sync.offline = false;
                self.sync.gated = Some(newer.clone());
                if !std::mem::take(&mut self.sync.play_anyway) {
                    let update = match self.offer() {
                        Some(offer) => format!(" Update now puts {} here and plays.", offer.version),
                        None => format!(" {}", self_update::update_hint()),
                    };
                    self.feedback.card_ask(format!(
                        "Another PC uses Survivor's Satchel {newer} and this one is {}, so nothing syncs until it's updated. Playing now plays this PC's progress, synced after the update.{update}",
                        about::VERSION
                    ));
                    self.feedback.card_gate();
                    self.sync.pending_play = true;
                    return false;
                }
                self.feedback.card_line(&format!("Not synced: this Satchel ({}) is older than {newer}", about::VERSION), false, true);
                return true;
            }
            Err(e) => {
                log!("check before play failed: {e}");
                self.feedback.card_line(&format!("The network folder couldn't be checked ({e}): you play with this PC's progress"), false, true);
                return true;
            }
        };
        self.sync.offline = false;
        let stale: Vec<Other> = outlook.others.iter().filter(|o| o.stale).cloned().collect();
        if !stale.is_empty() {
            for other in &stale {
                let stopped = format!("{} stopped at {} during a session", other.marker.pc, clock(other.marker.heartbeat));
                match self.engine.take_over_checkpoint(other, now) {
                    Ok(Some(v)) => self.feedback.card_line(&format!("{stopped}; its last checkpoint is now version {}", v.n), true, false),
                    Ok(None) => self.feedback.card_line(&format!("{stopped}; nothing newer was left"), true, false),
                    Err(e) => log!("checkpoint of {} not taken over: {e}", other.marker.pc),
                }
            }
            match self.engine.outlook(now) {
                Ok(fresh) => outlook = fresh,
                Err(e) => {
                    log!("second check failed: {e}");
                    return true;
                }
            }
        }
        let anyway = std::mem::take(&mut self.sync.play_anyway);
        if !anyway && let Some(other) = outlook.others.iter().find(|o| !o.stale) {
            self.feedback.card_ask(format!(
                "The game looks open on {}: started {}, still active {}. If you play here too, the two will split and you'll pick one afterwards.",
                other.marker.pc,
                clock(other.marker.started),
                ago(other.marker.heartbeat, now)
            ));
            self.sync.pending_play = true;
            self.sync.outlook = Some(outlook);
            return false;
        }
        // An empty or damaged save here is never sent, and never replaced without the player.
        if let Some(why) = outlook.local.as_ref().and_then(|l| l.problem.clone()) {
            self.sync.outlook = Some(outlook);
            if !anyway {
                self.feedback.card_ask(format!(
                    "This PC's save {why}, so it isn't synced. To carry on from the network folder, cancel and bring back its newest version under Settings → Progress versions."
                ));
                self.sync.pending_play = true;
                return false;
            }
            self.feedback.card_line("Playing with this PC's save as it is; it isn't synced", false, true);
            return true;
        }
        let decision = outlook.decision.clone();
        let index = outlook.index.clone();
        self.sync.outlook = Some(outlook);
        match decision {
            Decision::InSync { adopt } => {
                if let Some(version) = adopt.and_then(|n| index.get(n)).cloned() {
                    let _ = self.engine.adopt(&version, now);
                }
                self.feedback.card_line("Network folder checked: you have the newest progress", true, false);
            }
            Decision::Send => match self.engine.send(now, None, None, None) {
                Ok(Some(v)) => {
                    self.feedback.card_line(&format!("Network folder checked; this PC's progress went up first (version {})", v.n), true, false);
                    self.activity(Kind::Sync, &format!("Sent version {}", v.n));
                }
                Ok(None) => self.feedback.card_line("Network folder checked", true, false),
                Err(SyncError::MovedOn) => {
                    let newest = self.newest();
                    return self.split_found(newest);
                }
                Err(e) => {
                    log!("send before play failed: {e}");
                    self.feedback.card_line(&format!("Sending first failed ({e}); you play with this PC's progress"), false, true);
                }
            },
            Decision::BringIn(n) => {
                self.feedback.card_line("Network folder checked", true, false);
                return self.bring_in(n, now);
            }
            Decision::Split { nas } => return self.split_found(Some(nas)),
        }
        true
    }

    fn split_found(&mut self, nas: Option<u64>) -> bool {
        self.sync.split = nas;
        self.sync.needs_you_open = true;
        self.sync.pending_play = true;
        self.feedback.card_line("Two PCs moved on separately: pick one in the window", false, true);
        self.feedback.close_card_in(10);
        self.ui.show(Some(0));
        false
    }

    /// Brings version `n` in. `true`: go on.
    fn bring_in(&mut self, n: u64, now: DateTime<Utc>) -> bool {
        self.busy_push(format!("Bringing in version {n}…"));
        let result = self.engine.bring_in(n, now);
        self.feedback.clear_busy();
        match result {
            Ok(BroughtIn { version, safety }) => {
                self.feedback.card_line(&format!("Progress from {} brought in (saved {})", version.pc, when(version.saved)), true, false);
                self.activity(Kind::Sync, &format!("Brought in version {} from {}", version.n, version.pc));
                let kept = if safety.is_some() { " Yours is kept as a safety copy." } else { "" };
                self.notify(&format!("Progress from {} brought in", version.pc), &format!("Saved {}.{kept}", when(version.saved)));
                true
            }
            Err(SyncError::MovedOn) => self.split_found(Some(n)),
            Err(e) => {
                log!("bring-in failed: {e}");
                self.feedback.card_line(&format!("Bringing in failed ({e}); you play with this PC's progress"), false, true);
                true
            }
        }
    }

    pub(super) fn sync_now(&mut self) {
        if self.engine.shared().is_none() {
            return;
        }
        let now = Utc::now();
        self.busy_push("Checking the network folder…".into());
        // The addons too: what changed here goes up, what changed elsewhere shows.
        self.share_look();
        let mut checked = self.engine.outlook(now);
        if let Ok(outlook) = &checked {
            let stale: Vec<Other> = outlook.others.iter().filter(|o| o.stale).cloned().collect();
            for other in &stale {
                if let Ok(Some(v)) = self.engine.take_over_checkpoint(other, now) {
                    self.activity(Kind::Sync, &format!("{} stopped during a session; its checkpoint became version {}", other.marker.pc, v.n));
                }
            }
            if !stale.is_empty() {
                checked = self.engine.outlook(now);
            }
        }
        self.feedback.clear_busy();
        match checked {
            Err(SyncError::Offline) => self.sync.offline = true,
            Err(e) => self.feedback.set_problem(format!("The network folder can't be read: {e}")),
            Ok(outlook) => {
                self.sync.offline = false;
                match outlook.decision {
                    Decision::InSync { adopt } => {
                        if let Some(version) = adopt.and_then(|n| outlook.index.get(n)).cloned() {
                            let _ = self.engine.adopt(&version, now);
                        }
                        let mut text = match self.share_status() {
                            Some(addons) => format!("Synced: nothing to do. {addons}."),
                            None => "Synced: nothing to do.".into(),
                        };
                        if let Some(game) = self.game_settings_status() {
                            text.push_str(&format!(" {game}."));
                        }
                        self.feedback.say(text);
                    }
                    Decision::Send => {
                        let _hold = win::Hold::start();
                        match self.engine.send(now, None, None, None) {
                            Ok(Some(v)) => self.activity(Kind::Sync, &format!("Sent version {}", v.n)),
                            Ok(None) => {}
                            Err(SyncError::MovedOn) => self.sync.split = self.newest(),
                            Err(e) => self.feedback.set_problem(format!("Sending failed: {e}")),
                        }
                    }
                    Decision::BringIn(n) => {
                        if self.sync.tracker.game_running() {
                            self.feedback.warn(format!("Close the game to bring in version {n}."));
                        } else {
                            self.feedback.open_card();
                            self.bring_in(n, now);
                            self.feedback.close_card_in(8);
                        }
                    }
                    Decision::Split { nas } => {
                        self.sync.split = Some(nas);
                        self.sync.needs_you_open = true;
                        self.ui.show(Some(0));
                    }
                }
            }
        }
        self.refresh();
    }

    /// The player's pick after two PCs moved on separately. Both stay as versions. The pick
    /// applies to the two saves on screen; if either changed meanwhile, the question comes again.
    pub(super) fn resolve(&mut self, keep_here: bool) {
        let Some(shown) = self.sync.shown.clone() else {
            self.sync.needs_you_open = false;
            return;
        };
        if self.sync.tracker.game_running() {
            self.refuse_pick("Close the game first, then pick.");
            return;
        }
        let now = Utc::now();
        self.busy_push("Saving your choice…".into());
        let result = if keep_here {
            let note = Some("kept after two PCs moved on separately".to_string());
            self.engine
                .send(now, None, note, Some(&shown))
                .map(|v| v.map(|v| format!("Kept this PC's progress as version {}", v.n)))
        } else {
            self.engine
                .use_nas(&shown, now)
                .map(|brought| Some(format!("Continued with version {} ({})", brought.version.n, brought.version.pc)))
        };
        self.feedback.clear_busy();
        match result {
            Ok(text) => {
                self.sync.split = None;
                self.sync.needs_you_open = false;
                self.sync.needs_note = None;
                self.feedback.clear_problem();
                if let Some(text) = text {
                    self.activity(Kind::Sync, &text);
                }
                if self.sync.pending_play {
                    self.sync.pending_play = false;
                    self.play_sequence();
                }
            }
            Err(SyncError::Offline) => {
                self.sync.offline = true;
                self.refuse_pick("The network folder doesn't answer; pick again when it's back.");
            }
            Err(SyncError::Changed) => {
                log!("the choice was made on a view that had changed: asked again");
                self.refuse_pick("Something changed since you looked (another PC sent, or this PC's save changed). Check the two again and pick.");
            }
            Err(e) => {
                log!("choice not saved: {e}");
                let why = format!("Your choice couldn't be saved: {e}");
                self.sync.needs_note = Some((why.clone(), Instant::now()));
                self.feedback.set_problem(why);
            }
        }
        self.refresh();
    }

    pub(super) fn retry(&mut self) {
        self.sync.last_retry = Some(Instant::now());
        let now = Utc::now();
        let outlook = match self.engine.outlook(now) {
            Ok(outlook) => outlook,
            Err(SyncError::Offline) => {
                self.sync.offline = true;
                return;
            }
            Err(e) => {
                log!("retry failed: {e}");
                return;
            }
        };
        if self.sync.offline {
            log!("the NAS answers again");
        }
        self.sync.offline = false;
        self.sync.offline_told = false;
        self.flush_playtime();
        if self.engine.state.send_pending {
            match outlook.decision {
                Decision::Send => {
                    let _hold = win::Hold::start();
                    match self.engine.send(now, None, None, None) {
                        Ok(Some(v)) => {
                            self.activity(Kind::Sync, &format!("Sent version {} (it waited for the network folder)", v.n));
                            self.notify("Progress sent to the network folder", &format!("Version {}, which waited for the network folder.", v.n));
                        }
                        Ok(None) => {}
                        Err(SyncError::MovedOn) => self.sync.split = self.newest(),
                        Err(e) => log!("waiting send failed: {e}"),
                    }
                }
                Decision::Split { nas } => {
                    log!("while this PC waited, another PC moved on: the player picks");
                    self.sync.split = Some(nas);
                    self.sync.needs_you_open = true;
                    self.ui.show(Some(0));
                }
                Decision::InSync { adopt } => {
                    if let Some(version) = adopt.and_then(|n| outlook.index.get(n)).cloned() {
                        let _ = self.engine.adopt(&version, now);
                    }
                    self.engine.state.send_pending = false;
                    let _ = self.engine.save_state();
                }
                Decision::BringIn(_) => {
                    self.engine.state.send_pending = false;
                    let _ = self.engine.save_state();
                }
            }
        }
        self.refresh();
    }

    pub(super) fn bring_back(&mut self, index: usize) {
        if self.sync.tracker.game_running() {
            self.feedback.warn("Close the game first: a save is only brought in while the game is closed.".into());
            return;
        }
        let Some(row) = self.sync.lists.rows.get(index).cloned() else { return };
        let now = Utc::now();
        self.busy_push("Bringing it back…".into());
        let result = match row {
            Row::Version(n) => self
                .engine
                .version_bytes(n)
                .and_then(|(v, bytes)| self.engine.restore(bytes, format!("version {n} brought back"), v.game, now))
                .map(|b| format!("Brought back version {n} (now version {})", b.version.n)),
            Row::File(path) if self.engine.shared().is_some() => {
                let name = file_name(&path);
                fs::read(&path)
                    .map_err(SyncError::from)
                    .and_then(|bytes| self.engine.restore(bytes, format!("{name} brought back"), None, now))
                    .map(|b| format!("Brought back {name} (now version {})", b.version.n))
            }
            Row::File(path) => {
                let name = file_name(&path);
                self.engine.restore_copy(&path, now).map(|_| format!("Brought back {name}"))
            }
        };
        self.feedback.clear_busy();
        match result {
            Ok(text) => {
                self.activity(Kind::Sync, &text);
                self.feedback.say(format!("{text}. Your save from before is kept as a safety copy."));
            }
            Err(SyncError::Offline) => self.sync.offline = true,
            Err(e) => self.feedback.set_problem(format!("It couldn't be brought back: {e}")),
        }
        self.refresh();
    }

    /// Sync's settings changed (on or off, the folder, this PC's name): what was known of the
    /// network folder is forgotten until the next look.
    pub(super) fn sync_settings_changed(&mut self) {
        self.sync.outlook = None;
        self.sync.split = None;
        self.sync.offline = false;
    }

    /// The first start's sync, the game closed: this PC's save goes up when the network folder
    /// has nothing newer, the folder's newest comes here when this PC has nothing, and when both
    /// sides have progress the normal question is asked.
    pub(super) fn first_sync(&mut self, now: DateTime<Utc>) {
        if self.engine.shared().is_some() {
            match self.engine.outlook(now) {
                Ok(outlook) => match outlook.decision {
                    // Both sides have progress: the normal question, at Play and on Home.
                    Decision::Split { nas } => {
                        self.sync.split = Some(nas);
                        self.sync.needs_you_open = true;
                    }
                    Decision::Send => {
                        // 0.8 offered the newest export at the top of an empty folder in this
                        // PC's save's place; this PC's save goes first now, and the export stays
                        // one click away.
                        let export = self.differing_export(&outlook);
                        match self.engine.send(now, None, Some("first start".into()), None) {
                            Ok(Some(v)) => self.activity(Kind::Sync, &format!("Started the network folder with version {}", v.n)),
                            Ok(None) => {}
                            Err(e) => self.feedback.set_problem(format!("The first sync failed: {e}")),
                        }
                        if let Some(name) = export {
                            self.activity(Kind::Sync, &format!("Your export {name} is under Settings → Progress versions"));
                        }
                    }
                    Decision::BringIn(n) => {
                        self.feedback.open_card();
                        self.bring_in(n, now);
                        self.feedback.close_card_in(8);
                    }
                    Decision::InSync { adopt } => {
                        if let Some(version) = adopt.and_then(|n| outlook.index.get(n)).cloned() {
                            let _ = self.engine.adopt(&version, now);
                        }
                    }
                },
                Err(SyncError::Offline) => {
                    self.sync.offline = true;
                    self.engine.state.send_pending = true;
                    let _ = self.engine.save_state();
                }
                Err(e) => self.feedback.set_problem(format!("The network folder can't be read: {e}")),
            }
        }
    }

    /// Reads both sides again for what the window shows. Progress the network folder lacks goes up
    /// here too, with the game closed: a shutdown or a crash during a session leaves the last save
    /// unsent and `send_pending` unset, and it would wait for Play or Sync now while another PC
    /// could start from the older version.
    pub(super) fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        if let Some(why) = self.engine.folder_problem() {
            // Sync pauses (one-PC copies meanwhile) until another folder is chosen.
            self.sync.outlook = None;
            self.sync.split = None;
            self.feedback.set_problem(format!("Sync is paused: {why}. Choose another network folder in Settings."));
        } else if self.engine.shared().is_some() {
            match self.engine.outlook(Utc::now()) {
                Ok(mut outlook) => {
                    self.sync.offline = false;
                    // A send that failed (`send_pending`) is `retry`'s; this is the one nobody tried.
                    // A checkpoint this PC's save lost (a power cut after the game's last write) becomes
                    // a version first: sent as it is, the older save would count as the newer one and
                    // the checkpoint would go with this PC's marker.
                    if !self.sync.tracker.in_session() && self.game_dir().is_some_and(|d| !self.scanner.game_running(&d)) {
                        match self.engine.promote_own_checkpoint(&outlook, Utc::now()) {
                            Ok(Some(v)) => {
                                log!("this PC's own checkpoint, newer than its save, became version {}", v.n);
                                self.activity(Kind::Sync, &format!("This PC's last checkpoint became version {}", v.n));
                                match self.engine.outlook(Utc::now()) {
                                    Ok(fresh) => outlook = fresh,
                                    Err(e) => log!("the look after the checkpoint failed: {e}"),
                                }
                            }
                            Ok(None) => {}
                            Err(e) => log!("this PC's own checkpoint not looked at: {e}"),
                        }
                    }
                    let unsent = matches!(outlook.decision, Decision::Send) && outlook.local.as_ref().is_some_and(|l| l.problem.is_none());
                    if unsent
                        && !self.engine.state.send_pending
                        && !self.sync.tracker.in_session()
                        && self.feedback.busy().is_none()
                        && self.game_dir().is_some_and(|d| !self.scanner.game_running(&d))
                    {
                        let sent = {
                            let _hold = win::Hold::start();
                            self.engine.send(Utc::now(), None, None, None)
                        };
                        // After a send, or another PC's meanwhile, the rest goes on from a fresh look:
                        // the view shows what is in the folder now (a split, too), and the leftover
                        // marker is dealt with knowing the save is a version.
                        let look_again = match sent {
                            Ok(Some(v)) => {
                                log!("progress the network folder lacked went up as version {} at the look", v.n);
                                self.activity(Kind::Sync, &format!("Sent version {}", v.n));
                                self.notify(
                                    "Progress sent to the network folder",
                                    &format!("Version {}, from {}: it was waiting on this PC.", v.n, self.engine.pc()),
                                );
                                true
                            }
                            Ok(None) => false,
                            Err(SyncError::MovedOn) => {
                                log!("another PC sent newer progress meanwhile: both are kept and the player picks");
                                true
                            }
                            Err(SyncError::Offline) => {
                                self.sync.offline = true;
                                false
                            }
                            Err(e) => {
                                log!("progress the network folder lacked not sent at the look: {e}");
                                // From here it is `retry`'s, every few minutes, not every look.
                                self.engine.state.send_pending = true;
                                let _ = self.engine.save_state();
                                false
                            }
                        };
                        if look_again {
                            match self.engine.outlook(Utc::now()) {
                                Ok(fresh) => outlook = fresh,
                                Err(e) => log!("the look after the send failed: {e}"),
                            }
                        }
                    }
                    match outlook.local.as_ref().and_then(|l| l.problem.as_ref()) {
                        Some(why) => self.feedback.set_problem(format!("This PC's save {why}, so it isn't sent. To carry on from the network folder, bring back its newest version under Settings → Progress versions.")),
                        None => self.feedback.clear_problem(),
                    }
                    self.sync.split = match outlook.decision {
                        Decision::Split { nas } => Some(nas),
                        _ => None,
                    };
                    self.leftover(&outlook);
                    self.sync.outlook = Some(outlook);
                    self.sync.gated = None;
                }
                Err(SyncError::Offline) => self.sync.offline = true,
                Err(SyncError::Io(e)) if gate::newer_satchel(&e).is_some() => {
                    // The NAS answered; a newer Satchel's files keep this one out.
                    let newer = gate::newer_satchel(&e).map(|n| n.newer.clone()).unwrap_or_default();
                    self.sync.offline = false;
                    self.sync.outlook = None;
                    self.feedback.set_problem(format!("Nothing syncs until this Satchel ({}) is updated: another PC uses {newer}.", about::VERSION));
                    self.sync.gated = Some(newer);
                }
                Err(e) => self.feedback.set_problem(format!("The network folder can't be read: {e}")),
            }
        } else {
            self.sync.outlook = None;
            self.sync.gated = None;
        }
        self.build_lists();
    }

    /// Progress versions and Your PCs: the network folder's versions, newest first, plus the
    /// exports at its top, every PC in the index with its newest version and session; with sync
    /// off, this PC's copies.
    pub(super) fn sync_lists(&mut self, local_now: &DateTime<Local>) {
        let mut lists = ProgressLists::default();
        let pc = self.engine.pc().to_string();
        // Progress versions: the network folder's, newest first, plus the exports at its top; with
        // sync off, this PC's copies.
        if let Some(outlook) = &self.sync.outlook {
            let here = outlook.local.as_ref().map(|l| l.sha256.as_str());
            for version in &outlook.index.versions {
                let current = here == Some(version.sha256.as_str());
                let mut notes: Vec<String> = Vec::new();
                if current {
                    notes.push("the one you play".into());
                }
                if let Some(note) = &version.note {
                    notes.push(if note.contains("two PCs moved on separately") { "kept when two PCs changed it".into() } else { note.clone() });
                }
                lists.saves.push(SaveLine {
                    version: format!("v{}", version.n),
                    when: shell::when_short(version.saved, local_now),
                    from: version.pc.clone(),
                    size: size(version.bytes),
                    note: notes.join(" · "),
                    current,
                    can_bring: !current,
                });
                lists.rows.push(Row::Version(version.n));
                if version.pc == pc && lists.last_sync.is_none_or(|t| version.saved > t) {
                    lists.last_sync = Some(version.saved);
                }
            }
            // The newest few: older exports pile up in the game's Saves folder.
            if let Ok(exports) = self.engine.exports() {
                for export in exports.into_iter().take(5) {
                    lists.saves.push(SaveLine {
                        version: export.name.clone(),
                        when: export.modified.map(|t| shell::when_short(t, local_now)).unwrap_or_default(),
                        from: "–".into(),
                        size: size(export.bytes),
                        note: "your export".into(),
                        current: false,
                        can_bring: true,
                    });
                    lists.rows.push(Row::File(export.path));
                }
            }
            lists.pc_count = outlook.index.pcs.len().max(1);
            // The version this PC last synced, when it was another PC's.
            if let Some(synced) = &self.engine.state.last_synced
                && lists.last_sync.is_none_or(|t| synced.at > t)
            {
                lists.last_sync = Some(synced.at);
            }
            let versions = outlook.index.versions.len();
            lists.saves_summary = match (versions, outlook.index.newest_version()) {
                (0, _) | (_, None) => "No versions yet: the first session sends version 1".into(),
                (n, Some(v)) => format!("{n} version{} · newest {} from {}", if n == 1 { "" } else { "s" }, shell::when_short(v.saved, local_now), v.pc),
            };
            // Your PCs: every PC in the index, this PC first, with its newest version and its
            // newest session.
            let sessions = self.engine.sessions().unwrap_or_default();
            let mut names: Vec<String> = outlook.index.pcs.clone();
            if !names.iter().any(|n| n == &pc) {
                names.insert(0, pc.clone());
            }
            names.sort_by_key(|n| (n != &pc, n.to_lowercase()));
            lists.pcs = names
                .iter()
                .map(|name| {
                    let synced = outlook.index.versions.iter().filter(|v| &v.pc == name).map(|v| v.saved).max();
                    let played = playtime::last_played(&sessions, name);
                    let mut parts = Vec::new();
                    if let Some(at) = played {
                        parts.push(format!("played {}", shell::day_word(at, local_now)));
                    }
                    match synced {
                        Some(at) => parts.push(format!("synced {}", shell::when_short(at, local_now))),
                        None => parts.push("not synced yet".into()),
                    }
                    PcLine { name: name.clone(), this_pc: name == &pc, line: parts.join(" · ") }
                })
                .collect();
        } else if self.engine.shared().is_none()
            && let Ok(copies) = self.engine.copies()
        {
            lists.last_copy = copies.first().and_then(|c| c.modified);
            let count = copies.len();
            lists.saves_summary = match copies.first() {
                None => "No copies yet: one is kept after each session".into(),
                Some(c) => format!("{count} cop{} · newest {}", if count == 1 { "y" } else { "ies" }, c.modified.map(|t| shell::when_short(t, local_now)).unwrap_or_default()),
            };
            for (i, copy) in copies.into_iter().enumerate() {
                lists.saves.push(SaveLine {
                    version: copy.name.clone(),
                    when: copy.modified.map(|t| shell::when_short(t, local_now)).unwrap_or_default(),
                    from: pc.clone(),
                    size: size(copy.bytes),
                    note: if i == 0 { "the newest copy".into() } else { String::new() },
                    current: false,
                    can_bring: true,
                });
                lists.rows.push(Row::File(copy.path));
            }
        }
        self.sync.lists = lists;
    }

    /// The progress question's pick refused: why, under its buttons, and on the rail.
    pub(super) fn refuse_pick(&mut self, why: &str) {
        self.sync.needs_note = Some((why.to_string(), Instant::now()));
        self.feedback.warn(why.to_string());
    }

    /// The newest export at the top of an empty network folder, when it isn't this PC's save: its
    /// name.
    fn differing_export(&self, outlook: &Outlook) -> Option<String> {
        if !outlook.index.versions.is_empty() {
            return None;
        }
        let local = self.engine.local_save().ok().flatten()?;
        let export = self.engine.exports().ok()?.into_iter().next()?;
        let bytes = fs::read(&export.path).ok()?;
        (fsx::fingerprint(&bytes) != local.sha256).then_some(export.name)
    }

    // The player's commands ----------------------------------------------------------------------

    /// Play anyway: the card's question answered, Play goes on.
    pub(super) fn play_anyway(&mut self) {
        self.sync.play_anyway = true;
        self.play_sequence();
    }

    pub(super) fn cancel_play(&mut self) {
        self.sync.pending_play = false;
        self.sync.play_anyway = false;
        self.feedback.close_card();
    }

    /// Progress versions' Open the folder: the network folder's sync folder, or this PC's copies.
    pub(super) fn open_saves_folder(&self) {
        match self.engine.shared() {
            Some(shared) => win::open_folder(&shared.sync_dir()),
            None => win::open_folder(&self.engine.local.copies_dir()),
        }
    }

    pub(super) fn open_folder(&self) {
        if let Some(shared) = self.engine.shared() {
            win::open_folder(&shared.folder);
        }
    }

    /// Home's progress question: Choose… opens the question over the window, where both sides show.
    pub(super) fn choose_progress(&mut self) {
        if self.sync.split.is_some() {
            self.sync.needs_you_open = true;
            self.sync.needs_note = None;
        }
    }

    /// The question over the window answered with later: it closes, and a Play that waited for
    /// it is off.
    pub(super) fn decide_later(&mut self) {
        self.sync.needs_you_open = false;
        self.sync.needs_note = None;
        self.sync.pending_play = false;
        self.feedback.close_card();
    }

    /// The game's program files as they are now: an update is told from the next close by them.
    pub(super) fn stamp_game_files(&mut self) {
        self.sync.files_stamp = self.game_dir().and_then(|d| super::game_files_stamp(&d));
    }
}
