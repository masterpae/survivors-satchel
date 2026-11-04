# gen-39 — Map of the worker for the 0.9.1 split

Source: branch `pr-0.9` at `16fa767` ("The README as a player's page…"), read with `git show`; nothing built or run.
Paths below are relative to the repository root; `worker.rs` means `src/worker.rs`, `addons.rs` means `src/worker/addons.rs`, and so on.

Method: every `fn` inside the `impl Worker` blocks of the five worker files (201 methods) was indexed by a script for the `self.<field>` paths it reads or writes, including access through local aliases (`let a = &mut self.addons;`), and for the `self.<method>()` calls it makes. Read/write is classified syntactically (assignment, `&mut`, `insert/remove/push/take/…`), so it is approximate. One known miss was corrected by hand: `save_settings` writes `engine.state.pc_names` (worker.rs:1476). Each method was then assigned to one concern (its file, or its section in worker.rs); a field is "coupled" when a method of another concern touches it.

Concern codes used in the tables: **SYNC** (progress, index, markers, gate, session), **PT** (playtime), **ADD** (addons), **SHR** (sharing between PCs: addon list, addon settings, the game's settings), **FRG** (SurvivorsForge), **NEWS** (the game's patch notes), **SET** (settings, the first start, the game folder), **WIN** (window, tray, Play card, banner, view building), **UPD** (the Satchel's own update), **INF** (shared infrastructure: engine, loop, dispatch, timers).

---

## 1. The `Worker` struct's fields

`Worker` (worker.rs:185-251) has **50 direct fields**. Four of them are already per-concern sub-structs, and one is a derived-model struct:

| Sub-struct | Defined at | Fields |
|---|---|---|
| `Lists` | worker.rs:167-183 | 11 |
| `addons::AddonsState` | addons.rs:105-141 | 24 |
| `share::ShareState` | share.rs:33-55 | 10 |
| `forge::ForgeState` | forge.rs:65-115 | 22 |
| `game_news::NewsState` | game_news.rs:24-36 | 6 |

In total there are **50 direct + 73 nested = 123 fields**. A further block of shared state lives inside `engine` (`engine.settings.*`, `engine.state.*`) and is tabulated in 1.11, because it carries several of the couplings.

### Field count per concern

| Concern | Direct fields | Nested fields | Total |
|---|---|---|---|
| SYNC | 20 | 7 (`Lists` progress part) | 27 |
| PT | 2 | 3 (`Lists` playtime part) | 5 |
| ADD | 1 (`addons`) | 24 | 25 |
| SHR | 1 (`share`) | 10 | 11 |
| FRG | 1 (`forge`) | 22 | 23 |
| NEWS | 1 (`news`) | 6 | 7 |
| SET | 1 (`setup`) | 0 (+ `engine.settings`) | 1 |
| WIN | 10 (incl. the `lists` container) | 1 (`Lists.recent`) | 11 |
| UPD | 7 | 0 | 7 |
| INF | 6 | 0 | 6 |
| **Total** | **50** | **73** | **123** |

In the tables below, "Other concerns" lists the concerns outside the owner that touch the field. W means writes, R means reads. Function names follow each.

### 1.1 SYNC — progress, index, markers, gate, session (20 direct)

| Field | Line | Type | Owner's writers | Other concerns |
|---|---|---|---|---|
| `tracker` | 188 | `Tracker` | (mutated only through `tracker.tick`, worker.rs:694) | **ADD** R `addons_blocked`:799, `addons_view`:1674 · **SHR** R `share_look`:418, `share_follow_now`:1082, `game_settings_status` · **UPD** R `update_now`:2211 · **WIN** R `build_view` · **INF** R `tick`, `addons_answer`:553,557 |
| `marker` | 193 | `Option<InUse>` | `session_started`, `watch_save`, `heartbeat`, `marker_names`, `session_over` | none |
| `sent` | 196 | `Option<u64>` | `session_started`, `send_after_close`, `session_over` | **WIN** R `build_view`:1941 |
| `launched` | 197 | `bool` | `play`:1015, `session_over`:972 | none |
| `live_seen` | 198 | `Option<SystemTime>` | `session_started`, `watch_save` | none |
| `live_changed` | 199 | `Option<Instant>` | `session_started`, `watch_save` | none |
| `writes` | 200 | `u32` | `session_started`, `watch_save` (read by `session_over`, log only) | none |
| `files_stamp` | 201 | `Option<SystemTime>` | `session_started`, `on_event`(Resumed), `heartbeat` | **INF** R `tick`:693 |
| `last_heartbeat` | 202 | `Option<Instant>` | `session_started`, `watch_save`, `heartbeat` | none |
| `outlook` | 204 | `Option<Outlook>` | `session_started`, `check_before_play`, `refresh` | **SET** W `save_settings`:1484, R `form_view`:1537, `name_taken`:1520 · **WIN** R `build_lists`:1777, `build_view`:1925-2057 |
| `offline` | 205 | `bool` | 12 SYNC fns (`refresh`, `retry`, `heartbeat`, `check_before_play`, …) | **SET** W `save_settings`:1486, `finish_setup`:1645, R `form_view`:1533 · **WIN** R `build_view` · **INF** R `tick`:713 |
| `offline_told` | 206 | `bool` | `unsent_offline`, `retry` | none |
| `split` | 210 | `Option<u64>` | `send_after_close`, `split_found`, `sync_now`, `resolve`, `retry`, `refresh`, `session_over`(R) | **SET** W `save_settings`:1485, `finish_setup`:1625 · **WIN** R `build_view`:1914,2040,2054 |
| `needs_you_open` | 211 | `bool` | `split_found`, `sync_now`, `resolve`, `retry`, `session_over` | **SET** W `finish_setup`:1626 · **INF** W `handle`(DecideLater):556 · **WIN** R `build_view`:2054 |
| `shown` | 213 | `Option<Shown>` | read by `resolve`:1247 | **WIN** W `push`:2111. The view decides what the "Needs you" answer applies to. |
| `pending_play` | 215 | `bool` | `play`, `check_before_play`, `split_found`, `resolve` | **INF** W `handle`(CancelPlay, DecideLater) · **UPD** R `update_now`:2223, where it means "play after the update" |
| `play_anyway` | 216 | `bool` | `check_before_play` (taken) | **INF** W `handle`(PlayAnyway, CancelPlay) |
| `leftover_done` | 218 | `bool` | `leftover` | none |
| `gated` | 232 | `Option<String>` | `check_before_play`:1040, `refresh`:1750,1759,1765 | **SHR** W `share_look`:396 · **UPD** R `satchel_look`:482, `update_line`:2304 |
| `last_retry` | 245 | `Option<Instant>` | `retry` | **INF** R `tick`:714 |

### 1.2 PT — playtime (2 direct)

| Field | Line | Type | Writers | Other concerns |
|---|---|---|---|---|
| `session_start` | 194 | `Option<DateTime<Utc>>` | **SYNC** `session_started`:764, `session_over`:930 (take) | **WIN** R `build_view`:2010 (the rail's "playing N min") |
| `closed_at` | 195 | `Option<DateTime<Utc>>` | **SYNC** `on_event`:736, `session_started`:765, `session_over`:931 | none |

No PT method exists today. Playtime is recorded inside `session_over` (worker.rs:932-941), flushed by `startup`:467 and `retry`:1317, and presented by `build_lists`:1875-1898.

### 1.3 WIN — window, tray, Play card, banner (10 direct)

| Field | Line | Type | Writers | Other concerns |
|---|---|---|---|---|
| `ui` | 187 | `Ui` (view.rs:548) | none (handle) | **SYNC** `show` in `play`:995, `split_found`:1147, `sync_now`:1236, `retry`:1338, `session_over`:975 · **SET** `push_form`/`show` (8 sites) · **FRG** `push_icon`:743, `push_thumb`:755, `push_shot`:766,799 · **INF** `run`:462 `quit` |
| `notice` | 208 | `Option<String>` | `say`:2134 | **INF** W `tick`:705 (expiry). `say` is called from SYNC, SET, UPD, and from SHR through `game_say`:1242. |
| `notice_at` | 209 | `Option<Instant>` | `say` | **INF** W `tick`:706 |
| `problem` | 207 | `Option<String>` | none in WIN; WIN only reads it in `build_view` | **SYNC** W `play`, `send_after_close`, `sync_now`, `resolve`, `bring_back`, `refresh` · **SET** W `save_settings`, `find_game`, and `finish_setup` reads it at 1612 as "did saving fail" |
| `card` | 239 | `Option<(Vec<(String,bool,bool)>, bool)>` | `open_card`, `card_line`, `card_ask` | **INF** W `handle`:534,558, `tick`:709 · **SYNC** R `on_event`:753. `card_line` is called by **SYNC**, **ADD** (`addons_before_play` 740-775) and **SHR** (`share_before_play`, `addons_before_play_shared`). |
| `card_until` | 240 | `Option<Instant>` | `open_card`, `close_card_in` | **INF** W `tick`:710 |
| `card_gate` | 238 | `bool` | `card_ask`:2333 | **SYNC** W `check_before_play`:1050. Read in `build_view`:2103 together with **UPD** `offer()`. |
| `busy` | 241 | `Option<String>` | (read in `build_view`) | **SYNC** W 7 fns (`send_after_close`, `play`, `bring_in`, `sync_now`, `resolve`, `bring_back`, and `refresh` reads it) · **UPD** W `update_now` · **INF** W `run`:454 (panic reset), R `tick`:712 |
| `last_view` | 246 | `Option<View>` | `push`:2113 | **INF** W `handle`:500 (forced re-push on installs) |
| `lists` | 243 | `Lists` | `build_lists`:1905 | **SYNC** R `bring_back`:1361 (`lists.rows`: the window's row index → version or file) |

`Lists` (worker.rs:167-183) is a derived model, all written by `build_lists`:

| `Lists` field | Concern | Built from |
|---|---|---|
| `saves`, `saves_summary`, `rows` | SYNC | `outlook.index.versions`, `engine.exports()`, `engine.copies()` |
| `pcs` | SYNC + PT | the index's PCs plus `engine.sessions()` (1831-1852) |
| `last_copy`, `last_sync`, `pc_count` | SYNC | the index, `engine.state.last_synced`:1819 |
| `playtime`, `last_session`, `pcs_week` | PT | `engine.sessions()` → `playtime::totals/per_day` |
| `recent` | WIN | `activity::recent` (the activity log file) |

### 1.4 UPD — the Satchel's own update (7 direct)

| Field | Line | Type | Writers | Other concerns |
|---|---|---|---|---|
| `github_offer` | 222 | `Option<update::Offer>` | `satchel_answer` | read by `offer()`. That reader is used by SYNC `check_before_play`:1042 and WIN `build_view`:2103. |
| `satchel_looked` | 223 | `Option<Instant>` | `satchel_look` (addons.rs:491), `check_updates` | none |
| `satchel_failed` | 224 | `bool` | `satchel_answer` | read by `satchel_look` |
| `check_pending` | 226 | `bool` | `check_updates`, `satchel_answer` | **WIN** R `build_view`:2101 |
| `check_result` | 228 | `Option<String>` | `check_updates`, `satchel_answer`, `update_now` | **WIN** R `build_view`:2100 |
| `checks` | 231 | `u32` | `check_updates` | **WIN** R `build_view`:2102 |
| `update_after_session` | 236 | `bool` | `update_now`:2212 | **SYNC** W/R `session_over`:979 (take, then calls `update_now`) |

### 1.5 SET — settings and the first start (1 direct)

| Field | Line | Type | Writers | Other concerns |
|---|---|---|---|---|
| `setup` | 242 | `Option<Setup>` | `enter_setup`, `finish_setup` | **INF** R `run`:430,448 (skips `tick` during setup) · **WIN** R `build_view`:2092 |

The settings themselves live in `engine.settings` (see 1.11).

### 1.6 INF — shared infrastructure (6 direct)

| Field | Line | Type | Users |
|---|---|---|---|
| `engine` | 186 | `Engine` (satchel-core) | Every concern. `engine.shared()` is the universal "sync is on" test, and `engine.local` supplies the paths for every concern. |
| `scanner` | 189 | `win::Scanner` | **INF** `tick` · **SYNC** `leftover`, `refresh` · **SET** `finish_setup`:1603 · **SHR** `game_started_meanwhile`:240 · **UPD** `update_now`:2205 |
| `away` | 190 | `win::AwayWatch` | `tick`:690 only (driven by `engine.settings.pause_when_away`) |
| `epoch` | 191 | `Instant` | `tick`:686 only |
| `quit` | 234 | `bool` | **UPD** W `update_now`:2226 · **INF** R `run`:457 |
| `last_refresh` | 244 | `Option<Instant>` | **SYNC** W `refresh`:1665 · **INF** R `tick`:716 |

### 1.7 ADD — `AddonsState` (24 nested)

| Field | Line | Type | Other concerns |
|---|---|---|---|
| `catalog` | 106 | `Catalog` | **FRG** R `forge_load`:342, `forge_registry_answer`:377, `forge_panel`, `forge_published`, `offer_line`, `offer_slug` · **SHR** R `newer`:251, `share_incoming`:1364 |
| `store` | 107 | `Store` (addons.json) | **SHR** W `share_follow`:863 (`Addons{store:&mut}`), 912, 926 · `addon_local_only`:1143-1146 · `addon_settings_per_pc`:1157-1160. SHR also reads it in `here`, `has_release`, `settings_set`, `share_decide`:438 and more. **INF** R `handle`:650 (OkAnyway routing) |
| `found` | 108 | `Vec<Found>` | **SHR** R `default_off`:202 |
| `prints` | 110 | `BTreeMap<String,String>` | none |
| `enabled` | 111 | `EnableList` | **SHR** R `folders_off`:207 |
| `lines` | 112 | `Vec<Line>` | **SHR** R `here`:226, `share_line` · **FRG** R `forge_icons_wanted`, `offer_line`, `panel_action` |
| `offers` | 113 | `Vec<Offer>` | **FRG** R `forge_icons_wanted`, `forge_panel` |
| `newer` | 115 | `BTreeMap<String,Release>` | **SHR** R `newer`:251, `url_of` · **FRG** R `forge_panel`, `offer_tag` |
| `ready` | 117 | `BTreeMap<(String,String),String>` | **SHR** W `share_follow`:894 |
| `fetching` | 118 | `BTreeSet<(String,String)>` | none (written by the Net dispatcher in addons.rs) |
| `refused` | 120 | `BTreeMap<(String,String),String>` | **SHR** R `share_follow`:897 |
| `waiting` | 122 | `Vec<Want>` | **FRG** R `panel_action` |
| `restore` | 125 | `BTreeMap<String,Gone>` | none |
| `unreachable` | 127 | `BTreeMap<String,String>` | none |
| `looked` | 128 | `Option<DateTime<Utc>>` | none |
| `looking` | 129 | `bool` | none |
| `link` | 130 | `Link` | none |
| `avrt` | 131 | `bool` | none |
| `other_manager` | 132 | `bool` | none |
| `stamp` | 134 | `Vec<(String,Option<SystemTime>)>` | none |
| `adopted_note` | 136 | `Option<String>` | none |
| `message` | 137 | `Option<(String,Instant)>` | none. Written via `addons_say`, which **SHR** calls (`share_follow_now`, `incoming_install_anyway`, `addon_local_only`, …). |
| `versions_open` | 139 | `Option<String>` | **INF** W `handle`:645 (VersionsClose) |
| `net` | 140 | `Sender<Job>` | **FRG** R 7 fns (`forge_look`:357, `forge_ask_first`:500, `forge_ask_follow`:552, `browse_open`, `forge_fetch_versions`, `forge_pictures_next`, `forge_show_scan`) · **NEWS** R `news_look`:103 · **UPD** R `satchel_look`:492. This is the one network channel for every concern. It lives in `AddonsState` only by history. |

### 1.8 SHR — `ShareState` (10 nested)

| Field | Line | Type | Other concerns |
|---|---|---|---|
| `memory` | 34 | `share::Memory` (addon-sync.json) | none direct. Loaded in `Worker::new`:350-362, which also repairs it against a missing addons.json, so it depends on ADD. |
| `list` | 36 | `Option<List>` | **SET** R `name_taken`:1521 (the PC names on the shared addon list) |
| `steps` | 39 | `BTreeMap<String,Step>` | none |
| `settings` | 40 | `BTreeMap<String,SettingsStep>` | none |
| `game` | 42 | `Option<SettingsStep>` | none (the game's own settings) |
| `game_problem` | 44 | `Option<String>` | none |
| `problem` | 46 | `Option<String>` | none |
| `game_note` | 48 | `Option<(String,Instant)>` | none |
| `oks` | 51 | `forge::Oks` | none |
| `follow_now` | 54 | `BTreeSet<String>` | none |

SHR's couplings run through methods rather than fields: ADD's `addons_view` calls `share_line`, `share_incoming` and `share_header`, WIN's `build_view` calls `game_settings_view`, and SYNC's `sync_now` calls `share_status` and `game_settings_status`.

### 1.9 FRG — `ForgeState` (22 nested)

| Field | Line | Type | Other concerns |
|---|---|---|---|
| `registry` | 72 | `Option<forge::Registry>` | **ADD** R `versions_of`:1517 |
| `versions` | 76 | `BTreeMap<String,Vec<forge::Version>>` | **ADD** R `versions_of`:1517 |
| `held` | 93 | `Vec<(String,String,Held)>` | none direct. ADD reads it via `forge.holds()` in `addons_view`:1677 and via `holds_version` in `versions_view`. |
| `cache`, `state`, `join`, `asking`, `details_out`, `versions_out`, `findings_out`, `open`, `details`, `scans`, `failed`, `asked`, `follow_asked`, `icons`, `thumbs`, `thumbs_for`, `picture_out`, `viewer`, `shown` | 67-114 | (see forge.rs) | none direct |

FRG's outward coupling is `forge.view()` / `forge_view()` (`ForgeView`, forge.rs:153, 333). ADD calls it from 12 methods (and `addons_lines`:391 calls `forge.view` directly), SHR from 3 (`share_verdict`, `entry_slug`, `share_incoming`).

### 1.10 NEWS — `NewsState` (6 nested)

`cache`, `state`, `latest`, `notes`, `latest_out`, `notes_out` (game_news.rs:24-36). No other concern touches them. The only couplings are `addons.net` (to send jobs) and `news_view()`, which `build_view`:1996 calls.

### 1.11 State held inside `engine` (shared across concerns)

| Path | Writers | Readers outside SET/SYNC |
|---|---|---|
| `engine.settings` (whole) | SET `save_settings`:1488, `find_game`:2188, `window_closed`:1403, `browse_view`:1506 | — |
| `engine.settings.game_dir` | SET (via the above) | Everyone, through `game_dir()` (worker.rs:681). Callers: SYNC, SET, UPD, WIN, INF `tick`, ADD `game_addons`:321 (→ all of ADD), SHR `game_started_meanwhile`. |
| `engine.settings.addons.{forge,shared,update_by_themselves,browse_view}` | SET | ADD `addons_lines`:389-391, `addons_view`:1671 · FRG `forge_on`:329 · SHR `share_place`:170 |
| `engine.settings.game.shared` | SET | SHR `game_share_place`:179 |
| `engine.settings.notifications` | SET | INF `notify`:2121 (called by SYNC) |
| `engine.settings.pause_when_away` | SET | INF `tick`:690 |
| `engine.settings.theme` | SET | WIN `theme_dark`:2142 |
| `engine.settings.pc_name` | SET | WIN `build_view`:2047 |
| `engine.state.send_pending` | SYNC `refresh`, `retry`, `send_after_close`, `unsent_offline` · SET `finish_setup`:1646 | INF `tick`:713 · WIN `build_view`:1920 |
| `engine.state.seen_version` | SET `finish_setup`:1617 · UPD `whats_new_seen`:2160 | UPD `version_new`:2155 |
| `engine.state.setup_done` | SET `finish_setup`:1615 | UPD `version_new`:2155 |
| `engine.state.pc_names` | SET `save_settings`:1476 | SET `name_taken`:1517 |
| `engine.state.last_checkpoint` | SYNC `marker_names`:896 | — |
| `engine.state.last_synced` | (core) | WIN `build_lists`:1819 |

---

## 2. The `Command` enum (window → worker)

`pub enum Command` is at worker.rs:46-158 and has **64 variants**. Dispatch happens in `Worker::handle` (worker.rs:485-677). Two passes run before the `match`:

- **Forced re-push** (488-501). For 9 install commands, `last_view = None`, so the window always gets a view.
- **Addon change notice** (503-524, applied at 674-676). For 19 commands, `share_look()` runs after the arm. These are the 17 addon-changing commands plus `SaveSettings` and `RestoreDefaults`. So the dispatcher itself encodes which commands concern SHR.

"Sent from" gives main.rs lines unless marked otherwise.

### 2.1 SYNC (10)

| Variant | Handler | Sent from |
|---|---|---|
| `Play` | `play` worker.rs:986 | 172; tray 518; single-instance `--play` 557; startup `--play` 571 |
| `PlayAnyway` | inline 527-530, then `play` | card 536 |
| `CancelPlay` | inline 531-535 (`pending_play`, `play_anyway`, `card`) | card 535 |
| `SyncNow` | `sync_now` worker.rs:1174 | 173; tray 519 |
| `BringBack(usize)` | `bring_back` worker.rs:1356 (indexes `lists.rows`) | 182-184 |
| `OpenSavesFolder` | inline 538-541 | 179 |
| `OpenFolder` | inline 542-546 | 174 |
| `KeepHere` | `resolve(true)` worker.rs:1246 | 176; question action 0 (194) |
| `UseOther` | `resolve(false)` worker.rs:1246 | 177; question action 1 (195) |
| `DecideLater` | inline 555-559 | 178 |

### 2.2 SET — settings and the first start (10)

| Variant | Handler | Sent from |
|---|---|---|
| `SaveSettings(Form)` | `save_settings` worker.rs:1425, then inline `refresh`, `push_form`, `addons_scan`, `forge_look` (560-568), then `share_look` | 232, 241, 262 |
| `RestoreDefaults` | inline 569-589 → `form_view`, `save_settings`, then the same follow-up chain and `share_look` | 270 |
| `DetectGame(Form)` | inline 590-610 → `win::detect_game_dir`, `save_settings`, `refresh`, `push_form`, `addons_scan`, `share_look` | 277 |
| `GameFolderPicked(Form, PathBuf)` | inline 611-618 (form only) | 291 (picker thread) |
| `FinishSetup(Form)` | `finish_setup` worker.rs:1600 | 300 |
| `WindowClosed(bool)` | `window_closed` worker.rs:1396 | 512 |
| `FindGame(Form)` | `find_game` worker.rs:2168 | 326 |
| `OpenGameFolder` | inline 547-551 | 175 |
| `OpenGameSite` | inline 670 | 342 |
| `BrowseView(String)` | `browse_view` worker.rs:1498. It writes `settings.addons` and belongs to the Browse UI. | 445 |

### 2.3 ADD (24)

| Variant | Handler (addons.rs unless noted) | Sent from |
|---|---|---|
| `AddonsCheck` | `addons_check`:817 | 305 |
| `AddonsUpdateAll` | `addons_update_all`:898 | 306 |
| `AddonsOpenFolder` | `addons_open_folder`:1014 | 307 |
| `AddonSwitch(String,bool)` | `addon_switch`:824 | 353 |
| `AddonUpdate(String)` | `addon_update`:846 → `addon_update_as`:859 | 386 |
| `AddonUpdateAnyway(String)` | `addon_update_anyway`:851 | 387 |
| `AddonBack(String)` | `addon_back`:917 → `version_back`:924 | 388 |
| `AddonKeep(String,bool)` | `addon_keep`:945 | 359 |
| `AddonRemove(String,bool)` | `addon_remove`:959 (→ SHR `share_removed`) | 383 |
| `AddonPage(String)` | `addon_page`:989 (→ FRG `forge_repo`, SHR `share_repo`) | 389 |
| `AddonFolder(String)` | `addon_folder`:1006 | 390 |
| `BrowseInstall(String)` | `browse_install`:1023 → `browse_install_with`:1036 | 391 |
| `BrowseInstallAnyway(String)` | `browse_install_anyway`:1028 | 392 |
| `VersionsOpen(String)` | `versions_open`:1504 | 478 |
| `VersionsClose` | inline worker.rs:645 (`addons.versions_open = None`) | 479 |
| `VersionInstall(String,String,bool)` | `version_install_as`:1580 | 483 |
| `VersionBack(String,String)` | `version_back`:924 | 489 |
| `LinkLookup(String)` | `link_lookup`:1320 | 418 |
| `LinkInstall` | `link_install`:1332 | 315 |
| `LinkClear` | `link_clear`:1328 | 318 |
| `ImportZip(PathBuf)` | `import_zip`:1399 | 501 (picker thread) |
| `InstallAgain(String)` | `install_again_as(_, false, true)`:1200 | 319 |
| `InstallAgainAnyway(String)` | `install_again_as(_, true, true)`:1200 | 492 |
| `OkAnyway(String)` | inline worker.rs:649-655. If `addons.store` has the id it calls `addon_update_anyway`, otherwise SHR `incoming_install_anyway`. It spans ADD and SHR. | question action 7 (207) |

### 2.4 SHR (6)

| Variant | Handler (share.rs) | Sent from |
|---|---|---|
| `AddonLocalOnly(String,bool)` | `addon_local_only`:1142 (writes `addons.store`) | 365 |
| `AddonSettingsPerPc(String,bool)` | `addon_settings_per_pc`:1156 (writes `addons.store`) | 371 |
| `AddonSettingsChoice(String,bool)` | `addon_settings_choice`:1170 | 377; question actions 5-6 (199-200) |
| `IncomingInstallAnyway(String)` | `incoming_install_anyway`:1053 | 493 |
| `GameSettingsChoice(bool)` | `game_settings_choice`:1179 | 347; question actions 2-3 (196-197) |
| `GameSettingsOwn` | `game_use_own`:1190 | 341; question action 4 (198) |

### 2.5 FRG (8)

| Variant | Handler (forge.rs) | Sent from |
|---|---|---|
| `BrowseOpen(String)` | `browse_open`:814 | 395 |
| `BrowseClose` | `browse_close`:849 | 400 |
| `ForgeShowScan(String)` | `forge_show_scan`:857 | 410; question action 8 (213) |
| `AddonsShown(i32)` | `forge_icons_wanted`:637 | 450 |
| `PictureShow(String,usize)` | `forge_picture_show`:788 | 464 |
| `PictureClose` | `forge_picture_close`:805 | 474 |
| `ThumbsShown(String,usize)` | `forge_thumbs_wanted`:671 | 407 |
| `OpenLink(String)` | `open_link`:874 (an allow-listed URL opener) | 415 |

### 2.6 NEWS (1), UPD (3), INF (2)

| Variant | Concern | Handler | Sent from |
|---|---|---|---|
| `GameNewsSeen` | NEWS | `news_seen` game_news.rs:144 | 321 |
| `UpdateNow` | UPD | `update_now` worker.rs:2204 | 330; card 537 |
| `CheckUpdates` | UPD | `check_updates` worker.rs:2240 | 338 |
| `WhatsNewSeen` | UPD | `whats_new_seen` worker.rs:2158 | 320 |
| `Net(Answer)` | INF | `addons_answer` addons.rs:496-581, the dispatcher for all 10 `Answer` variants (net.rs:91-105) | `net.rs`:193 (network thread) |
| `Quit` | INF | `run` loop break worker.rs:438 (the `handle` arm at 672 is a no-op) | tray 520 |

`Net(Answer)` fans out like this:

| `Answer` variant | Target | Concern |
|---|---|---|
| `Looked` | inline addons.rs:498-533 | ADD |
| `Fetched` | inline addons.rs:534-560, then `install_waiting` and `share_follow_now` | ADD + SHR |
| `LookedUp` | inline addons.rs:568-579 | ADD |
| `Satchel` | `satchel_answer` worker.rs:2269 | UPD |
| `ForgeRegistry` | forge.rs:363 | FRG |
| `ForgeDetails` | forge.rs:407 | FRG |
| `ForgeVersions` | forge.rs:443 | FRG |
| `ForgeFindings` | forge.rs:466 | FRG |
| `Picture` | forge.rs:722 | FRG |
| `GameNews` | game_news.rs:110 | NEWS |

### 2.7 Window-only callbacks (no `Command`)

These run on the UI thread and never reach the worker:

- `on_open_url` (343).
- `on_browse_filter`, `on_filter_toggled`, `on_filters_clear` (421-441).
- `tray.on_open_tab` (523).
- The optimistic UI marks `view::installing`, `view::show_scan` and `view::checking` (main.rs:203, 211, 334-336).

---

## 3. The run loop, startup, and the idle tick

### 3.1 `run` (worker.rs:420-463)

1. `win::com_init` and `register_notifications`.
2. `enter_setup()` if this is the first start, otherwise `startup()`. Then `push()`.
3. Loop:
   1. `recv_timeout(TICK = 2 s)` (31).
   2. In `catch_unwind`: `handle(cmd)`, then `tick()` unless `setup` is Some, then `push()`.
   3. On panic: `busy = None` plus a banner line.
   4. Break on `Quit`, on disconnect, or when `quit` is set.
4. `ui.quit()`.

Every loop step pushes, whether or not a command came. `push` builds the whole `View` every 2 s and sends it only when it differs from `last_view` (2112).

### 3.2 `startup` (worker.rs:465-483), in order

| # | Step | Concern | Note |
|---|---|---|---|
| 1 | `apply_windows_settings` | SET | autostart and the shortcut |
| 2 | `engine.flush_playtime()` | PT | |
| 3 | `forge_load` | FRG | reads `addons.catalog` for the join |
| 4 | `news_load` | NEWS | |
| 5 | `refresh` | SYNC → `build_lists` → `addons_tick` → (maybe) `addons_scan` | Lists use the forge verdicts loaded in step 3. |
| 6 | `push_form` | SET | |
| 7 | `addons_scan` | ADD (→ `addons_lines` → `addons_fetch` → SHR `share_wanted`; FRG `forge_icons_again`) | |
| 8 | `addons_prune_cache` | ADD | |
| 9 | `addons_look` | ADD (network job) | |
| 10 | `forge_look` | FRG (network job) | |
| 11 | `news_look` | NEWS (network job) | |
| 12 | `share_look` | SHR | needs the lines from step 7 |
| 13 | `satchel_look` | UPD (network job) | reads `gated`, which may have been set in step 5 or 12 |

`finish_setup` (1600-1656) runs its own first-sync decision (1619-1651), sets `setup = None`, and then calls `startup()`.

### 3.3 `tick` (worker.rs:685-728), in order

| # | Step | Lines | Concerns touched |
|---|---|---|---|
| 1 | `dir = game_dir()`; `running = scanner.game_running(dir)` | 687-688 | SET, INF |
| 2 | `away.watch(...)` if running and `pause_when_away` | 690 | SET, INF |
| 3 | `changed` = game closed, in session, and `game_files_stamp(dir) > files_stamp` | 691-693 | SYNC |
| 4 | `tracker.tick(...)` → `on_event(event)` | 694-696 | SYNC; see 3.4 |
| 5 | `watch_save()` while the game runs (checkpoint after 3 s quiet) | 697-699 | SYNC |
| 6 | `heartbeat()` while in session | 701-703 | SYNC |
| 7 | expire `notice` after 60 s | 704-707 | WIN |
| 8 | expire `card` at `card_until` | 708-711 | WIN |
| 9 | only when not in session and `busy` is None: | 712-721 | |
| 9a | if sync is on and (`offline` or `send_pending`) and the retry is due (180 s) → `retry()` | 714-715 | SYNC, PT (`flush_playtime`), `engine.state` |
| 9b | else if the refresh is due (60 s) → `refresh()`, `share_look()`, `satchel_look()` | 716-719 | SYNC → WIN (`build_lists`) → ADD (`addons_tick`: rescan on stamp change, message expiry); SHR (may set `gated`); UPD (reads `gated`) |
| 10 | `forge_look()` (cheap until due, 30 min) | 725 | FRG |
| 11 | `news_look()` (cheap until due, 30 min) | 727 | NEWS |
| — | `run` then calls `push()` → `build_view` | 451 | WIN plus every concern's `*_view` |

9a and 9b exclude each other within one tick. While an unsent save waits for its retry, the 60-second refresh still runs whenever the retry is not due.

### 3.4 Session events (`on_event`, worker.rs:730-760)

| Event | Sequence | Concerns |
|---|---|---|
| `Started` | `session_started`:762. Times are reset, the marker is written, and a game not started by Play gets an outlook check with notifications. | SYNC, PT, WIN |
| `Closed{updating}` | `closed_at = now`. If sync is on: `send_after_close` (busy, push, send, `refresh`), then `share_look`, because the game writes addon settings at close. Then `card_line` if updating. | SYNC, PT, SHR, WIN |
| `Updating` | `card_line` | WIN |
| `Resumed` | `files_stamp` again; a `card` line if a card is open | SYNC, WIN |
| `Over` | `session_over`:929. Record the Session (PT), clear the marker, notify, `activity`, show "Needs you" if `split`, `refresh`, `addons_after_session` (ADD: scan, `install_waiting`, `share_look`), then `update_now` if `update_after_session`. | SYNC, PT, WIN, ADD, SHR, UPD |

### 3.5 Play (`play`, worker.rs:986-1026)

| # | Step | Concerns |
|---|---|---|
| 1 | Game running → card "already running" | SYNC, WIN |
| 2 | No game folder → `problem` and the Settings tab | SET, WIN |
| 3 | `open_card` | WIN |
| 4 | If sync is on: `busy`, `push`, then `check_before_play` (1029-1139). Outlook, the gate (sets `gated`, `card_gate`, and asks with the UPD `offer()`), stale take-over, another PC open, a damaged save, then the Decision: adopt, send, bring in, or split. | SYNC, UPD, WIN |
| 5 | `share_before_play` (share.rs:738). `share_look`, then `addons_before_play_shared` (follow the list's addons via ADD `addons_install`, `addons_scan`, then the addon settings bring-in), then the game's settings bring-in, then `share_look`. | SHR, ADD, WIN (card) |
| 6 | `addons_before_play` (addons.rs:704). `addons_scan`; auto-updates and waiting installs via `addons_install` (FRG `forge_judge` inside); card lines; `addons_look` if stale. | ADD, FRG, WIN |
| 7 | `win::launch_game`; `launched = true`; card | SYNC, WIN |

---

## 4. The push to the window

### 4.1 Entry points

| Direction | Function | Line | What it carries |
|---|---|---|---|
| worker → UI | `Ui::push(View)` | view.rs:556 → `apply` view.rs:1295-1506 | the whole window, tray and Play card; per-section diff against thread-local `SHOWN` (642) |
| worker → UI | `Ui::push_form(FormView)` | view.rs:566 → `apply_form` 1512 | the Settings form (start, after a save or refusal, Detect, Find it, setup) |
| worker → UI | `Ui::show(Option<i32>)` | view.rs:570 | raise the window, optionally on a tab |
| worker → UI | `Ui::quit()` | view.rs:583 | |
| worker → UI | `Ui::push_icon`, `push_thumb`, `push_shot` | view.rs:591, 600, 617 | images, bypassing `View`; called only from FRG `forge_picture_answer`:743-766 and `forge_picture_show`:799 |
| UI → worker | `view::read_form(&MainWindow) -> Form` | view.rs:1558 | the Settings form, inside `SaveSettings`, `DetectGame`, `FinishSetup`, `FindGame`, `GameFolderPicked` |

Worker side:

- `push` (worker.rs:2109-2116) calls `build_view` (1910-2107), stores the returned `Shown` in `self.shown`, and sends only when the view differs from `last_view`.
- `push_form` (1570) calls `form_view` (1525-1568).

### 4.2 What each part of `View` (view.rs:97-146) reads

| `View` field (UI area) | Built by | Worker fields and calls read | Concerns |
|---|---|---|---|
| `tray`, `status`, `banner` (tray, banner) | `build_view` 1914-1989 | `split`, `problem`, `notice`, `busy`, `tracker.in_session/game_running`, `offline`, `engine.state.send_pending`, `sent`, `outlook.decision`, `outlook.index.newest_version`, `game_dir()`, `engine.shared()` | SYNC, WIN, SET |
| `sync_on`, `playing`, `busy` | 2072-2078 | `engine.shared()`, `tracker`, `busy` | SYNC, WIN |
| `rail.build` | 1996-1997 | `news_view().version` | NEWS |
| `rail.progress` | 1998-2006 | tray dot, banner, `lists.last_sync` or `last_copy`, `lists.pc_count` | SYNC, WIN |
| `rail.addons` | 2007-2008 | `addons_view().rail` | ADD (+FRG, SHR inside) |
| `rail.last` | 2009-2015 | `tracker.in_session`, `session_start`, `lists.last_session` | SYNC, PT |
| `pill` (top bar) | 2018-2025 | `update_line()` (`github_offer`, `gated`), `version_new()` (`engine.state.setup_done`, `seen_version`) | UPD, SYNC |
| `questions` (Home, Needs you) | 2027-2050 → `shell::needs_you` | `split` with `outlook.index`; `game_settings_view()` (SHR); `addons.rows` (`reason_kind`, `can_update`, `incoming`, `ask_pc`); `name_taken(pc_name)` (SET: `engine.settings.pc_name`, `engine.state.pc_names`, `outlook.index.pcs`, `share.list`); `offline` | SYNC, SHR, ADD, FRG, SET |
| `news_blocks` (Home, What's new) | 2051 | `news_view().notes[0].blocks` | NEWS |
| `pcs`, `pcs_week` (Home, Your PCs) | `build_lists` 1829-1852, 1897 | `outlook.index`, `engine.sessions()` | SYNC, PT |
| `recent` (Home, activity) | `build_lists` 1899-1903 | the activity file | WIN |
| `playtime` (Home, Playtime) | `build_lists` 1876-1896 | `engine.sessions()` | PT |
| `saves`, `saves_summary`, `saves_note` (Settings, Progress versions) | `build_lists` 1777-1874, `build_view` 2066 | `outlook`, `engine.exports()`, `engine.copies()`, `engine.state.last_synced` | SYNC |
| `needs_you` (the split dialog) | 2053-2064 | `needs_you_open`, `split`, `engine.local_save()` (a disk read every push, 1992), `outlook`; produces `Shown` | SYNC |
| `card`, `card_update` (Play card) | 2091, 2103 | `card`; `card_gate` with `offer()` | WIN, UPD |
| `setup` (first start) | 2092 | `setup` | SET |
| `addons` (Addons tab, all of it) | `addons_view` addons.rs:1669-2063 | the `AddonsState` fields; `forge_view()`, `forge.holds`, `forge_status`, `forge_panel`, `offer_tag`, `offer_line`; `share_line`, `share_incoming`, `share_header`; `tracker.game_running`; `game_dir()`; `engine.settings.addons` | ADD, FRG, SHR, SYNC, SET |
| `dark`, `system_dark` | `theme_dark` 2140 | `engine.settings.theme`, `win::apps_use_dark` | SET, WIN |
| `app_released` (About) | `released_text` 2369 | `about::released` | — |
| `game_found` | 2097 | `game_dir()` | SET |
| `game_settings` (Settings, Home question) | `game_settings_view` share.rs:1451 | `share.game`, `game_problem`, `game_note`, `memory.game_own_kept`, `memory.splits` | SHR |
| `update`, `check_result`, `checking`, `checks` (About, Settings) | 2099-2102 | `update_line()`, `check_result`, `check_pending`, `checks` | UPD |
| `game_news` (Play line, panel, About) | `news_view` game_news.rs:152 | `NewsState` | NEWS |

The `FormView` (view.rs:532) is built by `form_view` (worker.rs:1525-1568). It reads `engine.settings.*`, `engine.folder_problem()`, `offline` (SYNC), `link_warning()` (SET) and `outlook.index` (SYNC).

---

## 5. Entanglements

### 5.1 Functions that touch three or more concerns

The count is the number of distinct non-INF concerns whose fields a function touches or whose methods it calls.

| Concerns | Function | Location | Touches |
|---|---|---|---|
| 8 | `tick` | worker.rs:685 | FRG, NEWS, SET, SHR, STATE, SYNC, UPD, WIN |
| 8 | `handle` | worker.rs:485 | all |
| 8 | `build_view` | worker.rs:1910 | ADD, NEWS, SET, SHR, STATE, SYNC, UPD, WIN |
| 7 | `startup` | worker.rs:465 | ADD, FRG, NEWS, SET, SHR, SYNC, UPD |
| 6 | `addons_answer` (Net dispatcher) | addons.rs:496 | ADD, FRG, NEWS, SHR, SYNC, UPD |
| 5 | `play` | worker.rs:986 | ADD, SET, SHR, SYNC, WIN |
| 5 | `find_game` | worker.rs:2168 | ADD, SET, SHR, SYNC, WIN |
| 5 | `addons_view` | addons.rs:1669 | ADD, FRG, SET, SHR, SYNC |
| 4 | `session_over` | worker.rs:929 | ADD (+SHR), SYNC, UPD, WIN (+PT) |
| 4 | `on_event` | worker.rs:730 | SET, SHR, SYNC, WIN |
| 4 | `update_now` | worker.rs:2204 | SET, SYNC, UPD, WIN |
| 4 | `build_lists` | worker.rs:1770 | ADD (`addons_tick`), STATE, SYNC, WIN (+PT) |
| 4 | `refresh` | worker.rs:1664 | SET, STATE, SYNC, WIN |
| 4 | `finish_setup` | worker.rs:1600 | SET, STATE, SYNC, WIN |
| 4 | `name_taken` | worker.rs:1514 | SET, SHR, STATE, SYNC |
| 4 | `save_settings` | worker.rs:1425 | SET, STATE, SYNC, WIN |
| 3 | `send_after_close` | worker.rs:855 | STATE, SYNC, WIN |
| 3 | `session_started` | worker.rs:762 | SET, SYNC, WIN |
| 3 | `retry` | worker.rs:1298 | STATE, SYNC, WIN |
| 3 | `sync_now` | worker.rs:1174 | SHR, SYNC, WIN |
| 3 | `check_before_play` | worker.rs:1029 | SYNC, UPD, WIN |
| 3 | `share_look` | share.rs:370 | ADD, SHR, SYNC (writes `gated`) |
| 3 | `addons_before_play_shared` | share.rs:780 | ADD, SHR, WIN |
| 3 | `share_follow_now` | share.rs:1081 | ADD, SHR, SYNC |
| 3 | `share_incoming` | share.rs:1352 | ADD, FRG, SHR |
| 3 | `incoming_install_anyway` | share.rs:1053 | ADD, FRG, SHR |
| 3 | `forge_release_held` | forge.rs:513 | ADD, FRG, SHR |
| 3 | `forge_versions_answer` | forge.rs:443 | ADD, FRG, SHR |
| 3 | `addon_page` | addons.rs:989 | ADD, FRG, SHR |
| 3 | `addons_before_play` | addons.rs:704 | ADD, FRG, WIN |
| 3 | `satchel_look` | addons.rs:478 | ADD (`net`), SYNC (`gated`), UPD |
| 3 | `addons_lines` | addons.rs:388 | ADD, FRG, SET |

### 5.2 Fields and calls read by many concerns

| Field or call | Concerns that touch it |
|---|---|
| `engine.shared()`, `engine.local` | all |
| `game_dir()` / `engine.settings.game_dir` | SYNC, SET, ADD (all through `game_addons`), SHR, UPD, WIN, INF (7) |
| `tracker` (game running, in session) | SYNC, ADD, SHR, UPD, WIN, INF (6) |
| `scanner` | SYNC, SET, SHR, UPD, INF (5) |
| `addons.net` | ADD, FRG, NEWS, UPD (4) |
| `busy` | SYNC, UPD, WIN, INF (4) |
| `card` (via `card_line`) | SYNC, ADD, SHR, WIN, INF (5) |
| `offline` | SYNC, SET, WIN, INF (4) |
| `engine.state.send_pending` | SYNC, SET, WIN, INF (4) |
| `problem` | SYNC, SET, WIN (3) |
| `outlook` | SYNC, SET, WIN (3) |
| `gated` | SYNC, SHR, UPD (3) |
| `addons.catalog` | ADD, FRG, SHR (3) |
| `addons.store` | ADD, SHR (written by both), INF (3) |
| `addons.lines`, `addons.newer` | ADD, FRG, SHR (3) |
| `forge_view()` | FRG, ADD (12 callers + `addons_lines` via `forge.view`), SHR (3 callers) |

### 5.3 Call cycles between the existing modules

1. **ADD ↔ FRG.**
   - ADD calls `forge_view`, `forge_ask_first`:493, `forge_judge`, `forge_folders`, `forge_published`, `forge_panel`, `forge_status`, `offer_tag` and `offer_line`.
   - `forge_release_held` (forge.rs:513-531) calls back into ADD's `browse_install_with`, `addon_update_as`, `install_again_as` and `version_install_as`.
   - FRG also reads `addons.catalog`, `lines`, `offers`, `newer` and `waiting` directly.
2. **ADD ↔ SHR.**
   - ADD calls `share_wanted` (`addons_fetch`:445), `share_line`, `share_incoming`, `share_header` (`addons_view`), `share_removed` (`addon_remove`) and `share_repo` (`addon_page`). `install_waiting`:615 and `addons_after_session`:790 call `share_look`.
   - SHR calls `addons_install`, `addons_scan`, `addons_fetch` and `addons_say`.
   - SHR writes `addons.store` (share.rs:863, 912, 926, 1143-1146, 1157-1160) and `addons.ready` (894).
3. **FRG → SHR.** `forge_versions_answer`:462 calls `share_follow_now`, and `forge_release_held`:530 calls `share_look`.
4. **SHR → SYNC.** `share_look`:396 writes `gated` when the shared addon list says a newer Satchel wrote it.
5. **SYNC → SHR/ADD/UPD** (through orchestration, not cycles). Paths:
   - `play` → `share_before_play` → `addons_before_play`.
   - `on_event`(Closed) → `share_look`.
   - `session_over` → `addons_after_session` → `update_now`.
   - `sync_now` → `share_look`, `share_status` and `game_settings_status`.

### 5.4 Ordering dependencies, checked against the code

| # | Dependency | Evidence |
|---|---|---|
| 1 | Progress before addons at Play: sync check, then the other PCs' addons and settings, then this PC's own auto-updates, then launch. | `play` worker.rs:1002-1012 |
| 2 | Lines before sharing decisions. `here()` treats an addon with no line, or a `Standing::Gone` line, as Missing, so `share_look` must run after `addons_scan`. | share.rs:226; `startup` 476 → 481; `install_waiting` 614 → 615; `addons_after_session` 786 → 790 |
| 3 | Forge cache before the first lists. `forge_load` must precede `refresh` and `addons_scan`, because verdicts and offers come from `ForgeView`. | `startup` 470-476 (comment at 470-471) |
| 4 | The catalog before the forge join. `forge_load`:342 and `forge_registry_answer`:377 join the forge registry against `addons.catalog`. | forge.rs:342, 377 |
| 5 | The game must be closed for AddOns and settings writes. | `addons_blocked`:799; `share_look`:418 (settings only when closed); `share_follow_now`:1082; `game_started_meanwhile` share.rs:238 (re-checked between steps of the Play path); `Fetched` installs only outside a session (addons.rs:553-559); `update_now`:2205-2215 |
| 6 | Addons need the game folder. `game_addons()` (addons.rs:320) returns None without `game_dir()`, which requires the exe (worker.rs:681). Then `addons_scan` clears found, prints and stamp, and `share_place`/`game_share_place` return None. | addons.rs:320-338; share.rs:169-183 |
| 7 | Send at close before addon settings go up (the game writes them as it closes). | `on_event` 737-741 |
| 8 | At session end: `refresh` before `addons_after_session` before `update_now`. | `session_over` 977-981 |
| 9 | `busy` is the global "don't start another network-folder operation" flag. `tick` skips retry and refresh while busy (712), and `refresh`'s auto-send checks it (1697). Long operations set `busy` and then `push()` in the middle of a step: `send_after_close`:857-858, `play`:1000-1001, `bring_in`:1153-1154, `sync_now`:1179-1180, `resolve`:1256-1257, `bring_back`:1363-1364, `update_now`:2221-2222. | as listed |
| 10 | `shown` is written by `push` and consumed by `resolve`. The "Needs you" answer applies to the last view built. | worker.rs:2111, 1247 |
| 11 | The gate has three writers and one reader. `refresh` sets or clears it (1750, 1759, 1765), `check_before_play` sets it (1040), and `share_look` sets it (share.rs:396) but never clears it. `satchel_look` reads it to pick its cadence (10 min while gated) and the wanted version. | as listed |
| 12 | After any settings save: `refresh`, `push_form`, `addons_scan`, then `forge_look` or `share_look`. This sequence is written out 4 times: `SaveSettings` 561-567, `RestoreDefaults` 583-588, `DetectGame` 601-606, `find_game` 2197-2199. The first two get `share_look` from the dispatcher's notice; the last two call it themselves, and `forge_look` is missing from them. | as listed |

**Premise check (the brief's examples).**

- **"Sharing needs the index lock" does not hold.** The worker never takes a lock. The progress index lock is taken inside the core (`IndexLock::acquire` in `crates/satchel-core/src/engine.rs`:190, 230, 283, 320, 474). The shared addon list uses a separate lock file: `share::lock` → `shared.addons_lock()` at `crates/satchel-core/src/addons/share.rs`:662-663. The split need not order SHR against SYNC for locking. Ordering 1 above is a product rule, not a lock rule.
- **"The gate blocks sync" holds in effect.** It is not a worker-side block. The core returns an `Io` error that `gate::newer_satchel` recognises, from `engine.outlook()` or `share::read()`. The worker records it in `gated` and keeps going with this PC's progress.
- **"Addons need the game folder" holds** (row 6).

### 5.5 Code in the wrong home (cheap to fix first)

| Item | Where | Belongs to |
|---|---|---|
| `net: Sender<Job>` | `AddonsState` addons.rs:140 | INF. FRG, NEWS and UPD use it. |
| `satchel_look` | addons.rs:478-493 | UPD (uses `super::SATCHEL_*` consts and `due`) |
| `addons_answer`, the dispatcher for all 10 `Answer`s | addons.rs:496-581 | INF (`on_net`), with the `Looked`, `Fetched` and `LookedUp` bodies kept in ADD |
| `browse_view` | worker.rs:1498 | SET (a settings write for Browse's UI) |
| The first-sync decision inside `finish_setup` | worker.rs:1619-1651 | SYNC |
| The `Decision::InSync { adopt }` handling | written 4 times: 1111-1116, 1202-1205, 1340-1346, 1638-1642 | one SYNC helper |
| The stale-PC take-over | twice: 1064-1081, 1184-1194 | one SYNC helper |

### 5.6 Test coverage

`src/` has **no tests**: no `#[cfg(test)]` in any of the 11 files. The only tests are in `crates/satchel-core`. A worker refactor is checked only by the compiler and manual runs. Each extraction step should therefore be a pure move with no logic change, so its diff can be read as a move.

---

## 6. Proposed split

### 6.1 Shape (recommendation)

Keep the one-thread `Worker` that owns all state. Give each concern its own file and its own state struct with **private fields**. Put state-only logic in `impl <Concern>State`. Keep cross-concern operations as `impl Worker` blocks inside the concern's file.

This is the pattern `forge`, `share`, `addons` and `game_news` already use, so the steps are mechanical. Private fields turn today's silent field reaches into named methods, which is the point of the split.

Independent actors exchanging messages are not recommended for 0.9.1. Play, session end and settings save need strictly ordered, synchronous steps across SYNC, SHR, ADD and UPD, with game-closed re-checks between them (5.4 rows 1, 5 and 8).

The orchestration sequences stay in the remaining `Worker` as named functions:

- `play_sequence`, from today's `play`.
- `on_game_closed`.
- `on_session_over`.
- `after_settings_changed`, which consolidates the four copies in 5.4 row 12.

### 6.2 Modules

| Module (file) | Fields that move | Commands that move | Needs from the others (interface in words) | Est. lines |
|---|---|---|---|---|
| **news** (`worker/game_news.rs`, exists) | `NewsState` (6), as today | `GameNewsSeen` | Sends network jobs on the shared channel. Exposes `news_view()` and `news_answer()`. | ~186 |
| **self-update** (new `worker/self_update.rs`; `crate::update` already exists as the installer) | `github_offer`, `satchel_looked`, `satchel_failed`, `check_pending`, `check_result`, `checks`, `update_after_session` (7) | `UpdateNow`, `CheckUpdates`, `WhatsNewSeen`; `Answer::Satchel` | **Reads:** the gate (read-only, from sync); "game running / in session" (`tracker`, `scanner`, `game_dir`); whether a Play is pending (to play after the update); `engine.state.setup_done/seen_version`. **Writes:** banner and busy through the feedback module. Sets the loop's `quit`. **Exposes:** `offer()` (sync's gate question and the card), `update_line()`, the pill facts, and `on_session_over()` (the deferred update). | ~190 |
| **playtime** (new `worker/playtime.rs`) | `session_start`, `closed_at`; `Lists.playtime`, `last_session`, `pcs_week` (5) | none | From sync's session lifecycle: `started(now)`, `closed(now)`, `over(now, updated) -> Session`. Uses `engine.record_session`, `flush_playtime`, `sessions()`. **Exposes:** the playtime view, the rail's "playing N min", and per-PC last-played for Your PCs. | ~90 |
| **feedback** (new `worker/feedback.rs`, the WIN part that others write into) | `notice`, `notice_at`, `problem`, `card`, `card_until`, `card_gate`, `busy` (7) | inline arms of `CancelPlay` and `DecideLater` (card reset) | Nothing. It is a sink. Others call `say`, `card_line`, `card_ask`, `open_card`, `close_card_in`, `set_busy`/`clear_busy`, `set_problem`. Its expiry runs from `tick`. **Exposes:** read-only facts for `build_view`. "Set busy, then push now" stays a `Worker` method, because the push needs every concern. | ~90 |
| **settings** (new `worker/settings.rs`) | `setup` (1). `engine.settings` stays in the engine. | `SaveSettings`, `RestoreDefaults`, `DetectGame`, `GameFolderPicked`, `FinishSetup`, `FindGame`, `WindowClosed`, `OpenGameFolder`, `OpenGameSite`, `BrowseView` (10) | **From sync:** the outlook's PC names and `offline` (form status, name clash); a hook "sync settings changed → forget outlook/split/offline" (today SET writes them at 1484-1486); the first-start decision moved to sync as `first_sync(now)`. **From share:** the PC names on the shared list. **Exposes:** `game_dir()`, `form_view()`, `name_taken()`, `theme_dark()`. Triggers `after_settings_changed` on `Worker`. | ~400 |
| **forge** (`worker/forge.rs`, exists) | `ForgeState` (22), as today | `BrowseOpen`, `BrowseClose`, `ForgeShowScan`, `AddonsShown`, `PictureShow`, `PictureClose`, `ThumbsShown`, `OpenLink` (8); `Answer::Forge*`, `Answer::Picture` | **From addons, read-only:** the catalog, the current lines, offers, newer releases and waiting installs. Pass `&AddonsState` accessors instead of reaching into fields. **Breaks the cycle:** `forge_versions_answer` returns the released `Held` installs, and `Worker` dispatches them to addons and then runs `share_look` and `share_follow_now`. **Exposes:** `ForgeView`, `holds()`, the panel and status, `ask_first`, `ask_follow`. Sends to the UI image cache through `Ui`. | ~1,000 |
| **sync** (new `worker/sync.rs`; optionally `sync/session.rs` + `sync/progress.rs`) | `tracker`, `marker`, `sent`, `launched`, `live_seen`, `live_changed`, `writes`, `files_stamp`, `last_heartbeat`, `outlook`, `offline`, `offline_told`, `split`, `needs_you_open`, `shown`, `pending_play`, `play_anyway`, `leftover_done`, `gated`, `last_retry` (20); `Lists.saves`, `saves_summary`, `rows`, `pcs`, `last_copy`, `last_sync`, `pc_count` (7); the `Row` enum | `Play`, `PlayAnyway`, `CancelPlay`, `SyncNow`, `BringBack`, `OpenSavesFolder`, `OpenFolder`, `KeepHere`, `UseOther`, `DecideLater` (10) | **From settings:** `game_dir()`, the sync folder through `engine`. **From feedback:** card and banner. **From playtime:** session hooks. **From update:** `offer()` for the gate question. **Exposes to the others:** `game_running()`, `in_session()` (ADD, SHR, UPD), `gated()` (UPD) and `set_gated()` (SHR's list read), progress facts for the view, and the outlook's PC names (SET). The Play, closed and over sequences stay on `Worker`; sync provides `check_before_play`, `send_after_close`, `session_over_sync`. | ~900 (session ~260, progress ~640) |
| **view assembly** (`worker/view_model.rs`, or kept in worker.rs) | `last_view`, `lists` (the container), `Lists.recent` | none | Calls every concern's `*_view()` or facts and builds `View` and `NeedsYouFacts`. It owns `push`, `build_view`, the `recent` part of `build_lists`, `released_text` and the time-format helpers `when`, `clock`, `ago`, `size` (worker.rs:263-309, also used by ADD, FRG, NEWS). | ~300 |
| **game-settings sharing** (optional new `worker/game_settings.rs`, split out of share.rs) | `ShareState.game`, `game_problem`, `game_note` (3) | `GameSettingsChoice`, `GameSettingsOwn` | Shares the list, the memory and the settings-version machinery with share (`settings_decide`, `share_send_settings`, `share_bring_settings`, `place_settings`). It touches no addon state except `settings_set`. Moves `game_share_place`, `game_settings_look`, `game_settings_choice`, `game_use_own(_at)`, `game_say`, `game_settings_view/line/status` and the game part of `share_before_play` (752-773). | ~230 |
| **share** (`worker/share.rs`, exists) | `ShareState` (10, or 7 after the split above) | `AddonLocalOnly`, `AddonSettingsPerPc`, `AddonSettingsChoice`, `IncomingInstallAnyway` (4, plus the share half of `OkAnyway`) | **From addons:** the record store through named mutations (`set_local_only`, `set_settings_per_pc`, `apply_followed_switches`, `remove_as_on(pc)`, `mark_ready(id, tag, sha)`), plus read access to found, enabled, lines, catalog, newer, refused, and the calls `install(want)`, `scan()`, `fetch()`, `say()`. **From sync:** game running, and the gate setter. **From feedback:** card lines. **From forge:** `ForgeView`, `ask_follow`. **Exposes:** `share_wanted()` (downloads), `share_line`, `incoming`, `header`, `status`, `share_removed()`, `share_repo()`. | ~1,320 |
| **addons** (`worker/addons.rs`, exists) | `AddonsState` minus `net` (23) | 24 commands (2.3, plus the addon half of `OkAnyway`); `Answer::Looked`, `Fetched`, `LookedUp` | **From settings:** `game_dir`, `engine.settings.addons`. **From sync:** `game_running()`, `in_session()`. **From forge:** `ForgeView`, `ask_first`, `judge`. **From share:** `share_wanted`, the share view pieces, `share_removed`. **From feedback:** card lines at Play. Sends on the shared channel. | ~1,990 |

### 6.3 What the remaining `Worker` holds

| Kind | Contents |
|---|---|
| **Fields (~17)** | `engine`, `scanner`, `away`, `epoch`, `quit`, `net` (moved from `AddonsState`), `last_refresh`, and one field per concern: `sync`, `playtime`, `settings`, `feedback`, `update`, `view`, `addons`, `share`, `forge`, `news`, optionally `game_settings`. |
| **Functions** | `new`, `run`, `startup`, `tick`; `handle`, which routes each variant to its concern and keeps the two pre-dispatch passes, or better, has each concern declare "this command changes addons"; `on_net` (the `Answer` dispatcher); the orchestration sequences (`play_sequence`, `on_game_closed`, `on_session_over`, `after_settings_changed`); `busy_push`; `notify`, `activity`; `game_files_stamp`. |
| **Size** | about 600 lines |

### 6.4 Extraction order, least to most entangled

| Step | What | Why here |
|---|---|---|
| 0 | **Pure moves that remove false couplings.** Move `net` from `AddonsState` to `Worker`. Move `satchel_look` (addons.rs:478) next to the other update code. Move the `Answer` dispatcher (addons.rs:496-581) to `Worker::on_net`, leaving the three ADD bodies as ADD methods. Move `browse_view` to the settings section. | No behaviour change. Afterwards NEWS, FRG and UPD no longer depend on ADD for the channel. |
| 1 | **news** | After step 0 it has zero field couplings: one command, one `Answer`, one view function. |
| 2 | **self-update** | 7 fields. Its couplings are read-only (`gated`, game running, pending play) plus a write to `quit`. |
| 3 | **playtime** | 2 fields plus 3 `Lists` fields. Fed by three session hooks. |
| 4 | **feedback** | 7 fields. Everyone writes to it but only `build_view` reads it, so it is a mechanical rename of call sites (`self.card_line` becomes `self.feedback.card_line`). |
| 5 | **settings and first start** | Needs the "sync settings changed" hook and `first_sync` moved into sync first. Consolidates `after_settings_changed`. |
| 6 | **forge** | Needs the read-only addons accessors and the "return held installs" change that breaks the ADD↔FRG cycle. |
| 7 | **sync** (session, then progress) | The spine. Do it after feedback, playtime, update and settings exist, so its outward calls already have names. Merge the four Decision copies and the two take-over copies only in a later, separate change, not during the move. |
| 8 | **view assembly** | Once every concern exposes its `*_view` or facts, `build_view` is a pure assembler. |
| 9 | **game-settings sharing** (optional) | Out of share.rs. |
| 10 | **addons ↔ share, last and together** | The two-way field writes (`store`, `ready`) must first become named mutations on `AddonsState`; then share stops touching addon fields. These two are the most entangled pair. Do them in one step, or keep them as one "addons" concern with `share` as its submodule, as the brief's grouping already suggests. |

---

## 7. Line counts

### 7.1 Files today (`pr-0.9`, `wc -l`)

| File | Lines |
|---|---|
| src/worker.rs | 2,373 |
| src/worker/addons.rs | 2,098 |
| src/worker/share.rs | 1,550 |
| src/worker/forge.rs | 1,004 |
| src/worker/game_news.rs | 186 |
| **Worker total** | **7,211** |
| src/main.rs | 584 |
| src/view.rs | 1,575 |

### 7.2 Method lines by concern today

These are the summed spans of the `impl Worker` methods assigned to each concern. Free helpers, consts and struct definitions are excluded.

| Concern | Method lines | Where |
|---|---|---|
| ADD | 1,637 | addons.rs. A further ~353 lines of helpers and types sit outside `impl`: 1-318 and 2064-2098. |
| SHR | 1,381 | share.rs (+165 helpers: 1-165) |
| SYNC | 772 | worker.rs 730-1394, 1664-1768 |
| FRG | 676 | forge.rs (+324 helpers: 1-324) |
| INF | 482 | worker.rs 393 (`new`, `run`, `startup`, `handle`, `tick`, `notify`, `activity`) + addons.rs 89 (`addons_answer`) |
| WIN | 399 | worker.rs (`build_lists`, `build_view`, `push`, the card helpers, `say`, `theme_dark`) |
| SET | 308 | worker.rs 1396-1663, 2168-2203, `game_dir`. The inline `handle` arms (560-618, about 59 lines) are counted under INF. |
| UPD | 144 | worker.rs 126 + addons.rs 18 |
| NEWS | 102 | game_news.rs (+81 helpers) |

### 7.3 Proposed modules (estimates)

| Module | Est. lines | Built from |
|---|---|---|
| worker.rs (remaining core and orchestration) | ~600 | `new`, `run`, `startup`, `tick`, `handle` routing, `on_net`, 4 sequences, helpers |
| worker/sync.rs (or session ~260 + progress ~640) | ~900 | 730-1394, 1664-1768, progress part of `build_lists` 1775-1874, the first-sync part of `finish_setup` |
| worker/settings.rs | ~400 | 1396-1663 minus the first sync, 2168-2203, `handle` arms 560-618, 2346-2366 |
| worker/view_model.rs | ~300 | 1910-2116, `recent` part of `build_lists`, 263-309, 2369-2373 |
| worker/self_update.rs | ~190 | 2153-2167, 2204-2313, addons.rs 473-493, 253-261, consts 34-38 |
| worker/feedback.rs | ~90 | 2133-2136, 2315-2343, expiry from `tick`, state |
| worker/playtime.rs | ~90 | 1875-1898, the record part of `session_over` 930-941, state |
| worker/game_news.rs | ~186 | unchanged |
| worker/forge.rs | ~1,000 | unchanged, plus small accessor and return-value changes |
| worker/addons.rs | ~1,990 | 2,098 minus `satchel_look` (21) minus the dispatcher shell (~86) plus named store mutations (~20) |
| worker/share.rs | ~1,320 | 1,550 minus game-settings (~230) |
| worker/game_settings.rs (optional) | ~230 | see 6.2 |
| **Sum** | **~7,300** | about 1% over today's 7,211, for state structs and accessors |

`view.rs` (1,575) is UI-thread code and outside this split. Its `apply` (view.rs:1295-1506) is already sectioned to match the `View` areas in 4.2, so it could later be split the same way.
