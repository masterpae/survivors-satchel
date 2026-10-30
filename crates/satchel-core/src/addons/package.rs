//! An addon zip, judged before anything is unpacked: its table of contents is read first, and a
//! path that climbs out, an absolute path, a link or a zip bomb refuses the whole package. Addon
//! folders are found the way the game's own installer finds them: a folder holding a `.toc` with
//! its own name, or its only `.toc` (a wrapper like `BigCursor-main/BigCursor.toc` is named after
//! the `.toc`). Only addon file types are ever written; anything else is left out and counted.

use super::toc::Toc;
use super::{is_addon_file, tree};
use crate::fsx;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::io::{self, Cursor, Read};
use std::path::Path;
use zip::ZipArchive;

/// More entries than any addon has (the largest seen holds 178).
const MAX_ENTRIES: usize = 20_000;
/// Per file, and for the whole package, unpacked.
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: u64 = 512 * 1024 * 1024;
/// A file that unpacks to more than this many times its packed size is a bomb, not an addon.
const MAX_RATIO: u64 = 200;

#[derive(Debug, PartialEq)]
pub enum PackageError {
    /// Not a zip, or a damaged one.
    Unreadable(String),
    /// Something in it could write outside its folder, or unpack into far too much.
    Unsafe(String),
    /// No folder in it is an addon the game would load.
    NoAddon,
    /// Two folders in it would be one addon.
    Duplicate(String),
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageError::Unreadable(why) => write!(f, "the file isn't a zip that can be read ({why})"),
            PackageError::Unsafe(why) => write!(f, "the zip was refused: {why}"),
            PackageError::NoAddon => write!(f, "the zip holds no addon folder (a folder with a .toc of its own name)"),
            PackageError::Duplicate(name) => write!(f, "the zip holds two folders for {name}"),
        }
    }
}

impl std::error::Error for PackageError {}

/// One addon folder in a package.
#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    /// The name it gets in AddOns: its `.toc`'s name.
    pub name: String,
    /// Where it sits in the zip ("" for the top).
    pub source: String,
    /// The files taken: path inside the folder (with `/`) → entry index in the zip.
    pub files: BTreeMap<String, usize>,
    pub toc: Toc,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub folders: Vec<Folder>,
    /// Files not taken: other types (programs above all), or outside every addon folder.
    pub left_out: Vec<String>,
}

/// A zip entry's path made safe, or why it can't be.
fn clean(name: &str) -> Result<Option<String>, PackageError> {
    let unsafe_path = || PackageError::Unsafe(format!("a path that leaves its folder ({name})"));
    let name = name.replace('\\', "/");
    if name.contains('\0') || name.starts_with('/') || name.as_bytes().get(1) == Some(&b':') {
        return Err(unsafe_path());
    }
    let mut parts = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(unsafe_path()),
            part if part.contains(':') => return Err(unsafe_path()),
            part => parts.push(part),
        }
    }
    Ok((!parts.is_empty()).then(|| parts.join("/")))
}

