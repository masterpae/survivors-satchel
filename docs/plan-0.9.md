# Survivor's Satchel 0.9: the design pass

The whole brief for whoever implements 0.9, including an implementer who has only this repository.
Written 2026-11-04 against `main` after 0.8.0 (`816e8ad`). **Branch `0.9` from `main`.** The visual
spec is `docs/design/0.9-mock.html`: open it in a browser, or read it as text (the CSS variables, the
page sections and the notes under the window say what every part is). Its toolbar switches the theme,
the three window sizes, the first start and "Game running"; its Addons controls open on **"In the
rail"**, the arrangement to build (the other two are history). Where the mock, this plan and the code
disagree: the mock wins on looks, this plan on behaviour, the code on what exists; say so in the pull
request.

0.9 changes how every screen looks and where things sit. It adds no feature that fetches, syncs or
installs anything new: everything on the new screens is data the app already has.

## 0. Ground rules, read first

1. **Version and changelog first.** `version = "0.9.0"` in `Cargo.toml` and a dated `## 0.9.0 (YYYY-MM-DD)`
   section at the top of `CHANGELOG.md` before any other change (a test and the release workflow refuse a
   build without it; never wrap a code span across two changelog lines).
2. **Do not touch:** the update and signing path (`src/update.rs`, `crates/satchel-core/src/signing.rs`),
   the sync engine and its files (`engine.rs`, `index.rs`, `decide.rs`, `markers.rs`, `session.rs`,
   `gate.rs`, `savefile.rs`), the addon logic in `crates/satchel-core/src/addons/` beyond section 3.6, the
   game-news fetching, `catalog/`, the workflows, `build/`, `.cargo/`, the README's warning callout and its
   roadmap table (the maintainer's). **No new dependency.** 0.9 lives in `ui/app.slint`, `src/view.rs`,
   `src/main.rs`, `src/win.rs`, `src/worker.rs` and its halves, the preview tool, the README, and small pure
   helpers in satchel-core (settings, playtime, game news).
3. **Serialized types:** no new enum variants in anything written to disk that an older Satchel reads; new
   meaning goes in optional fields with `#[serde(default)]`.
4. **Nothing decoded, parsed or fetched on the UI thread;** the window only shows.
5. **Slint 1.18.1 on the software renderer:** flat surfaces; no drop shadows, no rounded clipping, no emoji
   glyphs, no gradients on paths (they arrive in 1.19); a linear gradient on a Rectangle is fine; never
   `width` and `min-width` on one element; the forced view after an install command stays as it is.
6. **Player text:** "the game" or "Azeroth Survivors", never the developer's handle; "network folder", never
   anything else; "progress", "addons", "PCs", "Play" as the only words for those things; a date or a time
   is shown only when it is real. **The same words everywhere:** a thing has one name on every screen.
7. **Commits:** plain messages, no trailers of any kind. The maintainer squashes the branch into one release
   commit at merge.
8. The principles the mock carries, as acceptance criteria: one primary action per screen (Play); every
   screen readable in three seconds; no setting a player has to discover (defaults work, the first start is
   one screen); details on click, never up front; nothing destructive without a question.

## 1. The shape: window, frame, top bar, rail, content

### 1.1 Sizes

- **Default 1,280 × 720, minimum 1,120 × 630** (both 16:9; 0.8's 1,120 × 630 default and 960 × 540 minimum
  go: the rail needs the width). Logical pixels; Slint scales them.
- **At start:** 1,280 × 720, centred on the work area (`win::work_area()`). If the work area is too small for
  that plus the frame (a 1,366 × 768 screen with the taskbar), open at the minimum; if even the minimum does
  not fit, open maximised. A pure function (work area → size or maximised), tested.
- **Maximised is remembered:** a new optional `window.maximised` bool in `settings.toml`
  (`#[serde(default)]`), written when the window closes to the tray or exits; a maximised window reopens
  maximised. Nothing else about the window is remembered. Use `slint::Window::set_maximized` and
  `is_maximized` if 1.18.1 has them (check its docs); else the winit window through the backend's accessor.
- **The three sizes to design and render:** 1,280 × 720 (default), 1,120 × 630 (minimum) and 1,920 × 1,080
  (maximised on a Full HD screen). The stretch rules are in 1.5.

### 1.2 The frame: native, painted

The window keeps Windows' own frame (title bar, caption buttons, resize borders, snapping). The app paints
the frame to match the theme through `DwmSetWindowAttribute` on the window's HWND (`src/win.rs` already
gets it through `raw-window-handle` in `bring_to_front` and `dark_title_bar`):

