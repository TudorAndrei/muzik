# GUI and Beets API Migration

This document captures the current direction for putting a GUI around `muzik`
and migrating beets integration away from subprocess calls.

## Current App Shape

`muzik` is currently a Python CLI built with Typer and Rich. It wraps external
tools and services:

- `yt-dlp` for YouTube metadata/downloads.
- `ffmpeg`/`ffprobe` for audio splitting and inspection.
- `slskd`/Soulseek for higher-quality audio acquisition.
- Bandcamp browser/session automation for collection downloads.
- `beet import`/`beet write` for library organization and tagging.

The code already has useful source-neutral models in
`muzik/core/sources/base.py`, including `DownloadRequest`, `Candidate`,
`ResolvedTrack`, `ResolvedRelease`, and `DownloadResult`. These are good
building blocks for a GUI because candidate selection and track decisions can be
represented as structured data instead of console text.

The main GUI blocker is not the visual framework. The workflow is still coupled
to terminal interactions:

- Rich output is emitted directly through `muzik/ui/console.py`.
- Some decisions use `input()` directly, such as chapter confirmation.
- Soulseek candidate selection currently uses `typer.prompt`.
- `organize` and `import` call `beet` as passthrough subprocesses so beets can
  own the terminal prompt.
- Progress is mostly terminal-specific Rich progress/live output.

Before a polished GUI can exist, the workflow needs a small event/decision
boundary.

## Recommended GUI Direction

### First Choice: Textual

Textual is the best initial fit.

Reasons:

- It is Python-native and aligns with the existing Typer/Rich codebase.
- It supports terminal UIs and can also be served in a browser.
- It has widgets that map well to this app: data tables, progress bars, logs,
  modals, selection lists, and background workers.
- It avoids introducing a JavaScript/Rust/frontend build stack before the
  domain workflow has been made UI-neutral.
- It is a good stepping stone: the same service/event layer needed for Textual
  would also support a later native desktop app.

Best initial GUI surface:

- A `muzik tui` command.
- Search/download panel for YouTube, Soulseek, Bandcamp, and local files.
- Candidate tables for Soulseek and beets metadata matches.
- Chapter review/editor view.
- Pipeline timeline: download, classify, split, organize.
- Progress bars for downloads, Soulseek transfer polling, ffmpeg split jobs,
  and beets import stages.
- Rich log pane for external tool output and warnings.

### Viable Later Option: PySide6 / Qt for Python

PySide6 is the best option if the goal becomes a true native desktop app.

Pros:

- Mature native widgets.
- Strong table/model-view support.
- Good thread/signal model for long-running imports/downloads.
- Better fit for a desktop music library manager if the UI grows large.

Cons:

- More verbose than Textual.
- Bigger rewrite.
- Less reuse of the current Rich/Typer mental model.
- Still requires the same internal workflow/event migration first.

Recommendation: do not start here unless native desktop packaging is the top
priority.

### Not Recommended First: Tauri or Electron

Tauri/Electron are not the right first move.

They could work later, but they add a frontend build stack and a backend
bridge while the Python workflow is still terminal-bound. We would still need
the same event/decision API in Python, so starting with Tauri/Electron adds
complexity before solving the real boundary problem.

### Possible But Less Ideal: NiceGUI

NiceGUI is a reasonable Python-only local web UI option. It may be useful if we
want a browser-first UI without writing frontend JavaScript.

It is less attractive than Textual for this repo because the app is already
Rich-oriented, CLI-first, and long-running-terminal-workflow shaped.

## Required Architecture Change

Introduce a UI-neutral workflow/service layer.

Instead of commands directly printing and prompting, long-running operations
should emit events and ask for decisions through callbacks/protocols.

Example event types:

