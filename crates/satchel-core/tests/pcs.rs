//! Several simulated PCs sharing one NAS folder: the routines the Satchel exists for.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use satchel_core::decide::Decision;
use satchel_core::engine::{BroughtIn, Engine, Shown, SyncError};
use satchel_core::fsx::fingerprint;
use satchel_core::index::{self, Version};
use satchel_core::markers::InUse;
use satchel_core::paths::{Local, Shared};
use satchel_core::playtime::Session;
use satchel_core::settings::{Settings, SyncSettings};
use satchel_core::state::LocalState;
use std::fs;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

struct Pc {
    _dir: TempDir,
    engine: Engine,
}

fn pc(name: &str, nas: Option<&Path>) -> Pc {
    let dir = tempfile::tempdir().unwrap();
    let local = Local {
        save_dir: dir.path().join("Roaming").join("wenilla-rogue"),
        app_dir: dir.path().join("Roaming").join("SurvivorsSatchel"),
        local_dir: dir.path().join("Local").join("SurvivorsSatchel"),
    };
    fs::create_dir_all(local.game_saves()).unwrap();
    let settings = Settings {
        pc_name: name.into(),
        sync: SyncSettings { enabled: nas.is_some(), folder: nas.map(Path::to_path_buf) },
        ..Settings::default()
    };
    let mut engine = Engine::new(local, settings, LocalState::default());
    engine.read_gap = Duration::ZERO;
    Pc { _dir: dir, engine }
}

/// A save in the game's shape (`RSE1:` + base64) that carries `label`.
fn save(label: &str) -> String {
    format!("RSE1:{label:A<48}")
}

impl Pc {
    fn play(&self, label: &str) {
        fs::write(self.engine.local.live_save(), save(label)).unwrap();
    }
    fn holds(&self, label: &str) -> bool {
        fs::read_to_string(self.engine.local.live_save()).unwrap() == save(label)
    }
    /// The save on disk as last written at `at`, by this PC's clock.
    fn written_at(&self, at: DateTime<Utc>) {
        fs::File::options().write(true).open(self.engine.local.live_save()).unwrap().set_modified(at.into()).unwrap();
    }
    fn decision(&self, now: DateTime<Utc>) -> Decision {
        self.engine.outlook(now).unwrap().decision
    }
    /// What the "Needs you" question shows this PC right now.
    fn shown(&self, now: DateTime<Utc>) -> Shown {
        let outlook = self.engine.outlook(now).unwrap();
        let Decision::Split { nas } = outlook.decision else { panic!("expected a split, got {:?}", outlook.decision) };
        Shown { local: outlook.local.map(|l| l.sha256), nas }
    }
}

/// Minutes after 10:00 UTC on 29 Oct 2026.
fn t(minutes: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 29, 10, 0, 0).unwrap() + TimeDelta::minutes(minutes)
}

fn shared(nas: &Path) -> Shared {
    Shared { folder: nas.into() }
}

fn nas_index(nas: &Path) -> index::SyncIndex {
    index::read(&shared(nas)).unwrap()
}

/// PC 1 seeds version 1 with "A"; PC 2 brings it in.
fn two_pcs(nas: &Path) -> (Pc, Pc) {
    let mut one = pc("PC 1", Some(nas));
    let mut two = pc("PC 2", Some(nas));
    one.play("A");
    one.engine.send(t(0), None, None, None).unwrap();
    two.engine.bring_in(1, t(1)).unwrap();
    (one, two)
}

