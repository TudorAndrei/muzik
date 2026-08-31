# Plan: Add a YouTube playlist watchlist viewer

## Goal

Add a watchlist to the DearPyGui app. A user can save multiple YouTube playlists,
view all videos as thumbnail cards, and see which muzik stages ran on this
computer. Refresh checks for playlist changes and processes only pending videos.
Each video has controls to run, retry, download again, parse chapters again, split
again, organize again, or run the full workflow again.

## Approach

### Playlist discovery and saved state

The current playlist workflow already has most of the processing logic.
`get_playlist_video_ids()` in `muzik/core/sources/youtube.py` gets ordered IDs.
`load_playlist_state()`, `save_playlist_state()`, and
`_process_playlist_video()` in `muzik/core/workflow/service.py` keep resume state
and process one playlist video. The implementation will make the per-video runner
a supported core function. The existing `run_workflow()` playlist path will call
the same function, so CLI behavior stays the same.

The viewer also needs titles, positions, and thumbnail URLs. A new structured
playlist lookup in `muzik/core/sources/youtube.py` will read yt-dlp flat-playlist
JSON and return those fields for every item. The existing ID-only function will
stay compatible with current callers. Private and deleted entries will remain in
the result when yt-dlp provides a playlist position but no usable video ID.

The watchlist needs durable storage. Cache files are not suitable because
`muzik cache clean` can remove them. A new `MUZIK_WATCHLIST_FILE` path in
`muzik/config.py` will point to `watchlist.json` in `MUZIK_CONFIG_DIR`. A new
`muzik/core/watchlist.py` module will own a versioned JSON format and the add,
remove, load, save, status, and refresh operations. Each playlist record will
contain its normalized URL, playlist ID, latest item snapshot, processed IDs,
check time, and errors. Each item will contain its position, title, video ID,
video URL, thumbnail URL, action state, paths, and errors.

The repository will write a temporary file in the same directory and then call
`Path.replace()`. An interrupted write cannot leave partial JSON. A malformed file
or an unsupported schema version will produce an error. The app will not replace
such a file with an empty watchlist.

### Local processing state

Each usable video will have four stage records:

- Download
- Parse
- Split
- Organize

A stage can be `Not started`, `Running`, `Complete`, `Failed`, `Skipped`, or
`Stale`. The card summary will be `Pending`, `Processing`, `Processed`, `Failed`,
or `Unavailable`. The current launcher options decide which stages are required.
For example, `No split` makes Split a skipped stage instead of a missing stage.

`muzik/core/watchlist.py` will reconcile new watchlist records with
`load_playlist_state()`, `backfill_playlist_entry_from_legacy_cache()`, and files
with matching YouTube IDs in the configured download and split folders. Thus, work
from a prior GUI run, CLI playlist run, or direct YouTube download can appear in
the viewer. The watchlist record remains useful after Beets moves a completed file
out of the download folder.

When an earlier stage runs again, later stage results can no longer prove that the
new output is processed. The action runner will apply these rules:

- Download again marks Parse, Split, and Organize as stale.
- Parse again marks Split and Organize as stale.
- Split again marks Organize as stale.
- Organize again changes only Organize.

The runner will not delete stale files. A confirmation dialog will identify files
that a force action can replace. The state file will save after every completed or
failed stage, so a crash does not hide completed work.

### Per-video commands

A new `muzik/core/workflow/item_actions.py` module will provide one UI-neutral
entry point for card commands. It will use the current workflow services and
operations instead of calling Typer command functions.

The commands will work as follows:

- `Run` or `Resume` starts at the first required stage that is incomplete or
  stale.
- `Retry` repeats the failed stage and continues with later required stages.
- `Download again` uses the current force-download behavior and stops after the
  download stage.
- `Parse again` refreshes the yt-dlp info metadata, rebuilds chapters from the
  embedded data, description, or pinned comment, and opens the current chapter
  review dialog. It replaces the chapter sidecar only after a successful result.
- `Split again` uses the current chapters and force-splits the available source
  audio.
- `Organize again` force-imports the available audio or split directory through
  the current Beets service.
- `Run all again` runs all required stages with force enabled.

The app will disable a command when its input does not exist. For example, Split
again needs source audio and accepted chapters. The command menu will show the
reason. All actions will use the current launcher output paths, source policy,
metadata policy, Beets config, and interactive setting.

### Thumbnail cache

A new `muzik/core/thumbnails.py` module will cache JPEG or PNG data under
`CACHE_DIR` with the video ID in the filename. The structured playlist result will
supply the thumbnail URL. The refresh worker will download missing thumbnails
with a small fixed concurrency limit through the existing `aiohttp` dependency.
It will validate the response type and use same-directory temporary-file
replacement. A failed image request will keep a local placeholder and can retry on
the next refresh.

The file names will stay directly under `CACHE_DIR`, so the current cache listing,
size, and age-clean operations can manage them. The GUI worker will read cached
files. It will use `GuiBridge.submit()` to create or replace DearPyGui textures on
the render thread. The view will release texture objects when it closes.

