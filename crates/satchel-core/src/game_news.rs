//! The game's patch notes, from two read-only sources the app fetches (`net`) and this module
//! reads, remembers and merges: the game's own update index (`latest.json`, the newest build's
//! version and its one-paragraph note) and the full notes kept by hand in this repository
//! (`game/patch-notes.md`, in `CHANGELOG.md`'s format). Both are kept on this PC as received
//! (`Cache`), beside `state.json`: the ETags, when each was last asked and last answered, every
//! live note seen here by version, and the version whose panel was opened. No network here.

use crate::changelog::{self, Block, Version};
use crate::{fsx, log};
use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

const STATE_FILE: &str = "state.json";
/// The panel shows at most this many versions; older ones are on the game's Discord.
pub const PANEL_MOST: usize = 30;

/// The two sources, each fetched from one fixed address the app holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    /// The game's update index: the newest build's version and note.
    Latest,
    /// The full notes, from this repository.
    Notes,
}

impl Feed {
    /// The file the body is kept under in the cache folder.
    pub fn file_name(self) -> &'static str {
        match self {
            Feed::Latest => "latest.json",
            Feed::Notes => "patch-notes.md",
        }
    }

    /// The largest body read, in bytes; a larger one is dropped and logged.
    pub fn cap(self) -> u64 {
        match self {
            Feed::Latest => 64 << 10,
            Feed::Notes => 512 << 10,
        }
    }

    /// How the log and About name it.
    pub fn name(self) -> &'static str {
        match self {
            Feed::Latest => "the game's update server",
            Feed::Notes => "the patch-notes file",
        }
    }
}

/// Each source is asked for at most this often, counted from the last attempt, across restarts.
pub fn every() -> TimeDelta {
    TimeDelta::minutes(30)
}

/// Time to ask: never asked, or asked `every()` ago. A last try in the future means the clock
/// went back, which is nothing to wait for.
pub fn due(s: &Source, now: DateTime<Utc>) -> bool {
    s.tried.is_none_or(|tried| tried > now || now - tried >= every())
}

/// The game's update index, as far as the Satchel reads it: the newest build and its note. The
/// rest (its size, checksum, parts) is the game's business.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Latest {
    pub version: String,
    pub notes: String,
}

/// `latest.json` read: a body that isn't that (or names no version) counts as no answer.
pub fn parse_latest(body: &str) -> Result<Latest, String> {
    let latest: Latest = serde_json::from_str(body).map_err(|e| format!("not the game's update index ({e}); it began: {}", head(body)))?;
    let latest = Latest { version: latest.version.trim().to_string(), notes: latest.notes.trim().to_string() };
    if latest.version.is_empty() {
        return Err(format!("the game's update index names no version; it began: {}", head(body)));
    }
    Ok(latest)
}

/// The first 200 characters of a body, for the log.
fn head(body: &str) -> String {
    body.chars().take(200).map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// A `Last-Modified` header read; `None` for anything that isn't an HTTP date.
pub fn http_date(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(text.trim()).ok().map(|at| at.with_timezone(&Utc))
}

/// One source's bookkeeping.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    /// Its ETag, sent back as `If-None-Match`: only while the body it names is kept.
    pub etag: Option<String>,
    /// The last request, answered or not.
    pub tried: Option<DateTime<Utc>>,
    /// The last answer, 200 or 304.
    pub answered: Option<DateTime<Utc>>,
}

/// A live note as this PC saw it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seen {
    /// The note's text, one paragraph.
    pub notes: String,
    /// The `Last-Modified` it came with, the only date the update index gives.
    pub modified: Option<DateTime<Utc>>,
    /// When this PC first saw this version.
    pub first_seen: Option<DateTime<Utc>>,
}

/// What this PC remembers about the game's notes (`state.json` in the cache folder).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub latest: Source,
    pub notes: Source,
    /// Version → the live note seen for it. A version's entry is its own: a newer or an older
    /// build arriving adds its own entry and touches no other.
    pub seen: BTreeMap<String, Seen>,
    /// The version whose panel was opened last: the Play line is marked as new until the panel
    /// is opened for the version it names.
    pub opened: Option<String>,
}

