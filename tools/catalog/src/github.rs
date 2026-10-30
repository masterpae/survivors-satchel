//! The few GitHub calls the catalog needs, and the downloads. API calls carry `GITHUB_TOKEN` when
//! it is set; downloads never do. Every download is capped, per file and for the whole run, by the
//! bytes actually streamed.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::time::Duration;

const API: &str = "https://api.github.com";
const USER_AGENT: &str = concat!("survivors-satchel-catalog/", env!("CARGO_PKG_VERSION"));
/// The most pages of 100 releases read per repository.
const MAX_PAGES: u32 = 5;

#[derive(Deserialize)]
pub struct GhRelease {
    pub tag_name: String,
    pub published_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
pub struct GhAsset {
    pub name: String,
    pub browser_download_url: String,
    /// `sha256:<hex>`, on files uploaded since mid-2025.
    pub digest: Option<String>,
}

#[derive(Deserialize)]
pub struct GhTag {
    pub name: String,
    pub commit: GhSha,
}

#[derive(Deserialize)]
pub struct GhSha {
    pub sha: String,
}

#[derive(Deserialize)]
struct GhCommit {
    commit: GhCommitBody,
}

#[derive(Deserialize)]
struct GhCommitBody {
    committer: Option<GhWhen>,
    author: Option<GhWhen>,
}

#[derive(Deserialize)]
struct GhWhen {
    date: DateTime<Utc>,
}

#[derive(Deserialize)]
struct GhRate {
    resources: GhResources,
}

#[derive(Deserialize)]
struct GhResources {
    core: GhLimit,
}

#[derive(Deserialize)]
pub struct GhLimit {
    pub remaining: u32,
    pub reset: i64,
}

pub struct GitHub {
    agent: ureq::Agent,
    token: Option<String>,
    max_file: u64,
    max_run: u64,
    /// API calls made (the rate-limit check doesn't count, for GitHub either).
    pub calls: u32,
    /// Bytes downloaded.
    pub streamed: u64,
}

impl GitHub {
    pub fn new(token: Option<String>, max_file: u64, max_run: u64) -> GitHub {
        let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(300))).build().into();
        GitHub { agent, token, max_file, max_run, calls: 0, streamed: 0 }
    }

    fn get<T: DeserializeOwned>(&mut self, path: &str) -> Result<T, String> {
        let mut request = self
            .agent
            .get(format!("{API}{path}"))
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let mut response = request.call().map_err(|e| format!("GET {path}: {e}"))?;
        let text = response.body_mut().with_config().limit(32 << 20).read_to_string().map_err(|e| format!("GET {path}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("GET {path}: {e}"))
    }

    /// API calls left for this address (or token), and when they come back.
    pub fn rate(&mut self) -> Result<GhLimit, String> {
        Ok(self.get::<GhRate>("/rate_limit")?.resources.core)
    }

    pub fn releases(&mut self, repo: &str) -> Result<Vec<GhRelease>, String> {
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            self.calls += 1;
            let batch: Vec<GhRelease> = self.get(&format!("/repos/{repo}/releases?per_page=100&page={page}"))?;
            let more = batch.len() == 100;
            all.extend(batch);
            if !more {
                break;
            }
        }
        Ok(all)
    }

    pub fn tags(&mut self, repo: &str) -> Result<Vec<GhTag>, String> {
        self.calls += 1;
        self.get(&format!("/repos/{repo}/tags?per_page=100"))
    }

    /// When the commit a tag points to was made.
    pub fn commit_date(&mut self, repo: &str, sha: &str) -> Result<DateTime<Utc>, String> {
        self.calls += 1;
        let commit: GhCommit = self.get(&format!("/repos/{repo}/commits/{sha}"))?;
        let when = commit.commit.committer.or(commit.commit.author).ok_or_else(|| format!("commit {sha} has no date"))?;
        Ok(when.date)
    }

    /// A release file or a source zip, refused past the caps.
    pub fn download(&mut self, url: &str) -> Result<Vec<u8>, String> {
        let left = self.max_run.saturating_sub(self.streamed);
        if left == 0 {
            return Err("the run's download cap is used up".into());
        }
        let mut response = self.agent.get(url).header("User-Agent", USER_AGENT).call().map_err(|e| format!("{url}: {e}"))?;
        let bytes = response.body_mut().with_config().limit(self.max_file.min(left)).read_to_vec().map_err(|e| format!("{url}: {e}"))?;
        self.streamed += bytes.len() as u64;
        Ok(bytes)
    }
}
