# Soulseek Migration Plan

## Goal

Move `muzik` from a YouTube-first downloader to a source-agnostic music acquisition pipeline where:

- Soulseek is the preferred source for audio files, especially FLAC and other high-quality formats.
- `yt-dlp` is retained mainly as a parser/resolver for YouTube albums, mixes, playlists, chapters, descriptions, and track listings.
- The existing split and organize stages continue to work for downloaded audio regardless of source.

The intended end state is:

```text
query / url / playlist
  -> resolve desired tracks or release metadata
  -> search Soulseek for high-quality candidates
  -> download selected files/folders
  -> verify audio and metadata
  -> split only when needed
  -> organize with beets
```

## Current Architecture

Today the pipeline is tightly coupled to YouTube:

- `muzik download <url>` wraps `yt-dlp` directly.
- `muzik workflow <url>` assumes YouTube URLs, YouTube video IDs, YouTube playlist IDs, and `yt-dlp` archive files.
- Download cache keys use `yt_<id>`.
- Downloaded filenames embed the YouTube ID with `%(title)s [%(id)s].%(ext)s`.
- Chapter detection depends on `.chapters.txt` or `yt-dlp` `.info.json`.
- Metadata extraction prefers `yt-dlp` `.info.json`, then embedded tags.

The reusable parts are:

- Audio probing and duration checks.
- Chapter parsing from `.chapters.txt`.
- MusicBrainz lookup.
- Splitting with `ffmpeg`.
- Organizing/importing with beets.
- Validation and cache primitives.

## Target Architecture

Introduce a source/resolver boundary:

```text
muzik/core/sources/
  base.py
  youtube.py
  soulseek.py

muzik/core/resolution.py
muzik/core/quality.py
muzik/core/downloads.py
```

### Source Interface

Each source should expose a normalized interface:

```python
class Source:
    name: str

    def resolve(self, request: DownloadRequest) -> ResolvedRelease | ResolvedPlaylist:
        ...

    def search(self, resolved: ResolvedRelease) -> list[Candidate]:
        ...

    def download(self, candidate: Candidate, output: Path) -> DownloadResult:
        ...
```

Suggested models:

```python
DownloadRequest(
    raw: str,
    source: Literal["auto", "youtube", "soulseek"],
    prefer_format: str | None,
    album: bool | None,
)

ResolvedRelease(
    title: str,
    artist: str | None,
    album: str | None,
    year: str | None,
    tracks: list[ResolvedTrack],
    source_metadata: dict,
)

Candidate(
    source: str,
    source_id: str,
    title: str,
    user: str | None,
    path: str | None,
    files: list[CandidateFile],
    quality: QualityInfo,
    score: float,
)

DownloadResult(
    source: str,
    source_id: str,
    files: list[Path],
    root: Path,
    metadata_path: Path,
)
```

## Role Of `yt-dlp`

`yt-dlp` should stay, but its responsibility changes.

Keep `yt-dlp` for:

- Expanding YouTube playlists into ordered video IDs.
- Reading YouTube titles, descriptions, chapters, upload metadata, and playlist metadata.
- Extracting timestamped track lists from descriptions.
- Providing fallback audio downloads when Soulseek does not find acceptable candidates.

Stop treating `yt-dlp` as:

- The default high-quality audio source.
- The owner of cache identity.
- The only producer of metadata sidecars.
- The assumption behind workflow inputs.

In practice:

```text
YouTube URL
  -> yt-dlp --dump-json / --flat-playlist
  -> resolved track/release metadata
  -> Soulseek search query
  -> Soulseek download
  -> beets organize
```

Only if Soulseek fails or the user asks for `--source youtube` should `muzik` download audio through `yt-dlp`.

## Soulseek Backend

Use `slskd` as the first backend.

Reasoning:

- It handles Soulseek login, sessions, peer connections, transfers, queues, retries, and download directories.
- `muzik` can interact with it through an API instead of owning the Soulseek protocol.
- It can run as a background service or container.

Configuration needed:

```text
SLSKD_URL=http://localhost:5030
SLSKD_API_KEY=...
SLSKD_DOWNLOAD_DIR=...
```

