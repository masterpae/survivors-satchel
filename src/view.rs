//! What the window, the tray and the Play card show. The worker builds a `View`; `apply` puts it
//! on screen on the UI thread, touching lists only when they changed. Icons and screenshots come
//! apart from it, as pixels the network thread decoded: the window only wraps them
//! (`slint::Image::from_rgba8`), and the lines name an icon by its key.

use crate::{
    AddonRow, BadgeRow, BarRow, CardLine, ChangelogBlock, ChangelogEntry, DayBar, MainWindow, OfferRow, PanelRow, PcRow, PcTime, PictureRow,
    PlayCard, QuestionButton, QuestionRow, RecentLine, RemovedRow, SatchelTray, SaveRow, SessionRow, SettingsForm, TagRow, TrayState,
    VersionItem, VersionsRow, about, win,
};
use satchel_core::activity::Kind;
use satchel_core::addons::forge;
use satchel_core::addons::overview::{self, SafetyCell, Sort, SortKeys, browse_order};
use satchel_core::changelog::Block;
use satchel_core::shell::{self, Action, Dot, Question, RailLine as Rail};
use slint::{ComponentHandle, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tray {
    Synced,
    Busy,
    Playing,
    NeedsYou,
    Offline,
}

/// A line of Home's Recent activity, with the kind its icon shows and when it happened, as the
/// card says it ("2 min ago", "yesterday 21:40").
#[derive(Clone, Debug, PartialEq)]
pub struct Recent {
    pub kind: Kind,
    pub text: String,
    pub when: String,
}

/// The rail's status lines and its last line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RailView {
    /// "Build 0.141.0" with the What's new link (`true`), or the muted line without a build.
    pub build: (String, bool),
    pub progress: Option<Rail>,
    pub addons: Option<Rail>,
    pub last: String,
}

/// The top bar's update pill: its words, its link's words, and what the link does (0 none: no
/// pill; 1 Update now; 2 see what's new).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pill {
    pub text: String,
    pub link: String,
    pub kind: i32,
}

/// One PC in Home's Your PCs card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PcLine {
    pub name: String,
    pub this_pc: bool,
    /// "played today · synced 2 min ago".
    pub line: String,
}

/// Home's playtime card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaytimeView {
    /// "4 h 12 min", and the sessions this week.
    pub week: String,
    pub week_sessions: u32,
    pub today: String,
    pub all: String,
    /// The last seven days: the weekday ("Today" for the last), the bar's share of the tallest
    /// day, the minutes.
    pub days: Vec<(String, f32, u32)>,
    /// This week per PC (sync on): the name, the minutes as words, the share of the most.
    pub per_pc: Vec<(String, String, f32)>,
    /// The last sessions, newest first, 30 at most: the day, the PC, the minutes as words.
    pub sessions: Vec<(String, String, String)>,
}

/// One row of Settings' Progress versions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaveLine {
    pub version: String,
    pub when: String,
    pub from: String,
    pub size: String,
    pub note: String,
    /// The one you play: bold, no button.
    pub current: bool,
    pub can_bring: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub tray: Tray,
    pub status: String,
    pub banner: String,
    pub busy: bool,
    pub sync_on: bool,
    /// The game runs: Play reads "Playing…".
    pub playing: bool,
    pub rail: RailView,
    pub pill: Pill,
    /// Home: the newest note's version, its paragraph and first lines (as What's new shows them)
    /// and whether lines were left out ("More ▾"); the PCs, the questions, the activity, the
    /// playtime.
    pub news_version: String,
    pub news_blocks: Vec<(i32, String)>,
    pub news_more: bool,
    pub pcs: Vec<PcLine>,
    pub pcs_week: String,
    pub questions: Vec<Question>,
    pub recent: Vec<Recent>,
    pub playtime: PlaytimeView,
    /// Settings' Progress versions, newest first, and their summary line.
    pub saves: Vec<SaveLine>,
    pub saves_summary: String,
    pub saves_note: String,
    /// "Needs you" at Play: this PC's save, the other one, the other PC's name; and why the last
    /// pick didn't go through, under its buttons (empty: none).
    pub needs_you: Option<[String; 3]>,
    pub needs_note: String,
    /// The Play card's lines (text, done, warning) and whether it asks "Play anyway?".
    pub card: Option<(Vec<(String, bool, bool)>, bool)>,
    pub setup: Option<Setup>,
    pub addons: AddonsView,
    /// The window's colours, and Windows' own app mode.
    pub dark: bool,
    pub system_dark: bool,
    /// "released today", for About and What's new.
    pub app_released: String,
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
    /// The game's patch notes: the Play tab's line, the panel and About's two answer times.
    pub game_news: GameNewsView,
}

/// The game's patch notes as the window shows them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameNewsView {
    /// The panel hasn't been opened for the newest build yet: the rail's square is filled.
    pub new: bool,
    /// The newest build the game's update server names; empty when none is known.
    pub version: String,
    /// The panel's versions, newest first, 30 at most.
    pub notes: Vec<GameNote>,
    /// "Older notes are in the game's Discord" when there are more than 30; empty otherwise.
    pub footer: String,
    /// What the panel says when there is nothing cached at all; empty otherwise.
    pub empty: String,
    /// About's two lines: when each source last answered, or that it hasn't yet.
    pub latest_answer: String,
    pub notes_answer: String,
}

/// One version in the game's patch notes: its version and date, then its blocks as What's new
/// shows them (0 a paragraph, 1 a heading, 2 a point, 3 a point inside a point, 4 an aside in
/// muted text: "The full notes are on the game's Discord").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameNote {
    pub version: String,
    /// "4 Nov 2026", or empty when no date is known.
    pub date: String,
    pub blocks: Vec<(i32, String)>,
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

/// A newer Satchel: downloaded from GitHub (Update now), or used by another PC, which keeps this
/// one out of the shared files until it's updated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateLine {
    /// The Play tab's line; empty when nothing is newer.
    pub text: String,
    /// The version Update now puts in place; empty when none has been downloaded.
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
    /// The panel's content for the id of the last `BrowseOpen` (None after `BrowseClose`), kept
    /// current as its details, versions and findings come in. The window shows it only while that
    /// id is the open one, so a late answer for a panel since closed does no harm.
    pub panel: Option<PanelLine>,
    /// Browse's status line under the top row, only when there is something to say: "Checking
    /// SurvivorsForge…" (the first request on this PC), "SurvivorsForge isn't answering · last
    /// answer 14:40" (or "· no answer yet on this PC"). Empty when it answers, and with its switch off.
    pub forge_status: String,
    /// Settings' line under the SurvivorsForge switch: "Last answer 14:40", "Isn't answering since
    /// 14:40", "Off".
    pub forge_answer: String,
    /// SurvivorsForge hasn't answered for an hour (switch on): both lines above in amber.
    pub forge_offline: bool,
    /// The day the built-in list was last compared with the game's verified-addons channel, as
    /// shown ("31 Oct 2026"), for the Legend, Settings and About; empty when the catalog doesn't say.
    pub discord_checked: String,
    pub link: LinkView,
    /// The Removed list, newest first.
    pub removed: Vec<RemovedLine>,
    /// Versions…, for the addon it is open for (the last `VersionsOpen`).
    pub versions: Option<VersionsView>,
    /// The Installed tab's state in numbers, for the rail's addons line.
    pub rail: shell::AddonsFacts,
}

