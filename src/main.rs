// Release builds run without a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about;
mod net;
mod update;
// The window's side stays as it is until its own split: not formatted, and three findings (a doc
// list's indentation, the Play card's tuple type, a `% 3 == 0`) wait for then.
#[rustfmt::skip]
#[allow(clippy::doc_lazy_continuation, clippy::type_complexity, clippy::manual_is_multiple_of)]
mod view;
mod win;
mod worker;

use chrono::Local as LocalTime;
use satchel_core::engine::Engine;
use satchel_core::log;
use satchel_core::paths::Local;
use satchel_core::settings::{Defaults, Settings, Theme};
use satchel_core::state::LocalState;
use slint::ComponentHandle;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Sender};
use worker::Command;

slint::include_modules!();

/// One log file per day in `%APPDATA%\SurvivorsSatchel\logs`, the last 14 kept: every start,
/// exit, update and shutdown is there.
fn start_log(dir: &Path) {
    let _ = fs::create_dir_all(dir);
    let _ = satchel_core::fsx::prune(dir, 14);
    let dir = dir.to_path_buf();
    let guard = Mutex::new(());
    satchel_core::log::set_sink(move |message| {
        let _held = guard.lock();
        let now = LocalTime::now();
        let path = dir.join(format!("satchel-{}.log", now.format("%Y-%m-%d")));
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{} {message}", now.format("%H:%M:%S"));
        }
    });
}

/// This PC's settings; on the very first start they come from the defaults file beside the exe
/// (if any) and from detection, and the first-start panel shows them.
fn load_settings(local: &Local) -> (Settings, bool) {
    match Settings::load(&local.settings()) {
        Ok(Some(settings)) => (settings, false),
        Ok(None) => (first_settings(), true),
        Err(e) => {
            log!("settings.toml unreadable ({e}); kept as settings.toml.broken, first start again");
            let _ = fs::rename(local.settings(), local.settings().with_extension("toml.broken"));
            (first_settings(), true)
        }
    }
}

fn first_settings() -> Settings {
    let mut settings = Settings::default();
    let defaults = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.join("SurvivorsSatchel.defaults.toml")));
    if let Some(path) = defaults {
        match Defaults::load(&path) {
            Ok(Some(defaults)) => {
                log!("defaults read from {}", path.display());
                settings.apply(&defaults);
            }
            Ok(None) => {}
            Err(e) => log!("defaults file unreadable: {e}"),
        }
    }
    settings.pc_name = win::computer_name();
    settings.game_dir = win::detect_game_dir();
    settings.sync.folder = settings.sync.folder.take().map(|f| win::network_path(&f));
    settings
}

/// This PC's memory, in Local AppData; the first start of 0.1.2 moves it there from Roaming.
fn load_state(local: &Local) -> LocalState {
    LocalState::load_or_move(&local.state(), &local.old_state()).unwrap_or_else(|e| {
        // Without its memory this PC asks before replacing anything, so starting fresh is safe.
        log!("state.json unreadable ({e}); kept as state.json.broken, starting fresh");
        for path in [local.state(), local.old_state()] {
            if path.exists() {
                let _ = fs::rename(&path, path.with_extension("json.broken"));
            }
        }
        LocalState::default()
    })
}

fn sender(tx: &Sender<Command>, make: impl Fn() -> Command + 'static) -> impl Fn() + 'static {
    let tx = tx.clone();
    move || {
        let _ = tx.send(make());
    }
}

/// A callback that sends a command naming one addon.
fn with_id(tx: &Sender<Command>, make: fn(String) -> Command) -> impl FnMut(slint::SharedString) + 'static {
    let tx = tx.clone();
    move |id| {
        let _ = tx.send(make(id.into()));
    }
}

/// An addon's Install, Update now or Replace: it shows as under way at once, then goes to the
/// worker, whose answer replaces that.
fn install_with_id(window: &MainWindow, tx: &Sender<Command>, make: fn(String) -> Command) -> impl FnMut(slint::SharedString) + 'static {
    let (tx, weak) = (tx.clone(), window.as_weak());
    move |id| {
        if let Some(window) = weak.upgrade() {
            view::installing(&window, Some(&id));
        }
        let _ = tx.send(make(id.into()));
    }
}