#[test]
fn progress_follows_the_player_across_pcs() {
    let nas = tempfile::tempdir().unwrap();
    let mut one = pc("PC 1", Some(nas.path()));
    let mut two = pc("PC 2", Some(nas.path()));

    one.play("A");
    assert_eq!(one.decision(t(0)), Decision::Send);
    let v1 = one.engine.send(t(0), Some("0.127.0".into()), None, None).unwrap().unwrap();
    assert_eq!(v1.n, 1);
    assert_eq!(one.decision(t(1)), Decision::InSync { adopt: None });
    assert_eq!(one.engine.send(t(1), None, None, None).unwrap(), None, "nothing new: nothing sent");

    // PC 2 has never played: it simply takes version 1.
    assert_eq!(two.decision(t(2)), Decision::BringIn(1));
    assert_eq!(two.engine.bring_in(1, t(2)).unwrap(), BroughtIn { version: v1, safety: None });
    assert!(two.holds("A"));

    // PC 2 plays on and sends; PC 1 brings that in, keeping a safety copy of its own save.
    two.play("B");
    let v2 = two.engine.send(t(60), None, None, None).unwrap().unwrap();
    assert_eq!(one.decision(t(61)), Decision::BringIn(v2.n));
    let BroughtIn { safety: Some(safety), .. } = one.engine.bring_in(v2.n, t(61)).unwrap() else {
        panic!("expected a swap with a safety copy");
    };
    assert_eq!(safety, "Before sync 2026-10-29 11-01-00.sav");
    assert!(one.holds("B"));
    assert_eq!(fs::read_to_string(one.engine.local.game_saves().join(&safety)).unwrap(), save("A"));
    assert_eq!(fs::read_to_string(one.engine.local.safety_dir().join(&safety)).unwrap(), save("A"));
    assert_eq!(one.decision(t(62)), Decision::InSync { adopt: None });

    let index = nas_index(nas.path());
    assert_eq!((index.newest, index.pcs.clone()), (2, vec!["PC 1".to_string(), "PC 2".to_string()]));
    assert_eq!(index.get(1).unwrap().file, "AzerothSurvivors 2026-10-29 10-00-00 (PC 1).sav");
}

#[test]
fn pcs_that_moved_on_separately_keep_both() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());

    // PC 1 plays while the NAS is away; PC 2 plays and sends.
    one.play("C");
    two.play("D");
    let v2 = two.engine.send(t(30), None, None, None).unwrap().unwrap();
    assert_eq!(one.decision(t(40)), Decision::Split { nas: v2.n });
    assert!(matches!(one.engine.send(t(40), None, None, None), Err(SyncError::MovedOn)));
    assert!(
        matches!(one.engine.bring_in(v2.n, t(40)), Err(SyncError::MovedOn)),
        "a changed save is never replaced without asking"
    );
    assert!(one.holds("C"));

    // The player picks PC 2's progress: PC 1's is kept as a version first.
    let shown = one.shown(t(41));
    let chosen = one.engine.use_nas(&shown, t(41)).unwrap();
    let index = nas_index(nas.path());
    assert_eq!(index.newest, 4);
    let (kept, picked) = (index.get(3).unwrap(), index.get(4).unwrap());
    assert_eq!(kept.sha256, fingerprint(save("C").as_bytes()));
    assert_eq!(picked.sha256, fingerprint(save("D").as_bytes()));
    assert_ne!(kept.file, picked.file, "two versions in one second get two files");
    assert_eq!(chosen.version.n, 4);
    assert!(one.holds("D"));
    assert_eq!(two.decision(t(42)), Decision::InSync { adopt: None });
}

#[test]
fn keeping_this_pcs_save_after_a_split() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());
    one.play("C");
    two.play("D");
    two.engine.send(t(30), None, None, None).unwrap();

    let shown = one.shown(t(41));
    let kept = one.engine.send(t(41), None, Some("kept after a split".into()), Some(&shown)).unwrap().unwrap();
    assert_eq!(kept.n, 3);
    assert_eq!(one.decision(t(42)), Decision::InSync { adopt: None });
    // PC 2 hasn't changed since its own send: it follows.
    assert_eq!(two.decision(t(42)), Decision::BringIn(3));
}

#[test]
fn a_choice_applies_only_to_what_was_shown() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());
    let mut three = pc("PC 3", Some(nas.path()));
    one.play("C");
    two.play("D");
    two.engine.send(t(30), None, None, None).unwrap();
    three.engine.bring_in(2, t(30)).unwrap();
    let asked = one.shown(t(31));
    assert_eq!(asked.nas, 2);

    // While the question waits, PC 3 sends version 3: neither answer applies any more.
    three.play("E");
    three.engine.send(t(32), None, None, None).unwrap();
    assert!(matches!(one.engine.use_nas(&asked, t(33)), Err(SyncError::Changed)));
    assert!(matches!(one.engine.send(t(33), None, None, Some(&asked)), Err(SyncError::Changed)));
    assert!(one.holds("C"));
    assert_eq!(nas_index(nas.path()).newest, 3, "nothing was written");

    // Asked again, with version 3 shown; then this PC's save changes before the answer.
    let asked = one.shown(t(34));
    assert_eq!(asked.nas, 3);
    one.play("C2");
    assert!(matches!(one.engine.send(t(35), None, None, Some(&asked)), Err(SyncError::Changed)));
    assert!(matches!(one.engine.use_nas(&asked, t(35)), Err(SyncError::Changed)));

    // Asked once more: now the answer goes through.
    let asked = one.shown(t(36));
    one.engine.use_nas(&asked, t(36)).unwrap();
    assert!(one.holds("E"));
    let index = nas_index(nas.path());
    assert_eq!(index.get(4).unwrap().sha256, fingerprint(save("C2").as_bytes()), "this PC's save was kept first");
}

