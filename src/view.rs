//! What the window, the tray and the Play card show. The worker builds a `View`; `apply` puts it
//! on screen on the UI thread, touching lists only when they changed.

use crate::{
    AddonRow, CardLine, ChangelogBlock, ChangelogEntry, MainWindow, OfferRow, PcTime, PlayCard, RecentLine, RemovedRow, SatchelTray,
    SettingsForm, TrayState, about, win,
};
use satchel_core::changelog::Block;
use satchel_core::activity::Kind;
use slint::{ComponentHandle, ModelRc, SharedString, StandardListViewItem, VecModel};
use std::cell::{Cell, RefCell};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tray {
    Synced,
    Busy,
    Playing,
    NeedsYou,
    Offline,
}

/// A line of "Recent on this PC", with the kind its icon shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Recent {
    pub kind: Kind,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub tray: Tray,
    pub status: String,
    pub banner: String,
    pub busy: bool,
    pub sync_on: bool,
    /// Heading, main line, detail of the two cards on the Play tab.
    pub here: [String; 3],
    pub other: [String; 3],
    pub recent: Vec<Recent>,
    pub saves: Vec<[String; 5]>,
    pub saves_note: String,
    pub today: String,
    pub week: String,
    pub all_time: String,
    pub pc_times: Vec<(String, String, f32)>,
    pub sessions: Vec<String>,
    /// "Needs you": this PC's save, the other one, the other PC's name.
    pub needs_you: Option<[String; 3]>,
    /// The Play card's lines (text, done, warning) and whether it asks "Play anyway?".
    pub card: Option<(Vec<(String, bool, bool)>, bool)>,
    pub setup: Option<Setup>,
    pub addons: AddonsView,
    /// The window's colours, and Windows' own app mode.
    pub dark: bool,
    pub system_dark: bool,
    /// "released today", for About and What's new.
    pub app_released: String,
    /// The Play tab's last line, and whether it is "Updated to …".
    pub version_line: String,
    pub version_new: bool,
    /// The game is in its folder: else Play, Addons, Saves and Playtime say it isn't.
    pub game_found: bool,
    /// Settings' line under "The game's own settings on every PC".
    pub game_settings: GameSettingsLine,
    pub update: UpdateLine,
    /// What About's Check for updates found; empty before a check.
    pub check_result: String,
    /// The check waits for GitHub's answer (a build that knows its repository asks there).
    pub checking: bool,
    /// How many checks the worker has taken up: "Checking…", shown from the click, holds until
    /// this counts that click.
    pub checks: u32,
    /// The version the Play card's Update now puts in place: only on the version check's question.
    pub card_update: String,
}

/// Where the game's own settings stand with the other PCs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameSettingsLine {
    pub text: String,
    /// 0 plain, 1 news, 2 needs a look.
    pub kind: i32,
    /// They changed here and on this PC: the player picks.
    pub ask_pc: String,
    /// This PC's own settings from before it joined, kept on the NAS: "Use them instead".
    pub own: String,
    /// The last answer about them, for a minute.
    pub note: String,
}

/// A newer Satchel: beside `sync\` (Update now), or used by another PC, which keeps this one
/// out of the shared files until it's updated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateLine {
    /// The Play tab's line; empty when nothing is newer.
    pub text: String,
    /// The version Update now puts in place; empty when there's none beside `sync\`.
    pub version: String,
    /// Nothing syncs here until this Satchel is updated.
    pub gated: bool,
}

/// The Addons tab.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AddonsView {
    pub count: usize,
    pub rows: Vec<AddonLine>,
    pub header: String,
    pub update_all: bool,
    pub footer: String,
    /// About the other PCs; empty when addons stay on each PC.
    pub share: String,
    /// About the whole tab: the game folder, the game running, other updaters.
    pub notice: String,
    /// The outcome of the last action, for a minute.
    pub message: String,
    pub offers: Vec<OfferLine>,
    pub browse_note: String,
    pub link: LinkView,
    /// The Removed list, newest first.
    pub removed: Vec<RemovedLine>,
}