| Attribute (the `windows` crate, `Win32::Graphics::Dwm`) | Value | Dark | Light |
|---|---|---|---|
| `DWMWA_USE_IMMERSIVE_DARK_MODE` (already set by `win::dark_title_bar`) | BOOL | TRUE | FALSE |
| `DWMWA_CAPTION_COLOR` | COLORREF | `#1f232b` | `#e9ecf1` |
| `DWMWA_TEXT_COLOR` | COLORREF | `#e8ebf0` | `#1b1f27` |
| `DWMWA_BORDER_COLOR` | COLORREF | `#2b313c` | `#d9dde4` |

- COLORREF is `0x00BBGGRR`, the reverse of the CSS order: `#1f232b` is `0x002b231f`. Get this right: a swapped
  byte order paints the caption orange.
- The three colour attributes exist from Windows 11 build 22000 (Microsoft's `DWMWINDOWATTRIBUTE` page,
  read 2026-11-04). On Windows 10 the calls fail; ignore the error, and immersive dark mode alone stays (as
  today). `DWMWA_COLOR_DEFAULT` (`0xFFFFFFFF`) resets an attribute; never needed here.
- Applied right after the window is created (before it shows) and again at every theme change (Settings →
  Theme applies at once, section 4). One function `win::paint_frame(handle, dark)` replaces `dark_title_bar`.
- The window's title stays "Survivor's Satchel"; the icon in the caption is the app icon, as today.

### 1.3 The top bar

44 px tall, the full width (over the rail too), the window's background colour, a 1 px line under it.

- **Three tabs on the left:** Home · Addons · Settings, 14 px text, muted; the active one in the text colour
  with a 2 px accent line under it; hover brightens. The app's own component (`TopTab`), not the std
  `TabWidget` (it cannot look like this). `root.tab` stays an int: 0 Home, 1 Addons, 2 Settings; the tray's
  "open this tab" callbacks follow (section 7). Accessible role: tab.
- **The update pill on the right:** today's Play-tab update line, as a pill (a 1 px border, 14 px radius,
  12 px text): "Satchel 0.8.0 → 0.9.0 ready · **Update now**" while an update is downloaded (the link runs
  today's Update now), "Updated to 0.9.0 · **see what's new**" after an update until opened (today's
  `whats_new_seen`), nothing otherwise. Check for updates' "Checking…" stays in About.
- While the first start shows, the tabs are greyed and inert.

### 1.4 The rail

232 px wide, under the top bar, the full height, the panel colour, a 1 px line on its right. Top to bottom:

1. **The art block,** 120 px tall: "AZEROTH" (25 px, weight 800, letter-spacing 3 px) over "SURVIVORS"
   (15 px, weight 700, letter-spacing 5.4 px) in `#f0cf8a`, left-aligned with 18 px padding, on a linear
   gradient at 160° from `#4a2414` through `#221632` (55 %) to `#0c1726`. The mock adds a soft radial
   highlight; add it only if the software renderer draws radial gradients on a Rectangle, else leave it out.
   No image file: the lettering is Text.
