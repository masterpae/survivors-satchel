//! The game's own settings on every PC, the worker's side: they travel in the shared addon list
//! as one more settings set, so the machinery is share's (`settings_decide`, the sends, the
//! bring-ins, the choice); what is the game's alone is here: the look with the game closed, the
//! question in Settings, "Use them instead", and what Settings and Sync now say about them.

use super::Worker;
use crate::view::GameSettingsLine;
use chrono::{DateTime, Utc};
use satchel_core::activity::Kind;
use satchel_core::addons::share::{self, Bundle, SettingsStep};
use satchel_core::addons::GameAddons;
use satchel_core::game_settings;
use satchel_core::log;
use satchel_core::paths::Shared;
use std::time::{Duration, Instant};

/// How long an answer about the game's settings stays in Settings.
const GAME_NOTE_SHOWN: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(super) struct GameSettingsState {
    /// What the game's own settings need, from the last look with the game closed.
    step: Option<SettingsStep>,
    /// Why the game's settings can't be read here, in plain words.
    problem: Option<String>,
    /// The last answer about the game's settings, shown in Settings for a minute.
    note: Option<(String, Instant)>,
}

impl GameSettingsState {
    /// The game's settings stay on each PC now: nothing is needed of them.
    pub(super) fn forget(&mut self) {
        self.step = None;
        self.problem = None;
    }
}

impl Worker {
    /// The shared folder and the game's folders, when the game's own settings go to every PC:
    /// sync on with a folder it can use, Settings' switch on, the game's folder known.
    pub(super) fn game_share_place(&self) -> Option<(Shared, GameAddons)> {
        if !self.engine.settings.game.shared {
            return None;
        }
        Some((self.engine.shared()?, self.game_addons()?))
    }

    /// The game's own settings, decided (the game closed): this PC's go up; the list's wait for
    /// Play; a question waits when both changed.
    pub(super) fn game_settings_look(&mut self, shared: &Shared, game: &GameAddons) {
        let Some(list) = self.share.list.clone() else { return };
        let id = game_settings::ID;
        let set = match game_settings::read(&game.config) {
            Ok(set) => set,
            Err(e) => {
                log!("{} not read: {e}", game_settings::NAME);
                self.game_settings.step = None;
                self.game_settings.problem = Some(format!("They can't be read here ({e}); nothing goes or comes until they can"));
                return;
            }
        };
        self.game_settings.problem = None;
        let mut step = self.settings_decide(id, game_settings::NAME, &set, &list);
        if step == SettingsStep::Send {
            let seen = list.newest_settings(id).map(|v| v.n);
            match self.share_send_settings(shared, vec![(id.to_string(), set, seen, None)]) {
                Ok(1) => step = SettingsStep::InStep { adopt: None },
                Ok(_) => {}
                Err(e) => log!("{} not sent yet: {e}", game_settings::NAME),
            }
        }
        self.game_settings.step = Some(step);
    }

    /// The same pick for the game's own settings, in Settings.
    pub(super) fn game_settings_choice(&mut self, keep_here: bool) {
        if !keep_here && self.game_started_meanwhile() {
            self.game_say("Close the game first: its settings change only while it's closed.".into(), true);
            return;
        }
        let Some((shared, game)) = self.game_share_place() else { return };
        self.settings_choice(&shared, &game, game_settings::ID, keep_here);
    }

    /// "Use them instead": the game settings this PC had when it joined come back here and go to
    /// the other PCs as the newest version.
    pub(super) fn game_use_own(&mut self) {
        if self.game_started_meanwhile() {
            self.game_say("Close the game first: its settings change only while it's closed.".into(), true);
            return;
        }
        let Some((shared, game)) = self.game_share_place() else { return };
        let Some(n) = self.share.memory().game_own_kept.as_ref().map(|k| k.n) else { return };
        match self.game_use_own_at(&shared, &game, n) {
            Ok(text) => {
                self.share.memory_mut().game_own_kept = None;
                self.activity(Kind::Game, &text);
                self.game_say(text, false);
            }
            Err(text) => {
                log!("{text}");
                self.game_say(text, true);
            }
        }
        self.share_save();
        self.share_look();
    }

