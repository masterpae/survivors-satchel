//! Settings and the first start, the worker's side: each change the window sends, checked and
//! written (the core's rules), the form the window shows, the first start's one screen, and what
//! the settings say about the game's folder, the theme and Windows (autostart, the shortcut).

use super::{Worker, non_empty};
use crate::about;
use crate::view::{Answered, Form, FormView, Setup};
use crate::win;
use chrono::Utc;
use satchel_core::engine::REACH_WAIT;
use satchel_core::settings::{self as settings_rules, Settings, Theme};
use satchel_core::{log, paths};
use std::path::{Path, PathBuf};

pub(super) struct SettingsState {
    /// The first start's screen, while it is up.
    setup: Option<Setup>,
}

impl SettingsState {
    pub(super) fn new(first_start: bool) -> SettingsState {
        SettingsState { setup: first_start.then(Setup::default) }
    }

    /// Whether the first start's screen is up: the idle tick waits for Start.
    pub(super) fn in_setup(&self) -> bool {
        self.setup.is_some()
    }

    fn enter(&mut self) {
        self.setup = Some(Setup::default());
    }

    fn leave(&mut self) {
        self.setup = None;
    }
}

impl Worker {
    pub(super) fn game_dir(&self) -> Option<PathBuf> {
        self.engine.settings.game_dir.clone().filter(|dir| dir.join(win::GAME_EXE).is_file())
    }

    /// The gear's Open the game's folder.
    pub(super) fn open_game_folder(&self) {
        if let Some(dir) = self.engine.settings.game_dir.clone() {
            win::open_folder(&dir);
        }
    }

    /// The no-game panel's Get the game.
    pub(super) fn open_game_site(&self) {
        win::open_url(win::GAME_SITE);
    }

    /// The window closed to the tray: whether it was maximised is remembered for the next start.
    pub(super) fn window_closed(&mut self, maximised: bool) {
        if self.engine.settings.window.maximised == maximised {
            return;
        }
        let mut settings = self.engine.settings.clone();
        settings.window.maximised = maximised;
        match settings.save(&self.engine.local.settings()) {
            Ok(()) => self.engine.settings = settings,
            Err(e) => log!("the window's state not saved: {e}"),
        }
    }

    pub(super) fn apply_windows_settings(&self) {
        let settings = &self.engine.settings;
        if let Err(e) = win::set_start_with_windows(settings.start_with_windows) {
            log!("start with Windows not set: {e}");
        }
        let icon = self.game_dir().map(|d| d.join(win::GAME_EXE));
        if let Err(e) = win::set_play_shortcut(settings.desktop_shortcut, icon.as_deref()) {
            log!("desktop shortcut not set: {e}");
        }
    }

    /// One Settings change, from its row (`key`, as the window names it) with the value the window
    /// sent. Only that row is checked (`settings::change`), and only a value that changed and went
    /// through is written (atomically, as every settings file is). The window gets the form as
    /// saved, with the row that applied ("✓ Saved") or why it was refused (the text typed stays in
    /// its field). Only sync, the network folder and this PC's name read the folder again.
    pub(super) fn setting(&mut self, key: &str, typed: &str) {
        // As the settings keep it: a mapped drive as its network path; a name emptied, this PC's
        // Windows name.
        let value = match key {
            "folder" => non_empty(typed).map(|f| win::network_path(Path::new(f)).display().to_string()).unwrap_or_default(),
            "pc-name" => non_empty(typed).map_or_else(win::computer_name, str::to_string),
            _ => typed.to_string(),
        };
        let answer = settings_rules::change(&self.engine.settings, key, &value, &Checks(self));
        let mut answered = Answered { key: key.to_string(), typed: typed.to_string(), applied: false, why: String::new() };
        if let Some(refusal) = answer.refused {
            log!("setting {key} not applied: {}", refusal.why);
            answered.why = refusal.why;
        } else if answer.applied.is_some() {
            match self.write_settings(answer.settings) {
                Ok(()) => answered.applied = true,
                Err(e) => {
                    log!("setting {key} not saved: {e}");
                    answered.why = format!("Not saved: {e}");
                }
            }
        }
        if !answered.applied {
            self.answer_form(answered);
            return;
        }
        self.after_settings_changed(key);
        self.answer_form(answered);
    }