/// An addon removed on this PC.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemovedLine {
    pub id: String,
    pub initials: String,
    pub name: String,
    pub badge: String,
    pub badge_kind: i32,
    pub version: String,
    /// "removed 2 days ago, here", "removed yesterday, as on PC 2".
    pub when: String,
    /// Where its settings went.
    pub detail: String,
    pub can_install: bool,
    /// Why it can't come back from here.
    pub note: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AddonLine {
    pub id: String,
    pub initials: String,
    pub name: String,
    pub badge: String,
    /// 0 verified, 1 added by link, 2 from a file, 3 installed by hand.
    pub badge_kind: i32,
    pub author: String,
    pub version: String,
    /// "released 3 days ago", or empty when the date isn't known.
    pub released: String,
    /// The version an update brings, else empty.
    pub update: String,
    pub state: String,
    /// 0 plain, 1 good news (an update), 2 needs a look.
    pub state_kind: i32,
    pub on: bool,
    pub switchable: bool,
    pub problem: String,
    pub problems: String,
    pub notes: String,
    /// "What's new in 3.2.66": the version the notes are for.
    pub notes_title: String,
    pub back_to: String,
    pub pinned: bool,
    pub can_pin: bool,
    pub can_update: bool,
    pub has_page: bool,
    pub replace_with: String,
    /// An update or a replacement is under way.
    pub installing: bool,
    /// About the other PCs: "On every PC", what comes at the next Play, a settings question.
    pub share: String,
    /// 0 plain, 1 news, 2 needs a look.
    pub share_kind: i32,
    /// Addons go to every PC and this one can: its two switches show.
    pub shared: bool,
    pub local_only: bool,
    pub settings_per_pc: bool,
    /// Its settings changed here and on this PC since they last matched: the player picks.
    pub ask_pc: String,
    /// Not here yet: another PC has it, and it comes at the next Play.
    pub incoming: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OfferLine {
    pub id: String,
    pub initials: String,
    pub name: String,
    pub author: String,
    pub summary: String,
    pub group: String,
    pub version: String,
    pub installed: bool,
    /// Its install is under way.
    pub installing: bool,
    pub warning: String,
    pub notes: String,
}

/// "Add from a link": 0 nothing yet, 1 looking, 2 found, 3 failed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkView {
    pub state: i32,
    pub message: String,
    pub owner: String,
    pub release: String,
    pub goes_into: String,
    pub note: String,
    /// The note is the red look-alike warning.
    pub warn: bool,
    /// Its install is under way (only ever shown by the window itself, until the worker's answer).
    pub installing: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Setup {
    /// This PC's progress and the shared folder's, when both exist and differ.
    pub pick: Option<[String; 2]>,
    /// The pre-selected one: the newer.
    pub choice: i32,
}

/// The Settings fields, as the user edits them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Form {
    pub game_dir: String,
    pub sync: bool,
    pub folder: String,
    pub pc_name: String,
    pub start_with_windows: bool,
    pub desktop_shortcut: bool,
    pub notifications: bool,
    pub addons_auto: bool,
    pub addons_shared: bool,
    pub pause_when_away: bool,
    /// 0 System, 1 Light, 2 Dark.
    pub theme: i32,
    pub game_shared: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormView {
    pub form: Form,
    /// The fields as saved: Save lights up only when `form` differs.
    pub saved: Form,
    pub game_status: String,
    pub folder_status: String,
    /// Why the name typed isn't saved (another PC syncing there has it); empty otherwise.
    pub name_status: String,
    pub settings_path: String,
}

/// The worker's handle on the UI: weak references, used through the event loop.
#[derive(Clone)]
pub struct Ui {
    pub window: slint::Weak<MainWindow>,
    pub tray: slint::Weak<SatchelTray>,
    pub card: slint::Weak<PlayCard>,
}

impl Ui {
    pub fn push(&self, view: View) {
        let ui = self.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let (Some(window), Some(tray), Some(card)) = (ui.window.upgrade(), ui.tray.upgrade(), ui.card.upgrade()) {
                apply(&window, &tray, &card, &view);
            }
        });
    }

    /// Overwrites the Settings fields: only on start, after Save or Cancel, and after Find it.
    pub fn push_form(&self, form: FormView) {
        let _ = self.window.upgrade_in_event_loop(move |window| apply_form(&window, &form));
    }

    pub fn show(&self, tab: Option<i32>) {
        let _ = self.window.upgrade_in_event_loop(move |window| {
            if let Some(tab) = tab {
                window.set_tab(tab);
            }
            let _ = window.show();
            after_show(window.window(), window.get_dark());
            win::bring_to_front(&window.window().window_handle());
        });
    }

    pub fn quit(&self) {
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    }
}

