//! An addon folder's fingerprint: one SHA-256 over its addon files' paths and contents. Equal
//! fingerprints mean the folder is exactly a known release; a different one means its files were
//! changed (by the player or another tool). Paths compare without case, as Windows does, and
//! files of other types (a `Thumbs.db`, a tool's notes) don't count.

use super::is_addon_file;
use crate::fsx;
use std::fs;
use std::io;
use std::path::Path;

/// Combines `(path inside the folder, SHA-256 of its content)` pairs into the folder's fingerprint.
pub fn combine(mut files: Vec<(String, String)>) -> String {
    for (path, _) in files.iter_mut() {
        *path = path.replace('\\', "/").to_lowercase();
    }
    files.sort();
    let mut listing = String::new();
    for (path, sha) in files {
        listing.push_str(&path);
        listing.push('\t');
        listing.push_str(&sha);
        listing.push('\n');
    }
    fsx::fingerprint(listing.as_bytes())
}

/// The fingerprint of an addon folder on disk.
pub fn fingerprint_dir(dir: &Path) -> io::Result<String> {
    let mut files = Vec::new();
    walk(dir, "", &mut files)?;
    Ok(combine(files))
}

fn walk(dir: &Path, prefix: &str, files: &mut Vec<(String, String)>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&entry.path(), &path, files)?;
        } else if kind.is_file() && is_addon_file(&name) {
            files.push((path, fsx::fingerprint(&fs::read(entry.path())?)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_files_same_fingerprint_whatever_the_case_or_extras() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one");
        fs::create_dir_all(one.join("Media")).unwrap();
        fs::write(one.join("A.toc"), b"toc").unwrap();
        fs::write(one.join("Media").join("x.blp"), b"img").unwrap();
        let print = fingerprint_dir(&one).unwrap();
        assert_eq!(
            print,
            combine(vec![("a.TOC".into(), fsx::fingerprint(b"toc")), ("media\\X.BLP".into(), fsx::fingerprint(b"img"))])
        );
        fs::write(one.join("Thumbs.db"), b"windows").unwrap();
        assert_eq!(fingerprint_dir(&one).unwrap(), print, "other file types don't count");
        fs::write(one.join("Media").join("x.blp"), b"changed").unwrap();
        assert_ne!(fingerprint_dir(&one).unwrap(), print);
    }
}
