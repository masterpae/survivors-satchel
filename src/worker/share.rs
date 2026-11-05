//! Addons on every PC, the worker's side. The shared list is read at start, at every refresh and
//! after every change made here. What this PC changed goes up at once; what another PC changed
//! comes in at Play with the game closed, its downloads fetched beforehand. Each addon's settings,
//! and the game's own, go up when the game closes and come in before it starts.

use super::Worker;
use super::addons::{FORGE_BADGE, Refused, Want};
use super::forge::forge_badge;
use crate::view::AddonLine;
use chrono::{DateTime, Utc};
use satchel_core::activity::Kind;
use satchel_core::addons::catalog::Release;
use satchel_core::addons::enabled::EnableList;
use satchel_core::addons::forge::{self, Verdict};
use satchel_core::addons::overview::{self, Standing, initials};
use satchel_core::addons::share::{self, Agreed, Bundle, Entry, Here, List, Memory, OwnKept, SettingsStep, Shipped, Split, Step};
use satchel_core::addons::store::{Badge, Record};
use satchel_core::addons::{GameAddons, toc};
use satchel_core::engine::REACH_WAIT;
use satchel_core::game_settings;
use satchel_core::gate;
use satchel_core::paths::Shared;
use satchel_core::{fsx, log, names};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

pub(super) struct ShareState {
    memory: Memory,
    /// The list as last read.
    list: Option<List>,
    /// What each addon needs from this PC, from the last look: what it follows, and what of its
    /// own didn't go up.
    steps: BTreeMap<String, Step>,
    settings: BTreeMap<String, SettingsStep>,
    /// Why the last look didn't happen, in plain words.
    problem: Option<String>,
    /// The player's OKs for incoming addons SurvivorsForge asks about first here, each for the
    /// exact file another PC has (Install anyway on its row).
    oks: forge::Oks,
    /// Incoming addons the player asked for now (Install anyway): they come in as soon as the game
    /// is closed and their download is here, not only at the next Play.
    follow_now: BTreeSet<String>,
}

impl ShareState {
    pub(super) fn new(memory: Memory) -> ShareState {
        ShareState {
            memory,
            list: None,
            steps: BTreeMap::new(),
            settings: BTreeMap::new(),
            problem: None,
            oks: forge::Oks::default(),
            follow_now: BTreeSet::new(),
        }
    }

    /// This PC's memory of the list (addon-sync.json): what it agreed to, what it removed, the
    /// settings questions open. The game's own settings share it.
    pub(super) fn memory(&self) -> &Memory {
        &self.memory
    }

    pub(super) fn memory_mut(&mut self) -> &mut Memory {
        &mut self.memory
    }

    /// Why the last look didn't happen, in plain words.
    pub(super) fn problem(&self) -> Option<&String> {
        self.problem.as_ref()
    }

    /// The list as last read.
    pub(super) fn list(&self) -> Option<&List> {
        self.list.as_ref()
    }

    /// The list as just written.
    pub(super) fn set_list(&mut self, list: List) {
        self.list = Some(list);
    }

    /// The PCs the list names, with the addons they have.
    pub(super) fn pc_names(&self) -> impl Iterator<Item = &String> {
        self.list.iter().flat_map(|l| &l.addons).flat_map(|e| &e.pcs)
    }
}

/// Backups of one settings set kept in addon-backups: the newest of them.
const SETTINGS_BACKUPS_KEPT: usize = 10;

/// A settings set that travels: one addon's, or the game's own.
pub(super) enum SettingsSet {
    Addon { name: String, folders: Vec<String> },
    Game,
}

impl SettingsSet {
    /// "LuckyOdds's settings", "The game's settings".
    fn what(&self) -> String {
        match self {
            SettingsSet::Addon { name, .. } => format!("{name}'s settings"),
            SettingsSet::Game => game_settings::NAME.into(),
        }
    }

    fn read(&self, game: &GameAddons) -> io::Result<Bundle> {
        match self {
            SettingsSet::Addon { folders, .. } => Bundle::read(&game.saved, folders),
            SettingsSet::Game => game_settings::read(&game.config),
        }
    }

    fn place(&self, bundle: &Bundle, game: &GameAddons, backup: &Path) -> io::Result<()> {
        match self {
            SettingsSet::Addon { folders, .. } => bundle.place(&game.saved, folders, backup),
            SettingsSet::Game => game_settings::place(bundle, &game.config, backup),
        }
    }
}

/// This PC's memory of the list, or an empty one when it can't be read (kept aside as
/// `addon-sync.json.broken`): the next look then decides as on a first run, which loses nothing.
pub(super) fn load_memory(path: &Path) -> Memory {
    Memory::load(path).unwrap_or_else(|e| {
        log!("addon-sync.json unreadable ({e}); kept as addon-sync.json.broken, decided again as on a first run");
        let _ = fs::rename(path, path.with_extension("json.broken"));
        Memory::default()
    })
}

/// What following the list did for one addon.
enum Followed {
    Done(String),
    /// Not yet: SurvivorsForge asks first, and the player hasn't OK'd it.
    Waits(String),
    /// Not yet: its download is still under way; it comes next time.
    Downloading(String),
    Failed(String),
    Quiet,
}

/// The install that brings an entry's release here.
fn want_of(entry: &Entry, release: &Shipped) -> Want {
    // An id that isn't a checksum (a record from before checksums were kept) can't check a download.
    let sha256 = if release.sha256.starts_with("tag:") { String::new() } else { release.sha256.clone() };
    Want {
        id: entry.id.clone(),
        name: entry.name.clone(),
        badge: entry.badge,
        repo: entry.repo.clone(),
        release: Release {
            tag: release.tag.clone().unwrap_or_else(|| release.sha256.clone()),
            version: release.version.clone(),
            published: release.released.unwrap_or_else(Utc::now),
            prerelease: false,
            file: release.file.clone().unwrap_or_default(),
            url: release.url.clone(),
            sha256,
            size: release.size,
            // The version is the one the first PC installed: install keeps it.
            interface: if release.version.is_some() { toc::GAME_INTERFACE } else { 0 },
            folders: BTreeMap::new(),
            notes: None,
            downloads: None,
        },
        released: release.released,
        // SurvivorsForge's own keeps its slug here too. Its file is the one the first PC installed,
        // held to that PC's checksum.
        forge: entry.forge.clone(),
        expected_folders: None,
        // Followed only on SurvivorsForge's Go here, and judged again as it goes in.
        ok: false,
        pin: false,
    }
}

impl Worker {
    /// The shared folder and the game's addon folders, when addons go to every PC: sync on with a
    /// folder it can use, Settings' switch on, the game's folder known.
    pub(super) fn share_place(&self) -> Option<(Shared, GameAddons)> {
        if !self.engine.settings.addons.shared {
            return None;
        }
        Some((self.engine.shared()?, self.game_addons()?))
    }

    /// The settings set behind an id among the list's settings versions.
    pub(super) fn settings_set(&self, id: &str) -> Option<SettingsSet> {
        if id == game_settings::ID {
            return Some(SettingsSet::Game);
        }
        let record = self.addons.record(id)?;
        Some(SettingsSet::Addon { name: record.name.clone(), folders: record.folders.keys().cloned().collect() })
    }

