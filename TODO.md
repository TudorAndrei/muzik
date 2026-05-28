# Soulseek Migration TODO

This checklist implements the architecture described in `PLAN.md`.

## 0. Groundwork

- [x] Decide the supported Soulseek backend for v1.
  - [x] Use `slskd` as the primary backend.
  - [x] Defer direct Soulseek protocol clients until later.
- [x] Decide the first user-facing workflow.
  - [x] Target: `muzik workflow "Artist - Album" --audio-source soulseek --prefer flac`.
  - [x] Keep existing YouTube workflow working during migration.
- [x] Add test fixtures directory if missing.
  - [x] `tests/fixtures/youtube/`
  - [x] `tests/fixtures/slskd/`
  - [x] `tests/fixtures/audio/`

## 1. Add Source Models

- [x] Create `muzik/core/sources/__init__.py`.
- [x] Create `muzik/core/sources/base.py`.
- [x] Add source-neutral dataclasses or Pydantic models:
  - [x] `DownloadRequest`
  - [x] `ResolvedTrack`
  - [x] `ResolvedRelease`
  - [x] `ResolvedPlaylist`
  - [x] `CandidateFile`
  - [x] `QualityInfo`
  - [x] `Candidate`
  - [x] `DownloadResult`
- [x] Add a source protocol/base class:
  - [x] `resolve(request)`
  - [x] `search(resolved)`
  - [x] `download(candidate, output)`
- [x] Add unit tests for model construction and serialization.

## 2. Extract Current YouTube Logic

- [x] Create `muzik/core/sources/youtube.py`.
- [x] Move YouTube helpers out of workflow/download commands:
  - [x] `_youtube_id`
  - [x] `_playlist_id`
  - [x] `_get_playlist_video_ids`
  - [x] YouTube filename/result discovery.
- [x] Move `yt-dlp` command construction out of `muzik/commands/download.py`.
- [x] Implement YouTube metadata resolution:
  - [x] `yt-dlp --dump-json` for single videos.
  - [x] `yt-dlp --flat-playlist` for playlists.
  - [x] Extract title, uploader/artist hints, description, chapters, upload date.
- [x] Implement YouTube fallback audio download using the existing `yt-dlp` flags.
- [x] Keep `muzik download <url>` behavior compatible.
- [x] Add tests using mocked `yt-dlp` output.

## 3. Add Source-Neutral Metadata Sidecars

- [x] Create `muzik/core/metadata.py`.
- [x] Define `.muzik.json` schema version `1`.
- [x] Add helpers:
  - [x] `metadata_sidecar_for(path)`
  - [x] `write_muzik_metadata(path_or_root, data)`
  - [x] `read_muzik_metadata(path_or_root)`
  - [x] `find_muzik_metadata(audio_path)`
- [x] Update `muzik/core/audio.py` metadata priority:
  - [x] `.muzik.json`
  - [x] `.info.json`
  - [x] Embedded tags via `ffprobe`
  - [x] Filename parsing fallback
- [x] Add tests for metadata precedence.

## 4. Add Source-Neutral Cache Keys

- [x] Add cache key helpers to `muzik/core/cache.py` or a new `muzik/core/downloads.py`.
- [x] Implement:
  - [x] `stable_hash(value)`
  - [x] `download_cache_key(source, source_id)`
  - [x] `workflow_cache_key(source, request)`
  - [x] `candidate_cache_key(candidate)`
- [x] Keep compatibility reads for old `yt_<id>` entries.
- [x] Write new downloads to `download_<source>_<hash>`.
- [x] Write workflow resume state to `workflow_<source>_<hash>.json`.
- [x] Add tests for deterministic keys and legacy YouTube fallback.
  - [x] Deterministic source-neutral keys.
  - [x] Legacy YouTube fallback.

## 5. Refactor Workflow Into Phases

- [x] Split `muzik/commands/workflow.py` into smaller internal helpers:
  - [x] Resolve phase.
  - [x] Acquire/download phase.
  - [x] Process phase.
- [x] Make process phase source-agnostic:
  - [x] Accept files/directories from `DownloadResult`.
  - [x] Detect folders of already split tracks.
  - [x] Detect single long files.
  - [x] Split only when chapters are found or accepted from MusicBrainz.
  - [x] Organize files/folders with beets.
