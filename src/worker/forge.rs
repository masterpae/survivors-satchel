//! SurvivorsForge, the worker's side. Its registry is asked for at most every 30 minutes (at start
//! and on the idle tick, while its switch is on), an addon's details and versions when its panel
//! opens (or its Versions…), a version's findings when the player asks, and a version's word before
//! an install the player asked for when nothing cached has it. Every answer goes to this PC's cache
//! (`forge::Cache`), which reads and remembers it; what it means is the core's (`forge`,
//! `overview::ForgeView`). Icons are asked for when the Addons tab shows the addons they belong to,
//! a screenshot when the viewer shows it: one picture at a time, behind whatever already waits for
//! the network thread. The network thread asks; nothing here waits for it.

use super::{Worker, clock, when};
use crate::net::{ForgeAnswer, Job};
use crate::view::{PanelLine, PictureLine};
use crate::win;
use chrono::{DateTime, Local, Utc};
use satchel_core::addons::forge::{self, Counts, PictureKind, Verdict, Why, Word};
use satchel_core::addons::github;
use satchel_core::addons::overview::{self, ForgeView, Offer, Standing};
use satchel_core::log;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A panel request that failed is sent again only when the panel is opened this long after.
const PANEL_RETRY: Duration = Duration::from_secs(60);
/// What the panel says until then.
const PANEL_FAILED: &str = "SurvivorsForge didn't answer; try again in a minute";
/// The panel's download bars: the newest versions'.
const BARS: usize = 8;
/// The panel's findings: the worst few.
const FINDINGS: usize = 5;
/// How much of a finding's message the panel shows.
const FINDING_CHARS: usize = 200;

/// An install the player asked for, held until SurvivorsForge's word on its version comes.
#[derive(Clone, Debug)]
pub(super) enum Held {
    /// Browse's Install, or Install anyway.
    Install { id: String, anyway: bool },
    /// Update now, or Update anyway.
    Update { id: String, anyway: bool },
    /// The Removed list's Install again, or Install anyway.
    Again { id: String, anyway: bool },
    /// Versions…' Install this version, or Install anyway.
    Version { id: String, tag: String, anyway: bool },
}

impl Held {
    fn id(&self) -> &str {
        match self {
            Held::Install { id, .. } | Held::Update { id, .. } | Held::Again { id, .. } | Held::Version { id, .. } => id,
        }
    }
}

/// The picture viewer: the screenshot it shows (its place in the strip, its address), and whether
/// its download still waits for its turn.
#[derive(Clone, Debug)]
pub(super) struct Viewer {
    index: usize,
    url: String,
    waiting: bool,
}

pub(super) struct ForgeState {
    /// Its answers on this PC (`%LOCALAPPDATA%\SurvivorsSatchel\forge`).
    cache: forge::Cache,
    /// What this PC remembers (the ETag, the last answer and try, the slugs listed, deleted and
    /// blocked), as of the cache's last change.
    state: forge::State,
    /// The registry as last received.
    registry: Option<forge::Registry>,
    /// Our catalog's ids → their slugs.
    join: BTreeMap<String, String>,
    /// Each slug's versions, as cached under its registry entry as it stands now.
    versions: BTreeMap<String, Vec<forge::Version>>,
    /// The registry request is on its way.
    asking: bool,
    /// Requests on their way: details and versions by slug, findings by slug and tag.
    details_out: BTreeSet<String>,
    versions_out: BTreeSet<String>,
    findings_out: BTreeSet<(String, String)>,
    /// The offer whose panel was opened last, until it closes.
    open: Option<String>,
    /// Addons' details read this run, with their About text: for the panel.
    details: BTreeMap<String, (forge::Details, String)>,
    /// Versions' scans fetched this run, by slug and tag: the panel's findings.
    scans: BTreeMap<(String, String), forge::Scan>,
    /// Slugs whose last request failed, and when: the panel says so, and opening it asks again
    /// only a minute later.
    failed: BTreeMap<String, Instant>,
    /// Installs waiting for a version's word: its slug and tag, and the install.
    held: Vec<(String, String, Held)>,
    /// Versions asked about for an install the player asked for: decided with what came.
    asked: BTreeSet<(String, String)>,
    /// Versions asked about for an addon another PC brings, until the next registry.
    follow_asked: BTreeSet<(String, String)>,
    /// The icons asked for this run, and the ones waiting for the network thread: a line shows its
    /// icon once the window holds it.
    icons: forge::IconQueue,
    /// The thumbnails of the addon opened (`open`), as its pictures row asked for them: a frame
    /// shows its thumbnail once the window holds it. Begun again when another addon opens or the
    /// pane closes, as the window drops what it held.
    thumbs: forge::IconQueue,
    /// The addon whose pictures row the window shows, and how many frames it shows.
    thumbs_for: Option<(String, usize)>,
    /// The picture on its way now, one at a time: its address, its kind, and for a thumbnail the
    /// addon it is for.
    picture_out: Option<(String, PictureKind, Option<String>)>,
    /// The viewer, while it is open.
    viewer: Option<Viewer>,
    /// The Addons page in view (0 Installed, 1 Browse; -1 neither): the icons of what it lists
    /// are asked for, also of what it lists later.
    shown: i32,
}

impl ForgeState {
    pub(super) fn new(dir: PathBuf) -> ForgeState {
        ForgeState {
            cache: forge::Cache::open(dir),
            state: forge::State::default(),
            registry: None,
            join: BTreeMap::new(),
            versions: BTreeMap::new(),
            asking: false,
            details_out: BTreeSet::new(),
            versions_out: BTreeSet::new(),
            findings_out: BTreeSet::new(),
            open: None,
            details: BTreeMap::new(),
            scans: BTreeMap::new(),
            failed: BTreeMap::new(),
            held: Vec::new(),
            asked: BTreeSet::new(),
            follow_asked: BTreeSet::new(),
            icons: forge::IconQueue::default(),
            thumbs: forge::IconQueue::default(),
            thumbs_for: None,
            picture_out: None,
            viewer: None,
            shown: -1,
        }
    }

