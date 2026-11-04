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
        installed_name: if row.action >= 3 { row.name.clone() } else { SharedString::new() },
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

fn strings(values: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(values.iter().map(|v| SharedString::from(*v)).collect::<Vec<_>>()))
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


/// A line of Home's Recent activity: its kind's icon, its words, and when.
fn recent(values: &[(i32, &str, &str)]) -> ModelRc<RecentLine> {
    ModelRc::new(VecModel::from(values.iter().map(|(kind, text, when)| RecentLine { kind: *kind, text: (*text).into(), when: (*when).into() }).collect::<Vec<_>>()))
}

fn tags(values: &[(&str, bool)]) -> ModelRc<TagRow> {
    ModelRc::new(VecModel::from(values.iter().map(|(t, on)| TagRow { text: (*t).into(), on: *on }).collect::<Vec<_>>()))
}

/// Every group of the offers, once each, A to Z: the rail's Groups filter and the Legend's line.
fn groups(offers: &[Offer]) -> Vec<String> {
    let mut tags: Vec<String> = offers.iter().flat_map(|o| o.tags.iter().cloned()).collect();
    tags.sort_by_key(|t| t.to_lowercase());
    tags.dedup();
    tags
}

/// Browse as the app fills it: the addons that pass the rail's filters (within a group any ticked
/// box matches, across groups all must: the lists, the safety levels, the groups), the rail's
/// filter boxes with those on, and the Legend's groups line.
fn set_browse(ui: &MainWindow, offers: &[Offer], lists: &[&str], safety: &[&str], on_groups: &[&str], installed_only: bool) {
    use satchel_core::shell::filter_match;
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let ticked = vec![s(lists), s(safety), s(on_groups)];
    let shown: Vec<OfferRow> = offers
        .iter()
        .filter(|o| {
            let mut lists = Vec::new();
            if o.origin != 2 {
                lists.push("Discord Verified".to_string());
            }
            if o.origin != 0 {
                lists.push("SurvivorsForge".to_string());
            }
            filter_match(&ticked, &[lists, vec![o.trust.0.clone()], o.tags.clone()]) && (!installed_only || o.row.installed)
        })
        .map(|o| o.row.clone())
        .collect();
    ui.set_offer_rows(ModelRc::new(VecModel::from(shown)));
    ui.set_filter_lists(tags(&[("Discord Verified", lists.contains(&"Discord Verified")), ("SurvivorsForge", lists.contains(&"SurvivorsForge"))]));
    let levels: Vec<(&str, bool)> = ["verified", "scanned", "flagged", "unscanned"].iter().map(|l| (*l, safety.contains(l))).collect();
    ui.set_filter_safety(tags(&levels));
    let all = groups(offers);
    let group_rows: Vec<(&str, bool)> = all.iter().map(|g| (g.as_str(), on_groups.contains(&g.as_str()))).collect();
    ui.set_filter_groups(tags(&group_rows));
    ui.set_filters_on((lists.len() + safety.len() + on_groups.len()) as i32);
    ui.set_groups_on(on_groups.len() as i32);
    ui.set_browse_installed_only(installed_only);
    ui.set_browse_tag_line(all.join(" · ").into());
}

/// One of Home's playtime days: the weekday, the bar's share of the tallest day, the minutes.
fn days(values: &[(&str, i32)]) -> ModelRc<DayBar> {
    let most = values.iter().map(|(_, m)| *m).max().unwrap_or(0).max(1);
    ModelRc::new(VecModel::from(values.iter().map(|(label, m)| DayBar { label: (*label).into(), share: *m as f32 / most as f32, minutes: *m }).collect::<Vec<_>>()))
}

fn sessions(values: &[(&str, &str, &str)]) -> ModelRc<SessionRow> {
    ModelRc::new(VecModel::from(values.iter().map(|(day, pc, minutes)| SessionRow { day: (*day).into(), pc: (*pc).into(), minutes: (*minutes).into() }).collect::<Vec<_>>()))
}