    fn game_use_own_at(&mut self, shared: &Shared, game: &GameAddons, n: u64) -> Result<String, String> {
        let what = game_settings::NAME;
        let set = self.settings_set(game_settings::ID).ok_or_else(|| format!("{what} aren't shared here"))?;
        let pc = self.engine.pc().to_string();
        let (own, version) = {
            let _lock = share::lock(shared, &pc).map_err(|e| format!("{what}: {e}"))?;
            let mut list = share::read(shared).map_err(|e| format!("{what}: {e}"))?;
            let kept = list
                .settings_version(n)
                .filter(|v| v.addon == game_settings::ID && v.pc == pc)
                .cloned()
                .ok_or_else(|| format!("{what}: this PC's own (version {n}) are gone from the network folder"))?;
            let own = share::settings_bytes(shared, &kept).map_err(|e| format!("{what}: version {n} couldn't be read: {e}"))?;
            let note = Some(format!("{pc}'s own settings from before it joined (version {n}), chosen"));
            let again = Bundle { newest: kept.written, ..own.clone() };
            let version = share::send_settings(shared, &mut list, game_settings::ID, &pc, &again, note, Utc::now()).map_err(|e| format!("{what}: {e}"))?;
            self.share.list = Some(list);
            (own, version)
        };
        // Sent: if the game started meanwhile, they come in at the next Play like any newer version.
        if self.game_started_meanwhile() {
            return Ok(format!("{what}: this PC's own went to the network folder, and come in here once the game is closed"));
        }
        self.place_settings(&set, &own, game, game_settings::ID, Utc::now(), false).map_err(|e| format!("{what} weren't put in place: {e}"))?;
        self.share.memory_mut().settings.insert(game_settings::ID.into(), version.sha256.clone());
        Ok(format!("{what}: this PC's own are back, and go to the other PCs at their next Play"))
    }

    /// An answer about the game's settings: on the rail and, for a minute, in Settings where the
    /// question was; `problem` when it didn't go through.
    pub(super) fn game_say(&mut self, text: String, problem: bool) {
        self.game_settings.note = Some((text.clone(), Instant::now()));
        self.feedback.tell(text, problem);
    }

    /// Settings' line under "The game's own settings on every PC", with the last answer about them.
    pub(super) fn game_settings_view(&self) -> GameSettingsLine {
        let mut line = self.game_settings_line();
        if let Some((note, at)) = &self.game_settings.note
            && at.elapsed() < GAME_NOTE_SHOWN
        {
            line.note = note.clone();
        }
        line
    }

