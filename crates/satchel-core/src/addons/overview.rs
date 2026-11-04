//! The Addons tab's model: every addon in AddOns with where it stands, what's wrong with it in
//! plain words and what can be done with it; the updates that go in by themselves at Play; the
//! addons on offer, our catalog's and SurvivorsForge's, and Browse's order for them; every version
//! of an addon (Versions…); and what to ask GitHub about. The app formats and wires it.

use super::catalog::{self, Asset, Catalog, Release, State};
use super::enabled::EnableList;
use super::forge::{self, Origin, Verdict};
use super::github::Pick;
use super::store::{Badge, Record, Store};
use super::Found;
use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// DragonUI's folder: its optional `avrt.dll` beside the game updates it at every game start.
pub const SELF_UPDATING: &str = "SurvivorsDragonUi";

/// Where an addon stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Standing {
    Current,
    /// A newer release. `by_itself`: it goes in at the next Play; else it waits for the player.
    Update { to: Release, by_itself: bool },
    /// "Keep this version" is on.
    Kept { newer: Option<Release> },
    /// Its files changed since it was installed.
    Changed,
    /// Its folder is gone from AddOns.
    Gone,
    /// In AddOns, but not installed by the Satchel and not one of its author's releases.
    ByHand,
    /// Another updater keeps it current (DragonUI's `avrt.dll`).
    Elsewhere,
    /// A verified addon the built-in list no longer carries (its post left the verified channel):
    /// it stays as it is, and nothing updates it.
    Unlisted,
}

/// One row of the Installed list.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The record's id; `folder:<name>` for a folder the Satchel doesn't manage.
    pub id: String,
    pub name: String,
    /// `None`: installed by hand.
    pub badge: Option<Badge>,
    pub author: String,
    pub repo: Option<String>,
    pub version: String,
    /// When the installed release was published, when that's known.
    pub released: Option<DateTime<Utc>>,
    pub standing: Standing,
    /// The game loads it (as far as its on/off list goes).
    pub on: bool,
    /// Turned off: the game loads none of its folders, so its updates wait until it's on. One
    /// folder of several turned off leaves the addon in use.
    pub off: bool,
    /// On/off applies: the game would load the folder at all.
    pub switchable: bool,
    pub folders: Vec<String>,
    /// What's wrong, in plain words.
    pub problems: Vec<String>,
    /// The author's notes for the update, else for the installed release.
    pub notes: Option<String>,
    /// The version "Go back" returns to.
    pub back_to: Option<String>,
    pub pinned: bool,
    /// A copy installed by hand of a verified addon: the release that can replace it.
    pub replace_with: Option<String>,
    /// SurvivorsForge's verdict on the update, or on the replacement for a copy installed by hand.
    /// With neither, only a No stays (blocked, off, not answering, no longer listed): nothing
    /// waits for an OK.
    pub verdict: Verdict,
    /// The slug SurvivorsForge lists it under (its own for a SurvivorsForge install), with the
    /// switch on.
    pub forge_slug: Option<String>,
    /// Its icon from SurvivorsForge's registry, when it lists the addon and has one (`ForgeView::icon`).
    pub icon_url: Option<String>,
}

pub struct Inputs<'a> {
    pub catalog: &'a Catalog,
    pub store: &'a Store,
    pub found: &'a [Found],
    /// Each folder in AddOns (as `found` names it) → its fingerprint now.
    pub prints: &'a BTreeMap<String, String>,
    pub enabled: &'a EnableList,
    /// Releases found on GitHub that the catalog or the record doesn't know, by addon id.
    pub newer: &'a BTreeMap<String, Release>,
    /// Verified addons update by themselves before Play (Settings).
    pub auto: bool,
    /// DragonUI's own updater (`avrt.dll`) sits beside the game's exe.
    pub avrt: bool,
}

/// What SurvivorsForge says, as the worker holds it in memory (no I/O here): the switch, the time,
/// the cache's state, the registry and its join with our catalog, and each slug's versions as
/// cached under its entry as it stands now (`forge::Cache::versions`).
#[derive(Clone, Copy, Debug)]
pub struct ForgeView<'a> {
    /// The switch in Settings: off, SurvivorsForge is ignored completely.
    pub switch_on: bool,
    pub now: DateTime<Utc>,
    pub state: &'a forge::State,
    /// The registry as last received.
    pub registry: Option<&'a forge::Registry>,
    /// Our catalog's ids → their slugs (`forge::join`).
    pub join: &'a BTreeMap<String, String>,
    /// Slug → its versions.
    pub versions: &'a BTreeMap<String, Vec<forge::Version>>,
}

impl<'a> ForgeView<'a> {
    /// It answered within the hour.
    pub fn online(&self) -> bool {
        forge::online(self.state, self.now)
    }

    /// A slug's entry in the registry, with the switch on.
    pub fn entry(&self, slug: &str) -> Option<&'a forge::ForgeAddon> {
        self.registry.filter(|_| self.switch_on)?.get(slug)
    }

    /// The slug SurvivorsForge lists an addon under, with the switch on: a SurvivorsForge install's
    /// own; else the one joined to its catalog id; else the one listed with its repository, now or
    /// before, so an addon that left the registry keeps its slug.
    pub fn slug_for(&self, own_slug: Option<&str>, catalog_id: Option<&str>, repo: Option<&str>) -> Option<String> {
        if !self.switch_on {
            return None;
        }
        if let Some(slug) = own_slug.or_else(|| self.join.get(catalog_id?).map(String::as_str)) {
            return Some(slug.to_string());
        }
        let repo = repo.filter(|r| !r.trim().is_empty())?;
        let listed = self.registry.and_then(|r| r.addons.iter().find(|a| forge::same_repo(&a.repo, repo)));
        listed.map(|a| a.id.clone()).or_else(|| self.state.repos.iter().find(|(_, r)| forge::same_repo(r, repo)).map(|(slug, _)| slug.clone()))
    }

    /// Its word on version `tag` of the addon listed as `slug` (`forge::word_for`), from current
    /// answers only. `None`: before an install the player asked for, its versions are worth one
    /// request.
    pub fn word(&self, slug: &str, tag: &str) -> Option<(forge::Word, forge::Counts, Option<String>)> {
        forge::word_for(self.entry(slug), self.versions.get(slug).map(Vec::as_slice), tag)
    }

    /// A word that held version `tag` back in an answer no longer current (`State::restrictions`),
    /// with the switch on, when it was about the file with checksum `sha256` (or either checksum
    /// isn't known): a file replaced under the same tag is judged again.
    pub fn restriction(&self, slug: &str, tag: &str, sha256: Option<&str>) -> Option<(forge::Word, forge::Counts)> {
        let kept = self.state.restrictions.get(slug).filter(|_| self.switch_on)?.get(tag).filter(|kept| kept.covers(sha256))?;
        Some((forge::Word::parse(&kept.trust), kept.findings)).filter(|(word, _)| word.restricts())
    }

    /// The icon SurvivorsForge has for the addon it lists as `slug`, with the switch on: an address
    /// a picture may be fetched from (`forge::picture_url_ok`) only.
    pub fn icon(&self, slug: Option<&str>) -> Option<String> {
        self.entry(slug?)?.icon_url.clone().map(|url| url.trim().to_string()).filter(|url| forge::picture_url_ok(url))
    }

    /// The verdict on installing version `tag` (our checksum `sha256`) of an addon listed as `slug`
    /// (a SurvivorsForge install's own slug is in `origin`): the facts gathered here, the rule in
    /// `forge::decide`. The same for every path that installs. With no current word on the
    /// version, a word that held it back before still holds: old data can't let anything in.
    pub fn verdict(&self, origin: Origin, slug: Option<&str>, tag: &str, sha256: Option<&str>) -> Verdict {
        self.judge(origin, slug, tag, sha256, true)
    }

    /// The verdict a Browse card shows. A No that only a word kept from an older answer gives
    /// shows as Ask, with its reason: the card stays, and opening it asks SurvivorsForge again,
    /// while the install itself is refused until a current answer lifts the word. A No from current
    /// answers hides the card.
    pub fn offer_verdict(&self, origin: Origin, slug: Option<&str>, tag: &str, sha256: Option<&str>) -> Verdict {
        match self.verdict(origin, slug, tag, sha256) {
            Verdict::No(why) if !matches!(self.judge(origin, slug, tag, sha256, false), Verdict::No(_)) => Verdict::Ask(why),
            verdict => verdict,
        }
    }

    /// The verdict with, or without (`kept` false), the words kept from older answers.
    fn judge(&self, origin: Origin, slug: Option<&str>, tag: &str, sha256: Option<&str>, kept: bool) -> Verdict {
        let slug = match origin {
            Origin::ForgeOnly { slug } => Some(slug),
            _ => slug,
        };
        let word = slug.and_then(|s| self.word(s, tag));
        let held = slug.filter(|_| kept && word.is_none()).and_then(|s| self.restriction(s, tag, sha256));
        let facts = forge::Facts {
            switch_on: self.switch_on,
            online: self.online(),
            listed: slug.and_then(|s| self.entry(s)),
            dropped: slug.is_some_and(|s| forge::dropped(self.state, self.registry, s)),
            blocked: slug.and_then(|s| self.state.blocked.get(s)).map(String::as_str),
            word: word.as_ref().map(|(w, c, _)| (*w, *c)).or(held),
            forge_sha: word.as_ref().and_then(|(_, _, sha)| sha.as_deref()),
        };
        forge::decide(origin, sha256, &facts)
    }

    /// A SurvivorsForge install's releases, newest first by when this PC first saw each as the
    /// registry's latest: the latest now and, when this PC saw the installed version as latest
    /// before (it went back to it, or followed another PC to it), that one too. A later latest is
    /// then an update, and an earlier one a step back. A latest not dated yet (a lost cache) is no
    /// newer than the install, until the next answer dates it.
    fn releases_of(&self, record: &Record, slug: &str) -> Vec<Release> {
        let Some(entry) = self.entry(slug) else { return Vec::new() };
        let seen = self.state.seen.get(slug);
        let tag = entry.latest.as_ref().map_or("", |l| l.tag.as_str());
        let latest_seen = seen.and_then(|tags| tags.get(tag)).copied().unwrap_or(record.installed);
        let Some(latest) = forge::latest_release(entry, latest_seen) else { return Vec::new() };
        let installed_seen = record.tag.as_deref().filter(|t| *t != latest.tag).and_then(|t| Some((t, *seen?.get(t)?)));
        let Some((installed_tag, first_seen)) = installed_seen else { return vec![latest] };
        // The installed version as a release of its own, only to place the latest: never downloaded.
        let installed = Release {
            tag: installed_tag.to_string(),
            version: record.version.clone(),
            published: first_seen,
            prerelease: false,
            file: record.file.clone().unwrap_or_default(),
            url: String::new(),
            sha256: record.sha256.clone().unwrap_or_default(),
            size: 0,
            interface: 0,
            folders: BTreeMap::new(),
            notes: None,
            downloads: None,
        };
        if latest.published > installed.published { vec![latest, installed] } else { vec![installed, latest] }
    }
}