thread_local! {
    static SHOWN: RefCell<Option<View>> = const { RefCell::new(None) };
    // How many times Check for updates was clicked (`View::checks` counts those taken up).
    static CHECKS_ASKED: Cell<u32> = const { Cell::new(0) };
}

/// Right after a window is shown (Windows has made it): its title bar in the window's colours,
/// and its name and pin on the taskbar.
pub fn after_show(window: &slint::Window, dark: bool) {
    win::dark_title_bar(&window.window_handle(), dark);
    win::app_identity(&window.window_handle());
}

/// What doesn't change while the Satchel runs: About's version and build, and What's new.
pub fn set_about(window: &MainWindow) {
    window.set_app_version(text(about::VERSION));
    window.set_app_build(text(&about::build()));
    window.set_repo_url(text(&about::repository_url().unwrap_or_default()));
    window.set_issue_url(text(&about::issue_url().unwrap_or_default()));
    let entries: Vec<ChangelogEntry> = about::versions()
        .iter()
        .map(|v| ChangelogEntry {
            version: text(&v.version),
            date: text(&v.date.map(|d| d.format("%-d %b %Y").to_string()).unwrap_or_default()),
            blocks: ModelRc::new(VecModel::from(
                v.blocks
                    .iter()
                    .map(|b| match b {
                        Block::Paragraph(t) => ChangelogBlock { kind: 0, text: text(t) },
                        Block::Heading(t) => ChangelogBlock { kind: 1, text: text(t) },
                        Block::Bullet(t) => ChangelogBlock { kind: 2, text: text(t) },
                        Block::Inner(t) => ChangelogBlock { kind: 3, text: text(t) },
                    })
                    .collect::<Vec<_>>(),
            )),
        })
        .collect();
    window.set_changelog(ModelRc::new(VecModel::from(entries)));
}

fn text(value: &str) -> SharedString {
    value.into()
}

fn strings(values: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(values.iter().map(|v| text(v)).collect::<Vec<_>>()))
}

