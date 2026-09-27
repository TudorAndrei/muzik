# Desktop interface and workflow architecture

`muzik gui` starts the Rust desktop app in `rust/gpui_app/`. The app uses GPUI
Kit 0.6.6. It starts `python -m muzik.native_gui` as a child process and sets
the Python path from the active `muzik` installation. The Python process owns
the existing workflow, Beets, watchlist, library, and Spotify services.

## Run from a source checkout

```sh
mise run gui
```

`mise run gui` runs `cargo run` for the Rust app with the local Python
interpreter. `./scripts/build-native-gui.sh` makes a release build and
copies the binary into `muzik/bin/` for a Python wheel. Release builds include
the binary in the wheel. The script requires the Rust version in `mise.toml`.

## Ownership

`muzik.core.workflow.service.run_workflow` and `WorkflowRunOperations` own a
workflow run. `muzik.core.watchlist` owns saved playlist and item state.
`muzik.core.beets.service` owns Beets import work. The command-line interface
uses these services without the desktop app.

The Python service in `muzik/native_gui/server.py` converts JSON commands into
core calls. It runs long work in a Python worker thread. It sends events and
decision requests to the Rust app. The Rust app keeps view state and draws the
window. It does not download, split, or organize audio.

The protocol is in [muzik/native_gui/PROTOCOL.md](muzik/native_gui/PROTOCOL.md).
Standard output carries only JSON records. Standard error carries service
errors. The Rust app reads the records outside the GPUI event loop and updates
its view on the event loop.

## Pages

- **Workflow** accepts a URL or local path. It uses saved settings and shows
  job progress, logs, and decisions.
- **Config** is a tab for output paths, source choices, quality policy, and
  processing options. Save them in `config.yaml` once for later runs.
- **Watchlist** shows saved YouTube and Spotify sources, item states, thumbnails,
  filters, and item commands. The Python service reads and saves the same
  watchlist file as the command-line interface.
- **Library** lists audio files in the selected download directory.
- **Settings** checks external services.
- **Spotify** stores a client ID, starts login, shows account state, and adds
  playlists or Liked Songs to the watchlist.

The Python service sends a result for each command. A job also sends progress,
completion, failure, or cancellation events. A decision request stops the
worker until the user replies. The UI can choose a Soulseek candidate, review
and edit chapters, approve a quality replacement, choose a Beets match, or
resolve a duplicate.

## Watchlist state

The app reads a saved watchlist before it asks YouTube or Spotify for new data.
The service reconciles saved items with local downloads, split files, and the
Beets library. Refresh checks the remote source and saves each completed item.
Spotify supplies track metadata only. Soulseek supplies audio for Spotify
tracks.

Each item has Download, Quality, Parse, Split, and Organize state. Item commands
use `run_item_action` in the core workflow. An earlier command can make later
stages stale:

| Command | Stages that become stale |
| --- | --- |
| Download again | Quality, Parse, Split, Organize |
| Check quality again | Parse, Split, Organize if it replaces audio |
| Parse again | Split, Organize |
| Split again | Organize |
| Organize again | None |
| Run all again | None after the full run |

The app asks for confirmation before a command replaces local files.

## Cancellation and process lifetime

One long job can run at a time. `job.cancel` sets a `CancellationToken` in the
Python service and wakes a pending decision. Core operations stop at their safe
points. The service closes when the Rust app closes its input pipe. The service
does not put tokens or passwords in protocol records.

Spotify login waits for a browser callback. A cancellation request cannot stop
that callback wait before its timeout. The service reports its result when the
wait ends.

## Verification

Run `mise run check` for Python and Rust checks. Run
`target/debug/muzik-gpui --check-backend` with `MUZIK_PYTHON`
set to the installed Python interpreter to check the live process link. A
release check must open the installed native app and test input, focus,
decisions, cancellation, resize, and the light and dark themes on each claimed
platform.