impl State {
    pub fn source(&self, feed: Feed) -> &Source {
        match feed {
            Feed::Latest => &self.latest,
            Feed::Notes => &self.notes,
        }
    }

    fn source_mut(&mut self, feed: Feed) -> &mut Source {
        match feed {
            Feed::Latest => &mut self.latest,
            Feed::Notes => &mut self.notes,
        }
    }
}

/// A live note read: its version's entry, kept under that version alone. The same version again
/// updates its text and date (the developer edited the note) and keeps when it was first seen;
/// another version's entry is never touched, so an older build arriving after a newer one (a
/// rollback) leaves the newer one's text as it was.
pub fn note_seen(s: &mut State, latest: &Latest, modified: Option<DateTime<Utc>>, now: DateTime<Utc>) {
    let entry = s.seen.entry(latest.version.clone()).or_default();
    entry.notes = latest.notes.clone();
    entry.modified = modified.or(entry.modified);
    entry.first_seen.get_or_insert(now);
}

/// A version as numbers, `0.141.0` → [0, 141, 0], as the catalog tool sorts tags: the leading
/// digits of each part up to the first part that isn't a number. A version without a number is
/// empty, and sorts last.
pub fn version_key(version: &str) -> Vec<u64> {
    let start = version.find(|c: char| c.is_ascii_digit()).unwrap_or(version.len());
    version[start..].split(|c: char| !c.is_ascii_digit()).take_while(|n| !n.is_empty()).map(|n| n.parse().unwrap_or(u64::MAX)).collect()
}

/// `a` before `b` when `a` is the newer version; versions without a number come last, in the
/// order they were given.
pub fn newest_first(a: &str, b: &str) -> Ordering {
    let (a, b) = (version_key(a), version_key(b));
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => b.cmp(&a),
    }
}

/// One version as the panel shows it: the file's section when it has one, else the live note
/// alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Note {
    pub version: String,
    /// The file's heading date, else the day the live note was last modified, else none.
    pub date: Option<NaiveDate>,
    /// The paragraph: the file's, else the live note's; empty when neither has one.
    pub paragraph: String,
    /// The detail lines after the paragraph, as the file has them; empty when the file lacks the
    /// version, or has only its paragraph.
    pub blocks: Vec<Block>,
    /// The file has this version.
    pub in_file: bool,
}

/// The file's versions and every live note seen that the file lacks, newest first by version
/// number (`newest_first`; the file's order breaks ties).
pub fn merge(file: &[Version], seen: &BTreeMap<String, Seen>) -> Vec<Note> {
    let mut notes: Vec<Note> = file
        .iter()
        .map(|v| {
            let live = seen.get(&v.version);
            let (paragraph, blocks) = match v.blocks.split_first() {
                Some((Block::Paragraph(p), rest)) => (p.clone(), rest.to_vec()),
                _ => (live.map(|s| s.notes.clone()).unwrap_or_default(), v.blocks.clone()),
            };
            Note {
                version: v.version.clone(),
                date: v.date.or_else(|| live.and_then(|s| s.modified).map(|at| at.date_naive())),
                paragraph,
                blocks,
                in_file: true,
            }
        })
        .collect();
    for (version, live) in seen {
        if notes.iter().any(|n| n.version == *version) {
            continue;
        }
        notes.push(Note { version: version.clone(), date: live.modified.map(|at| at.date_naive()), paragraph: live.notes.clone(), blocks: Vec::new(), in_file: false });
    }
    notes.sort_by(|a, b| newest_first(&a.version, &b.version));
    notes
}

/// The newest build's note is new on this PC until the panel is opened while the Play line (0.8)
/// or the rail's Build line (0.9) names that version: `State::opened` remembers the version the
/// panel was opened for. No build known: nothing is new.
pub fn newest_is_new(s: &State, version: &str) -> bool {
    !version.is_empty() && s.opened.as_deref() != Some(version)
}

/// Home's What's new card shows a note's paragraph and its first three lines; the panel has the
/// rest.
pub const CARD_LINES: usize = 3;

