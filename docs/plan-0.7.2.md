# Survivor's Satchel 0.7.2: implementation plan

This file is the whole brief for whoever implements 0.7.2, including an implementer who has
only this repository. It says what 0.7.2 adds, what it must not touch, how the work is checked
on Linux, and what the pull request has to show. Written 2026-11-04 against `main` at the 0.7.1
release plus the catalog robot's refreshes. Where this plan and the code disagree, read the code,
then say so in the pull request.

## 0. Ground rules, read first

1. **Version and changelog first.** Set `version = "0.7.2"` in `Cargo.toml` and write the
   `## 0.7.2 (YYYY-MM-DD)` section at the top of `CHANGELOG.md` before any other change, dated
   the day the pull request is opened (the maintainer corrects the date at release). A test
   (`the_version_being_built_has_a_dated_section`) and the release workflow refuse a build
   without it. Never wrap a code span across two changelog lines (another test refuses it).
2. **Do not touch:** the update and signing path (`src/update.rs`, `crates/satchel-core/src/signing.rs`),
   the save-sync engine, `catalog/sources.toml` and `catalog/catalog.json` (the catalog robot
   owns the second), the workflows under `.github/`, `build/`, `.cargo/`, the README's roadmap
   table (ticked at release by the maintainer), and the trust rule `forge::decide` except where
   section 1.5 says so.
3. **Dependencies.** One addition only: the `image` crate (the 0.25 line already in `Cargo.lock`)
   with `default-features = false` and the features `png`, `jpeg`, `webp`. No other new crate, no
   version bumps. `cargo tree -i image-webp` must show it pulled in by `image` alone.
4. **Serialized types.** No new enum variants in anything written to `addons.json`, the shared
   list in the network folder, `settings.toml` or the SurvivorsForge cache: an older Satchel reads
   those files before its version gate speaks. New meaning goes in optional fields with
   `#[serde(default)]`, and a new setting must read as its default when absent or unknown.
5. **SurvivorsForge's client rules stand:** the registry at most once per 30 minutes with
   `If-None-Match`; details and versions only when a player opens an addon or right before
   installing a version; a 429 waits a minute; every request carries the existing User-Agent.
   Icons and screenshots are static files on their host and may be fetched when they are shown,
   once, then kept.
6. **Hosts.** A picture or icon URL is fetched only from `forge.rezondes.net`, `github.com` or a
   `*.githubusercontent.com` host; anything else is refused and logged.
7. **Nothing is decoded on the UI thread.** No `Image::load_from_path`, no Slint
   `image-default-formats` feature: pictures are decoded on the network thread into RGBA8 and
   handed over as `slint::Image::from_rgba8`.
8. **Player text.** The built-in list is "Discord Verified"; every SurvivorsForge badge names
   SurvivorsForge; "network folder", never "NAS" or "shared folder"; a date is shown only when it
   is a real publish date. No new badge kinds.
9. **Commits.** Plain commit messages, no trailers of any kind (no `Co-authored-by`, no session
   links). The maintainer squashes the branch into one release commit under the repository's
   author at merge.

## 1. Scope

### 1.1 The registry's new fields: `icon_url`, `latest.published_at`, `downloads`

SurvivorsForge's registry (`GET /v1/registry`, schema 1) now carries, per addon, `icon_url`
(a WebP on `forge.rezondes.net/img/<sha256>.webp`; absent for some addons), the addon's total
`downloads` over all versions (null for tag-based addons, refreshed about every 15 minutes), and
inside `latest` a `published_at` (RFC 3339). On 2026-11-04 the registry listed 31 addons: 22 with
an icon, 30 with a count, all with a date.

- `ForgeAddon` gains `icon_url: Option<String>` and `downloads: Option<u64>`; `Latest` gains
  `published_at: Option<DateTime<Utc>>`. Same lenient, defaulted parsing as the other fields.
- **The entry fingerprint** (`forge::fingerprint`) must leave out `downloads`: with the count in
  it, every registry read changed every fingerprint and the cached details and versions were
  thrown away each time. Remove the top-level `downloads` key from the entry's value before
  hashing. Tests: a change of `downloads` alone keeps the fingerprint; a change of `latest.tag`
  changes it.
- **Last updated** for an addon that is only on SurvivorsForge comes from `latest.published_at`
  (no versions fetch needed). For an addon in our built-in list the catalog's own `published`
  keeps deciding, as today.
- **Popularity** for a SurvivorsForge-only addon is `downloads`; null sorts last, by name, and is
  never shown or treated as zero. Our list keeps the catalog's `downloads`.
- The card and row show the version with its date for SurvivorsForge-only addons now
  ("v1.1.1 · 3 days ago"), with the same date words the catalog's addons use.

### 1.2 Icons and screenshots

