//! The Satchel updating itself, from the exe beside `sync\` (where a family's builds are put) or
//! from its GitHub release once the repository is public (downloaded by the network thread): the
//! new exe is copied beside the running one and checked, the running one moves aside, the new one
//! takes its name and starts, and this one quits. The new one waits for it to close, then clears
//! the copy that moved aside.

use crate::win;
use satchel_core::{fsx, gate, log};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The exe's name in the shared folder, beside `sync\`.
pub const SHARED_EXE: &str = "SurvivorsSatchel.exe";

/// A newer Satchel found beside `sync\`, or downloaded from its GitHub release.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub version: String,
    pub source: PathBuf,
    pub from_github: bool,
    /// The checksum GitHub published for it: the copy is checked against it again at Update now.
    pub sha256: Option<String>,
}

impl Offer {
    /// "0.6.2 is ready, downloaded from GitHub" or "0.6.2 is ready beside sync\": the words every
    /// update line uses for the offer, the comma included.
    pub fn ready(&self) -> String {
        if self.from_github {
            format!("{} is ready, downloaded from GitHub", self.version)
        } else {
            format!("{} is ready beside sync\\", self.version)
        }
    }
}

/// The exe beside `sync\` as last seen (size, time, version): its version is read again only
/// when the file changes.
#[derive(Default)]
pub struct Watch {
    seen: Option<(u64, SystemTime, Option<String>)>,
}

impl Watch {
    /// A Satchel newer than `this` in the shared `folder`, if there is one.
    pub fn look(&mut self, folder: &Path, this: &str) -> Option<Offer> {
        let source = folder.join(SHARED_EXE);
        let meta = fs::metadata(&source).ok()?;
        let (len, at) = (meta.len(), meta.modified().ok()?);
        let version = match &self.seen {
            Some((l, t, version)) if (*l, *t) == (len, at) => version.clone(),
            _ => {
                // A read that failed (the share busy, the file being replaced) is tried again
                // at the next look.
                let version = win::exe_version(&source);
                if version.is_some() {
                    self.seen = Some((len, at, version.clone()));
                }
                version
            }
        };
        let version = version?;
        gate::is_newer(&version, this).then_some(Offer { version, source, from_github: false, sha256: None })
    }
}

/// The newer of two offers; GitHub's when they're the same version: it's already on this PC,
/// and checked against its published checksum.
pub fn newer<'a>(shared: Option<&'a Offer>, github: Option<&'a Offer>) -> Option<&'a Offer> {
    match (shared, github) {
        (Some(s), Some(g)) if gate::is_newer(&s.version, &g.version) => Some(s),
        (_, Some(g)) => Some(g),
        (s, None) => s,
    }
}

fn beside(me: &Path, ending: &str) -> PathBuf {
    let mut name = me.file_name().unwrap_or_default().to_os_string();
    name.push(ending);
    me.with_file_name(name)
}

/// Copies the offered Satchel beside this one, checks it, swaps it in and starts it (playing,
/// with `then_play`). On `Ok` the caller quits; on an error this Satchel is as it was.
pub fn install(offer: &Offer, then_play: bool) -> io::Result<()> {
    let me = std::env::current_exe()?;
    // A Satchel started from a network folder (the shared one, say) is never swapped there.
    if win::network_path(&me).to_string_lossy().starts_with(r"\\") {
        return Err(io::Error::other("this Satchel runs from a network folder, not from this PC"));
    }
    let (incoming, aside) = (beside(&me, ".new"), beside(&me, ".old"));
    let size = fs::metadata(&offer.source)?.len();
    fs::copy(&offer.source, &incoming)?;
    if let Some(expected) = &offer.sha256
        && fs::read(&incoming).map(|bytes| fsx::fingerprint(&bytes)).ok().as_ref() != Some(expected)
    {
        let _ = fs::remove_file(&incoming);
        return Err(io::Error::other("the copy isn't the exe GitHub published (its checksum differs)"));
    }
    let copied = win::exe_version(&incoming);
    let copied_size = fs::metadata(&incoming).map(|m| m.len()).unwrap_or(0);
    if copied.as_deref() != Some(offer.version.as_str()) || copied_size != size {
        let _ = fs::remove_file(&incoming);
        return Err(io::Error::other(format!(
            "the copy reads as {} ({copied_size} bytes) instead of {} ({size} bytes)",
            copied.as_deref().unwrap_or("no version"),
            offer.version
        )));
    }
    if let Err(e) = win::swap_in_exe(&me, &incoming, &aside) {
        // When this exe couldn't be put back either, the new one stays for a hand copy.
        if me.exists() {
            let _ = fs::remove_file(&incoming);
        }
        return Err(e);
    }
    log!("Satchel {} put in place of this one: {}", offer.version, me.display());
    // The new one waits for this one to close; one that ends before that couldn't run here, and
    // this one goes back in place.
    let mut child = match win::start_updated(&me, then_play) {
        Ok(child) => child,
        Err(e) => return Err(put_back(&me, &incoming, &aside, format!("the new Satchel didn't start ({e})"))),
    };
    for _ in 0..25 {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(put_back(&me, &incoming, &aside, format!("the new Satchel couldn't run on this PC ({status})")));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// This Satchel back where it was, the new one aside as `.new`: the error to show.
fn put_back(me: &Path, incoming: &Path, aside: &Path, why: String) -> io::Error {
    match win::swap_in_exe(me, aside, incoming) {
        Ok(()) => {
            log!("{why}; this Satchel is back in place");
            io::Error::other(why)
        }
        Err(e) => {
            log!("{why}; and this Satchel couldn't be put back: {e}");
            io::Error::other(format!("{why}; to go back, rename {} to {}", aside.display(), me.display()))
        }
    }
}

/// In a Satchel an update started: the copy that moved aside goes, once the old one has closed.
pub fn clear_aside() {
    let Ok(me) = std::env::current_exe() else { return };
    let aside = beside(&me, ".old");
    for _ in 0..20 {
        match fs::remove_file(&aside) {
            Ok(()) => return,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
    log!("{} couldn't be removed; the next update replaces it", aside.display());
}