    /// The window's name for an icon it holds, for a line whose icon is `url`; `None` until then
    /// (the initials show).
    pub(super) fn icon_key(&self, url: Option<&str>) -> Option<String> {
        let url = url?;
        self.icons.shown(url).then(|| forge::Pictures::name(url))
    }

    /// What SurvivorsForge says, for the core's verdicts and offers.
    pub(super) fn view(&self, switch_on: bool, now: DateTime<Utc>) -> ForgeView<'_> {
        ForgeView { switch_on, now, state: &self.state, registry: self.registry.as_ref(), join: &self.join, versions: &self.versions }
    }

    /// An install of `id` waits for SurvivorsForge's word: it shows as under way.
    pub(super) fn holds(&self, id: &str) -> bool {
        self.held.iter().any(|(_, _, held)| held.id() == id)
    }

    /// Versions…' install of version `tag` of `id` waits for SurvivorsForge's word on it.
    pub(super) fn holds_version(&self, id: &str, tag: &str) -> bool {
        self.held.iter().any(|(_, _, held)| matches!(held, Held::Version { id: i, tag: t, .. } if i == id && t == tag))
    }

    /// The registry's entry for `slug`, as last received.
    pub(super) fn listed(&self, slug: &str) -> Option<&forge::ForgeAddon> {
        self.registry.as_ref()?.get(slug)
    }

    /// `slug`'s versions, as cached under its registry entry as it stands now.
    pub(super) fn versions(&self, slug: &str) -> Option<&[forge::Version]> {
        self.versions.get(slug).map(Vec::as_slice)
    }

    /// The installs that waited for `slug`'s versions, taken out to be decided with what came.
    fn take_held(&mut self, slug: &str) -> Vec<(String, String, Held)> {
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held).into_iter().partition(|(s, _, _)| s.as_str() == slug);
        self.held = rest;
        mine
    }

    /// A version just asked about is decided with what is known: while it is, `forge_ask_first`
    /// doesn't ask again.
    pub(super) fn deciding(&mut self, key: (String, String)) {
        self.asked.insert(key);
    }

    pub(super) fn decided(&mut self, key: &(String, String)) {
        self.asked.remove(key);
    }

    /// The cache's state again, after it changed.
    fn reread(&mut self) {
        self.state = self.cache.state();
    }

    /// The versions cached under each registry entry as it stands now, again: after the registry
    /// or a versions answer changed what is current.
    fn reload_versions(&mut self) {
        let slugs: Vec<String> = self.state.fetched_under.keys().filter_map(|key| key.strip_prefix("versions/")).map(str::to_string).collect();
        self.versions = slugs.into_iter().filter_map(|slug| self.cache.versions(&slug).map(|versions| (slug, versions))).collect();
        // A file that couldn't be read was dropped from the cache meanwhile.
        self.reread();
    }

    /// An addon's details for the panel, kept with their About text.
    fn keep_details(&mut self, slug: &str, details: forge::Details) {
        let about = forge::about_text(&details.description_md);
        self.details.insert(slug.to_string(), (details, about));
    }
}

/// SurvivorsForge's badge for an addon it lists, or listed, as `slug`, from this PC's cache: its
/// trust level, or that it blocked the addon, dropped it, or scanned another file than the version
/// in question (`verdict`). On a card the level is the one of the version offered (`offered`);
/// when SurvivorsForge says nothing about that version, the badge names the version its level is
/// of. While it is silent only what holds an addon back stays; off, none.
pub(super) fn forge_badge(view: &ForgeView, slug: Option<&str>, verdict: &Verdict, offered: Option<&str>) -> Option<(String, i32)> {
    let (level, kind, of) = forge_level(view, slug, verdict, offered)?;
    let of = of.filter(|tag| !tag.is_empty()).map(|tag| format!(" ({tag})")).unwrap_or_default();
    Some((format!("SurvivorsForge · {level}{of}"), kind))
}

/// What `forge_badge` says, in parts: the level as the badge words it ("verified", "flagged", "no
/// longer listed"…), its kind, and the version it is of when that isn't the one offered. The
/// table's SurvivorsForge column and its filters use the level alone.
pub(super) fn forge_level(view: &ForgeView, slug: Option<&str>, verdict: &Verdict, offered: Option<&str>) -> Option<(&'static str, i32, Option<String>)> {
    let slug = slug.filter(|_| view.switch_on)?;
    let level_of = |word: Word| match word {
        Word::Verified => ("verified", 4),
        Word::Scanned => ("scanned", 4),
        Word::Flagged => ("flagged", 5),
        Word::Unscanned => ("unscanned", 5),
        Word::Blocked => ("blocked", 6),
        Word::SourceChanged => ("source changed", 6),
    };
    let ((level, kind), of) = if view.state.blocked.contains_key(slug) || matches!(verdict, Verdict::No(Why::Blocked(_))) {
        (("blocked", 6), None)
    } else if matches!(verdict, Verdict::No(Why::SourceChanged)) {
        (("source changed", 6), None)
    } else if matches!(verdict, Verdict::No(Why::OtherFile)) {
        (("another file", 6), None)
    } else if forge::dropped(view.state, view.registry, slug) {
        (("no longer listed", 5), None)
    } else if let Some((word, _, _)) = offered.and_then(|tag| view.word(slug, tag)) {
        (level_of(word), None)
    } else if let Some((word, _)) = offered.and_then(|tag| view.restriction(slug, tag, None)) {
        // A word that held the offered version back before still holds it back.
        (level_of(word), None)
    } else {
        let entry = view.entry(slug)?;
        // Its latest version's level, and on a card, which version that is.
        (level_of(entry.word()?), offered.and(entry.latest.as_ref()).map(|latest| latest.tag.clone()))
    };
    (kind != 4 || view.online()).then_some((level, kind, of))
}

