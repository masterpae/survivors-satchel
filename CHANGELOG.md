# Changelog

What changed in each version of Survivor's Satchel, newest first. Every release gets a section
here before it is built; changes on their way to the next release collect under **Unreleased**.

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

## 0.5.1 (2026-10-30)

The network folder by its name, a Check for updates button, each addon folder's own on/off on
every PC, and the first build that runs on a clean Windows.

### Added
- **Check for updates** in Settings → About: it looks at the exe beside `sync\` right away and, in
  a release build, asks GitHub now instead of at the next six-hour look, then says what it found.
- Each folder of an addon keeps its own on/off on every PC: Survivor Plates turned off in the game
  on one PC is turned off on the others, and Survivor UI itself stays on. Before, one switch stood
  for the whole addon.
- Settings refuses a PC name another PC already uses in the network folder: two PCs with one name
  would share a marker, a checkpoint and a playtime file.

### Changed
- "Network folder" everywhere the Satchel speaks of the folder the PCs share; "NAS" and "shared
  folder" are gone from its texts.
- Settings' sections alternate their background, so each is easy to find.
- The exe carries its C runtime, so it runs on a clean Windows without the Visual C++
  Redistributable.
- The one-time question after the first bring-in ("did the game show the progress brought in?")
  is gone, with the other checks of the first test builds: three PCs have answered them since
  29 October.

### Fixed
- Progress left unsent by a Windows shutdown or a crash during a session goes to the network
  folder at the Satchel's next look, instead of waiting for Play or Sync now; meanwhile another PC
  could start from the older version, and the two would split.
- A lock left behind in the network folder could be broken by two PCs at once, and both would
  then write the index. One PC breaks it now.
- After an update, the new Satchel waits a good two minutes for the old one to finish, instead
  of thirty seconds, so a slow last send can no longer leave the PC without a Satchel running.
- A checkpoint newer than this PC's save (a power cut that lost the game's last write) becomes a
  version at the next look, before the save is sent, and you pick between the two; both are kept.
- A PC whose name changed only in its case no longer takes its own old marker for another PC's.
- Sync now says that an addon's settings, or an addon change of this PC's, still have to go up
  when their send failed, instead of calling them the same as in the network folder.
- The first start refuses to finish while the game is running: the progress it brings in is
  only written with the game closed.

## 0.5.0 (2026-10-30)

Ready for its public repository: a README, the release builds and the catalog refresh as GitHub
workflows, and updates from the Satchel's own GitHub releases.

### Added
- Updates from the Satchel's GitHub releases: a release build looks on start and every six hours
  (twenty minutes after a look that failed), downloads a newer release's exe, checks it against
  the checksum GitHub published for it and against the version it calls itself, and offers it
  with **Update now**, as it does the one beside `sync\`. When a newer Satchel keeps this one out,
  that very release is looked for every ten minutes, a pre-release too. A build that isn't from
  the repository's release workflow never looks.

## 0.4.2 (2026-10-30)

A turned-off addon waits for you, and a session cut short at its very end can no longer take
every PC's progress back.

### Changed
- A turned-off addon is no longer updated when you press Play, and nothing is downloaded for it
  ahead of time. Its row still shows the newer release, and **Update now** on its row still works;
  turn it on and the next Play updates it. The Addons header mentions its update apart, and Update
  all leaves it alone. An addon of several folders counts as turned off only when all of them are.

### Fixed
- When the Satchel was closed while a session was still closing (up to three minutes after the
  game), another PC could later take that session's last checkpoint as the newest progress,
  although the progress sent when the game closed was newer; every PC would then have gone back
  to the checkpoint. A checkpoint now carries the time it was taken, and only one taken after the
  newest version can become the newest; after the send at the close, the session's marker names
  what was sent. Both protect the progress of a PC that runs 0.4.2. The marker such a session
  leaves in the shared folder is cleared once the Satchel runs again and its progress is on the
  NAS.
- Sync now no longer says the game's settings are the same as on the NAS when the shared list
  can't be read, when this PC's settings haven't gone up yet, or while the game is running.

## 0.4.1 (2026-10-30)

The Satchel updates itself from the shared folder, a PC that joins takes the game settings the
others share, and Settings comes in groups.

### Added
- **Update now**: when a newer Satchel is in the shared folder (beside `sync\`), the Play tab and
  Settings → About offer it. One click copies it to this PC, puts it in place of the running one
  and starts it. The game has to be closed; a click while its session is still closing goes in
  once the session is over. A new Satchel that can't run on the PC is taken out again, and the old
  one goes on.
- The shared files now record the newest Satchel that wrote them. An older Satchel syncs nothing
  (progress, addons, the game's settings) until it is updated, and Play asks first: update now, or
  play with this PC's progress as it is, synced after the update. This holds among Satchels from
  0.4.1 on.
- A PC that starts sharing the game's own settings takes the ones the other PCs share, instead of
  asking. Its own are kept in the shared folder and backed up, and Settings offers **Use them
  instead** until the settings are changed on this PC.

### Changed
- Settings in groups: General, Sync, Addons, Game and About. About is a card of its own, with the
  version in large type, What's new, and a smaller Slint badge.
- The line under "The game's own settings on every PC" names the PC they came from, also when
  another PC passed them on.
- Answers about the game's settings show in Settings for a minute, as well as on the Play tab.
- Find it, on the panel shown when the game isn't found, checks the game's folder against the
  shared folder as Save does, and keeps what was typed in Settings and not saved yet.
- Settings that come from another PC are backed up first, as before; now the 10 newest backups of
  each are kept and older ones go.

### Fixed
- Taking another PC's game settings checks once more, right before writing, that the game is still
  closed.

## 0.4.0 (2026-10-30)

The game's own settings on every PC, a list of removed addons, light or dark, and the Satchel's
own version and changes.

### Added
- With sync on, the game's own settings go to every PC too, the way the addons' settings do:
  interface options, key bindings, macros, chat, the camera, where windows sit (addon windows
  included), and the game's HUD switches, stored level-ups, combat text and combat log. They go up
  when the game closes and come to the other PCs before their next game, after a backup. When two
  PCs changed them, both are kept and you pick, in Settings. You pick too when a PC first meets
  them with settings of its own, and when a PC's settings lost files the others have (a game
  reinstalled): a fresh game's files are the newest, so age alone decides nothing. Settings of a
  hero played only on one PC stay there. Graphics, the window, the UI scale and the realm stay on
  each PC. Settings: "The game's own settings on every PC" (on with sync).
- Addons → **Removed**: every addon removed on this PC, by you or following another PC, with when,
  from where, and whether its settings stayed. **Install again** puts that version back, and its
  settings when they went with it.
- Settings → **Theme**: System, Light or Dark. System follows Windows' app mode, also when it
  changes; the title bar follows too.
- Settings → **About**: the Satchel's version, how long ago it was released and the build it came
  from, and **What's new**: every version's changes. The Play tab's last line shows the version
  too, and after an update it says "Updated to …" until its What's new is opened.
- When the game isn't found, Play, Addons, Saves and Playtime say so, with **Find it**, **Choose
  its folder** and **Get the game** (the game's website).

### Changed
- Settings scrolls.

### Fixed
- A Satchel pinned to the taskbar opens its window. Before, a pin made while the Satchel had been
  started by the Play shortcut started the game. A pin made before 0.4 still does: unpin it and
  pin it again.

## 0.3.2 (2026-10-30)

### Changed
- "Start with Windows, in the tray" is off on a new install, and Settings' **Default** turns it off:
  starting with Windows is something you choose. It is set for your Windows user only. PCs that
  already have it on keep it.

## 0.3.1 (2026-10-30)

### Changed
- Settings has two buttons, **Default** and **Save**. Save lights up only while something in the form
  differs from what's saved, and goes dim once it's saved. Default puts the switches back to how a
  new install starts (the game's folder, the shared folder and this PC's name stay); nothing sticks
  until Save. Before, Cancel and Save looked the same whether or not anything had changed.
- **Sync now** also checks the addons and says where they stand: "Synced: nothing to do. Addons and
  their settings are the same as on the NAS." Addon changes already go to the NAS the moment they're
  made; before, Sync now spoke only of progress, so a change just sent looked unsynced.

## 0.3.0 (2026-10-30)

The same addons, and the same addon settings, on every PC.

### Added
- With sync on, every PC has the same addons, at the same version, on or off alike, with the same
  settings. One list in the shared folder (`sync\addons\addons.json`) says what every PC has:
  - An addon installed, updated, turned on or off, kept at its version or removed on one PC goes
    into the list at once. The others follow at their next Play, in the seconds before the game
    starts, with their downloads fetched beforehand. The Play card says what came and from which
    PC.
  - A removal follows only when you removed the addon with the Satchel: it goes to the Recycle Bin
    on the other PCs too, with a backup. A folder that vanished from AddOns some other way comes
    back from the list instead.
  - Each addon's settings (its saved variables in `benilla-config\saved`, account-wide and per
    character) go up as a numbered version when the game closes, and come to the other PCs before
    their next game; the files they replace are backed up first. When a PC changed them while
    another did too, both are kept and you pick, as with saves; until then that PC keeps its own.
  - A zip imported from a file goes to the shared folder, so the other PCs can install it.
  - On a PC's first start with this version, every addon any PC has comes to it at its newest
    version, with the settings of the PC played last; the other PC's are kept as a version.
- Two switches on each addon's row: **Only on this PC** (it stays out of the list here, and the
  other PCs' changes don't touch it) and **Settings per PC** (every PC keeps its own settings for
  it, for screens that differ). Each row says where the addon stands with the other PCs; addons
  coming from another PC show before they arrive.
- Settings: "The same addons and their settings on every PC" (on with sync), and **Pause the game
  when I switch away**, an experiment, off unless turned on: when the game stops being the window
  in front (Alt-Tab, the Windows key, a click elsewhere, the lock screen), the Satchel sends it Esc,
  as a message to its window. If a menu is already open, that Esc closes it.

### Changed
- The Satchel now writes in the game's `benilla-config\saved` too, for addon settings: only with
  the game closed, and after a backup, as in AddOns.

### Fixed
- A new release seen only on GitHub's page, whose name there is a code (DragonUI's
  `build-d2129131…`), shows the version its own files carry once it's downloaded.
- The log names the game folder and safety copies plainly, and says "1 file left out".

## 0.2.2 (2026-10-30)

### Changed
- Install, Update now and Replace show "Installing…" with a spinner the moment they're clicked,
  until the addon is in or the Satchel says why it isn't. Before, the button kept saying Install
  while the download ran.
- Installed addons show when their version was released, beside it: "released 3 days ago". A
  release seen only on GitHub's page, or a zip from elsewhere, has no date to show.
- Importing a zip, or installing from a link, that is exactly the version already installed says
  so and changes nothing. Before, it was installed again, with a backup of the same files.

### Fixed
- The Installed list no longer says "Everything is up to date" while an addon is kept at an older
  version; it says how many are kept.
- An addon's notes are headed with the version they belong to, "What's new in 3.2.66", also for
  a newer release you're keeping away. Before, they read "About this version".
- Release notes no longer show a code block's language ("text") as a line of its own.
- "1 file that isn't an addon file was left out", not "1 files that aren't…".

## 0.2.1 (2026-10-29)

### Fixed
- The Addons tab reaches GitHub on Windows. In 0.2.0 every look for new releases and every
  download stopped at once with "the Satchel hit an internal error": the network library was
  built without the setting that gives it Windows' own secure connection. Sync wasn't affected,
  and addons stayed as they were.

### Changed
- The exe carries the network library's own list of root certificates, which that setting builds
  in; connections still use Windows' own certificates.

## 0.2.0 (2026-10-29)

Addons: found, installed, updated and kept safe from the Satchel's window.

### Added
- The Addons tab, next to Play:
  - **Installed**: every addon in AddOns, with who vouches for it (verified, added by link, from a
    file, installed by hand), its version and an on/off switch that writes the game's own list.
    Updates show in green. A click on a row opens what's new, Update now, Go back to the version
    before, Keep this version, its GitHub page, its folder, and Remove (to the Recycle Bin, its
    settings kept unless asked otherwise).
  - **Browse**: the verified addons, searchable and in groups, one click to install. An addon that
    doesn't mix with one already there says so before it is installed.
  - **Add**: any GitHub addon by its link or owner/name, shown before it is installed (owner,
    release, licence, checksum, the folders it brings and anything left out) with a "not reviewed"
    note, or a red warning when it looks like a verified addon from someone else. And "Import a
    zip file" for addons shared as files.
- Updates ride along with Play. Verified addons update by themselves in the seconds before the
  game starts (Settings can make them ask), addons added by link ask first, and an addon whose
  files changed on disk is never updated by itself. Downloads happen beforehand in the background;
  an update that fails its checks leaves the addon as it was, and the game starts anyway.
- New releases are looked up on GitHub's own pages, which need no account and don't count against
  GitHub's hourly limit: at the start, at Play when the last look is over half an hour old, and on
  "Check now". Each file is checked against the checksum GitHub published for it.
- DragonUI's own updater (`avrt.dll` beside the game) is respected: the Satchel leaves DragonUI to
  it and says so.
- The exe has its own icon and version details, and "Recent on this PC" an icon per kind of entry.
- The addon engine underneath:
  - the game's own rules for `.toc` files and for its on/off list (`AddOns\account.txt`);
  - addon zips judged before anything is unpacked: paths that leave their folder, links and zip
    bombs refuse the whole file, and programs and other non-addon files are never written;
  - addon folders found the way the game's own installer finds them, so a wrapper like
    `BigCursor-main\` is named after its `.toc`;
  - install and update all-or-nothing, with the version replaced and the addon's settings kept
    for going back (three earlier versions per addon), "Keep this version", and removal to the
    Recycle Bin with the settings kept if wanted;
  - addons installed by hand recognised when their files are exactly a known release, and files
    changed on disk noticed;
  - AddOns read as the game reads it: a wrong folder name, an extra folder level, an old
    `Interface` and a folder another tool turned off are each named, with the fix.
- The catalog, built into the Satchel: the addons in the game Discord's verified channel, each
  pinned to its author's GitHub repository, with the latest release and up to 50 earlier ones.
  Each release carries its file's checksum and every folder's fingerprint, so a copy installed by
  hand is recognised. `tools/catalog` refreshes it from GitHub with each build,
  downloading only releases it hasn't seen.

### Changed
- The tray icon tells its state by its shape as well as its colour: a check (synced), dots (busy),
  a play mark (playing), "!" (needs you), "×" (the NAS doesn't answer).
- Settings show the sync choice as "Local network folder", with "Cloud (maybe later)" greyed out,
  and a switch for addon updates.
- The Saves tab lists only the newest five of the game's own exports.
- The Play card grows with its lines.
- Downloads go through Windows' own secure connection and certificates; the exe bundles none.
- An internal error is logged and the Satchel carries on, so sync never stops over one.

## 0.1.2 (2026-10-29)

Five sync fixes, found by reading the code of the other community sync tools.

### Fixed
- A damaged or empty save never travels. Only a save in the game's own format (`RSE1:` and its
  data) is sent, kept as a checkpoint or a copy, or brought in, and this PC's save is read twice
  so a half-written file never goes out. If this PC's save is ever empty or damaged, the window
  says so, Play asks first, and the Saves tab brings the newest version back.
- A shared folder whose index file went missing (in a tidy-up, say) is rebuilt from its version
  files instead of starting over at version 1.
- A "Needs you" answer applies only to the two saves on screen: if another PC sent, or this PC's
  save changed, while the choice was open, it asks again.

### Changed
- A shared folder inside the game's folders or the Satchel's own, or holding one of them, or a
  whole drive, is refused, with the reason under the field in Settings. A folder that is a link
  (junction) to another place gets a warning.
- Each PC's memory of its last sync moved to `%LOCALAPPDATA%\SurvivorsSatchel`, so a roaming
  Windows profile can never carry it to another PC. It moves by itself on the first start; the old
  file stays as `state.json.moved`.

## 0.1.1 (2026-10-29)

Fixes from the first real run.

### Fixed
- Settings and the other tray items, and a second start of the Satchel, bring the window to the
  front.
- After a session the window shows what was just sent ("Game closed · your progress went up as
  version N"), not the check from before the session.
- The in-use marker's heartbeat covers the whole session, including the wait after the game
  closes.
- The game's update restart could be quicker than one look at the running processes, so the next
  ordinary close looked like an update; the game's files are now re-read while it runs.
- The Saves tab's PC column fits longer computer names.

## 0.1.0 (2026-10-29)

The first test build: the same progress on every PC.

### Added
- Sync through a shared folder (a NAS): Play checks it, brings newer progress in (a safety copy of
  this PC's save first, in the game's own Saves list) and starts the game; the save goes back the
  moment the game closes, as a new numbered version. Versions only accumulate.
- Decisions by content, never by clock: this PC's save, its last sync and the newest version. When
  two PCs moved on separately, the player picks, and both are kept.
- While the game runs, each save it writes goes up as a checkpoint; a PC that crashed or lost power
  leaves one, and the next PC carries on from it.
- A warning before playing when the game looks open on another PC. It never blocks.
- The game's own self-update restart stays one session.
- Windows' shutdown waits a few seconds while a send is running.
- Playtime per PC; a one-PC mode that keeps a copy after each session; a tray icon whose colour
  says the state; a window with Play, Saves, Playtime and Settings; start with Windows and a
  desktop shortcut; a daily log.
