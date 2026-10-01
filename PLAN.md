# Plan: Deepen the watchlist, acquisition, queue, and GUI modules

## Goal

Implement the eight candidates from the 2026-10-01 architecture review. The
watchlist, the runner, the CLI, and the GUI now share untyped JSON, global
config reads, three copies of the acquisition steps, and side channels for
"waiting for a choice". After this work, each concept has one deep module with
a small interface: a typed watchlist item with its stage rules, one Source
module per source kind, shared yt-dlp and Soulseek modules, one import
decision policy, one item identity in one database, a returned `Waiting`
outcome, settings resolved once, and a typed application module behind the
GUI. The work also fixes the `--duplicates` divergence between the direct CLI
path and the queue path.

## Approach

### Order

The phases go from the foundation up, so each phase can use the modules of
the phases before it:

1. Settings, paths, and the decision agent become values that callers pass in.
2. One yt-dlp module, one Soulseek `search_and_fetch`, and one import decision
   policy replace the copies in the CLI, the runner, and muzik-workflow.
3. The CLI `workflow` command runs through the runner, so `CliOperations` goes
   away.
4. The watchlist document becomes typed (`Watchlist`, `Playlist`, `WatchItem`,
   `StageRecord`) with transition methods. The stored JSON shape stays the
   same, so muzik.db needs no data migration for this step.
5. The job queue moves into muzik.db, and `ItemId` becomes the one item
   identity.
6. Each source kind becomes one module behind a `Source` trait.
7. A pause for a choice becomes a returned value; the thread-local stage and
   the `item_waiting` control event go away.
8. A one-time import of the legacy Python cache files replaces most of
   `reconcile.rs`.
9. A typed `App` module in muzik-runner replaces the string-command bridge.

### Key design decisions

- **Settings (`muzik-runner/src/settings.rs`).** `Settings::resolve(defaults:
  &Value, params: &Value)` merges the GUI defaults with the request params one
  time and parses every field into types. It replaces
  `local_workflow::parse`, `remote_workflow::parse`, `watchlist::Prepared::new`,
  `apps/gui/src/watchlist.rs::Options`, and `apps/cli/src/watchlist.rs::output`.
  `muzik_core::paths::expand_home` becomes the one `~` expander. The other
  four copies are deleted (`local_workflow.rs:105-120`,
  `apps/gui/src/watchlist.rs:76-97`, `apps/cli/src/config.rs:195`,
  `apps/cli/src/soulseek.rs:833`).
- **Paths (`muzik-core/src/paths.rs`).** A `Paths` value (`data`, `cache`,
  `config_file`, `downloads`) with `Paths::user()` for the app and
  `Paths::under(root)` for tests. `Repository::default()` and
  `db::default_path()` take a `&Paths`. Tests stop reading the real user
  config and muzik.db.
- **Decision agent seam (`muzik-runner/src/agent.rs`).** A `Chooser` trait with
  two adapters: `CodexChooser` (wraps `muzik_agent::decide`) and `NoChooser`.
  `Runner` `Options` gets `chooser: Arc<dyn Chooser>` and `settings` (a
  function that returns the current GUI defaults), so `agent_model` stops
  reading `app_config::path()` per decision. Tests use `NoChooser`, so they
  never start `codex`.
- **yt-dlp module (`muzik-workflow/src/ytdlp.rs`).** One cancellable runner
  (`run(args, timeout, cancelled) -> Output`) plus `json`, `print`,
  `download`, `playlist_ids`, and `environment_args`. It replaces
  `remote_workflow::{execute, execute_with_path, download, playlist_ids,
  yt_dlp_environment_args}`, `runner/watchlist.rs::{youtube_source,
  youtube_video_metadata}`, `discovery.rs:84-112`, `apps/cli/src/download.rs`
  argument building, and `apps/cli/src/workflow.rs::{youtube_print,
  youtube_playlist_video_ids}`. Gate entry stays at the runner call sites.
- **Soulseek (`muzik-soulseek/src/fetch.rs`).** `Session::search(query,
  prefer, timeout, cancelled) -> Vec<RankedCandidate>` and
  `Session::fetch(candidate, destination, limit, timeout, cancelled) ->
  Vec<PathBuf>` block, poll, cancel, check file names, and apply the timeout
  inside the crate. `remote_workflow::soulseek_download`,
  `apps/cli/src/soulseek.rs::{ranked_search, wait_download}` and
  `quality.rs::{SoulseekBackend::search, SoulseekBackend::download,
  await_job}` call them. Timeout settings move into a `fetch::Timeouts`
  value (not `SessionSettings`: a timeout change must not reconnect the
  shared session).
- **Import policy (`muzik-import/src/decide.rs`).** `decide_album(album,
  policy, ask) -> AlbumDecision` holds the match and duplicate rules now in
  `local_workflow.rs:224-279`. `ImportPolicy { interactive, force,
  duplicates }` is the input; `ask` is the only callback. `apps/cli/src/import.rs`
  uses the same function, so `--duplicates` applies on every path.
