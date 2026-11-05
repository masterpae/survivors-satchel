//! The Satchel's own update, the worker's side: its newest GitHub release looked for now and then
//! (in a build that knows its repository), downloaded, checked and verified by the network
//! thread, then offered; Update now puts it in place. `crate::update` is the installer.

use super::{Worker, due};
use crate::about;
use crate::net::{self, Job};
use crate::update;
use crate::view::UpdateLine;
use satchel_core::{gate, log};
use std::time::{Duration, Instant};

/// How often the Satchel's own GitHub releases are looked at, in a build that knows its
/// repository; sooner after a look that failed, and soon when a newer Satchel keeps this one out.
const SATCHEL_LOOK_EVERY: Duration = Duration::from_secs(6 * 3600);
const SATCHEL_RETRY_EVERY: Duration = Duration::from_secs(20 * 60);
const SATCHEL_GATED_EVERY: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub(super) struct UpdateState {
    /// A newer Satchel from its GitHub release: downloaded, checked against its checksum and the
    /// version it calls itself, its signature verified. The only update there is: since 0.7.0
    /// nothing is taken from the network folder.
    github_offer: Option<update::Offer>,
    satchel_looked: Option<Instant>,
    satchel_failed: bool,
    /// Check for updates asked GitHub: its answer finishes the check.
    check_pending: bool,
    /// What Check for updates found, shown in About until the next check or an update.
    check_result: Option<String>,
    /// How many times Check for updates was taken up: the window shows "Checking…" from the
    /// click until a view counts it (and, while `check_pending`, after that too).
    checks: u32,
    /// Update now was pressed while a session was closing: it goes in when the session is over.
    update_after_session: bool,
}

impl UpdateState {
    /// The newer Satchel on offer: downloaded from GitHub, checked and verified.
    pub(super) fn offer(&self) -> Option<&update::Offer> {
        self.github_offer.as_ref()
    }

    /// What Check for updates found, for About; empty until a check.
    pub(super) fn check_result(&self) -> String {
        self.check_result.clone().unwrap_or_default()
    }

    /// Whether a check waits for GitHub's answer.
    pub(super) fn checking(&self) -> bool {
        self.check_pending
    }

    /// How many times Check for updates was taken up.
    pub(super) fn checks(&self) -> u32 {
        self.checks
    }
}

/// How a Satchel that a newer one keeps out gets the new one while none is on offer: from GitHub,
/// the only place updates come from.
pub(super) fn update_hint() -> &'static str {
    if about::REPOSITORY.is_empty() {
        "Get the new Satchel from its GitHub releases to update this one."
    } else {
        "Update now shows here once the new Satchel has downloaded from GitHub."
    }
}

impl Worker {
    /// The newer Satchel on offer: downloaded from GitHub, checked and verified.
    pub(super) fn offer(&self) -> Option<&update::Offer> {
        self.update.offer()
    }

    /// After an update, the top bar's pill says "Updated to …" until this version's What's new is
    /// opened: a PC that ran an earlier Satchel (its first start done) and hasn't opened it; a new
    /// install sets the version at its first start.
    pub(super) fn version_new(&self) -> bool {
        let state = &self.engine.state;
        state.setup_done && state.seen_version.as_deref() != Some(about::VERSION)
    }

    pub(super) fn whats_new_seen(&mut self) {
        if self.engine.state.seen_version.as_deref() != Some(about::VERSION) {
            self.engine.state.seen_version = Some(about::VERSION.into());
            if let Err(e) = self.engine.save_state() {
                log!("state not saved: {e}");
            }
        }
    }

    /// Update now: the Satchel downloaded from GitHub takes this one's place and starts (playing,
    /// when asked from Play), and this one closes.
    pub(super) fn update_now(&mut self) {
        if self.game_dir().is_some_and(|d| self.scanner.game_running(&d)) {
            self.feedback.warn("Close the game first: the Satchel updates while the game is closed.".into());
            return;
        }
        // The session closes up to a few minutes after the game (playtime, this PC's in-use
        // marker): the update waits for that.
        if self.sync.in_session() {
            self.update.update_after_session = true;
            self.feedback.say("The update goes in as soon as this session has closed, within a few minutes.".into());
            return;
        }
        let Some(offer) = self.offer().cloned() else {
            self.feedback.say("There's no newer Satchel to update from.".into());
            return;
        };
        self.update.check_result = None;
        self.busy_push(format!("Updating to {}…", offer.version));
        match update::install(&offer, self.sync.pending_play()) {
            Ok(()) => {
                log!("updating to {}: the new Satchel starts, this one closes", offer.version);
                self.quit = true;
            }
            Err(e) => {
                self.feedback.clear_busy();
                log!("the update to {} failed: {e}", offer.version);
                let by_hand = about::repository_url().map(|url| format!("; the new one can also be downloaded from {url}/releases")).unwrap_or_default();
                self.feedback.warn(format!("The Satchel didn't update ({e}). It goes on as it was{by_hand}."));
            }
        }
    }