/// What Home's card shows of a note (`blocks` as the panel has them, its paragraph first when it
/// has one): the paragraph and the first `CARD_LINES` lines after it, and whether lines were left
/// out (the card's link then reads "More" instead of "Older notes").
pub fn card<T>(blocks: &[T], is_paragraph: impl Fn(&T) -> bool) -> (&[T], bool) {
    let lead = usize::from(blocks.first().is_some_and(is_paragraph));
    let shown = (lead + CARD_LINES).min(blocks.len());
    (&blocks[..shown], blocks.len() > shown)
}

/// The versions the panel shows, `PANEL_MOST` at most, and whether older ones were left out (the
/// panel's last line then says they are on the game's Discord).
pub fn panel_notes(merged: &[Note]) -> (&[Note], bool) {
    let shown = merged.len().min(PANEL_MOST);
    (&merged[..shown], merged.len() > PANEL_MOST)
}

/// Both sources on this PC (`%LOCALAPPDATA%\SurvivorsSatchel\game-news`): each body exactly as
/// received, beside `state.json`. Only a cache: a file that can't be read is dropped and fetched
/// again. Writes are atomic, as the rest of the cache's.
#[derive(Debug)]
pub struct Cache {
    dir: PathBuf,
    /// `state.json` as last read or written: a write that fails mustn't make the next tick ask
    /// again.
    state: Mutex<Option<State>>,
}

impl Cache {
    pub fn open(dir: PathBuf) -> Cache {
        Cache { dir, state: Mutex::new(None) }
    }

    /// What this PC remembers. An ETag only while the body it names is kept: without the body, a
    /// 304 would leave nothing to read.
    pub fn state(&self) -> State {
        let mut memo = self.memo();
        let state = memo.get_or_insert_with(|| self.read_state());
        for feed in [Feed::Latest, Feed::Notes] {
            if !self.dir.join(feed.file_name()).is_file() {
                state.source_mut(feed).etag = None;
            }
        }
        state.clone()
    }

    /// The update index as last received, when it is kept and reads.
    pub fn latest(&self) -> Option<Latest> {
        self.read(Feed::Latest, parse_latest)
    }

    /// The notes file as last received, as versions; `None` when it isn't kept.
    pub fn notes(&self) -> Option<Vec<Version>> {
        self.read(Feed::Notes, |body| Ok(changelog::versions(body)))
    }

    fn read<T>(&self, feed: Feed, parse: impl Fn(&str) -> Result<T, String>) -> Option<T> {
        let path = self.dir.join(feed.file_name());
        let read = match self.body(&path) {
            Ok(None) => return None,
            Ok(Some(body)) => parse(&body),
            Err(why) => Err(why),
        };
        match read {
            Ok(value) => Some(value),
            Err(why) => {
                self.drop_file(&path, &why);
                self.change(|s| s.source_mut(feed).etag = None);
                None
            }
        }
    }

