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

- [ ] Add `muzik/core/sources/seakarr.py` and map Rust data to `Candidate`,
  `CandidateFile`, `QualityInfo`, and `DownloadResult`.
- [ ] Replace `SoulseekSource` use in workflow operations, workflow services,
  CLI commands, and service checks.
- [ ] Add Seakarr and Soulseek settings to `muzik/config.py` and
  `.env.example`.
- [ ] Update `muzik config`, `muzik init`, and Settings status text.
- [ ] Keep `muzik soulseek` and `source="soulseek"` data compatible.
- [ ] Remove `slskd-api` from `pyproject.toml`.
- [ ] Remove the slskd service from `docker-compose.yml`.
- [ ] Replace slskd fixtures with bridge data used by active behavior tests.
- [ ] Test readiness, login errors, search, selection, download, progress,
  cancellation, cache compatibility, and secret redaction.
- [ ] Run `mise run check` and the Rust checks.
- [ ] Commit: `refactor(soulseek): replace slskd with embedded Seakarr`

## Phase 4: Route Spotify tracks directly to Seakarr

- [ ] Change metadata-only acquisition in `WorkflowRunOperations` to accept a
  `ResolvedTrack`.
- [ ] Pass each Spotify entry directly from
  `_run_resolved_playlist_workflow()` to the Seakarr adapter.
- [ ] Use artist, title, album, duration, track number, and ISRC as separate
  identity evidence.
- [ ] Use track search for playlist tracks. Do not get a complete album for one
  playlist track.
- [ ] Keep playlist state, duplicate occurrence, and resume behavior.
- [ ] Test JSON and CSV input, direct structured requests, no YouTube call,
  unsafe matches, cancellation, duplicate tracks, and resume behavior.
- [ ] Run `mise run check` and the Rust checks.
- [ ] Commit: `feat(spotify): acquire tracks directly with Seakarr`

## Phase 5: Add the YouTube-first quality flow

- [ ] Make YouTube video and playlist input download with `YouTubeSource` before
  any Seakarr search.
- [ ] Add the quality operation to `WorkflowRunOperations` and
  `build_workflow_operations()`.
- [ ] Measure the YouTube download and apply the configured quality policy.
- [ ] Search Seakarr only when the policy requests a better copy.
- [ ] Keep the YouTube file until the replacement passes audio, duration, and
  identity checks.
- [ ] Use a safe multi-file replacement as pre-split input.
- [ ] Use YouTube chapters with a one-file replacement only when durations are
  compatible.
- [ ] Continue with the YouTube file when the policy permits and replacement is
  not possible.
- [ ] Test single videos, playlists, full albums, no chapters, no Seakarr result,
  bad replacement audio, wrong duration, cancellation, and recovery.
- [ ] Prove that no Seakarr call occurs before the YouTube download.
- [ ] Run `mise run check` and the Rust checks.
- [ ] Commit: `feat(workflow): add YouTube-first quality upgrades`

## Phase 6: Show quality state and controls

- [ ] Move the watchlist schema from version 1 to version 2.
- [ ] Migrate version 1 records without changing their existing stage results.
- [ ] Add Quality between Download and Parse in `STAGE_NAMES`.
- [ ] Add Check quality again to `ItemAction` and keep all current actions.
- [ ] Mark later stages stale only when the active audio file changes.
- [ ] Show measured quality, quality decision, selected candidate, and transfer
  progress in the pipeline and watchlist views.
- [ ] Add quality controls to the launcher and a Seakarr row to Settings.
- [ ] Test schema migration, stage order, action availability, stale rules,
  errors, progress, cancellation, and all visible action buttons.
- [ ] Run a DearPyGui render-context smoke test for a pending quality check, an
  active transfer, a kept YouTube file, and a completed replacement.
- [ ] Run `mise run check` and the Rust checks.
- [ ] Commit: `feat(gui): add quality and Seakarr workflow controls`

## Phase 7: Package the native integration

