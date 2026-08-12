# Plan: Replace the Textual TUI with a DearPyGui desktop front end

## Goal

Replace the Textual TUI with a native desktop front end built on DearPyGui,
exposed as `muzik gui`. The DearPyGui app becomes the only interactive interface.
The Typer CLI (`muzik download`, `split`, `organize`, and the rest) stays fully
active and unchanged. The Textual layer (`muzik/tui/`), its test, and the
`textual` dependency are removed. All download, split, and organization behavior
stays in the shared UI-neutral core; none of it is copied or changed.

## Approach

The muzik core is already isolated behind protocols in
`muzik.core.workflow.decisions`, `muzik.core.workflow.events`,
`muzik.core.workflow.service`, and the Beets adapters in `muzik.core.beets`. The
Textual layer in `muzik/tui/` is only a set of adapter classes
(`TuiWorkflowEventEmitter`, `TuiWorkflowDecisions`, `TuiBeetsDecisions`,
`TuiBeetsEventEmitter`) plus screens and `DataTable` widgets. The DearPyGui front
end recreates that presentation layer in a new `muzik/gui/` package, then the old
`muzik/tui/` package is deleted.

The build stays working at every phase: the new `muzik gui` command lands and is
proven before the `muzik tui` command, the `muzik/tui/` package, and `textual`
are removed in the final cutover phase.

### Verified facts (web check, Aug 2026)

