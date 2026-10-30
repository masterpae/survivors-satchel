//! Which releases go into the catalog. Newest first by the time they were published, because
//! GitHub's own list is not in that order. The latest is the newest stable release, or the newest
//! pre-release for an author who publishes nothing else; the earlier ones are those published
//! before it, up to a count. A count, not an age: an addon that stops releasing keeps its history,
//! so a copy installed by hand is still recognised. A pre-release newer than the latest stable one
//! is left out.

use chrono::{DateTime, Utc};

/// A release that holds the addon, before its file is read.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub tag: String,
    pub published: DateTime<Utc>,
    pub prerelease: bool,
    pub file: String,
    pub url: String,
    /// GitHub's SHA-256 of the file, when it published one.
    pub sha256: Option<String>,
    pub notes: Option<String>,
}

/// The latest release and at most `max_earlier` releases published before it, newest first.
pub fn pick(mut all: Vec<Candidate>, max_earlier: usize) -> Option<(Candidate, Vec<Candidate>)> {
    all.sort_by(|a, b| b.published.cmp(&a.published));
    let latest = all.iter().find(|c| !c.prerelease).or(all.first())?.clone();
    let earlier = all.into_iter().filter(|c| c.published < latest.published).take(max_earlier).collect();
    Some((latest, earlier))
}

/// A tag's version as numbers, `v2.7.4-restored` → [2, 7, 4], to put tags newest first before
/// their dates are known. A tag without a number sorts last.
pub fn version_of(tag: &str) -> Vec<u64> {
    let start = tag.find(|c: char| c.is_ascii_digit()).unwrap_or(tag.len());
    tag[start..]
        .split(|c: char| !c.is_ascii_digit())
        .take_while(|n| !n.is_empty())
        .map(|n| n.parse().unwrap_or(u64::MAX))
        .collect()
}

/// The author's release notes, cut at a line end to at most `max` characters ("…" when cut).
pub fn trim_notes(notes: &str, max: usize) -> Option<String> {
    let notes = notes.trim();
    if notes.is_empty() {
        return None;
    }
    if notes.chars().count() <= max {
        return Some(notes.to_string());
    }
    let cut: String = notes.chars().take(max).collect();
    let cut = cut.rfind('\n').map_or(cut.as_str(), |at| &cut[..at]).trim_end();
    Some(format!("{cut}\n…"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0).unwrap()
    }

    fn release(tag: &str, day: u32, prerelease: bool) -> Candidate {
        Candidate {
            tag: tag.into(),
            published: at(day),
            prerelease,
            file: format!("A-{tag}.zip"),
            url: format!("https://github.com/o/a/releases/download/{tag}/A-{tag}.zip"),
            sha256: None,
            notes: None,
        }
    }

    fn tags(releases: &[Candidate]) -> Vec<&str> {
        releases.iter().map(|r| r.tag.as_str()).collect()
    }

    #[test]
    fn newest_by_publish_time_not_by_list_order() {
        // GitHub listed DragonUI's 3.2.57 first, before 3.2.64 and 3.2.58 (29 Oct).
        let listed = vec![release("3.2.57", 20, false), release("3.2.64", 28, false), release("3.2.58", 21, false)];
        let (latest, earlier) = pick(listed, 50).unwrap();
        assert_eq!(latest.tag, "3.2.64");
        assert_eq!(tags(&earlier), ["3.2.58", "3.2.57"]);
    }

    #[test]
    fn the_latest_is_stable_unless_there_is_none() {
        let mixed = vec![release("v4.6.0-beta.1", 28, true), release("v4.5.0", 27, false), release("v4.4.0-beta.1", 26, true)];
        let (latest, earlier) = pick(mixed, 50).unwrap();
        assert_eq!(latest.tag, "v4.5.0");
        // The newer beta is neither the latest nor an earlier release; the older one is earlier.
        assert_eq!(tags(&earlier), ["v4.4.0-beta.1"]);

        let betas_only = vec![release("v1.6.2-beta.1", 27, true), release("v1.7.0-beta.1", 28, true)];
        let (latest, earlier) = pick(betas_only, 50).unwrap();
        assert!(latest.prerelease && latest.tag == "v1.7.0-beta.1");
        assert_eq!(tags(&earlier), ["v1.6.2-beta.1"]);

        assert!(pick(Vec::new(), 50).is_none());
    }

    #[test]
    fn earlier_releases_are_capped_by_count_not_age() {
        let many: Vec<Candidate> = (1..=28).map(|day| release(&format!("v{day}"), day, false)).collect();
        let (_, earlier) = pick(many.clone(), 3).unwrap();
        assert_eq!(tags(&earlier), ["v27", "v26", "v25"]);
        let (_, earlier) = pick(many, 50).unwrap();
        assert_eq!((earlier.len(), earlier.last().map(|r| r.tag.as_str())), (27, Some("v1")));
        // An addon that stopped releasing months ago keeps the releases before its latest.
        let january = Candidate { published: Utc.with_ymd_and_hms(2026, 1, 5, 12, 0, 0).unwrap(), ..release("v1.0", 1, false) };
        let (latest, earlier) = pick(vec![release("v1.1", 2, false), january], 50).unwrap();
        assert_eq!((latest.tag.as_str(), tags(&earlier)), ("v1.1", vec!["v1.0"]));
    }

    #[test]
    fn tags_by_version() {
        assert_eq!(version_of("v2.8.6"), [2, 8, 6]);
        assert_eq!(version_of("v2.7.4-restored"), [2, 7, 4]);
        assert_eq!(version_of("Version1.2.2"), [1, 2, 2]);
        assert!(version_of("FirstVersion").is_empty());
        let mut names = vec!["v2.7.10", "v2.0.0", "v2.8.6", "v2.7.9"];
        names.sort_by(|a, b| version_of(b).cmp(&version_of(a)));
        assert_eq!(names, ["v2.8.6", "v2.7.10", "v2.7.9", "v2.0.0"]);
    }

    #[test]
    fn notes_are_cut_at_a_line_end() {
        assert_eq!(trim_notes("  \n ", 10), None);
        assert_eq!(trim_notes("Fixed the meter.", 100).as_deref(), Some("Fixed the meter."));
        let long = "line one\nline two\nline three";
        assert_eq!(trim_notes(long, 20).as_deref(), Some("line one\nline two\n…"));
    }
}
