//! What the window shows, assembled: every concern's facts and views in one `View` (the tray,
//! the rail, Home, the Addons tab, Settings, the Play card), built every step and sent only
//! when it differs from what the window has; the lists read again from disk at each look; and
//! the words for times and sizes the views share.

use super::{Worker, non_empty};
use crate::about;
use crate::view::{Pill, RailView, Recent, Tray, View};
use chrono::{DateTime, Datelike, Local, Utc};
use satchel_core::activity;
use satchel_core::decide::Decision;
use satchel_core::engine::Shown;
use satchel_core::game_news::card as news_card;
use satchel_core::shell::{self, Dot, NeedsYouFacts, WaitingOk};

/// The lists the window shows that belong to no concern: Home's recent activity.
#[derive(Default)]
struct ViewLists {
    recent: Vec<Recent>,
}

#[derive(Default)]
pub(super) struct ViewState {
    lists: ViewLists,
    /// What the window was last sent: nothing goes out while it still holds.
    last_view: Option<View>,
}

impl ViewState {
    /// The next view goes out whether or not anything changed: the window shows an install as
    /// under way the moment it's asked for, and a view must follow.
    pub(super) fn push_again(&mut self) {
        self.last_view = None;
    }
}

pub(super) fn when(at: DateTime<Utc>) -> String {
    let local = at.with_timezone(&Local);
    let today = Local::now().date_naive();
    let day = local.date_naive();
    if day == today {
        format!("today {}", local.format("%H:%M"))
    } else if today.pred_opt() == Some(day) {
        format!("yesterday {}", local.format("%H:%M"))
    } else if day.year() == today.year() {
        local.format("%-d %b %H:%M").to_string()
    } else {
        local.format("%-d %b %Y").to_string()
    }
}

pub(super) fn clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M").to_string()
}

pub(super) fn ago(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    match (now - at).num_minutes() {
        ..=0 => "just now".into(),
        1 => "a minute ago".into(),
        m => format!("{m} minutes ago"),
    }
}

pub(super) fn size(bytes: u64) -> String {
    format!("{:.1} KB", bytes as f64 / 1024.0)
}

/// "released today", from this version's CHANGELOG heading; empty when it has no date.
fn released_text() -> String {
    about::released()
        .map(|day| satchel_core::addons::overview::released_ago((Local::now().date_naive() - day).num_days()))
        .unwrap_or_default()
}

impl Worker {
    // What the window shows ------------------------------------------------------------------

    /// The lists the window shows, read again from disk: the progress versions and PCs, the
    /// playtime, the recent activity; then the addons, whose folder may have changed.
    pub(super) fn build_lists(&mut self) {
        let now = Utc::now();
        let local_now = Local::now();
        let mut lists = ViewLists::default();
        // Progress versions, Your PCs: the network folder's versions, or this PC's copies.
        self.sync_lists(&local_now);
        // Playtime: this week, the last seven days, per PC, the last sessions.
        self.playtime_lists(&local_now);
        lists.recent = activity::recent(&self.engine.local.activity(), 5)
            .unwrap_or_default()
            .iter()
            .map(|e| Recent { kind: e.shown_kind(), text: e.text.clone(), when: shell::when_short(e.at, &local_now) })
            .collect();
        let _ = now;
        self.view.lists = lists;
        self.addons_tick();
    }