/// Reads the zip's table of contents and finds the addon folders in it. Unpacks nothing.
pub fn inspect(zip: &[u8]) -> Result<Package, PackageError> {
    let mut archive = ZipArchive::new(Cursor::new(zip)).map_err(|e| PackageError::Unreadable(e.to_string()))?;
    if archive.len() > MAX_ENTRIES {
        return Err(PackageError::Unsafe(format!("{} files is far more than an addon has", archive.len())));
    }
    // Every entry is judged before anything else happens.
    let mut files: Vec<(String, usize)> = Vec::new();
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i).map_err(|e| PackageError::Unreadable(e.to_string()))?;
        let name = entry.name().map_err(|e| PackageError::Unreadable(e.to_string()))?;
        let Some(path) = clean(&name)? else { continue };
        // zip 9 gives an entry made on MS-DOS no Unix mode, yet its upper 16 bits can still say
        // "link" (many addon zips are marked that way): those bits are judged, and zip's view too.
        let link = |mode: u32| mode & 0o170000 == 0o120000;
        if link(entry.external_attributes() >> 16) || entry.unix_mode().is_some_and(link) {
            return Err(PackageError::Unsafe(format!("a link ({path})")));
        }
        if entry.is_dir() {
            continue;
        }
        let (size, packed) = (entry.size(), entry.compressed_size());
        total = total.saturating_add(size);
        if size > MAX_FILE || total > MAX_TOTAL || size > packed.max(1).saturating_mul(MAX_RATIO).max(1024 * 1024) {
            return Err(PackageError::Unsafe(format!("it unpacks to far too much ({path})")));
        }
        if !seen.insert(path.to_lowercase()) {
            return Err(PackageError::Unsafe(format!("two files with one name ({path})")));
        }
        files.push((path, i));
    }

    // Candidate folders: a folder holding a .toc of its own name, or its only .toc.
    let mut tocs_in: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for (path, i) in &files {
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
        if let Some(stem) = toc_stem(file) {
            tocs_in.entry(dir.to_string()).or_default().push((stem.to_string(), *i));
        }
    }
    let mut candidates: Vec<(String, String, usize)> = Vec::new(); // (dir, name, toc entry)
    for (dir, tocs) in &tocs_in {
        let base = dir.rsplit('/').next().unwrap_or("");
        let own = tocs.iter().find(|(stem, _)| stem.eq_ignore_ascii_case(base));
        match (own, tocs.as_slice()) {
            (Some((stem, i)), _) => candidates.push((dir.clone(), stem.clone(), *i)),
            (None, [(stem, i)]) => candidates.push((dir.clone(), stem.clone(), *i)),
            _ => {}
        }
    }
    // Shallowest first; a folder inside an addon's folder is part of that addon.
    candidates.sort_by_key(|(dir, _, _)| (dir.matches('/').count() + usize::from(!dir.is_empty()), dir.clone()));
    let mut folders: Vec<Folder> = Vec::new();
    for (dir, name, toc_index) in candidates {
        let inside = |outer: &str| outer.is_empty() || dir == outer || dir.starts_with(&format!("{outer}/"));
        if folders.iter().any(|f| inside(&f.source)) {
            continue;
        }
        if folders.iter().any(|f| f.name.eq_ignore_ascii_case(&name)) {
            return Err(PackageError::Duplicate(name));
        }
        let mut toc_bytes = Vec::new();
        read_entry(&mut archive, toc_index, &mut toc_bytes).map_err(|e| PackageError::Unreadable(e.to_string()))?;
        folders.push(Folder { name, source: dir, files: BTreeMap::new(), toc: Toc::parse(&toc_bytes) });
    }
    if folders.is_empty() {
        return Err(PackageError::NoAddon);
    }

    let mut left_out = Vec::new();
    for (path, i) in files {
        let home = folders.iter_mut().find(|f| f.source.is_empty() || path.starts_with(&format!("{}/", f.source)));
        match home {
            Some(folder) if is_addon_file(&path) => {
                let inner = if folder.source.is_empty() { path } else { path[folder.source.len() + 1..].to_string() };
                folder.files.insert(inner, i);
            }
            _ => left_out.push(path),
        }
    }
    Ok(Package { folders, left_out })
}

/// Reads one entry, never more than its declared size (a zip that lies about it is refused).
fn read_entry(archive: &mut ZipArchive<Cursor<&[u8]>>, index: usize, into: &mut Vec<u8>) -> io::Result<()> {
    let entry = archive.by_index(index).map_err(io::Error::other)?;
    let size = entry.size();
    into.clear();
    entry.take(size + 1).read_to_end(into)?;
    if into.len() as u64 != size {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a file in the zip isn't the size it claims"));
    }
    Ok(())
}

/// Unpacks the package's addon folders into `into` (one subfolder each, named for the game) and
/// returns each folder's fingerprint. Only the files `inspect` took are written.
pub fn unpack(zip: &[u8], package: &Package, into: &Path) -> io::Result<BTreeMap<String, String>> {
    let mut archive = ZipArchive::new(Cursor::new(zip)).map_err(io::Error::other)?;
    let mut prints = BTreeMap::new();
    let mut bytes = Vec::new();
    for folder in &package.folders {
        let mut listed = Vec::new();
        for (inner, &index) in &folder.files {
            read_entry(&mut archive, index, &mut bytes)?;
            fsx::write_atomic(&into.join(&folder.name).join(inner), &bytes)?;
            listed.push((inner.clone(), fsx::fingerprint(&bytes)));
        }
        prints.insert(folder.name.clone(), tree::combine(listed));
    }
    Ok(prints)
}

/// Each folder's fingerprint, read from the zip without unpacking it.
pub fn fingerprints(zip: &[u8], package: &Package) -> io::Result<BTreeMap<String, String>> {
    let mut archive = ZipArchive::new(Cursor::new(zip)).map_err(io::Error::other)?;
    let mut prints = BTreeMap::new();
    let mut bytes = Vec::new();
    for folder in &package.folders {
        let mut listed = Vec::new();
        for (inner, &index) in &folder.files {
            read_entry(&mut archive, index, &mut bytes)?;
            listed.push((inner.clone(), fsx::fingerprint(&bytes)));
        }
        prints.insert(folder.name.clone(), tree::combine(listed));
    }
    Ok(prints)
}

