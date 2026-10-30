//! Builds the catalog the Satchel carries. For each addon in `catalog/sources.toml` (the game
//! Discord's verified addons, each pinned to its author's GitHub repository) it records the latest
//! release and the recent earlier ones: the file, its SHA-256, and every folder's fingerprint, read
//! by the same package code the Satchel installs with, so a copy installed by hand is recognised.
//! Releases already in the catalog are reused, so a run downloads only what is new.
//!
//! `cargo run -p satchel-catalog -- <sources.toml> <catalog.json> [--offline]`
//!
//! Without an account GitHub answers 60 API calls an hour per address; a run needs one per addon,
//! plus one per new tag for an addon published by tag. `--offline` makes no calls at all: it
//! rewrites the catalog from `sources.toml` and the releases already recorded. Exit 1: written,
//! but an addon failed (its last entry is kept); exit 2: nothing written.

mod github;
mod select;

use chrono::{DateTime, Utc};
use github::GitHub;
use satchel_core::addons::catalog::{Asset, Catalog, Entry, Release};
use satchel_core::addons::package;
use satchel_core::fsx;
use select::Candidate;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;
use std::{env, fs};

/// Earlier releases kept per addon: the 50 newest before the latest, whatever their age.
const MAX_EARLIER: usize = 50;
/// Release notes are kept for the latest and the ten before it, each cut to 2,000 characters.
const NOTES_KEPT: usize = 11;
const NOTES_CHARS: usize = 2_000;
/// For an addon published by tag, the ten newest tags by version.
const TAGS_LOOKED_AT: usize = 10;
/// Download caps, per file and for the whole run.
const MAX_FILE: u64 = 128 << 20;
const MAX_RUN: u64 = 2 << 30;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sources {
    addon: Vec<Source>,
}

/// One addon as we list it; everything else is read from GitHub.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    id: String,
    name: String,
    /// `owner/repo` on GitHub.
    repo: String,
    group: String,
    /// What it does, in one line, from the author's own description.
    summary: String,
    /// Its folders in AddOns, the main one first.
    folders: Vec<String>,
    /// The release file that is the addon (`*` = any text).
    #[serde(default)]
    asset: Option<String>,
    /// Published by tag, with no file attached: the source zip at each version tag.
    #[serde(default)]
    by_tag: bool,
    #[serde(default)]
    doesnt_mix_with: Vec<String>,
    #[serde(default)]
    notes: Vec<String>,
}

impl Source {
    fn asset(&self) -> Asset {
        match &self.asset {
            Some(pattern) => Asset::File { pattern: pattern.clone() },
            None => Asset::Source,
        }
    }

    /// The entry with this source's own fields, around `latest` and `earlier`.
    fn entry(&self, latest: Release, earlier: Vec<Release>) -> Entry {
        Entry {
            id: self.id.clone(),
            name: self.name.clone(),
            repo: self.repo.clone(),
            author: self.repo.split('/').next().unwrap_or_default().to_string(),
            summary: self.summary.clone(),
            group: self.group.clone(),
            folders: self.folders.clone(),
            asset: self.asset(),
            latest,
            earlier,
            doesnt_mix_with: self.doesnt_mix_with.clone(),
            notes: self.notes.clone(),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let offline = args.iter().any(|a| a == "--offline");
    let paths: Vec<&str> = args.iter().filter(|a| !a.starts_with("--")).map(String::as_str).collect();
    let [sources, out] = paths[..] else {
        eprintln!("usage: satchel-catalog <sources.toml> <catalog.json> [--offline]");
        return ExitCode::from(2);
    };
    // 3, not 1: a failure before the tool runs (the build image, the compile) exits 1 too.
    match run(sources, out, offline) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(3),
        Err(why) => {
            eprintln!("catalog: {why}");
            ExitCode::from(2)
        }
    }
}

/// Whether every addon was built.
fn run(sources_path: &str, out_path: &str, offline: bool) -> Result<bool, String> {
    let text = fs::read_to_string(sources_path).map_err(|e| format!("{sources_path}: {e}"))?;
    let sources: Sources = toml::from_str(&text).map_err(|e| format!("{sources_path}: {e}"))?;
    check(&sources.addon)?;
    let previous: Option<Catalog> = fs::read_to_string(out_path).ok().and_then(|t| serde_json::from_str(&t).ok());
    let old = |source: &Source| previous.as_ref().and_then(|c| c.get(&source.id)).filter(|e| e.repo == source.repo);
    let known: BTreeMap<String, Release> = previous
        .iter()
        .flat_map(|c| &c.addons)
        .flat_map(Entry::releases)
        .filter(|r| !r.folders.is_empty())
        .map(|r| (r.url.clone(), r.clone()))
        .collect();

    let now = Utc::now();
    let mut github = GitHub::new(env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()), MAX_FILE, MAX_RUN);
    if !offline {
        let rate = github.rate()?;
        let needed: usize = sources.addon.iter().map(|s| if s.by_tag { 1 + TAGS_LOOKED_AT } else { 1 }).sum();
        if (rate.remaining as usize) < needed {
            let back = DateTime::from_timestamp(rate.reset, 0).map_or("later".into(), |t| t.format("%H:%M UTC").to_string());
            return Err(format!("GitHub has {} API calls left here, the run needs up to {needed}; more at {back}", rate.remaining));
        }
    }

