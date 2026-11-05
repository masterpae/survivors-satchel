//! The Addons tab, the worker's side. Everything that changes AddOns happens here, on the worker
//! thread, and only with the game closed. The network thread fetches; nothing here waits for it.

use super::forge::{Held, forge_badge, forge_level, row_reason};
use super::Worker;
use super::view_model::{ago, when};
use crate::net::{Job, Lookup};
use crate::view::{AddonLine, AddonsView, LinkView, OfferLine, RemovedLine, VersionLine, VersionsView};
use crate::win;
use chrono::{DateTime, Local, TimeDelta, Utc};
use satchel_core::activity::Kind;
use satchel_core::addons::catalog::{self, Catalog, Release};
use satchel_core::addons::enabled::EnableList;
use satchel_core::addons::forge::{self, Verdict, Why, Word};
use satchel_core::addons::github;
use satchel_core::addons::install::{Addons, Incoming};
use satchel_core::addons::overview::{self, Inputs, Line, Offer, Standing, VersionAction, initials};
use satchel_core::addons::store::{Badge, Gone, Record, Store};
use satchel_core::addons::{Found, GameAddons, package, scan, toc, tree};
use satchel_core::{fsx, log, names};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// At Play, a look for new releases when the last one is older than this.
const LOOK_EVERY: TimeDelta = TimeDelta::minutes(30);
/// Downloads nobody needs anymore are deleted after this long.
const CACHE_KEEP: Duration = Duration::from_secs(14 * 24 * 3600);
/// How long a message stays on the Addons tab.
const MESSAGE_FOR: Duration = Duration::from_secs(60);
/// The largest zip file imported.
const MAX_IMPORT: u64 = 128 << 20;

/// "Add from a link", where it stands.
pub(super) enum Link {
    Idle,
    Looking,
    Found {
        lookup: Lookup,
        /// The red warning: it looks like a verified addon from someone else.
        warning: Option<String>,
        /// The repository is a verified addon's: it installs as that addon.
        verified: Option<String>,
    },
    Failed(String),
}

/// An install to make.
#[derive(Clone)]
pub(super) struct Want {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) badge: Badge,
    pub(super) repo: Option<String>,
    pub(super) release: Release,
    /// When the release was published, when that's known (`release.published` is only the time
    /// it was seen for a release from GitHub's page, a link's or an import's).
    pub(super) released: Option<DateTime<Utc>>,
    /// Installed from SurvivorsForge alone: its slug there, which the record keeps.
    pub(super) forge: Option<String>,
    /// The addon folders its zip must hold exactly: the ones SurvivorsForge lists for its own.
    pub(super) expected_folders: Option<Vec<String>>,
    /// The player OK'd it where SurvivorsForge asks first (Install anyway, Update anyway, Replace
    /// beside the reason), or it's theirs to judge (a link, a zip file): an Ask it meets when it
    /// goes in doesn't stop it.
    pub(super) ok: bool,
    /// A version picked in Versions…: kept at it once in, as Go back keeps one.
    pub(super) pin: bool,
}

/// Why an install didn't go in.
pub(super) enum Refused {
    /// SurvivorsForge asks first and the player didn't OK it: the whole sentence.
    Waits(String),
    /// SurvivorsForge says No: the whole sentence.
    Never(String),
    /// The install failed: why, to follow "<name> wasn't installed: ".
    Failed(String),
}

impl Refused {
    /// What the player reads about `name`.
    pub(super) fn said(self, name: &str) -> String {
        match self {
            Refused::Waits(text) | Refused::Never(text) => text,
            Refused::Failed(why) => format!("{name} wasn't installed: {why}"),
        }
    }
}

impl From<String> for Refused {
    fn from(why: String) -> Refused {
        Refused::Failed(why)
    }
}

impl From<&str> for Refused {
    fn from(why: &str) -> Refused {
        Refused::Failed(why.to_string())
    }
}

pub(super) struct AddonsState {
    pub(super) catalog: Catalog,
    pub(super) store: Store,
    pub(super) found: Vec<Found>,
    /// Each managed folder's fingerprint now.
    prints: BTreeMap<String, String>,
    pub(super) enabled: EnableList,
    pub(super) lines: Vec<Line>,
    pub(super) offers: Vec<Offer>,
    /// Releases newer than the catalog or the record knows, from GitHub's pages, by addon id.
    pub(super) newer: BTreeMap<String, Release>,
    /// Downloaded zips: (addon id, release tag) → checksum.
    pub(super) ready: BTreeMap<(String, String), String>,
    pub(super) fetching: BTreeSet<(String, String)>,
    /// Downloads that failed, with why; tried again after the next look or "Check now".
    pub(super) refused: BTreeMap<(String, String), String>,
    /// Installs waiting for their download (or for the game to close).
    pub(super) waiting: Vec<Want>,
    /// Addons installed again from the Removed list, by id: once one is in, its switches are set
    /// as they were, and its settings come back from the removal's backup when they went with it.
    restore: BTreeMap<String, Gone>,
    /// Why the last look failed, by addon id.
    unreachable: BTreeMap<String, String>,
    looked: Option<DateTime<Utc>>,
    looking: bool,
    link: Link,
    avrt: bool,
    other_manager: bool,
    /// AddOns' folders, the on/off list and their times: a change shows without hashing anything.
    stamp: Vec<(String, Option<SystemTime>)>,
    /// What the last adoption found, for the Installed list's footer.
    adopted_note: Option<String>,
    message: Option<(String, Instant)>,
    /// The addon whose Versions… is open (its id), until it closes.
    pub(super) versions_open: Option<String>,
}

impl AddonsState {
    pub(super) fn new(store: Store) -> AddonsState {
        AddonsState {
            catalog: Catalog::builtin(),
            store,
            found: Vec::new(),
            prints: BTreeMap::new(),
            enabled: EnableList::default(),
            lines: Vec::new(),
            offers: Vec::new(),
            newer: BTreeMap::new(),
            ready: BTreeMap::new(),
            fetching: BTreeSet::new(),
            refused: BTreeMap::new(),
            waiting: Vec::new(),
            restore: BTreeMap::new(),
            unreachable: BTreeMap::new(),
            looked: None,
            looking: false,
            link: Link::Idle,
            avrt: false,
            other_manager: false,
            stamp: Vec::new(),
            adopted_note: None,
            message: None,
            versions_open: None,
        }
    }
}

impl AddonsState {
    /// Our list of verified addons, built in.
    pub(super) fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The Installed list as last built.
    pub(super) fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// Browse's offers as last built.
    pub(super) fn offers(&self) -> &[Offer] {
        &self.offers
    }

    /// A release newer than the catalog or the record knows, from GitHub's page, for an addon.
    pub(super) fn newer(&self, id: &str) -> Option<&Release> {
        self.newer.get(id)
    }

    /// Whether an install of `id` waits for its download (or for the game to close).
    pub(super) fn waits_for(&self, id: &str) -> bool {
        self.waiting.iter().any(|w| w.id == id)
    }
}

/// The record store, or an empty one when it can't be read (kept aside as `addons.json.broken`):
/// the addons in AddOns are then recognised again from their files.
pub(super) fn load_store(path: &Path) -> Store {
    Store::load(path).unwrap_or_else(|e| {
        log!("addons.json unreadable ({e}); kept as addons.json.broken, addons recognised again");
        let _ = fs::rename(path, path.with_extension("json.broken"));
        Store::default()
    })
}

fn stamp(game: &GameAddons) -> Vec<(String, Option<SystemTime>)> {
    let mut stamp: Vec<(String, Option<SystemTime>)> = fs::read_dir(&game.addons)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.metadata().and_then(|m| m.modified()).ok()))
                .collect()
        })
        .unwrap_or_default();
    stamp.sort();
    stamp
}

/// The downloaded zip for this release, when it's in the cache.
pub(super) fn zip_ready(cache: &Path, ready: &BTreeMap<(String, String), String>, id: &str, release: &Release) -> Option<(PathBuf, String)> {
    let sha = if release.sha256.is_empty() { ready.get(&(id.to_string(), release.tag.clone()))?.clone() } else { release.sha256.clone() };
    let path = cache.join(format!("{sha}.zip"));
    path.is_file().then_some((path, sha))
}

/// The badge of a verified addon the built-in list no longer carries: its post left the verified
/// channel, so the Verified badge would claim what the list no longer says.
pub(super) const UNLISTED_BADGE: (&str, i32) = ("No longer listed", 1);

/// The badge of an addon installed from SurvivorsForge alone, when its trust level isn't shown
/// (its switch is off, or it is silent and the level would let things in).
pub(super) const FORGE_BADGE: (&str, i32) = ("SurvivorsForge", 4);

/// How long ago a version was released, under the Installed tab's Released heading: "today",
/// "3 days ago" (the heading says the rest).
fn released_when(days: i64) -> String {
    let said = overview::released_ago(days);
    said.strip_prefix("released ").unwrap_or(&said).to_string()
}

/// A Status line's colour as the window numbers it: 0 plain, 1 good news, 2 needs a look, 3 never.
fn tone_kind(tone: overview::Tone) -> i32 {
    match tone {
        overview::Tone::Plain => 0,
        overview::Tone::Good => 1,
        overview::Tone::Look => 2,
        overview::Tone::Never => 3,
    }
}

fn badge_text(badge: Option<Badge>) -> (&'static str, i32) {
    match badge {
        Some(Badge::Verified) => ("Discord Verified", 0),
        Some(Badge::Added) => ("Added by you", 1),
        Some(Badge::File) => ("From a file", 2),
        None => ("Installed by hand", 3),
    }
}

/// A release's version as installs know it: the catalog's (from the `.toc`); a release seen on
/// GitHub's page gets its `.toc`'s version when it's installed.
fn known_version(release: &Release) -> Option<String> {
    (release.interface != 0).then(|| release.version.clone()).flatten()
}

fn shown(release: &Release) -> String {
    release.shown_version().to_string()
}

/// Calendar days from `t` to today, on this PC's clock.
fn days_since(t: DateTime<Utc>) -> i64 {
    (Local::now().date_naive() - t.with_timezone(&Local).date_naive()).num_days()
}

/// A publish date as a card says it: "today", "3 days ago".
fn day_text(t: DateTime<Utc>) -> String {
    let ago = overview::released_ago(days_since(t));
    ago.strip_prefix("released ").unwrap_or(&ago).to_string()
}

/// "8,382 downloads": a count as people read it.
fn downloads_text(n: u64) -> String {
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if n == 1 { "1 download".into() } else { format!("{grouped} downloads") }
}

/// "2.1 MB", "640 KB": a file's size as people read it.
fn size_text(bytes: u64) -> String {
    if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / (1 << 20) as f64) } else { format!("{} KB", bytes.div_ceil(1024)) }
}

/// The game versions a version says it works with, when it says: "for the game 1.0 to 1.2".
fn game_text(game: &(Option<String>, Option<String>)) -> Option<String> {
    let clean = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    match (clean(&game.0), clean(&game.1)) {
        (Some(from), Some(to)) if from == to => Some(format!("for the game {from}")),
        (Some(from), Some(to)) => Some(format!("for the game {from} to {to}")),
        (Some(from), None) => Some(format!("for the game {from} and later")),
        (None, Some(to)) => Some(format!("for the game up to {to}")),
        (None, None) => None,
    }
}