    fn game_settings_line(&self) -> GameSettingsLine {
        if self.game_share_place().is_none() {
            return GameSettingsLine::default();
        }
        if let Some(split) = self.share.memory().splits.get(game_settings::ID) {
            let text = format!("They changed here and on {} since they last matched: pick one. This PC's stay until you do.", split.pc);
            return GameSettingsLine { text, kind: 2, ask_pc: split.pc.clone(), ..Default::default() };
        }
        if let Some(problem) = self.share.problem().or(self.game_settings.problem.as_ref()) {
            return GameSettingsLine { text: problem.clone(), kind: 2, ..Default::default() };
        }
        let list = self.share.list.as_ref();
        let newest = list.and_then(|l| l.newest_settings(game_settings::ID));
        // A version sent again by the PC that chose it is named after the PC it came from.
        let from = |v: &share::SettingsVersion| -> String {
            list.and_then(|l| l.settings.iter().filter(|o| o.addon == v.addon && o.sha256 == v.sha256).min_by_key(|o| o.n))
                .map_or_else(|| v.pc.clone(), |first| first.pc.clone())
        };
        let mut line = match (&self.game_settings.step, newest) {
            (Some(SettingsStep::BringIn { .. }), Some(v)) => GameSettingsLine { text: format!("{}'s come in at the next Play", from(v)), kind: 1, ..Default::default() },
            (Some(SettingsStep::InStep { .. }), Some(v)) => {
                GameSettingsLine { text: format!("The same on every PC · {}'s, version {}", from(v), v.n), kind: 0, ..Default::default() }
            }
            (Some(SettingsStep::Send), _) => GameSettingsLine { text: "This PC's go to the network folder at the next look".into(), kind: 0, ..Default::default() },
            _ => GameSettingsLine { text: "They go to the network folder when the game closes".into(), kind: 0, ..Default::default() },
        };
        if let Some(kept) = &self.share.memory().game_own_kept
            && let Some(own) = list
                .and_then(|l| l.settings_version(kept.n))
                .filter(|v| v.addon == game_settings::ID && v.pc == self.engine.pc())
        {
            let lacks = newest.is_some_and(|v| v.files.iter().any(|f| !own.files.iter().any(|o| o.eq_ignore_ascii_case(f))));
            line.own = format!(
                "This PC's own, from before it joined, are kept as version {}{}",
                own.n,
                if lacks { ": they lack files the shared ones have, which would then go from every PC" } else { "" }
            );
        }
        line
    }

    /// One sentence for Sync now about the game's settings; `None` when they stay on each PC.
    pub(super) fn game_settings_status(&self) -> Option<String> {
        self.game_share_place()?;
        if self.share.memory().splits.contains_key(game_settings::ID) {
            return Some("The game's settings: a question in Settings".into());
        }
        if let Some(problem) = &self.game_settings.problem {
            return Some(format!("The game's settings: {}", problem.trim_end_matches('.')));
        }
        // They travel in the shared addon list: when it can't be used, they wait too. The addon
        // sentence before this one says why, when addons go to every PC.
        if let Some(problem) = self.share.problem() {
            return Some(match self.share_place() {
                Some(_) => "The game's settings wait too".into(),
                None => format!("The game's settings: {}", problem.trim_end_matches('.')),
            });
        }
        // They're compared only with the game closed: while it runs, the last look may be old.
        Some(match &self.game_settings.step {
            _ if self.sync.game_running() => "The game's settings go to the network folder when the game closes".into(),
            None => "The game's settings are compared at the next look".into(),
            Some(SettingsStep::BringIn { .. }) => "The game's settings from another PC come in at the next Play".into(),
            Some(SettingsStep::Send) => "This PC's game settings go to the network folder at the next look".into(),
            _ => "The game's settings are the same as in the network folder".into(),
        })
    }

    /// Play's part for the game's own settings, after the addons': another PC's come in, and a
    /// question still open is said. `false` when the game was started meanwhile, so nothing more
    /// is written.
    pub(super) fn game_settings_before_play(&mut self, started: &str, now: DateTime<Utc>) -> bool {
        if let Some((shared, game)) = self.game_share_place() {
            if let Some(SettingsStep::BringIn { n, keep_mine }) = self.game_settings.step.clone() {
                if self.game_started_meanwhile() {
                    self.feedback.card_line(started, false, true);
                    return false;
                }
                match self.share_bring_settings(&shared, &game, game_settings::ID, n, keep_mine, now) {
                    Ok(text) => {
                        self.activity(Kind::Game, &text);
                        self.feedback.card_line(&text, true, false);
                    }
                    Err(text) => {
                        log!("{text}");
                        self.feedback.card_line(&text, false, true);
                    }
                }
            }
            if let Some(split) = self.share.memory().splits.get(game_settings::ID) {
                let text = format!("The game's settings changed here and on {}: pick one in Settings (this PC's stay for now)", split.pc);
                self.feedback.card_line(&text, false, true);
            }
        }
        true
    }
}
