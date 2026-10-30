//! Reading GitHub without an account. An addon's repository comes from what a player pastes.
//! The newest stable release comes from the `/releases/latest` redirect, and a release's files
//! and their checksums from its assets page; neither counts against the 60 API calls an hour.
//! The REST API's answers serve a one-off "Add from a link". No network here: the app fetches,
//! this decides.

use super::catalog::{Asset, Release};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;

/// `owner/repo` from "owner/repo", "github.com/owner/repo" or any link into the repository
/// (`https://github.com/owner/repo/releases/…`, `.git` at the end). `None` for anything else.
pub fn repo_from_input(text: &str) -> Option<String> {
    let text = text.trim();
    let text = text.split(['?', '#']).next().unwrap_or_default();
    let lower = text.to_ascii_lowercase();
    let path = if let Some(at) = lower.find("://") {
        let rest = &text[at + 3..];
        let (host, path) = rest.split_once('/')?;
        if !matches!(host.to_ascii_lowercase().as_str(), "github.com" | "www.github.com") {
            return None;
        }
        path
    } else if lower.starts_with("github.com/") || lower.starts_with("www.github.com/") {
        text.split_once('/')?.1
    } else {
        text
    };
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    // Bare input is exactly owner/repo; a link may go deeper into the repository.
    if !lower.contains("github.com/") && parts.next().is_some() {
        return None;
    }
    let owner_ok = (1..=39).contains(&owner.len())
        && !owner.starts_with('-')
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let repo_ok = (1..=100).contains(&repo.len())
        && repo != "."
        && repo != ".."
        && repo.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (owner_ok && repo_ok).then(|| format!("{owner}/{repo}"))
}

/// The tag `/releases/latest` redirects to; `None` when it redirects to the list of releases
/// (the repository has only pre-releases, or none at all).
pub fn latest_tag(repo: &str, location: &str) -> Option<String> {
    let path = location.strip_prefix("https://github.com").unwrap_or(location);
    let prefix = format!("/{repo}/releases/tag/");
    let head = path.get(..prefix.len())?;
    let tag = &path[prefix.len()..];
    (head.eq_ignore_ascii_case(&prefix) && !tag.is_empty()).then(|| percent_decode(tag))
}

/// A release file as its assets page lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct PageFile {
    pub name: String,
    pub url: String,
    /// GitHub's `sha256:` digest; files uploaded before mid-2025 have none.
    pub sha256: Option<String>,
}

/// The release files on `github.com/<repo>/releases/expanded_assets/<tag>`. Each sits in a list
/// row with a `/<repo>/releases/download/<tag>/<name>` link and, for newer uploads,
/// `sha256:<hex>`. The source archives GitHub adds to every release aren't release files.
pub fn page_files(repo: &str, html: &str) -> Vec<PageFile> {
    let prefix = format!("/{repo}/releases/download/").to_ascii_lowercase();
    let mut files = Vec::new();
    for row in html.split("<li").skip(1) {
        let row = row.split("</li>").next().unwrap_or_default();
        let href = row
            .split("href=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .map(|href| href.replace("&amp;", "&"))
            .find(|href| href.to_ascii_lowercase().starts_with(&prefix));
        let Some(href) = href else { continue };
        let Some((_, name)) = href[prefix.len()..].split_once('/') else { continue };
        if name.is_empty() || files.iter().any(|f: &PageFile| f.url.ends_with(&href)) {
            continue;
        }
        files.push(PageFile { name: percent_decode(name), url: format!("https://github.com{href}"), sha256: sha256_in(row) });
    }
    files
}

/// The first `sha256:<64 hex>` in `text`, lowercased.
fn sha256_in(text: &str) -> Option<String> {
    text.match_indices("sha256:").find_map(|(at, _)| {
        let hex = text.get(at + 7..at + 7 + 64)?;
        hex.chars().all(|c| c.is_ascii_hexdigit()).then(|| hex.to_ascii_lowercase())
    })
}

/// `%20` and the like back to text.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) = (bytes.get(i + 1).and_then(|b| hex(*b)), bytes.get(i + 2).and_then(|b| hex(*b)))
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// "AZSSpellMeter-1.34.zip" → "AZSSpellMeter-*.*.zip": the same file in any version.
pub fn numbers_as_any(file: &str) -> String {
    let mut out = String::new();
    let mut in_number = false;
    for c in file.chars() {
        if c.is_ascii_digit() {
            if !in_number {
                out.push('*');
            }
            in_number = true;
        } else {
            out.push(c);
            in_number = false;
        }
    }
    out
}

/// Which file of a release is the addon.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// A verified addon: the catalog's pattern.
    Pattern(String),
    /// An addon added by link: the only zip, or the one named like the file it came from.
    Like(Option<String>),
}