    pub(super) fn share_save(&self) {
        if let Err(e) = self.share.memory.save(&self.engine.local.addon_share()) {
            log!("addon-sync.json not saved: {e}");
        }
    }

    /// This PC's side of an addon; `None`: kept to this PC ("Only on this PC", or kept off it).
    fn here(&self, id: &str) -> Option<Here> {
        // An addon that is here speaks for itself, whatever was removed or kept off before.
        let Some(record) = self.addons.record(id) else {
            if self.share.memory.removed.contains_key(id) {
                return Some(Here::Removed);
            }
            if self.share.memory.opted_out.contains(id) {
                return None;
            }
            return Some(Here::Missing);
        };
        if record.local_only {
            return None;
        }
        // A folder that vanished from AddOns is put back from the list, never removed everywhere.
        if self.addons.lines().iter().find(|l| l.id == id).is_none_or(|l| l.standing == Standing::Gone) {
            return Some(Here::Missing);
        }
        Some(Here::Has(Agreed::of(record, self.addons.folders_off(record))))
    }

    /// Whether this PC has the entry's release in place (a folder deleted by hand doesn't count).
    fn has_release(&self, id: &str, release: &Shipped) -> bool {
        self.addons.record(id).is_some_and(|r| share::identity(r) == release.sha256) && !matches!(self.here(id), Some(Here::Missing))
    }

    /// The game was started (by hand) since Play was pressed: nothing is written in its folder.
    pub(super) fn game_started_meanwhile(&mut self) -> bool {
        let dir = self.game_dir();
        dir.is_some_and(|d| self.scanner.game_running(&d))
    }

    /// Which of two releases is newer: by their dates, else by their order among the addon's known
    /// releases; `None` when neither tells.
    fn newer(&self, record: &Record, nas: &Shipped) -> Option<Ordering> {
        if let (Some(mine), Some(theirs)) = (record.released, nas.released)
            && mine != theirs
        {
            return Some(mine.cmp(&theirs));
        }
        let releases = overview::releases(record, self.addons.catalog(), self.addons.newer_releases());
        let at = |tag: Option<&str>| releases.iter().position(|r| Some(r.tag.as_str()) == tag);
        match (at(record.tag.as_deref()), at(nas.tag.as_deref())) {
            // Newest first: the earlier one in the list is the newer.
            (Some(mine), Some(theirs)) if mine != theirs => Some(theirs.cmp(&mine)),
            _ => None,
        }
    }

    /// Where the other PCs download this record's release from; empty for a zip imported from a
    /// file (it goes to the NAS instead).
    pub(super) fn url_of(&self, record: &Record) -> String {
        if record.badge == Badge::File {
            return String::new();
        }
        if let Some(url) = record.url.clone().filter(|u| !u.is_empty()) {
            return url;
        }
        let tag = record.tag.clone().unwrap_or_default();
        let known = self
            .addons
            .catalog()
            .get(&record.id)
            .and_then(|e| e.releases().find(|r| r.tag == tag).cloned())
            .or_else(|| self.addons.newer(&record.id).filter(|r| r.tag == tag).cloned());
        if let Some(release) = known.filter(|r| !r.url.is_empty()) {
            return release.url;
        }
        match (&record.repo, &record.file) {
            (Some(repo), Some(file)) if !tag.is_empty() => format!("https://github.com/{repo}/releases/download/{tag}/{file}"),
            _ => String::new(),
        }
    }

    /// This PC's addon as the list carries it.
    fn entry_of(&self, record: &Record, list: &List, now: DateTime<Utc>) -> Entry {
        let pc = self.engine.pc().to_string();
        let mut pcs = list.get(&record.id).map(|e| e.pcs.clone()).unwrap_or_default();
        if !pcs.contains(&pc) {
            pcs.push(pc.clone());
        }
        // Its folders the game doesn't load, as the list keeps them (lower case, sorted).
        let here = Agreed::of(record, self.addons.folders_off(record));
        Entry {
            id: record.id.clone(),
            name: record.name.clone(),
            badge: record.badge,
            repo: record.repo.clone(),
            release: Some(Shipped {
                sha256: share::identity(record),
                tag: record.tag.clone(),
                file: record.file.clone(),
                version: record.version.clone(),
                url: self.url_of(record),
                released: record.released,
                size: 0,
            }),
            folders: record.folders.keys().cloned().collect(),
            on: here.on,
            off: here.off,
            pinned: record.pinned,
            settings_per_pc: record.settings_per_pc,
            keep_settings: false,
            pcs,
            pc,
            at: now,
            // Where it came from on SurvivorsForge, and nothing of its word: each PC asks it.
            forge: record.forge.clone(),
        }
    }

    /// The releases the list brings at the next Play, for downloading beforehand: only those
    /// SurvivorsForge lets in here, by this PC's own cache. One of its own addons in a version its
    /// cached word doesn't cover is asked about first, and comes once the answer lets it in.
    pub(super) fn share_wanted(&mut self) -> Vec<(String, Release)> {
        let Some(list) = &self.share.list else { return Vec::new() };
        let mut wanted = Vec::new();
        let mut ask: Vec<(String, String)> = Vec::new();
        for (id, step) in &self.share.steps {
            let Some(entry) = list.get(id).filter(|_| *step == Step::Follow) else { continue };
            let Some(release) = entry.release.as_ref() else { continue };
            // A zip imported from a file comes from the NAS at Play.
            if self.has_release(id, release) || release.url.is_empty() {
                continue;
            }
            let tag = release.tag.as_deref().unwrap_or_default();
            if let Some(slug) = entry.forge.as_deref().filter(|slug| forge::slug_ok(slug))
                && self.forge_unknown(slug, tag)
            {
                ask.push((slug.to_string(), tag.to_string()));
            } else if forge::admits(&self.share_verdict(entry, release), self.share.oks.has(id, &release.sha256)) {
                wanted.push((id.clone(), want_of(entry, release).release));
            }
        }
        for (slug, tag) in ask {
            self.forge_ask_follow(&slug, &tag);
        }
        wanted
    }

    /// SurvivorsForge's verdict here on following an entry's release, by this PC's own cache (the
    /// list carries nothing of the other PC's word), as for every install.
    fn share_verdict(&self, entry: &Entry, release: &Shipped) -> Verdict {
        let slug = self.entry_slug(entry);
        // An id that isn't a checksum (a record from before checksums were kept) has none to compare.
        let sha = (!release.sha256.starts_with("tag:")).then_some(release.sha256.as_str());
        let origin = forge::origin_of(entry.badge, entry.forge.as_deref());
        self.forge_view().verdict(origin, slug.as_deref(), release.tag.as_deref().unwrap_or_default(), sha)
    }

    /// The slug SurvivorsForge lists an entry's addon under, here.
    fn entry_slug(&self, entry: &Entry) -> Option<String> {
        let catalog_id = (entry.badge == Badge::Verified).then_some(entry.id.as_str());
        self.forge_view().slug_for(entry.forge.as_deref(), catalog_id, entry.repo.as_deref())
    }