/// Versions…: every version of one addon known here, newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersionsView {
    /// The addon's id, as `VersionsOpen` named it.
    pub id: String,
    pub name: String,
    pub rows: Vec<VersionLine>,
    /// SurvivorsForge's versions of it are on their way.
    pub loading: bool,
}

/// One version in Versions….
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersionLine {
    /// "1.34", "v1.1.1".
    pub version: String,
    /// "installed", "installed · kept at this version until you let it update", "kept on 20 Oct",
    /// "pre-release".
    pub marks: String,
    /// Its date, downloads, size, the game range and what the scan found, joined with " · ".
    pub detail: String,
    /// "SurvivorsForge · verified" and its kind, as on the cards; empty when it says nothing.
    pub badge: String,
    pub badge_kind: i32,
    /// The row's one button: 0 none, 1 Go back to this, 2 Install this version, 3 Install anyway
    /// (SurvivorsForge asks first; `reason` says why), 4 installing.
    pub action: i32,
    /// What the button names: the kept version's backup for 1, the version's tag for 2 and 3.
    pub key: String,
    /// Why it waits for an OK (`reason_kind` 1) or never goes in (2).
    pub reason: String,
    pub reason_kind: i32,
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
    /// SurvivorsForge's word on the version it had, as Install again will judge it: why it waits
    /// for an OK (`reason_kind` 1: its button reads Install anyway) or never comes back (2: none).
    pub reason: String,
    pub reason_kind: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AddonLine {
    pub id: String,
    pub initials: String,
    /// The key of its icon when the window holds it (`push_icon`); empty: the initials.
    pub icon: String,
    pub name: String,
    /// Its one badge, shown when `badges` is empty.
    pub badge: String,
    /// The Initials' colour: 0 Discord Verified, 1 added by link, 2 from a file, 3 installed by
    /// hand, 4 SurvivorsForge only.
    pub badge_kind: i32,
    /// Every badge that applies, in order, with its kind as on `OfferLine::badges`: our badge
    /// ("Discord Verified", "Added by you", …) and "SurvivorsForge · <level>". Shown in place of
    /// `badge`.
    pub badges: Vec<(String, i32)>,
    /// SurvivorsForge's word against its update or its version, as a whole sentence:
    /// "Waits for your OK: SurvivorsForge hasn't scanned this version yet", "SurvivorsForge
    /// blocked it: <reason>. It stays as you have it; no updates.", or for a SurvivorsForge-only
    /// addon while it's silent, "SurvivorsForge isn't answering: no updates until it's back".
    /// Empty when nothing stands in the way. Shown under the state line, so `state` needn't say it.
    pub reason: String,
    /// 0 none; 1 Ask: amber, and the row's update button reads Update anyway (`AddonUpdateAnyway`);
    /// 2 No: red, and neither Update now nor Replace shows.
    pub reason_kind: i32,
    pub author: String,
    pub version: String,
    /// When it was released, under the Released heading: "3 days ago"; empty when not known.
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
    /// The Safety column: SurvivorsForge's level with "SurvivorsForge" under it, else its badge's
    /// word (`overview::safety_cell`).
    pub safety: SafetyCell,
    /// The Status column's one line (`overview::row_status`), and its colour: 0 plain, 1 good
    /// news, 2 needs a look, 3 never. Its opened area has every line whole.
    pub status: String,
    pub status_kind: i32,
}

/// An addon on offer in Browse: one card. The worker sends every offer; Browse searches, filters
/// and sorts them on the UI thread (`filter_offers`) by the keys here, so typing never waits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OfferLine {
    /// What Install, Install anyway and the panel name it by: the catalog id for an addon in our
    /// list, the id its record gets for one only on SurvivorsForge (`forge:<slug>`).
    pub id: String,
    pub initials: String,
    /// The key of its icon when the window holds it (`push_icon`); empty: the initials.
    pub icon: String,
    pub name: String,
    pub author: String,
    /// Two lines on the card: our catalog's summary, or the registry's description for an addon
    /// only on SurvivorsForge.
    pub summary: String,
    /// Every badge that applies, in order, each with its kind (its colour, `Look.badge-ink`):
    /// 0 Discord Verified; 1 added by link, or no longer listed; 2 from a file; 3 by hand;
    /// 4 SurvivorsForge lets it in ("SurvivorsForge · verified", "· scanned"); 5 it waits for the
    /// player's OK ("· flagged", "· unscanned", no longer listed there); 6 never ("· blocked").
    /// Browse shows exactly these: nothing is hard-coded there.
    pub badges: Vec<(String, i32)>,
    /// SurvivorsForge's level as its badge words it ("verified", "flagged"…): the table's Safety
    /// filters (its Safety column shows it, coloured, in `safety`). Empty when there is no badge.
    pub trust: String,
    /// Our group and SurvivorsForge's category ("Meters and HUDs", "Combat"): the filters offer
    /// every tag of every line, and the search looks in them.
    pub tags: Vec<String>,
    /// 0 our list only, 1 both lists, 2 SurvivorsForge only: the filters' list choice.
    pub origin: i32,
    /// The version shown ("3.0.48").
    pub version: String,
    /// Its publish date as the card says it ("today", "3 days ago"); empty unless it is a real
    /// publish date (a release seen only on GitHub's page has none).
    pub date: String,
    /// The Last updated sort: the newest publish time, Unix seconds. `None` sorts last.
    pub sort_date: Option<i64>,
    /// The Popularity sort: downloads over its releases. `None` sorts last.
    pub downloads: Option<u64>,
    /// The count as the card says it ("8,382 downloads"); empty when unknown.
    pub downloads_text: String,
    /// Its button, as its pane has it (`PanelLine::action`): 0 Install; 1 Install anyway (it asks
    /// first: the card and the table's row select it, and its pane says why, with Install anyway);
    /// 2 Installing…; 3 Installed (the table's row: Uninstall); 4 Update; 5 Update anyway (asks
    /// first, as 1). Its install shows as under way at once (`installing`), until the worker's answer.
    pub action: i32,
    /// Why it waits for the player's OK, for actions 1 and 5: "SurvivorsForge hasn't scanned this
    /// version yet". Empty otherwise.
    pub reason: String,
    /// It doesn't mix with an addon this PC has; shown only while it isn't installed.
    pub warning: String,
    /// It is in AddOns: a check mark after its name, and the table's "Installed only".
    pub installed: bool,
    /// Its row in the Installed tab, which Uninstall removes (its record's id, or `folder:<name>`
    /// for a copy installed by hand, `overview::installed_line`), and that row's name, which the
    /// question names. Empty while it isn't installed, and when no row is this addon alone (a zip
    /// pack holding it and others): no Uninstall then.
    pub installed_id: String,
    pub installed_name: String,
    /// It was removed on this PC and isn't installed now: its Install says Install again.
    pub again: bool,
    /// The table's Safety column (`overview::safety_cell`).
    pub safety: SafetyCell,
}

