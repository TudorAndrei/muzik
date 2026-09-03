# TODO: Replace slskd with embedded Seakarr

## Phase 1: Add measured audio quality decisions

- [x] Extend `QualityInfo` in `muzik/core/sources/base.py` with measured audio
  fields.
- [x] Add local `ffprobe` measurement in `muzik/core/quality.py`.
- [x] Add explicit keep, ask, replace, and no-safe-replacement policy results.
- [x] Add inactive quality options to `WorkflowOptions` and
  `WorkflowLaunchConfig`.
- [x] Test measured quality, invalid audio, missing fields, policy thresholds,
  and launcher value conversion.
- [x] Run `mise run check`.
- [x] Commit: `feat(quality): add measured audio quality decisions`

## Phase 2: Build the embedded Seakarr bridge

Corrected against the real dependency before implementation — see PLAN.md's
"PyO3 bridge" section for the full explanation. The real crate is
`soulseek-rs-lib` at github.com/michel/soulseek-rs (MIT), a synchronous
`Client`, not the originally assumed API/Tokio/commit.

- [x] Get a compatible Seakarr license or written distribution permission.
  (Resolved: MIT — no permission needed.)
- [x] Record the license and required notices in the repository.
  (`rust/seakarr_bridge/THIRD_PARTY_NOTICES.md`.)
- [x] Add `rust/seakarr_bridge/` and pin the verified `soulseek-rs-lib` commit
  `a62bab1e6a505362109b8303aa528af03403eeae`.
- [x] Add `SeakarrSession` and pollable `SeakarrJob` Python classes.
- [x] Add structured track requests, candidates, progress, results, and
  errors. (No album requests: the real crate has no album-level search: see
  PLAN.md.)
- [x] Connect `SeakarrJob.cancel()` to active search and download work.
- [x] ~~Add or consume the reviewed public Seakarr integration module.~~ Not
  applicable: the real `Client` API needed is already public.
- [x] Add Rust tests covering the bridge's own conversion and job-state-machine
  logic against `soulseek-rs-lib`'s public wire types (no mock client exists
  upstream to test search/rank/download against; see PLAN.md).
- [x] Add a Python test that imports `muzik._seakarr` and checks its API shape.
- [x] Run Rust formatting, Rust lint checks, Rust tests, and the Python import
  smoke test.
- [x] Commit: `feat(seakarr): add the embedded Soulseek bridge`

## Phase 3: Replace the slskd adapter

- [x] Add `muzik/core/sources/seakarr.py` and map Rust data to `Candidate`,
  `CandidateFile`, `QualityInfo`, and `DownloadResult`.
- [x] Replace `SoulseekSource` use in workflow operations, workflow services,
  CLI commands, and service checks.
- [x] Add Seakarr and Soulseek settings to `muzik/config.py` and
  `.env.example`.
- [x] Update `muzik config`, `muzik init`, and Settings status text.
- [x] Keep `muzik soulseek` and `source="soulseek"` data compatible.
- [x] Remove `slskd-api` from `pyproject.toml`.
- [x] Remove the slskd service from `docker-compose.yml`.
- [x] Replace slskd fixtures with bridge data used by active behavior tests.
- [x] Test readiness, login errors, search, selection, download, progress,
  cancellation, cache compatibility, and secret redaction.
- [x] Run `mise run check` and the Rust checks.
- [x] Commit: `refactor(soulseek): replace slskd with embedded Seakarr`

## Phase 4: Route Spotify tracks directly to Seakarr

- [x] Change metadata-only acquisition in `WorkflowRunOperations` to accept a
  `ResolvedTrack`.
- [x] Pass each Spotify entry directly from
  `_run_resolved_playlist_workflow()` to the Seakarr adapter.
- [x] Use artist, title, album, duration, track number, and ISRC as separate
  identity evidence.
- [x] Use track search for playlist tracks. Do not get a complete album for one
  playlist track.