fn table(rows: &[[String; 5]]) -> ModelRc<ModelRc<StandardListViewItem>> {
    let rows: Vec<ModelRc<StandardListViewItem>> = rows
        .iter()
        .map(|row| ModelRc::new(VecModel::from(row.iter().map(|cell| StandardListViewItem::from(cell.as_str())).collect::<Vec<_>>())))
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// The icon a kind of activity shows (`RecentLine.kind` in the UI).
fn kind_icon(kind: Kind) -> i32 {
    match kind {
        Kind::Sync => 0,
        Kind::Addon => 1,
        Kind::Game => 2,
        Kind::Playtime => 3,
        Kind::Warning => 4,
        Kind::Satchel => 5,
    }
}

fn addon_row(line: &AddonLine) -> AddonRow {
    AddonRow {
        id: text(&line.id),
        initials: text(&line.initials),
        name: text(&line.name),
        badge: text(&line.badge),
        badge_kind: line.badge_kind,
        author: text(&line.author),
        version: text(&line.version),
        released: text(&line.released),
        update: text(&line.update),
        state: text(&line.state),
        state_kind: line.state_kind,
        on: line.on,
        switchable: line.switchable,
        problem: text(&line.problem),
        problems: text(&line.problems),
        notes: text(&line.notes),
        notes_title: text(&line.notes_title),
        back_to: text(&line.back_to),
        pinned: line.pinned,
        can_pin: line.can_pin,
        can_update: line.can_update,
        has_page: line.has_page,
        replace_with: text(&line.replace_with),
        installing: line.installing,
        share: text(&line.share),
        share_kind: line.share_kind,
        shared: line.shared,
        local_only: line.local_only,
        settings_per_pc: line.settings_per_pc,
        ask_pc: text(&line.ask_pc),
        incoming: line.incoming,
    }
}

fn removed_row(line: &RemovedLine) -> RemovedRow {
    RemovedRow {
        id: text(&line.id),
        initials: text(&line.initials),
        name: text(&line.name),
        badge: text(&line.badge),
        badge_kind: line.badge_kind,
        version: text(&line.version),
        when: text(&line.when),
        detail: text(&line.detail),
        can_install: line.can_install,
        note: text(&line.note),
    }
}

fn offer_row(offer: &OfferLine) -> OfferRow {
    OfferRow {
        id: text(&offer.id),
        initials: text(&offer.initials),
        name: text(&offer.name),
        author: text(&offer.author),
        summary: text(&offer.summary),
        group: text(&offer.group),
        version: text(&offer.version),
        installed: offer.installed,
        installing: offer.installing,
        warning: text(&offer.warning),
        notes: text(&offer.notes),
    }
}

/// An install asked for in the window shows as under way at once, before the worker takes it
/// up: `Some(id)` for an addon's Install, Update now or Replace button, `None` for "Add from a
/// link". The worker always answers an install with a fresh view, which shows where it stands.
pub fn installing(window: &MainWindow, id: Option<&str>) {
    SHOWN.with(|shown| {
        let mut shown = shown.borrow_mut();
        let Some(view) = shown.as_mut() else { return };
        match id {
            Some(id) => {
                view.addons.offers.iter_mut().filter(|o| o.id == id).for_each(|o| o.installing = true);
                view.addons.rows.iter_mut().filter(|r| r.id == id).for_each(|r| r.installing = true);
                window.set_addon_rows(ModelRc::new(VecModel::from(view.addons.rows.iter().map(addon_row).collect::<Vec<_>>())));
            }
            None => {
                view.addons.link.installing = true;
                window.set_link_installing(true);
            }
        }
    });
    filter_offers(window);
}

/// About's Check for updates shows "Checking…" the moment it's clicked: the worker may be held
/// up by the network folder for seconds before it takes the check up, and the views it sends
/// until then don't count the check yet, so they leave "Checking…" as it is (`apply`).
pub fn checking(window: &MainWindow) {
    CHECKS_ASKED.set(CHECKS_ASKED.get() + 1);
    window.set_checking(true);
}

/// The groups Browse offers, in the catalog's order.
fn groups(offers: &[OfferLine]) -> Vec<String> {
    let mut groups: Vec<String> = Vec::new();
    for offer in offers {
        if !groups.contains(&offer.group) {
            groups.push(offer.group.clone());
        }
    }
    groups
}

/// Browse's rows for the search text and the group picked (0: all), on the UI thread: typing
/// never waits for the worker.
pub fn filter_offers(window: &MainWindow) {
    let search = window.get_browse_search().to_lowercase();
    let group = window.get_browse_group();
    SHOWN.with(|shown| {
        let shown = shown.borrow();
        let Some(view) = shown.as_ref() else { return };
        let offers = &view.addons.offers;
        let picked = usize::try_from(group).ok().and_then(|g| g.checked_sub(1)).and_then(|g| groups(offers).get(g).cloned());
        let rows: Vec<OfferRow> = offers
            .iter()
            .filter(|o| picked.as_ref().is_none_or(|g| *g == o.group))
            .filter(|o| {
                search.is_empty()
                    || [&o.name, &o.summary, &o.author, &o.group].iter().any(|field| field.to_lowercase().contains(search.as_str()))
            })
            .map(offer_row)
            .collect();
        window.set_offer_rows(ModelRc::new(VecModel::from(rows)));
    });
}

/// How many lines the Play card's text takes: about 44 characters fit on one at its width.
pub fn card_text_lines<'a>(lines: impl Iterator<Item = &'a str>) -> i32 {
    lines.map(|text| text.chars().count().div_ceil(44).max(1) as i32).sum::<i32>().max(1)
}

/// The Play card sits in the bottom-right corner, above the taskbar; it grows with its lines.
fn place(card: &PlayCard) {
    let Some((_, _, right, bottom)) = win::work_area() else { return };
    let scale = card.window().scale_factor();
    let (width, height, margin) = ((380.0 * scale) as i32, (card.get_wanted_height() * scale) as i32, (16.0 * scale) as i32);
    card.window().set_position(slint::PhysicalPosition::new(right - width - margin, bottom - height - margin));
}

