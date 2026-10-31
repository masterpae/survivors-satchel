//! The catalog: the addons the game's devs verified, each pinned to its author's GitHub
//! repository, with its latest release and recent earlier ones: the file, its checksum and each
//! folder's fingerprint, so a copy installed by hand is recognised exactly. It ships inside the
//! exe (`catalog/catalog.json`, refreshed with each build) until a robot keeps it fresh on GitHub.

use super::store::{Badge, Record, Store};
use super::{Found, GameAddons, tree};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub format: u32,
    pub generated: DateTime<Utc>,
    /// The day the list was last compared with the game Discord's verified-addons channel
    /// (`2026-10-31`), for "checked on"; catalogs from before have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_checked: Option<String>,
    pub addons: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// `owner/repo` on GitHub: the only place its files come from.
    pub repo: String,
    pub author: String,
    /// What it does, in one line.
    pub summary: String,
    pub group: String,
    /// Its folders in AddOns.
    pub folders: Vec<String>,
    /// Which file of a release is the addon.
    pub asset: Asset,
    pub latest: Release,
    /// Recent earlier releases, newest first.
    #[serde(default)]
    pub earlier: Vec<Release>,
    /// Folders of addons it doesn't mix with, from its author's own notes.
    #[serde(default)]
    pub doesnt_mix_with: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    /// GitHub's download counts, summed over the releases listed here: Browse's popularity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Asset {
    /// The release file whose name matches `pattern` (`*` stands for any text; letters in any case).
    File { pattern: String },
    /// The repository's source zip at each version tag, for authors who publish by tag and attach
    /// no file.
    Source,
}

impl Asset {
    /// Whether the release file `name` is the addon.
    pub fn matches(&self, name: &str) -> bool {
        match self {
            Asset::File { pattern } => glob(pattern, name),
            Asset::Source => false,
        }
    }
}

/// `*` matches any text, anything else itself; letters in any case.
fn glob(pattern: &str, name: &str) -> bool {
    let (pattern, name) = (pattern.to_ascii_lowercase(), name.to_ascii_lowercase());
    let mut parts: Vec<&str> = pattern.split('*').collect();
    let last = parts.pop().unwrap_or_default();
    let Some(first) = (!parts.is_empty()).then(|| parts.remove(0)) else { return name == last };
    let Some(mut rest) = name.strip_prefix(first) else { return false };
    for part in parts {
        let Some(at) = rest.find(part) else { return false };
        rest = &rest[at + part.len()..];
    }
    rest.ends_with(last)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    /// The `## Version` inside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub published: DateTime<Utc>,
    #[serde(default)]
    pub prerelease: bool,
    pub file: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub interface: u32,
    /// Each folder's fingerprint (empty when it wasn't computed).
    #[serde(default)]
    pub folders: BTreeMap<String, String>,
    /// The author's release notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// How many times GitHub had served its file when the catalog was built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<u64>,
}

impl Release {
    /// The version to show: the `.toc`'s, else the tag.
    pub fn shown_version(&self) -> &str {
        self.version.as_deref().unwrap_or(&self.tag)
    }
}

const BUILTIN: &str = include_str!("../../../../catalog/catalog.json");

impl Catalog {
    /// The catalog this build carries.
    pub fn builtin() -> Catalog {
        serde_json::from_str(BUILTIN).expect("catalog/catalog.json is checked by its test")
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.addons.iter().find(|e| e.id == id)
    }

    /// The addon one of whose folders is `folder` (any case).
    pub fn for_folder(&self, folder: &str) -> Option<&Entry> {
        self.addons.iter().find(|e| e.folders.iter().any(|f| f.eq_ignore_ascii_case(folder)))
    }

    /// When the addon `id`'s release `tag` was published, when the catalog holds that release: a
    /// release seen only on GitHub's page has no true date.
    pub fn published(&self, id: &str, tag: &str) -> Option<DateTime<Utc>> {
        self.get(id)?.releases().find(|r| r.tag == tag).map(|r| r.published)
    }
}

impl Entry {
    /// The latest release, then the earlier ones.
    pub fn releases(&self) -> impl Iterator<Item = &Release> {
        std::iter::once(&self.latest).chain(&self.earlier)
    }

    /// The release whose folders are exactly these (by fingerprint, folder names without case).
    pub fn release_matching(&self, prints: &BTreeMap<String, String>) -> Option<&Release> {
        let lower = |m: &BTreeMap<String, String>| m.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect::<BTreeMap<_, _>>();
        let prints = lower(prints);
        self.releases().find(|r| !r.folders.is_empty() && lower(&r.folders) == prints)
    }
}

