//! Where things live: on this PC, and in the shared folder.

use crate::names::safe;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// This PC's folders.
#[derive(Clone, Debug)]
pub struct Local {
    /// `%APPDATA%\wenilla-rogue`: the game's progress.
    pub save_dir: PathBuf,
    /// `%APPDATA%\SurvivorsSatchel`: the Satchel's own files.
    pub app_dir: PathBuf,
    /// `%LOCALAPPDATA%\SurvivorsSatchel`: what belongs to this PC alone. Roaming AppData can
    /// follow a profile to another PC; this PC's memory of its last sync must not.
    pub local_dir: PathBuf,
}

impl Local {
    pub fn live_save(&self) -> PathBuf {
        self.save_dir.join("characters.txt")
    }
    pub fn game_saves(&self) -> PathBuf {
        self.save_dir.join("Saves")
    }
    pub fn settings(&self) -> PathBuf {
        self.app_dir.join("settings.toml")
    }
    pub fn state(&self) -> PathBuf {
        self.local_dir.join("state.json")
    }
    /// Where 0.1.0 and 0.1.1 kept `state.json`; it moves to `state()` on the first start.
    pub fn old_state(&self) -> PathBuf {
        self.app_dir.join("state.json")
    }
    pub fn safety_dir(&self) -> PathBuf {
        self.app_dir.join("safety")
    }
    /// With sync off: the copy kept after each session.
    pub fn copies_dir(&self) -> PathBuf {
        self.app_dir.join("copies")
    }
    /// With sync off: this PC's playtime.
    pub fn playtime(&self) -> PathBuf {
        self.app_dir.join("playtime.jsonl")
    }
    /// With sync on: sessions that ended while the NAS was unreachable.
    pub fn playtime_pending(&self) -> PathBuf {
        self.app_dir.join("playtime-pending.jsonl")
    }
    pub fn activity(&self) -> PathBuf {
        self.app_dir.join("activity.jsonl")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.app_dir.join("logs")
    }
    /// What the Satchel installed on this PC, and from where.
    pub fn addon_store(&self) -> PathBuf {
        self.local_dir.join("addons.json")
    }
    /// Earlier versions of addons, and settings copies, for going back.
    pub fn addon_backups(&self) -> PathBuf {
        self.local_dir.join("addon-backups")
    }
    /// Where a package is unpacked and checked before it goes into AddOns.
    pub fn addon_staging(&self) -> PathBuf {
        self.local_dir.join("addon-staging")
    }
    /// Downloaded addon zips, named by their checksum.
    pub fn addon_cache(&self) -> PathBuf {
        self.local_dir.join("addon-cache")
    }
    /// SurvivorsForge's answers, kept on this PC: its registry, and the details, versions and scans
    /// of addons opened here (`addons::forge::Cache`).
    pub fn forge_dir(&self) -> PathBuf {
        self.local_dir.join("forge")
    }
    /// A newer Satchel downloaded from its GitHub release, waiting for Update now.
    pub fn satchel_download(&self) -> PathBuf {
        self.local_dir.join("update")
    }
    /// What this PC and the shared addon list last agreed on.
    pub fn addon_share(&self) -> PathBuf {
        self.local_dir.join("addon-sync.json")
    }
}

/// The shared folder (on the NAS); the Satchel works in its `sync` subfolder.
#[derive(Clone, Debug)]
pub struct Shared {
    pub folder: PathBuf,
}