    /// Reads the list and decides every addon, then sends what changed here: the addons at any
    /// time, their settings and the game's only with the game closed. What comes from the other
    /// PCs waits for Play.
    pub(super) fn share_look(&mut self) {
        let addons = self.share_place();
        let game = self.game_share_place();
        if addons.is_none() {
            self.share.steps.clear();
            self.share.settings.clear();
        }
        if game.is_none() {
            self.game_settings.forget();
            self.share.memory.splits.remove(game_settings::ID);
        }
        let Some(shared) = addons.as_ref().or(game.as_ref()).map(|(shared, _)| shared.clone()) else {
            self.share.list = None;
            self.share.problem = None;
            return;
        };
        if !shared.reachable_within(REACH_WAIT) {
            self.share.problem = Some("The network folder doesn't answer: changes made here go to the other PCs once it does.".into());
            return;
        }
        let mut list = match share::read(&shared) {
            Ok(list) => list,
            Err(e) => {
                log!("the shared addon list wasn't read: {e}");
                if let Some(newer) = gate::newer_satchel(&e) {
                    self.sync.set_gated(newer.newer.clone());
                }
                self.share.problem = Some(format!("The shared addon list can't be read ({e}). Nothing changes until it can."));
                return;
            }
        };
        self.share.problem = None;
        if addons.is_some() {
            let (send, leave, join) = self.share_decide(&list);
            if !send.is_empty() || !leave.is_empty() || !join.is_empty() {
                match self.share_write(&shared, &send, &leave, &join) {
                    Ok(fresh) => list = fresh,
                    Err(e) => log!("addon changes not sent yet: {e}"),
                }
                // What didn't go up (the write failed, or a zip couldn't go first) goes at the next
                // look; Sync now says so.
                let (unsent, ..) = self.share_decide(&list);
                self.share.steps.extend(unsent.into_iter().map(|(id, _)| (id, Step::Send)));
            }
        }
        self.share.memory.rev = list.rev;
        self.share.list = Some(list);
        if !self.sync.game_running() {
            if let Some((shared, folders)) = &addons {
                self.share_settings(shared, folders);
            }
            if let Some((shared, folders)) = &game {
                self.game_settings_look(shared, folders);
            }
        }
        self.share_save();
        if addons.is_some() {
            self.addons_fetch();
        }
    }

    /// Decides every addon against `list`: what this PC follows is kept for Play; returns what goes
    /// up, with the list's state it was decided on (sent only if that's still so), what this PC
    /// keeps to itself now and what it has that the list doesn't count it for.
    fn share_decide(&mut self, list: &List) -> (Vec<(String, Option<Agreed>)>, Vec<String>, Vec<String>) {
        let pc = self.engine.pc().to_string();
        // Installed again: an older removal or opt-out no longer speaks for it.
        let addons = &self.addons;
        self.share.memory.removed.retain(|id, _| addons.record(id).is_none());
        self.share.memory.opted_out.retain(|id| addons.record(id).is_none());
        let mut ids: BTreeSet<String> = list.addons.iter().map(|e| e.id.clone()).collect();
        ids.extend(self.addons.records().iter().map(|r| r.id.clone()));
        ids.extend(self.share.memory.agreed.keys().cloned());
        ids.extend(self.share.memory.removed.keys().cloned());
        let (mut send, mut leave, mut join) = (Vec::new(), Vec::new(), Vec::new());
        let mut steps = BTreeMap::new();
        for id in ids {
            let nas = list.get(&id);
            let Some(here) = self.here(&id) else {
                if nas.is_some_and(|e| e.pcs.contains(&pc)) {
                    leave.push(id.clone());
                }
                self.share.memory.agreed.remove(&id);
                continue;
            };
            // B written before 0.5.1 says only on or off. Read in the addon's folders, as the list's
            // entry is, it matches the list again; else the first look would turn the whole addon off.
            let folders: Vec<String> = match nas {
                Some(entry) => entry.folders.clone(),
                None => self.addons.record(&id).map(|r| r.folders.keys().cloned().collect()).unwrap_or_default(),
            };
            let base = self.share.memory.agreed.get(&id).cloned().map(|b| b.with_folders(&folders));
            let theirs = nas.map(Entry::agreed);
            let step = share::step(&here, base.as_ref(), theirs.as_ref(), || match (&here, nas) {
                (Here::Has(mine), Some(entry)) => {
                    let record = self.addons.record(&id);
                    share::here_wins(mine, &entry.agreed(), base.is_none(), || {
                        record.zip(entry.release.as_ref()).and_then(|(r, release)| self.newer(r, release))
                    })
                }
                _ => false,
            });
            match step {
                Step::InStep => {
                    if let Some(theirs) = theirs {
                        self.share.memory.agreed.insert(id.clone(), theirs);
                    }
                    self.share.memory.removed.remove(&id);
                    if matches!(here, Here::Has(_)) && nas.is_some_and(|e| !e.pcs.contains(&pc)) {
                        join.push(id.clone());
                    }
                }
                Step::Send => send.push((id.clone(), theirs)),
                Step::Follow => {
                    steps.insert(id.clone(), Step::Follow);
                }
                Step::Nothing => {
                    self.share.memory.agreed.remove(&id);
                    self.share.memory.removed.remove(&id);
                }
            }
        }
        self.share.steps = steps;
        // A settings question about settings that no longer travel (the addon removed, kept to this
        // PC, or its settings per PC) goes.
        let stale: Vec<String> = self
            .share
            .memory
            .splits
            .keys()
            // The game's own settings aren't an addon's: their question stands on its own.
            .filter(|id| id.as_str() != game_settings::ID)
            .filter(|id| {
                let travels = list.get(id.as_str()).is_some_and(|e| !e.removed() && !e.settings_per_pc);
                !travels || !matches!(self.here(id), Some(Here::Has(_)))
            })
            .cloned()
            .collect();
        for id in stale {
            self.share.memory.splits.remove(&id);
        }
        (send, leave, join)
    }

    /// Writes this PC's changes into the list, under its lock; returns the list as written.
    fn share_write(&mut self, shared: &Shared, send: &[(String, Option<Agreed>)], leave: &[String], join: &[String]) -> io::Result<List> {
        let pc = self.engine.pc().to_string();
        let now = Utc::now();
        // An imported zip goes to the NAS first, before the lock (it can be big): without it the
        // other PCs couldn't install it.
        let mut held: BTreeSet<String> = BTreeSet::new();
        for (id, _) in send {
            if let Some(Here::Has(_)) = self.here(id)
                && let Some(record) = self.addons.record(id).filter(|r| r.badge == Badge::File)
                && let Err(e) = self.share_upload(shared, record)
            {
                log!("addons: {} not sent: its zip couldn't go to the NAS: {e}", record.name);
                held.insert(id.clone());
            }
        }
        let _lock = share::lock(shared, &pc)?;
        let mut list = share::read(shared)?;
        // This PC's memory moves only once the list is written: a write that fails changes nothing,
        // and the next look sends again.
        let mut agreed: Vec<(String, Agreed)> = Vec::new();
        let mut removals: Vec<String> = Vec::new();
        for (id, seen) in send {
            // Another PC changed it since the look: the next look decides again.
            if held.contains(id) || list.get(id).map(Entry::agreed) != *seen {
                continue;
            }
            match self.here(id) {
                Some(Here::Has(_)) => {
                    let Some(record) = self.addons.record(id) else { continue };
                    let entry = self.entry_of(record, &list, now);
                    log!("addons: {} {} goes to the other PCs", entry.name, record.shown_version());
                    agreed.push((id.clone(), entry.agreed()));
                    list.put(entry);
                }
                Some(Here::Removed) => {
                    let keep = self.share.memory.removed.get(id).copied().unwrap_or(true);
                    if let Some(old) = list.get(id).cloned() {
                        log!("addons: {} removed here, so on the other PCs too", old.name);
                        let pcs = old.pcs.iter().filter(|p| **p != pc).cloned().collect();
                        let removed = Entry {
                            release: None,
                            on: false,
                            pinned: false,
                            settings_per_pc: false,
                            keep_settings: keep,
                            pcs,
                            pc: pc.clone(),
                            at: now,
                            ..old
                        };
                        list.put(removed);
                    }
                    agreed.push((id.clone(), Agreed::removed()));
                    removals.push(id.clone());
                }
                Some(Here::Missing) | None => {}
            }
        }
        for id in leave {
            list.leave(id, &pc);
        }
        for id in join {
            list.join(id, &pc);
        }
        share::write(shared, &list)?;
        for (id, state) in agreed {
            self.share.memory.agreed.insert(id, state);
        }
        for id in removals {
            self.share.memory.removed.remove(&id);
        }
        Ok(list)
    }

