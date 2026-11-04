//! Renders the Satchel's screens to PPM images with Slint's software renderer (the one the app
//! uses), filled with sample data, so the UI can be looked at without Windows:
//! `cargo run -p ui-preview -- <out dir>`.

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, Image, Model, ModelRc, PhysicalSize, Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer, SharedString, StandardListViewItem, VecModel};
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
    let badge = ["Discord Verified", "Added by you", "From a file", "Installed by hand"][kind as usize];
    AddonRow {
        id: name.into(),
        initials: satchel_core::addons::overview::initials(name).into(),
        icon: Image::default(),
        name: name.into(),
        badge: badge.into(),
        badge_kind: kind,
        badges: badge_rows(&[]),
        author: author.into(),
        version: version.into(),
        released: if kind < 2 { "6 days ago".into() } else { SharedString::new() },
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
        reason: SharedString::new(),
        reason_kind: 0,
        ..Default::default()
    }
}

/// An installed row's Safety and Status columns, as the app makes them from its badges and lines
/// (`overview::safety_cell`, `overview::row_status`).
fn columns(row: AddonRow) -> AddonRow {
    use satchel_core::addons::overview::{RowLines, Tone, row_status, safety_cell};
    let badges: Vec<(String, i32)> = row.badges.iter().map(|b| (b.text.to_string(), b.kind)).collect();
    let level = badges.iter().find_map(|(text, kind)| text.strip_prefix("SurvivorsForge · ").map(|level| (level.to_string(), *kind)));
    let ours = badges.iter().find(|(text, _)| !text.starts_with("SurvivorsForge · ")).cloned().unwrap_or((row.badge.to_string(), row.badge_kind));
    let safety = safety_cell(Some((ours.0.as_str(), ours.1)), level.as_ref().map(|(l, k)| (l.as_str(), *k)));
    let current = row.update.is_empty() && row.state.is_empty() && row.replace_with.is_empty() && ours.0 != "Installed by hand";
    let (status, tone) = row_status(&RowLines {
        incoming: row.incoming,
        reason: (row.reason.as_str(), row.reason_kind),
        state: (row.state.as_str(), row.state_kind),
        share: (row.share.as_str(), row.share_kind),
        problem: row.problem.as_str(),
        current,
    });
    let status_kind = match tone {
        Tone::Plain => 0,
        Tone::Good => 1,
        Tone::Look => 2,
        Tone::Never => 3,
    };
    AddonRow { safety: safety.word.as_str().into(), safety_kind: safety.kind, safety_by: safety.by.as_str().into(), status: status.into(), status_kind, ..row }
}

/// The Installed tab's rows, with their columns.
fn set_rows(ui: &MainWindow, rows: Vec<AddonRow>) {
    ui.set_addon_rows(ModelRc::new(VecModel::from(rows.into_iter().map(columns).collect::<Vec<_>>())));
}

/// The same row with its line about the other PCs (0.3).
fn shared(row: AddonRow, share: &str, kind: i32) -> AddonRow {
    AddonRow { share: share.into(), share_kind: kind, shared: true, ..row }
}

fn badge_rows(values: &[(&str, i32)]) -> ModelRc<BadgeRow> {
    ModelRc::new(VecModel::from(values.iter().map(|(t, kind)| BadgeRow { text: (*t).into(), kind: *kind }).collect::<Vec<_>>()))
}

/// What SurvivorsForge said of the 12 addons on both lists on 31 Oct (the registry: the latest
/// version's trust, the category).
const FORGE_JOINED: [(&str, &str, &str); 12] = [
    ("aether-hud", "verified", "Unit Frames"),
    ("as-builder", "verified", "Miscellaneous"),
    ("azerloot", "verified", "Miscellaneous"),
    ("azeroth-fieldkit", "verified", "Combat"),
    ("azeroth-stats", "verified", "Miscellaneous"),
    ("azs-spell-meter", "verified", "Combat"),
    ("blackwell-ui", "verified", "Combat"),
    ("keijin-ui", "verified", "Unit Frames"),
    ("lucky-odds", "verified", "Miscellaneous"),
    ("survivor-ui", "scanned", "Unit Frames"),
    ("survivors-dragon-ui", "verified", "Unit Frames"),
    ("survivor-ui-classic", "scanned", "Unit Frames"),
];

/// A Browse card and what the filters and sorts read from it (the app's `OfferLine`, in short).
#[derive(Clone)]
struct Offer {
    row: OfferRow,
    tags: Vec<String>,
    /// 0 our list only, 1 both, 2 SurvivorsForge only.
    origin: i32,
    /// SurvivorsForge's level and its badge's kind, for the Safety column and its filters.
    trust: (String, i32),
    /// Its downloads, for the popularity sort; `None` when not known.
    downloads: Option<u64>,
}

/// The table's cells as the app makes them (`overview::safety_cell`, `overview::install_cell`):
/// who vouches for it, and its Install column's button, from its action; `again`: removed here.
fn cells(row: OfferRow, trust: (&str, i32), again: bool) -> OfferRow {
    use satchel_core::addons::overview::{install_cell, safety_cell};
    let vouch = if row.ours { ("Discord Verified", 0) } else { ("SurvivorsForge", 4) };
    let safety = safety_cell(Some(vouch), Some(trust));
    let button = install_cell(row.action, again);
    OfferRow {
        safety: safety.word.as_str().into(),
        safety_kind: safety.kind,
        safety_by: safety.by.as_str().into(),
        install_text: button.text.into(),
        install_primary: button.primary,
        installed: row.action >= 3,
        installed_id: if row.action >= 3 { row.id.clone() } else { SharedString::new() },
        ..row
    }
}

/// An offer with another action (and its Install column's button with it).
fn acting(offer: &Offer, action: i32, again: bool) -> Offer {
    let row = OfferRow { action, ..offer.row.clone() };
    Offer { row: cells(row, (offer.trust.0.as_str(), offer.trust.1), again), ..offer.clone() }
}

/// The offers in Browse's order at start: the most downloads first, unknown counts last, by name
/// (`overview::browse_order`).
fn by_popularity(offers: &[Offer]) -> Vec<Offer> {
    use satchel_core::addons::overview::{Sort, SortKeys, browse_order};
    fn key(o: &Offer) -> SortKeys<'_> {
        SortKeys { name: o.row.name.as_str(), id: o.row.id.as_str(), date: None, downloads: o.downloads }
    }
    let mut sorted = offers.to_vec();
    sorted.sort_by(|a, b| browse_order(&key(a), &key(b), Sort::DEFAULT, false));
    sorted
}

/// "today", "yesterday", "3 days ago": a publish date beside its version.
fn days_ago(days: i64) -> String {
    match days {
        ..=0 => "today".into(),
        1 => "yesterday".into(),
        days => format!("{days} days ago"),
    }
}