/// SurvivorsForge's word against an installed addon's update, or its replacement, as its row says
/// it, with its kind: 1 it waits for the player's OK (Update anyway), 2 never.
pub(super) fn row_reason(verdict: &Verdict) -> (String, i32) {
    match verdict {
        Verdict::Go => (String::new(), 0),
        Verdict::Ask(why) => (format!("Waits for your OK: {}", why.text()), 1),
        Verdict::No(Why::Offline) => ("SurvivorsForge isn't answering: no updates until it's back".into(), 2),
        Verdict::No(why) => {
            let why = why.text();
            (format!("{}. It stays as you have it; no updates.", why.trim_end_matches('.')), 2)
        }
    }
}

/// When SurvivorsForge last answered: "14:40" today, else with its day.
fn answer_time(at: DateTime<Utc>) -> String {
    if at.with_timezone(&Local).date_naive() == Local::now().date_naive() { clock(at) } else { when(at) }
}

/// A request that brought nothing to keep, for the log.
fn failure(answer: &ForgeAnswer) -> String {
    match answer {
        ForgeAnswer::Body { .. } => "an answer".into(),
        ForgeAnswer::NotModified => "nothing new (304) where a body was due".into(),
        ForgeAnswer::NotFound => "nothing by that name (404)".into(),
        ForgeAnswer::Limited => "asked too often (429); the next try waits".into(),
        ForgeAnswer::Down(why) => format!("no answer ({why})"),
    }
}

/// The scan of the version shown, in one line: SurvivorsForge's word and what it found. While it
/// is silent, only a word that holds the addon back is shown, as of its last answer.
fn scan_line(word: Word, counts: Counts, online: bool, answered: Option<DateTime<Utc>>) -> String {
    let counted = |n: u32, what: &str| if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") };
    let line = match word {
        Word::Verified => "verified: an admin approved this file".to_string(),
        Word::Scanned => "scanned: nothing found".into(),
        Word::Flagged => {
            let mut found = Vec::new();
            if counts.critical > 0 {
                found.push(counted(counts.critical, "critical finding"));
            }
            if counts.warn > 0 {
                found.push(counted(counts.warn, "warning"));
            }
            if counts.info > 0 {
                found.push(format!("{} info", counts.info));
            }
            if found.is_empty() { "flagged".into() } else { format!("flagged: {}", found.join(", ")) }
        }
        Word::Unscanned => "unscanned: not scanned yet".into(),
        Word::Blocked => "blocked".into(),
        Word::SourceChanged => "source changed: the file changed after its scan".into(),
    };
    if online {
        return line;
    }
    if matches!(word, Word::Verified | Word::Scanned) {
        return String::new();
    }
    match answered {
        Some(at) => format!("{line} (as of {})", answer_time(at)),
        None => line,
    }
}

/// One finding as the panel lists it: "lua.api_override · BordoUI/ThemeUI.lua, line 2627: Replaces
/// the game function ChatEdit_SendText".
fn finding_line(finding: &forge::Finding) -> String {
    let place = match (finding.file.trim(), finding.line) {
        ("", _) => String::new(),
        (file, Some(line)) => format!(" · {file}, line {line}"),
        (file, None) => format!(" · {file}"),
    };
    let message: String = finding.message.split_whitespace().collect::<Vec<_>>().join(" ");
    let message = if message.chars().count() > FINDING_CHARS {
        format!("{}…", message.chars().take(FINDING_CHARS).collect::<String>().trim_end())
    } else {
        message
    };
    let message = if message.is_empty() { String::new() } else { format!(": {message}") };
    format!("{}{place}{message}", finding.rule.trim())
}

/// Downloads per version, newest first, for the panel's bars: the version, its count and its share
/// of the largest. Versions without a count are left out; none counted, no bars.
fn bars(versions: &[forge::Version]) -> Vec<(String, u64, f32)> {
    let counted: Vec<(String, u64)> = versions.iter().filter_map(|v| Some((v.tag.clone(), v.downloads?))).take(BARS).collect();
    let most = counted.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1);
    counted.into_iter().map(|(tag, n)| (tag, n, n as f32 / most as f32)).collect()
}

impl Worker {
    /// SurvivorsForge's switch in Settings: off, nothing is asked of it and its word counts for
    /// nothing.
    pub(super) fn forge_on(&self) -> bool {
        self.engine.settings.addons.forge
    }

