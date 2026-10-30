//! Playtime per PC: one line per session, appended to that PC's own file.

use chrono::{DateTime, Datelike, Days, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub pc: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub minutes: u32,
    /// The game updated itself during the session and it still counted as one.
    #[serde(default)]
    pub updated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
}

impl Session {
    pub fn new(pc: &str, start: DateTime<Utc>, end: DateTime<Utc>, updated: bool, game: Option<String>) -> Session {
        let seconds = (end - start).num_seconds().max(0);
        Session { pc: pc.into(), start, end, minutes: ((seconds + 30) / 60) as u32, updated, game }
    }
}

pub fn append(path: &Path, sessions: &[Session]) -> io::Result<()> {
    if sessions.is_empty() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut text = String::new();
    for session in sessions {
        text.push_str(&serde_json::to_string(session).map_err(io::Error::other)?);
        text.push('\n');
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// The sessions in one file; a damaged line is skipped, never fatal.
pub fn read_file(path: &Path) -> io::Result<Vec<Session>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// The sessions of every PC (every `.jsonl` in the folder).
pub fn read_dir(dir: &Path) -> io::Result<Vec<Session>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut sessions = Vec::new();
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            sessions.extend(read_file(&path)?);
        }
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.start));
    Ok(sessions)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub today: u32,
    pub week: u32,
    pub all: u32,
    /// Minutes this week per PC, most first.
    pub week_per_pc: Vec<(String, u32)>,
}

/// Today, this week (from Monday) and all time, in the time zone of `now`.
pub fn totals<Tz: TimeZone>(sessions: &[Session], now: &DateTime<Tz>) -> Totals {
    let zone = now.timezone();
    let today = now.date_naive();
    let monday = today - Days::new(u64::from(now.weekday().num_days_from_monday()));
    let mut totals = Totals::default();
    let mut per_pc: BTreeMap<String, u32> = BTreeMap::new();
    for session in sessions {
        let day = session.start.with_timezone(&zone).date_naive();
        totals.all += session.minutes;
        if day == today {
            totals.today += session.minutes;
        }
        if day >= monday {
            totals.week += session.minutes;
            *per_pc.entry(session.pc.clone()).or_default() += session.minutes;
        }
    }
    totals.week_per_pc = per_pc.into_iter().collect();
    totals.week_per_pc.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    totals
}

/// "0 m", "35 m", "1 h 35 m".
pub fn format_minutes(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} m"),
        (h, m) => format!("{h} h {m:02} m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
    }

    #[test]
    fn sessions_round_trip_and_bad_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("playtime").join("PC 1.jsonl");
        let one = Session::new("PC 1", at(29, 10), at(29, 11), false, Some("0.127.0".into()));
        append(&path, std::slice::from_ref(&one)).unwrap();
        fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{broken\n").unwrap();
        let two = Session::new("PC 1", at(29, 12), at(29, 14), true, None);
        append(&path, std::slice::from_ref(&two)).unwrap();
        assert_eq!(read_file(&path).unwrap(), [one.clone(), two.clone()]);
        assert_eq!(read_dir(path.parent().unwrap()).unwrap(), [two, one], "newest first");
    }

    #[test]
    fn minutes_round_to_the_nearest() {
        let start = at(29, 10);
        assert_eq!(Session::new("PC", start, start + TimeDelta::seconds(89), false, None).minutes, 1);
        assert_eq!(Session::new("PC", start, start + TimeDelta::seconds(95 * 60 + 31), false, None).minutes, 96);
    }

    #[test]
    fn totals_use_the_local_day_and_monday_weeks() {
        // Thursday 29 Oct 2026, 15:00 in Bucharest (UTC+2).
        let now = FixedOffset::east_opt(2 * 3600).unwrap().with_ymd_and_hms(2026, 10, 29, 15, 0, 0).unwrap();
        let sessions = [
            Session::new("PC 1", at(29, 8), at(29, 9), false, None),   // today, 60
            Session::new("PC 2", at(28, 22), at(28, 23), false, None), // 29 Oct 00:00 local: today, 60
            Session::new("PC 2", at(26, 10), at(26, 12), false, None), // Monday, 120
            Session::new("PC 1", at(25, 10), at(25, 11), false, None), // Sunday before: last week, 60
        ];
        let totals = totals(&sessions, &now);
        assert_eq!((totals.today, totals.week, totals.all), (120, 240, 300));
        assert_eq!(totals.week_per_pc, [("PC 2".to_string(), 180), ("PC 1".to_string(), 60)]);
    }

    #[test]
    fn formats_hours_and_minutes() {
        assert_eq!(format_minutes(0), "0 m");
        assert_eq!(format_minutes(35), "35 m");
        assert_eq!(format_minutes(95), "1 h 35 m");
        assert_eq!(format_minutes(2470), "41 h 10 m");
    }

    use chrono::TimeDelta;
}
