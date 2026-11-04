# Survivor's Satchel 0.8, part 2: the table as Browse's default, the window at 16:9, the addon pane, the Installed tab in columns

The second half of 0.8, for the implementer who built the first half (the game's patch notes) on branch `0.8`.
Written 2026-11-04 from the maintainer's test of 0.7.2. **Continue on branch `0.8`, after its existing commits;
the version stays 0.8.0 and the `## 0.8.0` changelog section grows.** Part 1's plan is `docs/plan-0.8.md`; its
ground rules (section 0) hold here too, with these additions: no new dependency; no new enum variants in
anything written to disk that an older Satchel reads; nothing decoded on the UI thread; player text says
"network folder", never anything else; plain commits, no trailers; the maintainer squashes at merge. Where this
plan and the code disagree, read the code, then say so in the pull request.

Everything below comes from eleven asks made after a day with 0.7.2's three Browse views. The short version:
the table view wins, the window gets the shape the table needs, the addon pane loses its clutter, and the
Installed tab takes the table's style.

## 1. The window: 16:9

- `preferred-width` 1120 px, `preferred-height` 630 px; `min-width` 960 px, `min-height` 540 px (both 16:9).
  Today: 600 × 660, minimum 540 × 600.
- The table shows the selected addon beside it from 900 px of window width (today 1000 px), so the smallest
  window still holds both panes. The pane's width stays `max(400px, 40% of the row)`. The Tags column keeps
  its rule (two panes: from 1240 px). The one-pane mode (the pane in the table's place, with Back) is no
  longer reachable above the minimum size: keep it or remove it, and say which in the pull request.
- **The other tabs keep their layout at their old width.** Play, Saves, Playtime, Settings and the first-start
  screen: their content column is capped at 680 px and centered in the window. Overlays (the Satchel's What's
  new, the game's notes, Versions…, the remove question, the Legend) cap their text column at 760 px, centered.
  Nothing else changes on those tabs; a later release re-lays them out. The Play card (its own window) is
  untouched.
- The software renderer stays as it is: flat surfaces, no drop shadows, no rounded clipping (Slint 1.18.1).

## 2. Browse: the table is the default

### 2.1 Defaults

- A new or unreadable `settings.toml` opens Browse in the **table** view (`addons.browse_view` defaults to
  `table`; an unknown value falls back to `table`). A saved `cards` or `list` is kept: the player's choice.
  The settings tests follow.
- The default sort, in every view, is **Popularity, most downloads first** (sort index 2, not reversed; unknown
  counts last, as today). The sort is a session choice, as today.
- **The top row is selected whenever the table is shown and nothing is selected:** on entering Browse, on
  switching to the table, and when the selected addon leaves the filtered list. Selection does what a click
  does today: the pane opens, the details and versions are asked for. The cards and the list keep their
  behaviour (nothing opens by itself there).

### 2.2 Columns

From left: **Name · Safety · Version · Tags (wide only, as today) · Last updated · Popularity · Install.**

- **Name**, as today (icon, name, author), plus: an installed addon shows a green check mark right after its
  name (accessible label "installed"). The "Installed only" filter stays.
- **Lists** goes. Its filter (the two lists' check boxes under Filters) stays, as does the Legend's line about
  the game's verified-addons channel.
- **Safety** (96 px) replaces the "SurvivorsForge" column. Two lines in the cell:
  - an addon SurvivorsForge lists: its level word as today (verified · scanned · flagged · unscanned · left the
    list), in today's colour, with "SurvivorsForge" under it in muted 10 px text, so the level is never shown
    without who gave it;
  - an addon only on the game's verified-addons channel (our built-in list): **"Discord Verified"**, in the
    Discord badge colour, one line.
  - Under Filters, the row of SurvivorsForge's level check boxes is labelled "Safety" and keeps its boxes.
- **Install** (84 px, the last column, heading "Install"): one small button per row, from the addon's action:
  - Install (action 0): a primary button, "Install"; it installs as the pane's button does;
  - Install anyway (1): a plain button, "Install…"; a click selects the row so the pane shows why it asks and
    its "Install anyway" button; the row never installs an addon that waits for an OK;
  - Installing… (2): the spinner line the pane uses;
  - Installed (3): a plain button, **"Uninstall"**; it opens the same question the Installed tab's Remove…
    opens (Recycle Bin, keep its settings); after a removal the row says Install again;
  - Update (4): a primary button, "Update"; Update anyway (5): a plain button, "Update…", which selects the row
    as (1) does.
  - A click on the button never changes the selection except in (1) and (5). A click anywhere else in the row
    selects it, as today.

## 3. The addon pane: About · Versions

- **Two tabs: About and Versions.** The Pictures, Safety and Files tabs go. Versions is unchanged.
- **About's order:** the pictures row, the About text with its "more", one Safety line, one Folders line, then
  "What's new in <version>" and "Downloads per version" as today. The header (icon, name, author, downloads,
  badges) and the foot (version · date, View on GitHub, the action) stay.
- **The pictures row: thumbnails, loaded at once.** Frames of 88 × 55 px with 6 px gaps, as many as fit the
  pane's width (four at the narrowest pane, five from about 490 px), the last frame carrying "+N" over it when
  more pictures exist. A click on a frame opens the viewer at that picture. The same component replaces the
  placeholder strip in the cards' panel and the list's open row, and sits before the About text there too.
  - Thumbnails are decoded off the UI thread, as icons and the viewer's picture are, at a third size: a new
    constant beside `ICON_SIDE` and `SHOT_SIDE`, 192 px a side, the picture scaled down to fit (never up).
    The raw picture is kept in the existing picture cache by hash, under its existing caps; the window holds
    the decoded thumbnails of the open addon only and drops them when the pane closes or another addon opens,
    as it holds the viewer's one picture today.
  - They go through the existing picture queue: the viewer's picture first, then the thumbnails of the open
    addon (only the frames shown, never every picture), then icons. Thumbnails are asked for only while the
    pane is open for that addon and the Addons tab shows Browse; otherwise the waiting ones are dropped, the
    rule icons follow since 0.7.2.
  - A thumbnail that fails to decode shows today's placeholder frame. No pictures: no row, no text.
- **The Safety line.** For an addon SurvivorsForge lists: "Safety: " + today's scan text, with "Show what it
  found" when there are findings and the findings listed under it once asked for (today's Safety tab, without
  its heading). For an addon only on our list: "Safety: Discord Verified · on the game's verified-addons
  channel" + ", checked on <date>" when the date is known.
- **The Folders line:** "Folders: " + today's folders text, or "Its folders show once it is installed."; the
  ⚠ warning line under it, as today's Files tab shows it.

## 4. The picture viewer

- A centered box over the window instead of the whole window: width `min(1000px, 80% of the window)`, the
  picture fitted inside at most 80 % of the window's height; the backdrop darkens the whole window.
- In the box: the title line (addon · picture n of N) with ✕; the picture; the caption under it.
- **Previous and Next are round buttons on the picture's left and right edges**, vertically centered, greyed
  at the ends; the arrow keys work as today.
- **Closing: Esc, ✕, a click on the picture, a click on the backdrop.** The buttons never close it.
- Loading and "This picture isn't available." stay, inside the box.

## 5. The Installed tab in columns

Today each installed addon is a card that stacks its facts under its name. It becomes a row in a table of the
same style as Browse's, a click on the row still opening its detail area under it.

- **A heading row** (the table's heading component): **Name · Safety · Version · Released · Status · On.**
- **The closed row,** 44 px: icon, name and author (stretch) · Safety · Version · Released · Status · the
  on/off switch (absent for an incoming addon, as today).
  - Safety: the same words as Browse's Safety column, from the row's badges: the SurvivorsForge level with
    "SurvivorsForge" under it, or "Discord Verified"; and for the other kinds the existing badge word
    (GitHub link, Imported, By hand, Left the list).
  - Version: the installed version, with "→ <new>" in green beside it when an update is ready, as today.
  - Released: today's released text.
  - Status: today's one-line state ("Up to date", "Kept at this version; 0.8.0 is out", "Waits for your OK: …",
    "From <PC>"), in today's colour; elided when long, whole in the opened area.
- **The opened area** is today's: the problems, the notes, the buttons (Update now, Replace with…, Go back
  to…, Versions…, GitHub page, Folder, Remove…), the check boxes; plus the addon's badges on their own line
  at its top, since the closed row no longer shows them.
- The header line ("Up to date, except…", Update all), the sharing line and the footer stay.

## 6. Texts

Under the existing `## 0.8.0 (2026-11-04)` section, draft, adjust to what shipped:

```
### Added
- Browse's table has an Install column: Install, Update or Uninstall right in the row; an installed
  addon carries a check mark after its name.
- An addon's pictures show as a row of thumbnails above its description, in every view; a click on a
  thumbnail opens it, and a click on the picture closes it.

### Changed
- The window opens at 16:9 (1,120 × 630) and never smaller than 960 × 540, so the table and the addon
  beside it fit; the other tabs keep their width, centered, until the design pass.
- The table is Browse's default view, sorted by popularity with the top addon open. The Lists column
  is gone; the "SurvivorsForge" column is "Safety", and says "Discord Verified" for an addon that is
  on the game's verified-addons channel only.
- The addon pane has two tabs, About and Versions; the scan and the folders are one line each in About.
- The Installed tab shows its addons in columns, like the table.
```

The README's "At a glance" and roadmap are the maintainer's; leave them. `docs/plan-0.8.md` stays as written.

## 7. Not in part 2

A first-start wizard; settings applying without Save; the other tabs' new layout; new README screenshots;
changes to the cards' and the list's layouts beyond the shared pictures row; anything on part 1's "Not in
0.8" list; the catalog, the sync engine, the update and signing path, the workflows.

## 8. How to check the work on Linux

1. `cargo test -p satchel-core -p satchel-catalog`.
2. `cargo check --release --target x86_64-pc-windows-msvc` with 0 warnings.
3. `cargo run --release -p ui-preview -- dist/preview`: render, in both themes, at **1120 × 630**: the table
   with the top row open and its thumbnails, the table with "Install…" and "Uninstall" rows, the Installed
   tab, Play and Settings (the centered content); at **960 × 540**: the table with the pane; the viewer (shown
   and loading). Attach the PNGs to the pull request.
4. `cargo tree -d` unchanged but for the app's own version; no new dependency.
5. The changelog test, the version 0.8.0 untouched.

New tests expected: the default view and the fallback for an unknown value; the default sort; the Safety
cell's two lines from an addon's lists and level (a pure function); the Install column's text per action; the
thumbnail queue gated as the icon queue is (only while the pane is open and Browse shows, the viewer first,
the waiting ones dropped otherwise); the top-row selection when nothing is selected, as far as it lives in
testable code.

## 9. Where things live today

| File | What part 2 touches there |
|---|---|
| `ui/app.slint` | the window size; the table's headings, row and Install column; the pane; the pictures row component; the viewer; the Installed heading and row; the centering caps |
| `src/view.rs` | the offer row's Safety lines and action text; the top-row selection in the filter; the thumbnails the window holds |
| `src/worker/forge.rs`, `src/net.rs` | the thumbnail size in the picture job; the queue's order and gate |
| `crates/satchel-core/src/addons/forge.rs` | the thumbnail side constant |
| `crates/satchel-core/src/settings.rs` | the default view |
| `tools/ui-preview/src/main.rs` | the mock states and the two sizes |
| `CHANGELOG.md` | the 0.8.0 section grows |

## 10. The pull request

The same branch `0.8` and the same pull request (#6): add a "Part 2" section to its description with what was
done and what was left (with the reason), every check of section 8 with its result, and the screenshots
(under `docs/screenshots/0.8/`, as part 1's). The maintainer reviews both parts, squashes the branch into one
commit, tags `v0.8.0`, and the release workflow builds the exe.
