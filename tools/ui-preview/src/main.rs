//! Renders the Satchel's screens to PPM images with Slint's software renderer (the one the app
//! uses), filled with sample data, so the UI can be looked at without Windows:
//! `cargo run -p ui-preview -- <out dir>`.

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, Model, ModelRc, PhysicalSize, Rgb8Pixel, SharedString, StandardListViewItem, VecModel};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

slint::include_modules!();

type Windows = Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>;

struct Preview {
    windows: Windows,
}

impl Platform for Preview {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.windows.borrow_mut().push(window.clone());
        Ok(window)
    }
}

fn snap(window: &MinimalSoftwareWindow, size: PhysicalSize, path: &Path) {
    window.window().set_size(size);
    let (width, height) = (size.width as usize, size.height as usize);
    let mut pixels = vec![Rgb8Pixel::new(0, 0, 0); width * height];
    // Two passes: the std widgets start a colour fade (150–200 ms) when they are next drawn, so the
    // first pass starts it and the second shows where it ends.
    for pass in 0..2 {
        if pass == 1 {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width);
        });
    }
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for pixel in &pixels {
        ppm.extend([pixel.r, pixel.g, pixel.b]);
    }
    fs::write(path, ppm).expect("the image could not be written");
    println!("{}", path.display());
}

fn recent(values: &[(i32, &str)]) -> ModelRc<RecentLine> {
    ModelRc::new(VecModel::from(values.iter().map(|(kind, text)| RecentLine { kind: *kind, text: (*text).into() }).collect::<Vec<_>>()))
}

/// An installed addon's row, from its main fields.
fn addon(name: &str, kind: i32, author: &str, version: &str, update: &str, state: (&str, i32), problem: &str) -> AddonRow {
    let badge = ["Verified", "Added by you", "From a file", "Installed by hand"][kind as usize];
    AddonRow {
        id: name.into(),
        initials: satchel_core::addons::overview::initials(name).into(),
        name: name.into(),
        badge: badge.into(),
        badge_kind: kind,
        author: author.into(),
        version: version.into(),
        released: if kind < 2 { "released 6 days ago".into() } else { SharedString::new() },
        update: update.into(),
        state: state.0.into(),
        state_kind: state.1,
        on: true,
        switchable: true,
        problem: problem.into(),
        problems: problem.into(),
        notes: SharedString::new(),
        notes_title: format!("What's new in {}", if update.is_empty() { version } else { update }).into(),
        back_to: SharedString::new(),
        pinned: false,
        can_pin: kind < 2,
        can_update: !update.is_empty(),
        has_page: kind < 2,
        replace_with: SharedString::new(),
        installing: false,
        share: SharedString::new(),
        share_kind: 0,
        shared: false,
        local_only: false,
        settings_per_pc: false,
        ask_pc: SharedString::new(),
        incoming: false,
    }
}

/// The same row with its line about the other PCs (0.3).
fn shared(row: AddonRow, share: &str, kind: i32) -> AddonRow {
    AddonRow { share: share.into(), share_kind: kind, shared: true, ..row }
}

/// Browse, from the catalog this build carries; LuckyOdds and AZS Spell Meter count as installed.
fn offers() -> (Vec<OfferRow>, Vec<SharedString>) {
    use satchel_core::addons::catalog::Catalog;
    let catalog = Catalog::builtin();
    let mut groups: Vec<SharedString> = Vec::new();
    let rows = catalog
        .addons
        .iter()
        .map(|e| {
            if !groups.iter().any(|g| g.as_str() == e.group) {
                groups.push(e.group.as_str().into());
            }
            let installed = ["lucky-odds", "azs-spell-meter", "survivors-dragon-ui"].contains(&e.id.as_str());
            let warning = if e.id == "keijin-ui" { "doesn't mix with SurvivorsDragonUi, which you have" } else { "" };
            OfferRow {
                id: e.id.as_str().into(),
                initials: satchel_core::addons::overview::initials(&e.name).into(),
                name: e.name.as_str().into(),
                author: e.author.as_str().into(),
                summary: e.summary.as_str().into(),
                group: e.group.as_str().into(),
                version: e.latest.shown_version().into(),
                installed,
                installing: false,
                warning: warning.into(),
                notes: e.latest.notes.clone().unwrap_or_default().chars().take(500).collect::<String>().into(),
            }
        })
        .collect();
    (rows, groups)
}

fn strings(values: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(values.iter().map(|v| SharedString::from(*v)).collect::<Vec<_>>()))
}

