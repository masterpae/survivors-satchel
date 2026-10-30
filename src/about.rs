//! The Satchel itself: its version, the day it was released, the build it came from, and what
//! changed in every version (CHANGELOG.md, built into the exe; the same file becomes each GitHub
//! release's notes).

use chrono::NaiveDate;
use satchel_core::changelog::{self, Version};
use std::sync::OnceLock;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const CHANGELOG: &str = include_str!("../CHANGELOG.md");
/// The repository the Satchel is published from, `owner/name`, given to the build (the release
/// workflow sets `SATCHEL_REPOSITORY`). Without it the app makes no GitHub calls about itself,
/// and About shows no links to it.
pub const REPOSITORY: &str = match option_env!("SATCHEL_REPOSITORY") {
    Some(repository) => repository,
    None => "",
};

pub fn versions() -> &'static [Version] {
    static VERSIONS: OnceLock<Vec<Version>> = OnceLock::new();
    VERSIONS.get_or_init(|| changelog::versions(CHANGELOG))
}

/// The day this version was released, from its CHANGELOG heading.
pub fn released() -> Option<NaiveDate> {
    changelog::find(versions(), VERSION)?.date
}

/// The build, for About and for a problem report: the commit it came from, when the build was
/// told (`SATCHEL_GIT_COMMIT`, which the GitHub workflows set).
pub fn build() -> String {
    match option_env!("SATCHEL_COMMIT") {
        Some(commit) => {
            let (id, dirty) = match commit.strip_suffix("+dirty") {
                Some(id) => (id, " with changes not yet committed"),
                None => (commit, ""),
            };
            format!("Built from {}{dirty}", id.chars().take(7).collect::<String>())
        }
        None => "A local build".into(),
    }
}

/// The repository's page, when there is one.
pub fn repository_url() -> Option<String> {
    (!REPOSITORY.is_empty()).then(|| format!("https://github.com/{REPOSITORY}"))
}

/// About's Report a problem: the repository's bug report form with this version filled in, when
/// there is a repository (the URL's shape and its test live in satchel-core, which `test` builds).
pub fn issue_url() -> Option<String> {
    (!REPOSITORY.is_empty()).then(|| satchel_core::addons::github::issue_url(REPOSITORY, VERSION))
}