#[test]
fn a_damaged_save_never_travels() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());
    let torn = save("B");
    let torn = &torn.as_bytes()[..torn.len() - 1];

    for bad in [&b""[..], b"{\"heroes\": [], \"gold\": 1000000000}", torn] {
        fs::write(one.engine.local.live_save(), bad).unwrap();
        assert!(matches!(one.engine.send(t(10), None, None, None), Err(SyncError::Damaged(_))));
        let mut marker = InUse { pc: "PC 1".into(), started: t(10), heartbeat: t(10), checkpoint: None, checkpointed: None };
        assert!(matches!(one.engine.checkpoint(&mut marker, t(11)), Err(SyncError::Damaged(_))));
        let outlook = one.engine.outlook(t(12)).unwrap();
        assert!(outlook.local.unwrap().problem.is_some());
    }
    assert_eq!(nas_index(nas.path()).newest, 1, "nothing damaged reached the NAS");
    assert!(!shared(nas.path()).live("PC 1").exists(), "nor a checkpoint");

    // Picking the NAS's progress over a damaged save brings it back; the damaged file is kept
    // only as a safety copy, never as a version.
    two.play("B");
    two.engine.send(t(20), None, None, None).unwrap();
    let shown = one.shown(t(21));
    one.engine.use_nas(&shown, t(21)).unwrap();
    assert!(one.holds("B"));
    assert_eq!(nas_index(nas.path()).newest, 2);

    // A damaged version (as an older Satchel could have sent) is never brought in.
    let share = shared(nas.path());
    let mut index = index::read(&share).unwrap();
    fs::write(share.version("damaged.sav"), torn).unwrap();
    index.push(Version {
        n: 0,
        pc: "PC 9".into(),
        file: "damaged.sav".into(),
        sha256: fingerprint(torn),
        bytes: torn.len() as u64,
        saved: t(30),
        game: None,
        note: None,
    });
    index::write(&share, &index).unwrap();
    assert_eq!(two.decision(t(31)), Decision::BringIn(3));
    assert!(matches!(two.engine.bring_in(3, t(31)), Err(SyncError::Damaged(_))));
    assert!(two.holds("B"));
    assert!(matches!(
        two.engine.restore(torn.to_vec(), "brought back".into(), None, t(32)),
        Err(SyncError::Damaged(_))
    ));
    assert_eq!(nas_index(nas.path()).newest, 3);
}

#[test]
fn a_missing_index_is_rebuilt_not_restarted() {
    let nas = tempfile::tempdir().unwrap();
    let (_one, mut two) = two_pcs(nas.path());
    two.play("B");
    two.engine.send(t(30), None, None, None).unwrap();
    two.play("C");
    two.engine.send(t(30), None, None, None).unwrap(); // same second: a " v3" file
    let share = shared(nas.path());
    let before = nas_index(nas.path());
    // The folder is tidied by hand: the index goes elsewhere, a stray empty file appears.
    fs::rename(share.index(), nas.path().join("satchel-index.json")).unwrap();
    fs::write(share.version("AzerothSurvivors 2026-10-29 10-45-00 (PC 9).sav"), b"").unwrap();

    // A third PC with its own progress asks; it doesn't seed a new version 1.
    let three = pc("PC 3", Some(nas.path()));
    three.play("Z");
    assert_eq!(three.decision(t(40)), Decision::Split { nas: 3 });
    // The rebuilt index is written back: the same versions in the same order.
    assert!(share.index().exists());
    let after = nas_index(nas.path());
    let list = |index: &index::SyncIndex| {
        index.versions.iter().map(|v| (v.n, v.pc.clone(), v.file.clone(), v.sha256.clone())).collect::<Vec<_>>()
    };
    assert_eq!(list(&after), list(&before));
    assert_eq!(after.get(3).unwrap().note.as_deref(), Some("index rebuilt"));
    // The PCs that synced before agree with it.
    assert_eq!(two.decision(t(41)), Decision::InSync { adopt: None });
}