    /// Writes `next` as this PC's settings and puts it in force. Every name this PC has had is
    /// remembered: typed again, it's never taken for another PC's.
    fn write_settings(&mut self, next: Settings) -> std::io::Result<()> {
        next.save(&self.engine.local.settings())?;
        log!("settings saved (sync {}, folder {:?}, PC {:?})", next.sync.enabled, next.sync.folder, next.pc_name);
        let mut named = false;
        for name in [self.engine.settings.pc_name.clone(), next.pc_name.clone()] {
            if !self.engine.state.pc_names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                self.engine.state.pc_names.push(name);
                named = true;
            }
        }
        if named && let Err(e) = self.engine.save_state() {
            log!("state not saved: {e}");
        }
        if next.sync != self.engine.settings.sync {
            self.sync_settings_changed();
        }
        self.engine.settings = next;
        self.feedback.clear_problem();
        Ok(())
    }

    /// Restore defaults: the switches and the theme as a new install has them, except sync and
    /// the two sharing switches; the folders and this PC's name stay (`settings::restore_defaults`).
    pub(super) fn restore_defaults(&mut self) {
        let next = settings_rules::restore_defaults(&self.engine.settings);
        let mut answered = Answered { key: "defaults".into(), ..Answered::default() };
        if next != self.engine.settings {
            match self.write_settings(next) {
                Ok(()) => self.after_settings_changed("defaults"),
                Err(e) => {
                    log!("defaults not saved: {e}");
                    self.feedback.warn(format!("The defaults couldn't be saved: {e}"));
                    self.answer_form(answered);
                    return;
                }
            }
        }
        // Said even when nothing had to change: the switches are as a new install has them.
        answered.applied = true;
        self.answer_form(answered);
    }

    /// Settings' Find it: the game looked for in the usual folders, and kept as the game's folder
    /// when it passes the same check a folder typed there does (at the first start, until Start).
    pub(super) fn detect_game(&mut self) {
        match win::detect_game_dir() {
            Some(dir) => self.game_folder_picked(dir),
            None => self.answer_form(Answered {
                key: "game-dir".into(),
                typed: self.form_view().saved.game_dir,
                applied: false,
                why: "The game wasn't found in the usual folders: paste its folder and press Enter".into(),
            }),
        }
    }

    /// A game folder picked or found: at the first start shown, and kept with Start; else it goes
    /// as a typed game folder does.
    pub(super) fn game_folder_picked(&mut self, dir: PathBuf) {
        if !self.settings.in_setup() {
            self.setting("game-dir", &dir.display().to_string());
            return;
        }
        self.engine.settings.game_dir = Some(dir);
        self.answer_form(Answered { key: "game-dir".into(), ..Answered::default() });
    }

    /// Browse's switch: the view is kept in settings.toml as soon as it is picked, on its own, so
    /// fields being edited in Settings stay as they are (the form isn't pushed again). It is the
    /// player's pick from then on (`AddonSettings::pick_view`): the cards too stay, where a cards
    /// nobody picked loads as the table.
    pub(super) fn browse_view(&mut self, view: &str) {
        let addons = &self.engine.settings.addons;
        if addons.browse_view_picked && addons.browse_view == satchel_core::settings::browse_view_or_default(view) {
            return;
        }
        let mut settings = self.engine.settings.clone();
        settings.addons.pick_view(view);
        match settings.save(&self.engine.local.settings()) {
            Ok(()) => self.engine.settings = settings,
            Err(e) => log!("Browse's view not saved: {e}"),
        }
    }

    /// How another PC syncing here spells `name` (in any case), as the versions' index and the
    /// shared addon list name the PCs; `None` when none does, and for this PC's own names, now
    /// and before.
    fn name_taken(&self, name: &str) -> Option<String> {
        // Names meet in file names, made safe first: "PC:2" and "PC-2" would share one file.
        let same = |other: &str| satchel_core::names::safe(other).eq_ignore_ascii_case(&satchel_core::names::safe(name));
        if std::iter::once(&self.engine.settings.pc_name).chain(&self.engine.state.pc_names).any(|n| same(n)) {
            return None;
        }
        let index = self.sync.pc_names();
        let list = self.share.pc_names();
        index.chain(list).find(|pc| same(pc)).cloned()
    }

    fn form_view(&self) -> FormView {
        let settings = &self.engine.settings;
        let folder_status = if !settings.sync.enabled {
            String::new()
        } else if settings.sync.folder.is_none() {
            "Paste the network folder (a mapped drive like F:\\ is stored as its network path).".into()
        } else if let Some(why) = self.engine.folder_problem() {
            format!("✗ Can't be used: {why}")
        } else if self.sync.offline() {
            "✗ Not answering right now".into()
        } else if let Some(warning) = self.link_warning() {
            warning
        } else if let Some(outlook) = self.sync.outlook() {
            let pcs = outlook.index.pcs.len().max(1);
            format!("✓ Folder answers · {pcs} PC{} seen · {} version{}", if pcs == 1 { "" } else { "s" }, outlook.index.versions.len(), if outlook.index.versions.len() == 1 { "" } else { "s" })
        } else {
            "✓ Versions go in its sync subfolder".into()
        };
        let form = Form {
            game_dir: settings.game_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default(),
            sync: settings.sync.enabled,
            folder: settings.sync.folder.as_ref().map(|f| f.display().to_string()).unwrap_or_default(),
            pc_name: settings.pc_name.clone(),
            start_with_windows: settings.start_with_windows,
            desktop_shortcut: settings.desktop_shortcut,
            notifications: settings.notifications,
            addons_auto: settings.addons.update_by_themselves,
            addons_shared: settings.addons.shared,
            addons_forge: settings.addons.forge,
            pause_when_away: settings.pause_when_away,
            theme: theme_index(settings.theme),
            game_shared: settings.game.shared,
            browse_view: settings.addons.browse_view.clone(),
        };
        FormView {
            saved: form,
            game_status: game_status(settings.game_dir.as_deref()),
            game_ok: game_ok(settings.game_dir.as_deref()),
            folder_status,
            settings_path: self.engine.local.settings().display().to_string(),
            answered: None,
        }
    }

    pub(super) fn push_form(&self) {
        self.ui.push_form(self.form_view());
    }

    /// The form as saved, answering one change: the row that applied, or why it was refused.
    fn answer_form(&self, answered: Answered) {
        self.ui.push_form(FormView { answered: Some(answered), ..self.form_view() });
    }

    /// The game's progress folder or the shared folder is a link (junction) to another place,
    /// as some guides suggest for cloud sync: then two syncs would work on one file.
    fn link_warning(&self) -> Option<String> {
        if paths::is_link(&self.engine.local.save_dir) {
            return Some("⚠ The game's progress folder is a link to another place: if a cloud app syncs it, two syncs work on one file".into());
        }
        let folder = self.engine.settings.sync_folder()?;
        paths::is_link(folder)
            .then(|| "⚠ This folder is a link to another place: if that is a cloud folder, two syncs work on one file".into())
    }

    /// First start: one screen. The game's folder comes from detection (or the defaults file
    /// beside the exe); sync, when that file turned it on, is said in one line (`setup_view`). The
    /// "which progress" choice of before 0.9 is gone: the normal "two PCs changed it" question
    /// handles that case at the first Play and on Home.
    pub(super) fn enter_setup(&mut self) {
        self.settings.enter();
        self.push_form();
        self.ui.show(None);
    }

    /// The first-start screen's sync line, for the game folder it shows now: "Sync is on ·
    /// <folder>" when the defaults file turned sync on, or, when that folder can't be used with
    /// this game folder, the line that says sync stays off (in amber), as Start will say it too.
    pub(super) fn setup_view(&self) -> Setup {
        let settings = &self.engine.settings;
        let Some(folder) = settings.sync_folder() else { return Setup::default() };
        match paths::sync_folder_problem(folder, &self.engine.local, settings.game_dir.as_deref()) {
            Some(why) => Setup { sync_line: defaults_folder_line(&why), sync_warn: true },
            None => Setup { sync_line: format!("Sync is on · {}", folder.display()), sync_warn: false },
        }
    }

    /// The first start's Start: the game's folder (detected, or picked there) and what the
    /// defaults file beside the exe set are written, and the first sync runs.
    pub(super) fn finish_setup(&mut self) {
        let refuse = |worker: &Self, why: &str| {
            let typed = worker.form_view().saved.game_dir;
            worker.answer_form(Answered { key: "game-dir".into(), typed, applied: false, why: why.into() });
        };
        let mut settings = self.engine.settings.clone();
        let Some(dir) = settings.game_dir.clone().filter(|d| game_ok(Some(d))) else {
            refuse(self, "The game isn't in this folder: choose the one it is in");
            return;
        };
        // The first sync may replace this PC's save: only with the game closed.
        if self.scanner.game_running(&dir) {
            refuse(self, "The game is running: close it, then press Start");
            return;
        }
        // A network folder from the defaults file that can't be used: sync stays off rather than
        // the first start stopping here, where Settings can't be reached to change it.
        let problem = settings.sync_folder().and_then(|folder| paths::sync_folder_problem(folder, &self.engine.local, Some(&dir)));
        if let Some(why) = problem {
            log!("sync left off at the first start: {why}");
            settings.sync.enabled = false;
            self.feedback.warn(defaults_folder_line(&why));
        }
        if let Err(e) = settings.save(&self.engine.local.settings()) {
            log!("settings not saved: {e}");
            refuse(self, &format!("Settings couldn't be saved: {e}"));
            return;
        }
        log!("settings saved (sync {}, folder {:?}, PC {:?})", settings.sync.enabled, settings.sync.folder, settings.pc_name);
        if !self.engine.state.pc_names.iter().any(|n| n.eq_ignore_ascii_case(&settings.pc_name)) {
            self.engine.state.pc_names.push(settings.pc_name.clone());
        }
        self.engine.settings = settings;
        self.apply_windows_settings();
        self.engine.state.setup_done = true;
        // A new install has nothing to tell about earlier versions.
        self.engine.state.seen_version = Some(about::VERSION.into());
        let _ = self.engine.save_state();
        self.first_sync(Utc::now());
        self.settings.leave();
        log!("first start done");
        self.push_form();
        self.startup();
        self.ui.show(Some(0));
    }

    /// Whether the window is dark, and whether Windows' apps are (the title bar is set again when
    /// either changes: Windows resets it when its own mode changes).
    pub(super) fn theme_dark(&self) -> (bool, bool) {
        let system = win::apps_use_dark();
        let dark = match self.engine.settings.theme {
            Theme::System => system,
            Theme::Light => false,
            Theme::Dark => true,
        };
        (dark, system)
    }

    /// The no-game panel's Find it: the game looked for in the usual folders, and kept when it
    /// passes the check a folder typed in Settings does; Settings shows it.
    pub(super) fn find_game(&mut self) {
        let Some(dir) = win::detect_game_dir() else {
            self.feedback.warn("The game wasn't found in the usual folders. If it's installed, choose its folder in Settings.".into());
            return;
        };
        let found = dir.display().to_string();
        let answer = settings_rules::change(&self.engine.settings, "game-dir", &found, &Checks(self));
        if let Some(refusal) = answer.refused {
            log!("found game not used: {}", refusal.why);
            let mut why = refusal.why.chars();
            let why = why.next().map(|first| first.to_lowercase().collect::<String>() + why.as_str()).unwrap_or_default();
            self.feedback.warn(format!("The game was found in {found}, but {why}. See Settings."));
            return;
        }
        if answer.applied.is_some()
            && let Err(e) = self.write_settings(answer.settings)
        {
            self.feedback.set_problem(format!("Settings couldn't be saved: {e}"));
            return;
        }
        log!("game found at {found}");
        self.apply_windows_settings();
        self.feedback.say(format!("The game was found in {found}"));
        self.answer_form(Answered { key: "game-dir".into(), typed: found, applied: answer.applied.is_some(), why: String::new() });
        self.refresh();
        self.addons_scan();
        self.share_look();
    }
}

