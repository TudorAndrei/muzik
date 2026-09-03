# Plan: Replace slskd with embedded Seakarr

## Goal

Replace the slskd service and `slskd-api` client with an embedded Seakarr Rust
module. Spotify export tracks will go directly to Seakarr. YouTube videos will
download from YouTube first. Muzik will then check the downloaded audio quality
and can use Seakarr to get a better copy. The quality check will be a visible
workflow stage before chapter parsing, splitting, and Beets import.

## Approach

### Source routing

The input type will set the first audio source:

- A YouTube video or playlist will always download with `YouTubeSource` first.
- A Spotify JSON or CSV export will send each `ResolvedTrack` directly to
  Seakarr. It will never send a Spotify item to `yt-dlp`.
- A local path will use the local audio first.
- A plain artist, album, or track query will use Seakarr directly.

The current `AudioSource.AUTO` path in `muzik/core/workflow/service.py` searches
Soulseek before YouTube. This behavior will stop for YouTube input. The input
type will control acquisition. A separate quality policy will control a later
Seakarr search.

Spotify support will remain metadata-only. The existing parsers in
`muzik/core/sources/spotify.py` will stay. Direct Spotify OAuth and Spotify media
download are not part of this work.

### Quality stage

`muzik/core/quality.py` currently scores names and candidate metadata. It does
not measure a local audio file. The module will add an `ffprobe`-based check for
codec, bit rate, sample rate, bit depth, channel count, duration, and file size.
It will also add a policy result that says one of these things:

- Keep the current file.
- Ask for a Seakarr replacement.
- Replace the file automatically.
- Keep the current file because no safe replacement was found.

The new workflow order for YouTube will be:

```text
Download from YouTube
  -> Check quality
  -> Optionally download a Seakarr replacement
  -> Parse chapters
  -> Split when necessary
  -> Import with Beets
```

Muzik will keep the YouTube file until the Seakarr download passes audio and
identity checks. A failed quality search or download will not destroy the
YouTube file. The policy can let the workflow continue with the YouTube copy.

For a full-album video, a Seakarr result can contain many track files. Muzik will
treat that result as a pre-split album and skip chapter parsing and splitting.
If Seakarr returns one long file, Muzik will use the YouTube chapter data only
when the two durations are within a safe tolerance. Otherwise, it will keep the
YouTube file.

The quality stage will run before Beets. This prevents Beets from importing a
file that Muzik replaces in the same run.

### PyO3 bridge

A new Rust crate at `rust/seakarr_bridge/` will build the private Python module
`muzik._seakarr`. The crate will use PyO3 and a Tokio runtime. Maturin will build
the extension.

The bridge will have a small API:

```text
SeakarrSession.connect(settings)
SeakarrSession.start_track_search(request) -> SeakarrJob
SeakarrSession.start_album_search(request) -> SeakarrJob
SeakarrSession.start_download(candidate_id, destination) -> SeakarrJob
SeakarrJob.poll() -> status and progress events
SeakarrJob.cancel()
SeakarrJob.result() -> candidates or downloaded files
SeakarrSession.close()
```

The GUI already runs workflows on a worker thread in `muzik/gui/app.py`. The
Python adapter will poll `SeakarrJob` from that worker. It will emit the current
events from `muzik/core/workflow/events.py`. A cancel request will call both the
current `CancellationToken` and `SeakarrJob.cancel()`. DearPyGui calls will stay
on the render thread through `GuiBridge`.

The Rust boundary will return owned values only. It will not return Seakarr
database connections, Tokio channels, or internal Rust references to Python.
Python values will contain candidate IDs, peers, file names, sizes, measured
quality, queue state, transfer progress, and error details.

The bridge will use these public Seakarr parts where possible:

- `RealClient` and `SoulseekClient`
- `search_album_with_fallback()` and the public client search method
- `filter_results()` and `rank_candidates()`
- `download_file()` and `download_album()`
- `SearchResult`, `FileInfo`, `DownloadStatus`, and `DownloadHandle`

Some useful Seakarr helpers are only `pub(crate)`. An external PyO3 crate cannot
call them. The preferred fix is a small public integration module in Seakarr.
If that change is not accepted, Muzik will use a pinned fork with the same
module. Muzik will not copy private Seakarr logic into Python.

The Seakarr dependency will be pinned to a full commit hash. The first reviewed
target is `571819f1101bb99bce5c839cb4ffcdf967c94d0c`. An update will need explicit
review and bridge tests.

### Python source adapter

A new `muzik/core/sources/seakarr.py` module will map Rust values to the current
`Candidate`, `CandidateFile`, `QualityInfo`, and `DownloadResult` models in
`muzik/core/sources/base.py`. The adapter will keep `Candidate.source` equal to
`"soulseek"`. This keeps existing cache keys and saved workflow data valid.

The user command can remain `muzik soulseek` because Soulseek is the network.
Its implementation and messages will identify Seakarr as the client. The new
adapter will replace imports of `SoulseekSource` in
`muzik/core/workflow/operations.py`, `muzik/core/workflow/service.py`,
`muzik/commands/soulseek.py`, and `muzik/commands/workflow.py`.