fn pcs(values: &[(&str, bool, &str)]) -> ModelRc<PcRow> {
    ModelRc::new(VecModel::from(values.iter().map(|(name, this_pc, line)| PcRow { name: (*name).into(), this_pc: *this_pc, line: (*line).into() }).collect::<Vec<_>>()))
}

fn questions(values: &[(&str, &[(&str, i32, &str)])]) -> ModelRc<QuestionRow> {
    ModelRc::new(VecModel::from(
        values
            .iter()
            .map(|(text, buttons)| QuestionRow {
                text: (*text).into(),
                buttons: ModelRc::new(VecModel::from(
                    buttons.iter().map(|(t, action, id)| QuestionButton { text: (*t).into(), action: *action, id: (*id).into() }).collect::<Vec<_>>(),
                )),
            })
            .collect::<Vec<_>>(),
    ))
}

fn saves(values: &[(&str, &str, &str, &str, &str, bool, bool)]) -> ModelRc<SaveRow> {
    ModelRc::new(VecModel::from(
        values
            .iter()
            .map(|(version, when, from, size, note, current, can_bring)| SaveRow {
                version: (*version).into(),
                when: (*when).into(),
                from: (*from).into(),
                size: (*size).into(),
                note: (*note).into(),
                current: *current,
                can_bring: *can_bring,
            })
            .collect::<Vec<_>>(),
    ))
}

/// The newest note of the real patch-notes file, as Home's What's new card shows it: its
/// paragraph, then its lines.
fn newest_note() -> (String, ModelRc<ChangelogBlock>) {
    let file = satchel_core::changelog::versions(include_str!("../../../game/patch-notes.md"));
    let newest = file.first().expect("game/patch-notes.md has a version");
    let notes = game_notes(&satchel_core::game_news::merge(std::slice::from_ref(newest), &std::collections::BTreeMap::new()));
    let entry = notes.row_data(0).expect("the newest note");
    (newest.version.clone(), entry.blocks)
}