- **CLI workflow through the runner.** `muzik workflow` builds the params,
  enqueues one workflow job, and drains the queue with the CLI `ask` prompt.
  `--queue` stays as an accepted alias. `--compilation` becomes a workflow
  option that the runner passes to `SplitOptions::compilation`. A chapter edit
  question opens the editor through `split::review_chapters`.
- **Typed watchlist (`muzik-core/src/watchlist/item.rs`).** Serde structs with
  the current JSON field names. `WatchItem` methods: `start(action)`,
  `complete(stage)`, `fail(stage, message)`, `wait(stage, question)`,
  `invalidate_after(stage)`, `skip(stage)`, `stage(stage)`,
  `downloaded_audio(output)`, `is_unavailable()`, `is_done()`. One timestamp
  rule. One audio lookup (recursive) for the view and the runner. JSON exists
  only in `Repository` and in the `Serialize` output for the GUI.
- **One database.** muzik.db migration 2 creates the `jobs` table (with
  `cancel_requested`). `muzik_jobs::Store::from_connection` takes the
  connection that `muzik_core::db::open` returns. On first open, open jobs in
  `jobs.db` are copied and the file is renamed to `jobs.db.migrated`.
  `ItemId { playlist_id, position, key }` lives in the watchlist module; the
  queue, `park`, the GUI, and `processed_video_ids` use it. Park and the
  waiting stage write in one transaction.
- **Source modules (`muzik-runner/src/sources/{mod,youtube,spotify,bandcamp}.rs`).**
  `trait Source { fn load(..) -> LoadedSource; fn process(item, action, ctx) ->
  Result<WatchItem, JobError>; fn stages(..); fn availability(item, action) ->
  Availability; fn answers_import_questions(&self) -> bool; }` plus
  `SourceKind::parse_input` in core for the add step. `runner/watchlist.rs`
  keeps the `Operations` adapter and dispatches by kind one time.
  `queue.rs::release_spotify_questions` asks the source. The Bandcamp
  `ensure` step moves from the GUI bridge into the watchlist load. The CLI
  `bandcamp` command uses the Rust Bandcamp module, and the bandsnatch
  dependency goes away.
- **Waiting as a value.** The decide closure returns `Err(Decision::Waiting
  (question))` through a typed error in `muzik_core::DecisionError`; the
  source modules map each stage step to `JobError::Failed { stage }` or
  `JobError::Waiting { stage, question }` at the call site. `jobs::run_item`
  returns `ItemOutcome::Waiting { stage, question }`, and the runner parks from
  it. `gates::{mark_stage, take_stage}`, `Parked`, and the `item_waiting`
  control path are deleted; `item_waiting` stays only as an event for display.
- **Legacy cache.** A one-time import in `watchlist/legacy.rs` reads
  `playlist_{id}.json` and `yt_{id}.txt`, writes the stage state into
  muzik.db, and records `legacy_cache_imported` in a `meta` table (migration
  3). `reconcile` keeps only: reset of stale `Running` stages at start, the
  library lookup, and the output folder lookup.
- **GUI app module (`muzik-runner/src/app.rs`).** `App` with typed methods
  (`load_watchlist`, `edit_watchlist`, `refresh`, `run_item`, `start_workflow`,
  `answer`, `cancel`, `jobs`) and an `AppEvent` enum. `apps/gui/src/bridge.rs`
  shrinks to a thin adapter over `App`; `main.rs::message` matches on
  `AppEvent`, not on strings. The GUI stops calling `muzik_runner::item_key`
  and stops merging launcher defaults.

### Out of scope

- New features, new source kinds, and GUI layout changes.
- The Python-era `watchlist.json` import (it already exists and stays).
- `choices.rs` (clean and tested; no change).

## Implementation Phases

### Phase 1: Settings, paths, and the decision agent as values

- Add `Paths` and `expand_home` to `crates/muzik-core/src/paths.rs`; make
  `db::default_path`, `watchlist::Repository`, `app_config::path` take or use
  `&Paths`.
- Add `crates/muzik-runner/src/settings.rs` with `Settings::resolve`; move the
  parsing from `local_workflow::parse` into it; use it in
  `remote_workflow::supported`, `local_workflow::supported`,
  `watchlist::Prepared`, the GUI `watchlist::Options`, and the CLI watchlist.
- Add `crates/muzik-runner/src/agent.rs` with `Chooser`, `CodexChooser`,
  `NoChooser`; pass it and the settings source in `runner::Options`.
- Delete the four extra `~` expanders.
- Fix `remote_workflow` test `selects_youtube_video_and_playlist` to use an
  explicit `Paths::under(temp)`.
  **Commit:** `refactor(runner): resolve settings and paths once and inject the decision agent`

### Phase 2: One yt-dlp module

- Add `crates/muzik-workflow/src/ytdlp.rs`; move the fake-script tests from
  `remote_workflow.rs` into it.
- Replace the yt-dlp code in `remote_workflow.rs`, `runner/watchlist.rs`,
  `discovery.rs`, `apps/cli/src/download.rs`, `apps/cli/src/workflow.rs`.
  **Commit:** `refactor(workflow): run yt-dlp through one cancellable module`

### Phase 3: Soulseek search and fetch in muzik-soulseek