    /// The session is over: an update that waited for it goes in now.
    pub(super) fn update_on_session_over(&mut self) {
        if std::mem::take(&mut self.update.update_after_session) {
            self.update_now();
        }
    }

    /// About's Check for updates: in a build that knows its repository, GitHub is asked now
    /// instead of at the next look, and its answer finishes the check (`satchel_answer`). What was
    /// found shows under the button and in the banner.
    pub(super) fn check_updates(&mut self) {
        // Counted first: whatever happens below, the next view ends the window's own "Checking…"
        // (a check that asks GitHub goes on showing it until the answer).
        self.update.checks += 1;
        // Pending only once the question is on its way to the network thread: a hand-off that
        // failed would otherwise keep "Checking…" up until the Satchel restarts.
        let asks_github = !about::REPOSITORY.is_empty();
        if asks_github {
            self.update.satchel_looked = None;
            self.update.check_pending = self.satchel_look();
        }
        let text = if self.update.check_pending {
            "Checking GitHub…".to_string()
        } else if let Some(offer) = self.offer() {
            format!("Checked: {}", offer.ready())
        } else if asks_github {
            "Checked: GitHub couldn't be asked".into()
        } else {
            "Checked: this build doesn't look on GitHub for updates".into()
        };
        self.update.check_result = Some(text.clone());
        let failed = asks_github && !self.update.check_pending && self.offer().is_none();
        self.feedback.tell(text, failed);
    }

    /// In a build that knows its repository: the Satchel's newest GitHub release, on start and
    /// every few hours (sooner after a failed look). A newer one is downloaded, checked and its
    /// signature verified, then offered (Update now). When a newer Satchel keeps this one out and nothing
    /// offers it yet, that very release is looked for, every few minutes. True when a look was
    /// handed to the network thread just now.
    pub(super) fn satchel_look(&mut self) -> bool {
        if about::REPOSITORY.is_empty() {
            return false;
        }
        let want = self.sync.gated().map(str::to_string).filter(|newer| self.offer().is_none_or(|o| gate::is_newer(newer, &o.version)));
        let every = match (&want, self.update.satchel_failed) {
            (Some(_), _) => SATCHEL_GATED_EVERY,
            (None, true) => SATCHEL_RETRY_EVERY,
            (None, false) => SATCHEL_LOOK_EVERY,
        };
        if !due(self.update.satchel_looked, every) {
            return false;
        }
        self.update.satchel_looked = Some(Instant::now());
        self.net.send(Job::Satchel { repo: about::REPOSITORY.into(), dir: self.engine.local.satchel_download(), want }).is_ok()
    }

    pub(super) fn satchel_answer(&mut self, result: Result<Option<update::Offer>, String>) {
        self.update.satchel_failed = result.is_err();
        let failed = match result {
            Ok(offer) => {
                self.update.github_offer = offer;
                None
            }
            Err(e) => {
                log!("the Satchel's GitHub release not taken: {e}");
                // An offer stays only while its files do: a refused release takes everything
                // downloaded with it.
                if self.update.github_offer.as_ref().is_some_and(|o| !o.source.is_file() || !o.signature.is_file()) {
                    self.update.github_offer = None;
                }
                Some(e)
            }
        };
        // The answer Check for updates waits for.
        if std::mem::take(&mut self.update.check_pending) {
            // GitHub not answering, or a release refused, is a problem; anything else news.
            let problem = failed.is_some() && self.offer().is_none();
            let text = match (self.offer(), failed) {
                (Some(offer), _) => format!("Checked: {}", offer.ready()),
                // No answer says so itself ("GitHub didn't answer (…)"); else a release was refused
                // (no checksum, a wrong one, another version inside, no signature or a wrong one).
                (None, Some(e)) if e.starts_with(net::NO_ANSWER) => format!("Checked: {e}"),
                (None, Some(e)) => format!("Checked: nothing could be taken from GitHub ({e})"),
                (None, None) => format!("Checked: you have the newest version ({})", about::VERSION),
            };
            self.update.check_result = Some(text.clone());
            self.feedback.tell(text, problem);
        }
    }

    /// The Play tab's update line; the banner says why nothing syncs when the version check
    /// keeps this Satchel out.
    pub(super) fn update_line(&self) -> UpdateLine {
        match (self.sync.gated(), self.offer()) {
            (gated, Some(offer)) => {
                UpdateLine { text: format!("Survivor's Satchel {}.", offer.ready()), version: offer.version.clone(), gated: gated.is_some() }
            }
            (Some(_), None) => UpdateLine { text: update_hint().into(), version: String::new(), gated: true },
            (None, None) => UpdateLine::default(),
        }
    }
}