/// The addon among a release's files (by index in `names`). With `Like`: the only zip; else the
/// one named like the previous file, its numbers aside; else the only zip that isn't a source or
/// installer package.
pub fn pick(names: &[&str], how: &Pick) -> Option<usize> {
    let only = |found: Vec<usize>| if let [one] = found[..] { Some(one) } else { None };
    match how {
        Pick::Pattern(pattern) => {
            let asset = Asset::File { pattern: pattern.clone() };
            names.iter().position(|n| asset.matches(n))
        }
        Pick::Like(previous) => {
            let zips: Vec<usize> = (0..names.len()).filter(|&i| names[i].to_ascii_lowercase().ends_with(".zip")).collect();
            if zips.len() == 1 {
                return Some(zips[0]);
            }
            if let Some(previous) = previous {
                let like = Asset::File { pattern: numbers_as_any(previous) };
                if let Some(one) = only(zips.iter().copied().filter(|&i| like.matches(names[i])).collect()) {
                    return Some(one);
                }
            }
            only(
                zips.into_iter()
                    .filter(|&i| {
                        let name = names[i].to_ascii_lowercase();
                        !["source", "src", "installer", "setup"].iter().any(|w| name.contains(w))
                    })
                    .collect(),
            )
        }
    }
}

/// The version a tag names: "v1.35" → "1.35", "Version1.2.2" → "1.2.2"; `None` for a tag that
/// isn't a version ("build-6284ab9", "Release").
pub fn tag_version(tag: &str) -> Option<String> {
    let at = tag.find(|c: char| c.is_ascii_digit())?;
    let prefix = tag[..at].trim_end_matches(['-', '_', '.', ' ']).to_ascii_lowercase();
    matches!(prefix.as_str(), "" | "v" | "version" | "release").then(|| tag[at..].to_string())
}

/// A release seen on the assets page: what the page tells (no publish time, no folders yet).
pub fn page_release(tag: &str, file: &PageFile, seen: DateTime<Utc>) -> Release {
    Release {
        tag: tag.to_string(),
        version: tag_version(tag),
        published: seen,
        prerelease: false,
        file: file.name.clone(),
        url: file.url.clone(),
        sha256: file.sha256.clone().unwrap_or_default(),
        size: 0,
        interface: 0,
        folders: BTreeMap::new(),
        notes: None,
    }
}

