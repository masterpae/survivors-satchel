# Changelog

What changed in each version of Survivor's Satchel, newest first. Every release gets a section
here before it is built; changes on their way to the next release collect under **Unreleased**.

## 0.8.0 (2026-11-04)

The game's patch notes in the app, and a wider window built around Browse's table.

### Added
- A line on the Play tab says which game build is current and opens the game's patch notes: the
  newest build's note from the game's own update server, and the full notes kept in this
  repository, which players get within half an hour of an edit, without an update of the Satchel.
- About says where the game's notes come from.
- Browse's table has an Install column: Install, Update or Uninstall right in the row; an
  installed addon carries a check mark after its name.
- An addon's pictures show as a row of thumbnails above its description, in every view; a click
  on a thumbnail opens it, and a click on the picture closes it.

### Changed
- The window opens at 16:9 (1,120 × 630) and never smaller than 960 × 540, so the table and the
  addon beside it fit; the other tabs keep their width, centred, until the design pass.
- The table is Browse's default view, sorted by popularity with the top addon open. The Lists
  column is gone; the "SurvivorsForge" column is "Safety", and says "Discord Verified" for an
  addon that is on the game's verified-addons channel only.
- The addon pane has two tabs, About and Versions; the scan and the folders are one line each in
  About.
- The picture viewer is a box over the darkened window, with previous and next on the picture's
  edges.
- The Installed tab shows its addons in columns, like the table.

## 0.7.2 (2026-11-04)

Three ways to browse, icons and pictures, and any version you like.

### Added
- Browse has three views: list, cards and table, with a switch in its top row; the choice is
  kept. The table view sorts from its column headings, filters with check boxes, and shows the
  selected addon beside it, or in its place in a narrower window.
- Addon icons from SurvivorsForge show as soon as Browse opens, and screenshots open in a
  viewer. Pictures come only from SurvivorsForge's own site and GitHub, and are kept on this PC
  once fetched.
- Versions…: every version of an addon in one list, with SurvivorsForge's word on each; go back
  to a kept one or install an older release. It stays at that version until you let it update.
- SurvivorsForge-only addons now show the date of their version and sort by last update and
  popularity, from its registry.

### Changed
- An addon's details and versions from SurvivorsForge stay cached when only its download count
  changed.
- An addon arriving from another PC that waits for your OK has its own button; Install again
  asks first when SurvivorsForge flags that version or hasn't scanned it.
- A version SurvivorsForge held back stays held back for the file it judged: a file replaced
  under the same version is judged again.

## 0.7.1 (2026-10-31)

SurvivorsForge in the Addons tab, and Browse as cards.

### Added
- SurvivorsForge, Rezondes' addon registry, is a second source of addons. Its addons install
  from Browse with one click, and its trust level shows next to every addon it lists. A version
  it blocks never installs; a version it flags, or hasn't scanned yet, waits for your OK. It can
  be turned off in Settings, under Addons, Sources.
- Browse shows addons as cards, with a panel for the details: what the addon does, downloads per
  version and SurvivorsForge's scan. Filters by list and by tag, sorting by name, last update or
  popularity, and a Legend that explains every badge.
- About explains where addons come from.

### Changed
- The built-in list is now called Discord Verified: the addons confirmed in the game's
  verified-addons channel on Discord.
- An update that SurvivorsForge flags, or hasn't scanned yet, waits for your OK instead of going
  in by itself.

## 0.7.0 (2026-10-31)

Signed updates, and from GitHub only.

### Added
- Every release from this one on carries `SurvivorsSatchel.exe.minisig`, a minisign signature
  over the exe made with the maintainer's key, which never touches GitHub. The Satchel now takes
  an update only when that signature checks out and names the release's version. A release
  without one, or with a wrong one, is refused, and nothing downloaded is kept.
- The README gives the public key and the command to check a download by hand.

### Changed
- A release starts as a draft and goes public only once its exe is checked and signed, so no
  Satchel sees a release before it is complete. Satchels before 0.7.0 take this one by its
  checksum, as before.

### Removed
- Updates from the network folder: a newer `SurvivorsSatchel.exe` placed beside `sync\` is no
  longer offered or installed. Updates come only from the Satchel's GitHub releases, signed. The
  network folder still carries progress, addons and settings.

## 0.6.2 (2026-10-31)

The built-in addon list brought in line with the game's verified-addons channel, and a missing
comma.

### Added
- Two addons from the verified-addons channel: SurvivorUI Classic, Survivor UI in the game's own
  look, and MoveAnything!, which moves, scales and hides any frame, other addons' frames too.

### Changed
- The built-in addon list is GitHub's of 31 October: newer releases for most addons, and the file
  Big Cursor's author replaced, so installing Big Cursor from Browse works again.
- An installed addon that leaves the built-in list says so: its row reads "No longer listed" and
  that the Satchel doesn't update it, and the Addons header no longer counts it as up to date.

### Removed
- BordoUI leaves the built-in list, as it left the game's verified-addons channel. A copy already
  installed stays and still works, but the Satchel no longer updates it, and Browse no longer
  offers it.

### Fixed
- The update lines on the Play tab and in Settings → About read "is ready, downloaded from
  GitHub", with its comma.

## 0.6.1 (2026-10-30)

Upkeep with nothing new to see, and the first update a 0.6.0 Satchel finds on GitHub by itself.

### Changed
- Addon zips, and the addon settings the PCs share, are read with version 9.0 of the zip library,
  which closes three security advisories about unpacking symbolic links and permission bits. The
  Satchel never unpacked that way: it reads every file itself and refuses an addon zip that holds
  a link.
- The builds use the current versions of GitHub's checkout and artifact actions, still pinned by
  commit, and a build image pinned by its digest. Dependabot proposes newer ones for both.
- The README says where the Satchel keeps its settings: the exe needs no file beside it.

## 0.6.0 (2026-10-30)

The first public release, on GitHub: from this version on, the Satchel knows where it comes from.
Versions 0.1.0 to 0.5.1 were private test builds. Their notes stay in the repository's history.

### Added
- **Report a problem** in Settings → About: in a release build, which knows its repository, it
  opens a new issue on GitHub with the Satchel's version already filled in.
- 0.6.0 is the first release build, so it is the first Satchel that looks at GitHub for a newer
  one (on start, every few hours and at **Check for updates**) and offers it with **Update now**,
  checked against the checksum GitHub published for it. The look came in 0.5.0; no earlier build
  knew its repository.

### Changed
- **Check for updates** shows a spinner and "Checking…" in its place until the answer arrives;
  before, nothing showed for a few seconds.
- The README explains Windows SmartScreen's warning on a downloaded exe, a build from `main` and
  the version check between PCs, and what the Satchel connects to.

### Fixed
- What's new shows `sync\` with its backslash again; before, a stray backtick stood in its place.
