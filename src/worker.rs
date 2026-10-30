//! The worker thread: watches the game, runs the sync engine, tells the UI what to show.
//! Everything slow (the network share above all) happens here, never on the UI thread.

mod addons;
mod share;

use crate::net::{self, Answer};
use crate::about;
use crate::update;
use crate::view::{Form, FormView, Recent, Setup, Tray, Ui, UpdateLine, View};
use crate::win;
use chrono::{DateTime, Datelike, Local, Utc};
use satchel_core::activity::{self, Kind};
use satchel_core::decide::Decision;
use satchel_core::engine::{BroughtIn, Engine, Outlook, REACH_WAIT, Shown, SyncError};
use satchel_core::fsx::fingerprint;
use satchel_core::gate;
use satchel_core::log;
use satchel_core::markers::{HEARTBEAT_SECS, InUse, Other};
use satchel_core::paths;
use satchel_core::playtime::{self, Session, format_minutes};
use satchel_core::session::{Event, Tracker};
use satchel_core::settings::Theme;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

const TICK: Duration = Duration::from_secs(2);
const REFRESH_EVERY: Duration = Duration::from_secs(60);
const RETRY_EVERY: Duration = Duration::from_secs(180);
/// How often the Satchel's own GitHub releases are looked at, in a build that knows its
/// repository; sooner after a look that failed, and soon when a newer Satchel keeps this one out.
const SATCHEL_LOOK_EVERY: Duration = Duration::from_secs(6 * 3600);
const SATCHEL_RETRY_EVERY: Duration = Duration::from_secs(20 * 60);
const SATCHEL_GATED_EVERY: Duration = Duration::from_secs(10 * 60);
/// A write to the live save is checkpointed once it has been quiet this long.
const SETTLE: Duration = Duration::from_secs(3);
/// How long a one-off message stays in the banner.
const NOTICE_FOR: Duration = Duration::from_secs(60);
/// The window's tabs, in order: Play, Addons, Saves, Playtime, Settings.
const TAB_SETTINGS: i32 = 4;

pub enum Command {
    Play,
    PlayAnyway,
    CancelPlay,
    SyncNow,
    BringBack(usize),
    ShowInFolder(usize),
    OpenFolder,
    KeepHere,
    UseOther,
    DecideLater,
    SaveSettings(Form),
    /// Settings' switches back to how a new install starts (in the form; Save makes it stick).
    DefaultSettings,
    DetectGame(Form),
    FinishSetup(Form, i32),
    // Addons: ids are a record's, a catalog entry's, or `folder:<name>`.
    AddonsCheck,
    AddonsUpdateAll,
    AddonsOpenFolder,
    AddonSwitch(String, bool),
    AddonUpdate(String),
    AddonBack(String),
    AddonKeep(String, bool),
    AddonRemove(String, bool),
    AddonPage(String),
    AddonFolder(String),
    /// "Only on this PC" on or off.
    AddonLocalOnly(String, bool),
    /// "Settings per PC" on or off (for every PC).
    AddonSettingsPerPc(String, bool),
    /// After an addon's settings changed here and on another PC: `true` keeps this PC's.
    AddonSettingsChoice(String, bool),
    /// Installs a verified addon, or replaces a copy installed by hand (`folder:<name>`).
    BrowseInstall(String),
    LinkLookup(String),
    LinkInstall,
    LinkClear,
    ImportZip(PathBuf),
    /// The Removed list's Install again.
    InstallAgain(String),
    /// After the game's settings changed here and on another PC: `true` keeps this PC's.
    GameSettingsChoice(bool),
    /// "Use them instead": this PC's own game settings from before it joined.
    GameSettingsOwn,
    /// The newer Satchel beside `sync\` replaces this one.
    UpdateNow,
    /// About's Check for updates: the exe beside `sync\` now and, in a release build, GitHub now.
    CheckUpdates,
    /// What's new was opened: "Updated to …" goes.
    WhatsNewSeen,
    /// The no-game panel's Find it: the game's folder is looked for and kept when found.
    /// The no-game panel's Find it, with the Settings form as it stands (unsaved edits stay).
    FindGame(Form),
    /// The no-game panel's Get the game.
    OpenGameSite,
    /// An answer from the network thread.
    Net(Answer),
    Quit,
}

/// What a row of the Saves tab stands for.
#[derive(Clone)]
enum Row {
    Version(u64),
    File(PathBuf),
}

#[derive(Default)]
struct Lists {
    saves: Vec<[String; 5]>,
    rows: Vec<Row>,
    today: String,
    week: String,
    all_time: String,
    pc_times: Vec<(String, String, f32)>,
    sessions: Vec<String>,
    recent: Vec<Recent>,
    last_copy: Option<DateTime<Utc>>,
}

pub struct Worker {
    engine: Engine,
    ui: Ui,
    tracker: Tracker,
    scanner: win::Scanner,
    away: win::AwayWatch,
    epoch: Instant,
    // The session.
    marker: Option<InUse>,
    session_start: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
    sent: Option<u64>,
    launched: bool,
    live_seen: Option<SystemTime>,
    live_changed: Option<Instant>,
    writes: u32,
    files_stamp: Option<SystemTime>,
    last_heartbeat: Option<Instant>,
    // The PC and the NAS.
    outlook: Option<Outlook>,
    offline: bool,
    offline_told: bool,
    problem: Option<String>,
    notice: Option<String>,
    notice_at: Option<Instant>,
    split: Option<u64>,
    needs_you_open: bool,
    /// What the "Needs you" choice shows on screen: an answer applies to exactly that.
    shown: Option<Shown>,
    /// The same for the pick on the first-start panel.
    setup_shown: Option<Shown>,
    // Play.
    pending_play: bool,
    play_anyway: bool,
    // A newer Satchel: the exe beside `sync\` as last seen, and the one it offers; the version
    // another PC used when that keeps this one out of the shared files (`gate`).
    /// This PC's own in-use marker from an earlier run has been dealt with (or there was none).
    leftover_done: bool,
    update_watch: update::Watch,
    /// A newer Satchel beside `sync\`.
    update_offer: Option<update::Offer>,
    /// A newer Satchel from its GitHub release, downloaded and checked.
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
    gated: Option<String>,
    /// The loop ends after this step (an update started the new Satchel).
    quit: bool,
    /// Update now was pressed while a session was closing: it goes in when the session is over.
    update_after_session: bool,
    /// The Play card's question is the version check's: it offers Update now.
    card_gate: bool,
    card: Option<(Vec<(String, bool, bool)>, bool)>,
    card_until: Option<Instant>,
    busy: Option<String>,
    setup: Option<Setup>,
    lists: Lists,
    last_refresh: Option<Instant>,
    last_retry: Option<Instant>,
    last_view: Option<View>,
    addons: addons::AddonsState,
    share: share::ShareState,
}

fn due(last: Option<Instant>, every: Duration) -> bool {
    last.is_none_or(|t| t.elapsed() >= every)
}

fn when(at: DateTime<Utc>) -> String {
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

fn when_opt(at: Option<DateTime<Utc>>) -> String {
    at.map(when).unwrap_or_else(|| "–".into())
}

fn clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M").to_string()
}

fn ago(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    match (now - at).num_minutes() {
        ..=0 => "just now".into(),
        1 => "a minute ago".into(),
        m => format!("{m} minutes ago"),
    }
}

fn size(bytes: u64) -> String {
    format!("{:.1} KB", bytes as f64 / 1024.0)
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The newest change among the game's own program files (exe, libraries, `data`), never its
/// settings: an update replaces these, an ordinary session doesn't.
fn game_files_stamp(dir: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut seen = 0;
    let note = |path: &Path, newest: &mut Option<SystemTime>| {
        if let Ok(modified) = fs::metadata(path).and_then(|m| m.modified()) {
            *newest = Some(newest.map_or(modified, |n| n.max(modified)));
        }
    };
    if let Ok(entries) = fs::read_dir(dir) {
        for path in entries.flatten().map(|e| e.path()) {
            let program = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| ["exe", "dll", "pak", "bin", "dat"].contains(&e.to_ascii_lowercase().as_str()));
            if program {
                note(&path, &mut newest);
            }
        }
    }
    if let Ok(entries) = fs::read_dir(dir.join("data")) {
        for path in entries.flatten().map(|e| e.path()) {
            seen += 1;
            if seen > 2000 {
                break;
            }
            note(&path, &mut newest);
        }
    }
    newest
}