    /// A zip imported from a file here goes to the NAS, for the other PCs to install.
    fn share_upload(&self, shared: &Shared, record: &Record) -> io::Result<()> {
        let Some(sha) = &record.sha256 else { return Ok(()) };
        let target = shared.shared_zip(sha);
        if target.is_file() {
            return Ok(());
        }
        let bytes = fsx::read_verified(&self.engine.local.addon_cache().join(format!("{sha}.zip")), sha)?;
        fsx::write_atomic(&target, &bytes)
    }

    /// Each shared addon's settings, decided (the game closed): this PC's go up; the list's are
    /// kept for Play; a question waits when both changed.
    fn share_settings(&mut self, shared: &Shared, game: &GameAddons) {
        let Some(list) = self.share.list.clone() else { return };
        let mut decided = BTreeMap::new();
        let mut sends: Vec<(String, Bundle, Option<u64>, Option<String>)> = Vec::new();
        for entry in list.addons.iter().filter(|e| !e.removed() && !e.settings_per_pc) {
            let id = &entry.id;
            // Only for an addon this PC has in step with the list.
            if !matches!(self.here(id), Some(Here::Has(_))) || self.share.steps.contains_key(id) {
                continue;
            }
            let Some(set) = self.settings_set(id) else { continue };
            let bundle = match set.read(game) {
                Ok(bundle) => bundle,
                Err(e) => {
                    log!("{} not read: {e}", set.what());
                    continue;
                }
            };
            let step = self.settings_decide(id, &set.what(), &bundle, &list);
            if step == SettingsStep::Send {
                sends.push((id.clone(), bundle, list.newest_settings(id).map(|v| v.n), None));
            }
            decided.insert(id.clone(), step);
        }
        // Each send is in step once it went up; one that failed or waits stays to be sent.
        self.share.settings = decided;
        if !sends.is_empty()
            && let Err(e) = self.share_send_settings(shared, sends)
        {
            log!("addon settings not sent yet: {e}");
        }
    }

    /// One settings set against the list (the game closed): what this PC does about it. A question
    /// already asked stands while what it showed still is; a new one is remembered.
    pub(super) fn settings_decide(&mut self, id: &str, what: &str, bundle: &Bundle, list: &List) -> SettingsStep {
        let local = bundle.fingerprint();
        let base = self.share.memory.settings.get(id).cloned();
        let newest = list.newest_settings(id);
        if let Some(split) = self.share.memory.splits.get(id) {
            if newest.map(|v| v.n) == Some(split.nas) && local.as_deref() == Some(split.local.as_str()) {
                return SettingsStep::Ask { nas: split.nas };
            }
            self.share.memory.splits.remove(id);
        }
        let nas = newest.map(|v| (v.sha256.as_str(), v.n));
        let step = if id == game_settings::ID {
            let local_files = bundle.files.keys().map(|k| k.to_lowercase()).collect();
            game_settings::step(local.as_deref(), base.as_deref(), nas, &local_files, newest.map(|v| v.files.as_slice()).unwrap_or_default())
        } else {
            share::settings_step(local.as_deref(), base.as_deref(), nas, || {
                bundle.newest.zip(newest.map(|v| v.written.unwrap_or(v.saved))).is_some_and(|(mine, theirs)| mine > theirs)
            })
        };
        match &step {
            SettingsStep::InStep { adopt } => {
                if let Some(version) = adopt.and_then(|n| list.settings_version(n)) {
                    self.share.memory.settings.insert(id.to_string(), version.sha256.clone());
                }
            }
            SettingsStep::Ask { nas } => {
                let pc = list.settings_version(*nas).map(|v| v.pc.clone()).unwrap_or_default();
                if let Some(local) = &local {
                    log!("{what} changed here and on {pc} since they last matched: the player picks");
                    self.share.memory.splits.insert(id.to_string(), Split { nas: *nas, pc, local: local.clone() });
                }
            }
            SettingsStep::Send | SettingsStep::BringIn { .. } => {}
        }
        step
    }

    /// Sends settings as their addons' next versions, under the list's lock. One whose newest
    /// version moved since it was decided (`seen`) waits for the next look. An addon's settings
    /// now the newest are in step; one that waits, or a send that failed, stays
    /// `SettingsStep::Send`, which Sync now counts. Returns how many are now the newest on the NAS
    /// (sent, or already there).
    pub(super) fn share_send_settings(&mut self, shared: &Shared, sends: Vec<(String, Bundle, Option<u64>, Option<String>)>) -> io::Result<usize> {
        let pc = self.engine.pc().to_string();
        let _lock = share::lock(shared, &pc)?;
        let mut list = share::read(shared)?;
        let mut done = 0;
        for (id, bundle, seen, note) in sends {
            let Some(print) = bundle.fingerprint() else { continue };
            let newest = list.newest_settings(&id);
            if newest.map(|v| v.n) != seen {
                continue;
            }
            if newest.is_none_or(|v| v.sha256 != print) {
                let version = share::send_settings(shared, &mut list, &id, &pc, &bundle, note, Utc::now())?;
                log!("{id}: settings sent as version {}", version.n);
                if id == game_settings::ID && self.share.memory.game_own_kept.as_ref().is_some_and(|k| k.placed.as_deref() != Some(print.as_str())) {
                    self.share.memory.game_own_kept = None;
                }
            }
            // The game's own step is its caller's.
            if id != game_settings::ID {
                self.share.settings.insert(id.clone(), SettingsStep::InStep { adopt: None });
            }
            self.share.memory.settings.insert(id, print);
            done += 1;
        }
        self.share.list = Some(list);
        Ok(done)
    }

    /// At Play, the game closed: what the other PCs changed comes here, then their settings, then
    /// the game's own.
    pub(super) fn share_before_play(&mut self) {
        if self.share_place().is_none() && self.game_share_place().is_none() {
            return;
        }
        self.share_look();
        // The list wasn't read just now: nothing follows an older reading of it.
        if self.share.problem.is_some() {
            return;
        }
        let started = "The game was started meanwhile: what the other PCs changed comes in once it's closed";
        let now = Utc::now();
        if self.share_place().is_some() && !self.addons_before_play_shared(started, now) {
            return;
        }
        if !self.game_settings_before_play(started, now) {
            return;
        }
        self.share_save();
        self.share_look();
    }

