# Survivor's Satchel 0.8: implementation plan

The whole brief for whoever implements 0.8, including an implementer who has only this repository.
Written 2026-11-04 against `main` after 0.7.2. **Branch from `main` only once the 0.7.2 pull request is
merged:** 0.8 touches `ui/app.slint`, `src/view.rs`, `src/worker.rs` and `src/net.rs`, which 0.7.2
also changes. Where this plan and the code disagree, read the code, then say so in the pull request.

## 0. Ground rules, read first

1. **Version and changelog first.** `version = "0.8.0"` in `Cargo.toml` and a dated `## 0.8.0 (YYYY-MM-DD)`
   section at the top of `CHANGELOG.md` before any other change (a test and the release workflow refuse a
   build without it; never wrap a code span across two changelog lines).
2. **Do not touch:** the update and signing path (`src/update.rs`, `crates/satchel-core/src/signing.rs`),
   the save-sync engine, the addon code paths, `catalog/`, the workflows, `build/`, `.cargo/`, the README's
   roadmap table. No new dependency: everything 0.8 needs is already in the tree (ureq, serde_json,
   chrono, the changelog parser).
3. **Serialized types:** no new enum variants in anything written to disk that an older Satchel reads;
   new meaning goes in optional fields with `#[serde(default)]`.
4. **Two network sources, both read-only, both text, both capped and cached.** Nothing fetched is ever
   executed, linked, or used to build another URL. The two URLs are constants in code.
5. **Nothing decoded or parsed on the UI thread;** the network thread fetches, the worker parses and
   caches, the window only shows.
6. **Player text:** "the game" or "Azeroth Survivors", never the developer's handle; "network folder",
   never "NAS"; a date is shown only when it is real.
7. **Commits:** plain messages, no trailers of any kind. The maintainer squashes the branch into one
   release commit at merge.

## 1. What 0.8 adds: the game's patch notes in the app

Players love reading the game's patch notes and hate opening Discord for them. The game publishes two
things the Satchel can read:

- **The live note.** The game's own update index, `https://wow.aethus.pl/exe/latest.json`, is the file the
  game reads to update itself. It is about 600 bytes of JSON: `version`, `notes`, `size`, `sha256`, `parts`.
  `notes` is one paragraph about the newest build, word for word the first paragraph of the developer's
  Discord post. The server answers `ETag` and `Last-Modified`, and 304 to `If-None-Match` or
  `If-Modified-Since`. It changes with every game build (several a week), holds only the newest build, and
  carries no date other than `Last-Modified`.
- **The full notes, maintained by hand.** `game/patch-notes.md` in this repository holds the full posts
  (the paragraph plus the detail lines), one section per version, in the Satchel's own changelog format
  (`## <version> (<date>)`, the date optional, then a paragraph, then `- ` lines or plain lines). The
  maintainer adds posts as they appear. The app fetches it from
  `https://raw.githubusercontent.com/masterpae/survivors-satchel/main/game/patch-notes.md`, so an edit on
  GitHub reaches every PC within half an hour, with no release.

The app merges the two: for every version it knows, the paragraph (from the file when present, else the
live note) and the detail lines (from the file when present, else a line saying the full notes are on
Discord).

### 1.1 Fetching and caching

- **Cadence:** both files at app start and then on the idle tick, at most once per 30 minutes each,
  counted from the last attempt and kept across restarts, exactly as the SurvivorsForge registry is polled
  today (`src/worker/forge.rs`, `forge::due`). Always `If-None-Match` with the stored ETag. A 304 keeps the
  cache. A 429 or a 5xx waits for the next tick. A 15-second timeout. Caps: 64 KB for the JSON, 512 KB
  for the markdown; a larger body is dropped and logged.
- **The helper:** one function beside `forge_json` in `src/net.rs` that fetches a URL from a fixed list
  of two, with the existing User-Agent, `max_redirects(0)`, the ETag, and returns the body, the ETag, the
  Last-Modified and the status in the same shape `forge_json` uses. No URL is ever built from data.
