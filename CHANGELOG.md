# Changelog

What changed in each version of Survivor's Satchel, newest first. Every release gets a section
here before it is built; changes on their way to the next release collect under **Unreleased**.

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