/// A version's word as its SurvivorsForge badge says it, with the badge's kind.
fn version_level(word: Word) -> (&'static str, i32) {
    match word {
        Word::Verified => ("verified", 4),
        Word::Scanned => ("scanned", 4),
        Word::Flagged => ("flagged", 5),
        Word::Unscanned => ("unscanned", 5),
        Word::Blocked => ("blocked", 6),
        Word::SourceChanged => ("source changed", 6),
    }
}

/// What a scan found, counted: "nothing found", "1 critical finding and 2 warnings".
fn found_text(counts: forge::Counts) -> String {
    let counted = |n: u32, what: &str| if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") };
    match (counts.critical, counts.warn) {
        (0, 0) => "nothing found".into(),
        (0, warn) => counted(warn, "warning"),
        (critical, 0) => counted(critical, "critical finding"),
        (critical, warn) => format!("{} and {}", counted(critical, "critical finding"), counted(warn, "warning")),
    }
}

/// The day the built-in list was last compared with the game's verified-addons channel, as shown:
/// "31 Oct 2026" (as written when it isn't a date); empty when the catalog doesn't say.
fn discord_day(day: Option<&str>) -> String {
    let Some(day) = day.map(str::trim).filter(|d| !d.is_empty()) else { return String::new() };
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").map_or_else(|_| day.to_string(), |d| d.format("%-d %b %Y").to_string())
}

impl Worker {
    pub(super) fn game_addons(&self) -> Option<GameAddons> {
        let dir = self.game_dir()?;
        Some(GameAddons::of(&dir, std::env::var_os("BENILLA_HOME").map(PathBuf::from)))
    }

    /// Reads AddOns again: what's there, each managed folder's fingerprint and the on/off list.
    /// Copies that are exactly a verified addon's release are adopted; records another tool moved
    /// follow it.
    pub(super) fn addons_scan(&mut self) {
        let now = Utc::now();
        let avrt = self.game_dir().is_some_and(|d| d.join("avrt.dll").is_file());
        let Some(game) = self.game_addons() else {
            let a = &mut self.addons;
            a.found.clear();
            a.prints.clear();
            a.stamp.clear();
            self.addons_lines();
            return;
        };
        let mut said: Vec<String> = Vec::new();
        {
            let a = &mut self.addons;
            a.stamp = stamp(&game);
            a.avrt = avrt;
            a.other_manager = game.addons.join(".survivorsforge").is_dir();
            a.found = scan(&game.addons).unwrap_or_else(|e| {
                log!("AddOns not read: {e}");
                Vec::new()
            });
            a.enabled = EnableList::read(&game.enable_list()).unwrap_or_else(|e| {
                log!("the game's on/off list not read: {e}");
                EnableList::default()
            });
            let adopted = catalog::adopt(&a.catalog, &game, &a.store, &a.found, now);
            if !adopted.is_empty() {
                let names: Vec<String> = adopted.iter().map(|r| format!("{} {}", r.name, r.shown_version())).collect();
                a.adopted_note = Some(match adopted.len() {
                    1 => format!("{} matched its GitHub release exactly.", names[0]),
                    n => format!("{n} addons matched their GitHub releases exactly: {}.", names.join(", ")),
                });
                for record in adopted {
                    said.push(format!("{} {} recognised: its files are exactly its GitHub release", record.name, record.shown_version()));
                    a.store.put(record);
                }
            }
            let managed: Vec<String> = a.store.addons.iter().flat_map(|r| r.folders.keys().cloned()).collect();
            a.prints = a
                .found
                .iter()
                .filter(|f| f.toc.is_some() && managed.iter().any(|m| m.eq_ignore_ascii_case(&f.folder)))
                .filter_map(|f| Some((f.folder.clone(), tree::fingerprint_dir(&game.addons.join(&f.folder)).ok()?)))
                .collect();
            said.extend(overview::follow(&mut a.store, &a.catalog, &a.newer, &a.prints));
        }
        if !said.is_empty() {
            if let Err(e) = self.addons.store.save(&self.engine.local.addon_store()) {
                log!("addons.json not saved: {e}");
            }
            for text in &said {
                log!("{text}");
                self.activity(Kind::Addon, text);
            }
        }
        self.addons_lines();
    }

    /// The lists again from what is known (nothing read from disk), and downloads for any update
    /// that showed up.
    pub(super) fn addons_lines(&mut self) {
        let auto = self.engine.settings.addons.update_by_themselves;
        // SurvivorsForge's word as this PC's cache holds it: the verdicts, and its own offers.
        let forge_view = self.forge.view(self.engine.settings.addons.forge, Utc::now());
        let a = &mut self.addons;
        a.lines = overview::lines(
            &Inputs {
                catalog: &a.catalog,
                store: &a.store,
                found: &a.found,
                prints: &a.prints,
                enabled: &a.enabled,
                newer: &a.newer,
                auto,
                avrt: a.avrt,
            },
            &forge_view,
        );
        a.offers = overview::offers(&a.catalog, &a.store, &a.found, &forge_view);
        self.addons_fetch();
        // Icons for what the page in view lists now (SurvivorsForge's own come with its registry).
        self.forge_icons_again();
    }

    /// Every 60 seconds outside a session: AddOns read again only when something in it changed.
    pub(super) fn addons_tick(&mut self) {
        let changed = match self.game_addons() {
            Some(game) => stamp(&game) != self.addons.stamp,
            None => !self.addons.stamp.is_empty(),
        };
        if changed {
            self.addons_scan();
        }
        if self.addons.message.as_ref().is_some_and(|(_, at)| at.elapsed() >= MESSAGE_FOR) {
            self.addons.message = None;
        }
    }

    /// Asks GitHub's pages what's new for the installed addons; the answer comes back later.
    pub(super) fn addons_look(&mut self) {
        if self.addons.looking {
            return;
        }
        let watches = overview::watches(&self.addons.store, &self.addons.catalog, self.addons.avrt);
        if watches.is_empty() {
            self.addons.looked = Some(Utc::now());
            return;
        }
        self.addons.looking = true;
        let _ = self.net.send(Job::Look(watches));
    }

    /// Downloads for every update shown (a turned-off addon's waits until it's on, and one
    /// SurvivorsForge says No to never comes), every install waiting and every release the shared
    /// list brings at the next Play.
    pub(super) fn addons_fetch(&mut self) {
        let cache = self.engine.local.addon_cache();
        let shared = self.share_wanted();
        let a = &mut self.addons;
        let mut wanted: Vec<(String, Release)> = a
            .lines
            .iter()
            .filter(|l| !l.off && !matches!(l.verdict, Verdict::No(_)))
            .filter_map(|l| match &l.standing {
                Standing::Update { to, .. } => Some((l.id.clone(), to.clone())),
                _ => None,
            })
            .collect();
        wanted.extend(a.waiting.iter().map(|w| (w.id.clone(), w.release.clone())));
        wanted.extend(shared);
        for (id, release) in wanted {
            let key = (id.clone(), release.tag.clone());
            if release.url.is_empty()
                || zip_ready(&cache, &a.ready, &id, &release).is_some()
                || a.fetching.contains(&key)
                || a.refused.contains_key(&key)
            {
                continue;
            }
            a.fetching.insert(key);
            let sha256 = (!release.sha256.is_empty()).then(|| release.sha256.clone());
            let _ = self.net.send(Job::Fetch { id, tag: release.tag.clone(), url: release.url.clone(), sha256 });
        }
    }

    /// GitHub's answer about the installed addons' releases: what's new, by addon id.
    pub(super) fn addons_looked(&mut self, results: Vec<(String, Result<Option<Release>, String>)>) {
        let a = &mut self.addons;
        for (id, result) in results {
            match result {
                Ok(Some(release)) => {
                    log!("{id}: {} is out on GitHub", release.tag);
                    a.unreachable.remove(&id);
                    a.newer.insert(id, release);
                }
                Ok(None) => {
                    a.unreachable.remove(&id);
                    a.newer.remove(&id);
                }
                Err(e) => {
                    log!("{id}: not looked up: {e}");
                    a.unreachable.insert(id, e);
                }
            }
        }
        a.looked = Some(Utc::now());
        a.looking = false;
        a.refused.clear();
        // A release downloaded before (an earlier run) is named from its zip now.
        let cache = self.engine.local.addon_cache();
        let named: Vec<(String, String, String)> = self
            .addons
            .newer
            .iter()
            .filter(|(_, r)| r.interface == 0)
            .filter_map(|(id, r)| zip_ready(&cache, &self.addons.ready, id, r).map(|(_, sha)| (id.clone(), r.tag.clone(), sha)))
            .collect();
        for (id, tag, sha) in named {
            self.name_from_zip(&id, &tag, &sha);
        }
        self.addons_lines();
    }

    /// A download finished (or failed): the installs waiting for it go in, the game closed.
    pub(super) fn addons_fetched(&mut self, id: String, tag: String, sha256: Result<String, String>) {
        let key = (id.clone(), tag.clone());
        self.addons.fetching.remove(&key);
        match sha256 {
            Ok(sha) => {
                self.name_from_zip(&id, &tag, &sha);
                self.addons.ready.insert(key, sha);
            }
            Err(e) => {
                log!("{id} {tag} not downloaded: {e}");
                self.addons.refused.insert(key, e.clone());
                let gone: Vec<Want> = self.addons.waiting.iter().filter(|w| w.id == id && w.release.tag == tag).cloned().collect();
                self.addons.waiting.retain(|w| !(w.id == id && w.release.tag == tag));
                for want in gone {
                    self.addons_say(format!("{} wasn't downloaded: {e}", want.name));
                }
            }
        }
        // During a session they wait for its end (the game may be restarting itself).
        if !self.sync.in_session() && !self.addons.waiting.is_empty() {
            self.install_waiting();
        }
        // An incoming addon the player OK'd comes in once its download is here.
        if !self.sync.in_session() {
            self.share_follow_now(true);
        }
    }

    /// "Add from a link": what GitHub says about the repository typed.
    pub(super) fn addons_looked_up(&mut self, result: Result<Lookup, String>) {
        let catalog = &self.addons.catalog;
        self.addons.link = match result {
            Ok(lookup) => {
                let verified = catalog.addons.iter().find(|e| e.repo.eq_ignore_ascii_case(&lookup.repo)).map(|e| e.id.clone());
                let name = lookup.title.clone().unwrap_or_else(|| lookup.repo.split('/').nth(1).unwrap_or_default().to_string());
                let warning = if verified.is_some() { None } else { overview::look_alike(catalog, &lookup.repo, &name, &lookup.folders) };
                Link::Found { lookup, warning, verified }
            }
            Err(e) => Link::Failed(e),
        };
    }

    /// A release seen only on GitHub's page has no version name there (DragonUI's tags are
    /// `build-<commit>`): once its zip is here, its `.toc` names it.
    fn name_from_zip(&mut self, id: &str, tag: &str, sha: &str) {
        let zip = self.engine.local.addon_cache().join(format!("{sha}.zip"));
        let Some(release) = self.addons.newer.get_mut(id).filter(|r| r.tag == tag && r.interface == 0) else { return };
        let Ok(found) = fs::read(&zip).map_err(|e| e.to_string()).and_then(|bytes| package::inspect(&bytes).map_err(|e| e.to_string())) else {
            return;
        };
        if let Some(toc) = found.folders.iter().map(|f| &f.toc).find(|t| t.version.is_some()) {
            log!("{id}: {tag} is {}", toc.version.as_deref().unwrap_or_default());
            release.version = toc.version.clone();
            release.interface = toc.interface;
        }
    }