### Viewer design

The page will look like a compact video browser, but the main information is the
muzik pipeline. It will keep the current app palette:

- Canvas: `#181A1E`
- Raised panel: `#1E2126`
- Card control: `#262A30`
- Selection and active work: `#7AB2FF`
- Complete: `#78C878`
- Failed: `#E67878`

The left rail will list playlists and their pending counts. The main area will
have Refresh, status filter, and page controls above a card grid. Each 16:9 card
will show a cached thumbnail, playlist position, title, YouTube ID, summary state,
a primary Run, Resume, or Retry button, and an Actions button. Actions opens a
menu for the stage-specific commands.

The distinctive element will be a four-part pipeline rail below each thumbnail.
It will show Download, Parse, Split, and Organize in order. Color and a short text
label will both show state. The rail makes this a muzik viewer instead of a copy of
the YouTube site.

```text
+ Playlists --------+  Refresh   Status: All                 Page 1 / 4
| Late-night sets 3 |  +----------------------+  +----------------------+
| DJ archives     0 |  |      thumbnail       |  |      thumbnail       |
| Radio shows     1 |  +----------------------+  +----------------------+
|                  |  | 12  Video title       |  | 13  Video title       |
| + Add playlist   |  | DL  PA  SP  OR        |  | DL  PA  SP  OR        |
|                  |  | Resume   Actions...   |  | Process  Actions...  |
+------------------+  +----------------------+  +----------------------+
```

The grid will calculate its column count from the available width. It will page
items instead of creating all DearPyGui textures and widgets at once. Every item
will remain reachable. Cached thumbnails and the item snapshot let the page open
without a network request.

The launcher in `muzik/gui/launcher.py` will get a Watchlist button. A new
`WatchlistView` in `muzik/gui/watchlist.py` will own the rail, toolbar, card grid,
and action menus. Refresh and per-item commands will open the existing
`PipelineView`. The current event adapters, decision dialogs, progress display,
worker thread, and cancellation rules will remain in use. Back will return to the
watchlist and reload saved card state.

### Refresh behavior

Refresh will run playlists in list order. For each playlist, it will:

1. Fetch the structured flat-playlist result one time.
2. Save the current item list and queue missing thumbnails.
3. Reconcile each item with local muzik state.
4. Pass only pending or failed usable IDs to the extracted playlist runner.
5. Save each stage and item result at once.
6. Save the check time and playlist error, then continue to the next playlist.

On the first refresh, all usable current videos are pending unless existing muzik
state shows prior work. Removed videos will leave the current card grid, but their
processed IDs will remain in the watchlist record. If YouTube adds them again, the
app will not repeat completed work.

One playlist error will appear in the pipeline and the playlist rail. The refresh
will continue with the next playlist. Cancellation will stop the full refresh and
will not mark the active stage as complete.

This feature does not add a timer, background polling, YouTube API credentials,
video playback, playlist editing on YouTube, or CLI watchlist commands. It does
not remove local music when a video leaves a playlist. It does not store separate
workflow options for each playlist.

## Implementation phases

### Phase 1: Add durable playlist items and state

- Add `MUZIK_WATCHLIST_FILE` beside `MUZIK_CONFIG_FILE` in `muzik/config.py`.
- Add the versioned watchlist data types and repository operations in the new
  `muzik/core/watchlist.py` file.
- Add a structured flat-playlist lookup in `muzik/core/sources/youtube.py` that
  returns position, title, video ID, video URL, and thumbnail URL. Keep
  `get_playlist_video_ids()` compatible with current callers.
- Accept only YouTube playlist URLs that `playlist_id()` can parse. Normalize
  stored URLs and reject duplicate playlist IDs.
- Store current item snapshots, per-stage results, paths, errors, processed IDs,
  and playlist check results.
- Make load failures explicit and save with same-directory temporary-file
  replacement.
- Add `tests/test_watchlist.py` for missing files, save and load, duplicates,
  invalid URLs, malformed JSON, unsupported versions, and atomic replacement.
- Extend `tests/test_youtube_source.py` for ordered item metadata, missing titles,
  private or deleted items, invalid JSON, and yt-dlp errors.

Commit: `feat(watchlist): store YouTube playlist items and state`

### Phase 2: Process only pending playlist videos

- Refactor `_run_playlist_workflow()` and `_process_playlist_video()` in
  `muzik/core/workflow/service.py` so a core function can process an explicit,
  ordered list of video IDs and return per-ID results.
- Keep `run_workflow()` as the owner of normal URL detection. A CLI playlist run
  will still fetch and process the complete ordered ID list.
- Add watchlist refresh and local-state reconciliation in
  `muzik/core/watchlist.py`.
- Reuse `load_playlist_state()`, the per-playlist yt-dlp archive,
  `backfill_playlist_entry_from_legacy_cache()`, and
  `seed_archive_from_downloads()`.
