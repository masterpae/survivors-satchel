//! The network, on its own thread, so a slow answer never holds up the worker's checkpoints:
//! what's new on GitHub for the installed addons, downloads into the addon cache, the one-off
//! lookup behind "Add from a link", and the Satchel's own newest release once its repository is
//! public. Jobs come from the worker and the answers go back to it as `Command::Net`. Nothing here
//! touches AddOns.
//!
//! What's new is read from two plain GitHub pages that don't count against the 60 API calls an
//! hour (the `/releases/latest` redirect and a release's assets page, with its checksums). Only
//! "Add from a link" asks the API, once per lookup, and falls back to the pages when GitHub says
//! the hour's calls are used up.

use crate::update::{self, Offer};
use crate::win;
use crate::worker::Command;
use chrono::{DateTime, Utc};
use satchel_core::addons::catalog::Release;
use satchel_core::addons::github::{self, Pick, RestRelease, RestRepo, RestTag};
use satchel_core::addons::overview::{Watch, plain};
use satchel_core::addons::package;
use satchel_core::{fsx, gate, log, signing};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

// ureq puts Windows' TLS in its connector only with its `native-tls` feature: with
// `native-tls-no-default` alone, every https call panics ("provider is NativeTls but feature is
// not enabled"). This type exists only with that feature, so a build without it stops here.
const _: Option<ureq::unversioned::transport::NativeTlsConnector> = None;

const USER_AGENT: &str = concat!("SurvivorsSatchel/", env!("CARGO_PKG_VERSION"));
const API: &str = "https://api.github.com";
/// The largest addon file downloaded, counted by the bytes actually streamed.
const MAX_FILE: u64 = 128 << 20;
/// The largest page or API answer read.
const MAX_PAGE: u64 = 4 << 20;
/// How long a page or an API answer may take, so one stalled connection can't hold up the jobs
/// behind it; a release file gets longer.
const PAGE_TIME: Duration = Duration::from_secs(30);
const FILE_TIME: Duration = Duration::from_secs(180);

pub enum Job {
    /// What's new for these installed addons.
    Look(Vec<Watch>),
    /// A release file into the cache, checked against `sha256` when it's known.
    Fetch { id: String, tag: String, url: String, sha256: Option<String> },
    /// "Add from a link": the repository, its release and its zip, judged.
    Lookup(String),
    /// The Satchel's newest release in `repo` (or the release of `want`, the version a newer
    /// Satchel stamped on the shared files), downloaded into `dir` when it's newer than this one.
    Satchel { repo: String, dir: PathBuf, want: Option<String> },
}

pub enum Answer {
    /// Per addon: a release newer than the ones known (or none), or why it couldn't be looked up.
    Looked(Vec<(String, Result<Option<Release>, String>)>),
    /// The file is in the cache under its checksum, or why not.
    Fetched { id: String, tag: String, sha256: Result<String, String> },
    LookedUp(Result<Lookup, String>),
    /// A newer Satchel from GitHub, downloaded and checked; `None` when there's none.
    Satchel(Result<Option<Offer>, String>),
}

/// What "Look up" found.
#[derive(Clone, Debug)]
pub struct Lookup {
    pub repo: String,
    pub owner: String,
    pub description: Option<String>,
    /// `None` when only the pages could be read (GitHub's hour of API calls was used up).
    pub stars: Option<u64>,
    pub pushed: Option<DateTime<Utc>>,
    pub license: Option<String>,
    pub tag: String,
    /// When the release was published: from GitHub's API, not from its pages or a bare tag.
    pub released: Option<DateTime<Utc>>,
    pub file: String,
    /// GitHub published the file's checksum and the download matched it.
    pub checked: bool,
    /// The zip's checksum: it waits in the cache under this name.
    pub sha256: String,
    pub size: u64,
    pub folders: Vec<String>,
    pub left_out: usize,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub version: Option<String>,
    /// Made for this game version (`## Interface: 11200`).
    pub current: bool,
}