    /// What SurvivorsForge says now, for the core's verdicts.
    pub(super) fn forge_view(&self) -> ForgeView<'_> {
        self.forge.view(self.forge_on(), Utc::now())
    }

    /// What this PC's cache holds, read at start: the registry and its join with our catalog, and
    /// the versions cached under each entry as it stands.
    pub(super) fn forge_load(&mut self) {
        let f = &mut self.forge;
        f.registry = f.cache.registry().map(|(registry, _)| registry);
        f.join = f.registry.as_ref().map(|registry| forge::join(self.addons.catalog(), registry)).unwrap_or_default();
        f.reload_versions();
    }

    /// The registry, when the switch is on and it's due: at most every 30 minutes, counted from
    /// the last try, across restarts (`forge::due`), with the ETag of the one kept. Called at every
    /// tick, so it costs nothing until it's due (the state in memory follows every change).
    pub(super) fn forge_look(&mut self) {
        if !self.forge_on() || self.forge.asking || !forge::due(&self.forge.state, Utc::now()) {
            return;
        }
        let f = &mut self.forge;
        // The ETag only while the registry it names is still on disk.
        f.reread();
        let etag = f.state.etag.clone();
        f.asking = self.net.send(Job::ForgeRegistry { etag }).is_ok();
    }

    /// The registry's answer, kept with its bookkeeping: a 304 keeps what is there, and anything
    /// but an answer counts as a try (the next one waits its 30 minutes). The lists go again after
    /// it (the caller's part): verdicts, badges and SurvivorsForge's own offers follow it.
    pub(super) fn forge_registry_answer(&mut self, answer: ForgeAnswer) {
        let now = Utc::now();
        let f = &mut self.forge;
        f.asking = false;
        match answer {
            ForgeAnswer::Body { text, etag } => match f.cache.store_registry(&text, etag.as_deref(), now) {
                Ok(registry) => {
                    log!("SurvivorsForge's registry: {} addons", registry.addons.len());
                    // An icon anywhere but on SurvivorsForge's host or GitHub's is never fetched.
                    let elsewhere: Vec<&str> =
                        registry.addons.iter().filter(|a| a.icon_url.as_deref().is_some_and(|url| !forge::picture_url_ok(url.trim()))).map(|a| a.id.as_str()).collect();
                    if !elsewhere.is_empty() {
                        log!("SurvivorsForge's registry: the icons of {} are on another host, so they aren't fetched", elsewhere.join(", "));
                    }
                    f.join = forge::join(self.addons.catalog(), &registry);
                    f.registry = Some(registry);
                    // What was fetched under an entry that changed is stale: it is asked for again
                    // at the next open or install.
                    f.details.clear();
                    f.scans.clear();
                    f.follow_asked.clear();
                }
                Err(why) => log!("{why}"),
            },
            ForgeAnswer::NotModified => f.cache.touch(now),
            other => {
                log!("SurvivorsForge's registry: {}", failure(&other));
                f.cache.tried(now);
            }
        }
        f.reload_versions();
        // The open panel's details, when they are still current.
        if let Some(slug) = self.forge.open.clone().and_then(|id| self.offer_slug(&id))
            && let Some(details) = self.forge.cache.details(&slug)
        {
            self.forge.keep_details(&slug, details);
        }
        self.forge.reread();
    }

    /// An addon's details (its panel opened, or a dropped addon's check), kept; a blocked addon is
    /// remembered as blocked (the cache sees to it). A dropped addon's 404 means it was deleted
    /// there, so our rule applies to it again. The lists go again after it, then the open addon's
    /// pictures, now known (`forge_thumbs_again`): the caller's part.
    pub(super) fn forge_details_answer(&mut self, slug: &str, answer: ForgeAnswer) {
        let f = &mut self.forge;
        f.details_out.remove(slug);
        match answer {
            ForgeAnswer::Body { text, .. } => match f.cache.store_details(slug, &text) {
                Ok(details) => {
                    // A screenshot from anywhere but SurvivorsForge's host or GitHub's is left out
                    // of the strip, and never fetched.
                    let refused = details.images.iter().filter(|p| !forge::picture_url_ok(p.url.trim())).count();
                    if refused > 0 {
                        log!("SurvivorsForge's details of {slug}: {refused} picture(s) on another host left out");
                    }
                    f.keep_details(slug, details);
                }
                Err(why) => {
                    log!("{why}");
                    f.failed.insert(slug.to_string(), Instant::now());
                }
            },
            ForgeAnswer::NotFound if forge::dropped(&f.state, f.registry.as_ref(), slug) => {
                log!("SurvivorsForge: {slug} left its registry because it was deleted there");
                f.cache.mark_deleted(slug);
            }
            other => {
                log!("SurvivorsForge's details of {slug}: {}", failure(&other));
                f.failed.insert(slug.to_string(), Instant::now());
            }
        }
        f.reread();
    }

    /// An addon's versions (its panel opened, or an install asked about its version), kept. The
    /// installs that waited for them are returned, to be decided with what came once the lists
    /// have gone again (`install_held`, the caller's part).
    pub(super) fn forge_versions_answer(&mut self, slug: &str, answer: ForgeAnswer) -> Vec<(String, String, Held)> {
        let f = &mut self.forge;
        f.versions_out.remove(slug);
        match answer {
            ForgeAnswer::Body { text, .. } => {
                if let Err(why) = f.cache.store_versions(slug, &text) {
                    log!("{why}");
                    f.failed.insert(slug.to_string(), Instant::now());
                }
            }
            other => {
                log!("SurvivorsForge's versions of {slug}: {}", failure(&other));
                f.failed.insert(slug.to_string(), Instant::now());
            }
        }
        f.reload_versions();
        self.forge.take_held(slug)
    }

    /// A version's findings, asked for by "Show what it found": kept, for the panel.
    pub(super) fn forge_findings_answer(&mut self, slug: &str, tag: &str, answer: ForgeAnswer) {
        let f = &mut self.forge;
        f.findings_out.remove(&(slug.to_string(), tag.to_string()));
        match answer {
            ForgeAnswer::Body { text, .. } => match f.cache.store_scan(slug, tag, &text) {
                Ok(scan) => {
                    f.scans.insert((slug.to_string(), tag.to_string()), scan);
                }
                Err(why) => {
                    log!("{why}");
                    f.failed.insert(slug.to_string(), Instant::now());
                }
            },
            other => {
                log!("SurvivorsForge's findings of {slug} {tag}: {}", failure(&other));
                f.failed.insert(slug.to_string(), Instant::now());
            }
        }
        f.reread();
    }

    /// Before an install the player asked for (Install, Install anyway, Update now, Update
    /// anyway): when nothing cached gives SurvivorsForge's word on the version, its versions are
    /// asked for once (its rules allow that before installing a version), and the install waits
    /// for the answer: `true`. Not with the switch off or SurvivorsForge silent, not for an addon
    /// it doesn't list now (versions fetched then never count), and not again for a version just
    /// asked about: that is decided with what is known.
    pub(super) fn forge_ask_first(&mut self, slug: &str, tag: &str, held: Held) -> bool {
        let key = (slug.to_string(), tag.to_string());
        let listed = self.forge_view().entry(slug).is_some();
        if !listed || !forge::online(&self.forge.state, Utc::now()) || self.forge.asked.contains(&key) {
            return false;
        }
        if !self.forge.versions_out.contains(slug) {
            if self.net.send(Job::ForgeVersions(slug.to_string())).is_err() {
                return false;
            }
            self.forge.versions_out.insert(slug.to_string());
        }
        if !self.forge.holds(held.id()) {
            self.forge.held.push((key.0, key.1, held));
        }
        true
    }

    /// SurvivorsForge's word on version `tag` of its own addon `slug` isn't cached, and it can be
    /// asked about it for an addon another PC brings: switch on, listed now, answering, not asked
    /// yet.
    pub(super) fn forge_unknown(&self, slug: &str, tag: &str) -> bool {
        let view = self.forge_view();
        view.entry(slug).is_some()
            && view.online()
            && view.word(slug, tag).is_none()
            && !self.forge.follow_asked.contains(&(slug.to_string(), tag.to_string()))
    }

    /// An addon of SurvivorsForge's own that another PC brings, in a version its cached word
    /// doesn't cover: its versions are asked for once (until the next registry), and it comes in
    /// at a Play after the answer, if that lets it in.
    pub(super) fn forge_ask_follow(&mut self, slug: &str, tag: &str) {
        let key = (slug.to_string(), tag.to_string());
        if self.forge.follow_asked.contains(&key) || self.forge.versions_out.contains(slug) {
            return;
        }
        if self.net.send(Job::ForgeVersions(slug.to_string())).is_ok() {
            self.forge.versions_out.insert(slug.to_string());
            self.forge.follow_asked.insert(key);
        }
    }

    /// The slug SurvivorsForge lists a Browse offer under: its own for `forge:<slug>`, else the one
    /// joined to our catalog id (or listed before with its repository); none with the switch off.
    pub(super) fn offer_slug(&self, id: &str) -> Option<String> {
        let view = self.forge_view();
        match forge::slug_in(id) {
            Some(slug) => view.switch_on.then(|| slug.to_string()),
            None => {
                let entry = self.addons.catalog().get(id)?;
                view.slug_for(None, Some(entry.id.as_str()), Some(entry.repo.as_str()))
            }
        }
    }

    /// The tag of the version Browse shows for an offer: a newer release found on GitHub, else our
    /// catalog's latest; for SurvivorsForge's own, its registry's latest.
    pub(super) fn offer_tag(&self, id: &str) -> Option<String> {
        if let Some(slug) = forge::slug_in(id) {
            return self.forge.registry.as_ref()?.get(slug)?.latest.as_ref().map(|l| l.tag.clone());
        }
        let entry = self.addons.catalog().get(id)?;
        Some(self.addons.newer(id).map_or_else(|| entry.latest.tag.clone(), |r| r.tag.clone()))
    }

    /// The repository of an addon only on SurvivorsForge (`forge:<slug>`), as listed now or before.
    pub(super) fn forge_repo(&self, id: &str) -> Option<String> {
        let slug = forge::slug_in(id)?;
        let listed = self.forge.registry.as_ref().and_then(|r| r.get(slug)).map(|a| a.repo.clone());
        listed.or_else(|| self.forge.state.repos.get(slug).cloned()).filter(|r| !r.trim().is_empty())
    }

    /// The folders SurvivorsForge lists for its own addon `slug`: its zip must hold exactly these,
    /// so a listing without folders (or with none that could be read) lets no zip in.
    pub(super) fn forge_folders(&self, slug: Option<&str>) -> Option<Vec<String>> {
        let entry = self.forge.registry.as_ref()?.get(slug?)?;
        Some(entry.folders.clone())
    }

    /// When release `tag` of an addon was published, when that's known: our catalog's date, else
    /// SurvivorsForge's for the slug it lists the addon under: its registry's latest, or one of its
    /// versions (cached).
    pub(super) fn forge_published(&self, id: &str, slug: Option<&str>, tag: &str) -> Option<DateTime<Utc>> {
        self.addons.catalog().published(id, tag).or_else(|| {
            let slug = slug?;
            let latest = self.forge.registry.as_ref().and_then(|r| r.get(slug)).and_then(|e| e.latest.as_ref()).filter(|l| l.tag == tag);
            latest.and_then(|l| l.published_at).or_else(|| self.forge.versions.get(slug)?.iter().find(|v| v.tag == tag)?.published_at)
        })
    }

    /// An addon's versions on SurvivorsForge, for its Versions…: asked for when it lists the addon
    /// and they aren't cached under its entry as it stands (nor on their way, nor failed a moment
    /// ago), as opening its panel does.
    pub(super) fn forge_fetch_versions(&mut self, slug: &str) {
        let listed = self.forge_view().entry(slug).is_some();
        let failed = self.forge.failed.get(slug).is_some_and(|at| at.elapsed() < PANEL_RETRY);
        if !listed || failed || self.forge.versions.contains_key(slug) || self.forge.versions_out.contains(slug) {
            return;
        }
        if self.net.send(Job::ForgeVersions(slug.to_string())).is_ok() {
            self.forge.versions_out.insert(slug.to_string());
        }
    }

    /// The icons of what the Addons page in view lists now: after the lists changed.
    pub(super) fn forge_icons_again(&mut self) {
        if self.forge.shown >= 0 {
            self.forge_icons_wanted(self.forge.shown);
        }
    }

    /// Whether SurvivorsForge's versions of `slug` are on their way.
    pub(super) fn forge_versions_loading(&self, slug: &str) -> bool {
        self.forge.versions_out.contains(slug)
    }

    // Pictures -------------------------------------------------------------------------------------

    /// The icons of what the Addons tab shows on `page` (0 Installed, 1 Browse; -1 neither): each
    /// one not asked for yet this run joins the queue, while icons may be fetched at all
    /// (`forge_icons_allowed`); when they may not, what waits is dropped.
    pub(super) fn forge_icons_wanted(&mut self, page: i32) {
        self.forge.shown = page;
        if self.forge_icons_allowed() {
            let urls: Vec<String> = match page {
                0 => self.addons.lines().iter().filter_map(|l| l.icon_url.clone()).collect(),
                1 => self.addons.offers().iter().filter_map(|o| o.icon_url.clone()).collect(),
                _ => Vec::new(),
            };
            for url in urls {
                self.forge.icons.want(&url);
            }
        } else {
            self.forge.icons.drop_waiting();
        }
        // Browse left, or back: the open addon's thumbnails stop, or go on.
        self.forge_thumbs_again();
    }

    /// Whether icons may be fetched now: SurvivorsForge's switch is on, and the Addons tab shows
    /// its Installed or Browse page. Off, or the tab left, no more go out (one already on its way
    /// finishes, and is kept), and the queue is dropped.
    fn forge_icons_allowed(&self) -> bool {
        self.forge_on() && matches!(self.forge.shown, 0 | 1)
    }

    /// Whether the open addon's thumbnails may be fetched now: SurvivorsForge's switch is on, the
    /// Addons tab shows Browse, and the pictures row the window shows is the open addon's.
    fn forge_thumbs_allowed(&self) -> bool {
        let open = self.forge.open.as_deref();
        self.forge_on() && self.forge.shown == 1 && open.is_some() && self.forge.thumbs_for.as_ref().map(|(id, _)| id.as_str()) == open
    }

    /// The open addon's pictures row shows `frames` frames: their thumbnails are asked for, in
    /// their order, and only those (never every picture it has).
    pub(super) fn forge_thumbs_wanted(&mut self, id: &str, frames: usize) {
        self.forge.thumbs_for = Some((id.to_string(), frames));
        self.forge_thumbs_again();
    }

    /// The thumbnails of the frames shown, asked for again where they may be (after the details
    /// came, Browse shown again); where they may not, what waits is dropped.
    pub(super) fn forge_thumbs_again(&mut self) {
        if self.forge_thumbs_allowed()
            && let Some((id, frames)) = self.forge.thumbs_for.clone()
        {
            let slug = self.offer_slug(&id);
            let urls: Vec<String> = self.forge_pictures(slug.as_deref()).iter().take(frames).map(|p| p.url.trim().to_string()).collect();
            for url in urls {
                self.forge.thumbs.want(&url);
            }
        } else {
            self.forge.thumbs.drop_waiting();
        }
        self.forge_pictures_next();
    }

    /// The next picture to the network thread, when none is on its way: the viewer's screenshot
    /// first, then the open addon's thumbnails, then the icons, each in the order asked for and
    /// only while it may be fetched (`forge::next_picture`). One at a time, so whatever already
    /// waits there goes first, and an install asked for meanwhile waits for one picture at most.
    fn forge_pictures_next(&mut self) {
        if self.forge.picture_out.is_some() {
            return;
        }
        let (thumbs_allowed, icons_allowed) = (self.forge_thumbs_allowed(), self.forge_icons_allowed());
        let shot = match self.forge.viewer.as_mut().filter(|v| v.waiting) {
            Some(viewer) => {
                viewer.waiting = false;
                Some(viewer.url.clone())
            }
            None => None,
        };
        let f = &mut self.forge;
        let Some((url, kind)) = forge::next_picture(shot, &mut f.thumbs, thumbs_allowed, &mut f.icons, icons_allowed) else { return };
        let addon = (kind == PictureKind::Thumb).then(|| f.open.clone()).flatten();
        if self.net.send(Job::Picture { url: url.clone(), kind }).is_ok() {
            self.forge.picture_out = Some((url, kind, addon));
        }
    }

    /// A picture's answer: an icon goes to the window, which keeps it for this run; a thumbnail,
    /// when its addon is still the one open, to the window, which keeps it while it stays open; the
    /// viewer's screenshot shows if it is still the one open. A failure is one log line: the
    /// initials stay, the frame keeps its placeholder, and the viewer says the picture isn't
    /// available.
    pub(super) fn forge_picture_answer(&mut self, url: &str, kind: PictureKind, pixels: Result<SharedPixelBuffer<Rgba8Pixel>, String>) {
        let mut thumb_for = None;
        if let Some((out, out_kind, addon)) = self.forge.picture_out.take() {
            if out == url && out_kind == kind {
                thumb_for = addon;
            } else {
                self.forge.picture_out = Some((out, out_kind, addon));
            }
        }
        if let Err(why) = &pixels {
            let what = match kind {
                PictureKind::Icon => "icon",
                PictureKind::Thumb => "thumbnail",
                PictureKind::Shot => "picture",
            };
            log!("{url}: the {what} wasn't shown: {why}");
        }
        match kind {
            PictureKind::Icon => {
                let shown = match pixels {
                    Ok(pixels) => {
                        self.ui.push_icon(forge::Pictures::name(url), pixels);
                        true
                    }
                    Err(_) => false,
                };
                self.forge.icons.done(url, shown);
            }
            // Its addon closed meanwhile, or another opened: the window holds none of its now.
            PictureKind::Thumb => {
                if let Some(id) = thumb_for.filter(|id| self.forge.open.as_deref() == Some(id.as_str())) {
                    let shown = match pixels {
                        Ok(pixels) => {
                            self.ui.push_thumb(id, forge::Pictures::name(url), pixels);
                            true
                        }
                        Err(_) => false,
                    };
                    self.forge.thumbs.done(url, shown);
                }
            }
            PictureKind::Shot => {
                if let Some(viewer) = self.forge.viewer.as_mut().filter(|v| v.url == url) {
                    viewer.waiting = false;
                    self.ui.push_shot(viewer.index, pixels.ok());
                }
            }
        }
        self.forge_pictures_next();
    }

    /// The window's name for a thumbnail it holds of the open addon, for a frame whose picture is
    /// `url`; empty until then (the placeholder shows).
    fn thumb_key(&self, url: &str) -> String {
        if self.forge.thumbs.shown(url) { forge::Pictures::name(url) } else { String::new() }
    }

    /// The screenshots of the addon SurvivorsForge lists as `slug` that may be shown: those on its
    /// own host or GitHub's, in their order.
    fn forge_pictures(&self, slug: Option<&str>) -> Vec<&forge::Picture> {
        let details = slug.filter(|_| self.forge_on()).and_then(|s| self.forge.details.get(s));
        details.map(|(d, _)| d.images.iter().filter(|p| forge::picture_url_ok(p.url.trim())).collect()).unwrap_or_default()
    }

    /// The viewer opened on, or moved to, picture `index` of addon `id`'s strip: its download goes
    /// ahead of the icons. Only the one shown is held decoded, by the window.
    pub(super) fn forge_picture_show(&mut self, id: &str, index: usize) {
        let slug = self.offer_slug(id);
        let url = self.forge_pictures(slug.as_deref()).get(index).map(|p| p.url.trim().to_string());
        match url {
            Some(url) => {
                self.forge.viewer = Some(Viewer { index, url, waiting: true });
                self.forge_pictures_next();
            }
            None => {
                log!("{id}: picture {} isn't in its details", index + 1);
                self.forge.viewer = None;
                self.ui.push_shot(index, None);
            }
        }
    }

    /// The viewer closed: a screenshot still on its way is dropped when it comes.
    pub(super) fn forge_picture_close(&mut self) {
        self.forge.viewer = None;
    }

    /// Browse opened an offer's panel (again, perhaps). Its details and versions are fetched when
    /// SurvivorsForge lists it and they aren't cached under its entry as it stands; for an addon
    /// that left the registry, its details, which tell whether it was deleted or blocked. Our
    /// addons SurvivorsForge doesn't list fetch nothing. A request that failed isn't sent again by
    /// itself: opening the panel a minute later asks again.
    pub(super) fn browse_open(&mut self, id: &str) {
        // Another addon: the window drops the thumbnails it held, so they are asked for anew.
        if self.forge.open.as_deref() != Some(id) {
            self.forge.thumbs = forge::IconQueue::default();
        }
        self.forge.open = Some(id.to_string());
        self.forge_thumbs_again();
        let Some(slug) = self.offer_slug(id) else { return };
        let listed = self.forge.registry.as_ref().is_some_and(|r| r.get(&slug).is_some());
        let dropped = forge::dropped(&self.forge.state, self.forge.registry.as_ref(), &slug);
        if !listed && !dropped {
            return;
        }
        if self.forge.failed.get(&slug).is_some_and(|at| at.elapsed() < PANEL_RETRY) {
            return;
        }
        self.forge.failed.remove(&slug);
        if listed
            && !self.forge.details.contains_key(&slug)
            && let Some(details) = self.forge.cache.details(&slug)
        {
            self.forge.keep_details(&slug, details);
        }
        let want_details = (dropped || !self.forge.details.contains_key(&slug)) && !self.forge.details_out.contains(&slug);
        let want_versions = listed && !self.forge.versions.contains_key(&slug) && !self.forge.versions_out.contains(&slug);
        if want_details && self.net.send(Job::ForgeDetails(slug.clone())).is_ok() {
            self.forge.details_out.insert(slug.clone());
        }
        if want_versions && self.net.send(Job::ForgeVersions(slug.clone())).is_ok() {
            self.forge.versions_out.insert(slug);
        }
        self.forge.reread();
    }

    /// The panel closed: its thumbnails stop, and the window drops those it held.
    pub(super) fn browse_close(&mut self) {
        self.forge.open = None;
        self.forge.thumbs = forge::IconQueue::default();
        self.forge.thumbs_for = None;
    }

    /// "Show what it found": the findings of the version the panel shows, from the cache or asked
    /// for.
    pub(super) fn forge_show_scan(&mut self, id: &str) {
        let (Some(slug), Some(tag)) = (self.offer_slug(id), self.offer_tag(id)) else { return };
        let key = (slug.clone(), tag.clone());
        if self.forge.scans.contains_key(&key) || self.forge.findings_out.contains(&key) {
            return;
        }
        if let Some(scan) = self.forge.cache.scan(&slug, &tag) {
            self.forge.scans.insert(key, scan);
        } else if self.net.send(Job::ForgeFindings { slug: slug.clone(), tag }).is_ok() {
            self.forge.failed.remove(&slug);
            self.forge.findings_out.insert(key);
        }
        self.forge.reread();
    }

    /// A link from Settings or About: only SurvivorsForge's repository and website, and a GitHub
    /// repository's own page, are opened.
    pub(super) fn open_link(&self, url: &str) {
        let repository = url.strip_prefix("https://github.com/").is_some_and(|rest| github::repo_from_input(rest).as_deref() == Some(rest));
        if url == forge::REPO || url == forge::SITE || repository {
            win::open_url(url);
        } else {
            log!("a link that isn't SurvivorsForge's or a GitHub repository's wasn't opened: {url}");
        }
    }

    /// Browse's status line, Settings' line under the switch, and whether SurvivorsForge is silent:
    /// switched on and no answer within the hour.
    pub(super) fn forge_status(&self) -> (String, String, bool) {
        if !self.forge_on() {
            return (String::new(), "Off".into(), false);
        }
        let state = &self.forge.state;
        if forge::online(state, Utc::now()) {
            let answer = state.answered.map(|at| format!("Last answer {}", answer_time(at))).unwrap_or_default();
            return (String::new(), answer, false);
        }
        // The first request on this PC, or one on its way after the hour (at a start of day, say):
        // until it answers or fails, it is being asked, not silent.
        if self.forge.asking || state.tried.is_none() {
            return ("Checking SurvivorsForge…".into(), "Checking…".into(), false);
        }
        match state.answered {
            None => ("SurvivorsForge isn't answering · no answer yet on this PC".into(), "Isn't answering; no answer yet on this PC".into(), true),
            Some(at) => {
                let at = answer_time(at);
                (format!("SurvivorsForge isn't answering · last answer {at}"), format!("Isn't answering since {at}"), true)
            }
        }
    }

    /// The panel's content for the offer opened last: SurvivorsForge's About, our notes when they
    /// are the shown version's, its downloads per version, the scan, the folders and the action.
    pub(super) fn forge_panel(&self) -> Option<PanelLine> {
        let id = self.forge.open.as_deref()?;
        let offer = self.addons.offers().iter().find(|o| o.id == id)?;
        let view = self.forge_view();
        let online = view.online();
        let slug = offer.forge_slug.as_deref();
        let listed = slug.and_then(|s| view.entry(s));
        let details = slug.and_then(|s| self.forge.details.get(s));
        let catalog = self.addons.catalog().get(id);
        let tag = self.offer_tag(id).unwrap_or_default();
        // Our notes are our catalog's latest's: shown only under that version.
        let (notes_title, notes) = match catalog.filter(|_| self.addons.newer(id).is_none()).and_then(|e| e.latest.notes.as_deref().map(|n| (e, n))) {
            Some((entry, notes)) => (format!("What's new in {}", entry.latest.shown_version()), overview::plain_notes(notes, 700)),
            None => (String::new(), String::new()),
        };
        // Its word on the version shown, or one that held that version back before and still does.
        let sha = match catalog {
            Some(entry) => (entry.latest.tag == tag).then_some(entry.latest.sha256.as_str()),
            None => listed.and_then(|e| e.latest.as_ref()).filter(|l| l.tag == tag).and_then(|l| l.sha256.as_deref()),
        };
        let word = slug.and_then(|s| view.word(s, &tag).or_else(|| view.restriction(s, &tag, sha).map(|(word, counts)| (word, counts, None))));
        let found = slug.and_then(|s| self.forge.scans.get(&(s.to_string(), tag.clone())));
        let finding_out = slug.is_some_and(|s| self.forge.findings_out.contains(&(s.to_string(), tag.clone())));
        let flagged = matches!(word, Some((Word::Flagged, counts, _)) if counts.critical > 0 || counts.warn > 0);
        let (action, reason) = self.panel_action(offer);
        Some(PanelLine {
            id: id.to_string(),
            verified_author: online && details.and_then(|(d, _)| d.verified_author).or(listed.and_then(|e| e.verified_author)) == Some(true),
            about: details.map(|(d, _)| d.summary.trim()).filter(|s| !s.is_empty()).map_or_else(|| offer.summary.clone(), str::to_string),
            about_more: details.map(|(_, about)| about.clone()).unwrap_or_default(),
            notes_title,
            notes,
            bars: slug.and_then(|s| self.forge.versions.get(s)).map(|v| bars(v)).unwrap_or_default(),
            scan: word.as_ref().map(|(w, c, _)| scan_line(*w, *c, online, self.forge.state.answered)).unwrap_or_default(),
            scan_more: flagged && found.is_none() && !finding_out,
            findings: found.map(|s| s.worst_first(FINDINGS).into_iter().map(finding_line).collect::<Vec<_>>()).unwrap_or_default(),
            folders: catalog.map(|e| e.folders.clone()).or_else(|| listed.map(|e| e.folders.clone())).unwrap_or_default(),
            pictures: self
                .forge_pictures(slug)
                .iter()
                .map(|p| PictureLine {
                    width: p.width.unwrap_or(16),
                    height: p.height.unwrap_or(9),
                    caption: p.caption.as_deref().map(str::trim).unwrap_or_default().to_string(),
                    thumb: self.thumb_key(p.url.trim()),
                })
                .collect(),
            action,
            reason,
            loading: finding_out || slug.is_some_and(|s| self.forge.details_out.contains(s) || self.forge.versions_out.contains(s)),
            error: if slug.is_some_and(|s| self.forge.failed.contains_key(s)) { PANEL_FAILED.into() } else { String::new() },
        })
    }

    /// The installed addon an offer stands for, as the Installed tab lists it, which Browse's
    /// Uninstall removes (`overview::installed_line`): its own record, else the one line from its
    /// repository or holding its main folder whose folders are all its own (our catalog's, or
    /// SurvivorsForge's). `None` while it isn't installed, and when no line is this addon alone
    /// (a zip pack holding it and others): its row then says Installed, without Uninstall.
    pub(super) fn offer_line(&self, offer: &Offer) -> Option<&overview::Line> {
        if !offer.installed {
            return None;
        }
        let folders = match forge::slug_in(&offer.id) {
            Some(slug) => self.forge_folders(Some(slug)).unwrap_or_default(),
            None => self.addons.catalog().get(&offer.id).map(|e| e.folders.clone()).unwrap_or_default(),
        };
        overview::installed_line(&offer.id, &offer.repo, &folders, self.addons.lines())
    }

    /// An offer's button, and the reason above it, in its pane and on its table row: Install;
    /// Install anyway when SurvivorsForge asks first; Installing…; Installed; or, for an installed
    /// addon with a newer version, Update or Update anyway.
    pub(super) fn panel_action(&self, offer: &Offer) -> (i32, String) {
        let busy = self.addons.waits_for(&offer.id) || self.forge.holds(&offer.id);
        if busy {
            return (2, String::new());
        }
        if !offer.installed {
            return match &offer.verdict {
                Verdict::Ask(why) => (1, why.text()),
                _ => (0, String::new()),
            };
        }
        let line = self.addons.lines().iter().find(|l| l.id == offer.id);
        let update = line.is_some_and(|l| matches!(l.standing, Standing::Update { .. } | Standing::Kept { newer: Some(_) }));
        match line.map(|l| &l.verdict) {
            Some(Verdict::Go) if update => (4, String::new()),
            Some(Verdict::Ask(why)) if update => (5, why.text()),
            _ => (3, String::new()),
        }
    }
}
