//! The Addons tab's model: every addon in AddOns with where it stands, what's wrong with it in
//! plain words and what can be done with it; the updates that go in by themselves at Play; the
//! verified addons on offer; and what to ask GitHub about. The app formats and wires it.

use super::catalog::{self, Asset, Catalog, Release, State};
use super::enabled::EnableList;
use super::github::Pick;
use super::store::{Badge, Record, Store};
use super::Found;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

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

/// An addon's known releases, newest first: one found on GitHub, then the catalog's.
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
pub fn lines(inputs: &Inputs) -> Vec<Line> {
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
        let releases = releases(record, inputs.catalog, inputs.newer);
        let state = catalog::state(record, &releases, &on_disk(record, inputs.prints));
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
                // turned-off addon waits until it's turned on.
                let by_itself = by_itself && inputs.auto && !to.sha256.is_empty() && !off;
                Standing::Update { to, by_itself }
            }
            State::Kept { newer } => Standing::Kept { newer },
            State::Changed => Standing::Changed,
            State::Gone => Standing::Gone,
        };
        let mut problems = folder_problems(&folders, entry);
        match &standing {
            Standing::Changed => problems.push("its files changed since the Satchel installed it, so it isn't updated by itself".into()),
            Standing::Gone => problems.push("its folder is gone from AddOns".into()),
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

/// A verified addon on offer in Browse.
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
}

/// The catalog's addons, in its order (by group).
pub fn offers(catalog: &Catalog, store: &Store, found: &[Found]) -> Vec<Offer> {
    let present = |folder: &str| found.iter().any(|f| f.folder.eq_ignore_ascii_case(folder));
    catalog
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
            }
        })
        .collect()
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
/// catalog.
pub fn watches(store: &Store, catalog: &Catalog, avrt: bool) -> Vec<Watch> {
    store
        .addons
        .iter()
        .filter(|r| !(avrt && r.folders.keys().any(|f| f.eq_ignore_ascii_case(SELF_UPDATING))))
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
                "{} is a verified addon by {}. This one is by {}: a copy or a look-alike, not the verified one.",
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
    use crate::addons::toc::Toc;
    use chrono::{TimeZone, Utc};

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
    }

    impl World {
        fn lines(&self) -> Vec<Line> {
            lines(&Inputs {
                catalog: &self.catalog,
                store: &self.store,
                found: &self.found,
                prints: &self.prints,
                enabled: &self.enabled,
                newer: &self.newer,
                auto: true,
                avrt: self.avrt,
            })
        }
    }

    fn world() -> World {
        let mut catalog = Catalog { format: 1, generated: Utc::now(), addons: vec![entry("a", "A", &[]), entry("k", "Keijin", &["A"])] };
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
        }
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
        let offers = offers(&w.catalog, &Store::default(), &w.found[..1]);
        assert_eq!(offers.len(), 2);
        assert!(offers[0].installed, "its folder is there");
        assert_eq!(offers[1].warning.as_deref(), Some("doesn't mix with A, which you have"));
        assert!(look_alike(&w.catalog, "other/A", "Something", &["A".into()]).unwrap().starts_with("A is a verified addon by o."));
        assert_eq!(look_alike(&w.catalog, "o/A", "A", &["A".into()]), None, "the verified repository itself");
        let watch = &watches(&w.store, &w.catalog, false)[0];
        assert_eq!((watch.repo.as_str(), &watch.pick), ("o/A", &Pick::Pattern("A-*.zip".into())));
        assert_eq!(watch.known, ["v2", "v1", "v1"]);
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
}
