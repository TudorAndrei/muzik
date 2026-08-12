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

## Validation and resume behavior

Episodes are rejected. Every track needs a title, artist, and positive unique
position. Tracks without a Spotify ID are treated as local tracks and receive a
deterministic synthetic ID derived from their title, artist, and position.

Workflow state is stored by playlist source ID and track ID. After an entry is
organized, a rerun skips it even if the export is reordered; newly added entries
are acquired and processed. Repeated tracks remain distinct through their
occurrence in the playlist.

## Deferred work

Direct OAuth/API export, Spotify account-data archive import, album grouping,
and Spotify media playback/download are intentionally out of scope.