    /// Play's part for the addons: what the other PCs changed, then their settings. `false` when
    /// the game was started meanwhile, so nothing more is written.
    fn addons_before_play_shared(&mut self, started: &str, now: DateTime<Utc>) -> bool {
        let Some((shared, game)) = self.share_place() else { return true };
        let Some(list) = self.share.list.clone() else { return true };
        let mut changed = false;
        let follow: Vec<String> = self.share.steps.iter().filter(|(_, s)| **s == Step::Follow).map(|(id, _)| id.clone()).collect();
        for id in follow {
            if self.game_started_meanwhile() {
                self.feedback.card_line(started, false, true);
                return false;
            }
            let Some(entry) = list.get(&id) else { continue };
            match self.share_follow(&shared, &game, entry, now) {
                Followed::Done(text) => {
                    changed = true;
                    self.activity(Kind::Addon, &text);
                    self.feedback.card_line(&text, true, false);
                }
                Followed::Waits(text) | Followed::Downloading(text) => self.feedback.card_line(&text, false, false),
                Followed::Failed(text) => {
                    log!("{text}");
                    self.feedback.card_line(&text, false, true);
                }
                Followed::Quiet => {}
            }
        }
        self.share_save();
        if changed {
            self.addons_scan();
        }
        // The settings, now that the addons are in step.
        self.share_decide(&list);
        self.share_settings(&shared, &game);
        let bring: Vec<(String, u64, bool)> = self
            .share
            .settings
            .iter()
            .filter_map(|(id, step)| match step {
                SettingsStep::BringIn { n, keep_mine } => Some((id.clone(), *n, *keep_mine)),
                _ => None,
            })
            .collect();
        for (id, n, keep_mine) in bring {
            if self.game_started_meanwhile() {
                self.feedback.card_line(started, false, true);
                break;
            }
            match self.share_bring_settings(&shared, &game, &id, n, keep_mine, now) {
                Ok(text) => {
                    self.activity(Kind::Addon, &text);
                    self.feedback.card_line(&text, true, false);
                }
                Err(text) => {
                    log!("{text}");
                    self.feedback.card_line(&text, false, true);
                }
            }
        }
        let asks: Vec<String> = self
            .share
            .memory
            .splits
            .iter()
            .filter(|(id, _)| id.as_str() != game_settings::ID)
            .map(|(id, split)| {
                let name = list.get(id).map(|e| e.name.clone()).unwrap_or_else(|| id.clone());
                format!("{name}'s settings changed here and on {}: pick one in Addons (this PC's stay for now)", split.pc)
            })
            .collect();
        for text in asks {
            self.feedback.card_line(&text, false, true);
        }
        true
    }

    /// Brings one addon in step with the list: removed, installed or updated, its switches set.
    fn share_follow(&mut self, shared: &Shared, game: &GameAddons, entry: &Entry, now: DateTime<Utc>) -> Followed {
        let from = entry.pc.clone();
        let id = entry.id.clone();
        let Some(release) = &entry.release else {
            if self.addons.record(&id).is_none() {
                self.share.memory.agreed.insert(id, Agreed::removed());
                return Followed::Quiet;
            }
            return match self.addons.remove_as_on(game, &self.engine.local, &id, entry.keep_settings, &from, now) {
                Ok(record) => {
                    self.share.memory.agreed.insert(id, Agreed::removed());
                    let kept = if entry.keep_settings { "; its settings stay for a reinstall" } else { "" };
                    Followed::Done(format!("{} removed, as on {from}: it's in the Recycle Bin{kept}", record.name))
                }
                Err(e) => Followed::Failed(format!("{} wasn't removed as on {from}: {e}", entry.name)),
            };
        };
        let mut text = None;
        if !self.has_release(&id, release) {
            // Only what SurvivorsForge lets in here comes in by itself, or what the player OK'd on
            // its row (that very file); its row says why the rest waits.
            let verdict = self.share_verdict(entry, release);
            let ok = self.share.oks.has(&id, &release.sha256);
            match verdict {
                _ if forge::admits(&verdict, ok) => {}
                Verdict::No(_) => return Followed::Quiet,
                _ => return Followed::Waits(format!("{} {} from {from} waits for your OK (Addons)", entry.name, release.shown_version())),
            }
            let mut want = want_of(entry, release);
            want.ok = ok;
            let cache = self.engine.local.addon_cache();
            if release.url.is_empty() {
                // A zip imported from a file on another PC waits on the NAS.
                let fetched = fsx::read_verified(&shared.shared_zip(&release.sha256), &release.sha256)
                    .and_then(|bytes| fsx::write_atomic(&cache.join(format!("{}.zip", release.sha256)), &bytes));
                if let Err(e) = fetched {
                    return Followed::Failed(format!("{} can't come from {from}: its zip in the network folder: {e}", entry.name));
                }
                self.addons.mark_ready(id.clone(), want.release.tag.clone(), release.sha256.clone());
            }
            if self.addons.downloaded(&cache, &id, &want.release).is_none() {
                let why = self.addons.refused(&id, &want.release.tag);
                return match why {
                    Some(why) => Followed::Failed(format!("{} {} from {from} wasn't downloaded: {why}", entry.name, release.shown_version())),
                    None => Followed::Downloading(format!("{} {} from {from} is still downloading; it comes in next time", entry.name, release.shown_version())),
                };
            }
            match self.addons_install(&want) {
                Ok(done) => text = Some(format!("{done}, as on {from}")),
                Err(Refused::Waits(said)) => return Followed::Waits(said),
                Err(Refused::Never(said)) => return Followed::Failed(said),
                Err(Refused::Failed(e)) => return Followed::Failed(format!("{} {} from {from} wasn't installed: {e}", entry.name, release.shown_version())),
            }
        }
        // The switches, as on the other PCs.
        let Some((mut switches, folders)) = self.addons.apply_followed_switches(&id, entry.pinned, entry.settings_per_pc) else {
            return Followed::Failed(format!("{} from {from} isn't recorded here", entry.name));
        };
        if let Err(e) = self.addons.save_store(&self.engine.local.addon_store()) {
            return Followed::Failed(format!("{} from {from}: addons.json not saved: {e}", entry.name));
        }
        // Each folder on or off as the list has it; only the folders that differ are set.
        let off = entry.agreed().off;
        let default_off: Vec<bool> = folders.iter().map(|f| self.addons.default_off(f)).collect();
        let switched = EnableList::read(&game.enable_list()).and_then(|mut list| {
            let changed: Vec<(String, bool)> = folders
                .iter()
                .zip(&default_off)
                .map(|(folder, disabled)| (folder, list.is_on(folder, *disabled), !off.iter().any(|o| o.eq_ignore_ascii_case(folder))))
                .filter(|(_, is_on, on)| is_on != on)
                .map(|(folder, _, on)| (folder.clone(), on))
                .collect();
            for (folder, on) in &changed {
                list.set(folder, *on);
            }
            if !changed.is_empty() {
                list.write(&game.enable_list())?;
            }
            Ok(changed)
        });
        let mut by_folder = false;
        match switched {
            Ok(changed) => {
                // Every folder turned the same way is the addon turned on or off; else each by name.
                let whole = changed.len() == folders.len() && changed.windows(2).all(|w| w[0].1 == w[1].1);
                match changed.first() {
                    Some((_, on)) if whole => switches.push(if *on { "turned on" } else { "turned off" }.into()),
                    _ => {
                        by_folder = !changed.is_empty();
                        switches.extend(changed.iter().map(|(folder, on)| format!("{folder} turned {}", if *on { "on" } else { "off" })));
                    }
                }
            }
            Err(e) => return Followed::Failed(format!("{} couldn't be switched as on {from}: {e}", entry.name)),
        }
        self.share.memory.agreed.insert(id.clone(), entry.agreed());
        self.share.memory.removed.remove(&id);
        if text.is_none() && !switches.is_empty() {
            // "Survivor UI: SurvivorPlates turned off", where a folder is named.
            let gap = if by_folder { ": " } else { " " };
            text = Some(format!("{}{gap}{}, as on {from}", entry.name, switches.join(", ")));
        }
        text.map_or(Followed::Quiet, Followed::Done)
    }

