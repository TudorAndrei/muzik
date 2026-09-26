# muzik

![muzik](assets/muzik-logo-v2.png)

Music organizer CLI — download, split, and organize music from Soulseek, YouTube,
and Bandcamp.

---

Wraps an embedded Soulseek client, **yt-dlp**, **ffmpeg**, and **beets** with
better progress feedback and an interactive chapter editor. Soulseek is used
for higher-quality audio acquisition when configured; yt-dlp remains
available for YouTube metadata, playlist parsing, and fallback audio
downloads. Chapter sidecars can come from `.chapters.txt`, yt-dlp
`.info.json`, or album `.cue` sheets. Also downloads your full Bandcamp
collection.

## Requirements

- Python 3.14+
- [`uv`](https://github.com/astral-sh/uv)
- `yt-dlp`, `ffmpeg`, `ffprobe` on `$PATH`
- Optional for Soulseek: a Soulseek account (username/password) — the client
  is embedded, no separate server to run

Check external tools before running a full workflow:

```sh
yt-dlp --version
ffmpeg -version
ffprobe -version
uv run muzik soulseek check      # when using Soulseek
uv run playwright install chromium
```

Bandcamp collection downloads use Playwright browser automation. The first
Bandcamp run opens a browser so you can log in, then stores cookies under the
app data directory.

### macOS arm64 prerequisites

Install the required command-line tools and confirm that they are on `PATH`:

```sh
brew install ffmpeg yt-dlp
ffmpeg -version
ffprobe -version
yt-dlp --version
```

Before you use Bandcamp, install the Playwright Chromium browser once from a
source checkout:

```sh
uv run playwright install chromium
```

## Soulseek setup

Soulseek support is an embedded Rust client (`rust/seakarr_bridge/`) — there
is no separate server process to run. Set your Soulseek account credentials
as environment variables:

```sh
export MUZIK_SOULSEEK_USERNAME="your-soulseek-username"
export MUZIK_SOULSEEK_PASSWORD="your-soulseek-password"
```

Or write them to `muzik`'s own config file (see `muzik config set-soulseek
--help`). `MUZIK_SOULSEEK_DOWNLOAD_DIR` controls where completed downloads
are written (defaults to the platform data directory).

Then run `muzik soulseek check`; it should report `Soulseek reachable`.

## Quality policy

`--quality-policy` decides what happens after a YouTube download finishes:

| Value | Behavior |
|-------|----------|
| `off` (default) | Never measure or replace the downloaded file. |
| `ask` | Measure the file; if it is lossy and below `--min-bitrate`, search Soulseek and ask before replacing it. |
| `auto` | Same measurement, but replace automatically when a safe match is found — no prompt. |

`--min-bitrate` (default `256`) sets the lossy-bitrate floor: a lossy file at
or above it is kept as-is. A lossless file is always kept.

A quality check never turns a workflow into a failure: a Soulseek search or
download error, no safe candidate, or a duration mismatch all keep the
YouTube file and log a warning instead of raising. "Safe" means the candidate
passes the same identity and duration checks as any other Soulseek search
result (see [SPOTIFY.md](SPOTIFY.md)). A multi-file replacement (a Soulseek
result with more than one track) is treated as a pre-split album and skips
chapter parsing for that item.

```sh
uv run muzik workflow "https://youtube.com/watch?v=..." --quality-policy ask
uv run muzik workflow "https://youtube.com/watch?v=..." --quality-policy auto --min-bitrate 192
```

In `muzik gui`, the launcher exposes the same **Quality policy** and **Min
bitrate** fields. With `ask`, the desktop app asks you before it replaces a
YouTube file.

## Direct Spotify acquisition

A Spotify JSON or CSV entry is a resolved track, not a search query, so it
skips YouTube entirely: `acquire_track_from_soulseek` searches Soulseek by
track title/artist/duration and downloads the match directly. There is no
YouTube fallback for a Spotify-derived track — if no safe Soulseek candidate
is found, the run stops at that track (exit code 0) rather than skipping it;
already-organized tracks before it are not re-downloaded on the next run. See
[SPOTIFY.md](SPOTIFY.md) for the full source-routing policy.

## Install

Install the GitHub release wheel as an isolated command-line tool:

```sh
uv tool install \
  https://github.com/TudorAndrei/muzik/releases/download/v0.2.0/muzik-0.2.0-py3-none-any.whl
muzik --help
```

Or install with Homebrew, which also pulls `ffmpeg` and `yt-dlp`:

```sh
brew install TudorAndrei/muzik/muzik
```

### macOS app in Applications

After installing, add a desktop entry so the interface opens from Launchpad,
Spotlight, or Finder:

```sh
muzik install-app          # writes to /Applications, falls back to ~/Applications
muzik install-app --user   # force ~/Applications
```

The bundle launches `muzik gui` from the current install. Re-run the command
after reinstalling muzik to point the entry at the new location.

For development, install from a source checkout:

```sh
git clone <repo>
cd muzik
uv sync
uv run playwright install chromium
uv run muzik init
```

## Development

Run the complete locked local and CI verification gate with:

```sh
mise run check
```

The same checks run in CI and as individual pre-push hooks.

## Workflow source policy

`--audio-source` chooses where audio comes from: `youtube`, `soulseek`, or
`auto`. `auto` uses Soulseek when the configured service is ready and otherwise
uses YouTube for YouTube inputs. `--fallback youtube` is available only when a
YouTube input has no acceptable Soulseek result. `--metadata-source` controls
chapter/metadata lookup for downloaded audio: `youtube`, `musicbrainz`, `none`,
or `auto`.

Local audio paths are processed without remote acquisition. Spotify export files
are detected before local audio discovery and are the exception to the fallback
rule: they are metadata-only and require Soulseek or a ready `auto` source.

## Commands

| Command | Description |
|---------|-------------|
| `muzik init` | Create app directories and configure beets |
| `muzik workflow <url-or-path>` | Full pipeline: acquire → split → organize |
| `muzik download <url>` | Download audio from YouTube via yt-dlp |
| `muzik downloaded` | List audio already in the output folder |
| `muzik soulseek check` | Verify the embedded Soulseek client can connect and log in |
| `muzik soulseek search <query>` | Search Soulseek and rank candidates |
| `muzik soulseek download <query>` | Search Soulseek and enqueue a selected download |
| `muzik soulseek check-library` | Measure real quality across the Beets library and suggest Soulseek replacements |
| `muzik spotify set-client-id <id>` | Save the client ID of your own Spotify application |
| `muzik spotify login` \| `logout` \| `status` | Connect, disconnect, and check the Spotify account |
| `muzik spotify playlists` | List Liked Songs and your Spotify playlists |
| `muzik spotify export <ref>` | Write one Spotify playlist as a metadata export |
| `muzik spotify watch <ref>` | Add one Spotify playlist to the watchlist |
| `muzik bandcamp` | Download Bandcamp collection and organize with beets |
| `muzik split <file>` | Split audio file by chapters (with optional `--review`) |
| `muzik organize <dir>` | Tag/import audio with beets |
| `muzik import <dir>` | Import an existing music library into beets (`--agent` auto-tags) |
| `muzik archive <dir>` | Process existing downloaded files (split + organize) |
| `muzik validate <dir>` | Validate audio files, chapters, and metadata |
| `muzik gui` | Open the GPUI Kit desktop interface |
| `muzik cache` | Manage the platform-specific `muzik` cache |
| `muzik config` | Manage beets configuration |

## Agentic tagging

`muzik import --agent` tags a library without prompts. For each album, beets
finds candidate releases on MusicBrainz; the agent then chooses:

- A **strong match** (distance ≤ 0.10) is applied at once, with no LLM call.
- An **uncertain match** is sent to an LLM, which picks a candidate, keeps the
  files as-is, or skips them. It only chooses from the candidates beets found;
  it never invents tags.
- **Confident picks are applied; the rest are skipped** for manual review.

```sh
muzik import ~/Music --agent                     # tag untracked files
muzik import --agent --library "mb_albumid::^$"  # re-tag unmatched albums
muzik import ~/Music --agent --dry-run           # preview (beets shows nothing to apply)
```

Files are moved and retagged, so keep a backup.

### Agent backends

Choose the backend with `MUZIK_TAG_BACKEND` and the model with `MUZIK_TAG_MODEL`:

| Backend | How | Setup |
|---------|-----|-------|
| `openrouter` (default) | pydantic-ai over OpenRouter | `OPENROUTER_API_KEY`; model defaults to `z-ai/glm-5.2:free` |
| `codex` | shells out to `codex exec` | Codex CLI on `PATH`, signed in; uses your ChatGPT subscription. Defaults to the fast `gpt-5.3-codex-spark` model |
| `opencode` | shells out to `opencode run` | OpenCode CLI on `PATH`. Defaults to the free `opencode/deepseek-v4-flash-free` zen model — free, so no paid quota, only rate limits |

```sh
MUZIK_TAG_BACKEND=codex uv run muzik import --agent --library "mb_albumid::^$"
MUZIK_TAG_BACKEND=opencode uv run muzik import ~/Music --agent   # free zen model
```

For bulk tagging, prefer `openrouter` or a free `opencode` zen model: each
`codex exec` runs a multi-turn agent loop that costs tens of thousands of tokens
per album, which exhausts a subscription quota quickly. Keep codex for a few
hard cases.

Without a usable backend, only the strong-match fast path runs and uncertain
albums are skipped. When a CLI backend errors (missing model, not signed in),
the reason is printed as an `agent:` line and the album is skipped.

## Spotify playlist exports

`muzik` accepts a canonical version-1 Spotify playlist JSON file and
Exportify-style CSV files as metadata-only workflow inputs. It does not sign in
to Spotify, call the Spotify API, or pass Spotify URLs to `yt-dlp`.

Use Soulseek (or `auto` when `muzik soulseek check` reports ready) to acquire
audio for the exported tracks:

```sh
uv run muzik workflow playlist.spotify.json --audio-source soulseek --fallback none
uv run muzik workflow exportify-playlist.csv --audio-source soulseek --fallback none
```

Spotify exports reject episodes and require track title, artist, positive unique
position, and a deterministic track identity. Local tracks are supported with a
stable synthetic identity. Re-running an updated export skips entries already
organized by track ID, tolerates reordering, and acquires only new entries.
`--audio-source youtube` is intentionally rejected for Spotify exports.

See [SPOTIFY.md](SPOTIFY.md) for the supported JSON/CSV fields and the
metadata-only policy.

## Desktop interface

Build and run the desktop interface from a source checkout with:

```sh
mise run gui
```

An installed release wheel includes the native desktop program. Run it with
`muzik gui`. To build the program for a local wheel, run
`./scripts/build-native-gui.sh` before the wheel build.

The interface provides a workflow launcher, pipeline progress and logs, source
candidate tables, chapter review and editing, and Beets match and duplicate
decisions. It uses the same workflow and Beets service layer as the CLI. The
Rust window sends commands to a Python worker process. Cancel asks the worker
to stop at the next safe point.

The **Library** page lists the audio already in the output folder, so you can
see what is downloaded before you start a run.

### Playlist watchlist

Use **Watchlist** when you follow YouTube playlists and only want to ingest new
videos.

1. Set the Downloads and Splits paths in the launcher. Set the source, split,
   organization, and review options that you want to use.
2. Select **Watchlist**.
3. Paste a YouTube playlist URL and select **Add playlist**. You can add more
   than one source.
4. Select **Refresh**.

The first refresh reads every playlist item. It compares the item IDs with the
saved watchlist, yt-dlp archive, muzik playlist state, download folder, and split
folder. It sends only pending IDs to the workflow. A later refresh reads the
playlist again and sends only new or failed IDs. An error in one playlist does
not stop the other playlists.

The left rail keeps all saved sources. It shows the service, the name, the
item count, and the link of each source. Open, Copy, Rename, and Remove source
apply to the selected source.

You can also add a Spotify playlist, a Spotify album, or your Liked Songs.
Connect your Spotify account first on the **Spotify** page, or run
`muzik spotify login`. Each refresh then reads the current tracks
with the Spotify Web API and acquires the new ones from Soulseek. muzik reads
metadata only; it never downloads Spotify media. Set the audio source to
Soulseek for these sources. See [SPOTIFY.md](SPOTIFY.md) for the application
setup, the scopes, and the limits.

Select a source in the left rail. The page shows all current items in a paged
card list. Each card shows its title and the Download, Quality, Parse, Split,
and Organize states. **Processed** means that this computer
has the required local workflow state. A private or deleted video stays in the
list as **Unavailable** and does not run.

Use **Run** or **Retry** for the normal next command. Each card also has
these focused commands:

- **Download again** replaces the download and makes later stages stale.
- **Parse again** replaces accepted chapter data only after the new parse
  succeeds.
- **Split again** requires downloaded audio and accepted chapters.
- **Organize again** requires downloaded audio or an existing split directory.
- **Run all again** runs the complete item workflow again.

The interface asks for confirmation before a command can replace local files.
If a command is not available, the card shows the missing input.

**Load thumbnails** stores valid JPEG or PNG images in the normal muzik cache.
The viewer uses cached images when it starts and does not request them from the
network. The current `muzik cache` commands list and clean these files.

## Avoiding re-downloads

Each YouTube download keeps the video id in the filename (for example
`Title [dQw4w9WgXcQ].m4a`). Before a playlist run, `muzik` seeds the yt-dlp
download archive with every id already in the output folder. yt-dlp then skips
those ids, so a track is fetched only once — even when it appears in more than
one playlist. Use `muzik downloaded` to list the current inventory.

To override the skip and download again, use `--force` (`-f`) on the CLI, or the
**Force** checkbox in the desktop launcher. Force ignores the archive and passes
`--force-overwrites` to yt-dlp, so an existing file is replaced.

## Credits

- Bandcamp collection downloading is a Python port of [bandsnatch](https://github.com/Ovyerus/bandsnatch)
- Soulseek integration via the embedded [soulseek-rs](https://github.com/michel/soulseek-rs) client
- YouTube metadata and fallback audio via [yt-dlp](https://github.com/yt-dlp/yt-dlp)
- Audio processing via [FFmpeg](https://ffmpeg.org/)
- Music library management via [beets](https://beets.io/)

## Quick start

```sh
# Download, split by chapters, and import into beets
muzik workflow "https://youtube.com/watch?v=..."

# Search Soulseek for a FLAC/lossless album candidate
muzik soulseek search "Artist - Album flac"

# Download a selected Soulseek candidate
muzik soulseek download "Artist - Album" --prefer flac

# Or download a candidate ID shown by `muzik soulseek search`
muzik soulseek download --candidate <id>

# Measure real quality across the Beets library and suggest replacements
# (read-only; scope it to one artist first with --query)
muzik soulseek check-library --query "albumartist:Etnobotanika"
muzik soulseek download --candidate <id>   # fetch a suggested replacement

# Use YouTube metadata/playlist parsing but Soulseek for audio
muzik workflow "https://youtube.com/watch?v=..." --audio-source soulseek --prefer flac

# Fall back to YouTube audio if Soulseek finds no acceptable candidate
muzik workflow "https://youtube.com/watch?v=..." --audio-source soulseek --fallback youtube

# Download your full Bandcamp collection (opens browser on first run)
muzik bandcamp

# Import an existing music collection
muzik import ~/Music --copy
```

Bandcamp setup stores the authenticated cookies and username in the app config
directory. Cookie scope is preserved; use `muzik bandcamp --setup` to log in
again after expiry. Only releases downloaded successfully in a run are sent to
Beets for organization.

Only download music you are authorized to access.