- [x] Keep playlist state, duplicate occurrence, and resume behavior.
- [x] Test JSON and CSV input, direct structured requests, no YouTube call,
  unsafe matches, cancellation, duplicate tracks, and resume behavior.
- [x] Run `mise run check` and the Rust checks.
- [x] Commit: `feat(spotify): acquire tracks directly with Seakarr`

## Phase 5: Add the YouTube-first quality flow

- [x] Make YouTube video and playlist input download with `YouTubeSource` before
  any Seakarr search.
- [x] Add the quality operation to `WorkflowRunOperations` and
  `build_workflow_operations()`.
- [x] Measure the YouTube download and apply the configured quality policy.
- [x] Search Seakarr only when the policy requests a better copy.
- [x] Keep the YouTube file until the replacement passes audio, duration, and
  identity checks.
- [x] Use a safe multi-file replacement as pre-split input.
- [x] Use YouTube chapters with a one-file replacement only when durations are
  compatible.
- [x] Continue with the YouTube file when the policy permits and replacement is
  not possible.
- [x] Test single videos, playlists, full albums, no chapters, no Seakarr result,
  bad replacement audio, wrong duration, cancellation, and recovery.
- [x] Prove that no Seakarr call occurs before the YouTube download.
- [x] Run `mise run check` and the Rust checks.
- [x] Commit: `feat(workflow): add YouTube-first quality upgrades`

## Phase 6: Show quality state and controls

- [x] Move the watchlist schema from version 1 to version 2.
- [x] Migrate version 1 records without changing their existing stage results.
- [x] Add Quality between Download and Parse in `STAGE_NAMES`.
- [x] Add Check quality again to `ItemAction` and keep all current actions.
- [x] Mark later stages stale only when the active audio file changes.
- [x] Show measured quality, quality decision, and selected candidate in the
  pipeline and watchlist views.
- [ ] Show live transfer progress. **Not done**: `SeakarrJob` (the Rust bridge)
  only reports terminal status (running/completed/failed/cancelled), not
  partial bytes-downloaded while a download is in flight — showing real
  transfer progress needs new Rust bridge work (expose partial
  `DownloadProgress` from a running job) before the GUI side can display it.
- [x] Add quality controls to the launcher (done in Phase 1; this phase wired
  them through `WorkflowLaunchConfig` into the GUI-launched `WorkflowOptions`,
  which Phase 1 had missed) and a Seakarr row to Settings (automatic since
  Phase 3 renamed the service to "Soulseek (Seakarr)" in the existing generic
  status list).
- [x] Test schema migration, stage order, action availability, stale rules,
  errors, cancellation, and all visible action buttons.
- [x] Run a DearPyGui render-context smoke test for a pending quality check, an
  active transfer, a kept YouTube file, and a completed replacement.
- [x] Run `mise run check` and the Rust checks.
- [x] Commit: `feat(gui): add quality and Seakarr workflow controls`

## Phase 7: Package the native integration

- [x] Change `pyproject.toml` to a Maturin mixed Python and Rust build.
- [x] Add Rust and Maturin tools and checks to `mise.toml`.
- [x] Update `uv.lock`.
- [x] Update `.github/workflows/check.yml` for the Rust build and tests.
- [x] Update `.github/workflows/release.yml` for macOS arm64 and supported Linux
  platform wheels, a source archive, and one shared version.
- [x] Update `packaging/homebrew/muzik.rb` and `DISTRIBUTION.md` for the Rust build
  dependency and native module.
- [x] Build the release wheel and source archive. **Local only**: built with
  `maturin build --release` and `maturin sdist`, per the user's instruction not
  to trigger an actual release. The CI release job itself has not been run.