fn main() {
    // The UI thread's COM, for the shell calls it makes (the taskbar's name for each window).
    win::com_init();
    // The shared files record the newest Satchel that wrote them; an older one stays out.
    satchel_core::gate::set_this(about::VERSION);
    let args: Vec<String> = std::env::args().collect();
    let play = args.iter().any(|a| a == "--play");
    let tray_only = args.iter().any(|a| a == "--tray");
    // Started by an update: the Satchel it replaces closes first (one Satchel per PC). A slow last
    // send can hold it up: it gets two minutes to exit, then half a minute more to be gone.
    let replaced = args.iter().position(|a| a == win::AFTER_UPDATE).and_then(|i| args.get(i + 1)?.parse::<u32>().ok());
    if let Some(pid) = replaced {
        win::wait_for_exit(pid, std::time::Duration::from_secs(120));
    }
    let wait = if replaced.is_some() { std::time::Duration::from_secs(30) } else { std::time::Duration::ZERO };
    if !win::first_instance(if play { win::Request::Play } else { win::Request::Show }, wait) {
        return;
    }

    let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) else { return };
    let local_appdata = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| appdata.clone());
    let local = Local { save_dir: appdata.join("wenilla-rogue"), app_dir: appdata.join("SurvivorsSatchel"), local_dir: local_appdata.join("SurvivorsSatchel") };
    start_log(&local.logs_dir());
    std::panic::set_hook(Box::new(|info| log!("panic: {info}")));
    if let Some(pid) = replaced {
        log!("started by an update, in place of process {pid}");
    }

    let (settings, first_start) = load_settings(&local);
    let state = load_state(&local);
    // The colours before the window first shows: a dark PC never sees a light flash.
    let dark = match settings.theme {
        Theme::System => win::apps_use_dark(),
        Theme::Light => false,
        Theme::Dark => true,
    };
    // A window closed maximised reopens maximised.
    let remember_maximised = settings.window.maximised;
    let engine = Engine::new(local, settings, state);

    let window = MainWindow::new().expect("the window could not be created");
    let tray = SatchelTray::new().expect("the tray icon could not be created");
    let card = PlayCard::new().expect("the Play card could not be created");
    window.set_dark(dark);
    card.set_dark(dark);
    let (tx, rx) = mpsc::channel::<Command>();
    view::set_about(&window);
    // Browse's sort at start, in every view: the most downloads first.
    window.set_browse_sort(satchel_core::addons::overview::Sort::DEFAULT.index());

    window.on_play(sender(&tx, || Command::Play));
    window.on_sync_now(sender(&tx, || Command::SyncNow));
    window.on_open_folder(sender(&tx, || Command::OpenFolder));
    window.on_open_game_folder(sender(&tx, || Command::OpenGameFolder));
    window.on_keep_here(sender(&tx, || Command::KeepHere));
    window.on_use_other(sender(&tx, || Command::UseOther));
    window.on_decide_later(sender(&tx, || Command::DecideLater));
    window.on_open_saves_folder(sender(&tx, || Command::OpenSavesFolder));
    {
        let tx = tx.clone();
        window.on_bring_back(move |row| {
            if row >= 0 {
                let _ = tx.send(Command::BringBack(row as usize));
            }
        });
    }
    // Home's Needs-you card: each button is one of today's callbacks; the progress question's
    // Choose… opens the question over the window, where the choice is made.
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_question_action(move |action, id| {
            let id: String = id.into();
            let command = match action {
                0 => Command::ChooseProgress,
                1 => Command::GameSettingsChoice(true),
                2 => Command::GameSettingsChoice(false),
                3 => Command::GameSettingsOwn,
                4 => Command::AddonSettingsChoice(id, true),
                5 => Command::AddonSettingsChoice(id, false),
                6 => {
                    if let Some(window) = weak.upgrade() {
                        view::installing(&window, Some(&id));
                    }
                    // An update that waits for an OK, or an addon another PC brings: the worker
                    // tells the two apart.
                    Command::OkAnyway(id)
                }
                7 => {
                    if let Some(window) = weak.upgrade() {
                        view::show_scan(&window, &id);
                    }
                    Command::ForgeShowScan(id)
                }
                _ => {
                    if let Some(window) = weak.upgrade() {
                        window.set_tab(2);
                    }
                    return;
                }
            };
            let _ = tx.send(command);
        });
    }
    // Settings apply one change at a time, the moment it's made: a switch or the theme on click, a
    // typed field with Enter, the network folder's Browse… once a folder is picked. The worker
    // checks only that row and answers with the form as saved. A click on another control than
    // the field being typed in ends that field's typing: the answer may then put its saved value
    // back.
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_setting_bool(move |key, on| {
            if let Some(window) = weak.upgrade() {
                window.set_focused_field("".into());
            }
            let _ = tx.send(Command::Setting(key.into(), on.to_string()));
        });
    }
    // The theme: its colours and the frame at once; the worker saves it.
    {
        let (tx, weak, card) = (tx.clone(), window.as_weak(), card.as_weak());
        window.on_setting_theme(move |index| {
            let (theme, name) = match index {
                1 => (Theme::Light, "light"),
                2 => (Theme::Dark, "dark"),
                _ => (Theme::System, "system"),
            };
            if let (Some(window), Some(card)) = (weak.upgrade(), card.upgrade()) {
                window.set_focused_field("".into());
                let dark = match theme {
                    Theme::System => win::apps_use_dark(),
                    Theme::Light => false,
                    Theme::Dark => true,
                };
                view::set_theme(&window, &card, dark);
            }
            let _ = tx.send(Command::Setting("theme".into(), name.into()));
        });
    }
    {
        let tx = tx.clone();
        window.on_setting_text(move |key, value| {
            let _ = tx.send(Command::Setting(key.into(), value.into()));
        });
    }
    // The network folder's Browse…: Windows' folder picker, on a thread of its own; the folder
    // picked applies at once.
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_pick_sync_folder(move || {
            let Some(window) = weak.upgrade() else { return };
            window.set_focused_field("".into());
            let owner = win::native_handle(&window.window().window_handle());
            let tx = tx.clone();
            std::thread::spawn(move || {
                if let Some(path) = win::pick_folder(owner, "The network folder for your progress") {
                    let _ = tx.send(Command::Setting("folder".into(), path.display().to_string()));
                }
            });
        });
    }
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_restore_defaults(move || {
            if let Some(window) = weak.upgrade() {
                window.set_focused_field("".into());
            }
            let _ = tx.send(Command::RestoreDefaults);
        });
    }
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_detect_game(move || {
            if let Some(window) = weak.upgrade() {
                window.set_focused_field("".into());
            }
            let _ = tx.send(Command::DetectGame);
        });
    }
    // The first start's Change… and Find it: Windows' folder picker, on a thread of its own.
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_pick_game_folder(move || {
            let Some(window) = weak.upgrade() else { return };
            let owner = win::native_handle(&window.window().window_handle());
            let tx = tx.clone();
            std::thread::spawn(move || {
                if let Some(path) = win::pick_folder(owner, "Where is Azeroth Survivors?") {
                    let _ = tx.send(Command::GameFolderPicked(path));
                }
            });
        });
    }
    window.on_finish_setup(sender(&tx, || Command::FinishSetup));
    // The Addons tab. Rows are named by an id (a record's, a catalog entry's, or `folder:<name>`).
    window.on_addons_check(sender(&tx, || Command::AddonsCheck));
    window.on_addons_update_all(sender(&tx, || Command::AddonsUpdateAll));
    window.on_addons_open_folder(sender(&tx, || Command::AddonsOpenFolder));
    // An install shows as under way at once; the worker's answer replaces that.
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_link_install(move || {
            if let Some(window) = weak.upgrade() {
                view::installing(&window, None);
            }
            let _ = tx.send(Command::LinkInstall);
        });
    }
    window.on_link_clear(sender(&tx, || Command::LinkClear));
    window.on_install_again(with_id(&tx, Command::InstallAgain));
    window.on_whats_new_seen(sender(&tx, || Command::WhatsNewSeen));
    window.on_game_news_seen(sender(&tx, || Command::GameNewsSeen));
    window.on_find_game(sender(&tx, || Command::FindGame));
    window.on_update_now(sender(&tx, || Command::UpdateNow));
    // Check for updates shows "Checking…" at once; the worker's answer replaces that.
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_check_updates(move || {
            if let Some(window) = weak.upgrade() {
                view::checking(&window);
            }
            let _ = tx.send(Command::CheckUpdates);
        });
    }
    window.on_game_settings_own(sender(&tx, || Command::GameSettingsOwn));
    window.on_open_game_site(sender(&tx, || Command::OpenGameSite));
    window.on_open_url(|url| win::open_url(&url));
    {
        let tx = tx.clone();
        window.on_game_settings_choice(move |keep_here| {
            let _ = tx.send(Command::GameSettingsChoice(keep_here));
        });
    }
    {
        let tx = tx.clone();
        window.on_addon_switch(move |id, on| {
            let _ = tx.send(Command::AddonSwitch(id.into(), on));
        });
    }
    {
        let tx = tx.clone();
        window.on_addon_keep(move |id, keep| {
            let _ = tx.send(Command::AddonKeep(id.into(), keep));
        });
    }
    {
        let tx = tx.clone();
        window.on_addon_local_only(move |id, on| {
            let _ = tx.send(Command::AddonLocalOnly(id.into(), on));
        });
    }
    {
        let tx = tx.clone();
        window.on_addon_settings_per_pc(move |id, on| {
            let _ = tx.send(Command::AddonSettingsPerPc(id.into(), on));
        });
    }
    {
        let tx = tx.clone();
        window.on_addon_settings_choice(move |id, keep_here| {
            let _ = tx.send(Command::AddonSettingsChoice(id.into(), keep_here));
        });
    }
    {
        let tx = tx.clone();
        window.on_addon_remove(move |id, keep_settings| {
            let _ = tx.send(Command::AddonRemove(id.into(), keep_settings));
        });
    }
    window.on_addon_update(install_with_id(&window, &tx, Command::AddonUpdate));
    window.on_addon_update_anyway(install_with_id(&window, &tx, Command::AddonUpdateAnyway));
    window.on_addon_back(with_id(&tx, Command::AddonBack));
    window.on_addon_page(with_id(&tx, Command::AddonPage));
    window.on_addon_folder(with_id(&tx, Command::AddonFolder));
    window.on_browse_install(install_with_id(&window, &tx, Command::BrowseInstall));
    window.on_browse_install_anyway(install_with_id(&window, &tx, Command::BrowseInstallAnyway));
    // Browse's panel: the window shows it at once; the worker sends SurvivorsForge's part. Closed,
    // the thumbnails the window held of it go.
    window.on_browse_open(with_id(&tx, Command::BrowseOpen));
    {
        let tx = tx.clone();
        window.on_browse_close(move || {
            view::drop_thumbs("");
            let _ = tx.send(Command::BrowseClose);
        });
    }
    // The open addon's pictures row: how many frames it shows, whose thumbnails the worker asks for.
    {
        let tx = tx.clone();
        window.on_thumbs_shown(move |id, frames| {
            let _ = tx.send(Command::ThumbsShown(id.into(), frames.max(0) as usize));
        });
    }
    window.on_forge_show_scan(with_id(&tx, Command::ForgeShowScan));
    // SurvivorsForge's links in Settings and About: the worker opens only those it knows.
    {
        let tx = tx.clone();
        window.on_open_link(move |url| {
            let _ = tx.send(Command::OpenLink(url.into()));
        });
    }
    window.on_link_lookup(with_id(&tx, Command::LinkLookup));
    {
        let weak = window.as_weak();
        window.on_browse_filter(move || {
            if let Some(window) = weak.upgrade() {
                view::filter_offers(&window);
            }
        });
    }
    // The rail's filters: a box of a group on or off, or every box off.
    {
        let weak = window.as_weak();
        window.on_filter_toggled(move |group, name| {
            if let Some(window) = weak.upgrade() {
                view::filter_toggled(&window, group, &name);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_filters_clear(move || {
            if let Some(window) = weak.upgrade() {
                view::filters_clear(&window);
            }
        });
    }
    // Browse's switch: the window shows the view at once; the worker keeps it in settings.toml.
    window.on_browse_view_changed(with_id(&tx, Command::BrowseView));
    // The Addons tab, or its Installed or Browse page, came into view: their icons.
    {
        let tx = tx.clone();
        window.on_addons_shown(move |page| {
            let _ = tx.send(Command::AddonsShown(page));
        });
    }
    // The picture viewer: the picture held so far goes at once (only the one shown is held), and
    // the worker sends the next one's pixels.
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_picture_show(move |id, index| {
            if let Some(window) = weak.upgrade() {
                window.set_viewer_image(slint::Image::default());
                window.set_viewer_state(0);
                window.set_viewer_index(index);
                window.set_viewer_open(true);
            }
            let _ = tx.send(Command::PictureShow(id.into(), index.max(0) as usize));
        });
    }
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_picture_close(move || {
            if let Some(window) = weak.upgrade() {
                window.set_viewer_open(false);
                window.set_viewer_image(slint::Image::default());
            }
            let _ = tx.send(Command::PictureClose);
        });
    }
    // Versions…
    window.on_versions_open(with_id(&tx, Command::VersionsOpen));
    window.on_versions_close(sender(&tx, || Command::VersionsClose));
    {
        let tx = tx.clone();
        window.on_version_install(move |id, tag, anyway| {
            let _ = tx.send(Command::VersionInstall(id.into(), tag.into(), anyway));
        });
    }
    {
        let tx = tx.clone();
        window.on_version_back(move |id, backup| {
            let _ = tx.send(Command::VersionBack(id.into(), backup.into()));
        });
    }
    window.on_install_again_anyway(with_id(&tx, Command::InstallAgainAnyway));
    window.on_incoming_install_anyway(install_with_id(&window, &tx, Command::IncomingInstallAnyway));
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_import_zip(move || {
            let owner = weak.upgrade().and_then(|w| win::native_handle(&w.window().window_handle()));
            let tx = tx.clone();
            std::thread::spawn(move || {
                if let Some(path) = win::pick_zip(owner) {
                    let _ = tx.send(Command::ImportZip(path));
                }
            });
        });
    }
    // Closing the window only hides it; the Satchel lives on in the tray. Whether it was maximised
    // is remembered, so it reopens so.
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.window().on_close_requested(move || {
            if let Some(window) = weak.upgrade() {
                let _ = tx.send(Command::WindowClosed(window.window().is_maximized()));
            }
            slint::CloseRequestResponse::HideWindow
        });
    }

    tray.on_play(sender(&tx, || Command::Play));
    tray.on_sync_now(sender(&tx, || Command::SyncNow));
    // Exit: a window open maximised is remembered so first, as closing it would.
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        tray.on_quit(move || {
            if let Some(window) = weak.upgrade()
                && window.window().is_visible()
            {
                let _ = tx.send(Command::WindowClosed(window.window().is_maximized()));
            }
            let _ = tx.send(Command::Quit);
        });
    }
    {
        let (weak, maximised) = (window.as_weak(), remember_maximised);
        tray.on_open_tab(move |tab| {
            if let Some(window) = weak.upgrade() {
                if tab >= 0 {
                    window.set_tab(tab);
                }
                let _ = window.show();
                view::place_main(&window, maximised);
                view::after_show(window.window(), window.get_dark());
                win::bring_to_front(&window.window().window_handle());
            }
        });
    }
    card.on_cancel(sender(&tx, || Command::CancelPlay));
    card.on_play_anyway(sender(&tx, || Command::PlayAnyway));
    card.on_update_now(sender(&tx, || Command::UpdateNow));

    // A second start (the desktop shortcut, or the exe again) reaches this one.
    {
        let weak = Mutex::new(window.as_weak());
        let play_tx = Mutex::new(tx.clone());
        let maximised = remember_maximised;
        win::start_message_window(
            move || {
                if let Ok(weak) = weak.lock() {
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        let _ = window.show();
                        view::place_main(&window, maximised);
                        view::after_show(window.window(), window.get_dark());
                        win::bring_to_front(&window.window().window_handle());
                    });
                }
            },
            move || {
                if let Ok(tx) = play_tx.lock() {
                    let _ = tx.send(Command::Play);
                }
            },
        );
    }

    let ui = view::Ui { window: window.as_weak(), tray: tray.as_weak(), card: card.as_weak(), maximised: remember_maximised };
    let worker = worker::Worker::new(engine, ui, first_start, tx.clone());
    std::thread::Builder::new().name("satchel-worker".into()).spawn(move || worker.run(rx)).expect("the worker thread could not start");

    if play && !first_start {
        let _ = tx.send(Command::Play);
    } else if !tray_only && !first_start {
        let _ = window.show();
        view::place_main(&window, remember_maximised);
        view::after_show(window.window(), window.get_dark());
    }
    // Up and running: the Satchel an update replaced can go.
    if replaced.is_some() {
        std::thread::spawn(update::clear_aside);
    }
    if let Err(e) = slint::run_event_loop_until_quit() {
        log!("the event loop ended with an error: {e}");
    }
}
