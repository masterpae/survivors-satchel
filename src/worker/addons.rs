//! The Addons tab, the worker's side. Everything that changes AddOns happens here, on the worker
//! thread, and only with the game closed. The network thread fetches; nothing here waits for it.

use super::{Worker, ago, when};
use crate::net::{Answer, Job, Lookup};
use crate::view::{AddonLine, AddonsView, LinkView, OfferLine, RemovedLine};
use crate::win;
use chrono::{DateTime, Local, TimeDelta, Utc};
use satchel_core::activity::Kind;
use satchel_core::addons::catalog::{self, Catalog, Release};
use satchel_core::addons::enabled::EnableList;
use satchel_core::addons::install::{Addons, Incoming};
use satchel_core::addons::overview::{self, Inputs, Line, Offer, Standing, initials};
use satchel_core::addons::store::{Badge, Gone, Record, Store};
use satchel_core::addons::{Found, GameAddons, package, scan, toc, tree};
use satchel_core::{fsx, gate, log, names};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
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
}

pub(super) struct AddonsState {
    pub(super) catalog: Catalog,
    pub(super) store: Store,
    pub(super) found: Vec<Found>,
    /// Each managed folder's fingerprint now.
    prints: BTreeMap<String, String>,
    pub(super) enabled: EnableList,
    pub(super) lines: Vec<Line>,
    offers: Vec<Offer>,
    /// Releases newer than the catalog or the record knows, from GitHub's pages, by addon id.
    pub(super) newer: BTreeMap<String, Release>,
    /// Downloaded zips: (addon id, release tag) → checksum.
    pub(super) ready: BTreeMap<(String, String), String>,
    pub(super) fetching: BTreeSet<(String, String)>,
    /// Downloads that failed, with why; tried again after the next look or "Check now".
    pub(super) refused: BTreeMap<(String, String), String>,
    /// Installs waiting for their download (or for the game to close).
    waiting: Vec<Want>,
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
    net: Sender<Job>,
}

impl AddonsState {
    pub(super) fn new(store: Store, net: Sender<Job>) -> AddonsState {
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
            net,
        }
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

fn badge_text(badge: Option<Badge>) -> (&'static str, i32) {
    match badge {
        Some(Badge::Verified) => ("Verified", 0),
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
        let a = &mut self.addons;
        a.lines = overview::lines(&Inputs {
            catalog: &a.catalog,
            store: &a.store,
            found: &a.found,
            prints: &a.prints,
            enabled: &a.enabled,
            newer: &a.newer,
            auto,
            avrt: a.avrt,
        });
        a.offers = overview::offers(&a.catalog, &a.store, &a.found);
        self.addons_fetch();
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
        let _ = self.addons.net.send(Job::Look(watches));
    }

    /// Downloads for every update shown (a turned-off addon's waits until it's on), every install
    /// waiting and every release the shared list brings at the next Play.
    pub(super) fn addons_fetch(&mut self) {
        let cache = self.engine.local.addon_cache();
        let shared = self.share_wanted();
        let a = &mut self.addons;
        let mut wanted: Vec<(String, Release)> = a
            .lines
            .iter()
            .filter(|l| !l.off)
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
            let _ = a.net.send(Job::Fetch { id, tag: release.tag.clone(), url: release.url.clone(), sha256 });
        }
    }

    /// In a build that knows its repository: the Satchel's newest GitHub release, on start and
    /// every few hours (sooner after a failed look). A newer one is downloaded, checked and its
    /// signature verified, then offered (Update now). When a newer Satchel keeps this one out and nothing
    /// offers it yet, that very release is looked for, every few minutes. True when a look was
    /// handed to the network thread just now.
    pub(super) fn satchel_look(&mut self) -> bool {
        if crate::about::REPOSITORY.is_empty() {
            return false;
        }
        let want = self.gated.clone().filter(|newer| self.offer().is_none_or(|o| gate::is_newer(newer, &o.version)));
        let every = match (&want, self.satchel_failed) {
            (Some(_), _) => super::SATCHEL_GATED_EVERY,
            (None, true) => super::SATCHEL_RETRY_EVERY,
            (None, false) => super::SATCHEL_LOOK_EVERY,
        };
        if !super::due(self.satchel_looked, every) {
            return false;
        }
        self.satchel_looked = Some(Instant::now());
        self.addons.net.send(Job::Satchel { repo: crate::about::REPOSITORY.into(), dir: self.engine.local.satchel_download(), want }).is_ok()
    }