fn apply(window: &MainWindow, tray: &SatchelTray, card: &PlayCard, view: &View) {
    SHOWN.with(|shown| {
        let before = shown.borrow_mut().take();
        let old = before.as_ref();

        let state = match view.tray {
            Tray::Synced => TrayState::Synced,
            Tray::Busy => TrayState::Busy,
            Tray::Playing => TrayState::Playing,
            Tray::NeedsYou => TrayState::NeedsYou,
            Tray::Offline => TrayState::Offline,
        };
        tray.set_state(state);
        tray.set_status(text(&view.status));
        tray.set_sync_on(view.sync_on);

        window.set_dark(view.dark);
        card.set_dark(view.dark);
        // Windows draws the title bar: set at the first view (the window may be shown already) and
        // again when the colours or Windows' own mode change.
        if old.is_none_or(|o| o.dark != view.dark || o.system_dark != view.system_dark) {
            win::dark_title_bar(&window.window().window_handle(), view.dark);
        }
        window.set_app_released(text(&view.app_released));
        window.set_version_line(text(&view.version_line));
        window.set_version_new(view.version_new);
        window.set_game_found(view.game_found);
        window.set_game_line(text(&view.game_settings.text));
        window.set_game_line_kind(view.game_settings.kind);
        window.set_game_ask_pc(text(&view.game_settings.ask_pc));
        window.set_game_own(text(&view.game_settings.own));
        window.set_game_note(text(&view.game_settings.note));
        window.set_update_text(text(&view.update.text));
        window.set_update_version(text(&view.update.version));
        window.set_update_gated(view.update.gated);
        window.set_check_result(text(&view.check_result));
        // "Checking…" from the click until the worker has taken that check up and, when it asks
        // GitHub, until the answer.
        window.set_checking(view.checking || CHECKS_ASKED.get() > view.checks);

        window.set_state(state);
        window.set_banner(text(&view.banner));
        window.set_busy(view.busy);
        window.set_sync_on(view.sync_on);
        let [heading, main, detail] = &view.here;
        window.set_here_title(text(heading));
        window.set_here_main(text(main));
        window.set_here_detail(text(detail));
        let [heading, main, detail] = &view.other;
        window.set_other_title(text(heading));
        window.set_other_main(text(main));
        window.set_other_detail(text(detail));
        if old.is_none_or(|o| o.recent != view.recent) {
            let lines: Vec<RecentLine> = view.recent.iter().map(|r| RecentLine { kind: kind_icon(r.kind), text: text(&r.text) }).collect();
            window.set_recent(ModelRc::new(VecModel::from(lines)));
        }
        if old.is_none_or(|o| o.saves != view.saves) {
            window.set_save_rows(table(&view.saves));
            window.set_save_row(-1);
        }
        window.set_saves_note(text(&view.saves_note));
        window.set_today(text(&view.today));
        window.set_week(text(&view.week));
        window.set_all_time(text(&view.all_time));
        if old.is_none_or(|o| o.pc_times != view.pc_times) {
            let rows: Vec<PcTime> = view
                .pc_times
                .iter()
                .map(|(name, time, share)| PcTime { name: text(name), time: text(time), share: *share })
                .collect();
            window.set_pc_times(ModelRc::new(VecModel::from(rows)));
        }
        if old.is_none_or(|o| o.sessions != view.sessions) {
            window.set_sessions(strings(&view.sessions));
        }

        let addons = &view.addons;
        if old.is_none_or(|o| o.addons.rows != addons.rows) {
            window.set_addon_rows(ModelRc::new(VecModel::from(addons.rows.iter().map(addon_row).collect::<Vec<_>>())));
        }
        if old.is_none_or(|o| o.addons.removed != addons.removed) {
            window.set_removed_rows(ModelRc::new(VecModel::from(addons.removed.iter().map(removed_row).collect::<Vec<_>>())));
        }
        window.set_addons_count(addons.count as i32);
        window.set_addons_header(text(&addons.header));
        window.set_addons_share(text(&addons.share));
        window.set_can_update_all(addons.update_all);
        window.set_addons_footer(text(&addons.footer));
        window.set_addons_notice(text(&addons.notice));
        window.set_addons_message(text(&addons.message));
        window.set_browse_note(text(&addons.browse_note));
        let link = &addons.link;
        window.set_link_state(link.state);
        window.set_link_message(text(&link.message));
        window.set_link_owner(text(&link.owner));
        window.set_link_release(text(&link.release));
        window.set_link_goes(text(&link.goes_into));
        window.set_link_note(text(&link.note));
        window.set_link_warn(link.warn);
        window.set_link_installing(link.installing);
        // A lookup that was installed: the field empties and the Installed list shows it.
        if old.is_some_and(|o| o.addons.link.state == 2) && link.state == 0 {
            window.set_link_text("".into());
            window.set_addons_page(0);
        }

        match &view.needs_you {
            Some([here, other, pc]) => {
                window.set_needs_here(text(here));
                window.set_needs_other(text(other));
                window.set_needs_other_pc(text(pc));
                window.set_needs_you(true);
            }
            None => window.set_needs_you(false),
        }

        match &view.setup {
            Some(setup) => {
                if let Some([here, other]) = &setup.pick {
                    window.set_setup_here(text(here));
                    window.set_setup_other(text(other));
                }
                window.set_setup_pick(setup.pick.is_some());
                if old.is_none_or(|o| o.setup.is_none()) {
                    window.set_setup_choice(setup.choice);
                }
                window.set_setup(true);
            }
            None => window.set_setup(false),
        }

        match &view.card {
            Some((lines, asking)) => {
                let lines: Vec<CardLine> =
                    lines.iter().map(|(line, done, warn)| CardLine { text: text(line), done: *done, warn: *warn }).collect();
                card.set_text_lines(card_text_lines(lines.iter().map(|l| l.text.as_str())));
                card.set_lines(ModelRc::new(VecModel::from(lines)));
                card.set_asking(*asking);
                card.set_update_version(text(&view.card_update));
                if old.is_none_or(|o| o.card.is_none()) {
                    let _ = card.show();
                    after_show(card.window(), card.get_dark());
                }
                place(card);
            }
            None => {
                if old.is_some_and(|o| o.card.is_some()) {
                    let _ = card.hide();
                }
            }
        }

        let offers_changed = old.is_none_or(|o| o.addons.offers != addons.offers);
        if offers_changed {
            window.set_browse_groups(strings(&groups(&addons.offers)));
        }
        *shown.borrow_mut() = Some(view.clone());
        if offers_changed {
            filter_offers(window);
        }
    });
}

