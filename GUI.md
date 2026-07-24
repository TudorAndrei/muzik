# TUI and workflow architecture

`muzik tui` is the supported interactive interface. It is a Textual application
over the same core workflow and Beets services used by the CLI; it is not a
separate implementation of download, split, or organization behavior.

## Boundaries

The core workflow is driven by `run_workflow` and `WorkflowRunOperations` in
`muzik.core.workflow.service`.

- Source adapters return source-neutral `ResolvedTrack`, `ResolvedRelease`, and
  `ResolvedPlaylist` values.
- `WorkflowDecisions` requests candidate and chapter choices without depending
  on a terminal prompt.
- `WorkflowEventEmitter` reports step, progress, candidate, chapter, message,
  and error events without depending on Rich or Textual.
- `build_workflow_operations` supplies concrete source, splitter, and Beets
  operations to both UI adapters.

The CLI maps those decisions and events to Rich. The TUI maps them to Textual
screens, tables, logs, and modals. Inputs may be YouTube URLs, local audio,
or supported Spotify export files; Spotify exports are metadata-only and route
to Soulseek when that source is selected.

## Beets interaction

`muzik.core.beets.service.organize_paths` owns organization requests. The
importer adapter keeps mutable Beets task/candidate objects in its owning worker
and exposes only immutable view models with opaque IDs across the UI boundary.

The TUI supplies `TuiBeetsDecisions` for match and duplicate modals and a
`TuiBeetsEventEmitter` for progress, logs, completion, and failures. In
non-interactive mode the same adapter path uses deterministic decisions rather
than opening a modal.

## Cancellation

Each pipeline run owns a thread-safe `CancellationToken`. The UI cancels the
token and waits for its worker to finish before returning to the launcher or
allowing another run. Core operations check the token at safe boundaries,
including transfer polling, split work, playlist entries, and before state or
source-deletion commits. Muzik-owned subprocesses are terminated on
cancellation; unrelated processes are never targeted.

Events arriving after a screen has unmounted are ignored. This keeps a canceled
pipeline from updating an unrelated screen.

## Operating the TUI

```sh
uv run muzik tui
```

The launcher configures the input, destination paths, source policy, split and
organization options, and interactivity. The pipeline screen shows progress and
logs, then opens candidate, chapter, and Beets decision screens only when the
chosen workflow requires them.

For a future native desktop application, reuse these core services and adapters
rather than calling command modules or moving Beets objects across UI threads.
