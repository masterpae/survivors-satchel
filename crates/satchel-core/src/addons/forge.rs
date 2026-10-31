//! SurvivorsForge: Rezondes' registry of the game's addons. It scans every version, and its admins
//! approve exact files. The Satchel reads its public API for launchers and keeps its rules: the
//! registry at most once per 30 minutes with its ETag, an addon's details and versions only when a
//! player opens or installs it, a checksum before every install, never a blocked or changed file,
//! a question before a flagged or unscanned one. Its word can restrict our Discord Verified list,
//! and it is the only one that can permit its own addons (`decide`). Its answers are kept on this
//! PC (`Cache`). No network here: the app fetches; this reads, remembers and decides.

use super::catalog::{Catalog, Release};
use super::github::{percent_decode, repo_from_input, tag_version};
use super::overview::plain_notes;
use super::store::Badge;
use crate::{fsx, log};
use chrono::{DateTime, TimeDelta, Utc};
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Its API for launchers. Requests are built only from this and a slug that passes `slug_ok`.
pub const API: &str = "https://forgeapi.rezondes.net";
/// Its website and its repository: the credit its conditions ask for, in the app and the README.
pub const SITE: &str = "https://forge.rezondes.net";
pub const REPO: &str = "https://github.com/Rezondes/SurvivorsForge";
/// The registry's schema this Satchel reads. Within a schema fields are only ever added; a newer
/// one may mean something else, so it isn't read.
const SCHEMA: u64 = 1;
/// An About text, in characters.
const ABOUT_CHARS: usize = 600;
/// How much of a block reason is shown.
const REASON_CHARS: usize = 160;
const REGISTRY_FILE: &str = "registry.json";
const STATE_FILE: &str = "state.json";

/// The registry is asked for at most this often, counted from the last attempt, across restarts.
pub fn every() -> TimeDelta {
    TimeDelta::minutes(30)
}

/// An answer this recent means SurvivorsForge is online: two missed reads at that cadence.
pub fn fresh() -> TimeDelta {
    TimeDelta::minutes(60)
}

/// A slug SurvivorsForge could have given, `^[a-z0-9][a-z0-9-]{0,99}$`, and nothing else: only such
/// a slug goes into a request, a file name or a record id.
pub fn slug_ok(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    (1..=100).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// The id of an addon installed from SurvivorsForge alone: its record's, and its offer's in Browse.
pub fn id_for(slug: &str) -> String {
    format!("forge:{slug}")
}

/// The slug in such an id; `None` for any other id.
pub fn slug_in(id: &str) -> Option<&str> {
    id.strip_prefix("forge:").filter(|slug| slug_ok(slug))
}

// What SurvivorsForge sends -----------------------------------------------------------------------

/// A field as SurvivorsForge sent it, or its default when it is null or of another type. Schema 1
/// only grows, and one odd field mustn't cost the whole answer, only that field's say.
fn lenient<'de, D: Deserializer<'de>, T: Default + DeserializeOwned>(d: D) -> Result<T, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(d)?).unwrap_or_default())
}

/// A list without its odd items.
fn lenient_list<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(items) => items.into_iter().filter_map(|item| serde_json::from_value(item).ok()).collect(),
        _ => Vec::new(),
    })
}

/// `GET /v1/registry`: every addon neither deleted nor blocked, with its version to install. Read
/// it with `parse_registry`, which also refuses a newer schema and leaves odd entries out.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Registry {
    #[serde(default, deserialize_with = "lenient")]
    pub schema: u32,
    #[serde(default, deserialize_with = "lenient_list")]
    pub addons: Vec<ForgeAddon>,
}

impl Registry {
    pub fn get(&self, slug: &str) -> Option<&ForgeAddon> {
        self.addons.iter().find(|a| a.id == slug)
    }
}

/// One addon in the registry.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ForgeAddon {
    /// Its slug: stable, and the key to its other endpoints.
    #[serde(default, deserialize_with = "lenient")]
    pub id: String,
    #[serde(default, deserialize_with = "lenient")]
    pub name: String,
    /// The GitHub login, or the author's name until the author claims it.
    #[serde(default, deserialize_with = "lenient")]
    pub author: String,
    /// `owner/name` on GitHub.
    #[serde(default, deserialize_with = "lenient")]
    pub repo: String,
    /// One line.
    #[serde(default, deserialize_with = "lenient")]
    pub description: String,
    #[serde(default, deserialize_with = "lenient")]
    pub category: String,
    /// Every folder it installs.
    #[serde(default, deserialize_with = "lenient_list")]
    pub folders: Vec<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub source: Source,
    /// Its latest version's trust level.
    #[serde(default, deserialize_with = "lenient")]
    pub trust: Option<String>,
    /// An admin verified at least one of its versions.
    #[serde(default, deserialize_with = "lenient")]
    pub verified_author: Option<bool>,
    /// The version to install.
    #[serde(default, deserialize_with = "lenient")]
    pub latest: Option<Latest>,
}

impl ForgeAddon {
    /// Its trust level as its badge shows it: its latest version's, else the addon's.
    pub fn word(&self) -> Option<Word> {
        self.latest.as_ref().and_then(|l| l.trust.as_deref()).or(self.trust.as_deref()).map(Word::parse)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Source {
    /// `release` or `tag`.
    #[serde(rename = "type", default, deserialize_with = "lenient")]
    pub kind: String,
    /// The release file's name pattern.
    #[serde(default, deserialize_with = "lenient")]
    pub asset: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Latest {
    #[serde(default, deserialize_with = "lenient")]
    pub tag: String,
    /// The author's own GitHub release, by their rules; checked anyway (`asset_url_ok`).
    #[serde(default, deserialize_with = "lenient")]
    pub asset_url: String,
    /// Lowercase hex; `None`: not hashed yet, so not installed.
    #[serde(default, deserialize_with = "lenient")]
    pub sha256: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub size: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    pub trust: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub findings: Counts,
    #[serde(default, deserialize_with = "lenient")]
    pub game_min: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub game_max: Option<String>,
}

/// What a scan found, by severity: critical and warn make a version flagged; info counts as clean.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Counts {
    #[serde(default, deserialize_with = "lenient")]
    pub critical: u32,
    #[serde(default, deserialize_with = "lenient")]
    pub warn: u32,
    #[serde(default, deserialize_with = "lenient")]
    pub info: u32,
}

/// `GET /v1/addons/{slug}`: what a player reads before installing. A blocked addon sends only its
/// name, `blocked` and `block_reason`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Details {
    #[serde(default, deserialize_with = "lenient")]
    pub name: String,
    #[serde(default, deserialize_with = "lenient")]
    pub summary: String,
    #[serde(default, deserialize_with = "lenient")]
    pub description_md: String,
    #[serde(default, deserialize_with = "lenient")]
    pub category: String,
    #[serde(default, deserialize_with = "lenient")]
    pub owner: Option<Owner>,
    /// Its author claimed it on SurvivorsForge.
    #[serde(default, deserialize_with = "lenient")]
    pub claimed: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub verified_author: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    pub trust: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub latest_tag: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub blocked: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub block_reason: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub icon_url: Option<String>,
    /// Read, and unused until pictures come.
    #[serde(default, deserialize_with = "lenient_list")]
    pub images: Vec<Picture>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Owner {
    #[serde(default, deserialize_with = "lenient")]
    pub login: String,
    #[serde(default, deserialize_with = "lenient")]
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Picture {
    #[serde(default, deserialize_with = "lenient")]
    pub url: String,
    #[serde(default, deserialize_with = "lenient")]
    pub width: Option<u32>,
    #[serde(default, deserialize_with = "lenient")]
    pub height: Option<u32>,
    #[serde(default, deserialize_with = "lenient")]
    pub caption: Option<String>,
}

/// One version, from `GET /v1/addons/{slug}/versions` (newest first). Their rules apply to every
/// version, not only the latest.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Version {
    #[serde(default, deserialize_with = "lenient")]
    pub tag: String,
    #[serde(default, deserialize_with = "lenient")]
    pub prerelease: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub published_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "lenient")]
    pub asset_name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub asset_url: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub size: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    pub sha256: Option<String>,
    /// GitHub's count for its file.
    #[serde(default, deserialize_with = "lenient")]
    pub downloads: Option<u64>,
    /// `pending`, `running`, `failed` or `done`.
    #[serde(default, deserialize_with = "lenient")]
    pub scan_status: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub trust: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub findings: Counts,
    #[serde(default, deserialize_with = "lenient")]
    pub deleted_on_github: bool,
}

