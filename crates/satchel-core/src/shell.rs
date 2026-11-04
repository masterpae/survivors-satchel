//! The window's shell as words and numbers, pure and tested here (since 0.9.0): the size the
//! window opens at and its smallest size, the frame's colours as Windows takes them, the rail's
//! status lines and its last line, Home's Needs-you list, Browse's filter rule, and the relative
//! times the cards show. The app (`src/`) only turns these into what is on screen; nothing here
//! reads a file.

use crate::addons::forge::Verdict;
use crate::addons::overview::{Line, Standing};
use chrono::{DateTime, Datelike, TimeZone, Utc};

// The window ------------------------------------------------------------------------------------

/// The window as it opens, in logical pixels, 16:9.
pub const DEFAULT_SIZE: (u32, u32) = (1280, 720);
/// The window at its smallest, 16:9: the rail needs the width.
pub const MIN_SIZE: (u32, u32) = (1120, 630);

/// Windows' own frame around the client area, in logical pixels: the title bar and the borders.
const FRAME: (u32, u32) = (16, 39);

/// How the window opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    /// At this size, in logical pixels, centred on the work area.
    Size(u32, u32),
    /// Even the smallest window doesn't fit the work area: maximised.
    Maximised,
}

/// The size the window opens at, from the work area (the screen minus the taskbar, in physical
/// pixels) and the display's scale: the default when that fits with Windows' frame, else the
/// minimum (a 1,366 × 768 screen with the taskbar), else maximised.
pub fn open_size(work_width: i32, work_height: i32, scale: f32) -> Open {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let fits = |size: (u32, u32)| {
        let needed = |logical: u32, frame: u32| ((logical + frame) as f32 * scale).ceil() as i64;
        needed(size.0, FRAME.0) <= i64::from(work_width) && needed(size.1, FRAME.1) <= i64::from(work_height)
    };
    if fits(DEFAULT_SIZE) {
        Open::Size(DEFAULT_SIZE.0, DEFAULT_SIZE.1)
    } else if fits(MIN_SIZE) {
        Open::Size(MIN_SIZE.0, MIN_SIZE.1)
    } else {
        Open::Maximised
    }
}

/// The window's smallest size on this screen, in logical pixels: 1,120 × 630, or less where the
/// work area (physical pixels, at the display's scale) can't hold that with Windows' frame around
/// it, so a maximised window never reaches past the screen. A 1,920 × 1,080 screen at 175 % has a
/// work area of about 1,097 × 590 logical pixels.
pub fn min_size(work_width: i32, work_height: i32, scale: f32) -> (u32, u32) {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let logical = |physical: i32, frame: u32| ((physical.max(0) as f32 / scale).floor() as u32).saturating_sub(frame);
    (MIN_SIZE.0.min(logical(work_width, FRAME.0)), MIN_SIZE.1.min(logical(work_height, FRAME.1)))
}

/// A CSS colour (`#1f232b`) as Windows' COLORREF, `0x00BBGGRR`: the bytes in the reverse order
/// (`0x002b231f`). `None` for anything but six hex digits after `#`.
pub fn colorref(hex: &str) -> Option<u32> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let part = |at: usize| u32::from_str_radix(&digits[at..at + 2], 16).ok();
    let (red, green, blue) = (part(0)?, part(2)?, part(4)?);
    Some(blue << 16 | green << 8 | red)
}

// Times -------------------------------------------------------------------------------------------

/// A moment as the cards say it, short: "just now", "2 min ago" (within the hour), "today 13:20",
/// "yesterday 21:40", "Sat 16:30" (within the week), "31 Oct", "31 Oct 2025" (another year); a
/// moment after `now` (a clock put back) reads as "just now".
pub fn when_short<Tz: TimeZone>(at: DateTime<Utc>, now: &DateTime<Tz>) -> String {
    let zone = now.timezone();
    let local = at.with_timezone(&zone);
    let minutes = (now.clone().with_timezone(&Utc) - at).num_minutes();
    if minutes < 1 {
        return "just now".into();
    }
    if minutes < 60 {
        return format!("{minutes} min ago");
    }
    let clock = local.naive_local().format("%H:%M");
    match day_word(at, now).as_str() {
        "today" => format!("today {clock}"),
        "yesterday" => format!("yesterday {clock}"),
        day if day.len() == 3 => format!("{day} {clock}"),
        day => day.to_string(),
    }
}

