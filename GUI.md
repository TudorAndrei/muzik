# Desktop interface and workflow architecture

`muzik gui` is the supported interactive interface. It uses DearPyGui over the
same core workflow and Beets services that the command-line interface uses. It
does not contain separate download, split, or organization logic.

## Boundaries

`run_workflow` and `WorkflowRunOperations` in `muzik.core.workflow.service`
drive the core workflow.

- Source adapters return source-neutral track, release, and playlist values.
- `WorkflowDecisions` requests candidate and chapter choices without a toolkit
  dependency.
- `WorkflowEventEmitter` reports step, progress, candidate, chapter, message,
  and error events without a toolkit dependency.
- `build_workflow_operations` supplies the concrete source, splitter, and Beets
  operations to both interactive adapters.

The command-line interface maps decisions and events to Rich. The desktop
interface maps them to DearPyGui windows, tables, logs, and modal dialogs. Both
interfaces accept YouTube URLs, local audio, and supported Spotify export files.

## Render-thread bridge

DearPyGui owns the main render thread. A workflow runs in a separate Python
thread. `GuiBridge` is the only path from a workflow worker to the interface.

- `submit` adds a zero-argument interface update to a thread-safe queue.
- The manual render loop calls `drain` once per frame.
- `request` adds a modal builder to the same queue and blocks the worker on a
  result queue.
- A modal callback puts one result in that queue.
- Cancellation and shutdown unblock all current requests.
- Shutdown rejects late submissions. A stopped pipeline cannot change a new
  view.

All DearPyGui item changes occur on the render thread. Core services and worker
adapters do not call DearPyGui directly.

Watchlist thumbnail downloads also run outside the render thread. The worker
writes validated JPEG or PNG files to the normal cache. The app then submits
texture creation through `GuiBridge`. `WatchlistView` owns the texture IDs. It
keeps textures only for the current page. It deletes old page textures at a page
change. It deletes all remaining textures when the page closes, before it
deletes the texture registry.

## Watchlist viewer

`muzik.core.watchlist` owns the versioned playlist snapshot and stage state.
The GUI reads this file when the Watchlist page opens. Opening the page does
not read YouTube.

Reconciliation compares the saved state with the download folder, the muzik
playlist state, and the Beets library. It takes seconds on a large library,
thus it never runs on the render thread. The page shows the saved file at
once, a worker reconciles it, and the result comes back through `GuiBridge`.
If the watchlist file changed while that worker ran, its result is thrown away
and the worker starts again: a saved snapshot from before an addition must
never overwrite the new source.

The page has two main areas:

- The left rail is the permanent list of saved sources. Each entry shows its
  service, its name, its item count or `Link only`, its last check time, and
  its errors. The footer of the rail shows the link of the selected source,
  with Open, Copy, Rename, and Remove source.
- The main area has status filtering, paging, and a responsive card list.
  Paging limits the active card widgets and textures.

### Sources

`parse_source` finds the reference in the text that the user adds:

- A YouTube playlist URL becomes a YouTube source. Refresh reads its items
  with yt-dlp and saves its name.
- A Spotify playlist link, a Spotify album link, or `liked` becomes a Spotify
  source. Refresh reads its tracks with the Spotify Web API and acquires the
  pending ones from Soulseek. A card is one track, thus its Quality, Parse,
  and Split stages stay `Skipped`.

### Spotify window

**Spotify...** opens `SpotifyDialog`. It holds the client ID of the user's own
application, the redirect URI, the login, and the list of the account's
playlists with an **Add** button for each.

Every call to Spotify runs in a worker thread, never on the render thread. The
worker reports back through `GuiBridge.submit`, which rebuilds the window with
a new `SpotifyState`. The window itself holds no Spotify logic: the app builds
each state from the config file, the token store, and the saved watchlist.

Each card has a five-part Download, Quality, Parse, Split, and Organize bar.
Each part has a status color, and the tooltip of the bar gives the text state
of every part. The summary is Pending, Processing, Failed, Processed, or
Unavailable. The primary button is Run, Resume, or Retry. More actions opens
the Commands window, which has all focused commands, a link to the video, and
a reason under each disabled command.

Card text is cut to one line, because a card has a calculated height. The
card width comes from the rendered width of the card area, not from the
viewport width: the page is inside the shell tab bar, which is more narrow
than the viewport.

### Quality stage

The Quality stage runs `check_youtube_quality` against the item's downloaded
file (see [README.md](README.md#quality-policy)). It only acts when the
launcher's **Quality policy** field is `ask` or `auto` — `off` leaves the
stage `Skipped`. A search/download error, no safe Soulseek candidate, or a
duration mismatch all complete the stage as `Complete` with the YouTube file
kept, not `Failed` — quality checking never fails an item.

`ask` has no replacement dialog in the desktop interface yet:
`GuiWorkflowDecisions.confirm_quality_replacement` always declines, so `ask`
behaves like a check-only pass in the GUI even though the CLI prompts. Use
`auto` in the GUI to actually replace a low-quality YouTube file.

**Check quality again** in the item Actions window re-runs this stage on a
downloaded item. When it replaces the file, Download is marked complete
against the new file and Parse/Split/Organize become stale. When the
replacement is a multi-file Soulseek album, Download is repointed at the
result directory and the same later stages become stale — chapter parsing is
skipped for that item going forward.

`refresh_watchlist` performs one flat playlist lookup for each saved playlist.
It merges the ordered snapshot, keeps unavailable items, reconciles local work,
and sends only pending IDs to `run_youtube_playlist_videos`. It saves each item
result before it starts the next ID. Thumbnail cache work starts after playlist
processing.

`run_item_action` owns focused item state changes. An earlier stage can make
later stages stale:

| Command | Completed stage | Stale stages |
|---------|-----------------|--------------|
| Download again | Download | Quality, Parse, Split, Organize |
| Check quality again | Quality | Parse, Split, Organize (only if it replaces the file) |
| Parse again | Parse | Split, Organize |
| Split again | Split | Organize |
| Organize again | Organize | None |
| Run all again | Full workflow | None |

The app uses the current launcher paths and options for refresh and item work.
It shows the existing `PipelineView` while a worker runs. Completion or
cancellation returns to a new watchlist load. Back first cancels an active token
and waits for the worker to stop. It does not let an old worker update the new
page.

## Beets interaction

`muzik.core.beets.service.organize_paths` owns organization requests. The Beets
adapter keeps mutable task and candidate objects in its worker. It sends only
immutable view models and opaque candidate IDs across the interface boundary.

`GuiBeetsDecisions` supplies match and duplicate dialogs.
`GuiBeetsEventEmitter` supplies progress, logs, completion, and failure events.
Non-interactive runs use deterministic decisions and do not open dialogs.

## Cancellation

Each pipeline run owns one thread-safe `CancellationToken`. Back cancels the
token, closes decision dialogs, unblocks requests, and waits for the worker to
stop before it returns to the launcher. Closing the viewport uses the same
cancellation path. Core operations check the token at safe boundaries and stop
processes that muzik owns.

## Operation

```sh
uv run muzik gui
```

The launcher configures the input, destination paths, source policy, split and
organization options, and interactivity. Each path field accepts text and also
has a file or directory picker. The pipeline view shows progress and logs. It
opens decision dialogs only when the selected workflow needs them.

All existing command-line commands stay active for headless use.