/// `GET /v1/addons/{slug}/versions/{tag}/findings`: one version's latest scan. Its findings are
/// read where the documentation's two readings put them: beside `scan`, or inside it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(from = "ScanBody")]
pub struct Scan {
    pub tag: String,
    pub scan_status: Option<String>,
    pub findings: Vec<Finding>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ScanBody {
    #[serde(default, deserialize_with = "lenient")]
    tag: String,
    #[serde(default, deserialize_with = "lenient")]
    scan_status: Option<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    findings: Vec<Finding>,
    #[serde(default, deserialize_with = "lenient")]
    scan: Option<ScanInner>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ScanInner {
    #[serde(default, deserialize_with = "lenient_list")]
    findings: Vec<Finding>,
}

impl From<ScanBody> for Scan {
    fn from(body: ScanBody) -> Scan {
        let findings = if body.findings.is_empty() { body.scan.map(|s| s.findings).unwrap_or_default() } else { body.findings };
        Scan { tag: body.tag, scan_status: body.scan_status, findings }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Finding {
    /// Like `lua.api_override`.
    #[serde(default, deserialize_with = "lenient")]
    pub rule: String,
    #[serde(default, deserialize_with = "lenient")]
    pub severity: String,
    #[serde(default, deserialize_with = "lenient")]
    pub file: String,
    #[serde(default, deserialize_with = "lenient")]
    pub line: Option<u32>,
    #[serde(default, deserialize_with = "lenient")]
    pub message: String,
}

impl Scan {
    /// Its findings, worst first (critical, warnings, notices, info), at most `most`.
    pub fn worst_first(&self, most: usize) -> Vec<&Finding> {
        let rank = |f: &&Finding| match f.severity.trim().to_ascii_lowercase().as_str() {
            "critical" => 0,
            "warn" | "warning" => 1,
            "notice" => 2,
            "info" => 3,
            _ => 4,
        };
        let mut findings: Vec<&Finding> = self.findings.iter().collect();
        findings.sort_by_key(rank);
        findings.truncate(most);
        findings
    }
}

/// An error answer: `{"error": code, "message": text}`, like a 404's `not_found`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ApiError {
    #[serde(default, deserialize_with = "lenient")]
    pub error: String,
    #[serde(default, deserialize_with = "lenient")]
    pub message: String,
}

/// The error an answer carries, when it is one.
pub fn api_error(body: &str) -> Option<ApiError> {
    error_of(&serde_json::from_str(body).ok()?)
}

fn error_of(value: &Value) -> Option<ApiError> {
    value.get("error")?.as_str()?;
    serde_json::from_value(value.clone()).ok()
}

/// An error answer is never read as what was asked for (a 404 never becomes an addon's details).
fn not_an_error(value: &Value) -> Result<(), String> {
    match error_of(value) {
        Some(e) => Err(format!("an error answer: {} ({})", e.error, e.message)),
        None => Ok(()),
    }
}

/// The first 200 characters of an answer, on one line, for the log.
fn head(body: &str) -> String {
    body.chars().take(200).map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// The registry, and each addon's entry fingerprint by slug. Refused: an answer without its list
/// of addons, or a newer schema. Left out: an entry that isn't an addon, or whose slug isn't one.
fn read_registry(body: &str) -> Result<(Registry, BTreeMap<String, String>), String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    not_an_error(&value)?;
    let schema = value.get("schema").and_then(Value::as_u64).unwrap_or(0);
    if schema > SCHEMA {
        return Err(format!("schema {schema}; this Satchel reads schema {SCHEMA}"));
    }
    let Some(Value::Array(items)) = value.get("addons") else { return Err("no list of addons".into()) };
    let mut addons = Vec::new();
    let mut prints = BTreeMap::new();
    for item in items {
        let Ok(addon) = serde_json::from_value::<ForgeAddon>(item.clone()) else { continue };
        if !slug_ok(&addon.id) || prints.contains_key(&addon.id) {
            continue;
        }
        prints.insert(addon.id.clone(), fingerprint(item));
        addons.push(addon);
    }
    Ok((Registry { schema: schema as u32, addons }, prints))
}

pub fn parse_registry(body: &str) -> Result<Registry, String> {
    read_registry(body).map(|(registry, _)| registry)
}

pub fn parse_details(body: &str) -> Result<Details, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    not_an_error(&value)?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// The versions endpoint's answer: a list, or an object holding it as `versions`. Odd items are
/// left out.
pub fn parse_versions(body: &str) -> Result<Vec<Version>, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    not_an_error(&value)?;
    let items = match value {
        Value::Array(items) => items,
        Value::Object(mut map) => match map.remove("versions") {
            Some(Value::Array(items)) => items,
            _ => return Err("no list of versions".into()),
        },
        _ => return Err("no list of versions".into()),
    };
    Ok(items.into_iter().filter_map(|item| serde_json::from_value::<Version>(item).ok()).filter(|v| !v.tag.is_empty()).collect())
}

pub fn parse_scan(body: &str) -> Result<Scan, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    not_an_error(&value)?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

// Its word, and the verdict ----------------------------------------------------------------------

/// A trust level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Word {
    /// An admin approved exactly this file.
    Verified,
    /// The scan found nothing; no admin looked.
    Scanned,
    /// The scan found something.
    Flagged,
    /// Not scanned yet, or the scan failed.
    Unscanned,
    Blocked,
    /// The file on GitHub is no longer the one scanned.
    SourceChanged,
}

impl Word {
    /// A word schema 1 may add later reads as `Unscanned`: ask first.
    pub fn parse(s: &str) -> Word {
        match s.trim().to_ascii_lowercase().as_str() {
            "verified" => Word::Verified,
            "scanned" => Word::Scanned,
            "flagged" => Word::Flagged,
            "blocked" => Word::Blocked,
            "source_changed" => Word::SourceChanged,
            _ => Word::Unscanned,
        }
    }

    /// The word as SurvivorsForge writes it, for its badge.
    pub fn as_str(self) -> &'static str {
        match self {
            Word::Verified => "verified",
            Word::Scanned => "scanned",
            Word::Flagged => "flagged",
            Word::Unscanned => "unscanned",
            Word::Blocked => "blocked",
            Word::SourceChanged => "source_changed",
        }
    }

    /// A word that holds a version back: anything but verified and scanned.
    pub fn restricts(self) -> bool {
        !matches!(self, Word::Verified | Word::Scanned)
    }
}

/// The registry's word on an addon's latest version: the latest's own, else the addon's; none at
/// all asks first.
fn latest_word(entry: &ForgeAddon, latest: &Latest) -> Word {
    latest.trust.as_deref().or(entry.trust.as_deref()).map_or(Word::Unscanned, Word::parse)
}

/// A version's word in its list; none asks first.
fn version_word(version: &Version) -> Word {
    version.trust.as_deref().map_or(Word::Unscanned, Word::parse)
}

/// SurvivorsForge's word on version `tag` of an addon, with its counts and its checksum: from the
/// registry when `tag` is its latest (the latest's trust, else the addon's), else from the addon's
/// versions as cached under its entry as it stands now; else nothing.
pub fn word_for(entry: Option<&ForgeAddon>, versions: Option<&[Version]>, tag: &str) -> Option<(Word, Counts, Option<String>)> {
    if tag.is_empty() {
        return None;
    }
    let sha = |given: &Option<String>| given.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_ascii_lowercase);
    if let Some(entry) = entry
        && let Some(latest) = entry.latest.as_ref().filter(|l| l.tag == tag)
    {
        return Some((latest_word(entry, latest), latest.findings, sha(&latest.sha256)));
    }
    let version = versions?.iter().find(|v| v.tag == tag)?;
    Some((version_word(version), version.findings, sha(&version.sha256)))
}

/// Where an addon comes from, for the verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin<'a> {
    /// Our Discord Verified list.
    OurList,
    /// SurvivorsForge alone: the registry's latest is what it installs.
    ForgeOnly { slug: &'a str },
    /// Added by link, or from a file.
    Other,
}

/// The origin of a record or a shared-list entry: a SurvivorsForge slug makes it SurvivorsForge's.
pub fn origin_of(badge: Badge, forge: Option<&str>) -> Origin<'_> {
    match (forge, badge) {
        (Some(slug), _) => Origin::ForgeOnly { slug },
        (None, Badge::Verified) => Origin::OurList,
        (None, _) => Origin::Other,
    }
}

/// What is known when deciding about one version of one addon.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts<'a> {
    /// The switch in Settings.
    pub switch_on: bool,
    /// An answer within the hour (`online`).
    pub online: bool,
    /// Its entry in the registry as last received.
    pub listed: Option<&'a ForgeAddon>,
    /// It left the registry, and its details haven't said why (`dropped`).
    pub dropped: bool,
    /// SurvivorsForge blocked it, with its reason.
    pub blocked: Option<&'a str>,
    /// The word on this version (`word_for`).
    pub word: Option<(Word, Counts)>,
    /// SurvivorsForge's checksum of this version.
    pub forge_sha: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Go,
    /// Never by itself; the player's OK installs it.
    Ask(Why),
    /// Never installed.
    No(Why),
}

impl Verdict {
    pub fn is_go(&self) -> bool {
        matches!(self, Verdict::Go)
    }

