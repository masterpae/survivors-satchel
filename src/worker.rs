//! The worker thread: watches the game, runs the sync engine, tells the UI what to show.
//! Everything slow (the network share above all) happens here, never on the UI thread.

mod addons;
mod feedback;
mod forge;
mod game_news;
mod playtime;
mod self_update;
mod settings;
mod share;

use crate::net::{self, Answer, Job};
use crate::about;
use crate::view::{PcLine, Pill, RailView, Recent, SaveLine, Tray, Ui, View};
use crate::win;
use chrono::{DateTime, Datelike, Local, Utc};
use satchel_core::activity::{self, Kind};
use satchel_core::decide::Decision;
use satchel_core::engine::{BroughtIn, Engine, Outlook, Shown, SyncError};
use satchel_core::fsx;
use satchel_core::gate;
use satchel_core::log;
use satchel_core::markers::{HEARTBEAT_SECS, InUse, Other};
use satchel_core::playtime::format_minutes;
use satchel_core::session::{Event, Tracker};
use satchel_core::shell::{self, Dot, NeedsYouFacts, WaitingOk};
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
    /// The PCs of Home's Your PCs card, this PC first.
    pcs: Vec<PcLine>,
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
    feedback: feedback::Feedback,
    settings: settings::SettingsState,
    lists: Lists,
    last_refresh: Option<Instant>,
    last_retry: Option<Instant>,
    last_view: Option<View>,
    /// The one channel to the network thread; it answers on the worker's own (`Command::Net`).
    net: Sender<Job>,
    playtime: playtime::PlaytimeState,
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
            playtime: playtime::PlaytimeState::default(),
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
            needs_note: None,
            split: None,
            needs_you_open: false,
            shown: None,
            pending_play: false,
            play_anyway: false,
            leftover_done: false,
            gated: None,
            quit: false,
            feedback: feedback::Feedback::default(),
            settings: settings::SettingsState::new(first_start),
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
        if self.settings.in_setup() {
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
                if !self.settings.in_setup() {
                    self.tick();
                }
                self.push();
            }));
            if step.is_err() {
                self.feedback.clear_busy();
                self.feedback.warn("The Satchel hit an internal error and carried on; its log has the details.".into());
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
        self.flush_playtime();
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
                self.feedback.close_card();
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
            Command::OpenGameFolder => self.open_game_folder(),
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
                self.feedback.close_card();
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
            Command::OpenGameSite => self.open_game_site(),
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
            // The lists follow what SurvivorsForge says: verdicts, badges and its own offers.
            Answer::ForgeRegistry(answer) => {
                self.forge_registry_answer(answer);
                self.addons_lines();
            }
            Answer::ForgeDetails { slug, answer } => {
                self.forge_details_answer(&slug, answer);
                self.addons_lines();
                // The open addon's pictures are known now: the frames shown get their thumbnails.
                self.forge_thumbs_again();
            }
            Answer::ForgeVersions { slug, answer } => {
                let held = self.forge_versions_answer(&slug, answer);
                self.addons_lines();
                self.install_held(held);
                // An incoming addon OK'd while its version's word was asked for comes in now, if it may.
                self.share_follow_now(false);
            }
            Answer::ForgeFindings { slug, tag, answer } => self.forge_findings_answer(&slug, &tag, answer),
            Answer::GameNews { feed, answer } => self.news_answer(feed, answer),
            Answer::LookedUp(result) => self.addons_looked_up(result),
        }
    }

    /// What follows a settings change, by the row that changed (`key`, as the window names it;
    /// `defaults` for Restore defaults). Only sync, the network folder and this PC's name read
    /// the folder again.
    fn after_settings_changed(&mut self, key: &str) {
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
            "defaults" => {
                self.apply_windows_settings();
                self.addons_scan();
                self.forge_look();
            }
            _ => {}
        }
    }

    // The game ------------------------------------------------------------------------------

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
        self.feedback.expire();
        if !self.tracker.in_session() && self.feedback.busy().is_none() {
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
                self.playtime.closed(now);
                if self.engine.shared().is_some() {
                    self.send_after_close();
                    // The game writes the addons' settings as it closes: they go up now.
                    self.share_look();
                }
                if updating {
                    self.feedback.card_line("The game is updating itself. Waiting for it to restart…", false, false);
                }
            }
            Event::Updating => {
                log!("the game's files changed after it closed: an update");
                self.feedback.card_line("The game is updating itself. Waiting for it to restart…", false, false);
            }
            Event::Resumed => {
                log!("the game started again inside the quiet window: the same session goes on");
                self.files_stamp = self.game_dir().and_then(|d| game_files_stamp(&d));
                if self.feedback.card_open() {
                    self.feedback.card_line("Game restarted after its update", true, false);
                    self.feedback.close_card_in(8);
                }
            }
            Event::Over => self.session_over(now),
        }
    }

    fn session_started(&mut self, now: DateTime<Utc>) {
        log!("game started{}", if self.launched { " by Play" } else { " without Play" });
        self.playtime.started(now);
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
                self.feedback.warn(format!("{from} has newer progress: close the game and press Play to bring it in."));
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
        self.busy_push("Sending your progress…".into());
        let result = self.engine.send(Utc::now(), None, None, None);
        self.feedback.clear_busy();
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
                self.feedback.set_problem(format!("Sending failed: {e}. Your progress is safe on this PC."));
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
        let session = self.playtime.over(self.engine.pc(), now, self.tracker.updated);
        log!(
            "session over: {}, {} writes to the save while playing{}",
            format_minutes(session.minutes),
            self.writes,
            if session.updated { ", the game updated itself" } else { "" }
        );
        self.record_session(&session);
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
            self.feedback.open_card();
            self.feedback.card_line("The game is already running", false, true);
            self.feedback.close_card_in(6);
            return;
        }
        let Some(dir) = self.game_dir() else {
            self.feedback.set_problem("The game wasn't found. Set its folder in Settings.".into());
            self.ui.show(Some(TAB_SETTINGS));
            return;
        };
        self.feedback.open_card();
        if self.engine.shared().is_some() {
            self.busy_push("Checking the network folder…".into());
            let go_on = self.check_before_play(Utc::now());
            self.feedback.clear_busy();
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
                self.feedback.clear_problem();
                self.feedback.card_line("Game started", true, false);
                self.feedback.close_card_in(8);
            }
            Err(e) => {
                log!("the game didn't start: {e}");
                self.feedback.card_line(&format!("The game didn't start: {e}"), false, true);
                self.feedback.close_card_in(20);
            }
        }
    }

    /// The NAS side of Play. `true`: go on and start the game.
    fn check_before_play(&mut self, now: DateTime<Utc>) -> bool {
        let mut outlook = match self.engine.outlook(now) {
            Ok(outlook) => outlook,
            Err(SyncError::Offline) => {
                self.offline = true;
                self.feedback.card_line("The network folder doesn't answer: you play with this PC's progress, sent when it's back", false, true);
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
                    self.feedback.card_ask(format!(
                        "Another PC uses Survivor's Satchel {newer} and this one is {}, so nothing syncs until it's updated. Playing now plays this PC's progress, synced after the update.{update}",
                        about::VERSION
                    ));
                    self.feedback.card_gate();
                    self.pending_play = true;
                    return false;
                }
                self.feedback.card_line(&format!("Not synced: this Satchel ({}) is older than {newer}", about::VERSION), false, true);
                return true;
            }
            Err(e) => {
                log!("check before play failed: {e}");
                self.feedback.card_line(&format!("The network folder couldn't be checked ({e}): you play with this PC's progress"), false, true);
                return true;
            }
        };
        self.offline = false;
        let stale: Vec<Other> = outlook.others.iter().filter(|o| o.stale).cloned().collect();
        if !stale.is_empty() {
            for other in &stale {
                let stopped = format!("{} stopped at {} during a session", other.marker.pc, clock(other.marker.heartbeat));
                match self.engine.take_over_checkpoint(other, now) {
                    Ok(Some(v)) => self.feedback.card_line(&format!("{stopped}; its last checkpoint is now version {}", v.n), true, false),
                    Ok(None) => self.feedback.card_line(&format!("{stopped}; nothing newer was left"), true, false),
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
            self.feedback.card_ask(format!(
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
                self.feedback.card_ask(format!(
                    "This PC's save {why}, so it isn't synced. To carry on from the network folder, cancel and bring back its newest version under Settings → Progress versions."
                ));
                self.pending_play = true;
                return false;
            }
            self.feedback.card_line("Playing with this PC's save as it is; it isn't synced", false, true);
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
                self.feedback.card_line("Network folder checked: you have the newest progress", true, false);
            }
            Decision::Send => match self.engine.send(now, None, None, None) {
                Ok(Some(v)) => {
                    self.feedback.card_line(&format!("Network folder checked; this PC's progress went up first (version {})", v.n), true, false);
                    self.activity(Kind::Sync, &format!("Sent version {}", v.n));
                }
                Ok(None) => self.feedback.card_line("Network folder checked", true, false),
                Err(SyncError::MovedOn) => {
                    let newest = self.newest();
                    return self.split_found(newest);
                }
                Err(e) => {
                    log!("send before play failed: {e}");
                    self.feedback.card_line(&format!("Sending first failed ({e}); you play with this PC's progress"), false, true);
                }
            },
            Decision::BringIn(n) => {
                self.feedback.card_line("Network folder checked", true, false);
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
        self.feedback.card_line("Two PCs moved on separately: pick one in the window", false, true);
        self.feedback.close_card_in(10);
        self.ui.show(Some(0));
        false
    }

    /// Brings version `n` in. `true`: go on.
    fn bring_in(&mut self, n: u64, now: DateTime<Utc>) -> bool {
        self.busy_push(format!("Bringing in version {n}…"));
        let result = self.engine.bring_in(n, now);
        self.feedback.clear_busy();
        match result {
            Ok(BroughtIn { version, safety }) => {
                self.feedback.card_line(&format!("Progress from {} brought in (saved {})", version.pc, when(version.saved)), true, false);
                self.activity(Kind::Sync, &format!("Brought in version {} from {}", version.n, version.pc));
                let kept = if safety.is_some() { " Yours is kept as a safety copy." } else { "" };
                self.notify(&format!("Progress from {} brought in", version.pc), &format!("Saved {}.{kept}", when(version.saved)));
                true
            }
            Err(SyncError::MovedOn) => self.split_found(Some(n)),
            Err(e) => {
                log!("bring-in failed: {e}");
                self.feedback.card_line(&format!("Bringing in failed ({e}); you play with this PC's progress"), false, true);
                true
            }
        }
    }

    fn sync_now(&mut self) {
        if self.engine.shared().is_none() {
            return;
        }
        let now = Utc::now();
        self.busy_push("Checking the network folder…".into());
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
        self.feedback.clear_busy();
        match checked {
            Err(SyncError::Offline) => self.offline = true,
            Err(e) => self.feedback.set_problem(format!("The network folder can't be read: {e}")),
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
                        self.feedback.say(text);
                    }
                    Decision::Send => {
                        let _hold = win::Hold::start();
                        match self.engine.send(now, None, None, None) {
                            Ok(Some(v)) => self.activity(Kind::Sync, &format!("Sent version {}", v.n)),
                            Ok(None) => {}
                            Err(SyncError::MovedOn) => self.split = self.newest(),
                            Err(e) => self.feedback.set_problem(format!("Sending failed: {e}")),
                        }
                    }
                    Decision::BringIn(n) => {
                        if self.tracker.game_running() {
                            self.feedback.warn(format!("Close the game to bring in version {n}."));
                        } else {
                            self.feedback.open_card();
                            self.bring_in(n, now);
                            self.feedback.close_card_in(8);
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
        self.busy_push("Saving your choice…".into());
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
        self.feedback.clear_busy();
        match result {
            Ok(text) => {
                self.split = None;
                self.needs_you_open = false;
                self.needs_note = None;
                self.feedback.clear_problem();
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
                self.feedback.set_problem(why);
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
        self.flush_playtime();
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
            self.feedback.warn("Close the game first: a save is only brought in while the game is closed.".into());
            return;
        }
        let Some(row) = self.lists.rows.get(index).cloned() else { return };
        let now = Utc::now();
        self.busy_push("Bringing it back…".into());
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
        self.feedback.clear_busy();
        match result {
            Ok(text) => {
                self.activity(Kind::Sync, &text);
                self.feedback.say(format!("{text}. Your save from before is kept as a safety copy."));
            }
            Err(SyncError::Offline) => self.offline = true,
            Err(e) => self.feedback.set_problem(format!("It couldn't be brought back: {e}")),
        }
        self.refresh();
    }

    /// Sync's settings changed (on or off, the folder, this PC's name): what was known of the
    /// network folder is forgotten until the next look.
    fn sync_settings_changed(&mut self) {
        self.outlook = None;
        self.split = None;
        self.offline = false;
    }

    /// The first start's sync, the game closed: this PC's save goes up when the network folder
    /// has nothing newer, the folder's newest comes here when this PC has nothing, and when both
    /// sides have progress the normal question is asked.
    fn first_sync(&mut self, now: DateTime<Utc>) {
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
                            Err(e) => self.feedback.set_problem(format!("The first sync failed: {e}")),
                        }
                        if let Some(name) = export {
                            self.activity(Kind::Sync, &format!("Your export {name} is under Settings → Progress versions"));
                        }
                    }
                    Decision::BringIn(n) => {
                        self.feedback.open_card();
                        self.bring_in(n, now);
                        self.feedback.close_card_in(8);
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
                Err(e) => self.feedback.set_problem(format!("The network folder can't be read: {e}")),
            }
        }
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
            self.feedback.set_problem(format!("Sync is paused: {why}. Choose another network folder in Settings."));
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
                        && self.feedback.busy().is_none()
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
                    match outlook.local.as_ref().and_then(|l| l.problem.as_ref()) {
                        Some(why) => self.feedback.set_problem(format!("This PC's save {why}, so it isn't sent. To carry on from the network folder, bring back its newest version under Settings → Progress versions.")),
                        None => self.feedback.clear_problem(),
                    }
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
                    self.feedback.set_problem(format!("Nothing syncs until this Satchel ({}) is updated: another PC uses {newer}.", about::VERSION));
                    self.gated = Some(newer);
                }
                Err(e) => self.feedback.set_problem(format!("The network folder can't be read: {e}")),
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
                    let played = playtime::last_played(&sessions, name);
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
        self.playtime_lists(&local_now);
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
        let tray = if self.split.is_some() || self.feedback.problem().is_some() {
            Tray::NeedsYou
        } else if self.feedback.busy().is_some() {
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
        let progress = shell::progress_line(dot, &banner, sync_on, last.as_deref(), self.lists.pc_count, self.feedback.notice());
        let addons = self.addons_view();
        let addons_line = shell::addons_line(addons.rail);
        let playing = self.tracker.in_session();
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
            (Some(_), Some((why, at))) if at.elapsed() < feedback::NOTICE_FOR => why.clone(),
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
            busy: self.feedback.busy().is_some(),
            sync_on,
            playing,
            rail,
            pill,
            news_version,
            news_blocks,
            news_more,
            pcs: self.lists.pcs.clone(),
            pcs_week: self.playtime.pcs_week(),
            questions,
            recent: self.lists.recent.clone(),
            playtime: self.playtime.view(),
            saves: self.lists.saves.clone(),
            saves_summary: self.lists.saves_summary.clone(),
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

    fn push(&mut self) {
        // A problem newly set is a notice too: it shows at once, ahead of an older one, and the
        // tray's needs-you state keeps it on the rail after its minute.
        if let Some(problem) = self.feedback.problem_untold() {
            self.feedback.warn(problem);
        }
        let (view, shown) = self.build_view();
        self.shown = shown;
        if self.last_view.as_ref() != Some(&view) {
            self.last_view = Some(view.clone());
            self.ui.push(view);
        }
    }

    /// A long operation begins: the window says so now, before it runs.
    fn busy_push(&mut self, text: String) {
        self.feedback.set_busy(text);
        self.push();
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

    /// The progress question's pick refused: why, under its buttons, and on the rail.
    fn refuse_pick(&mut self, why: &str) {
        self.needs_note = Some((why.to_string(), Instant::now()));
        self.feedback.warn(why.to_string());
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

}

/// "released today", from this version's CHANGELOG heading; empty when it has no date.
fn released_text() -> String {
    about::released()
        .map(|day| satchel_core::addons::overview::released_ago((Local::now().date_naive() - day).num_days()))
        .unwrap_or_default()
}