    /// What the window and tray show, and what the "Needs you" choice shows (if it is open).
    fn build_view(&self) -> (View, Option<Shown>) {
        let sync_on = self.engine.shared().is_some();
        let (dark, system_dark) = self.theme_dark();
        let version_new = self.version_new();
        let tray = if self.sync.split().is_some() || self.feedback.problem().is_some() {
            Tray::NeedsYou
        } else if self.feedback.busy().is_some() {
            Tray::Busy
        } else if self.sync.in_session() {
            Tray::Playing
        } else if sync_on && (self.sync.offline() || self.engine.state.send_pending) {
            Tray::Offline
        } else {
            Tray::Synced
        };
        let decision = self.sync.outlook().map(|o| &o.decision);
        let newest = self.sync.outlook().and_then(|o| o.index.newest_version());
        let (status, banner) = match tray {
            Tray::NeedsYou => match self.feedback.problem() {
                Some(text) => ("Needs you".to_string(), text.clone()),
                None => (
                    "Needs you".into(),
                    "Needs you: two PCs moved on separately. Press Play or Sync now to pick one; both are kept.".into(),
                ),
            },
            Tray::Busy => {
                let busy = self.feedback.busy().cloned().unwrap_or_default();
                (busy.clone(), busy)
            }
            Tray::Playing if !self.sync.game_running() => {
                // The quiet window: the game is closed, the session not yet over.
                let sent = match (sync_on, self.sync.sent()) {
                    (true, Some(n)) => format!("your progress went up as version {n}"),
                    (true, None) => "nothing new to send".into(),
                    (false, _) => "a copy is kept when the session closes".into(),
                };
                ("Game closed".into(), format!("Game closed · {sent}. The session closes once the game stays closed for a moment (longer after an update)."))
            }
            Tray::Playing => (
                "Game running".into(),
                if sync_on { "Game running · each time the game saves, a checkpoint goes to the network folder".into() } else { "Game running".into() },
            ),
            Tray::Offline => (
                "Network folder unreachable".into(),
                "Network folder unreachable: your progress is safe on this PC and goes up when the network folder is back.".into(),
            ),
            Tray::Synced => {
                let status = match decision {
                    Some(Decision::BringIn(_)) => "Newer progress in the network folder".to_string(),
                    _ if sync_on => "Synced".into(),
                    _ => "Ready".into(),
                };
                let banner = if self.game_dir().is_none() {
                    "The game wasn't found: set its folder in Settings.".into()
                } else {
                    match (decision, newest) {
                        (Some(Decision::BringIn(n)), Some(v)) => {
                            format!("{} has newer progress (version {n}). Press Play to bring it in and start.", v.pc)
                        }
                        (Some(Decision::Send), _) => "This PC has progress the network folder doesn't have yet. Sync now sends it.".into(),
                        _ if sync_on => "Synced: this PC has the newest progress".into(),
                        _ => "Ready · a copy of your progress is kept after each session".into(),
                    }
                };
                (status, banner)
            }
        };

        // Read fresh each time (a few KB): the cached check may predate the last session.
        let local = self.engine.local_save().ok().flatten();
        let local_now = Local::now();

        // The rail: the build, the progress, the addons, the last line.
        let news = self.news_view();
        let build = shell::build_line(&news.version);
        let dot = match tray {
            Tray::Synced => Dot::Green,
            Tray::Busy => Dot::Blue,
            Tray::Playing => Dot::Gold,
            Tray::NeedsYou | Tray::Offline => Dot::Amber,
        };
        let last = if sync_on { self.sync.last_sync() } else { self.sync.last_copy() };
        let last = last.map(|t| shell::when_short(t, &local_now));
        // A notice, for its minute, comes first whatever the state.
        let progress = shell::progress_line(dot, &banner, sync_on, last.as_deref(), self.sync.pc_count(), self.feedback.notice());
        let addons = self.addons_view();
        let addons_line = shell::addons_line(addons.rail);
        let playing = self.sync.in_session();
        let session_minutes = self.playtime.session_minutes();
        let last_line = shell::last_line(if playing { Some(session_minutes.unwrap_or(0)) } else { None }, sync_on, self.playtime.last_session());
        let rail = RailView { build, progress: Some(progress), addons: Some(addons_line), last: last_line };
        // The top bar's pill: an update downloaded, or this version's What's new unseen.
        let update = self.update_line();
        let pill = if !update.version.is_empty() {
            Pill { text: format!("Satchel {} → {} ready", about::VERSION, update.version), link: "Update now".into(), kind: 1 }
        } else if version_new {
            Pill { text: format!("Updated to {}", about::VERSION), link: "see what's new".into(), kind: 2 }
        } else {
            Pill::default()
        };
        // Home's Needs-you card: every question the app asks outside an overlay.
        let game_settings = self.game_settings_view();
        let waiting_ok: Vec<WaitingOk> = addons
            .rows
            .iter()
            .filter(|r| r.reason_kind == 1 && (r.can_update || r.incoming) && !r.installing)
            .map(|r| WaitingOk {
                id: r.id.clone(),
                name: r.name.clone(),
                version: if r.incoming { r.version.clone() } else { r.update.clone() },
                reason: r.reason.strip_prefix("Waits for your OK: ").unwrap_or(&r.reason).to_string(),
                incoming: r.incoming,
            })
            .collect();
        let split_pc = self.sync.split_pc();
        let facts = NeedsYouFacts {
            progress_split: self.sync.split().map(|_| split_pc.unwrap_or_else(|| "the other PC".into())),
            game_ask_pc: non_empty(&game_settings.ask_pc).map(str::to_string),
            game_own: non_empty(&game_settings.own).map(str::to_string),
            addon_asks: addons.rows.iter().filter(|r| !r.ask_pc.is_empty()).map(|r| (r.id.clone(), r.name.clone(), r.ask_pc.clone())).collect(),
            waiting_ok,
            folder_down: sync_on && self.sync.offline(),
        };
        let questions = shell::needs_you(&facts);
        // Home's What's new: the newest note the panel has, its version with it (not the update
        // server's, which a rollback there can make differ), its paragraph and first lines.
        let (news_version, news_blocks, news_more) = match news.notes.first() {
            Some(note) => {
                let (shown, more) = news_card(&note.blocks, |(kind, _)| *kind == 0);
                (note.version.clone(), shown.to_vec(), more)
            }
            None => (String::new(), Vec::new(), false),
        };

        let (needs_you, shown) = self.sync.needs_you_view(local.as_ref());
        let needs_note = self.sync.needs_note(needs_you.is_some());

        let saves_note = if sync_on {
            "Every version in the network folder, newest first, plus your own exports; bring an older one back if something went wrong. Your save from before is kept as a safety copy.".into()
        } else {
            "The copies kept on this PC after each session, newest first; bring an older one back if something went wrong.".into()
        };

        let view = View {
            tray,
            status,
            banner,
            busy: self.feedback.busy().is_some(),
            sync_on,
            playing,
            rail,
            pill,
            news_version,
            news_blocks,
            news_more,
            pcs: self.sync.pcs().to_vec(),
            pcs_week: self.playtime.pcs_week(),
            questions,
            recent: self.view.lists.recent.clone(),
            playtime: self.playtime.view(),
            saves: self.sync.saves().to_vec(),
            saves_summary: self.sync.saves_summary().to_string(),
            saves_note,
            needs_you,
            needs_note,
            card: self.feedback.card(),
            setup: self.settings.in_setup().then(|| self.setup_view()),
            addons,
            dark,
            system_dark,
            app_released: released_text(),
            game_found: self.game_dir().is_some(),
            game_settings,
            update,
            check_result: self.update.check_result(),
            checking: self.update.checking(),
            checks: self.update.checks(),
            card_update: if self.feedback.card_gated() { self.offer().map(|o| o.version.clone()).unwrap_or_default() } else { String::new() },
            game_news: news,
        };
        (view, shown)
    }

    /// The window and tray as they should be now, sent when that differs from what they show.
    pub(super) fn push(&mut self) {
        // A problem newly set is a notice too: it shows at once, ahead of an older one, and the
        // tray's needs-you state keeps it on the rail after its minute.
        if let Some(problem) = self.feedback.problem_untold() {
            self.feedback.warn(problem);
        }
        let (view, shown) = self.build_view();
        self.sync.set_shown(shown);
        if self.view.last_view.as_ref() != Some(&view) {
            self.view.last_view = Some(view.clone());
            self.ui.push(view);
        }
    }

    /// A long operation begins: the window says so now, before it runs.
    pub(super) fn busy_push(&mut self, text: String) {
        self.feedback.set_busy(text);
        self.push();
    }
}