    let mut addons = Vec::new();
    let mut failed = Vec::new();
    for source in &sources.addon {
        let built = match (offline, old(source)) {
            (true, Some(entry)) => Ok(source.entry(entry.latest.clone(), entry.earlier.clone())),
            (true, None) => Err("not in the last catalog, and --offline reads nothing new".to_string()),
            (false, _) => build(&mut github, source, &known),
        };
        match built {
            Ok(entry) => {
                println!(
                    "{:<20} {} · {} ({}{}) · {} earlier",
                    entry.id,
                    entry.repo,
                    entry.latest.shown_version(),
                    entry.latest.tag,
                    if entry.latest.prerelease { ", pre-release" } else { "" },
                    entry.earlier.len()
                );
                addons.push(entry);
            }
            Err(why) => {
                eprintln!("FAILED {}: {why}", source.id);
                // In a GitHub workflow the run's summary shows it.
                if env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
                    println!("::warning::{} wasn't read: {}", source.id, why.replace(['\r', '\n'], " "));
                }
                failed.push(source.id.as_str());
                if let Some(entry) = old(source) {
                    eprintln!("  keeping its entry from the last catalog");
                    addons.push(source.entry(entry.latest.clone(), entry.earlier.clone()));
                }
            }
        }
    }

    // Unchanged addons keep the time they were last read with, so the file changes only when an
    // addon does (the catalog workflow commits only real changes).
    let generated = match &previous {
        Some(p) if p.addons == addons => p.generated,
        _ => now,
    };
    let catalog = Catalog { format: 1, generated, addons };
    let json = serde_json::to_string_pretty(&catalog).map_err(|e| e.to_string())? + "\n";
    fs::write(out_path, json).map_err(|e| format!("{out_path}: {e}"))?;
    println!(
        "{} addons · {} API calls · {:.1} MB downloaded · {out_path}{}",
        catalog.addons.len(),
        github.calls,
        github.streamed as f64 / 1e6,
        if failed.is_empty() { String::new() } else { format!(" · FAILED: {}", failed.join(", ")) }
    );
    Ok(failed.is_empty())
}

/// The list makes sense before anything is fetched.
fn check(sources: &[Source]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    for s in sources {
        let fault = if !ids.insert(s.id.as_str()) {
            Some("listed twice")
        } else if s.repo.split('/').count() != 2 || s.repo.split('/').any(str::is_empty) {
            Some("repo must be owner/name")
        } else if s.folders.is_empty() {
            Some("no folders")
        } else if s.asset.is_some() == s.by_tag {
            Some("needs exactly one of asset or by_tag")
        } else {
            None
        };
        if let Some(fault) = fault {
            return Err(format!("sources: {}: {fault}", s.id));
        }
    }
    Ok(())
}

/// One addon's entry, fresh from GitHub.
fn build(github: &mut GitHub, source: &Source, known: &BTreeMap<String, Release>) -> Result<Entry, String> {
    let candidates = if source.by_tag { by_tag(github, source, known)? } else { from_releases(github, source)? };
    let (latest, earlier) = select::pick(candidates, MAX_EARLIER).ok_or("no release holds the addon")?;
    let latest = release(github, source, &latest, known, true)?;
    let mut kept = Vec::new();
    for candidate in &earlier {
        match release(github, source, candidate, known, false) {
            Ok(mut release) => {
                if kept.len() + 1 >= NOTES_KEPT {
                    release.notes = None;
                }
                kept.push(release);
            }
            Err(why) => eprintln!("  {} {}: left out, {why}", source.id, candidate.tag),
        }
    }
    Ok(source.entry(latest, kept))
}