/// Home, the rail and Settings as a PC with sync on sees them on 4 Nov: three PCs, the real
/// patch notes, sample playtimes, progress versions and activity (the mock's figures).
fn fill(ui: &MainWindow) {
    ui.set_state(TrayState::Synced);
    ui.set_sync_on(true);
    ui.set_app_version("0.9.0".into());
    ui.set_app_released("released today".into());
    ui.set_app_build("Built from 1b9cab0".into());
    ui.set_settings_path(r"C:\Users\Player\AppData\Roaming\SurvivorsSatchel\settings.toml".into());
    // The rail.
    let (version, blocks) = newest_note();
    ui.set_build_line(format!("Build {version}").into());
    ui.set_build_link(true);
    ui.set_game_news_version(version.into());
    ui.set_game_news_new(true);
    ui.set_news_blocks(blocks);
    ui.set_progress_line("Progress synced 2 min ago · 3 PCs".into());
    ui.set_progress_dot(0);
    ui.set_addons_line("7 addons on · 1 waits for your OK".into());
    ui.set_addons_dot(3);
    ui.set_last_line("Last played today, 34 min · Laptop".into());
    // Home.
    ui.set_pcs(pcs(&[("Laptop", true, "played today · synced 2 min ago"), ("Desktop", false, "played yesterday · synced yesterday 21:40"), ("Study PC", false, "played Sun · synced Sun 19:05")]));
    ui.set_pcs_week("This week: 4 h 12 min · 6 sessions".into());
    ui.set_questions(questions(&[(
        "BordoUI 2.8.18 waits for your OK: SurvivorsForge's scan found 1 warning in this version",
        &[("Install anyway", 7, "bordo-ui"), ("Show what it found", 8, "bordo-ui")],
    )]));
    ui.set_recent(recent(&[
        (0, "Progress sent · v27 · 8.1 KB", "2 min ago"),
        (2, "Played 34 min on Laptop", "today 13:20"),
        (5, "The Satchel checked for updates", "today 12:45"),
        (0, "Desktop brought in v26", "yesterday 21:40"),
        (1, "LuckyOdds updated to 0.9.0", "yesterday 18:05"),
    ]));
    ui.set_playtime_week("4 h 12 min".into());
    ui.set_playtime_week_sessions(6);
    ui.set_playtime_today("34 min".into());
    ui.set_playtime_all("61 h 20 min".into());
    ui.set_playtime_days(days(&[("Thu", 0), ("Fri", 48), ("Sat", 72), ("Sun", 55), ("Mon", 0), ("Tue", 43), ("Today", 34)]));
    let per_pc = [("Laptop", "2 h 2 min", 1.0), ("Desktop", "1 h 15 min", 0.61), ("Study PC", "55 min", 0.45)];
    ui.set_pc_times(ModelRc::new(VecModel::from(per_pc.iter().map(|(n, t, s)| PcTime { name: (*n).into(), time: (*t).into(), share: *s }).collect::<Vec<_>>())));
    ui.set_sessions(sessions(&[
        ("Today", "Laptop", "34 min"),
        ("Tue 3 Nov", "Desktop", "43 min"),
        ("Sun 1 Nov", "Study PC", "55 min"),
        ("Sat 31 Oct", "Laptop", "40 min"),
        ("Sat 31 Oct", "Desktop", "32 min"),
        ("Fri 30 Oct", "Laptop", "48 min"),
        ("Wed 28 Oct", "Desktop", "1 h 5 min"),
        ("Tue 27 Oct", "Laptop", "52 min"),
        ("Sun 25 Oct", "Study PC", "1 h 20 min"),
        ("Sat 24 Oct", "Laptop", "38 min"),
    ]));
    // Settings.
    ui.set_game_dir(r"C:\games\AzerothSurvivors".into());
    ui.set_game_status("✓ Game found".into());
    ui.set_game_ok(true);
    ui.set_sync_switch(true);
    ui.set_sync_folder(r"\\server\share\Azeroth Survivors Saves".into());
    ui.set_folder_status("✓ Folder answers · 3 PCs seen · 27 versions".into());
    ui.set_folder_status_kind(1);
    ui.set_pc_name("Laptop".into());
    ui.set_start_with_windows(false);
    ui.set_desktop_shortcut(true);
    ui.set_notifications(true);
    ui.set_addons_auto(true);
    ui.set_addons_shared(true);
    ui.set_addons_forge(true);
    ui.set_pause_when_away(false);
    ui.set_theme(2);
    ui.set_game_shared(true);
    ui.set_game_line("The same on every PC · Desktop's, version 3".into());
    ui.set_saved_form(SettingsForm { addons_forge: true, ..Default::default() });
    ui.set_forge_answer("Last answer 14:40".into());
    ui.set_discord_checked("4 Nov 2026".into());
    ui.set_forge_repo_url(satchel_core::addons::forge::REPO.into());
    ui.set_forge_site_url(satchel_core::addons::forge::SITE.into());
    ui.set_game_news_latest_answer("last answer 14:40".into());
    ui.set_game_news_notes_answer("last answer 14:12".into());
    ui.set_saves_summary("27 versions · newest 2 min ago from Laptop".into());
    ui.set_save_rows(saves(&[
        ("v27", "2 min ago", "Laptop", "8.1 KB", "the one you play", true, false),
        ("v26", "yesterday 21:40", "Desktop", "8.0 KB", "", false, true),
        ("v25", "yesterday 18:10", "Laptop", "7.9 KB", "", false, true),
        ("v24", "Sun 19:05", "Study PC", "7.9 KB", "kept when two PCs changed it", false, true),
        ("v23", "Sat 16:30", "Laptop", "7.7 KB", "", false, true),
        ("v22", "Fri 20:15", "Desktop", "7.6 KB", "", false, true),
    ]));
    // Only a build that knows its repository shows the two links: a stand-in one here.
    ui.set_repo_url("https://github.com/owner/survivors-satchel".into());
    ui.set_issue_url("https://github.com/owner/survivors-satchel/issues/new?template=bug.yml&version=0.9.0".into());
    ui.set_changelog(changelog());
    // The Installed tab in columns: updates, one that waits for the player's OK, one held back,
    // one kept at its version, one installed by hand, one SurvivorsForge's own, one from another PC.
    let rows = vec![
        AddonRow { icon: icon_of("aether-hud"), badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · unscanned", 5)]), reason: "Waits for your OK: SurvivorsForge hasn't scanned this version yet".into(), reason_kind: 1, ..addon("AetherHUD", 0, "NORehabRocket", "1.4.0", "1.5.0", ("", 0), "") },
        AddonRow { icon: icon_of("azs-spell-meter"), badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · verified", 4)]), released: "2 days ago".into(), ..addon("AZS Spell Meter", 0, "kex0", "1.34", "1.36", ("Goes in when you press Play", 1), "") },
        AddonRow {
            badges: badge_rows(&[("SurvivorsForge · flagged", 5)]),
            badge: "SurvivorsForge · flagged".into(),
            badge_kind: 4,
            initials: "BO".into(),
            reason: "Waits for your OK: SurvivorsForge's scan found 1 warning in this version".into(),
            reason_kind: 1,
            state: "From Desktop".into(),
            incoming: true,
            switchable: false,
            can_pin: false,
            has_page: true,
            released: SharedString::new(),
            ..addon("BordoUI", 1, "threenox", "v2.8.18", "", ("", 0), "")
        },
        AddonRow {
            replace_with: "4.5.0".into(),
            icon: icon_of("keijin-ui"),
            ..addon("KeijinUI", 3, "KeijinDE", "4.4.1", "", ("Not one of its author's releases: replace it with 4.5.0 to get updates", 2), "installed by hand, and its files aren't one of its author's releases, so it isn't updated")
        },
        AddonRow {
            icon: icon_of("lucky-odds"),
            badges: badge_rows(&[("Discord Verified", 0), ("SurvivorsForge · verified", 4)]),
            back_to: "0.8.0".into(),
            pinned: true,
            share: "On every PC".into(),
            shared: true,
            state: "Kept at this version; 0.9.0 is out".into(),
            ..addon("LuckyOdds", 0, "cadoky", "0.8.0", "", ("", 0), "")
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
    set_rows(ui, rows);
    ui.set_addons_count(7);
    ui.set_addons_header("2 updates · 1 goes in when you press Play, 1 asks first · 1 kept at an older version".into());
    ui.set_addons_share("On every PC · 1 change from another PC comes in at the next Play".into());
    ui.set_addons_footer("New releases checked 12 minutes ago.".into());
    ui.set_can_update_all(true);
    ui.set_removed_rows(ModelRc::new(VecModel::from(vec![
        RemovedRow {
            reason: "Waits for your OK: SurvivorsForge hasn't scanned this version yet".into(),
            reason_kind: 1,
            ..removed("Azeroth Cursor", 0, "Discord Verified", "1.0.0", "today 10:24", "Its settings stayed in the game's folder", "")
        },
        RemovedRow {
            reason: "SurvivorsForge blocked it: Sends your chat elsewhere".into(),
            reason_kind: 2,
            ..removed("Big Cursor", 1, "Added by you", "0.3.4", "yesterday 23:10", "Its settings went with it; they come back with it", "")
        },
        removed("Old Meter", 2, "From a file", "2.1", "27 Oct 18:02", "Its settings stayed in the game's folder", "It came from a zip file: import the zip again"),
    ])));
    ui.set_browse_note("18 Discord Verified · 15 on SurvivorsForge · the catalog of today 10:54".into());
    ui.set_browse_view("table".into());
    ui.set_browse_sort(2);
    ui.set_link_text("github.com/Hearts47/RunPace".into());
    ui.set_link_state(2);
    ui.set_link_owner("Hearts47 · 1 star · last change today 09:12".into());
    ui.set_link_release("v1.1.0 · RunPace.zip · licence MIT · checksum matches GitHub's".into());
    ui.set_link_goes("RunPace · nothing left out · kills and XP per minute".into());
    ui.set_link_note("Not reviewed by the game's devs. Addon code can't reach your PC, and it asks before each update.".into());
}

/// Renders every state of section 9 of the 0.9 plan, in both themes, at the three sizes.
fn main() {
    let out = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| "preview".into());
    fs::create_dir_all(&out).expect("the output folder could not be created");
    let windows: Windows = Rc::default();
    slint::platform::set_platform(Box::new(Preview { windows: windows.clone() })).expect("platform");

    let ui = MainWindow::new().expect("window");
    fill(&ui);
    ui.show().expect("show");
    let main = windows.borrow()[0].clone();
    // The window as it opens, at its smallest, and maximised on a Full HD screen.
    let size = PhysicalSize::new(1280, 720);
    let small = PhysicalSize::new(1120, 630);
    let large = PhysicalSize::new(1920, 1080);
    // Each state in both themes: "50-home" and "50-dark-home".
    let both = |name: &str, size: PhysicalSize| {
        for dark in [true, false] {
            ui.set_dark(dark);
            let file = if dark { name.replacen('-', "-dark-", 1) } else { name.to_string() };
            snap(&main, size, &out.join(format!("{file}.ppm")));
        }
        ui.set_dark(true);
    };

    // Home: What's new, Your PCs, Needs you with one question, Recent activity, Playtime closed.
    ui.set_tab(0);
    both("50-home", size);
    // The game runs: Play greyed, the session line.
    ui.set_playing(true);
    ui.set_state(TrayState::Playing);
    ui.set_progress_dot(2);
    ui.set_last_line("Session 12 min · your progress is sent when the game closes".into());
    both("50b-home-playing", size);
    ui.set_playing(false);
    ui.set_state(TrayState::Synced);
    ui.set_progress_dot(0);
    ui.set_last_line("Last played today, 34 min · Laptop".into());
    // The playtime card open.
    ui.set_playtime_open(true);
    both("50c-home-playtime-open", size);
    ui.set_playtime_open(false);
    // Sync off: no Your PCs card, the progress line's local wording.
    ui.set_sync_on(false);
    ui.set_progress_line("Progress saved today 13:20 · this PC only".into());
    both("50d-home-sync-off", size);
    ui.set_sync_on(true);
    ui.set_progress_line("Progress synced 2 min ago · 3 PCs".into());
    // Nothing yet: no notes, no activity, no playtime, no questions.
    let (news_version, news_blocks) = (ui.get_game_news_version(), ui.get_news_blocks());
    let (recent_lines, question_rows, day_bars, session_rows) = (ui.get_recent(), ui.get_questions(), ui.get_playtime_days(), ui.get_sessions());
    ui.set_build_line("The game's notes come here once read".into());
    ui.set_build_link(false);
    ui.set_game_news_version("".into());
    ui.set_news_blocks(ModelRc::new(VecModel::from(Vec::<ChangelogBlock>::new())));
    ui.set_recent(recent(&[]));
    ui.set_questions(questions(&[]));
    ui.set_playtime_week("0 min".into());
    ui.set_playtime_week_sessions(0);
    ui.set_playtime_days(days(&[("Thu", 0), ("Fri", 0), ("Sat", 0), ("Sun", 0), ("Mon", 0), ("Tue", 0), ("Today", 0)]));
    ui.set_sessions(sessions(&[]));
    ui.set_progress_line("No progress saved yet".into());
    ui.set_addons_line("Addons not checked yet".into());
    ui.set_addons_dot(4);
    ui.set_last_line("Not played from the Satchel yet".into());
    ui.set_pcs(pcs(&[("Laptop", true, "synced just now")]));
    ui.set_pcs_week("This week: 0 min · 0 sessions".into());
    both("50e-home-nothing-yet", size);
    ui.set_build_line(format!("Build {news_version}").into());
    ui.set_build_link(true);
    ui.set_game_news_version(news_version);
    ui.set_news_blocks(news_blocks);
    ui.set_recent(recent_lines);
    ui.set_questions(question_rows);
    ui.set_playtime_week("4 h 12 min".into());
    ui.set_playtime_week_sessions(6);
    ui.set_playtime_days(day_bars);
    ui.set_sessions(session_rows);
    ui.set_progress_line("Progress synced 2 min ago · 3 PCs".into());
    ui.set_addons_line("7 addons on · 1 waits for your OK".into());
    ui.set_addons_dot(3);
    ui.set_last_line("Last played today, 34 min · Laptop".into());
    ui.set_pcs(pcs(&[("Laptop", true, "played today · synced 2 min ago"), ("Desktop", false, "played yesterday · synced yesterday 21:40"), ("Study PC", false, "played Sun · synced Sun 19:05")]));
    ui.set_pcs_week("This week: 4 h 12 min · 6 sessions".into());
    // A newer Satchel downloaded from GitHub: the pill.
    ui.set_pill_text("Satchel 0.8.0 → 0.9.0 ready".into());
    ui.set_pill_link("Update now".into());
    ui.set_pill_kind(1);
    ui.set_update_version("0.9.0".into());
    both("50f-home-update-ready", size);
    ui.set_pill_kind(0);
    ui.set_update_version("".into());

    // Addons: the hybrid, Browse's table with the top row open and its thumbnails, the rail's
    // search and filters, the segmented row.
    let all = offers();
    let popular = by_popularity(&all);
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
    let lucky_bars: Vec<BarRow> = [("v0.9.0", "2,758", 2758.0), ("v0.8.0", "5,971", 5971.0), ("v0.7.0", "2,881", 2881.0)]
        .iter()
        .map(|(label, count, n)| BarRow { label: (*label).into(), count: (*count).into(), share: n / 5971.0 })
        .collect();
    let catalog = satchel_core::addons::catalog::Catalog::builtin();
    let notes = |id: &str| {
        catalog.get(id).and_then(|e| e.latest.notes.as_deref()).map(|n| satchel_core::addons::overview::plain_notes(n, 700)).unwrap_or_default()
    };
    let lucky_panel = PanelRow {
        id: "lucky-odds".into(),
        verified_author: true,
        about: "Rarity odds from your luck; caches and guardian lairs on the minimap.".into(),
        about_more: "See what your Luck actually does. LuckyOdds shows the rarity odds for level-up cards and amulets based on your current Luck, and marks Battleground Caches and guardian lairs on your minimap.".into(),
        notes_title: "What's new in 0.9.0".into(),
        notes: notes("lucky-odds").into(),
        bars: ModelRc::new(VecModel::from(lucky_bars)),
        scan: "verified · an admin approved this exact file".into(),
        folders: "Folders: LuckyOdds".into(),
        pictures: lucky_pictures,
        action: 3,
        ..Default::default()
    };
    let open_in = |list: &[Offer], id: &str, panel: PanelRow| {
        if let Some(offer) = list.iter().find(|o| o.row.id.as_str() == id) {
            ui.set_panel_offer(offer.row.clone());
        }
        ui.set_browse_panel_id(id.into());
        ui.set_panel(panel);
    };
    ui.set_tab(1);
    ui.set_addons_page(1);
    ui.set_table_tab(0);
    set_browse(&ui, &popular, &[], &[], &[], false);
    open_in(&popular, "lucky-odds", lucky_panel.clone());
    both("51-addons-rail-browse", size);
    // The filters: Safety open with two ticked, "Clear" shown, the table filtered.
    set_browse(&ui, &popular, &[], &["verified", "scanned"], &[], false);
    both("51b-addons-rail-filters", size);
    set_browse(&ui, &popular, &[], &[], &[], false);
    // Installed: the table under the segmented row, with Update all.
    ui.set_addons_page(0);
    both("51c-addons-installed", size);
    // Removed: the new table, one row opened.
    ui.set_addons_page(2);
    ui.set_removed_open("Old Meter".into());
    both("51d-addons-removed", size);
    ui.set_removed_open("".into());
    // The Add box over the table.
    ui.set_addons_page(1);
    ui.set_add_open(true);
    both("51e-addons-add", size);
    ui.set_add_open(false);
    // The cards and the list, with the bigger check mark.
    ui.set_browse_panel_id("".into());
    ui.set_browse_view("cards".into());
    both("51f-addons-cards", size);
    ui.set_browse_view("list".into());
    both("51g-addons-list", size);
    ui.set_browse_view("table".into());
    open_in(&popular, "lucky-odds", lucky_panel.clone());

    // Settings: General to Game; About and Progress versions open (the page scrolled to them);
    // a "✓ Saved" showing, a refused folder with its reason.
    ui.set_tab(2);
    both("52-settings-top", size);
    ui.set_saves_open(true);
    // Tall enough for the whole page: the maximised window shows About and Progress versions open.
    both("52b-settings-about-versions-open", PhysicalSize::new(1280, 1180));
    ui.set_saves_open(false);
    ui.set_saved_key("sync".into());
    ui.set_sync_folder(r"C:\Users\Player\AppData\Roaming\wenilla-rogue\Saves".into());
    ui.set_folder_status("✗ Can't be used: the network folder is inside the game's progress folder".into());
    ui.set_folder_status_kind(2);
    both("52c-settings-saved", size);
    ui.set_saved_key("".into());
    ui.set_sync_folder(r"\\server\share\Azeroth Survivors Saves".into());
    ui.set_folder_status("✓ Folder answers · 3 PCs seen · 27 versions".into());
    ui.set_folder_status_kind(1);

    // The first start: the game found, and not found.
    ui.set_tab(0);
    ui.set_setup(true);
    both("53-first-start-found", size);
    ui.set_game_ok(false);
    both("53b-first-start-not-found", size);
    ui.set_game_ok(true);
    ui.set_setup(false);

    // The minimum size and maximised on a Full HD screen: Home and Addons.
    both("54-home-1120", small);
    ui.set_tab(1);
    both("54b-addons-1120", small);
    ui.set_tab(0);
    both("55-home-1920", large);
    ui.set_tab(1);
    both("55b-addons-1920", large);

    // The viewer, over Addons: LuckyOdds' second picture, fitted, with its caption.
    ui.set_viewer_open(true);
    ui.set_viewer_index(1);
    ui.set_viewer_image(mock_shot(1280, 720, 210.0));
    ui.set_viewer_state(1);
    both("56-viewer", size);
    ui.set_viewer_open(false);
    ui.set_tab(0);

    // The game's notes over the window (the changelog's shape), to check the notes' lines.
    let file = satchel_core::changelog::versions(include_str!("../../../game/patch-notes.md"));
    ui.set_game_notes(game_notes(&satchel_core::game_news::merge(&file, &std::collections::BTreeMap::new())));
    ui.set_game_news_open(true);
    both("58-game-news-panel", size);
    ui.set_game_news_open(false);
    ui.set_whats_new_open(true);
    ui.set_changelog_open("0.9.0".into());
    both("58b-whats-new", size);
    ui.set_whats_new_open(false);

    // The Play card, in the theme's colours.
    let card = PlayCard::new().expect("card");
    card.set_dark(true);
    card.set_lines(card_lines(&[
        ("Network folder checked", true, false),
        ("Progress from Desktop brought in (saved today 00:05)", true, false),
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
    snap(&card_window, card_size(&card), &out.join("57-dark-play-card.ppm"));
    card.set_dark(false);
    snap(&card_window, card_size(&card), &out.join("57-play-card.ppm"));
    card.set_lines(card_lines(&[(
        "The game looks open on Desktop: started 14:02, still active a minute ago. If you play here too, the two will split and you'll pick one afterwards.",
        false,
        true,
    )]));
    card.set_asking(true);
    snap(&card_window, card_size(&card), &out.join("57b-play-card-ask.ppm"));
}