    /// A 200 from the update server: the body kept as received, the version's note remembered
    /// under `seen`, and the bookkeeping. A body over the cap, or one that isn't the index, is an
    /// `Err` for the log and counts as a failed try.
    pub fn store_latest(&self, body: &str, etag: Option<&str>, modified: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Result<Latest, String> {
        let latest = match self.within_cap(Feed::Latest, body).and_then(|()| parse_latest(body)) {
            Ok(latest) => latest,
            Err(why) => {
                self.change(|s| s.latest.tried = Some(now));
                return Err(format!("{}: {why}", Feed::Latest.name()));
            }
        };
        let kept = self.write(&self.dir.join(Feed::Latest.file_name()), body);
        self.change(|s| {
            s.latest.etag = etag.filter(|_| kept).and_then(etag_ok);
            s.latest.answered = Some(now);
            s.latest.tried = Some(now);
            note_seen(s, &latest, modified, now);
        });
        Ok(latest)
    }

    /// A 200 for the notes file: the body kept as received (any text is; the sections it holds are
    /// the versions), and the bookkeeping. A body over the cap is an `Err` and a failed try.
    pub fn store_notes(&self, body: &str, etag: Option<&str>, now: DateTime<Utc>) -> Result<Vec<Version>, String> {
        if let Err(why) = self.within_cap(Feed::Notes, body) {
            self.change(|s| s.notes.tried = Some(now));
            return Err(format!("{}: {why}", Feed::Notes.name()));
        }
        let kept = self.write(&self.dir.join(Feed::Notes.file_name()), body);
        self.change(|s| {
            s.notes.etag = etag.filter(|_| kept).and_then(etag_ok);
            s.notes.answered = Some(now);
            s.notes.tried = Some(now);
        });
        Ok(changelog::versions(body))
    }

    fn within_cap(&self, feed: Feed, body: &str) -> Result<(), String> {
        if body.len() as u64 > feed.cap() {
            return Err(format!("its answer is {} KB, over the {} KB cap, so it was dropped", body.len() / 1024, feed.cap() / 1024));
        }
        Ok(())
    }

    /// A 304: the body kept is still the one.
    pub fn touch(&self, feed: Feed, now: DateTime<Utc>) {
        self.change(|s| {
            let source = s.source_mut(feed);
            source.answered = Some(now);
            source.tried = Some(now);
        });
    }

    /// No answer (a 429, a 5xx, the network): the next try waits.
    pub fn tried(&self, feed: Feed, now: DateTime<Utc>) {
        self.change(|s| s.source_mut(feed).tried = Some(now));
    }

    /// The panel was opened while the Play line named `version`: the line isn't new anymore.
    pub fn opened(&self, version: &str) {
        if self.state().opened.as_deref() != Some(version) {
            self.change(|s| s.opened = Some(version.to_string()));
        }
    }

    fn memo(&self) -> MutexGuard<'_, Option<State>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn read_state(&self) -> State {
        let path = self.dir.join(STATE_FILE);
        let read = match self.body(&path) {
            Ok(None) => return State::default(),
            Ok(Some(body)) => serde_json::from_str(&body).map_err(|e| e.to_string()),
            Err(why) => Err(why),
        };
        read.unwrap_or_else(|why| {
            self.drop_file(&path, &why);
            State::default()
        })
    }

    /// Changes the state, in memory and in `state.json`.
    fn change(&self, edit: impl FnOnce(&mut State)) {
        let mut memo = self.memo();
        let state = memo.get_or_insert_with(|| self.read_state());
        edit(state);
        match serde_json::to_vec_pretty(&*state) {
            Ok(bytes) => {
                self.write_bytes(&self.dir.join(STATE_FILE), &bytes);
            }
            Err(e) => log!("the game's notes: their state wasn't written: {e}"),
        }
    }

    /// A cached file as text: `Ok(None)` when it isn't there (or can't be opened now), `Err` when
    /// it isn't text.
    fn body(&self, path: &Path) -> Result<Option<String>, String> {
        match fsx::read_optional(path) {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => String::from_utf8(bytes).map(Some).map_err(|_| "not text".to_string()),
            Err(e) => {
                log!("the game's notes: {} not read: {e}", path.display());
                Ok(None)
            }
        }
    }

    fn write(&self, path: &Path, body: &str) -> bool {
        self.write_bytes(path, body.as_bytes())
    }

    fn write_bytes(&self, path: &Path, bytes: &[u8]) -> bool {
        match fsx::write_atomic(path, bytes) {
            Ok(()) => true,
            Err(e) => {
                log!("the game's notes: {} not written: {e}", path.display());
                false
            }
        }
    }

    fn drop_file(&self, path: &Path, why: &str) {
        log!("the game's notes: {} can't be read ({why}); dropped, to be fetched again", path.display());
        if let Err(e) = fsx::remove_if_present(path) {
            log!("the game's notes: {} not removed: {e}", path.display());
        }
    }
}