fn fill(ui: &MainWindow) {
    ui.set_state(TrayState::Synced);
    ui.set_sync_on(true);
    ui.set_banner("PC 2 has newer progress (version 42). Press Play to bring it in and start.".into());
    ui.set_here_title("This PC · PC 1".into());
    ui.set_here_main("Version 41".into());
    ui.set_here_detail("last played today 13:47".into());
    ui.set_other_title("Network folder · newest".into());
    ui.set_other_main("Version 42".into());
    ui.set_other_detail("sent today 14:30, from PC 2".into());
    ui.set_recent(recent(&[
        (0, "today 13:47  Sent version 41 · session 1 h 35 m"),
        (1, "today 13:45  AZS Spell Meter 1.32 → 1.34"),
        (0, "today 12:12  Brought in version 40 from PC 2"),
        (2, "yesterday 17:47  Played · session 2 h 27 m · the game updated itself, still one session"),
        (4, "yesterday 17:40  RunPace wasn't downloaded: GitHub answered 503"),
    ]));
    let rows = [
        ["42", "today 14:30", "PC 2", "7.9 KB", "newest"],
        ["41", "today 13:47", "PC 1", "7.8 KB", "this PC"],
        ["40", "yesterday 17:47", "PC 1", "7.5 KB", ""],
        ["39", "27 Oct 22:06", "Laptop", "7.4 KB", "checkpoint: Laptop stopped during a session"],
        ["–", "27 Oct 22:06", "–", "7.0 KB", "your export"],
    ];
    let rows: Vec<ModelRc<StandardListViewItem>> = rows
        .iter()
        .map(|row| ModelRc::new(VecModel::from(row.iter().map(|c| StandardListViewItem::from(*c)).collect::<Vec<_>>())))
        .collect();
    ui.set_save_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_saves_note("Every version in the network folder, newest first, plus your own exports. Bringing one in makes it the newest; your save from before is kept as a safety copy.".into());
    ui.set_today("1 h 35 m".into());
    ui.set_week("12 h 05 m".into());
    ui.set_all_time("41 h 10 m".into());
    let pcs = [("PC 1", "6 h 10 m", 1.0), ("PC 2", "4 h 35 m", 0.74), ("Laptop", "1 h 20 m", 0.22)];
    ui.set_pc_times(ModelRc::new(VecModel::from(
        pcs.iter().map(|(n, t, s)| PcTime { name: (*n).into(), time: (*t).into(), share: *s }).collect::<Vec<_>>(),
    )));
    ui.set_sessions(strings(&[
        "today 12:12 PC 1 · 1 h 35 m",
        "yesterday 20:10 PC 2 · 3 h 55 m",
        "yesterday 15:20 PC 1 · 2 h 27 m · game updated, still one session",
    ]));
    ui.set_game_dir(r"C:\games\AzerothSurvivors".into());
    ui.set_game_status("✓ Game found".into());
    ui.set_sync_switch(true);
    ui.set_sync_folder(r"\\homenas\games\Azeroth Survivors Saves".into());
    ui.set_folder_status("✓ Versions go in its sync subfolder".into());
    ui.set_pc_name("PC 1".into());
    ui.set_start_with_windows(true);
    ui.set_desktop_shortcut(true);
    ui.set_notifications(true);
    ui.set_addons_auto(true);
    ui.set_addons_shared(true);

    let rows = vec![
        addon("AZS Spell Meter", 0, "kex0", "1.32", "1.34", ("Goes in when you press Play", 1), ""),
        AddonRow {
            notes: "Boss warnings, run records with personal bests, and pointers to downed party members.\n\n## New\n- Boss warnings: a bar before each big hit".into(),
            back_to: "0.5.2".into(),
            ..addon("LuckyOdds", 0, "cadoky", "0.6.0", "0.7.0", ("Goes in when you press Play", 1), "")
        },
        addon("RunPace", 1, "Hearts47", "1.1.0", "1.2.0", ("Asks first: nobody reviewed it", 1), ""),
        addon("SurvivorsDragonUi", 0, "migwynkriid13", "3.2.57", "3.2.64", ("Goes in when you press Play", 1), ""),
        AddonRow {
            replace_with: "4.5.0".into(),
            ..addon(
                "KeijinUI",
                3,
                "KeijinDE",
                "4.4.1",
                "",
                ("Not one of its author's releases: replace it with 4.5.0 to get updates", 2),
                "doesn't mix with SurvivorsDragonUi (its author says so); turn one off",
            )
        },
    ];
    ui.set_addon_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_addons_count(5);
    ui.set_addons_header("4 updates · 3 go in when you press Play, 1 asks first".into());
    ui.set_can_update_all(true);
    ui.set_addons_footer("3 addons matched their GitHub releases exactly: AZS Spell Meter 1.32, LuckyOdds 0.6.0, SurvivorsDragonUi 3.2.57. New releases checked 12 minutes ago.".into());
    let (offers, groups) = offers();
    ui.set_offer_rows(ModelRc::new(VecModel::from(offers)));
    ui.set_browse_groups(ModelRc::new(VecModel::from(groups)));
    ui.set_browse_note("17 verified addons · the catalog of today 21:27 · each pinned to its author's GitHub".into());
    ui.set_link_text("github.com/Hearts47/RunPace".into());
    ui.set_link_state(2);
    ui.set_link_owner("Hearts47 · 1 star · last change today 09:12".into());
    ui.set_link_release("v1.1.0 · RunPace.zip · licence MIT · checksum matches GitHub's".into());
    ui.set_link_goes("RunPace · nothing left out · kills and XP per minute".into());
    ui.set_link_note("Not reviewed by the game's devs. Addon code can't reach your PC, and it asks before each update.".into());
    ui.set_settings_path(r"C:\Users\Player\AppData\Roaming\SurvivorsSatchel\settings.toml".into());
    ui.set_needs_here("today 13:47 · 7.8 KB".into());
    ui.set_needs_other("today 14:30 · 7.9 KB".into());
    ui.set_needs_other_pc("PC 2".into());
    ui.set_setup_here("Last played today 13:47 · 7.8 KB · newer".into());
    ui.set_setup_other("AzerothSurvivors 2026-10-28 22-05-36.sav · saved 28 Oct 22:05 · 7.8 KB".into());
}

