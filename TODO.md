# GUI and Beets API Migration TODO

## Phase 1: Workflow Boundary

- [x] Add `muzik/core/workflow/events.py` with event dataclasses and an emitter
      protocol.
- [x] Add `muzik/core/workflow/decisions.py` with `WorkflowDecisions` and
      non-interactive default policies.
- [x] Add `muzik/ui/cli/events.py` to render workflow events with Rich.
- [x] Add `muzik/ui/cli/decisions.py` to contain terminal prompts.
- [x] Move Soulseek candidate selection in `muzik.commands.workflow` behind
      `WorkflowDecisions.choose_soulseek_candidate`.
- [x] Move Soulseek candidate selection in `muzik.commands.soulseek` behind
      the same decision adapter.
- [x] Move MusicBrainz chapter confirmation in `muzik.commands.workflow` behind
      `confirm_chapters` and `edit_chapters`.
- [x] Move `muzik.ui.chapter_editor` prompt handling behind a reusable chapter
      decision helper.
- [x] Add tests for candidate selection without `typer.prompt`.
- [x] Add tests for chapter confirmation/edit decisions without `input()`.

## Phase 2: Workflow Service Extraction

- [x] Add `muzik/core/workflow/service.py`.
- [x] Add workflow request/options dataclasses for the current
      `workflow_cmd` flags.
- [x] Move local audio discovery and validation into the workflow service.
- [x] Move playlist state handling into the workflow service.
- [x] Move source acquisition orchestration into the workflow service.
- [x] Move split/organize orchestration into the workflow service.
- [x] Keep `workflow_cmd` as a thin Typer wrapper.
- [x] Emit `StepStartedEvent` and `StepFinishedEvent` for download, split, and
      organize.
- [x] Emit candidate and chapter events where decisions are requested.
- [x] Add workflow service tests using fake decisions and fake emitters.

## Phase 3: Beets Core Boundary

- [x] Add `muzik/core/beets/__init__.py`.
- [x] Add `muzik/core/beets/config.py` with `open_library(config_path)`.
- [x] Add `muzik/core/beets/decisions.py` with `BeetsDecisions`.
- [x] Add `muzik/core/beets/events.py` for import task, match, duplicate, log,
      and completion events.
- [x] Add `muzik/core/beets/views.py` for UI-safe beets view models.
- [x] Add `muzik/core/beets/importer.py` with `MuzikImportSession`.
- [x] Implement `should_resume`, `choose_match`, `choose_item`, and
      `resolve_duplicate`.
- [x] Add `ImportOptions` covering move, copy, link, incremental, nowrite,
      quiet, dry-run, and config path.
- [x] Add import serialization so only one beets session runs per process.
- [x] Add tests for option translation and decision mapping.

## Phase 4: Replace `muzik organize`

- [x] Change `organize_cmd` to use `muzik.core.beets.import_paths` for default
      import behavior.
- [x] Preserve `--import` behavior as incremental move import.
- [x] Preserve `--dry-run` as beets pretend behavior.
- [x] Preserve `--config`.
- [x] Implement or isolate `--tag-only` support.
- [x] Keep a documented fallback to `beet` passthrough during rollout.
- [x] Add command tests for flag-to-option mapping.
- [x] Add failure-path tests for missing directory and missing beets config.

## Phase 5: Replace `muzik import`

- [x] Change `import_cmd` to use `muzik.core.beets.import_paths`.
- [x] Preserve incremental import.
- [x] Preserve `--copy`.
- [x] Preserve `--link`.
- [x] Preserve default move behavior.
- [x] Preserve `--nowrite`.
- [x] Preserve `--quiet`.
- [x] Preserve `--dry-run`.
- [x] Preserve `--config`.
- [x] Map quiet mode to non-interactive beets decision policies.
- [x] Add command tests for import option mapping.

## Phase 6: Add Textual TUI

- [x] Add `textual` to project dependencies.
- [x] Add `muzik/tui/app.py`.
- [x] Register `muzik tui` in `muzik.app`.
- [x] Build workflow launcher screen.
- [x] Build Soulseek candidate table screen.
- [x] Build chapter review/editor screen.
- [x] Build beets album/track match screen.
- [x] Build duplicate resolution modal.
- [x] Build pipeline progress/log screen.
- [x] Run workflow service in Textual workers.
- [x] Add smoke tests for TUI startup.

## Phase 7: Stabilization

- [x] Remove temporary beets subprocess fallback after API importer is stable.
- [x] Document external tool checks for `yt-dlp`, `ffmpeg`, `ffprobe`, `slskd`,
      and browser automation.
- [x] Add user-facing docs for `muzik tui`.
- [x] Add regression tests around playlist resume behavior.
- [x] Add regression tests around split cache behavior.
- [x] Add regression tests around beets duplicate decisions.
- [x] Evaluate PySide6 only after Textual validates the workflow boundary.

## Acceptance Checklist

- [x] Existing CLI commands keep their current flags and expected behavior.
- [x] Core workflow code can run without Rich, Typer, or terminal prompts.
- [x] Beets Python API usage is isolated under `muzik.core.beets`.
- [x] UI code receives view models, not raw beets internals.
- [x] GUI and CLI use the same service layer.
- [x] Long-running GUI operations are cancelable or visibly in progress.
- [x] Errors include enough context for the CLI and GUI to show actionable
      messages.