- [x] Install the wheel in a clean Python 3.14 environment.
- [x] Verify `muzik --help`, `muzik soulseek check`, a direct Spotify track run,
  and the `--quality-policy` flag. **Not done**: a live YouTube-first quality
  run and an interactive `muzik gui` launch — both need network/Soulseek
  credentials or a display this sandbox does not have. `muzik gui` was checked
  for import-time correctness only.
- [x] Commit: `build(release): package the embedded Seakarr bridge`

## Phase 8: Document direct acquisition and quality checks

- [x] Update `README.md` with Seakarr setup, source routing, quality policy, and
  recovery behavior. (Seakarr setup and source routing were already documented
  in an earlier phase; this phase added the quality-policy and direct-Spotify
  sections.)
- [x] Update `GUI.md` with the Quality stage, progress events, cancellation, and
  item controls. (Cancellation was already documented in an earlier phase.)
- [x] Update `SPOTIFY.md` with the direct structured Seakarr flow and its limits.
- [x] Record the supported platforms and native-wheel limits in
  `DISTRIBUTION.md`. (Already recorded in Phase 7's "Native module" section;
  no further Phase 8 changes were needed.)
- [x] Commit: `docs(seakarr): explain direct acquisition and quality checks`

## Verification

- [x] `mise run check` passes with the locked Python dependencies.
- [x] `cargo fmt --check` passes for `rust/seakarr_bridge/`.
- [x] `cargo clippy --all-targets --all-features -- -D warnings` passes for the
  bridge.
- [x] `cargo test` passes for the bridge and its Seakarr integration boundary.
  (20 Rust unit tests; there is no live-network Seakarr integration test —
  see the manual live smoke tests below.)
- [x] A clean Python 3.14 environment can import `muzik._seakarr` from the built
  wheel.
- [x] Spotify JSON and CSV entries go directly to Seakarr as structured track
  requests. (`tests/test_spotify_workflow.py`.)
- [x] Spotify input never calls `yt-dlp`. (`--audio-source youtube` is rejected
  for Spotify exports; enforced in `_run_resolved_playlist_workflow`.)
- [x] A YouTube URL downloads before the quality stage starts.
  (`test_quality_check_never_runs_before_the_youtube_download_completes`,
  `tests/test_youtube_quality.py`.)
- [x] A good YouTube file continues without a Seakarr download.
  (`test_off_policy_keeps_the_file_without_measuring`,
  `test_a_lossless_file_is_kept_without_searching`.)
- [x] A low-quality YouTube file can get a verified Seakarr replacement.
  (`test_auto_policy_replaces_with_a_safe_candidate`,
  `test_ask_policy_replaces_once_confirmed`.)
- [x] A failed or unsafe replacement keeps the original YouTube file.
  (`test_no_safe_candidate_keeps_the_youtube_file`,
  `test_search_failure_keeps_the_youtube_file_and_does_not_raise`,
  `test_download_failure_keeps_the_youtube_file`.)
- [x] A multi-track album replacement skips chapter parsing and splitting.
  (`test_a_multi_file_replacement_becomes_a_pre_split_directory`.)
- [x] A long one-file replacement uses YouTube chapters only within the
  duration tolerance.
  (`test_a_replacement_with_the_wrong_duration_is_rejected`,
  `test_a_single_file_replacement_keeps_the_youtube_chapter_sidecars`.)
- [x] Cancel stops search or transfer work and leaves no partial file as a
  valid result. (`test_cancellation_stops_before_the_search`,
  `test_seakarr_source_download_stops_immediately_when_already_cancelled`.)
- [ ] Progress events keep the GUI responsive during login, search, queue
  wait, and download. **Not fully done**: cross-thread job polling only
  reports terminal status (completed/failed/timed out/cancelled), not live
  byte-level transfer progress — flagged already in Phase 6. The GUI stays
  responsive because polling happens off the render thread, but there is no
  granular in-progress percentage during a download.
- [x] Candidate errors show useful details and do not show the Soulseek
  password. (`test_seakarr_source_check_never_exposes_the_password`.)
