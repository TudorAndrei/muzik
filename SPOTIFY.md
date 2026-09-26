# Spotify playlist exports

`muzik` supports Spotify playlists as **metadata-only** workflow inputs. It
does not authenticate with Spotify, call its API, download Spotify media, or
send Spotify URLs to `yt-dlp`.

The export supplies track identity and metadata; audio is acquired from
Soulseek. Only use this workflow for music you are authorized to obtain.

## Run an export

```sh
uv run muzik workflow playlist.spotify.json --audio-source soulseek --fallback none
uv run muzik workflow exportify-playlist.csv --audio-source soulseek --fallback none
```

`--audio-source auto` is also supported when `muzik soulseek check` reports a
ready service. `--audio-source youtube` is rejected for an export because the
workflow never treats Spotify as a media URL.

The same file can be entered as the input path in `muzik gui`; choose Soulseek
as the audio source before starting the workflow.

## Canonical JSON v1

JSON is the preferred format. A file must be an object with `version: 1`,
`source: "spotify"`, `type: "playlist"`, a playlist `id`, a `title`, and an
`entries` array.

```json
{
  "version": 1,
  "source": "spotify",
  "type": "playlist",
  "id": "playlist-id",
  "title": "Road trip",
  "snapshot_id": "snapshot-id",
  "entries": [
    {
      "index": 1,
      "title": "Track title",
      "artists": ["Primary artist", "Guest artist"],
      "album": "Album title",
      "release_date": "2024-02-03",
      "duration_ms": 123000,
      "source_id": "spotify:track:track-id",
      "isrc": "US-ABC-24-00001",
      "disc_number": 1,
      "track_number": 2,
      "added_at": "2024-02-04T05:06:07Z"
    }
  ]
}
```

`artist` may be used instead of `artists`; the first artist becomes the search
artist. `duration` in seconds may be used instead of `duration_ms`. Per-track
`source_metadata` is retained, and the common fields above are normalized into
that metadata as well.

## Exportify-style CSV

CSV exports must have `track_name` and `artist_name`, plus at least one Spotify
identifier column: `spotify_track_id`, `spotify_track_uri`,
`spotify_track_url`, or `isrc`. Supported optional columns are:

```text
position,track_name,artist_name,artist_names,album_name,release_date,duration_ms,
spotify_track_uri,spotify_track_id,spotify_track_url,isrc,disc_number,
album_track_number,added_at
```

This marker requirement keeps unrelated CSV files on the normal local-input
path instead of misclassifying them as Spotify exports.

## Direct structured acquisition

Each entry is a resolved track (title, artist, duration), not a free-text
search query. `acquire_track_from_soulseek` searches Soulseek by that
identity and downloads the match directly — it never requests a complete
album for one playlist track, and a Spotify-derived track is never sent to
`yt-dlp` even with `--fallback youtube` (rejected, see above).

A returned candidate is kept only when its title/artist text and duration
plausibly match the track (see `candidate_matches_track` in
`muzik/core/sources/seakarr.py`); a multi-file candidate matches on the sum
of its file durations, for a candidate that is itself a bundled release.
Limits:

- If no safe candidate is found for a track, the run stops there (a graceful
  exit, code 0) instead of skipping that track and continuing to the rest of
  the playlist. A Soulseek search or download error stops the run the same
  way. Tracks already organized before the stopping point are recorded in
  playlist state and are not re-downloaded on the next run, but the run will
  hit the same failing track again until it is resolved (a better search
  term, a wider `--min-bitrate`, or removing that entry from the export).
- Track identity matching can still choose a wrong pressing or edit that
  happens to match on title, artist, and duration — review acquired files
  before trusting them for anything but casual listening.

## Validation and resume behavior

Episodes are rejected. Every track needs a title, artist, and positive unique
position. Tracks without a Spotify ID are treated as local tracks and receive a
deterministic synthetic ID derived from their title, artist, and position.

Workflow state is stored by playlist source ID and track ID. After an entry is
organized, a rerun skips it even if the export is reordered; newly added entries
are acquired and processed. Repeated tracks remain distinct through their
occurrence in the playlist.

## Spotify Web API

muzik can also read playlists directly from your Spotify account. It reads
**metadata only**: names, artists, albums, durations, and ISRC codes. It never
downloads Spotify media, and it never sends a Spotify URL to `yt-dlp`. The
audio still comes from Soulseek.

### Set up your own application

muzik has no Spotify application of its own, thus each user registers one:

1. Open <https://developer.spotify.com/dashboard> and create an application.
2. In **Edit settings**, add this redirect URI, then select **Add** and
   **Save**: `http://127.0.0.1:8888/callback`

   The value must agree character for character. Spotify accepts a loopback
   IP address, but not the name `localhost`, and the path `/callback` is part
   of the value. Use `muzik spotify login --port <number>` for a different
   port; muzik then saves that port and shows the new URI.
3. Save the client ID:

```sh
uv run muzik spotify set-client-id <client-id>
uv run muzik spotify status
```

The flow is Authorization Code with PKCE, thus there is no client secret.
Tokens are written to `spotify-token.json` in the muzik config directory,
with owner-only permissions. `MUZIK_SPOTIFY_CLIENT_ID` overrides the
config file.

A new application starts in Development Mode. Spotify then answers only for
the users that you add to the application, and its owner needs a Spotify
Premium account. A 403 answer usually has one of these two causes.

### Connect and read

```sh
uv run muzik spotify login          # opens the browser, then saves the tokens
uv run muzik spotify playlists      # Liked Songs and your playlists
uv run muzik spotify export liked -o liked.json
uv run muzik spotify watch liked    # add it to the watchlist
uv run muzik spotify logout
```

`export` writes the same canonical JSON v1 document as above, thus a file
from the API and a file from a manual export behave identically.

Scopes: `playlist-read-private`, `playlist-read-collaborative`, and
`user-library-read`. muzik asks for no write scope.

### Login problems

| What you see | Cause and correction |
|---|---|
| `redirect_uri: Not matching configuration` in the browser | The application does not have the URI that `muzik spotify status` shows. Add it in **Edit settings**, then **Save**. |
| `Port 8888 is in use` | Another program, or an earlier login that still waits, holds the port. Stop it, or use `--port`. |
| `No Spotify answer was received` | The browser never came back. This is almost always the redirect URI. |
| `403` with a Development Mode message | Add your Spotify user to the application, and give the owner account Premium. |

### Watchlist sync

A Spotify playlist, album, or the Liked Songs collection can be a watchlist
source. Each refresh reads the current tracks, adds new cards, keeps the
state of the tracks that are done, and acquires only the tracks that are not
done. Set the audio source to Soulseek: a Spotify source cannot use YouTube.

Unlike a single export run, a watchlist sync does not stop at the first track
that Soulseek cannot supply. That track keeps a `Failed` download stage, and
the sync continues with the other tracks. Use **Retry** on the card, or the
next refresh, to try that track again.

A Spotify card has only two stages that do work: Download (Soulseek) and
Organize (beets). Quality, Parse, and Split stay `Skipped`, because one track
is one file with no chapters.

Podcast episodes and local files in a playlist are skipped.

## Deferred work

Spotify account-data archive import, album grouping, and Spotify media
playback/download are intentionally out of scope.