pub fn apply_form(window: &MainWindow, view: &FormView) {
    let form = &view.form;
    window.set_game_dir(text(&form.game_dir));
    window.set_game_status(text(&view.game_status));
    window.set_sync_switch(form.sync);
    window.set_sync_folder(text(&form.folder));
    window.set_folder_status(text(&view.folder_status));
    window.set_pc_name(text(&form.pc_name));
    window.set_name_status(text(&view.name_status));
    window.set_start_with_windows(form.start_with_windows);
    window.set_desktop_shortcut(form.desktop_shortcut);
    window.set_notifications(form.notifications);
    window.set_addons_auto(form.addons_auto);
    window.set_addons_shared(form.addons_shared);
    window.set_pause_when_away(form.pause_when_away);
    window.set_theme(form.theme);
    window.set_game_shared(form.game_shared);
    window.set_settings_path(text(&view.settings_path));
    let saved = &view.saved;
    window.set_saved_form(SettingsForm {
        game_dir: text(&saved.game_dir),
        sync: saved.sync,
        folder: text(&saved.folder),
        pc_name: text(&saved.pc_name),
        start_with_windows: saved.start_with_windows,
        desktop_shortcut: saved.desktop_shortcut,
        notifications: saved.notifications,
        addons_auto: saved.addons_auto,
        addons_shared: saved.addons_shared,
        pause_when_away: saved.pause_when_away,
        theme: saved.theme,
        game_shared: saved.game_shared,
    });
}

pub fn read_form(window: &MainWindow) -> Form {
    Form {
        game_dir: window.get_game_dir().to_string(),
        sync: window.get_sync_switch(),
        folder: window.get_sync_folder().to_string(),
        pc_name: window.get_pc_name().to_string(),
        start_with_windows: window.get_start_with_windows(),
        desktop_shortcut: window.get_desktop_shortcut(),
        notifications: window.get_notifications(),
        addons_auto: window.get_addons_auto(),
        addons_shared: window.get_addons_shared(),
        pause_when_away: window.get_pause_when_away(),
        theme: window.get_theme(),
        game_shared: window.get_game_shared(),
    }
}
