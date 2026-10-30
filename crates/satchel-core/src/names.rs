//! File names in the game's own style. The game writes UTC times into its export names.

use chrono::{DateTime, NaiveDateTime, Utc};

const STAMP: &str = "%Y-%m-%d %H-%M-%S";

pub fn stamp(at: DateTime<Utc>) -> String {
    at.format(STAMP).to_string()
}

/// "AzerothSurvivors 2026-10-29 11-47-26 (PC 1).sav": one numbered version on the NAS. Two
/// from one PC within the same second get " v<n>" before ".sav".
pub fn version_file(at: DateTime<Utc>, pc: &str) -> String {
    format!("AzerothSurvivors {} ({}).sav", stamp(at), safe(pc))
}

/// A version file's name read back: when it was saved, by which PC, and its same-second
/// number (0 when it has none). `None` for any other name, the game's own exports included.
pub fn parse_version_file(name: &str) -> Option<(DateTime<Utc>, String, u64)> {
    let rest = name.strip_prefix("AzerothSurvivors ")?.strip_suffix(".sav")?;
    let saved = NaiveDateTime::parse_from_str(rest.get(..19)?, STAMP).ok()?.and_utc();
    let (pc, extra) = rest.get(19..)?.strip_prefix(" (")?.rsplit_once(')')?;
    let same_second = if extra.is_empty() { 0 } else { extra.strip_prefix(" v")?.parse().ok()? };
    (!pc.is_empty()).then(|| (saved, pc.to_string(), same_second))
}

/// "Before sync 2026-10-29 11-47-26.sav": the safety copy of a save about to be replaced.
pub fn safety_file(at: DateTime<Utc>) -> String {
    format!("Before sync {}.sav", stamp(at))
}

/// A PC name made safe to use inside a file name.
pub fn safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '-' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    if trimmed.is_empty() { "PC".into() } else { trimmed.into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn names_follow_the_game_style() {
        let at = Utc.with_ymd_and_hms(2026, 10, 29, 11, 47, 26).unwrap();
        assert_eq!(version_file(at, "PC 1"), "AzerothSurvivors 2026-10-29 11-47-26 (PC 1).sav");
        assert_eq!(safety_file(at), "Before sync 2026-10-29 11-47-26.sav");
    }

    #[test]
    fn version_file_names_read_back() {
        let at = Utc.with_ymd_and_hms(2026, 10, 29, 11, 47, 26).unwrap();
        assert_eq!(parse_version_file(&version_file(at, "PC 1")), Some((at, "PC 1".into(), 0)));
        assert_eq!(
            parse_version_file("AzerothSurvivors 2026-10-29 11-47-26 (Sam (laptop)) v12.sav"),
            Some((at, "Sam (laptop)".into(), 12))
        );
        // The game's own exports and backups, and anything else, are not versions.
        assert_eq!(parse_version_file("AzerothSurvivors 2026-10-22 06-07-02.sav"), None);
        assert_eq!(parse_version_file("Backup 2026-10-24 18-12-54.sav"), None);
        assert_eq!(parse_version_file("AzerothSurvivors 2026-13-45 99-99-99 (PC 1).sav"), None);
        assert_eq!(parse_version_file("AzerothSurvivors 2026-10-29 11-47-26 ().sav"), None);
        assert_eq!(parse_version_file("satchel-index.json"), None);
    }

    #[test]
    fn unsafe_pc_names_are_cleaned() {
        assert_eq!(safe("Sam's PC: \"main\""), "Sam's PC- -main-");
        assert_eq!(safe("  ..  "), "PC");
        assert_eq!(safe("Laptop."), "Laptop");
    }
}