/// Addons already in AddOns that the Satchel didn't install, recognised: when all of a catalog
/// addon's folders are there and are exactly one of its releases, they get a record as if the
/// Satchel had installed that release (and from then on update like any verified addon).
pub fn adopt(catalog: &Catalog, game: &GameAddons, store: &Store, found: &[Found], now: DateTime<Utc>) -> Vec<Record> {
    let mut adopted = Vec::new();
    for entry in &catalog.addons {
        if store.get(&entry.id).is_some() || entry.folders.iter().any(|f| store.owner_of(f).is_some()) {
            continue;
        }
        let present = entry.folders.iter().all(|f| found.iter().any(|x| x.folder.eq_ignore_ascii_case(f) && x.toc.is_some()));
        if !present {
            continue;
        }
        let prints: Option<BTreeMap<String, String>> = entry
            .folders
            .iter()
            .map(|f| Some((f.clone(), tree::fingerprint_dir(&game.folder(f)?).ok()?)))
            .collect();
        let Some(prints) = prints else { continue };
        if let Some(release) = entry.release_matching(&prints) {
            adopted.push(Record {
                id: entry.id.clone(),
                name: entry.name.clone(),
                badge: Badge::Verified,
                repo: Some(entry.repo.clone()),
                tag: Some(release.tag.clone()),
                file: Some(release.file.clone()),
                version: release.version.clone(),
                sha256: Some(release.sha256.clone()),
                folders: prints,
                installed: now,
                released: Some(release.published),
                pinned: false,
                ask: false,
                url: (!release.url.is_empty()).then(|| release.url.clone()),
                local_only: false,
                settings_per_pc: false,
                earlier: Vec::new(),
                forge: None,
            });
        }
    }
    adopted
}

/// Where an installed addon stands.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// The newest version is in.
    Current,
    /// A newer release: installed by itself before Play, or after asking.
    Update { to: Release, by_itself: bool },
    /// Kept at this version by the player ("Keep this version").
    Kept { newer: Option<Release> },
    /// Its files are now exactly another known release: another tool updated it. The record
    /// follows.
    Moved { to: Release },
    /// Its files changed since it was installed: never updated by itself.
    Changed,
    /// Its folder is gone from AddOns.
    Gone,
}