    pub fn why(&self) -> Option<&Why> {
        match self {
            Verdict::Go => None,
            Verdict::Ask(why) | Verdict::No(why) => Some(why),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    NotScanned,
    Flagged { critical: u32, warn: u32 },
    /// It left the registry: deleted or blocked, not known which yet.
    Dropped,
    Blocked(Option<String>),
    SourceChanged,
    /// SurvivorsForge's checksum for the version isn't ours.
    OtherFile,
    Off,
    Offline,
    NotListed,
    /// No checksum yet.
    NotHashed,
    /// Its download isn't on its own repository's GitHub releases.
    NotOnGitHub,
}

impl Why {
    /// The reason in the player's words.
    pub fn text(&self) -> String {
        let counted = |n: u32, what: &str| if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") };
        match self {
            Why::NotScanned => "SurvivorsForge hasn't scanned this version yet".into(),
            Why::Flagged { critical: 0, warn: 0 } => "SurvivorsForge's scan flagged this version".into(),
            Why::Flagged { critical, warn } => {
                let found = match (*critical, *warn) {
                    (0, warn) => counted(warn, "warning"),
                    (critical, 0) => counted(critical, "critical finding"),
                    (critical, warn) => format!("{} and {}", counted(critical, "critical finding"), counted(warn, "warning")),
                };
                format!("SurvivorsForge's scan found {found} in this version")
            }
            Why::Dropped | Why::NotListed => "SurvivorsForge no longer lists it".into(),
            Why::Blocked(reason) => match reason.as_deref().map(|r| one_line(r, REASON_CHARS)).filter(|r| !r.is_empty()) {
                Some(reason) => format!("SurvivorsForge blocked it: {reason}"),
                None => "SurvivorsForge blocked it".into(),
            },
            Why::SourceChanged => "The file changed after SurvivorsForge scanned it".into(),
            Why::OtherFile => "SurvivorsForge scanned a different file".into(),
            Why::Off => "SurvivorsForge is off in Settings".into(),
            Why::Offline => "SurvivorsForge isn't answering".into(),
            Why::NotHashed => "SurvivorsForge has no checksum for this version yet".into(),
            Why::NotOnGitHub => "Its download isn't on the addon's own GitHub".into(),
        }
    }
}

/// Someone else's text on one line, at most about `most` characters.
fn one_line(text: &str, most: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= most {
        return flat;
    }
    format!("{}…", flat.chars().take(most).collect::<String>().trim_end())
}

/// Whether version V of an addon may go in: one rule for every install, whatever starts it.
/// SurvivorsForge can restrict our Discord Verified list, and it is the only one that can permit
/// its own addons. `target_sha` is our checksum of V. A restricting word holds while SurvivorsForge
/// is silent (it comes from the cache), so an outage never unlocks anything. Go is needed for an
/// update by itself, but isn't enough: the caller's own conditions still apply.
pub fn decide(origin: Origin, target_sha: Option<&str>, facts: &Facts) -> Verdict {
    // Off, SurvivorsForge is ignored completely: our list and link-added addons as before, its
    // own addons without updates.
    if !facts.switch_on {
        return match origin {
            Origin::ForgeOnly { .. } => Verdict::No(Why::Off),
            _ => Verdict::Go,
        };
    }
    // For every addon: never a blocked one, never a file changed after its scan, never another
    // file than the one scanned.
    if let Some(reason) = facts.blocked {
        return Verdict::No(Why::Blocked(Some(reason.trim().to_string()).filter(|r| !r.is_empty())));
    }
    let word = facts.word.map(|(word, _)| word);
    let flagged = || facts.word.map_or(Why::Flagged { critical: 0, warn: 0 }, |(_, c)| Why::Flagged { critical: c.critical, warn: c.warn });
    match word {
        Some(Word::Blocked) => return Verdict::No(Why::Blocked(None)),
        Some(Word::SourceChanged) => return Verdict::No(Why::SourceChanged),
        _ => {}
    }
    if let (Some(theirs), Some(ours)) = (facts.forge_sha.filter(|s| !s.is_empty()), target_sha.filter(|s| !s.is_empty()))
        && !theirs.eq_ignore_ascii_case(ours)
    {
        return Verdict::No(Why::OtherFile);
    }
    match origin {
        Origin::Other => Verdict::Go,
        Origin::OurList => match word {
            Some(Word::Flagged) => Verdict::Ask(flagged()),
            Some(Word::Unscanned) => Verdict::Ask(Why::NotScanned),
            _ if facts.dropped => Verdict::Ask(Why::Dropped),
            _ => Verdict::Go,
        },
        Origin::ForgeOnly { slug } => {
            if !facts.online {
                return Verdict::No(Why::Offline);
            }
            let Some(entry) = facts.listed.filter(|e| e.id == slug) else { return Verdict::No(Why::NotListed) };
            let Some(latest) = &entry.latest else { return Verdict::No(Why::NotHashed) };
            // Its checksum and ours, both: nothing of SurvivorsForge's own goes in unchecked.
            if facts.forge_sha.is_none_or(str::is_empty) || target_sha.is_none_or(str::is_empty) {
                return Verdict::No(Why::NotHashed);
            }
            if !asset_url_ok(&latest.asset_url, &entry.repo) {
                return Verdict::No(Why::NotOnGitHub);
            }
            match word {
                Some(Word::Verified | Word::Scanned) => Verdict::Go,
                Some(Word::Flagged) => Verdict::Ask(flagged()),
                _ => Verdict::Ask(Why::NotScanned),
            }
        }
    }
}

// Reading the registry ---------------------------------------------------------------------------

/// Whether two `owner/name` repositories are the same one: GitHub doesn't tell case apart.
pub fn same_repo(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// Our catalog's addons that the registry lists too: catalog id → slug, by repository.
pub fn join(catalog: &Catalog, registry: &Registry) -> BTreeMap<String, String> {
    catalog
        .addons
        .iter()
        .filter_map(|entry| Some((entry.id.clone(), registry.addons.iter().find(|a| same_repo(&a.repo, &entry.repo))?.id.clone())))
        .collect()
}

/// Time for the registry: never asked, or asked `every()` ago. A last try in the future means the
/// clock went back, which is nothing to wait for.
pub fn due(s: &State, now: DateTime<Utc>) -> bool {
    s.tried.is_none_or(|tried| tried > now || now - tried >= every())
}

/// SurvivorsForge answered within the hour (an answer from the future doesn't count).
pub fn online(s: &State, now: DateTime<Utc>) -> bool {
    s.answered.is_some_and(|answered| answered <= now && now - answered < fresh())
}

/// The slug was listed on this PC before and isn't now, and its details haven't told why: deleted
/// or blocked, not known which. Without a registry nothing is known, so nothing dropped.
pub fn dropped(s: &State, registry: Option<&Registry>, slug: &str) -> bool {
    registry.is_some_and(|r| r.get(slug).is_none()) && s.listed.contains(slug) && !s.deleted.contains(slug) && !s.blocked.contains_key(slug)
}

/// SHA-256 of a registry entry with its keys sorted: the same entry, the same fingerprint, however
/// its keys come.
pub fn fingerprint(entry: &Value) -> String {
    fn canonical(value: &Value, out: &mut String) {
        match value {
            Value::Object(map) => {
                let mut fields: Vec<(&String, &Value)> = map.iter().collect();
                fields.sort_by(|a, b| a.0.cmp(b.0));
                out.push('{');
                for (i, (key, value)) in fields.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String(key.clone()).to_string());
                    out.push(':');
                    canonical(value, out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    canonical(item, out);
                }
                out.push(']');
            }
            scalar => out.push_str(&scalar.to_string()),
        }
    }
    let mut text = String::new();
    canonical(entry, &mut text);
    fsx::fingerprint(text.as_bytes())
}

/// Whether a download address is a file of `repo`'s own GitHub releases
/// (`https://github.com/<repo>/releases/download/…`) or its source zip
/// (`https://codeload.github.com/<repo>/…`): nothing else is downloaded for a SurvivorsForge addon.
pub fn asset_url_ok(url: &str, repo: &str) -> bool {
    let Some(repo) = repo_from_input(repo).filter(|r| r.eq_ignore_ascii_case(repo.trim())) else { return false };
    if url.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '\\' | '?' | '#' | '@')) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    let repo = repo.to_ascii_lowercase();
    let prefixes = [format!("https://github.com/{repo}/releases/download/"), format!("https://codeload.github.com/{repo}/")];
    let Some(rest) = prefixes.iter().find(|p| lower.starts_with(p.as_str())).map(|p| &url[p.len()..]) else { return false };
    let parts: Vec<String> = rest.split('/').map(percent_decode).collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p != "." && p != ".." && !p.contains(['/', '\\']))
}

/// The registry's latest version of an addon as a release: what a SurvivorsForge-only addon
/// installs and updates to. The registry carries no dates, so `published` is `seen`, when this PC
/// first saw it as latest; no folder fingerprints either. No checksum yet, or a download that isn't
/// its own GitHub's: no address, so nothing downloads it.
pub fn latest_release(entry: &ForgeAddon, seen: DateTime<Utc>) -> Option<Release> {
    let latest = entry.latest.as_ref().filter(|l| !l.tag.is_empty())?;
    let sha256 = latest.sha256.as_deref().map(str::trim).unwrap_or_default().to_ascii_lowercase();
    let usable = !sha256.is_empty() && asset_url_ok(&latest.asset_url, &entry.repo);
    Some(Release {
        tag: latest.tag.clone(),
        version: tag_version(&latest.tag),
        published: seen,
        prerelease: false,
        file: latest.asset_url.rsplit('/').next().map(percent_decode).unwrap_or_default(),
        url: if usable { latest.asset_url.clone() } else { String::new() },
        sha256,
        size: latest.size.unwrap_or(0),
        interface: 0,
        folders: BTreeMap::new(),
        notes: None,
        downloads: None,
    })
}

/// An addon's About text from its Markdown description: flattened like release notes, at about
/// 600 characters, and cut at the first heading on installing, usage, changes or credits.
pub fn about_text(md: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut fenced = false;
    for line in md.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        } else if !fenced && ends_about(trimmed) {
            break;
        }
        kept.push(line);
    }
    plain_notes(&without_tags(&without_pictures(&kept.join("\n"))), ABOUT_CHARS)
}

/// A heading named Install, Installation, Usage, Changelog or Credits (any case, any marks around).
fn ends_about(line: &str) -> bool {
    let hashes = line.len() - line.trim_start_matches('#').len();
    if !(1..=6).contains(&hashes) || !line[hashes..].chars().next().is_none_or(char::is_whitespace) {
        return false;
    }
    let words: String = line[hashes..].chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' }).collect();
    matches!(words.split_whitespace().next(), Some("install" | "installation" | "installing" | "usage" | "changelog" | "credits"))
}