`muzik/config.py` will replace `get_slskd_settings()` with Seakarr settings for
the Soulseek username, password, server, listen port, search limits, transfer
limits, and staging directory. Environment variables will override
`MUZIK_CONFIG_FILE`. Errors and status output must never show the password.

After the Seakarr adapter passes the same source contract, the project will
remove `slskd-api` from `pyproject.toml` and remove the slskd service from
`docker-compose.yml`. This is a replacement. Muzik will not support two active
Soulseek clients.

### Direct Spotify acquisition

`_run_resolved_playlist_workflow()` in `muzik/core/workflow/service.py` currently
joins artist, title, and album into one string. It then calls
`WorkflowRunOperations.acquire_soulseek`. The new operation will accept the
existing `ResolvedTrack` object. The Seakarr adapter can then use artist, title,
album, duration, track number, and ISRC as separate evidence.

Spotify playlist entries will use track search. Album search will be used only
when the input is an album request or when a later feature supplies a complete
album. This work will not download a complete album for each playlist track.

The adapter will reject a candidate when its duration is outside the configured
tolerance or its title and artist do not have enough identity evidence. The
normal interactive candidate decision in `WorkflowDecisions` will remain. A
non-interactive run will use the highest safe result.

### Watchlist and desktop interface

`STAGE_NAMES` in `muzik/core/watchlist.py` will become `download`, `quality`,
`parse`, `split`, and `organize`. The watchlist schema will move from version 1
to version 2. The loader will add a quality stage to version 1 records. It will
not change the other stage results.

`muzik/core/workflow/item_actions.py` will add a quality action. Each watchlist
item will keep these buttons:

- Run or Retry
- Download again
- Check quality again
- Parse again
- Split again
- Organize again
- Run all again

Download again will mark Quality, Parse, Split, and Organize as stale. A quality
check that keeps the same file will not mark later stages as stale. A quality
check that installs a replacement will mark Parse, Split, and Organize as stale.

`PipelineView` and `WatchlistView` will show measured quality, the decision, the
selected Seakarr candidate, transfer progress, and clear failure text. The
Settings page will check the embedded Seakarr session instead of slskd HTTP
state.

### Build and distribution

The current Hatch build creates a platform-neutral wheel. A PyO3 extension needs
a platform wheel. The build will move to a Maturin mixed Python and Rust layout.
The Python package will stay under `muzik/`.

`mise.toml` will add the Rust toolchain and Maturin commands. The check task will
run Python tests, Rust tests, formatting, lint checks, and a release-mode bridge
build. No Deno task will be added.

`.github/workflows/release.yml` will build the macOS arm64 wheel that the current
desktop release needs. It will also build the supported Linux wheel and the
source archive. The workflow will keep the version from the conventional commit
release step in sync with Cargo package metadata. The Homebrew formula will add
Rust as a build dependency when it builds from the source archive.

### Prerequisite and limits

Seakarr has no license file or declared package license at the reviewed commit.
Muzik is proprietary. Do not add, link, or distribute the Seakarr dependency
until its author adds a compatible license or gives written permission. The
license text and required notices must be in the release package.

This work does not use Seakarr to scan, organize, replace, or delete files in the
Beets library. Muzik remains the only owner of library changes. This work also
does not add scheduled quality scans, Spotify OAuth, Spotify playback, or an LLM
candidate selector.

## Implementation Phases

### Phase 1: Add measured audio quality decisions

- Extend `QualityInfo` in `muzik/core/sources/base.py` with the measured fields
  that the policy needs.
- Add local `ffprobe` measurement and explicit quality policy results in
  `muzik/core/quality.py`.
- Add quality options to `WorkflowOptions` and `WorkflowLaunchConfig` without
  changing source routing yet.
- Add focused tests in `tests/test_cache_quality.py`,
  `tests/test_workflow_sources.py`, and `tests/test_gui_launcher.py`.
- Keep the new policy inactive until the Seakarr adapter is ready.

**Commit:** `feat(quality): add measured audio quality decisions`

### Phase 2: Build the embedded Seakarr bridge

- Complete the license prerequisite before this phase starts.
- Add the PyO3 crate under `rust/seakarr_bridge/` and pin the reviewed Seakarr
  commit.
- Add the session, job, candidate, progress, cancellation, and structured error
  boundary.
- Use a public Seakarr integration module. Submit the small upstream change or
  record the pinned fork commit before the bridge depends on it.
- Add Rust unit tests with Seakarr's mock client. Cover track search, album
  search, ranking, download progress, cancellation, unsafe paths, and errors.
- Add a Python import smoke test for `muzik._seakarr`.

**Commit:** `feat(seakarr): add the embedded Soulseek bridge`

### Phase 3: Replace the slskd adapter

- Add `muzik/core/sources/seakarr.py` and map bridge results to the existing
  source-neutral models.
- Replace `SoulseekSource` use in workflow operations, workflow services, CLI
  commands, and service checks.
- Replace slskd URL and API-key settings with Seakarr and Soulseek settings in
  `muzik/config.py`, `.env.example`, `muzik/commands/config.py`, and
  `muzik/commands/init.py`.
