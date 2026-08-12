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