impl Shared {
    pub fn sync_dir(&self) -> PathBuf {
        self.folder.join("sync")
    }
    pub fn index(&self) -> PathBuf {
        self.sync_dir().join("satchel-index.json")
    }
    pub fn lock(&self) -> PathBuf {
        self.sync_dir().join("satchel-index.lock")
    }
    pub fn version(&self, file: &str) -> PathBuf {
        self.sync_dir().join(file)
    }
    pub fn in_use_dir(&self) -> PathBuf {
        self.sync_dir().join("in-use")
    }
    pub fn in_use(&self, pc: &str) -> PathBuf {
        self.in_use_dir().join(format!("{}.json", safe(pc)))
    }
    pub fn live(&self, pc: &str) -> PathBuf {
        self.sync_dir().join("live").join(format!("{}.sav", safe(pc)))
    }
    pub fn playtime_dir(&self) -> PathBuf {
        self.sync_dir().join("playtime")
    }
    pub fn playtime(&self, pc: &str) -> PathBuf {
        self.playtime_dir().join(format!("{}.jsonl", safe(pc)))
    }
    /// Addons on every PC: the list, each addon's settings versions, zips imported from a file.
    pub fn addons_dir(&self) -> PathBuf {
        self.sync_dir().join("addons")
    }
    pub fn addons_list(&self) -> PathBuf {
        self.addons_dir().join("addons.json")
    }
    pub fn addons_lock(&self) -> PathBuf {
        self.addons_dir().join("addons.lock")
    }
    pub fn settings_version(&self, addon: &str, file: &str) -> PathBuf {
        self.addons_dir().join("settings").join(safe(addon)).join(file)
    }
    /// A zip imported from a file on one PC, for the others to install.
    pub fn shared_zip(&self, sha256: &str) -> PathBuf {
        self.addons_dir().join("files").join(format!("{sha256}.zip"))
    }

    /// Whether the shared folder answers within `wait`. An unreachable network path can block
    /// for half a minute, so the probe runs on its own thread and is abandoned after `wait`.
    pub fn reachable_within(&self, wait: Duration) -> bool {
        let folder = self.folder.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(folder.is_dir());
        });
        rx.recv_timeout(wait).unwrap_or(false)
    }
}

/// Why `folder` can't be the shared folder, or `None` when it can. A shared folder inside the
/// game's folders or the Satchel's own, or holding one of them, would sync into itself: versions
/// showing up in the game's Saves list, safety copies sent as progress. A whole drive is refused
/// too. Paths are compared the way Windows compares them: either slash, any case, `.` and `..`
/// resolved.
pub fn sync_folder_problem(folder: &Path, local: &Local, game_dir: Option<&Path>) -> Option<String> {
    let shared = Parts::of(folder);
    if shared.is_drive() {
        return Some("the network folder is a whole drive; pick a folder on it".into());
    }
    let mut ours = vec![
        ("the game's progress folder", local.save_dir.as_path()),
        ("the Satchel's own folder", local.app_dir.as_path()),
        ("the Satchel's own folder", local.local_dir.as_path()),
    ];
    ours.extend(game_dir.map(|dir| ("the game's folder", dir)));
    for (name, dir) in ours {
        let dir = Parts::of(dir);
        if shared.within(&dir) {
            return Some(format!("the network folder is inside {name}"));
        }
        if dir.within(&shared) {
            return Some(format!("{name} is inside the network folder"));
        }
    }
    None
}

/// Whether `path` is a link to another place. Windows counts junctions and symbolic links as
/// links, cloud-file placeholders not.
pub fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// A path as Windows compares it: its root (`c:` or `\\server\share`) and the folder names
/// below it, lowercased.
#[derive(Debug, PartialEq)]
struct Parts {
    root: String,
    names: Vec<String>,
}