    /// An answer from the network thread.
    pub(super) fn addons_answer(&mut self, answer: Answer) {
        match answer {
            Answer::Looked(results) => {
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
            Answer::Fetched { id, tag, sha256 } => {
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
                if !self.tracker.in_session() && !self.addons.waiting.is_empty() {
                    self.install_waiting();
                }
            }
            Answer::Satchel(result) => self.satchel_answer(result),
            Answer::LookedUp(result) => {
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
        }
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
                Err(e) => self.addons_say(format!("{} wasn't installed: {e}", want.name)),
            }
        }
        self.addons_scan();
        self.share_look();
    }

    /// Installs `want` from its downloaded zip. The game must be closed (the caller's part).
    pub(super) fn addons_install(&mut self, want: &Want) -> Result<String, String> {
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

    /// Installs now when the download is here, else as soon as it is.
    fn install_or_wait(&mut self, want: Want) {
        let cache = self.engine.local.addon_cache();
        if zip_ready(&cache, &self.addons.ready, &want.id, &want.release).is_some() {
            match self.addons_install(&want) {
                Ok(text) => self.addons_say(text),
                Err(e) => self.addons_say(format!("{} wasn't installed: {e}", want.name)),
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
                    released: self.addons.catalog.published(&line.id, &to.tag),
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
                    Some(why) => self.card_line(&format!("{name} {version} wasn't downloaded: {why}"), false, true),
                    None => self.card_line(&format!("{name} {version} is still downloading; it goes in next time"), false, false),
                }
                continue;
            }
            self.addons.waiting.retain(|w| w.id != want.id);
            let from = self.addons.store.get(&want.id).map(|r| r.shown_version());
            match self.addons_install(&want) {
                Ok(text) => {
                    changed = true;
                    self.card_line(&text, true, false);
                }
                Err(e) => {
                    log!("{} not updated before play: {e}", want.name);
                    let stays = from.map(|f| format!(" stays at {f}")).unwrap_or_else(|| " wasn't installed".into());
                    self.card_line(&format!("{}{stays}: {e}", want.name), false, true);
                }
            }
        }
        if changed {
            self.addons_scan();
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
        if self.tracker.game_running() {
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

    pub(super) fn addon_update(&mut self, id: &str) {
        if self.addons_blocked() {
            return;
        }
        let Some(line) = self.line(id) else { return };
        let to = match &line.standing {
            Standing::Update { to, .. } | Standing::Kept { newer: Some(to) } => to.clone(),
            _ => return,
        };
        let Some(record) = self.addons.store.get(id) else { return };
        let released = self.addons.catalog.published(id, &to.tag);
        let want = Want { id: id.into(), name: record.name.clone(), badge: record.badge, repo: record.repo.clone(), release: to, released };
        self.install_or_wait(want);
    }

    pub(super) fn addons_update_all(&mut self) {
        if self.addons_blocked() {
            return;
        }
        // A turned-off addon is left alone: its own row's Update now still updates it.
        let ids: Vec<String> = self
            .addons
            .lines
            .iter()
            .filter(|l| matches!(l.standing, Standing::Update { .. }) && !l.off)
            .map(|l| l.id.clone())
            .collect();
        for id in ids {
            self.addon_update(&id);
        }
    }

    pub(super) fn addon_back(&mut self, id: &str) {
        if self.addons_blocked() {
            return;
        }
        let Some(game) = self.game_addons() else { return };
        let mut addons = Addons { game: &game, local: &self.engine.local, store: &mut self.addons.store };
        match addons.go_back(id, Utc::now()) {
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
            .or_else(|| match &self.addons.link {
                Link::Found { lookup, .. } if lookup.repo == id => Some(lookup.repo.clone()),
                _ => None,
            });
        if let Some(repo) = repo {
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

    /// Installs a verified addon from Browse, or replaces a copy installed by hand with its
    /// author's latest release.
    pub(super) fn browse_install(&mut self, id: &str) {
        if self.addons_blocked() {
            return;
        }
        let entry = match id.strip_prefix("folder:") {
            Some(folder) => self.addons.catalog.for_folder(folder),
            None => self.addons.catalog.get(id),
        };
        let Some(entry) = entry else { return };
        let release = self.addons.newer.get(&entry.id).cloned().unwrap_or_else(|| entry.latest.clone());
        let released = self.addons.catalog.published(&entry.id, &release.tag);
        let want =
            Want { id: entry.id.clone(), name: entry.name.clone(), badge: Badge::Verified, repo: Some(entry.repo.clone()), release, released };
        self.install_or_wait(want);
    }

    /// The Removed list's Install again: the version it had, from its download link, the catalog,
    /// this PC's cache or the NAS. Its settings come back once it's in, when they went with it.
    pub(super) fn install_again(&mut self, id: &str) {
        if self.addons_blocked() {
            return;
        }
        let Some(gone) = self.addons.store.removal(id).cloned() else { return };
        let record = gone.record;
        let url = self.url_of(&record);
        let sha256 = record.sha256.clone().unwrap_or_default();
        if url.is_empty() && !self.fetch_imported(&sha256) {
            return self.addons_say(format!("{}'s zip isn't on this PC or in the network folder any more: import the zip again.", record.name));
        }
        let release = Release {
            tag: record.tag.clone().unwrap_or_else(|| sha256.clone()),
            version: record.version.clone(),
            published: record.released.unwrap_or_else(Utc::now),
            prerelease: false,
            file: record.file.clone().unwrap_or_default(),
            url,
            sha256,
            size: 0,
            // The version it had: install keeps it.
            interface: if record.version.is_some() { toc::GAME_INTERFACE } else { 0 },
            folders: BTreeMap::new(),
            notes: None,
        };
        let want =
            Want { id: record.id.clone(), name: record.name.clone(), badge: record.badge, repo: record.repo.clone(), release, released: record.released };
        self.addons.restore.insert(id.to_string(), Gone { record, ..gone });
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
                } else {
                    badge_text(Some(record.badge))
                };
                let when = match &g.from {
                    Some(pc) => format!("Removed {}, as on {pc}", when(g.at)),
                    None => format!("Removed {}", when(g.at)),
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
                }
            })
            .collect()
    }

    pub(super) fn link_lookup(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        self.addons.link = Link::Looking;
        let _ = self.addons.net.send(Job::Lookup(text));
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
        };
        let want = match entry {
            Some(e) => Want {
                id: e.id.clone(),
                name: e.name.clone(),
                badge: Badge::Verified,
                repo: Some(e.repo.clone()),
                released: self.addons.catalog.published(&e.id, &release.tag).or(lookup.released),
                release,
            },
            None => Want {
                id: lookup.repo.clone(),
                name: lookup.title.clone().unwrap_or(repo_name),
                badge: Badge::Added,
                repo: Some(lookup.repo.clone()),
                release,
                released: lookup.released,
            },
        };
        let installed = self.addons_install(&want);
        match installed {
            Ok(text) => {
                self.addons.link = Link::Idle;
                self.addons_say(text);
            }
            Err(e) => self.addons_say(format!("{} wasn't installed: {e}", want.name)),
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
                // Its files are exactly that release's: the file picked goes in, not a download.
                let released = Some(release.published);
                let release = Release { sha256: sha.clone(), url: String::new(), ..release };
                Want { id: entry.id.clone(), name: entry.name.clone(), badge: Badge::Verified, repo: Some(entry.repo.clone()), release, released }
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
                };
                Want { id: format!("file:{}", main.name), name, badge: Badge::File, repo: None, release, released: None }
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

    // What the Addons tab shows --------------------------------------------------------------

    pub(super) fn addons_view(&self) -> AddonsView {
        let a = &self.addons;
        let auto = self.engine.settings.addons.update_by_themselves;
        let cache = self.engine.local.addon_cache();
        let running = self.tracker.game_running();
        // An install waiting for its download shows as under way.
        let waiting = |id: &str| a.waiting.iter().any(|w| w.id == id);
        let rows: Vec<AddonLine> = a
            .lines
            .iter()
            .map(|line| {
                let (badge, badge_kind) = match line.standing {
                    // Its badge said the game's devs had verified it; the list no longer says so.
                    Standing::Unlisted => UNLISTED_BADGE,
                    _ => badge_text(line.badge),
                };
                let record = a.store.get(&line.id);
                let (share, share_kind, ask_pc) = self.share_line(&line.id);
                // Its switches show once addons go to every PC (an addon installed by hand stays here).
                let shared = !share.is_empty() && record.is_some();
                let downloaded = |to: &Release| zip_ready(&cache, &a.ready, &line.id, to).is_some();
                let (update, mut state, mut state_kind) = match &line.standing {
                    Standing::Current => (String::new(), String::new(), 0),
                    Standing::Update { to, by_itself: true } if downloaded(to) => {
                        (shown(to), "Goes in when you press Play".into(), 1)
                    }
                    Standing::Update { to, by_itself: true } => (shown(to), "Downloading; it goes in at a Play once it's here".into(), 1),
                    Standing::Update { to, by_itself: false } => {
                        let why = if line.badge == Some(Badge::Verified) && !to.sha256.is_empty() && auto && line.off {
                            "Turned off, so it waits: it goes in at the first Play after you turn it on"
                        } else if line.badge == Some(Badge::Verified) && !to.sha256.is_empty() {
                            "Asks first (Settings)"
                        } else if line.badge == Some(Badge::Verified) {
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
                    Standing::Unlisted => (String::new(), "No longer in the verified list: the Satchel doesn't update it".into(), 2),
                };
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
                AddonLine {
                    id: line.id.clone(),
                    initials: initials(&line.name),
                    name: line.name.clone(),
                    badge: badge.into(),
                    badge_kind,
                    author: line.author.clone(),
                    version: line.version.clone(),
                    released: line.released.map(|t| overview::released_ago(days_since(t))).unwrap_or_default(),
                    update,
                    state,
                    state_kind,
                    on: line.on,
                    switchable: line.switchable && !running,
                    problem: problems.first().cloned().unwrap_or_default(),
                    problems: problems.join("\n"),
                    notes: line.notes.as_deref().map(|n| overview::plain_notes(n, 700)).unwrap_or_default(),
                    notes_title: format!("What's new in {notes_of}"),
                    back_to: line.back_to.clone().unwrap_or_default(),
                    pinned: line.pinned,
                    can_pin: line.badge.is_some_and(|b| b != Badge::File),
                    can_update: matches!(line.standing, Standing::Update { .. } | Standing::Kept { newer: Some(_) }),
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
                }
            })
            .collect();

        let mut rows = rows;
        rows.extend(self.share_incoming());
        rows.sort_by_key(|r| r.name.to_lowercase());
        // A turned-off addon's update waits for it to be on: it isn't counted, only mentioned.
        let off = a.lines.iter().filter(|l| matches!(l.standing, Standing::Update { .. }) && l.off).count();
        let updates: Vec<&Line> = a.lines.iter().filter(|l| matches!(l.standing, Standing::Update { .. }) && !l.off).collect();
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
                1 => Some("1 addon no longer in the verified list".to_string()),
                n => Some(format!("{n} addons no longer in the verified list")),
            },
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut header = match (updates.len(), by_itself) {
            (0, _) if a.lines.is_empty() => "No addons yet: the verified ones are in Browse".to_string(),
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
                n => header.push_str(&format!(" · {n} no longer in the verified list")),
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

        let offers = a
            .offers
            .iter()
            .map(|o| OfferLine {
                id: o.id.clone(),
                initials: initials(&o.name),
                name: o.name.clone(),
                author: o.author.clone(),
                summary: o.summary.clone(),
                group: o.group.clone(),
                version: a.newer.get(&o.id).map(shown).unwrap_or_else(|| o.version.clone()),
                installed: o.installed,
                installing: waiting(&o.id),
                warning: o.warning.clone().unwrap_or_default(),
                notes: o.notes.as_deref().map(|n| overview::plain_notes(n, 700)).unwrap_or_default(),
            })
            .collect();
        let browse_note = format!(
            "{} verified addons · the catalog of {} · each pinned to its author's GitHub",
            a.catalog.addons.len(),
            when(a.catalog.generated)
        );

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
                    (None, Some(_)) => ("This is a verified addon: it installs as one and updates by itself.".into(), false),
                    (None, None) => {
                        ("Not reviewed by the game's devs. Addon code can't reach your PC, and it asks before each update.".into(), false)
                    }
                };
                LinkView { state: 2, message: String::new(), owner, release, goes_into: goes, note, warn, installing: false }
            }
        };

        AddonsView {
            count: a.lines.len(),
            rows,
            header,
            update_all: !updates.is_empty() && !running,
            footer,
            share: self.share_header(),
            notice: notice.join(" "),
            message: a.message.as_ref().map(|(m, _)| m.clone()).unwrap_or_default(),
            offers,
            browse_note,
            link,
            removed: self.removed_lines(),
        }
    }
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
