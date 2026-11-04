# Survivor's Satchel 0.9.1: the worker split

The whole brief for whoever implements 0.9.1, including an implementer who has only this repository.
Written 2026-11-05 against `main` after 0.9.0. **Branch `0.9.1` from `main`.** 0.9.1 changes nothing a player
sees: it reorganises the app's worker (the one thread that syncs progress, manages addons, talks to
SurvivorsForge, reads the game's notes, applies settings and feeds the window) into one module per concern,
then adds the clippy and rustfmt gates. The map of the code as it stands is `docs/plan-0.9.1-map.md`: read it
first, whole. Its line numbers come from the 0.9 branch before its last four commits, so they are a few dozen
lines off here and there; the names and the structure hold. Where the map, this plan and the code disagree,
the code wins on what exists, this plan on what to do; say so in the pull request.

## 0. Ground rules, read first

1. **Version and changelog first.** `version = "0.9.1"` in `Cargo.toml` and a dated `## 0.9.1 (YYYY-MM-DD)`
   section at the top of `CHANGELOG.md` (section 5's text) before any other change.
2. **No behaviour change.** Every step is a pure move: code changes its file, its `self.x` becomes
   `self.concern.x` or a named method, and nothing else changes. A reader must be able to read each step's
   diff as a move. Any bug you meet on the way is noted in the pull request, not fixed in the move; a fix,
   if small and certain, is its own commit after the moves, named as a fix.
3. **Do not touch:** `crates/satchel-core/` (the logic the worker calls), `ui/app.slint`, `src/view.rs`,
   `tools/`, `catalog/`, `build/`, `.cargo/`, the README. `src/main.rs` only where a command's name or path
   changes. The workflows only in step 11, and only to add the two gates.
4. **No new dependency.** No new crate, no new feature on an existing one.
5. **Shape** (the map's section 6.1): one thread, one `Worker` that owns all state; one file per concern with
   its own state struct and **private fields**; state-only logic in `impl <Concern>State`; cross-concern
   operations as `impl Worker` blocks inside the concern's file; the orchestration sequences stay in
   `worker.rs` as named functions: `play_sequence`, `on_game_closed`, `on_session_over`,
   `after_settings_changed` (the four copies the map's row 12 lists become that one function). No actors,
   no message bus, no async: Play, the session's end and a settings change need strictly ordered steps with
   game-closed re-checks between them.
6. **One commit per step,** plain messages ("0.9.1 step 3: playtime into worker/playtime.rs"), no trailers;
   each commit compiles for Windows with 0 warnings and passes the tests before the next step starts.
   The maintainer squashes the branch at merge.
7. Comments and names stay in the code's voice: plain words, no references to plans or reviews.

## 1. The steps, in this order (the map's section 6.4)

| Step | What moves | Done when |
|---|---|---|
| 0 | The false couplings: `net: Sender<Job>` from `AddonsState` to `Worker`; `satchel_look` out of `addons.rs` next to the other self-update code; the `Answer` dispatcher (`addons_answer`) to `Worker::on_net`, leaving the `Looked`, `Fetched` and `LookedUp` bodies as addons methods; `browse_view` to the settings code. | news, forge and self-update no longer reach into addons for the channel. |
| 1 | **news** → `worker/game_news.rs` gets its `NewsState` with private fields, `news_view()`, `news_answer()`. | one command, one answer, one view function; no field reach from outside. |
| 2 | **self-update** → new `worker/self_update.rs`: the seven fields (`github_offer`, `satchel_looked`, `satchel_failed`, `check_pending`, `check_result`, `checks`, `update_after_session`), `UpdateNow`, `CheckUpdates`, `WhatsNewSeen`, `Answer::Satchel`; exposes `offer()`, `update_line()`, the pill facts, `on_session_over()`. Reads the gate and "game running" through sync's accessors. | |
| 3 | **playtime** → new `worker/playtime.rs`: `session_start`, `closed_at`, the three list fields; hooks `started(now)`, `closed(now)`, `over(now, updated)`; the playtime view, the rail's session line, per-PC last played. | |
| 4 | **feedback** → new `worker/feedback.rs`: `notice`, `notice_at`, `problem`, `card`, `card_until`, `card_gate`, `busy`; `say`, `card_line`, `card_ask`, `open_card`, `close_card_in`, `set_busy` / `clear_busy`, `set_problem`; its expiry from `tick`. "Set busy, then push now" stays a `Worker` method. | every `self.card_line(..)` call site reads `self.feedback.card_line(..)`; `build_view` reads its facts. |
| 5 | **settings and the first start** → new `worker/settings.rs`: `setup` and the ten settings commands; `game_dir()`, `form_view()`, `name_taken()`, `theme_dark()`. First move the first-sync decision out of `finish_setup` into sync as `first_sync(now)`, and give sync a hook "sync settings changed". Then consolidate the four copies of "after a settings change" into `Worker::after_settings_changed` (the map's row 12; note the two copies that skip `forge_look` today: keep each copy's exact behaviour in this step, and list the difference in the pull request as a candidate fix). | |
| 6 | **forge**: `ForgeState` fields private; read-only accessors on `AddonsState` for the catalog, lines, offers, newer and waiting (forge stops reaching into addons' fields); `forge_versions_answer` and `forge_release_held` **return** the released held installs, and `Worker` dispatches them to addons and then runs `share_look` / `share_follow_now`. This breaks the addons ↔ forge call cycle. | `grep -n 'addons\.' src/worker/forge.rs` shows only accessor calls. |
| 7 | **sync** → new `worker/sync.rs` (or `sync/session.rs` + `sync/progress.rs`): the twenty fields, the seven list fields, the `Row` enum, the ten commands; exposes `game_running()`, `in_session()`, `gated()`, `set_gated()`, the progress facts, the outlook's PC names; provides `check_before_play`, `send_after_close`, `session_over_sync` for the sequences on `Worker`. The four `Decision::InSync` copies and the two take-over copies stay as they are in this step (merging them is a later change, not a move). | |
| 8 | **view assembly** → `worker/view_model.rs`: `last_view`, the lists container, the recent lines; `push`, `build_view`, `released_text`, the time helpers (`when`, `clock`, `ago`, `size`). Every concern exposes `*_view()` or facts; `build_view` becomes a pure assembler. | |
| 9 | **game-settings sharing** → `worker/game_settings.rs` out of `share.rs` (the three fields, two commands, the functions the map names). Optional: do it if steps 0–8 took less than two thirds of your budget; else say so. | |
| 10 | **addons and share, together, last:** the two-way field writes become named mutations on `AddonsState` (`set_local_only`, `set_settings_per_pc`, `apply_followed_switches`, `remove_as_on(pc)`, `mark_ready(id, tag, sha)`); share stops touching addons' fields; both state structs' fields go private. | `grep -n 'addons\.store\|addons\.ready' src/worker/share.rs` is empty. |
| 11 | **The gates:** `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings` clean, fixing the cheap lints and allowing the rest one by one with a reason (`#[allow(clippy::…)]` with a comment; never a crate-wide allow); a `rustfmt.toml` with the width the code already uses (measure it; keep the diff of `cargo fmt` to whitespace); `cargo fmt --all --check` clean, the fmt run as its own commit, last; both gates added to the `check` job of `.github/workflows/check.yml` as two steps, nothing else in the workflows changed. | the Check workflow is green with the gates on. |

After step 10 the remaining `worker.rs` holds about 17 fields and about 600 lines: `new`, `run`, `startup`,
`tick`, `handle` (routing only), `on_net`, the four sequences, `busy_push`, `notify`, `activity`,
`game_files_stamp` (the map's section 6.3). The sum of all modules stays within a few percent of today's
7,200 lines (section 7.3).

## 2. What to keep in sight while moving

- **Ordering rules the moves must not disturb** (the map's section 5.4): progress before addons at Play;
  lines before sharing decisions; the forge cache before the first lists; the catalog before the forge join;
  the game closed for every write into its folder; addons need the game folder; the send at close before
  addon settings go up; `refresh` before `addons_after_session` before `update_now` at a session's end;
  `busy` as the one "no second network-folder operation" flag; `shown` written by `push` and consumed by
  `resolve`; the gate's three writers and one reader.
- **`src/` has no tests.** The compiler, the core tests and the preview renders are the only checks, so a
  step is right when its diff reads as a move. Keep the steps small enough to read.
- **Public-safe text:** nothing personal, no PC names, "network folder" in any new words.

## 3. Checks, per step and at the end

1. `cargo test -p satchel-core -p satchel-catalog` (234 + 20 + 7 today; the same after every step).
2. `cargo check --release --target x86_64-pc-windows-msvc` with 0 warnings, after every step.
3. `cargo run --release -p ui-preview -- dist/preview` once at the end: the 53 images must be identical to
   the ones on `main` (the preview does not run the worker, so any difference is a sign something else moved).
4. `cargo tree -d` unchanged but for the app's own version; no new dependency.
5. Step 11's two gates, clean.
6. `wc -l src/worker.rs src/worker/*.rs` before and after, in the pull request.

## 4. Not in 0.9.1

Merging the four `Decision::InSync` copies and the two take-over copies; splitting `src/view.rs`; any
change a player notices; new behaviour tests; anything in satchel-core; the README.

## 5. Texts

Under `## 0.9.1 (YYYY-MM-DD)`, draft, adjust to what shipped:

```
Housekeeping, nothing new to see.

### Changed
- The app's insides are reorganised by concern (progress sync, addons, SurvivorsForge, the game's
  notes, settings, the window), with no change in what the app does.
- The code checks now also run clippy and rustfmt, so later changes stay tidy.
```

## 6. The pull request

One branch, `0.9.1`, from `main` after 0.9.0; one pull request against `main`; one commit per step. The
description lists each step with what moved and its before/after line counts, the final module table, every
bug met on the way (not fixed, or fixed in its own named commit), the differences kept in step 5, the clippy
allows with their reasons, and every check of section 3 with its result. The maintainer reviews each commit
as a move, squashes, tags `v0.9.1`, and the release workflow builds the exe.