impl Parts {
    fn of(path: &Path) -> Parts {
        let mut text = path.to_string_lossy().replace('/', r"\").to_lowercase();
        if let Some(rest) = text.strip_prefix(r"\\?\unc\") {
            text = format!(r"\\{rest}");
        } else if let Some(rest) = text.strip_prefix(r"\\?\") {
            text = rest.to_string();
        }
        let (root, rest) = if let Some(unc) = text.strip_prefix(r"\\") {
            let mut parts = unc.splitn(3, '\\');
            let (server, share) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
            (format!(r"\\{server}\{share}"), parts.next().unwrap_or_default().to_string())
        } else if text.as_bytes().get(1) == Some(&b':') {
            (text[..2].to_string(), text[2..].to_string())
        } else {
            (String::new(), text.clone())
        };
        let mut names: Vec<String> = Vec::new();
        for name in rest.split('\\') {
            // Windows drops trailing dots and spaces from a name.
            match name {
                "." | "" => {}
                ".." => {
                    names.pop();
                }
                name => match name.trim_end_matches(['.', ' ']) {
                    "" => {}
                    name => names.push(name.to_string()),
                },
            }
        }
        Parts { root, names }
    }

    /// A drive's top folder (a network share's top folder is a folder like any other).
    fn is_drive(&self) -> bool {
        self.names.is_empty() && !self.root.starts_with(r"\\")
    }

    /// Whether this path is `other` or inside it.
    fn within(&self, other: &Parts) -> bool {
        self.root == other.root && self.names.starts_with(&other.names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player() -> Local {
        Local {
            save_dir: PathBuf::from(r"C:\Users\Player\AppData\Roaming\wenilla-rogue"),
            app_dir: PathBuf::from(r"C:\Users\Player\AppData\Roaming\SurvivorsSatchel"),
            local_dir: PathBuf::from(r"C:\Users\Player\AppData\Local\SurvivorsSatchel"),
        }
    }

    fn problem(folder: &str) -> Option<String> {
        sync_folder_problem(Path::new(folder), &player(), Some(Path::new(r"C:\games\AzerothSurvivors")))
    }

    #[test]
    fn a_folder_of_its_own_is_fine() {
        assert_eq!(problem(r"\\homenas\games\SatchelSync"), None);
        assert_eq!(problem(r"\\?\UNC\homenas\games\SatchelSync\"), None);
        assert_eq!(problem(r"\\homenas\games"), None, "a share's top folder is a folder");
        assert_eq!(problem(r"D:\Saves\Azeroth"), None);
        assert_eq!(problem(r"C:\games\AzerothSurvivorsSync"), None, "a similar name is another folder");
    }

    #[test]
    fn folders_that_would_sync_into_themselves_are_refused() {
        let inside_saves = Some("the network folder is inside the game's progress folder".to_string());
        assert_eq!(problem(r"C:\Users\Player\AppData\Roaming\wenilla-rogue\Saves"), inside_saves);
        assert_eq!(problem(r"c:/users/player/appdata/roaming/WENILLA-ROGUE/"), inside_saves);
        assert_eq!(problem(r"C:\Users\Player\AppData\Roaming\SurvivorsSatchel\..\wenilla-rogue."), inside_saves);
        assert_eq!(
            problem(r"C:\Users\Player\AppData\Roaming"),
            Some("the game's progress folder is inside the network folder".into())
        );
        assert_eq!(
            problem(r"C:\games\AzerothSurvivors\benilla-config\sync"),
            Some("the network folder is inside the game's folder".into())
        );
        assert_eq!(
            problem(r"C:\Users\Player\AppData\Local\SurvivorsSatchel"),
            Some("the network folder is inside the Satchel's own folder".into())
        );
    }

    #[test]
    fn a_whole_drive_is_refused() {
        let drive = Some("the network folder is a whole drive; pick a folder on it".to_string());
        assert_eq!(problem(r"E:\"), drive);
        assert_eq!(problem("E:"), drive);
        assert_eq!(problem(r"\\?\E:\"), drive);
    }

    #[test]
    fn survivorsforges_cache_belongs_to_this_pc_beside_the_addon_downloads() {
        let local = player();
        assert_eq!(local.forge_dir(), local.local_dir.join("forge"));
        assert_eq!(local.forge_dir().parent(), local.addon_cache().parent());
    }

    #[test]
    fn links_are_told_apart_from_folders() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        assert!(!is_link(&real));
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert!(is_link(&link));
        }
    }
}