/// The checksum to judge a record's version `tag` by: for an addon of our list whose release with
/// that tag the catalog holds, that release's (a copy imported from a re-zipped file records its own
/// zip's checksum, though its files are exactly the release's); else `sha256`, the one recorded.
pub fn judged_sha<'a>(catalog: &'a Catalog, record: &Record, tag: Option<&str>, sha256: Option<&'a str>) -> Option<&'a str> {
    let release = (record.badge == Badge::Verified && record.forge.is_none())
        .then(|| catalog.get(&record.id))
        .flatten()
        .and_then(|entry| entry.releases().find(|r| Some(r.tag.as_str()) == tag))
        .filter(|r| !r.sha256.is_empty());
    release.map_or(sha256, |r| Some(r.sha256.as_str()))
}

/// SurvivorsForge's verdict on the version already in: with nothing to install nothing waits for
/// an OK, and what speaks only of the registry's latest (no checksum, a download elsewhere) doesn't
/// count for a file already in.
fn standing_verdict(verdict: Verdict) -> Verdict {
    match verdict {
        Verdict::Ask(_) | Verdict::No(forge::Why::NotHashed | forge::Why::NotOnGitHub) => Verdict::Go,
        other => other,
    }
}

/// An addon's known releases, newest first: one found on GitHub, then the catalog's. (A
/// SurvivorsForge install's come from its registry, in `lines`.)
pub fn releases(record: &Record, catalog: &Catalog, newer: &BTreeMap<String, Release>) -> Vec<Release> {
    let mut all: Vec<Release> = newer.get(&record.id).cloned().into_iter().collect();
    if record.badge == Badge::Verified
        && let Some(entry) = catalog.get(&record.id)
    {
        all.extend(entry.releases().cloned());
    }
    all
}

fn print_of<'a>(prints: &'a BTreeMap<String, String>, folder: &str) -> Option<&'a String> {
    prints.iter().find(|(name, _)| name.eq_ignore_ascii_case(folder)).map(|(_, print)| print)
}

fn on_disk(record: &Record, prints: &BTreeMap<String, String>) -> BTreeMap<String, Option<String>> {
    record.folders.keys().map(|f| (f.clone(), print_of(prints, f).cloned())).collect()
}

/// Records whose files are now exactly another known release (another tool updated them) follow
/// that release. Returns a line per record that moved, for the activity list.
pub fn follow(store: &mut Store, catalog: &Catalog, newer: &BTreeMap<String, Release>, prints: &BTreeMap<String, String>) -> Vec<String> {
    let mut moved = Vec::new();
    for record in store.addons.iter_mut() {
        let releases = releases(record, catalog, newer);
        if let State::Moved { to } = catalog::state(record, &releases, &on_disk(record, prints)) {
            record.folders = record.folders.keys().filter_map(|f| Some((f.clone(), print_of(prints, f)?.clone()))).collect();
            record.tag = Some(to.tag.clone());
            record.file = Some(to.file.clone());
            record.version = to.version.clone();
            record.sha256 = Some(to.sha256.clone());
            record.released = catalog.published(&record.id, &to.tag);
            record.url = (!to.url.is_empty()).then(|| to.url.clone());
            moved.push(format!("{} is now {}: another tool updated it", record.name, to.shown_version()));
        }
    }
    moved
}