- **The cache,** a new `Local::game_news_dir()` beside `forge_dir()` in `crates/satchel-core/src/paths.rs`
  (`%LOCALAPPDATA%\SurvivorsSatchel\game-news\`): `latest.json` and `patch-notes.md` as received, and
  `state.json` with the two ETags, the last attempt and last answer times, and `seen`: version → the live
  note's text, the Last-Modified it came with, and the time this PC first saw it. Atomic writes, as the
  rest of the cache. An unreadable file is dropped and fetched again. A new live note adds a `seen` entry;
  an older version never overwrites a newer one's text.
- **Offline:** the cached copies show; nothing says "offline" unless there is nothing cached at all.
- **No switch in Settings:** the Satchel already talks to GitHub, and the game's update server is the
  game's own. About names both sources (section 1.4).

### 1.2 Parsing

- `latest.json`: `#[serde(default)]`, unknown fields ignored, `version` and `notes` strings; a body that
  does not parse counts as no answer and is logged with its first 200 characters.
- `patch-notes.md`: the existing `satchel_core::changelog::versions` reads it unchanged (it is the format
  `CHANGELOG.md` uses). Versions keep the file's order (newest first). Blocks render through the existing
  plain-text path the app uses for its own What's new.
- **The merge** (a pure function in satchel-core, tested): the file's versions plus every `seen` version
  the file lacks, newest first by version number (the numeric parse the catalog tool uses: 0.141.0 before
  0.139.2; unknown shapes last), each with: the version, the date (the file's heading date, else the
  Last-Modified of the live note, else none), the paragraph, the detail blocks (may be empty), and whether
  the file has it.

### 1.3 Where it shows

- **The Play card** gains one line under the game's name when a note exists: "Azeroth Survivors 0.141.0 ·
  what's new". The version is the live note's `version` (the newest build the game will take at its next
  start). The line is marked as new until the panel is opened once for that version, the way "Updated to …
  · see what's new" works for the Satchel's own updates (`worker.rs` `whats_new_seen`), with its own
  remembered version in `state.json`. A click opens the panel.
- **The panel,** "What's new in the game", is the pattern of the Satchel's own What's new (`app.slint`,
  `whats-new-open`): over the window, its own scroll, Esc or ✕ closes. Newest first; each version shows
  the version and date, the paragraph, then the detail lines when the file has them, else one line: "The
  full notes are on the game's Discord" (no link into Discord). At most 30 versions, then "older notes are
  in the game's Discord". A version seen live but absent from the file shows its paragraph alone.
- **About** gains, under "Where addons come from", a short block "Where the game's patch notes come from":
  the game's own update server for the newest build's note, and this repository's `game/patch-notes.md`
  for the full posts, with the time of the last answer from each.
- **Not a tab.** No version footer, no new tab; the panel is the place.

### 1.4 Texts

- The README's privacy paragraph gains the game's update server ("for the newest build's note") and the
  repository's patch-notes file ("for the full notes"); the Addons feature line is untouched.
- The README's "At a glance" is the maintainer's to edit at release; leave it.
- The changelog section (draft, adjust to what shipped):

```
## 0.8.0 (YYYY-MM-DD)

The game's patch notes, in the app.

### Added
- A line on the Play card says which game build is current and opens the game's patch notes: the
  newest build's note from the game's own update server, and the full notes kept in this repository,
  which players get within half an hour of an edit, without an update of the Satchel.
- About says where the game's notes come from.
```

### 1.5 Not in 0.8

Reading Discord in any form; a switch for the feature; translating the notes; detecting which build is
installed on this PC (the line names the newest build on the server); any change to the addons, the sync
or the update path; the README's roadmap tick.

## 2. The patch-notes file and how the maintainer feeds it

`game/patch-notes.md` ships seeded with the last five posts (added 2026-11-04). Its header comment states
the format. The maintainer pastes a Discord post as `## <version>` (with ` (YYYY-MM-DD)` when the date is
known), the paragraph, then the detail lines, dropping the developer's trailer line and any user mentions.
The app's tests read the real file, so a malformed edit fails the Check workflow instead of reaching
players as garbage; a version heading without a date is fine.

## 3. How to check the work on Linux

1. `cargo test -p satchel-core -p satchel-catalog`: the core tests, including the new ones below.
2. `rustup target add x86_64-pc-windows-msvc`, then `cargo check --release --target x86_64-pc-windows-msvc`
   with 0 warnings (no linker needed for a check).
3. `cargo run --release -p ui-preview -- dist/preview` (needs fontconfig and a font: `fonts-dejavu-core`,
   `libfontconfig-dev`, `pkg-config` on Debian): render the Play card with the new line (new and seen), the
   panel with a version from the file, one seen live only, and the empty state; both themes; attach the
   PNGs to the pull request.
4. `cargo tree -d` shows no new duplicate crate; `Cargo.toml` has no new dependency.
5. The changelog test, the version in `Cargo.toml`, `Cargo.lock` updated by cargo.

New tests expected: `latest.json` parsed from a hand-written fixture (and a body that is not JSON); the
merge (file + seen, newest first, a version in both, a version only seen, an empty file); the version
ordering; the cadence with a clock (`due` after 30 minutes, not before); `state.json` round trip with
`seen` kept across a newer live note and never overwritten by an older one; the size caps; the real
`game/patch-notes.md` parses and its first heading is the newest version.

## 4. Where things live today

| File | What 0.8 touches there |
|---|---|
| `crates/satchel-core/src/changelog.rs` | reused as is for `patch-notes.md` |
| `crates/satchel-core/src/paths.rs` | `game_news_dir()` |
| `crates/satchel-core/src/` (new module `game_news.rs`) | the JSON struct, `State`, the cache, `due`, the merge |
| `src/net.rs` | the two-URL fetch helper beside `forge_json` |
| `src/worker.rs` (and a new `src/worker/game_news.rs`) | the cadence on the idle tick, answers into the cache, the Play-card line, the panel's lines, "seen" |
| `src/view.rs` | the Play-card line, the panel model, the About block |
| `ui/app.slint` | the line, the panel, the About block |
| `tools/ui-preview/src/main.rs` | the mock states |
| `README.md`, `CHANGELOG.md`, `Cargo.toml` | the privacy paragraph; the 0.8.0 section; the version |

## 5. The pull request

One branch, `0.8`, from `main` after 0.7.2 is merged; one pull request against `main`. Its description
lists what was done and what was left (with the reason), every check of section 3 with its result, and the
screenshots. The maintainer reviews, merges as one commit, tags `v0.8.0`, and the release workflow builds
the exe.