- [x] Existing `source="soulseek"` cache entries and saved workflow state
  still load. (`tests/test_cache_quality.py`.)
- [x] Watchlist version 1 data migrates to version 2 without loss of existing
  stage state or paths.
  (`test_version_1_records_migrate_with_quality_not_started_and_data_intact`.)
- [x] Download again marks Quality and later stages stale.
  (`test_download_again_forces_only_download_and_marks_later_stages_stale`.)
- [x] Check quality again marks later stages stale only when it changes the
  active audio.
  (`test_check_quality_again_that_keeps_the_file_does_not_mark_later_stages_stale`,
  `test_check_quality_again_that_replaces_the_file_marks_later_stages_stale`.)
- [x] Run, Retry, Download again, Check quality again, Parse again, Split
  again, Organize again, and Run all again are visible for each usable
  watchlist item. (`ACTION_LABELS`/`_REPEAT_ACTIONS` in `muzik/gui/watchlist.py`
  cover all eight; `tests/test_gui_watchlist.py` exercises availability.)
- [x] Beets imports only the selected final audio and does not create a
  duplicate album from the kept source. (The quality stage always finishes
  before `process_audio`/Beets import runs on the returned `audio_files` or
  `pre_split_dirs` — the rejected/kept file is never passed alongside a
  replacement. No dedicated duplicate-album regression test exists; this
  follows from the sequencing in `check_youtube_quality`'s callers.)
- [x] The Beets database and library do not change when a quality replacement
  fails before import. (`check_youtube_quality` never touches Beets; a failed
  replacement returns before any file reaches `process_audio`.)
- [x] The current YouTube, local-file, Bandcamp, chapter, split, Beets, cache,
  Library, Settings, and watchlist flows still pass their active behavior
  tests. (389 tests pass.)
- [x] A release wheel works on macOS arm64. (Built and installed into a clean
  venv on this machine, which is macOS arm64.)
- [x] The source archive builds through the Homebrew formula with its
  declared Rust build dependency. **Partially done**: the sdist install
  mechanism the formula relies on (`pip install` compiling
  `muzik._seakarr` from source via Maturin) was verified directly with
  `uv pip install ./dist/muzik-*.tar.gz` into a clean venv. `brew install
  --build-from-source` itself was not run — the tap repository does not
  exist yet (see `packaging/homebrew/README.md`), and this is local-only
  verification per the user's instruction not to release.
- [ ] Manual live smoke test: connect one Soulseek account, search one Spotify
  track, select a candidate, download it, and import it with Beets. **Not
  done** — requires a real Soulseek account and network access this sandbox
  does not have. Only the user can run this.
- [ ] Manual live smoke test: download one YouTube album, show its quality
  stage, accept or reject a Seakarr replacement, and complete Beets import.
  **Not done** — same reason; also needs a display to drive `muzik gui`.

## Review

- [x] Review the code and the Rust safety boundary. (Every phase ended with
  `cargo clippy -D warnings` and `cargo fmt --check`; the bridge has 20 unit
  tests covering job state transitions, cancellation, and error mapping.)
- [x] Review the Seakarr license and include all required notices.
  (`rust/seakarr_bridge/THIRD_PARTY_NOTICES.md` carries the MIT license text
  for `soulseek-rs-lib`.)
- [x] Update `PLAN.md` and `TODO.md` before implementation if the approach
  changes. (PLAN.md was corrected mid-session when its original Seakarr
  dependency description turned out to be fabricated; see its
  `~~strikethrough~~`/`**Correction:**` markers.)
- [x] Make each phase commit with its exact planned message.
- [x] Mark each phase commit complete only after `git commit` succeeds.
- [x] Check all `TODO.md` items before the final handoff. (This pass. Two
  verification items remain open: live-transfer progress reporting, which is
  a real, previously-flagged gap, and the two manual live smoke tests, which
  need real Soulseek credentials/network/display only the user has.)