/// "|cff33ff99Lucky|r Odds" → "Lucky Odds": a `.toc` title without the game's colour codes.
pub fn plain(title: &str) -> String {
    let mut out = String::new();
    let mut chars = title.chars();
    while let Some(c) = chars.next() {
        if c == '|' {
            match chars.clone().next() {
                Some('c' | 'C') => {
                    chars.nth(8);
                    continue;
                }
                Some('r' | 'R') => {
                    chars.next();
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// "AZS Spell Meter" → "SM", "SurvivorsDragonUi" → "DU", "Azerloot" → "AZ": the last two words'
/// first letters (camel case splits words), for the row's icon.
pub fn initials(name: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for part in name.split(|c: char| c.is_whitespace() || c == '-' || c == '_') {
        let mut word = String::new();
        let mut last_lower = false;
        for c in part.chars().filter(|c| c.is_alphanumeric()) {
            if c.is_uppercase() && last_lower && !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            last_lower = c.is_lowercase();
            word.push(c);
        }
        if !word.is_empty() {
            words.push(word);
        }
    }
    let letters: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [.., a, b] => [a.chars().next(), b.chars().next()].into_iter().flatten().collect(),
    };
    letters.to_uppercase()
}

/// An author's release notes as plain text, at most about `most` characters (cut at a line):
/// Markdown's headings, emphasis, code marks, code fences and link targets go, list items get a
/// bullet.
pub fn plain_notes(notes: &str, most: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in notes.lines() {
        let line = line.trim_end();
        // A fence line goes whole: "```text" would otherwise leave its language behind.
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let mut text = line.trim_start().trim_start_matches('#').trim_start().to_string();
        if let Some(item) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
            text = format!("{}• {item}", " ".repeat(indent.min(4)));
        }
        // [words](link) → words
        while let Some(open) = text.find('[') {
            let Some(close) = text[open..].find("](").map(|c| open + c) else { break };
            let Some(end) = text[close..].find(')').map(|e| close + e) else { break };
            text = format!("{}{}{}", &text[..open], &text[open + 1..close], &text[end + 1..]);
        }
        let text = text.replace("**", "").replace("__", "").replace('`', "");
        if text.trim().is_empty() && lines.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        lines.push(text);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let text = lines.join("\n");
    if text.chars().count() <= most {
        return text;
    }
    let cut: String = text.chars().take(most).collect();
    let cut = cut.rfind('\n').filter(|&at| at > most / 2).map_or(cut.as_str(), |at| &cut[..at]);
    format!("{} …", cut.trim_end())
}

/// "released 3 days ago": a release's age in calendar days, beside its version.
pub fn released_ago(days: i64) -> String {
    match days {
        ..=0 => "released today".into(),
        1 => "released yesterday".into(),
        2..60 => format!("released {days} days ago"),
        60..365 => format!("released {} months ago", days / 30),
        365..730 => "released a year ago".into(),
        _ => format!("released {} years ago", days / 365),
    }
}

fn owner(repo: &str) -> String {
    repo.split('/').next().unwrap_or_default().to_string()
}

/// The Installed list: the Satchel's own addons, then any other folder in AddOns, by name.
pub fn lines(inputs: &Inputs, forge_view: &ForgeView) -> Vec<Line> {
    let present = |folder: &str| inputs.found.iter().find(|f| f.folder.eq_ignore_ascii_case(folder));
    let loads = |folder: &str| present(folder).is_some_and(|f| f.toc.is_some());
    let is_on = |folder: &str| {
        let default_off = present(folder).and_then(|f| f.toc.as_ref()).is_some_and(|t| t.default_disabled);
        loads(folder) && inputs.enabled.is_on(folder, default_off)
    };
    // What each folder needs and doesn't find, and which addons it doesn't mix with.
    let folder_problems = |folders: &[String], entry: Option<&catalog::Entry>| {
        let mut problems = Vec::new();
        for folder in folders {
            let Some(found) = present(folder) else { continue };
            problems.extend(found.problem.clone());
            for need in found.toc.iter().flat_map(|t| &t.required) {
                if !loads(need) {
                    problems.push(format!("needs {need}, which isn't in AddOns"));
                } else if !is_on(need) {
                    problems.push(format!("needs {need}, which is turned off"));
                }
            }
        }
        let mine = |f: &String| folders.iter().any(|g| g.eq_ignore_ascii_case(f));
        let mut clashes: Vec<String> =
            entry.into_iter().flat_map(|e| e.doesnt_mix_with.iter()).filter(|f| !mine(f) && is_on(f)).cloned().collect();
        for other in &inputs.catalog.addons {
            let theirs_on = other.folders.first().is_some_and(|f| !mine(f) && is_on(f));
            if theirs_on && other.doesnt_mix_with.iter().any(mine) && !clashes.iter().any(|c| other.folders.contains(c)) {
                clashes.push(other.folders[0].clone());
            }
        }
        if !clashes.is_empty() && folders.iter().any(|f| is_on(f)) {
            problems.push(format!("doesn't mix with {} (its author says so); turn one off", clashes.join(", ")));
        }
        problems
    };

    let mut lines = Vec::new();
    for record in &inputs.store.addons {
        let entry = (record.badge == Badge::Verified).then(|| inputs.catalog.get(&record.id)).flatten();
        let slug = forge_view.slug_for(
            record.forge.as_deref(),
            (record.badge == Badge::Verified).then_some(record.id.as_str()),
            entry.map(|e| e.repo.as_str()).or(record.repo.as_deref()),
        );
        let origin = forge::origin_of(record.badge, record.forge.as_deref());
        // A SurvivorsForge install's releases are its registry's, and it has no look on GitHub.
        let releases = match &record.forge {
            Some(own) => forge_view.releases_of(record, own),
            None => releases(record, inputs.catalog, inputs.newer),
        };
        let state = catalog::state(record, &releases, &on_disk(record, inputs.prints));
        let verdict = match &state {
            State::Update { to, .. } | State::Kept { newer: Some(to) } => forge_view.verdict(origin, slug.as_deref(), &to.tag, Some(to.sha256.as_str())),
            _ => {
                let sha = judged_sha(inputs.catalog, record, record.tag.as_deref(), record.sha256.as_deref());
                standing_verdict(forge_view.verdict(origin, slug.as_deref(), record.tag.as_deref().unwrap_or_default(), sha))
            }
        };
        let elsewhere = inputs.avrt && record.folders.keys().any(|f| f.eq_ignore_ascii_case(SELF_UPDATING));
        let folders: Vec<String> = record.folders.keys().cloned().collect();
        let on = folders.iter().all(|f| is_on(f));
        let switchable = folders.iter().all(|f| loads(f));
        let off = switchable && !folders.iter().any(|f| is_on(f));
        let standing = match state {
            State::Current | State::Moved { .. } => Standing::Current,
            State::Update { .. } | State::Changed if elsewhere => Standing::Elsewhere,
            State::Update { to, by_itself } => {
                // A release whose checksum GitHub didn't publish asks first, verified or not; a
                // turned-off addon waits until it's turned on; SurvivorsForge's verdict holds.
                let by_itself = by_itself && inputs.auto && !to.sha256.is_empty() && !off && verdict.is_go();
                Standing::Update { to, by_itself }
            }
            State::Kept { newer } => Standing::Kept { newer },
            State::Changed => Standing::Changed,
            State::Gone => Standing::Gone,
        };
        // A verified addon the list dropped gets no catalog releases and no look on GitHub: the row
        // says so instead of passing for verified and up to date. A lost folder says more.
        let standing =
            if record.badge == Badge::Verified && entry.is_none() && standing != Standing::Gone { Standing::Unlisted } else { standing };
        let mut problems = folder_problems(&folders, entry);
        match &standing {
            Standing::Changed => problems.push("its files changed since the Satchel installed it, so it isn't updated by itself".into()),
            Standing::Gone => problems.push("its folder is gone from AddOns".into()),
            Standing::Unlisted => problems.push("it left the Discord Verified list, so the Satchel doesn't update it".into()),
            _ => {}
        }
        let notes = match &standing {
            Standing::Update { to, .. } | Standing::Kept { newer: Some(to) } => to.notes.clone(),
            _ => releases.iter().find(|r| record.tag.as_deref() == Some(r.tag.as_str())).and_then(|r| r.notes.clone()),
        };
        lines.push(Line {
            id: record.id.clone(),
            name: record.name.clone(),
            badge: Some(record.badge),
            author: entry.map(|e| e.author.clone()).or_else(|| record.repo.as_deref().map(owner)).unwrap_or_default(),
            repo: record.repo.clone(),
            version: record.shown_version(),
            // Installed before the Satchel kept the date: the catalog's release has it.
            released: record.released.or_else(|| entry.and_then(|e| e.releases().find(|r| record.tag.as_deref() == Some(r.tag.as_str()))).map(|r| r.published)),
            on,
            off,
            switchable,
            folders,
            standing,
            problems,
            notes,
            back_to: record.earlier.first().map(|k| k.version.clone().or_else(|| k.tag.clone()).unwrap_or_else(|| "the earlier version".into())),
            pinned: record.pinned,
            replace_with: None,
            verdict,
            icon_url: forge_view.icon(slug.as_deref()),
            forge_slug: slug,
        });
    }

    for found in inputs.found {
        if inputs.store.owner_of(&found.folder).is_some() {
            continue;
        }
        let entry = inputs.catalog.for_folder(&found.folder);
        // A verified addon with several folders shows once, under its main folder.
        if let Some(e) = entry
            && let Some(main) = e.folders.first()
            && !main.eq_ignore_ascii_case(&found.folder)
            && present(main).is_some()
        {
            continue;
        }
        let folders: Vec<String> = match entry {
            Some(e) => e.folders.iter().filter_map(|f| present(f).map(|x| x.folder.clone())).collect(),
            None => vec![found.folder.clone()],
        };
        let toc = found.toc.as_ref();
        let mut problems = folder_problems(&folders, entry);
        if entry.is_some() {
            problems.push("installed by hand, and its files aren't one of its author's releases, so it isn't updated".into());
        }
        let slug = entry.and_then(|e| forge_view.slug_for(None, Some(e.id.as_str()), Some(e.repo.as_str())));
        // Replacing the copy installs the catalog's latest: the verdict on that.
        let verdict = match entry {
            Some(e) => forge_view.verdict(Origin::OurList, slug.as_deref(), &e.latest.tag, Some(e.latest.sha256.as_str())),
            None => Verdict::Go,
        };
        lines.push(Line {
            id: format!("folder:{}", found.folder),
            name: entry
                .map(|e| e.name.clone())
                .or_else(|| toc.and_then(|t| t.title.as_deref()).map(plain).filter(|t| !t.is_empty()))
                .unwrap_or_else(|| found.folder.clone()),
            badge: None,
            author: entry.map(|e| e.author.clone()).or_else(|| toc.and_then(|t| t.author.clone())).unwrap_or_default(),
            repo: entry.map(|e| e.repo.clone()),
            version: toc.and_then(|t| t.version.clone()).unwrap_or_else(|| "–".into()),
            released: None,
            standing: Standing::ByHand,
            on: folders.iter().all(|f| is_on(f)),
            off: folders.iter().all(|f| loads(f)) && !folders.iter().any(|f| is_on(f)),
            switchable: folders.iter().all(|f| loads(f)),
            folders,
            problems,
            notes: None,
            back_to: None,
            pinned: false,
            replace_with: entry.map(|e| e.latest.shown_version().to_string()),
            verdict,
            icon_url: forge_view.icon(slug.as_deref()),
            forge_slug: slug,
        });
    }
    lines.sort_by_key(|l| l.name.to_lowercase());
    lines
}

/// The updates that go in by themselves at the next Play.
pub fn at_play(lines: &[Line]) -> Vec<(&Line, &Release)> {
    lines
        .iter()
        .filter_map(|l| match &l.standing {
            Standing::Update { to, by_itself: true } => Some((l, to)),
            _ => None,
        })
        .collect()
}

/// An addon on offer in Browse: from our catalog, or from SurvivorsForge alone (`forge:<slug>`).
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub id: String,
    pub name: String,
    pub author: String,
    pub repo: String,
    pub summary: String,
    pub group: String,
    pub version: String,
    pub installed: bool,
    /// Shown before installing: an addon it doesn't mix with is already in AddOns.
    pub warning: Option<String>,
    pub notes: Option<String>,
    /// Our group for an addon of our list and SurvivorsForge's category for one it lists: both for
    /// an addon on both, while SurvivorsForge answers.
    pub tags: Vec<String>,
    /// The slug SurvivorsForge lists it under, with the switch on: for its details and versions.
    pub forge_slug: Option<String>,
    /// 0 our list only, 1 both (while SurvivorsForge answers), 2 SurvivorsForge only.
    pub origin: i32,
    /// When `version` was published: a real date only, never when it was first seen. For an addon
    /// only on SurvivorsForge, its registry's `latest.published_at`.
    pub date: Option<DateTime<Utc>>,
    /// GitHub's downloads, Browse's popularity: our catalog's count for our list, the registry's
    /// for an addon only on SurvivorsForge. `None` isn't known (an addon published by tag): it
    /// sorts last and is never shown, nor taken for none.
    pub downloads: Option<u64>,
    /// SurvivorsForge's verdict on installing `version`: Go, or Ask with why. A No is never offered.
    pub verdict: Verdict,
    /// Its icon from SurvivorsForge's registry, while it lists the addon (`ForgeView::icon`).
    pub icon_url: Option<String>,
}

/// Tags as Browse shows them: each once (without case), none empty.
fn tags(names: &[&str]) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for name in names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
        if !tags.iter().any(|t| t.eq_ignore_ascii_case(name)) {
            tags.push(name.to_string());
        }
    }
    tags
}

/// Our catalog's addons, in its order (by group), then SurvivorsForge's own while it answers. An
/// addon SurvivorsForge's current answers say No to (blocked, changed after its scan, another file
/// than the one it scanned, not installable) isn't offered; a No only an older answer gives asks
/// (`ForgeView::offer_verdict`).
pub fn offers(catalog: &Catalog, store: &Store, found: &[Found], forge_view: &ForgeView) -> Vec<Offer> {
    let present = |folder: &str| found.iter().any(|f| f.folder.eq_ignore_ascii_case(folder));
    let online = forge_view.online();
    let mut offers: Vec<Offer> = catalog
        .addons
        .iter()
        .map(|entry| {
            let installed = store.get(&entry.id).is_some() || entry.folders.iter().all(|f| present(f));
            let mut clashes: Vec<String> = entry.doesnt_mix_with.iter().filter(|f| present(f)).cloned().collect();
            for other in &catalog.addons {
                if other.id != entry.id
                    && other.folders.first().is_some_and(|f| present(f))
                    && entry.folders.iter().any(|f| other.doesnt_mix_with.iter().any(|d| d.eq_ignore_ascii_case(f)))
                    && !clashes.contains(&other.folders[0])
                {
                    clashes.push(other.folders[0].clone());
                }
            }
            let warning = (!clashes.is_empty() && !installed).then(|| format!("doesn't mix with {}, which you have", clashes.join(", ")));
            let slug = forge_view.slug_for(None, Some(entry.id.as_str()), Some(entry.repo.as_str()));
            // Listed now, and answering: its category, and the "both" list.
            let listed = slug.as_deref().and_then(|s| forge_view.entry(s)).filter(|_| online);
            Offer {
                id: entry.id.clone(),
                name: entry.name.clone(),
                author: entry.author.clone(),
                repo: entry.repo.clone(),
                summary: entry.summary.clone(),
                group: entry.group.clone(),
                version: entry.latest.shown_version().to_string(),
                installed,
                warning,
                notes: entry.latest.notes.clone(),
                tags: tags(&[entry.group.as_str(), listed.map_or("", |l| l.category.as_str())]),
                origin: if listed.is_some() { 1 } else { 0 },
                date: Some(entry.latest.published),
                downloads: entry.downloads,
                verdict: forge_view.offer_verdict(Origin::OurList, slug.as_deref(), &entry.latest.tag, Some(entry.latest.sha256.as_str())),
                icon_url: forge_view.icon(slug.as_deref()),
                forge_slug: slug,
            }
        })
        .collect();
    // SurvivorsForge's own addons, while it answers: those our catalog doesn't carry.
    if forge_view.switch_on
        && online
        && let Some(registry) = forge_view.registry
    {
        let joined: BTreeSet<&str> = forge_view.join.values().map(String::as_str).collect();
        for listed in registry.addons.iter().filter(|a| !joined.contains(a.id.as_str())) {
            let Some(latest) = &listed.latest else { continue };
            let versions = forge_view.versions.get(&listed.id);
            let installed = store.addons.iter().any(|r| {
                r.forge.as_deref() == Some(listed.id.as_str()) || r.repo.as_deref().is_some_and(|repo| forge::same_repo(repo, &listed.repo))
            }) || (!listed.folders.is_empty() && listed.folders.iter().all(|f| present(f)));
            offers.push(Offer {
                id: forge::id_for(&listed.id),
                name: listed.name.clone(),
                author: listed.author.clone(),
                repo: listed.repo.clone(),
                summary: listed.description.clone(),
                group: listed.category.clone(),
                version: latest.tag.clone(),
                installed,
                warning: None,
                notes: None,
                tags: tags(&[listed.category.as_str()]),
                forge_slug: Some(listed.id.clone()),
                origin: 2,
                // Its date and count are the registry's (no versions fetch needed); a registry
                // without the date leaves the one in its versions, once they are cached.
                date: latest.published_at.or_else(|| versions.and_then(|v| v.iter().find(|x| x.tag == latest.tag)).and_then(|x| x.published_at)),
                downloads: listed.downloads,
                verdict: forge_view.offer_verdict(Origin::ForgeOnly { slug: listed.id.as_str() }, None, &latest.tag, latest.sha256.as_deref()),
                icon_url: forge_view.icon(Some(listed.id.as_str())),
            });
        }
    }
    offers.retain(|o| !matches!(o.verdict, Verdict::No(_)));
    offers
}

/// Browse's three sorts, in all three of its views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    /// A to Z.
    Name,
    /// Last updated: the newest publish date first.
    Updated,
    /// Popularity: the most downloads first.
    Popularity,
}

impl Sort {
    /// The sort as the window numbers it (`browse-sort`): 0 name, 1 last updated, 2 popularity.
    pub fn from_index(index: i32) -> Sort {
        match index {
            1 => Sort::Updated,
            2 => Sort::Popularity,
            _ => Sort::Name,
        }
    }
}

/// What Browse sorts an offer by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortKeys<'a> {
    pub name: &'a str,
    pub id: &'a str,
    /// The publish time of the version shown, in Unix seconds; `None` when no real date is known.
    pub date: Option<i64>,
    /// Its downloads; `None` when they aren't known.
    pub downloads: Option<u64>,
}