**Icons.**
- When Browse opens, and when the Addons tab lists an installed addon that is joined to a
  registry entry, every addon with an allowed `icon_url` shows its icon in the existing 34-pixel
  initials square (Browse cards and rows, the Browse panel's header, the installed rows). The
  square's size never changes; the initials stay when there is no icon, the switch is off, or
  the decode failed.
- A missing icon is queued for the network thread: one download at a time, after any install or
  update that is already waiting; 2 MB cap per file; decoded with `image` (PNG, JPEG, WebP),
  4096 pixels a side at most, scaled down to at most 128 pixels for the cache in memory. A failed
  download or decode is one log line and the initials, never an error on screen.
- Cache on disk: `%LOCALAPPDATA%\SurvivorsSatchel\forge\pictures\<first 16 hex of SHA-256(url)>`
  holding the bytes as received (the URL names the content, so a changed icon is a new file).
  The folder is capped at 50 MB; the least recently used files go first. Writes are atomic like
  the rest of the cache. With the SurvivorsForge switch off nothing is fetched and cached icons
  are not shown. Offline, cached icons still show (they are static).
- No avatars from GitHub as a fallback in 0.7.2.

**Screenshots.**
- The addon's details (fetched when its panel or row opens, as in 0.7.1) carry `images[]` with
  `url`, `width`, `height`, `caption`. Where the details are shown (the Browse panel, the opened
  row of the list view, the Pictures tab of the table view) a strip of small thumbnails appears
  with "Pictures · N"; a click on the strip opens a viewer over the window: one picture at a
  time fitted to the window, previous and next, the caption when there is one, Esc or ✕ closes.
- Pictures download and decode when the viewer opens, not before, one at a time, with the same
  caps, hosts and cache as icons. Only the shown picture is held decoded.

### 1.3 Three Browse views and a switcher

Browse gets three presentations of the same data. The filters (search, the list an addon comes
from, the tags), the three sorts and the Legend of 0.7.1 apply to all three, and the opened
addon's content (what the 0.7.1 panel shows: About, What's new, downloads per version, the scan,
folders, the action, the GitHub page) is one model reused by each view.

- **The switcher:** three small icon buttons in Browse's top row (list, cards, table) with the
  active one marked. The choice is kept per PC: a new setting `addons.browse_view` in
  `settings.toml`, a string, `"cards"` by default; an absent or unknown value reads as `"cards"`.
  It threads through the settings struct, its defaults and the preview tool's settings literal
  (the compiler lists every place). It is not shown in the Settings tab.
- **Cards** (the default; 0.7.1's view, unchanged): two columns from 540 pixels, three from about
  900; a click slides the panel over the cards. New in 0.7.2: the icon in the square, the
  pictures strip and Versions… in the panel.
- **List:** one row per addon, the whole width: the square, the name, the author, the badges;
  under it one line with the summary, the version and its date. A click opens the row in place
  (one row open at a time) with the opened addon's content: About with "more", the pictures
  strip, the scan in one line, Versions…, the GitHub page link and the action. It fits the
  540-pixel minimum width.
- **Table:** a dense table on the left, one row per addon: the square and name, short marks for
  the lists it is on, the version, SurvivorsForge's trust word, the tags, Last updated and
  Popularity. Clicking a column header sorts by it (name, last updated, popularity; the active
  column and direction marked); the Filters section offers the same filters as check boxes (each
  list, each trust word, each tag, "installed only"). The selected addon shows on the right with
  the tabs About · Pictures · Versions · Safety · Files (Safety is the scan and its findings;
  Files is the folders). Two panes from about 1,000 pixels of window width; narrower, the pane
  replaces the table with a Back button. The window's preferred and minimum sizes stay as they
  are (600 × 660, 540 minimum); the table view must work at every width through its one-pane
  mode. Widening the window once when the player picks the table view, if the screen allows, is
  welcome but not required.
- In Slint, the three views are conditional subtrees of the Browse page bound to one property;
  the Legend stays a conditional element, not a PopupWindow.

### 1.4 Versions…

A Versions… button for an addon, in the cards panel, the list view's opened row, the table
view's Versions tab, and the Addons tab's opened row of an installed addon.

- **The list**, newest first, one row per version, three sources merged: the installed version
  (marked "installed"), the kept earlier versions ("kept on <date>", the backups the Satchel
  already keeps for Go back), the catalog's releases for an addon in our list (date, downloads),
  and for an addon SurvivorsForge lists its versions (the versions endpoint, fetched on open as
  0.7.1 already does: date, size, its trust word and the findings count, the game range when
  given).
- **One action per row.** "Go back to this" on a kept version swaps it in from its backup the way
  Go back works today: `go_back` gains a target (which kept version) instead of always taking the
  first. "Install this version" on a release or a SurvivorsForge version downloads and installs
  it through the existing install path and its checks (the checksum, the folders, SurvivorsForge's
  word on that exact version, which may ask first or refuse, with the reason shown).
- **Pinned.** A version installed from this list is pinned the way Go back pins, so the
  auto-update does not replace it at the next Play; the row says "kept at this version until you
  let it update", and the existing path that lets an addon update again unpins it.
- A SurvivorsForge version is installable only with a `sha256` and an `asset_url` on the addon's
  own GitHub repository (the existing check).

### 1.5 Two follow-ups from the 0.7.1 review

- **OK buttons where the OK is needed.** An incoming addon from another PC that waits for the
  player's OK (SurvivorsForge flags the version, or hasn't scanned it) gets its own "Install
  anyway" button with the reason on its row, instead of sending the player to Browse. "Install
  again" on a removed addon whose version SurvivorsForge flags or hasn't scanned shows the reason
  and asks the same way instead of installing silently. Both go through the existing verdict and
  OK path, nothing new in the rule.