#[test]
fn an_unsafe_shared_folder_pauses_sync() {
    let mut one = pc("PC 1", None);
    one.play("A");
    let saves = one.engine.local.game_saves();
    one.engine.settings.sync = SyncSettings { enabled: true, folder: Some(saves) };
    assert_eq!(
        one.engine.folder_problem().as_deref(),
        Some("the network folder is inside the game's progress folder")
    );
    assert!(one.engine.shared().is_none());
    assert!(matches!(one.engine.outlook(t(0)), Err(SyncError::NotConfigured)));
    assert!(matches!(one.engine.send(t(0), None, None, None), Err(SyncError::NotConfigured)));
}

#[test]
fn a_version_that_arrived_partly_is_never_brought_in() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());
    one.play("B");
    one.engine.send(t(30), None, None, None).unwrap();
    let file = shared(nas.path()).version(&nas_index(nas.path()).get(2).unwrap().file);
    let whole = fs::read(&file).unwrap();

    // Cut short, changed after it was listed, or not there at all: refused, nothing replaced.
    fs::write(&file, &whole[..whole.len() / 2]).unwrap();
    assert!(two.engine.bring_in(2, t(40)).is_err());
    fs::write(&file, save("X")).unwrap();
    assert!(two.engine.bring_in(2, t(41)).is_err());
    fs::remove_file(&file).unwrap();
    assert!(two.engine.bring_in(2, t(42)).is_err());
    assert!(two.holds("A"));
    assert_eq!(two.decision(t(43)), Decision::BringIn(2), "this PC's memory didn't move");

    // Once the whole file is there, it comes in.
    fs::write(&file, &whole).unwrap();
    two.engine.bring_in(2, t(44)).unwrap();
    assert!(two.holds("B"));
}

#[test]
fn a_crash_mid_swap_leaves_the_old_save_and_heals() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());
    one.play("B");
    one.engine.send(t(30), None, None, None).unwrap();

    // A crash during a swap left a half-written temporary file beside the save.
    let leftover = two.engine.local.save_dir.join("characters.txt.satchel-tmp");
    fs::write(&leftover, &save("B").as_bytes()[..20]).unwrap();
    assert!(two.holds("A"));
    assert_eq!(two.decision(t(40)), Decision::BringIn(2));
    two.engine.bring_in(2, t(40)).unwrap();
    assert!(two.holds("B"));
    assert!(!leftover.exists());
}

#[test]
fn a_swap_that_cant_happen_changes_nothing() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());
    one.play("B");
    one.engine.send(t(30), None, None, None).unwrap();

    // Something holds the place the swap writes through (on Windows: the save held open).
    let blocker = two.engine.local.save_dir.join("characters.txt.satchel-tmp");
    fs::create_dir_all(blocker.join("held")).unwrap();
    assert!(two.engine.bring_in(2, t(40)).is_err());
    assert!(two.holds("A"));
    assert_eq!(two.decision(t(41)), Decision::BringIn(2), "this PC's memory didn't move");

    fs::remove_dir_all(&blocker).unwrap();
    two.engine.bring_in(2, t(42)).unwrap();
    assert!(two.holds("B"));
}

