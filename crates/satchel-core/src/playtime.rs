//! Playtime per PC: one line per session, appended to that PC's own file.

use chrono::{DateTime, Datelike, Days, NaiveDate, TimeZone, Utc};
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
    /// How many sessions this week (since 0.9.0: Home's "This week: 4 h 12 min · 6 sessions").
    pub week_sessions: u32,
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
            totals.week_sessions += 1;
            *per_pc.entry(session.pc.clone()).or_default() += session.minutes;
        }
    }
    totals.week_per_pc = per_pc.into_iter().collect();
    totals.week_per_pc.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    totals
}

/// One of the last seven local days, for Home's playtime chart: the day, its minutes and how
/// many sessions started on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Day {
    pub date: NaiveDate,
    pub minutes: u32,
    pub sessions: u32,
}

/// The last seven local days, oldest first, today last, in the time zone of `now`: a day without
/// a session has zero minutes. A session counts on the day it started.
pub fn per_day<Tz: TimeZone>(sessions: &[Session], now: &DateTime<Tz>) -> Vec<Day> {
    let zone = now.timezone();
    let today = now.date_naive();
    let mut days: Vec<Day> = (0..7).rev().map(|back| Day { date: today - Days::new(back), ..Day::default() }).collect();
    for session in sessions {
        let day = session.start.with_timezone(&zone).date_naive();
        if let Some(slot) = days.iter_mut().find(|d| d.date == day) {
            slot.minutes += session.minutes;
            slot.sessions += 1;
        }
    }
    days
}

/// "0 m", "35 m", "1 h 35 m".
pub fn format_minutes(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} m"),
        (h, m) => format!("{h} h {m:02} m"),
    }
}

/// "0 min", "34 min", "4 h 12 min", "61 h 20 min": the minutes as 0.9's screens say them.
pub fn format_long(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
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
        assert_eq!(totals.week_sessions, 3);
    }

    /// The last seven local days for Home's chart: today last, an empty day at zero, a session
    /// on the day it started in the local zone, older ones left out.
    #[test]
    fn the_last_seven_days_end_today_and_count_local_starts() {
        // Thursday 29 Oct 2026, 15:00 in Bucharest (UTC+2).
        let now = FixedOffset::east_opt(2 * 3600).unwrap().with_ymd_and_hms(2026, 10, 29, 15, 0, 0).unwrap();
        let sessions = [
            Session::new("PC 1", at(29, 8), at(29, 9), false, None),   // today, 60
            Session::new("PC 2", at(28, 22), at(28, 23), false, None), // 29 Oct 00:00 local: today, 60
            Session::new("PC 2", at(26, 10), at(26, 12), false, None), // Monday, 120
            Session::new("PC 1", at(23, 10), at(23, 11), false, None), // Friday 23 Oct: the first of the seven, 60
            Session::new("PC 1", at(22, 10), at(22, 11), false, None), // 22 Oct: older, left out
        ];
        let days = per_day(&sessions, &now);
        let day = |d: u32| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
        assert_eq!(days.iter().map(|d| d.date).collect::<Vec<_>>(), [day(23), day(24), day(25), day(26), day(27), day(28), day(29)]);
        assert_eq!(days.iter().map(|d| (d.minutes, d.sessions)).collect::<Vec<_>>(), [(60, 1), (0, 0), (0, 0), (120, 1), (0, 0), (0, 0), (120, 2)]);
        assert_eq!(per_day(&[], &now).iter().map(|d| d.minutes).sum::<u32>(), 0);
        assert_eq!(per_day(&[], &now).len(), 7);
    }

    #[test]
    fn formats_hours_and_minutes() {
        assert_eq!(format_minutes(0), "0 m");
        assert_eq!(format_minutes(35), "35 m");
        assert_eq!(format_minutes(95), "1 h 35 m");
        assert_eq!(format_minutes(2470), "41 h 10 m");
        assert_eq!(format_long(0), "0 min");
        assert_eq!(format_long(34), "34 min");
        assert_eq!(format_long(252), "4 h 12 min");
        assert_eq!(format_long(120), "2 h");
        assert_eq!(format_long(3680), "61 h 20 min");
    }

    use chrono::TimeDelta;
}
