# Survivor's Satchel

> [!WARNING]
> **Early development: not meant for general use yet.** Features and screens still change from one
> release to the next. If you try it anyway, keep your own copy of your saves.

**Your Azeroth Survivors addons and progress, managed for you and the same on every PC you play on.**

Survivor's Satchel is a small Windows app you start [Azeroth Survivors](https://wow.aethus.pl) from.
It installs your addons from trusted lists and keeps them up to date. If you play on more than one
PC, it carries your progress, the game's settings and your addons from one PC to the next, through
a folder on your own home network: no account, no cloud. On a single PC it is simply an addon
manager with playtime and backups.

## At a glance

- 🧩 **Addons without the hassle.** Browse the addons verified in the game's Discord and the ones
  on SurvivorsForge, install with one click, and let them update by themselves before you play.
  Go back a version, keep one where it is, turn one off, or remove it to the Recycle Bin.
- 🛡️ **Checked before they install.** Downloads from the lists must match their published
  checksums, and SurvivorsForge's scans are followed: a blocked version never installs, and a
  flagged or unscanned one waits for your OK.
- 🖥️ **The same game on every PC.** Your progress, the game's own settings (key bindings,
  interface, where windows sit) and your addons with their settings follow you. Graphics stay on
  each PC.
- 💾 **Nothing gets lost.** A backup before every change. Two PCs that played at the same time
  both keep their progress, and you choose. After a crash, the next PC carries on from the last
  checkpoint.
- ⏱️ **Playtime** per PC: today, this week and in all.
- 🔄 **Updates itself** in one click, only from the signed releases published here.

## Roadmap

| | Version | What it brings |
|:-:|---|---|
| ✅ | 0.6 | First public release |
| ✅ | 0.7.0 | Updates signed by the maintainer |
| ✅ | 0.7.1 | SurvivorsForge's addons and trust levels, Browse as cards |
| 🔜 | 0.7.2 | Two more ways to browse (a list and a two-pane catalog), addon icons and screenshots, going back to any older version |
| | 0.8 | The game's patch notes inside the app |
| | 0.9 | A design pass over every screen, a first-start guide, and a clearer page here |
| | 1.0 | A code-signed exe Windows can recognize, an option to archive old save versions, and a month of real use without surprises |

Later, maybe: syncing through a cloud service, beside the network folder.

> Survivor's Satchel is a community project. It is not made by, endorsed by or affiliated with
> Aethus (the maker of Azeroth Survivors) or Blizzard Entertainment. It changes files the game
> reads; it backs them up first, but you use it at your own risk.

## What it does

- **Play, with your newest progress.** Before the game starts, the Satchel brings in newer
  progress from your other PCs (this PC's save is kept as a safety copy first). The moment the
  game closes, your progress goes to the network folder.
- **Every PC the same, if you want it.** Progress; the game's own settings (interface options,
  key bindings, macros, chat, camera, where windows sit); and your addons with their settings.
  Graphics, the window and the UI scale stay on each PC. With one PC, the Satchel is simply an
  addon manager with playtime and local backups.
- **Careful when things go wrong.** Two PCs that played at the same time both keep their progress,
  and you pick. A PC that crashed during a session leaves a checkpoint the next PC carries on from.
  A damaged or empty save never travels. Nothing is replaced without a backup, and addons you
  remove go to the Recycle Bin.
- **Addons.** Two lists in the app: Discord Verified, the addons confirmed in the game's
  verified-addons channel on Discord, and [SurvivorsForge](https://forge.rezondes.net), Rezondes'
  addon registry, which scans every version (its trust level shows next to each addon, and a
  version it flags, or hasn't scanned yet, waits for your OK). Install, update (by themselves
  before Play, if you like), go back a version, keep one at its version, turn on and off, remove.
  Any other addon comes in by its GitHub link or as a zip, and is marked as not reviewed.
- **The game updating itself** is noticed: a restart of the game is still one session.
- **Playtime** per PC, today, this week and in all.
- **Updates itself**: a newer release here is one click away (Update now). A release build looks
  here on start and every few hours, and checks the download against the checksum GitHub
  published for it and against the maintainer's signature (see
  [Checking a download](#checking-a-download)). Updates come only from here, never from the
  network folder.

The Satchel connects to GitHub, to read releases and download addons; to SurvivorsForge, for its
addon list and details (you can turn that off in Settings); and to the network folder you chose.
It sends nothing about you anywhere.

## Getting started

1. Download `SurvivorsSatchel.exe` from [Releases](../../releases). Put it in a folder of its own,
   not inside the game's folder, and run it. It needs no installer and nothing else installed.
   Windows SmartScreen warns about an app it doesn't know yet ("Windows protected your PC"):
   click **More info**, then **Run anyway**.
2. It finds the game in the usual places, or you point it at the game's folder.
3. For several PCs: in **Settings → Sync**, choose a network folder every PC can reach (on a
   network drive, or shared from one of the PCs), and do the same on each PC. Progress then lives
   in that folder's `sync` subfolder as numbered versions, every one kept; each PC also keeps its
   own 30 newest safety copies. One network folder holds one player's progress; give each PC a
   name of its own in Settings → General.

Windows 10 or 11, 64-bit.

The exe needs no file beside it: the Satchel keeps its settings in
`%APPDATA%\SurvivorsSatchel\settings.toml` (Settings shows the path).

## Checking a download

From 0.7.0 every release carries `SurvivorsSatchel.exe.minisig`, a
[minisign](https://github.com/jedisct1/minisign) signature over the exe, made with the maintainer's
key, which never touches GitHub. The Satchel checks it before it takes an update. To check a
download yourself, with both files in one folder:

    minisign -Vm SurvivorsSatchel.exe -P RWT8U8YLPwCbLp80L/d7d2g3uRL8SXo+SSH5qaoxx5KZE1bmnLfIvRKw

Its last line shows the release's version as the trusted comment, `SurvivorsSatchel 0.7.0` for
this one.

## How sync decides

Each PC remembers the version it last had. When the Satchel looks (at start, at Play, when the
game closes, and every minute while idle), it compares three things: this PC's save, the version
it last had, and the newest version in the network folder.

- Only this PC changed: its progress goes up as the next version.
- Only the network folder changed: the newer version comes in before the game starts.
- Both changed (two PCs played without syncing in between): both are kept, and you pick.

While a PC plays, a small marker in the network folder says so, and the other PCs warn before they
start the game. Saves are compared by content, never by file dates.

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