    /// The installs that waited for their downloads, now (the game is closed).
    fn install_waiting(&mut self) {
        let cache = self.engine.local.addon_cache();
        let (ready, still): (Vec<Want>, Vec<Want>) = std::mem::take(&mut self.addons.waiting)
            .into_iter()
            .partition(|w| zip_ready(&cache, &self.addons.ready, &w.id, &w.release).is_some());
        self.addons.waiting = still;
        if ready.is_empty() {
            return;
        }
        for want in ready {
            match self.addons_install(&want) {
                Ok(text) => self.addons_say(text),
                Err(e) => self.addons_say(e.said(&want.name)),
            }
        }
        self.addons_scan();
        self.share_look();
    }

    /// Installs `want` from its downloaded zip. The game must be closed (the caller's part).
    /// Every install ends here, so SurvivorsForge's word is asked once more, as it stands now: a
    /// download can outlast its last answer, or meet a block on the way.
    pub(super) fn addons_install(&mut self, want: &Want) -> Result<String, Refused> {
        if let Err(refused) = self.forge_judge(want) {
            // An Install again that doesn't go in leaves nothing to restore later.
            self.addons.restore.remove(&want.id);
            return Err(refused);
        }
        let game = self.game_addons().ok_or("the game's folder isn't set")?;
        let cache = self.engine.local.addon_cache();
        let (path, sha) = zip_ready(&cache, &self.addons.ready, &want.id, &want.release).ok_or("its download isn't finished")?;
        let zip = fs::read(&path).map_err(|e| e.to_string())?;
        let before = self.addons.store.get(&want.id).map(|r| r.shown_version());
        let incoming = Incoming {
            zip,
            sha256: Some(sha),
            id: want.id.clone(),
            name: want.name.clone(),
            badge: want.badge,
            repo: want.repo.clone(),
            tag: Some(want.release.tag.clone()),
            file: Some(want.release.file.clone()),
            version: known_version(&want.release),
            released: want.released,
            url: (!want.release.url.is_empty()).then(|| want.release.url.clone()),
            forge: want.forge.clone(),
            expected_folders: want.expected_folders.clone(),
            pinned: want.pin,
        };
        let mut addons = Addons { game: &game, local: &self.engine.local, store: &mut self.addons.store };
        let done = addons.install(incoming, Utc::now()).map_err(|e| e.to_string())?;
        let now = done.record.shown_version();
        if done.already_installed {
            return Ok(format!("{} {now} is already installed, exactly this version", want.name));
        }
        let mut text = match before {
            Some(old) if old != now => format!("{} {old} → {now}", want.name),
            _ => format!("{} {now} installed", want.name),
        };
        if want.pin {
            text.push_str("; it's kept at that version until you let it update");
        }
        if let Some(gone) = self.addons.restore.remove(&want.id) {
            // "Only on this PC" above all: without it the addon would go to every PC.
            if let Some(record) = self.addons.store.get_mut(&want.id) {
                record.local_only = gone.record.local_only;
                record.pinned = gone.record.pinned;
                record.settings_per_pc = gone.record.settings_per_pc;
            }
            if let Err(e) = self.addons.store.save(&self.engine.local.addon_store()) {
                log!("{}: addons.json not saved: {e}", want.name);
            }
            if !gone.settings_kept && self.restore_settings(&game, &done.record, &gone.backup) {
                text.push_str(", its settings back");
            }
        }
        match done.left_out {
            0 => {}
            1 => text.push_str(" (1 file that isn't an addon file was left out)"),
            n => text.push_str(&format!(" ({n} files that aren't addon files were left out)")),
        }
        self.activity(Kind::Addon, &text);
        Ok(text)
    }

    /// The installs that waited for SurvivorsForge's versions of an addon, decided now with what
    /// came (`forge_versions_answer`): an answer that failed counts as nothing known. What went in
    /// goes to the other PCs.
    pub(super) fn install_held(&mut self, held: Vec<(String, String, Held)>) {
        if held.is_empty() {
            return;
        }
        for (slug, tag, held) in held {
            let key = (slug, tag);
            self.forge.deciding(key.clone());
            match held {
                Held::Install { id, anyway } => self.browse_install_with(&id, anyway),
                Held::Update { id, anyway } => self.addon_update_as(&id, anyway, true),
                Held::Again { id, anyway } => self.install_again_as(&id, anyway, true),
                Held::Version { id, tag, anyway } => self.version_install_as(&id, &tag, anyway, true),
            }
            self.forge.decided(&key);
        }
        self.share_look();
    }

    /// Installs now when the download is here, else as soon as it is.
    fn install_or_wait(&mut self, want: Want) {
        let cache = self.engine.local.addon_cache();
        if zip_ready(&cache, &self.addons.ready, &want.id, &want.release).is_some() {
            match self.addons_install(&want) {
                Ok(text) => self.addons_say(text),
                Err(e) => self.addons_say(e.said(&want.name)),
            }
            self.addons_scan();
        } else {
            self.addons_say(format!("{} is downloading; it goes in once it's here and the game is closed.", want.name));
            self.addons.refused.remove(&(want.id.clone(), want.release.tag.clone()));
            self.addons.waiting.retain(|w| w.id != want.id);
            self.addons.waiting.push(want);
            self.addons_fetch();
        }
    }

    /// Before the game starts (it's closed): the updates that go in by themselves and the installs
    /// that waited. One that fails stays as it was, and the game starts anyway.
    pub(super) fn addons_before_play(&mut self) {
        if self.game_addons().is_none() {
            return;
        }
        self.addons_scan();
        // Only what SurvivorsForge lets in goes in by itself: its verdict is in `by_itself`.
        let mut due: Vec<Want> = overview::at_play(&self.addons.lines)
            .into_iter()
            .filter_map(|(line, to)| {
                let record = self.addons.store.get(&line.id)?;
                Some(Want {
                    id: line.id.clone(),
                    name: line.name.clone(),
                    badge: record.badge,
                    repo: record.repo.clone(),
                    release: to.clone(),
                    released: self.forge_published(&line.id, line.forge_slug.as_deref(), &to.tag),
                    forge: record.forge.clone(),
                    expected_folders: self.forge_folders(record.forge.as_deref()),
                    ok: false,
                    pin: false,
                })
            })
            .collect();
        // An install still waiting for its download stays waiting: it goes in once it's here.
        for want in &self.addons.waiting {
            if !due.iter().any(|d| d.id == want.id) {
                due.push(want.clone());
            }
        }
        let cache = self.engine.local.addon_cache();
        let mut changed = false;
        for want in due {
            if zip_ready(&cache, &self.addons.ready, &want.id, &want.release).is_none() {
                let (name, version) = (&want.name, shown(&want.release));
                match self.addons.refused.get(&(want.id.clone(), want.release.tag.clone())).cloned() {
                    Some(why) => self.feedback.card_line(&format!("{name} {version} wasn't downloaded: {why}"), false, true),
                    None => self.feedback.card_line(&format!("{name} {version} is still downloading; it goes in next time"), false, false),
                }
                continue;
            }
            self.addons.waiting.retain(|w| w.id != want.id);
            let from = self.addons.store.get(&want.id).map(|r| r.shown_version());
            match self.addons_install(&want) {
                Ok(text) => {
                    changed = true;
                    self.feedback.card_line(&text, true, false);
                }
                // SurvivorsForge's word changed while it waited: it doesn't go in, and the line says why.
                Err(Refused::Waits(text)) => self.feedback.card_line(&text, false, false),
                Err(Refused::Never(text)) => self.feedback.card_line(&text, false, true),
                Err(Refused::Failed(e)) => {
                    log!("{} not updated before play: {e}", want.name);
                    let stays = from.map(|f| format!(" stays at {f}")).unwrap_or_else(|| " wasn't installed".into());
                    self.feedback.card_line(&format!("{}{stays}: {e}", want.name), false, true);
                }
            }
        }
        if changed {
            self.addons_scan();
        }
        // Updates SurvivorsForge asks about first wait for the player's OK on their rows.
        let asks = self
            .addons
            .lines
            .iter()
            .filter(|l| matches!(l.standing, Standing::Update { .. }) && !l.off && matches!(l.verdict, Verdict::Ask(_)))
            .count();
        match asks {
            0 => {}
            1 => self.feedback.card_line("1 update waits for your OK (Addons)", false, false),
            n => self.feedback.card_line(&format!("{n} updates wait for your OK (Addons)"), false, false),
        }
        if self.addons.looked.is_none_or(|t| Utc::now() - t >= LOOK_EVERY) {
            self.addons_look();
        }
    }

    /// After a session, the game closed for good: AddOns read again (another updater, like
    /// DragonUI's avrt.dll or a second manager, may have changed it), then the installs that
    /// waited for the game go in.
    pub(super) fn addons_after_session(&mut self) {
        self.addons_scan();
        if !self.addons.waiting.is_empty() {
            self.install_waiting();
        }
        self.share_look();
    }

    /// Changes to AddOns wait for the game to close; `true` (with the reason shown) when they must.
    pub(super) fn addons_blocked(&mut self) -> bool {
        if self.game_addons().is_none() {
            self.addons_say("Set the game's folder in Settings first.".into());
            return true;
        }
        if self.sync.game_running() {
            self.addons_say("Close the game first: addons change only while it's closed.".into());
            return true;
        }
        false
    }

    pub(super) fn addons_say(&mut self, text: String) {
        log!("addons: {text}");
        self.addons.message = Some((text, Instant::now()));
    }

    fn line(&self, id: &str) -> Option<&Line> {
        self.addons.lines.iter().find(|l| l.id == id)
    }

    // The player's actions --------------------------------------------------------------------

    pub(super) fn addons_check(&mut self) {
        self.addons.refused.clear();
        self.addons_scan();
        self.addons_look();
        self.addons_say("Looking for new releases…".into());
    }

    pub(super) fn addon_switch(&mut self, id: &str, on: bool) {
        if self.addons_blocked() {
            self.addons_lines();
            return;
        }
        let Some(game) = self.game_addons() else { return };
        let Some(line) = self.line(id) else { return };
        let (name, folders) = (line.name.clone(), line.folders.clone());
        let result = EnableList::read(&game.enable_list()).and_then(|mut list| {
            for folder in &folders {
                list.set(folder, on);
            }
            list.write(&game.enable_list())
        });
        match result {
            Ok(()) => self.activity(Kind::Addon, &format!("{name} turned {}", if on { "on" } else { "off" })),
            Err(e) => self.addons_say(format!("{name} couldn't be turned {}: {e}", if on { "on" } else { "off" })),
        }
        self.addons_scan();
    }

    /// Update now, on an installed addon's row or in its panel.
    pub(super) fn addon_update(&mut self, id: &str) {
        self.addon_update_as(id, false, true);
    }

    /// Update anyway: the player's OK for an update SurvivorsForge asks about first.
    pub(super) fn addon_update_anyway(&mut self, id: &str) {
        self.addon_update_as(id, true, true);
    }

