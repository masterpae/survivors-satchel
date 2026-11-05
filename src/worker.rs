//! The worker thread: watches the game, runs the sync engine, tells the UI what to show.
//! Everything slow (the network share above all) happens here, never on the UI thread.

mod addons;
mod forge;
mod game_news;
mod self_update;
mod share;

use crate::net::{self, Answer, Job};
use crate::about;
use crate::view::{Answered, Form, FormView, PcLine, Pill, PlaytimeView, RailView, Recent, SaveLine, Setup, Tray, Ui, View};
use crate::win;
use chrono::{DateTime, Datelike, Local, Utc};
use satchel_core::activity::{self, Kind};
use satchel_core::decide::Decision;
use satchel_core::engine::{BroughtIn, Engine, Outlook, REACH_WAIT, Shown, SyncError};
use satchel_core::fsx;
use satchel_core::gate;
use satchel_core::log;
use satchel_core::markers::{HEARTBEAT_SECS, InUse, Other};
use satchel_core::paths;
use satchel_core::playtime::{self, Session, format_long, format_minutes};
use satchel_core::session::{Event, Tracker};
use satchel_core::settings::{self as settings_rules, Settings, Theme};
use satchel_core::shell::{self, Dot, NeedsYouFacts, Notice, WaitingOk};
use satchel_core::game_news::card as news_card;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

const TICK: Duration = Duration::from_secs(2);
const REFRESH_EVERY: Duration = Duration::from_secs(60);
const RETRY_EVERY: Duration = Duration::from_secs(180);
/// A write to the live save is checkpointed once it has been quiet this long.
const SETTLE: Duration = Duration::from_secs(3);
/// How long a one-off message (a notice) stays on the rail's progress line, and the progress
/// question's own refusal in its overlay.
const NOTICE_FOR: Duration = Duration::from_secs(60);
/// The window's tabs, in order: Home, Addons, Settings.
const TAB_SETTINGS: i32 = 2;

pub enum Command {
    Play,
    PlayAnyway,
    CancelPlay,
    SyncNow,
    BringBack(usize),
    /// Progress versions' Open the folder: the network folder's sync folder, or this PC's copies.
    OpenSavesFolder,
    OpenFolder,
    /// The gear's Open the game's folder.
    OpenGameFolder,
    /// Home's progress question: Choose… opens the question over the window, where both sides show.
    ChooseProgress,
    /// The question over the window: this PC's progress, the other PC's, or later.
    KeepHere,
    UseOther,
    DecideLater,
    /// One Settings change: its row (`sync`, `folder`, `theme`, …) and the value the window sent
    /// (`true`/`false`, the theme's name, the text typed or picked).
    Setting(String, String),
    /// Restore defaults: the switches and the theme as a new install has them, except sync and
    /// the two sharing switches; the folders and this PC's name stay.
    RestoreDefaults,
    /// Settings' Find it: the game looked for in the usual folders, kept when found.
    DetectGame,
    /// The first start's folder picker chose this folder.
    GameFolderPicked(PathBuf),
    FinishSetup,
    /// The window closed to the tray, maximised or not: remembered for the next start.
    WindowClosed(bool),
    // Addons: ids are a record's, a catalog entry's, SurvivorsForge's own (`forge:<slug>`), or
    // `folder:<name>`.
    AddonsCheck,
    AddonsUpdateAll,
    AddonsOpenFolder,
    AddonSwitch(String, bool),
    AddonUpdate(String),
    /// Update anyway: the player's OK for an update SurvivorsForge asks about first.
    AddonUpdateAnyway(String),
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
    /// Installs an addon from Browse (our list's, or SurvivorsForge's own), or replaces a copy
    /// installed by hand (`folder:<name>`).
    BrowseInstall(String),
    /// Install anyway: the player's OK for an addon SurvivorsForge asks about first.
    BrowseInstallAnyway(String),
    /// Browse opened an addon's panel (again, perhaps): its SurvivorsForge details and versions.
    BrowseOpen(String),
    /// The panel closed.
    BrowseClose,
    /// The panel's "Show what it found": the findings of the version it shows.
    ForgeShowScan(String),
    /// The Addons tab, or one of its pages, came into view (0 Installed, 1 Browse): the icons of
    /// the addons it lists.
    AddonsShown(i32),
    /// Browse's switch: `list`, `cards` or `table`, kept in settings.toml.
    BrowseView(String),
    /// The picture viewer opened on, or moved to, one of an addon's screenshots (its id, the
    /// picture's place in its details).
    PictureShow(String, usize),
    /// The viewer closed.
    PictureClose,
    /// The open addon's pictures row shows this many frames (its id, the count): their thumbnails.
    ThumbsShown(String, usize),
    /// Versions… opened for an addon (Browse's, or an installed one).
    VersionsOpen(String),
    VersionsClose,
    /// Versions…' Install this version (`false`), or Install anyway (`true`): an addon's id and
    /// the version's tag.
    VersionInstall(String, String, bool),
    /// Versions…' Go back to this: an addon's id and the backup of the version kept.
    VersionBack(String, String),
    /// An incoming addon's Install anyway: the player's OK for the file another PC has.
    IncomingInstallAnyway(String),
    /// Home's Install anyway for an addon that waits for an OK: its update, or its arrival from
    /// another PC, whichever it is.
    OkAnyway(String),
    /// A link in Settings or About: SurvivorsForge's repository or website, or a GitHub
    /// repository's page; nothing else is opened.
    OpenLink(String),
    LinkLookup(String),
    LinkInstall,
    LinkClear,
    ImportZip(PathBuf),
    /// The Removed list's Install again.
    InstallAgain(String),
    /// Its Install anyway: the player's OK where SurvivorsForge asks first about that version.
    InstallAgainAnyway(String),
    /// After the game's settings changed here and on another PC: `true` keeps this PC's.
    GameSettingsChoice(bool),
    /// "Use them instead": this PC's own game settings from before it joined.
    GameSettingsOwn,
    /// The newer Satchel downloaded from GitHub replaces this one.
    UpdateNow,
    /// About's Check for updates: GitHub now, in a build that knows its repository.
    CheckUpdates,
    /// What's new was opened: "Updated to …" goes.
    WhatsNewSeen,
    /// The game's patch notes were opened: the Play line stops being new for the build it names.
    GameNewsSeen,
    /// The no-game panel's Find it: the game's folder is looked for and kept when found.
    FindGame,
    /// The no-game panel's Get the game.
    OpenGameSite,
    /// An answer from the network thread.
    Net(Answer),
    Quit,
}