/// What's new from the real CHANGELOG, as the app reads it.
fn changelog() -> ModelRc<ChangelogEntry> {
    use satchel_core::changelog::{Block, versions};
    let entries: Vec<ChangelogEntry> = versions(include_str!("../../../CHANGELOG.md"))
        .iter()
        .map(|v| ChangelogEntry {
            version: v.version.as_str().into(),
            date: v.date.map(|d| d.format("%-d %b %Y").to_string()).unwrap_or_default().into(),
            blocks: ModelRc::new(VecModel::from(
                v.blocks
                    .iter()
                    .map(|b| match b {
                        Block::Paragraph(t) => ChangelogBlock { kind: 0, text: t.as_str().into() },
                        Block::Heading(t) => ChangelogBlock { kind: 1, text: t.as_str().into() },
                        Block::Bullet(t) => ChangelogBlock { kind: 2, text: t.as_str().into() },
                        Block::Inner(t) => ChangelogBlock { kind: 3, text: t.as_str().into() },
                    })
                    .collect::<Vec<_>>(),
            )),
        })
        .collect();
    ModelRc::new(VecModel::from(entries))
}

fn removed(name: &str, kind: i32, badge: &str, version: &str, when: &str, detail: &str, note: &str) -> RemovedRow {
    RemovedRow {
        id: name.into(),
        initials: name.chars().filter(|c| c.is_uppercase()).take(2).collect::<String>().into(),
        name: name.into(),
        badge: badge.into(),
        badge_kind: kind,
        version: version.into(),
        when: when.into(),
        detail: detail.into(),
        can_install: note.is_empty(),
        note: note.into(),
    }
}

fn card_lines(lines: &[(&str, bool, bool)]) -> ModelRc<CardLine> {
    ModelRc::new(VecModel::from(
        lines.iter().map(|(t, done, warn)| CardLine { text: (*t).into(), done: *done, warn: *warn }).collect::<Vec<_>>(),
    ))
}