/// A name as Browse sorts it: case aside, and a leading "[KMA] " sorts under K.
pub fn sort_name(name: &str) -> String {
    name.trim_start_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

/// Browse's order for `sort`, the other way round when the table's column header says so
/// (`reverse`); ties by name, A to Z, then by id. An unknown date or count goes last either way,
/// by name: it is never taken for the oldest, or for no downloads.
pub fn browse_order(a: &SortKeys, b: &SortKeys, sort: Sort, reverse: bool) -> Ordering {
    let by_name = || sort_name(a.name).cmp(&sort_name(b.name)).then_with(|| a.id.cmp(b.id));
    match sort {
        Sort::Name if reverse => by_name().reverse(),
        Sort::Name => by_name(),
        Sort::Updated => known_first(a.date, b.date, reverse).then_with(by_name),
        Sort::Popularity => known_first(a.downloads, b.downloads, reverse).then_with(by_name),
    }
}

/// The larger first (the smaller, `reverse`), and the unknown after both.
fn known_first<T: Ord>(a: Option<T>, b: Option<T>, reverse: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) if reverse => a.cmp(&b),
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// One row of Versions…: one version of an addon, with all that is known of it here.
#[derive(Clone, Debug, PartialEq)]
pub struct VersionRow {
    pub tag: String,
    /// As it is shown: the version inside, else the tag.
    pub version: String,
    /// The version in AddOns now.
    pub installed: bool,
    /// In the backups, for Go back: its backup's name, and when it was kept.
    pub kept: Option<(String, DateTime<Utc>)>,
    /// When its author published it: a real date only.
    pub published: Option<DateTime<Utc>>,
    /// GitHub's count for its file: our catalog's, else SurvivorsForge's.
    pub downloads: Option<u64>,
    /// Its file's size in bytes, when known.
    pub size: Option<u64>,
    /// SurvivorsForge's word on it and what its scan found, from its versions or its registry.
    pub word: Option<(forge::Word, forge::Counts)>,
    /// The game versions it says it works with, when given: from, to.
    pub game: (Option<String>, Option<String>),
    pub prerelease: bool,
    /// What its one button does.
    pub action: VersionAction,
}

/// The one thing a row of Versions… does.
#[derive(Clone, Debug, PartialEq)]
pub enum VersionAction {
    /// Nothing: it is the one installed, or there is neither a backup nor a download of it.
    None,
    /// Go back to this: swapped in from its backup, as Go back does (`Kept::backup`).
    Back { backup: String },
    /// Install this version: downloaded and installed through the install path and its checks,
    /// from our catalog's release or from SurvivorsForge's list of versions.
    Install { release: Release },
}

/// What Versions… is made from: whatever is known of the addon here.
#[derive(Clone, Copy, Debug, Default)]
pub struct VersionsOf<'a> {
    /// Its record, when it is installed: the version in, and the earlier ones kept.
    pub record: Option<&'a Record>,
    /// Its entry in our catalog, for an addon of our list: its releases.
    pub catalog: Option<&'a catalog::Entry>,
    /// Its entry in SurvivorsForge's registry, with the switch on, and its versions there as
    /// cached under that entry.
    pub listed: Option<&'a forge::ForgeAddon>,
    pub versions: Option<&'a [forge::Version]>,
}

/// The row for `tag`, made when it isn't there yet.
fn row_for<'r>(rows: &'r mut Vec<VersionRow>, tag: &str, version: impl FnOnce() -> String) -> &'r mut VersionRow {
    match rows.iter().position(|r| r.tag == tag) {
        Some(at) => &mut rows[at],
        None => {
            rows.push(VersionRow {
                tag: tag.to_string(),
                version: version(),
                installed: false,
                kept: None,
                published: None,
                downloads: None,
                size: None,
                word: None,
                game: (None, None),
                prerelease: false,
                action: VersionAction::None,
            });
            rows.last_mut().expect("just pushed")
        }
    }
}