- [x] Add workflow options:
  - [x] `--metadata-source youtube|musicbrainz|none|auto`
  - [x] `--audio-source soulseek|youtube|auto`
  - [x] `--prefer flac|lossless|mp3-320|any`
  - [x] `--fallback youtube|none`
  - [x] `--interactive/--no-interactive`
- [x] Preserve existing options:
  - [x] `--review`
  - [x] `--no-split`
  - [x] `--no-organize`
  - [x] `--import`
  - [x] `--tag-only`
  - [x] `--dry-run`
  - [x] `--jobs`
  - [x] `--config`
  - [x] `--keep-source`
  - [x] `--force`
- [x] Add tests around routing:
  - [x] YouTube URL + YouTube audio fallback.
  - [x] YouTube URL + Soulseek audio.
  - [x] Plain text query + Soulseek audio.
  - [x] Local folder direct organize.

## 6. Add Soulseek Configuration

- [x] Add config values in `muzik/config.py`:
  - [x] `SLSKD_URL`
  - [x] `SLSKD_API_KEY`
  - [x] `SLSKD_DOWNLOAD_DIR`
  - [x] `DEFAULT_SOULSEEK_DIR`
- [x] Read values from environment first.
- [x] Optionally add `$XDG_CONFIG_HOME/muzik/config.yaml` support.
- [x] Update `muzik commands config` if needed.
- [x] Add `muzik init` checks/instructions for `slskd`.
- [x] Add validation for missing `slskd` config.

## 7. Implement `slskd` API Client

- [x] Create `muzik/core/soulseek.py` or `muzik/core/sources/soulseek.py`.
- [x] Add HTTP client helpers:
  - [x] Base URL handling.
  - [x] API key auth.
  - [x] Timeout handling.
  - [x] Error mapping.
- [x] Implement health/session check.
- [x] Implement search request.
- [x] Implement search result polling.
- [x] Implement download enqueue for:
  - [x] Single file.
  - [x] Folder/all files from candidate.
- [x] Implement transfer polling until:
  - [x] Complete.
  - [x] Failed.
  - [x] Timed out.
  - [x] Queued too long.
- [x] Add mocked API tests using fixture JSON.
  - [x] Mocked Soulseek API response coverage.
  - [x] JSON fixture-file coverage.

## 8. Implement Quality Detection And Ranking

- [x] Create `muzik/core/quality.py`.
- [x] Add extension-based format detection:
  - [x] `flac`
  - [x] `alac`
  - [x] `wav`
  - [x] `aiff`
  - [x] `ape`
  - [x] `wv`
  - [x] `mp3`
  - [x] `m4a`
  - [x] `opus`
- [x] Add lossless/lossy classification.
- [x] Add bitrate parsing from Soulseek/slskd result metadata when available.
- [x] Add optional `ffprobe` verification after download.
- [x] Add candidate scoring:
  - [x] Lossless preference.
  - [x] Preferred format match.
  - [x] Album folder completeness.
  - [x] Track count match.
  - [x] Track numbering.
  - [x] Artist/title/album filename similarity.
  - [x] Peer availability.
  - [x] Queue length or transfer availability.
  - [x] Penalties for partials, duplicates, tiny files, and unrelated folders.
- [x] Add tests for ranking order.

## 9. Add Soulseek Commands

- [x] Create `muzik/commands/soulseek.py`.
- [x] Register command group in `muzik/app.py`.
- [x] Add `muzik soulseek check`.
  - [x] Verify `slskd` is reachable.
  - [x] Verify auth works.
  - [x] Print configured download directory.
- [x] Add `muzik soulseek search <query>`.
  - [x] Print ranked candidate table.
  - [x] Include format, lossless, bitrate, size, user, path, file count, score.
- [x] Add `muzik soulseek download <query>`.
  - [x] Search and rank.
  - [x] Prompt for candidate selection unless `--no-interactive`.
  - [x] Cache displayed candidate IDs for later download.
  - [x] Support `muzik soulseek download --candidate <id>`.
  - [x] Download selected candidate.
  - [x] Write `.muzik.json`.
  - [x] Optionally organize.
- [x] Add `--prefer`, `--limit`, `--output`, `--no-organize`, and `--dry-run`.
  - [x] `--prefer`
  - [x] `--limit`
  - [x] `--output`
  - [x] `--no-organize`
  - [x] `--dry-run`