/// `Name` for a file called `Name.toc` (any case).
pub fn toc_stem(file: &str) -> Option<&str> {
    let split = file.len().checked_sub(4)?;
    let (stem, ext) = (file.get(..split)?, file.get(split..)?);
    (ext.eq_ignore_ascii_case(".toc") && !stem.is_empty()).then_some(stem)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// A zip of `(path, content)` pairs, deflated.
    pub(crate) fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (path, content) in files {
            writer.start_file(*path, SimpleFileOptions::default()).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    const TOC: &[u8] = b"## Interface: 11200\n## Title: Lucky Odds\n## Version: 0.7.0\n";

    fn names(package: &Package) -> Vec<(&str, &str, Vec<&str>)> {
        package.folders.iter().map(|f| (f.name.as_str(), f.source.as_str(), f.files.keys().map(String::as_str).collect())).collect()
    }

    #[test]
    fn a_tidy_package() {
        let zip = zip_of(&[("LuckyOdds/LuckyOdds.toc", TOC), ("LuckyOdds/core.lua", b"x"), ("LuckyOdds/README.md", b"r")]);
        let package = inspect(&zip).unwrap();
        assert_eq!(names(&package), [("LuckyOdds", "LuckyOdds", vec!["LuckyOdds.toc", "README.md", "core.lua"])]);
        assert_eq!(package.folders[0].toc.version.as_deref(), Some("0.7.0"));
        assert!(package.left_out.is_empty());
    }

    #[test]
    fn wrappers_and_extra_levels_are_named_after_the_toc() {
        // BigCursor's source-style zip: a wrapper with a different name and a git file.
        let zip = zip_of(&[("BigCursor-main/BigCursor.toc", TOC), ("BigCursor-main/.gitattributes", b"*"), ("BigCursor-main/c.lua", b"x")]);
        let package = inspect(&zip).unwrap();
        assert_eq!(names(&package), [("BigCursor", "BigCursor-main", vec!["BigCursor.toc", "c.lua"])]);
        assert_eq!(package.left_out, ["BigCursor-main/.gitattributes"]);
        // An extra level with a version in its name.
        let zip = zip_of(&[("LuckyOdds-v0.6.0/LuckyOdds/LuckyOdds.toc", TOC), ("LuckyOdds-v0.6.0/LuckyOdds/a.lua", b"x")]);
        assert_eq!(names(&inspect(&zip).unwrap())[0].0, "LuckyOdds");
        // A .toc at the very top.
        let zip = zip_of(&[("Clock.toc", TOC), ("clock.lua", b"x")]);
        assert_eq!(names(&inspect(&zip).unwrap()), [("Clock", "", vec!["Clock.toc", "clock.lua"])]);
    }

    #[test]
    fn several_addons_and_nested_tocs() {
        // Survivor UI: two folders and a licence at the top; a library inside keeps its .toc.
        let zip = zip_of(&[
            ("LICENSE", b"GPL"),
            ("SurvivorUI/SurvivorUI.toc", TOC),
            ("SurvivorUI/Libs/LibX/LibX.toc", TOC),
            ("SurvivorUI/Libs/LibX/LibX.lua", b"x"),
            ("SurvivorPlates/SurvivorPlates.toc", TOC),
        ]);
        let package = inspect(&zip).unwrap();
        let folders: Vec<&str> = package.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(folders, ["SurvivorPlates", "SurvivorUI"]);
        assert!(package.folders[1].files.contains_key("Libs/LibX/LibX.lua"));
        assert_eq!(package.left_out, ["LICENSE"]);
    }

    #[test]
    fn programs_never_come_out() {
        let zip = zip_of(&[("Cheat/Cheat.toc", TOC), ("Cheat/speed.exe", b"MZ"), ("Cheat/hook.dll", b"MZ"), ("Cheat/run.ps1", b"x")]);
        let package = inspect(&zip).unwrap();
        assert_eq!(names(&package), [("Cheat", "Cheat", vec!["Cheat.toc"])]);
        assert_eq!(package.left_out.len(), 3);
    }

    #[test]
    fn unsafe_zips_are_refused_whole() {
        for bad in ["../evil.lua", "A/../../evil.lua", "/abs/A.toc", "C:/Windows/A.toc", "A/b:stream"] {
            let zip = zip_of(&[("A/A.toc", TOC), (bad, b"x")]);
            assert!(matches!(inspect(&zip), Err(PackageError::Unsafe(_))), "{bad}");
        }
        let zip = zip_of(&[("A/A.toc", TOC), ("A/x.lua", b"1"), ("a/X.LUA", b"2")]);
        assert!(matches!(inspect(&zip), Err(PackageError::Unsafe(_))), "two files, one name");
        // A bomb: a small packed file that unpacks to a lot.
        let zip = zip_of(&[("A/A.toc", TOC), ("A/big.txt", &vec![b'a'; 8 * 1024 * 1024])]);
        assert!(matches!(inspect(&zip), Err(PackageError::Unsafe(_))));
        assert!(matches!(inspect(b"not a zip"), Err(PackageError::Unreadable(_))));
        assert_eq!(inspect(&zip_of(&[("readme.txt", b"x")])), Err(PackageError::NoAddon));
        let zip = zip_of(&[("One/A/A.toc", TOC), ("Two/A/A.toc", TOC)]);
        assert_eq!(inspect(&zip), Err(PackageError::Duplicate("A".into())));
    }

    #[test]
    fn a_link_refuses_the_package() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file("A/A.toc", SimpleFileOptions::default()).unwrap();
        writer.write_all(TOC).unwrap();
        writer.add_symlink("A/core.lua", "../../../outside.lua", SimpleFileOptions::default()).unwrap();
        let zip = writer.finish().unwrap().into_inner();
        let refused = |zip: &[u8]| matches!(inspect(zip), Err(PackageError::Unsafe(why)) if why.starts_with("a link"));
        assert!(refused(&zip));
        // Marked as made on MS-DOS, zip reports no Unix mode, yet the upper 16 bits say "link".
        assert!(refused(&made_on_dos(&zip)));
        // Plain files marked that way, as many addon zips are, still go in.
        assert!(inspect(&made_on_dos(&zip_of(&[("A/A.toc", TOC), ("A/core.lua", b"x")]))).is_ok());
    }

    /// The zip with each central header's host (the high byte of "version made by") set to MS-DOS.
    fn made_on_dos(zip: &[u8]) -> Vec<u8> {
        let mut dos = zip.to_vec();
        for at in (0..zip.len().saturating_sub(3)).filter(|&i| zip[i..i + 4] == *b"PK\x01\x02") {
            dos[at + 5] = 0;
        }
        dos
    }

    #[test]
    fn a_name_in_utf8_reads_as_utf8_even_without_its_flag() {
        // zip 9 reads a name that is valid UTF-8 as UTF-8 whatever bit 11 says (zip 8 read it as
        // CP437 then). Pinned, because names make the folders and their fingerprints.
        let zip = zip_of(&[("A/A.toc", TOC), ("A/é.lua", b"x")]);
        let mut unflagged = zip.clone();
        for i in 0..zip.len().saturating_sub(3) {
            match &zip[i..i + 4] {
                b"PK\x03\x04" => unflagged[i + 7] &= !0x08, // local header: flags at 6, bit 11
                b"PK\x01\x02" => unflagged[i + 9] &= !0x08, // central header: flags at 8, bit 11
                _ => {}
            }
        }
        assert_ne!(unflagged, zip, "the writer set bit 11");
        assert!(inspect(&unflagged).unwrap().folders[0].files.contains_key("é.lua"));
    }

    #[test]
    fn a_file_that_is_not_the_size_it_claims_refuses_the_package() {
        // A stored .toc whose two headers claim another size: longer than it claims or shorter.
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file("A/A.toc", options).unwrap();
        writer.write_all(TOC).unwrap();
        let honest = writer.finish().unwrap().into_inner();
        assert!(inspect(&honest).is_ok());
        let central = honest.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
        assert_eq!(&honest[..4], b"PK\x03\x04");
        for claim in [3u32, TOC.len() as u32 + 100] {
            let mut zip = honest.clone();
            zip[22..26].copy_from_slice(&claim.to_le_bytes());
            zip[central + 24..central + 28].copy_from_slice(&claim.to_le_bytes());
            assert!(inspect(&zip).is_err(), "claims {claim}");
        }
    }

    #[test]
    fn unpacks_only_what_was_taken_and_fingerprints_it() {
        let zip = zip_of(&[("W-main/W.toc", TOC), ("W-main/w.lua", b"x"), ("W-main/tool.exe", b"MZ")]);
        let package = inspect(&zip).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let prints = unpack(&zip, &package, dir.path()).unwrap();
        assert!(dir.path().join("W").join("w.lua").is_file());
        assert!(!dir.path().join("W").join("tool.exe").exists());
        assert_eq!(prints, fingerprints(&zip, &package).unwrap());
        assert_eq!(prints["W"], tree::fingerprint_dir(&dir.path().join("W")).unwrap());
    }
}