/// The day of a moment as the cards say it: "today", "yesterday", its weekday ("Sun") within the
/// last six days, else "31 Oct", or "31 Oct 2025" in another year.
pub fn day_word<Tz: TimeZone>(at: DateTime<Utc>, now: &DateTime<Tz>) -> String {
    let zone = now.timezone();
    let (day, today) = (at.with_timezone(&zone).date_naive(), now.date_naive());
    let back = (today - day).num_days();
    match back {
        ..=0 => "today".into(),
        1 => "yesterday".into(),
        2..=6 => day.format("%a").to_string(),
        _ if day.year() == today.year() => day.format("%-d %b").to_string(),
        _ => day.format("%-d %b %Y").to_string(),
    }
}

/// A moment as the playtime table says it: "Today", "Yesterday", "Tue 3 Nov", "31 Oct 2025".
pub fn day_long<Tz: TimeZone>(at: DateTime<Utc>, now: &DateTime<Tz>) -> String {
    let zone = now.timezone();
    let (day, today) = (at.with_timezone(&zone).date_naive(), now.date_naive());
    match (today - day).num_days() {
        ..=0 => "Today".into(),
        1 => "Yesterday".into(),
        _ if day.year() == today.year() => day.format("%a %-d %b").to_string(),
        _ => day.format("%-d %b %Y").to_string(),
    }
}

// The rail --------------------------------------------------------------------------------------

/// The dot in front of a rail line, coloured by the tray's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    Green,
    Blue,
    Gold,
    Amber,
    /// Nothing to colour yet: muted.
    Muted,
    /// A notice's plain news: the text colour.
    Text,
}

/// One of the rail's status lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailLine {
    pub text: String,
    pub dot: Dot,
    /// The text in amber (the banner's words, a problem) rather than the text colour.
    pub amber: bool,
    /// It may take two lines (a notice); else it is elided on one.
    pub wrap: bool,
}

/// A one-off message for the player, shown on the rail's progress line for its minute: plain news
/// (the text colour) or a problem (amber).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub problem: bool,
}

/// The rail's Build line: "Build 0.141.0" with the What's new link beside it (`true`), or, with
/// no note cached yet, the muted line that says the notes come once read (`false`).
pub fn build_line(version: &str) -> (String, bool) {
    let version = version.trim();
    if version.is_empty() { ("The game's notes come here once read".into(), false) } else { (format!("Build {version}"), true) }
}

/// The rail's progress line. A notice (`notice`, while its minute lasts) comes first, whatever the
/// tray's state: a problem in amber with an amber dot, plain news in the text colour with a dot of
/// that colour, on two lines when it needs them. Else the dot follows the tray: green synced, blue
/// busy, gold playing, amber needs-you or offline; amber lines carry the banner's words (`banner`)
/// instead. `last`: the last successful send or bring-in (sync on), or the last local safety copy
/// (sync off), as `when_short` says it; `pcs`: the PCs in the shared index.
pub fn progress_line(dot: Dot, banner: &str, sync_on: bool, last: Option<&str>, pcs: usize, notice: Option<&Notice>) -> RailLine {
    if let Some(notice) = notice.filter(|n| !n.text.trim().is_empty()) {
        let dot = if notice.problem { Dot::Amber } else { Dot::Text };
        return RailLine { text: notice.text.trim().to_string(), dot, amber: notice.problem, wrap: true };
    }
    if dot == Dot::Amber {
        return RailLine { text: banner.trim().to_string(), dot, amber: true, wrap: false };
    }
    let text = match (sync_on, last) {
        (true, Some(last)) => format!("Progress synced {last} · {}", if pcs == 1 { "1 PC".to_string() } else { format!("{pcs} PCs") }),
        (false, Some(last)) => format!("Progress saved {last} · this PC only"),
        (_, None) => "No progress saved yet".into(),
    };
    RailLine { text, dot, amber: false, wrap: false }
}

/// What the rail's addons line is made from: the Installed tab's state in numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AddonsFacts {
    /// Addons in AddOns.
    pub installed: usize,
    /// Of those, the ones the game loads: one turned off isn't counted.
    pub on: usize,
    /// Updates that go in at Play.
    pub at_play: usize,
    /// Updates, or addons another PC brings, that wait for the player's OK.
    pub waiting_ok: usize,
    /// A download runs.
    pub downloading: bool,
    /// New releases have been checked at least once this run.
    pub checked: bool,
}