    /// Updates an installed addon to its newer version as SurvivorsForge's verdict on that version
    /// allows (the row's, `Line::verdict`): Go installs, Ask only with the player's OK (`anyway`),
    /// No never. `ask_first`: an update the player asked for first asks SurvivorsForge about a
    /// version it says nothing about yet, and waits for the answer (`forge_ask_first`).
    pub(super) fn addon_update_as(&mut self, id: &str, anyway: bool, ask_first: bool) {
        if self.addons_blocked() {
            return;
        }
        let Some(line) = self.line(id) else { return };
        let to = match &line.standing {
            Standing::Update { to, .. } | Standing::Kept { newer: Some(to) } => to.clone(),
            _ => return,
        };
        let (name, slug, verdict) = (line.name.clone(), line.forge_slug.clone(), line.verdict.clone());
        if ask_first
            && let Some(slug) = slug.as_deref()
            && self.forge_view().word(slug, &to.tag).is_none()
            && self.forge_ask_first(slug, &to.tag, Held::Update { id: id.to_string(), anyway })
        {
            return self.addons_say(format!("Asking SurvivorsForge about {name} {} before it goes in…", shown(&to)));
        }
        match verdict {
            Verdict::Go => {}
            Verdict::Ask(_) if anyway => {}
            Verdict::Ask(why) => return self.addons_say(format!("{name} waits for your OK: {}", why.text())),
            Verdict::No(why) => return self.addons_say(format!("{name} isn't updated: {}", why.text())),
        }
        let Some(record) = self.addons.store.get(id) else { return };
        let want = Want {
            id: id.into(),
            name: record.name.clone(),
            badge: record.badge,
            repo: record.repo.clone(),
            released: self.forge_published(id, slug.as_deref(), &to.tag),
            release: to,
            forge: record.forge.clone(),
            expected_folders: self.forge_folders(record.forge.as_deref()),
            ok: anyway,
            pin: false,
        };
        self.install_or_wait(want);
    }

    pub(super) fn addons_update_all(&mut self) {
        if self.addons_blocked() {
            return;
        }
        // A turned-off addon is left alone (its own row's Update now still updates it), and so is
        // one SurvivorsForge holds back: Update all takes only what it lets in.
        let ids: Vec<String> = self
            .addons
            .lines
            .iter()
            .filter(|l| matches!(l.standing, Standing::Update { .. }) && !l.off && l.verdict.is_go())
            .map(|l| l.id.clone())
            .collect();
        for id in ids {
            self.addon_update_as(&id, false, false);
        }
    }

    /// Go back: the version kept from before, from the backups.
    pub(super) fn addon_back(&mut self, id: &str) {
        self.version_back(id, None);
    }

    /// Go back to a version kept in the backups: `backup` names it (Versions…' Go back to this),
    /// `None` the newest (the row's Go back). The player's own act, so a question of
    /// SurvivorsForge's doesn't stop it; a file it says never to install does.
    pub(super) fn version_back(&mut self, id: &str, backup: Option<&str>) {
        if self.addons_blocked() {
            return;
        }
        if let Some(Verdict::No(why)) = self.back_verdict(id, backup).filter(barred_back) {
            let name = self.line(id).map(|l| l.name.clone()).unwrap_or_default();
            return self.addons_say(format!("{name} doesn't go back: {}", why.text()));
        }
        let Some(game) = self.game_addons() else { return };
        let mut addons = Addons { game: &game, local: &self.engine.local, store: &mut self.addons.store };
        match addons.go_back(id, backup, Utc::now()) {
            Ok(record) => {
                let text = format!("{} went back to {}; it's kept at that version until you let it update", record.name, record.shown_version());
                self.activity(Kind::Addon, &text);
                self.addons_say(text);
            }
            Err(e) => self.addons_say(format!("It couldn't go back: {e}")),
        }
        self.addons_scan();
    }

    pub(super) fn addon_keep(&mut self, id: &str, keep: bool) {
        let Some(game) = self.game_addons() else { return };
        let mut addons = Addons { game: &game, local: &self.engine.local, store: &mut self.addons.store };
        match addons.pin(id, keep) {
            Ok(()) => {
                let name = self.line(id).map(|l| l.name.clone()).unwrap_or_default();
                let text = if keep { format!("{name} is kept at this version") } else { format!("{name} updates again") };
                self.activity(Kind::Addon, &text);
            }
            Err(e) => self.addons_say(format!("{e}")),
        }
        self.addons_lines();
    }

    pub(super) fn addon_remove(&mut self, id: &str, keep_settings: bool) {
        if self.addons_blocked() {
            return;
        }
        let Some(game) = self.game_addons() else { return };
        let Some(line) = self.line(id).cloned() else { return };
        let now = Utc::now();
        // A shared addon goes from the other PCs too; one kept to this PC stays off it.
        let local_only = self.addons.store.get(id).map(|r| r.local_only);
        let result = if id.starts_with("folder:") {
            remove_by_hand(&game, &self.engine.local.addon_backups(), &line.folders, keep_settings, now).map_err(|e| e.to_string())
        } else {
            let mut addons = Addons { game: &game, local: &self.engine.local, store: &mut self.addons.store };
            addons.remove(id, keep_settings, None, now, win::recycle).map(|_| ()).map_err(|e| e.to_string())
        };
        match result {
            Ok(()) => {
                if let Some(local_only) = local_only {
                    self.share_removed(id, keep_settings, local_only);
                }
                let kept = if keep_settings { "; its settings stay for a reinstall" } else { "" };
                let text = format!("{} removed: it's in the Recycle Bin{kept}", line.name);
                self.activity(Kind::Addon, &text);
                self.addons_say(text);
            }
            Err(e) => self.addons_say(format!("{} couldn't be removed: {e}", line.name)),
        }
        self.addons_scan();
    }

    pub(super) fn addon_page(&self, id: &str) {
        let repo = self
            .line(id)
            .and_then(|l| l.repo.clone())
            .or_else(|| self.addons.catalog.get(id).map(|e| e.repo.clone()))
            .or_else(|| self.share_repo(id))
            .or_else(|| self.forge_repo(id))
            .or_else(|| match &self.addons.link {
                Link::Found { lookup, .. } if lookup.repo == id => Some(lookup.repo.clone()),
                _ => None,
            });
        // A repository's own page only: a name from SurvivorsForge or another PC is checked first.
        if let Some(repo) = repo.and_then(|r| github::repo_from_input(&r).filter(|clean| clean.eq_ignore_ascii_case(r.trim()))) {
            win::open_url(&format!("https://github.com/{repo}"));
        }
    }

    pub(super) fn addon_folder(&self, id: &str) {
        let Some(game) = self.game_addons() else { return };
        match self.line(id).and_then(|l| l.folders.first()).and_then(|f| game.folder(f)) {
            Some(folder) => win::open_folder(&folder),
            None => win::open_folder(&game.addons),
        }
    }

    pub(super) fn addons_open_folder(&self) {
        if let Some(game) = self.game_addons() {
            let _ = fs::create_dir_all(&game.addons);
            win::open_folder(&game.addons);
        }
    }

    /// Browse's Install: an addon of our list, one only on SurvivorsForge (`forge:<slug>`), or our
    /// list's latest release in place of a copy installed by hand (`folder:<name>`).
    pub(super) fn browse_install(&mut self, id: &str) {
        self.browse_install_with(id, false);
    }

    /// Install anyway: the player's OK for an addon SurvivorsForge asks about first.
    pub(super) fn browse_install_anyway(&mut self, id: &str) {
        self.browse_install_with(id, true);
    }

    /// Installs from Browse as SurvivorsForge's verdict allows (`ForgeView::verdict`, as for every
    /// install): Go installs; Ask only with the player's OK (`anyway`, or Replace pressed under the
    /// reason on a copy's row); No never. A version SurvivorsForge says nothing about yet is asked
    /// about first, and the install waits for the answer.
    pub(super) fn browse_install_with(&mut self, id: &str, anyway: bool) {
        if self.addons_blocked() {
            return;
        }
        let Some(mut want) = self.browse_want(id) else {
            if let Some(slug) = forge::slug_in(id) {
                let why = if self.forge_on() { Why::NotListed } else { Why::Off };
                self.addons_say(format!("{slug} isn't installed: {}", why.text()));
            }
            return;
        };
        if let Some(slug) = self.want_slug(&want)
            && self.forge_view().word(&slug, &want.release.tag).is_none()
            && self.forge_ask_first(&slug, &want.release.tag, Held::Install { id: id.to_string(), anyway })
        {
            return self.addons_say(format!("Asking SurvivorsForge about {} {} before it goes in…", want.name, shown(&want.release)));
        }
        let verdict = self.want_verdict(&want);
        match &verdict {
            Verdict::Go => {}
            // The player's OK: Install anyway, or Replace pressed under the reason on the copy's row.
            Verdict::Ask(_) if anyway || id.starts_with("folder:") => {}
            Verdict::Ask(why) => return self.addons_say(format!("{} waits for your OK: {}", want.name, why.text())),
            Verdict::No(why) => return self.addons_say(format!("{} isn't installed: {}", want.name, why.text())),
        }
        // The OK goes with it: a question SurvivorsForge raises while it downloads doesn't stop it.
        want.ok = anyway || matches!(verdict, Verdict::Ask(_));
        self.install_or_wait(want);
    }

    /// What Browse installs for an offer: our catalog's release (a newer one found on GitHub, else
    /// its latest), or for SurvivorsForge's own its registry's latest, from where its author
    /// published it, its zip held to the folders SurvivorsForge lists.
    fn browse_want(&self, id: &str) -> Option<Want> {
        if let Some(slug) = forge::slug_in(id) {
            let view = self.forge_view();
            let entry = view.entry(slug)?;
            let tag = entry.latest.as_ref().map_or("", |l| l.tag.as_str());
            let seen = view.state.seen.get(slug).and_then(|tags| tags.get(tag)).copied().unwrap_or_else(Utc::now);
            let release = forge::latest_release(entry, seen)?;
            return Some(Want {
                id: forge::id_for(slug),
                name: if entry.name.trim().is_empty() { slug.to_string() } else { entry.name.clone() },
                badge: Badge::Added,
                repo: Some(entry.repo.clone()),
                released: self.forge_published(id, Some(slug), &release.tag),
                release,
                forge: Some(slug.to_string()),
                expected_folders: self.forge_folders(Some(slug)),
                ok: false,
                pin: false,
            });
        }
        let entry = match id.strip_prefix("folder:") {
            Some(folder) => self.addons.catalog.for_folder(folder),
            None => self.addons.catalog.get(id),
        }?;
        let release = self.addons.newer.get(&entry.id).cloned().unwrap_or_else(|| entry.latest.clone());
        Some(Want {
            id: entry.id.clone(),
            name: entry.name.clone(),
            badge: Badge::Verified,
            repo: Some(entry.repo.clone()),
            released: self.addons.catalog.published(&entry.id, &release.tag),
            release,
            forge: None,
            expected_folders: None,
            ok: false,
            pin: false,
        })
    }

    /// The slug SurvivorsForge lists the addon of an install under (its own, for one of its own).
    pub(super) fn want_slug(&self, want: &Want) -> Option<String> {
        let catalog_id = (want.badge == Badge::Verified).then_some(want.id.as_str());
        self.forge_view().slug_for(want.forge.as_deref(), catalog_id, want.repo.as_deref())
    }