/// What a row of Progress versions stands for.
#[derive(Clone)]
enum Row {
    Version(u64),
    File(PathBuf),
}

#[derive(Default)]
struct Lists {
    saves: Vec<SaveLine>,
    saves_summary: String,
    rows: Vec<Row>,
    playtime: PlaytimeView,
    /// The newest session: its day as the rail says it, its minutes, its PC.
    last_session: Option<(String, u32, String)>,
    /// The PCs of Home's Your PCs card, this PC first.
    pcs: Vec<PcLine>,
    pcs_week: String,
    recent: Vec<Recent>,
    last_copy: Option<DateTime<Utc>>,
    /// The last successful send or bring-in: the newest version this PC sent or has.
    last_sync: Option<DateTime<Utc>>,
    pc_count: usize,
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
    /// The problem the rail has been told of: a new one shows at once, as a notice.
    problem_told: Option<String>,
    notice: Option<Notice>,
    notice_at: Option<Instant>,
    split: Option<u64>,
    needs_you_open: bool,
    /// Why the progress question's pick didn't go through (the game running, the folder not
    /// answering, …), under its buttons while it is open.
    needs_note: Option<(String, Instant)>,
    /// What the "Needs you" choice shows on screen: an answer applies to exactly that.
    shown: Option<Shown>,
    // Play.
    pending_play: bool,
    play_anyway: bool,
    /// This PC's own in-use marker from an earlier run has been dealt with (or there was none).
    leftover_done: bool,
    gated: Option<String>,
    /// The loop ends after this step (an update started the new Satchel).
    quit: bool,
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
    /// The one channel to the network thread; it answers on the worker's own (`Command::Net`).
    net: Sender<Job>,
    update: self_update::UpdateState,
    addons: addons::AddonsState,
    share: share::ShareState,
    forge: forge::ForgeState,
    news: game_news::NewsState,
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
        let net = net::start(engine.local.addon_cache(), engine.local.forge_dir(), commands);
        let forge = forge::ForgeState::new(engine.local.forge_dir());
        let news = game_news::NewsState::new(engine.local.game_news_dir());
        Worker {
            net,
            update: self_update::UpdateState::default(),
            addons: addons::AddonsState::new(store),
            share: share::ShareState::new(memory),
            forge,
            news,
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
            problem_told: None,
            notice: None,
            notice_at: None,
            needs_note: None,
            split: None,
            needs_you_open: false,
            shown: None,
            pending_play: false,
            play_anyway: false,
            leftover_done: false,
            gated: None,
            quit: false,
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
                self.warn("The Satchel hit an internal error and carried on; its log has the details.".into());
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
        // SurvivorsForge's answers kept on this PC, before the first lists are made from them;
        // the game's patch notes as last fetched.
        self.forge_load();
        self.news_load();
        self.refresh();
        self.push_form();
        self.addons_scan();
        self.addons_prune_cache();
        self.addons_look();
        self.forge_look();
        self.news_look();
        self.share_look();
        self.satchel_look();
    }

