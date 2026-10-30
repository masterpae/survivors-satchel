//! The Satchel's own changelog, `CHANGELOG.md`, built into the exe: each version's date and what
//! changed, for Settings' About and the What's new panel. The same file becomes each GitHub
//! release's notes, so the app and GitHub tell one story.

use chrono::NaiveDate;

/// One block of a version's section, as the app shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Text under the version's heading, before its first list.
    Paragraph(String),
    /// "Added", "Changed", "Fixed".
    Heading(String),
    Bullet(String),
    /// A bullet inside a bullet.
    Inner(String),
}

/// One version's section: `## 0.4.0 (2026-10-30)` and what follows, up to the next version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub version: String,
    /// When it was released; `None` for a heading without a date.
    pub date: Option<NaiveDate>,
    pub blocks: Vec<Block>,
}

/// Every version in the file, newest first as the file has them. **Unreleased** is left out: it
/// isn't a version anyone has.
pub fn versions(markdown: &str) -> Vec<Version> {
    let mut out: Vec<Version> = Vec::new();
    // Inside a section that isn't a version (the title's, Unreleased): its lines are skipped.
    let mut skipping = true;
    // The line before was blank or a heading: text starts a new paragraph instead of going on.
    let mut fresh = true;
    for line in markdown.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            skipping = true;
            fresh = true;
            if let Some(version) = heading_version(heading) {
                out.push(version);
                skipping = false;
            }
            continue;
        }
        if skipping {
            continue;
        }
        let Some(current) = out.last_mut() else { continue };
        let line = line.trim_end();
        if line.is_empty() {
            fresh = true;
            continue;
        }
        if let Some(heading) = line.strip_prefix("### ") {
            current.blocks.push(Block::Heading(plain(heading)));
            fresh = true;
            continue;
        }
        let indented = line.starts_with(' ');
        let rest = line.trim_start();
        match rest.strip_prefix("- ") {
            Some(text) if indented => current.blocks.push(Block::Inner(plain(text))),
            Some(text) => current.blocks.push(Block::Bullet(plain(text))),
            // A wrapped line goes on with the block before it.
            None => match current.blocks.last_mut() {
                Some(Block::Paragraph(p) | Block::Bullet(p) | Block::Inner(p)) if !fresh => {
                    p.push(' ');
                    p.push_str(&plain(rest));
                }
                _ => current.blocks.push(Block::Paragraph(plain(rest))),
            },
        }
        fresh = false;
    }
    out
}

/// `0.4.0 (2026-10-30)` → the version and its date; `None` for `Unreleased` or anything that
/// doesn't start with a version number.
fn heading_version(heading: &str) -> Option<Version> {
    let heading = heading.trim();
    let (version, rest) = heading.split_once(' ').unwrap_or((heading, ""));
    if !version.starts_with(|c: char| c.is_ascii_digit()) || !version.chars().all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c)) {
        return None;
    }
    let date = rest.trim().strip_prefix('(').and_then(|r| r.strip_suffix(')')).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    Some(Version { version: version.to_string(), date, blocks: Vec::new() })
}

/// One line with Markdown's marks dropped: `**bold**`, `` `code` ``, and a `\` before a mark.
/// Inside a code span everything is literal (`` `sync\` `` keeps its backslash), as GitHub shows it.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut code = false;
    while let Some(c) = chars.next() {
        match c {
            '`' => code = !code,
            _ if code => out.push(c),
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
            }
            '\\' if chars.peek().is_some_and(|n| "*`_\\".contains(*n)) => {
                out.extend(chars.next());
            }
            _ => out.push(c),
        }
    }
    out
}

/// The section of `version`, when the file has one.
pub fn find<'a>(versions: &'a [Version], version: &str) -> Option<&'a Version> {
    versions.iter().find(|v| v.version == version)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog\n\nWhat changed, newest first.\n\n## Unreleased\n\n- not yet\n\n## 0.3.1 (2026-10-30)\n\nThe same addons on every PC.\n\n### Changed\n- Settings has **Default** and\n  **Save**, with `saved-form`.\n  - an inner point\n    that wraps\n- A second point.\n\n### Fixed\n- The log says \\*one\\* thing.\n\n## 0.1.0 (2026-10-29)\n\n### Added\n- The first version.\n";

    #[test]
    fn sections_newest_first_without_unreleased() {
        let all = versions(SAMPLE);
        assert_eq!(all.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(), ["0.3.1", "0.1.0"]);
        assert_eq!(all[0].date, NaiveDate::from_ymd_opt(2026, 10, 30));
        assert_eq!(
            all[0].blocks,
            [
                Block::Paragraph("The same addons on every PC.".into()),
                Block::Heading("Changed".into()),
                Block::Bullet("Settings has Default and Save, with saved-form.".into()),
                Block::Inner("an inner point that wraps".into()),
                Block::Bullet("A second point.".into()),
                Block::Heading("Fixed".into()),
                Block::Bullet("The log says *one* thing.".into()),
            ]
        );
        assert_eq!(find(&all, "0.1.0").unwrap().blocks, [Block::Heading("Added".into()), Block::Bullet("The first version.".into())]);
        assert!(find(&all, "0.2.0").is_none());
    }

    /// A backslash inside backticks is part of the text (`sync\`), not an escape; marks inside
    /// backticks are literal too. Outside, `\*` is still an escaped star.
    #[test]
    fn a_backslash_inside_a_code_span_is_literal() {
        assert_eq!(plain("the exe beside `sync\\` right away"), "the exe beside sync\\ right away");
        assert_eq!(plain("`**not bold**` and \\*one\\*"), "**not bold** and *one*");
    }

    /// `plain` runs per line, so a code span that wrapped onto the next line would leave its
    /// closing backtick to open a new one there. Every line of CHANGELOG.md closes its spans.
    #[test]
    fn the_changelog_never_wraps_a_code_span() {
        for (n, line) in include_str!("../../../CHANGELOG.md").lines().enumerate() {
            assert_eq!(line.matches('`').count() % 2, 0, "CHANGELOG.md line {} wraps a code span", n + 1);
        }
    }

    #[test]
    fn a_heading_without_a_date_has_none() {
        let all = versions("## 1.0.0\n- x\n## Notes\n- not a version\n");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].date, None);
        assert_eq!(all[0].blocks, [Block::Bullet("x".into())]);
    }

    /// Every version built has its section, with the day it was released: a build without
    /// one fails here, before it can ship.
    #[test]
    fn the_version_being_built_has_a_dated_section() {
        let manifest = include_str!("../../../Cargo.toml");
        let package = manifest.split("[package]").nth(1).expect("the app's [package]");
        let version = package
            .lines()
            .find_map(|l| l.trim().strip_prefix("version = \"").and_then(|v| v.strip_suffix('"')))
            .expect("the app's version");
        let all = versions(include_str!("../../../CHANGELOG.md"));
        let section = find(&all, version).unwrap_or_else(|| panic!("CHANGELOG.md has no section for {version}"));
        assert!(section.date.is_some(), "CHANGELOG.md's {version} has no date");
        assert!(!section.blocks.is_empty(), "CHANGELOG.md's {version} says nothing");
        assert_eq!(all[0].version, version, "the version being built comes first in CHANGELOG.md");
    }
}
