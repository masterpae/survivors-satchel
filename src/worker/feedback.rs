//! What the worker tells the window about its work as it goes: the rail's notice for a minute,
//! the problem the tray's needs-you state keeps up, the Play card's lines and its question, and
//! the busy line while a long operation runs. Every concern writes here; the view reads it.

use satchel_core::shell::Notice;
use std::time::{Duration, Instant};

/// How long a one-off message (a notice) stays on the rail's progress line, and the progress
/// question's own refusal in its overlay.
pub(super) const NOTICE_FOR: Duration = Duration::from_secs(60);

/// The Play card: its lines (the text, done or not, a warning or not) and whether it asks.
pub(super) type Card = (Vec<(String, bool, bool)>, bool);

#[derive(Default)]
pub(super) struct Feedback {
    notice: Option<Notice>,
    notice_at: Option<Instant>,
    problem: Option<String>,
    /// The problem the rail has been told of: a new one shows at once, as a notice.
    problem_told: Option<String>,
    /// The Play card's question is the version check's: it offers Update now.
    card_gate: bool,
    card: Option<Card>,
    card_until: Option<Instant>,
    busy: Option<String>,
}

impl Feedback {
    /// A one-off message, plain news: on the rail's progress line for a minute, whatever the
    /// tray's state, in the text colour.
    pub(super) fn say(&mut self, text: String) {
        self.tell(text, false);
    }

    /// A one-off problem: on the rail's progress line for a minute, in amber.
    pub(super) fn warn(&mut self, text: String) {
        self.tell(text, true);
    }

    pub(super) fn tell(&mut self, text: String, problem: bool) {
        self.notice = Some(Notice { text, problem });
        self.notice_at = Some(Instant::now());
    }

    /// The notice on the rail now, if its minute isn't up.
    pub(super) fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub(super) fn set_problem(&mut self, text: String) {
        self.problem = Some(text);
    }

    pub(super) fn clear_problem(&mut self) {
        self.problem = None;
    }

    pub(super) fn problem(&self) -> Option<&String> {
        self.problem.as_ref()
    }

    /// A problem newly set, once: it shows at once as a notice too, ahead of an older one.
    pub(super) fn problem_untold(&mut self) -> Option<String> {
        if self.problem == self.problem_told {
            return None;
        }
        self.problem_told = self.problem.clone();
        self.problem.clone()
    }

    pub(super) fn set_busy(&mut self, text: String) {
        self.busy = Some(text);
    }

    pub(super) fn clear_busy(&mut self) {
        self.busy = None;
    }

    pub(super) fn busy(&self) -> Option<&String> {
        self.busy.as_ref()
    }

    pub(super) fn open_card(&mut self) {
        if self.card.is_none() {
            self.card = Some((Vec::new(), false));
        }
        self.card_until = None;
    }

    /// A line on the Play card; on the rail too, for its minute (the card closes sooner), in amber
    /// when it warns. The card's question (`card_ask`) is not a notice.
    pub(super) fn card_line(&mut self, text: &str, done: bool, warn: bool) {
        self.tell(text.to_string(), warn);
        if let Some((lines, asking)) = self.card.as_mut() {
            *asking = false;
            lines.push((text.into(), done, warn));
            if lines.len() > 8 {
                lines.remove(0);
            }
        }
    }

    pub(super) fn card_ask(&mut self, text: String) {
        self.card_gate = false;
        self.open_card();
        if let Some((lines, asking)) = self.card.as_mut() {
            lines.push((text, false, true));
            *asking = true;
        }
    }

    /// The card's question is the version check's: Update now goes with it.
    pub(super) fn card_gate(&mut self) {
        self.card_gate = true;
    }

    pub(super) fn close_card(&mut self) {
        self.card = None;
    }

    pub(super) fn close_card_in(&mut self, seconds: u64) {
        self.card_until = Some(Instant::now() + Duration::from_secs(seconds));
    }

    pub(super) fn card_open(&self) -> bool {
        self.card.is_some()
    }

    /// The Play card's lines (each done or not, a warning or not) and whether it asks.
    pub(super) fn card(&self) -> Option<Card> {
        self.card.clone()
    }

    pub(super) fn card_gated(&self) -> bool {
        self.card_gate
    }

    /// The idle tick: the notice goes after its minute, the card at its time.
    pub(super) fn expire(&mut self) {
        if self.notice_at.is_some_and(|t| t.elapsed() >= NOTICE_FOR) {
            self.notice = None;
            self.notice_at = None;
        }
        if self.card_until.is_some_and(|t| Instant::now() >= t) {
            self.card = None;
            self.card_until = None;
        }
    }
}