- `message`: status/log text.
- `step_started`: workflow step began.
- `step_finished`: workflow step completed.
- `progress_started`: progress task created.
- `progress_advanced`: progress task advanced.
- `progress_finished`: progress task completed.
- `candidate_choices`: UI should present selectable source/beets candidates.
- `chapter_review`: UI should show/edit chapter list.
- `duplicate_found`: UI should resolve a beets duplicate.
- `error`: recoverable or fatal error.

Example decision methods:

```python
class WorkflowDecisions:
    def choose_soulseek_candidate(self, candidates): ...
    def confirm_chapters(self, source, chapters): ...
    def edit_chapters(self, chapters): ...
    def choose_beets_album_match(self, task): ...
    def choose_beets_track_match(self, task): ...
    def resolve_beets_duplicate(self, task, duplicates): ...
    def should_resume_beets_import(self, path): ...
```

The CLI becomes one adapter over this boundary. The GUI becomes another.

## Beets Migration Goal

The goal is to stop treating beets as an interactive subprocess and integrate
it through its Python importer/library APIs.

Current subprocess call sites:

- `muzik/commands/organize.py` calls `beet import` or `beet write`.
- `muzik/commands/import_.py` calls `beet import --incremental`.

Those should move behind a `muzik.core.beets` package.

Proposed structure:

```text
muzik/core/beets/
  __init__.py
  config.py       # load beets config, open Library, load plugins
  importer.py     # ImportSession subclass + import_paths()
  decisions.py    # CLI/GUI decision protocol
  events.py       # progress/log/task/candidate event types
  views.py        # beets task/match objects converted to UI-safe view models
```

## Beets Internal API Integration

Beets already has an importer pipeline that is separate from its terminal UI.
The key API is `beets.importer.ImportSession`.

The terminal UI implements a subclass called `TerminalImportSession`. We should
implement our own `MuzikImportSession` instead.

Important methods to implement:

- `should_resume(path)`
- `choose_match(task)`
- `choose_item(task)`
- `resolve_duplicate(task, found_duplicates)`

The methods return beets decisions:

- A selected `AlbumMatch` or `TrackMatch`.
- `beets.importer.Action.ASIS`
- `beets.importer.Action.SKIP`
- Other supported actions when appropriate, such as grouping albums or tracks.

Sketch:

```python
from beets import importer


class MuzikImportSession(importer.ImportSession):
    def __init__(self, lib, loghandler, paths, query, decisions, events):
        super().__init__(lib, loghandler, paths, query)
        self.decisions = decisions
        self.events = events

    def should_resume(self, path):
        return self.decisions.should_resume_beets_import(path)

    def choose_match(self, task):
        self.events.emit_beets_task(task)
        return self.decisions.choose_beets_album_match(task)

    def choose_item(self, task):
        self.events.emit_beets_task(task)
        return self.decisions.choose_beets_track_match(task)

    def resolve_duplicate(self, task, found_duplicates):
        decision = self.decisions.resolve_beets_duplicate(task, found_duplicates)

        if decision == "skip":
            task.set_choice(importer.Action.SKIP)
        elif decision == "keep_all":
            pass
        elif decision == "remove_old":
            task.should_remove_duplicates = True
        elif decision == "merge":
            task.should_merge_duplicates = True
        else:
            raise ValueError(f"unknown duplicate decision: {decision}")
```

Opening the beets library should also be centralized:

```python
from beets import config, plugins
from beets.library import Library
from beets.ui import get_path_formats, get_replacements


def open_library(config_path):
    config.set_file(str(config_path))
    plugins.load_plugins()

    lib = Library(
        config["library"].as_filename(),
        config["directory"].as_filename(),
        get_path_formats(),
        get_replacements(),
    )
    plugins.send("library_opened", lib=lib)
    return lib
```

Beets uses global config/plugin state, so imports should be serialized. Avoid
running multiple beets import sessions concurrently in one process.

## Beets CLI Behavior To Preserve

The migration should preserve existing command behavior first, then improve it.

`muzik organize` currently supports:

- Default import with auto-tagging and moving into the beets library.
- `--import`
- `--tag-only`
- `--dry-run`
- `--config`

