# TODO: Replace the Textual TUI with a DearPyGui desktop front end

## Phase 1: Relocate shared pieces out of the TUI (no behavior change)

- [x] Create `muzik/core/workflow/launch.py` with `WorkflowLaunchConfig` (fields and defaults unchanged)
- [x] Move `_parse_chapter_text`/`_CHAPTER_RE` into `muzik/core/chapters.py` as public `parse_chapters(text)`
- [x] Preserve the shared parsing behavior through the final cutover
- [x] Run the chapter and full test suites — pass
- [x] Include the phase in the atomic cutover commit

## Phase 2: GUI dependency and entry point skeleton

- [x] Add `dearpygui>=2.3.1` to `dependencies` in `pyproject.toml`
- [x] Confirm the install resolves with `uv sync`
- [x] Create `muzik/gui/__init__.py` and `muzik/gui/app.py` with `gui_cmd()` and a manual render loop
- [x] Register `app.command("gui", ...)(gui_cmd)` in `muzik/app.py`
- [x] Smoke test: `uv run muzik gui` opens a viewport titled "muzik"
- [x] Include the phase in the atomic cutover commit

## Phase 3: Threading bridge

- [x] Implement `GuiBridge.submit` / `drain` in `muzik/gui/bridge.py`
- [x] Implement `GuiBridge.request` (blocking decision with result queue + `CancellationToken`)
- [x] Late submissions after shutdown are ignored
- [x] `request` raises `WorkflowCancelled` when the token cancels while waiting
- [x] Include the phase in the atomic cutover commit

## Phase 4: Launcher form

- [x] Build launcher fields matching the current launcher (inputs, combos, checkboxes)
- [x] Add a "Browse…" file dialog next to each path field (raw → file, downloads → dir, splits → dir, beets config → file) that writes the chosen path back into the input; text stays editable
- [x] `read_config()` returns a `WorkflowLaunchConfig` with the same parsing rules (path expansion, enum coercion, `jobs` default 0)
- [x] "Run" rejects an empty `raw`; "Quit" stops the viewport
- [x] Include the phase in the atomic cutover commit

## Phase 5: Pipeline view and event adapters

- [x] Build pipeline view: status label, progress bar, log, and candidate/chapter/beets tables with the prior columns
- [x] Implement `GuiWorkflowEventEmitter.emit` for every workflow event type
- [x] Implement `GuiBeetsEventEmitter.emit` for every Beets event type
- [x] Both emitters check cancellation/shutdown and mutate the interface only through `GuiBridge.submit`
- [x] Include the phase in the atomic cutover commit

## Phase 6: Decision modals and adapters

- [x] Implement candidate, chapter-review, chapter-edit, beets-match, and duplicate modals
- [x] Chapter editor uses `parse_chapters` (shared core helper, no duplicate logic)
- [x] Implement `GuiWorkflowDecisions` (candidate/chapters/edit) via `GuiBridge.request`
- [x] Implement `GuiBeetsDecisions` (match/duplicate) via `GuiBridge.request`
- [x] Non-interactive mode returns deterministic defaults (candidate 0, `ChapterDecision.ACCEPT`, `BeetsDuplicateDecision.SKIP`)
- [x] Include the phase in the atomic cutover commit

## Phase 7: Wire the run and cancellation

- [x] Build `WorkflowRequest`/`WorkflowOptions` from config and call `build_workflow_operations` with GUI adapters
- [x] Run `run_workflow` on a background thread with a fresh `CancellationToken`
- [x] Pipeline close/Back cancels the token, unblocks pending decisions, joins the worker, returns to the launcher
- [x] Include the phase in the atomic cutover commit

## Phase 8: Cutover — remove the Textual TUI, add GUI tests, update docs

- [x] `muzik/app.py`: remove the old interactive command and its import; keep only `gui`
- [x] Delete the old interactive package and its test
- [x] `pyproject.toml`: remove the old interface dependency
- [x] `mise.toml`: replace the old interface task with `[tasks.gui]`
- [x] `README.md`: document `muzik gui` and remove the superseded interface note
- [x] `muzik/core/workflow/operations.py`: use interface-neutral wording
- [x] Rewrite `GUI.md` for DearPyGui and the `GuiBridge` contract; state that the CLI stays active
- [x] Add `tests/test_gui_bridge.py`, `tests/test_gui_adapters.py`, `tests/test_gui_launcher.py`, and `tests/test_gui_app.py`
- [x] Commit: `feat(gui): remove old interface, add DearPyGui desktop app`

## Verification

- [x] `uv run pytest` — 175 tests pass; no superseded interface import remains
- [x] Repository search for the old interface names returns nothing in code, tests, and user documentation
- [x] New tests written: `test_gui_bridge.py`, `test_gui_adapters.py`, `test_gui_launcher.py`, `test_gui_app.py`
- [x] Bridge tested: `submit`/`drain` order, request result, cancellation, shutdown, and late submit
- [x] Adapters tested: non-interactive defaults and result routing through a fake bridge
- [x] Launcher tested: `read_config()` maps fields and coerces enums
- [x] Refactor check: launch defaults are unchanged and `parse_chapters` output matches the prior parser
- [x] CLI unaffected: root help and `muzik split --help` work without a viewport
- [x] Smoke checks: viewport opens, all dialog types build, and the shared workflow completes with a small YouTube input
- [x] Edge cases: cancel returns to the launcher after worker teardown, blocking requests cancel, and empty `raw` is rejected
- [x] `dearpygui` imports and builds widgets without a display or viewport

## Phase 9: Review findings (hardening)

- [x] `GuiBridge.drain()`: wrap each callback in `try`/`except Exception`, log the failure, and keep draining the rest of the queue
- [x] Confirm which exceptions to swallow (do not eat `WorkflowCancelled` or deliberate control-flow exceptions)
- [x] Add `test_gui_bridge.py` case: a raising callback neither stops a following callback nor propagates out of `drain()`
- [x] Run `uv run pytest`, `uv run ruff check`, `uv run ty check` — all pass
- [x] Commit: `fix(gui): isolate render-loop callback failures in the bridge`

## Review

- [x] Code reviewed
- [x] PLAN.md records the atomic cutover approach
- [x] The atomic cutover leaves the build working
- [x] TODO.md items all checked off
- [x] Phase 9 review finding resolved