/// An ETag worth keeping: one line of plain characters, not too long.
fn etag_ok(etag: &str) -> Option<String> {
    let etag = etag.trim();
    (!etag.is_empty() && etag.len() <= 256 && etag.chars().all(|c| (' '..='~').contains(&c))).then(|| etag.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const LATEST: &str = r#"{"version":"0.141.0","notes":"Scholomance's bosses now drop more often.","size":512345678,"sha256":"ab","parts":["a","b"],"later":{"x":1}}"#;
    const NOTES: &str = "# Notes\n\nthe header\n\n## 0.141.0 (2026-11-04)\n\nBosses drop more often.\n\n- Loot: 25%.\n- Gandling is unchanged.\n\n## 0.139.2\n\nAddons can mark the world.\n\n- New for addons: anchors.\n";

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 4, hour, 0, 0).unwrap()
    }

    #[test]
    fn the_update_index_is_read_and_anything_else_is_refused() {
        let latest = parse_latest(LATEST).unwrap();
        assert_eq!(latest, Latest { version: "0.141.0".into(), notes: "Scholomance's bosses now drop more often.".into() });
        assert_eq!(parse_latest(r#"{"version":" 0.2 ","notes":""}"#).unwrap(), Latest { version: "0.2".into(), notes: String::new() });
        let why = parse_latest("<html>not json").unwrap_err();
        assert!(why.contains("not the game's update index") && why.contains("<html>not json"), "{why}");
        assert!(parse_latest(r#"{"notes":"no version"}"#).unwrap_err().contains("names no version"));
        assert!(parse_latest("").is_err());
    }

    #[test]
    fn an_http_date_is_read() {
        assert_eq!(http_date("Wed, 04 Nov 2026 12:30:00 GMT"), Some(Utc.with_ymd_and_hms(2026, 11, 4, 12, 30, 0).unwrap()));
        assert_eq!(http_date("yesterday"), None);
    }

    #[test]
    fn versions_sort_as_numbers_and_unknown_shapes_last() {
        assert_eq!(version_key("0.141.0"), [0, 141, 0]);
        assert_eq!(version_key("v0.139.2-hotfix"), [0, 139, 2]);
        assert!(version_key("soon").is_empty());
        let mut versions = vec!["0.139.2", "soon", "0.141.0", "0.9.9", "0.140.0", "later"];
        versions.sort_by(|a, b| newest_first(a, b));
        assert_eq!(versions, ["0.141.0", "0.140.0", "0.139.2", "0.9.9", "soon", "later"]);
    }

    #[test]
    fn the_merge_takes_the_file_first_and_the_live_notes_for_the_rest() {
        let file = changelog::versions(NOTES);
        let mut seen = BTreeMap::new();
        // In both: the file's paragraph and lines, the file's date.
        seen.insert("0.141.0".to_string(), Seen { notes: "The live note, which the file replaces.".into(), modified: Some(at(9)), first_seen: Some(at(9)) });
        // Only seen live: its paragraph alone, the day it was modified.
        seen.insert("0.140.0".to_string(), Seen { notes: "A new zone opens.".into(), modified: Some(at(1)), first_seen: Some(at(2)) });
        let notes = merge(&file, &seen);
        assert_eq!(notes.iter().map(|n| n.version.as_str()).collect::<Vec<_>>(), ["0.141.0", "0.140.0", "0.139.2"]);
        assert_eq!(
            notes[0],
            Note {
                version: "0.141.0".into(),
                date: NaiveDate::from_ymd_opt(2026, 11, 4),
                paragraph: "Bosses drop more often.".into(),
                blocks: vec![Block::Bullet("Loot: 25%.".into()), Block::Bullet("Gandling is unchanged.".into())],
                in_file: true,
            }
        );
        assert_eq!(
            notes[1],
            Note { version: "0.140.0".into(), date: NaiveDate::from_ymd_opt(2026, 11, 4), paragraph: "A new zone opens.".into(), blocks: Vec::new(), in_file: false }
        );
        // A heading without a date, not seen live: no date.
        assert_eq!(notes[2].date, None);
        assert!(notes[2].in_file);
        assert_eq!(notes[2].paragraph, "Addons can mark the world.");

        // An empty file: the live notes alone, newest first.
        let live_only = merge(&[], &seen);
        assert_eq!(live_only.iter().map(|n| (n.version.as_str(), n.in_file)).collect::<Vec<_>>(), [("0.141.0", false), ("0.140.0", false)]);
        assert_eq!(live_only[0].paragraph, "The live note, which the file replaces.");
        assert!(merge(&[], &BTreeMap::new()).is_empty());

        // A section that starts with its lines (no paragraph): the live note's paragraph, if any.
        let no_paragraph = changelog::versions("## 0.141.0\n- only lines\n");
        let merged = merge(&no_paragraph, &seen);
        assert_eq!(merged[0].paragraph, "The live note, which the file replaces.");
        assert_eq!(merged[0].blocks, [Block::Bullet("only lines".into())]);
    }

    #[test]
    fn each_source_is_due_after_thirty_minutes() {
        let mut source = Source::default();
        assert!(due(&source, at(10)), "never asked");
        source.tried = Some(at(10));
        assert!(!due(&source, at(10)));
        assert!(!due(&source, at(10) + TimeDelta::minutes(29)));
        assert!(due(&source, at(10) + TimeDelta::minutes(30)));
        assert!(due(&source, at(9)), "a try in the future: the clock went back");
    }

    #[test]
    fn the_state_round_trips_and_a_version_keeps_its_own_note() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path().join("game-news"));
        assert_eq!(cache.state(), State::default());
        assert_eq!(cache.latest(), None);
        assert_eq!(cache.notes(), None);

        let newer = cache.store_latest(LATEST, Some("\"etag-1\""), Some(at(9)), at(10)).unwrap();
        assert_eq!(newer.version, "0.141.0");
        let state = cache.state();
        assert_eq!(state.latest, Source { etag: Some("\"etag-1\"".into()), tried: Some(at(10)), answered: Some(at(10)) });
        assert_eq!(state.seen["0.141.0"], Seen { notes: newer.notes.clone(), modified: Some(at(9)), first_seen: Some(at(10)) });
        assert_eq!(cache.latest(), Some(newer.clone()));

        // An older build (a rollback) adds its own entry and leaves the newer one as it was.
        let older = cache.store_latest(r#"{"version":"0.140.0","notes":"A new zone opens."}"#, None, Some(at(1)), at(11)).unwrap();
        let again = Cache::open(dir.path().join("game-news"));
        let state = again.state();
        assert_eq!(state.seen["0.141.0"].notes, newer.notes);
        assert_eq!(state.seen["0.140.0"], Seen { notes: older.notes.clone(), modified: Some(at(1)), first_seen: Some(at(11)) });
        assert_eq!(state.latest.etag, None, "no ETag came with it");
        assert_eq!(again.latest(), Some(older));

        // The same version again: the edited text and date, the first sighting kept.
        again.store_latest(r#"{"version":"0.141.0","notes":"Edited."}"#, Some("\"etag-2\""), Some(at(12)), at(13)).unwrap();
        let seen = &again.state().seen["0.141.0"];
        assert_eq!((seen.notes.as_str(), seen.modified, seen.first_seen), ("Edited.", Some(at(12)), Some(at(10))));

        // The notes file, a 304 and a failed try.
        let versions = again.store_notes(NOTES, Some("\"n-1\""), at(14)).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(again.notes().map(|v| v.len()), Some(2));
        again.touch(Feed::Notes, at(15));
        again.tried(Feed::Latest, at(16));
        let state = again.state();
        assert_eq!(state.notes, Source { etag: Some("\"n-1\"".into()), tried: Some(at(15)), answered: Some(at(15)) });
        assert_eq!(state.latest.tried, Some(at(16)));
        assert_eq!(state.latest.answered, Some(at(13)));
        assert!(!due(&state.latest, at(16) + TimeDelta::minutes(10)));

        // The panel opened for the version shown.
        again.opened("0.141.0");
        assert_eq!(Cache::open(dir.path().join("game-news")).state().opened.as_deref(), Some("0.141.0"));

        // The ETag stands for the body kept: the body gone, no ETag is sent.
        std::fs::remove_file(dir.path().join("game-news").join("patch-notes.md")).unwrap();
        assert_eq!(again.state().notes.etag, None);
        assert_eq!(again.notes(), None);
    }

    #[test]
    fn a_body_that_isnt_the_index_or_is_too_large_counts_as_a_try() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path().to_path_buf());
        let why = cache.store_latest("<html>", None, None, at(10)).unwrap_err();
        assert!(why.contains("<html>"), "{why}");
        assert_eq!(cache.state().latest, Source { etag: None, tried: Some(at(10)), answered: None });
        assert_eq!(cache.latest(), None);

        let large = format!(r#"{{"version":"0.1.0","notes":"{}"}}"#, "x".repeat(Feed::Latest.cap() as usize));
        assert!(cache.store_latest(&large, None, None, at(11)).unwrap_err().contains("over the 64 KB cap"));
        assert_eq!(cache.state().latest.tried, Some(at(11)));
        let large = "#".repeat(Feed::Notes.cap() as usize + 1);
        assert!(cache.store_notes(&large, None, at(12)).unwrap_err().contains("over the 512 KB cap"));
        assert_eq!(cache.state().notes, Source { etag: None, tried: Some(at(12)), answered: None });
        assert_eq!(cache.notes(), None);
        // Right at the cap is fine.
        assert!(cache.store_notes(&"#".repeat(Feed::Notes.cap() as usize), None, at(13)).is_ok());
    }

    /// The caps 0.8 shipped: 64 KB for the update index, 512 KB for the notes file. The cache keeps
    /// a body of up to the cap and drops a longer one, as a try. The boundary on the wire is one
    /// byte lower: the network thread reads with ureq's `limit(cap)`, whose reader fails on the read
    /// after `cap` bytes, so a body of exactly the cap never reaches the cache and the largest body
    /// kept from the network is `cap - 1` bytes (that read is the game-news fetching, left as 0.8
    /// shipped it).
    #[test]
    fn the_body_caps_are_64_and_512_kb() {
        assert_eq!(Feed::Latest.cap(), 64 * 1024);
        assert_eq!(Feed::Notes.cap(), 512 * 1024);
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path().to_path_buf());
        // An index body padded to exactly the cap with a field the Satchel ignores.
        let head = r#"{"version":"0.1.0","notes":"n","pad":""#;
        let at_cap = format!("{head}{}\"}}", "x".repeat(Feed::Latest.cap() as usize - head.len() - 2));
        assert_eq!(at_cap.len() as u64, Feed::Latest.cap());
        assert_eq!(cache.store_latest(&at_cap, None, None, at(10)).unwrap().version, "0.1.0");
        assert_eq!(cache.state().latest.answered, Some(at(10)));
        // The largest body the network delivers: one byte under the cap.
        let under = format!("{head}{}\"}}", "x".repeat(Feed::Latest.cap() as usize - head.len() - 3));
        assert_eq!(under.len() as u64, Feed::Latest.cap() - 1);
        assert!(cache.store_latest(&under, None, None, at(10)).is_ok());
        let over = format!("{head}{}\"}}", "x".repeat(Feed::Latest.cap() as usize - head.len() - 1));
        assert_eq!(over.len() as u64, Feed::Latest.cap() + 1);
        assert!(cache.store_latest(&over, None, None, at(11)).unwrap_err().contains("over the 64 KB cap"));
        assert_eq!(cache.state().latest.tried, Some(at(11)), "a dropped body counts as a try");
        assert_eq!(cache.state().latest.answered, Some(at(10)), "and not as an answer");
        assert_eq!(cache.latest().map(|l| l.version), Some("0.1.0".into()), "the body kept stays");
        let notes_at_cap = format!("## 0.1.0\n\n{}\n", "n".repeat(Feed::Notes.cap() as usize - 11));
        assert_eq!(notes_at_cap.len() as u64, Feed::Notes.cap());
        assert_eq!(cache.store_notes(&notes_at_cap, None, at(12)).unwrap().len(), 1);
        let notes_over = format!("{notes_at_cap}\n");
        assert!(cache.store_notes(&notes_over, None, at(13)).unwrap_err().contains("over the 512 KB cap"));
        assert_eq!(cache.state().notes, Source { etag: None, tried: Some(at(13)), answered: Some(at(12)) });
    }

    /// The newest note is new until the panel is opened for that very version: opened for an
    /// older one, or never, it stays new; opened for it, it stops; a newer build makes it new again.
    #[test]
    fn the_newest_note_is_new_until_opened_for_that_version() {
        let mut state = State::default();
        assert!(!newest_is_new(&state, ""), "no build known: nothing is new");
        assert!(newest_is_new(&state, "0.141.0"));
        state.opened = Some("0.140.0".into());
        assert!(newest_is_new(&state, "0.141.0"));
        state.opened = Some("0.141.0".into());
        assert!(!newest_is_new(&state, "0.141.0"));
        assert!(newest_is_new(&state, "0.141.1"), "a newer build arrived: new again");
        // Through the cache, as the app does it.
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path().to_path_buf());
        cache.store_latest(LATEST, None, None, at(10)).unwrap();
        assert!(newest_is_new(&cache.state(), "0.141.0"));
        cache.opened("0.141.0");
        assert!(!newest_is_new(&cache.state(), "0.141.0"));
    }

    /// Home's card: the paragraph and three lines; the link says when lines were left out.
    #[test]
    fn the_card_shows_the_paragraph_and_three_lines() {
        let blocks = [(0, "p"), (2, "a"), (2, "b"), (3, "c"), (2, "d")];
        let paragraph = |b: &(i32, &str)| b.0 == 0;
        assert_eq!(card(&blocks, paragraph), (&blocks[..4], true));
        assert_eq!(card(&blocks[..4], paragraph), (&blocks[..4], false));
        assert_eq!(card(&blocks[..2], paragraph), (&blocks[..2], false));
        // A note without a paragraph: its first three lines.
        assert_eq!(card(&blocks[1..], paragraph), (&blocks[1..4], true));
        let empty: [(i32, &str); 0] = [];
        assert_eq!(card(&empty, paragraph), (&empty[..], false));
    }

    /// The panel shows 30 versions at most, the newest, and says when older ones were left out.
    #[test]
    fn the_panel_shows_thirty_versions_at_most() {
        assert_eq!(PANEL_MOST, 30);
        let note = |n: usize| Note { version: format!("0.{n}.0"), ..Note::default() };
        let few: Vec<Note> = (0..3).map(note).collect();
        let (shown, more) = panel_notes(&few);
        assert_eq!((shown.len(), more), (3, false));
        let thirty: Vec<Note> = (0..30).map(note).collect();
        let (shown, more) = panel_notes(&thirty);
        assert_eq!((shown.len(), more), (30, false));
        let mut seen = BTreeMap::new();
        for n in 0..35 {
            seen.insert(format!("0.{n}.0"), Seen { notes: format!("note {n}"), modified: None, first_seen: None });
        }
        let merged = merge(&[], &seen);
        let (shown, more) = panel_notes(&merged);
        assert_eq!((shown.len(), more), (30, true));
        assert_eq!((shown[0].version.as_str(), shown[29].version.as_str()), ("0.34.0", "0.5.0"), "the newest thirty");
        assert_eq!(panel_notes(&[]), (&[][..], false));
    }

    #[test]
    fn an_unreadable_cached_file_is_dropped_and_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path().to_path_buf());
        cache.store_latest(LATEST, Some("\"e\""), None, at(10)).unwrap();
        std::fs::write(dir.path().join("latest.json"), b"{broken").unwrap();
        assert_eq!(cache.latest(), None);
        assert!(!dir.path().join("latest.json").exists());
        assert_eq!(cache.state().latest.etag, None);
        std::fs::write(dir.path().join("state.json"), b"not json").unwrap();
        assert_eq!(Cache::open(dir.path().to_path_buf()).state(), State::default());
    }

    /// The real file: it parses, its first heading is the newest version, every section starts
    /// with its paragraph, and no line wraps a code span (`plain` runs per line).
    #[test]
    fn the_patch_notes_file_parses_newest_first() {
        let text = include_str!("../../../game/patch-notes.md");
        let versions = changelog::versions(text);
        assert!(!versions.is_empty(), "game/patch-notes.md has no version");
        let newest = versions.iter().map(|v| v.version.as_str()).min_by(|a, b| newest_first(a, b)).unwrap();
        assert_eq!(versions[0].version, newest, "the newest version comes first in game/patch-notes.md");
        for (i, v) in versions.iter().enumerate() {
            assert!(!version_key(&v.version).is_empty(), "{} isn't a version number", v.version);
            assert!(matches!(v.blocks.first(), Some(Block::Paragraph(_))), "{} doesn't start with its paragraph", v.version);
            assert!(!versions[..i].iter().any(|earlier| earlier.version == v.version), "{} is in the file twice", v.version);
        }
        for (n, line) in text.lines().enumerate() {
            assert_eq!(line.matches('`').count() % 2, 0, "game/patch-notes.md line {} wraps a code span", n + 1);
        }
    }
}