`muzik import` currently supports:

- Existing library import.
- `--copy`
- `--link`
- Default move behavior.
- `--nowrite`
- `--quiet`
- `--dry-run`
- `--config`
- Incremental import.

The API migration should keep the CLI flags stable.

## Why This Helps The GUI

Once beets is internal, the GUI can show real beets state instead of embedding a
terminal.

Examples:

- Candidate table for album matches.
- Candidate table for singleton track matches.
- Track-by-track diff view: current title/artist/track number/duration vs
  proposed metadata.
- Duplicate resolution modal with old vs new items.
- Import progress by task.
- Write/move/import completion events.
- Non-interactive policies for unattended imports.

This also makes command-line behavior testable without driving an interactive
terminal subprocess.

## Migration Plan

### Phase 1: Add Beets Core Boundary

Add `muzik/core/beets/config.py` and `muzik/core/beets/importer.py`.

Deliverables:

- Open a beets `Library` from `BEETS_CONFIG`.
- Load plugins.
- Run an import session using a custom `MuzikImportSession`.
- Provide a CLI decision adapter that mimics current beets terminal prompts
  closely enough for existing workflows.

At the end of this phase, keep the old subprocess path available as a fallback.

### Phase 2: Replace `organize`

Move `muzik organize` from `run_passthrough(["beet", "import", ...])` to the
internal API.

Start with import/move/write behavior. Keep `--tag-only` working, either by
using beets item/library APIs directly or by routing through a dedicated write
service.

### Phase 3: Replace `import`

Move `muzik import` to the internal API.

Preserve incremental import behavior, copy/move/link options, quiet mode,
dry-run, and nowrite.

### Phase 4: Add Event Model

Add event emission around:

- Beets task creation.
- Candidate lookup completion.
- User decision requested.
- Duplicate found.
- Import task completed.
- File move/write operations where practical.

This can be done partly through the custom session and partly through beets
plugin events.

### Phase 5: Add Textual UI

Add `muzik tui`.

Initial screens:

- Workflow launcher.
- Source/candidate selection.
- Chapter review/editor.
- Beets candidate selection.
- Pipeline progress/log screen.

The Textual UI should call the same services used by the CLI.

### Phase 6: Consider Native Desktop

If Textual proves the workflow and the app needs a polished packaged desktop
experience, evaluate PySide6.

Do not start Tauri/Electron until the Python workflow is fully UI-neutral.

## Risks and Constraints

### Beets API Stability

The importer API is semi-internal. It is clearly designed for alternate
sessions, but the public documentation emphasizes the CLI.

Mitigation:

- Pin beets to `2.6.x` initially.
- Keep all beets imports and internal object handling inside `muzik.core.beets`.
- Convert beets objects to our own view models before handing them to GUI code.

### Global Beets State

Beets uses global config and plugin state.

Mitigation:

- Run one beets import session at a time.
- Avoid mutating beets config from multiple GUI workers.
- Treat beets import as a serialized background job.

### External Tools Still Matter

The GUI does not remove external tools.

Still needed:

- `yt-dlp`
- `ffmpeg`
- `ffprobe`
- `slskd`
- browser automation for Bandcamp setup

The GUI should make these statuses visible and actionable.

### Progress Granularity

Some progress is naturally granular, such as ffmpeg split count. Some is less
granular, such as beets candidate lookup.

Mitigation:

- Use determinate progress when totals are known.
- Use indeterminate progress/spinners for lookups and external operations.
- Preserve raw logs in a log panel for diagnostics.

## Recommendation

Build the internal beets API migration first, behind `muzik.core.beets`.

Then add a Textual GUI on top of the same workflow services. This gives the
best path to nicer decision-making and progress bars without committing early
to a heavy frontend stack. PySide6 remains the likely native desktop option if
the Textual version proves the workflow and a packaged desktop app becomes
worth the added complexity.
