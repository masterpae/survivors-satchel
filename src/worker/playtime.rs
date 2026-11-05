//! Playtime, the worker's side: the session under way, from the game's start to its last close;
//! each one recorded as it ends and sent on with the progress (the core keeps the files); Home's
//! Playtime card, the rail's last line and each PC's last session, from the sessions kept here.

use super::Worker;
use crate::view::PlaytimeView;
use chrono::{DateTime, Local, Utc};
use satchel_core::log;
use satchel_core::playtime::{self, Session, format_long};
use satchel_core::shell;

#[derive(Default)]
pub(super) struct PlaytimeState {
    session_start: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
    /// Home's Playtime card.
    view: PlaytimeView,
    /// The newest session: its day as the rail says it, its minutes, its PC.
    last_session: Option<(String, u32, String)>,
    pcs_week: String,
}

impl PlaytimeState {
    /// The game started: a session begins.
    pub(super) fn started(&mut self, now: DateTime<Utc>) {
        self.session_start = Some(now);
        self.closed_at = None;
    }

    /// The game closed; the session goes on until it stays closed.
    pub(super) fn closed(&mut self, now: DateTime<Utc>) {
        self.closed_at = Some(now);
    }

    /// The session is over: from its start to the game's last close (now, where either isn't
    /// known), played on this PC.
    pub(super) fn over(&mut self, pc: &str, now: DateTime<Utc>, updated: bool) -> Session {
        let start = self.session_start.take().unwrap_or(now);
        let end = self.closed_at.take().unwrap_or(now);
        Session::new(pc, start, end, updated, None)
    }

    /// How long the session under way has run, in minutes; `None` outside one.
    pub(super) fn session_minutes(&self) -> Option<u32> {
        self.session_start.map(|start| ((Utc::now() - start).num_seconds().max(0) / 60) as u32)
    }

    pub(super) fn view(&self) -> PlaytimeView {
        self.view.clone()
    }

    pub(super) fn pcs_week(&self) -> String {
        self.pcs_week.clone()
    }

    /// The newest session, for the rail's last line: its day, its minutes, its PC.
    pub(super) fn last_session(&self) -> Option<(&str, u32, &str)> {
        self.last_session.as_ref().map(|(day, minutes, pc)| (day.as_str(), *minutes, pc.as_str()))
    }

    /// Playtime: this week, the last seven days, per PC, the last sessions.
    fn build(&mut self, sessions: &[Session], local_now: &DateTime<Local>) {
        let totals = playtime::totals(sessions, local_now);
        let most = totals.week_per_pc.iter().map(|(_, m)| *m).max().unwrap_or(0).max(1);
        let days = playtime::per_day(sessions, local_now);
        let tallest = days.iter().map(|d| d.minutes).max().unwrap_or(0).max(1);
        self.view = PlaytimeView {
            week: format_long(totals.week),
            week_sessions: totals.week_sessions,
            today: format_long(totals.today),
            all: format_long(totals.all),
            days: days
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let label = if i == days.len() - 1 { "Today".to_string() } else { d.date.format("%a").to_string() };
                    (label, d.minutes as f32 / tallest as f32, d.minutes)
                })
                .collect(),
            per_pc: totals.week_per_pc.iter().map(|(pc, m)| (pc.clone(), format_long(*m), *m as f32 / most as f32)).collect(),
            sessions: sessions.iter().take(30).map(|s| (shell::day_long(s.start, local_now), s.pc.clone(), format_long(s.minutes))).collect(),
        };
        self.pcs_week =
            format!("This week: {} · {} session{}", format_long(totals.week), totals.week_sessions, if totals.week_sessions == 1 { "" } else { "s" });
        self.last_session = sessions.first().map(|s| (shell::day_word(s.start, local_now), s.minutes, s.pc.clone()));
    }
}

/// When a PC last played, among the sessions known here: its newest session's start.
pub(super) fn last_played(sessions: &[Session], pc: &str) -> Option<DateTime<Utc>> {
    sessions.iter().filter(|s| s.pc == pc).map(|s| s.start).max()
}

impl Worker {
    /// Playtime that waited for the network folder goes up.
    pub(super) fn flush_playtime(&self) {
        if let Err(e) = self.engine.flush_playtime() {
            log!("waiting playtime not sent: {e}");
        }
    }

    /// A session over, recorded on this PC and sent on with the progress.
    pub(super) fn record_session(&self, session: &Session) {
        if let Err(e) = self.engine.record_session(session) {
            log!("playtime not recorded: {e}");
        }
    }

    /// The Playtime card and the rail's last line again, from the sessions kept on this PC.
    pub(super) fn playtime_lists(&mut self, local_now: &DateTime<Local>) {
        let sessions = self.engine.sessions().unwrap_or_default();
        self.playtime.build(&sessions, local_now);
    }
}
