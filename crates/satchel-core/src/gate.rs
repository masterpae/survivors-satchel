//! The version gate. Each shared file (the save index, the addon list) records the newest
//! Satchel that wrote it; an older Satchel refuses to read it, so it syncs nothing until it is
//! updated: a newer Satchel may have written what an older one would misread, or drop when it
//! writes the file back.

use std::fmt;
use std::io;
use std::sync::OnceLock;

static THIS: OnceLock<String> = OnceLock::new();

/// This Satchel's version, set once by the app at start. Left unset (the tests), nothing is
/// stamped or refused.
pub fn set_this(version: &str) {
    let _ = THIS.set(version.to_string());
}

pub fn this() -> Option<&'static str> {
    THIS.get().map(String::as_str)
}

/// A newer Satchel wrote the shared file: this one stays out of it until it is updated.
#[derive(Clone, Debug, PartialEq)]
pub struct NewerSatchel {
    pub newer: String,
    pub this: String,
}

impl fmt::Display for NewerSatchel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "another PC uses Survivor's Satchel {}; this one is {}: nothing syncs until it is updated", self.newer, self.this)
    }
}

impl std::error::Error for NewerSatchel {}

/// The gate's refusal inside an error, if that is what it is.
pub fn newer_satchel(e: &io::Error) -> Option<&NewerSatchel> {
    e.get_ref().and_then(|inner| inner.downcast_ref::<NewerSatchel>())
}

/// `Err` when `newest` (as read from a shared file) is newer than this Satchel.
pub fn check(newest: Option<&str>) -> io::Result<()> {
    check_as(this(), newest)
}

pub fn check_as(this: Option<&str>, newest: Option<&str>) -> io::Result<()> {
    match (this, newest) {
        (Some(mine), Some(theirs)) if is_newer(theirs, mine) => {
            Err(io::Error::other(NewerSatchel { newer: theirs.to_string(), this: mine.to_string() }))
        }
        _ => Ok(()),
    }
}

/// What a shared file records after this Satchel writes it: the newer of the two.
pub fn stamp(newest: Option<&str>) -> Option<String> {
    stamp_as(this(), newest)
}

pub fn stamp_as(this: Option<&str>, newest: Option<&str>) -> Option<String> {
    match (this, newest) {
        (Some(mine), Some(theirs)) if is_newer(theirs, mine) => Some(theirs.to_string()),
        (Some(mine), _) => Some(mine.to_string()),
        (None, theirs) => theirs.map(str::to_string),
    }
}

/// `a` is a later version than `b` ("0.4.10" after "0.4.9", "0.5.0" after "0.5.0-rc.1"). A
/// version that doesn't read as numbers is never newer, so a damaged stamp can't lock anyone out.
pub fn is_newer(a: &str, b: &str) -> bool {
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// "0.4.1" → (0, 4, 1, 1); a pre-release ("0.5.0-rc.1") ends in 0, so it comes before its
/// release, and pre-releases of one version rank alike. A build tag (+…) is ignored.
fn parts(version: &str) -> Option<(u64, u64, u64, u8)> {
    let version = version.trim().split('+').next()?;
    let (core, release) = match version.split_once('-') {
        Some((core, _)) => (core, 0),
        None => (version, 1),
    };
    let mut numbers = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = numbers.next()??;
    let minor = numbers.next().unwrap_or(Some(0))?;
    let patch = numbers.next().unwrap_or(Some(0))?;
    numbers.next().is_none().then_some((major, minor, patch, release))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.4.2", "0.4.1"));
        assert!(is_newer("0.4.10", "0.4.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.4.1", "0.4.1"));
        assert!(!is_newer("0.4.0", "0.4.1"));
        assert!(!is_newer("0.5.0-rc.1", "0.5.0"));
        assert!(is_newer("0.5.0", "0.5.0-rc.1"));
        assert!(is_newer("0.5.0-rc.1", "0.4.9"));
        assert!(!is_newer("garbage", "0.4.1"));
        assert!(!is_newer("0.5.0", "garbage"));
        assert!(!is_newer("0.4.1.7", "0.4.1"));
    }

    #[test]
    fn an_older_satchel_is_refused_and_the_stamp_never_goes_down() {
        assert!(check_as(Some("0.4.1"), None).is_ok());
        assert!(check_as(Some("0.4.1"), Some("0.4.1")).is_ok());
        assert!(check_as(Some("0.4.2"), Some("0.4.1")).is_ok());
        assert!(check_as(None, Some("9.0.0")).is_ok());
        let refused = check_as(Some("0.4.1"), Some("0.4.2")).unwrap_err();
        assert_eq!(newer_satchel(&refused), Some(&NewerSatchel { newer: "0.4.2".into(), this: "0.4.1".into() }));
        assert!(refused.to_string().contains("0.4.2"));
        assert_eq!(stamp_as(Some("0.4.1"), None).as_deref(), Some("0.4.1"));
        assert_eq!(stamp_as(Some("0.4.1"), Some("0.4.0")).as_deref(), Some("0.4.1"));
        assert_eq!(stamp_as(Some("0.4.1"), Some("0.4.2")).as_deref(), Some("0.4.2"));
        assert_eq!(stamp_as(None, Some("0.4.0")).as_deref(), Some("0.4.0"));
        assert_eq!(stamp_as(None, None), None);
    }
}
