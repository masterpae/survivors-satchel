//! The worker thread: watches the game, runs the sync engine, tells the UI what to show.
//! Everything slow (the network share above all) happens here, never on the UI thread.

mod addons;
mod feedback;
mod forge;
mod game_news;
mod game_settings;
mod playtime;
mod self_update;
mod settings;
mod share;
mod sync;
mod view_model;

use crate::net::{self, Answer, Job};
use crate::view::Ui;
use crate::win;
use chrono::{DateTime, Utc};
use satchel_core::activity::{self, Kind};
use satchel_core::engine::Engine;
use satchel_core::log;
use satchel_core::playtime::format_minutes;
use satchel_core::session::Event;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

const TICK: Duration = Duration::from_secs(2);
const REFRESH_EVERY: Duration = Duration::from_secs(60);
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

pub struct Worker {
    engine: Engine,
    ui: Ui,
    scanner: win::Scanner,
    away: win::AwayWatch,
    epoch: Instant,
    /// The loop ends after this step (an update started the new Satchel).
    quit: bool,
    sync: sync::SyncState,
    game_settings: game_settings::GameSettingsState,
    feedback: feedback::Feedback,
    settings: settings::SettingsState,
    view: view_model::ViewState,
    last_refresh: Option<Instant>,
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

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
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
            scanner: win::Scanner::new(),
            away: win::AwayWatch::start(),
            epoch: Instant::now(),
            quit: false,
            sync: sync::SyncState::default(),
            game_settings: game_settings::GameSettingsState::default(),
            feedback: feedback::Feedback::default(),
            settings: settings::SettingsState::new(first_start),
            view: view_model::ViewState::default(),
            last_refresh: None,
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
            self.view.push_again();
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
            Command::Play => self.play_sequence(),
            Command::PlayAnyway => self.play_anyway(),
            Command::CancelPlay => self.cancel_play(),
            Command::SyncNow => self.sync_now(),
            Command::BringBack(row) => self.bring_back(row),
            Command::OpenSavesFolder => self.open_saves_folder(),
            Command::OpenFolder => self.open_folder(),
            Command::OpenGameFolder => self.open_game_folder(),
            Command::WindowClosed(maximised) => self.window_closed(maximised),
            Command::ChooseProgress => self.choose_progress(),
            Command::KeepHere => self.resolve(true),
            Command::UseOther => self.resolve(false),
            Command::DecideLater => self.decide_later(),
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
        if let Some(event) = self.sync.session_tick(now_s, running, dir.as_deref()) {
            self.on_event(event);
        }
        if self.sync.game_running() {
            self.watch_save();
        }
        // The marker stays alive for the whole session, the quiet window after a close included.
        if self.sync.in_session() {
            self.heartbeat();
        }
        self.feedback.expire();
        if !self.sync.in_session() && self.feedback.busy().is_none() {
            let unsent = self.sync.offline() || self.engine.state.send_pending;
            if self.engine.shared().is_some() && unsent && self.sync.retry_due() {
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
            Event::Closed { updating } => self.on_game_closed(now, updating),
            Event::Updating => {
                log!("the game's files changed after it closed: an update");
                self.feedback.card_line("The game is updating itself. Waiting for it to restart…", false, false);
            }
            Event::Resumed => {
                log!("the game started again inside the quiet window: the same session goes on");
                self.stamp_game_files();
                if self.feedback.card_open() {
                    self.feedback.card_line("Game restarted after its update", true, false);
                    self.feedback.close_card_in(8);
                }
            }
            Event::Over => self.on_session_over(now),
        }
    }

    /// The game closed: the save on disk is complete and goes out now, and the addons' settings
    /// after it.
    fn on_game_closed(&mut self, now: DateTime<Utc>, updating: bool) {
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

    /// The session is over (the game stayed closed): playtime, the marker and what was sent,
    /// then the addons that waited for the game, then an update that waited for the session.
    fn on_session_over(&mut self, now: DateTime<Utc>) {
        let session = self.playtime.over(self.engine.pc(), now, self.sync.updated());
        log!(
            "session over: {}, {} writes to the save while playing{}",
            format_minutes(session.minutes),
            self.sync.writes(),
            if session.updated { ", the game updated itself" } else { "" }
        );
        self.record_session(&session);
        self.session_over_sync(now, &session);
        self.refresh();
        self.addons_after_session();
        self.update_on_session_over();
    }

    // Play ------------------------------------------------------------------------------------

    /// Play: the network folder checked first (the gate, the other PCs, the decision), then what
    /// the other PCs changed and the addons' updates go in, then the game starts.
    fn play_sequence(&mut self) {
        if self.sync.game_running() {
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
        self.sync.set_pending_play(false);
        // What the other PCs changed, then the addons' updates, go in while the game is still closed.
        self.share_before_play();
        self.addons_before_play();
        match win::launch_game(&dir) {
            Ok(()) => {
                log!("game launched");
                self.sync.launched();
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

}
