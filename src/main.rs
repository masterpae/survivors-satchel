// Release builds run without a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about;
mod net;
mod update;
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
    let local = Local {
        save_dir: appdata.join("wenilla-rogue"),
        app_dir: appdata.join("SurvivorsSatchel"),
        local_dir: local_appdata.join("SurvivorsSatchel"),
    };
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
    let engine = Engine::new(local, settings, state);

    let window = MainWindow::new().expect("the window could not be created");
    let tray = SatchelTray::new().expect("the tray icon could not be created");
    let card = PlayCard::new().expect("the Play card could not be created");
    window.set_dark(dark);
    card.set_dark(dark);
    let (tx, rx) = mpsc::channel::<Command>();
    view::set_about(&window);

    window.on_play(sender(&tx, || Command::Play));
    window.on_sync_now(sender(&tx, || Command::SyncNow));
    window.on_open_folder(sender(&tx, || Command::OpenFolder));
    window.on_keep_here(sender(&tx, || Command::KeepHere));
    window.on_use_other(sender(&tx, || Command::UseOther));
    window.on_decide_later(sender(&tx, || Command::DecideLater));
    window.on_default_settings(sender(&tx, || Command::DefaultSettings));
    {
        let tx = tx.clone();
        window.on_show_in_folder(move |row| {
            if row >= 0 {
                let _ = tx.send(Command::ShowInFolder(row as usize));
            }
        });
    }
    {
        let tx = tx.clone();
        window.on_bring_back(move |row| {
            if row >= 0 {
                let _ = tx.send(Command::BringBack(row as usize));
            }
        });
    }
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_save_settings(move || {
            if let Some(window) = weak.upgrade() {
                let _ = tx.send(Command::SaveSettings(view::read_form(&window)));
            }
        });
    }
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_detect_game(move || {
            if let Some(window) = weak.upgrade() {
                let _ = tx.send(Command::DetectGame(view::read_form(&window)));
            }
        });
    }
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_finish_setup(move || {
            if let Some(window) = weak.upgrade() {
                let _ = tx.send(Command::FinishSetup(view::read_form(&window), window.get_setup_choice()));
            }
        });
    }
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
    {
        let (tx, weak) = (tx.clone(), window.as_weak());
        window.on_find_game(move || {
            if let Some(window) = weak.upgrade() {
                let _ = tx.send(Command::FindGame(view::read_form(&window)));
            }
        });
    }
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
    // Browse's panel: the window shows it at once; the worker sends SurvivorsForge's part.
    window.on_browse_open(with_id(&tx, Command::BrowseOpen));
    window.on_browse_close(sender(&tx, || Command::BrowseClose));
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
    // Closing the window only hides it; the Satchel lives on in the tray.
    window.window().on_close_requested(|| slint::CloseRequestResponse::HideWindow);

    tray.on_play(sender(&tx, || Command::Play));
    tray.on_sync_now(sender(&tx, || Command::SyncNow));
    tray.on_quit(sender(&tx, || Command::Quit));
    {
        let weak = window.as_weak();
        tray.on_open_tab(move |tab| {
            if let Some(window) = weak.upgrade() {
                if tab >= 0 {
                    window.set_tab(tab);
                }
                let _ = window.show();
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
        win::start_message_window(
            move || {
                if let Ok(weak) = weak.lock() {
                    let _ = weak.upgrade_in_event_loop(|window| {
                        let _ = window.show();
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

    let ui = view::Ui { window: window.as_weak(), tray: tray.as_weak(), card: card.as_weak() };
    let worker = worker::Worker::new(engine, ui, first_start, tx.clone());
    std::thread::Builder::new()
        .name("satchel-worker".into())
        .spawn(move || worker.run(rx))
        .expect("the worker thread could not start");

    if play && !first_start {
        let _ = tx.send(Command::Play);
    } else if !tray_only && !first_start {
        let _ = window.show();
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