    /// Puts version `n` of a settings set in place (the game closed): an addon's, or the game's
    /// own. With `keep_mine`, this PC's settings go up first as a version of their own, and the
    /// chosen ones after them, so the chosen ones stay the newest.
    pub(super) fn share_bring_settings(&mut self, shared: &Shared, game: &GameAddons, id: &str, n: u64, keep_mine: bool, now: DateTime<Utc>) -> Result<String, String> {
        let list = self.share.list.clone().ok_or("the shared list isn't read")?;
        let version = list.settings_version(n).cloned().ok_or_else(|| format!("settings version {n} is gone from the list"))?;
        let set = self.settings_set(id).ok_or_else(|| format!("{id} isn't installed here"))?;
        let what = set.what();
        let theirs = share::settings_bytes(shared, &version).map_err(|e| format!("{what} from {} couldn't be read: {e}", version.pc))?;
        // A PC joining (no B yet) keeps its own game settings as a version: "Use them instead".
        let joining = keep_mine && id == game_settings::ID && !self.share.memory.settings.contains_key(id);
        let mut kept = None;
        if keep_mine {
            let mine = set.read(game).map_err(|e| format!("{what} here couldn't be read: {e}"))?;
            kept = self.share_keep_then(shared, id, mine, &theirs, &version).map_err(|e| format!("{what}: {e}"))?;
        }
        // The keep can wait on the lock: the game may have started meanwhile.
        if self.game_started_meanwhile() {
            return Err(format!("{what}: the game was started meanwhile, so they come in once it's closed"));
        }
        self.place_settings(&set, &theirs, game, id, now, joining).map_err(|e| format!("{what} from {} weren't put in place: {e}", version.pc))?;
        self.share.memory.settings.insert(id.to_string(), version.sha256.clone());
        self.share.memory.splits.remove(id);
        if joining {
            let placed = set.read(game).ok().and_then(|b| b.fingerprint());
            self.share.memory.game_own_kept = kept.map(|n| OwnKept { n, placed });
        }
        self.share_save();
        Ok(match set {
            SettingsSet::Addon { name, .. } => format!("{name}: settings from {}", version.pc),
            SettingsSet::Game => format!("{} came from {}", game_settings::NAME, version.pc),
        })
    }

    /// This PC's settings kept as a version, then the chosen version sent again as the newest.
    /// Returns the version that holds this PC's settings (sent now, or already on the NAS).
    fn share_keep_then(&mut self, shared: &Shared, id: &str, mine: Bundle, theirs: &Bundle, chosen: &share::SettingsVersion) -> io::Result<Option<u64>> {
        let pc = self.engine.pc().to_string();
        let _lock = share::lock(shared, &pc)?;
        let mut list = share::read(shared)?;
        // Another PC sent newer settings since the choice: nothing moves, the next look decides.
        if list.newest_settings(id).map(|v| v.n) != Some(chosen.n) {
            return Err(io::Error::other("another PC sent newer settings meanwhile; see what it shows now"));
        }
        let mut kept = None;
        if let Some(print) = mine.fingerprint() {
            match list.settings.iter().find(|v| v.addon == id && v.sha256 == print) {
                Some(there) => kept = Some(there.n),
                None => {
                    let note = Some(format!("{pc}'s settings, kept when {}'s were taken", chosen.pc));
                    kept = Some(share::send_settings(shared, &mut list, id, &pc, &mine, note, Utc::now())?.n);
                    let note = Some(format!("version {} from {}, chosen", chosen.n, chosen.pc));
                    let again = Bundle { newest: chosen.written, ..theirs.clone() };
                    share::send_settings(shared, &mut list, id, &pc, &again, note, Utc::now())?;
                }
            }
        }
        self.share.list = Some(list);
        Ok(kept)
    }

    /// Puts a settings set in place after a backup of this PC's; the 10 newest backups of that set
    /// are kept. The backup made when this PC joined is named apart and stays.
    pub(super) fn place_settings(&mut self, set: &SettingsSet, bundle: &Bundle, game: &GameAddons, id: &str, now: DateTime<Utc>, joining: bool) -> io::Result<()> {
        let backups = self.engine.local.addon_backups();
        let suffix = format!(" {} settings", names::safe(id));
        let name = format!("{}{suffix}{}", names::stamp(now), if joining { " before joining" } else { "" });
        set.place(bundle, game, &backups.join(name))?;
        if let Err(e) = fsx::prune_dirs(&backups, &suffix, SETTINGS_BACKUPS_KEPT) {
            log!("old settings backups not cleared: {e}");
        }
        Ok(())
    }

    // The player's choices -------------------------------------------------------------------------

    /// An incoming addon's Install anyway: the player's OK for exactly the file another PC has,
    /// where SurvivorsForge asks first here, through the same verdict as every install (a No stays
    /// a No). It comes in now, the game closed, once its download is here; else at the next Play.
    /// One of SurvivorsForge's own in a version its word here doesn't cover is asked about first.
    pub(super) fn incoming_install_anyway(&mut self, id: &str) {
        if self.addons_blocked() {
            return;
        }
        let Some(entry) = self.share.list.as_ref().and_then(|l| l.get(id)).cloned() else { return };
        let Some(release) = entry.release.clone() else { return };
        match self.share_verdict(&entry, &release) {
            Verdict::No(why) => return self.addons_say(format!("{} doesn't come here: {}", entry.name, why.text())),
            Verdict::Ask(_) => self.share.oks.give(id, &release.sha256),
            Verdict::Go => {}
        }
        self.share.follow_now.insert(id.to_string());
        let tag = release.tag.clone().unwrap_or_default();
        if let Some(slug) = entry.forge.as_deref().filter(|slug| forge::slug_ok(slug))
            && self.forge_unknown(slug, &tag)
        {
            self.forge_ask_follow(slug, &tag);
            return self.addons_say(format!("Asking SurvivorsForge about {} {} before it comes in…", entry.name, release.shown_version()));
        }
        self.share_follow_now(true);
        if self.share.follow_now.contains(id) {
            self.addons_say(format!("{} {} is downloading; it comes in once it's here and the game is closed.", entry.name, release.shown_version()));
        }
    }

