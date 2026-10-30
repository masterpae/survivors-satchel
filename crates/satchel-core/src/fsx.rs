//! Fingerprints, and writes that never leave a half-written file behind.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

/// How many more reads `read_settled` makes before it gives up on a file that keeps changing.
const SETTLE_TRIES: usize = 5;

/// SHA-256 of the bytes in lowercase hex: same fingerprint, same save.
pub fn fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest.iter() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn temp_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".satchel-tmp");
    target.with_file_name(name)
}

/// Writes to a temporary name beside the target, reads it back to check its fingerprint,
/// then renames it over the target in one move. When the move can't happen (on Windows, a file
/// someone holds open), the target stays as it was and the temporary file goes.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = temp_path(target);
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if fingerprint(&fs::read(&tmp)?) != fingerprint(bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} did not read back intact", tmp.display()),
        ));
    }
    fs::rename(&tmp, target).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Reads a file another program may be writing: two reads `gap` apart must agree, since a read
/// that overlaps a write can catch a torn file. `None` when it does not exist.
pub fn read_settled(path: &Path, gap: Duration) -> io::Result<Option<Vec<u8>>> {
    match settle(|| read_optional(path), gap)? {
        Some(bytes) => Ok(bytes),
        None => {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            Err(io::Error::other(format!("{name} kept changing while it was read")))
        }
    }
}

/// Calls `read` until two calls `gap` apart agree: `Some` with what they read, `None` when no
/// two did.
fn settle<T: PartialEq>(mut read: impl FnMut() -> io::Result<T>, gap: Duration) -> io::Result<Option<T>> {
    let mut last = read()?;
    for _ in 0..SETTLE_TRIES {
        thread::sleep(gap);
        let next = read()?;
        if next == last {
            return Ok(Some(next));
        }
        last = next;
    }
    Ok(None)
}

/// Reads a file and checks it against the fingerprint it is supposed to have.
pub fn read_verified(path: &Path, sha256: &str) -> io::Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    if fingerprint(&bytes) != sha256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} does not match its fingerprint", path.display()),
        ));
    }
    Ok(bytes)
}

/// Reads a file, or `None` when it does not exist.
pub fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Removes a file; a file that is already gone is fine.
pub fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Removes a folder and everything in it; one that is already gone is fine.
pub fn remove_dir_if_present(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Moves a folder in one rename; across drives (the game on another drive than the Satchel's
/// files) it is copied under a temporary name, renamed into place, and only then removed.
pub fn move_dir(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(from, to) {
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            let tmp = temp_path(to);
            remove_dir_if_present(&tmp)?;
            copy_dir(from, &tmp).and_then(|()| fs::rename(&tmp, to)).inspect_err(|_| {
                let _ = remove_dir_if_present(&tmp);
            })?;
            fs::remove_dir_all(from)
        }
        other => other,
    }
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Keeps the `keep` newest files in `dir` (by name, which starts with a sortable date) and
/// deletes the rest.
pub fn prune(dir: &Path, keep: usize) -> io::Result<()> {
    let mut names: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    names.sort();
    let excess = names.len().saturating_sub(keep);
    for path in names.into_iter().take(excess) {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Keeps the `keep` newest folders in `dir` whose name ends with `suffix` (by name, which starts
/// with a sortable date) and deletes the rest; other folders and files stay.
pub fn prune_dirs(dir: &Path, suffix: &str, keep: usize) -> io::Result<()> {
    let mut names: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(suffix))
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    names.sort();
    let excess = names.len().saturating_sub(keep);
    for path in names.into_iter().take(excess) {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_sha256_hex() {
        assert_eq!(
            fingerprint(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sub").join("characters.txt");
        write_atomic(&target, b"one").unwrap();
        write_atomic(&target, b"two").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(target.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }

    #[test]
    fn a_move_that_cant_happen_leaves_the_target_and_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        // A folder in the target's place stands for a file held open: the rename fails.
        let target = dir.path().join("characters.txt");
        fs::create_dir_all(target.join("held")).unwrap();
        assert!(write_atomic(&target, b"new").is_err());
        assert!(target.join("held").is_dir());
        assert!(!temp_path(&target).exists());
    }

    #[test]
    fn folders_move_whole_and_copies_are_exact() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a");
        fs::create_dir_all(from.join("sub")).unwrap();
        fs::write(from.join("sub").join("x.lua"), b"x").unwrap();
        let to = dir.path().join("deep").join("b");
        move_dir(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(fs::read(to.join("sub").join("x.lua")).unwrap(), b"x");
        let copy = dir.path().join("copy");
        copy_dir(&to, &copy).unwrap();
        assert_eq!(fs::read(copy.join("sub").join("x.lua")).unwrap(), b"x");
        remove_dir_if_present(&copy).unwrap();
        remove_dir_if_present(&copy).unwrap();
        assert!(!copy.exists());
    }

    #[test]
    fn settled_reads_wait_for_two_that_agree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("characters.txt");
        assert_eq!(read_settled(&path, Duration::ZERO).unwrap(), None);
        fs::write(&path, b"steady").unwrap();
        assert_eq!(read_settled(&path, Duration::from_millis(5)).unwrap().as_deref(), Some(&b"steady"[..]));

        // A read that caught a write in progress is followed by ones that agree.
        let mut reads = ["torn", "whole", "whole"].into_iter();
        assert_eq!(settle(|| Ok(reads.next().unwrap()), Duration::ZERO).unwrap(), Some("whole"));
        // A file that never stops changing: nothing is returned as read.
        let mut n = 0;
        assert_eq!(
            settle(
                || {
                    n += 1;
                    Ok(n)
                },
                Duration::ZERO
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn read_verified_rejects_other_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.sav");
        fs::write(&path, b"save").unwrap();
        assert!(read_verified(&path, &fingerprint(b"save")).is_ok());
        assert!(read_verified(&path, &fingerprint(b"other")).is_err());
    }

    #[test]
    fn prune_keeps_the_newest_names() {
        let dir = tempfile::tempdir().unwrap();
        for day in 1..=5 {
            fs::write(dir.path().join(format!("Before sync 2026-10-0{day}.sav")), b"x").unwrap();
        }
        prune(dir.path(), 2).unwrap();
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["Before sync 2026-10-04.sav", "Before sync 2026-10-05.sav"]);
    }

    #[test]
    fn prune_dirs_keeps_the_newest_of_one_set() {
        let dir = tempfile::tempdir().unwrap();
        for day in 1..=4 {
            fs::create_dir_all(dir.path().join(format!("2026-10-0{day} 10-00-00 _game settings")).join("saved")).unwrap();
        }
        fs::create_dir(dir.path().join("2026-10-01 10-00-00 lucky-odds settings")).unwrap();
        fs::write(dir.path().join("2026-10-01 note _game settings"), b"a file").unwrap();
        prune_dirs(dir.path(), " _game settings", 2).unwrap();
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "2026-10-01 10-00-00 lucky-odds settings",
                "2026-10-01 note _game settings",
                "2026-10-03 10-00-00 _game settings",
                "2026-10-04 10-00-00 _game settings",
            ]
        );
    }
}