    /// SurvivorsForge's verdict on an install: the one rule for every path that installs.
    pub(super) fn want_verdict(&self, want: &Want) -> Verdict {
        let slug = self.want_slug(want);
        let origin = forge::origin_of(want.badge, want.forge.as_deref());
        self.forge_view().verdict(origin, slug.as_deref(), &want.release.tag, Some(want.release.sha256.as_str()))
    }

    /// Whether `want` may go in now, by SurvivorsForge's verdict as it stands: Go, or an Ask the
    /// player OK'd (`Want::ok`); else why not, as the player reads it.
    pub(super) fn forge_judge(&self, want: &Want) -> Result<(), Refused> {
        let verdict = self.want_verdict(want);
        if forge::admits(&verdict, want.ok) {
            return Ok(());
        }
        match verdict {
            Verdict::No(why) => {
                let what = if self.addons.store.get(&want.id).is_some() { "wasn't updated" } else { "wasn't installed" };
                Err(Refused::Never(format!("{} {what}: {}", want.name, why.text())))
            }
            other => Err(Refused::Waits(format!("{} waits for your OK: {}", want.name, other.why().map(Why::text).unwrap_or_default()))),
        }
    }

    /// SurvivorsForge's verdict on the version an installed addon would go back to: the kept one
    /// `backup` names, else the newest kept (`Go back`). While it is silent, one of its own that the
    /// registry kept here doesn't list still counts as gone from it: an outage doesn't let it go
    /// back.
    fn back_verdict(&self, id: &str, backup: Option<&str>) -> Option<Verdict> {
        let record = self.addons.store.get(id)?;
        let kept = match backup {
            Some(backup) => record.earlier.iter().find(|k| k.backup == backup)?,
            None => record.earlier.first()?,
        };
        let view = self.forge_view();
        let slug = self.line(id).and_then(|l| l.forge_slug.clone());
        let origin = forge::origin_of(record.badge, record.forge.as_deref());
        // A re-zipped copy of our list's release is judged as that release (`judged_sha`).
        let sha = overview::judged_sha(&self.addons.catalog, record, kept.tag.as_deref(), kept.sha256.as_deref());
        let verdict = view.verdict(origin, slug.as_deref(), kept.tag.as_deref().unwrap_or_default(), sha);
        Some(match verdict {
            Verdict::No(Why::Offline) if record.forge.as_deref().is_some_and(|own| view.entry(own).is_none()) => Verdict::No(Why::NotListed),
            other => other,
        })
    }

    /// What Install again installs for a removed addon: the version it had, from its download
    /// link, the catalog, or (`url` empty) the zip kept on this PC or in the network folder. A copy
    /// of our list's release that an import re-zipped is judged, and fetched, as that release
    /// (`judged_sha`), as its Installed row judged it: not as another file than the one
    /// SurvivorsForge scanned.
    fn again_want(&self, record: &Record) -> Want {
        let sha256 = overview::judged_sha(&self.addons.catalog, record, record.tag.as_deref(), record.sha256.as_deref()).unwrap_or_default().to_string();
        let release = Release {
            tag: record.tag.clone().unwrap_or_else(|| sha256.clone()),
            version: record.version.clone(),
            published: record.released.unwrap_or_else(Utc::now),
            prerelease: false,
            file: record.file.clone().unwrap_or_default(),
            url: self.url_of(record),
            sha256,
            size: 0,
            // The version it had: install keeps it.
            interface: if record.version.is_some() { toc::GAME_INTERFACE } else { 0 },
            folders: BTreeMap::new(),
            notes: None,
            downloads: None,
        };
        Want {
            id: record.id.clone(),
            name: record.name.clone(),
            badge: record.badge,
            repo: record.repo.clone(),
            release,
            released: record.released,
            forge: record.forge.clone(),
            expected_folders: None,
            ok: false,
            pin: false,
        }
    }

    /// The Removed list's Install again, or its Install anyway (`anyway`, the player's OK): the
    /// version it had. Its settings come back once it's in, when they went with it. The one rule
    /// for every install, now and again when it goes in: a version SurvivorsForge says nothing
    /// about yet is asked about first (`ask_first`); one it asks about waits for the player's OK,
    /// its row showing why; what it says No to doesn't come back.
    pub(super) fn install_again_as(&mut self, id: &str, anyway: bool, ask_first: bool) {
        if self.addons_blocked() {
            return;
        }
        let Some(gone) = self.addons.store.removal(id).cloned() else { return };
        let mut want = self.again_want(&gone.record);
        if want.release.url.is_empty() && !self.fetch_imported(&want.release.sha256) {
            return self.addons_say(format!("{}'s zip isn't on this PC or in the network folder any more: import the zip again.", want.name));
        }
        if ask_first
            && let Some(slug) = self.want_slug(&want)
            && self.forge_view().word(&slug, &want.release.tag).is_none()
            && self.forge_ask_first(&slug, &want.release.tag, Held::Again { id: id.to_string(), anyway })
        {
            return self.addons_say(format!("Asking SurvivorsForge about {} {} before it goes in…", want.name, shown(&want.release)));
        }
        let verdict = self.want_verdict(&want);
        if !forge::admits(&verdict, anyway) {
            return self.addons_say(match verdict {
                Verdict::No(why) => format!("{} isn't installed: {}", want.name, why.text()),
                other => format!("{} waits for your OK: {}", want.name, other.why().map(Why::text).unwrap_or_default()),
            });
        }
        // The OK goes with it: a question SurvivorsForge raises while it downloads doesn't stop it.
        want.ok = anyway && matches!(verdict, Verdict::Ask(_));
        self.addons.restore.insert(id.to_string(), gone);
        self.install_or_wait(want);
    }

    /// A zip imported from a file, for Install again: in this PC's cache, or brought from the NAS.
    fn fetch_imported(&self, sha256: &str) -> bool {
        if sha256.is_empty() {
            return false;
        }
        let target = self.engine.local.addon_cache().join(format!("{sha256}.zip"));
        if target.is_file() {
            return true;
        }
        let Some(shared) = self.engine.shared() else { return false };
        fsx::read_verified(&shared.shared_zip(sha256), sha256).and_then(|bytes| fsx::write_atomic(&target, &bytes)).is_ok()
    }

    /// The settings an addon took with it when it was removed, back from that removal's backup,
    /// only where it has none now. `true` when any came back.
    fn restore_settings(&self, game: &GameAddons, record: &Record, backup: &str) -> bool {
        let saved = self.engine.local.addon_backups().join(backup).join("saved");
        let mut back = false;
        for folder in record.folders.keys() {
            let target = game.settings_file(folder);
            if target.exists() {
                continue;
            }
            match fsx::read_optional(&saved.join(format!("{folder}.lua"))) {
                Ok(Some(bytes)) => match fsx::write_atomic(&target, &bytes) {
                    Ok(()) => back = true,
                    Err(e) => log!("{}: its settings didn't come back: {e}", record.name),
                },
                Ok(None) => {}
                Err(e) => log!("{}: its settings' backup couldn't be read: {e}", record.name),
            }
        }
        back
    }

    /// The Removed list: addons removed here and not installed again since, newest first.
    fn removed_lines(&self) -> Vec<RemovedLine> {
        let cache = self.engine.local.addon_cache();
        let sync_on = self.engine.shared().is_some();
        self.addons
            .store
            .removed
            .iter()
            .filter(|g| self.addons.store.get(&g.record.id).is_none())
            .map(|g| {
                let record = &g.record;
                let (badge, badge_kind) = if record.badge == Badge::Verified && self.addons.catalog.get(&record.id).is_none() {
                    UNLISTED_BADGE
                } else if record.forge.is_some() {
                    FORGE_BADGE
                } else {
                    badge_text(Some(record.badge))
                };
                let when = match &g.from {
                    Some(pc) => format!("{}, as on {pc}", when(g.at)),
                    None => when(g.at),
                };
                let detail = if g.settings_kept {
                    "Its settings stayed in the game's folder".to_string()
                } else {
                    "Its settings went with it; they come back with it".to_string()
                };
                // A zip imported from a file can come back only from this PC's cache or the NAS.
                let cached = record.sha256.as_ref().is_some_and(|sha| cache.join(format!("{sha}.zip")).is_file());
                let can_install = record.badge != Badge::File || cached || sync_on;
                let note = if can_install { String::new() } else { "It came from a zip file: import the zip again".into() };
                // SurvivorsForge's word on the version it had, as Install again will judge it: a
                // question shows its reason, with Install anyway; a No, its reason and no button.
                let (reason, reason_kind) = match self.want_verdict(&self.again_want(record)) {
                    Verdict::Go => (String::new(), 0),
                    Verdict::Ask(why) => (format!("Waits for your OK: {}", why.text()), 1),
                    Verdict::No(why) => (why.text(), 2),
                };
                RemovedLine {
                    id: record.id.clone(),
                    initials: initials(&record.name),
                    name: record.name.clone(),
                    badge: badge.into(),
                    badge_kind,
                    version: record.shown_version(),
                    when,
                    detail,
                    can_install,
                    note,
                    reason,
                    reason_kind,
                }
            })
            .collect()
    }

    pub(super) fn link_lookup(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        self.addons.link = Link::Looking;
        let _ = self.net.send(Job::Lookup(text));
    }

    pub(super) fn link_clear(&mut self) {
        self.addons.link = Link::Idle;
    }

    pub(super) fn link_install(&mut self) {
        if self.addons_blocked() {
            return;
        }
        let Link::Found { lookup, verified, .. } = &self.addons.link else { return };
        let entry = verified.as_deref().and_then(|id| self.addons.catalog.get(id));
        let repo_name = lookup.repo.split('/').nth(1).unwrap_or_default().to_string();
        let release = Release {
            tag: lookup.tag.clone(),
            version: lookup.version.clone(),
            published: Utc::now(),
            prerelease: false,
            file: lookup.file.clone(),
            url: String::new(),
            sha256: lookup.sha256.clone(),
            size: lookup.size,
            // Marks the version as the `.toc`'s.
            interface: toc::GAME_INTERFACE,
            folders: BTreeMap::new(),
            notes: None,
            downloads: None,
        };
        let want = match entry {
            Some(e) => Want {
                id: e.id.clone(),
                name: e.name.clone(),
                badge: Badge::Verified,
                repo: Some(e.repo.clone()),
                released: self.addons.catalog.published(&e.id, &release.tag).or(lookup.released),
                release,
                forge: None,
                expected_folders: None,
                // A link the player looked up and installs: only SurvivorsForge's No stops it.
                ok: true,
                pin: false,
            },
            None => Want {
                id: lookup.repo.clone(),
                name: lookup.title.clone().unwrap_or(repo_name),
                badge: Badge::Added,
                repo: Some(lookup.repo.clone()),
                release,
                released: lookup.released,
                forge: None,
                expected_folders: None,
                ok: true,
                pin: false,
            },
        };
        // What SurvivorsForge says No to (blocked, changed after its scan, another file than the
        // one it scanned) isn't installed; anything else as before.
        if let Verdict::No(why) = self.want_verdict(&want) {
            return self.addons_say(format!("{} wasn't installed: {}", want.name, why.text()));
        }
        let installed = self.addons_install(&want);
        match installed {
            Ok(text) => {
                self.addons.link = Link::Idle;
                self.addons_say(text);
            }
            Err(e) => self.addons_say(e.said(&want.name)),
        }
        self.addons_scan();
    }