/// The panel Browse slides in for one addon, its content only: the name, author,
/// badges, version and date come from the addon's `OfferLine`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelLine {
    /// The `OfferLine::id` it is for.
    pub id: String,
    /// SurvivorsForge says an admin verified the author: "verified author" beside the name.
    pub verified_author: bool,
    /// About, shown at once: SurvivorsForge's summary, else our catalog's summary.
    pub about: String,
    /// The longer description behind "more" (`forge::about_text`); empty when there is none.
    pub about_more: String,
    /// "What's new in 0.8.0" and the notes; both empty unless the notes belong to the version shown.
    pub notes_title: String,
    pub notes: String,
    /// Downloads per version, newest first: the version ("v0.8.0"), its count, and its share of the
    /// largest count (0 to 1) for the bar's length. Empty: the section hides.
    pub bars: Vec<(String, u64, f32)>,
    /// The scan in one line: "verified: an admin approved this file", "scanned: nothing found",
    /// "1 warning"; empty when SurvivorsForge doesn't list it.
    pub scan: String,
    /// The scan found something not fetched yet: "Show what it found" sends `ForgeShowScan`.
    pub scan_more: bool,
    /// What it found, worst first, five at most: "lua.api_override · BordoUI/ThemeUI.lua, line
    /// 2627: Replaces the game function ChatEdit_SendText".
    pub findings: Vec<String>,
    /// The folders it installs.
    pub folders: Vec<String>,
    /// Its screenshots that may be shown (from SurvivorsForge's host or GitHub's), in order: the
    /// strip's frames. The pictures themselves come only when the viewer opens.
    pub pictures: Vec<PictureLine>,
    /// The button: 0 Install, 1 Install anyway, 2 Installing…, 3 Installed, 4 Update (an installed
    /// addon with a newer version; `AddonUpdate`), 5 Update anyway (`AddonUpdateAnyway`).
    pub action: i32,
    /// Why it waits for the player's OK, shown above the button for 1 and 5.
    pub reason: String,
    /// Details or versions are on their way: a small spinner.
    pub loading: bool,
    /// "SurvivorsForge didn't answer; try again in a minute"; empty otherwise.
    pub error: String,
}

/// One screenshot in the pictures row: its shape, as its details give it, its caption, and the key
/// of its thumbnail once the window holds it (`push_thumb`); empty: the placeholder frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PictureLine {
    pub width: u32,
    pub height: u32,
    pub caption: String,
    pub thumb: String,
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

/// The first start: one screen. `sync_line`: "Sync is on · <folder>" when the defaults file beside
/// the exe turned sync on, or why sync stays off when that folder can't be used (`sync_warn`, in
/// amber); empty otherwise.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Setup {
    pub sync_line: String,
    pub sync_warn: bool,
}

/// The Settings fields, as saved.
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
    /// SurvivorsForge's switch (Settings → Addons → Sources).
    pub addons_forge: bool,
    pub pause_when_away: bool,
    /// 0 System, 1 Light, 2 Dark.
    pub theme: i32,
    pub game_shared: bool,
    /// Browse's view as its switch left it (`list`, `cards`, `table`): not one of Settings' fields.
    pub browse_view: String,
}

/// The change a form answers: its row (`key`, as the window names it), the text the window sent
/// for it, and what came of it: `applied` (its value changed and was written: "✓ Saved"), or the
/// reason it was refused (`why`; nothing was written).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Answered {
    pub key: String,
    pub typed: String,
    pub applied: bool,
    pub why: String,
}

/// Settings as the app has them: the saved fields, the game's and the network folder's status
/// lines, and the change this form answers, if any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormView {
    pub saved: Form,
    pub game_status: String,
    /// The game is in the saved folder (at the first start, the one chosen there).
    pub game_ok: bool,
    pub folder_status: String,
    pub settings_path: String,
    pub answered: Option<Answered>,
}

/// The worker's handle on the UI: weak references, used through the event loop; `maximised`:
/// the window was closed maximised last time, so its first show maximises it.
#[derive(Clone)]
pub struct Ui {
    pub window: slint::Weak<MainWindow>,
    pub tray: slint::Weak<SatchelTray>,
    pub card: slint::Weak<PlayCard>,
    pub maximised: bool,
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

    /// Settings as saved, at start, after each Settings change (applied or refused), after Find
    /// it and at the first start's steps; `apply_form` decides what each field takes of it.
    pub fn push_form(&self, form: FormView) {
        let _ = self.window.upgrade_in_event_loop(move |window| apply_form(&window, &form));
    }

    pub fn show(&self, tab: Option<i32>) {
        let maximised = self.maximised;
        let _ = self.window.upgrade_in_event_loop(move |window| {
            if let Some(tab) = tab {
                window.set_tab(tab);
            }
            let _ = window.show();
            place_main(&window, maximised);
            after_show(window.window(), window.get_dark());
            win::bring_to_front(&window.window().window_handle());
        });
    }

    pub fn quit(&self) {
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    }

    /// An icon decoded on the network thread, kept by the window under `key` for this run: lines
    /// naming that key show it (the next view does) in place of their initials.
    pub fn push_icon(&self, key: String, pixels: SharedPixelBuffer<Rgba8Pixel>) {
        let _ = slint::invoke_from_event_loop(move || {
            ICONS.with(|icons| icons.borrow_mut().insert(key, slint::Image::from_rgba8(pixels)));
        });
    }

    /// A thumbnail of the open addon `id`'s pictures row, decoded on the network thread, kept by
    /// the window under `key` while that addon stays open: its frame shows it (the next view does)
    /// in place of the placeholder. One for an addon no longer open is dropped.
    pub fn push_thumb(&self, id: String, key: String, pixels: SharedPixelBuffer<Rgba8Pixel>) {
        let _ = self.window.upgrade_in_event_loop(move |window| {
            if window.get_browse_panel_id().as_str() != id {
                return;
            }
            THUMBS.with(|thumbs| {
                let mut thumbs = thumbs.borrow_mut();
                if thumbs.0 != id {
                    *thumbs = (id, BTreeMap::new());
                }
                thumbs.1.insert(key, slint::Image::from_rgba8(pixels));
            });
        });
    }

    /// The viewer's screenshot `index`, decoded on the network thread, or `None`: it isn't
    /// available. Shown only while the viewer is still on that picture; the window holds no other.
    pub fn push_shot(&self, index: usize, pixels: Option<SharedPixelBuffer<Rgba8Pixel>>) {
        let _ = self.window.upgrade_in_event_loop(move |window| {
            if !window.get_viewer_open() || window.get_viewer_index() != index as i32 {
                return;
            }
            match pixels {
                Some(pixels) => {
                    window.set_viewer_image(slint::Image::from_rgba8(pixels));
                    window.set_viewer_state(1);
                }
                None => {
                    window.set_viewer_image(slint::Image::default());
                    window.set_viewer_state(2);
                }
            }
        });
    }
}

