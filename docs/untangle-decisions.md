# Code structure decisions

The user selected Comments and Tests. The first review made no source changes.

## Fixes and evidence

- Give configured Beets paths one owner in core. Keep config discovery separate.
  - Import `beets.rs` and store `watchlist/library_lookup.rs` repeat home expansion and config-relative path rules.
  - Store rejects native path bytes that the library decoder supports.
  - Import sync and library pruning repeat the native SQL path decoder.
- Give import file rollback one private owner.
  - Singleton sync, album sync, and MusicBrainz ID fill repeat backup, write, and restore steps.
  - Make every backup before writes. Keep SQL transactions and field selection with each workflow.
- Give featured performer parsing one owner in core.
  - Import `ftclean.rs` and media `splitter.rs` repeat patterns and cleanup.
  - Keep their distinct artist formatting rules.
- Make quality measurement return its actual result type.
  - Media `quality::measure` cannot return an error. Its callers still handle that impossible result.
- Give GUI navigation one page list.
  - Main tabs and command palette repeat page names and order. Keep menu choices and shortcuts.
- Keep queue ownership with runner threads.
  - Runner releases its lock before detached workers finish. Shared state must retain the lock.
  - Shutdown must cancel active jobs. It must not block the UI on terminal input or model calls.
- Use the same release candidate sequence for model questions and answers.
  - Agent options filter missing IDs, but prompt numbering includes them. Preserve raw Soulseek indices.
- Split Bandcamp code by its reasons to change.
  - Cookie/login, collection access, transfers, and archive handling share one large file.
  - Keep public API and protocol/security tests.
- Remove comments that only repeat code in import files, workflow quality, media splitting, and match code.
- Remove `session_settings_defaults_leave_optional_fields_unset` last.
  - It constructs a struct and checks its own input. It calls no production behavior.
  - Keep connection, keychain, and profile tests.

## Requirements that remain

- Keep process polling for cancellation, timeouts, and queue changes from other processes.
- Keep paid service tests disabled by default.
- Keep retry rules for service limits, token refresh, concurrent database open, and watchlist revisions.
- Keep legacy format recovery, optional library reads, and native Unix path bytes.
- Keep archive containment, range validation, and keychain security checks.
- Keep cross-device move fallback, partial sync fields, repeated recording disambiguation, and ordered playlist checkpoints.
- Keep audio extension hints for ffmpeg and closed temporary files on Windows.
- Keep unknown-quality audio, Beets normalization rules, and external mapping order.
- Keep separate filename marker parsers: final generic IDs and embedded strict YouTube IDs have different contracts.
- Keep request settings behavior. A common schema is not justified by the current merge and default rules.

## Verification

Baseline: `cargo test --workspace --locked --offline`. The build needs the shared Rust cache outside the workspace. Results will be recorded after the run.

## Results

All listed fixes are complete. Three independent reviews found no further issue worth a change in the reviewed areas. The local dependency graph has 21 packages and no cycles. The first refactor added no package dependencies. Import no longer needs its direct `regex` dependency. The later lock-location fix uses the existing locked versions of `sha2` and `hex`.

Evidence:

- Baseline: 334 Rust tests and 192 feature scenarios passed; two paid or live service tests were skipped.
- Final: 337 tests passed with Nextest; the same two live service tests were skipped. All 192 feature scenarios passed.
- Format, strict workspace Clippy, documentation tests, documentation build, workflow validation, and dependency checks passed.
- Changed-file spelling and Markdown checks passed. The full spelling check reports existing changelog commit IDs.
- The candidate numbering regression test failed on the old code and passed after the fix.
- The native path lookup regression test failed with the old decoder and passed with the shared decoder.
- The runner test now uses channels to check shutdown cancellation, queue lock ownership, and queued job retention. It has no fixed sleep.
- The backup test checks successful writes and restoration of both files on error.

Test review:

