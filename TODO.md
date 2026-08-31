# TODO: Add a YouTube playlist watchlist viewer

## Phase 1: Add durable playlist items and state

- [x] Add `MUZIK_WATCHLIST_FILE` in `muzik/config.py`.
- [x] Add versioned watchlist data types and repository operations in
  `muzik/core/watchlist.py`.
- [x] Add structured flat-playlist lookup in
  `muzik/core/sources/youtube.py` without changing current ID-list behavior.
- [x] Store position, title, video ID, video URL, thumbnail URL, stage results,
  local paths, and errors for each item.
- [x] Validate and normalize YouTube playlist URLs. Reject duplicate playlist
  IDs.
- [x] Reject malformed JSON and unsupported schema versions without data loss.
- [x] Save watchlist changes with same-directory temporary-file replacement.
- [x] Add persistence and validation tests in `tests/test_watchlist.py`.
- [x] Extend `tests/test_youtube_source.py` for structured and unavailable items.
- [x] Commit: `feat(watchlist): store YouTube playlist items and state`

## Phase 2: Process only pending playlist videos

- [ ] Extract an explicit playlist-video runner from
  `muzik/core/workflow/service.py` and return per-ID results.
- [ ] Keep the current `run_workflow()` playlist behavior for CLI users.
- [ ] Add sequential watchlist refresh and local-state reconciliation in
  `muzik/core/watchlist.py`.
- [ ] Reuse playlist state, yt-dlp archives, legacy YouTube cache, and configured
  download and split folders.
- [ ] Keep unavailable items visible and exclude them from workflow work.
- [ ] Save each completed ID and leave failed IDs pending.
- [ ] Continue after one playlist error.
- [ ] Emit playlist check, pending, error, progress, and summary events.
- [ ] Extend `tests/test_workflow_service.py` for explicit IDs, CLI compatibility,
  result reporting, and cancellation.
- [ ] Extend `tests/test_watchlist.py` for pending-only work, incremental saves,
  retry, error isolation, removed IDs, no-change refreshes, and local state.
- [ ] Commit: `feat(watchlist): process only pending playlist videos`

## Phase 3: Add thumbnails and per-video actions

- [ ] Add thumbnail download and cache operations in
  `muzik/core/thumbnails.py`.
- [ ] Validate JPEG or PNG responses, limit concurrency, save atomically, and
  retry failed images on a later refresh.
- [ ] Keep thumbnail files directly under `CACHE_DIR` so current cache commands
  manage them.
- [ ] Add the stage-state and item-action API in
  `muzik/core/workflow/item_actions.py`.
- [ ] Share concrete download, parse, split, and organize operations with
  `build_workflow_operations()`.
- [ ] Implement Run, Resume, Retry, Download again, Parse again, Split again,
  Organize again, and Run all again.
- [ ] Preserve the old chapter sidecar until a new parse succeeds.
- [ ] Mark later stages stale after an earlier stage runs again.
- [ ] Return a reason for every disabled action.
- [ ] Add `tests/test_thumbnails.py` for cache, validation, failure, atomic writes,
  and retry.
- [ ] Add `tests/test_item_actions.py` for command routing, state changes, paths,
  force behavior, errors, persistence, and cancellation.
- [ ] Prove that normal single-video and playlist workflows have no behavior
  change after the operation refactor.
- [ ] Commit: `feat(watchlist): add cached thumbnails and item actions`

## Phase 4: Build the desktop watchlist viewer

- [ ] Add the Watchlist callback and button in `muzik/gui/launcher.py`.
- [ ] Add the playlist rail, toolbar, paged thumbnail grid, and card controls in
  `muzik/gui/watchlist.py`.
- [ ] Show a four-part Download, Parse, Split, and Organize rail on each card.
- [ ] Show each item's position, title, YouTube ID, summary state, primary action,
  and Actions menu.
- [ ] Add confirmation dialogs for force and overwrite actions in
  `muzik/gui/modals.py`.
- [ ] Add card and stage tokens to `muzik/gui/theme.py`.
- [ ] Load cached thumbnails and create DearPyGui textures through `GuiBridge`.
- [ ] Release watchlist textures when the view closes.
- [ ] Connect add, remove, refresh, filters, paging, item actions, and Back in
  `muzik/gui/app.py`.
- [ ] Use current launcher paths and options for refresh and item commands.
- [ ] Reuse `PipelineView` and return to a reloaded watchlist after completion or
  cancellation.
- [ ] Add `tests/test_gui_watchlist.py` for card layout, item data, stage rails,
  paging, filtering, placeholders, actions, and disabled reasons.
- [ ] Extend GUI app, launcher, and bridge tests for navigation, textures, worker
  lifecycle, and cancellation.
- [ ] Commit: `feat(gui): add the YouTube-style watchlist viewer`

## Phase 5: Document and verify the feature

- [ ] Document playlist setup, card states, Refresh, first-refresh behavior,
  retries, item actions, thumbnails, and launcher options in `README.md`.
- [ ] Document viewer layout, stage invalidation, texture lifecycle, and
  `GuiBridge` use in `GUI.md`.
- [ ] Run `mise run check`.
- [ ] Run a DearPyGui render-context smoke test with two playlists, thumbnails,
  multiple card states, one item action, and Back navigation.
- [ ] Commit: `docs(watchlist): explain playlist viewer and item actions`

## Verification

- [ ] `mise run check` passes with the locked dependencies.
- [ ] `tests/test_watchlist.py` proves that save and load keep item metadata, stage
  state, paths, and errors, and that an invalid file is not overwritten.
- [ ] `tests/test_youtube_source.py` proves that one flat lookup returns every
  ordered playlist item and keeps unavailable items visible.
- [ ] `tests/test_workflow_service.py` proves that a normal playlist CLI run still
  fetches and processes its complete ordered ID list.
- [ ] A second watchlist refresh sends no workflow work for processed IDs.
- [ ] A playlist with one added video sends only that video to the workflow.
- [ ] A failed video stays pending and runs again on the next refresh.
- [ ] One bad playlist does not stop checks for later playlists.
- [ ] Cancellation stops before the next video and does not mark the active stage
  as complete.
- [ ] A selected playlist shows every current item with position, title, ID,
  thumbnail, and local stage state.
- [ ] Work from an earlier GUI or CLI run appears as local work.
- [ ] A private or deleted item shows as Unavailable and does not run.
- [ ] Cached thumbnails load without a network request on the next app start.
- [ ] A failed thumbnail uses a placeholder and retries on a later refresh.
- [ ] Download again marks Parse, Split, and Organize stale.
- [ ] Parse again preserves the old chapter sidecar on failure and marks Split and
  Organize stale on success.
- [ ] Split again requires audio and chapters, then marks Organize stale.
- [ ] Organize again requires an existing audio file or split directory.
- [ ] Force and overwrite actions require confirmation.
- [ ] Disabled card commands show the missing input reason.
- [ ] Manual smoke test: add two playlists, refresh, page through all cards, filter
  by status, run one item command, go Back, and see the updated stage rail.
- [ ] Manual smoke test: cancel an active refresh or item command and return after
  the worker stops.
- [ ] The launcher, Library page, Settings window, CLI playlist command, split
  flow, organize flow, and cache commands still work.

## Review

- [ ] Review the code.
- [ ] Update `PLAN.md` and `TODO.md` before implementation if the approach changes.
- [ ] Make each phase commit with its exact planned message.
- [ ] Check each completed TODO item after its phase commit succeeds.