/// Starts the network thread; answers come back on `commands`.
pub fn start(cache: PathBuf, commands: Sender<Command>) -> Sender<Job> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("satchel-net".into())
        .spawn(move || run(rx, &cache, &commands))
        .expect("the network thread could not start");
    tx
}

fn run(jobs: Receiver<Job>, cache: &Path, commands: &Sender<Command>) {
    let net = Net::new();
    for job in jobs {
        let answer = match job {
            Job::Look(watches) => Answer::Looked(watches.iter().map(|w| (w.id.clone(), guarded(|| net.look(w)))).collect()),
            Job::Fetch { id, tag, url, sha256 } => Answer::Fetched { sha256: guarded(|| net.fetch(&url, sha256.as_deref(), cache)), id, tag },
            Job::Lookup(input) => Answer::LookedUp(guarded(|| net.lookup(&input, cache))),
            Job::Satchel { repo, dir, want } => Answer::Satchel(guarded(|| net.satchel(&repo, &dir, want.as_deref()))),
        };
        if commands.send(Command::Net(answer)).is_err() {
            break;
        }
    }
}

/// Only the files in `keep` stay among the downloaded Satchels (an exe and its signature); none
/// when it's empty.
fn keep_only(dir: &Path, keep: &[&Path]) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if !keep.contains(&path.as_path()) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A release's signature, `bytes` as downloaded from `file`, over the exe at `exe`: checked against
/// the checksum GitHub published for it, verified with the release keys and written beside the
/// exe. Its path, or why it was refused.
fn accept_signature(file: &github::PageFile, bytes: Vec<u8>, tag: &str, version: &str, exe: &Path) -> Result<PathBuf, String> {
    if file.sha256.as_deref().is_some_and(|expected| fsx::fingerprint(&bytes) != expected) {
        return Err(format!("the signature of {tag} downloaded isn't the one published (its checksum differs)"));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("the signature of {tag} isn't text"))?;
    let exe_bytes = std::fs::read(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    let key = signing::verify(&exe_bytes, &text, version).map_err(|why| format!("{version} was refused: {why}"))?;
    log!("Satchel {version}'s signature verified with the release key {key}");
    let path = exe.with_file_name(format!("{}.minisig", exe.file_name().unwrap_or_default().to_string_lossy()));
    fsx::write_atomic(&path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Runs a job: one that panics answers with an error, and the thread lives on for the next.
fn guarded<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|_| Err("the Satchel hit an internal error".into()))
}

enum ApiError {
    NotFound,
    /// The hour's 60 calls are used up.
    Limited,
    Other(String),
}

struct Net {
    agent: ureq::Agent,
}

/// How every error about a request GitHub didn't answer starts: Check for updates tells such an
/// error apart from a release it refused.
pub const NO_ANSWER: &str = "GitHub didn't answer";

/// A request GitHub didn't answer, in the player's words; the address goes to the log.
fn no_answer(url: &str, e: impl std::fmt::Display) -> String {
    log!("{url}: {e}");
    format!("{NO_ANSWER} ({e})")
}

/// A URL path segment: anything but letters, digits and `-._~` percent-encoded.
fn segment(text: &str) -> String {
    text.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

impl Net {
    fn new() -> Net {
        // Windows' own TLS and certificate store; ureq's built-in root list goes unused.
        let tls = TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(true)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .timeout_global(Some(PAGE_TIME))
            .build()
            .into();
        Net { agent }
    }

    /// Where a URL redirects to, without following it.
    fn location(&self, url: &str) -> Result<Option<String>, String> {
        let response = self.agent.get(url).config().max_redirects(0).build().call().map_err(|e| no_answer(url, e))?;
        match response.status().as_u16() {
            301 | 302 | 303 | 307 | 308 => {
                Ok(response.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string))
            }
            404 => Err("its GitHub repository isn't there anymore".into()),
            200 => Ok(None),
            status => Err(format!("GitHub answered {status}")),
        }
    }

    fn text(&self, url: &str) -> Result<String, String> {
        let mut response = self.agent.get(url).call().map_err(|e| no_answer(url, e))?;
        match response.status().as_u16() {
            200 => response.body_mut().with_config().limit(MAX_PAGE).read_to_string().map_err(|e| no_answer(url, e)),
            status => Err(format!("GitHub answered {status}")),
        }
    }

    fn api<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let url = format!("{API}{path}");
        let mut response = self
            .agent
            .get(url.as_str())
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .map_err(|e| ApiError::Other(no_answer(&url, e)))?;
        let left = response.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()).map(str::to_string);
        match response.status().as_u16() {
            200 => {}
            404 => return Err(ApiError::NotFound),
            403 | 429 if response.status().as_u16() == 429 || left.as_deref() == Some("0") => return Err(ApiError::Limited),
            status => return Err(ApiError::Other(format!("GitHub answered {status}"))),
        }
        let text = response.body_mut().with_config().limit(MAX_PAGE).read_to_string().map_err(|e| ApiError::Other(no_answer(&url, e)))?;
        serde_json::from_str(&text).map_err(|e| ApiError::Other(format!("GitHub's answer for {path} wasn't understood: {e}")))
    }

    /// The bytes behind `url`, refused past the size cap.
    fn download(&self, url: &str) -> Result<Vec<u8>, String> {
        self.download_limited(url, MAX_FILE)
    }

    /// A file of at most `limit` bytes, counted as they stream in.
    fn download_limited(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        let mut response =
            self.agent.get(url).config().timeout_global(Some(FILE_TIME)).build().call().map_err(|e| no_answer(url, e))?;
        match response.status().as_u16() {
            200 => response.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| no_answer(url, e)),
            status => Err(format!("GitHub answered {status}")),
        }
    }

    /// A release newer than the ones the Satchel knows for this addon: GitHub's latest stable
    /// release, when its tag is new and it holds the addon's file. `None`: nothing new (or only
    /// pre-releases, which the catalog brings).
    fn look(&self, watch: &Watch) -> Result<Option<Release>, String> {
        let Some(location) = self.location(&format!("https://github.com/{}/releases/latest", watch.repo))? else {
            return Ok(None);
        };
        let Some(tag) = github::latest_tag(&watch.repo, &location) else { return Ok(None) };
        if watch.known.iter().any(|k| *k == tag) {
            return Ok(None);
        }
        let page = self.text(&format!("https://github.com/{}/releases/expanded_assets/{}", watch.repo, segment(&tag)))?;
        let files = github::page_files(&watch.repo, &page);
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        // A release without the addon's file (DragonUI's updater releases) is nothing to install.
        Ok(github::pick(&names, &watch.pick).map(|i| github::page_release(&tag, &files[i], Utc::now())))
    }

    /// The Satchel's newest stable release in `repo` (or `want`'s release, a pre-release too),
    /// when it's newer than this one: its exe, downloaded into `dir` (once), checked against the
    /// checksum GitHub published for it and against the version it calls itself, and its signature
    /// verified with the release keys (`signing`). A release without that checksum or signature
    /// isn't offered: an exe is never run unchecked.
    fn satchel(&self, repo: &str, dir: &Path, want: Option<&str>) -> Result<Option<Offer>, String> {
        let (tag, version) = match want {
            Some(version) => (format!("v{version}"), version.to_string()),
            None => {
                let Some(location) = self.location(&format!("https://github.com/{repo}/releases/latest"))? else {
                    log!("the Satchel's GitHub releases: none published yet");
                    keep_only(dir, &[]);
                    return Ok(None);
                };
                let Some(tag) = github::latest_tag(repo, &location) else {
                    log!("the Satchel's newest GitHub release isn't one of {repo}'s ({location})");
                    return Ok(None);
                };
                let Some(version) = github::tag_version(&tag) else {
                    log!("the Satchel's newest GitHub release, {tag}, isn't named by a version");
                    return Ok(None);
                };
                (tag, version)
            }
        };
        if !gate::is_newer(&version, env!("CARGO_PKG_VERSION")) {
            keep_only(dir, &[]);
            return Ok(None);
        }
        let page = self.text(&format!("https://github.com/{repo}/releases/expanded_assets/{}", segment(&tag)))?;
        let files = github::page_files(repo, &page);
        let file = files
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(update::RELEASE_EXE))
            .ok_or_else(|| format!("its release {tag} has no {}", update::RELEASE_EXE))?;
        let expected = file.sha256.as_deref().ok_or_else(|| format!("GitHub published no checksum for the exe of {tag}"))?;
        // From 0.7.0 every release carries the maintainer's signature over the exe. A release
        // without one isn't offered, and its exe isn't even downloaded.
        let signed = files
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(signing::SIGNATURE_NAME))
            .ok_or_else(|| format!("its release {tag} carries no signature"))?;
        let path = dir.join(format!("SurvivorsSatchel-{}.exe", segment(&version)));
        let held = fsx::read_optional(&path).ok().flatten().is_some_and(|bytes| fsx::fingerprint(&bytes) == expected);
        if !held {
            let bytes = self.download(&file.url)?;
            if fsx::fingerprint(&bytes) != expected {
                log!("download of {} refused: its checksum isn't the published {expected}", file.url);
                return Err("the exe downloaded isn't the one published (its checksum differs), so it wasn't kept".into());
            }
            fsx::write_atomic(&path, &bytes).map_err(|e| format!("{}: {e}", dir.display()))?;
            log!("Satchel {version} downloaded from GitHub ({} KB)", bytes.len() / 1024);
        }
        // An exe calling itself another version would be refused at every Update now.
        let calls_itself = win::exe_version(&path);
        if calls_itself.as_deref() != Some(version.as_str()) {
            keep_only(dir, &[]);
            return Err(format!("the exe of {tag} calls itself {}, not {version}", calls_itself.as_deref().unwrap_or("nothing")));
        }
        // A signature that doesn't download refuses nothing: what an earlier look verified stays.
        // One that doesn't verify takes everything downloaded with it.
        let bytes = self.download_limited(&signed.url, signing::SIGNATURE_MAX)?;
        let signature = match accept_signature(signed, bytes, &tag, &version, &path) {
            Ok(signature) => signature,
            Err(e) => {
                keep_only(dir, &[]);
                return Err(e);
            }
        };
        keep_only(dir, &[path.as_path(), signature.as_path()]);
        Ok(Some(Offer { version, source: path, sha256: expected.to_string(), signature }))
    }

    /// Downloads into the cache, checked against the published checksum when there is one.
    fn fetch(&self, url: &str, expected: Option<&str>, cache: &Path) -> Result<String, String> {
        let bytes = self.download(url)?;
        let sha256 = fsx::fingerprint(&bytes);
        if expected.is_some_and(|e| !e.eq_ignore_ascii_case(&sha256)) {
            log!("download of {url} refused: its checksum is {sha256}, not {}", expected.unwrap_or_default());
            return Err("the file isn't the one its author published (its checksum differs), so it wasn't kept".into());
        }
        fsx::write_atomic(&cache.join(format!("{sha256}.zip")), &bytes).map_err(|e| format!("the addon cache: {e}"))?;
        log!("downloaded {url} ({} KB)", bytes.len() / 1024);
        Ok(sha256)
    }

    fn lookup(&self, input: &str, cache: &Path) -> Result<Lookup, String> {
        let repo = github::repo_from_input(input).ok_or("That isn't a GitHub link or an owner/name.")?;
        let (info, tag, file, url, published, released) = match self.api::<RestRepo>(&format!("/repos/{repo}")) {
            Ok(info) => {
                let repo = info.full_name.clone();
                let releases: Vec<RestRelease> = self.api(&format!("/repos/{repo}/releases?per_page=30")).map_err(api_error)?;
                match github::latest_release(&releases) {
                    Some(release) => {
                        let names: Vec<&str> = release.assets.iter().map(|a| a.name.as_str()).collect();
                        let Some(i) = github::pick(&names, &Pick::Like(None)) else {
                            return Err(format!("Its latest release ({}) has no single zip file that is clearly the addon.", release.tag_name));
                        };
                        let asset = &release.assets[i];
                        let tag = release.tag_name.clone();
                        (Some(info), tag, asset.name.clone(), asset.browser_download_url.clone(), asset.sha256(), release.published_at)
                    }
                    None => {
                        // No releases: the newest version tag, as the repository's zip at its commit.
                        let tags: Vec<RestTag> = self.api(&format!("/repos/{repo}/tags?per_page=100")).map_err(api_error)?;
                        let Some(tag) = github::newest_version_tag(&tags) else {
                            return Err("It has no releases and no version tags, so there's nothing to install.".into());
                        };
                        let name = repo.split('/').nth(1).unwrap_or("addon");
                        let url = format!("https://codeload.github.com/{repo}/zip/{}", tag.commit.sha);
                        (Some(info), tag.name.clone(), format!("{name}-{}.zip", tag.name), url, None, None)
                    }
                }
            }
            Err(ApiError::NotFound) => return Err("There's no public repository by that name on GitHub.".into()),
            Err(ApiError::Limited) => {
                log!("lookup of {repo}: GitHub's hour of API calls is used up; reading its pages instead");
                let watch = Watch { id: repo.clone(), repo: repo.clone(), pick: Pick::Like(None), known: Vec::new() };
                let release = self.look(&watch)?.ok_or(
                    "GitHub's lookups for this hour are used up, and its release page shows no stable release with a single zip. Try again in an hour.",
                )?;
                (None, release.tag.clone(), release.file.clone(), release.url.clone(), Some(release.sha256).filter(|s| !s.is_empty()), None)
            }
            Err(ApiError::Other(e)) => return Err(e),
        };

        let bytes = self.download(&url)?;
        let sha256 = fsx::fingerprint(&bytes);
        if published.as_deref().is_some_and(|p| *p != sha256) {
            return Err("The file isn't the one its author published (its checksum differs), so it wasn't kept.".into());
        }
        let found = package::inspect(&bytes).map_err(|e| format!("Its zip can't be installed: {e}."))?;
        fsx::write_atomic(&cache.join(format!("{sha256}.zip")), &bytes).map_err(|e| format!("the addon cache: {e}"))?;
        let first = found.folders.first().map(|f| &f.toc);
        Ok(Lookup {
            owner: info.as_ref().map(|i| i.owner.login.clone()).unwrap_or_else(|| repo.split('/').next().unwrap_or_default().into()),
            repo: info.as_ref().map(|i| i.full_name.clone()).unwrap_or(repo),
            description: info.as_ref().and_then(|i| i.description.clone()),
            stars: info.as_ref().map(|i| i.stargazers_count),
            pushed: info.as_ref().and_then(|i| i.pushed_at),
            license: info.as_ref().and_then(|i| i.license.as_ref()).and_then(|l| l.shown()),
            tag,
            released,
            file,
            checked: published.is_some(),
            sha256,
            size: bytes.len() as u64,
            folders: found.folders.iter().map(|f| f.name.clone()).collect(),
            left_out: found.left_out.len(),
            title: first.and_then(|t| t.title.as_deref()).map(plain).filter(|t| !t.is_empty()),
            notes: first.and_then(|t| t.notes.as_deref()).map(plain).filter(|t| !t.is_empty()),
            version: first.and_then(|t| t.version.clone()),
            current: found.folders.iter().all(|f| f.toc.current()),
        })
    }
}

fn api_error(e: ApiError) -> String {
    match e {
        ApiError::NotFound => "GitHub doesn't have that anymore.".into(),
        ApiError::Limited => "GitHub's lookups for this hour are used up. Try again in an hour.".into(),
        ApiError::Other(e) => e,
    }
}