- Removed one test that only checked a struct literal against its input.
- Kept format, migration, protocol, security, paid service, and compatibility tests.
- Changed Rust files contain 7,850 production lines and 2,088 test lines. The same files at the baseline contained 7,873 production lines and 1,973 test lines. This count separates test modules at their conditional compilation attribute.

Compatibility and limits:

- Public Bandcamp exports remain available. Fuzz selectors follow the moved tests.
- Quality measurement now returns `Option`; injected probes retain their real error contract.
- Store path lookup now preserves native Unix path bytes and shares import's home path rules.
- Runner shutdown retains queue ownership until workers stop. It does not wait for blocking terminal input.
- No live network account test or device UI check was run. No dependency upgrade was made.
- Existing third-party future compatibility and dependency notices remain outside this refactor.

## PR 31 review corrections

The PR review identified existing defects that the first structural review did not find. The fixes cover these contracts:

- Collection pages must contain an item array and a continuation flag. Continuing pages need a new, nonempty token. Repeated tokens stop with an error.
- Bandcamp requests check HTTPS origins before sending cookies. Cookie imports match domain boundaries. Existing cookie files receive private permissions before replacement.
- A cancelled download makes no initial page request. Resume requires the same purchase, strong ETag, and safe filename. A confirmed complete partial file can finish after HTTP 416.
- ZIP extraction checks entry count and expanded size. Download names reject drive prefixes. Purchase scans do not follow filesystem links.
- Downloads are extracted and checked in a temporary folder. A successful download replaces old content. A failed download preserves it. Failed replacement recovery retains the previous files and reports their location.
- Home expansion uses an absolute HOME or USERPROFILE, or returns an error. Empty library paths count as missing.
- Featured performer parsing preserves version text such as `(Remix)` in the title.
- Tag updates restore snapshots after returned errors and panic unwinding. Failed restoration retains snapshots and reports both saved and original paths.
- Cooperative tag updates lock canonical paths in a fixed order through snapshot, update, and restoration. Persistent hashed lock files in the library state directory keep lock identity stable between processes. Separate hard-link names and unrelated tag writers do not share this path-based coordination.
- Runner claim and registration hold the same mutex used by shutdown. An interrupted wait cannot miss a claimed but unregistered job.
- Quality documentation states the extension-only fallback. Tests cover successful probing, home expansion, malformed candidate exclusion, and database-only updates.

The review's general documentation coverage target is not a runtime contract. Documentation was added where it explains behavior and recovery. Comments that only repeat code remain excluded.

Verification: all 357 workspace tests and 192 feature scenarios passed. The final local HTTP test includes rejection of unknown-length partial responses. Strict workspace Clippy, formatting, spelling of changed files, documentation tests, documentation build, and dependency checks passed.

## Claude review corrections

The independent Claude review found five further issues. All five were verified and corrected:

- Featured-credit keywords need word boundaries. Ordinary titles such as `Left Behind` and `Soft Rain` now remain unchanged.
- Tag locks now use SHA-256 names under `muzik-locks` beside the library database. The backup owner receives the database path explicitly. Tag updates can write existing files in an album directory that is read-only, and the empty album directory can then be removed.
- The Bandcamp runner no longer deletes an old purchase before download. A failed `Download again` keeps the old audio.
- The first download-page lookup occurs after the cancellation check and outside the transfer retry loop. A permanent page or format error returns at once.
- The CLI checks all purchase folder names before downloading. It rejects identical names, case collisions, and names that become identical after filename cleanup. This preserves existing folder names without allowing one purchase to replace another.

The ordinary-title, permanent-lookup-error, and runner purchase-retention tests failed on the reviewed code before the fixes. A control run with album-side lock files also failed for the expected permission error; the restored state-folder implementation passed. The final suite covers conflicting purchase folders. All 362 workspace tests and 192 feature scenarios passed, as did strict Clippy, formatting, documentation and dependency checks. No live account or device UI test was run.