## 10. Integrate Soulseek Into Workflow

- [x] Wire `--audio-source soulseek` to the Soulseek source.
- [x] For plain text queries:
  - [x] Search Soulseek directly.
  - [x] Prefer album folders for album-looking queries.
- [x] For YouTube URLs:
  - [x] Resolve metadata with YouTube source.
  - [x] Build Soulseek search query from title/artist/album/track list.
  - [x] Download from Soulseek.
- [x] For YouTube playlists:
  - [x] Resolve playlist entries using `yt-dlp`.
  - [x] Search per release or per track.
  - [x] Resume after interruptions.
- [x] Add fallback behavior:
  - [x] If Soulseek finds no acceptable candidate and `--fallback youtube`, use YouTube audio download.
  - [x] If fallback is `none`, fail with a useful message.
- [x] Add tests for fallback behavior.

## 11. Process Downloaded Soulseek Files

- [x] Normalize completed download roots.
- [x] Identify audio files recursively.
- [x] Ignore non-audio files by default:
  - [x] Images.
  - [x] Logs.
  - [x] Playlists initially.
- [x] Parse `.cue` sidecars as chapter metadata for single-file albums.
- [x] Detect folder-of-tracks and organize directly.
- [x] Detect single-file album and pass through chapter/MusicBrainz flow.
- [x] Detect single-track file and organize directly.
- [x] Add optional handling for `.cue` files later.
- [x] Validate downloaded files before organizing:
  - [x] File exists.
  - [x] Extension is supported.
  - [x] `ffprobe` succeeds.
  - [x] Duration is plausible.

## 12. Update Validation

- [x] Update `muzik/commands/validate.py` to understand `.muzik.json`.
- [x] Report source, source ID, preferred format, detected format, and lossless/lossy status.
- [x] Warn when:
  - [x] Requested lossless but downloaded lossy.
  - [x] Metadata sidecar is missing.
  - [x] Album folder appears incomplete.
  - [x] Files are corrupt or unprobeable.

## 13. Update Documentation

- [x] Update `README.md`.
- [x] Document `slskd` requirement.
- [x] Document environment variables:
  - [x] `SLSKD_URL`
  - [x] `SLSKD_API_KEY`
  - [x] `SLSKD_DOWNLOAD_DIR`
- [x] Document new commands.
- [x] Document examples:
  - [x] Search only.
  - [x] Download FLAC album.
  - [x] Use YouTube metadata with Soulseek audio.
  - [x] Use YouTube fallback.
- [x] Clarify that users are responsible for only downloading music they are authorized to access.

## 14. Regression Checks

Live end-to-end versions of these checks still require network access, external
credentials, or representative user media/services.

- [x] Existing `muzik download <youtube-url>` still works.
- [x] Existing `muzik workflow <youtube-url>` still works.
- [x] Existing split flow still works.
- [x] Existing archive flow still works.
- [x] Existing Bandcamp flow still works.
- [x] Existing cache command still works.
- [x] Existing validation command still works.
- [x] Run formatter/linter.
- [x] Run test suite.

## 15. First Milestone Acceptance Criteria

Live acceptance requires a reachable `slskd` instance plus `SLSKD_API_KEY`
configured in the environment or `$XDG_CONFIG_HOME/muzik/config.yaml`.
Local check on 2026-05-28: slskd API auth works, but Docker logs show
`INVALIDPASS` after trying `soulseek.username: slskd` and
`soulseek.password: slskd`; configure valid Soulseek network credentials in
slskd, then restart it.

- [x] `muzik soulseek check` confirms `slskd` connectivity.
- [x] `muzik soulseek search "Artist - Album flac"` returns ranked candidates.
- [ ] `muzik soulseek download "Artist - Album" --prefer flac --no-organize` downloads a selected candidate.
- [ ] Download writes `.muzik.json`.
- [ ] Downloaded files pass `ffprobe`.
- [ ] `muzik workflow "Artist - Album" --audio-source soulseek --prefer flac --no-organize` downloads from Soulseek.
- [ ] `muzik workflow "Artist - Album" --audio-source soulseek --prefer flac` organizes with beets.
- [x] YouTube audio download is not used unless explicitly requested or configured as fallback.