fn main() {
    let out = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| "preview".into());
    fs::create_dir_all(&out).expect("the output folder could not be created");
    let windows: Windows = Rc::default();
    slint::platform::set_platform(Box::new(Preview { windows: windows.clone() })).expect("platform");

    let ui = MainWindow::new().expect("window");
    fill(&ui);
    ui.show().expect("show");
    let main = windows.borrow()[0].clone();
    let size = PhysicalSize::new(580, 600);
    for (tab, name) in [(0, "01-play"), (2, "02-saves"), (3, "03-playtime"), (4, "04-settings")] {
        ui.set_tab(tab);
        snap(&main, size, &out.join(format!("{name}.ppm")));
    }
    // The Addons tab: Installed (a row opened), Browse, Add, and the question before a removal.
    ui.set_tab(1);
    snap(&main, size, &out.join("05-addons-installed.ppm"));
    ui.set_addon_open("LuckyOdds".into());
    snap(&main, size, &out.join("05b-addons-row-open.ppm"));
    ui.set_addon_open("".into());
    ui.set_addons_page(1);
    snap(&main, size, &out.join("05c-addons-browse.ppm"));
    ui.set_offer_open("lucky-odds".into());
    ui.set_browse_group(2);
    snap(&main, size, &out.join("05d-addons-browse-group.ppm"));
    ui.set_addons_page(2);
    snap(&main, size, &out.join("05e-addons-add.ppm"));
    ui.set_link_warn(true);
    ui.set_link_note("AZS Spell Meter is a verified addon by kex0. This one is by someone-else: a copy or a look-alike, not the verified one.".into());
    snap(&main, size, &out.join("05f-addons-add-lookalike.ppm"));
    ui.set_addons_page(0);
    ui.set_remove_name("RunPace".into());
    ui.set_remove_id("RunPace".into());
    snap(&main, size, &out.join("05g-addons-remove.ppm"));
    ui.set_remove_id("".into());
    ui.set_tab(1);
    ui.set_addons_message("Close the game first: addons change only while it's closed.".into());
    ui.set_addons_notice("The game is running: changes to addons wait until it's closed.".into());
    snap(&main, size, &out.join("05h-addons-game-running.ppm"));
    ui.set_addons_message("".into());
    ui.set_addons_notice("".into());

    // 0.2.2: an install shows under way at once; one kept at an older version isn't "up to date".
    let rows: Vec<AddonRow> = ui.get_addon_rows().iter().collect();
    let mut updating = rows.clone();
    updating[1].installing = true;
    ui.set_addon_rows(ModelRc::new(VecModel::from(updating)));
    ui.set_addon_open("LuckyOdds".into());
    snap(&main, size, &out.join("05i-addons-updating.ppm"));
    let offers: Vec<OfferRow> = ui.get_offer_rows().iter().collect();
    let mut installing = offers.clone();
    if let Some(offer) = installing.iter_mut().find(|o| !o.installed) {
        offer.installing = true;
    }
    ui.set_offer_rows(ModelRc::new(VecModel::from(installing)));
    ui.set_offer_open("".into());
    ui.set_addons_page(1);
    snap(&main, size, &out.join("05j-addons-browse-installing.ppm"));
    ui.set_offer_rows(ModelRc::new(VecModel::from(offers)));
    ui.set_addons_page(2);
    ui.set_link_warn(false);
    ui.set_link_note("Not reviewed by the game's devs. Addon code can't reach your PC, and it asks before each update.".into());
    ui.set_link_installing(true);
    snap(&main, size, &out.join("05k-addons-add-installing.ppm"));
    ui.set_link_installing(false);
    let kept = vec![
        AddonRow { released: "released 2 days ago".into(), ..addon("AZS Spell Meter", 0, "kex0", "1.34", "", ("", 0), "") },
        AddonRow { released: "released 4 days ago".into(), ..addon("LuckyOdds", 0, "cadoky", "0.7.0", "", ("", 0), "") },
        AddonRow {
            released: "released 9 days ago".into(),
            notes: "Fixes the minimap border on wide screens.\nThe action bars remember their scale.".into(),
            notes_title: "What's new in 3.2.66-survivors".into(),
            back_to: "3.2.66-survivors".into(),
            pinned: true,
            can_update: true,
            ..addon("SurvivorsDragonUi", 0, "migwynkriid13", "3.2.57-survivors", "", ("Kept at this version; 3.2.66-survivors is out", 0), "")
        },
    ];
    ui.set_addon_rows(ModelRc::new(VecModel::from(kept)));
    ui.set_addons_count(3);
    ui.set_addons_header("Up to date, except 1 addon kept at an older version".into());
    ui.set_can_update_all(false);
    ui.set_addon_open("SurvivorsDragonUi".into());
    ui.set_addons_page(0);
    snap(&main, size, &out.join("05l-addons-kept.ppm"));

    // 0.3: the same addons and settings on every PC.
    let every = vec![
        shared(addon("AZS Spell Meter", 0, "kex0", "1.34", "", ("", 0), ""), "On every PC", 0),
        AddonRow {
            state: "From Laptop: it comes here at the next Play".into(),
            state_kind: 1,
            incoming: true,
            switchable: false,
            can_pin: false,
            released: SharedString::new(),
            ..addon("Azeroth Cursor", 0, "Maidenquinn", "1.0.0", "", ("", 0), "")
        },
        shared(addon("LuckyOdds", 0, "cadoky", "0.7.0", "", ("", 0), ""), "Settings from PC 2 come in at the next Play", 1),
        shared(addon("RunPace", 1, "Hearts47", "1.2.0", "", ("", 0), ""), "Only on this PC", 0),
        AddonRow { local_only: true, ..shared(addon("RunPace", 1, "Hearts47", "1.2.0", "", ("", 0), ""), "Only on this PC", 0) },
        shared(addon("SurvivorsDragonUi", 0, "migwynkriid13", "3.2.66-survivors", "", ("", 0), ""), "PC 2 has 3.2.67-survivors: it comes here at the next Play", 1),
    ];
    let every: Vec<AddonRow> = every.into_iter().enumerate().filter(|(i, _)| *i != 3).map(|(_, r)| r).collect();
    ui.set_addon_rows(ModelRc::new(VecModel::from(every.clone())));
    ui.set_addons_count(5);
    ui.set_addons_header("Everything is up to date".into());
    ui.set_addons_share("On every PC · 3 changes from other PCs come in at the next Play".into());
    ui.set_addon_open("".into());
    snap(&main, size, &out.join("05m-addons-every-pc.ppm"));
    ui.set_addon_open("AZS Spell Meter".into());
    snap(&main, size, &out.join("05n-addons-every-pc-open.ppm"));
    let mut asking = every.clone();
    asking[0] = AddonRow { ask_pc: "PC 2".into(), ..shared(addon("AZS Spell Meter", 0, "kex0", "1.34", "", ("", 0), ""), "Its settings changed here and on PC 2 since they last matched: pick one below", 2) };
    ui.set_addon_rows(ModelRc::new(VecModel::from(asking)));
    ui.set_addons_share("On every PC · 3 changes from other PCs come in at the next Play · 1 settings question below".into());
    snap(&main, size, &out.join("05o-addons-settings-question.ppm"));
    ui.set_addon_open("RunPace".into());
    ui.set_addon_rows(ModelRc::new(VecModel::from(every)));
    snap(&main, size, &out.join("05p-addons-only-here.ppm"));
    ui.set_addons_share("".into());

    // 0.4.2: a turned-off addon's update waits until it's on.
    let off = vec![
        AddonRow { released: "released 2 days ago".into(), ..addon("AZS Spell Meter", 0, "kex0", "1.34", "", ("", 0), "") },
        AddonRow { released: "released 4 days ago".into(), ..addon("LuckyOdds", 0, "cadoky", "0.7.0", "", ("", 0), "") },
        AddonRow {
            on: false,
            released: "released 3 days ago".into(),
            ..addon("Survivor UI", 0, "morderous", "3.0.45", "3.0.46", ("Turned off, so it waits: it goes in at the first Play after you turn it on", 1), "")
        },
    ];
    ui.set_addon_rows(ModelRc::new(VecModel::from(off)));
    ui.set_addons_count(3);
    ui.set_addons_header("Up to date, except 1 turned-off addon with a newer version".into());
    ui.set_can_update_all(false);
    ui.set_addon_open("".into());
    snap(&main, size, &out.join("05q-addons-turned-off.ppm"));
    ui.set_addon_open("Survivor UI".into());
    snap(&main, size, &out.join("05r-addons-turned-off-open.ppm"));
    ui.set_addon_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_addons_count(5);
    ui.set_addons_header("4 updates · 3 go in when you press Play, 1 asks first".into());
    ui.set_can_update_all(true);
    ui.set_addon_open("".into());
    ui.set_tab(0);
    ui.set_needs_you(true);
    snap(&main, size, &out.join("06-needs-you.ppm"));
    ui.set_needs_you(false);
    ui.set_setup(true);
    ui.set_setup_pick(true);
    snap(&main, size, &out.join("08-first-start.ppm"));
    ui.set_setup(false);
    ui.set_state(TrayState::Offline);
    ui.set_banner("Network folder unreachable: your progress is safe on this PC and goes up when the network folder is back.".into());
    ui.set_other_main("Not answering".into());
    ui.set_other_detail("retried every few minutes".into());
    snap(&main, size, &out.join("09-play-offline.ppm"));
    ui.set_state(TrayState::Synced);
    ui.set_sync_on(false);
    ui.set_banner("Ready · a copy of your progress is kept after each session".into());
    ui.set_here_main("today 13:47".into());
    ui.set_here_detail("last played".into());
    ui.set_other_title("Last copy kept".into());
    ui.set_other_main("today 13:47".into());
    ui.set_other_detail("a copy is kept after each session".into());
    snap(&main, size, &out.join("10-play-one-pc.ppm"));

    // 0.1.2: a shared folder that can't be used, and a damaged save.
    ui.set_sync_on(true);
    ui.set_tab(4);
    ui.set_sync_folder(r"C:\Users\Player\AppData\Roaming\wenilla-rogue\Saves".into());
    ui.set_folder_status("✗ Can't be used: the network folder is inside the game's progress folder".into());
    snap(&main, size, &out.join("13-settings-refused.ppm"));
    ui.set_sync_folder(r"\\homenas\games\Azeroth Survivors Saves".into());
    ui.set_folder_status("✓ Versions go in its sync subfolder".into());
    ui.set_pause_when_away(true);
    snap(&main, size, &out.join("13b-settings-every-pc-pause.ppm"));
    ui.set_pause_when_away(false);
    // 0.3.1: Save lights up only when something changed.
    ui.set_saved_form(SettingsForm {
        game_dir: ui.get_game_dir(),
        sync: ui.get_sync_switch(),
        folder: ui.get_sync_folder(),
        pc_name: ui.get_pc_name(),
        start_with_windows: ui.get_start_with_windows(),
        desktop_shortcut: ui.get_desktop_shortcut(),
        notifications: ui.get_notifications(),
        addons_auto: ui.get_addons_auto(),
        addons_shared: ui.get_addons_shared(),
        pause_when_away: ui.get_pause_when_away(),
        theme: ui.get_theme(),
        game_shared: ui.get_game_shared(),
    });
    snap(&main, size, &out.join("13c-settings-nothing-to-save.ppm"));
    // 0.5.1: a name another PC syncing there already has isn't saved.
    ui.set_pc_name("PC 2".into());
    ui.set_name_status("✗ Another PC syncing here is already called PC 2; give this one a name of its own".into());
    snap(&main, size, &out.join("13d-settings-name-taken.ppm"));
    ui.set_pc_name("PC 1".into());
    ui.set_name_status("".into());
    ui.set_tab(0);
    ui.set_state(TrayState::NeedsYou);
    ui.set_banner(
        "This PC's save is empty, so it isn't sent. To carry on from the network folder, bring back its newest version in the Saves tab."
            .into(),
    );
    snap(&main, size, &out.join("14-play-damaged.ppm"));

    // 0.4: the version line, What's new, Removed, the game's own settings, About, no game, dark.
    ui.set_state(TrayState::Synced);
    ui.set_banner("Synced: version 42 is on this PC and in the network folder.".into());
    ui.set_app_version("0.4.0".into());
    ui.set_app_released("released today".into());
    ui.set_app_build("Built from 1b9cab0".into());
    ui.set_version_line("Updated to 0.4.0 · see what's new".into());
    ui.set_version_new(true);
    snap(&main, size, &out.join("18-play-updated.ppm"));
    ui.set_version_line("Survivor's Satchel 0.4.0 · released today".into());
    ui.set_version_new(false);
    snap(&main, size, &out.join("18b-play-version.ppm"));
    ui.set_changelog(changelog());
    ui.set_changelog_open("0.4.0".into());
    ui.set_whats_new_open(true);
    snap(&main, size, &out.join("19-whats-new.ppm"));
    ui.set_changelog_open("0.3.0".into());
    snap(&main, size, &out.join("19b-whats-new-older.ppm"));
    ui.set_whats_new_open(false);
    ui.set_tab(1);
    ui.set_addons_page(3);
    snap(&main, size, &out.join("20-addons-removed-empty.ppm"));
    ui.set_removed_rows(ModelRc::new(VecModel::from(vec![
        removed("Azeroth Cursor", 0, "Verified", "1.0.0", "Removed today 10:24", "Its settings stayed in the game's folder", ""),
        removed("Big Cursor", 1, "Added by you", "0.3.4", "Removed yesterday 23:10, as on PC 2", "Its settings went with it; they come back with it", ""),
        removed("Old Meter", 2, "From a file", "2.1", "Removed 27 Oct 18:02", "Its settings stayed in the game's folder", "It came from a zip file: import the zip again"),
    ])));
    snap(&main, size, &out.join("20b-addons-removed.ppm"));
    ui.set_tab(4);
    ui.set_game_shared(true);
    ui.set_game_line("The same on every PC · version 3 from PC 2".into());
    ui.set_game_line_kind(0);
    snap(&main, size, &out.join("21-settings-game.ppm"));
    ui.set_game_line("They changed here and on PC 2 since they last matched: pick one. This PC's stay until you do.".into());
    ui.set_game_line_kind(2);
    ui.set_game_ask_pc("PC 2".into());
    // 0.5.1: About's Check for updates has answered.
    ui.set_app_version("0.5.1".into());
    ui.set_check_result("Checked: you have the newest version (0.5.1)".into());
    // Tall enough for every field, About at the foot.
    snap(&main, PhysicalSize::new(580, 1360), &out.join("21b-settings-whole-game-question.ppm"));
    ui.set_app_version("0.4.0".into());
    ui.set_check_result("".into());
    ui.set_game_ask_pc("".into());
    ui.set_game_line("The same on every PC · version 3 from PC 2".into());
    ui.set_game_line_kind(0);
    ui.set_game_found(false);
    for (tab, name) in [(0, "22-no-game-play"), (1, "22b-no-game-addons")] {
        ui.set_tab(tab);
        snap(&main, size, &out.join(format!("{name}.ppm")));
    }
    ui.set_game_found(true);
    ui.set_dark(true);
    ui.set_addons_page(0);
    for (tab, name) in [(0, "23-dark-play"), (1, "23b-dark-addons"), (4, "23c-dark-settings")] {
        ui.set_tab(tab);
        snap(&main, size, &out.join(format!("{name}.ppm")));
    }
    ui.set_tab(0);
    ui.set_state(TrayState::NeedsYou);
    ui.set_banner("This PC's save is empty, so it isn't sent. To carry on from the network folder, bring back its newest version in the Saves tab.".into());
    snap(&main, size, &out.join("23d-dark-play-needs-you.ppm"));
    ui.set_state(TrayState::Synced);
    ui.set_whats_new_open(true);
    snap(&main, size, &out.join("23e-dark-whats-new.ppm"));
    ui.set_whats_new_open(false);
    ui.set_dark(false);

    // A newer Satchel downloaded from GitHub (Update now), the version check, Settings in groups.
    ui.set_tab(0);
    ui.set_app_version("0.4.1".into());
    ui.set_app_build("Built from 0a1b2c3".into());
    ui.set_version_line("Survivor's Satchel 0.4.1 · released today".into());
    ui.set_update_text("Survivor's Satchel 0.4.2 is ready, downloaded from GitHub.".into());
    ui.set_update_version("0.4.2".into());
    snap(&main, size, &out.join("25-play-update-ready.ppm"));
    ui.set_update_text("Nothing syncs until this Satchel is updated: another PC uses 0.4.2.".into());
    ui.set_update_gated(true);
    snap(&main, size, &out.join("25b-play-update-gated.ppm"));
    ui.set_tab(4);
    ui.set_game_line("The same on every PC · Laptop's, version 32".into());
    snap(&main, size, &out.join("26-settings-groups.ppm"));
    ui.set_game_own("This PC's own, from before it joined, are kept as version 31".into());
    ui.set_game_note("The game's settings came from Laptop".into());
    // Tall enough for every group, the About card at the foot.
    snap(&main, PhysicalSize::new(580, 1500), &out.join("26b-settings-whole.ppm"));
    ui.set_dark(true);
    snap(&main, PhysicalSize::new(580, 1500), &out.join("26c-dark-settings-whole.ppm"));
    ui.set_dark(false);
    ui.set_game_own("".into());
    ui.set_game_note("".into());
    ui.set_update_text("".into());
    ui.set_update_version("".into());
    ui.set_update_gated(false);

    // 0.6.0: About's Check for updates under way ("Checking…" where the button was, until the
    // answer), and Report a problem beside the link to the repository. Only a build that knows
    // its repository shows those two: a stand-in one here.
    ui.set_app_version("0.6.0".into());
    ui.set_repo_url("https://github.com/owner/survivors-satchel".into());
    ui.set_issue_url("https://github.com/owner/survivors-satchel/issues/new?template=bug.yml&version=0.6.0".into());
    for (dark, checking, name) in [
        (false, true, "28-settings-about-checking"),
        (false, false, "28b-settings-about-links"),
        (true, true, "28c-dark-settings-about-checking"),
        (true, false, "28d-dark-settings-about-links"),
    ] {
        ui.set_dark(dark);
        ui.set_checking(checking);
        // The worker's line while GitHub is asked; the window shows only the answer.
        let result = if checking { "Checking GitHub…" } else { "Checked: you have the newest version (0.6.0)" };
        ui.set_check_result(result.into());
        // Tall enough for every group, the About card at the foot.
        snap(&main, PhysicalSize::new(580, 1500), &out.join(format!("{name}.ppm")));
    }
    ui.set_dark(false);
    ui.set_checking(false);
    ui.set_check_result("".into());
    ui.set_repo_url("".into());
    ui.set_issue_url("".into());
    ui.set_app_version("0.4.1".into());

    let card = PlayCard::new().expect("card");
    card.set_lines(card_lines(&[
        ("Network folder checked", true, false),
        ("Progress from PC 2 brought in (saved today 00:05)", true, false),
        ("The game is updating itself. Waiting for it to restart…", false, false),
    ]));
    card.show().expect("show card");
    let card_window = windows.borrow()[1].clone();
    // The app counts the card's wrapped lines (about 44 characters each); so does the preview.
    let card_size = |card: &PlayCard| {
        let texts: Vec<String> = (0..card.get_lines().row_count()).filter_map(|i| card.get_lines().row_data(i)).map(|l| l.text.to_string()).collect();
        let lines: i32 = texts.iter().map(|t| t.chars().count().div_ceil(44).max(1) as i32).sum::<i32>().max(1);
        card.set_text_lines(lines);
        PhysicalSize::new(380, card.get_wanted_height() as u32)
    };
    snap(&card_window, card_size(&card), &out.join("11-play-card.ppm"));
    card.set_lines(card_lines(&[(
        "The game looks open on PC 2: started 14:02, still active a minute ago. If you play here too, the two will split and you'll pick one afterwards.",
        false,
        true,
    )]));
    card.set_asking(true);
    snap(&card_window, card_size(&card), &out.join("12-play-card-ask.ppm"));
    card.set_lines(card_lines(&[(
        "This PC's save is cut short, so it isn't synced. To carry on from the network folder, cancel and bring back its newest version in the Saves tab.",
        false,
        true,
    )]));
    snap(&card_window, card_size(&card), &out.join("15-play-card-damaged.ppm"));
    // 0.2: the addons' updates ride along with Play.
    card.set_asking(false);
    card.set_lines(card_lines(&[
        ("Network folder checked: you have the newest progress", true, false),
        ("AZS Spell Meter 1.32 → 1.34", true, false),
        ("LuckyOdds 0.6.0 → 0.7.0", true, false),
        ("SurvivorsDragonUi 3.2.64 is still downloading; it goes in next time", false, false),
        ("Game started", true, false),
    ]));
    snap(&card_window, card_size(&card), &out.join("16-play-card-addons.ppm"));
    // 0.3: what the other PCs changed comes in at Play.
    card.set_lines(card_lines(&[
        ("Network folder checked: you have the newest progress", true, false),
        ("Azeroth Cursor 1.0.0 installed, as on Laptop", true, false),
        ("Big Cursor removed, as on Laptop: it's in the Recycle Bin; its settings stay for a reinstall", true, false),
        ("LuckyOdds: settings from PC 2", true, false),
        ("AZS Spell Meter's settings changed here and on PC 2: pick one in Addons (this PC's stay for now)", false, true),
        ("Game started", true, false),
    ]));
    snap(&card_window, card_size(&card), &out.join("17-play-card-every-pc.ppm"));
    // 0.4: the game's own settings come at Play too; the card in dark.
    card.set_lines(card_lines(&[
        ("Network folder checked: you have the newest progress", true, false),
        ("The game's settings came from PC 2", true, false),
        ("Game started", true, false),
    ]));
    card.set_dark(true);
    snap(&card_window, card_size(&card), &out.join("24-dark-play-card.ppm"));
    // Another PC uses a newer Satchel; Play asks, and offers the one downloaded from GitHub.
    card.set_dark(false);
    card.set_lines(card_lines(&[(
        "Another PC uses Survivor's Satchel 0.4.2 and this one is 0.4.1, so nothing syncs until it's updated. Playing now plays this PC's progress, synced after the update. Update now puts 0.4.2 here and plays.",
        false,
        true,
    )]));
    card.set_asking(true);
    card.set_update_version("0.4.2".into());
    snap(&card_window, card_size(&card), &out.join("27-play-card-gated.ppm"));
}