- [ ] Change `pyproject.toml` to a Maturin mixed Python and Rust build.
- [ ] Add Rust and Maturin tools and checks to `mise.toml`.
- [ ] Update `uv.lock`.
- [ ] Update `.github/workflows/check.yml` for the Rust build and tests.
- [ ] Update `.github/workflows/release.yml` for macOS arm64 and supported Linux
  platform wheels, a source archive, and one shared version.
- [ ] Update `packaging/homebrew/muzik.rb` and `DISTRIBUTION.md` for the Rust build
  dependency and native module.
- [ ] Build the release wheel and source archive.
- [ ] Install the wheel in a clean Python 3.14 environment.
- [ ] Verify `muzik --help`, `muzik soulseek check`, a direct Spotify track run,
  a YouTube-first quality run, and `muzik gui`.
- [ ] Commit: `build(release): package the embedded Seakarr bridge`

## Phase 8: Document direct acquisition and quality checks

- [ ] Update `README.md` with Seakarr setup, source routing, quality policy, and
  recovery behavior.
- [ ] Update `GUI.md` with the Quality stage, progress events, cancellation, and
  item controls.
- [ ] Update `SPOTIFY.md` with the direct structured Seakarr flow and its limits.
- [ ] Record the supported platforms and native-wheel limits in
  `DISTRIBUTION.md`.
- [ ] Commit: `docs(seakarr): explain direct acquisition and quality checks`

## Verification

- [ ] `mise run check` passes with the locked Python dependencies.
- [ ] `cargo fmt --check` passes for `rust/seakarr_bridge/`.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes for the
  bridge.
- [ ] `cargo test` passes for the bridge and its Seakarr integration boundary.
- [ ] A clean Python 3.14 environment can import `muzik._seakarr` from the built
  wheel.
- [ ] Spotify JSON and CSV entries go directly to Seakarr as structured track
  requests.
- [ ] Spotify input never calls `yt-dlp`.
- [ ] A YouTube URL downloads before the quality stage starts.
- [ ] A good YouTube file continues without a Seakarr download.
- [ ] A low-quality YouTube file can get a verified Seakarr replacement.
- [ ] A failed or unsafe replacement keeps the original YouTube file.
- [ ] A multi-track album replacement skips chapter parsing and splitting.
- [ ] A long one-file replacement uses YouTube chapters only within the duration
  tolerance.
- [ ] Cancel stops search or transfer work and leaves no partial file as a valid
  result.
- [ ] Progress events keep the GUI responsive during login, search, queue wait,
  and download.
- [ ] Candidate errors show useful details and do not show the Soulseek password.
- [ ] Existing `source="soulseek"` cache entries and saved workflow state still
  load.
- [ ] Watchlist version 1 data migrates to version 2 without loss of existing
  stage state or paths.
- [ ] Download again marks Quality and later stages stale.
- [ ] Check quality again marks later stages stale only when it changes the
  active audio.
- [ ] Run, Retry, Download again, Check quality again, Parse again, Split again,
  Organize again, and Run all again are visible for each usable watchlist item.
- [ ] Beets imports only the selected final audio and does not create a duplicate
  album from the kept source.
- [ ] The Beets database and library do not change when a quality replacement
  fails before import.
- [ ] The current YouTube, local-file, Bandcamp, chapter, split, Beets, cache,
  Library, Settings, and watchlist flows still pass their active behavior tests.
- [ ] A release wheel works on macOS arm64.
- [ ] The source archive builds through the Homebrew formula with its declared
  Rust build dependency.
- [ ] Manual live smoke test: connect one Soulseek account, search one Spotify
  track, select a candidate, download it, and import it with Beets.
- [ ] Manual live smoke test: download one YouTube album, show its quality stage,
  accept or reject a Seakarr replacement, and complete Beets import.

## Review

- [ ] Review the code and the Rust safety boundary.
- [ ] Review the Seakarr license and include all required notices.
- [ ] Update `PLAN.md` and `TODO.md` before implementation if the approach
  changes.
- [ ] Make each phase commit with its exact planned message.
- [ ] Mark each phase commit complete only after `git commit` succeeds.
- [ ] Check all `TODO.md` items before the final handoff.