impl Worker {
    /// `commands` is the worker's own channel: the network thread answers on it.
    pub fn new(engine: Engine, ui: Ui, first_start: bool, commands: Sender<Command>) -> Worker {
        let store_path = engine.local.addon_store();
        let had_store = store_path.exists();
        let store = addons::load_store(&store_path);
        let mut memory = share::load_memory(&engine.local.addon_share());
        // Records recognised again from AddOns carry none of their switches: decided as on a first
        // run, the list's switches come back here instead of being cleared on every PC.
        if (!had_store || !store_path.exists()) && memory != satchel_core::addons::share::Memory::default() {
            log!("addons.json was missing or unreadable: the shared addon list is taken as on a first run");
            // The game's own settings aren't an addon's: their memory and question stay.
            let id = satchel_core::game_settings::ID;
            let (base, question, own) = (memory.settings.remove(id), memory.splits.remove(id), memory.game_own_kept.take());
            memory = satchel_core::addons::share::Memory::default();
            memory.settings.extend(base.map(|b| (id.to_string(), b)));
            memory.splits.extend(question.map(|q| (id.to_string(), q)));
            memory.game_own_kept = own;
        }
        let net = net::start(engine.local.addon_cache(), commands);
        Worker {
            addons: addons::AddonsState::new(store, net),
            share: share::ShareState::new(memory),
            engine,
            ui,
            tracker: Tracker::default(),
            scanner: win::Scanner::new(),
            away: win::AwayWatch::start(),
            epoch: Instant::now(),
            marker: None,
            session_start: None,
            closed_at: None,
            sent: None,
            launched: false,
            live_seen: None,
            live_changed: None,
            writes: 0,
            files_stamp: None,
            last_heartbeat: None,
            outlook: None,
            offline: false,
            offline_told: false,
            problem: None,
            notice: None,
            notice_at: None,
            split: None,
            needs_you_open: false,
            shown: None,
            setup_shown: None,
            pending_play: false,
            play_anyway: false,
            leftover_done: false,
            update_watch: update::Watch::default(),
            update_offer: None,
            github_offer: None,
            satchel_looked: None,
            satchel_failed: false,
            check_pending: false,
            check_result: None,
            checks: 0,
            gated: None,
            quit: false,
            update_after_session: false,
            card_gate: false,
            card: None,
            card_until: None,
            busy: None,
            setup: first_start.then(Setup::default),
            lists: Lists::default(),
            last_refresh: None,
            last_retry: None,
            last_view: None,
        }
    }

