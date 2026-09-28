# Desktop app

`muzik gui` opens the Rust GPUI app in `apps/gui`. The CLI finds
`muzik-gpui` beside its own binary or through `MUZIK_GPUI_BIN`.

## Run from a source checkout

```sh
mise run gui
```

The app reads the same Beets config and SQLite library files as the CLI.
Its Rust bridge in `apps/gui/src/bridge.rs` accepts UI commands and sends
results, progress, and decision requests. The Rust workflow and watchlist
services do the work outside the GPUI event loop.

## Pages

- **Workflow** accepts a URL or local path. It shows progress, logs, and
  decisions for download, split, and import work.
- **Watchlist** shows YouTube and Spotify sources, item states, filters, and
  item commands. The item sheet shows the thumbnail, IDs, and commands.
- **Library** lists audio in the selected download directory.
- **Settings** saves output paths, source choices, quality policy, and
  processing options in `config.yaml`, and checks external services.
- **Spotify** stores a client ID, connects an account, and adds playlists or
  Liked Songs to the watchlist.

## Watchlist

The app reads saved watchlist state before it checks a remote source. A
refresh adds new items and resumes failed ones. Spotify supplies track
metadata, and Soulseek supplies audio for those tracks.

Each item has Download, Quality, Parse, Split, and Organize state. The item
menu can run the next stage, retry a failed stage, or repeat a selected stage.
The app asks before a command replaces local files. A command that changes an
early stage can make later stages stale.

## Cancellation

One long job runs at a time. `job.cancel` marks the Rust job for cancellation.
The job stops at a safe point and keeps completed files and saved state.

## Verification

Run `mise run check` for the Rust checks. Run
`target/debug/muzik-gpui --check-backend` to check the startup path. Before
release, open the installed app and check input, focus, decisions,
cancellation, resize, and light and dark themes on each supported platform.