#[test]
fn a_crashed_pc_leaves_a_checkpoint_the_next_pc_carries_on_from() {
    let nas = tempfile::tempdir().unwrap();
    let mut one = pc("PC 1", Some(nas.path()));
    let mut laptop = pc("Laptop", Some(nas.path()));
    one.play("A");
    one.engine.send(t(0), None, None, None).unwrap();
    laptop.engine.bring_in(1, t(1)).unwrap();

    // PC 1 plays; a checkpoint goes out; then the power goes.
    let mut marker = InUse { pc: "PC 1".into(), started: t(10), heartbeat: t(10), checkpoint: None, checkpointed: None };
    one.engine.write_marker(&marker).unwrap();
    one.play("A+progress");
    assert!(one.engine.checkpoint(&mut marker, t(20)).unwrap());
    assert!(!one.engine.checkpoint(&mut marker, t(21)).unwrap(), "unchanged: no second checkpoint");

    // Ten minutes later the laptop starts.
    let outlook = laptop.engine.outlook(t(30)).unwrap();
    assert_eq!(outlook.others.len(), 1);
    assert!(outlook.others[0].stale);
    let promoted = laptop.engine.take_over_checkpoint(&outlook.others[0], t(30)).unwrap().unwrap();
    assert_eq!(promoted.pc, "PC 1");
    assert_eq!(laptop.decision(t(31)), Decision::BringIn(promoted.n));
    laptop.engine.bring_in(promoted.n, t(31)).unwrap();
    assert!(laptop.holds("A+progress"));
    assert!(laptop.engine.outlook(t(32)).unwrap().others.is_empty(), "the stale marker is gone");

    // PC 1 comes back holding the same save: nothing to ask.
    assert_eq!(one.decision(t(40)), Decision::InSync { adopt: Some(promoted.n) });
}

#[test]
fn an_old_checkpoint_never_overtakes_newer_versions() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());
    let mut marker = InUse { pc: "PC 1".into(), started: t(5), heartbeat: t(5), checkpoint: None, checkpointed: None };
    one.play("A+old");
    one.engine.checkpoint(&mut marker, t(6)).unwrap();
    // PC 2 played later and sent, without noticing PC 1's marker.
    two.play("B");
    two.engine.send(t(60), None, None, None).unwrap();

    let outlook = two.engine.outlook(t(70)).unwrap();
    assert_eq!(two.engine.take_over_checkpoint(&outlook.others[0], t(70)).unwrap(), None);
    assert_eq!(nas_index(nas.path()).newest, 2);
}

#[test]
fn a_checkpoint_older_than_the_progress_sent_at_the_close_never_becomes_the_newest() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());
    // PC 1 plays: a checkpoint, then the game writes once more as it closes and that goes up.
    let mut marker = InUse { pc: "PC 1".into(), started: t(10), heartbeat: t(10), checkpoint: None, checkpointed: None };
    one.engine.write_marker(&marker).unwrap();
    one.play("A+halfway");
    assert!(one.engine.checkpoint(&mut marker, t(20)).unwrap());
    one.play("A+atclose");
    let sent = one.engine.send(t(30), None, None, None).unwrap().unwrap();
    // The Satchel is closed while the session is still closing: a last heartbeat, no clean-up.
    marker.heartbeat = t(31);
    one.engine.write_marker(&marker).unwrap();

    let outlook = two.engine.outlook(t(40)).unwrap();
    assert!(outlook.others[0].stale);
    assert_eq!(two.engine.take_over_checkpoint(&outlook.others[0], t(40)).unwrap(), None);
    assert_eq!(nas_index(nas.path()).newest, sent.n, "what was sent at the close stays the newest");
    assert!(!shared(nas.path()).in_use("PC 1").exists(), "the stale marker is gone");
    assert_eq!(two.decision(t(41)), Decision::BringIn(sent.n));
}

#[test]
fn a_marker_from_before_checkpoint_times_counts_from_its_session_start() {
    // A crash in a session that started after the newest version: its checkpoint goes on.
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());
    let mut marker = InUse { pc: "PC 1".into(), started: t(10), heartbeat: t(10), checkpoint: None, checkpointed: None };
    one.play("A+progress");
    one.engine.checkpoint(&mut marker, t(20)).unwrap();
    marker.checkpointed = None;
    one.engine.write_marker(&marker).unwrap();
    let outlook = two.engine.outlook(t(30)).unwrap();
    assert!(two.engine.take_over_checkpoint(&outlook.others[0], t(30)).unwrap().is_some());

    // The same session sent at its close, then the Satchel was closed: the checkpoint stays put.
    let nas = tempfile::tempdir().unwrap();
    let (mut one, mut two) = two_pcs(nas.path());
    let mut marker = InUse { pc: "PC 1".into(), started: t(10), heartbeat: t(10), checkpoint: None, checkpointed: None };
    one.play("A+halfway");
    one.engine.checkpoint(&mut marker, t(20)).unwrap();
    one.play("A+atclose");
    one.engine.send(t(30), None, None, None).unwrap();
    marker.checkpointed = None;
    marker.heartbeat = t(31);
    one.engine.write_marker(&marker).unwrap();
    let outlook = two.engine.outlook(t(40)).unwrap();
    assert_eq!(two.engine.take_over_checkpoint(&outlook.others[0], t(40)).unwrap(), None);
    assert_eq!(nas_index(nas.path()).newest, 2);
}