    /// The incoming addons the player asked for now, in step with the list as at Play: the game
    /// closed, their downloads here. One still downloading stays asked for; one that came in, that
    /// asks first again (said when `announce`), or that SurvivorsForge says No to, is done with.
    pub(super) fn share_follow_now(&mut self, announce: bool) {
        if self.share.follow_now.is_empty() || self.sync.game_running() {
            return;
        }
        let (Some((shared, game)), Some(list)) = (self.share_place(), self.share.list.clone()) else { return };
        let now = Utc::now();
        let mut changed = false;
        for id in self.share.follow_now.clone() {
            let Some(entry) = list.get(&id).filter(|e| self.share.steps.get(&id) == Some(&Step::Follow) && !e.removed()) else {
                self.share.follow_now.remove(&id);
                continue;
            };
            match self.share_follow(&shared, &game, entry, now) {
                Followed::Done(text) => {
                    changed = true;
                    self.share.follow_now.remove(&id);
                    self.activity(Kind::Addon, &text);
                    self.addons_say(text);
                }
                // It asks first again (another file, say): its row's Install anyway is the way on.
                Followed::Waits(text) => {
                    self.share.follow_now.remove(&id);
                    if announce {
                        self.addons_say(text);
                    }
                }
                Followed::Downloading(_) => {}
                Followed::Failed(text) => {
                    self.share.follow_now.remove(&id);
                    log!("{text}");
                    self.addons_say(text);
                }
                Followed::Quiet => {
                    self.share.follow_now.remove(&id);
                }
            }
        }
        self.share_save();
        if changed {
            self.addons_scan();
            self.share_look();
        } else {
            self.addons_fetch();
        }
    }

    /// The player removed a shared addon here: the other PCs remove it too, once the list hears.
    /// The player removed an addon here. A shared one goes from the other PCs too, once the list
    /// hears; one kept to this PC stays off this PC, whatever the others have.
    pub(super) fn share_removed(&mut self, id: &str, keep_settings: bool, local_only: bool) {
        if self.share_place().is_none() {
            return;
        }
        if local_only {
            self.share.memory.opted_out.insert(id.into());
        } else {
            self.share.memory.removed.insert(id.into(), keep_settings);
        }
        self.share_save();
    }

    pub(super) fn addon_local_only(&mut self, id: &str, on: bool) {
        let Some(name) = self.addons.set_local_only(id, on) else { return };
        if let Err(e) = self.addons.save_store(&self.engine.local.addon_store()) {
            return self.addons_say(format!("{name}: addons.json not saved: {e}"));
        }
        // Shared again: decided as on a first look, so nothing either side has is lost.
        self.share.memory.agreed.remove(id);
        let text = if on { format!("{name} is kept on this PC only") } else { format!("{name} goes to every PC again") };
        self.activity(Kind::Addon, &text);
        self.share_look();
    }

    pub(super) fn addon_settings_per_pc(&mut self, id: &str, on: bool) {
        let Some(name) = self.addons.set_settings_per_pc(id, on) else { return };
        if let Err(e) = self.addons.save_store(&self.engine.local.addon_store()) {
            return self.addons_say(format!("{name}: addons.json not saved: {e}"));
        }
        let text = if on { format!("{name} keeps its settings per PC, on every PC") } else { format!("{name}'s settings go to every PC again") };
        self.activity(Kind::Addon, &text);
        self.share_look();
    }

    /// The player's pick after one addon's settings changed here and on another PC: this PC's go
    /// up as the newest, or the other PC's come here (this PC's kept as a version first).
    pub(super) fn addon_settings_choice(&mut self, id: &str, keep_here: bool) {
        if !keep_here && self.addons_blocked() {
            return;
        }
        let Some((shared, game)) = self.share_place() else { return };
        self.settings_choice(&shared, &game, id, keep_here);
    }

    pub(super) fn settings_choice(&mut self, shared: &Shared, game: &GameAddons, id: &str, keep_here: bool) {
        let Some(split) = self.share.memory.splits.get(id).cloned() else { return };
        let Some(set) = self.settings_set(id) else { return };
        // The game's answers show in Settings and on the rail, an addon's on the Addons tab; the
        // flag says a problem.
        let (kind, say): (Kind, fn(&mut Worker, String, bool)) = match &set {
            SettingsSet::Game => (Kind::Game, |w, text, problem| w.game_say(text, problem)),
            SettingsSet::Addon { .. } => (Kind::Addon, |w, text, _| w.addons_say(text)),
        };
        let what = set.what();
        let mine = match set.read(game) {
            Ok(mine) => mine,
            Err(e) => return say(self, format!("{what} here couldn't be read: {e}"), true),
        };
        // The answer applies to what was shown.
        let list = match share::read(shared) {
            Ok(list) => list,
            Err(e) => return say(self, format!("The shared addon list can't be read: {e}"), true),
        };
        if mine.fingerprint().as_deref() != Some(split.local.as_str()) || list.newest_settings(id).map(|v| v.n) != Some(split.nas) {
            say(self, format!("{what} changed again since you looked: see what it shows now."), true);
            self.share_look();
            return;
        }
        self.share.list = Some(list);
        let result = if keep_here {
            let note = Some(format!("kept over {}'s", split.pc));
            let kept = match &set {
                SettingsSet::Addon { name, .. } => format!("{name}: this PC's settings kept and sent to the other PCs"),
                SettingsSet::Game => format!("{}: this PC's kept and sent to the other PCs", game_settings::NAME),
            };
            match self.share_send_settings(shared, vec![(id.to_string(), mine, Some(split.nas), note)]) {
                Ok(1) => Ok(kept),
                Ok(_) => Err("another PC sent newer settings meanwhile; see what it shows now".into()),
                Err(e) => Err(e.to_string()),
            }
        } else {
            self.share_bring_settings(shared, game, id, split.nas, true, Utc::now())
        };
        match result {
            Ok(text) => {
                self.share.memory.splits.remove(id);
                self.activity(kind, &text);
                say(self, text, false);
            }
            Err(e) => say(self, format!("{what}: {e}"), true),
        }
        self.share_save();
        self.share_look();
    }

    // What the Addons tab shows ------------------------------------------------------------------