- Keep unavailable items visible and exclude them from workflow work.
- Save each completed ID, leave failed IDs pending, and continue after one
  playlist error.
- Emit workflow messages and progress for checks, pending counts, failures, and
  the final summary.
- Extend `tests/test_workflow_service.py` for explicit IDs, CLI compatibility,
  result reporting, and cancellation.
- Extend `tests/test_watchlist.py` for pending-only work, incremental saves,
  retries, error isolation, removed IDs, no-change refreshes, and local-state
  reconciliation.

Commit: `feat(watchlist): process only pending playlist videos`

### Phase 3: Add thumbnails and per-video actions

- Add the thumbnail cache in `muzik/core/thumbnails.py`, with response validation,
  bounded downloads, atomic writes, retry behavior, and cache-compatible names.
- Add the item action and stage-state API in
  `muzik/core/workflow/item_actions.py`.
- Refactor the concrete functions inside `build_workflow_operations()` in
  `muzik/core/workflow/operations.py` only as needed so the normal workflow and
  targeted actions call the same download, parse, split, and organize code.
- Implement Run, Resume, Retry, Download again, Parse again, Split again,
  Organize again, and Run all again.
- Preserve the old chapter sidecar until Parse again has a successful accepted
  result. Mark later stages stale after an earlier stage runs again.
- Return disabled-action reasons when required audio, chapters, or split outputs
  do not exist.
- Add `tests/test_thumbnails.py` for cache hits, valid images, invalid responses,
  failed requests, atomic writes, and retry.
- Add `tests/test_item_actions.py` for stage selection, force options, path checks,
  stale-state rules, failure state, incremental saves, and cancellation.
- Extend current workflow tests to prove that the operation refactor does not
  change a normal single-video or playlist run.

Commit: `feat(watchlist): add cached thumbnails and item actions`

### Phase 4: Build the desktop watchlist viewer

- Add the Watchlist button and callback to `LauncherView` in
  `muzik/gui/launcher.py`.
- Add `WatchlistView` in `muzik/gui/watchlist.py` with the playlist rail, toolbar,
  paged thumbnail grid, pipeline rails, primary action, and per-card action menu.
- Add confirmation dialogs in `muzik/gui/modals.py` for commands that overwrite or
  force-process local results.
- Update `muzik/gui/theme.py` with card, image-outline, stage, selection, disabled,
  and error tokens derived from the current palette.
- Update `MuzikGuiApp` in `muzik/gui/app.py` to load playlists, refresh them, cache
  thumbnails, create textures through `GuiBridge`, and run item actions on the
  existing worker thread.
- Reuse `PipelineView` for refresh and item-action events. Track the prior page so
  Back returns to a reloaded watchlist after completion or cancellation.
- Disable only the active card and conflicting global controls during one item
  action. Keep status visible until the pipeline page opens.
- Add `tests/test_gui_watchlist.py` for playlist selection, pagination, card data,
  pipeline rails, status filters, placeholders, action availability, and action
  callbacks.
- Extend `tests/test_gui_launcher.py`, `tests/test_gui_app.py`, and
  `tests/test_gui_bridge.py` for navigation, texture updates, worker completion,
  return destination, and cancellation.

Commit: `feat(gui): add the YouTube-style watchlist viewer`

### Phase 5: Document and verify the feature

- Add a watchlist section to `README.md` with card states, first refresh behavior,
  retries, item commands, launcher option use, thumbnails, and manual refresh.
- Update `GUI.md` with the viewer layout, stage invalidation rules, shared pipeline
  view, thumbnail texture lifecycle, and `GuiBridge` boundary.
- Run `mise run check` and a render-context smoke test with two playlists, cached
  thumbnails, pending and complete cards, one item action, and Back navigation.

Commit: `docs(watchlist): explain playlist viewer and item actions`

## Risks and tradeoffs

- `yt-dlp --flat-playlist` must still list all items during each check. The feature
  avoids old processing work, but discovery time still grows with playlist size.
- Thumbnail requests add network and disk work. A fixed concurrency limit,
  permanent cache hits, paging, and placeholders will keep the interface usable.
- Old muzik state does not have four explicit stage records. Reconciliation must
  infer stages from playlist state, legacy cache, and file paths. The viewer will
  show `Pending` when the evidence is not sufficient.
- Re-running one stage can make later output stale. The explicit stage model and
  confirmation dialogs prevent the UI from reporting stale output as processed.
- Parse again needs current metadata and a local audio file. Private videos,
  missing source files, and removed comments can make the action unavailable or
  fail without changing the old chapter sidecar.
- Interactive chapter or Beets decisions can make a batch refresh wait for user
  input. The existing modals will show which card is waiting.
- The watchlist uses one option set for one refresh or action. Users cannot set a
  different source, output path, or Beets config for each playlist.

## Open questions

- None. The planned item command set is Run or Resume, Retry, Download again,
  Parse again, Split again, Organize again, and Run all again. These names and
  stage rules can change before implementation starts.