/// The rail's addons numbers from the Installed list's lines: the addons in AddOns, the ones the
/// game loads, and the updates that go in by themselves at Play (not one turned off, nor one
/// SurvivorsForge holds back); `waiting_ok`, `downloading` and `checked` come from the worker.
pub fn addons_facts(lines: &[Line], waiting_ok: usize, downloading: bool, checked: bool) -> AddonsFacts {
    let at_play = lines
        .iter()
        .filter(|l| matches!(l.standing, Standing::Update { by_itself: true, .. }) && !l.off && !matches!(l.verdict, Verdict::No(_)))
        .count();
    AddonsFacts { installed: lines.len(), on: lines.iter().filter(|l| l.on).count(), at_play, waiting_ok, downloading, checked }
}

/// The rail's addons line: "5 addons on · all up to date", "5 addons on · 2 updates at Play",
/// "5 addons on · 1 waits for your OK", "Addons not checked yet"; the dot green, blue while a
/// download runs, amber when one waits for an OK. Only the addons the game loads count as on.
pub fn addons_line(f: AddonsFacts) -> RailLine {
    if f.installed == 0 {
        return RailLine { text: "No addons yet".into(), dot: Dot::Muted, amber: false, wrap: false };
    }
    if !f.checked && f.waiting_ok == 0 && f.at_play == 0 {
        return RailLine { text: "Addons not checked yet".into(), dot: Dot::Muted, amber: false, wrap: false };
    }
    let on = if f.on == 1 { "1 addon on".to_string() } else { format!("{} addons on", f.on) };
    let (tail, dot) = if f.waiting_ok > 0 {
        (if f.waiting_ok == 1 { "1 waits for your OK".to_string() } else { format!("{} wait for your OK", f.waiting_ok) }, Dot::Amber)
    } else if f.at_play > 0 {
        (if f.at_play == 1 { "1 update at Play".to_string() } else { format!("{} updates at Play", f.at_play) }, if f.downloading { Dot::Blue } else { Dot::Green })
    } else {
        ("all up to date".to_string(), if f.downloading { Dot::Blue } else { Dot::Green })
    };
    RailLine { text: format!("{on} · {tail}"), dot, amber: false, wrap: false }
}

/// The rail's last line, under Play: the newest session ("Last played today, 34 min · Laptop"),
/// "Not played from the Satchel yet", or, while the game runs, the session so far and what
/// happens when it closes. `last`: the newest session's day (`day_word`), its minutes and its PC.
pub fn last_line(playing: Option<u32>, sync_on: bool, last: Option<(&str, u32, &str)>) -> String {
    if let Some(minutes) = playing {
        let then = if sync_on { "your progress is sent when the game closes" } else { "your progress is backed up when the game closes" };
        return format!("Session {} · {then}", crate::playtime::format_long(minutes));
    }
    match last {
        Some((day, minutes, pc)) => format!("Last played {day}, {} · {pc}", crate::playtime::format_long(minutes)),
        None => "Not played from the Satchel yet".into(),
    }
}

// Needs you --------------------------------------------------------------------------------------

/// What a button of a Needs-you question does; the app routes each to today's callback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// The progress choice: it opens the question over the window, where both sides show with
    /// their dates and sizes and the choice is made.
    ChooseProgress,
    /// The game's settings choice, and this PC's own from before it joined.
    GameKeepHere,
    GameUseOther,
    GameUseOwn,
    /// An addon's settings choice (its id).
    AddonKeepHere(String),
    AddonUseOther(String),
    /// An update, or an addon another PC brings, that waits for an OK (its id): the OK, and what
    /// SurvivorsForge found.
    InstallAnyway(String),
    ShowScan(String),
    OpenSettings,
}

/// One question on Home's Needs-you card: its words and its buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub text: String,
    pub buttons: Vec<(String, Action)>,
}

/// An addon whose update, or whose arrival from another PC, waits for the player's OK.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaitingOk {
    pub id: String,
    pub name: String,
    /// The version that waits ("2.8.18"); empty when not known.
    pub version: String,
    /// Why, as the row says it after "Waits for your OK: ".
    pub reason: String,
    /// Not here yet: another PC brings it.
    pub incoming: bool,
}

/// Everything the app asks the player outside an overlay, as facts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NeedsYouFacts {
    /// Two PCs changed the progress: the other PC's name.
    pub progress_split: Option<String>,
    /// The game's settings changed here and on this PC.
    pub game_ask_pc: Option<String>,
    /// This PC's own game settings from before it joined are kept: the line that says so.
    pub game_own: Option<String>,
    /// An addon's settings changed here and on another PC: its id, its name, the PC.
    pub addon_asks: Vec<(String, String, String)>,
    pub waiting_ok: Vec<WaitingOk>,
    /// The network folder doesn't answer.
    pub folder_down: bool,
}