    /// The line under an installed addon about the other PCs, its kind (0 plain, 1 news, 2 needs
    /// a look) and, for a settings question, the other PC.
    pub(super) fn share_line(&self, id: &str) -> (String, i32, String) {
        // The list can be read for the game's settings alone: addons stay on each PC then.
        if self.share_place().is_none() {
            return (String::new(), 0, String::new());
        }
        let Some(list) = &self.share.list else { return (String::new(), 0, String::new()) };
        let Some(record) = self.addons.record(id) else { return (String::new(), 0, String::new()) };
        if record.local_only {
            return ("Only on this PC".into(), 0, String::new());
        }
        if let Some(split) = self.share.memory.splits.get(id) {
            return (format!("Its settings changed here and on {} since they last matched: pick one below", split.pc), 2, split.pc.clone());
        }
        let gone = self.addons.lines().iter().find(|l| l.id == id).is_some_and(|l| l.standing == Standing::Gone);
        if gone && list.get(id).is_some_and(|e| !e.removed()) {
            return ("Its folder is gone here: it comes back at the next Play, as on the other PCs. Remove it to remove it everywhere".into(), 2, String::new());
        }
        if self.share.steps.get(id) == Some(&Step::Follow)
            && let Some(entry) = list.get(id)
        {
            let text = match &entry.release {
                None => format!("Removed on {}: it goes from this PC at the next Play", entry.pc),
                Some(release) if share::identity(record) != release.sha256 => match self.share_verdict(entry, release) {
                    Verdict::Go => format!("{} has {}: it comes here at the next Play", entry.pc, release.shown_version()),
                    // SurvivorsForge holds it back here, by this PC's own cache.
                    Verdict::Ask(why) => {
                        return (format!("{} has {}, which waits for your OK: {}", entry.pc, release.shown_version(), why.text()), 2, String::new());
                    }
                    Verdict::No(why) => {
                        return (format!("{} has {}, which doesn't come here: {}", entry.pc, release.shown_version(), why.text()), 2, String::new());
                    }
                },
                Some(_) => format!("Changed on {}: the same here at the next Play", entry.pc),
            };
            return (text, 1, String::new());
        }
        if let Some(SettingsStep::BringIn { .. }) = self.share.settings.get(id)
            && let Some(version) = list.newest_settings(id)
        {
            return (format!("Settings from {} come in at the next Play", version.pc), 1, String::new());
        }
        let per_pc = if record.settings_per_pc { " · settings per PC" } else { "" };
        (format!("On every PC{per_pc}"), 0, String::new())
    }

    /// The repository of an addon the list brings (its row's GitHub page).
    pub(super) fn share_repo(&self, id: &str) -> Option<String> {
        self.share.list.as_ref()?.get(id)?.repo.clone()
    }

    /// Rows for the addons the list brings that this PC doesn't have yet.
    pub(super) fn share_incoming(&self) -> Vec<AddonLine> {
        let Some(list) = &self.share.list else { return Vec::new() };
        let view = self.forge_view();
        self.share
            .steps
            .iter()
            .filter(|(id, step)| **step == Step::Follow && self.addons.record(id.as_str()).is_none())
            .filter_map(|(id, _)| {
                let entry = list.get(id)?;
                let release = entry.release.as_ref()?;
                let own = entry.forge.is_some();
                let (badge, badge_kind) = match entry.badge {
                    Badge::Verified if self.addons.catalog().get(id).is_none() => super::addons::UNLISTED_BADGE,
                    Badge::Verified => ("Discord Verified", 0),
                    _ if own => FORGE_BADGE,
                    Badge::Added => ("Added by you", 1),
                    Badge::File => ("From a file", 2),
                };
                // SurvivorsForge's word as this PC's own cache has it, never the other PC's.
                let verdict = self.share_verdict(entry, release);
                let slug = self.entry_slug(entry);
                let theirs = forge_badge(&view, slug.as_deref(), &verdict, None);
                let badges: Vec<(String, i32)> =
                    if own { theirs.into_iter().collect() } else { std::iter::once((badge.to_string(), badge_kind)).chain(theirs).collect() };
                // OK'd on its row (Install anyway, for exactly this file): it comes as one SurvivorsForge lets in.
                let okd = self.share.oks.has(id, &release.sha256);
                let (reason, reason_kind) = match &verdict {
                    _ if forge::admits(&verdict, okd) => (String::new(), 0),
                    // Its row's Install anyway is the player's OK.
                    Verdict::Ask(why) => (format!("Waits for your OK: {}", why.text()), 1),
                    Verdict::No(why) => (why.text(), 2),
                    Verdict::Go => (String::new(), 0),
                };
                let comes = forge::admits(&verdict, okd);
                let now = self.share.follow_now.contains(id);
                Some(AddonLine {
                    id: id.clone(),
                    initials: initials(&entry.name),
                    icon: self.forge.icon_key(view.icon(slug.as_deref()).as_deref()).unwrap_or_default(),
                    name: entry.name.clone(),
                    badge: badges.first().map_or_else(|| badge.to_string(), |(text, _)| text.clone()),
                    badge_kind,
                    badges,
                    reason,
                    reason_kind,
                    author: entry.repo.as_deref().and_then(|r| r.split('/').next()).unwrap_or_default().to_string(),
                    version: release.shown_version(),
                    state: match (comes, now) {
                        (true, true) => format!("From {}: it comes here as soon as its download is in", entry.pc),
                        (true, false) => format!("From {}: it comes here at the next Play", entry.pc),
                        (false, _) => format!("From {}", entry.pc),
                    },
                    state_kind: if comes { 1 } else { 0 },
                    has_page: entry.repo.is_some(),
                    installing: comes && now,
                    incoming: true,
                    ..AddonLine::default()
                })
            })
            .collect()
    }

    /// One sentence for Sync now about the addons; `None` when addons stay on each PC.
    pub(super) fn share_status(&self) -> Option<String> {
        self.share_place()?;
        if let Some(problem) = &self.share.problem {
            return Some(problem.trim_end_matches('.').to_string());
        }
        let coming = self.share.steps.values().filter(|s| **s == Step::Follow).count()
            + self.share.settings.values().filter(|s| matches!(s, SettingsStep::BringIn { .. })).count();
        let asks = self.share.memory.splits.keys().filter(|id| id.as_str() != game_settings::ID).count();
        // What changed here and didn't go up (a write that failed) goes at the next look.
        let unsent: Vec<&str> = self
            .share
            .settings
            .iter()
            .filter(|(_, step)| **step == SettingsStep::Send)
            .map(|(id, _)| self.addons.record(id).map_or(id.as_str(), |r| r.name.as_str()))
            .collect();
        let waits = match unsent.as_slice() {
            _ if self.share.steps.values().any(|s| *s == Step::Send) => Some("This PC's addon changes go up at the next look".to_string()),
            [] => None,
            [name] => Some(format!("{name}'s settings go up at the next look")),
            names => Some(format!("The settings of {} addons go up at the next look", names.len())),
        };
        let text = match (coming, asks) {
            (0, 0) => return Some(waits.unwrap_or_else(|| "Addons and their settings are the same as in the network folder".into())),
            (0, 1) => "Addons: 1 settings question on the Addons tab".to_string(),
            (0, n) => format!("Addons: {n} settings questions on the Addons tab"),
            (1, _) => "Addons: 1 change from another PC comes in at the next Play".into(),
            (n, _) => format!("Addons: {n} changes from other PCs come in at the next Play"),
        };
        Some(match waits {
            Some(waits) => format!("{text}. {waits}"),
            None => text,
        })
    }

    /// The Installed list's line about the other PCs; empty when addons stay on each PC.
    pub(super) fn share_header(&self) -> String {
        if self.share_place().is_none() {
            return String::new();
        }
        if let Some(problem) = &self.share.problem {
            return problem.clone();
        }
        let coming = self.share.steps.values().filter(|s| **s == Step::Follow).count()
            + self.share.settings.values().filter(|s| matches!(s, SettingsStep::BringIn { .. })).count();
        let asks = self.share.memory.splits.keys().filter(|id| id.as_str() != game_settings::ID).count();
        let mut text = match coming {
            0 => "The same addons and settings on every PC".to_string(),
            1 => "On every PC · 1 change from another PC comes in at the next Play".to_string(),
            n => format!("On every PC · {n} changes from other PCs come in at the next Play"),
        };
        if asks > 0 {
            text.push_str(&format!(" · {asks} settings question{} below", if asks == 1 { "" } else { "s" }));
        }
        text
    }
}
