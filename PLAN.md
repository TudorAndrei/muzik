# GUI and Beets API Migration Plan

This plan turns the direction in `GUI.md` into an implementation path for this
repo. The main goal is to make the existing workflow UI-neutral before adding a
GUI, then replace interactive `beet` subprocesses with a beets Python API
boundary.

## Current Findings

The project is still CLI-first:

- `muzik.app` registers Typer commands directly.
- `muzik.commands.workflow` orchestrates download, chapter detection, split,
  Soulseek acquisition, and beets organization in one terminal-oriented flow.
- `muzik.commands.organize` and `muzik.commands.import_` shell out to `beet`
  through `run_passthrough` so beets can own stdin/stdout.
- `muzik.commands.workflow`, `muzik.commands.soulseek`, and
  `muzik.ui.chapter_editor` still use `input()` or `typer.prompt`.
- Progress is tied to Rich/terminal rendering through `console.print`,
  `Live`, and `Progress`.
- The best existing GUI-ready model boundary is in
  `muzik.core.sources.base`, especially `DownloadRequest`, `Candidate`,
  `ResolvedTrack`, `ResolvedRelease`, and `DownloadResult`.

The first GUI should be Textual, exposed as `muzik tui`. Native desktop can be
revisited later after the workflow has a stable event and decision API.

## Target Architecture

Introduce three layers:

1. Core services
   - Own workflow behavior.
   - Emit structured events.
   - Request choices through a decisions protocol.
   - Avoid direct Rich, Typer, or `input()` usage.

2. UI adapters
   - CLI adapter renders events with Rich and answers decisions with terminal
     prompts.
   - Textual adapter renders the same events in widgets and answers decisions
     with tables, modals, and forms.

3. Command entrypoints
   - Typer commands parse flags and call services.
   - Commands should become thin wrappers around service functions.

## Proposed Package Layout

```text
muzik/core/workflow/
  __init__.py
  decisions.py      # workflow decision protocol and default policies
  events.py         # workflow event dataclasses and emitter protocol
  service.py        # download/split/organize orchestration
  views.py          # UI-safe view models for candidates, chapters, steps

muzik/core/beets/
  __init__.py
  config.py         # load beets config, plugins, Library
  decisions.py      # beets-specific decision protocol
  events.py         # beets import events
  importer.py       # MuzikImportSession and import_paths()
  views.py          # beets task/match/duplicate view models
  write.py          # tag-only/write support if not handled by import session

muzik/ui/cli/
  __init__.py
  decisions.py      # CLI prompts for workflow and beets decisions
  events.py         # Rich renderer for structured events

muzik/tui/
  __init__.py
  app.py            # Textual app
  screens.py
  widgets.py
```

This layout keeps terminal UI concerns out of `muzik.core` and keeps beets
internal objects isolated inside `muzik.core.beets`.

## Event and Decision Boundary

Add a small event API before moving large workflows.

Initial workflow event types:

- `MessageEvent`: user-visible status/log text.
- `StepStartedEvent`: named workflow step began.
- `StepFinishedEvent`: named workflow step completed.
- `ProgressStartedEvent`: determinate or indeterminate task started.
- `ProgressAdvancedEvent`: task progress changed.
- `ProgressFinishedEvent`: task ended.
- `CandidatesFoundEvent`: source candidates are available.
- `ChapterReviewRequestedEvent`: chapter set needs user review.
- `ErrorEvent`: recoverable or fatal failure.

Initial decision methods:

```python
class WorkflowDecisions(Protocol):
    def choose_soulseek_candidate(self, candidates: list[Candidate]) -> Candidate: ...
    def confirm_chapters(self, source: Path, chapters: list[Chapter]) -> bool: ...
    def edit_chapters(self, chapters: list[Chapter]) -> list[Chapter] | None: ...
```

Add beets-specific methods separately:

```python
class BeetsDecisions(Protocol):
    def should_resume_beets_import(self, path: Path) -> bool: ...
    def choose_beets_album_match(self, task: object) -> object: ...
    def choose_beets_track_match(self, task: object) -> object: ...
    def resolve_beets_duplicate(self, task: object, duplicates: list[object]) -> object: ...
```

Convert beets objects to local view models before exposing them outside
`muzik.core.beets`.

## Implementation Phases

### Phase 1: Workflow Boundary

Create the event and decision protocols and a CLI adapter. Keep behavior
unchanged while moving interactive choices behind the protocol.