/// The releases holding exactly one file that matches the addon's pattern.
fn from_releases(github: &mut GitHub, source: &Source) -> Result<Vec<Candidate>, String> {
    let asset = source.asset();
    let mut candidates = Vec::new();
    for release in github.releases(&source.repo)? {
        let Some(published) = release.published_at.filter(|_| !release.draft) else { continue };
        let files: Vec<_> = release.assets.iter().filter(|a| asset.matches(&a.name)).collect();
        let [file] = files[..] else {
            if files.len() > 1 {
                eprintln!("  {} {}: left out, {} files match the pattern", source.id, release.tag_name, files.len());
            }
            continue;
        };
        candidates.push(Candidate {
            tag: release.tag_name.clone(),
            published,
            prerelease: release.prerelease,
            file: file.name.clone(),
            url: file.browser_download_url.clone(),
            sha256: file.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")).map(str::to_ascii_lowercase),
            notes: release.body.as_deref().and_then(|b| select::trim_notes(b, NOTES_CHARS)),
        });
    }
    Ok(candidates)
}

/// The newest version tags, each as the source zip of its commit (so a moved tag is a new file).
/// A commit already in the catalog needs no call for its date.
fn by_tag(github: &mut GitHub, source: &Source, known: &BTreeMap<String, Release>) -> Result<Vec<Candidate>, String> {
    let mut tags = github.tags(&source.repo)?;
    tags.sort_by(|a, b| select::version_of(&b.name).cmp(&select::version_of(&a.name)));
    let name = source.repo.split('/').nth(1).unwrap_or_default();
    let mut candidates = Vec::new();
    for tag in tags.into_iter().take(TAGS_LOOKED_AT) {
        let url = format!("https://codeload.github.com/{}/zip/{}", source.repo, tag.commit.sha);
        let published = match known.get(&url) {
            Some(release) => release.published,
            None => github.commit_date(&source.repo, &tag.commit.sha)?,
        };
        candidates.push(Candidate {
            file: format!("{name}-{}.zip", tag.name),
            tag: tag.name,
            published,
            prerelease: false,
            url,
            sha256: None,
            notes: None,
        });
    }
    Ok(candidates)
}

/// The catalog's record of one release: reused when the last catalog has the same file, else
/// downloaded, checked against GitHub's own checksum, and read by the Satchel's package code. The
/// latest release must hold exactly the addon's folders.
fn release(github: &mut GitHub, source: &Source, c: &Candidate, known: &BTreeMap<String, Release>, latest: bool) -> Result<Release, String> {
    if let Some(old) = known.get(&c.url).filter(|old| c.sha256.as_ref().is_none_or(|sha| *sha == old.sha256)) {
        return Ok(Release { tag: c.tag.clone(), published: c.published, prerelease: c.prerelease, notes: c.notes.clone(), ..old.clone() });
    }
    let bytes = github.download(&c.url)?;
    let sha256 = fsx::fingerprint(&bytes);
    if let Some(published) = &c.sha256
        && *published != sha256
    {
        return Err(format!("the file's SHA-256 is {sha256}, GitHub published {published}"));
    }
    let package = package::inspect(&bytes).map_err(|e| e.to_string())?;
    let folders = package::fingerprints(&bytes, &package).map_err(|e| e.to_string())?;
    let has: BTreeSet<String> = folders.keys().map(|f| f.to_lowercase()).collect();
    let wants: BTreeSet<String> = source.folders.iter().map(|f| f.to_lowercase()).collect();
    if has != wants {
        let why = format!("its folders are {:?}, the catalog lists {:?}", folders.keys().collect::<Vec<_>>(), source.folders);
        if latest {
            return Err(why);
        }
        eprintln!("  {} {}: {why}", source.id, c.tag);
    }
    let main = package.folders.iter().find(|f| f.name.eq_ignore_ascii_case(&source.folders[0])).or(package.folders.first());
    Ok(Release {
        tag: c.tag.clone(),
        version: main.and_then(|f| f.toc.version.clone()),
        published: c.published,
        prerelease: c.prerelease,
        file: c.file.clone(),
        url: c.url.clone(),
        sha256,
        size: bytes.len() as u64,
        interface: main.map_or(0, |f| f.toc.interface),
        folders,
        notes: c.notes.clone(),
    })
}
