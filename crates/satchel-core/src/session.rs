//! Following the game across its own update restarts.
//!
//! Any process running from the game folder counts as the game. `Closed` fires the moment the
//! last one is gone, when the save on disk is complete and gets sent. `Over` comes only after a
//! quiet window (30 s, or 3 min when the game's files just changed), so an update restart stays
//! one session. Times are seconds on any monotonic scale.

pub const QUIET_SECS: i64 = 30;
pub const QUIET_AFTER_UPDATE_SECS: i64 = 180;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Running,
    Quiet { since: i64, window: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Started,
    /// The last game process is gone. `updating`: the game's files just changed.
    Closed { updating: bool },
    /// The game came back inside the quiet window: the same session goes on.
    Resumed,
    /// The game's files changed during the quiet window: an update is under way.
    Updating,
    /// Nothing ran for the whole quiet window.
    Over,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tracker {
    pub phase: Phase,
    pub started_at: Option<i64>,
    /// When the game last closed: the session's real end for playtime.
    pub closed_at: Option<i64>,
    /// The game updated itself during this session.
    pub updated: bool,
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker { phase: Phase::Idle, started_at: None, closed_at: None, updated: false }
    }
}

impl Tracker {
    /// A session is open (the game runs, or closed less than a quiet window ago).
    pub fn in_session(&self) -> bool {
        self.phase != Phase::Idle
    }

    pub fn game_running(&self) -> bool {
        self.phase == Phase::Running
    }

    pub fn tick(&mut self, now: i64, game_running: bool, files_changed: bool) -> Option<Event> {
        match self.phase {
            Phase::Idle if game_running => {
                *self = Tracker { phase: Phase::Running, started_at: Some(now), ..Tracker::default() };
                Some(Event::Started)
            }
            Phase::Idle => None,
            Phase::Running if !game_running => {
                let window = if files_changed { QUIET_AFTER_UPDATE_SECS } else { QUIET_SECS };
                self.phase = Phase::Quiet { since: now, window };
                self.closed_at = Some(now);
                self.updated |= files_changed;
                Some(Event::Closed { updating: files_changed })
            }
            Phase::Running => None,
            Phase::Quiet { .. } if game_running => {
                self.phase = Phase::Running;
                Some(Event::Resumed)
            }
            Phase::Quiet { since, window } => {
                if files_changed && window < QUIET_AFTER_UPDATE_SECS {
                    self.phase = Phase::Quiet { since, window: QUIET_AFTER_UPDATE_SECS };
                    self.updated = true;
                    return Some(Event::Updating);
                }
                if now - since >= window {
                    self.phase = Phase::Idle;
                    return Some(Event::Over);
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Event::*;

    fn run(steps: &[(i64, bool, bool)]) -> (Vec<(i64, Event)>, Tracker) {
        let mut tracker = Tracker::default();
        let events = steps
            .iter()
            .filter_map(|&(t, running, changed)| tracker.tick(t, running, changed).map(|e| (t, e)))
            .collect();
        (events, tracker)
    }

    #[test]
    fn a_plain_session() {
        let (events, tracker) = run(&[(0, true, false), (100, true, false), (200, false, false), (229, false, false), (230, false, false)]);
        assert_eq!(events, [(0, Started), (200, Closed { updating: false }), (230, Over)]);
        assert_eq!((tracker.started_at, tracker.closed_at, tracker.updated), (Some(0), Some(200), false));
    }

    #[test]
    fn an_update_restart_is_one_session() {
        let (events, tracker) = run(&[
            (0, true, false),
            (100, false, true), // closes with fresh files: the 3-minute window
            (200, false, false),
            (250, true, false), // the updated game starts again
            (900, false, false),
            (930, false, false),
        ]);
        assert_eq!(events, [(0, Started), (100, Closed { updating: true }), (250, Resumed), (900, Closed { updating: false }), (930, Over)]);
        assert!(tracker.updated);
        assert_eq!(tracker.closed_at, Some(900));
    }

    #[test]
    fn files_changing_after_the_close_stretch_the_window() {
        let (events, _) = run(&[
            (0, true, false),
            (100, false, false), // 30 s window
            (110, false, true),  // the updater writes: 3 minutes from the close
            (140, false, false), // would have ended the session without the stretch
            (200, true, false),
        ]);
        assert_eq!(events, [(0, Started), (100, Closed { updating: false }), (110, Updating), (200, Resumed)]);
    }

    #[test]
    fn a_new_session_starts_clean() {
        let mut tracker = Tracker::default();
        tracker.tick(0, true, false);
        tracker.tick(10, false, true);
        tracker.tick(400, false, false);
        assert_eq!(tracker.tick(500, true, false), Some(Started));
        assert!(!tracker.updated);
        assert_eq!(tracker.closed_at, None);
    }
}