- Add `crates/muzik-soulseek/src/fetch.rs` with `Session::search` and
  `Session::fetch`; move timeouts into `fetch::Timeouts`.
- Use them in `remote_workflow::soulseek_download`, `quality.rs`
  `SoulseekBackend`, and `apps/cli/src/soulseek.rs`.
  **Commit:** `refactor(soulseek): search and fetch through one blocking interface`

### Phase 4: One import decision policy

- Add `crates/muzik-import/src/decide.rs` with `ImportPolicy` and
  `decide_album`; move the rules from `local_workflow::organize`.
- Use it in `apps/cli/src/import.rs` and `organize.rs`; add a `--duplicates`
  option to `muzik import`.
- Test the policy matrix (interactive × force × duplicates × has-duplicates).
  **Commit:** `fix(import): apply the duplicates setting on every import path`

### Phase 5: CLI workflow through the runner

- Add `compilation` to `WorkflowOptions` and pass it to `SplitOptions`.
- `apps/cli/src/workflow.rs::run` enqueues and drains; delete
  `CliOperations`; the CLI `ask` handles `ChapterEdit` with the editor.
  **Commit:** `refactor(cli): run the workflow command through the shared runner`

### Phase 6: Typed watchlist item

- Add `crates/muzik-core/src/watchlist/item.rs` with the typed document and
  transition methods; use it in `watchlist.rs`, `jobs.rs`, `reconcile.rs`,
  `view.rs`, and `runner/watchlist.rs`.
- Delete the three `set_stage` copies, `stage_path`, `set_path`,
  `view::find_audio` and `runner::downloaded_audio` in favor of the methods.
- Keep the JSON shape; add a round-trip test on a stored document.
  **Commit:** `refactor(watchlist): type the watchlist item and own its stage transitions`

### Phase 7: Job queue in muzik.db and one item identity

- muzik.db migration 2 with the `jobs` table; `Store::from_connection`;
  one-time copy of open `jobs.db` jobs; rename to `jobs.db.migrated`.
- Add `ItemId`; use it in `queue.rs`, `runner.rs::park_item`, `jobs.rs`, and
  the GUI; write park and waiting state in one transaction.
  **Commit:** `feat(jobs): store the job queue in muzik.db with one item identity`

### Phase 8: Source modules

- Add `crates/muzik-runner/src/sources/`; move `process_youtube`,
  `process_spotify`, `process_bandcamp`, the item mapping, and the
  availability rules into one module per kind.
- Move the Bandcamp `ensure` step into the watchlist load; ask the source in
  `release_spotify_questions`.
- Replace the bandsnatch CLI path with the Rust Bandcamp module; remove the
  bandsnatch service check and docs.
  **Commit:** `refactor(watchlist): give each source kind one module behind a Source seam`

### Phase 9: Waiting for a choice as a returned value

- Typed `DecisionError` for the decide callback; `ItemOutcome::Waiting`
  carries the question; the runner parks from it.
- Delete `gates::{mark_stage, take_stage}`, `Parked`, and the event-driven
  park.
- Add an end-to-end test: decide → park → answer → resume through the real
  watchlist adapter with a fake source.
  **Commit:** `refactor(runner): return a pause for a choice instead of side channels`

### Phase 10: Retire the legacy cache reconcile

- muzik.db migration 3 (`meta` table); `watchlist/legacy.rs` one-time import.
- Shrink `reconcile.rs` to the stale-run reset, the library lookup, and the
  output lookup; update `crates/muzik-core/tests/watchlist.rs`.
  **Commit:** `refactor(watchlist): import the legacy cache once and slim the reconcile`

### Phase 11: Typed application module behind the GUI

- Add `crates/muzik-runner/src/app.rs` (`App`, `AppEvent`); move the watchlist
  load, reconcile-and-save, and retry logic out of `apps/gui/src/bridge.rs`.
- `main.rs` matches on `AppEvent`; delete the string command table and the
  `pending` / `latest_reads` command-string bookkeeping where `App` covers it.
- Bridge tests use `Paths::under(temp)` and `NoChooser`.
  **Commit:** `refactor(gui): drive the desktop app through a typed application module`

## Risks & Tradeoffs

- **Size.** Eleven phases touch most crates. Each phase must pass `mise run
  check` before its commit, so a regression stops at its phase.
- **Stored data.** Phase 6 keeps the JSON shape, so no data changes. Phases 7
  and 10 change muzik.db; each migration runs in one transaction, keeps the
  old file as `*.migrated`, and has a test that opens a pre-migration fixture.
- **Behavior changes that are intended.** A direct `muzik workflow` run now
  asks the import match question in a terminal (as the queue path does) and
  applies `--duplicates`. `muzik bandcamp` stops using bandsnatch.
- **Item identity.** For Spotify and Bandcamp, the queue key changes from the
  track ID to the entry ID. The jobs.db copy recomputes keys from the
  watchlist item at the same playlist and position.
- **GUI phase.** GPUI code is hard to test; the `App` module carries the
  logic so tests run without GPUI.

## Open Questions

- None that block the work. If a phase shows that a design above does not
  fit, PLAN.md and TODO.md change before that phase continues.