Start with the smallest terminal blockers:

- Soulseek candidate selection in `muzik.commands.workflow`.
- Soulseek candidate selection in `muzik.commands.soulseek`.
- MusicBrainz/LLM chapter confirmation in `muzik.commands.workflow`.
- Chapter review/edit choice in `muzik.ui.chapter_editor`.

Acceptance criteria:

- Existing commands still behave the same in interactive CLI usage.
- Non-interactive mode can use deterministic policies.
- Unit tests can select candidates and chapters without monkeypatching
  `input()` or `typer.prompt`.

### Phase 2: Workflow Service Extraction

Move orchestration from `muzik.commands.workflow` into
`muzik.core.workflow.service`.

Keep Typer command signatures stable, but make `workflow_cmd` construct a
request/options object, CLI event renderer, and CLI decisions adapter, then call
the service.

Acceptance criteria:

- `muzik workflow` behavior and flags remain stable.
- Tests can invoke the workflow service without Typer.
- The service emits step, progress, candidate, chapter, and error events.

### Phase 3: Beets Core Boundary

Add `muzik.core.beets` and centralize all beets configuration, library opening,
and import options.

Implement:

- `open_library(config_path)`
- `ImportOptions`
- `MuzikImportSession`
- `import_paths(paths, options, decisions, events)`
- UI-safe view models for album matches, track matches, tasks, and duplicates.

Keep the current subprocess path as an escape hatch while the API integration is
being proven.

Acceptance criteria:

- Tests can exercise option translation without running `beet`.
- Beets global config/plugin state is isolated in `muzik.core.beets`.
- Only one beets import session is allowed at a time in-process.

### Phase 4: Replace `muzik organize`

Change `muzik.commands.organize` to call the internal beets service for default
import and `--import`.

Handle `--tag-only` through a dedicated write service or a well-contained
fallback if the beets API path is not complete yet.

Acceptance criteria:

- CLI flags stay stable: `--import`, `--tag-only`, `--dry-run`, `--config`.
- Dry-run maps to beets pretend behavior.
- Failures are surfaced as structured events and translated to Typer exits.
- The old passthrough path is still available behind an explicit fallback flag
  or internal emergency path during rollout.

### Phase 5: Replace `muzik import`

Change `muzik.commands.import_` to call the same beets service.

Preserve:

- Incremental import.
- `--copy`
- `--link`
- Default move behavior.
- `--nowrite`
- `--quiet`
- `--dry-run`
- `--config`

Acceptance criteria:

- Existing command-line behavior remains compatible.
- Quiet mode skips interactive decisions using policy choices.
- Import events expose enough state for a GUI progress/log view.

### Phase 6: Add Textual TUI

Add a `textual` dependency and register `muzik tui`.

Initial screens:

- Workflow launcher for URL/path, audio source, metadata source, quality, and
  common flags.
- Source candidate table for Soulseek.
- Chapter review/editor.
- Beets album/track match table.
- Duplicate resolution modal.
- Pipeline screen with step timeline, progress bars, and log pane.

Acceptance criteria:

- The TUI uses the same workflow and beets services as the CLI.
- Long-running work runs in Textual workers so the UI remains responsive.
- User decisions flow through the same decision protocol.

### Phase 7: Stabilize and Decide on Desktop

After the Textual TUI proves the workflow boundary, evaluate whether the app
needs a native desktop UI.

PySide6 is the preferred native option if packaging and desktop interaction
become top priority. Tauri/Electron should wait until the Python service layer
is stable and worth bridging.

## Migration Strategy

Keep each phase shippable:

- Preserve current CLI behavior first.
- Add adapters before replacing command behavior.
- Keep beets internals contained to one package.
- Add focused tests around decisions, event emission, and option translation.
- Use fallback paths during the beets API migration, then remove them once the
  internal importer is stable.

## Key Risks

- Beets importer APIs are semi-internal. Mitigate by pinning beets to the
  current `2.6.x` line and hiding beets objects behind view models.
- Beets config and plugin state are global. Mitigate by serializing imports.
- External tools remain required. The GUI should surface missing `yt-dlp`,
  `ffmpeg`, `ffprobe`, `slskd`, and browser automation dependencies clearly.
- Progress granularity varies. Use determinate progress where totals exist and
  indeterminate progress plus logs for lookup/import phases.
