# muzik

![muzik](assets/muzik-logo-v2.png)

Rust CLI and desktop app to download, split, and organize music from Soulseek,
YouTube, and Bandcamp.

---

Uses an embedded Soulseek client, the Rust `yt-dlp` crate, **ffmpeg**, and native
Rust music library tools. The crate starts the `yt-dlp` program to read YouTube
data, so that program must also be installed. The app gives progress feedback
and an interactive chapter editor.
Soulseek is used for higher-quality audio acquisition when configured;
yt-dlp remains available for YouTube metadata, playlist parsing, and fallback
audio downloads. Chapter sidecars can come from `.chapters.txt`, yt-dlp
`.info.json`, or album `.cue` sheets. Also downloads your full Bandcamp
collection.

## Requirements

- `yt-dlp` and `ffmpeg` on `$PATH`
- Optional for Soulseek: a Soulseek account (username/password) — the client
  is embedded, no separate server to run

Check external tools before running a full workflow:

```sh
yt-dlp --version
ffmpeg -version
muzik soulseek check      # when using Soulseek
```

For Bandcamp, export your own cookies to a file and give that file to
`muzik bandcamp --cookies` one time, or save the login in **Settings** in the
app. Muzik keeps the login for later runs.

### macOS arm64 prerequisites

Install the required command-line tools and confirm that they are on `PATH`:

```sh
brew install ffmpeg yt-dlp
ffmpeg -version
yt-dlp --version
```

## Soulseek setup

Soulseek support is an embedded Rust client (`crates/muzik-soulseek/`) — there
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

A Soulseek search or download error, no safe candidate, or a duration mismatch
keeps the YouTube file and logs a warning. "Safe" means the candidate
passes the same identity and duration checks as any other Soulseek search
result (see [SPOTIFY.md](SPOTIFY.md)). A multi-file replacement (a Soulseek
result with more than one track) is treated as a pre-split album and skips
chapter parsing for that item.

```sh
muzik workflow "https://youtube.com/watch?v=..." --quality-policy ask
muzik workflow "https://youtube.com/watch?v=..." --quality-policy auto --min-bitrate 192
```

In the desktop app, the launcher exposes the same **Quality policy** and **Min
bitrate** fields. With `ask`, the desktop app asks you before it replaces a
YouTube file.

## Spotify track acquisition

A Spotify JSON or CSV entry supplies a track title, artist, and duration.
`--audio-source` selects Soulseek or YouTube for its audio. Spotify supplies
metadata only. Already organized tracks are saved in playlist state, so a
later run can skip them. See [SPOTIFY.md](SPOTIFY.md).

## Install

### Desktop app on macOS

Homebrew installs `Muzik.app` in `/Applications`, with `ffmpeg` and `yt-dlp`:

```sh
brew install --cask tudorandrei/muzik/muzik
```

### Command-line program

mise installs only the `muzik` command-line program:

```toml
[tools]
"github:TudorAndrei/muzik" = { version = "latest", matching = "muzik-cli" }
```