    fn handle(&mut self, command: Command) {
        // The window shows an install as under way the moment it's asked for: a view always
        // follows one, even when nothing else changed (a refusal said a second time, say).
        if matches!(
            command,
            Command::BrowseInstall(_)
                | Command::BrowseInstallAnyway(_)
                | Command::AddonUpdate(_)
                | Command::AddonUpdateAnyway(_)
                | Command::LinkInstall
                | Command::VersionInstall(..)
                | Command::InstallAgainAnyway(_)
                | Command::IncomingInstallAnyway(_)
                | Command::OkAnyway(_)
        ) {
            self.last_view = None;
        }
        // What changed here goes to the other PCs at once.
        let changes_addons = matches!(
            command,
            Command::AddonsUpdateAll
                | Command::AddonSwitch(..)
                | Command::AddonUpdate(_)
                | Command::AddonUpdateAnyway(_)
                | Command::AddonBack(_)
                | Command::AddonKeep(..)
                | Command::AddonRemove(..)
                | Command::BrowseInstall(_)
                | Command::BrowseInstallAnyway(_)
                | Command::LinkInstall
                | Command::ImportZip(_)
                | Command::InstallAgain(_)
                | Command::InstallAgainAnyway(_)
                | Command::VersionInstall(..)
                | Command::VersionBack(..)
                | Command::IncomingInstallAnyway(_)
                | Command::OkAnyway(_)
                | Command::Setting(..)
                | Command::RestoreDefaults
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
            Command::OpenSavesFolder => match self.engine.shared() {
                Some(shared) => win::open_folder(&shared.sync_dir()),
                None => win::open_folder(&self.engine.local.copies_dir()),
            },
            Command::OpenFolder => {
                if let Some(shared) = self.engine.shared() {
                    win::open_folder(&shared.folder);
                }
            }
            Command::OpenGameFolder => {
                if let Some(dir) = self.engine.settings.game_dir.clone() {
                    win::open_folder(&dir);
                }
            }
            Command::WindowClosed(maximised) => self.window_closed(maximised),
            Command::ChooseProgress => {
                if self.split.is_some() {
                    self.needs_you_open = true;
                    self.needs_note = None;
                }
            }
            Command::KeepHere => self.resolve(true),
            Command::UseOther => self.resolve(false),
            Command::DecideLater => {
                self.needs_you_open = false;
                self.needs_note = None;
                self.pending_play = false;
                self.card = None;
            }
            Command::Setting(key, value) => self.setting(&key, &value),
            Command::RestoreDefaults => self.restore_defaults(),
            Command::DetectGame => self.detect_game(),
            Command::GameFolderPicked(dir) => self.game_folder_picked(dir),
            Command::FinishSetup => self.finish_setup(),
            Command::AddonsCheck => self.addons_check(),
            Command::AddonsUpdateAll => self.addons_update_all(),
            Command::AddonsOpenFolder => self.addons_open_folder(),
            Command::AddonSwitch(id, on) => self.addon_switch(&id, on),
            Command::AddonUpdate(id) => self.addon_update(&id),
            Command::AddonUpdateAnyway(id) => self.addon_update_anyway(&id),
            Command::AddonBack(id) => self.addon_back(&id),
            Command::AddonKeep(id, keep) => self.addon_keep(&id, keep),
            Command::AddonRemove(id, keep_settings) => self.addon_remove(&id, keep_settings),
            Command::AddonPage(id) => self.addon_page(&id),
            Command::AddonFolder(id) => self.addon_folder(&id),
            Command::AddonLocalOnly(id, on) => self.addon_local_only(&id, on),
            Command::AddonSettingsPerPc(id, on) => self.addon_settings_per_pc(&id, on),
            Command::AddonSettingsChoice(id, keep_here) => self.addon_settings_choice(&id, keep_here),
            Command::BrowseInstall(id) => self.browse_install(&id),
            Command::BrowseInstallAnyway(id) => self.browse_install_anyway(&id),
            Command::BrowseOpen(id) => self.browse_open(&id),
            Command::BrowseClose => self.browse_close(),
            Command::ForgeShowScan(id) => self.forge_show_scan(&id),
            Command::AddonsShown(page) => self.forge_icons_wanted(page),
            Command::BrowseView(view) => self.browse_view(&view),
            Command::PictureShow(id, index) => self.forge_picture_show(&id, index),
            Command::PictureClose => self.forge_picture_close(),
            Command::ThumbsShown(id, frames) => self.forge_thumbs_wanted(&id, frames),
            Command::VersionsOpen(id) => self.versions_open(&id),
            Command::VersionsClose => self.addons.versions_open = None,
            Command::VersionInstall(id, tag, anyway) => self.version_install_as(&id, &tag, anyway, true),
            Command::VersionBack(id, backup) => self.version_back(&id, Some(&backup)),
            Command::IncomingInstallAnyway(id) => self.incoming_install_anyway(&id),
            Command::OkAnyway(id) => {
                if self.addons.store.get(&id).is_some() {
                    self.addon_update_anyway(&id);
                } else {
                    self.incoming_install_anyway(&id);
                }
            }
            Command::OpenLink(url) => self.open_link(&url),
            Command::LinkLookup(text) => self.link_lookup(text),
            Command::LinkInstall => self.link_install(),
            Command::LinkClear => self.link_clear(),
            Command::ImportZip(path) => self.import_zip(path),
            Command::InstallAgain(id) => self.install_again_as(&id, false, true),
            Command::InstallAgainAnyway(id) => self.install_again_as(&id, true, true),
            Command::GameSettingsChoice(keep_here) => self.game_settings_choice(keep_here),
            Command::GameSettingsOwn => self.game_use_own(),
            Command::UpdateNow => self.update_now(),
            Command::CheckUpdates => self.check_updates(),
            Command::WhatsNewSeen => self.whats_new_seen(),
            Command::GameNewsSeen => self.news_seen(),
            Command::FindGame => self.find_game(),
            Command::OpenGameSite => win::open_url(win::GAME_SITE),
            Command::Net(answer) => self.on_net(answer),
            Command::Quit => {}
        }
        if changes_addons {
            self.share_look();
        }
    }