// The REST API, for a one-off lookup ------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct RestRepo {
    pub full_name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub stargazers_count: u64,
    pub pushed_at: Option<DateTime<Utc>>,
    pub license: Option<RestLicense>,
    #[serde(default)]
    pub archived: bool,
    pub owner: RestOwner,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RestOwner {
    pub login: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RestLicense {
    pub spdx_id: Option<String>,
    pub name: Option<String>,
}

impl RestLicense {
    /// "MIT", or the licence's name when GitHub has no short id for it.
    pub fn shown(&self) -> Option<String> {
        self.spdx_id.clone().filter(|id| id != "NOASSERTION").or_else(|| self.name.clone())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct RestRelease {
    pub tag_name: String,
    pub published_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<RestAsset>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RestAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
    /// `sha256:<hex>`, on files uploaded since mid-2025.
    pub digest: Option<String>,
}

impl RestAsset {
    pub fn sha256(&self) -> Option<String> {
        let hex = self.digest.as_deref()?.strip_prefix("sha256:")?;
        (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| hex.to_ascii_lowercase())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct RestTag {
    pub name: String,
    pub commit: RestCommit,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RestCommit {
    pub sha: String,
}

/// The release to install: the newest stable one by publish time, else the newest pre-release
/// (GitHub's list isn't in publish order). Never a draft.
pub fn latest_release(releases: &[RestRelease]) -> Option<&RestRelease> {
    let mut published: Vec<&RestRelease> = releases.iter().filter(|r| !r.draft).collect();
    published.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    published.iter().find(|r| !r.prerelease).or(published.first()).copied()
}

/// The newest version tag, for a repository that publishes by tag: "v2.8.10" beats "v2.8.9";
/// tags that aren't versions are left out.
pub fn newest_version_tag(tags: &[RestTag]) -> Option<&RestTag> {
    let numbers = |tag: &str| -> Vec<u64> {
        tag_version(tag)
            .unwrap_or_default()
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .map(|p| p.parse().unwrap_or(u64::MAX))
            .collect()
    };
    tags.iter().filter(|t| tag_version(&t.name).is_some()).max_by(|a, b| numbers(&a.name).cmp(&numbers(&b.name)))
}

/// The repository's bug report form with the Satchel's version filled in: GitHub fills an issue
/// form's field from the query parameter named after the field's id (the form's `version`). A
/// version is digits, dots and hyphens, so nothing needs encoding.
pub fn issue_url(repository: &str, version: &str) -> String {
    format!("https://github.com/{repository}/issues/new?template=bug.yml&version={version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bug_form_opens_with_the_version_filled_in() {
        assert_eq!(
            issue_url("owner/survivors-satchel", "0.6.0"),
            "https://github.com/owner/survivors-satchel/issues/new?template=bug.yml&version=0.6.0"
        );
    }

    #[test]
    fn repositories_from_what_players_paste() {
        let ok = [
            ("kex0/AZSSpellMeter", "kex0/AZSSpellMeter"),
            (" github.com/kex0/AZSSpellMeter ", "kex0/AZSSpellMeter"),
            ("https://github.com/kex0/AZSSpellMeter", "kex0/AZSSpellMeter"),
            ("https://www.github.com/kex0/AZSSpellMeter/", "kex0/AZSSpellMeter"),
            ("https://github.com/kex0/AZSSpellMeter.git", "kex0/AZSSpellMeter"),
            ("https://github.com/kex0/AZSSpellMeter/releases/tag/v1.34", "kex0/AZSSpellMeter"),
            ("https://github.com/Maidenquinn/azeroth-fieldkit?tab=readme-ov-file#install", "Maidenquinn/azeroth-fieldkit"),
            ("HTTPS://GitHub.com/a-b/c_d.e", "a-b/c_d.e"),
        ];
        for (input, repo) in ok {
            assert_eq!(repo_from_input(input).as_deref(), Some(repo), "{input}");
        }
        for input in ["", "kex0", "https://gitlab.com/kex0/AZSSpellMeter", "kex0/AZS/extra", "-x/y", "a/..", "a b/c", "https://github.com/kex0"] {
            assert_eq!(repo_from_input(input), None, "{input}");
        }
    }

    #[test]
    fn the_latest_redirect() {
        let repo = "kex0/AZSSpellMeter";
        assert_eq!(latest_tag(repo, "https://github.com/kex0/AZSSpellMeter/releases/tag/v1.34").as_deref(), Some("v1.34"));
        assert_eq!(latest_tag(repo, "/kex0/azsspellmeter/releases/tag/my%20tag").as_deref(), Some("my tag"));
        // Only pre-releases: GitHub sends the list.
        assert_eq!(latest_tag("Maidenquinn/azeroth-fieldkit", "https://github.com/Maidenquinn/azeroth-fieldkit/releases"), None);
        assert_eq!(latest_tag(repo, "https://github.com/other/repo/releases/tag/v1"), None);
    }

    // The shape of github.com/kex0/AZSSpellMeter/releases/expanded_assets/v1.34 on 29 Oct 2026,
    // shortened: one release file with its digest, then the two source archives.
    const PAGE: &str = r#"<div data-view-component="true" class="Box Box--condensed mt-3">
  <ul data-view-component="true">
        <li data-view-component="true" class="Box-row d-flex flex-column flex-md-row">      <div class="d-flex">
          <a href="/kex0/AZSSpellMeter/releases/download/v1.34/AZSSpellMeter-1.34.zip" rel="nofollow" data-turbo="false" class="wb-break-all">
            <span data-view-component="true" class="Truncate">AZSSpellMeter-1.34.zip</span></a>
            <span style="min-width: 0; max-width: 250px" data-view-component="true" class="Truncate text-mono text-small color-fg-muted">
    <span data-view-component="true" class="Truncate-text">sha256:5d3634b182ef81494c88f11b0ed209cd21031598117d26a31bfc59827c2044cb</span>
</span>
  <clipboard-copy id="clipboard-button-sha256:5d3634b182ef81494c88f11b0ed209cd21031598117d26a31bfc59827c2044cb" aria-label="Copy to clipboard digest for AZSSpellMeter-1.34.zip"></clipboard-copy>
</div></li>
        <li data-view-component="true" class="Box-row d-flex flex-column flex-md-row">      <div class="d-flex">
          <a href="/kex0/AZSSpellMeter/archive/refs/tags/v1.34.zip" rel="nofollow" data-turbo="false" class="wb-break-all">
</div></li>
        <li data-view-component="true" class="Box-row d-flex flex-column flex-md-row">      <div class="d-flex">
          <a href="/kex0/AZSSpellMeter/releases/download/v1.34/Read%20me.txt" rel="nofollow">
</div></li>
  </ul></div>"#;

    #[test]
    fn release_files_and_their_checksums_from_the_assets_page() {
        let files = page_files("kex0/AZSSpellMeter", PAGE);
        assert_eq!(
            files,
            [
                PageFile {
                    name: "AZSSpellMeter-1.34.zip".into(),
                    url: "https://github.com/kex0/AZSSpellMeter/releases/download/v1.34/AZSSpellMeter-1.34.zip".into(),
                    sha256: Some("5d3634b182ef81494c88f11b0ed209cd21031598117d26a31bfc59827c2044cb".into()),
                },
                PageFile {
                    name: "Read me.txt".into(),
                    url: "https://github.com/kex0/AZSSpellMeter/releases/download/v1.34/Read%20me.txt".into(),
                    sha256: None,
                },
            ]
        );
        assert!(page_files("someone/else", PAGE).is_empty(), "links into another repository are ignored");
        let release = page_release("v1.34", &files[0], Utc::now());
        assert_eq!((release.shown_version(), release.sha256.len()), ("1.34", 64));
    }

    #[test]
    fn the_addon_among_a_releases_files() {
        let verified = Pick::Pattern("AZSSpellMeter-*.zip".into());
        assert_eq!(pick(&["notes.txt", "AZSSpellMeter-1.35.zip"], &verified), Some(1));
        assert_eq!(pick(&["SurvivorsDragonUi-updater.json", "avrt.dll"], &Pick::Pattern("SurvivorsDragonUi.zip".into())), None);
        let added = |previous: Option<&str>| Pick::Like(previous.map(str::to_string));
        assert_eq!(pick(&["RunPace.zip"], &added(None)), Some(0));
        assert_eq!(pick(&["RunPace-2.0.zip", "RunPace-2.0-source.zip"], &added(Some("RunPace-1.1.zip"))), Some(0));
        assert_eq!(pick(&["Tool-Installer.zip", "Tool-1.0.zip"], &added(None)), Some(1));
        assert_eq!(pick(&["A.zip", "B.zip"], &added(None)), None, "two candidates: can't tell");
        assert_eq!(numbers_as_any("LuckyOdds-v0.7.0.zip"), "LuckyOdds-v*.*.*.zip");
    }

    #[test]
    fn versions_from_tags() {
        assert_eq!(tag_version("v1.35").as_deref(), Some("1.35"));
        assert_eq!(tag_version("Version1.2.2").as_deref(), Some("1.2.2"));
        assert_eq!(tag_version("release-2.0").as_deref(), Some("2.0"));
        assert_eq!(tag_version("2.8.6").as_deref(), Some("2.8.6"));
        assert_eq!(tag_version("build-6284ab9995305ec4"), None);
        assert_eq!(tag_version("FirstVersion"), None);
        let tag = |name: &str| RestTag { name: name.into(), commit: RestCommit { sha: format!("sha-{name}") } };
        let tags = [tag("v2.8.9"), tag("v2.8.10"), tag("nightly"), tag("v2.7.1")];
        assert_eq!(newest_version_tag(&tags).map(|t| t.name.as_str()), Some("v2.8.10"));
    }

    #[test]
    fn the_latest_release_is_the_newest_stable_one() {
        let json = r#"[
            {"tag_name": "v3.0-beta", "published_at": "2026-10-29T10:00:00Z", "prerelease": true, "assets": []},
            {"tag_name": "v2.1", "published_at": "2026-10-20T10:00:00Z", "assets": [
                {"name": "A-2.1.zip", "browser_download_url": "https://github.com/o/a/releases/download/v2.1/A-2.1.zip", "size": 10,
                 "digest": "sha256:5D3634B182EF81494C88F11B0ED209CD21031598117D26A31BFC59827C2044CB"}]},
            {"tag_name": "v2.2", "published_at": "2026-10-25T10:00:00Z", "draft": true, "assets": []},
            {"tag_name": "v2.0", "published_at": "2026-10-01T10:00:00Z", "assets": []}
        ]"#;
        let releases: Vec<RestRelease> = serde_json::from_str(json).unwrap();
        let latest = latest_release(&releases).unwrap();
        assert_eq!(latest.tag_name, "v2.1");
        assert_eq!(latest.assets[0].sha256().as_deref(), Some("5d3634b182ef81494c88f11b0ed209cd21031598117d26a31bfc59827c2044cb"));
        let betas: Vec<RestRelease> = serde_json::from_str(r#"[{"tag_name": "v0.2.3", "published_at": "2026-10-29T10:00:00Z", "prerelease": true}]"#).unwrap();
        assert_eq!(latest_release(&betas).unwrap().tag_name, "v0.2.3", "pre-releases only: the newest");
    }
}