Add config support under `$XDG_CONFIG_HOME/muzik/config.yaml` or environment variables first, then formalize later.

## CLI Shape

Add explicit Soulseek commands:

```bash
muzik soulseek search "Artist - Album flac"
muzik soulseek download "Artist - Album" --format flac
muzik soulseek download --candidate <id>
```

Update workflow:

```bash
muzik workflow "Artist - Album" --source soulseek
muzik workflow "https://youtube.com/playlist?list=..." --audio-source soulseek
muzik workflow "https://youtube.com/watch?v=..." --metadata-source youtube --audio-source soulseek
muzik workflow "Artist - Track" --source auto --prefer flac
```

Recommended option split:

- `--metadata-source youtube|musicbrainz|none|auto`
- `--audio-source soulseek|youtube|auto`
- `--prefer flac|lossless|mp3-320|any`
- `--interactive/--no-interactive`
- `--fallback youtube|none`

## Quality Ranking

Soulseek result selection needs quality scoring. Do not download the first result blindly.

Score candidates using:

- Format: `flac`, `alac`, `wav`, `aiff`, `ape`, `wv`, `mp3`, `m4a`, `opus`.
- Lossless preference: FLAC/ALAC/WAV above MP3.
- Bitrate for lossy files: prefer 320 kbps, reject very low bitrate by default.
- Folder completeness: album folders with expected track count rank higher.
- Track numbering: files named `01`, `02`, etc. rank higher.
- Filename similarity to requested artist/album/track.
- Embedded tag quality when available.
- Peer availability, queue length, transfer speed, and failed transfer history.
- Avoid obviously bad folders: partial downloads, duplicates, random compilations, very small files.

Default policy:

```text
prefer lossless
accept mp3 320 if no lossless result is available
ask before downloading lower quality
```

## Metadata Sidecars

Introduce a source-neutral sidecar:

```text
<download-root>/.muzik.json
```

For single files:

```text
Artist - Title.flac
Artist - Title.muzik.json
```

Suggested structure:

```json
{
  "version": 1,
  "source": "soulseek",
  "source_id": "user:/path/to/file-or-folder",
  "requested": "Artist - Album",
  "resolved": {
    "artist": "Artist",
    "album": "Album",
    "title": null,
    "year": "1999",
    "tracks": []
  },
  "candidate": {
    "user": "peer-name",
    "path": "/Music/Artist/Album",
    "quality": {
      "format": "flac",
      "lossless": true,
      "bitrate": null,
      "sample_rate": 44100
    }
  },
  "downloaded_at": "2026-05-28T00:00:00"
}
```

Update metadata extraction to prefer:

1. `.muzik.json`
2. `.info.json`
3. Embedded tags
4. Filename parsing

## Cache Migration

Replace YouTube-only cache keys with source-neutral keys.

Current:

```text
yt_<youtube_id>
playlist_<youtube_playlist_id>
ytdlp_archive_<playlist_id>.txt
```

Target:

```text
download_<source>_<stable_hash>
workflow_<source>_<stable_hash>.json
source_youtube_<id>
source_soulseek_<candidate_hash>
```

Keep backward compatibility:

- Continue reading existing `yt_<id>` cache entries.
- Write new cache entries using the source-neutral key.
- Do not delete existing cache files automatically.

## Workflow Changes

Refactor `workflow_cmd` into three phases:

### 1. Resolve

Input may be:

- YouTube video URL
- YouTube playlist URL
- Search text like `Artist - Album`
- Existing local file or directory

Resolution returns a normalized desired release/track list.

For YouTube playlist URLs:

- Use `yt-dlp --flat-playlist` to enumerate entries.
- Use `yt-dlp --dump-json` when richer title/description/chapter data is needed.
- Build Soulseek search queries from each item.

### 2. Acquire

Download audio from selected audio source.

For Soulseek:

- Search candidates.
- Rank by quality.
- Ask the user to choose when ambiguous.
- Download folder or files through `slskd`.
- Wait until transfers complete.
- Return local paths.

For YouTube fallback:

- Use current `yt-dlp` behavior through `sources/youtube.py`.