thread_local! {
    // The icons the network thread decoded, by key, held for this run (at most 128 pixels a side).
    static ICONS: RefCell<BTreeMap<String, slint::Image>> = const { RefCell::new(BTreeMap::new()) };
    // The open addon's thumbnails, by key, with the addon they are of (at most 192 pixels a side):
    // dropped when its pane closes or another addon opens.
    static THUMBS: RefCell<(String, BTreeMap<String, slint::Image>)> = const { RefCell::new((String::new(), BTreeMap::new())) };
    static SHOWN: RefCell<Option<View>> = const { RefCell::new(None) };
    // The panel content the window holds: it is set again only when the content differs.
    static PANEL: RefCell<Option<PanelLine>> = const { RefCell::new(None) };
    // How many times Check for updates was clicked (`View::checks` counts those taken up).
    static CHECKS_ASKED: Cell<u32> = const { Cell::new(0) };
    // The main window has been sized and placed once.
    static PLACED: Cell<bool> = const { Cell::new(false) };
}

/// Right after a window is shown (Windows has made it): its frame in the theme's colours, and its
/// name and pin on the taskbar.
pub fn after_show(window: &slint::Window, dark: bool) {
    win::paint_frame(&window.window_handle(), dark);
    win::app_identity(&window.window_handle());
}

/// The main window's size the first time it shows: 1,280 × 720 centred on the work area, the
/// minimum when that doesn't fit, else maximised; maximised when it was closed so last time.
/// Its smallest size is never larger than the work area (`shell::min_size`), so a maximised window
/// fits a small screen at a high scale. Later shows leave it where it is.
pub fn place_main(window: &MainWindow, maximised: bool) {
    if PLACED.replace(true) {
        return;
    }
    let scale = window.window().scale_factor();
    let open = match win::work_area() {
        Some((left, top, right, bottom)) => {
            let (min_w, min_h) = shell::min_size(right - left, bottom - top, scale);
            window.set_min_w(min_w as f32);
            window.set_min_h(min_h as f32);
            let open = shell::open_size(right - left, bottom - top, scale);
            if let shell::Open::Size(w, h) = open {
                let (pw, ph) = ((w as f32 * scale) as i32, (h as f32 * scale) as i32);
                window.window().set_size(slint::PhysicalSize::new(pw as u32, ph as u32));
                window.window().set_position(slint::PhysicalPosition::new(left + (right - left - pw) / 2, top + (bottom - top - ph) / 2));
            }
            open
        }
        None => shell::Open::Size(shell::DEFAULT_SIZE.0, shell::DEFAULT_SIZE.1),
    };
    if maximised || open == shell::Open::Maximised {
        window.window().set_maximized(true);
    }
}

