# Survivor's Satchel

> [!WARNING]
> **Early development: not meant for general use yet.** Features and screens still change from one
> release to the next. If you try it anyway, keep your own copy of your saves.

**Your Azeroth Survivors addons and progress, managed for you and the same on every PC you play on.**

![Home, dark](docs/screenshots/readme/home-dark.png#gh-dark-mode-only)
![Home, light](docs/screenshots/readme/home-light.png#gh-light-mode-only)

## Download

**[SurvivorsSatchel.exe](https://github.com/masterpae/survivors-satchel/releases/latest/download/SurvivorsSatchel.exe)**,
the newest release.

Windows 10 or 11, 64-bit; one exe, nothing else to install; put it in a folder of its own, not
inside the game's. Windows SmartScreen warns about an app it doesn't know yet ("Windows protected
your PC"): click **More info**, then **Run anyway**. To check the download against the maintainer's
signature, see [Checking a download](#checking-a-download).

## What you get

- 🧩 **Addons without the hassle.** Browse the addons verified in the game's Discord and the ones on
  SurvivorsForge, install with one click, let them update by themselves before you play; go back a
  version, keep one where it is, turn one off, or remove it to the Recycle Bin.
- 🛡️ **Checked before they install.** Downloads must match their published checksums, and
  SurvivorsForge's scans are followed: a blocked version never installs, a flagged or unscanned one
  waits for your OK.
- 🖥️ **The same game on every PC.** Your progress, the game's own settings and your addons with
  their settings follow you; graphics stay on each PC.
- 💾 **Nothing gets lost.** A backup before every change; two PCs that played at the same time both
  keep their progress, and you choose; after a crash, the next PC carries on from the last checkpoint.
- ⏱️ **Playtime** per PC: today, this week and in all; and **updates itself** in one click, only
  from the signed releases published here.

## Roadmap

| | Version | What it brings |
|:-:|---|---|
| ✅ | 0.6 | First public release |
| ✅ | 0.7.0 | Updates signed by the maintainer |
| ✅ | 0.7.1 | SurvivorsForge's addons and trust levels, Browse as cards |
| ✅ | 0.7.2 | Two more ways to browse (a list and a two-pane catalog), addon icons and screenshots, going back to any older version |
| ✅ | 0.8 | The game's patch notes inside the app; Browse as a table in a 16:9 window, with an Install column, thumbnails and the Installed tab in columns |
| 🔜 | 0.9 | A design pass over every screen, a first-start guide, and a clearer page here |
| | 1.0 | A code-signed exe Windows can recognize, an option to archive old save versions, and a month of real use without surprises |

Later, maybe: syncing through a cloud service, beside the network folder.

> Survivor's Satchel is a community project. It is not made by, endorsed by or affiliated with
> Aethus (the maker of Azeroth Survivors) or Blizzard Entertainment. It changes files the game
> reads; it backs them up first, but you use it at your own risk.

## Getting started

1. Run it. It finds the game in the usual places, or you point it at the game's folder; press Start.
2. Press **Play**. Before the game starts, the Satchel brings in newer progress from your other PCs
   (this PC's save is kept as a safety copy first) and puts your addons' updates in; the moment the
   game closes, your progress goes to the network folder.
3. For several PCs: in **Settings → Sync**, turn sync on and choose a network folder every PC can
   reach (on a network drive, or shared from one of the PCs), and do the same on each PC. Progress
   lives in that folder's `sync` subfolder as numbered versions, every one kept; each PC also keeps
   its own 30 newest safety copies. One network folder holds one player's progress; give each PC a
   name of its own in Settings → Sync.

Progress travels through a folder on your own home network: no account, no cloud. With one PC, the
Satchel is simply an addon manager with playtime and local backups. Addons come from two lists:
Discord Verified, the addons confirmed in the game's verified-addons channel on Discord, and
[SurvivorsForge](https://forge.rezondes.net), Rezondes' addon registry, which scans every version
(its trust level shows next to each addon). Any other addon comes in by its GitHub link or as a
zip, and is marked as not reviewed. The game updating itself is noticed: a restart of the game is
still one session. A release build looks here
on start and every few hours for a newer Satchel, and checks the download against the checksum
GitHub published for it and against the maintainer's signature; updates come only from here, never
from the network folder.

## What it touches

- Inside the game's folder, only `benilla-config\AddOns` (your addons, backed up first) and, with
  sync on, the game's settings files it carries (interface options, key bindings, macros, chat,
  camera, where windows sit; graphics, the window and the UI scale stay on each PC).
- The progress file `%APPDATA%\wenilla-rogue\characters.txt`, replaced only after a safety copy,
  with the game closed. A damaged or empty save never travels.
- `%APPDATA%\SurvivorsSatchel\settings.toml` (Settings → About shows the path).
- `%LOCALAPPDATA%\SurvivorsSatchel\` (backups, caches, the log).
- The network folder you chose, with sync on.

The Satchel connects to GitHub, to read releases and download addons; to SurvivorsForge, for its
addon list and details (you can turn that off in Settings); to the game's update server, for the
newest build's note; to this repository's `game/patch-notes.md`, for the full notes; and to the
network folder you chose. It sends nothing about you anywhere.

## How sync decides

Each PC remembers the version it last had. When the Satchel looks (at start, at Play, when the
game closes, and every minute while idle), it compares this PC's save, the version it last had, and
the newest version in the network folder:

- Only this PC changed: its progress goes up as the next version.
- Only the network folder changed: the newer version comes in before the game starts.
- Both changed (two PCs played without syncing in between): both are kept, and you pick.

While a PC plays, a small marker in the network folder says so, and the other PCs warn before they
start the game. A PC that crashed during a session leaves a checkpoint the next PC carries on from.
Saves are compared by content, never by file dates.

## Checking a download

From 0.7.0 every release carries `SurvivorsSatchel.exe.minisig`, a
[minisign](https://github.com/jedisct1/minisign) signature over the exe, made with the maintainer's
key, which never touches GitHub. The Satchel checks it before it takes an update. To check a
download yourself, with both files in one folder:

    minisign -Vm SurvivorsSatchel.exe -P RWT8U8YLPwCbLp80L/d7d2g3uRL8SXo+SSH5qaoxx5KZE1bmnLfIvRKw

Its last line shows the release's version as the trusted comment, `SurvivorsSatchel 0.7.0` for
this one.

## Code signing policy

The exe is not code-signed yet, so Windows warns once, as above. Every release carries the
maintainer's minisign signature, which the Satchel checks before any update, and which you can check
by hand as shown. A certificate-based signature, which Windows recognises by itself, is planned for
1.0.

## Building

The Windows exe is cross-compiled on x86_64 Linux in a throwaway container; the host needs only
Docker.

    sh build/build.sh            # dist/SurvivorsSatchel.exe
    sh build/build.sh test       # the core tests
    sh build/build.sh preview    # the screens as images, in dist/preview/
    sh build/build.sh catalog    # refreshes catalog/catalog.json from catalog/sources.toml

The build uses [cargo-xwin](https://github.com/rust-cross/cargo-xwin), which downloads the
Microsoft CRT and Windows SDK on first use; using it means accepting
[Microsoft's license terms](https://go.microsoft.com/fwlink/?LinkId=2086102). On Windows with Rust
and LLVM (`llvm-rc`) installed, `cargo build --release` works too.

A build from `main` may carry the next version number before that version is released. Once a PC
runs it, the other PCs using the same network folder stop syncing until they run that version
too, so use such a build on every PC or on none.

The repository holds:

- `src/`: the app (window, tray, the worker that syncs and manages addons);
- `crates/satchel-core/`: the sync and addon logic, tested on Linux;
- `ui/`: the window, in Slint;
- `catalog/`: the verified addons (`sources.toml`, ours) and what the catalog tool found for them
  on GitHub (`catalog.json`, built into the exe);
- `tools/`: the catalog tool and the screen preview.

Every release's changes since the first public one, 0.6.0, are in [CHANGELOG.md](CHANGELOG.md); the
app shows the same file under Settings → About → What's new.

## Problems and ideas

Problems and ideas go in [Issues](../../issues). Settings → About has **Report a problem**, which
opens a new issue with the Satchel's version already filled in.

## Credits

- Built with [Slint](https://slint.dev), under the Slint Royalty-free License 2.0.
- The game runs on [benilla](https://github.com/samwhosung/benilla), whose source showed where the
  game keeps its settings and addon data.
- The Discord Verified addons are the ones confirmed in the game's Discord; each belongs to its
  author, and the Satchel downloads them from the author's own GitHub releases.
- Addon descriptions, more addons and trust levels come from SurvivorsForge
  ([repository](https://github.com/Rezondes/SurvivorsForge), [website](https://forge.rezondes.net)),
  when its switch in Settings is on.

[![#MadeWithSlint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-light.svg#gh-light-mode-only)](https://slint.dev)
[![#MadeWithSlint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-dark.svg#gh-dark-mode-only)](https://slint.dev)

## License

MIT, see [LICENSE](LICENSE).