### 3. Process

Process local audio paths.

- If input is a folder of tracks, organize directly.
- If input is a single long file with chapters, split then organize.
- If input is a single long file without chapters, try MusicBrainz lookup.
- If input is a normal single track, organize directly.

This phase should not know whether the source was Soulseek or YouTube.

## Implementation Phases

### Phase 1: Extract Current YouTube Source

- Create `muzik/core/sources/base.py`.
- Move `yt-dlp` command construction into `muzik/core/sources/youtube.py`.
- Return `DownloadResult` instead of relying on directory diffs in workflow.
- Keep CLI behavior unchanged.
- Add tests around YouTube ID parsing, result discovery, and cache compatibility.

### Phase 2: Source-Neutral Workflow

- Refactor workflow to call a selected source provider.
- Rename internal variables from `yt_id`, `video_ids`, and `playlist_id` where they are now generic.
- Add source-neutral cache helpers.
- Keep old YouTube cache reads as compatibility fallback.
- Add `.muzik.json` metadata reading.

### Phase 3: Soulseek Search MVP

- Add `slskd` config.
- Add API client module.
- Implement search command.
- Display ranked candidates in a table.
- Include quality fields: extension, lossless, bitrate, file count, size, peer/user, queue state if available.

### Phase 4: Soulseek Download MVP

- Implement folder/file download through `slskd`.
- Poll transfers until complete, failed, or timed out.
- Write `.muzik.json`.
- Add `muzik soulseek download`.
- Add `muzik workflow --audio-source soulseek`.

### Phase 5: Album/Playlist Migration

- Use YouTube playlist/album parsing to create desired track lists.
- Search Soulseek per album or per track.
- Prefer complete album folders over individual track downloads.
- Fall back to per-track downloads when no complete folder is found.
- Add resume state for multi-track/multi-release workflows.

### Phase 6: Quality Verification

- Probe downloaded files with `ffprobe`.
- Reject files that are corrupt, too short, wrong extension, or below requested quality.
- Detect duplicate tracks.
- Detect incomplete albums.
- Ask user before accepting lower-quality fallbacks.

### Phase 7: Polish And Defaults

- Make `--audio-source soulseek` the default if configured.
- Keep `--fallback youtube` available.
- Update README and init flow.
- Add validation output that shows source and quality.
- Add tests for source selection, ranking, cache keys, and sidecar metadata.

## Testing Strategy

Unit tests:

- Source request parsing.
- Cache key generation.
- Quality ranking.
- Metadata sidecar read/write.
- YouTube metadata parsing.
- Soulseek API response parsing using fixtures.

Integration tests:

- Mock `slskd` API for search/download lifecycle.
- Mock `yt-dlp` JSON output for playlist resolution.
- Verify workflow routes folders directly to organize.
- Verify single long files still use chapter detection/splitting.

Manual tests:

```bash
muzik soulseek search "Artist - Album flac"
muzik soulseek download "Artist - Album" --prefer flac --no-organize
muzik workflow "Artist - Album" --audio-source soulseek --no-organize
muzik workflow "https://youtube.com/playlist?list=..." --audio-source soulseek --no-organize
```

## Risks

- Soulseek results are inconsistent and require careful ranking.
- Transfers may queue indefinitely or fail mid-download.
- Folder structure is not standardized.
- Metadata may be wrong or missing.
- Some albums may only appear as lossy files.
- Legal availability depends on what the user is authorized to download.

Mitigations:

- Keep an interactive candidate picker.
- Keep YouTube fallback explicit.
- Validate downloaded files before organizing.
- Let beets remain the final metadata authority.
- Store source sidecars for auditability and resume behavior.

## First Milestone

The first useful milestone is:

```bash
muzik workflow "Artist - Album" --audio-source soulseek --prefer flac
```

It should:

1. Search Soulseek.
2. Show ranked album-folder candidates.
3. Download the selected candidate through `slskd`.
4. Write `.muzik.json`.
5. Probe downloaded audio.
6. Organize the folder with beets.

No YouTube audio download should be involved unless the user explicitly requests fallback.