/// Versions…: every version of an addon known here, merged by tag into one row each, newest first
/// by publish date (rows without one follow, in the order they came): the installed one, the
/// earlier ones kept in the backups, our catalog's releases and SurvivorsForge's versions (its
/// registry's latest too, before its versions are fetched). One action each: none for the installed
/// one; Go back for a kept one, which needs no download; Install for a release or a SurvivorsForge
/// version that can be downloaded (`forge::version_release`'s checks). Whether SurvivorsForge lets
/// it in is the verdict's, asked by the caller as for every install.
pub fn versions(of: &VersionsOf) -> Vec<VersionRow> {
    let mut rows: Vec<VersionRow> = Vec::new();
    if let Some(record) = of.record {
        let row = row_for(&mut rows, &record.tag.clone().unwrap_or_else(|| record.shown_version()), || record.shown_version());
        row.installed = true;
        row.published = record.released;
        for kept in &record.earlier {
            let shown = kept.version.clone().or_else(|| kept.tag.clone()).unwrap_or_else(|| "an earlier version".into());
            let row = row_for(&mut rows, &kept.tag.clone().unwrap_or_else(|| shown.clone()), || shown.clone());
            // A backup of the very version in AddOns (an edited copy put right) isn't a version to go back to.
            if !row.installed && row.kept.is_none() {
                row.kept = Some((kept.backup.clone(), kept.kept));
                row.published = row.published.or(kept.released);
                row.action = VersionAction::Back { backup: kept.backup.clone() };
            }
        }
    }
    for release in of.catalog.into_iter().flat_map(catalog::Entry::releases) {
        let row = row_for(&mut rows, &release.tag, || release.shown_version().to_string());
        row.published = row.published.or(Some(release.published));
        row.downloads = row.downloads.or(release.downloads);
        row.size = row.size.or((release.size > 0).then_some(release.size));
        row.prerelease |= release.prerelease;
        if row.action == VersionAction::None && !release.url.is_empty() {
            row.action = VersionAction::Install { release: release.clone() };
        }
    }
    if let Some(listed) = of.listed {
        for version in of.versions.unwrap_or_default() {
            let row = row_for(&mut rows, &version.tag, || tag_shown(&version.tag));
            row.published = row.published.or(version.published_at);
            row.downloads = row.downloads.or(version.downloads);
            row.size = row.size.or(version.size);
            row.word = Some((forge::Word::parse(version.trust.as_deref().unwrap_or("unscanned")), version.findings));
            row.game = (version.game_min.clone(), version.game_max.clone());
            row.prerelease |= version.prerelease;
            let release = forge::version_release(listed, version);
            if row.action == VersionAction::None && !release.url.is_empty() {
                row.action = VersionAction::Install { release };
            }
        }
        if let Some(latest) = listed.latest.as_ref().filter(|l| !l.tag.is_empty()) {
            let row = row_for(&mut rows, &latest.tag, || tag_shown(&latest.tag));
            row.published = row.published.or(latest.published_at);
            row.size = row.size.or(latest.size);
            row.word = row.word.or_else(|| latest.trust.as_deref().or(listed.trust.as_deref()).map(|t| (forge::Word::parse(t), latest.findings)));
            if row.game == (None, None) {
                row.game = (latest.game_min.clone(), latest.game_max.clone());
            }
            if row.action == VersionAction::None
                && let Some(release) = forge::latest_release(listed, latest.published_at.unwrap_or(DateTime::<Utc>::MIN_UTC)).filter(|r| !r.url.is_empty())
            {
                row.action = VersionAction::Install { release };
            }
        }
    }
    // The one installed is never installed again, nor gone back to.
    for row in rows.iter_mut().filter(|r| r.installed) {
        row.action = VersionAction::None;
    }
    rows.sort_by(|a, b| match (a.published, b.published) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    rows
}

/// A tag as a version is shown: "v1.2" → "1.2", else the tag.
fn tag_shown(tag: &str) -> String {
    super::github::tag_version(tag).unwrap_or_else(|| tag.to_string())
}

/// An installed addon to look up on GitHub: its repository, how to pick its file, and the tags
/// already known (a latest release with one of them is nothing new).
#[derive(Clone, Debug, PartialEq)]
pub struct Watch {
    pub id: String,
    pub repo: String,
    pub pick: Pick,
    pub known: Vec<String>,
}

/// What to look up: verified addons published with release files, and addons added by link.
/// Addons published by tag, imported files and DragonUI under its own updater are left to the
/// catalog; SurvivorsForge's own, to its registry.
pub fn watches(store: &Store, catalog: &Catalog, avrt: bool) -> Vec<Watch> {
    store
        .addons
        .iter()
        .filter(|r| !(avrt && r.folders.keys().any(|f| f.eq_ignore_ascii_case(SELF_UPDATING))))
        .filter(|r| r.forge.is_none())
        .filter_map(|r| match r.badge {
            Badge::Verified => {
                let entry = catalog.get(&r.id)?;
                let Asset::File { pattern } = &entry.asset else { return None };
                let known = entry.releases().map(|x| x.tag.clone()).chain(r.tag.clone()).collect();
                Some(Watch { id: r.id.clone(), repo: entry.repo.clone(), pick: Pick::Pattern(pattern.clone()), known })
            }
            Badge::Added => {
                let known = r.tag.iter().cloned().chain(r.earlier.iter().filter_map(|k| k.tag.clone())).collect();
                Some(Watch { id: r.id.clone(), repo: r.repo.clone()?, pick: Pick::Like(r.file.clone()), known })
            }
            Badge::File => None,
        })
        .collect()
}

/// A warning for an addon added by link that looks like a verified one from someone else: the
/// same name, or a folder of the same name.
pub fn look_alike(catalog: &Catalog, repo: &str, name: &str, folders: &[String]) -> Option<String> {
    catalog
        .addons
        .iter()
        .find(|e| {
            !e.repo.eq_ignore_ascii_case(repo)
                && (e.name.eq_ignore_ascii_case(name) || e.folders.iter().any(|f| folders.iter().any(|g| g.eq_ignore_ascii_case(f))))
        })
        .map(|e| {
            format!(
                "{} is a Discord Verified addon by {}. This one is by {}: a copy or a look-alike, not the Discord Verified one.",
                e.name,
                e.author,
                owner(repo)
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::catalog::Entry;
    use crate::addons::store::Kept;
    use crate::addons::toc::Toc;
    use chrono::{TimeDelta, TimeZone, Utc};

    fn release(tag: &str, print: &str) -> Release {
        Release {
            tag: tag.into(),
            version: Some(tag.trim_start_matches('v').into()),
            published: Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap(),
            prerelease: false,
            file: format!("A-{tag}.zip"),
            url: format!("https://github.com/o/a/releases/download/{tag}/A-{tag}.zip"),
            sha256: format!("sha-{tag}"),
            size: 1,
            interface: 11200,
            folders: [("A".to_string(), print.to_string())].into(),
            notes: Some(format!("notes {tag}")),
            downloads: None,
        }
    }

    fn entry(id: &str, folder: &str, mix: &[&str]) -> Entry {
        Entry {
            id: id.into(),
            name: folder.into(),
            repo: format!("o/{folder}"),
            author: "o".into(),
            summary: "s".into(),
            group: "g".into(),
            folders: vec![folder.into()],
            asset: Asset::File { pattern: format!("{folder}-*.zip") },
            latest: release("v2", "p2"),
            earlier: vec![release("v1", "p1")],
            doesnt_mix_with: mix.iter().map(|m| m.to_string()).collect(),
            notes: Vec::new(),
            downloads: None,
        }
    }

    fn record(id: &str, folder: &str, tag: &str, print: &str) -> Record {
        Record {
            id: id.into(),
            name: folder.into(),
            badge: Badge::Verified,
            repo: Some(format!("o/{folder}")),
            tag: Some(tag.into()),
            file: Some(format!("A-{tag}.zip")),
            version: Some(tag.trim_start_matches('v').into()),
            sha256: Some(format!("sha-{tag}")),
            folders: [(folder.to_string(), print.to_string())].into(),
            installed: Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap(),
            released: None,
            pinned: false,
            ask: false,
            url: None,
            local_only: false,
            settings_per_pc: false,
            earlier: Vec::new(),
            forge: None,
        }
    }

    fn found(folder: &str, toc: Option<&str>) -> Found {
        Found { folder: folder.into(), toc: toc.map(|t| Toc::parse(t.as_bytes())), problem: None }
    }

    struct World {
        catalog: Catalog,
        store: Store,
        found: Vec<Found>,
        prints: BTreeMap<String, String>,
        enabled: EnableList,
        newer: BTreeMap<String, Release>,
        avrt: bool,
        /// SurvivorsForge: the switch, what this PC remembers, the registry, cached versions.
        forge_on: bool,
        forge_state: forge::State,
        registry: Option<forge::Registry>,
        versions: BTreeMap<String, Vec<forge::Version>>,
        now: DateTime<Utc>,
    }

    impl World {
        fn lines(&self) -> Vec<Line> {
            let join = self.join();
            let inputs = Inputs {
                catalog: &self.catalog,
                store: &self.store,
                found: &self.found,
                prints: &self.prints,
                enabled: &self.enabled,
                newer: &self.newer,
                auto: true,
                avrt: self.avrt,
            };
            lines(&inputs, &self.view(&join))
        }

        fn offers(&self, store: &Store, found: &[Found]) -> Vec<Offer> {
            let join = self.join();
            offers(&self.catalog, store, found, &self.view(&join))
        }

        fn join(&self) -> BTreeMap<String, String> {
            self.registry.as_ref().map(|r| forge::join(&self.catalog, r)).unwrap_or_default()
        }

        fn view<'a>(&'a self, join: &'a BTreeMap<String, String>) -> ForgeView<'a> {
            ForgeView {
                switch_on: self.forge_on,
                now: self.now,
                state: &self.forge_state,
                registry: self.registry.as_ref(),
                join,
                versions: &self.versions,
            }
        }
    }

    fn world() -> World {
        let mut catalog =
            Catalog { format: 1, generated: Utc::now(), discord_checked: None, addons: vec![entry("a", "A", &[]), entry("k", "Keijin", &["A"])] };
        catalog.addons[1].latest.folders = [("Keijin".to_string(), "k2".to_string())].into();
        let mut store = Store::default();
        store.put(record("a", "A", "v1", "p1"));
        World {
            catalog,
            store,
            found: vec![found("A", Some("## Interface: 11200\n## Version: 1\n"))],
            prints: [("A".to_string(), "p1".to_string())].into(),
            enabled: EnableList::default(),
            newer: BTreeMap::new(),
            avrt: false,
            forge_on: true,
            forge_state: forge::State::default(),
            registry: None,
            versions: BTreeMap::new(),
            now: Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap(),
        }
    }

    /// A SurvivorsForge entry for these tests, its latest version `tag` with `trust` (flagged: 2
    /// warnings) and its checksum.
    fn listed(slug: &str, repo: &str, folders: &[&str], tag: &str, trust: &str, sha: Option<&str>) -> forge::ForgeAddon {
        forge::ForgeAddon {
            id: slug.into(),
            name: slug.into(),
            author: "sam".into(),
            repo: repo.into(),
            description: format!("{slug}, in one line"),
            category: "Combat".into(),
            folders: folders.iter().map(|f| f.to_string()).collect(),
            trust: Some(trust.into()),
            latest: Some(forge::Latest {
                tag: tag.into(),
                asset_url: format!("https://github.com/{repo}/releases/download/{tag}/{slug}.zip"),
                sha256: sha.map(str::to_string),
                trust: Some(trust.into()),
                findings: forge::Counts { critical: 0, warn: if trust == "flagged" { 2 } else { 0 }, info: 0 },
                ..forge::Latest::default()
            }),
            ..forge::ForgeAddon::default()
        }
    }

    fn registry(addons: Vec<forge::ForgeAddon>) -> Option<forge::Registry> {
        Some(forge::Registry { schema: 1, addons })
    }

    /// Totems, installed from SurvivorsForge alone at v1.0 on 29 Oct; SurvivorsForge answering,
    /// its latest v1.1 (verified) first seen the next day.
    fn forge_world() -> World {
        let mut w = world();
        let mut totems = record("forge:totems", "Totems", "v1.0", "t1");
        (totems.badge, totems.repo, totems.forge, totems.sha256) = (Badge::Added, Some("sam/Totems".into()), Some("totems".into()), Some("s10".into()));
        w.store = Store::default();
        w.store.put(totems);
        w.found = vec![found("Totems", Some("## Interface: 11200\n"))];
        w.prints = [("Totems".to_string(), "t1".to_string())].into();
        w.forge_state.answered = Some(w.now);
        w.registry = registry(vec![listed("totems", "sam/Totems", &["Totems"], "v1.1", "verified", Some("s11"))]);
        w.forge_state.seen.insert("totems".into(), [("v1.1".to_string(), Utc.with_ymd_and_hms(2026, 10, 30, 9, 0, 0).unwrap())].into());
        w
    }

    #[test]
    fn a_verified_update_goes_in_at_play() {
        let w = world();
        let lines = w.lines();
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(matches!(&line.standing, Standing::Update { to, by_itself: true } if to.tag == "v2"));
        assert_eq!(line.notes.as_deref(), Some("notes v2"), "what's new: the update's notes");
        assert!(line.on && line.switchable);
        assert_eq!(at_play(&lines).len(), 1);
    }

    #[test]
    fn a_verified_addon_the_list_dropped_is_unlisted_and_not_updated() {
        let mut w = world();
        w.catalog.addons.retain(|e| e.id != "a");
        let lines = w.lines();
        assert_eq!(lines[0].standing, Standing::Unlisted);
        assert_eq!(lines[0].badge, Some(Badge::Verified), "the record keeps its badge; the row says why");
        assert!(lines[0].problems.iter().any(|p| p.contains("left the Discord Verified list")));
        assert!(at_play(&lines).is_empty(), "nothing updates it");
        w.prints.clear();
        assert_eq!(w.lines()[0].standing, Standing::Gone, "a lost folder says more");
    }

    #[test]
    fn a_turned_off_addon_waits_until_it_is_turned_on() {
        let mut w = world();
        w.enabled.set("A", false);
        let lines = w.lines();
        assert!(!lines[0].on && lines[0].off && lines[0].switchable);
        assert!(matches!(&lines[0].standing, Standing::Update { to, by_itself: false } if to.tag == "v2"), "it shows, and waits");
        assert!(at_play(&lines).is_empty(), "Play leaves it alone");
        w.enabled.set("A", true);
        assert_eq!(at_play(&w.lines()).len(), 1, "turned on, the next Play updates it");
    }

    #[test]
    fn an_addon_with_one_folder_of_two_off_keeps_updating() {
        let mut w = world();
        w.store.get_mut("a").unwrap().folders.insert("B".into(), "q1".into());
        w.prints.insert("B".into(), "q1".into());
        w.found.push(found("B", Some("## Interface: 11200\n## Version: 1\n")));
        w.enabled.set("B", false);
        let lines = w.lines();
        assert!(!lines[0].on && !lines[0].off, "one folder still loads: the addon is in use");
        assert_eq!(at_play(&lines).len(), 1);
        w.enabled.set("A", false);
        let lines = w.lines();
        assert!(lines[0].off);
        assert!(at_play(&lines).is_empty(), "neither loads: it waits");
    }

    #[test]
    fn a_release_found_on_github_is_newest() {
        let mut w = world();
        let mut seen = super::super::github::page_release("v3", &super::super::github::PageFile { name: "A-v3.zip".into(), url: "u".into(), sha256: None }, Utc::now());
        w.newer.insert("a".into(), seen.clone());
        let lines = w.lines();
        assert!(matches!(&lines[0].standing, Standing::Update { to, by_itself: false } if to.tag == "v3"), "no published checksum: it asks");
        seen.sha256 = "c".repeat(64);
        w.newer.insert("a".into(), seen);
        assert!(matches!(&w.lines()[0].standing, Standing::Update { by_itself: true, .. }));
    }

    #[test]
    fn a_release_newer_than_the_catalog_is_not_undone() {
        // v3 went in from GitHub's page, and the next look, knowing its tag, dropped it: the
        // catalog's latest (v2) is older, so nothing goes in at Play.
        let mut w = world();
        let a = w.store.get_mut("a").unwrap();
        a.tag = Some("v3".into());
        a.sha256 = Some("c".repeat(64));
        a.installed = Utc::now();
        let lines = w.lines();
        assert_eq!(lines[0].standing, Standing::Current);
        assert!(at_play(&lines).is_empty());
    }

    #[test]
    fn changed_kept_gone_and_moved() {
        let mut w = world();
        w.prints.insert("A".into(), "edited".into());
        let line = &w.lines()[0];
        assert_eq!(line.standing, Standing::Changed);
        assert!(line.problems[0].contains("changed since the Satchel installed it"));
        assert!(at_play(&w.lines()).is_empty(), "a changed addon is never updated by itself");

        let mut w = world();
        w.store.get_mut("a").unwrap().pinned = true;
        assert!(matches!(&w.lines()[0].standing, Standing::Kept { newer: Some(to) } if to.tag == "v2"));

        let mut w = world();
        w.prints.clear();
        w.found.clear();
        assert_eq!(w.lines()[0].standing, Standing::Gone);

        // Another tool put v2 in: the record follows.
        let mut w = world();
        w.prints.insert("A".into(), "p2".into());
        let moved = follow(&mut w.store, &w.catalog, &w.newer, &w.prints);
        assert_eq!(moved, ["A is now 2: another tool updated it"]);
        let record = w.store.get("a").unwrap();
        assert_eq!((record.tag.as_deref(), record.folders["A"].as_str()), (Some("v2"), "p2"));
        assert_eq!(record.released, Some(Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()), "v2's date");
        assert_eq!(w.lines()[0].standing, Standing::Current);
    }

    #[test]
    fn a_release_date_from_the_record_else_the_catalog() {
        let mut w = world();
        let catalog_date = Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap();
        assert_eq!(w.lines()[0].released, Some(catalog_date), "installed before dates were kept: the catalog's");
        let kept = Utc.with_ymd_and_hms(2026, 10, 1, 8, 0, 0).unwrap();
        w.store.get_mut("a").unwrap().released = Some(kept);
        assert_eq!(w.lines()[0].released, Some(kept));
        // A release the catalog doesn't hold, seen only on GitHub's page: no date.
        let a = w.store.get_mut("a").unwrap();
        (a.released, a.tag) = (None, Some("v9".into()));
        assert_eq!(w.lines()[0].released, None);
        w.found.push(found("Keijin", Some("## Interface: 11200\n")));
        assert_eq!(w.lines()[1].released, None, "installed by hand");
    }

    #[test]
    fn release_ages() {
        assert_eq!(released_ago(0), "released today");
        assert_eq!(released_ago(-1), "released today", "a PC clock behind GitHub's");
        assert_eq!(released_ago(1), "released yesterday");
        assert_eq!(released_ago(12), "released 12 days ago");
        assert_eq!(released_ago(59), "released 59 days ago");
        assert_eq!(released_ago(60), "released 2 months ago");
        assert_eq!(released_ago(400), "released a year ago");
        assert_eq!(released_ago(800), "released 2 years ago");
    }

    #[test]
    fn folders_installed_by_hand_and_their_problems() {
        let mut w = world();
        w.found.push(found("Keijin", Some("## Interface: 11200\n## Title: |cff33ff99Keijin|r UI\n## Dependencies: KeijinLib\n")));
        w.found.push(found("Plates.disabled", None));
        w.found[2].problem = Some("turned off by another tool (renamed from Plates)".into());
        w.prints.insert("Keijin".into(), "hand".into());
        let lines = w.lines();
        let names: Vec<&str> = lines.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["A", "Keijin", "Plates.disabled"]);
        let keijin = &lines[1];
        assert_eq!((keijin.id.as_str(), keijin.badge, &keijin.standing), ("folder:Keijin", None, &Standing::ByHand));
        assert_eq!(keijin.replace_with.as_deref(), Some("2"));
        assert!(keijin.problems.iter().any(|p| p == "needs KeijinLib, which isn't in AddOns"));
        assert!(keijin.problems.iter().any(|p| p.starts_with("doesn't mix with A")), "{:?}", keijin.problems);
        assert!(lines[0].problems.iter().any(|p| p.starts_with("doesn't mix with Keijin")), "both sides are told");
        let plates = &lines[2];
        assert!(!plates.switchable && !plates.on);
        // Turning Keijin off settles the clash.
        w.enabled.set("Keijin", false);
        assert!(!w.lines()[0].problems.iter().any(|p| p.starts_with("doesn't mix")));
    }

    #[test]
    fn dragon_ui_under_its_own_updater() {
        let mut w = world();
        let mut dragon = record("dragon", SELF_UPDATING, "v1", "d1");
        dragon.name = "SurvivorsDragonUi".into();
        w.store.put(dragon);
        w.catalog.addons.push(Entry { folders: vec![SELF_UPDATING.into()], ..entry("dragon", SELF_UPDATING, &[]) });
        w.found.push(found(SELF_UPDATING, Some("## Interface: 11200\n")));
        w.prints.insert(SELF_UPDATING.into(), "replaced-by-avrt".into());
        w.avrt = true;
        let line = w.lines().into_iter().find(|l| l.id == "dragon").unwrap();
        assert_eq!(line.standing, Standing::Elsewhere);
        assert!(!watches(&w.store, &w.catalog, true).iter().any(|x| x.id == "dragon"));
        assert!(watches(&w.store, &w.catalog, false).iter().any(|x| x.id == "dragon"));
    }

    #[test]
    fn offers_and_look_alikes() {
        let mut w = world();
        w.found.push(found("Keijin", Some("## Interface: 11200\n")));
        let offers = w.offers(&Store::default(), &w.found[..1]);
        assert_eq!(offers.len(), 2);
        assert!(offers[0].installed, "its folder is there");
        assert_eq!(offers[1].warning.as_deref(), Some("doesn't mix with A, which you have"));
        assert!(look_alike(&w.catalog, "other/A", "Something", &["A".into()]).unwrap().starts_with("A is a Discord Verified addon by o."));
        assert_eq!(look_alike(&w.catalog, "o/A", "A", &["A".into()]), None, "the verified repository itself");
        let watch = &watches(&w.store, &w.catalog, false)[0];
        assert_eq!((watch.repo.as_str(), &watch.pick), ("o/A", &Pick::Pattern("A-*.zip".into())));
        assert_eq!(watch.known, ["v2", "v1", "v1"]);
    }

    #[test]
    fn survivorsforge_holds_back_updates_of_our_list() {
        let mut w = world();
        w.forge_state.answered = Some(w.now);
        // Our "a" is listed there as "a-forge", its v2 flagged.
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v2", "flagged", Some("sha-v2"))]);
        let lines = w.lines();
        assert!(matches!(&lines[0].standing, Standing::Update { to, by_itself: false } if to.tag == "v2"));
        assert_eq!(lines[0].verdict, Verdict::Ask(forge::Why::Flagged { critical: 0, warn: 2 }));
        assert_eq!(lines[0].forge_slug.as_deref(), Some("a-forge"));
        assert!(at_play(&lines).is_empty(), "it waits for the player's OK");
        // Silent for an hour: the warning holds.
        w.forge_state.answered = Some(w.now - TimeDelta::hours(2));
        assert!(matches!(w.lines()[0].verdict, Verdict::Ask(_)) && at_play(&w.lines()).is_empty());
        // Off in Settings: as before SurvivorsForge.
        w.forge_on = false;
        assert_eq!((w.lines()[0].verdict.clone(), w.lines()[0].forge_slug.clone()), (Verdict::Go, None));
        assert_eq!(at_play(&w.lines()).len(), 1);
        // Verified there: in by itself, as before.
        w.forge_on = true;
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v2", "verified", Some("sha-v2"))]);
        assert_eq!(at_play(&w.lines()).len(), 1);
        // Another file than ours under that tag: never.
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v2", "verified", Some("sha-other"))]);
        assert_eq!(w.lines()[0].verdict, Verdict::No(forge::Why::OtherFile));
        assert!(at_play(&w.lines()).is_empty());
        // Another version as its latest: its word on v2 comes from the versions, when cached.
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v3", "verified", Some("sha-v3"))]);
        assert_eq!(w.lines()[0].verdict, Verdict::Go, "nothing known about v2: our rule");
        w.versions.insert("a-forge".into(), vec![forge::Version { tag: "v2".into(), trust: Some("unscanned".into()), ..forge::Version::default() }]);
        assert_eq!(w.lines()[0].verdict, Verdict::Ask(forge::Why::NotScanned));
    }

    #[test]
    fn our_addon_that_leaves_survivorsforge_asks_first_until_its_details_say_why() {
        let mut w = world();
        w.forge_state.answered = Some(w.now);
        w.forge_state.listed.insert("a-forge".into());
        w.forge_state.repos.insert("a-forge".into(), "o/A".into());
        w.registry = registry(Vec::new());
        let lines = w.lines();
        assert_eq!((lines[0].verdict.clone(), lines[0].forge_slug.as_deref()), (Verdict::Ask(forge::Why::Dropped), Some("a-forge")));
        assert!(at_play(&lines).is_empty());
        // Its details answered 404: deleted there, so our rule again.
        w.forge_state.deleted.insert("a-forge".into());
        assert_eq!(at_play(&w.lines()).len(), 1);
        // Blocked there: never, and Browse doesn't offer it.
        w.forge_state.deleted.clear();
        w.forge_state.blocked.insert("a-forge".into(), "Not its author's".into());
        assert_eq!(w.lines()[0].verdict, Verdict::No(forge::Why::Blocked(Some("Not its author's".into()))));
        assert!(!w.offers(&Store::default(), &[]).iter().any(|o| o.id == "a"));
    }

    #[test]
    fn a_survivorsforge_install_updates_from_its_registry() {
        let mut w = forge_world();
        let lines = w.lines();
        assert!(matches!(&lines[0].standing, Standing::Update { to, by_itself: true } if to.tag == "v1.1" && to.url.ends_with("/v1.1/totems.zip")));
        assert_eq!((lines[0].verdict.clone(), lines[0].forge_slug.as_deref()), (Verdict::Go, Some("totems")));
        assert_eq!(at_play(&lines).len(), 1, "verified there: in by itself, like our list");
        assert!(watches(&w.store, &w.catalog, false).is_empty(), "no look on GitHub for it");
        // Flagged there: it waits for the player's OK.
        w.registry = registry(vec![listed("totems", "sam/Totems", &["Totems"], "v1.1", "flagged", Some("s11"))]);
        assert!(matches!(w.lines()[0].standing, Standing::Update { by_itself: false, .. }));
        assert_eq!(w.lines()[0].verdict, Verdict::Ask(forge::Why::Flagged { critical: 0, warn: 2 }));
        // Silent for an hour: no updates until it's back.
        w.registry = forge_world().registry;
        w.forge_state.answered = Some(w.now - TimeDelta::minutes(61));
        assert_eq!(w.lines()[0].verdict, Verdict::No(forge::Why::Offline));
        assert!(at_play(&w.lines()).is_empty());
        // Off in Settings: no updates at all.
        w.forge_state.answered = Some(w.now);
        w.forge_on = false;
        assert_eq!((w.lines()[0].standing.clone(), w.lines()[0].verdict.clone()), (Standing::Current, Verdict::No(forge::Why::Off)));
        // No longer listed: it stays as it is, without updates.
        w.forge_on = true;
        w.forge_state.listed.insert("totems".into());
        w.registry = registry(Vec::new());
        assert_eq!((w.lines()[0].standing.clone(), w.lines()[0].verdict.clone()), (Standing::Current, Verdict::No(forge::Why::NotListed)));
    }

    #[test]
    fn a_step_back_on_survivorsforge_is_no_update() {
        let mut w = forge_world();
        // Its latest went back to v0.9, which this PC saw before it installed v1.0.
        w.registry = registry(vec![listed("totems", "sam/Totems", &["Totems"], "v0.9", "verified", Some("s09"))]);
        w.forge_state.seen.insert("totems".into(), [("v0.9".to_string(), Utc.with_ymd_and_hms(2026, 10, 20, 9, 0, 0).unwrap())].into());
        let lines = w.lines();
        assert_eq!((lines[0].standing.clone(), lines[0].verdict.clone()), (Standing::Current, Verdict::Go));
        // A newer tag this PC never dated (a lost cache) waits until the next answer dates it.
        w.registry = forge_world().registry;
        w.forge_state.seen.clear();
        assert_eq!(w.lines()[0].standing, Standing::Current);
    }

    #[test]
    fn after_going_back_a_survivorsforge_install_updates_to_the_latest_it_saw() {
        let mut w = forge_world();
        // v1.0 was SurvivorsForge's latest from 25 Oct, v1.1 from 30 Oct; today the player went
        // back to v1.0, which keeps it.
        let totems = w.store.get_mut("forge:totems").unwrap();
        (totems.installed, totems.pinned) = (Utc.with_ymd_and_hms(2026, 10, 31, 11, 0, 0).unwrap(), true);
        let day = |d: u32| Utc.with_ymd_and_hms(2026, 10, d, 9, 0, 0).unwrap();
        w.forge_state.seen.insert("totems".into(), [("v1.0".to_string(), day(25)), ("v1.1".to_string(), day(30))].into());
        assert!(matches!(&w.lines()[0].standing, Standing::Kept { newer: Some(to) } if to.tag == "v1.1"));
        // Kept no more: v1.1 is an update again, and goes in by itself.
        w.store.get_mut("forge:totems").unwrap().pinned = false;
        let lines = w.lines();
        assert!(matches!(&lines[0].standing, Standing::Update { to, by_itself: true } if to.tag == "v1.1" && !to.url.is_empty()));
        assert_eq!(at_play(&lines).len(), 1);
        // A latest this PC saw before v1.0 is still a step back.
        w.registry = registry(vec![listed("totems", "sam/Totems", &["Totems"], "v0.9", "verified", Some("s09"))]);
        w.forge_state.seen.get_mut("totems").unwrap().insert("v0.9".into(), day(20));
        assert_eq!(w.lines()[0].standing, Standing::Current);
    }

    #[test]
    fn a_word_from_an_older_answer_still_holds_back_an_update_of_our_list() {
        let mut w = world();
        w.forge_state.answered = Some(w.now);
        // SurvivorsForge's latest moved past our v2, to v3, and no current list of its versions is
        // cached: nothing current is known about v2, so our rule.
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v3", "verified", Some("sha-v3"))]);
        assert_eq!(w.lines()[0].verdict, Verdict::Go);
        // An earlier answer flagged v2: that word still holds v2 back.
        let flagged = forge::Restriction { trust: "flagged".into(), findings: forge::Counts { critical: 0, warn: 1, info: 0 }, sha256: None };
        w.forge_state.restrictions.insert("a-forge".into(), [("v2".to_string(), flagged)].into());
        let lines = w.lines();
        assert_eq!(lines[0].verdict, Verdict::Ask(forge::Why::Flagged { critical: 0, warn: 1 }));
        assert!(at_play(&lines).is_empty(), "not by itself");
        // The install-time question is still asked: the word isn't a current one.
        let join = w.join();
        assert!(w.view(&join).word("a-forge", "v2").is_none());
        // A current list that lets v2 in decides; with the switch off nothing holds.
        w.versions.insert("a-forge".into(), vec![forge::Version { tag: "v2".into(), trust: Some("verified".into()), ..forge::Version::default() }]);
        assert_eq!(w.lines()[0].verdict, Verdict::Go);
        w.versions.clear();
        w.forge_on = false;
        assert_eq!(w.lines()[0].verdict, Verdict::Go);
    }

    #[test]
    fn a_card_held_back_only_by_an_older_answer_stays_and_asks() {
        let mut w = world();
        w.forge_state.answered = Some(w.now);
        // An earlier answer said our v2's file changed after its scan; SurvivorsForge's latest has
        // moved on to v3 since, and nothing current speaks of v2.
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v3", "verified", Some("sha-v3"))]);
        let changed = forge::Restriction { trust: "source_changed".into(), findings: forge::Counts::default(), sha256: None };
        w.forge_state.restrictions.insert("a-forge".into(), [("v2".to_string(), changed)].into());
        let offers = w.offers(&Store::default(), &[]);
        assert_eq!((offers[0].id.as_str(), offers[0].verdict.clone()), ("a", Verdict::Ask(forge::Why::SourceChanged)), "the card stays, and asks");
        // The install itself is still refused until a current answer lifts the word.
        let join = w.join();
        assert_eq!(w.view(&join).verdict(Origin::OurList, Some("a-forge"), "v2", Some("sha-v2")), Verdict::No(forge::Why::SourceChanged));
        // The same word in a current answer hides the card.
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v2", "source_changed", Some("sha-v2"))]);
        assert!(!w.offers(&Store::default(), &[]).iter().any(|o| o.id == "a"));
    }

    #[test]
    fn a_rezipped_import_of_our_release_is_judged_by_the_releases_checksum() {
        let mut w = world();
        w.forge_state.answered = Some(w.now);
        // v2 imported from a zip of its own: exactly the release's files, but that zip's checksum.
        let a = w.store.get_mut("a").unwrap();
        (a.tag, a.sha256, a.folders) = (Some("v2".into()), Some("rezipped".into()), [("A".to_string(), "p2".to_string())].into());
        w.prints.insert("A".into(), "p2".into());
        // SurvivorsForge scanned the release's own file.
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v2", "verified", Some("sha-v2"))]);
        let lines = w.lines();
        assert_eq!((lines[0].standing.clone(), lines[0].verdict.clone()), (Standing::Current, Verdict::Go));
        let record = w.store.get("a").unwrap();
        assert_eq!(judged_sha(&w.catalog, record, Some("v2"), Some("rezipped")), Some("sha-v2"));
        assert_eq!(judged_sha(&w.catalog, record, Some("v9"), Some("rezipped")), Some("rezipped"), "a release the catalog doesn't hold");
        let linked = Record { badge: Badge::Added, ..record.clone() };
        assert_eq!(judged_sha(&w.catalog, &linked, Some("v2"), Some("rezipped")), Some("rezipped"), "our list only");
        // SurvivorsForge's checksum for v2 isn't the release's: that is still another file.
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v2", "verified", Some("sha-other"))]);
        assert_eq!(w.lines()[0].verdict, Verdict::No(forge::Why::OtherFile));
    }

    #[test]
    fn survivorsforge_only_addons_in_browse() {
        let mut w = world();
        w.forge_state.answered = Some(w.now);
        w.registry = registry(vec![
            listed("a-forge", "o/A", &["A"], "v2", "verified", Some("sha-v2")),
            listed("totems", "sam/Totems", &["Totems"], "v1.1", "scanned", Some("s11")),
            listed("odd", "sam/Odd", &["Odd"], "v1", "flagged", Some("s1")),
            listed("unhashed", "sam/Unhashed", &["Unhashed"], "v1", "verified", None),
        ]);
        let offers = w.offers(&Store::default(), &[]);
        let ids: Vec<&str> = offers.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["a", "k", "forge:totems", "forge:odd"], "no checksum yet, no install: not offered");
        let a = &offers[0];
        assert_eq!((a.origin, a.tags.clone(), a.forge_slug.as_deref()), (1, vec!["g".to_string(), "Combat".to_string()], Some("a-forge")));
        assert_eq!((a.date, a.downloads, a.verdict.clone()), (Some(Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()), None, Verdict::Go));
        assert_eq!((offers[1].origin, offers[1].tags.clone()), (0, vec!["g".to_string()]));
        let totems = &offers[2];
        assert_eq!((totems.origin, totems.tags.clone(), totems.version.as_str()), (2, vec!["Combat".to_string()], "v1.1"));
        assert_eq!((totems.summary.as_str(), totems.forge_slug.as_deref()), ("totems, in one line", Some("totems")));
        assert_eq!((totems.installed, totems.date, totems.downloads, totems.verdict.clone()), (false, None, None, Verdict::Go));
        assert_eq!(offers[3].verdict, Verdict::Ask(forge::Why::Flagged { critical: 0, warn: 2 }), "flagged: it asks first");
        // Its date and count are the registry's: no versions fetch needed.
        let day = Utc.with_ymd_and_hms(2026, 10, 30, 9, 0, 0).unwrap();
        let totems = w.registry.as_mut().unwrap().addons.iter_mut().find(|a| a.id == "totems").unwrap();
        (totems.downloads, totems.latest.as_mut().unwrap().published_at) = (Some(1_234), Some(day));
        let offers = w.offers(&Store::default(), &[]);
        assert_eq!((offers[2].date, offers[2].downloads), (Some(day), Some(1_234)));
        // Cached versions don't stand in for the registry's count (an addon published by tag has
        // none: unknown, never zero or a sum); a registry without the date leaves its versions'.
        let totems = w.registry.as_mut().unwrap().addons.iter_mut().find(|a| a.id == "totems").unwrap();
        (totems.downloads, totems.latest.as_mut().unwrap().published_at) = (None, None);
        let earlier = Utc.with_ymd_and_hms(2026, 10, 29, 9, 0, 0).unwrap();
        w.versions.insert(
            "totems".into(),
            vec![
                forge::Version { tag: "v1.1".into(), published_at: Some(earlier), downloads: Some(5), ..forge::Version::default() },
                forge::Version { tag: "v1.0".into(), downloads: Some(7), ..forge::Version::default() },
            ],
        );
        let offers = w.offers(&Store::default(), &[]);
        assert_eq!((offers[2].date, offers[2].downloads), (Some(earlier), None));
        // Installed: from SurvivorsForge, added by link from the same repository, or its folders there.
        let mut store = Store::default();
        let mut linked = record("sam/Totems", "Totems", "v1.1", "t1");
        (linked.badge, linked.repo) = (Badge::Added, Some("SAM/totems".into()));
        store.put(linked);
        assert!(w.offers(&store, &[])[2].installed);
        assert!(w.offers(&Store::default(), &[found("Totems", Some("## Interface: 11200\n"))])[2].installed);
        // Silent for an hour, or off: our list alone, without SurvivorsForge's tags.
        w.forge_state.answered = Some(w.now - TimeDelta::hours(2));
        let offers = w.offers(&Store::default(), &[]);
        assert_eq!((offers.len(), offers[0].origin, offers[0].tags.clone()), (2, 0, vec!["g".to_string()]));
        w.forge_state.answered = Some(w.now);
        w.forge_on = false;
        let offers = w.offers(&Store::default(), &[]);
        assert_eq!((offers.len(), offers[0].forge_slug.clone()), (2, None));
    }

    #[test]
    fn initials_like_the_mock() {
        assert_eq!(initials("AZS Spell Meter"), "SM");
        assert_eq!(initials("LuckyOdds"), "LO");
        assert_eq!(initials("SurvivorsDragonUi"), "DU");
        assert_eq!(initials("KeijinUI"), "KU");
        assert_eq!(initials("Move UI"), "MU");
        assert_eq!(initials("Azerloot"), "AZ");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn release_notes_as_plain_text() {
        let notes = "## New\n\n- **Boss warnings:** a bar before each big hit\n  - see [the wiki](https://x.y/w)\n\n\nUse `/azs` to open it.\n";
        assert_eq!(plain_notes(notes, 700), "New\n\n• Boss warnings: a bar before each big hit\n  • see the wiki\n\nUse /azs to open it.");
        let long = (0..100).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let short = plain_notes(&long, 60);
        assert!(short.ends_with(" …") && short.len() <= 64 && !short.contains("line 9\nline 1"), "{short}");
        // A code block keeps its lines; its fences go, the language tag with them.
        let fenced = "Type:\n```text\n/azs reset\n```\n~~~\n/azs show\n~~~\nDone.";
        assert_eq!(plain_notes(fenced, 700), "Type:\n/azs reset\n/azs show\nDone.");
    }

    #[test]
    fn titles_without_colour_codes() {
        assert_eq!(plain("|cff33ff99Lucky|r Odds"), "Lucky Odds");
        assert_eq!(plain("Plain | Title"), "Plain | Title");
    }

    #[test]
    fn browses_sorts_put_unknown_dates_and_counts_last() {
        let keys = [
            SortKeys { name: "Totems", id: "forge:totems", date: Some(300), downloads: Some(12) },
            SortKeys { name: "[KMA] Arrow", id: "forge:kma", date: Some(100), downloads: None },
            SortKeys { name: "azs meter", id: "azs", date: None, downloads: Some(500) },
            SortKeys { name: "Big Cursor", id: "big", date: Some(200), downloads: None },
            SortKeys { name: "Zed", id: "zed", date: None, downloads: Some(12) },
        ];
        let sorted = |sort: Sort, reverse: bool| {
            let mut keys = keys.to_vec();
            keys.sort_by(|a, b| browse_order(a, b, sort, reverse));
            keys.iter().map(|k| k.id).collect::<Vec<_>>()
        };
        assert_eq!(sorted(Sort::Name, false), ["azs", "big", "forge:kma", "forge:totems", "zed"], "case aside, [KMA] under K");
        assert_eq!(sorted(Sort::Name, true), ["zed", "forge:totems", "forge:kma", "big", "azs"]);
        // Last updated: newest first, or oldest first from the table's header; no date last either way.
        assert_eq!(sorted(Sort::Updated, false), ["forge:totems", "big", "forge:kma", "azs", "zed"]);
        assert_eq!(sorted(Sort::Updated, true), ["forge:kma", "big", "forge:totems", "azs", "zed"]);
        // Popularity: most first; the same count by name; no count last, never taken for none.
        assert_eq!(sorted(Sort::Popularity, false), ["azs", "forge:totems", "zed", "big", "forge:kma"]);
        assert_eq!(sorted(Sort::Popularity, true), ["forge:totems", "zed", "azs", "big", "forge:kma"]);
        assert_eq!((Sort::from_index(1), Sort::from_index(2), Sort::from_index(7)), (Sort::Updated, Sort::Popularity, Sort::Name));
    }

    #[test]
    fn versions_merge_the_installed_the_kept_our_releases_and_survivorsforges() {
        let mut w = world();
        let day = |d: u32| Utc.with_ymd_and_hms(2026, 10, d, 12, 0, 0).unwrap();
        // Our "a": v2 (29 Oct, 100 downloads) and v1 (25 Oct) in the catalog; v1 installed, kept at
        // it, with v0.9 (1 Oct) in the backups.
        let entry = w.catalog.addons.iter_mut().find(|e| e.id == "a").unwrap();
        entry.latest.downloads = Some(100);
        entry.earlier[0].published = day(25);
        let a = w.store.get_mut("a").unwrap();
        a.pinned = true;
        a.earlier = vec![Kept {
            tag: Some("v0.9".into()),
            file: None,
            version: Some("0.9".into()),
            sha256: Some("sha-v0.9".into()),
            released: Some(day(1)),
            url: None,
            folders: BTreeMap::new(),
            backup: "2026-10-20 10-00-00 a".into(),
            kept: day(20),
        }];
        // SurvivorsForge lists it as "a-forge": its v3 (31 Oct, flagged, for game 1.0 to 1.1) on the
        // author's GitHub, v2 verified, and v0.8 without a checksum.
        let mut entry = listed("a-forge", "o/A", &["A"], "v3", "flagged", Some("sha-v3"));
        entry.latest.as_mut().unwrap().published_at = Some(day(31));
        let theirs = vec![
            forge::Version {
                tag: "v3".into(),
                published_at: Some(day(31)),
                asset_url: Some("https://github.com/o/A/releases/download/v3/A-v3.zip".into()),
                sha256: Some("sha-v3".into()),
                trust: Some("flagged".into()),
                findings: forge::Counts { critical: 0, warn: 2, info: 0 },
                game_min: Some("1.0".into()),
                game_max: Some("1.1".into()),
                ..forge::Version::default()
            },
            forge::Version { tag: "v2".into(), downloads: Some(77), trust: Some("verified".into()), ..forge::Version::default() },
            forge::Version { tag: "v0.8".into(), published_at: Some(day(2)), asset_url: Some("https://github.com/o/A/releases/download/v0.8/A.zip".into()), ..forge::Version::default() },
        ];
        let record = w.store.get("a").unwrap();
        let rows = versions(&VersionsOf { record: Some(record), catalog: w.catalog.get("a"), listed: Some(&entry), versions: Some(theirs.as_slice()) });
        let tags: Vec<&str> = rows.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(tags, ["v3", "v2", "v1", "v0.8", "v0.9"], "newest first, one row per version");
        let (v3, v2, v1, v08, v09) = (&rows[0], &rows[1], &rows[2], &rows[3], &rows[4]);
        assert_eq!((v3.word, v3.game.clone()), (Some((forge::Word::Flagged, theirs[0].findings)), (Some("1.0".into()), Some("1.1".into()))));
        assert!(matches!(&v3.action, VersionAction::Install { release } if release.url.ends_with("/v3/A-v3.zip") && release.sha256 == "sha-v3"), "on the author's GitHub, with a checksum");
        assert_eq!((v2.published, v2.downloads, v2.word.map(|(w, _)| w)), (Some(day(29)), Some(100), Some(forge::Word::Verified)), "our catalog's date and count");
        assert!(matches!(&v2.action, VersionAction::Install { release } if release.url == w.catalog.get("a").unwrap().latest.url), "our catalog's release");
        assert_eq!((v1.installed, v1.published, &v1.action), (true, Some(day(25)), &VersionAction::None), "the one in AddOns does nothing");
        assert_eq!((v08.word.map(|(w, _)| w), &v08.action), (Some(forge::Word::Unscanned), &VersionAction::None), "no checksum yet: nothing downloads it");
        assert_eq!((v09.kept.clone(), &v09.action), (Some(("2026-10-20 10-00-00 a".to_string(), day(20))), &VersionAction::Back { backup: "2026-10-20 10-00-00 a".into() }));
        // Kept at v1, the way a version picked here is: nothing goes in by itself at Play.
        let lines = w.lines();
        assert!(matches!(&lines[0].standing, Standing::Kept { newer: Some(to) } if to.tag == "v2"));
        assert!(at_play(&lines).is_empty());

        // SurvivorsForge's own Totems, its versions not fetched yet: the registry's latest stands
        // for them, with its date.
        let w = forge_world();
        let mut totems = w.registry.as_ref().unwrap().addons[0].clone();
        totems.latest.as_mut().unwrap().published_at = Some(day(30));
        let rows = versions(&VersionsOf { record: w.store.get("forge:totems"), listed: Some(&totems), ..VersionsOf::default() });
        assert_eq!(rows.iter().map(|r| (r.tag.as_str(), r.installed)).collect::<Vec<_>>(), [("v1.1", false), ("v1.0", true)]);
        assert_eq!((rows[0].published, rows[0].word.map(|(w, _)| w)), (Some(day(30)), Some(forge::Word::Verified)));
        assert!(matches!(&rows[0].action, VersionAction::Install { release } if release.url.ends_with("/v1.1/totems.zip")));
        // Not installed and not listed anywhere: nothing.
        assert!(versions(&VersionsOf::default()).is_empty());
    }

    #[test]
    fn install_again_and_an_incoming_addon_ask_first_and_go_in_with_the_ok() {
        let mut w = world();
        w.forge_state.answered = Some(w.now);
        // Our "a", removed here at v2; SurvivorsForge flags v2 now. Install again asks the way
        // Browse does: the reason on its row, and it goes in only with Install anyway.
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v2", "flagged", Some("sha-v2"))]);
        let join = w.join();
        let again = w.view(&join).verdict(Origin::OurList, Some("a-forge"), "v2", Some("sha-v2"));
        assert_eq!(again, Verdict::Ask(forge::Why::Flagged { critical: 0, warn: 2 }));
        assert_eq!(again.why().unwrap().text(), "SurvivorsForge's scan found 2 warnings in this version", "the reason its row shows");
        assert!(!forge::admits(&again, false), "not by itself");
        assert!(forge::admits(&again, true), "with the player's OK");
        // Not scanned yet: the same question.
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v2", "unscanned", Some("sha-v2"))]);
        let join = w.join();
        assert_eq!(w.view(&join).verdict(Origin::OurList, Some("a-forge"), "v2", Some("sha-v2")), Verdict::Ask(forge::Why::NotScanned));
        // Blocked: never, OK or not.
        w.forge_state.blocked.insert("a-forge".into(), "Not its author's".into());
        let blocked = w.view(&join).verdict(Origin::OurList, Some("a-forge"), "v2", Some("sha-v2"));
        assert!(matches!(blocked, Verdict::No(forge::Why::Blocked(_))) && !forge::admits(&blocked, true));

        // SurvivorsForge's own Totems arriving from another PC, its v1.1 flagged here: it waits,
        // and its row's Install anyway lets in exactly that file.
        let mut w = forge_world();
        w.registry = registry(vec![listed("totems", "sam/Totems", &["Totems"], "v1.1", "flagged", Some("s11"))]);
        let join = w.join();
        let incoming = w.view(&join).verdict(Origin::ForgeOnly { slug: "totems" }, None, "v1.1", Some("s11"));
        assert_eq!(incoming, Verdict::Ask(forge::Why::Flagged { critical: 0, warn: 2 }));
        let mut oks = forge::Oks::default();
        assert!(!forge::admits(&incoming, oks.has("forge:totems", "s11")), "it waits for the OK");
        oks.give("forge:totems", "s11");
        assert!(forge::admits(&incoming, oks.has("forge:totems", "s11")), "Install anyway");
        assert!(!forge::admits(&incoming, oks.has("forge:totems", "s12")), "another file needs its own OK");
    }

    #[test]
    fn icons_come_from_the_registry_from_allowed_hosts_only() {
        let mut w = forge_world();
        w.registry.as_mut().unwrap().addons[0].icon_url = Some("https://forge.rezondes.net/img/abc.webp".into());
        let mut theirs = listed("drums", "sam/Drums", &["Drums"], "v1", "verified", Some("d1"));
        theirs.icon_url = Some("https://example.com/drums.webp".into());
        w.registry.as_mut().unwrap().addons.push(theirs);
        let icon = Some("https://forge.rezondes.net/img/abc.webp".to_string());
        assert_eq!(w.lines()[0].icon_url, icon, "an installed addon joined to its entry");
        let offers = w.offers(&Store::default(), &[]);
        let icon_of = |id: &str| offers.iter().find(|o| o.id == id).and_then(|o| o.icon_url.clone());
        assert_eq!((icon_of("forge:totems"), icon_of("forge:drums")), (icon, None), "another host is refused");
        // Offline the registry kept still names it (pictures are static); off, nothing does.
        w.forge_state.answered = Some(w.now - TimeDelta::hours(2));
        assert!(w.lines()[0].icon_url.is_some());
        w.forge_on = false;
        assert_eq!(w.lines()[0].icon_url, None);
    }

    #[test]
    fn a_word_kept_about_another_file_under_the_tag_holds_nothing_back() {
        let mut w = world();
        w.forge_state.answered = Some(w.now);
        // SurvivorsForge's latest moved on to v3; an older answer flagged v2's file "sha-old".
        w.registry = registry(vec![listed("a-forge", "o/A", &["A"], "v3", "verified", Some("sha-v3"))]);
        let flagged = forge::Restriction { trust: "flagged".into(), findings: forge::Counts { critical: 0, warn: 1, info: 0 }, sha256: Some("sha-old".into()) };
        w.forge_state.restrictions.insert("a-forge".into(), [("v2".to_string(), flagged)].into());
        let join = w.join();
        let view = w.view(&join);
        assert_eq!(view.verdict(Origin::OurList, Some("a-forge"), "v2", Some("sha-v2")), Verdict::Go, "our v2 is another file: judged again, by our rule");
        assert_eq!(view.verdict(Origin::OurList, Some("a-forge"), "v2", Some("sha-old")), Verdict::Ask(forge::Why::Flagged { critical: 0, warn: 1 }), "the file it was about stays held back");
    }
}
