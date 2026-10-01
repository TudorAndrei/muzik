# TODO: Deepen the watchlist, acquisition, queue, and GUI modules

## Phase 1: Settings, paths, and the decision agent as values

- [x] `Paths` and `expand_home` in `muzik-core/src/paths.rs`
- [x] `Repository` and `db` take `&Paths`
- [x] `muzik-runner/src/settings.rs` with `Settings::resolve`
- [x] Local, remote, watchlist, GUI, and CLI callers use `Settings`
- [x] `Chooser` seam (`Codex` adapter; `None` in tests) in `runner::Options`
- [x] Delete the four extra `~` expanders
- [x] Tests use `Paths::under(temp)`
- [x] Commit: `refactor(runner): resolve settings and paths once and inject the decision agent`

## Phase 2: One yt-dlp module

- [x] `muzik-workflow/src/ytdlp.rs` with tests
- [x] Replace yt-dlp code in runner, workflow discovery, and CLI
- [x] Commit: `refactor(workflow): run yt-dlp through one cancellable module`

## Phase 3: Soulseek search and fetch in muzik-soulseek

- [x] `Session::search` and `Session::fetch`
- [x] Timeouts in `fetch::Timeouts`
- [x] Runner, quality check, and CLI use them
- [x] Commit: `refactor(soulseek): search and fetch through one blocking interface`

## Phase 4: One import decision policy

- [x] `muzik-import/src/decide.rs` with `ImportPolicy` and `decide_album`
- [x] Runner and CLI import use it; `muzik import --duplicates`
- [x] Policy matrix test
- [x] Commit: `fix(import): apply the duplicates setting on every import path`

## Phase 5: CLI workflow through the runner

- [x] `compilation` workflow option
- [x] `muzik workflow` enqueues and drains; `CliOperations` deleted
- [x] CLI `ask` handles chapter edits
- [x] `drain` returns an error when a job fails, so the exit code stays correct
- [x] Commit: `refactor(cli): run the workflow command through the shared runner`

## Phase 6: Typed watchlist item

- [x] `watchlist/item.rs` typed document and transitions
- [x] Core and runner use the typed item
- [x] Delete duplicated stage helpers and audio lookups
- [x] Stored JSON round-trip test
- [x] Real watchlist: `muzik watchlist list --items` output is the same before and after
- [x] Commit: `refactor(watchlist): type the watchlist item and own its stage transitions`

## Phase 7: Job queue in muzik.db and one item identity

- [x] muzik.db migration 2 with `jobs`
- [x] `Store::from_connection`; one-time `jobs.db` copy (only under the runner lock, never for an in-memory queue)
- [x] `ItemId` in queue, park, jobs, GUI (same key format, so open jobs keep their keys)
- [x] Bridge tests use temporary paths
- [ ] Park and waiting state in one transaction (moved to Phase 9, where the pause becomes a returned value)
- [x] Commit: `feat(jobs): store the job queue in muzik.db with one item identity`

## Phase 8: Source modules

- [x] `muzik-runner/src/sources/` with YouTube, Spotify, Bandcamp behind a `Source` trait
- [x] Availability rules move into `muzik-core/src/watchlist/source.rs`, one function per kind (core must compute them for the view)
- [x] Bandcamp `ensure` in the watchlist sync (`watchlist::ensure_sources`); the GUI calls it
- [x] `release_import_questions` asks `SourceKind::keeps_current_tags`
- [x] CLI `bandcamp` uses the Rust module; bandsnatch removed
- [x] Same `watchlist list --items` output on a copy of the real data
- [x] Commit: `refactor(watchlist): give each source kind one module behind a Source seam`

## Phase 9: Waiting for a choice as a returned value

- [x] Park and waiting state in one transaction (`Operations::park`, `Repository::update_with`, `muzik_jobs::park_on`)
- [x] Typed `DecisionError` — not done; the decide callback keeps `String` (see PLAN.md)
- [x] `ItemOutcome::Waiting` carries the question
- [x] Delete `mark_stage`, `take_stage`, and the event-driven park; explicit `Cell<Stage>`
- [x] End-to-end decide → park → answer → resume test
- [x] Commit: `refactor(runner): return a pause for a choice instead of side channels`

## Phase 10: Retire the legacy cache reconcile

- [x] muzik.db migration 3 with `meta`
- [x] `watchlist/legacy.rs` one-time import (runner sync and GUI check call `import_cache`)
- [x] Slim `reconcile.rs`; update core watchlist tests
- [x] A second reconcile changes no rows (test), so a load does not rewrite thousands of rows
- [x] Same item states as the old reconcile on a copy of the real data
- [x] Commit: `refactor(watchlist): import the legacy cache once and slim the reconcile`

## Phase 11: Typed application module behind the GUI

- [x] `muzik-runner/src/app.rs` with `App`; `AppEvent` from the runner `Sink`
- [x] Bridge is a thin adapter; watchlist load, reconcile, edits, and keys out of the GUI
- [x] `main.rs` matches on `AppEvent` (responses for GUI-local settings stay string commands)
- [x] Bridge tests use temp paths and no chooser
- [x] `muzik-gpui --check-backend` works on a copy of the real data; 79 open jobs move from `jobs.db`
- [x] Commit: `refactor(gui): drive the desktop app through a typed application module`

## Verification

- [x] `mise run check` passes after each phase (fmt, clippy `-D warnings`, tests, cargo deny); 296 tests at the end
- [x] New tests: settings resolve, yt-dlp fake script, Soulseek fetch cancel, import policy matrix, stored JSON round trip, jobs.db copy, legacy cache import, decide → park → resume
- [x] Manual smoke test with a temporary HOME: `muzik workflow <local flac> --dry-run`, `muzik watchlist list --items`, `muzik-gpui --check-backend` (the GPUI window itself was not opened)
- [x] Edge cases: muzik.db at version 1 with a legacy `jobs.db`; waiting jobs survive the copy; Spotify waiting import questions still release; cancelled yt-dlp stops within 5 s
- [x] No behavior change in the watchlist cards: same `watchlist list --items` output and the same item states after reconcile, on a copy of the real data
- [x] Migration: a version 1 database keeps its rows; `jobs.db.migrated` kept; a newer database version still refuses to open

## Review

- [ ] Code reviewed (by you)
- [x] PLAN.md updated if approach changed during implementation
- [x] All phase commits are clean and describe their intent
- [x] TODO.md items all checked off, except the review