- **The judged file, keyed by its bytes.** The record of "this version was judged" is keyed by
  the tag today; key it by the file's SHA-256 (tag plus hash), so a file replaced under the same
  tag is judged again. If this turns out larger than a small change, leave it and say so in the
  pull request.

### 1.6 Not in 0.7.2

Avatars; dependencies and game ranges beyond showing what the registry gives; any change to
which addons are in the built-in list; the README's roadmap and feature text (the maintainer
edits them at release); a changelog of the game; a redesign of the other tabs; cloud sync.

## 2. Changelog draft

Adjust to what actually shipped.

```
## 0.7.2 (YYYY-MM-DD)

Three ways to browse, icons and pictures, and any version you like.

### Added
- Browse has three views: list, cards and table, with a switch in its top row; the choice is
  kept. The table view sorts by any column and shows the selected addon beside it.
- Addon icons from SurvivorsForge show as soon as Browse opens, and screenshots open in a
  viewer. Pictures are kept on this PC once fetched.
- Versions…: every version of an addon in one list; go back to a kept one or install an older
  release. It stays at that version until you let it update.
- SurvivorsForge-only addons now sort by last update and popularity, from its registry.

### Changed
- An addon's details and versions from SurvivorsForge stay cached when only its download count
  changed.
- An addon arriving from another PC that waits for your OK has its own button; Install again
  asks first when SurvivorsForge flags that version.
```

## 3. How to check the work on Linux

The repository builds the Windows exe in a container (`build/`), which is not available
everywhere. The checks below need only a Rust toolchain of the version `Cargo.toml` names.

1. `cargo test -p satchel-core -p satchel-catalog`: the core tests run natively on Linux.
2. `rustup target add x86_64-pc-windows-msvc`, then
   `cargo check --release --target x86_64-pc-windows-msvc`: the app crate for Windows, with
   0 warnings. `cargo check` needs no linker, so the Windows SDK is not needed. If the target's
   standard library cannot be installed where you are, say so in the pull request.
3. `cargo run --release -p ui-preview -- dist/preview`: renders the screens to PPM files without
   Windows. It needs fontconfig and a font (`fonts-dejavu-core`, `libfontconfig-dev`,
   `pkg-config` on Debian). Convert the PPMs to PNG, look at every new state in light and dark,
   and attach them to the pull request: the switcher, the list view with a row open, the table
   view at about 1,100 pixels and in its one-pane mode, the viewer, icons on cards and rows,
   Versions… with all three sources.
4. `cargo tree -i image-webp` shows it under `image` only; `cargo tree -d` shows no new
   duplicate crate.
5. The changelog test, the version in `Cargo.toml`, `Cargo.lock` updated by cargo.
6. `grep -rn "load_from_path\|image-default-formats" src crates ui tools` finds nothing new.

New tests expected: the fingerprint rule; parsing of the three new fields from a small
hand-written fixture (not a copy of the live registry); the two sorts with nulls last; the
picture host check; the pictures cache's cap and eviction; `browse_view` read, written and
defaulted; `go_back` with a target; a pinned install from Versions…; the verdict on Install again
and on an incoming row's OK.

## 4. Where things live today

| File | Lines | What 0.7.2 touches there |
|---|---|---|
| `crates/satchel-core/src/addons/forge.rs` | 1,799 | the structs, `fingerprint`, the cache (plus the pictures folder), the host check |
| `crates/satchel-core/src/addons/overview.rs` | 1,389 | the offers' icon, date and popularity from the registry; the Versions list model |
| `crates/satchel-core/src/addons/install.rs` | 682 | `go_back` with a target; a pinned install of a chosen version |
| `crates/satchel-core/src/settings.rs` | 231 | `addons.browse_view` |
| `src/net.rs` | 534 | picture download and decode on the network thread |
| `src/worker/forge.rs` | 680 | the icon queue, the viewer's pictures, versions for the list |
| `src/worker/addons.rs` | 1,759 | Install again and the incoming row's OK; Versions… actions |
| `src/view.rs` | 1,023 | the lines gain an icon; the opened addon's content gains pictures and versions; new commands |
| `ui/app.slint` | 2,961 | the switcher, the list and table views, the viewer, Versions… |
| `tools/ui-preview/src/main.rs` | 1,003 | mock states for every new screen |
| `CHANGELOG.md`, `Cargo.toml` | | the 0.7.2 section; the version and the `image` dependency |

## 5. The pull request

One branch, `0.7.2`, one pull request against `main`. Its description lists what was done and
what was left (with the reason), every check of section 3 with its result, the exe size change
if known, and the screenshots. The maintainer reviews, merges as one commit, tags `v0.7.2`, and
the release workflow builds the exe.