/// Where `record` stands: `releases` are its source's releases, newest first (the catalog's, or
/// its GitHub page's), and `on_disk` each of its folders' fingerprint now (`None` when gone).
pub fn state(record: &Record, releases: &[Release], on_disk: &BTreeMap<String, Option<String>>) -> State {
    if record.folders.keys().any(|f| on_disk.get(f).is_none_or(Option::is_none)) {
        return State::Gone;
    }
    let now: BTreeMap<String, String> = on_disk.iter().filter_map(|(k, v)| Some((k.clone(), v.clone()?))).collect();
    if now != record.folders {
        let lower = |m: &BTreeMap<String, String>| m.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect::<BTreeMap<_, _>>();
        return match releases.iter().find(|r| !r.folders.is_empty() && lower(&r.folders) == lower(&now)) {
            Some(release) => State::Moved { to: release.clone() },
            None => State::Changed,
        };
    }
    // Newer means before the installed release in the list. One the list doesn't hold went in
    // from GitHub's page after the catalog was built: then only a release out after it is newer.
    let installed = releases
        .iter()
        .position(|r| record.sha256.as_deref() == Some(r.sha256.as_str()) || record.tag.as_deref() == Some(r.tag.as_str()));
    let newer = match installed {
        Some(at) => releases[..at].first(),
        None => releases.first().filter(|latest| latest.published > record.installed),
    }
    .cloned();
    // Our list's addons and SurvivorsForge's own go in by themselves (SurvivorsForge's verdict and
    // the caller's conditions still apply); anything else waits for the player.
    let by_itself = (record.badge == Badge::Verified || record.forge.is_some()) && !record.ask;
    match (record.pinned, newer) {
        (true, newer) => State::Kept { newer },
        (false, Some(to)) => State::Update { by_itself, to },
        (false, None) => State::Current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::store::Kept;
    use chrono::TimeZone;
    use std::fs;

    fn release(tag: &str, prints: &[(&str, &str)]) -> Release {
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
            folders: prints.iter().map(|(f, p)| (f.to_string(), p.to_string())).collect(),
            notes: None,
            downloads: None,
        }
    }

    fn record(tag: &str, prints: &[(&str, &str)]) -> Record {
        Record {
            id: "a".into(),
            name: "A".into(),
            badge: Badge::Verified,
            repo: Some("o/a".into()),
            tag: Some(tag.into()),
            file: Some(format!("A-{tag}.zip")),
            version: None,
            sha256: Some(format!("sha-{tag}")),
            folders: prints.iter().map(|(f, p)| (f.to_string(), p.to_string())).collect(),
            installed: Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap(),
            released: None,
            pinned: false,
            ask: false,
            url: None,
            local_only: false,
            settings_per_pc: false,
            earlier: Vec::<Kept>::new(),
            forge: None,
        }
    }

    fn disk(prints: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
        prints.iter().map(|(f, p)| (f.to_string(), p.map(str::to_string))).collect()
    }

    #[test]
    fn the_builtin_catalog_reads() {
        let catalog = Catalog::builtin();
        assert_eq!(catalog.format, 1);
        for entry in &catalog.addons {
            assert!(!entry.folders.is_empty() && entry.repo.contains('/'), "{}", entry.id);
            assert_eq!(entry.latest.sha256.len(), 64, "{}", entry.id);
        }
    }

    #[test]
    fn release_files_are_picked_by_pattern() {
        let file = |pattern: &str| Asset::File { pattern: pattern.into() };
        assert!(file("AZSSpellMeter-*.zip").matches("AZSSpellMeter-1.34.zip"));
        assert!(file("BigCursor.zip").matches("bigcursor.ZIP"));
        assert!(!file("BigCursor.zip").matches("BigCursor-main.zip"));
        // A checksum file beside the zip is not the addon.
        assert!(!file("AzerothCursor-*.zip").matches("AzerothCursor-1.0.0.zip.sha256"));
        // Two zips per release: the addon has dots in its version, the source review has none.
        assert!(file("AzerothStats-*.*.*.zip").matches("AzerothStats-1.7.0-beta.1.zip"));
        assert!(!file("AzerothStats-*.*.*.zip").matches("AzerothStats-source-review-6d551e73ea4b.zip"));
        // A file renamed between releases.
        assert!(file("MoveUI*.zip").matches("MoveUI.zip") && file("MoveUI*.zip").matches("MoveUI_AzerothSurvivors_build_1.2.2.zip"));
        // An installer in the same release is not the addon.
        assert!(!file("SurvivorUI-v*.zip").matches("SurvivorUI-Installer.zip"));
        assert!(!file("a*a").matches("a"));
        assert!(!Asset::Source.matches("BordoUI.zip"));
    }

    #[test]
    fn states() {
        let releases = [release("v3", &[("A", "p3")]), release("v2", &[("A", "p2")])];
        let mut mine = record("v2", &[("A", "p2")]);
        assert!(matches!(state(&mine, &releases, &disk(&[("A", Some("p2"))])), State::Update { by_itself: true, .. }));
        mine.ask = true;
        assert!(matches!(state(&mine, &releases, &disk(&[("A", Some("p2"))])), State::Update { by_itself: false, .. }));
        mine.pinned = true;
        assert!(matches!(state(&mine, &releases, &disk(&[("A", Some("p2"))])), State::Kept { newer: Some(_) }));
        assert_eq!(state(&mine, &releases, &disk(&[("A", Some("edited"))])), State::Changed);
        assert!(matches!(state(&mine, &releases, &disk(&[("A", Some("p3"))])), State::Moved { to } if to.tag == "v3"));
        assert_eq!(state(&mine, &releases, &disk(&[("A", None)])), State::Gone);
        assert_eq!(state(&record("v3", &[("A", "p3")]), &releases, &disk(&[("A", Some("p3"))])), State::Current);

        // v4 went in from GitHub's page after the catalog was built: the catalog's latest is
        // older, not an update. A release out after it went in is one.
        let mut page = record("v4", &[("A", "p4")]);
        page.installed = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
        assert_eq!(state(&page, &releases, &disk(&[("A", Some("p4"))])), State::Current);
        let mut v5 = release("v5", &[]);
        v5.published = Utc.with_ymd_and_hms(2026, 11, 5, 12, 0, 0).unwrap();
        let seen = [v5, releases[0].clone(), releases[1].clone()];
        assert!(matches!(state(&page, &seen, &disk(&[("A", Some("p4"))])), State::Update { to, .. } if to.tag == "v5"));
    }

    #[test]
    fn our_list_and_survivorsforge_alone_update_by_themselves() {
        let releases = [release("v3", &[("A", "p3")]), release("v2", &[("A", "p2")])];
        let here = disk(&[("A", Some("p2"))]);
        let mut forged = record("v2", &[("A", "p2")]);
        (forged.badge, forged.forge) = (Badge::Added, Some("a-forge".into()));
        assert!(matches!(state(&forged, &releases, &here), State::Update { by_itself: true, .. }), "installed from SurvivorsForge alone");
        forged.ask = true;
        assert!(matches!(state(&forged, &releases, &here), State::Update { by_itself: false, .. }));
        let mut linked = record("v2", &[("A", "p2")]);
        linked.badge = Badge::Added;
        assert!(matches!(state(&linked, &releases, &here), State::Update { by_itself: false, .. }), "added by link: as before");
    }

    #[test]
    fn a_catalog_from_before_popularity_and_the_channel_date_reads() {
        let before = r#"{"format": 1, "generated": "2026-10-31T10:54:01Z", "addons": [{"id": "a", "name": "A", "repo": "o/a",
            "author": "o", "summary": "s", "group": "g", "folders": ["A"], "asset": {"kind": "file", "pattern": "A-*.zip"},
            "latest": {"tag": "v1", "published": "2026-10-29T12:00:00Z", "file": "A-v1.zip",
                       "url": "https://github.com/o/a/releases/download/v1/A-v1.zip", "sha256": "x", "size": 1, "interface": 11200}}]}"#;
        let catalog: Catalog = serde_json::from_str(before).unwrap();
        assert_eq!((catalog.discord_checked.as_deref(), catalog.addons[0].downloads, catalog.addons[0].latest.downloads), (None, None, None));
        let written = serde_json::to_string(&catalog).unwrap();
        assert!(!written.contains("discord_checked") && !written.contains("downloads"), "{written}");
        let mut counted = Catalog { discord_checked: Some("2026-10-31".into()), ..catalog };
        counted.addons[0].downloads = Some(4_700);
        counted.addons[0].latest.downloads = Some(1_668);
        let back: Catalog = serde_json::from_str(&serde_json::to_string(&counted).unwrap()).unwrap();
        assert_eq!(back, counted);
    }

    #[test]
    fn copies_installed_by_hand_are_recognised_by_their_files() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameAddons::of(dir.path(), None);
        let lucky = game.addons.join("LuckyOdds");
        fs::create_dir_all(&lucky).unwrap();
        fs::write(lucky.join("LuckyOdds.toc"), "## Interface: 11200\n## Version: 0.6.0\n").unwrap();
        let print = tree::fingerprint_dir(&lucky).unwrap();
        let mut entry_release = release("v0.6.0", &[("LuckyOdds", &print)]);
        entry_release.sha256 = "6".repeat(64);
        let catalog = Catalog {
            format: 1,
            generated: Utc::now(),
            discord_checked: None,
            addons: vec![Entry {
                id: "lucky-odds".into(),
                name: "LuckyOdds".into(),
                repo: "cadoky/LuckyOdds".into(),
                author: "cadoky".into(),
                summary: "Shows the odds".into(),
                group: "Planners and journals".into(),
                folders: vec!["LuckyOdds".into()],
                asset: Asset::File { pattern: "LuckyOdds-v*.zip".into() },
                latest: release("v0.7.0", &[("LuckyOdds", "other")]),
                earlier: vec![entry_release],
                doesnt_mix_with: Vec::new(),
                notes: Vec::new(),
                downloads: None,
            }],
        };
        let found = super::super::scan(&game.addons).unwrap();
        let adopted = adopt(&catalog, &game, &Store::default(), &found, Utc::now());
        assert_eq!(adopted.len(), 1);
        assert_eq!(adopted[0].tag.as_deref(), Some("v0.6.0"));
        assert_eq!(adopted[0].sha256.as_deref(), Some("6".repeat(64).as_str()));
        assert_eq!(adopted[0].released, Some(Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()), "its release's date");
        assert_eq!(catalog.published("lucky-odds", "v0.6.0"), adopted[0].released);
        assert_eq!(catalog.published("lucky-odds", "v0.8.0"), None, "a release the catalog doesn't hold");
        assert_eq!(catalog.published("other", "v0.6.0"), None);
        // A copy someone edited is not adopted.
        fs::write(lucky.join("extra.lua"), "x").unwrap();
        let found = super::super::scan(&game.addons).unwrap();
        assert!(adopt(&catalog, &game, &Store::default(), &found, Utc::now()).is_empty());
    }
}