/// What doesn't change while the Satchel runs: About's version and build, and What's new.
pub fn set_about(window: &MainWindow) {
    window.set_app_version(text(about::VERSION));
    window.set_app_build(text(&about::build()));
    window.set_repo_url(text(&about::repository_url().unwrap_or_default()));
    window.set_issue_url(text(&about::issue_url().unwrap_or_default()));
    // SurvivorsForge's credit links, in Settings and About; the app opens only these two (and
    // GitHub repositories) for `open-link`.
    window.set_forge_repo_url(text(forge::REPO));
    window.set_forge_site_url(text(forge::SITE));
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

/// The number a Needs-you button's action goes by in the window, and the addon it names.
fn question_button(label: &str, action: &Action) -> QuestionButton {
    let (code, id) = match action {
        Action::ChooseProgress => (0, ""),
        Action::GameKeepHere => (1, ""),
        Action::GameUseOther => (2, ""),
        Action::GameUseOwn => (3, ""),
        Action::AddonKeepHere(id) => (4, id.as_str()),
        Action::AddonUseOther(id) => (5, id.as_str()),
        Action::InstallAnyway(id) => (6, id.as_str()),
        Action::ShowScan(id) => (7, id.as_str()),
        Action::OpenSettings => (8, ""),
    };
    QuestionButton { text: text(label), action: code, id: text(id) }
}

fn questions(list: &[Question]) -> ModelRc<QuestionRow> {
    ModelRc::new(VecModel::from(
        list.iter()
            .map(|q| QuestionRow {
                text: text(&q.text),
                buttons: ModelRc::new(VecModel::from(q.buttons.iter().map(|(label, action)| question_button(label, action)).collect::<Vec<_>>())),
            })
            .collect::<Vec<_>>(),
    ))
}

/// A rail dot's number in the window.
fn dot(dot: Dot) -> i32 {
    match dot {
        Dot::Green => 0,
        Dot::Blue => 1,
        Dot::Gold => 2,
        Dot::Amber => 3,
        Dot::Muted => 4,
        Dot::Text => 5,
    }
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

fn badges(values: &[(String, i32)]) -> ModelRc<BadgeRow> {
    ModelRc::new(VecModel::from(values.iter().map(|(t, kind)| BadgeRow { text: text(t), kind: *kind }).collect::<Vec<_>>()))
}

/// The icon the window holds under `key`; none (the initials show) for an empty key or one not
/// here yet.
fn icon(key: &str) -> slint::Image {
    if key.is_empty() {
        return slint::Image::default();
    }
    ICONS.with(|icons| icons.borrow().get(key).cloned().unwrap_or_default())
}

/// The open addon's thumbnail the window holds under `key`; none (the placeholder shows) for an
/// empty key, one not here yet, or one of another addon.
fn thumb(id: &str, key: &str) -> slint::Image {
    if key.is_empty() {
        return slint::Image::default();
    }
    THUMBS.with(|thumbs| {
        let thumbs = thumbs.borrow();
        if thumbs.0 == id { thumbs.1.get(key).cloned().unwrap_or_default() } else { slint::Image::default() }
    })
}

/// The pane closed, or another addon opened: the thumbnails held go (`open` is the addon open
/// now, empty for none).
pub fn drop_thumbs(open: &str) {
    THUMBS.with(|thumbs| {
        let mut thumbs = thumbs.borrow_mut();
        if thumbs.0 != open {
            *thumbs = (String::new(), BTreeMap::new());
        }
    });
}

fn addon_row(line: &AddonLine) -> AddonRow {
    AddonRow {
        id: text(&line.id),
        initials: text(&line.initials),
        icon: icon(&line.icon),
        name: text(&line.name),
        badge: text(&line.badge),
        badge_kind: line.badge_kind,
        badges: badges(&line.badges),
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
        reason: text(&line.reason),
        reason_kind: line.reason_kind,
        safety: text(&line.safety.word),
        safety_kind: line.safety.kind,
        safety_by: text(&line.safety.by),
        status: text(&line.status),
        status_kind: line.status_kind,
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
        reason: text(&line.reason),
        reason_kind: line.reason_kind,
    }
}

/// The card's summary is two lines at most (Slint elides the rest); a SurvivorsForge description
/// can be long, so it is cut here too, at a word.
fn card_summary(summary: &str) -> String {
    const MOST: usize = 160;
    if summary.chars().count() <= MOST {
        return summary.to_string();
    }
    let cut: String = summary.chars().take(MOST).collect();
    let cut = cut.rfind(' ').map_or(cut.as_str(), |at| &cut[..at]);
    format!("{}…", cut.trim_end())
}

fn offer_row(offer: &OfferLine) -> OfferRow {
    // The table's Install column: its button's words, from the action.
    let cell = overview::install_cell(offer.action, offer.again);
    OfferRow {
        id: text(&offer.id),
        initials: text(&offer.initials),
        icon: icon(&offer.icon),
        // The colour of where it comes from: our list's green, SurvivorsForge's violet.
        initials_kind: if offer.origin == ORIGIN_FORGE { 4 } else { 0 },
        name: text(&offer.name),
        author: text(&offer.author),
        downloads: text(&offer.downloads_text),
        summary: text(&card_summary(&offer.summary)),
        badges: badges(&offer.badges),
        version: text(&offer.version),
        date: text(&offer.date),
        action: offer.action,
        reason: text(&offer.reason),
        warning: text(&offer.warning),
        installed: offer.installed,
        installed_id: text(&offer.installed_id),
        installed_name: text(&offer.installed_name),
        // The lists it is on: the pane's Safety line names ours.
        ours: offer.origin != ORIGIN_FORGE,
        forge: offer.origin != ORIGIN_OURS,
        // The table's columns.
        safety: text(&offer.safety.word),
        safety_kind: offer.safety.kind,
        safety_by: text(&offer.safety.by),
        tags: text(&offer.tags.join(" · ")),
        popularity: text(&offer.downloads.map(thousands).unwrap_or_default()),
        install_text: text(cell.text),
        install_primary: cell.primary,
        // A one-word name is cut short with "…" in a narrow column, not broken inside the word.
        one_word: !offer.name.trim().contains(char::is_whitespace),
    }
}

/// 2,870: a count as people read it.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn panel_row(panel: &PanelLine) -> PanelRow {
    let folders = match panel.folders.as_slice() {
        [] => String::new(),
        [one] => format!("Folder: {one}"),
        many => format!("Folders: {}", many.join(", ")),
    };
    let bars: Vec<BarRow> = panel
        .bars
        .iter()
        .map(|(label, count, share)| BarRow { label: text(label), count: text(&thousands(*count)), share: share.clamp(0.0, 1.0) })
        .collect();
    PanelRow {
        id: text(&panel.id),
        verified_author: panel.verified_author,
        about: text(&panel.about),
        about_more: text(&panel.about_more),
        notes_title: text(&panel.notes_title),
        notes: text(&panel.notes),
        bars: ModelRc::new(VecModel::from(bars)),
        scan: text(&panel.scan),
        scan_more: panel.scan_more,
        findings: strings(&panel.findings),
        folders: text(&folders),
        pictures: ModelRc::new(VecModel::from(
            panel
                .pictures
                .iter()
                .map(|p| PictureRow {
                    width: p.width.clamp(1, 1 << 14) as i32,
                    height: p.height.clamp(1, 1 << 14) as i32,
                    caption: text(&p.caption),
                    thumb: thumb(&panel.id, &p.thumb),
                })
                .collect::<Vec<_>>(),
        )),
        action: panel.action,
        reason: text(&panel.reason),
        loading: panel.loading,
        error: text(&panel.error),
    }
}

/// The game's patch notes as the window lists them, in What's new's own shape.
fn game_notes(notes: &[GameNote]) -> ModelRc<ChangelogEntry> {
    let entries: Vec<ChangelogEntry> = notes
        .iter()
        .map(|n| ChangelogEntry {
            version: text(&n.version),
            date: text(&n.date),
            blocks: ModelRc::new(VecModel::from(n.blocks.iter().map(|(kind, t)| ChangelogBlock { kind: *kind, text: text(t) }).collect::<Vec<_>>())),
        })
        .collect();
    ModelRc::new(VecModel::from(entries))
}

/// Versions… as the window lists it.
fn versions_row(versions: &VersionsView) -> VersionsRow {
    let rows: Vec<VersionItem> = versions
        .rows
        .iter()
        .map(|v| VersionItem {
            version: text(&v.version),
            marks: text(&v.marks),
            detail: text(&v.detail),
            badge: text(&v.badge),
            badge_kind: v.badge_kind,
            action: v.action,
            key: text(&v.key),
            reason: text(&v.reason),
            reason_kind: v.reason_kind,
        })
        .collect();
    VersionsRow { id: text(&versions.id), name: text(&versions.name), rows: ModelRc::new(VecModel::from(rows)), loading: versions.loading }
}

/// The panel's content, when it is for the addon open now and the window holds something else.
fn push_panel(window: &MainWindow, panel: Option<&PanelLine>) {
    let open = window.get_browse_panel_id();
    // Another addon opened: the thumbnails held of the one before go.
    drop_thumbs(open.as_str());
    let Some(panel) = panel.filter(|p| !p.id.is_empty() && p.id == open.as_str()) else { return };
    PANEL.with(|held| {
        let mut held = held.borrow_mut();
        if held.as_ref() != Some(panel) {
            window.set_panel(panel_row(panel));
            *held = Some(panel.clone());
        }
    });
}

/// An install asked for in the window shows as under way at once, before the worker takes it
/// up: `Some(id)` for an addon's Install, Install anyway, Update now, Update anyway or Replace
/// button (on its card, in its panel or on its row), `None` for "Add from a link". The worker
/// always answers an install with a fresh view, which shows where it stands.
pub fn installing(window: &MainWindow, id: Option<&str>) {
    SHOWN.with(|shown| {
        let mut shown = shown.borrow_mut();
        let Some(view) = shown.as_mut() else { return };
        match id {
            Some(id) => {
                // Install, Install anyway, Update or Update anyway becomes Installing…; an
                // installed one stays Installed.
                view.addons.offers.iter_mut().filter(|o| o.id == id && matches!(o.action, 0 | 1 | 4 | 5)).for_each(|o| o.action = 2);
                view.addons.rows.iter_mut().filter(|r| r.id == id).for_each(|r| r.installing = true);
                if let Some(panel) = view.addons.panel.as_mut().filter(|p| p.id == id && matches!(p.action, 0 | 1 | 4 | 5)) {
                    panel.action = 2;
                }
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

/// `OfferLine::origin`: our list only, both lists, SurvivorsForge only.
const ORIGIN_OURS: i32 = 0;
const ORIGIN_FORGE: i32 = 2;

/// The window's Addons tab, and its pages.
const TAB_ADDONS: i32 = 1;
const PAGE_BROWSE: i32 = 1;

/// The rail's filter groups, as `filter-toggled` numbers them.
const GROUP_LISTS: i32 = 0;
const GROUP_SAFETY: i32 = 1;
const GROUP_GROUPS: i32 = 2;

/// The two lists, as the Lists group names them.
const LIST_OURS: &str = "Discord Verified";
const LIST_FORGE: &str = "SurvivorsForge";

/// Every group (our catalog's groups and SurvivorsForge's categories) of the lines, once each, A to Z.
fn tags(offers: &[OfferLine]) -> Vec<String> {
    let mut tags: Vec<String> = offers.iter().flat_map(|o| o.tags.iter().cloned()).collect();
    tags.sort_by_key(|t| t.to_lowercase());
    tags.dedup();
    tags
}

/// The boxes ticked in one of the rail's groups.
fn ticked(rows: &ModelRc<TagRow>) -> Vec<String> {
    rows.iter().filter(|t| t.on).map(|t| t.text.to_string()).collect()
}

fn tag_rows(names: &[String], on: &[String]) -> ModelRc<TagRow> {
    ModelRc::new(VecModel::from(names.iter().map(|t| TagRow { text: text(t), on: on.iter().any(|o| o.eq_ignore_ascii_case(t)) }).collect::<Vec<_>>()))
}

/// SurvivorsForge's levels in the order the Legend gives them, for the Safety group.
const LEVELS: [&str; 8] = ["verified", "scanned", "flagged", "unscanned", "no longer listed", "source changed", "another file", "blocked"];

/// When the offers change: the rail's three groups (the boxes still offered keep being on), the
/// counts and the Legend's groups line.
fn set_browse_lists(window: &MainWindow, offers: &[OfferLine]) {
    let lists = ticked(&window.get_filter_lists());
    window.set_filter_lists(tag_rows(&[LIST_OURS.to_string(), LIST_FORGE.to_string()], &lists));
    let safety = ticked(&window.get_filter_safety());
    let levels: Vec<String> = LEVELS.iter().filter(|level| offers.iter().any(|o| o.trust == **level)).map(|l| l.to_string()).collect();
    window.set_filter_safety(tag_rows(&levels, &safety));
    let groups = ticked(&window.get_filter_groups());
    let all = tags(offers);
    window.set_filter_groups(tag_rows(&all, &groups));
    window.set_browse_tag_line(text(&all.join(" · ")));
    count_filters(window);
}

/// The headings' counts and the Clear link.
fn count_filters(window: &MainWindow) {
    let lists = ticked(&window.get_filter_lists()).len() as i32;
    let safety = ticked(&window.get_filter_safety()).len() as i32;
    let groups = ticked(&window.get_filter_groups()).len() as i32;
    window.set_groups_on(groups);
    window.set_filters_on(lists + safety + groups);
}

/// A box of one of the rail's groups turned on or off: the Installed table and Browse filter again.
pub fn filter_toggled(window: &MainWindow, group: i32, name: &str) {
    let flip = |rows: ModelRc<TagRow>| -> ModelRc<TagRow> {
        ModelRc::new(VecModel::from(rows.iter().map(|t| TagRow { on: if t.text == name { !t.on } else { t.on }, ..t }).collect::<Vec<_>>()))
    };
    match group {
        GROUP_LISTS => window.set_filter_lists(flip(window.get_filter_lists())),
        GROUP_SAFETY => window.set_filter_safety(flip(window.get_filter_safety())),
        GROUP_GROUPS => window.set_filter_groups(flip(window.get_filter_groups())),
        _ => {}
    }
    count_filters(window);
    filter_offers(window);
}

/// Clear: every box unticked.
pub fn filters_clear(window: &MainWindow) {
    let clear = |rows: ModelRc<TagRow>| -> ModelRc<TagRow> { ModelRc::new(VecModel::from(rows.iter().map(|t| TagRow { on: false, ..t }).collect::<Vec<_>>())) };
    window.set_filter_lists(clear(window.get_filter_lists()));
    window.set_filter_safety(clear(window.get_filter_safety()));
    window.set_filter_groups(clear(window.get_filter_groups()));
    window.set_browse_installed_only(false);
    count_filters(window);
    filter_offers(window);
}

fn keys(offer: &OfferLine) -> SortKeys<'_> {
    SortKeys { name: &offer.name, id: &offer.id, date: offer.sort_date, downloads: offer.downloads }
}

/// The lists an offer is on, as the Lists group names them.
fn offer_lists(offer: &OfferLine) -> Vec<String> {
    let mut lists = Vec::new();
    if offer.origin != ORIGIN_FORGE {
        lists.push(LIST_OURS.to_string());
    }
    if offer.origin != ORIGIN_OURS {
        lists.push(LIST_FORGE.to_string());
    }
    lists
}

/// The lists an installed addon is on and SurvivorsForge's level for it, from its badges.
fn row_lists_and_level(line: &AddonLine) -> (Vec<String>, Vec<String>) {
    let badges = if line.badges.is_empty() { vec![(line.badge.clone(), line.badge_kind)] } else { line.badges.clone() };
    let mut lists = Vec::new();
    let mut level = Vec::new();
    for (badge, _) in &badges {
        match badge.strip_prefix("SurvivorsForge · ") {
            Some(word) => {
                lists.push(LIST_FORGE.to_string());
                // "verified (v1.2)": the level alone.
                level.push(word.split(" (").next().unwrap_or_default().to_string());
            }
            None if badge == LIST_FORGE => lists.push(LIST_FORGE.to_string()),
            None if badge == LIST_OURS => lists.push(LIST_OURS.to_string()),
            None => {}
        }
    }
    (lists, level)
}

/// Browse's addons for the rail's search (name, summary, author, groups), its filters (within a
/// group any ticked box matches, across groups all must; installed only) and the sort
/// (`browse_order`), on the UI thread: typing never waits for the worker. The Installed table takes
/// the search and the Lists and Safety groups too; Removed the search only. The open addon's card
/// is kept current, and its content shown when the window doesn't hold it yet (opened again); one
/// no longer offered closes. When the selected addon leaves the filtered list, the top row is
/// selected.
pub fn filter_offers(window: &MainWindow) {
    let search = window.get_browse_search().to_lowercase();
    let sort = Sort::from_index(window.get_browse_sort());
    let lists = ticked(&window.get_filter_lists());
    let safety = ticked(&window.get_filter_safety());
    let groups = ticked(&window.get_filter_groups());
    let installed_only = window.get_browse_installed_only();
    let reverse = window.get_browse_view() == "table" && window.get_browse_sort_reverse();
    let hit = |field: &str| field.to_lowercase().contains(search.as_str());
    // Browse entered or the table picked since the last filter: the table's top row may open.
    let fresh = window.get_table_fresh();
    window.set_table_fresh(false);
    SHOWN.with(|shown| {
        let shown = shown.borrow();
        let Some(view) = shown.as_ref() else { return };
        let offers = &view.addons.offers;
        let ticked_groups = vec![lists.clone(), safety.clone(), groups.clone()];
        let mut picked: Vec<&OfferLine> = offers
            .iter()
            .filter(|o| shell::filter_match(&ticked_groups, &[offer_lists(o), vec![o.trust.clone()], o.tags.clone()]))
            .filter(|o| !installed_only || o.installed)
            .filter(|o| search.is_empty() || hit(&o.name) || hit(&o.summary) || hit(&o.author) || o.tags.iter().any(|t| hit(t)))
            .collect();
        picked.sort_by(|a, b| browse_order(&keys(a), &keys(b), sort, reverse));
        // The table on screen selects its top row when the addon selected left what it shows, and,
        // at a fresh look (Browse entered, the table picked), when none is selected either.
        let table = window.get_browse_view() == "table";
        let on_screen = table && window.get_tab() == TAB_ADDONS && window.get_addons_page() == PAGE_BROWSE;
        let pick = if on_screen {
            let ids: Vec<&str> = picked.iter().map(|o| o.id.as_str()).collect();
            overview::table_pick(window.get_browse_panel_id().as_str(), &ids, fresh).and_then(|id| picked.iter().find(|o| o.id == id).copied())
        } else {
            None
        };
        window.set_offer_rows(ModelRc::new(VecModel::from(picked.into_iter().map(offer_row).collect::<Vec<_>>())));
        // As a click on it does: its pane opens, its details and versions are asked for.
        if let Some(offer) = pick {
            window.invoke_open_offer(offer_row(offer));
        }
        let open = window.get_browse_panel_id();
        if !open.is_empty() {
            match offers.iter().find(|o| o.id == open.as_str()) {
                Some(offer) => {
                    window.set_panel_offer(offer_row(offer));
                    push_panel(window, view.addons.panel.as_ref());
                }
                // No longer offered: it closes, as its ✕ closes it.
                None => {
                    window.set_browse_panel_id("".into());
                    window.invoke_browse_close();
                }
            }
        }
        // The Installed table: the search, and the Lists and Safety groups by the rows' badges.
        let ticked_rows = vec![lists.clone(), safety.clone()];
        let rows: Vec<AddonRow> = view
            .addons
            .rows
            .iter()
            .filter(|r| {
                let (on_lists, level) = row_lists_and_level(r);
                shell::filter_match(&ticked_rows, &[on_lists, level])
            })
            .filter(|r| search.is_empty() || hit(&r.name) || hit(&r.author))
            .map(addon_row)
            .collect();
        window.set_addon_rows(ModelRc::new(VecModel::from(rows)));
        // Removed: the search only.
        let removed: Vec<RemovedRow> = view.addons.removed.iter().filter(|r| search.is_empty() || hit(&r.name)).map(removed_row).collect();
        window.set_removed_rows(ModelRc::new(VecModel::from(removed)));
    });
}

/// Home's "Show what it found" for an addon: Browse with its pane open when it is on offer (the
/// findings are asked for by the caller), else Installed with its row opened.
pub fn show_scan(window: &MainWindow, id: &str) {
    let offer = SHOWN.with(|shown| shown.borrow().as_ref().and_then(|v| v.addons.offers.iter().find(|o| o.id == id).map(offer_row)));
    match offer {
        Some(offer) => {
            window.invoke_show_scan(text(id), true);
            window.invoke_open_offer(offer);
            filter_offers(window);
        }
        None => window.invoke_show_scan(text(id), false),
    }
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

        // The colours and the frame Windows draws: at the first view (the window may be shown
        // already) and when the theme or Windows' own mode change, and only then: Settings' theme
        // applies them the moment it is clicked, and a view made before that click mustn't put
        // the old ones back.
        if old.is_none_or(|o| o.dark != view.dark || o.system_dark != view.system_dark) {
            set_theme(window, card, view.dark);
        }
        window.set_app_released(text(&view.app_released));
        window.set_pill_text(text(&view.pill.text));
        window.set_pill_link(text(&view.pill.link));
        window.set_pill_kind(view.pill.kind);
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
        let news = &view.game_news;
        window.set_game_news_new(news.new);
        window.set_game_news_version(text(&news.version));
        window.set_news_version(text(&view.news_version));
        window.set_news_more(view.news_more);
        window.set_game_notes_footer(text(&news.footer));
        window.set_game_notes_empty(text(&news.empty));
        window.set_game_news_latest_answer(text(&news.latest_answer));
        window.set_game_news_notes_answer(text(&news.notes_answer));
        if old.is_none_or(|o| o.game_news.notes != news.notes) {
            window.set_game_notes(game_notes(&news.notes));
        }

        window.set_state(state);
        window.set_busy(view.busy);
        window.set_sync_on(view.sync_on);
        window.set_playing(view.playing);
        // The rail.
        let rail = &view.rail;
        window.set_build_line(text(&rail.build.0));
        window.set_build_link(rail.build.1);
        match &rail.progress {
            Some(line) => {
                window.set_progress_line(text(&line.text));
                window.set_progress_dot(dot(line.dot));
                window.set_progress_amber(line.amber);
                window.set_progress_wrap(line.wrap);
            }
            None => window.set_progress_line("".into()),
        }
        match &rail.addons {
            Some(line) => {
                window.set_addons_line(text(&line.text));
                window.set_addons_dot(dot(line.dot));
            }
            None => window.set_addons_line("".into()),
        }
        window.set_last_line(text(&rail.last));
        // Home.
        if old.is_none_or(|o| o.news_blocks != view.news_blocks) {
            let blocks: Vec<ChangelogBlock> = view.news_blocks.iter().map(|(kind, t)| ChangelogBlock { kind: *kind, text: text(t) }).collect();
            window.set_news_blocks(ModelRc::new(VecModel::from(blocks)));
        }
        if old.is_none_or(|o| o.pcs != view.pcs) {
            let rows: Vec<PcRow> = view.pcs.iter().map(|p| PcRow { name: text(&p.name), this_pc: p.this_pc, line: text(&p.line) }).collect();
            window.set_pcs(ModelRc::new(VecModel::from(rows)));
        }
        window.set_pcs_week(text(&view.pcs_week));
        if old.is_none_or(|o| o.questions != view.questions) {
            window.set_questions(questions(&view.questions));
        }
        if old.is_none_or(|o| o.recent != view.recent) {
            let lines: Vec<RecentLine> = view.recent.iter().map(|r| RecentLine { kind: kind_icon(r.kind), text: text(&r.text), when: text(&r.when) }).collect();
            window.set_recent(ModelRc::new(VecModel::from(lines)));
        }
        let playtime = &view.playtime;
        window.set_playtime_week(text(&playtime.week));
        window.set_playtime_week_sessions(playtime.week_sessions as i32);
        window.set_playtime_today(text(&playtime.today));
        window.set_playtime_all(text(&playtime.all));
        if old.is_none_or(|o| o.playtime.days != playtime.days) {
            let days: Vec<DayBar> = playtime.days.iter().map(|(label, share, minutes)| DayBar { label: text(label), share: *share, minutes: *minutes as i32 }).collect();
            window.set_playtime_days(ModelRc::new(VecModel::from(days)));
        }
        if old.is_none_or(|o| o.playtime.per_pc != playtime.per_pc) {
            let rows: Vec<PcTime> = playtime.per_pc.iter().map(|(name, time, share)| PcTime { name: text(name), time: text(time), share: *share }).collect();
            window.set_pc_times(ModelRc::new(VecModel::from(rows)));
        }
        if old.is_none_or(|o| o.playtime.sessions != playtime.sessions) {
            let rows: Vec<SessionRow> = playtime.sessions.iter().map(|(day, pc, minutes)| SessionRow { day: text(day), pc: text(pc), minutes: text(minutes) }).collect();
            window.set_sessions(ModelRc::new(VecModel::from(rows)));
        }
        // Settings' Progress versions.
        if old.is_none_or(|o| o.saves != view.saves) {
            let rows: Vec<SaveRow> = view
                .saves
                .iter()
                .map(|s| SaveRow {
                    version: text(&s.version),
                    when: text(&s.when),
                    from: text(&s.from),
                    size: text(&s.size),
                    note: text(&s.note),
                    current: s.current,
                    can_bring: s.can_bring,
                })
                .collect();
            window.set_save_rows(ModelRc::new(VecModel::from(rows)));
        }
        window.set_saves_summary(text(&view.saves_summary));
        window.set_saves_note(text(&view.saves_note));

        let addons = &view.addons;
        let rows_changed = old.is_none_or(|o| o.addons.rows != addons.rows || o.addons.removed != addons.removed);
        if old.is_none_or(|o| o.addons.versions != addons.versions) {
            window.set_versions(addons.versions.as_ref().map(versions_row).unwrap_or_default());
        }
        window.set_addons_count(addons.count as i32);
        window.set_addons_header(text(&addons.header));
        window.set_addons_share(text(&addons.share));
        window.set_can_update_all(addons.update_all);
        window.set_addons_footer(text(&addons.footer));
        window.set_addons_notice(text(&addons.notice));
        window.set_addons_message(text(&addons.message));
        window.set_browse_note(text(&addons.browse_note));
        window.set_forge_status(text(&addons.forge_status));
        window.set_forge_answer(text(&addons.forge_answer));
        window.set_forge_offline(addons.forge_offline);
        window.set_discord_checked(text(&addons.discord_checked));
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
        window.set_needs_note(text(&view.needs_note));

        match &view.setup {
            Some(setup) => {
                window.set_setup_sync_line(text(&setup.sync_line));
                window.set_setup_sync_warn(setup.sync_warn);
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
            set_browse_lists(window, &addons.offers);
        }
        *shown.borrow_mut() = Some(view.clone());
        if offers_changed || rows_changed {
            filter_offers(window);
        }
        // The panel, when it's for the addon open now (a late answer for another one waits).
        push_panel(window, addons.panel.as_ref());
    });
}

/// The window's colours and its frame, light or dark: the window, the Play card, and the caption
/// Windows draws.
pub fn set_theme(window: &MainWindow, card: &PlayCard, dark: bool) {
    window.set_dark(dark);
    card.set_dark(dark);
    win::paint_frame(&window.window().window_handle(), dark);
}

/// Settings as the app answers them. Every switch and choice shows the saved value. Of the three
/// typed fields (the network folder, this PC's name, the game's folder):
/// - the one this answer refused keeps the text typed, with why under it in red; it stays so until
///   it is typed in again or applies (another row's answer leaves it as it is);
/// - the one this answer applied shows the value as saved (a mapped drive as its network path),
///   and "✓ Saved" lights on its row;
/// - the one being typed in (`focused-field`: Alt-Tab or a popup doesn't end that, a click on
///   another control does) is never touched, unless the answer is for the very text it holds;
/// - any other shows the saved value: text typed there and never applied with Enter goes.
/// A form that answers no change (at start, at the first start's steps) clears every refusal.
pub fn apply_form(window: &MainWindow, view: &FormView) {
    let saved = &view.saved;
    let focused = window.get_focused_field();
    let answered = view.answered.as_ref();
    for (key, value) in [("folder", &saved.folder), ("pc-name", &saved.pc_name), ("game-dir", &saved.game_dir)] {
        let now = field_text(window, key);
        // Typed in again since the change went out: what the player types now wins.
        let typed_since = |sent: &str| focused.as_str() == key && now.as_str() != sent;
        match answered.filter(|a| a.key == key) {
            Some(a) if !a.why.is_empty() => {
                if !typed_since(&a.typed) {
                    set_field(window, key, &a.typed);
                    set_refusal(window, key, &a.why);
                }
            }
            Some(a) => {
                if !typed_since(&a.typed) {
                    set_field(window, key, value);
                }
                set_refusal(window, key, "");
            }
            None if focused.as_str() == key || (answered.is_some() && !refusal(window, key).is_empty()) => {}
            None => {
                set_field(window, key, value);
                set_refusal(window, key, "");
            }
        }
    }
    // The sync switch turned on with a network folder that can't be used or doesn't answer: why,
    // under the folder.
    if let Some(a) = answered.filter(|a| a.key == "sync" && !a.why.is_empty()) {
        set_refusal(window, "folder", &a.why);
    }
    if let Some(a) = answered.filter(|a| a.applied) {
        window.invoke_show_saved(text(&a.key));
    }
    window.set_game_status(text(&view.game_status));
    window.set_game_ok(view.game_ok);
    window.set_sync_switch(saved.sync);
    window.set_folder_status(text(&view.folder_status));
    window.set_folder_status_kind(if view.folder_status.starts_with('✓') { 1 } else if view.folder_status.starts_with('✗') { 2 } else { 0 });
    window.set_start_with_windows(saved.start_with_windows);
    window.set_desktop_shortcut(saved.desktop_shortcut);
    window.set_notifications(saved.notifications);
    window.set_addons_auto(saved.addons_auto);
    window.set_addons_shared(saved.addons_shared);
    window.set_addons_forge(saved.addons_forge);
    window.set_pause_when_away(saved.pause_when_away);
    window.set_theme(saved.theme);
    window.set_game_shared(saved.game_shared);
    window.set_browse_view(text(&saved.browse_view));
    window.set_settings_path(text(&view.settings_path));
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
        addons_forge: saved.addons_forge,
        pause_when_away: saved.pause_when_away,
        theme: saved.theme,
        game_shared: saved.game_shared,
        browse_view: text(&saved.browse_view),
    });
}

fn field_text(window: &MainWindow, key: &str) -> SharedString {
    match key {
        "folder" => window.get_sync_folder(),
        "pc-name" => window.get_pc_name(),
        _ => window.get_game_dir(),
    }
}

fn set_field(window: &MainWindow, key: &str, value: &str) {
    match key {
        "folder" => window.set_sync_folder(text(value)),
        "pc-name" => window.set_pc_name(text(value)),
        _ => window.set_game_dir(text(value)),
    }
}

fn refusal(window: &MainWindow, key: &str) -> SharedString {
    match key {
        "folder" => window.get_folder_refused(),
        "pc-name" => window.get_name_refused(),
        _ => window.get_game_refused(),
    }
}

fn set_refusal(window: &MainWindow, key: &str, why: &str) {
    match key {
        "folder" => window.set_folder_refused(text(why)),
        "pc-name" => window.set_name_refused(text(why)),
        _ => window.set_game_refused(text(why)),
    }
}