- **Python 3.14 wheels exist.** DearPyGui 2.3.1 (2026-05-01) ships `cp314` wheels
  for macOS 13.0+ arm64, Windows x86-64, and Linux x86-64/aarch64. Issue
  [#2567](https://github.com/hoffstadt/DearPyGui/issues/2567) closed as completed
  on 2025-11-16. Pin `dearpygui>=2.3.1`.
- **Threaded-backend pattern.** DearPyGui runs callbacks on an internal worker
  thread for a steady framerate; long work runs on a separate Python thread with a
  killswitch flag. `configure_app(manual_callback_management=True)` lets the app
  drain `get_callback_queue()` inside its own render loop — the same place
  `GuiBridge.drain()` runs. The manual-render-loop design below matches this.

### Key design decisions

- **Reuse, do not fork.** The GUI imports `run_workflow`, `WorkflowRequest`,
  `WorkflowOptions`, `WorkflowRunOperations`, and `build_workflow_operations`
  unchanged, supplying new adapter objects that satisfy the same protocols.
- **Relocate shared, non-Textual pieces before deleting the package.** Two items
  in `muzik/tui/screens.py` are UI-neutral and must survive:
  - `WorkflowLaunchConfig` → new `muzik/core/workflow/launch.py`.
  - `_parse_chapter_text` / `_CHAPTER_RE` → `muzik/core/chapters.py` as a public
    `parse_chapters(text)` beside the existing `serialize_chapters`.
- **`dearpygui` is a base dependency**, replacing `textual`. It is now the primary
  interface. Importing DearPyGui does not require a display; a window opens only at
  `create_viewport`/`show_viewport`, so headless CLI use is unaffected.
- **Threading bridge is the core problem.** DearPyGui owns a single render loop on
  the main thread. The workflow runs on a background thread and makes **blocking**
  decision calls (for example `choose_soulseek_candidate`) that must wait for a
  modal result. DearPyGui has no equivalent of Textual's `call_from_thread` +
  `push_screen_wait`. A `GuiBridge` solves both needs:
  - A thread-safe `queue.Queue` of zero-argument callables. Worker threads push UI
    mutations; the main thread drains and runs them once per frame.
  - A blocking request primitive: the worker pushes a "show modal" callable
    carrying a result `queue.Queue`, then blocks on that queue. Modal button
    callbacks put the chosen value on it, unblocking the worker. A
    `CancellationToken` also unblocks it.
- **Manual render loop.** `create_context`/`create_viewport`/`setup_dearpygui`,
  then `while is_dearpygui_running():` draining the bridge queue and calling
  `render_dearpygui_frame`.
- **Cancellation matches the current contract.** Reuse `CancellationToken`.
  Closing the pipeline window or pressing Back cancels the token, unblocks any
  pending decision queue with a sentinel, and waits for the worker to finish
  before returning to the launcher — the discipline from `GUI.md` ("Cancellation")
  and `PipelineScreen._request_return_to_launcher`.

### New files

- `muzik/core/workflow/launch.py` — relocated `WorkflowLaunchConfig`.
- `muzik/gui/__init__.py`
- `muzik/gui/bridge.py` — `GuiBridge` (frame queue + blocking decision requests).
- `muzik/gui/adapters.py` — `GuiWorkflowEventEmitter`, `GuiWorkflowDecisions`,
  `GuiBeetsDecisions`, `GuiBeetsEventEmitter`.
- `muzik/gui/launcher.py` — launcher form → `WorkflowLaunchConfig`.
- `muzik/gui/pipeline.py` — status, progress bar, log, candidate/chapter/beets
  tables.
- `muzik/gui/modals.py` — candidate, chapter-review, chapter-edit, beets-match,
  duplicate modal windows.
- `muzik/gui/app.py` — `MuzikGuiApp` and `gui_cmd()`.
- `tests/test_gui_bridge.py`, `tests/test_gui_adapters.py`,
  `tests/test_gui_launcher.py`.

### Removed

- `muzik/tui/` (all files), `tests/test_tui_app.py`, the `textual` dependency, the
  `muzik tui` command, and the `[tasks.tui]` mise task.

### Out of scope

- Any change to download, split, metadata, or Beets behavior.
- Any change to the Typer CLI commands other than swapping `tui` for `gui`.
- Packaging a standalone desktop binary (PyInstaller, app bundles).
- New workflow features not already reachable from the current launcher.

## Implementation Phases

### Phase 1: Relocate shared pieces out of the TUI (no behavior change)

- Create `muzik/core/workflow/launch.py` and move `WorkflowLaunchConfig` there,
  unchanged (same fields, same defaults from `muzik.config`).
- Move `_parse_chapter_text`/`_CHAPTER_RE` into `muzik/core/chapters.py` as a
  public `parse_chapters(text) -> list[Chapter]`.
- Update `muzik/tui/screens.py` to import both from their new homes so the TUI and
  `tests/test_tui_app.py` keep working during the transition.
  **Commit:** `refactor(workflow): relocate launch config and chapter parsing out of the TUI`

### Phase 2: GUI dependency and entry point skeleton

- In `pyproject.toml`, add `dearpygui>=2.3.1` to `dependencies` (leave `textual`
  for now; it is removed in the cutover). Confirm the install resolves.
- Create `muzik/gui/__init__.py` and `muzik/gui/app.py` with `gui_cmd()` that
  creates a context, a viewport titled "muzik", shows an empty window, and runs
  the manual render loop.
- Register the command in `muzik/app.py` alongside the existing `tui` command:
  `app.command("gui", help="Open the DearPyGui workflow UI.")(gui_cmd)`.
- Manual smoke test: `uv run muzik gui` opens a window.
  **Commit:** `feat(gui): add dearpygui entry point and muzik gui command`

### Phase 3: Threading bridge

- Implement `GuiBridge` in `muzik/gui/bridge.py`:
  - `submit(callable)` — enqueue a UI mutation for the main thread.
  - `drain()` — run all queued callables; called once per frame.
  - `request(show_modal, cancellation)` — push a modal-builder that receives a
    result `queue.Queue`, block the caller, and return the result; raise
    `WorkflowCancelled` when the token cancels while waiting.
  - Ignore submissions after shutdown so late events cannot mutate a torn-down UI
    (mirrors the TUI "events after unmount are ignored" rule).
  **Commit:** `feat(gui): add thread-safe render-loop bridge for worker decisions`

### Phase 4: Launcher form

- Implement `muzik/gui/launcher.py` with the same fields as the current launcher
  (URL/path, downloads, splits, beets config, audio source, metadata, prefer,
  fallback, jobs, and the boolean switches).
- `read_config()` returns a `WorkflowLaunchConfig`, reusing the parsing rules from
  `WorkflowLauncherScreen.read_config` (path expansion, enum coercion for
  `MetadataSource`/`AudioSource`/`AudioFallback`, `jobs` default 0).
- Each path field (URL/path `raw`, downloads, splits, beets config) is a text
  input **plus** a "Browse…" button that opens a DearPyGui file dialog
  (`add_file_dialog`) and writes the chosen path back into the input. The `raw`
  field browses for a file (local audio / export); downloads and splits browse for
  a directory; beets config browses for a file. Text stays editable so a URL can
  still be typed or pasted.
- "Run" rejects an empty `raw`; "Quit" stops the viewport.
  **Commit:** `feat(gui): add workflow launcher form with file pickers`

### Phase 5: Pipeline view and event adapters

- Implement `muzik/gui/pipeline.py`: status label, progress bar, scrolling log, and
  three tables (candidates, chapters, beets matches) with the same columns as
  `muzik/tui/widgets.py`.
- Implement `GuiWorkflowEventEmitter` and `GuiBeetsEventEmitter` in
  `muzik/gui/adapters.py`, mapping every `WorkflowEvent`/`BeetsEvent` subclass
  handled in `PipelineScreen.handle_workflow_event` and `handle_beets_event`, each
  `emit` checking cancellation/shutdown and mutating only via `GuiBridge.submit`.
  **Commit:** `feat(gui): add pipeline view and workflow event adapters`

### Phase 6: Decision modals and adapters

- Implement `muzik/gui/modals.py`: candidate selection, chapter review
  (accept/edit/reject → `ChapterDecision`), chapter edit (text area →
  `parse_chapters`), beets match (`candidate_id` / `BeetsMatchDecision.AS_IS` /
  `None`), and duplicate resolution (`BeetsDuplicateDecision`).
- Implement `GuiWorkflowDecisions` and `GuiBeetsDecisions` in
  `muzik/gui/adapters.py`, each blocking method calling `GuiBridge.request`, with
  the same non-interactive deterministic defaults the Textual adapters use
  (candidate 0, `ChapterDecision.ACCEPT`, `BeetsDuplicateDecision.SKIP`).
  **Commit:** `feat(gui): add decision modals and workflow/beets decision adapters`

### Phase 7: Wire the run and cancellation

- In `muzik/gui/app.py`, build `WorkflowRequest`/`WorkflowOptions` from the config
  (same mapping as `PipelineScreen._run_workflow` and `_default_operations`), call
  `build_workflow_operations` with the GUI adapters, and run `run_workflow` on a
  background thread with a fresh `CancellationToken`.
- On pipeline close/Back: cancel the token, unblock any pending decision request,
  join the worker, and return to the launcher.
  **Commit:** `feat(gui): wire workflow run and cancellation`

### Phase 8: Cutover — remove the Textual TUI, add GUI tests, update docs

- `muzik/app.py`: remove the `tui` command and its import; keep only `gui`.
- Delete `muzik/tui/` and `tests/test_tui_app.py`.
- `pyproject.toml`: remove `textual`.
- `mise.toml`: replace `[tasks.tui]` with `[tasks.gui]` (`uv run muzik gui`).
- `README.md`: replace the "Textual TUI" section and the `muzik tui` command-table
  row with `muzik gui`; drop the "Textual is the first GUI target… PySide6" note.
- `muzik/core/workflow/operations.py`: reword the docstring that names Textual.
- Rewrite `GUI.md` for the DearPyGui front end and the `GuiBridge` threading
  contract; state that the CLI stays active over the same core.
- Add tests: `test_gui_bridge.py` (submit/drain order, request result,
  request-cancel raises, late submit ignored), `test_gui_adapters.py`
  (non-interactive defaults and result routing via a fake bridge, no real
  viewport), `test_gui_launcher.py` (`read_config()` field mapping and enum
  coercion). Guard viewport-dependent tests with a skip when no display is
  available.
  **Commit:** `feat(gui): remove Textual TUI, add GUI tests and docs`

## Risks & Tradeoffs

- **Python 3.14 wheel — RESOLVED.** DearPyGui 2.3.1 ships `cp314` macOS-arm64
  wheels; not a risk. Pin `dearpygui>=2.3.1`.
- **No built-in blocking modal.** Worker-thread decisions must block on a queue fed
  by the render loop. A wrong bridge can deadlock (worker waits for a result the
  main thread never delivers) or race. Mitigation: `GuiBridge` is isolated and
  unit-tested without a viewport (Phases 3 and 8).
- **Thread-safe UI mutation.** All item updates go through `GuiBridge.submit` and
  run on the main thread only. A direct `dearpygui` call from a worker is a defect.
- **Losing the terminal interface.** Removing Textual means no in-terminal / over-SSH
  interactive UI; a windowing system is now required for interactive runs. The CLI
  remains for headless use. This is the user's stated intent.
- **Headless CI for GUI tests.** Viewport-level tests need a display. Keep the
  render-loop-free logic (bridge, adapters, launcher config) testable without a
  viewport, and skip the rest when no display is present.
- **Immediate-mode UX.** DearPyGui's look is a tool aesthetic, not a polished
  consumer app. Acceptable for a power-user music tool.

## Decisions (resolved)

- **Path fields use both** a text input and a "Browse…" file dialog. Applies to
  the URL/path, downloads, splits, and beets config fields.
- **No `muzik tui` command and no mention of it anywhere.** The command is dropped
  outright — no compatibility hint, no alias. After Phase 8, a repository-wide grep
  for `tui`, tui-related `muzik.tui`, and `textual` (code, tests, docs, `mise.toml`,
  `pyproject.toml`) must return nothing.

## Implementation record (2026-08-12)

The implementation used one atomic cutover commit instead of eight intermediate
phase commits. This kept the dependency change, command cutover, package removal,
tests, and documentation in one reviewable state.

Verification used three layers:

- Render-context checks created the launcher, pipeline, and all decision dialogs
  without a viewport.
- A viewport smoke check opened `muzik gui` and kept its render loop active.
- The shared workflow completed with a 19-second YouTube input. An automated GUI
  worker test verified that the desktop adapter starts this same workflow and
  waits for cancellation before it returns to the launcher.

## Phase 9: Review findings (hardening)

A post-implementation review confirmed the threading bridge, adapters, decisions,
modals, cancellation, and the shared-core relocation are correct. One robustness
gap remains.

### Finding 1 — `GuiBridge.drain()` has no per-callback exception isolation

`drain()` (`muzik/gui/bridge.py`) runs each queued callback with no guard. If one
UI-update callback raises, the exception leaves the render loop; the `run()` loop
in `muzik/gui/app.py` unwinds to its `finally`, shuts the bridge down, and exits
the app. The old Textual message loop isolated each handler, so a single bad event
could not stop the interface. The update closures in `muzik/gui/adapters.py` are
defensive (type checks, guarded field access), so the current risk is low, but the
regression is real.

- Wrap each callback call in `drain()` in a `try`/`except Exception`, log the
  failure through the pipeline log (or a bridge error hook), and continue draining
  the rest of the queue.
- Keep `WorkflowCancelled` and any deliberate control-flow exceptions out of the
  swallow set, or none are raised inside UI callbacks by design — confirm before
  choosing the exception type to catch.
- Add a `test_gui_bridge.py` case: a callback that raises does not stop a following
  callback and does not propagate out of `drain()`.
  **Commit:** `fix(gui): isolate render-loop callback failures in the bridge`

### Resolution

`GuiBridge` now accepts an error hook. It catches ordinary `Exception` values,
reports them to the hook, and continues with the next queued callback.
`WorkflowCancelled`, `KeyboardInterrupt`, and `SystemExit` still propagate.
`MuzikGuiApp` uses the hook to write the error to the pipeline log.