/// Home's Needs-you list, one block per question, in the order the player should see them: the
/// progress first, then the game's settings, the addons' settings, the OKs, the folder. The
/// progress question's one button opens it over the window, where the choice is made.
pub fn needs_you(f: &NeedsYouFacts) -> Vec<Question> {
    let mut out = Vec::new();
    if let Some(pc) = f.progress_split.as_deref().filter(|pc| !pc.trim().is_empty()) {
        out.push(Question {
            text: format!("Your progress here and on {pc} differ: pick the one to continue with. The other is kept as a version."),
            buttons: vec![("Choose…".into(), Action::ChooseProgress)],
        });
    }
    if let Some(pc) = f.game_ask_pc.as_deref().filter(|pc| !pc.trim().is_empty()) {
        out.push(Question {
            text: format!("The game's settings changed here and on {pc} since they last matched: pick one. This PC's stay until you do."),
            buttons: vec![("Keep this PC's".into(), Action::GameKeepHere), (format!("Use {pc}'s"), Action::GameUseOther)],
        });
    }
    if let Some(own) = f.game_own.as_deref().filter(|own| !own.trim().is_empty()) {
        out.push(Question { text: own.trim().to_string(), buttons: vec![("Use them instead".into(), Action::GameUseOwn)] });
    }
    for (id, name, pc) in &f.addon_asks {
        out.push(Question {
            text: format!("{name}'s settings changed here and on {pc} since they last matched: pick one."),
            buttons: vec![("Keep this PC's".into(), Action::AddonKeepHere(id.clone())), (format!("Use {pc}'s"), Action::AddonUseOther(id.clone()))],
        });
    }
    for w in &f.waiting_ok {
        let what = if w.version.trim().is_empty() { w.name.clone() } else { format!("{} {}", w.name, w.version.trim()) };
        let from = if w.incoming { ", from another PC," } else { "" };
        let reason = w.reason.trim().trim_end_matches('.');
        let text = if reason.is_empty() { format!("{what}{from} waits for your OK") } else { format!("{what}{from} waits for your OK: {reason}") };
        out.push(Question {
            text,
            buttons: vec![("Install anyway".into(), Action::InstallAnyway(w.id.clone())), ("Show what it found".into(), Action::ShowScan(w.id.clone()))],
        });
    }
    if f.folder_down {
        out.push(Question {
            text: "The network folder doesn't answer: your progress is safe on this PC and goes up when the folder is back.".into(),
            buttons: vec![("Open Settings".into(), Action::OpenSettings)],
        });
    }
    out
}

// Browse's filters ---------------------------------------------------------------------------------