2. **Three status lines** (11.5 px, 7 px apart, 12 px padding, each elided on one line):
   - **"Build 0.141.0 · What's new"** with a 14 px square outline in front: the newest game build from the
     game news (0.8's live note), "What's new" a link opening the game's notes panel (today's
     `game-news-open`). The square is filled with the accent while the newest note is unseen on this PC
     (today's new-until-opened mark on the Play line) and an outline once seen. No note cached yet: "Build
     unknown · What's new" is wrong; show "The game's notes come here once read" in muted text instead.
   - **The progress line,** with an 8 px dot in front coloured by the tray state (green synced, blue busy,
     gold playing, amber needs-you or offline): sync on: "Progress synced 2 min ago · 3 PCs" (the last
     successful send or bring-in, relative; the PC count from the shared index); sync off: "Progress saved
     2 min ago · this PC only" (the last local safety copy); nothing yet: "No progress saved yet". Offline or
     needs-you: the words today's Play-tab banner (`root.banner`) carries, in amber, elided.
   - **The addons line,** with the same kind of dot: "5 addons on · all up to date", "5 addons on · 2 updates
     at Play", "1 waits for your OK", "Addons not checked yet", from today's addons header state; the dot
     green, blue while a download runs, amber when one waits for an OK.
   - On the first start only the Build line shows.
3. **The Addons block,** only while the Addons tab shows (section 3.2): the search field and the filters.
4. **The Play row,** pinned to the bottom (12 px padding): **Play**, 48 px tall, the accent colour, white
   18 px bold text, rounded 6 px on the left; beside it the **gear**, 48 × 48, `#2766cc`, rounded on the
   right, 2 px apart. Play does exactly what today's "Play Azeroth Survivors" button does (the gate, the
   bring-in, the addons at Play, the Play card). The gear opens a small popup (`PopupWindow`, "Game
   options"): "Sync now" and "Open the network folder" (sync on, today's two Play-tab buttons), "Open the
   game's folder", "Game settings…" (Settings → Game).
   - **While the game runs:** Play reads "Playing…" (16 px), the hover colour, muted text, disabled; the gear
     stays usable. On the first start both are greyed and inert.
5. **The last line** (11.5 px muted, under the Play row): "Last played today, 34 min · Laptop" (the newest
   playtime session: its day, length and PC; "today", "yesterday", else the weekday or date); never played:
   "Not played from the Satchel yet"; while the game runs: "Session 12 min · your progress is sent when the
   game closes" (sync off: "… is backed up when the game closes"), the minutes ticking with the idle refresh.

### 1.5 The content area and the stretch rules

The content area is the window minus the top bar and the rail; 16 px padding; it scrolls when a page is
taller than it. The rail never scrolls except its filters block.

- **Home:** two columns, 1.25 : 1, 12 px apart, the pair capped at 1,100 px and centred; the cards grow
  with their content (no fixed heights); the page is top-aligned, so on a tall window the space is below
  the cards, never between them. Maximised keeps two columns (never three).
- **Settings:** one column capped at 680 px, centred; 900 px when the content area is 1,500 px or wider.
- **Addons:** the table and the pane fill the area; the pane is 400 px at the default size and
  `clamp(400 px, 40 % of the area, 520 px)` when wider; the Tags column shows from 1,240 px of table plus
  pane (0.8's rule). At the minimum size the table's columns narrow (Safety 96, Version 60, Last updated
  64, Popularity 60, Install 64 px; rows 11.5 px; two-line headings) and the pane keeps 400 px.
- **Overlays** (the Satchel's What's new, the game's notes, Versions…, the remove question, the Legend, the
  picture viewer) are unchanged from 0.8: over the whole window, their text column capped at 760 px, centred.
- **The "no game" panel** (today's `NoGame`) shows over Home and Addons as today when the game folder is
  missing after the first start; Settings stays reachable.

## 2. Home

Replaces the Play tab, the Playtime tab and the Play tab's two progress cards. Five cards; each has a
13 px bold heading and 12 px padding, the panel colour, a 1 px line, 6 px radius.

**Left column**

- **What's new in the game · 0.141.0.** The newest entry of the merged game notes (0.8's `game_news`): its
  paragraph (13 px), then its detail lines as bullets (12.5 px muted), then "Older notes ▾" (a link opening
  the game's notes panel, today's `game-news-open`). Nothing cached yet: the heading reads "What's new in
  the game" and the body one muted line, "The game's notes show here once the Satchel has read them."
- **Your PCs,** only while sync is on (the card is absent otherwise; the mock's "Shown only when sync is on."
  is a note to the reader, not app text). One row per PC in the shared index, this PC first with "(this PC)"
  after its name in muted text, each with "played <when> · synced <when>" right-aligned in 11.5 px muted
  ("played" from that PC's playtime log, "synced" from its newest version in the index; a PC with no
  playtime line shows only "synced …"). Rows 4 px apart with a 1 px line between. A footer line "This
  week: 4 h 12 min · 6 sessions" across every PC's playtime.

**Right column**

- **Needs you · N,** only while something waits; absent otherwise. Every question the app asks the player
  outside an overlay, one block each (a 3 px amber line on the left, 10 px padding, the question in 13 px,
  its buttons under it, 8 px apart): the progress choice when two PCs changed it (today's Play-tab cards
  and their buttons), the game's settings choice (today's "Keep this PC's" / "Use <PC>'s" / "Use them
  instead" rows in Settings → Sync), an addon's settings choice (today's `ask-pc`), an installed addon whose
  update waits for an OK ("BordoUI 2.8.18 waits for your OK: SurvivorsForge's scan found 1 warning in this
  version" with "Install anyway" and "Show what it found"), a PC-name clash, the network folder not answering
  ("Open Settings"). The buttons call today's callbacks; nothing new is decided here. The blocking questions
  at Play keep today's overlay (`needs-you`): the card is the list, the overlay the moment.
- **Recent activity.** The newest five lines of the activity log (today's "Recent on this PC"): the kind's
  icon (18 px column, the same icons as today), the text (elided), and the time right-aligned in 11.5 px
  muted ("2 min ago", "today 13:20", "yesterday 21:40", "Sat 16:30", "31 Oct"). `RecentLine` gains a `when`
  field; the view fills it from the log's timestamps (nothing on disk changes).
- **Playtime · this week 4 h 12 min · 6 sessions,** a card that opens on click (a ▾ at the right of the
  heading, ▴ when open). Closed: a 7-day bar chart of the last seven local days (28 px tall; bars at 60 %
  of a column's width, at most 28 px wide, the accent colour, 3 px radius on top; height proportional to
  that day's minutes; an empty day has no bar), with the weekday under each column (11 px muted, "Today"
  for the last). Open, under the chart: one line "Today 34 min · In all 61 h 20 min"; then "This week, per
  PC" rows (sync on only: today's `PcTime` rows); then a table of the last sessions (day · PC · minutes),
  newest first, 8 rows, with "All sessions ▾" extending it to 30. A pure `playtime::per_day` (the last seven
  local days from the records) in satchel-core, tested. No tooltips (1.18.1 has none).

## 3. Addons

### 3.1 The row above the table

One row (28 px controls, 8 px apart), then the table and the pane under it (10 px gap).

- Left: the **sub-pages as a segmented control**: "Installed · 5" · "Browse" · "Removed · 3" (one border, the
  selected part on the selection colour; a count only when it is above zero; "Removed" alone at zero).
- Right, in this order: **"+ Add"** (a plain button; opens today's Add panel, link or zip file, as a box over
  the table, 520 px wide, with its own ✕); on Browse only: **"Sort: Popularity ▾"** (today's three sorts),
  the **three view icons** (cards · list · table, today's switcher, the active one on the selection colour),
  and **Legend** (today's hover-to-show, click-to-pin legend).
- Installed keeps its header line ("Up to date, except …" and **Update all**) under the row, above the table;
  its sharing line and its footer stay under the table. Removed has only the row and its table.
- The chips row and the two-row controls of 0.8 go.

### 3.2 The rail's Addons block

Under the rail's status lines, separated by a 1 px line, while the Addons tab shows:

- **"Search addons"** (a field with a magnifier glyph, 12.5 px): today's search, applied to Installed, Browse
  and Removed alike (name, author, summary).
- **The filter groups,** each a heading row (12 px bold, a ▸ at the right, ▾ when open) with its check boxes
  under it: **Lists** (Discord Verified, SurvivorsForge), **Safety** (verified, scanned, flagged, unscanned),
  **Groups** (the catalog's groups, data-driven as today). One group open at a time: a click on a heading
  opens it and closes the others. A heading shows the number of ticked boxes after its name in the accent
  colour ("Safety · 2"). Safety opens by default the first time the tab shows.
- **"Installed only"** under a 1 px line, a check box. Hidden on the Installed sub-page.
- **"Clear"** (a link beside the search field) while any box is ticked; a click unticks every box.
- The rule, as today: within a group any ticked box matches; across groups all must; the filters apply to all
  three Browse views and to Installed's table (Lists and Safety by the row's badges). Removed takes the
  search only. When the selected addon leaves the filtered list, the top row is selected (0.8's rule).
- Nothing in this block scrolls the rail except itself (the Play row stays pinned); the block scrolls when
  the Groups list is long.

### 3.3 The table, the pane, the viewer

As built in 0.8 (`docs/plan-0.8-part-2.md`, sections 2–4), inside the new shell:

- Columns Name · Safety · Version · Tags (wide only) · Last updated · Popularity · Install; row height 48 px
  at the default size (0.8's 44 px rows were for a smaller window), 12 px text, the selected row on the
  selection colour, hover on the hover colour.
- **The installed check mark, much bigger:** a bold green ✓ at 18 px right after the name (today's is at the
  text size), vertically centred, accessible label "installed". The same mark in the cards and the list.
- The pane: header, badges, About · Versions, the thumbnails row (four frames at 400 px, five from 490 px),
  the About text with "more", the Safety line, the Folders line, What's new, Downloads per version, the foot
  (version · date, View on GitHub, the action). Nothing changes inside it but the colours.
- The viewer: 0.8's box over the darkened window.

### 3.4 Installed and Removed in the same style

- Installed: 0.8's table (Name · Safety · Version · Released · Status · On) with the opened area under a
  clicked row, unchanged in content.
- **Removed becomes a table** of the same style: Name (icon, name, author) · Safety · Version · Removed (the
  date) · Note (today's `detail` and `note`, elided; whole on hover is not possible, so the full text shows
  in a line under the row when the row is clicked) · the button (today's Install again, Install anyway, or
  none, with its reason under the row when clicked). Today's `RemovedItem` cards go.

### 3.5 The cards and the list views

Unchanged in their layouts; they lose their own filter rows and chips (the rail and the row above carry
them) and gain the bigger check mark.

### 3.6 Uninstall: an exact match

Browse's Uninstall removes an installed record only when that record's folders equal the offer's folders
exactly (0.8 allowed a subset with exactly one qualifying record, so a hand-imported pack holding a listed
addon's folders plus more could go to the Recycle Bin whole). With no exact match the row shows no
Uninstall; the Installed tab's Remove… still handles the pack. `overview::installed_line` and its test change
accordingly (the four cases become: exact match, superset, subset, none).

## 4. Settings

One column (section 1.5). Sections in this order, each a bordered box with a 13 px bold heading, 10 px
padding and 10 px between boxes, their backgrounds alternating panel and alt: **General · Sync · Addons ·
Game · About · Progress versions**, then **"Restore defaults"** right-aligned under the last. **No Save
button, no Default button:** every setting applies the moment it changes.

### 4.1 Apply on change

- A **switch** (the std `Switch`, 36 × 20) applies on click. A **segmented choice** applies on click. A
  **text field** applies when it loses focus or Enter is pressed, after today's validation (the network
  folder must exist, answer and not be a risky place; the PC name must be a valid file name no other PC
  uses; the game folder must hold the game). A refused value shows its reason under the field in red, the
  field keeps the typed text, and the saved value stays in force; the next valid value applies.
- Each row has a "✓ Saved" (11.5 px green, 54 px wide) left of its control that appears the moment the
  value applied and fades out over 0.4 s after one second (a Slint animation on opacity; no other
  animation anywhere).
- The settings file is written atomically at every change, as Save does today; the worker takes the new
  settings through today's path (the same message Save sends). Theme and the frame colours apply at once.
- The sync switch and the network folder keep today's semantics (turning sync on with a folder that fails
  validation shows the reason and leaves sync off).

### 4.2 Sections

- **General:** "Start with Windows, in the tray" · "Desktop shortcut "Play Azeroth Survivors"" · "Windows
  notifications" (switches) · "Theme": a segmented System · Light · Dark.
- **Sync:** "Sync my progress across PCs" (switch) with the line "Off by default. Turn it on on every PC that
  shares the folder." under it · "Network folder" (label 96 px, the field; under it a status line: "✓ Folder
  answers · 3 PCs seen · 27 versions" in green, or the refusal in red) · "This PC's name" (a 200 px field) ·
  while sync is on, "The game's own settings on every PC" (switch) with today's explanation line (interface
  options, key bindings, macros, chat, camera, where windows sit; graphics stay on each PC). The "Keep this
  PC's / Use <PC>'s / Use them instead" rows move to Home's Needs you card. The "◉ Local network folder /
  ○ Cloud (maybe later)" lines go (the roadmap keeps the idea).
- **Addons:** "Updates go in by themselves before Play" with its line (for Discord Verified and SurvivorsForge
  addons; one you added by link always asks first) · while sync is on, "The same addons and their settings
  on every PC" · "More addons, descriptions and trust levels from SurvivorsForge" with the line
  "SurvivorsForge, Rezondes' addon registry: it scans every version of the addons it lists." (the credit the
  registry's terms ask for). Today's Sources list, the two SurvivorsForge links and the registry's answer
  line move behind About's "Where addons come from".
- **Game:** "Game folder" (label, the field, "✓ found" in green or "not found" in red, **Find it**) · "Pause
  the game when I switch away (experimental)" (switch) with today's explanation line.
- **About:** one row: "Survivor's Satchel 0.9.0" · **Check for updates** (today's button, spinner and result
  line) · links: "What's new" (the Satchel's changelog overlay), "Where addons come from" (today's overlay,
  now holding the Sources list and links too), "Where the game's notes come from" (0.8's block, as an
  overlay or folded under the links), "Report a problem", "The Satchel on GitHub". Under it, muted: the
  release date and build, and "Settings are saved in <path>" (today's line from the Game section).
- **Progress versions** (the Saves tab, folded): the muted line "Every copy of your progress the Satchel
  kept; bring an older one back if something went wrong."; a summary row "27 versions · newest 2 min ago
  from Laptop · **Show ▾**" ("Hide ▴" when open). Open: a table Version · When · From · Size · Note · (a
  button), the newest first, the current one in bold with the note "the one you play" and no button, a
  version kept at a conflict with "kept when two PCs changed it", every other row with **Bring this one in**
  (today's button and question); then **Open the folder** (today's Show in folder). With sync off the table
  lists this PC's local safety copies, as the Saves tab does today. **Export a copy…** (a save dialog that
  copies the current version file) only if it is under an hour of work; else leave it out and say so.
- **Restore defaults:** sets every switch and the theme to a fresh install's values and says "✓ Defaults
  restored" for a second; it keeps the game folder, the network folder and the PC name (the three things a
  player typed). It never asks.

## 5. The first start

One screen, over the window (today's setup overlay; the tabs greyed; the rail shows the art and the Build
line only, Play greyed):

- "Welcome to Survivor's Satchel" (20 px).
- A box: "Your game" (muted) · the folder found (`win::detect_game_dir`, 13 px bold) · "✓ found" (green) ·
  "Change…" (a link opening the folder picker). Not found: "Not found" (red) · **Find it**; the folder picker
  must find the game before Start enables.
- **Start** (the accent colour, 44 px, 15 px bold, the card's full width): finishes the first start as
  today's Finish does and opens Home.
- Under it, centred, 12.5 px muted: "Play on more than one PC? Turn on sync in Settings whenever you like."
- **No sync question, no PC question, no theme question.** The defaults file beside the exe (a private file
  that pre-sets sync) keeps working: when it turns sync on, the box gains one muted line "Sync is on ·
  <folder>". Today's "which progress to keep" choice at the first start goes: the normal "two PCs changed
  it" question handles that case at the first Play (the overlay) and on Home (the card).

## 6. Theme and colours

- **Dark is the default** for a new settings file (`Theme::Dark` as the serde default); an existing file keeps
  its value (System, Light or Dark). System keeps following Windows at start, as today.
- The app's own colours (`Look`) take the mock's values; extend `Look` with the surfaces. Where a value
  differs from today's, the mock wins, but the badge inks must still read at 4.5 : 1 on their papers in
  both themes: check each changed pair and say so in the pull request.

| Role | Dark | Light |
|---|---|---|
| window background | `#14171d` | `#f6f7f9` |
| panel (rail, cards, table, pane, boxes) | `#1b1f27` | `#ffffff` |
| alt (alternate Settings boxes) | `#171a21` | `#fafbfc` |
| hover | `#222733` | `#eef0f4` |
| line | `#2b313c` | `#d9dde4` |
| text | `#e8ebf0` | `#1b1f27` |
| muted | `#9aa1ab` | `#5f6672` |
| accent (Play, links, the active tab's line) | `#2f7cf6` | `#2f7cf6` |
| accent 2 (the gear) | `#2766cc` | `#2766cc` |
| green | `#3cb371` | `#1f8a4c` |
| amber | `#e0a526` | `#b56d00` |
| red | `#e5534b` | `#b3312a` |
| caption (the frame) | `#1f232b` | `#e9ecf1` |
| selected row | `#1c2a44` | `#e3edfd` |
| green paper (badges) | `#183326` | `#e3f3e9` |
| amber paper (badges) | `#3a2f14` | `#fbf0dc` |

- The std widgets that stay (Switch, LineEdit, ComboBox, ScrollView, Spinner, CheckBox) keep the style's
  `Palette` under the chosen colour scheme, as today. Where one of them clashes visibly with the mock's
  surfaces, replace it by an app component (as `RowButton`, `Chip` and `ViewButton` already are); never patch
  the style.
- Text sizes: body 13 px; the tabs 14; headings 16 (pages) and 13 bold (cards, sections); the rail's status
  lines and the last line 11.5; table rows 12, table headings 11.5; captions 10 (under names). The default UI
  font as today (Segoe UI on Windows; the preview's DejaVu is narrower, so watch long rows there).

## 7. The tray and the Play card

- The tray menu: "Play Azeroth Survivors" · "Sync now" (sync on) · "Home…" · "Addons…" · "Settings…" · "Exit".
  "Saves…" and "Playtime…" go (their screens are a Settings section and a Home card). Each "…" item shows the
  window on that tab.
- The tray icon and its states are unchanged. The Play card (its own window at Play) is unchanged but takes
  the theme's colours.

## 8. The README: a player's page

Rewrite the README in this order, keeping every fact it carries today (nothing about sync, checking a
download, building, credits or the licence is dropped; some of it moves down or is shortened):

1. The title; the warning callout exactly as it is; the one-line bold pitch as it is.
2. **One screenshot of Home**, dark, 1,280 × 720, from the preview tool, as `docs/screenshots/readme/home-dark.png`,
   and the light one beside it with GitHub's `#gh-dark-mode-only` / `#gh-light-mode-only` trick (the Slint
   badges already use it). The maintainer replaces them with Windows captures after the release; say so in
   the pull request, not in the README.
3. **## Download:** a bold link to `https://github.com/masterpae/survivors-satchel/releases/latest/download/SurvivorsSatchel.exe`
   (the asset's name never changes), one requirements line ("Windows 10 or 11, 64-bit; one exe, nothing
   else to install; put it in a folder of its own, not inside the game's"), the SmartScreen two clicks in
   the player's words (More info, then Run anyway), and one line pointing at "Checking a download".
4. **## What you get:** the five bullets of today's "At a glance", trimmed to one line each where they run long.
5. The roadmap table, unchanged (the maintainer's).
6. **## Getting started:** three numbered steps (run it: it finds the game or you point at it; press Play;
   for several PCs, Settings → Sync and a folder every PC can reach), shortened from today's.
7. **## What it touches:** one short list: inside the game's folder only `benilla-config\AddOns` (addons,
   backed up first) and, with sync on, the game's settings files it carries; the progress file
   `%APPDATA%\wenilla-rogue\characters.txt` (replaced only after a safety copy, with the game closed);
   `%APPDATA%\SurvivorsSatchel\settings.toml`; `%LOCALAPPDATA%\SurvivorsSatchel\` (backups, caches, the log);
   the network folder you chose. Then the privacy paragraph (what it connects to; it sends nothing about
   you) as today.
8. **## How sync decides** (today's, shortened), **## Checking a download** (today's, unchanged).
9. **## Code signing policy:** the current state in plain words: the exe is not code-signed yet, so Windows
   warns once; every release carries the maintainer's minisign signature, which the Satchel checks before
   any update; a certificate-based signature is planned for 1.0. (The section exists now so the 1.0 wording
   has a place.)
10. **## Building, ## Problems and ideas, ## Credits, ## License:** today's, unchanged, at the end.

The community-project note stays under the roadmap. The Slint badges stay at the end of Credits.

## 9. The preview tool

Replace the states of the screens that go (the Play tab's, the Saves and Playtime tabs', 46 and 47) and
add, in both themes:

- `50-home` (1,280 × 720: What's new, Your PCs, Needs you with one question, Recent activity, Playtime closed),
  `50b-home-playing` (Play greyed, the session line), `50c-home-playtime-open`, `50d-home-sync-off` (no
  Your PCs card, the progress line's local wording), `50e-home-nothing-yet` (no notes, no activity, no
  playtime), `50f-home-update-ready` (the pill);
- `51-addons-rail-browse` (the hybrid: the segmented row, the rail's search and filters, the table with the
  top row open and its thumbnails), `51b-addons-rail-filters` (Safety open with two ticked, "Clear" shown),
  `51c-addons-installed` (the table under the segmented row with Update all), `51d-addons-removed` (the new
  table), `51e-addons-add` (the Add box), `51f-addons-cards`, `51g-addons-list`;
- `52-settings-top` (General to Game), `52b-settings-about-versions-open` (About and Progress versions
  open), `52c-settings-saved` (a "✓ Saved" showing, a refused folder with its reason);
- `53-first-start-found`, `53b-first-start-not-found`;
- `54-home-1120`, `54b-addons-1120` (the minimum size); `55-home-1920`, `55b-addons-1920` (maximised);
- `56-viewer` (over Addons).

Attach every PNG to the pull request (`docs/screenshots/0.9/`; the maintainer drops them at the squash).

## 10. Texts

Under `## 0.9.0 (YYYY-MM-DD)`, draft, adjust to what shipped:

```
The design pass: a new shape for every screen, dark by default.

### Added
- A Home page: the game's newest patch notes, what needs you, your PCs, recent activity and your
  playtime, as cards.
- The game and Play live in a rail on the left of every page; the rail says which game build is
  current, how your progress stands and whether your addons are up to date.
- Settings apply as you change them; each one says "✓ Saved".
- The first start is one screen: your game, found, and Start.

### Changed
- Three pages: Home, Addons and Settings. The Saves tab is the last section of Settings, "Progress
  versions"; the Playtime tab is a Home card that opens on click.
- Browse's sub-pages sit above the table; search and the filters sit in the rail. The installed check
  mark is much bigger.
- The window opens at 1,280 × 720 and never smaller than 1,120 × 630; maximised, the pages grow with it,
  and a maximised window reopens maximised.
- Dark is the default theme; the title bar follows the theme.
- Uninstall in Browse's table removes an addon only when its folders match the installed copy exactly.

### Removed
- The Save and Default buttons in Settings, and the "Cloud (maybe later)" line.
```

The README's "At a glance" becomes "What you get" (section 8). The roadmap tick is the maintainer's.

## 11. Not in 0.9

Splitting the worker's state by concern and the clippy/rustfmt gates (0.9.1); cloud sync; the Microsoft
Store; code signing; a multi-step wizard; new network fetches; tooltips; animations beyond the "✓ Saved"
fade; changes to the catalog, the sync engine, the update and signing path, the workflows; the roadmap tick.

## 12. How to check the work on Linux

1. `cargo test -p satchel-core -p satchel-catalog`.
2. `cargo check --release --target x86_64-pc-windows-msvc` with 0 warnings.
3. `cargo run --release -p ui-preview -- dist/preview`: every state of section 9, both themes; attach the
   PNGs to the pull request; look at each one against the mock before attaching it.
4. `cargo tree -d` unchanged but for the app's own version; no new dependency.
5. The changelog test, the version 0.9.0.

New tests expected: the dark default for a new settings file and a kept value for an existing one; the
window-size rule from the work area; `playtime::per_day`; the Needs-you list from the app's states (a pure
builder); the filter rule (within a group any, across groups all) where it is a pure function; the rail's
status-line words from the states (pure); Uninstall's exact match (section 3.6); the apply-on-change
refusals keeping the saved value (folder, PC name, game folder); the COLORREF conversion from a hex colour;
and the three 0.8 behaviours that shipped untested: the game-news body caps (64 KB and 512 KB), the
new-until-opened mark of the newest note, and the panel's 30-version cap.

## 13. Where things live today

| File | Lines | What 0.9 touches there |
|---|---|---|
| `ui/app.slint` | 5,007 | the whole window: the top bar, the rail, Home, the Addons shell, Settings, the first start; `Look`; the structs `RecentLine` (+ `when`) and `SettingsForm` (the fields stay; no Save) |
| `src/view.rs` | 1,377 | the models of every card and line; the rail's words; the Needs-you list; the relative times |
| `src/main.rs` | 484 | the window size at start, maximised remembered, the frame painted, the theme default |
| `src/win.rs` | 843 | `paint_frame` (caption, text and border colours) replacing `dark_title_bar` |
| `src/worker.rs` (+ `worker/share.rs`, `worker/addons.rs`, `worker/forge.rs`, `worker/game_news.rs`) | 2,249 (+ 1,550, 2,086, 1,004, 187) | settings applied per change; the first start without the progress pick; the Needs-you callbacks; the gear's actions |
| `crates/satchel-core/src/settings.rs` | 355 | the theme default; `window.maximised` |
| `crates/satchel-core/src/playtime.rs` | 166 | `per_day` |
| `crates/satchel-core/src/game_news.rs` | 613 | the three missing tests |
| `crates/satchel-core/src/addons/overview.rs` | 2,133 | `installed_line`: the exact match |
| `tools/ui-preview/src/main.rs` | 1,686 | the states of section 9 and the three sizes |
| `README.md`, `CHANGELOG.md`, `Cargo.toml` | | section 8; the 0.9.0 section; the version |
| `docs/design/0.9-mock.html` | | the visual spec (read-only) |

## 14. The pull request

One branch, `0.9`, from `main` at `816e8ad` or later; one pull request against `main`. Its description lists
what was done and what was left (with the reason), every deviation from this plan or the mock (with the
reason), every check of section 12 with its result, the contrast checks of section 6, and the screenshots.
The maintainer reviews, squashes the branch into one commit, tags `v0.9.0`, and the release workflow builds
the exe.