Each [GitHub release](https://github.com/TudorAndrei/muzik/releases/latest) also
has `muzik-cli-<version>-<target>.tar.gz`, and a `muzik-gpui` archive for Linux.

For development, install from a source checkout:

```sh
git clone <repo>
cd muzik
mise run develop
cargo run --locked -p muzik-cli -- init
```

## Development

Run the locked checks that CI runs on each push:

```sh
mise run check
```

Add the release build with `mise run check-release`. The same checks run in CI
and as individual pre-push hooks.

## Workflow source policy

`--audio-source` chooses where audio comes from: `youtube`, `soulseek`, or
`auto`. `auto` uses Soulseek when the configured service is ready and otherwise
uses YouTube. `--fallback youtube` lets a Soulseek run use YouTube when it has
no acceptable result. `--metadata-source` controls chapter and metadata
lookup: `youtube`, `musicbrainz`, `none`, or `auto`.

Local audio paths are processed without remote acquisition. Spotify export
files provide metadata; the selected audio source acquires each track.

## Commands

| Command | Description |
|---------|-------------|
| `muzik init` | Create app directories and configure the music library |
| `muzik workflow <url-or-path>` | Full pipeline: acquire → split → import by default |
| `muzik workflow <url-or-path> --queue` | Add the run to the shared job queue, then run the queue |
| `muzik watchlist list` \| `add` \| `remove` | Show and change the watched playlists |
| `muzik watchlist refresh` | Check all playlists, queue their pending items, and run the queue |
| `muzik watchlist item <playlist> <position> --action <action>` | Queue one command for one item |
| `muzik jobs list` \| `show` \| `answer` \| `cancel` \| `run` | Show, answer, stop, and run queued jobs |
| `muzik download <url>` | Download audio from YouTube via yt-dlp |
| `muzik downloaded` | List audio already in the output folder |
| `muzik soulseek check` | Verify the embedded Soulseek client can connect and log in |
| `muzik soulseek search <query>` | Search Soulseek and rank candidates |
| `muzik soulseek download <query>` | Search Soulseek, download a selected result, and import its audio |
| `muzik soulseek check-library` | Measure audio quality in the music library and suggest Soulseek replacements |
| `muzik spotify set-client-id <id>` | Save the client ID of your own Spotify application |
| `muzik spotify login` \| `logout` \| `status` | Connect, disconnect, and check the Spotify account |
| `muzik spotify playlists` | List Liked Songs and your Spotify playlists |
| `muzik spotify export <ref>` | Write one Spotify playlist as a metadata export |
| `muzik spotify watch <ref>` | Add one Spotify playlist to the watchlist |
| `muzik bandcamp [--cookies <file>]` | Download the purchases of your Bandcamp collection |
| `muzik split <file>` | Split audio file by chapters (with optional `--review`) |
| `muzik organize <dir>` | Import audio by default, or write library tags with `--tag-only` |
| `muzik import <dir>` | Import audio into a Beets-compatible library |
| `muzik archive <dir>` | Process existing downloaded files (split + import by default) |
| `muzik validate <dir>` | Validate audio files, chapters, and metadata |
| `muzik cache` | Manage the platform-specific `muzik` cache |
| `muzik config` | Manage music library configuration |

## Spotify playlist exports

`muzik` accepts a canonical version-1 Spotify playlist JSON file and
Exportify-style CSV files as metadata-only workflow inputs. The CLI can also
sign in to Spotify to read playlist metadata. It never passes Spotify URLs to
`yt-dlp`.

Select an audio source for the exported tracks:

```sh
muzik workflow playlist.spotify.json --audio-source soulseek --fallback none
muzik workflow exportify-playlist.csv --audio-source youtube
```

Spotify exports reject episodes and require track title, artist, positive unique
position, and a deterministic track identity. Local tracks are supported with a
stable synthetic identity. Re-running an updated export skips entries already
organized by track ID, tolerates reordering, and acquires only new entries.
`--audio-source youtube` searches YouTube with each track's title and artist.

See [SPOTIFY.md](SPOTIFY.md) for the supported JSON/CSV fields and the
metadata-only policy.

## Desktop interface

Build and run the desktop interface from a source checkout with:

```sh
mise run gui
```

On macOS, install the app with Homebrew. On Linux, run `muzik-gpui` from its
release archive.

The interface provides a workflow launcher, pipeline progress and logs, source
candidate tables, chapter review and editing, and album match and duplicate
decisions. The app runs workflow and import work in Rust. Cancel asks the active
job to stop at the next safe point.

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
4. Select **Refresh** in the source header to check only that source, or
   **Refresh all** at the top to check every source.

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
`muzik spotify login`. Each refresh reads the current tracks through the
Spotify Web API. Watchlist Spotify items get audio from Soulseek only. muzik
reads Spotify metadata only; it never downloads Spotify media.
See [SPOTIFY.md](SPOTIFY.md) for the application
setup, the scopes, and the limits.

The **Bandcamp collection** source shows your Bandcamp purchases. It is added
automatically when you save a Bandcamp login in **Settings**. The Bandcamp
section there tells you how to copy the `identity` cookie from the browser
developer tools; muzik then finds your user name. A full Cookie header or a
`cookies.txt` file also works. muzik downloads each purchase
from Bandcamp in FLAC, then organizes it into the library. The login is kept
in `bandcamp_cookies.txt` and `bandcamp_user` in the muzik config directory,
so `muzik bandcamp` uses the same login.

Select a source in the left rail. The page shows its items in a table with the
title, the Download, Quality, Parse, Split, and Organize states, the status,
the next command, and the last error. Sort by number, title, or status. Double-
click a row, or right-click it and select **Details**, to see all commands.
**Processed** means that this computer has the required local workflow state.
A private or deleted video gets the **Unavailable** status. It does not run,
and it shows only in the **Unavailable** tab.

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

## Job queue

The CLI and the desktop app use the same job queue in `muzik.db` in the data
folder:

- A watchlist refresh checks the playlists, then adds one job for each pending
  item.
- An item command adds one job for that item. An item can have one open job.
- A workflow run from the app or from `muzik workflow` is also a queue job.

When a new version runs the queue for the first time, it moves the open jobs
from the old `jobs.db` into `muzik.db` and renames the old file to
`jobs.db.migrated`.

One process at a time runs the queue. It holds `jobs.lock` in the data folder.
When the app is open, it runs the queue, and a CLI command only adds jobs. When
the app is closed, the CLI command runs the queue until it is empty. Every
process can list, answer, and cancel jobs.

Five workers take jobs in this order: playlist checks, workflow runs, items.
Each stage waits for its resource:

| Gate | Stages | At the same time |
| --- | --- | --- |
| Download | YouTube and Soulseek downloads | 2 |
| Process | Quality check, split | 1 |
| Import | Organize into the library | 1 |

So one item can import while two others download. All jobs of a process use
one Soulseek login. The watchlist is in `muzik.db` in the data folder. Each
change is one SQLite transaction that writes only the playlists and items that
changed, so parallel jobs do not overwrite each other. On the first start,
muzik moves an old `watchlist.json` into `muzik.db` and renames the file to
`watchlist.json.migrated`. A job that was running when its process
stopped goes back into the queue when the queue runs again.

An item does not stop other items when it needs a choice. The item goes to the
Waiting state. Find it with `muzik jobs list` or in **Needs you** in the app.
Answer it with `muzik jobs show <id>` and `muzik jobs answer <id> <number>`, or
in the app. The answer puts the item back in the queue, and the waiting stage
runs again with that answer.

When an album is already in the library, a queued job uses the **Album already
in library** setting (`duplicates` in the `native_gui` config section):
`skip` (the default), `ask`, `keep_all`, or `remove_old`. Only `ask` shows a
question.

A workflow run that asks a question waits for the answer in the app, or in the
terminal for the CLI. It releases its gates while it waits, so other jobs
continue.

`muzik jobs cancel <id>` takes a queued job out of the queue, or stops a
running job at a safe point. The job keeps completed files and saved state.

```sh
muzik watchlist add "https://www.youtube.com/playlist?list=PL..."
muzik watchlist refresh
muzik jobs list
muzik jobs show queue-12
muzik jobs answer queue-12 1
```

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

- Soulseek integration via the embedded [soulseek-rs](https://github.com/michel/soulseek-rs) client
- YouTube metadata and fallback audio via [yt-dlp](https://github.com/yt-dlp/yt-dlp)
- Audio processing via [FFmpeg](https://ffmpeg.org/)
- Music library management via the native Rust library and importer

## Quick start

```sh
# Download, split by chapters, and import into the music library
muzik workflow "https://youtube.com/watch?v=..."

# Search Soulseek for a FLAC/lossless album candidate
muzik soulseek search "Artist - Album flac"

# Download a selected Soulseek candidate
muzik soulseek download "Artist - Album" --prefer flac

# Or download a candidate ID shown by `muzik soulseek search`
muzik soulseek download --candidate <id>

# Measure audio quality in the music library and suggest replacements
# (read-only; scope it to one artist first with --query)
muzik soulseek check-library --query "albumartist:Etnobotanika"
muzik soulseek download --candidate <id>   # fetch a suggested replacement

# Use YouTube metadata/playlist parsing but Soulseek for audio
muzik workflow "https://youtube.com/watch?v=..." --audio-source soulseek --prefer flac

# Fall back to YouTube audio if Soulseek finds no acceptable candidate
muzik workflow "https://youtube.com/watch?v=..." --audio-source soulseek --fallback youtube

# Download your Bandcamp collection with an exported cookie file
muzik bandcamp --cookies <file>

# Import an existing music collection
muzik import ~/Music --copy
```

The Bandcamp command needs an authenticated cookie file one time. After that it
uses the saved login in `bandcamp_cookies.txt` in the muzik config directory.

Only download music you are authorized to access.