#[test]
fn this_pcs_leftover_marker_goes_once_its_save_is_on_the_nas() {
    let nas = tempfile::tempdir().unwrap();
    let mut one = pc("PC 1", Some(nas.path()));
    let outlook = one.engine.outlook(t(0)).unwrap();
    assert!(one.engine.clear_own_leftover(&outlook, t(0)).unwrap(), "no marker: nothing to do");
    one.play("A");
    one.engine.send(t(0), None, None, None).unwrap();
    let marker = InUse { pc: "PC 1".into(), started: t(10), heartbeat: t(12), checkpoint: None, checkpointed: None };

    // Progress the NAS doesn't hold yet: the marker stays, for the other PCs to see.
    one.engine.write_marker(&marker).unwrap();
    one.play("A+unsent");
    let outlook = one.engine.outlook(t(13)).unwrap();
    assert!(!one.engine.clear_own_leftover(&outlook, t(13)).unwrap());
    assert!(shared(nas.path()).in_use("PC 1").exists());

    // Once the save is a version, the marker goes.
    one.engine.send(t(20), None, None, None).unwrap();
    let outlook = one.engine.outlook(t(21)).unwrap();
    assert!(one.engine.clear_own_leftover(&outlook, t(21)).unwrap());
    assert!(!shared(nas.path()).in_use("PC 1").exists());
    assert_eq!(nas_index(nas.path()).newest, 2, "nothing promoted");
}

#[test]
fn a_checkpoint_this_pcs_save_lost_is_promoted_when_it_starts_again() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, _two) = two_pcs(nas.path());
    let mut marker = InUse { pc: "PC 1".into(), started: t(10), heartbeat: t(10), checkpoint: None, checkpointed: None };
    one.play("A+progress");
    assert!(one.engine.checkpoint(&mut marker, t(20)).unwrap());
    // The power goes, and the save on disk is back at version 1.
    one.play("A");
    let outlook = one.engine.outlook(t(30)).unwrap();
    assert!(one.engine.clear_own_leftover(&outlook, t(30)).unwrap());
    assert_eq!(nas_index(nas.path()).newest, 2, "the checkpoint became version 2");
    assert!(!shared(nas.path()).in_use("PC 1").exists());
    assert_eq!(one.decision(t(31)), Decision::BringIn(2));
}

#[test]
fn a_checkpoint_newer_than_the_save_on_disk_becomes_a_version_before_the_send() {
    let nas = tempfile::tempdir().unwrap();
    let (mut one, _two) = two_pcs(nas.path());
    let mut marker = InUse { pc: "PC 1".into(), started: t(10), heartbeat: t(10), checkpoint: None, checkpointed: None };
    one.play("A+1");
    assert!(one.engine.checkpoint(&mut marker, t(20)).unwrap());
    one.play("A+2");
    assert!(one.engine.checkpoint(&mut marker, t(25)).unwrap());
    // The power goes and takes the game's last write: the disk holds A+1 again, written at t(21).
    one.play("A+1");
    one.written_at(t(21));

    let outlook = one.engine.outlook(t(30)).unwrap();
    assert_eq!(outlook.decision, Decision::Send, "sent first, A+1 would leave A+2 behind");
    let promoted = one.engine.promote_own_checkpoint(&outlook, t(30)).unwrap().unwrap();
    assert_eq!(promoted.sha256, fingerprint(save("A+2").as_bytes()));
    assert!(!shared(nas.path()).in_use("PC 1").exists(), "the marker is gone");
    // The player picks between the checkpoint and the save on disk, both kept.
    assert_eq!(one.decision(t(31)), Decision::Split { nas: promoted.n });
    assert!(one.holds("A+1"));

    // No power cut: the checkpoint is the save on disk, or the game wrote after it. Nothing changes.
    let nas = tempfile::tempdir().unwrap();
    let (mut one, _two) = two_pcs(nas.path());
    let mut marker = InUse { pc: "PC 1".into(), started: t(10), heartbeat: t(10), checkpoint: None, checkpointed: None };
    one.play("A+1");
    assert!(one.engine.checkpoint(&mut marker, t(20)).unwrap());
    let outlook = one.engine.outlook(t(30)).unwrap();
    assert_eq!(one.engine.promote_own_checkpoint(&outlook, t(30)).unwrap(), None);
    one.play("A+2");
    one.written_at(t(26));
    let outlook = one.engine.outlook(t(30)).unwrap();
    assert_eq!(one.engine.promote_own_checkpoint(&outlook, t(30)).unwrap(), None);
    assert!(shared(nas.path()).in_use("PC 1").exists(), "the marker stays");
    assert_eq!(nas_index(nas.path()).newest, 1);
}

