# Desktop app

The Rust GPUI app in `apps/gui` builds `muzik-gpui`. It is separate from the
`muzik` command-line program. Homebrew installs it on macOS as `Muzik.app`.

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

A watchlist job does not stop when an item needs a choice. The item goes to
the Waiting state, and the job continues with the next item. **Needs you** in
Activity lists the waiting items. When you answer, the app keeps the answer in
`jobs.db` in the data folder and runs the waiting stage again with it. If you
cancel that run, the item waits again. An answer that is not used yet stays
in the queue after the app closes.

## AI decisions

When **Choose automatically** is on in Settings, the app picks album matches
and Soulseek downloads itself. An album match with a distance of 0.10 or less
is applied at once. Other choices go to `codex exec` with the model from
Settings (default `gpt-6-luna`, low reasoning) and a JSON schema. The app
uses an answer with a confidence of 0.65 or more. It asks you when the model
is not sure, fails, or does not answer within two minutes, and it shows the
model's suggestion and reason. Recent events lists each automatic choice.

The Codex CLI must be installed and logged in. Run the live check with
`cargo test -p muzik-agent -- --ignored`; it uses the account quota.

## Cancellation

One long job runs at a time. `job.cancel` marks the Rust job for cancellation.
The job stops at a safe point and keeps completed files and saved state.

## Verification

Run `mise run check` for the Rust checks, or `mise run check-release` to add
the release build. Run `target/debug/muzik-gpui --check-backend` to check the
startup path. Before
release, open the installed app and check input, focus, decisions,
cancellation, resize, and light and dark themes on each supported platform.