/// The filter rule: within a group any ticked box matches, across groups all must. `ticked` is
/// the boxes on per group, `values` what the row has for each group, in the same order; a group
/// with nothing ticked lets everything through.
pub fn filter_match(ticked: &[Vec<String>], values: &[Vec<String>]) -> bool {
    ticked.iter().enumerate().all(|(group, on)| {
        on.is_empty() || values.get(group).is_some_and(|have| have.iter().any(|v| on.iter().any(|t| t.eq_ignore_ascii_case(v))))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, hour, minute, 0).unwrap()
    }

    #[test]
    fn the_window_opens_at_the_default_the_minimum_or_maximised() {
        // A Full HD screen with the taskbar: the default.
        assert_eq!(open_size(1920, 1032, 1.0), Open::Size(1280, 720));
        // A 1,366 × 768 screen with the taskbar: the frame doesn't fit over 720 px, the minimum does.
        assert_eq!(open_size(1366, 720, 1.0), Open::Size(1120, 630));
        // Too small even for the minimum: maximised.
        assert_eq!(open_size(1024, 700, 1.0), Open::Maximised);
        // At 150 %, a Full HD screen holds the minimum only (1,120 + 16 = 1,136 × 1.5 = 1,704 < 1,920;
        // 1,280 + 16 = 1,296 × 1.5 = 1,944 > 1,920).
        assert_eq!(open_size(1920, 1032, 1.5), Open::Size(1120, 630));
        // Exactly fitting counts as fitting.
        assert_eq!(open_size(1296, 759, 1.0), Open::Size(1280, 720));
        assert_eq!(open_size(1295, 759, 1.0), Open::Size(1120, 630));
        // A scale that makes no sense reads as 100 %.
        assert_eq!(open_size(1920, 1032, 0.0), Open::Size(1280, 720));
        assert_eq!(open_size(1920, 1032, f32::NAN), Open::Size(1280, 720));
    }

    /// The smallest window fits the screen with its frame: 1,120 × 630 where there is room, less
    /// on a small high-DPI screen, so a maximised window never reaches past it.
    #[test]
    fn the_smallest_window_fits_the_work_area() {
        assert_eq!(min_size(1920, 1032, 1.0), (1120, 630));
        assert_eq!(min_size(1366, 728, 1.0), (1120, 630));
        // 1,920 × 1,080 at 175 %, a 48 px taskbar: 1,097 × 589 logical, the frame inside that.
        assert_eq!(min_size(1920, 1032, 1.75), (1081, 550));
        // 1,280 × 800 at 150 %: 853 × 506 logical.
        assert_eq!(min_size(1280, 760, 1.5), (837, 467));
        assert_eq!(min_size(1920, 1032, f32::NAN), (1120, 630));
        assert_eq!(min_size(0, -5, 1.0), (0, 0));
        // Never more than the window the screen opens maximised at.
        for (w, h, scale) in [(1920, 1032, 1.75), (1280, 760, 1.5), (1024, 700, 1.0)] {
            let (mw, mh) = min_size(w, h, scale);
            assert!(((mw + 16) as f32 * scale) <= w as f32 && ((mh + 39) as f32 * scale) <= h as f32, "{w}×{h} at {scale}");
        }
    }

    #[test]
    fn a_colour_becomes_a_colorref_with_its_bytes_reversed() {
        assert_eq!(colorref("#1f232b"), Some(0x002b231f));
        assert_eq!(colorref("#e9ecf1"), Some(0x00f1ece9));
        assert_eq!(colorref("#e8ebf0"), Some(0x00f0ebe8));
        assert_eq!(colorref("#1b1f27"), Some(0x00271f1b));
        assert_eq!(colorref("#2b313c"), Some(0x003c312b));
        assert_eq!(colorref("#d9dde4"), Some(0x00e4ddd9));
        assert_eq!(colorref("#FF0000"), Some(0x000000ff), "red is the low byte");
        assert_eq!(colorref("#0000ff"), Some(0x00ff0000), "blue is the high byte");
        assert_eq!(colorref("1f232b"), None);
        assert_eq!(colorref("#1f23"), None);
        assert_eq!(colorref("#1f232bff"), None);
        assert_eq!(colorref("#gg232b"), None);
    }

    #[test]
    fn times_read_as_the_cards_say_them() {
        // Wednesday 4 Nov 2026, 14:00 in Bucharest (UTC+2).
        let now = FixedOffset::east_opt(2 * 3600).unwrap().with_ymd_and_hms(2026, 11, 4, 14, 0, 0).unwrap();
        assert_eq!(when_short(at(4, 12, 0), &now), "just now");
        assert_eq!(when_short(at(4, 12, 30), &now), "just now", "a clock put back");
        assert_eq!(when_short(at(4, 11, 58), &now), "2 min ago");
        assert_eq!(when_short(at(4, 11, 1), &now), "59 min ago");
        assert_eq!(when_short(at(4, 11, 0), &now), "today 13:00");
        assert_eq!(when_short(at(3, 19, 40), &now), "yesterday 21:40");
        assert_eq!(when_short(at(1, 17, 5), &now), "Sun 19:05");
        assert_eq!(when_short(Utc.with_ymd_and_hms(2026, 10, 31, 14, 30, 0).unwrap(), &now), "Sat 16:30");
        assert_eq!(when_short(Utc.with_ymd_and_hms(2026, 10, 29, 10, 0, 0).unwrap(), &now), "Thu 12:00", "six days back: the weekday");
        assert_eq!(when_short(Utc.with_ymd_and_hms(2026, 10, 28, 10, 0, 0).unwrap(), &now), "28 Oct", "a week back: the date");
        assert_eq!(when_short(Utc.with_ymd_and_hms(2025, 12, 31, 10, 0, 0).unwrap(), &now), "31 Dec 2025");
        assert_eq!(day_word(at(4, 1, 0), &now), "today");
        assert_eq!(day_word(at(3, 22, 30), &now), "today", "00:30 local on the 4th");
        assert_eq!(day_word(at(3, 10, 0), &now), "yesterday");
        assert_eq!(day_word(at(1, 10, 0), &now), "Sun");
        assert_eq!(day_word(Utc.with_ymd_and_hms(2026, 10, 20, 10, 0, 0).unwrap(), &now), "20 Oct");
        assert_eq!(day_long(at(4, 1, 0), &now), "Today");
        assert_eq!(day_long(at(3, 10, 0), &now), "Yesterday");
        assert_eq!(day_long(at(1, 10, 0), &now), "Sun 1 Nov");
        assert_eq!(day_long(Utc.with_ymd_and_hms(2025, 12, 31, 10, 0, 0).unwrap(), &now), "31 Dec 2025");
    }

    #[test]
    fn the_rails_lines_say_the_state_in_a_few_words() {
        assert_eq!(build_line("0.141.0"), ("Build 0.141.0".to_string(), true));
        assert_eq!(build_line(" "), ("The game's notes come here once read".to_string(), false));

        let line = progress_line(Dot::Green, "", true, Some("2 min ago"), 3, None);
        assert_eq!(line, RailLine { text: "Progress synced 2 min ago · 3 PCs".into(), dot: Dot::Green, amber: false, wrap: false });
        assert_eq!(progress_line(Dot::Blue, "", true, Some("just now"), 1, None).text, "Progress synced just now · 1 PC");
        assert_eq!(progress_line(Dot::Green, "", false, Some("yesterday 21:40"), 0, None).text, "Progress saved yesterday 21:40 · this PC only");
        assert_eq!(progress_line(Dot::Green, "", true, None, 0, None).text, "No progress saved yet");
        assert_eq!(progress_line(Dot::Gold, "", false, None, 0, None), RailLine { text: "No progress saved yet".into(), dot: Dot::Gold, amber: false, wrap: false });
        let offline = progress_line(Dot::Amber, "Network folder unreachable: your progress is safe on this PC.", true, Some("2 min ago"), 3, None);
        assert_eq!(offline, RailLine { text: "Network folder unreachable: your progress is safe on this PC.".into(), dot: Dot::Amber, amber: true, wrap: false });

        let facts = AddonsFacts { installed: 6, on: 5, checked: true, ..Default::default() };
        assert_eq!(addons_line(facts), RailLine { text: "5 addons on · all up to date".into(), dot: Dot::Green, amber: false, wrap: false });
        assert_eq!(addons_line(AddonsFacts { at_play: 2, ..facts }), RailLine { text: "5 addons on · 2 updates at Play".into(), dot: Dot::Green, amber: false, wrap: false });
        assert_eq!(addons_line(AddonsFacts { at_play: 1, downloading: true, ..facts }), RailLine { text: "5 addons on · 1 update at Play".into(), dot: Dot::Blue, amber: false, wrap: false });
        assert_eq!(addons_line(AddonsFacts { waiting_ok: 1, at_play: 2, ..facts }), RailLine { text: "5 addons on · 1 waits for your OK".into(), dot: Dot::Amber, amber: false, wrap: false });
        assert_eq!(addons_line(AddonsFacts { waiting_ok: 2, ..facts }).text, "5 addons on · 2 wait for your OK");
        assert_eq!(addons_line(AddonsFacts { installed: 1, on: 1, checked: true, ..Default::default() }).text, "1 addon on · all up to date");
        assert_eq!(addons_line(AddonsFacts { installed: 2, on: 0, checked: true, ..Default::default() }).text, "0 addons on · all up to date");
        assert_eq!(addons_line(AddonsFacts { installed: 5, on: 5, ..Default::default() }), RailLine { text: "Addons not checked yet".into(), dot: Dot::Muted, amber: false, wrap: false });
        // Not checked this run, but something is known from the last: it shows.
        assert_eq!(addons_line(AddonsFacts { installed: 5, on: 5, waiting_ok: 1, ..Default::default() }).text, "5 addons on · 1 waits for your OK");
        assert_eq!(addons_line(AddonsFacts::default()), RailLine { text: "No addons yet".into(), dot: Dot::Muted, amber: false, wrap: false });

        assert_eq!(last_line(None, true, Some(("today", 34, "Laptop"))), "Last played today, 34 min · Laptop");
        assert_eq!(last_line(None, true, Some(("Sun", 252, "Study PC"))), "Last played Sun, 4 h 12 min · Study PC");
        assert_eq!(last_line(None, false, None), "Not played from the Satchel yet");
        assert_eq!(last_line(Some(12), true, Some(("today", 34, "Laptop"))), "Session 12 min · your progress is sent when the game closes");
        assert_eq!(last_line(Some(0), false, None), "Session 0 min · your progress is backed up when the game closes");
    }

    /// A notice shows on the progress line for its minute whatever the tray's state, in its own
    /// colour (amber for a problem, the text colour for news, the dot the same), on two lines when
    /// it needs them; without one the line says the state again.
    #[test]
    fn a_notice_shows_on_the_progress_line_whatever_the_state() {
        let news = Notice { text: " Brought back 3 Nov 21:40.sav. Your save from before is kept as a safety copy. ".into(), problem: false };
        let problem = Notice { text: "Close the game first, then pick.".into(), problem: true };
        for dot in [Dot::Green, Dot::Blue, Dot::Gold, Dot::Amber] {
            assert_eq!(
                progress_line(dot, "Needs you: two PCs moved on separately.", true, Some("2 min ago"), 3, Some(&news)),
                RailLine {
                    text: "Brought back 3 Nov 21:40.sav. Your save from before is kept as a safety copy.".into(),
                    dot: Dot::Text,
                    amber: false,
                    wrap: true
                },
                "{dot:?}"
            );
            assert_eq!(
                progress_line(dot, "Needs you: two PCs moved on separately.", false, None, 0, Some(&problem)),
                RailLine { text: "Close the game first, then pick.".into(), dot: Dot::Amber, amber: true, wrap: true },
                "{dot:?}"
            );
        }
        // An empty notice says nothing: the state's words stay.
        let empty = Notice { text: "  ".into(), problem: true };
        assert_eq!(progress_line(Dot::Gold, "", true, Some("just now"), 1, Some(&empty)).text, "Progress synced just now · 1 PC");
        // Its minute over, the worker passes none: the state's words come back.
        assert_eq!(
            progress_line(Dot::Green, "", true, Some("2 min ago"), 3, None),
            RailLine { text: "Progress synced 2 min ago · 3 PCs".into(), dot: Dot::Green, amber: false, wrap: false }
        );
        assert_eq!(progress_line(Dot::Amber, "Network folder unreachable.", true, None, 3, None).text, "Network folder unreachable.");
    }

    fn line(name: &str, on: bool, off: bool, standing: Standing, verdict: Verdict) -> Line {
        Line {
            id: name.into(),
            name: name.into(),
            badge: None,
            author: String::new(),
            repo: None,
            version: "1.0".into(),
            released: None,
            standing,
            on,
            off,
            switchable: true,
            folders: vec![name.into()],
            problems: Vec::new(),
            notes: None,
            back_to: None,
            pinned: false,
            replace_with: None,
            verdict,
            forge_slug: None,
            icon_url: None,
        }
    }

    /// An addon turned off is in AddOns but isn't on; its update doesn't go in at Play, nor does one
    /// SurvivorsForge holds back.
    #[test]
    fn the_rail_counts_the_addons_the_game_loads() {
        use crate::addons::catalog::Release;
        use crate::addons::forge::Why;
        let to = Release {
            tag: "v2".into(),
            version: None,
            published: Utc.with_ymd_and_hms(2026, 11, 1, 10, 0, 0).unwrap(),
            prerelease: false,
            file: "a.zip".into(),
            url: String::new(),
            sha256: "ab".into(),
            size: 1,
            interface: 0,
            folders: Default::default(),
            notes: None,
            downloads: None,
        };
        let update = |by_itself| Standing::Update { to: to.clone(), by_itself };
        let lines = [
            line("A", true, false, Standing::Current, Verdict::Go),
            line("B", true, false, update(true), Verdict::Go),
            line("C", false, true, update(true), Verdict::Go),
            line("D", true, false, update(false), Verdict::Go),
            line("E", true, false, update(true), Verdict::No(Why::Blocked(Some("its scan found malware".into())))),
            line("F", false, false, Standing::Current, Verdict::Go),
        ];
        let facts = addons_facts(&lines, 1, false, true);
        assert_eq!(facts, AddonsFacts { installed: 6, on: 4, at_play: 1, waiting_ok: 1, downloading: false, checked: true });
        assert_eq!(addons_line(facts).text, "4 addons on · 1 waits for your OK");
        assert_eq!(addons_line(addons_facts(&lines, 0, true, true)), RailLine { text: "4 addons on · 1 update at Play".into(), dot: Dot::Blue, amber: false, wrap: false });
        assert_eq!(addons_line(addons_facts(&[], 0, false, true)).text, "No addons yet");
    }

    #[test]
    fn needs_you_lists_every_question_in_order_with_its_buttons() {
        assert!(needs_you(&NeedsYouFacts::default()).is_empty());
        let facts = NeedsYouFacts {
            progress_split: Some("Desktop".into()),
            game_ask_pc: Some("Laptop".into()),
            game_own: Some("This PC's own, from before it joined, are kept as version 31".into()),
            addon_asks: vec![("azs-spell-meter".into(), "AZS Spell Meter".into(), "PC 2".into())],
            waiting_ok: vec![
                WaitingOk { id: "bordo-ui".into(), name: "BordoUI".into(), version: "2.8.18".into(), reason: "SurvivorsForge's scan found 1 warning in this version".into(), incoming: false },
                WaitingOk { id: "forge:totems".into(), name: "Totems".into(), version: String::new(), reason: String::new(), incoming: true },
            ],
            folder_down: true,
        };
        let questions = needs_you(&facts);
        let texts: Vec<&str> = questions.iter().map(|q| q.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Your progress here and on Desktop differ: pick the one to continue with. The other is kept as a version.",
                "The game's settings changed here and on Laptop since they last matched: pick one. This PC's stay until you do.",
                "This PC's own, from before it joined, are kept as version 31",
                "AZS Spell Meter's settings changed here and on PC 2 since they last matched: pick one.",
                "BordoUI 2.8.18 waits for your OK: SurvivorsForge's scan found 1 warning in this version",
                "Totems, from another PC, waits for your OK",
                "The network folder doesn't answer: your progress is safe on this PC and goes up when the folder is back.",
            ]
        );
        assert_eq!(questions[0].buttons, [("Choose…".to_string(), Action::ChooseProgress)]);
        assert_eq!(questions[1].buttons, [("Keep this PC's".to_string(), Action::GameKeepHere), ("Use Laptop's".to_string(), Action::GameUseOther)]);
        assert_eq!(questions[2].buttons, [("Use them instead".to_string(), Action::GameUseOwn)]);
        assert_eq!(questions[3].buttons, [("Keep this PC's".to_string(), Action::AddonKeepHere("azs-spell-meter".into())), ("Use PC 2's".to_string(), Action::AddonUseOther("azs-spell-meter".into()))]);
        assert_eq!(questions[4].buttons, [("Install anyway".to_string(), Action::InstallAnyway("bordo-ui".into())), ("Show what it found".to_string(), Action::ShowScan("bordo-ui".into()))]);
        assert_eq!(questions[6].buttons, [("Open Settings".to_string(), Action::OpenSettings)]);
        assert_eq!(questions.len(), 7);
        // Blank names ask nothing.
        assert!(needs_you(&NeedsYouFacts { progress_split: Some(" ".into()), game_ask_pc: Some(String::new()), ..Default::default() }).is_empty());
    }

    #[test]
    fn within_a_group_any_ticked_box_matches_and_across_groups_all_must() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let row = |lists: &[&str], safety: &[&str], groups: &[&str]| vec![s(lists), s(safety), s(groups)];
        let lucky = row(&["Discord Verified", "SurvivorsForge"], &["verified"], &["Meters and HUDs", "Miscellaneous"]);
        let bordo = row(&["SurvivorsForge"], &["flagged"], &["Full interfaces"]);
        // Nothing ticked: everything through.
        let none = vec![vec![], vec![], vec![]];
        assert!(filter_match(&none, &lucky) && filter_match(&none, &bordo));
        // Within Safety, either ticked word matches.
        let either = vec![vec![], s(&["verified", "flagged"]), vec![]];
        assert!(filter_match(&either, &lucky) && filter_match(&either, &bordo));
        // Across groups, all must: verified and Full interfaces matches neither.
        let both = vec![vec![], s(&["verified"]), s(&["Full interfaces"])];
        assert!(!filter_match(&both, &lucky) && !filter_match(&both, &bordo));
        let lists = vec![s(&["Discord Verified"]), vec![], s(&["Miscellaneous"])];
        assert!(filter_match(&lists, &lucky) && !filter_match(&lists, &bordo));
        // Case doesn't matter; a group the row lacks fails when something is ticked in it.
        assert!(filter_match(&vec![vec![], s(&["VERIFIED"]), vec![]], &lucky));
        assert!(!filter_match(&vec![vec![], vec![], vec![], s(&["x"])], &lucky));
    }
}
