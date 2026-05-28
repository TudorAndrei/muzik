# Spotify Playlist Export For Muzik

Research date: 2026-05-28

This note covers how to export Spotify playlists into a readable data format that
`muzik` can later ingest and use as metadata for Soulseek searches. No code has
been changed for this research.

## Recommendation

Use Spotify as a metadata source only. Export playlist metadata to JSON as the
canonical format, with CSV as a convenience format. Then ingest that export into
`muzik` as a source-neutral `ResolvedPlaylist`.

The best long-term source is Spotify Web API:

- Endpoint: `GET /v1/playlists/{playlist_id}/items`
- Docs: https://developer.spotify.com/documentation/web-api/reference/get-playlists-items
- Use `limit`/`offset` pagination.
- Request only track fields needed for ingestion with the `fields` query param.
- Include `playlist-read-private` and `playlist-read-collaborative` scopes when
  reading a user's private or collaborative playlists.

Spotify account data export is less suitable for repeatable ingestion. It can
include playlist JSON with names, artists, albums, local track names, playlist
names, last modified dates, descriptions, and follower counts, but it is an
account archive workflow rather than a normal sync/export interface.

Spotify account data docs:

- https://support.spotify.com/in-en/article/understanding-your-data/

## Existing Export Tools

For a quick no-code or low-code export, Exportify-style tools are useful:

- Exportify: https://github.com/watsonbox/exportify
- exportify-cli: https://github.com/donmerendolo/exportify-cli

Exportify exports UTF-8 CSV and includes useful fields such as track URI, track
name, artists, album, release date, duration, ISRC, added by, and added at.
`exportify-cli` can export CSV or JSON and is more automation-friendly.

These are good bootstrap options, but `muzik` should eventually own a native
Spotify exporter/importer so the schema is stable and matches the resolver.

## Legal And Policy Caveat

Spotify's developer docs state that Spotify content may not be downloaded or
used for stream ripping. Treat Spotify as a metadata source. Any subsequent
Soulseek/download workflow should only be used for files the user is legally
allowed to obtain, verify, or already has rights to.

## Recommended JSON Shape

This JSON shape maps cleanly onto the existing source-neutral models in
`muzik/core/sources/base.py`, especially `ResolvedPlaylist` and `ResolvedTrack`.

```json
{
  "version": 1,
  "source": "spotify",
  "type": "playlist",
  "id": "37i9dQZF...",
  "title": "Playlist Name",
  "source_url": "https://open.spotify.com/playlist/...",
  "snapshot_id": "...",
  "entries": [
    {
      "index": 1,
      "title": "Track Title",
      "artist": "Primary Artist",
      "artists": ["Primary Artist", "Featured Artist"],
      "album": "Album Name",
      "year": "1999",
      "duration": 213.4,
      "source": "spotify",
      "source_id": "spotify:track:...",
      "source_url": "https://open.spotify.com/track/...",
      "source_metadata": {
        "spotify_track_id": "...",
        "spotify_album_id": "...",
        "isrc": "...",
        "disc_number": 1,
        "album_track_number": 7,
        "added_at": "2024-01-01T00:00:00Z"
      }
    }
  ]
}
```

Recommended CSV columns:

```text
position,track_name,artist_name,artist_names,album_name,release_date,duration_ms,
spotify_track_uri,spotify_track_id,spotify_track_url,isrc,disc_number,
album_track_number,added_at
```

JSON should be the canonical internal format because it preserves nested artist
lists, album IDs, playlist metadata, and per-track source metadata without
string encoding.

## Useful Spotify API Fields

From playlist items, keep:

- Playlist level:
  - playlist ID
  - playlist name
  - playlist URL
  - snapshot ID
  - total item count
- Playlist item level:
  - item position
  - `added_at`
  - `is_local`
- Track level:
  - `track.id`
  - `track.uri`
  - `track.external_urls.spotify`
  - `track.name`
  - `track.artists[].name`
  - `track.album.id`
  - `track.album.name`
  - `track.album.artists[].name`
  - `track.album.release_date`
  - `track.duration_ms`
  - `track.disc_number`
  - `track.track_number`
  - `track.external_ids.isrc`

Skip episodes for the Soulseek music workflow, or preserve them with a different
entry type for future handling.

## Mapping To Current Muzik

The current source-neutral models already contain most required fields:

- `ResolvedPlaylist.title`
- `ResolvedPlaylist.entries`
- `ResolvedTrack.title`
- `ResolvedTrack.artist`
- `ResolvedTrack.album`
- `ResolvedTrack.year`
- `ResolvedTrack.index`
- `ResolvedTrack.duration`
- `ResolvedTrack.source`
- `ResolvedTrack.source_id`
- `ResolvedTrack.source_url`
- `ResolvedTrack.source_metadata`

Current Soulseek support is text-oriented:

- `SoulseekSource.resolve()` parses `"Artist - Album"` into a release or a raw
  string into a track.
- `SoulseekSource.search()` builds one search query from artist, album, title,
  and the preferred format.
- `SoulseekSource.download()` writes `.muzik.json` sidecar metadata after
  downloads.

Current workflow playlist handling is still YouTube-specific. It detects a
YouTube playlist ID, enumerates videos with `yt-dlp`, and processes each video.
Spotify playlist ingestion should become a separate resolve path that creates a
`ResolvedPlaylist` directly from JSON or CSV, then acquires audio through the
selected audio source.

## Soulseek Search Strategy

For Spotify playlist ingestion into Soulseek:

1. Load the Spotify export into a `ResolvedPlaylist`.
2. Group adjacent or repeated entries by Spotify album ID when useful.
3. For album groups, search Soulseek for:

   ```text
   artist album flac
   ```

4. Prefer complete album folders when the folder track count matches the
   expected group or album track count.
5. For loose playlist entries, search per track:

   ```text
   artist title album flac
   ```

6. Rank candidates by:
   - lossless format preference
   - duration tolerance against Spotify duration
   - filename similarity to artist/title/album
   - expected track count for album folders
   - queue length
   - free upload slot
   - upload speed
7. Write Spotify metadata into `.muzik.json` alongside the Soulseek candidate
   metadata so later validation and beets organization can explain the source.

## Suggested CLI Shape

Possible future commands:

```bash
muzik spotify export PLAYLIST_URL --output playlist.spotify.json
muzik spotify import playlist.spotify.json --audio-source soulseek --prefer flac
muzik workflow playlist.spotify.json --metadata-source spotify --audio-source soulseek
```

For a minimal first pass, skip direct Spotify auth and accept an exported
Exportify/exportify-cli CSV or JSON:

```bash
muzik workflow playlist.spotify.json --audio-source soulseek --prefer flac
```

## Open Design Questions

- Should Spotify ingestion live under `muzik spotify import`, or should
  `muzik workflow` auto-detect `.spotify.json` and `.csv` inputs?
- Should the canonical export format be a dedicated `spotify_playlist` schema,
  or a direct serialized `ResolvedPlaylist`?
- Should album grouping use only adjacent tracks, or all tracks sharing the same
  Spotify album ID?
- How strict should duration matching be before rejecting a Soulseek candidate?
- How should local Spotify tracks be represented when there is no Spotify track
  ID?