    pub fn run(mut self, commands: Receiver<Command>) {
        win::com_init();
        win::register_notifications();
        log!(
            "Survivor's Satchel {} started on {} (sync {}, game folder {})",
            env!("CARGO_PKG_VERSION"),
            self.engine.pc(),
            if self.engine.shared().is_some() { "on" } else { "off" },
            self.engine.settings.game_dir.as_deref().map_or_else(|| "not set".into(), |d| d.display().to_string())
        );
        if self.setup.is_some() {
            self.enter_setup();
        } else {
            self.startup();
        }
        self.push();
        loop {
            let command = match commands.recv_timeout(TICK) {
                Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
            };
            // A step that hits a bug is logged (the panic hook) and the next one goes on: sync and
            // the tray never stop over one.
            let step = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Some(command) = command {
                    self.handle(command);
                }
                if self.setup.is_none() {
                    self.tick();
                }
                self.push();
            }));
            if step.is_err() {
                self.busy = None;
                self.say("The Satchel hit an internal error and carried on; its log has the details.".into());
            }
            if self.quit {
                break;
            }
        }
        log!("Survivor's Satchel closed");
        self.ui.quit();
    }

    fn startup(&mut self) {
        self.apply_windows_settings();
        if let Err(e) = self.engine.flush_playtime() {
            log!("waiting playtime not sent: {e}");
        }
        self.refresh();
        self.push_form();
        self.addons_scan();
        self.addons_prune_cache();
        self.addons_look();
        self.share_look();
        self.satchel_look();
    }

    fn handle(&mut self, command: Command) {
        // The window shows an install as under way the moment it's asked for: a view always
        // follows one, even when nothing else changed (a refusal said a second time, say).
        if matches!(command, Command::BrowseInstall(_) | Command::AddonUpdate(_) | Command::LinkInstall) {
            self.last_view = None;
        }
        // What changed here goes to the other PCs at once.
        let changes_addons = matches!(
            command,
            Command::AddonsUpdateAll
                | Command::AddonSwitch(..)
                | Command::AddonUpdate(_)
                | Command::AddonBack(_)
                | Command::AddonKeep(..)
                | Command::AddonRemove(..)
                | Command::BrowseInstall(_)
                | Command::LinkInstall
                | Command::ImportZip(_)
                | Command::InstallAgain(_)
                | Command::SaveSettings(_)
        );
        match command {
            Command::Play => self.play(),
            Command::PlayAnyway => {
                self.play_anyway = true;
                self.play();
            }
            Command::CancelPlay => {
                self.pending_play = false;
                self.play_anyway = false;
                self.card = None;
            }
            Command::SyncNow => self.sync_now(),
            Command::BringBack(row) => self.bring_back(row),
            Command::ShowInFolder(row) => self.show_in_folder(row),
            Command::OpenFolder => {
                if let Some(shared) = self.engine.shared() {
                    win::open_folder(&shared.folder);
                }
            }
            Command::KeepHere => self.resolve(true),
            Command::UseOther => self.resolve(false),
            Command::DecideLater => {
                self.needs_you_open = false;
                self.pending_play = false;
                self.card = None;
            }
            Command::SaveSettings(form) => {
                if self.save_settings(form) {
                    self.refresh();
                    self.push_form();
                    self.addons_scan();
                }
            }
            Command::DefaultSettings => {
                // The switches only: the game's folder, the shared folder and this PC's name stay.
                let defaults = satchel_core::settings::Settings::default();
                let mut view = self.form_view();
                view.form.start_with_windows = defaults.start_with_windows;
                view.form.desktop_shortcut = defaults.desktop_shortcut;
                view.form.notifications = defaults.notifications;
                view.form.addons_auto = defaults.addons.update_by_themselves;
                view.form.addons_shared = defaults.addons.shared;
                view.form.pause_when_away = defaults.pause_when_away;
                view.form.theme = theme_index(defaults.theme);
                view.form.game_shared = defaults.game.shared;
                self.ui.push_form(view);
            }
            Command::DetectGame(form) => {
                let mut view = self.form_view();
                view.form = form;
                if let Some(dir) = win::detect_game_dir() {
                    view.form.game_dir = dir.display().to_string();
                }
                view.game_status = game_status(non_empty(&view.form.game_dir).map(Path::new));
                self.ui.push_form(view);
            }
            Command::FinishSetup(form, choice) => self.finish_setup(form, choice),
            Command::AddonsCheck => self.addons_check(),
            Command::AddonsUpdateAll => self.addons_update_all(),
            Command::AddonsOpenFolder => self.addons_open_folder(),
            Command::AddonSwitch(id, on) => self.addon_switch(&id, on),
            Command::AddonUpdate(id) => self.addon_update(&id),
            Command::AddonBack(id) => self.addon_back(&id),
            Command::AddonKeep(id, keep) => self.addon_keep(&id, keep),
            Command::AddonRemove(id, keep_settings) => self.addon_remove(&id, keep_settings),
            Command::AddonPage(id) => self.addon_page(&id),
            Command::AddonFolder(id) => self.addon_folder(&id),
            Command::AddonLocalOnly(id, on) => self.addon_local_only(&id, on),
            Command::AddonSettingsPerPc(id, on) => self.addon_settings_per_pc(&id, on),
            Command::AddonSettingsChoice(id, keep_here) => self.addon_settings_choice(&id, keep_here),
            Command::BrowseInstall(id) => self.browse_install(&id),
            Command::LinkLookup(text) => self.link_lookup(text),
            Command::LinkInstall => self.link_install(),
            Command::LinkClear => self.link_clear(),
            Command::ImportZip(path) => self.import_zip(path),
            Command::InstallAgain(id) => self.install_again(&id),
            Command::GameSettingsChoice(keep_here) => self.game_settings_choice(keep_here),
            Command::GameSettingsOwn => self.game_use_own(),
            Command::UpdateNow => self.update_now(),
            Command::CheckUpdates => self.check_updates(),
            Command::WhatsNewSeen => self.whats_new_seen(),
            Command::FindGame(form) => self.find_game(form),
            Command::OpenGameSite => win::open_url(win::GAME_SITE),
            Command::Net(answer) => self.addons_answer(answer),
            Command::Quit => {}
        }
        if changes_addons {
            self.share_look();
        }
    }

    // The game ------------------------------------------------------------------------------

    fn game_dir(&self) -> Option<PathBuf> {
        self.engine.settings.game_dir.clone().filter(|dir| dir.join(win::GAME_EXE).is_file())
    }

    fn tick(&mut self) {
        let now_s = self.epoch.elapsed().as_secs() as i64;
        let dir = self.game_dir();
        let running = dir.as_ref().is_some_and(|d| self.scanner.game_running(d));
        // Esc to the game when it stops being the window in front, when that's asked for.
        self.away.watch(dir.as_deref().filter(|_| running && self.engine.settings.pause_when_away));
        let changed = !running
            && self.tracker.in_session()
            && dir.as_ref().and_then(|d| game_files_stamp(d)).is_some_and(|stamp| Some(stamp) > self.files_stamp);
        if let Some(event) = self.tracker.tick(now_s, running, changed) {
            self.on_event(event);
        }
        if self.tracker.game_running() {
            self.watch_save();
        }
        // The marker stays alive for the whole session, the quiet window after a close included.
        if self.tracker.in_session() {
            self.heartbeat();
        }
        if self.notice_at.is_some_and(|t| t.elapsed() >= NOTICE_FOR) {
            self.notice = None;
            self.notice_at = None;
        }
        if self.card_until.is_some_and(|t| Instant::now() >= t) {
            self.card = None;
            self.card_until = None;
        }
        if !self.tracker.in_session() && self.busy.is_none() {
            let unsent = self.offline || self.engine.state.send_pending;
            if self.engine.shared().is_some() && unsent && due(self.last_retry, RETRY_EVERY) {
                self.retry();
            } else if due(self.last_refresh, REFRESH_EVERY) {
                self.refresh();
                self.share_look();
                self.satchel_look();
            }
        }
    }

    fn on_event(&mut self, event: Event) {
        let now = Utc::now();
        match event {
            Event::Started => self.session_started(now),
            Event::Closed { updating } => {
                log!("game closed{}", if updating { " and its files changed: an update" } else { "" });
                self.closed_at = Some(now);
                if self.engine.shared().is_some() {
                    self.send_after_close();
                    // The game writes the addons' settings as it closes: they go up now.
                    self.share_look();
                }
                if updating {
                    self.card_line("The game is updating itself. Waiting for it to restart…", false, false);
                }
            }
            Event::Updating => {
                log!("the game's files changed after it closed: an update");
                self.card_line("The game is updating itself. Waiting for it to restart…", false, false);
            }
            Event::Resumed => {
                log!("the game started again inside the quiet window: the same session goes on");
                self.files_stamp = self.game_dir().and_then(|d| game_files_stamp(&d));
                if self.card.is_some() {
                    self.card_line("Game restarted after its update", true, false);
                    self.close_card_in(8);
                }
            }
            Event::Over => self.session_over(now),
        }
    }

    fn session_started(&mut self, now: DateTime<Utc>) {
        log!("game started{}", if self.launched { " by Play" } else { " without Play" });
        self.session_start = Some(now);
        self.closed_at = None;
        self.sent = None;
        self.writes = 0;
        self.live_seen = fs::metadata(self.engine.local.live_save()).and_then(|m| m.modified()).ok();
        self.live_changed = None;
        self.files_stamp = self.game_dir().and_then(|d| game_files_stamp(&d));
        if self.engine.shared().is_none() {
            return;
        }
        let marker = InUse { pc: self.engine.pc().into(), started: now, heartbeat: now, checkpoint: None, checkpointed: None };
        match self.engine.write_marker(&marker) {
            Ok(()) => {}
            Err(SyncError::Offline) => self.offline = true,
            Err(e) => log!("in-use marker not written: {e}"),
        }
        self.marker = Some(marker);
        self.last_heartbeat = Some(Instant::now());
        if self.launched {
            return;
        }
        // Started the old way: the Satchel can't swap a save the game has already loaded.
        if let Ok(outlook) = self.engine.outlook(now) {
            if let Decision::BringIn(n) | Decision::Split { nas: n } = outlook.decision {
                let from = outlook.index.get(n).map(|v| v.pc.clone()).unwrap_or_default();
                self.say(format!("{from} has newer progress: close the game and press Play to bring it in."));
                self.notify(
                    "Newer progress in the network folder",
                    &format!("{from} has newer progress. Close the game and press Play to bring it in; if you keep playing, you'll pick one at the end."),
                );
            }
            for other in outlook.others.iter().filter(|o| !o.stale) {
                self.notify(
                    &format!("Careful: the game is also open on {}", other.marker.pc),
                    &format!("Started {}. If you keep playing here, you'll pick one progress at the end.", clock(other.marker.started)),
                );
            }
            self.outlook = Some(outlook);
        }
    }

    /// While the game runs: every write to the live save is noted, and once it settles a
    /// checkpoint goes to the NAS.
    fn watch_save(&mut self) {
        let modified = fs::metadata(self.engine.local.live_save()).and_then(|m| m.modified()).ok();
        if modified != self.live_seen {
            self.live_seen = modified;
            self.live_changed = Some(Instant::now());
            self.writes += 1;
        }
        if self.live_changed.is_some_and(|t| t.elapsed() >= SETTLE) {
            self.live_changed = None;
            let Some(mut marker) = self.marker.take() else { return };
            match self.engine.checkpoint(&mut marker, Utc::now()) {
                Ok(true) => {
                    log!("checkpoint sent");
                    self.offline = false;
                    self.last_heartbeat = Some(Instant::now());
                }
                Ok(false) => {}
                Err(SyncError::Offline) => self.offline = true,
                Err(e) => log!("checkpoint failed: {e}"),
            }
            self.marker = Some(marker);
        }
    }

    fn heartbeat(&mut self) {
        if !due(self.last_heartbeat, Duration::from_secs(HEARTBEAT_SECS as u64)) {
            return;
        }
        self.last_heartbeat = Some(Instant::now());
        // An update restart can be quicker than one look at the processes, and then goes unseen.
        // Re-reading the game's files while it runs keeps such an update from making the next
        // ordinary close look like one.
        if self.tracker.game_running() {
            let now = self.game_dir().and_then(|d| game_files_stamp(&d));
            self.files_stamp = self.files_stamp.max(now);
        }
        if let Some(marker) = self.marker.as_mut() {
            marker.heartbeat = Utc::now();
            match self.engine.write_marker(marker) {
                Ok(()) => self.offline = false,
                Err(SyncError::Offline) => self.offline = true,
                Err(e) => log!("heartbeat failed: {e}"),
            }
        }
    }

    /// The game closed: the save on disk is complete and goes out now, while Windows is held
    /// from shutting down.
    fn send_after_close(&mut self) {
        let _hold = win::Hold::start();
        self.busy = Some("Sending your progress…".into());
        self.push();
        let result = self.engine.send(Utc::now(), None, None, None);
        self.busy = None;
        match result {
            Ok(Some(version)) => {
                self.offline = false;
                self.sent = Some(version.n);
                self.marker_names(&version);
            }
            Ok(None) => {}
            Err(SyncError::Offline) => self.unsent_offline(),
            Err(SyncError::MovedOn) => {
                log!("another PC sent newer progress meanwhile: both are kept and the player picks");
                self.split = self.newest();
            }
            // Nothing worth sending: no retry until the game writes a save again.
            Err(SyncError::Damaged(why)) => log!("not sent: {why}"),
            Err(e) => {
                log!("send failed: {e}");
                self.engine.state.send_pending = true;
                let _ = self.engine.save_state();
                self.problem = Some(format!("Sending failed: {e}. Your progress is safe on this PC."));
            }
        }
        // The cards show what was just sent, not what the NAS held before.
        self.refresh();
    }

    /// The marker names the version just sent as its checkpoint: a Satchel of any version reading
    /// it later (this PC closed before the session's end, say) finds it among the versions, and
    /// never takes an older checkpoint over it.
    fn marker_names(&mut self, version: &satchel_core::index::Version) {
        let Some(marker) = self.marker.as_mut() else { return };
        marker.checkpoint = Some(version.sha256.clone());
        marker.checkpointed = Some(version.saved);
        if let Err(e) = self.engine.write_marker(marker) {
            log!("in-use marker not updated after the send: {e}");
        }
        self.engine.state.last_checkpoint = Some(version.sha256.clone());
        if let Err(e) = self.engine.save_state() {
            log!("state not saved: {e}");
        }
    }

    /// This PC's own marker left by an earlier run (closed, updated or shut down while a session
    /// was still closing), dealt with after the first outlook that can: game closed, no session.
    fn leftover(&mut self, outlook: &Outlook) {
        if self.leftover_done || self.tracker.in_session() || self.game_dir().is_none_or(|d| self.scanner.game_running(&d)) {
            return;
        }
        match self.engine.clear_own_leftover(outlook, Utc::now()) {
            Ok(done) => self.leftover_done = done,
            Err(e) => log!("this PC's in-use marker from an earlier run not dealt with: {e}"),
        }
    }

    fn unsent_offline(&mut self) {
        self.offline = true;
        self.engine.state.send_pending = true;
        let _ = self.engine.save_state();
        log!("NAS unreachable: the save waits on this PC");
        if !self.offline_told {
            self.offline_told = true;
            self.notify("Network folder unreachable", "Your progress is safe on this PC and goes up when the network folder is back.");
        }
    }

    fn newest(&self) -> Option<u64> {
        self.engine.outlook(Utc::now()).ok().and_then(|o| o.index.newest_version().map(|v| v.n))
    }

    fn session_over(&mut self, now: DateTime<Utc>) {
        let start = self.session_start.take().unwrap_or(now);
        let end = self.closed_at.take().unwrap_or(now);
        let session = Session::new(self.engine.pc(), start, end, self.tracker.updated, None);
        log!(
            "session over: {}, {} writes to the save while playing{}",
            format_minutes(session.minutes),
            self.writes,
            if session.updated { ", the game updated itself" } else { "" }
        );
        if let Err(e) = self.engine.record_session(&session) {
            log!("playtime not recorded: {e}");
        }
        let mut line = format!("session {}", format_minutes(session.minutes));
        if session.updated {
            line.push_str(" · the game updated itself, still one session");
        }
        if self.engine.shared().is_some() {
            match self.engine.clear_marker() {
                Ok(()) => {}
                Err(SyncError::Offline) => self.offline = true,
                Err(e) => log!("in-use marker not removed: {e}"),
            }
            match self.sent.take() {
                Some(n) => {
                    self.notify(
                        "Progress sent to the network folder",
                        &format!("Version {n}, from {} · session {}.", self.engine.pc(), format_minutes(session.minutes)),
                    );
                    line = format!("Sent version {n} · {line}");
                }
                None => line = format!("Played · {line}"),
            }
        } else {
            match self.engine.keep_copy(now) {
                Ok(Some(file)) => log!("copy kept: {file}"),
                Ok(None) => {}
                Err(e) => log!("copy not kept: {e}"),
            }
            line = format!("Played · {line}");
        }
        self.activity(if line.starts_with("Sent") { Kind::Sync } else { Kind::Game }, &line);
        self.marker = None;
        self.launched = false;
        if self.split.is_some() {
            self.needs_you_open = true;
            self.ui.show(Some(0));
        }
        self.refresh();
        self.addons_after_session();
        if std::mem::take(&mut self.update_after_session) {
            self.update_now();
        }
    }

    // Play, sync, choices ---------------------------------------------------------------------

    fn play(&mut self) {
        if self.tracker.game_running() {
            self.open_card();
            self.card_line("The game is already running", false, true);
            self.close_card_in(6);
            return;
        }
        let Some(dir) = self.game_dir() else {
            self.problem = Some("The game wasn't found. Set its folder in Settings.".into());
            self.ui.show(Some(TAB_SETTINGS));
            return;
        };
        self.open_card();
        if self.engine.shared().is_some() {
            self.busy = Some("Checking the network folder…".into());
            self.push();
            let go_on = self.check_before_play(Utc::now());
            self.busy = None;
            if !go_on {
                return;
            }
        }
        self.pending_play = false;
        // What the other PCs changed, then the addons' updates, go in while the game is still closed.
        self.share_before_play();
        self.addons_before_play();
        match win::launch_game(&dir) {
            Ok(()) => {
                log!("game launched");
                self.launched = true;
                self.problem = None;
                self.card_line("Game started", true, false);
                self.close_card_in(8);
            }
            Err(e) => {
                log!("the game didn't start: {e}");
                self.card_line(&format!("The game didn't start: {e}"), false, true);
                self.close_card_in(20);
            }
        }
    }

    /// The NAS side of Play. `true`: go on and start the game.
    fn check_before_play(&mut self, now: DateTime<Utc>) -> bool {
        let mut outlook = match self.engine.outlook(now) {
            Ok(outlook) => outlook,
            Err(SyncError::Offline) => {
                self.offline = true;
                self.card_line("The network folder doesn't answer: you play with this PC's progress, sent when it's back", false, true);
                return true;
            }
            Err(SyncError::Io(e)) if gate::newer_satchel(&e).is_some() => {
                let newer = gate::newer_satchel(&e).map(|n| n.newer.clone()).unwrap_or_default();
                self.offline = false;
                self.gated = Some(newer.clone());
                if !std::mem::take(&mut self.play_anyway) {
                    let update = match self.offer() {
                        Some(offer) => format!(" Update now puts {} here and plays.", offer.version),
                        None if !about::REPOSITORY.is_empty() => " Update now shows here once the new Satchel has downloaded.".into(),
                        None => " Copy the new SurvivorsSatchel.exe to this PC to update it.".into(),
                    };
                    self.card_ask(format!(
                        "Another PC uses Survivor's Satchel {newer} and this one is {}, so nothing syncs until it's updated. Playing now plays this PC's progress, synced after the update.{update}",
                        about::VERSION
                    ));
                    self.card_gate = true;
                    self.pending_play = true;
                    return false;
                }
                self.card_line(&format!("Not synced: this Satchel ({}) is older than {newer}", about::VERSION), false, true);
                return true;
            }
            Err(e) => {
                log!("check before play failed: {e}");
                self.card_line(&format!("The network folder couldn't be checked ({e}): you play with this PC's progress"), false, true);
                return true;
            }
        };
        self.offline = false;
        let stale: Vec<Other> = outlook.others.iter().filter(|o| o.stale).cloned().collect();
        if !stale.is_empty() {
            for other in &stale {
                let stopped = format!("{} stopped at {} during a session", other.marker.pc, clock(other.marker.heartbeat));
                match self.engine.take_over_checkpoint(other, now) {
                    Ok(Some(v)) => self.card_line(&format!("{stopped}; its last checkpoint is now version {}", v.n), true, false),
                    Ok(None) => self.card_line(&format!("{stopped}; nothing newer was left"), true, false),
                    Err(e) => log!("checkpoint of {} not taken over: {e}", other.marker.pc),
                }
            }
            match self.engine.outlook(now) {
                Ok(fresh) => outlook = fresh,
                Err(e) => {
                    log!("second check failed: {e}");
                    return true;
                }
            }
        }
        let anyway = std::mem::take(&mut self.play_anyway);
        if !anyway && let Some(other) = outlook.others.iter().find(|o| !o.stale) {
            self.card_ask(format!(
                "The game looks open on {}: started {}, still active {}. If you play here too, the two will split and you'll pick one afterwards.",
                other.marker.pc,
                clock(other.marker.started),
                ago(other.marker.heartbeat, now)
            ));
            self.pending_play = true;
            self.outlook = Some(outlook);
            return false;
        }
        // An empty or damaged save here is never sent, and never replaced without the player.
        if let Some(why) = outlook.local.as_ref().and_then(|l| l.problem.clone()) {
            self.outlook = Some(outlook);
            if !anyway {
                self.card_ask(format!(
                    "This PC's save {why}, so it isn't synced. To carry on from the network folder, cancel and bring back its newest version in the Saves tab."
                ));
                self.pending_play = true;
                return false;
            }
            self.card_line("Playing with this PC's save as it is; it isn't synced", false, true);
            return true;
        }
        let decision = outlook.decision.clone();
        let index = outlook.index.clone();
        self.outlook = Some(outlook);
        match decision {
            Decision::InSync { adopt } => {
                if let Some(version) = adopt.and_then(|n| index.get(n)).cloned() {
                    let _ = self.engine.adopt(&version, now);
                }
                self.card_line("Network folder checked: you have the newest progress", true, false);
            }
            Decision::Send => match self.engine.send(now, None, None, None) {
                Ok(Some(v)) => {
                    self.card_line(&format!("Network folder checked; this PC's progress went up first (version {})", v.n), true, false);
                    self.activity(Kind::Sync, &format!("Sent version {}", v.n));
                }
                Ok(None) => self.card_line("Network folder checked", true, false),
                Err(SyncError::MovedOn) => {
                    let newest = self.newest();
                    return self.split_found(newest);
                }
                Err(e) => {
                    log!("send before play failed: {e}");
                    self.card_line(&format!("Sending first failed ({e}); you play with this PC's progress"), false, true);
                }
            },
            Decision::BringIn(n) => {
                self.card_line("Network folder checked", true, false);
                return self.bring_in(n, now);
            }
            Decision::Split { nas } => return self.split_found(Some(nas)),
        }
        true
    }

    fn split_found(&mut self, nas: Option<u64>) -> bool {
        self.split = nas;
        self.needs_you_open = true;
        self.pending_play = true;
        self.card_line("Two PCs moved on separately: pick one in the window", false, true);
        self.close_card_in(10);
        self.ui.show(Some(0));
        false
    }

    /// Brings version `n` in. `true`: go on.
    fn bring_in(&mut self, n: u64, now: DateTime<Utc>) -> bool {
        self.busy = Some(format!("Bringing in version {n}…"));
        self.push();
        let result = self.engine.bring_in(n, now);
        self.busy = None;
        match result {
            Ok(BroughtIn { version, safety }) => {
                self.card_line(&format!("Progress from {} brought in (saved {})", version.pc, when(version.saved)), true, false);
                self.activity(Kind::Sync, &format!("Brought in version {} from {}", version.n, version.pc));
                let kept = if safety.is_some() { " Yours is kept as a safety copy." } else { "" };
                self.notify(&format!("Progress from {} brought in", version.pc), &format!("Saved {}.{kept}", when(version.saved)));
                true
            }
            Err(SyncError::MovedOn) => self.split_found(Some(n)),
            Err(e) => {
                log!("bring-in failed: {e}");
                self.card_line(&format!("Bringing in failed ({e}); you play with this PC's progress"), false, true);
                true
            }
        }
    }

    fn sync_now(&mut self) {
        if self.engine.shared().is_none() {
            return;
        }
        let now = Utc::now();
        self.busy = Some("Checking the network folder…".into());
        self.push();
        // The addons too: what changed here goes up, what changed elsewhere shows.
        self.share_look();
        let mut checked = self.engine.outlook(now);
        if let Ok(outlook) = &checked {
            let stale: Vec<Other> = outlook.others.iter().filter(|o| o.stale).cloned().collect();
            for other in &stale {
                if let Ok(Some(v)) = self.engine.take_over_checkpoint(other, now) {
                    self.activity(Kind::Sync, &format!("{} stopped during a session; its checkpoint became version {}", other.marker.pc, v.n));
                }
            }
            if !stale.is_empty() {
                checked = self.engine.outlook(now);
            }
        }
        self.busy = None;
        match checked {
            Err(SyncError::Offline) => self.offline = true,
            Err(e) => self.problem = Some(format!("The network folder can't be read: {e}")),
            Ok(outlook) => {
                self.offline = false;
                match outlook.decision {
                    Decision::InSync { adopt } => {
                        if let Some(version) = adopt.and_then(|n| outlook.index.get(n)).cloned() {
                            let _ = self.engine.adopt(&version, now);
                        }
                        let mut text = match self.share_status() {
                            Some(addons) => format!("Synced: nothing to do. {addons}."),
                            None => "Synced: nothing to do.".into(),
                        };
                        if let Some(game) = self.game_settings_status() {
                            text.push_str(&format!(" {game}."));
                        }
                        self.say(text);
                    }
                    Decision::Send => {
                        let _hold = win::Hold::start();
                        match self.engine.send(now, None, None, None) {
                            Ok(Some(v)) => self.activity(Kind::Sync, &format!("Sent version {}", v.n)),
                            Ok(None) => {}
                            Err(SyncError::MovedOn) => self.split = self.newest(),
                            Err(e) => self.problem = Some(format!("Sending failed: {e}")),
                        }
                    }
                    Decision::BringIn(n) => {
                        if self.tracker.game_running() {
                            self.say(format!("Close the game to bring in version {n}."));
                        } else {
                            self.open_card();
                            self.bring_in(n, now);
                            self.close_card_in(8);
                        }
                    }
                    Decision::Split { nas } => {
                        self.split = Some(nas);
                        self.needs_you_open = true;
                        self.ui.show(Some(0));
                    }
                }
            }
        }
        self.refresh();
    }

    /// The player's pick after two PCs moved on separately. Both stay as versions. The pick
    /// applies to the two saves on screen; if either changed meanwhile, the question comes again.
    fn resolve(&mut self, keep_here: bool) {
        let Some(shown) = self.shown.clone() else {
            self.needs_you_open = false;
            return;
        };
        if self.tracker.game_running() {
            self.say("Close the game first, then pick.".into());
            return;
        }
        let now = Utc::now();
        self.busy = Some("Saving your choice…".into());
        self.push();
        let result = if keep_here {
            let note = Some("kept after two PCs moved on separately".to_string());
            self.engine
                .send(now, None, note, Some(&shown))
                .map(|v| v.map(|v| format!("Kept this PC's progress as version {}", v.n)))
        } else {
            self.engine
                .use_nas(&shown, now)
                .map(|brought| Some(format!("Continued with version {} ({})", brought.version.n, brought.version.pc)))
        };
        self.busy = None;
        match result {
            Ok(text) => {
                self.split = None;
                self.needs_you_open = false;
                self.problem = None;
                if let Some(text) = text {
                    self.activity(Kind::Sync, &text);
                }
                if self.pending_play {
                    self.pending_play = false;
                    self.play();
                }
            }
            Err(SyncError::Offline) => {
                self.offline = true;
                self.say("The network folder doesn't answer; pick again when it's back.".into());
            }
            Err(SyncError::Changed) => {
                log!("the choice was made on a view that had changed: asked again");
                self.say("Something changed since you looked (another PC sent, or this PC's save changed). Check the two again and pick.".into());
            }
            Err(e) => {
                log!("choice not saved: {e}");
                self.problem = Some(format!("Your choice couldn't be saved: {e}"));
            }
        }
        self.refresh();
    }

    fn retry(&mut self) {
        self.last_retry = Some(Instant::now());
        let now = Utc::now();
        let outlook = match self.engine.outlook(now) {
            Ok(outlook) => outlook,
            Err(SyncError::Offline) => {
                self.offline = true;
                return;
            }
            Err(e) => {
                log!("retry failed: {e}");
                return;
            }
        };
        if self.offline {
            log!("the NAS answers again");
        }
        self.offline = false;
        self.offline_told = false;
        if let Err(e) = self.engine.flush_playtime() {
            log!("waiting playtime not sent: {e}");
        }
        if self.engine.state.send_pending {
            match outlook.decision {
                Decision::Send => {
                    let _hold = win::Hold::start();
                    match self.engine.send(now, None, None, None) {
                        Ok(Some(v)) => {
                            self.activity(Kind::Sync, &format!("Sent version {} (it waited for the network folder)", v.n));
                            self.notify("Progress sent to the network folder", &format!("Version {}, which waited for the network folder.", v.n));
                        }
                        Ok(None) => {}
                        Err(SyncError::MovedOn) => self.split = self.newest(),
                        Err(e) => log!("waiting send failed: {e}"),
                    }
                }
                Decision::Split { nas } => {
                    log!("while this PC waited, another PC moved on: the player picks");
                    self.split = Some(nas);
                    self.needs_you_open = true;
                    self.ui.show(Some(0));
                }
                Decision::InSync { adopt } => {
                    if let Some(version) = adopt.and_then(|n| outlook.index.get(n)).cloned() {
                        let _ = self.engine.adopt(&version, now);
                    }
                    self.engine.state.send_pending = false;
                    let _ = self.engine.save_state();
                }
                Decision::BringIn(_) => {
                    self.engine.state.send_pending = false;
                    let _ = self.engine.save_state();
                }
            }
        }
        self.refresh();
    }

    fn bring_back(&mut self, index: usize) {
        if self.tracker.game_running() {
            self.say("Close the game first: a save is only brought in while the game is closed.".into());
            return;
        }
        let Some(row) = self.lists.rows.get(index).cloned() else { return };
        let now = Utc::now();
        self.busy = Some("Bringing it back…".into());
        self.push();
        let result = match row {
            Row::Version(n) => self
                .engine
                .version_bytes(n)
                .and_then(|(v, bytes)| self.engine.restore(bytes, format!("version {n} brought back"), v.game, now))
                .map(|b| format!("Brought back version {n} (now version {})", b.version.n)),
            Row::File(path) if self.engine.shared().is_some() => {
                let name = file_name(&path);
                fs::read(&path)
                    .map_err(SyncError::from)
                    .and_then(|bytes| self.engine.restore(bytes, format!("{name} brought back"), None, now))
                    .map(|b| format!("Brought back {name} (now version {})", b.version.n))
            }
            Row::File(path) => {
                let name = file_name(&path);
                self.engine.restore_copy(&path, now).map(|_| format!("Brought back {name}"))
            }
        };
        self.busy = None;
        match result {
            Ok(text) => {
                self.activity(Kind::Sync, &text);
                self.say(format!("{text}. Your save from before is kept as a safety copy."));
            }
            Err(SyncError::Offline) => self.offline = true,
            Err(e) => self.problem = Some(format!("It couldn't be brought back: {e}")),
        }
        self.refresh();
    }

    fn show_in_folder(&self, index: usize) {
        match self.lists.rows.get(index) {
            Some(Row::File(path)) => win::show_in_folder(path),
            Some(Row::Version(n)) => {
                let file = self.outlook.as_ref().and_then(|o| o.index.get(*n)).map(|v| v.file.clone());
                if let (Some(shared), Some(file)) = (self.engine.shared(), file) {
                    win::show_in_folder(&shared.version(&file));
                }
            }
            None => {}
        }
    }

    // Settings and first start ----------------------------------------------------------------

    fn apply_windows_settings(&self) {
        let settings = &self.engine.settings;
        if let Err(e) = win::set_start_with_windows(settings.start_with_windows) {
            log!("start with Windows not set: {e}");
        }
        let icon = self.game_dir().map(|d| d.join(win::GAME_EXE));
        if let Err(e) = win::set_play_shortcut(settings.desktop_shortcut, icon.as_deref()) {
            log!("desktop shortcut not set: {e}");
        }
    }

    /// Saves the form. A shared folder the Satchel can't safely use is refused, and so is a name
    /// another PC syncing there already has (two PCs with one name share a marker, a checkpoint
    /// and a playtime file): nothing is saved, the form stays as typed with the reason under the
    /// field, and the answer is `false`.
    fn save_settings(&mut self, form: Form) -> bool {
        let mut settings = self.engine.settings.clone();
        settings.game_dir = non_empty(&form.game_dir).map(PathBuf::from);
        settings.sync.enabled = form.sync;
        settings.sync.folder = non_empty(&form.folder).map(|f| win::network_path(Path::new(f)));
        settings.pc_name = non_empty(&form.pc_name).map(str::to_string).unwrap_or_else(win::computer_name);
        settings.start_with_windows = form.start_with_windows;
        settings.desktop_shortcut = form.desktop_shortcut;
        settings.notifications = form.notifications;
        settings.addons.update_by_themselves = form.addons_auto;
        settings.addons.shared = form.addons_shared;
        settings.pause_when_away = form.pause_when_away;
        settings.theme = match form.theme {
            1 => Theme::Light,
            2 => Theme::Dark,
            _ => Theme::System,
        };
        settings.game.shared = form.game_shared;
        if let Some(folder) = settings.sync_folder()
            && let Some(why) = paths::sync_folder_problem(folder, &self.engine.local, settings.game_dir.as_deref())
        {
            log!("settings not saved: {why}");
            let mut view = self.form_view();
            view.form = form;
            view.folder_status = format!("✗ Can't be used: {why}");
            self.ui.push_form(view);
            return false;
        }
        if settings.sync_folder().is_some()
            && let Some(taken) = self.name_taken(&settings.pc_name)
        {
            log!("settings not saved: another PC syncing here is already called {taken}");
            let mut view = self.form_view();
            view.form = form;
            view.name_status = format!("✗ Another PC syncing here is already called {taken}; give this one a name of its own");
            self.ui.push_form(view);
            return false;
        }
        if let Err(e) = settings.save(&self.engine.local.settings()) {
            log!("settings not saved: {e}");
            self.problem = Some(format!("Settings couldn't be saved: {e}"));
            return true;
        }
        log!("settings saved (sync {}, folder {:?}, PC {:?})", settings.sync.enabled, settings.sync.folder, settings.pc_name);
        // Every name this PC has had, the one it had before included: typed again, it's never
        // taken for another PC's.
        let mut named = false;
        for name in [self.engine.settings.pc_name.clone(), settings.pc_name.clone()] {
            if !self.engine.state.pc_names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                self.engine.state.pc_names.push(name);
                named = true;
            }
        }
        if named && let Err(e) = self.engine.save_state() {
            log!("state not saved: {e}");
        }
        if settings.sync != self.engine.settings.sync {
            self.outlook = None;
            self.split = None;
            self.offline = false;
        }
        self.engine.settings = settings;
        self.problem = None;
        self.apply_windows_settings();
        true
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
        let index = self.outlook.iter().flat_map(|o| &o.index.pcs);
        let list = self.share.list.iter().flat_map(|l| &l.addons).flat_map(|e| &e.pcs);
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
        } else if self.offline {
            "✗ Not answering right now".into()
        } else if let Some(warning) = self.link_warning() {
            warning
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
            pause_when_away: settings.pause_when_away,
            theme: theme_index(settings.theme),
            game_shared: settings.game.shared,
        };
        FormView {
            saved: form.clone(),
            form,
            game_status: game_status(settings.game_dir.as_deref()),
            folder_status,
            name_status: String::new(),
            settings_path: self.engine.local.settings().display().to_string(),
        }
    }

    fn push_form(&self) {
        self.ui.push_form(self.form_view());
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

    /// First start: the fields come from the defaults file and detection. With sync set up
    /// already, it also asks which progress to start from when both sides have one.
    fn enter_setup(&mut self) {
        let mut setup = Setup::default();
        self.setup_shown = None;
        if self.engine.shared().is_some()
            && let (Ok(Some(local)), Ok(outlook)) = (self.engine.local_save(), self.engine.outlook(Utc::now()))
        {
            let other = match outlook.decision {
                Decision::Split { nas } => {
                    self.setup_shown = Some(Shown { local: Some(local.sha256.clone()), nas });
                    outlook.index.get(nas).map(|v| {
                        (format!("Version {} from {} · saved {} · {}", v.n, v.pc, when(v.saved), size(v.bytes)), Some(v.saved))
                    })
                }
                Decision::Send if outlook.index.versions.is_empty() => self
                    .engine
                    .exports()
                    .ok()
                    .and_then(|exports| exports.into_iter().next())
                    .filter(|e| fs::read(&e.path).is_ok_and(|bytes| fingerprint(&bytes) != local.sha256))
                    .map(|e| (format!("{} · saved {} · {}", e.name, when_opt(e.modified), size(e.bytes)), e.modified)),
                _ => None,
            };
            if let Some((other_text, other_time)) = other {
                let newer_here = local.modified >= other_time;
                let here_text = format!("Last played {} · {}", when_opt(local.modified), size(local.bytes));
                let mark = |text: String, newer: bool| if newer { format!("{text} · newer") } else { text };
                setup.pick = Some([mark(here_text, newer_here), mark(other_text, !newer_here)]);
                setup.choice = if newer_here { 0 } else { 1 };
            }
        }
        self.setup = Some(setup);
        self.push_form();
        self.ui.show(None);
    }

    fn finish_setup(&mut self, form: Form, choice: i32) {
        let picked = self.setup.as_ref().is_some_and(|s| s.pick.is_some());
        // The first sync may replace this PC's save: only with the game closed.
        if let Some(dir) = non_empty(&form.game_dir).map(PathBuf::from)
            && self.scanner.game_running(&dir)
        {
            let mut view = self.form_view();
            view.form = form;
            view.game_status = "✗ The game is running: close it, then press Done".into();
            self.ui.push_form(view);
            return;
        }
        if !self.save_settings(form) || self.problem.is_some() {
            return;
        }
        self.engine.state.setup_done = true;
        // A new install has nothing to tell about earlier versions.
        self.engine.state.seen_version = Some(about::VERSION.into());
        let _ = self.engine.save_state();
        let now = Utc::now();
        if self.engine.shared().is_some() {
            match self.engine.outlook(now) {
                // The pick applies to the two saves the panel showed; anything else is asked again.
                Ok(outlook) => match (outlook.decision, self.setup_shown.take().filter(|_| picked)) {
                    (Decision::Split { .. }, Some(shown)) => {
                        let result = if choice == 1 {
                            self.engine.use_nas(&shown, now).map(|b| b.version.n)
                        } else {
                            self.engine
                                .send(now, None, Some("kept at first start".into()), Some(&shown))
                                .map(|v| v.map_or(shown.nas, |v| v.n))
                        };
                        match result {
                            Ok(n) => self.activity(Kind::Sync, &format!("Started from version {n}")),
                            Err(SyncError::Changed) => {
                                self.split = self.newest();
                                self.needs_you_open = true;
                            }
                            Err(e) => self.problem = Some(format!("The first sync failed: {e}")),
                        }
                    }
                    (Decision::Split { nas }, None) => {
                        self.split = Some(nas);
                        self.needs_you_open = true;
                    }
                    (Decision::Send, _) => {
                        let export = if choice == 1 && picked && outlook.index.versions.is_empty() {
                            self.engine.exports().ok().and_then(|e| e.into_iter().next())
                        } else {
                            None
                        };
                        let result = match export {
                            Some(export) => fs::read(&export.path)
                                .map_err(SyncError::from)
                                .and_then(|bytes| self.engine.restore(bytes, format!("{}, chosen at first start", export.name), None, now))
                                .map(|b| Some(b.version.n)),
                            None => self.engine.send(now, None, Some("first start".into()), None).map(|v| v.map(|v| v.n)),
                        };
                        match result {
                            Ok(Some(n)) => self.activity(Kind::Sync, &format!("Started the network folder with version {n}")),
                            Ok(None) => {}
                            Err(e) => self.problem = Some(format!("The first sync failed: {e}")),
                        }
                    }
                    (Decision::BringIn(n), _) => {
                        self.open_card();
                        self.bring_in(n, now);
                        self.close_card_in(8);
                    }
                    (Decision::InSync { adopt }, _) => {
                        if let Some(version) = adopt.and_then(|n| outlook.index.get(n)).cloned() {
                            let _ = self.engine.adopt(&version, now);
                        }
                    }
                },
                Err(SyncError::Offline) => {
                    self.offline = true;
                    self.engine.state.send_pending = true;
                    let _ = self.engine.save_state();
                }
                Err(e) => self.problem = Some(format!("The network folder can't be read: {e}")),
            }
        }
        self.setup = None;
        log!("first start done");
        self.startup();
        self.ui.show(Some(0));
    }

    // What the UI shows ------------------------------------------------------------------------

    /// Reads both sides again for what the window shows. Progress the network folder lacks goes up
    /// here too, with the game closed: a shutdown or a crash during a session leaves the last save
    /// unsent and `send_pending` unset, and it would wait for Play or Sync now while another PC
    /// could start from the older version.
    fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        if let Some(why) = self.engine.folder_problem() {
            // Sync pauses (one-PC copies meanwhile) until another folder is chosen.
            self.outlook = None;
            self.split = None;
            self.problem = Some(format!("Sync is paused: {why}. Choose another network folder in Settings."));
        } else if self.engine.shared().is_some() {
            match self.engine.outlook(Utc::now()) {
                Ok(mut outlook) => {
                    self.offline = false;
                    // A send that failed (`send_pending`) is `retry`'s; this is the one nobody tried.
                    // A checkpoint this PC's save lost (a power cut after the game's last write) becomes
                    // a version first: sent as it is, the older save would count as the newer one and
                    // the checkpoint would go with this PC's marker.
                    if !self.tracker.in_session() && self.game_dir().is_some_and(|d| !self.scanner.game_running(&d)) {
                        match self.engine.promote_own_checkpoint(&outlook, Utc::now()) {
                            Ok(Some(v)) => {
                                log!("this PC's own checkpoint, newer than its save, became version {}", v.n);
                                self.activity(Kind::Sync, &format!("This PC's last checkpoint became version {}", v.n));
                                match self.engine.outlook(Utc::now()) {
                                    Ok(fresh) => outlook = fresh,
                                    Err(e) => log!("the look after the checkpoint failed: {e}"),
                                }
                            }
                            Ok(None) => {}
                            Err(e) => log!("this PC's own checkpoint not looked at: {e}"),
                        }
                    }
                    let unsent = matches!(outlook.decision, Decision::Send) && outlook.local.as_ref().is_some_and(|l| l.problem.is_none());
                    if unsent
                        && !self.engine.state.send_pending
                        && !self.tracker.in_session()
                        && self.busy.is_none()
                        && self.game_dir().is_some_and(|d| !self.scanner.game_running(&d))
                    {
                        let sent = {
                            let _hold = win::Hold::start();
                            self.engine.send(Utc::now(), None, None, None)
                        };
                        // After a send, or another PC's meanwhile, the rest goes on from a fresh look:
                        // the view shows what is in the folder now (a split, too), and the leftover
                        // marker is dealt with knowing the save is a version.
                        let look_again = match sent {
                            Ok(Some(v)) => {
                                log!("progress the network folder lacked went up as version {} at the look", v.n);
                                self.activity(Kind::Sync, &format!("Sent version {}", v.n));
                                self.notify(
                                    "Progress sent to the network folder",
                                    &format!("Version {}, from {}: it was waiting on this PC.", v.n, self.engine.pc()),
                                );
                                true
                            }
                            Ok(None) => false,
                            Err(SyncError::MovedOn) => {
                                log!("another PC sent newer progress meanwhile: both are kept and the player picks");
                                true
                            }
                            Err(SyncError::Offline) => {
                                self.offline = true;
                                false
                            }
                            Err(e) => {
                                log!("progress the network folder lacked not sent at the look: {e}");
                                // From here it is `retry`'s, every few minutes, not every look.
                                self.engine.state.send_pending = true;
                                let _ = self.engine.save_state();
                                false
                            }
                        };
                        if look_again {
                            match self.engine.outlook(Utc::now()) {
                                Ok(fresh) => outlook = fresh,
                                Err(e) => log!("the look after the send failed: {e}"),
                            }
                        }
                    }
                    self.problem = outlook.local.as_ref().and_then(|l| l.problem.as_ref()).map(|why| {
                        format!("This PC's save {why}, so it isn't sent. To carry on from the network folder, bring back its newest version in the Saves tab.")
                    });
                    self.split = match outlook.decision {
                        Decision::Split { nas } => Some(nas),
                        _ => None,
                    };
                    self.leftover(&outlook);
                    self.outlook = Some(outlook);
                    self.gated = None;
                }
                Err(SyncError::Offline) => self.offline = true,
                Err(SyncError::Io(e)) if gate::newer_satchel(&e).is_some() => {
                    // The NAS answered; a newer Satchel's files keep this one out.
                    let newer = gate::newer_satchel(&e).map(|n| n.newer.clone()).unwrap_or_default();
                    self.offline = false;
                    self.outlook = None;
                    self.problem = Some(format!("Nothing syncs until this Satchel ({}) is updated: another PC uses {newer}.", about::VERSION));
                    self.gated = Some(newer);
                }
                Err(e) => self.problem = Some(format!("The network folder can't be read: {e}")),
            }
            // The exe beside sync\ is looked at only while the NAS answers.
            if !self.offline
                && let Some(shared) = self.engine.shared()
            {
                self.update_offer = self.update_watch.look(&shared.folder, about::VERSION);
            }
        } else {
            self.outlook = None;
            self.gated = None;
            self.update_offer = None;
        }
        self.build_lists();
    }

    fn build_lists(&mut self) {
        let mut lists = Lists::default();
        if let Some(outlook) = &self.outlook {
            let newest = outlook.index.newest_version().map(|v| v.n);
            let here = outlook.local.as_ref().map(|l| l.sha256.as_str());
            for version in &outlook.index.versions {
                let mut tags = Vec::new();
                if Some(version.n) == newest {
                    tags.push("newest".to_string());
                }
                if here == Some(version.sha256.as_str()) {
                    tags.push("this PC".to_string());
                }
                tags.extend(version.note.clone());
                lists.saves.push([version.n.to_string(), when(version.saved), version.pc.clone(), size(version.bytes), tags.join(" · ")]);
                lists.rows.push(Row::Version(version.n));
            }
            // The newest few: older exports pile up in the game's Saves folder.
            if let Ok(exports) = self.engine.exports() {
                for export in exports.into_iter().take(5) {
                    lists.saves.push(["–".into(), when_opt(export.modified), "–".into(), size(export.bytes), "your export".into()]);
                    lists.rows.push(Row::File(export.path));
                }
            }
        } else if self.engine.shared().is_none()
            && let Ok(copies) = self.engine.copies()
        {
            lists.last_copy = copies.first().and_then(|c| c.modified);
            for copy in copies {
                lists.saves.push(["–".into(), when_opt(copy.modified), self.engine.pc().into(), size(copy.bytes), "copy kept after a session".into()]);
                lists.rows.push(Row::File(copy.path));
            }
        }
        let sessions = self.engine.sessions().unwrap_or_default();
        let totals = playtime::totals(&sessions, &Local::now());
        let most = totals.week_per_pc.iter().map(|(_, m)| *m).max().unwrap_or(0).max(1);
        lists.pc_times =
            totals.week_per_pc.iter().map(|(pc, m)| (pc.clone(), format_minutes(*m), *m as f32 / most as f32)).collect();
        lists.today = format_minutes(totals.today);
        lists.week = format_minutes(totals.week);
        lists.all_time = format_minutes(totals.all);
        lists.sessions = sessions
            .iter()
            .take(8)
            .map(|s| {
                let updated = if s.updated { " · game updated, still one session" } else { "" };
                format!("{} {} · {}{updated}", when(s.start), s.pc, format_minutes(s.minutes))
            })
            .collect();
        lists.recent = activity::recent(&self.engine.local.activity(), 6)
            .unwrap_or_default()
            .iter()
            .map(|e| Recent { kind: e.shown_kind(), text: format!("{}  {}", when(e.at), e.text) })
            .collect();
        self.lists = lists;
        self.addons_tick();
    }

    /// What the window and tray show, and what the "Needs you" choice shows (if it is open).
    fn build_view(&self) -> (View, Option<Shown>) {
        let sync_on = self.engine.shared().is_some();
        let (dark, system_dark) = self.theme_dark();
        let (version_line, version_new) = self.version_line();
        let tray = if self.split.is_some() || self.problem.is_some() {
            Tray::NeedsYou
        } else if self.busy.is_some() {
            Tray::Busy
        } else if self.tracker.in_session() {
            Tray::Playing
        } else if sync_on && (self.offline || self.engine.state.send_pending) {
            Tray::Offline
        } else {
            Tray::Synced
        };
        let decision = self.outlook.as_ref().map(|o| &o.decision);
        let newest = self.outlook.as_ref().and_then(|o| o.index.newest_version());
        let (status, banner) = match tray {
            Tray::NeedsYou => match self.problem.as_ref().or(self.notice.as_ref()) {
                Some(text) => ("Needs you".to_string(), text.clone()),
                None => (
                    "Needs you".into(),
                    "Needs you: two PCs moved on separately. Press Play or Sync now to pick one; both are kept.".into(),
                ),
            },
            Tray::Busy => {
                let busy = self.busy.clone().unwrap_or_default();
                (busy.clone(), busy)
            }
            Tray::Playing if !self.tracker.game_running() => {
                // The quiet window: the game is closed, the session not yet over.
                let sent = match (sync_on, self.sent) {
                    (true, Some(n)) => format!("your progress went up as version {n}"),
                    (true, None) => "nothing new to send".into(),
                    (false, _) => "a copy is kept when the session closes".into(),
                };
                (
                    "Game closed".into(),
                    self.notice.clone().unwrap_or_else(|| {
                        format!("Game closed · {sent}. The session closes once the game stays closed for a moment (longer after an update).")
                    }),
                )
            }
            Tray::Playing => (
                "Game running".into(),
                self.notice.clone().unwrap_or_else(|| {
                    if sync_on {
                        "Game running · each time the game saves, a checkpoint goes to the network folder".into()
                    } else {
                        "Game running".into()
                    }
                }),
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
                let banner = if let Some(notice) = &self.notice {
                    notice.clone()
                } else if self.game_dir().is_none() {
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
        let base = self.engine.state.last_synced.as_ref();
        let pc = self.engine.pc().to_string();
        let (here, other) = if sync_on {
            let here_main = match (&local, base) {
                (Some(l), Some(b)) if l.sha256 == b.sha256 => format!("Version {}", b.n),
                (Some(_), Some(b)) => format!("Changed since version {}", b.n),
                (Some(_), None) => "Not synced yet".into(),
                (None, _) => "No progress yet".into(),
            };
            let here = [format!("This PC · {pc}"), here_main, format!("last played {}", when_opt(local.as_ref().and_then(|l| l.modified)))];
            let other = if self.offline {
                ["Network folder · newest".to_string(), "Not answering".into(), "retried every few minutes".into()]
            } else {
                match newest {
                    Some(v) => ["Network folder · newest".to_string(), format!("Version {}", v.n), format!("sent {}, from {}", when(v.saved), v.pc)],
                    None => ["Network folder · newest".to_string(), "Nothing yet".into(), "the first session sends version 1".into()],
                }
            };
            (here, other)
        } else {
            (
                [format!("This PC · {pc}"), when_opt(local.as_ref().and_then(|l| l.modified)), "last played".into()],
                ["Last copy kept".to_string(), when_opt(self.lists.last_copy), "a copy is kept after each session".into()],
            )
        };

        let mut shown = None;
        let needs_you = match (self.needs_you_open, self.split) {
            (true, Some(n)) => {
                shown = Some(Shown { local: local.as_ref().map(|l| l.sha256.clone()), nas: n });
                let version = self.outlook.as_ref().and_then(|o| o.index.get(n));
                let here = local.as_ref().map(|l| format!("{} · {}", when_opt(l.modified), size(l.bytes))).unwrap_or_default();
                let there = version.map(|v| format!("{} · {}", when(v.saved), size(v.bytes))).unwrap_or_default();
                let from = version.map(|v| v.pc.clone()).unwrap_or_else(|| "the other PC".into());
                Some([here, there, from])
            }
            _ => None,
        };

        let saves_note = if sync_on {
            "Every version in the network folder, newest first, plus your own exports. Bringing one in makes it the newest; your save from before is kept as a safety copy.".into()
        } else {
            "The copies kept on this PC after each session, newest first.".into()
        };

        let view = View {
            tray,
            status,
            banner,
            busy: self.busy.is_some(),
            sync_on,
            here,
            other,
            recent: self.lists.recent.clone(),
            saves: self.lists.saves.clone(),
            saves_note,
            today: self.lists.today.clone(),
            week: self.lists.week.clone(),
            all_time: self.lists.all_time.clone(),
            pc_times: self.lists.pc_times.clone(),
            sessions: self.lists.sessions.clone(),
            needs_you,
            card: self.card.clone(),
            setup: self.setup.clone(),
            addons: self.addons_view(),
            dark,
            system_dark,
            app_released: released_text(),
            version_line,
            version_new,
            game_found: self.game_dir().is_some(),
            game_settings: self.game_settings_view(),
            update: self.update_line(),
            check_result: self.check_result.clone().unwrap_or_default(),
            checking: self.check_pending,
            checks: self.checks,
            card_update: if self.card_gate { self.offer().map(|o| o.version.clone()).unwrap_or_default() } else { String::new() },
        };
        (view, shown)
    }

    fn push(&mut self) {
        let (view, shown) = self.build_view();
        self.shown = shown;
        if self.last_view.as_ref() != Some(&view) {
            self.last_view = Some(view.clone());
            self.ui.push(view);
        }
    }

    // Small helpers -----------------------------------------------------------------------------

    fn notify(&self, title: &str, text: &str) {
        if self.engine.settings.notifications {
            win::notify(title, text);
        }
    }

    fn activity(&self, kind: Kind, text: &str) {
        if let Err(e) = activity::add(&self.engine.local.activity(), Utc::now(), kind, text) {
            log!("activity not recorded: {e}");
        }
    }

    /// A one-off message for the banner; it goes away after a minute.
    fn say(&mut self, text: String) {
        self.notice = Some(text);
        self.notice_at = Some(Instant::now());
    }

    /// Whether the window is dark, and whether Windows' apps are (the title bar is set again when
    /// either changes: Windows resets it when its own mode changes).
    fn theme_dark(&self) -> (bool, bool) {
        let system = win::apps_use_dark();
        let dark = match self.engine.settings.theme {
            Theme::System => system,
            Theme::Light => false,
            Theme::Dark => true,
        };
        (dark, system)
    }

    /// The Play tab's last line: the version and how long ago it was released, or, after an
    /// update, "Updated to …" until its What's new is opened. `true`: the update line.
    fn version_line(&self) -> (String, bool) {
        let state = &self.engine.state;
        // A PC that ran an earlier Satchel (its first start done) and hasn't opened this one's
        // What's new; a new install sets the version at its first start.
        if state.setup_done && state.seen_version.as_deref() != Some(about::VERSION) {
            return (format!("Updated to {} · see what's new", about::VERSION), true);
        }
        let released = released_text();
        let released = if released.is_empty() { released } else { format!(" · {released}") };
        (format!("Survivor's Satchel {}{released}", about::VERSION), false)
    }

    fn whats_new_seen(&mut self) {
        if self.engine.state.seen_version.as_deref() != Some(about::VERSION) {
            self.engine.state.seen_version = Some(about::VERSION.into());
            if let Err(e) = self.engine.save_state() {
                log!("state not saved: {e}");
            }
        }
    }

    /// The no-game panel's Find it: a folder found is kept at once (Settings shows it).
    fn find_game(&mut self, form: Form) {
        let Some(dir) = win::detect_game_dir() else {
            self.say("The game wasn't found in the usual folders. If it's installed, choose its folder in Settings.".into());
            return;
        };
        let mut settings = self.engine.settings.clone();
        settings.game_dir = Some(dir.clone());
        // The same check as Save's: the game's folder and the shared one must stay apart.
        if let Some(folder) = settings.sync_folder()
            && let Some(why) = paths::sync_folder_problem(folder, &self.engine.local, Some(&dir))
        {
            log!("found game not used: {why}");
            self.say(format!("The game was found in {}, but it can't be used: {why}. See Settings.", dir.display()));
            return;
        }
        if let Err(e) = settings.save(&self.engine.local.settings()) {
            self.problem = Some(format!("Settings couldn't be saved: {e}"));
            return;
        }
        log!("game found at {}", dir.display());
        self.engine.settings = settings;
        self.apply_windows_settings();
        self.say(format!("The game was found in {}", dir.display()));
        // Settings keeps what was typed there and not saved yet; only the game's folder changes.
        let mut view = self.form_view();
        let saved = view.form.game_dir.clone();
        view.form = Form { game_dir: saved, ..form };
        view.game_status = game_status(Some(&dir));
        self.ui.push_form(view);
        self.refresh();
        self.addons_scan();
        self.share_look();
    }

    /// Update now: the Satchel beside `sync\` takes this one's place and starts (playing, when
    /// asked from Play), and this one closes.
    fn update_now(&mut self) {
        if self.game_dir().is_some_and(|d| self.scanner.game_running(&d)) {
            self.say("Close the game first: the Satchel updates while the game is closed.".into());
            return;
        }
        // The session closes up to a few minutes after the game (playtime, this PC's in-use
        // marker): the update waits for that.
        if self.tracker.in_session() {
            self.update_after_session = true;
            self.say("The update goes in as soon as this session has closed, within a few minutes.".into());
            return;
        }
        let Some(offer) = self.offer().cloned() else {
            self.say("There's no newer Satchel to update from.".into());
            return;
        };
        self.check_result = None;
        self.busy = Some(format!("Updating to {}…", offer.version));
        self.push();
        match update::install(&offer, self.pending_play) {
            Ok(()) => {
                log!("updating to {}: the new Satchel starts, this one closes", offer.version);
                self.quit = true;
            }
            Err(e) => {
                self.busy = None;
                log!("the update to {} failed: {e}", offer.version);
                let by_hand = match about::repository_url() {
                    Some(url) if offer.from_github => format!("downloaded from {url}/releases"),
                    _ => "copied from beside sync\\ by hand".into(),
                };
                self.say(format!("The Satchel didn't update ({e}). It goes on as it was; the new one can also be {by_hand}."));
            }
        }
    }

    /// About's Check for updates: the exe beside `sync\` is read again at once (its size and time
    /// as last seen are forgotten) and, in a build that knows its repository, GitHub is asked now
    /// instead of at the next look, whose answer finishes the check (`satchel_answer`). What was
    /// found shows under the button and in the banner.
    fn check_updates(&mut self) {
        // Counted first: whatever happens below, the next view ends the window's own "Checking…"
        // (a check that asks GitHub goes on showing it until the answer).
        self.checks += 1;
        self.update_watch = update::Watch::default();
        let shared = self.engine.shared();
        let sync_on = shared.is_some();
        let answers = shared.as_ref().is_some_and(|s| s.reachable_within(REACH_WAIT));
        if let Some(shared) = shared.filter(|_| answers) {
            // It answered just now: an `offline` left by an earlier look must not hide the exe.
            self.offline = false;
            self.update_offer = self.update_watch.look(&shared.folder, about::VERSION);
        }
        // Pending only once the question is on its way to the network thread: a hand-off that
        // failed would otherwise keep "Checking…" up until the Satchel restarts.
        let asks_github = !about::REPOSITORY.is_empty();
        if asks_github {
            self.satchel_looked = None;
            self.check_pending = self.satchel_look();
        }
        let text = if self.check_pending {
            "Checking GitHub…".to_string()
        } else if let Some(offer) = self.offer() {
            format!("Checked: {} is ready {}", offer.version, offer.place())
        } else if asks_github {
            "Checked: GitHub couldn't be asked".into()
        } else if !sync_on {
            "Checked: nothing to compare with — sync is off, and this build doesn't look on GitHub".into()
        } else if !answers {
            "Checked: the network folder doesn't answer".into()
        } else {
            format!("Checked: you have the newest version ({})", about::VERSION)
        };
        self.check_result = Some(text.clone());
        self.say(text);
    }

    /// The newer Satchel on offer: beside `sync\` (while the NAS answers) or from GitHub.
    fn offer(&self) -> Option<&update::Offer> {
        update::newer(self.update_offer.as_ref().filter(|_| !self.offline), self.github_offer.as_ref())
    }

    pub(crate) fn satchel_answer(&mut self, result: Result<Option<update::Offer>, String>) {
        self.satchel_failed = result.is_err();
        let failed = match result {
            Ok(offer) => {
                self.github_offer = offer;
                None
            }
            Err(e) => {
                log!("the Satchel's GitHub release not taken: {e}");
                Some(e)
            }
        };
        // The answer Check for updates waits for; a newer Satchel beside sync\ is named over it.
        if std::mem::take(&mut self.check_pending) {
            let text = match (self.offer(), failed) {
                (Some(offer), _) if offer.from_github => format!("Checked: {} is ready, downloaded from GitHub", offer.version),
                (Some(offer), _) => format!("Checked: {} is ready {}", offer.version, offer.place()),
                // No answer says so itself ("GitHub didn't answer (…)"); else a release was refused
                // (no checksum, a wrong one, another version inside).
                (None, Some(e)) if e.starts_with(net::NO_ANSWER) => format!("Checked: {e}"),
                (None, Some(e)) => format!("Checked: nothing could be taken from GitHub ({e})"),
                (None, None) => format!("Checked: you have the newest version ({})", about::VERSION),
            };
            self.check_result = Some(text.clone());
            self.say(text);
        }
    }

    /// The Play tab's update line; the banner says why nothing syncs when the version check
    /// keeps this Satchel out.
    fn update_line(&self) -> UpdateLine {
        match (&self.gated, self.offer()) {
            (gated, Some(offer)) => UpdateLine {
                text: format!("Survivor's Satchel {} is ready {}.", offer.version, offer.place()),
                version: offer.version.clone(),
                gated: gated.is_some(),
            },
            (Some(_), None) => UpdateLine { text: "Copy the new SurvivorsSatchel.exe to this PC to update it.".into(), version: String::new(), gated: true },
            (None, None) => UpdateLine::default(),
        }
    }

    fn open_card(&mut self) {
        if self.card.is_none() {
            self.card = Some((Vec::new(), false));
        }
        self.card_until = None;
    }

    fn card_line(&mut self, text: &str, done: bool, warn: bool) {
        if let Some((lines, asking)) = self.card.as_mut() {
            *asking = false;
            lines.push((text.into(), done, warn));
            if lines.len() > 8 {
                lines.remove(0);
            }
        }
    }

    fn card_ask(&mut self, text: String) {
        self.card_gate = false;
        self.open_card();
        if let Some((lines, asking)) = self.card.as_mut() {
            lines.push((text, false, true));
            *asking = true;
        }
    }

    fn close_card_in(&mut self, seconds: u64) {
        self.card_until = Some(Instant::now() + Duration::from_secs(seconds));
    }
}

fn game_status(dir: Option<&Path>) -> String {
    match dir {
        Some(dir) if dir.join(win::GAME_EXE).is_file() => "✓ Game found".into(),
        Some(_) => format!("✗ {} isn't in this folder", win::GAME_EXE),
        None => "Not set: press Find it, or paste the folder".into(),
    }
}

/// The theme as Settings' list shows it: System, Light, Dark.
fn theme_index(theme: Theme) -> i32 {
    match theme {
        Theme::System => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    }
}

/// "released today", from this version's CHANGELOG heading; empty when it has no date.
fn released_text() -> String {
    about::released()
        .map(|day| satchel_core::addons::overview::released_ago((Local::now().date_naive() - day).num_days()))
        .unwrap_or_default()
}