/// Markdown pictures (`![words](address)`) out: an About text shows words.
fn without_pictures(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("![") {
        let tail = &rest[at..];
        // `![words]` right before `(address)`, on one line.
        let end = tail[2..]
            .find(']')
            .map(|close| close + 2)
            .filter(|&close| tail[close..].starts_with("]("))
            .and_then(|close| tail[close..].find(')').map(|e| close + e + 1));
        match end.filter(|&end| !tail[..end].contains('\n')) {
            Some(end) => {
                out.push_str(&rest[..at]);
                rest = &tail[end..];
            }
            None => {
                out.push_str(&rest[..at + 2]);
                rest = &tail[2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// HTML tags out (their words stay), as descriptions written for a web page have them.
fn without_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        let tail = &rest[at..];
        let tag = tail[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '!');
        match tail.find('>').filter(|&end| tag && end <= 300) {
            Some(end) => {
                out.push_str(&rest[..at]);
                rest = &tail[end + 1..];
            }
            None => {
                out.push_str(&rest[..at + 1]);
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// The cache on this PC ---------------------------------------------------------------------------

/// What this PC remembers about SurvivorsForge (`state.json` in the cache folder).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The registry's ETag, sent back as `If-None-Match`: only while the body it names is kept.
    pub etag: Option<String>,
    /// The last answer, 200 or 304.
    pub answered: Option<DateTime<Utc>>,
    /// The last request, answered or not.
    pub tried: Option<DateTime<Utc>>,
    /// Every slug ever listed on this PC: one that leaves the registry was deleted or blocked.
    pub listed: BTreeSet<String>,
    /// Slug → tag → when this PC first saw that tag as the latest.
    pub seen: BTreeMap<String, BTreeMap<String, DateTime<Utc>>>,
    /// Slugs that left the registry and whose details answered 404: deleted on SurvivorsForge.
    pub deleted: BTreeSet<String>,
    /// Slugs that left the registry because SurvivorsForge blocked them, with its reason.
    pub blocked: BTreeMap<String, String>,
    /// Each cached body → the fingerprint of the registry entry it was fetched under:
    /// `details/<slug>`, `versions/<slug>`, `findings/<slug>/<tag>`.
    pub fetched_under: BTreeMap<String, String>,
    /// Slug → its repository when last listed, so an addon of ours that leaves the registry keeps
    /// its slug and shows as dropped.
    pub repos: BTreeMap<String, String>,
    /// Slug → tag → a word that held that version back (flagged, unscanned, blocked,
    /// source_changed). It keeps holding it back after the answer that gave it stops being current,
    /// until a current answer says otherwise: old data can't let anything in, but still holds back.
    pub restrictions: BTreeMap<String, BTreeMap<String, Restriction>>,
}

/// A word that held a version back, as SurvivorsForge gave it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Restriction {
    /// `flagged`, `unscanned`, `blocked` or `source_changed`.
    pub trust: String,
    pub findings: Counts,
}

/// A current answer's word on one version: one that holds it back is kept, any other lifts what
/// was kept.
fn note_word(s: &mut State, slug: &str, tag: &str, word: Word, findings: Counts) {
    if word.restricts() {
        s.restrictions.entry(slug.to_string()).or_default().insert(tag.to_string(), Restriction { trust: word.as_str().to_string(), findings });
    } else if let Some(tags) = s.restrictions.get_mut(slug) {
        tags.remove(tag);
        if tags.is_empty() {
            s.restrictions.remove(slug);
        }
    }
}

/// An ETag worth keeping: one line of plain characters, not too long.
fn etag_ok(etag: &str) -> Option<String> {
    let etag = etag.trim();
    (!etag.is_empty() && etag.len() <= 256 && etag.chars().all(|c| (' '..='~').contains(&c))).then(|| etag.to_string())
}

/// SurvivorsForge's answers on this PC (`%LOCALAPPDATA%\SurvivorsSatchel\forge`): the registry,
/// and the details, versions and scans of addons opened here, each body exactly as received (so a
/// newer Satchel reads fields this one doesn't know), beside `state.json`. Only a cache: a file
/// that can't be read is dropped and fetched again, never kept aside. With the switch off it stays,
/// unused.
#[derive(Debug)]
pub struct Cache {
    dir: PathBuf,
    /// `state.json` as last read or written: a write that fails mustn't make the next minute ask
    /// again.
    state: Mutex<Option<State>>,
}

impl Cache {
    pub fn open(dir: PathBuf) -> Cache {
        Cache { dir, state: Mutex::new(None) }
    }

    /// What this PC remembers. The ETag only while the registry it names is kept: without the
    /// body, a 304 would leave nothing to read.
    pub fn state(&self) -> State {
        let kept = self.dir.join(REGISTRY_FILE).is_file();
        let mut memo = self.memo();
        let state = memo.get_or_insert_with(|| self.read_state());
        if !kept {
            state.etag = None;
        }
        state.clone()
    }

    /// The registry as last received, and each addon's entry fingerprint by slug.
    pub fn registry(&self) -> Option<(Registry, BTreeMap<String, String>)> {
        let path = self.dir.join(REGISTRY_FILE);
        let read = match self.body(&path) {
            Ok(None) => return None,
            Ok(Some(body)) => read_registry(&body),
            Err(why) => Err(why),
        };
        match read {
            Ok(read) => Some(read),
            Err(why) => {
                self.drop_file(&path, &why);
                self.change(|s| s.etag = None);
                None
            }
        }
    }

    /// A 200: the body as received, and the bookkeeping. A body that isn't a registry is an `Err`
    /// with its first 200 characters, and counts as a failed try.
    pub fn store_registry(&self, body: &str, etag: Option<&str>, now: DateTime<Utc>) -> Result<Registry, String> {
        let registry = match read_registry(body) {
            Ok((registry, _)) => registry,
            Err(why) => {
                self.change(|s| s.tried = Some(now));
                return Err(format!("SurvivorsForge's registry wasn't read ({why}); it began: {}", head(body)));
            }
        };
        let kept = self.write(&self.dir.join(REGISTRY_FILE), body);
        self.change(|s| {
            // An ETag stands for the body kept: none kept, none sent.
            s.etag = etag.filter(|_| kept).and_then(etag_ok);
            s.answered = Some(now);
            s.tried = Some(now);
            for addon in &registry.addons {
                s.listed.insert(addon.id.clone());
                s.repos.insert(addon.id.clone(), addon.repo.clone());
                s.deleted.remove(&addon.id);
                s.blocked.remove(&addon.id);
                if let Some(latest) = addon.latest.as_ref().filter(|l| !l.tag.is_empty()) {
                    s.seen.entry(addon.id.clone()).or_default().entry(latest.tag.clone()).or_insert(now);
                    note_word(s, &addon.id, &latest.tag, latest_word(addon, latest), latest.findings);
                }
            }
        });
        Ok(registry)
    }

    /// A 304: the registry kept is still the one.
    pub fn touch(&self, now: DateTime<Utc>) {
        self.change(|s| {
            s.answered = Some(now);
            s.tried = Some(now);
        });
    }

    /// No answer (a 429, a 5xx, the network): the next try waits.
    pub fn tried(&self, now: DateTime<Utc>) {
        self.change(|s| s.tried = Some(now));
    }

    /// An addon's details, when they were fetched under its registry entry as it stands now.
    pub fn details(&self, slug: &str) -> Option<Details> {
        self.current(&format!("details/{slug}"), slug, &self.details_path(slug)?, parse_details)
    }

    pub fn versions(&self, slug: &str) -> Option<Vec<Version>> {
        self.current(&format!("versions/{slug}"), slug, &self.versions_path(slug)?, parse_versions)
    }

    pub fn scan(&self, slug: &str, tag: &str) -> Option<Scan> {
        self.current(&format!("findings/{slug}/{tag}"), slug, &self.scan_path(slug, tag)?, parse_scan)
    }

    /// An addon's details as received. Blocked, it is remembered as blocked.
    pub fn store_details(&self, slug: &str, body: &str) -> Result<Details, String> {
        let path = self.details_path(slug).ok_or_else(|| format!("{slug:?} isn't a SurvivorsForge slug"))?;
        let details = parse_details(body).map_err(|why| format!("SurvivorsForge's details of {slug} weren't read ({why}); they began: {}", head(body)))?;
        self.keep(&path, &format!("details/{slug}"), slug, body);
        if details.blocked {
            self.mark_blocked(slug, details.block_reason.as_deref().unwrap_or_default());
        }
        Ok(details)
    }

    pub fn store_versions(&self, slug: &str, body: &str) -> Result<Vec<Version>, String> {
        let path = self.versions_path(slug).ok_or_else(|| format!("{slug:?} isn't a SurvivorsForge slug"))?;
        let versions = parse_versions(body).map_err(|why| format!("SurvivorsForge's versions of {slug} weren't read ({why}); they began: {}", head(body)))?;
        self.keep(&path, &format!("versions/{slug}"), slug, body);
        // A current list: its words are kept or lift what was kept; versions it doesn't name keep
        // what an older one said.
        self.change(|s| {
            for version in &versions {
                note_word(s, slug, &version.tag, version_word(version), version.findings);
            }
        });
        Ok(versions)
    }

    pub fn store_scan(&self, slug: &str, tag: &str, body: &str) -> Result<Scan, String> {
        let path = self.scan_path(slug, tag).ok_or_else(|| format!("{slug:?} isn't a SurvivorsForge slug"))?;
        let scan = parse_scan(body).map_err(|why| format!("SurvivorsForge's scan of {slug} {tag} wasn't read ({why}); it began: {}", head(body)))?;
        self.keep(&path, &format!("findings/{slug}/{tag}"), slug, body);
        Ok(scan)
    }

    /// A dropped addon's details answered 404: deleted on SurvivorsForge, so our rule again, and
    /// none of its words holds anything back. Kept until the slug is listed again.
    pub fn mark_deleted(&self, slug: &str) {
        if slug_ok(slug) {
            self.change(|s| {
                s.deleted.insert(slug.to_string());
                s.blocked.remove(slug);
                s.restrictions.remove(slug);
            });
        }
    }

    /// Blocked on SurvivorsForge, with its reason. Kept until the slug is listed again.
    pub fn mark_blocked(&self, slug: &str, reason: &str) {
        if slug_ok(slug) {
            self.change(|s| {
                s.blocked.insert(slug.to_string(), reason.to_string());
                s.deleted.remove(slug);
            });
        }
    }

    fn details_path(&self, slug: &str) -> Option<PathBuf> {
        slug_ok(slug).then(|| self.dir.join("details").join(format!("{slug}.json")))
    }

    fn versions_path(&self, slug: &str) -> Option<PathBuf> {
        slug_ok(slug).then(|| self.dir.join("versions").join(format!("{slug}.json")))
    }

    /// A tag can hold anything, so its file is named by its fingerprint.
    fn scan_path(&self, slug: &str, tag: &str) -> Option<PathBuf> {
        slug_ok(slug).then(|| self.dir.join("findings").join(slug).join(format!("{}.json", &fsx::fingerprint(tag.as_bytes())[..16])))
    }

    /// A cached body read with `parse`, when it was fetched under the addon's registry entry as it
    /// stands now. One that can't be read is dropped.
    fn current<T>(&self, key: &str, slug: &str, path: &Path, parse: impl Fn(&str) -> Result<T, String>) -> Option<T> {
        let under = self.state().fetched_under.get(key).cloned()?;
        let (_, prints) = self.registry()?;
        if prints.get(slug) != Some(&under) {
            return None;
        }
        let read = match self.body(path) {
            Ok(None) => return None,
            Ok(Some(body)) => parse(&body),
            Err(why) => Err(why),
        };
        match read {
            Ok(value) => Some(value),
            Err(why) => {
                self.drop_file(path, &why);
                self.change(|s| {
                    s.fetched_under.remove(key);
                });
                None
            }
        }
    }

    /// Writes a body, and the registry entry it was fetched under; with none (the addon isn't
    /// listed now), it is never read as current.
    fn keep(&self, path: &Path, key: &str, slug: &str, body: &str) {
        let print = self.registry().and_then(|(_, prints)| prints.get(slug).cloned());
        let kept = self.write(path, body);
        self.change(|s| match print.filter(|_| kept) {
            Some(print) => {
                s.fetched_under.insert(key.to_string(), print);
            }
            None => {
                s.fetched_under.remove(key);
            }
        });
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
            Err(e) => log!("SurvivorsForge's cache: its state wasn't written: {e}"),
        }
    }

    /// A cached file as text: `Ok(None)` when it isn't there (or can't be opened now), `Err` when
    /// it isn't text.
    fn body(&self, path: &Path) -> Result<Option<String>, String> {
        match fsx::read_optional(path) {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => String::from_utf8(bytes).map(Some).map_err(|_| "not text".to_string()),
            Err(e) => {
                log!("SurvivorsForge's cache: {} not read: {e}", path.display());
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
                log!("SurvivorsForge's cache: {} not written: {e}", path.display());
                false
            }
        }
    }

    fn drop_file(&self, path: &Path, why: &str) {
        log!("SurvivorsForge's cache: {} can't be read ({why}); dropped, to be fetched again", path.display());
        if let Err(e) = fsx::remove_if_present(path) {
            log!("SurvivorsForge's cache: {} not removed: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::fs;
    use Verdict::{Ask, Go, No};

    fn t(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 31, hour, 0, 0).unwrap()
    }

    /// A registry in the documented shape, written for these tests: an addon with odd fields, a
    /// verified one, a flagged one, and two entries that aren't addons.
    fn registry_body(meter_tag: &str, with_totems: bool) -> String {
        let totems = format!(
            r#"{{"id": "totems", "name": "Totems", "author": "Sam", "repo": "sam/Totems", "description": null,
      "category": "Shaman", "folders": ["Totems"], "source": {{"type": "release", "asset": null}}, "trust": "flagged",
      "latest": {{"tag": "v1.1", "asset_url": "https://github.com/sam/Totems/releases/download/v1.1/Totems.zip",
                 "sha256": "{b}", "size": null, "trust": "flagged", "findings": {{"critical": 1, "warn": 2}}}},
      "added_later": {{"anything": [1, 2]}}}},"#,
            b = "b".repeat(64)
        );
        format!(
            r#"{{
  "schema": 1,
  "addons": [
    {{"id": "fresh", "name": "Fresh", "repo": "sam/Fresh", "folders": "Fresh"}},
    {{"id": "meter", "name": "Meter", "author": "maker", "repo": "Maker/Meter", "description": "Counts what you hit",
      "category": "Combat", "folders": ["Meter"], "source": {{"type": "release", "asset": "Meter-*.zip"}},
      "trust": "verified", "verified_author": true,
      "latest": {{"tag": "{meter_tag}", "asset_url": "https://github.com/Maker/Meter/releases/download/{meter_tag}/Meter.zip",
                 "sha256": "{a}", "size": 2048, "trust": "verified", "findings": {{"critical": 0, "warn": 0}},
                 "game_min": null, "game_max": null}},
      "dependencies": []}},
    {totems}
    {{"id": "Not A Slug", "name": "Odd", "repo": "x/y"}},
    "not an addon"
  ]
}}"#,
            a = "a".repeat(64),
            totems = if with_totems { totems.as_str() } else { "" }
        )
    }

    #[test]
    fn the_registry_in_its_documented_shape() {
        let registry = parse_registry(&registry_body("v2.0", true)).unwrap();
        let ids: Vec<&str> = registry.addons.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["fresh", "meter", "totems"], "entries that aren't addons, or whose slug isn't one, are left out");
        let meter = registry.get("meter").unwrap();
        assert_eq!((meter.source.kind.as_str(), meter.source.asset.as_deref(), meter.verified_author), ("release", Some("Meter-*.zip"), Some(true)));
        let latest = meter.latest.as_ref().unwrap();
        assert_eq!((latest.tag.as_str(), latest.size, latest.sha256.clone()), ("v2.0", Some(2048), Some("a".repeat(64))));
        let totems = registry.get("totems").unwrap();
        assert_eq!(totems.description, "", "null reads as empty");
        assert_eq!(totems.latest.as_ref().unwrap().findings, Counts { critical: 1, warn: 2, info: 0 });
        let fresh = registry.get("fresh").unwrap();
        assert!(fresh.folders.is_empty() && fresh.latest.is_none(), "a field of another type reads as its default");
        assert_eq!((totems.word(), meter.word(), fresh.word()), (Some(Word::Flagged), Some(Word::Verified), None), "the badge's level");
        // Not a registry: refused, rather than read as an empty list that would drop every addon.
        assert!(parse_registry(r#"{"schema": 1}"#).is_err());
        assert!(parse_registry(r#"{"schema": 2, "addons": []}"#).unwrap_err().contains("schema 2"));
        assert!(parse_registry("error code: 523").is_err());
    }

    #[test]
    fn details_in_their_documented_shape() {
        let body = r#"{"slug": "totems", "name": "Totems", "summary": "Keeps your totems in view",
            "description_md": "Keeps them in view.", "description_html": "<p>Keeps them in view.</p>", "category": "Shaman",
            "owner": {"github_id": 7, "login": "sam", "avatar_url": null}, "claimed": true, "verified_author": false,
            "trust": "flagged", "latest_tag": "v1.1", "icon_url": null, "install_order": ["Totems"],
            "images": [{"id": 1, "width": 640, "height": 360, "position": 0, "url": "https://example.invalid/1.webp", "caption": "In a fight"}, 5],
            "created_at": "2026-10-01T10:00:00Z"}"#;
        let details = parse_details(body).unwrap();
        assert_eq!((details.summary.as_str(), details.owner.as_ref().map(|o| o.login.as_str())), ("Keeps your totems in view", Some("sam")));
        assert!(details.claimed && !details.blocked);
        assert_eq!((details.verified_author, details.latest_tag.as_deref()), (Some(false), Some("v1.1")));
        assert_eq!(details.images.len(), 1, "an odd picture is left out");
        assert_eq!(details.images[0].caption.as_deref(), Some("In a fight"));
        let blocked = parse_details(r#"{"slug": "gone", "name": "Gone", "blocked": true, "block_reason": "Sends your chat elsewhere"}"#).unwrap();
        assert!(blocked.blocked);
        assert_eq!(blocked.block_reason.as_deref(), Some("Sends your chat elsewhere"));
        assert!(parse_details("error code: 523").is_err());
    }

    #[test]
    fn versions_as_a_list_or_inside_an_object() {
        let list = format!(
            r#"[
            {{"id": 3, "tag": "v1.1", "name": "1.1", "prerelease": false, "published_at": "2026-10-30T09:00:00Z",
              "asset_name": "Totems.zip", "asset_url": "https://github.com/sam/Totems/releases/download/v1.1/Totems.zip",
              "size": 100, "sha256": "{b}", "downloads": 1668, "scan_status": "done", "source": "available",
              "deleted_on_github": false, "trust": "flagged", "findings": {{"critical": 1, "warn": 2, "info": 75}},
              "game_min": null, "game_max": null, "compatible": null}},
            {{"tag": "v1.0", "published_at": "yesterday", "downloads": null, "trust": "verified"}},
            {{"name": "no tag"}}
        ]"#,
            b = "b".repeat(64)
        );
        let versions = parse_versions(&list).unwrap();
        assert_eq!(versions.len(), 2, "a version without a tag is left out");
        assert_eq!(versions[0].published_at, Some(Utc.with_ymd_and_hms(2026, 10, 30, 9, 0, 0).unwrap()));
        assert_eq!((versions[0].downloads, versions[0].findings.info, versions[0].scan_status.as_deref()), (Some(1668), 75, Some("done")));
        assert_eq!((versions[1].published_at, versions[1].downloads), (None, None), "an odd date reads as none");
        assert_eq!(parse_versions(&format!(r#"{{"versions": {list}}}"#)).unwrap(), versions);
        assert!(parse_versions(r#"{"items": []}"#).is_err());
        assert!(parse_versions("error code: 523").is_err());
    }

    #[test]
    fn a_scan_with_its_findings_beside_it_or_inside() {
        let warn = r#"{"rule": "lua.api_override", "severity": "warn", "file": "Totems/Core.lua", "line": 27, "message": "Replaces a game function", "snippet": "x = 1"}"#;
        let info = r#"{"rule": "lua.global", "severity": "info", "file": "Totems/Core.lua", "line": null, "message": "Sets a global"}"#;
        let beside = format!(r#"{{"tag": "v1.1", "scan_status": "done", "scan": {{"result": "flagged", "scanner_version": "1.0"}}, "findings": [{info}, {warn}]}}"#);
        let inside = format!(r#"{{"tag": "v1.1", "scan_status": "done", "scan": {{"result": "flagged", "findings": [{info}, {warn}]}}}}"#);
        let scan = parse_scan(&beside).unwrap();
        assert_eq!(scan, parse_scan(&inside).unwrap());
        assert_eq!((scan.tag.as_str(), scan.findings.len()), ("v1.1", 2));
        let worst = scan.worst_first(5);
        assert_eq!((worst[0].rule.as_str(), worst[0].line, worst[1].line), ("lua.api_override", Some(27), None));
        assert_eq!(scan.worst_first(1).len(), 1);
        assert!(parse_scan(r#"{"tag": "v1.1", "scan_status": "pending", "scan": null}"#).unwrap().findings.is_empty());
    }

    #[test]
    fn trust_words() {
        let words = [
            ("verified", Word::Verified),
            ("scanned", Word::Scanned),
            ("flagged", Word::Flagged),
            ("unscanned", Word::Unscanned),
            ("blocked", Word::Blocked),
            ("source_changed", Word::SourceChanged),
            (" Verified ", Word::Verified),
        ];
        for (text, word) in words {
            assert_eq!(Word::parse(text), word, "{text}");
        }
        assert_eq!(Word::parse("quarantined"), Word::Unscanned, "a word added later asks first");
        assert_eq!(Word::Flagged.as_str(), "flagged");
    }

    #[test]
    fn its_word_from_the_registry_then_the_versions() {
        let registry = parse_registry(&registry_body("v2.0", true)).unwrap();
        let totems = registry.get("totems");
        let (word, counts, sha) = word_for(totems, None, "v1.1").unwrap();
        assert_eq!((word, counts.warn, sha), (Word::Flagged, 2, Some("b".repeat(64))));
        // The latest without a word of its own: the addon's; neither: ask first.
        let mut quiet = totems.unwrap().clone();
        quiet.latest.as_mut().unwrap().trust = None;
        quiet.trust = Some("scanned".into());
        assert_eq!(word_for(Some(&quiet), None, "v1.1").unwrap().0, Word::Scanned);
        quiet.trust = None;
        assert_eq!(word_for(Some(&quiet), None, "v1.1").unwrap().0, Word::Unscanned);
        // Another version: from its versions, when they are cached.
        let versions = parse_versions(&format!(r#"[{{"tag": "v1.0", "trust": "verified", "sha256": "{}"}}]"#, "C".repeat(64))).unwrap();
        assert_eq!(word_for(totems, Some(versions.as_slice()), "v1.0"), Some((Word::Verified, Counts::default(), Some("c".repeat(64)))));
        assert_eq!(word_for(totems, None, "v1.0"), None, "nothing known about it");
        assert_eq!(word_for(None, Some(versions.as_slice()), "v0.9"), None);
        assert_eq!(word_for(None, None, "v1.1"), None);
    }

    fn listed(url: &str) -> ForgeAddon {
        ForgeAddon {
            id: "totems".into(),
            name: "Totems".into(),
            repo: "sam/Totems".into(),
            folders: vec!["Totems".into()],
            latest: Some(Latest { tag: "v1.1".into(), asset_url: url.into(), sha256: Some("c".repeat(64)), ..Latest::default() }),
            ..ForgeAddon::default()
        }
    }

    #[test]
    fn the_verdict_table() {
        let ours = "c".repeat(64);
        let other = "d".repeat(64);
        let sha = Some(ours.as_str());
        let good = listed("https://github.com/sam/Totems/releases/download/v1.1/Totems.zip");
        let elsewhere = listed("https://example.com/Totems.zip");
        let said = |word: &str| Some((Word::parse(word), Counts { critical: 0, warn: 2, info: 0 }));
        let on = Facts { switch_on: true, online: true, ..Facts::default() };
        let mine = Facts { listed: Some(&good), forge_sha: sha, ..on };
        let (list, forge, other_origin) = (Origin::OurList, Origin::ForgeOnly { slug: "totems" }, Origin::Other);
        let flagged = Why::Flagged { critical: 0, warn: 2 };
        let reason = "Sends your chat elsewhere";
        let rows: Vec<(Origin, Facts, Verdict)> = vec![
            // Any origin: blocked, changed after its scan, another file than the one scanned: never.
            (list, Facts { blocked: Some(reason), word: said("verified"), ..on }, No(Why::Blocked(Some(reason.into())))),
            (forge, Facts { blocked: Some(reason), word: said("verified"), ..mine }, No(Why::Blocked(Some(reason.into())))),
            (other_origin, Facts { blocked: Some(" "), ..on }, No(Why::Blocked(None))),
            (list, Facts { word: said("blocked"), ..on }, No(Why::Blocked(None))),
            (list, Facts { word: said("source_changed"), ..on }, No(Why::SourceChanged)),
            (forge, Facts { word: said("source_changed"), ..mine }, No(Why::SourceChanged)),
            (other_origin, Facts { word: said("source_changed"), ..on }, No(Why::SourceChanged)),
            (list, Facts { word: said("verified"), forge_sha: Some(&other), ..on }, No(Why::OtherFile)),
            (forge, Facts { word: said("verified"), forge_sha: Some(&other), ..mine }, No(Why::OtherFile)),
            (other_origin, Facts { word: said("scanned"), forge_sha: Some(&other), ..on }, No(Why::OtherFile)),
            // Our Discord Verified list: SurvivorsForge can only hold it back.
            (list, Facts { word: said("flagged"), forge_sha: sha, ..on }, Ask(flagged.clone())),
            (list, Facts { word: said("unscanned"), ..on }, Ask(Why::NotScanned)),
            (list, Facts { word: said("quarantined"), ..on }, Ask(Why::NotScanned)),
            (list, Facts { word: said("verified"), forge_sha: sha, ..on }, Go),
            (list, Facts { word: said("scanned"), ..on }, Go),
            (list, on, Go),
            (list, Facts { dropped: true, ..on }, Ask(Why::Dropped)),
            // SurvivorsForge's own addons: only its word lets them in.
            (forge, Facts { switch_on: false, word: said("verified"), ..mine }, No(Why::Off)),
            (forge, Facts { online: false, word: said("verified"), ..mine }, No(Why::Offline)),
            (forge, Facts { listed: None, dropped: true, ..mine }, No(Why::NotListed)),
            (forge, Facts { word: said("verified"), forge_sha: None, ..mine }, No(Why::NotHashed)),
            (forge, Facts { listed: Some(&elsewhere), word: said("verified"), ..mine }, No(Why::NotOnGitHub)),
            (forge, Facts { word: said("flagged"), ..mine }, Ask(flagged.clone())),
            (forge, Facts { word: said("unscanned"), ..mine }, Ask(Why::NotScanned)),
            (forge, Facts { word: said("quarantined"), ..mine }, Ask(Why::NotScanned)),
            (forge, Facts { word: said("verified"), ..mine }, Go),
            (forge, Facts { word: said("scanned"), ..mine }, Go),
            // Added by link or from a file: otherwise as in 0.7.0 (they ask first already).
            (other_origin, Facts { word: said("flagged"), ..on }, Go),
            (other_origin, Facts { dropped: true, ..on }, Go),
            (other_origin, on, Go),
        ];
        for (i, (origin, facts, verdict)) in rows.iter().enumerate() {
            assert_eq!(&decide(*origin, sha, facts), verdict, "row {i}");
            // Silent for an hour: a word that holds back holds, from the cache, and nothing else
            // changes, except for SurvivorsForge's own addons.
            if *origin != forge {
                assert_eq!(&decide(*origin, sha, &Facts { online: false, ..*facts }), verdict, "row {i}, offline");
            }
        }
        // Off, SurvivorsForge is ignored completely, whatever it said.
        let off = Facts { switch_on: false, blocked: Some(reason), word: said("blocked"), dropped: true, forge_sha: Some(&other), ..mine };
        assert_eq!(decide(list, sha, &off), Go);
        assert_eq!(decide(other_origin, sha, &off), Go);
        assert_eq!(decide(forge, sha, &off), No(Why::Off));
        // Silent for an hour, a block it gave says more than the silence.
        assert_eq!(decide(forge, sha, &Facts { online: false, blocked: Some(reason), ..mine }), No(Why::Blocked(Some(reason.into()))));
        // An entry under another slug isn't this addon's.
        assert_eq!(decide(Origin::ForgeOnly { slug: "other" }, sha, &Facts { word: said("verified"), ..mine }), No(Why::NotListed));
        // No checksum of ours to compare (GitHub's page gave none): nothing to tell apart.
        assert_eq!(decide(list, None, &Facts { word: said("verified"), forge_sha: Some(&other), ..on }), Go);
        // But SurvivorsForge's own addon needs ours too (a list entry with only a tag): never
        // installed unchecked.
        assert_eq!(decide(forge, None, &Facts { word: said("verified"), ..mine }), No(Why::NotHashed));
        assert_eq!(decide(forge, Some(""), &Facts { word: said("verified"), ..mine }), No(Why::NotHashed));
    }

    #[test]
    fn the_shapes_survivorsforge_answers_with_read() {
        // Written for this test in the shapes SurvivorsForge's API answered with on 31 Oct 2026.
        let versions = format!(
            r#"[{{"id": 41, "name": "v1.1", "tag": "v1.1", "prerelease": false, "published_at": "2026-10-31T07:26:37.607641+00:00",
              "asset_name": "Totems.zip", "asset_url": "https://github.com/sam/Totems/releases/download/v1.1/Totems.zip",
              "size": null, "sha256": "{b}", "downloads": 12, "scan_status": "done", "source": "available",
              "deleted_on_github": false, "trust": "scanned", "findings": {{"critical": 0, "info": 3, "warn": 0}},
              "game_min": null, "game_max": null, "compatible": null}}]"#,
            b = "b".repeat(64)
        );
        let read = parse_versions(&versions).unwrap();
        let published = Utc.with_ymd_and_hms(2026, 10, 31, 7, 26, 37).unwrap() + TimeDelta::microseconds(607_641);
        assert_eq!((read.len(), read[0].published_at, read[0].size, read[0].downloads), (1, Some(published), None, Some(12)));
        assert_eq!(read[0].findings, Counts { critical: 0, warn: 0, info: 3 });
        assert_eq!(word_for(None, Some(read.as_slice()), "v1.1"), Some((Word::Scanned, read[0].findings, Some("b".repeat(64)))));
        let details = r#"{"slug": "totems", "summary": "Keeps your totems in view", "description_md": "Keeps them **in view**.",
            "description_html": "<p>Keeps them <strong>in view</strong>.</p>", "icon_url": null,
            "images": [{"id": 3, "url": "https://example.invalid/3.webp", "width": 800, "height": 450, "position": 0, "caption": null}],
            "blocked": false, "owner": {"avatar_url": "https://example.invalid/a.png", "github_id": 7, "login": "sam"},
            "claimed": true, "trust": "scanned", "latest_tag": "v1.1", "created_at": "2026-10-20T10:00:00.5+00:00",
            "updated_at": "2026-10-31T07:26:37.607641+00:00", "findings": {"critical": 0, "info": 3, "warn": 0},
            "folders": ["Totems"], "dependencies": [], "install_order": ["Totems"], "contributors": [],
            "asset_pattern": "Totems.zip", "source_type": "release"}"#;
        let details = parse_details(details).unwrap();
        assert_eq!((details.summary.as_str(), details.latest_tag.as_deref(), details.trust.as_deref()), ("Keeps your totems in view", Some("v1.1"), Some("scanned")));
        assert_eq!((details.blocked, details.block_reason.as_deref(), details.owner.as_ref().map(|o| o.login.as_str())), (false, None, Some("sam")));
        assert_eq!((details.images.len(), details.icon_url.as_deref(), about_text(&details.description_md).as_str()), (1, None, "Keeps them in view."));
        // A 404 reads as SurvivorsForge's error, and never as an addon's details or versions.
        let missing = r#"{"error":"not_found","message":"Not found"}"#;
        assert_eq!(api_error(missing), Some(ApiError { error: "not_found".into(), message: "Not found".into() }));
        assert!(parse_details(missing).unwrap_err().contains("not_found"));
        assert!(parse_versions(missing).is_err() && parse_scan(missing).is_err() && parse_registry(missing).is_err());
        assert_eq!(api_error(&versions), None);
    }

    #[test]
    fn a_word_that_held_a_version_back_holds_until_a_current_answer_lifts_it() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("forge");
        let cache = Cache::open(folder.clone());
        let held = |slug: &str, tag: &str| cache.state().restrictions.get(slug).and_then(|tags| tags.get(tag)).map(|r| r.trust.clone());
        // The registry: Totems' latest v1.1 is flagged, Meter's verified.
        cache.store_registry(&registry_body("v2.0", true), None, t(12)).unwrap();
        assert_eq!(held("totems", "v1.1").as_deref(), Some("flagged"));
        assert_eq!(cache.state().restrictions["totems"]["v1.1"].findings, Counts { critical: 1, warn: 2, info: 0 });
        assert_eq!(held("meter", "v2.0"), None, "a verified latest holds nothing back");
        // Its versions: v1.0 unscanned, v0.9 verified.
        cache.store_versions("totems", r#"[{"tag": "v1.0", "trust": "unscanned"}, {"tag": "v0.9", "trust": "verified"}]"#).unwrap();
        assert_eq!((held("totems", "v1.0").as_deref(), held("totems", "v0.9")), (Some("unscanned"), None), "a word that lets in is never kept");
        // Totems' entry changes (its latest is now v1.2, verified): that list stops being current,
        // and the words that held versions back keep holding them back.
        let moved = registry_body("v2.0", true).replace("\"v1.1\"", "\"v1.2\"").replace("/v1.1/", "/v1.2/").replace("\"flagged\"", "\"verified\"");
        cache.store_registry(&moved, None, t(13)).unwrap();
        assert!(cache.versions("totems").is_none(), "the list is stale");
        assert_eq!((held("totems", "v1.1").as_deref(), held("totems", "v1.0").as_deref()), (Some("flagged"), Some("unscanned")));
        assert_eq!(held("totems", "v1.2"), None);
        assert_eq!(Cache::open(folder.clone()).state().restrictions, cache.state().restrictions, "kept across a restart");
        // A current list says v1.1 is verified now: lifted. It doesn't name v1.0: still held.
        cache.store_versions("totems", r#"{"versions": [{"tag": "v1.1", "trust": "verified"}]}"#).unwrap();
        assert_eq!((held("totems", "v1.1"), held("totems", "v1.0").as_deref()), (None, Some("unscanned")));
        // Deleted on SurvivorsForge: our rule again, and nothing of it holds anything back.
        cache.mark_deleted("totems");
        assert!(!cache.state().restrictions.contains_key("totems"));
    }

    #[test]
    fn the_cadence_and_online() {
        let mut s = State::default();
        assert!(due(&s, t(12)) && !online(&s, t(12)), "never asked: due, and not online");
        s.tried = Some(t(12));
        s.answered = Some(t(12));
        let at = |minutes: i64| t(12) + TimeDelta::minutes(minutes);
        assert!(!due(&s, at(29)) && due(&s, at(30)), "at most once per 30 minutes");
        assert!(online(&s, at(59)) && !online(&s, at(60)), "online for an hour after an answer");
        // A failed try waits too; the last answer stays as it was.
        s.tried = Some(at(30));
        assert!(!due(&s, at(59)) && due(&s, at(60)) && !online(&s, at(60)));
        // The clock went back: a try "in the future" is nothing to wait for, an answer from there
        // no sign of life.
        assert!(due(&s, t(11)) && !online(&s, t(11)));
    }

    #[test]
    fn the_registry_kept_with_its_bookkeeping() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("forge");
        let cache = Cache::open(folder.clone());
        assert_eq!(cache.state(), State::default());
        assert!(cache.registry().is_none());
        let body = registry_body("v2.0", true);
        let registry = cache.store_registry(&body, Some("\"r1\""), t(12)).unwrap();
        assert_eq!(registry.addons.len(), 3);
        assert_eq!(fs::read_to_string(folder.join("registry.json")).unwrap(), body, "kept exactly as received");
        let s = cache.state();
        assert_eq!((s.etag.as_deref(), s.answered, s.tried), (Some("\"r1\""), Some(t(12)), Some(t(12))));
        assert_eq!(s.listed.iter().map(String::as_str).collect::<Vec<_>>(), ["fresh", "meter", "totems"]);
        assert_eq!((s.seen["meter"]["v2.0"], s.seen["totems"]["v1.1"]), (t(12), t(12)));
        assert!(!s.seen.contains_key("fresh"), "no latest, nothing seen");
        assert_eq!(s.repos["meter"], "Maker/Meter");
        // After a restart all of it is still there.
        let cache = Cache::open(folder.clone());
        assert_eq!(cache.state(), s);
        assert_eq!(cache.registry().unwrap().0, registry);

        // A 304, then no answer.
        cache.touch(t(13));
        cache.tried(t(14));
        let s = cache.state();
        assert_eq!((s.answered, s.tried), (Some(t(13)), Some(t(14))));

        // Meter at a new tag, Totems gone: the new tag seen, the old one kept; Totems dropped.
        cache.store_registry(&registry_body("v2.1", false), None, t(15)).unwrap();
        let s = cache.state();
        assert_eq!((s.seen["meter"]["v2.0"], s.seen["meter"]["v2.1"]), (t(12), t(15)));
        assert_eq!(s.etag, None, "no ETag given, none kept");
        let (listed_now, _) = cache.registry().unwrap();
        assert!(dropped(&s, Some(&listed_now), "totems"));
        assert!(!dropped(&s, Some(&listed_now), "meter"), "still listed");
        assert!(!dropped(&s, Some(&listed_now), "elsewhere"), "never listed here");
        assert!(!dropped(&s, None, "totems"), "without a registry nothing is known");
        // Its details answered 404: deleted. Then it turns out to be blocked.
        cache.mark_deleted("totems");
        assert!(!dropped(&cache.state(), Some(&listed_now), "totems"));
        cache.mark_blocked("totems", "Sends your chat elsewhere");
        let s = cache.state();
        assert!(!s.deleted.contains("totems") && !dropped(&s, Some(&listed_now), "totems"));
        assert_eq!(s.blocked["totems"], "Sends your chat elsewhere");
        // Listed again: both forgotten, and when its tag was first seen stays.
        cache.store_registry(&registry_body("v2.1", true), None, t(16)).unwrap();
        let s = cache.state();
        assert!(s.blocked.is_empty() && s.deleted.is_empty());
        assert_eq!(s.seen["totems"]["v1.1"], t(12));

        // An answer that isn't a registry: refused with what it began with, counted as a try.
        let err = cache.store_registry("<html>error code: 523</html>", Some("\"x\""), t(17)).unwrap_err();
        assert!(err.contains("error code: 523"), "{err}");
        let after = cache.state();
        assert_eq!((after.tried, after.answered), (Some(t(17)), Some(t(16))));
        assert_eq!(cache.registry().unwrap().0.addons.len(), 3, "the registry kept stays");
    }

    #[test]
    fn cached_bodies_count_under_their_entry_as_it_stands() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path().join("forge"));
        cache.store_registry(&registry_body("v2.0", true), None, t(12)).unwrap();
        let versions = r#"[{"tag": "v1.1", "trust": "flagged", "downloads": 3}]"#;
        assert_eq!(cache.store_versions("totems", versions).unwrap().len(), 1);
        cache.store_versions("meter", r#"{"versions": [{"tag": "v2.0", "trust": "verified"}]}"#).unwrap();
        cache.store_details("totems", r#"{"name": "Totems", "summary": "Keeps your totems in view"}"#).unwrap();
        cache.store_scan("totems", "v1.1", r#"{"tag": "v1.1", "findings": [{"rule": "lua.global", "severity": "info"}]}"#).unwrap();
        assert_eq!(cache.versions("totems").unwrap()[0].downloads, Some(3));
        assert_eq!(cache.details("totems").unwrap().summary, "Keeps your totems in view");
        assert_eq!(cache.scan("totems", "v1.1").unwrap().findings.len(), 1);
        assert!(cache.scan("totems", "v1.0").is_none() && cache.details("meter").is_none(), "never fetched");

        // The same registry with its keys in another order: still current.
        let sorted = serde_json::to_string(&serde_json::from_str::<Value>(&registry_body("v2.0", true)).unwrap()).unwrap();
        assert_ne!(sorted, registry_body("v2.0", true));
        cache.store_registry(&sorted, None, t(13)).unwrap();
        assert!(cache.versions("totems").is_some() && cache.details("totems").is_some());

        // Totems' entry changes (a new version): what was fetched under the old one is stale;
        // Meter's stays.
        let changed = registry_body("v2.0", true).replace("\"v1.1\"", "\"v1.2\"").replace("/v1.1/", "/v1.2/");
        cache.store_registry(&changed, None, t(14)).unwrap();
        assert!(cache.versions("totems").is_none() && cache.details("totems").is_none() && cache.scan("totems", "v1.1").is_none());
        assert_eq!(cache.versions("meter").unwrap()[0].trust.as_deref(), Some("verified"));
        // Fetched again under the new entry: current again.
        cache.store_versions("totems", versions).unwrap();
        assert!(cache.versions("totems").is_some());

        // A slug SurvivorsForge couldn't have given never reaches a file name.
        assert!(cache.store_versions("../totems", versions).is_err());
        assert!(cache.versions("../totems").is_none() && cache.details("Totems").is_none());
        // A body that isn't one: refused, with what it began with.
        assert!(cache.store_details("totems", "error code: 523").unwrap_err().contains("error code: 523"));
        // Details saying it's blocked: remembered as blocked.
        cache.store_details("meter", r#"{"name": "Meter", "blocked": true, "block_reason": "Not its author's"}"#).unwrap();
        assert_eq!(cache.state().blocked["meter"], "Not its author's");
    }

    #[test]
    fn an_unreadable_cache_file_is_dropped_and_never_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("forge");
        let cache = Cache::open(folder.clone());
        cache.store_registry(&registry_body("v2.0", true), Some("\"r1\""), t(12)).unwrap();
        cache.store_versions("totems", r#"[{"tag": "v1.1"}]"#).unwrap();
        let names = || {
            let mut names: Vec<String> = fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            names
        };
        assert_eq!(names(), ["registry.json", "state.json", "versions"]);

        // Versions that don't read: dropped, so they are fetched again.
        let versions = folder.join("versions").join("totems.json");
        fs::write(&versions, "{ not json").unwrap();
        assert!(cache.versions("totems").is_none());
        assert!(!versions.exists() && !cache.state().fetched_under.contains_key("versions/totems"));

        // The registry: dropped, and its ETag with it, so the next answer is a whole one.
        fs::write(folder.join("registry.json"), b"\xff\xfe\x00").unwrap();
        assert!(cache.registry().is_none());
        assert_eq!(cache.state().etag, None);
        assert_eq!(names(), ["state.json", "versions"], "nothing renamed or kept aside");

        // The state, read at the next start: dropped too.
        fs::write(folder.join("state.json"), "{ not json").unwrap();
        assert_eq!(Cache::open(folder.clone()).state(), State::default());
        assert_eq!(names(), ["versions"]);
    }

    #[test]
    fn an_entrys_fingerprint_ignores_the_order_of_its_keys() {
        let entry = |text: &str| fingerprint(&serde_json::from_str::<Value>(text).unwrap());
        let a = entry(r#"{"id": "x", "latest": {"tag": "v1", "findings": {"warn": 0, "critical": 1}}, "folders": ["A", "B"], "n": null}"#);
        let b = entry(r#"{"n": null, "folders": ["A", "B"], "latest": {"findings": {"critical": 1, "warn": 0}, "tag": "v1"}, "id": "x"}"#);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        let changed = entry(r#"{"id": "x", "latest": {"tag": "v2", "findings": {"warn": 0, "critical": 1}}, "folders": ["A", "B"], "n": null}"#);
        let reordered_list = entry(r#"{"id": "x", "latest": {"tag": "v1", "findings": {"warn": 0, "critical": 1}}, "folders": ["B", "A"], "n": null}"#);
        assert_ne!(a, changed, "a value changed");
        assert_ne!(a, reordered_list, "a list's order counts");
    }

    #[test]
    fn our_catalog_joined_by_repository_without_case() {
        let catalog = Catalog::builtin();
        let ours = &catalog.addons[0];
        let body = format!(
            r#"{{"schema": 1, "addons": [{{"id": "joined", "name": "Ours", "repo": "{}"}}, {{"id": "theirs", "name": "Theirs", "repo": "someone/Theirs"}}]}}"#,
            ours.repo.to_uppercase()
        );
        let registry = parse_registry(&body).unwrap();
        assert_eq!(join(&catalog, &registry), BTreeMap::from([(ours.id.clone(), "joined".to_string())]));
    }

    #[test]
    fn downloads_only_from_the_addons_own_github() {
        let repo = "sam/Totems";
        for url in [
            "https://github.com/sam/Totems/releases/download/v1.1/Totems.zip",
            "https://github.com/SAM/totems/releases/download/v1.1/Totems%20v1.1.zip",
            "https://codeload.github.com/sam/Totems/zip/refs/tags/v1.1",
        ] {
            assert!(asset_url_ok(url, repo), "{url}");
        }
        for url in [
            "https://github.com/other/Totems/releases/download/v1.1/Totems.zip",
            "https://github.com/sam/Totems2/releases/download/v1.1/Totems.zip",
            "https://codeload.github.com/sam/TotemsX/zip/v1.1",
            "http://github.com/sam/Totems/releases/download/v1.1/Totems.zip",
            "https://example.com/sam/Totems/releases/download/v1.1/Totems.zip",
            "https://github.com.example.com/sam/Totems/releases/download/v1.1/Totems.zip",
            "https://github.com/sam/Totems/archive/refs/tags/v1.1.zip",
            "https://github.com/sam/Totems/releases/download/v1.1/",
            "https://github.com/sam/Totems/releases/download/v1.1/../../../other/x.zip",
            "https://github.com/sam/Totems/releases/download/v1.1/%2e%2e/x.zip",
            "https://github.com/sam/Totems/releases/download/v1.1/Totems.zip?x=1",
            "https://user@github.com/sam/Totems/releases/download/v1.1/Totems.zip",
            "",
        ] {
            assert!(!asset_url_ok(url, repo), "{url}");
        }
        assert!(!asset_url_ok("https://github.com/sam/Totems/releases/download/v1.1/Totems.zip", "sam/Totems/extra"));
        assert!(!asset_url_ok("https://github.com/sam/../releases/download/v1/x.zip", "sam/.."));
    }

    #[test]
    fn the_registrys_latest_as_a_release() {
        let registry = parse_registry(&registry_body("v2.0", true)).unwrap();
        let meter = registry.get("meter").unwrap();
        let release = latest_release(meter, t(9)).unwrap();
        assert_eq!((release.tag.as_str(), release.version.as_deref(), release.file.as_str()), ("v2.0", Some("2.0"), "Meter.zip"));
        assert_eq!(release.url, "https://github.com/Maker/Meter/releases/download/v2.0/Meter.zip");
        assert_eq!((release.sha256.clone(), release.published), ("a".repeat(64), t(9)));
        assert_eq!((release.size, release.interface, release.folders.len(), release.downloads), (2048, 0, 0, None));
        // No checksum yet, or a download from elsewhere: nothing downloads it.
        let mut unhashed = meter.clone();
        unhashed.latest.as_mut().unwrap().sha256 = None;
        assert_eq!(latest_release(&unhashed, t(9)).unwrap().url, "");
        let mut elsewhere = meter.clone();
        elsewhere.latest.as_mut().unwrap().asset_url = "https://example.com/Meter.zip".into();
        assert_eq!(latest_release(&elsewhere, t(9)).unwrap().url, "");
        assert!(latest_release(registry.get("fresh").unwrap(), t(9)).is_none(), "no latest, nothing to install");
    }

    #[test]
    fn about_texts_stop_where_installing_starts() {
        let md = "# Totems\n\n![A fight](https://example.invalid/shot.png)\nKeeps your **totems** in view, see [the wiki](https://example.invalid/w).\n\n<p align=\"center\">Works with <b>any</b> UI</p>\n\n```\n# install with care\n```\n\n## Features\n- Timers\n- Sounds\n\n## 📦 Installation\nUnzip it into AddOns.\n\n## Credits\nSam";
        assert_eq!(
            about_text(md),
            "Totems\n\nKeeps your totems in view, see the wiki.\n\nWorks with any UI\n\ninstall with care\n\nFeatures\n• Timers\n• Sounds"
        );
        assert_eq!(about_text("Short.\n#install is a tag here\n### Usage\n/totems"), "Short.\ninstall is a tag here");
        let long = about_text(&"word ".repeat(300));
        assert!(long.ends_with(" …") && long.chars().count() <= 602, "{long}");
    }

    #[test]
    fn reasons_in_words() {
        assert_eq!(Why::Flagged { critical: 0, warn: 2 }.text(), "SurvivorsForge's scan found 2 warnings in this version");
        assert_eq!(Why::Flagged { critical: 1, warn: 1 }.text(), "SurvivorsForge's scan found 1 critical finding and 1 warning in this version");
        assert_eq!(Why::Flagged { critical: 0, warn: 0 }.text(), "SurvivorsForge's scan flagged this version");
        assert_eq!(Why::Blocked(Some("Sends\nyour chat   elsewhere".into())).text(), "SurvivorsForge blocked it: Sends your chat elsewhere");
        assert_eq!(Why::Blocked(None).text(), "SurvivorsForge blocked it");
        assert!(Why::Blocked(Some("x".repeat(500))).text().chars().count() < 200);
        assert_eq!(Why::Dropped.text(), "SurvivorsForge no longer lists it");
        assert_eq!(Why::NotScanned.text(), "SurvivorsForge hasn't scanned this version yet");
        assert_eq!(Why::SourceChanged.text(), "The file changed after SurvivorsForge scanned it");
        assert_eq!(Why::OtherFile.text(), "SurvivorsForge scanned a different file");
        assert_eq!(Why::Offline.text(), "SurvivorsForge isn't answering");
        assert_eq!(Why::Off.text(), "SurvivorsForge is off in Settings");
        assert_eq!(Verdict::Ask(Why::Dropped).why(), Some(&Why::Dropped));
        assert!(Verdict::Go.is_go() && Verdict::Go.why().is_none());
    }

    #[test]
    fn slugs_ids_and_origins() {
        for slug in ["totems", "kma-minimap-arrow", "0day"] {
            assert!(slug_ok(slug), "{slug}");
        }
        let long = "a".repeat(101);
        for slug in ["", "-totems", "Totems", "to tems", "../totems", "totems/x", "tötems", long.as_str()] {
            assert!(!slug_ok(slug), "{slug}");
        }
        assert_eq!(id_for("totems"), "forge:totems");
        assert_eq!((slug_in("forge:totems"), slug_in("forge:../x"), slug_in("lucky-odds")), (Some("totems"), None, None));
        assert_eq!(origin_of(Badge::Added, Some("totems")), Origin::ForgeOnly { slug: "totems" });
        assert_eq!(origin_of(Badge::Verified, None), Origin::OurList);
        assert_eq!(origin_of(Badge::File, None), Origin::Other);
    }
}