- Keep the `muzik soulseek` command and existing `source="soulseek"` cache data.
- Remove `slskd-api` and the slskd Docker service after the bridge passes the
  current source tests.
- Replace slskd fixtures with bridge test data. Do not add tests that only prove
  that slskd code is absent.

**Commit:** `refactor(soulseek): replace slskd with embedded Seakarr`

### Phase 4: Route Spotify tracks directly to Seakarr

- Change `WorkflowRunOperations` so metadata-only acquisition can receive a
  `ResolvedTrack` instead of a flat text query.
- Pass each Spotify track from `_run_resolved_playlist_workflow()` directly to
  the Seakarr adapter.
- Use track search and identity checks. Do not request a complete album for one
  playlist track.
- Keep resume keys, duplicate occurrences, and playlist state behavior.
- Update `tests/test_spotify_workflow.py`, `tests/test_spotify_source.py`, and
  `tests/test_workflow_service.py` for structured direct acquisition,
  cancellation, no safe candidate, and resume behavior.

**Commit:** `feat(spotify): acquire tracks directly with Seakarr`

### Phase 5: Add the YouTube-first quality flow

- Change YouTube URL and playlist routing in
  `muzik/core/workflow/service.py` so YouTube download always happens before a
  Seakarr quality search.
- Add the quality operation to `WorkflowRunOperations` and
  `build_workflow_operations()`.
- Compare the measured YouTube file with safe Seakarr candidates.
- Keep the original file until a replacement passes audio, duration, and
  identity checks.
- Route a multi-file album replacement as a pre-split directory. Apply chapter
  data to a single-file replacement only when durations are compatible.
- Continue with the YouTube file when policy permits and Seakarr is unavailable
  or no safe result exists.
- Update workflow, YouTube, cancellation, and failure tests. Prove that Seakarr
  is not called before a YouTube download.

**Commit:** `feat(workflow): add YouTube-first quality upgrades`

### Phase 6: Show quality state and controls

- Add the version 1 to version 2 watchlist migration and the Quality stage in
  `muzik/core/watchlist.py`.
- Add Check quality again and the new stale-state rules in
  `muzik/core/workflow/item_actions.py`.
- Show five workflow stages, quality details, candidate details, and transfer
  progress in `muzik/gui/watchlist.py` and `muzik/gui/pipeline.py`.
- Update the launcher quality controls and the Seakarr service row in Settings.
- Update GUI, watchlist, item-action, event-adapter, and settings tests.
- Verify that all existing item buttons remain visible and usable.

**Commit:** `feat(gui): add quality and Seakarr workflow controls`

### Phase 7: Package the native integration

- Move the build from Hatch to a Maturin mixed project in `pyproject.toml`.
- Add Rust and Maturin tasks to `mise.toml` and update `uv.lock`.
- Update check and release workflows for Rust checks and platform wheels.
- Update the Homebrew formula and `DISTRIBUTION.md` for the Rust build.
- Build a wheel, install it in a clean Python 3.14 environment, import the Rust
  module, and run CLI and GUI smoke checks.

**Commit:** `build(release): package the embedded Seakarr bridge`

### Phase 8: Document direct acquisition and quality checks

- Update `README.md` with Seakarr setup, source routing, quality policy, and
  recovery behavior.
- Update `GUI.md` with the Quality stage, progress events, cancellation, and
  item controls.
- Update `SPOTIFY.md` with the direct structured Seakarr flow and its limits.
- Record the supported platforms and native-wheel limits in `DISTRIBUTION.md`.

**Commit:** `docs(seakarr): explain direct acquisition and quality checks`

## Risks & Tradeoffs

- Seakarr has no declared license. Implementation and distribution are blocked
  until this is resolved.
- A native module makes release files platform-specific. CI must build and test
  each supported wheel.
- Seakarr can change its Rust API. A full commit pin and bridge tests limit this
  risk.
- A Soulseek account must have only one active session. Starting Muzik can end
  another client session that uses the same account.
- Soulseek search results are not trusted. The Rust bridge must keep path checks,
  quality checks, duration checks, and staging isolation.
- A quality upgrade can find a wrong recording with a better codec. Identity and
  duration checks must have priority over codec quality.
- One Spotify track search can be slow for a large playlist. Resume state and
  cancellation must be saved after each track.
- A version 1 watchlist does not have quality history. Migration can add an
  unknown quality stage, but it cannot infer an old decision without a file
  check.
- Maturin replaces the current VCS version path. The release workflow must keep
  Cargo and Python versions equal.

## Open Questions

- Which license will the Seakarr author add, and is it compatible with Muzik's
  proprietary distribution?
- Will the required public integration module be accepted upstream, or must
  Muzik use a pinned fork?
- Should automatic quality replacement be the default, or should the first
  release only ask the user? The safe initial default is to check and ask.
- What is the minimum accepted quality for YouTube audio? The initial proposal
  is lossless when available, with a configurable minimum for lossy files.
- Which duration tolerance is safe for a track and for a full album? This must be
  set before automatic selection is enabled.
- Is macOS arm64 the only required desktop wheel for the first release, or must
  the first release also include macOS x86-64?
