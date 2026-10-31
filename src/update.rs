//! The Satchel updating itself from its GitHub release, which the network thread downloads,
//! checks against the checksum GitHub published and verifies with the release keys (`signing`):
//! the new exe is copied beside the running one and checked again, the running one moves aside,
//! the new one takes its name and starts, and this one quits. The new one waits for it to close,
//! then clears the copy that moved aside. Updates come only from GitHub: since 0.7.0 nothing is
//! taken from the network folder.

use crate::win;
use satchel_core::{fsx, log, signing};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The exe's name among a release's files.
pub const RELEASE_EXE: &str = "SurvivorsSatchel.exe";

/// A newer Satchel from its GitHub release, in this PC's update folder: checked against the
/// checksum GitHub published for it and the version it calls itself, its signature verified.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub version: String,
    pub source: PathBuf,
    /// The checksum GitHub published for it: the copy is checked against it again at Update now.
    pub sha256: String,
    /// Its signature, beside it: verified at download and again over the copy at Update now.
    pub signature: PathBuf,
}

impl Offer {
    /// "0.7.1 is ready, downloaded from GitHub": the words every update line uses for the offer,
    /// the comma included.
    pub fn ready(&self) -> String {
        format!("{} is ready, downloaded from GitHub", self.version)
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
    let copied_sha256 = fs::read(&incoming).map(|bytes| fsx::fingerprint(&bytes)).ok();
    if copied_sha256.as_deref() != Some(offer.sha256.as_str()) {
        let _ = fs::remove_file(&incoming);
        return Err(io::Error::other("the copy isn't the exe GitHub published (its checksum differs)"));
    }
    // The signature is checked again over the copy, so what runs is what was signed.
    if let Err(why) = check_signature(&incoming, &offer.signature, &offer.version) {
        let _ = fs::remove_file(&incoming);
        return Err(io::Error::other(format!("the copy was refused: {why}")));
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

/// The signature at `signature` verified over the exe at `exe`, release `version`: the key's id.
fn check_signature(exe: &Path, signature: &Path, version: &str) -> Result<&'static str, String> {
    let len = fs::metadata(signature).map_err(|e| format!("its signature: {e}"))?.len();
    if len > signing::SIGNATURE_MAX {
        return Err("its signature file is too big to be one".into());
    }
    let text = fs::read_to_string(signature).map_err(|e| format!("its signature: {e}"))?;
    let bytes = fs::read(exe).map_err(|e| format!("the copy: {e}"))?;
    let key = signing::verify(&bytes, &text, version)?;
    log!("Satchel {version}'s copy verified with the release key {key}");
    Ok(key)
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