/// What a Settings change is checked against, from this PC: the paths the network folder must
/// stay apart from, whether a folder answers, the names the other PCs syncing there use, and
/// whether the game is in a folder.
struct Checks<'a>(&'a Worker);

impl settings_rules::Checks for Checks<'_> {
    fn folder_problem(&self, folder: &Path, game_dir: Option<&Path>) -> Option<String> {
        paths::sync_folder_problem(folder, &self.0.engine.local, game_dir)
    }
    fn folder_answers(&self, folder: &Path) -> bool {
        paths::Shared { folder: folder.to_path_buf() }.reachable_within(REACH_WAIT)
    }
    fn name_taken(&self, name: &str) -> Option<String> {
        self.0.name_taken(name)
    }
    fn game_found(&self, dir: &Path) -> bool {
        game_ok(Some(dir))
    }
}

/// What the first start says, on its screen and then on the rail, when the network folder of the
/// defaults file beside the exe can't be used (`why`): sync stays off.
fn defaults_folder_line(why: &str) -> String {
    format!("Sync stays off: the network folder from the defaults file can't be used ({why}). Choose another in Settings.")
}

fn game_status(dir: Option<&Path>) -> String {
    match dir {
        Some(dir) if dir.join(win::GAME_EXE).is_file() => "✓ Game found".into(),
        Some(_) => format!("✗ {} isn't in this folder", win::GAME_EXE),
        None => "Not set: press Find it, or paste the folder".into(),
    }
}

/// The game is in this folder.
fn game_ok(dir: Option<&Path>) -> bool {
    dir.is_some_and(|dir| dir.join(win::GAME_EXE).is_file())
}

/// The theme as Settings' list shows it: System, Light, Dark.
fn theme_index(theme: Theme) -> i32 {
    match theme {
        Theme::System => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    }
}