    /// A zip file the player picked: a verified addon's release is installed as that release,
    /// anything else as "From a file". A copy stays in the cache, its only source.
    pub(super) fn import_zip(&mut self, path: PathBuf) {
        if self.addons_blocked() {
            return;
        }
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = match fs::metadata(&path) {
            Ok(m) if m.len() > MAX_IMPORT => Err(io::Error::other("the file is larger than any addon")),
            _ => fs::read(&path),
        };
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(e) => return self.addons_say(format!("{file} couldn't be read: {e}")),
        };
        let found = match package::inspect(&bytes) {
            Ok(found) => found,
            Err(e) => return self.addons_say(format!("{file} can't be installed: {e}")),
        };
        let prints = package::fingerprints(&bytes, &found).unwrap_or_default();
        let sha = fsx::fingerprint(&bytes);
        let cache = self.engine.local.addon_cache();
        if let Err(e) = fsx::write_atomic(&cache.join(format!("{sha}.zip")), &bytes) {
            return self.addons_say(format!("{file} couldn't be kept in the Satchel's cache: {e}"));
        }
        let known = self.addons.catalog.addons.iter().find_map(|e| e.release_matching(&prints).map(|r| (e.clone(), r.clone())));
        let want = match known {
            Some((entry, release)) => {
                let want = Want {
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                    badge: Badge::Verified,
                    repo: Some(entry.repo.clone()),
                    released: Some(release.published),
                    release: release.clone(),
                    forge: None,
                    expected_folders: None,
                    // A file the player picked: only SurvivorsForge's No stops it.
                    ok: true,
                    pin: false,
                };
                // SurvivorsForge's word on the release these files are: what it says No to isn't
                // installed.
                if let Verdict::No(why) = self.want_verdict(&want) {
                    return self.addons_say(format!("{file} isn't installed: {}", why.text()));
                }
                // Its files are exactly that release's: the file picked goes in, not a download. Its
                // own checksum waits in `ready` (below), so it is judged as that release when it goes
                // in too, not as another file than the one SurvivorsForge scanned.
                Want { release: Release { sha256: String::new(), url: String::new(), ..release }, ..want }
            }
            None => {
                let main = &found.folders[0];
                let name = main.toc.title.as_deref().map(overview::plain).filter(|t| !t.is_empty()).unwrap_or_else(|| main.name.clone());
                let release = Release {
                    tag: file.clone(),
                    version: main.toc.version.clone(),
                    published: Utc::now(),
                    prerelease: false,
                    file: file.clone(),
                    url: String::new(),
                    sha256: sha.clone(),
                    size: bytes.len() as u64,
                    interface: toc::GAME_INTERFACE,
                    folders: BTreeMap::new(),
                    notes: None,
                    downloads: None,
                };
                Want {
                    id: format!("file:{}", main.name),
                    name,
                    badge: Badge::File,
                    repo: None,
                    release,
                    released: None,
                    forge: None,
                    expected_folders: None,
                    ok: true,
                    pin: false,
                }
            }
        };
        self.addons.ready.insert((want.id.clone(), want.release.tag.clone()), sha);
        self.install_or_wait(want);
    }

    /// Imported zips are kept (they have no other source); other downloads go once nothing needs
    /// them and they're two weeks old.
    pub(super) fn addons_prune_cache(&self) {
        let keep: BTreeSet<String> =
            self.addons.store.addons.iter().filter(|r| r.badge == Badge::File).filter_map(|r| r.sha256.clone()).collect();
        let Ok(entries) = fs::read_dir(self.engine.local.addon_cache()) else { return };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let sha = name.trim_end_matches(".zip");
            let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > CACHE_KEEP);
            if old && !keep.contains(sha) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    // Versions… -----------------------------------------------------------------------------------

    /// Versions… opened, or moved to another addon: SurvivorsForge's versions of it are fetched when
    /// it lists the addon and they aren't cached under its entry (its rules allow that when a
    /// player opens an addon).
    pub(super) fn versions_open(&mut self, id: &str) {
        self.addons.versions_open = Some(id.to_string());
        let slug = self.versions_of(id).1;
        if let Some(slug) = slug {
            self.forge_fetch_versions(&slug);
        }
    }

    /// What Versions… lists for `id` (a Browse offer's or an installed addon's): its record; for an
    /// addon of our list its catalog entry (a copy installed by hand, `folder:<name>`, has its
    /// addon's); for one SurvivorsForge lists, of ours or its own, its entry and versions there,
    /// with the switch on; and that slug. An addon added by link or from a file has its installed
    /// and kept versions only.
    fn versions_of(&self, id: &str) -> (overview::VersionsOf<'_>, Option<String>) {
        let record = self.addons.store.get(id);
        let own = forge::slug_in(id).or_else(|| record.and_then(|r| r.forge.as_deref()));
        let ours = own.is_none() && record.is_none_or(|r| r.badge == Badge::Verified);
        let catalog = self.catalog_entry(id).filter(|_| ours);
        let view = self.forge_view();
        let slug = match own {
            Some(own) => view.switch_on.then(|| own.to_string()),
            None => catalog.and_then(|e| view.slug_for(None, Some(e.id.as_str()), Some(e.repo.as_str()))),
        };
        let listed = slug.as_deref().filter(|_| view.switch_on).and_then(|s| self.forge.listed(s));
        let versions = slug.as_deref().filter(|_| listed.is_some()).and_then(|s| self.forge.versions(s));
        (overview::VersionsOf { record, catalog, listed, versions }, slug)
    }

    /// The install of `release` of addon `id` picked in Versions…: through the install path like any
    /// other, pinned once in. A release from SurvivorsForge's list of versions (`forge`) is held to
    /// the folders SurvivorsForge lists the addon (`slug`) with, as its own addons' installs are.
    fn version_want(&self, id: &str, release: Release, released: Option<DateTime<Utc>>, slug: Option<&str>, forge: bool) -> Option<Want> {
        if let Some(slug) = forge::slug_in(id) {
            let entry = self.forge_view().entry(slug)?;
            return Some(Want {
                id: forge::id_for(slug),
                name: if entry.name.trim().is_empty() { slug.to_string() } else { entry.name.clone() },
                badge: Badge::Added,
                repo: Some(entry.repo.clone()),
                release,
                released,
                forge: Some(slug.to_string()),
                expected_folders: self.forge_folders(Some(slug)),
                ok: false,
                pin: true,
            });
        }
        // A copy installed by hand is replaced under its addon's id, as Replace does.
        let entry = self.catalog_entry(id)?;
        Some(Want {
            id: entry.id.clone(),
            name: entry.name.clone(),
            badge: Badge::Verified,
            repo: Some(entry.repo.clone()),
            release,
            released,
            forge: None,
            expected_folders: if forge { self.forge_folders(slug) } else { None },
            ok: false,
            pin: true,
        })
    }

    /// Our catalog's entry for an id: a catalog id's, or the addon whose folder a copy installed by
    /// hand (`folder:<name>`) is.
    fn catalog_entry(&self, id: &str) -> Option<&catalog::Entry> {
        match id.strip_prefix("folder:") {
            Some(folder) => self.addons.catalog.for_folder(folder),
            None => self.addons.catalog.get(id),
        }
    }

    /// Versions…' Install this version, or Install anyway (`anyway`, the player's OK): that release
    /// through the install path and all its checks (its checksum, its folders, SurvivorsForge's word
    /// on exactly that version, asked about first when nothing cached has it, `ask_first`), then
    /// kept at it, as Go back keeps a version, until the player lets it update.
    pub(super) fn version_install_as(&mut self, id: &str, tag: &str, anyway: bool, ask_first: bool) {
        if self.addons_blocked() {
            return;
        }
        let (of, slug) = self.versions_of(id);
        let row = overview::versions(&of).into_iter().find(|r| r.tag == tag);
        let Some(overview::VersionRow { action: VersionAction::Install { release, forge }, published, .. }) = row else { return };
        let Some(mut want) = self.version_want(id, release, published, slug.as_deref(), forge) else { return };
        if ask_first
            && let Some(slug) = slug.as_deref()
            && self.forge_view().word(slug, tag).is_none()
            && self.forge_ask_first(slug, tag, Held::Version { id: id.to_string(), tag: tag.to_string(), anyway })
        {
            return self.addons_say(format!("Asking SurvivorsForge about {} {} before it goes in…", want.name, shown(&want.release)));
        }
        let verdict = self.want_verdict(&want);
        if !forge::admits(&verdict, anyway) {
            return self.addons_say(match verdict {
                Verdict::No(why) => format!("{} {} isn't installed: {}", want.name, shown(&want.release), why.text()),
                other => format!("{} {} waits for your OK: {}", want.name, shown(&want.release), other.why().map(Why::text).unwrap_or_default()),
            });
        }
        want.ok = anyway && matches!(verdict, Verdict::Ask(_));
        self.install_or_wait(want);
    }

    /// Versions… as the window shows it, for the addon it is open for.
    fn versions_view(&self) -> Option<VersionsView> {
        let id = self.addons.versions_open.clone()?;
        let (of, slug) = self.versions_of(&id);
        let name = of
            .record
            .map(|r| r.name.clone())
            .or_else(|| of.catalog.map(|e| e.name.clone()))
            .or_else(|| of.listed.map(|e| e.name.clone()).filter(|n| !n.trim().is_empty()))
            .unwrap_or_else(|| id.clone());
        let pinned = of.record.is_some_and(|r| r.pinned);
        let online = self.forge_view().online();
        // The version on its way in: downloading, or waiting for SurvivorsForge's word on it.
        let busy = |tag: &str| self.addons.waiting.iter().any(|w| w.id == id && w.release.tag == tag) || self.forge.holds_version(&id, tag);
        let rows = overview::versions(&of)
            .into_iter()
            .map(|row| {
                let mut marks: Vec<String> = Vec::new();
                if row.installed {
                    marks.push(if pinned { "installed · kept at this version until you let it update".into() } else { "installed".into() });
                }
                if let Some((_, kept)) = &row.kept {
                    marks.push(format!("kept on {}", kept.with_timezone(&Local).format("%-d %b")));
                }
                if row.prerelease {
                    marks.push("pre-release".into());
                }
                let mut detail: Vec<String> = Vec::new();
                detail.extend(row.published.map(|t| t.with_timezone(&Local).format("%-d %b %Y").to_string()));
                detail.extend(row.downloads.map(downloads_text));
                detail.extend(row.size.filter(|s| *s > 0).map(size_text));
                detail.extend(game_text(&row.game));
                // SurvivorsForge's word, as its badges have it: one that lets a version in only while
                // it answers.
                let word = row.word.filter(|(word, _)| online || word.restricts());
                let (badge, badge_kind) = word.map(|(word, _)| version_level(word)).map_or((String::new(), 0), |(level, kind)| (format!("SurvivorsForge · {level}"), kind));
                if let Some((_, counts)) = word.filter(|(word, _)| *word != Word::Unscanned) {
                    detail.push(found_text(counts));
                }
                let (action, key, reason, reason_kind) = match &row.action {
                    VersionAction::None => (0, String::new(), String::new(), 0),
                    VersionAction::Back { backup } => match self.back_verdict(&id, Some(backup)).filter(barred_back) {
                        Some(verdict) => (0, String::new(), verdict.why().map(Why::text).unwrap_or_default(), 2),
                        None => (1, backup.clone(), String::new(), 0),
                    },
                    // Nothing can be done with it here: the row says why, in SurvivorsForge's words.
                    VersionAction::Refused { why } => (0, String::new(), why.text(), 2),
                    VersionAction::Install { .. } if busy(&row.tag) => (4, row.tag.clone(), String::new(), 0),
                    VersionAction::Install { release, forge } => match self.version_want(&id, release.clone(), row.published, slug.as_deref(), *forge).map(|w| self.want_verdict(&w)) {
                        Some(Verdict::Go) => (2, row.tag.clone(), String::new(), 0),
                        Some(Verdict::Ask(why)) => (3, row.tag.clone(), format!("Waits for your OK: {}", why.text()), 1),
                        Some(Verdict::No(why)) => (0, String::new(), why.text(), 2),
                        None => (0, String::new(), String::new(), 0),
                    },
                };
                VersionLine { version: row.version, marks: marks.join(" · "), detail: detail.join(" · "), badge, badge_kind, action, key, reason, reason_kind }
            })
            .collect();
        Some(VersionsView { id, name, rows, loading: slug.as_deref().is_some_and(|s| self.forge_versions_loading(s)) })
    }

    // What the Addons tab shows --------------------------------------------------------------

    pub(super) fn addons_view(&self) -> AddonsView {
        let a = &self.addons;
        let auto = self.engine.settings.addons.update_by_themselves;
        let forge_view = self.forge_view();
        let cache = self.engine.local.addon_cache();
        let running = self.sync.game_running();
        // An install waiting for its download, or for SurvivorsForge's word on its version, shows
        // as under way.
        let waiting = |id: &str| a.waiting.iter().any(|w| w.id == id) || self.forge.holds(id);
        let rows: Vec<AddonLine> = a
            .lines
            .iter()
            .map(|line| {
                let record = a.store.get(&line.id);
                // Installed from SurvivorsForge alone: its badge is SurvivorsForge's.
                let own = record.is_some_and(|r| r.forge.is_some());
                let (badge, badge_kind) = match line.standing {
                    // Its badge said the game's devs had verified it; the list no longer says so.
                    Standing::Unlisted => UNLISTED_BADGE,
                    _ if own => FORGE_BADGE,
                    _ => badge_text(line.badge),
                };
                // Every badge that applies: ours, then SurvivorsForge's (only its, for its own).
                let theirs = forge_badge(&forge_view, line.forge_slug.as_deref(), &line.verdict, None);
                let badges: Vec<(String, i32)> =
                    if own { theirs.into_iter().collect() } else { std::iter::once((badge.to_string(), badge_kind)).chain(theirs).collect() };
                let (reason, reason_kind) = row_reason(&line.verdict);
                let held_back = matches!(line.verdict, Verdict::No(_));
                let (share, share_kind, ask_pc) = self.share_line(&line.id);
                // Its switches show once addons go to every PC (an addon installed by hand stays here).
                let shared = !share.is_empty() && record.is_some();
                let downloaded = |to: &Release| zip_ready(&cache, &a.ready, &line.id, to).is_some();
                // Our list's addons and SurvivorsForge's own go in by themselves, unless Settings says not.
                let ours = line.badge == Some(Badge::Verified) || own;
                let (mut update, mut state, mut state_kind) = match &line.standing {
                    Standing::Current => (String::new(), String::new(), 0),
                    Standing::Update { to, by_itself: true } if downloaded(to) => {
                        (shown(to), "Goes in when you press Play".into(), 1)
                    }
                    Standing::Update { to, by_itself: true } => (shown(to), "Downloading; it goes in at a Play once it's here".into(), 1),
                    Standing::Update { to, by_itself: false } => {
                        let why = if ours && !to.sha256.is_empty() && auto && line.off {
                            "Turned off, so it waits: it goes in at the first Play after you turn it on"
                        } else if ours && !to.sha256.is_empty() {
                            "Asks first (Settings)"
                        } else if ours {
                            "Asks first: GitHub published no checksum for this file"
                        } else {
                            "Asks first: nobody reviewed it"
                        };
                        (shown(to), why.into(), 1)
                    }
                    Standing::Kept { newer: Some(to) } => (String::new(), format!("Kept at this version; {} is out", shown(to)), 0),
                    Standing::Kept { newer: None } => (String::new(), "Kept at this version".into(), 0),
                    Standing::Changed => (String::new(), "Changed on disk: it isn't updated by itself".into(), 2),
                    Standing::Gone => (String::new(), "Its folder is gone from AddOns".into(), 2),
                    Standing::ByHand => match &line.replace_with {
                        Some(v) => (String::new(), format!("Not one of its author's releases: replace it with {v} to get updates"), 2),
                        None => (String::new(), String::new(), 0),
                    },
                    Standing::Elsewhere => (String::new(), "DragonUI's own updater (avrt.dll beside the game) keeps it current".into(), 0),
                    Standing::Unlisted => (String::new(), "No longer in the Discord Verified list: the Satchel doesn't update it".into(), 2),
                };
                // SurvivorsForge's word stands in the way of the update: the reason line says it.
                if !line.verdict.is_go() {
                    if matches!(line.standing, Standing::Update { .. }) {
                        state.clear();
                        state_kind = 0;
                    }
                    if held_back {
                        update.clear();
                    }
                }
                // A step back on SurvivorsForge (its latest is a version this PC saw before this
                // one) isn't an update: the row says what SurvivorsForge offers now.
                if line.standing == Standing::Current
                    && let Some(record) = record.filter(|r| r.forge.is_some())
                    && let Some(latest) = record.forge.as_deref().and_then(|slug| forge_view.entry(slug)).and_then(|e| e.latest.as_ref())
                    && !latest.tag.is_empty()
                    && record.tag.as_deref() != Some(latest.tag.as_str())
                {
                    state = format!("SurvivorsForge now offers {}; you have {}", latest.tag, record.tag.as_deref().unwrap_or(&line.version));
                    state_kind = 0;
                }
                if let Standing::Update { to, .. } = &line.standing
                    && !downloaded(to)
                    && let Some(why) = a.refused.get(&(line.id.clone(), to.tag.clone()))
                {
                    state = format!("Its download failed ({why}); the Satchel tries again at the next check");
                    state_kind = 2;
                }
                let unreachable = a.unreachable.get(&line.id).map(|e| format!("Not checked for new releases: {e}"));
                let problems: Vec<String> = line.problems.iter().cloned().chain(unreachable).collect();
                // The notes are the newer release's when one is out, else the installed one's.
                let notes_of = match &line.standing {
                    Standing::Update { to, .. } | Standing::Kept { newer: Some(to) } => shown(to),
                    _ => line.version.clone(),
                };
                // A copy installed by hand is replaced under its verified addon's id.
                let replacing = line.id.strip_prefix("folder:").and_then(|f| a.catalog.for_folder(f)).is_some_and(|e| waiting(&e.id));
                // The Safety column: SurvivorsForge's level, else our badge's word.
                let level = forge_level(&forge_view, line.forge_slug.as_deref(), &line.verdict, None).map(|(level, kind, _)| (level, kind));
                let safety = overview::safety_cell(Some((badge, badge_kind)), level);
                // The Status column: the one line that matters most; the opened area has them all.
                let (status, tone) = overview::row_status(&overview::RowLines {
                    incoming: false,
                    reason: (&reason, reason_kind),
                    state: (&state, state_kind),
                    share: (&share, share_kind),
                    problem: problems.first().map(String::as_str).unwrap_or_default(),
                    current: line.standing == Standing::Current,
                });
                AddonLine {
                    id: line.id.clone(),
                    initials: initials(&line.name),
                    icon: self.forge.icon_key(line.icon_url.as_deref()).unwrap_or_default(),
                    name: line.name.clone(),
                    badge: badges.first().map_or_else(|| badge.to_string(), |(text, _)| text.clone()),
                    badge_kind,
                    badges,
                    reason,
                    reason_kind,
                    author: line.author.clone(),
                    version: line.version.clone(),
                    // Under the column's heading, Released: "3 days ago".
                    released: line.released.map(|t| released_when(days_since(t))).unwrap_or_default(),
                    update,
                    state,
                    state_kind,
                    on: line.on,
                    switchable: line.switchable && !running,
                    problem: problems.first().cloned().unwrap_or_default(),
                    problems: problems.join("\n"),
                    notes: line.notes.as_deref().map(|n| overview::plain_notes(n, 700)).unwrap_or_default(),
                    notes_title: format!("What's new in {notes_of}"),
                    // Go back shows only where SurvivorsForge lets it go back (`barred_back`).
                    back_to: line.back_to.clone().filter(|_| !self.back_verdict(&line.id, None).as_ref().is_some_and(barred_back)).unwrap_or_default(),
                    pinned: line.pinned,
                    can_pin: line.badge.is_some_and(|b| b != Badge::File),
                    can_update: matches!(line.standing, Standing::Update { .. } | Standing::Kept { newer: Some(_) }) && !held_back,
                    has_page: line.repo.is_some(),
                    replace_with: line.replace_with.clone().unwrap_or_default(),
                    installing: waiting(&line.id) || replacing,
                    share,
                    share_kind,
                    shared,
                    local_only: record.is_some_and(|r| r.local_only),
                    settings_per_pc: record.is_some_and(|r| r.settings_per_pc),
                    ask_pc,
                    incoming: false,
                    safety,
                    status,
                    status_kind: tone_kind(tone),
                }
            })
            .collect();

        let mut rows = rows;
        // The addons another PC brings: their Safety and Status columns, from their own lines.
        rows.extend(self.share_incoming().into_iter().map(|mut row| {
            let level = row.badges.iter().find_map(|(text, kind)| text.strip_prefix("SurvivorsForge · ").map(|level| (level.to_string(), *kind)));
            let ours = row.badges.iter().find(|(text, _)| !text.starts_with("SurvivorsForge · ")).cloned().unwrap_or_else(|| (row.badge.clone(), row.badge_kind));
            // "SurvivorsForge · verified (v1.2)": the level alone.
            let level = level.map(|(level, kind)| (level.split(" (").next().unwrap_or_default().to_string(), kind));
            row.safety = overview::safety_cell(Some((ours.0.as_str(), ours.1)), level.as_ref().map(|(l, k)| (l.as_str(), *k)));
            let (status, tone) = overview::row_status(&overview::RowLines {
                incoming: true,
                reason: (&row.reason, row.reason_kind),
                state: (&row.state, row.state_kind),
                ..Default::default()
            });
            row.status = status;
            row.status_kind = tone_kind(tone);
            row
        }));
        rows.sort_by_key(|r| r.name.to_lowercase());
        // A turned-off addon's update waits for it to be on: it isn't counted, only mentioned. One
        // SurvivorsForge says No to never comes, so it isn't counted at all.
        let coming = |l: &&Line| matches!(l.standing, Standing::Update { .. }) && !matches!(l.verdict, Verdict::No(_));
        let off = a.lines.iter().filter(coming).filter(|l| l.off).count();
        let updates: Vec<&Line> = a.lines.iter().filter(coming).filter(|l| !l.off).collect();
        let by_itself = updates.iter().filter(|l| matches!(l.standing, Standing::Update { by_itself: true, .. })).count();
        // One kept at an older version while a newer is out isn't news, but it isn't up to date.
        let kept = a.lines.iter().filter(|l| matches!(l.standing, Standing::Kept { newer: Some(_) })).count();
        // One the verified list dropped isn't up to date either: nothing checks it any more.
        let unlisted = a.lines.iter().filter(|l| l.standing == Standing::Unlisted).count();
        let except: Vec<String> = [
            match kept {
                0 => None,
                1 => Some("1 addon kept at an older version".to_string()),
                n => Some(format!("{n} addons kept at older versions")),
            },
            match off {
                0 => None,
                1 => Some("1 turned-off addon with a newer version".to_string()),
                n => Some(format!("{n} turned-off addons with newer versions")),
            },
            match unlisted {
                0 => None,
                1 => Some("1 addon no longer in the Discord Verified list".to_string()),
                n => Some(format!("{n} addons no longer in the Discord Verified list")),
            },
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut header = match (updates.len(), by_itself) {
            (0, _) if a.lines.is_empty() => "No addons yet: find them in Browse".to_string(),
            (0, _) if !except.is_empty() => format!("Up to date, except {}", except.join(" and ")),
            (0, _) => "Everything is up to date".into(),
            (1, 1) => "1 update · it goes in when you press Play".into(),
            (n, b) if n == b => format!("{n} updates · they go in when you press Play"),
            (1, 0) => "1 update · it asks first".into(),
            (n, 0) => format!("{n} updates · they ask first"),
            (n, b) => format!("{n} updates · {b} go in when you press Play, {} ask first", n - b),
        };
        if !updates.is_empty() {
            match kept {
                0 => {}
                1 => header.push_str(" · 1 kept at an older version"),
                n => header.push_str(&format!(" · {n} kept at older versions")),
            }
            match off {
                0 => {}
                1 => header.push_str(" · 1 turned-off addon has one too"),
                n => header.push_str(&format!(" · {n} turned-off addons have one too")),
            }
            match unlisted {
                0 => {}
                n => header.push_str(&format!(" · {n} no longer in the Discord Verified list")),
            }
        }
        let looked = if a.looking {
            "Looking for new releases…".to_string()
        } else {
            match a.looked {
                Some(t) if Utc::now() - t < TimeDelta::hours(1) => format!("New releases checked {}", ago(t, Utc::now())),
                Some(t) => format!("New releases checked {}", when(t)),
                None => "New releases not checked yet".into(),
            }
        };
        let footer = match &a.adopted_note {
            Some(note) => format!("{note} {looked}."),
            None => format!("{looked}."),
        };
        let mut notice = Vec::new();
        if self.game_dir().is_none() {
            notice.push("Set the game's folder in Settings to manage its addons.".to_string());
        } else if running {
            notice.push("The game is running: changes to addons wait until it's closed.".into());
        }
        if a.avrt {
            notice.push("DragonUI's own updater (avrt.dll) sits beside the game, so the Satchel leaves DragonUI to it.".into());
        }
        if a.other_manager {
            notice.push("SurvivorsForge also manages addons here. When both update one, the Satchel just records the newer release.".into());
        }

        let offers: Vec<OfferLine> = a
            .offers
            .iter()
            .map(|o| {
                let newer = a.newer.get(&o.id);
                // Our badge for an addon of our list, then SurvivorsForge's.
                let ours = (o.origin != 2).then(|| ("Discord Verified".to_string(), 0));
                // SurvivorsForge's badge speaks of the version the card offers.
                let offered = self.offer_tag(&o.id);
                let badges: Vec<(String, i32)> =
                    ours.into_iter().chain(forge_badge(&forge_view, o.forge_slug.as_deref(), &o.verdict, offered.as_deref())).collect();
                // Its level alone, for the table's Safety column and its filters.
                let (trust, trust_kind) = forge_level(&forge_view, o.forge_slug.as_deref(), &o.verdict, offered.as_deref()).map_or((String::new(), 0), |(level, kind, _)| (level.to_string(), kind));
                // Who vouches for it: SurvivorsForge's level, else our list, else SurvivorsForge for
                // one of its own while it gives no level.
                let vouch = if o.origin == 2 { FORGE_BADGE } else { ("Discord Verified", 0) };
                let safety = overview::safety_cell(Some(vouch), Some((trust.as_str(), trust_kind)));
                // A newer release found on GitHub's page carries no true date.
                let date = o.date.filter(|_| newer.is_none());
                // Its pane's button, the table row's too: Install, Install anyway, Installing…,
                // Installed, Update, Update anyway.
                let (action, reason) = self.panel_action(o);
                OfferLine {
                    id: o.id.clone(),
                    initials: initials(&o.name),
                    icon: self.forge.icon_key(o.icon_url.as_deref()).unwrap_or_default(),
                    name: o.name.clone(),
                    author: o.author.clone(),
                    summary: o.summary.clone(),
                    badges,
                    trust,
                    tags: o.tags.clone(),
                    origin: o.origin,
                    version: newer.map(shown).unwrap_or_else(|| o.version.clone()),
                    date: date.map(day_text).unwrap_or_default(),
                    // A real publish date: our catalog's, or SurvivorsForge's registry's for its own.
                    sort_date: o.date.map(|t| t.timestamp()),
                    downloads: o.downloads,
                    downloads_text: o.downloads.map(downloads_text).unwrap_or_default(),
                    action,
                    reason,
                    warning: o.warning.clone().unwrap_or_default(),
                    installed: o.installed,
                    // The Installed tab's line Uninstall removes, and its name for the question.
                    installed_id: self.offer_line(o).map(|l| l.id.clone()).unwrap_or_default(),
                    installed_name: self.offer_line(o).map(|l| l.name.clone()).unwrap_or_default(),
                    // Removed on this PC: its Install says Install again.
                    again: !o.installed && a.store.removal(&o.id).is_some(),
                    safety,
                }
            })
            .collect();
        // Our list, and SurvivorsForge's while it answers, counted as the filters count them.
        let ours = offers.iter().filter(|o| o.origin != 2).count();
        let theirs = offers.iter().filter(|o| o.origin != 0).count();
        let catalog_of = format!("the catalog of {}", when(a.catalog.generated));
        let browse_note = if self.forge_on() && forge_view.online() {
            format!("{ours} Discord Verified · {theirs} on SurvivorsForge · {catalog_of}")
        } else {
            format!("{ours} Discord Verified · {catalog_of}")
        };
        let (forge_status, forge_answer, forge_offline) = self.forge_status();

        let link = match &a.link {
            Link::Idle => LinkView::default(),
            Link::Looking => LinkView { state: 1, message: "Looking it up on GitHub…".into(), ..LinkView::default() },
            Link::Failed(e) => LinkView { state: 3, message: e.clone(), ..LinkView::default() },
            Link::Found { lookup, warning, verified } => {
                let mut owner = lookup.owner.clone();
                if let Some(stars) = lookup.stars {
                    owner.push_str(&format!(" · {stars} star{}", if stars == 1 { "" } else { "s" }));
                }
                if let Some(pushed) = lookup.pushed {
                    owner.push_str(&format!(" · last change {}", when(pushed)));
                }
                let mut release = format!("{} · {}", lookup.tag, lookup.file);
                if let Some(license) = &lookup.license {
                    release.push_str(&format!(" · licence {license}"));
                }
                release.push_str(if lookup.checked { " · checksum matches GitHub's" } else { " · GitHub published no checksum" });
                let left = match lookup.left_out {
                    0 => "nothing left out".to_string(),
                    1 => "1 file left out (not an addon file)".into(),
                    n => format!("{n} files left out (not addon files)"),
                };
                let mut goes = format!("{} · {left}", lookup.folders.join(", "));
                // What it does: its .toc's own words, else its repository's description.
                if let Some(about) = lookup.notes.as_ref().or(lookup.description.as_ref()) {
                    goes.push_str(&format!(" · {about}"));
                }
                if !lookup.current {
                    goes.push_str(" · made for another game version: it loads only with \"Load out of date AddOns\"");
                }
                let (note, warn) = match (warning, verified) {
                    (Some(w), _) => (w.clone(), true),
                    (None, Some(_)) => ("This is a Discord Verified addon: it installs as one and updates by itself.".into(), false),
                    (None, None) => {
                        ("Not reviewed by the game's devs. Addon code can't reach your PC, and it asks before each update.".into(), false)
                    }
                };
                LinkView { state: 2, message: String::new(), owner, release, goes_into: goes, note, warn, installing: false }
            }
        };

        // The rail's addons line: the updates at Play, what waits for an OK (an update of an
        // installed addon, or one another PC brings), a download under way, whether releases
        // were checked at all.
        let waiting_ok = rows.iter().filter(|r| r.reason_kind == 1 && (r.can_update || r.incoming) && !r.installing).count();
        let rail = satchel_core::shell::addons_facts(&a.lines, waiting_ok, !a.fetching.is_empty() || !a.waiting.is_empty(), a.looked.is_some());
        AddonsView {
            count: a.lines.len(),
            rows,
            header,
            update_all: updates.iter().any(|l| l.verdict.is_go()) && !running,
            footer,
            share: self.share_header(),
            notice: notice.join(" "),
            message: a.message.as_ref().map(|(m, _)| m.clone()).unwrap_or_default(),
            offers,
            browse_note,
            panel: self.forge_panel(),
            forge_status,
            forge_answer,
            forge_offline,
            discord_checked: discord_day(a.catalog.discord_checked.as_deref()),
            link,
            removed: self.removed_lines(),
            versions: self.versions_view(),
            rail,
        }
    }
}

