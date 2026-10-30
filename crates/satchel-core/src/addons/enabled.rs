//! The game's on/off list, `AddOns\account.txt`, read and written the way the game does: one
//! `<Addon>: enabled` or `<Addon>: disabled` per line, split at the first `:`, names compared
//! without case, the last row for a name counting. An addon without a row is on (unless its
//! `.toc` says `DefaultState: disabled`). The file appears only after an addon is first turned
//! off; rows for addons that aren't there are kept as they are. Written only with the game closed.

use super::toc;
use crate::fsx;
use std::io;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnableList {
    lines: Vec<String>,
}

fn row(line: &str) -> Option<(&str, &str)> {
    let (name, value) = line.split_once(':')?;
    let name = name.trim();
    (!name.is_empty()).then(|| (name, value.trim()))
}

impl EnableList {
    /// The list as the game left it; empty when the file isn't there.
    pub fn read(path: &Path) -> io::Result<EnableList> {
        let Some(bytes) = fsx::read_optional(path)? else { return Ok(EnableList::default()) };
        let text = toc::decode(&bytes);
        Ok(EnableList { lines: text.lines().map(str::to_string).filter(|l| !l.trim().is_empty()).collect() })
    }

    /// Whether the game loads `addon` (as far as this list goes).
    pub fn is_on(&self, addon: &str, default_disabled: bool) -> bool {
        let last = self.lines.iter().filter_map(|l| row(l)).filter(|(name, _)| name.eq_ignore_ascii_case(addon)).last();
        match last {
            Some((_, value)) => !value.eq_ignore_ascii_case("disabled"),
            None => !default_disabled,
        }
    }

    /// Turns `addon` on or off: its first row keeps its place (and spelling), any later ones go.
    pub fn set(&mut self, addon: &str, on: bool) {
        let state = if on { "enabled" } else { "disabled" };
        let mut kept = false;
        self.lines.retain_mut(|line| {
            let Some((name, _)) = row(line) else { return true };
            if !name.eq_ignore_ascii_case(addon) {
                return true;
            }
            if kept {
                return false;
            }
            *line = format!("{name}: {state}");
            kept = true;
            true
        });
        if !kept {
            self.lines.push(format!("{addon}: {state}"));
        }
    }

    /// Forgets `addon` entirely (after it is removed).
    pub fn forget(&mut self, addon: &str) {
        self.lines.retain(|line| row(line).is_none_or(|(name, _)| !name.eq_ignore_ascii_case(addon)));
    }

    /// Writes the list as the game writes it: UTF-8, LF line ends.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let mut text = self.lines.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        fsx::write_atomic(path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_like_the_game() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("account.txt");
        assert!(EnableList::read(&path).unwrap().is_on("LuckyOdds", false), "no file: everything is on");
        assert!(!EnableList::read(&path).unwrap().is_on("Quiet", true), "unless DefaultState says off");

        std::fs::write(&path, "\u{feff}LuckyOdds: disabled\r\nmeter : DISABLED\nMeter: enabled\nodd line\nGone: disabled\n").unwrap();
        let list = EnableList::read(&path).unwrap();
        assert!(!list.is_on("luckyodds", false));
        assert!(list.is_on("Meter", false), "the last row counts");
        assert!(list.is_on("Other", false));
        assert!(list.is_on("Quiet2", false));
    }

    #[test]
    fn writes_like_the_game_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AddOns").join("account.txt");
        let mut list = EnableList::read(&path).unwrap();
        list.set("LuckyOdds", false);
        list.write(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "LuckyOdds: disabled\n");

        std::fs::write(&path, "Gone: disabled\nluckyodds: disabled\nLuckyOdds: disabled\n").unwrap();
        let mut list = EnableList::read(&path).unwrap();
        list.set("LuckyOdds", true);
        list.set("Meter", false);
        list.write(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Gone: disabled\nluckyodds: enabled\nMeter: disabled\n");
        let mut list = EnableList::read(&path).unwrap();
        assert!(list.is_on("LuckyOdds", false) && !list.is_on("Meter", false));
        list.forget("METER");
        assert!(list.is_on("Meter", false));
    }
}