/// A colour from a hue (0 to 360), a saturation and a value (0 to 1).
fn hsv(hue: f32, saturation: f32, value: f32) -> [u8; 3] {
    let c = value * saturation;
    let h = (hue.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = value - c;
    [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
}

/// A stand-in icon, its pixels made here (the app gets the real ones decoded on its network
/// thread): a square in the hue `hue`, with a ring and a dot of a lighter one, its corners rounded
/// as the app rounds an icon's (7 px of the 34 px square).
fn mock_icon(hue: f32) -> Image {
    let side = 64u32;
    let radius = side as f32 * 7.0 / 34.0;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(side, side);
    for (i, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let (px, py) = ((i as u32 % side) as f32 + 0.5, (i as u32 / side) as f32 + 0.5);
        let (x, y) = (px / side as f32 - 0.5, py / side as f32 - 0.5);
        let r = (x * x + y * y).sqrt();
        let [red, green, blue] = if (r - 0.28).abs() < 0.06 || r < 0.1 { hsv(hue + 30.0, 0.25, 0.98) } else { hsv(hue, 0.6, 0.78 - y * 0.35) };
        let (dx, dy) = ((radius - px).max(px - (side as f32 - radius)).max(0.0), (radius - py).max(py - (side as f32 - radius)).max(0.0));
        let inside = if dx > 0.0 && dy > 0.0 { (radius - dx.hypot(dy) + 0.5).clamp(0.0, 1.0) } else { 1.0 };
        *pixel = Rgba8Pixel { r: red, g: green, b: blue, a: (255.0 * inside) as u8 };
    }
    Image::from_rgba8(buffer)
}

/// A stand-in screenshot, its pixels made here: a dusk sky, a sun, two lines of hills and a bar
/// along the foot, in the way a game's interface sits there.
fn mock_shot(width: u32, height: u32, hue: f32) -> Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    for (i, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let (px, py) = (i as u32 % width, i as u32 / width);
        let (x, y) = (px as f32 / width as f32, py as f32 / height as f32);
        let far = 0.55 + 0.06 * (x * 9.0).sin() + 0.03 * (x * 23.0).cos();
        let near = 0.7 + 0.08 * (x * 5.0 + 1.0).sin();
        let sun = ((x - 0.72) * width as f32 / height as f32).hypot(y - 0.3) < 0.07;
        let color = if y > 0.9 && (0.3..0.7).contains(&x) {
            hsv(hue + 180.0, 0.3, 0.25)
        } else if y > near {
            hsv(hue + 90.0, 0.55, 0.35 - (y - near) * 0.3)
        } else if y > far {
            hsv(hue + 70.0, 0.45, 0.5)
        } else if sun {
            hsv(hue + 40.0, 0.35, 1.0)
        } else {
            hsv(hue, 0.5, 0.45 + y * 0.5)
        };
        *pixel = Rgba8Pixel { r: color[0], g: color[1], b: color[2], a: 255 };
    }
    Image::from_rgba8(buffer)
}

/// The addons the preview shows an icon for, with its hue: most that SurvivorsForge lists (22 of
/// its 31 had one on 4 Nov); the others keep their initials, as without an icon.
const ICONS: [(&str, f32); 11] = [
    ("aether-hud", 200.0),
    ("azerloot", 40.0),
    ("azeroth-fieldkit", 120.0),
    ("azeroth-stats", 260.0),
    ("azs-spell-meter", 0.0),
    ("keijin-ui", 300.0),
    ("lucky-odds", 90.0),
    ("survivor-ui", 20.0),
    ("survivors-dragon-ui", 340.0),
    ("forge:totemmanager", 170.0),
    ("forge:vulchans-combat-text", 220.0),
];

fn icon_of(id: &str) -> Image {
    ICONS.iter().find(|(i, _)| *i == id).map(|(_, hue)| mock_icon(*hue)).unwrap_or_default()
}

/// Browse: our list from the catalog this build carries, joined with what SurvivorsForge said on
/// 31 Oct, plus three addons only on SurvivorsForge. LuckyOdds, AZS Spell Meter and
/// SurvivorsDragonUi count as installed. BordoUI shows as it would had no admin approved its
/// flagged scan (it waits for the player's OK); the rest is as read that day. 0.7.2: icons for most
/// of those SurvivorsForge lists, and its own addons' dates and download counts from its registry
/// (sample figures).
fn offers() -> Vec<Offer> {
    use satchel_core::addons::catalog::Catalog;
    use satchel_core::addons::overview::initials;
    let catalog = Catalog::builtin();
    // Dates as on the day the catalog was made, so the images don't drift.
    let today = catalog.generated.date_naive();
    let mut offers: Vec<Offer> = catalog
        .addons
        .iter()
        .map(|e| {
            let forge = FORGE_JOINED.iter().find(|(id, _, _)| *id == e.id);
            let mut badges = vec![("Discord Verified".to_string(), 0)];
            let mut tags = vec![e.group.clone()];
            if let Some((_, trust, category)) = forge {
                badges.push((format!("SurvivorsForge · {trust}"), 4));
                tags.push((*category).to_string());
            }
            let badges: Vec<(&str, i32)> = badges.iter().map(|(t, k)| (t.as_str(), *k)).collect();
            let installed = ["lucky-odds", "azs-spell-meter", "survivors-dragon-ui"].contains(&e.id.as_str());
            let warning = if e.id == "keijin-ui" { "doesn't mix with SurvivorsDragonUi, which you have" } else { "" };
            // SurvivorsForge's five LuckyOdds versions, 31 Oct: 230 + 761 + 2,853 + 2,870 + 1,668.
            let (downloads, popularity) = if e.id == "lucky-odds" { ("8,382 downloads", "8,382") } else { ("", "") };
            let trust = forge.map(|(_, trust, _)| *trust).unwrap_or_default();
            let row = OfferRow {
                id: e.id.as_str().into(),
                initials: initials(&e.name).into(),
                icon: icon_of(&e.id),
                initials_kind: 0,
                name: e.name.as_str().into(),
                author: e.author.as_str().into(),
                downloads: downloads.into(),
                summary: e.summary.as_str().into(),
                badges: badge_rows(&badges),
                version: e.latest.shown_version().into(),
                date: days_ago((today - e.latest.published.date_naive()).num_days()).into(),
                action: if installed { 3 } else { 0 },
                reason: SharedString::new(),
                warning: warning.into(),
                ours: true,
                forge: forge.is_some(),
                tags: tags.join(" · ").into(),
                popularity: popularity.into(),
                ..Default::default()
            };
            Offer {
                row: cells(row, (trust, 4), false),
                tags,
                origin: if forge.is_some() { 1 } else { 0 },
                trust: (trust.to_string(), 4),
                downloads: (e.id == "lucky-odds").then_some(8_382),
            }
        })
        .collect();
    let only_forge = [
        (
            "forge:totemmanager",
            "TotemManager",
            "BippyMiester",
            "Live totem cards for Azeroth Survivors: fill status, DPS and an upgrade verdict.",
            "v1.1.1",
            "Shaman",
            ("SurvivorsForge · scanned", 4),
            "",
            (3, 412),
        ),
        (
            "forge:vulchans-combat-text",
            "Vulchan's Combat Text",
            "Vulchan",
            "This is just an addon for Azeroth Survivors to condense the floating combat text into a much more manageable form. No longer are we blind to the game.",
            "v0.3.4",
            "Combat",
            ("SurvivorsForge · scanned", 4),
            "",
            (6, 1_203),
        ),
        ("forge:bordoui", "BordoUI", "threenox", "", "v2.8.15", "Combat", ("SurvivorsForge · flagged", 5), "SurvivorsForge's scan found 1 warning in this version", (1, 2_090)),
    ];
    for (id, name, author, summary, version, category, badge, reason, (days, downloads)) in only_forge {
        let count = thousands(downloads);
        let trust = badge.0.trim_start_matches("SurvivorsForge · ");
        // On our list since (the catalog of 4 Nov): one addon on both, as SurvivorsForge's join
        // makes it, its level and count with it.
        if let Some(ours) = offers.iter_mut().find(|o| o.row.name.as_str().eq_ignore_ascii_case(name)) {
            let mut badges: Vec<(String, i32)> = ours.row.badges.iter().map(|b| (b.text.to_string(), b.kind)).collect();
            badges.push((badge.0.to_string(), badge.1));
            let badges: Vec<(&str, i32)> = badges.iter().map(|(t, k)| (t.as_str(), *k)).collect();
            ours.tags.push(category.to_string());
            let row = OfferRow {
                badges: badge_rows(&badges),
                forge: true,
                action: if reason.is_empty() { ours.row.action } else { 1 },
                reason: reason.into(),
                downloads: format!("{count} downloads").into(),
                popularity: count.as_str().into(),
                tags: ours.tags.join(" · ").into(),
                ..ours.row.clone()
            };
            ours.row = cells(row, (trust, badge.1), false);
            ours.origin = 1;
            ours.trust = (trust.to_string(), badge.1);
            ours.downloads = Some(downloads);
            continue;
        }
        let row = OfferRow {
            id: id.into(),
            initials: initials(name).into(),
            icon: icon_of(id),
            initials_kind: 4,
            name: name.into(),
            author: author.into(),
            downloads: format!("{count} downloads").into(),
            summary: summary.into(),
            badges: badge_rows(&[badge]),
            version: version.into(),
            // 0.7.2: its date and count come with the registry.
            date: days_ago(days).into(),
            action: if reason.is_empty() { 0 } else { 1 },
            reason: reason.into(),
            warning: SharedString::new(),
            ours: false,
            forge: true,
            tags: category.into(),
            popularity: count.into(),
            ..Default::default()
        };
        offers.push(Offer {
            row: cells(row, (trust, badge.1), false),
            tags: vec![category.to_string()],
            origin: 2,
            trust: (trust.to_string(), badge.1),
            downloads: Some(downloads),
        });
    }
    offers.sort_by_key(|o| o.row.name.trim_start_matches(|c: char| !c.is_alphanumeric()).to_lowercase());
    offers
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

/// The tag chips line by line, as the app makes them (`view::tag_lines`), `widen` more each for
/// the table's check boxes.
fn tag_lines(offers: &[Offer], on: &[&str], widen: usize) -> ModelRc<ModelRc<TagRow>> {
    let mut tags: Vec<String> = offers.iter().flat_map(|o| o.tags.iter().cloned()).collect();
    tags.sort_by_key(|t| t.to_lowercase());
    tags.dedup();
    let mut lines: Vec<Vec<String>> = Vec::new();
    let mut used = 0;
    for tag in tags {
        let width = tag.chars().count() * 7 + 24 + widen;
        match lines.last_mut() {
            Some(line) if used + width <= 470 => {
                line.push(tag);
                used += width;
            }
            _ => {
                used = width;
                lines.push(vec![tag]);
            }
        }
    }
    ModelRc::new(VecModel::from(
        lines
            .iter()
            .map(|line| {
                ModelRc::new(VecModel::from(line.iter().map(|t| TagRow { text: t.as_str().into(), on: on.contains(&t.as_str()) }).collect::<Vec<_>>()))
            })
            .collect::<Vec<_>>(),
    ))
}

/// Browse as the app fills it: the addons (those with any of the tags `on`, none: all), the tag
/// chips and the table's check boxes, the table's trust words, the counts and the Legend's tag line.
fn set_browse(ui: &MainWindow, offers: &[Offer], on: &[&str]) {
    let shown: Vec<OfferRow> = offers.iter().filter(|o| on.is_empty() || o.tags.iter().any(|t| on.contains(&t.as_str()))).map(|o| o.row.clone()).collect();
    ui.set_offer_rows(ModelRc::new(VecModel::from(shown)));
    ui.set_browse_tags(tag_lines(offers, on, 0));
    ui.set_browse_tag_checks(tag_lines(offers, on, 14));
    ui.set_browse_tags_on(on.len() as i32);
    let levels: Vec<TagRow> = ["verified", "scanned", "flagged"]
        .iter()
        .filter(|level| offers.iter().any(|o| o.trust.0 == **level))
        .map(|level| TagRow { text: (*level).into(), on: false })
        .collect();
    ui.set_browse_trust(ModelRc::new(VecModel::from(levels)));
    let mut tags: Vec<String> = offers.iter().flat_map(|o| o.tags.iter().cloned()).collect();
    tags.sort_by_key(|t| t.to_lowercase());
    tags.dedup();
    ui.set_browse_tag_line(tags.join(" · ").into());
    ui.set_browse_count_all(offers.len() as i32);
    ui.set_browse_count_ours(offers.iter().filter(|o| o.origin != 2).count() as i32);
    ui.set_browse_count_forge(offers.iter().filter(|o| o.origin != 0).count() as i32);
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
    set_rows(&ui, rows);
    ui.set_addons_count(5);
    ui.set_addons_header("4 updates · 3 go in when you press Play, 1 asks first".into());
    ui.set_can_update_all(true);
    ui.set_addons_footer("3 addons matched their GitHub releases exactly: AZS Spell Meter 1.32, LuckyOdds 0.6.0, SurvivorsDragonUi 3.2.57. New releases checked 12 minutes ago.".into());
    set_browse(ui, &offers(), &[]);
    ui.set_browse_note("18 Discord Verified · 15 on SurvivorsForge · the catalog of today 10:54".into());
    // 0.7.1: SurvivorsForge on (as saved: the other fields stay unsaved, so Save stays lit as before).
    ui.set_discord_checked("31 Oct 2026".into());
    ui.set_forge_repo_url(satchel_core::addons::forge::REPO.into());
    ui.set_forge_site_url(satchel_core::addons::forge::SITE.into());
    ui.set_addons_forge(true);
    ui.set_saved_form(SettingsForm { addons_forge: true, ..Default::default() });
    ui.set_forge_answer("Last answer 14:40".into());
    ui.set_link_text("github.com/Hearts47/RunPace".into());
    ui.set_link_state(2);
    ui.set_link_owner("Hearts47 · 1 star · last change today 09:12".into());
    ui.set_link_release("v1.1.0 · RunPace.zip · licence MIT · checksum matches GitHub's".into());
    ui.set_link_goes("RunPace · nothing left out · kills and XP per minute".into());
    ui.set_link_note("Not reviewed by the game's devs. Addon code can't reach your PC, and it asks before each update.".into());
    ui.set_settings_path(r"C:\Users\Player\AppData\Roaming\SurvivorsSatchel\settings.toml".into());
    // The screens before 0.8 show Browse as cards, as they were drawn then; 0.8's show the table,
    // its default now.
    ui.set_browse_view("cards".into());
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

/// The game's patch notes as the app lists them: the version and date, the paragraph, then the
/// file's lines, or the one muted line that says the full notes are on Discord.
fn game_notes(notes: &[satchel_core::game_news::Note]) -> ModelRc<ChangelogEntry> {
    use satchel_core::changelog::Block;
    let entries: Vec<ChangelogEntry> = notes
        .iter()
        .map(|n| {
            let mut blocks: Vec<ChangelogBlock> = Vec::new();
            if !n.paragraph.is_empty() {
                blocks.push(ChangelogBlock { kind: 0, text: n.paragraph.as_str().into() });
            }
            if n.in_file && !n.blocks.is_empty() {
                blocks.extend(n.blocks.iter().map(|b| match b {
                    Block::Paragraph(t) => ChangelogBlock { kind: 0, text: t.as_str().into() },
                    Block::Heading(t) => ChangelogBlock { kind: 1, text: t.as_str().into() },
                    Block::Bullet(t) => ChangelogBlock { kind: 2, text: t.as_str().into() },
                    Block::Inner(t) => ChangelogBlock { kind: 3, text: t.as_str().into() },
                }));
            } else {
                blocks.push(ChangelogBlock { kind: 4, text: "The full notes are on the game's Discord".into() });
            }
            ChangelogEntry {
                version: n.version.as_str().into(),
                date: n.date.map(|d| d.format("%-d %b %Y").to_string()).unwrap_or_default().into(),
                blocks: ModelRc::new(VecModel::from(blocks)),
            }
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
        reason: SharedString::new(),
        reason_kind: 0,
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
    // The window as it opens (16:9), and as small as it goes.
    let size = PhysicalSize::new(1120, 630);
    let small = PhysicalSize::new(960, 540);
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
    // 0.7.1: the filters unfolded, one tag on.
    ui.set_browse_filters_open(true);
    set_browse(&ui, &offers(), &["Combat"]);
    snap(&main, size, &out.join("05d-addons-browse-filters.ppm"));
    ui.set_browse_filters_open(false);
    set_browse(&ui, &offers(), &[]);
    ui.set_addons_page(2);
    snap(&main, size, &out.join("05e-addons-add.ppm"));
    ui.set_link_warn(true);
    ui.set_link_note("AZS Spell Meter is a Discord Verified addon by kex0. This one is by someone-else: a copy or a look-alike, not the Discord Verified one.".into());
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
    set_rows(&ui, updating);
    ui.set_addon_open("LuckyOdds".into());
    snap(&main, size, &out.join("05i-addons-updating.ppm"));
    let cards: Vec<OfferRow> = ui.get_offer_rows().iter().collect();
    let mut installing = cards.clone();
    if let Some(offer) = installing.iter_mut().find(|o| o.action == 0) {
        offer.action = 2;
    }
    ui.set_offer_rows(ModelRc::new(VecModel::from(installing)));
    ui.set_addons_page(1);
    snap(&main, size, &out.join("05j-addons-browse-installing.ppm"));
    ui.set_offer_rows(ModelRc::new(VecModel::from(cards)));
    ui.set_addons_page(2);
    ui.set_link_warn(false);
    ui.set_link_note("Not reviewed by the game's devs. Addon code can't reach your PC, and it asks before each update.".into());
    ui.set_link_installing(true);
    snap(&main, size, &out.join("05k-addons-add-installing.ppm"));
    ui.set_link_installing(false);
    let kept = vec![
        AddonRow { released: "2 days ago".into(), ..addon("AZS Spell Meter", 0, "kex0", "1.34", "", ("", 0), "") },
        AddonRow { released: "4 days ago".into(), ..addon("LuckyOdds", 0, "cadoky", "0.7.0", "", ("", 0), "") },
        AddonRow {
            released: "9 days ago".into(),
            notes: "Fixes the minimap border on wide screens.\nThe action bars remember their scale.".into(),
            notes_title: "What's new in 3.2.66-survivors".into(),
            back_to: "3.2.66-survivors".into(),
            pinned: true,
            can_update: true,
            ..addon("SurvivorsDragonUi", 0, "migwynkriid13", "3.2.57-survivors", "", ("Kept at this version; 3.2.66-survivors is out", 0), "")
        },
    ];
    set_rows(&ui, kept);
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
    set_rows(&ui, every.clone());
    ui.set_addons_count(5);
    ui.set_addons_header("Everything is up to date".into());
    ui.set_addons_share("On every PC · 3 changes from other PCs come in at the next Play".into());
    ui.set_addon_open("".into());
    snap(&main, size, &out.join("05m-addons-every-pc.ppm"));
    ui.set_addon_open("AZS Spell Meter".into());
    snap(&main, size, &out.join("05n-addons-every-pc-open.ppm"));
    let mut asking = every.clone();
    asking[0] = AddonRow { ask_pc: "PC 2".into(), ..shared(addon("AZS Spell Meter", 0, "kex0", "1.34", "", ("", 0), ""), "Its settings changed here and on PC 2 since they last matched: pick one below", 2) };
    set_rows(&ui, asking);
    ui.set_addons_share("On every PC · 3 changes from other PCs come in at the next Play · 1 settings question below".into());
    snap(&main, size, &out.join("05o-addons-settings-question.ppm"));
    ui.set_addon_open("RunPace".into());
    set_rows(&ui, every);
    snap(&main, size, &out.join("05p-addons-only-here.ppm"));
    ui.set_addons_share("".into());

    // 0.4.2: a turned-off addon's update waits until it's on.
    let off = vec![
        AddonRow { released: "2 days ago".into(), ..addon("AZS Spell Meter", 0, "kex0", "1.34", "", ("", 0), "") },
        AddonRow { released: "4 days ago".into(), ..addon("LuckyOdds", 0, "cadoky", "0.7.0", "", ("", 0), "") },
        AddonRow {
            on: false,
            released: "3 days ago".into(),
            ..addon("Survivor UI", 0, "morderous", "3.0.45", "3.0.46", ("Turned off, so it waits: it goes in at the first Play after you turn it on", 1), "")
        },
    ];
    set_rows(&ui, off);
    ui.set_addons_count(3);
    ui.set_addons_header("Up to date, except 1 turned-off addon with a newer version".into());
    ui.set_can_update_all(false);
    ui.set_addon_open("".into());
    snap(&main, size, &out.join("05q-addons-turned-off.ppm"));
    ui.set_addon_open("Survivor UI".into());
    snap(&main, size, &out.join("05r-addons-turned-off-open.ppm"));
    set_rows(&ui, rows);
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
        addons_forge: ui.get_addons_forge(),
        pause_when_away: ui.get_pause_when_away(),
        theme: ui.get_theme(),
        game_shared: ui.get_game_shared(),
        browse_view: ui.get_browse_view(),
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
    ui.set_changelog_open("0.7.2".into());
    snap(&main, size, &out.join("19b-whats-new-older.ppm"));
    ui.set_whats_new_open(false);
    ui.set_tab(1);
    ui.set_addons_page(3);
    snap(&main, size, &out.join("20-addons-removed-empty.ppm"));
    ui.set_removed_rows(ModelRc::new(VecModel::from(vec![
        removed("Azeroth Cursor", 0, "Discord Verified", "1.0.0", "Removed today 10:24", "Its settings stayed in the game's folder", ""),
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
    snap(&main, PhysicalSize::new(1120, 1800), &out.join("21b-settings-whole-game-question.ppm"));
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
    snap(&main, PhysicalSize::new(1120, 1950), &out.join("26b-settings-whole.ppm"));
    ui.set_dark(true);
    snap(&main, PhysicalSize::new(1120, 1950), &out.join("26c-dark-settings-whole.ppm"));
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
        snap(&main, PhysicalSize::new(1120, 1950), &out.join(format!("{name}.ppm")));
    }
    ui.set_dark(false);
    ui.set_checking(false);
    ui.set_check_result("".into());
    ui.set_repo_url("".into());
    ui.set_issue_url("".into());
    ui.set_app_version("0.4.1".into());

    // 0.7.1: SurvivorsForge in the Addons tab. Browse as cards (two columns, three from about
    // 900 px), the filters, the Legend, the panel for three kinds of addon, Installed rows that
    // wait for the player's OK or never update, Sources in Settings and About's block; each in
    // both themes.
    let both = |name: &str, size: PhysicalSize| {
        for dark in [false, true] {
            ui.set_dark(dark);
            let file = if dark { name.replacen('-', "-dark-", 1) } else { name.to_string() };
            snap(&main, size, &out.join(format!("{file}.ppm")));
        }
        ui.set_dark(false);
    };
    let all = offers();
    ui.set_tab(1);
    ui.set_addons_page(1);
    set_browse(&ui, &all, &[]);
    both("29-browse-cards", size);
    ui.set_browse_filters_open(true);
    set_browse(&ui, &all, &["Miscellaneous", "Shaman"]);
    ui.set_browse_origin(2);
    both("29b-browse-filters", size);
    ui.set_browse_filters_open(false);
    ui.set_browse_origin(0);
    set_browse(&ui, &all, &[]);
    ui.set_legend_pinned(true);
    both("29c-browse-legend", size);
    ui.set_legend_pinned(false);

    // The panel: our addon with SurvivorsForge's data, one only on SurvivorsForge that asks first,
    // and one of our six SurvivorsForge doesn't list.
    let catalog = satchel_core::addons::catalog::Catalog::builtin();
    let notes = |id: &str| {
        catalog.get(id).and_then(|e| e.latest.notes.as_deref()).map(|n| satchel_core::addons::overview::plain_notes(n, 700)).unwrap_or_default()
    };
    let open = |id: &str, panel: PanelRow| {
        if let Some(offer) = all.iter().find(|o| o.row.id.as_str() == id) {
            ui.set_panel_offer(offer.row.clone());
        }
        ui.set_browse_panel_id(id.into());
        ui.set_panel(panel);
    };
    // SurvivorsForge's LuckyOdds of 31 Oct: its summary and description, five versions' downloads.
    let bars: Vec<BarRow> = [("v0.8.0", "1,668", 1668.0), ("v0.7.0", "2,870", 2870.0), ("v0.6.0", "2,853", 2853.0), ("v0.5.0", "761", 761.0), ("v0.4.0", "230", 230.0)]
        .iter()
        .map(|(label, count, n)| BarRow { label: (*label).into(), count: (*count).into(), share: n / 2870.0 })
        .collect();
    open(
        "lucky-odds",
        PanelRow {
            id: "lucky-odds".into(),
            verified_author: true,
            about: "Your odds for Rare, Epic and Legendary cards from luck, plus caches and guardian lairs on the minimap.".into(),
            about_more: "See what your Luck actually does. LuckyOdds shows the rarity odds for level-up cards and amulets based on your current Luck, and marks Battleground Caches and guardian lairs on your minimap.".into(),
            notes_title: "What's new in 0.8.0".into(),
            notes: notes("lucky-odds").into(),
            bars: ModelRc::new(VecModel::from(bars)),
            scan: "verified: an admin approved this file".into(),
            folders: "Folder: LuckyOdds".into(),
            action: 3,
            ..Default::default()
        },
    );
    both("30-browse-panel-both-lists", size);
    // BordoUI v2.8.15's scan of 30 Oct, as it would ask had no admin approved the file.
    open(
        "forge:bordoui",
        PanelRow {
            id: "forge:bordoui".into(),
            verified_author: true,
            scan: "flagged: 1 warning, 75 info".into(),
            findings: strings(&["lua.api_override · BordoUI/ThemeUI.lua, line 2627: Replaces the game function ChatEdit_SendText"]),
            action: 1,
            reason: "SurvivorsForge's scan found 1 warning in this version".into(),
            ..Default::default()
        },
    );
    both("30b-browse-panel-asks-first", size);
    open(
        "big-cursor",
        PanelRow {
            id: "big-cursor".into(),
            about: "A bigger cursor, in the size you choose".into(),
            notes_title: "What's new in 0.3.4".into(),
            notes: notes("big-cursor").into(),
            folders: "Folder: BigCursor".into(),
            action: 0,
            ..Default::default()
        },
    );
    both("30c-browse-panel-our-list-only", size);
    // A wide window: three columns, the cards beside the panel; then the panel still loading.
    open("lucky-odds", PanelRow { id: "lucky-odds".into(), loading: true, ..Default::default() });
    snap(&main, small, &out.join("30d-browse-small-panel-loading.ppm"));
    ui.set_browse_panel_id("".into());
    snap(&main, small, &out.join("30e-browse-small.ppm"));

    // SurvivorsForge silent for an hour: its addons and its badges that let things in step aside.
    let offline: Vec<Offer> = all
        .iter()
        .filter(|o| o.origin != 2)
        .map(|o| {
            let badges: Vec<BadgeRow> = o.row.badges.iter().filter(|b| b.kind != 4).collect();
            Offer { row: OfferRow { badges: ModelRc::new(VecModel::from(badges)), ..o.row.clone() }, ..o.clone() }
        })
        .collect();
    set_browse(&ui, &offline, &[]);
    ui.set_forge_status("SurvivorsForge isn't answering · last answer 14:40".into());
    ui.set_forge_offline(true);
    both("31-browse-offline", size);
    ui.set_forge_status("".into());
    ui.set_forge_offline(false);
    set_browse(&ui, &all, &[]);

    // Installed rows: an update that waits for the player's OK, one that never comes, and an
    // addon only on SurvivorsForge (hypothetical words, real names).
    let before: Vec<AddonRow> = ui.get_addon_rows().iter().collect();
    let forge_rows = vec![
        AddonRow {
            badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · unscanned", 5)]),
            reason: "Waits for your OK: SurvivorsForge hasn't scanned this version yet".into(),
            reason_kind: 1,
            ..addon("AZS Spell Meter", 0, "kex0", "1.35", "1.36", ("", 0), "")
        },
        AddonRow {
            badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · verified", 4)]),
            released: "yesterday".into(),
            ..addon("LuckyOdds", 0, "cadoky", "0.8.0", "", ("", 0), "")
        },
        AddonRow {
            badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · source changed", 6)]),
            reason: "The file changed after SurvivorsForge scanned it. It stays as you have it; no updates.".into(),
            reason_kind: 2,
            ..addon("Survivor UI", 0, "morderous", "3.0.47", "", ("", 0), "")
        },
        AddonRow {
            badge: "SurvivorsForge · scanned".into(),
            badge_kind: 4,
            badges: badge_rows(&[("SurvivorsForge · scanned", 4)]),
            released: SharedString::new(),
            ..addon("TotemManager", 1, "BippyMiester", "v1.1.1", "", ("", 0), "")
        },
    ];
    set_rows(&ui, forge_rows);
    ui.set_addons_count(4);
    ui.set_addons_header("1 update · it waits for your OK".into());
    ui.set_can_update_all(false);
    ui.set_addons_page(0);
    both("32-addons-asks-and-never", size);
    ui.set_addon_open("AZS Spell Meter".into());
    both("32b-addons-update-anyway", size);
    ui.set_addon_open("".into());
    set_rows(&ui, before);
    ui.set_addons_count(5);
    ui.set_addons_header("4 updates · 3 go in when you press Play, 1 asks first".into());
    ui.set_can_update_all(true);

    // Settings → Addons → Sources: SurvivorsForge on, off, silent; About's block at the foot.
    ui.set_tab(4);
    let whole = PhysicalSize::new(1120, 1950);
    both("33-settings-sources-on", whole);
    ui.set_addons_forge(false);
    ui.set_saved_form(SettingsForm { addons_forge: false, ..ui.get_saved_form() });
    ui.set_forge_answer("Off".into());
    both("33b-settings-sources-off", whole);
    ui.set_addons_forge(true);
    ui.set_saved_form(SettingsForm { addons_forge: true, ..ui.get_saved_form() });
    ui.set_forge_answer("Isn't answering since 14:40".into());
    ui.set_forge_offline(true);
    both("33c-settings-sources-offline", whole);
    ui.set_forge_answer("Last answer 14:40".into());
    ui.set_forge_offline(false);
    ui.set_tab(0);

    // 0.7.2: Browse's three views and their switch, icons, the pictures strip and the viewer, and
    // Versions…; each in both themes. Figures not of 31 Oct (dates, counts, captions, findings on
    // older versions) are samples.
    ui.set_tab(1);
    ui.set_addons_page(1);
    set_browse(&ui, &all, &[]);
    ui.set_browse_panel_id("".into());
    both("34-browse-cards-icons", size);
    both("34c-browse-cards-960", small);
    // Each picture with its thumbnail, as the app holds them for the frames shown (a stand-in
    // made here, at the thumbnails' size).
    let pictures = |captions: &[(&str, i32, i32)]| {
        ModelRc::new(VecModel::from(
            captions
                .iter()
                .enumerate()
                .map(|(i, (caption, width, height))| {
                    let (w, h) = satchel_core::addons::forge::fit_within(*width as u32, *height as u32, satchel_core::addons::forge::THUMB_SIDE);
                    PictureRow { width: *width, height: *height, caption: (*caption).into(), thumb: mock_shot(w, h, 30.0 * i as f32 + 190.0) }
                })
                .collect::<Vec<_>>(),
        ))
    };
    let lucky_pictures = pictures(&[
        ("The odds beside the level-up cards", 1920, 1080),
        ("A Battleground Cache on the minimap", 1280, 720),
        ("", 1080, 1080),
        ("The options window", 800, 600),
        ("Guardian lairs at night", 1920, 1080),
        ("", 1920, 1080),
        ("", 1920, 1080),
    ]);
    let lucky_bars: Vec<BarRow> = [("v0.8.0", "1,668", 1668.0), ("v0.7.0", "2,870", 2870.0), ("v0.6.0", "2,853", 2853.0), ("v0.5.0", "761", 761.0), ("v0.4.0", "230", 230.0)]
        .iter()
        .map(|(label, count, n)| BarRow { label: (*label).into(), count: (*count).into(), share: n / 2870.0 })
        .collect();
    let lucky_panel = PanelRow {
        id: "lucky-odds".into(),
        verified_author: true,
        about: "Your odds for Rare, Epic and Legendary cards from luck, plus caches and guardian lairs on the minimap.".into(),
        about_more: "See what your Luck actually does. LuckyOdds shows the rarity odds for level-up cards and amulets based on your current Luck, and marks Battleground Caches and guardian lairs on your minimap.".into(),
        notes_title: "What's new in 0.8.0".into(),
        notes: notes("lucky-odds").into(),
        bars: ModelRc::new(VecModel::from(lucky_bars)),
        scan: "verified: an admin approved this file".into(),
        folders: "Folder: LuckyOdds".into(),
        pictures: lucky_pictures.clone(),
        action: 3,
        ..Default::default()
    };
    open("lucky-odds", lucky_panel.clone());
    both("34b-browse-panel-pictures", size);

    // The list: one row per addon; LuckyOdds open in place.
    ui.set_browse_view("list".into());
    ui.set_browse_panel_id("".into());
    both("35-browse-list", size);
    // LuckyOdds open in place, the row before it above (the list scrolled to it).
    let at = all.iter().position(|o| o.row.id == "lucky-odds").unwrap_or(0);
    set_browse(&ui, &all[at.saturating_sub(1)..], &[]);
    open("lucky-odds", lucky_panel.clone());
    both("35b-browse-list-open", size);
    both("35c-browse-list-open-960", small);
    set_browse(&ui, &all, &[]);

    // The table: the selected addon beside it, its About, then its Versions.
    ui.set_browse_view("table".into());
    // Wide enough for the table's Tags column beside the pane.
    let wide = PhysicalSize::new(1280, 720);
    ui.set_table_tab(0);
    both("36-browse-table-wide", wide);
    // Versions… for LuckyOdds: the one installed and kept at, one kept to go back to, our
    // catalog's releases and SurvivorsForge's versions (one that asks first, one without a file).
    let version = |version: &str, marks: &str, detail: &str, badge: (&str, i32), action: i32, key: &str, reason: (&str, i32)| VersionItem {
        version: version.into(),
        marks: marks.into(),
        detail: detail.into(),
        badge: badge.0.into(),
        badge_kind: badge.1,
        action,
        key: key.into(),
        reason: reason.0.into(),
        reason_kind: reason.1,
    };
    let lucky_versions = VersionsRow {
        id: "lucky-odds".into(),
        name: "LuckyOdds".into(),
        rows: ModelRc::new(VecModel::from(vec![
            version("0.8.0", "", "30 Oct 2026 · 1,668 downloads · 48 KB · nothing found", ("SurvivorsForge · verified", 4), 2, "v0.8.0", ("", 0)),
            version("0.7.0", "installed · kept at this version until you let it update", "27 Oct 2026 · 2,870 downloads · 46 KB · nothing found", ("SurvivorsForge · verified", 4), 0, "", ("", 0)),
            version("0.6.0", "kept on 28 Oct", "22 Oct 2026 · 2,853 downloads · 45 KB · nothing found", ("SurvivorsForge · scanned", 4), 1, "2026-10-28 18-02-11 lucky-odds", ("", 0)),
            version(
                "0.5.0",
                "",
                "18 Oct 2026 · 761 downloads · 41 KB · for the game 1.0 to 1.1 · 1 warning",
                ("SurvivorsForge · flagged", 5),
                3,
                "v0.5.0",
                ("Waits for your OK: SurvivorsForge's scan found 1 warning in this version", 1),
            ),
            version("0.4.0", "", "12 Oct 2026 · 230 downloads", ("SurvivorsForge · unscanned", 5), 0, "", ("SurvivorsForge has no checksum for this version yet", 2)),
        ])),
        loading: false,
    };
    ui.set_versions(lucky_versions.clone());
    ui.set_versions_id("lucky-odds".into());
    ui.set_table_tab(1);
    both("36c-browse-table-versions", wide);
    // About's Safety line: BordoUI's scan of 30 Oct and what it found.
    open(
        "forge:bordoui",
        PanelRow {
            id: "forge:bordoui".into(),
            verified_author: true,
            scan: "flagged: 1 warning, 75 info".into(),
            findings: strings(&["lua.api_override · BordoUI/ThemeUI.lua, line 2627: Replaces the game function ChatEdit_SendText"]),
            folders: "Folders: BordoUI, BordoUI_Media".into(),
            action: 1,
            reason: "SurvivorsForge's scan found 1 warning in this version".into(),
            ..Default::default()
        },
    );
    ui.set_table_tab(0);
    both("36d-browse-table-safety", wide);
    // The table alone (its pane closed) at the smallest window, then its filters as check boxes.
    ui.set_browse_panel_id("".into());
    both("36f-browse-table-alone-960", small);
    ui.set_browse_filters_open(true);
    ui.set_browse_sort(1);
    ui.set_browse_sort_reverse(true);
    both("36g-browse-table-filters", size);
    ui.set_browse_filters_open(false);
    ui.set_browse_sort(2);
    ui.set_browse_sort_reverse(false);
    // The viewer, over the window: LuckyOdds' second picture, fitted, with its caption; then one
    // still on its way.
    open("lucky-odds", lucky_panel.clone());
    ui.set_viewer_open(true);
    ui.set_viewer_index(1);
    ui.set_viewer_image(mock_shot(1280, 720, 210.0));
    ui.set_viewer_state(1);
    both("37-viewer", size);
    both("37b-viewer-wide", wide);
    ui.set_viewer_index(2);
    ui.set_viewer_image(Image::default());
    ui.set_viewer_state(0);
    snap(&main, size, &out.join("37c-viewer-loading.ppm"));
    ui.set_viewer_open(false);
    // Versions… over the window, from the cards' panel.
    ui.set_browse_view("cards".into());
    ui.set_versions_overlay(true);
    both("38-versions", size);
    ui.set_versions_overlay(false);
    ui.set_versions_id("".into());
    ui.set_browse_panel_id("".into());

    // Installed rows with their icons; an addon arriving from another PC that waits for the
    // player's OK, opened: its own Install anyway. Then Removed: one that asks first, one never.
    let installed = vec![
        AddonRow { icon: icon_of("azs-spell-meter"), ..addon("AZS Spell Meter", 0, "kex0", "1.34", "", ("", 0), "") },
        AddonRow {
            badges: badge_rows(&[("SurvivorsForge · flagged", 5)]),
            badge: "SurvivorsForge · flagged".into(),
            badge_kind: 4,
            initials: "BO".into(),
            reason: "Waits for your OK: SurvivorsForge's scan found 1 warning in this version".into(),
            reason_kind: 1,
            state: "From Laptop".into(),
            incoming: true,
            switchable: false,
            can_pin: false,
            has_page: true,
            released: SharedString::new(),
            ..addon("BordoUI", 1, "threenox", "v2.8.15", "", ("", 0), "")
        },
        AddonRow {
            icon: icon_of("lucky-odds"),
            badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · verified", 4)]),
            back_to: "0.6.0".into(),
            pinned: true,
            state: "Kept at this version; 0.8.0 is out".into(),
            ..addon("LuckyOdds", 0, "cadoky", "0.7.0", "", ("", 0), "")
        },
        AddonRow {
            icon: icon_of("forge:totemmanager"),
            badge: "SurvivorsForge · scanned".into(),
            badge_kind: 4,
            badges: badge_rows(&[("SurvivorsForge · scanned", 4)]),
            released: "3 days ago".into(),
            ..addon("TotemManager", 1, "BippyMiester", "v1.1.1", "", ("", 0), "")
        },
    ];
    set_rows(&ui, installed);
    ui.set_addons_count(3);
    ui.set_addons_header("Up to date, except 1 addon kept at an older version".into());
    ui.set_addons_share("On every PC · 1 change from another PC comes in at the next Play".into());
    ui.set_can_update_all(false);
    ui.set_addons_page(0);
    ui.set_addon_open("BordoUI".into());
    both("39-addons-icons-incoming", size);
    ui.set_addon_open("LuckyOdds".into());
    both("39b-addons-row-versions", size);
    ui.set_addon_open("".into());
    ui.set_addons_share("".into());
    ui.set_removed_rows(ModelRc::new(VecModel::from(vec![
        RemovedRow {
            reason: "Waits for your OK: SurvivorsForge hasn't scanned this version yet".into(),
            reason_kind: 1,
            ..removed("Azeroth Cursor", 0, "Discord Verified", "1.0.0", "Removed today 10:24", "Its settings stayed in the game's folder", "")
        },
        RemovedRow {
            reason: "SurvivorsForge blocked it: Sends your chat elsewhere".into(),
            reason_kind: 2,
            ..removed("Big Cursor", 1, "Added by you", "0.3.4", "Removed yesterday 23:10, as on PC 2", "Its settings went with it; they come back with it", "")
        },
        removed("Old Meter", 2, "From a file", "2.1", "Removed 27 Oct 18:02", "Its settings stayed in the game's folder", ""),
    ])));
    ui.set_addons_page(3);
    both("39c-addons-removed-asks", size);
    ui.set_addons_page(0);
    ui.set_tab(0);

    // 0.8: the game's patch notes. The Play tab's line, new and then seen; the panel with the
    // real file's versions and one build seen live only (the newest, marked); the panel with
    // nothing cached; About's block with the two answer times; each in both themes.
    let file = satchel_core::changelog::versions(include_str!("../../../game/patch-notes.md"));
    let mut seen = std::collections::BTreeMap::new();
    seen.insert(
        "0.141.1".to_string(),
        satchel_core::game_news::Seen {
            notes: "A small fix: Scholomance's loot rolls once more per kill as 0.141.0 said, and the Armory shows the right chance again.".into(),
            modified: satchel_core::game_news::http_date("Wed, 04 Nov 2026 14:12:00 GMT"),
            first_seen: None,
        },
    );
    ui.set_game_notes(game_notes(&satchel_core::game_news::merge(&file, &seen)));
    ui.set_game_news_version("0.141.1".into());
    ui.set_game_news_line("Azeroth Survivors 0.141.1 · what's new".into());
    ui.set_game_news_new(true);
    ui.set_game_news_latest_answer("last answer 14:40".into());
    ui.set_game_news_notes_answer("last answer 14:12".into());
    both("40-play-game-news-new", size);
    ui.set_game_news_new(false);
    both("40b-play-game-news-seen", size);
    ui.set_game_news_open(true);
    both("41-game-news-panel", size);
    // Tall enough for a whole long version and the ones after it.
    both("41c-game-news-panel-tall", PhysicalSize::new(1120, 1600));
    ui.set_game_notes(game_notes(&[]));
    ui.set_game_notes_empty("No notes on this PC yet: they show once the game's update server or the notes file has answered.".into());
    ui.set_game_news_latest_answer("no answer yet on this PC".into());
    ui.set_game_news_notes_answer("no answer yet on this PC".into());
    both("41b-game-news-panel-empty", size);
    ui.set_game_news_open(false);
    ui.set_game_notes_empty("".into());
    ui.set_game_news_latest_answer("last answer 14:40".into());
    ui.set_game_news_notes_answer("last answer 14:12".into());
    ui.set_tab(4);
    // Tall enough for every group, the two About blocks at the foot.
    both("42-settings-about-game-news", PhysicalSize::new(1120, 2150));
    ui.set_tab(0);

    // 0.8, part 2: the window at 16:9 (1,120 × 630; 960 × 540 at its smallest). Browse opens on
    // the table, sorted by popularity, its top row open with its thumbnails; the Install column's
    // buttons (Install, Install…, Installing…, Uninstall, Update, Install again), with the row that
    // asks first selected; the Installed tab in columns, closed and opened; Play and Settings in
    // their centred column; at the smallest window the table with its pane, and the viewer as a
    // box, shown and loading. Each in both themes. Counts and states beyond 31 Oct are samples.
    let popular = by_popularity(&all);
    let open_in = |list: &[Offer], id: &str, panel: PanelRow| {
        if let Some(offer) = list.iter().find(|o| o.row.id.as_str() == id) {
            ui.set_panel_offer(offer.row.clone());
        }
        ui.set_browse_panel_id(id.into());
        ui.set_panel(panel);
    };
    ui.set_tab(1);
    ui.set_addons_page(1);
    ui.set_browse_view("table".into());
    ui.set_browse_sort(2);
    ui.set_browse_sort_reverse(false);
    ui.set_browse_filters_open(false);
    ui.set_table_tab(0);
    set_browse(&ui, &popular, &[]);
    // The top row, LuckyOdds (the most downloads), open: its thumbnails, the Safety and Folders
    // lines, What's new.
    let lucky_open = PanelRow { pictures: lucky_pictures.clone(), ..lucky_panel.clone() };
    open_in(&popular, "lucky-odds", lucky_open.clone());
    both("43-browse-table-top-row", size);
    // The Install column, one of each button; BordoUI, which asks first, selected: its pane says
    // why, with Install anyway.
    let acted: Vec<Offer> = popular
        .iter()
        .map(|o| match o.row.id.as_str() {
            "forge:totemmanager" => acting(o, 2, false),
            "aether-hud" => acting(o, 4, false),
            "azeroth-cursor" => acting(o, 0, true),
            _ => o.clone(),
        })
        .collect();
    set_browse(&ui, &acted, &[]);
    open_in(
        &acted,
        "bordo-ui",
        PanelRow {
            id: "bordo-ui".into(),
            about: "A complete interface for Azeroth Survivors: unit frames, action bars and a themed chat.".into(),
            scan: "flagged: 1 warning, 75 info".into(),
            scan_more: true,
            folders: "Folders: BordoUI, BordoUI_Media".into(),
            action: 1,
            reason: "SurvivorsForge's scan found 1 warning in this version".into(),
            ..Default::default()
        },
    );
    both("44-browse-table-install-column", size);
    // The Installed tab in columns: updates, one that waits for the player's OK, one held back, one
    // kept at its version, one installed by hand, one SurvivorsForge's own, one from another PC.
    let columns_rows = vec![
        AddonRow { icon: icon_of("aether-hud"), badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · unscanned", 5)]), reason: "Waits for your OK: SurvivorsForge hasn't scanned this version yet".into(), reason_kind: 1, ..addon("AetherHUD", 0, "Aethus", "1.4.0", "1.5.0", ("", 0), "") },
        AddonRow { icon: icon_of("azs-spell-meter"), badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · verified", 4)]), released: "2 days ago".into(), ..addon("AZS Spell Meter", 0, "kex0", "1.34", "1.36", ("Goes in when you press Play", 1), "") },
        AddonRow {
            badges: badge_rows(&[("SurvivorsForge · flagged", 5)]),
            badge: "SurvivorsForge · flagged".into(),
            badge_kind: 4,
            initials: "BO".into(),
            reason: "Waits for your OK: SurvivorsForge's scan found 1 warning in this version".into(),
            reason_kind: 1,
            state: "From Laptop".into(),
            incoming: true,
            switchable: false,
            can_pin: false,
            has_page: true,
            released: SharedString::new(),
            ..addon("BordoUI", 1, "threenox", "v2.8.15", "", ("", 0), "")
        },
        AddonRow {
            replace_with: "4.5.0".into(),
            icon: icon_of("keijin-ui"),
            ..addon("KeijinUI", 3, "KeijinDE", "4.4.1", "", ("Not one of its author's releases: replace it with 4.5.0 to get updates", 2), "installed by hand, and its files aren't one of its author's releases, so it isn't updated")
        },
        AddonRow {
            icon: icon_of("lucky-odds"),
            badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · verified", 4)]),
            back_to: "0.6.0".into(),
            pinned: true,
            share: "On every PC".into(),
            shared: true,
            state: "Kept at this version; 0.8.0 is out".into(),
            ..addon("LuckyOdds", 0, "cadoky", "0.7.0", "", ("", 0), "")
        },
        AddonRow {
            icon: icon_of("survivor-ui"),
            badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · source changed", 6)]),
            reason: "The file changed after SurvivorsForge scanned it. It stays as you have it; no updates.".into(),
            reason_kind: 2,
            released: "11 days ago".into(),
            ..addon("Survivor UI", 0, "morderous", "3.0.47", "", ("", 0), "")
        },
        AddonRow { icon: icon_of("survivors-dragon-ui"), badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · verified", 4)]), released: "5 days ago".into(), ..addon("SurvivorsDragonUi", 0, "migwynkriid13", "3.2.64", "", ("", 0), "") },
        AddonRow {
            icon: icon_of("forge:totemmanager"),
            badge: "SurvivorsForge · scanned".into(),
            badge_kind: 4,
            badges: badge_rows(&[("SurvivorsForge · scanned", 4)]),
            released: "3 days ago".into(),
            ..addon("TotemManager", 1, "BippyMiester", "v1.1.1", "", ("", 0), "")
        },
    ];
    set_rows(&ui, columns_rows);
    ui.set_addons_count(7);
    ui.set_addons_header("2 updates · 1 goes in when you press Play, 1 asks first · 1 kept at an older version".into());
    ui.set_addons_share("On every PC · 1 change from another PC comes in at the next Play".into());
    ui.set_addons_footer("New releases checked 12 minutes ago.".into());
    ui.set_can_update_all(true);
    ui.set_addons_page(0);
    ui.set_addon_open("".into());
    both("45-addons-installed-columns", size);
    ui.set_addon_open("AZS Spell Meter".into());
    both("45b-addons-installed-row-open", size);
    ui.set_addon_open("".into());
    ui.set_addons_share("".into());
    // Play and Settings in their centred column, as wide as before the window grew.
    ui.set_tab(0);
    ui.set_state(TrayState::Synced);
    ui.set_banner("Synced: this PC has the newest progress".into());
    both("46-play-centred", size);
    ui.set_tab(4);
    both("47-settings-centred", size);
    // The smallest window: the table with LuckyOdds' pane beside it; then the viewer as a box over
    // the window darkened behind it, a picture shown, then one still on its way.
    ui.set_tab(1);
    ui.set_addons_page(1);
    set_browse(&ui, &popular, &[]);
    open_in(&popular, "lucky-odds", lucky_open.clone());
    both("48-browse-table-small", small);
    ui.set_viewer_open(true);
    ui.set_viewer_index(1);
    ui.set_viewer_image(mock_shot(1280, 720, 210.0));
    ui.set_viewer_state(1);
    both("49-viewer-small", small);
    ui.set_viewer_index(3);
    ui.set_viewer_image(Image::default());
    ui.set_viewer_state(0);
    both("49b-viewer-small-loading", small);
    ui.set_viewer_open(false);
    ui.set_browse_panel_id("".into());
    ui.set_tab(0);

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