/// Go back is the player's own act on a backup this PC already holds. What bars it: SurvivorsForge's
/// word on the file itself (blocked, changed after its scan, another file than the one it scanned),
/// and for one of SurvivorsForge's own, its leaving the registry (deleted or blocked there, not
/// known which). Not a question, and not what speaks only of the registry's latest download (off,
/// silent, no checksum known for an older version).
fn barred_back(verdict: &Verdict) -> bool {
    matches!(verdict, Verdict::No(Why::Blocked(_) | Why::SourceChanged | Why::OtherFile | Why::NotListed))
}

/// Removes folders the Satchel didn't install: each goes to the Recycle Bin, its settings too
/// unless they're kept, with a copy of the settings in the backups either way.
fn remove_by_hand(game: &GameAddons, backups: &Path, folders: &[String], keep_settings: bool, now: DateTime<Utc>) -> io::Result<()> {
    let backup = backups.join(format!("{} {} removed", names::stamp(now), names::safe(folders.first().map(String::as_str).unwrap_or("addon"))));
    let mut list = EnableList::read(&game.enable_list())?;
    let before = list.clone();
    for name in folders {
        let settings = game.settings_file(name);
        if let Some(bytes) = fsx::read_optional(&settings)? {
            fsx::write_atomic(&backup.join("saved").join(format!("{name}.lua")), &bytes)?;
            if !keep_settings {
                win::recycle(&settings)?;
            }
        }
        if let Some(folder) = game.folder(name) {
            win::recycle(&folder)?;
        }
        list.forget(name);
    }
    if list != before {
        list.write(&game.enable_list())?;
    }
    Ok(())
}