    /// An answer from the network thread, to the concern that asked.
    fn on_net(&mut self, answer: Answer) {
        match answer {
            Answer::Looked(results) => self.addons_looked(results),
            Answer::Fetched { id, tag, sha256 } => self.addons_fetched(id, tag, sha256),
            Answer::Picture { url, kind, pixels } => self.forge_picture_answer(&url, kind, pixels),
            Answer::Satchel(result) => self.satchel_answer(result),
            Answer::ForgeRegistry(answer) => self.forge_registry_answer(answer),
            Answer::ForgeDetails { slug, answer } => self.forge_details_answer(&slug, answer),
            Answer::ForgeVersions { slug, answer } => self.forge_versions_answer(&slug, answer),
            Answer::ForgeFindings { slug, tag, answer } => self.forge_findings_answer(&slug, &tag, answer),
            Answer::GameNews { feed, answer } => self.news_answer(feed, answer),
            Answer::LookedUp(result) => self.addons_looked_up(result),
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
        // SurvivorsForge's registry keeps its cadence during a session too: one small request per
        // 30 minutes, usually a 304. What goes in after a session is then judged on a fresh answer,
        // and "isn't answering" never just means nobody asked.
        self.forge_look();
        // The game's patch notes keep theirs the same way: two small requests per 30 minutes.
        self.news_look();
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
                self.warn(format!("{from} has newer progress: close the game and press Play to bring it in."));
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
        self.update_on_session_over();
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
                        None => format!(" {}", self_update::update_hint()),
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
                    "This PC's save {why}, so it isn't synced. To carry on from the network folder, cancel and bring back its newest version under Settings → Progress versions."
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
                            self.warn(format!("Close the game to bring in version {n}."));
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
            self.refuse_pick("Close the game first, then pick.");
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
                self.needs_note = None;
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
                self.refuse_pick("The network folder doesn't answer; pick again when it's back.");
            }
            Err(SyncError::Changed) => {
                log!("the choice was made on a view that had changed: asked again");
                self.refuse_pick("Something changed since you looked (another PC sent, or this PC's save changed). Check the two again and pick.");
            }
            Err(e) => {
                log!("choice not saved: {e}");
                let why = format!("Your choice couldn't be saved: {e}");
                self.needs_note = Some((why.clone(), Instant::now()));
                self.problem = Some(why);
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
            self.warn("Close the game first: a save is only brought in while the game is closed.".into());
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

    /// The window closed to the tray: whether it was maximised is remembered for the next start.
    fn window_closed(&mut self, maximised: bool) {
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

    /// One Settings change, from its row (`key`, as the window names it) with the value the window
    /// sent. Only that row is checked (`settings::change`), and only a value that changed and went
    /// through is written (atomically, as every settings file is). The window gets the form as
    /// saved, with the row that applied ("✓ Saved") or why it was refused (the text typed stays in
    /// its field). Only sync, the network folder and this PC's name read the folder again.
    fn setting(&mut self, key: &str, typed: &str) {
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
        match key {
            "sync" | "folder" | "pc-name" => {
                self.refresh();
                self.share_look();
            }
            "start-with-windows" | "desktop-shortcut" => self.apply_windows_settings(),
            "game-dir" => {
                self.apply_windows_settings();
                self.addons_scan();
                self.share_look();
            }
            "addons-shared" | "game-shared" => self.share_look(),
            // SurvivorsForge switched on: asked at once when it's due. Off, nothing is.
            "addons-forge" => {
                self.addons_scan();
                self.forge_look();
            }
            "addons-auto" => self.addons_scan(),
            _ => {}
        }
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
            self.outlook = None;
            self.split = None;
            self.offline = false;
        }
        self.engine.settings = next;
        self.problem = None;
        Ok(())
    }

    /// Restore defaults: the switches and the theme as a new install has them, except sync and
    /// the two sharing switches; the folders and this PC's name stay (`settings::restore_defaults`).
    fn restore_defaults(&mut self) {
        let next = settings_rules::restore_defaults(&self.engine.settings);
        let mut answered = Answered { key: "defaults".into(), ..Answered::default() };
        if next != self.engine.settings {
            match self.write_settings(next) {
                Ok(()) => {
                    self.apply_windows_settings();
                    self.addons_scan();
                    self.forge_look();
                }
                Err(e) => {
                    log!("defaults not saved: {e}");
                    self.warn(format!("The defaults couldn't be saved: {e}"));
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
    fn detect_game(&mut self) {
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
    fn game_folder_picked(&mut self, dir: PathBuf) {
        if self.setup.is_none() {
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
    fn browse_view(&mut self, view: &str) {
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
        } else if let Some(outlook) = &self.outlook {
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

    fn push_form(&self) {
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
    fn enter_setup(&mut self) {
        self.setup = Some(Setup::default());
        self.push_form();
        self.ui.show(None);
    }

    /// The first-start screen's sync line, for the game folder it shows now: "Sync is on ·
    /// <folder>" when the defaults file turned sync on, or, when that folder can't be used with
    /// this game folder, the line that says sync stays off (in amber), as Start will say it too.
    fn setup_view(&self) -> Setup {
        let settings = &self.engine.settings;
        let Some(folder) = settings.sync_folder() else { return Setup::default() };
        match paths::sync_folder_problem(folder, &self.engine.local, settings.game_dir.as_deref()) {
            Some(why) => Setup { sync_line: defaults_folder_line(&why), sync_warn: true },
            None => Setup { sync_line: format!("Sync is on · {}", folder.display()), sync_warn: false },
        }
    }

    /// The first start's Start: the game's folder (detected, or picked there) and what the
    /// defaults file beside the exe set are written, and the first sync runs.
    fn finish_setup(&mut self) {
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
            self.warn(defaults_folder_line(&why));
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
        let now = Utc::now();
        if self.engine.shared().is_some() {
            match self.engine.outlook(now) {
                Ok(outlook) => match outlook.decision {
                    // Both sides have progress: the normal question, at Play and on Home.
                    Decision::Split { nas } => {
                        self.split = Some(nas);
                        self.needs_you_open = true;
                    }
                    Decision::Send => {
                        // 0.8 offered the newest export at the top of an empty folder in this
                        // PC's save's place; this PC's save goes first now, and the export stays
                        // one click away.
                        let export = self.differing_export(&outlook);
                        match self.engine.send(now, None, Some("first start".into()), None) {
                            Ok(Some(v)) => self.activity(Kind::Sync, &format!("Started the network folder with version {}", v.n)),
                            Ok(None) => {}
                            Err(e) => self.problem = Some(format!("The first sync failed: {e}")),
                        }
                        if let Some(name) = export {
                            self.activity(Kind::Sync, &format!("Your export {name} is under Settings → Progress versions"));
                        }
                    }
                    Decision::BringIn(n) => {
                        self.open_card();
                        self.bring_in(n, now);
                        self.close_card_in(8);
                    }
                    Decision::InSync { adopt } => {
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
        self.push_form();
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
                        format!("This PC's save {why}, so it isn't sent. To carry on from the network folder, bring back its newest version under Settings → Progress versions.")
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
        } else {
            self.outlook = None;
            self.gated = None;
        }
        self.build_lists();
    }

    fn build_lists(&mut self) {
        let now = Utc::now();
        let local_now = Local::now();
        let mut lists = Lists::default();
        let pc = self.engine.pc().to_string();
        // Progress versions: the network folder's, newest first, plus the exports at its top; with
        // sync off, this PC's copies.
        if let Some(outlook) = &self.outlook {
            let here = outlook.local.as_ref().map(|l| l.sha256.as_str());
            for version in &outlook.index.versions {
                let current = here == Some(version.sha256.as_str());
                let mut notes: Vec<String> = Vec::new();
                if current {
                    notes.push("the one you play".into());
                }
                if let Some(note) = &version.note {
                    notes.push(if note.contains("two PCs moved on separately") { "kept when two PCs changed it".into() } else { note.clone() });
                }
                lists.saves.push(SaveLine {
                    version: format!("v{}", version.n),
                    when: shell::when_short(version.saved, &local_now),
                    from: version.pc.clone(),
                    size: size(version.bytes),
                    note: notes.join(" · "),
                    current,
                    can_bring: !current,
                });
                lists.rows.push(Row::Version(version.n));
                if version.pc == pc && lists.last_sync.is_none_or(|t| version.saved > t) {
                    lists.last_sync = Some(version.saved);
                }
            }
            // The newest few: older exports pile up in the game's Saves folder.
            if let Ok(exports) = self.engine.exports() {
                for export in exports.into_iter().take(5) {
                    lists.saves.push(SaveLine {
                        version: export.name.clone(),
                        when: export.modified.map(|t| shell::when_short(t, &local_now)).unwrap_or_default(),
                        from: "–".into(),
                        size: size(export.bytes),
                        note: "your export".into(),
                        current: false,
                        can_bring: true,
                    });
                    lists.rows.push(Row::File(export.path));
                }
            }
            lists.pc_count = outlook.index.pcs.len().max(1);
            // The version this PC last synced, when it was another PC's.
            if let Some(synced) = &self.engine.state.last_synced
                && lists.last_sync.is_none_or(|t| synced.at > t)
            {
                lists.last_sync = Some(synced.at);
            }
            let versions = outlook.index.versions.len();
            lists.saves_summary = match (versions, outlook.index.newest_version()) {
                (0, _) | (_, None) => "No versions yet: the first session sends version 1".into(),
                (n, Some(v)) => format!("{n} version{} · newest {} from {}", if n == 1 { "" } else { "s" }, shell::when_short(v.saved, &local_now), v.pc),
            };
            // Your PCs: every PC in the index, this PC first, with its newest version and its
            // newest session.
            let sessions = self.engine.sessions().unwrap_or_default();
            let mut names: Vec<String> = outlook.index.pcs.clone();
            if !names.iter().any(|n| n == &pc) {
                names.insert(0, pc.clone());
            }
            names.sort_by_key(|n| (n != &pc, n.to_lowercase()));
            lists.pcs = names
                .iter()
                .map(|name| {
                    let synced = outlook.index.versions.iter().filter(|v| &v.pc == name).map(|v| v.saved).max();
                    let played = sessions.iter().filter(|s| &s.pc == name).map(|s| s.start).max();
                    let mut parts = Vec::new();
                    if let Some(at) = played {
                        parts.push(format!("played {}", shell::day_word(at, &local_now)));
                    }
                    match synced {
                        Some(at) => parts.push(format!("synced {}", shell::when_short(at, &local_now))),
                        None => parts.push("not synced yet".into()),
                    }
                    PcLine { name: name.clone(), this_pc: name == &pc, line: parts.join(" · ") }
                })
                .collect();
        } else if self.engine.shared().is_none()
            && let Ok(copies) = self.engine.copies()
        {
            lists.last_copy = copies.first().and_then(|c| c.modified);
            let count = copies.len();
            lists.saves_summary = match copies.first() {
                None => "No copies yet: one is kept after each session".into(),
                Some(c) => format!("{count} cop{} · newest {}", if count == 1 { "y" } else { "ies" }, c.modified.map(|t| shell::when_short(t, &local_now)).unwrap_or_default()),
            };
            for (i, copy) in copies.into_iter().enumerate() {
                lists.saves.push(SaveLine {
                    version: copy.name.clone(),
                    when: copy.modified.map(|t| shell::when_short(t, &local_now)).unwrap_or_default(),
                    from: pc.clone(),
                    size: size(copy.bytes),
                    note: if i == 0 { "the newest copy".into() } else { String::new() },
                    current: false,
                    can_bring: true,
                });
                lists.rows.push(Row::File(copy.path));
            }
        }
        // Playtime: this week, the last seven days, per PC, the last sessions.
        let sessions = self.engine.sessions().unwrap_or_default();
        let totals = playtime::totals(&sessions, &local_now);
        let most = totals.week_per_pc.iter().map(|(_, m)| *m).max().unwrap_or(0).max(1);
        let days = playtime::per_day(&sessions, &local_now);
        let tallest = days.iter().map(|d| d.minutes).max().unwrap_or(0).max(1);
        lists.playtime = PlaytimeView {
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
            sessions: sessions.iter().take(30).map(|s| (shell::day_long(s.start, &local_now), s.pc.clone(), format_long(s.minutes))).collect(),
        };
        lists.pcs_week = format!("This week: {} · {} session{}", format_long(totals.week), totals.week_sessions, if totals.week_sessions == 1 { "" } else { "s" });
        lists.last_session = sessions.first().map(|s| (shell::day_word(s.start, &local_now), s.minutes, s.pc.clone()));
        lists.recent = activity::recent(&self.engine.local.activity(), 5)
            .unwrap_or_default()
            .iter()
            .map(|e| Recent { kind: e.shown_kind(), text: e.text.clone(), when: shell::when_short(e.at, &local_now) })
            .collect();
        let _ = now;
        self.lists = lists;
        self.addons_tick();
    }

    /// What the window and tray show, and what the "Needs you" choice shows (if it is open).
    fn build_view(&self) -> (View, Option<Shown>) {
        let sync_on = self.engine.shared().is_some();
        let (dark, system_dark) = self.theme_dark();
        let version_new = self.version_new();
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
            Tray::NeedsYou => match self.problem.as_ref() {
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
        let last = if sync_on { self.lists.last_sync } else { self.lists.last_copy };
        let last = last.map(|t| shell::when_short(t, &local_now));
        // A notice, for its minute, comes first whatever the state.
        let progress = shell::progress_line(dot, &banner, sync_on, last.as_deref(), self.lists.pc_count, self.notice.as_ref());
        let addons = self.addons_view();
        let addons_line = shell::addons_line(addons.rail);
        let playing = self.tracker.in_session();
        let session_minutes = self.session_start.map(|start| ((Utc::now() - start).num_seconds().max(0) / 60) as u32);
        let last_line = shell::last_line(
            if playing { Some(session_minutes.unwrap_or(0)) } else { None },
            sync_on,
            self.lists.last_session.as_ref().map(|(day, minutes, pc)| (day.as_str(), *minutes, pc.as_str())),
        );
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
        let split_pc = self.split.and_then(|n| self.outlook.as_ref().and_then(|o| o.index.get(n)).map(|v| v.pc.clone()));
        let facts = NeedsYouFacts {
            progress_split: self.split.map(|_| split_pc.unwrap_or_else(|| "the other PC".into())),
            game_ask_pc: non_empty(&game_settings.ask_pc).map(str::to_string),
            game_own: non_empty(&game_settings.own).map(str::to_string),
            addon_asks: addons.rows.iter().filter(|r| !r.ask_pc.is_empty()).map(|r| (r.id.clone(), r.name.clone(), r.ask_pc.clone())).collect(),
            waiting_ok,
            folder_down: sync_on && self.offline,
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
        // The pick's refusal, under the overlay's buttons while it is open, for its minute.
        let needs_note = match (&needs_you, &self.needs_note) {
            (Some(_), Some((why, at))) if at.elapsed() < NOTICE_FOR => why.clone(),
            _ => String::new(),
        };

        let saves_note = if sync_on {
            "Every version in the network folder, newest first, plus your own exports; bring an older one back if something went wrong. Your save from before is kept as a safety copy.".into()
        } else {
            "The copies kept on this PC after each session, newest first; bring an older one back if something went wrong.".into()
        };

        let view = View {
            tray,
            status,
            banner,
            busy: self.busy.is_some(),
            sync_on,
            playing,
            rail,
            pill,
            news_version,
            news_blocks,
            news_more,
            pcs: self.lists.pcs.clone(),
            pcs_week: self.lists.pcs_week.clone(),
            questions,
            recent: self.lists.recent.clone(),
            playtime: self.lists.playtime.clone(),
            saves: self.lists.saves.clone(),
            saves_summary: self.lists.saves_summary.clone(),
            saves_note,
            needs_you,
            needs_note,
            card: self.card.clone(),
            setup: self.setup.as_ref().map(|_| self.setup_view()),
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
            card_update: if self.card_gate { self.offer().map(|o| o.version.clone()).unwrap_or_default() } else { String::new() },
            game_news: news,
        };
        (view, shown)
    }

    fn push(&mut self) {
        // A problem newly set is a notice too: it shows at once, ahead of an older one, and the
        // tray's needs-you state keeps it on the rail after its minute.
        if self.problem != self.problem_told {
            self.problem_told = self.problem.clone();
            if let Some(problem) = self.problem.clone() {
                self.warn(problem);
            }
        }
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

    /// A one-off message, plain news: on the rail's progress line for a minute, whatever the
    /// tray's state, in the text colour.
    fn say(&mut self, text: String) {
        self.tell(text, false);
    }

    /// A one-off problem: on the rail's progress line for a minute, in amber.
    fn warn(&mut self, text: String) {
        self.tell(text, true);
    }

    fn tell(&mut self, text: String, problem: bool) {
        self.notice = Some(Notice { text, problem });
        self.notice_at = Some(Instant::now());
    }

    /// The progress question's pick refused: why, under its buttons, and on the rail.
    fn refuse_pick(&mut self, why: &str) {
        self.needs_note = Some((why.to_string(), Instant::now()));
        self.warn(why.to_string());
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

    /// The no-game panel's Find it: the game looked for in the usual folders, and kept when it
    /// passes the check a folder typed in Settings does; Settings shows it.
    fn find_game(&mut self) {
        let Some(dir) = win::detect_game_dir() else {
            self.warn("The game wasn't found in the usual folders. If it's installed, choose its folder in Settings.".into());
            return;
        };
        let found = dir.display().to_string();
        let answer = settings_rules::change(&self.engine.settings, "game-dir", &found, &Checks(self));
        if let Some(refusal) = answer.refused {
            log!("found game not used: {}", refusal.why);
            let mut why = refusal.why.chars();
            let why = why.next().map(|first| first.to_lowercase().collect::<String>() + why.as_str()).unwrap_or_default();
            self.warn(format!("The game was found in {found}, but {why}. See Settings."));
            return;
        }
        if answer.applied.is_some()
            && let Err(e) = self.write_settings(answer.settings)
        {
            self.problem = Some(format!("Settings couldn't be saved: {e}"));
            return;
        }
        log!("game found at {found}");
        self.apply_windows_settings();
        self.say(format!("The game was found in {found}"));
        self.answer_form(Answered { key: "game-dir".into(), typed: found, applied: answer.applied.is_some(), why: String::new() });
        self.refresh();
        self.addons_scan();
        self.share_look();
    }

    /// The newest export at the top of an empty network folder, when it isn't this PC's save: its
    /// name.
    fn differing_export(&self, outlook: &Outlook) -> Option<String> {
        if !outlook.index.versions.is_empty() {
            return None;
        }
        let local = self.engine.local_save().ok().flatten()?;
        let export = self.engine.exports().ok()?.into_iter().next()?;
        let bytes = fs::read(&export.path).ok()?;
        (fsx::fingerprint(&bytes) != local.sha256).then_some(export.name)
    }

    fn open_card(&mut self) {
        if self.card.is_none() {
            self.card = Some((Vec::new(), false));
        }
        self.card_until = None;
    }

    /// A line on the Play card; on the rail too, for its minute (the card closes sooner), in amber
    /// when it warns. The card's question (`card_ask`) is not a notice.
    fn card_line(&mut self, text: &str, done: bool, warn: bool) {
        self.tell(text.to_string(), warn);
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

/// "released today", from this version's CHANGELOG heading; empty when it has no date.
fn released_text() -> String {
    about::released()
        .map(|day| satchel_core::addons::overview::released_ago((Local::now().date_naive() - day).num_days()))
        .unwrap_or_default()
}