#[test]
fn offline_sessions_wait_and_go_up_later() {
    let nas = tempfile::tempdir().unwrap();
    let folder = nas.path().join("not there yet");
    let one = pc("PC 1", Some(folder.as_path()));
    one.play("A");
    assert!(matches!(one.engine.outlook(t(0)), Err(SyncError::Offline)));

    let session = Session::new("PC 1", t(0), t(95), false, None);
    one.engine.record_session(&session).unwrap();
    assert!(one.engine.local.playtime_pending().exists());
    assert_eq!(one.engine.sessions().unwrap(), [session.clone()], "waiting sessions still count");

    fs::create_dir_all(&folder).unwrap(); // the NAS answers again
    one.engine.flush_playtime().unwrap();
    assert!(!one.engine.local.playtime_pending().exists());
    assert_eq!(one.engine.sessions().unwrap(), [session]);
    assert_eq!(one.decision(t(100)), Decision::Send);
}

#[test]
fn restoring_an_older_version_makes_it_the_newest() {
    let nas = tempfile::tempdir().unwrap();
    let mut one = pc("PC 1", Some(nas.path()));
    let mut two = pc("PC 2", Some(nas.path()));
    one.play("A");
    one.engine.send(t(0), None, None, None).unwrap();
    one.play("B");
    one.engine.send(t(10), None, None, None).unwrap();

    let (_, bytes) = one.engine.version_bytes(1).unwrap();
    let restored = one.engine.restore(bytes, "restored version 1".into(), None, t(20)).unwrap();
    assert_eq!(restored.version.n, 3);
    assert!(one.holds("A"));
    assert_eq!(one.decision(t(21)), Decision::InSync { adopt: None });
    assert_eq!(two.decision(t(21)), Decision::BringIn(3));
    two.engine.bring_in(3, t(22)).unwrap();
    assert!(two.holds("A"));
}

#[test]
fn one_pc_mode_keeps_copies_and_playtime_on_the_pc() {
    let mut solo = pc("PC 1", None);
    assert!(matches!(solo.engine.outlook(t(0)), Err(SyncError::NotConfigured)));
    solo.play("A");
    assert!(solo.engine.keep_copy(t(0)).unwrap().is_some());
    assert_eq!(solo.engine.keep_copy(t(1)).unwrap(), None, "unchanged: no second copy");
    solo.play("B");
    solo.engine.keep_copy(t(2)).unwrap();
    let copies = solo.engine.copies().unwrap();
    assert_eq!(copies.len(), 2);
    assert_eq!(copies[0].name, "AzerothSurvivors 2026-10-29 10-02-00.sav");
    fs::write(solo.engine.local.live_save(), b"").unwrap();
    assert_eq!(solo.engine.keep_copy(t(3)).unwrap(), None, "an empty save is never kept as a copy");

    let safety = solo.engine.restore_copy(&copies[1].path, t(4)).unwrap().unwrap();
    assert!(solo.holds("A"));
    assert_eq!(fs::read(solo.engine.local.safety_dir().join(&safety)).unwrap(), b"", "the empty one is kept");
    assert!(!solo.engine.local.game_saves().join(&safety).exists(), "but not in the game's Saves list");

    let session = Session::new("PC 1", t(0), t(30), false, None);
    solo.engine.record_session(&session).unwrap();
    assert_eq!(solo.engine.sessions().unwrap(), [session]);
}
