# Plan: Deepen queue admission, watchlist writes, and device sync

## Goal

Put queue admission, conditional watchlist writes, and device sync checks in
the modules that own those operations. Callers will not have to combine
separate checks and writes to use those modules correctly. Implement all
three candidates from the 2026-10-06 review, with queue admission first.
The queue and watchlist races come from code inspection. Reproduce them
before describing them as tested failures.

This plan replaces the completed 2026-10-01 plan. Its decisions and checklist
remain in Git at `631b41a78780c2d57eae7ed3334dcb6193e74086:PLAN.md` and the
same revision of `TODO.md`.

## Approach

### Queue admission

`Jobs::item` in `crates/muzik-runner/src/queue.rs` checks open jobs, cancels
waiting jobs, and then calls `Store::enqueue`. `run_refresh` in
`crates/muzik-runner/src/runner.rs` has another check before enqueue.
`Store::enqueue` in `crates/muzik-store/src/jobs.rs` also checks, then inserts.
The mutex protects one `Jobs` instance. It does not protect another process.

Move the admission decisions behind the store interface. Use an immediate
SQLite transaction for the check and each related write. Distinguish these
existing caller policies and return enough information to count new jobs:

- An explicit item request reports busy when a queued or running job exists.
  If only waiting jobs exist, cancel them and insert the requested job in
  the same transaction. On failure, keep their questions and status.
- A source refresh retains an existing open item job. Count only new jobs
  in the refresh result.
- Direct `Store::enqueue` calls retain their existing behavior of returning
  an existing open job ID. Make that check and insertion atomic too.

Keep validation and user text in `Jobs`. Keep admission rules in the store.
Use the existing SQLite adapter and persistence seam. Do not add a generic
storage trait or a new crate.

The contract is atomic admission, not a unique row across every open status.
`park_on` can create a waiting row before `run_job` finishes the running row.
Preserve that transition and its shared transaction with the watchlist stage.
Do not add an open-job unique index. Do not remove existing duplicate rows as
part of this change. If active rows already exist, reject or retain them
according to the caller policy without partly cancelling waiting rows.

### Conditional watchlist writes

`WatchlistCheck::check` in `crates/muzik-runner/src/app.rs` owns a revision,
load, reconcile, lock, revision, and save sequence. `Repository::revision`
and `Repository::save` in `crates/muzik-store/src/watchlist.rs` use separate
connections. The process mutex cannot prevent a second process from writing
between the final revision check and save.

Extend the repository interface to load a document with its revision from
one read transaction. Add a conditional write operation in the same module.
It must start an immediate transaction, compare the expected revision, and
call the existing `write_changes` before it commits. Return a conflict
separately from a storage error. A conflict must make no data changes.

Keep `reconcile` outside the write transaction. In `WatchlistCheck::check`,
use the repository operations and retain the three-attempt retry limit,
busy checks, generation checks, and saved-card then checked-card events.
Check the generation before the commit and before publishing results.
Keep lock ordering consistent with `App::edit`; do not call back into `Jobs`
while holding a database write transaction. Revisions protect stored data;
the application generation still controls which result the GUI displays.

Keep `Repository::update_with` and the atomic stage-and-question write used
by `Operations::park`. Reuse normalization and changed-row detection so an
unchanged document does not increment the revision. Restrict lower-level
write operations only after their remaining callers have been checked.

### Device sync execution checks

`crates/muzik-sync/src/run.rs` already owns `select`, `prepare`, and `apply`.
Keep that module. `Prepared` currently exposes mutable execution fields,
and `apply` accepts a separate `Target`. The CLI alone checks `fits`.

Keep the prepared plan, target, and execution options together inside
`Prepared`. Give the CLI read-only access to the information it needs for
preview and progress. Apply a prepared operation once, using its own target
and options. Adapt `apps/cli/src/sync.rs` and existing test callers together.

Put the capacity decision in the sync module. The preview path must retain
the current insufficient-space error, including during a dry run. Before
`apply` removes or writes files, check that the target directory still
exists and refresh the capacity estimate. Recalculate needed bytes and the
space that the remaining stale files can release. Do not count a stale file
that has already disappeared. Preserve the current behavior when available
space cannot be measured. Keep error formatting and progress text in the CLI.

Retain deletion protection for unreadable tracks, destination collision
handling, encoding records, partial-file replacement, and separate transfer
and record errors. Capacity remains an estimate. This change does not reserve
space or guarantee that the device stays connected after a check.

### Data format and rollback

No phase needs a database schema migration or a new dependency. Keep the
`MIGRATIONS` list in `crates/muzik-store/src/db.rs`, stored item keys, job
statuses, watchlist JSON, and sync encoding records compatible.

Verify existing migration tests and reopen a temporary database containing
jobs, questions, watchlist state, and sync records after the changes. Verify
that a failed transaction preserves the previous rows. Code rollback needs
no database conversion. An older binary restores the earlier race risks;
stop newer processes before testing a rollback on a copy of the data.

### Scope limits

Preserve the decisions for the string decision callback, `Parked`, source
modules, and GUI-local commands. This plan does not change the GUI layout,
job scheduling, provider behavior, legacy import policy, or device formats.
It does not repair old duplicate jobs, guarantee file rollback after a
transfer fails, or detect device identity after a remount.

## Implementation Phases

Each phase includes its caller changes and tests. Run its focused checks and
`mise run check` before its commit. Update this plan and TODO.md before
splitting a phase. Mark the commit checkbox only after the commit succeeds.

### Phase 1: Atomic queue admission

- Extend `Store::enqueue` and the item admission operations in
  `crates/muzik-store/src/jobs.rs` to own the transaction and caller policy.
  Return whether admission inserted a job or retained an existing job.
- Replace the check-and-write sequences in `Jobs::item` and `run_refresh`
  with calls through that interface. Count only inserted refresh jobs.
- Keep `park_on`, `answer`, `reopen`, cancellation, and legacy import
  compatible with the existing pause and resume path.
- Extend the store tests with two connections to one temporary database.
  Cover competing admissions, different item keys, waiting replacement,
  and rollback if insertion fails after cancellation.
- Extend the current runner tests only for caller-specific behavior:
  explicit requests report busy, refresh retains a job, and pause, answer,
  and resume still work. The existing queue test name mentions a waiting
  job but does not create one; add that case to its fixture.
- Reproduce the admission race against the baseline. Use controlled
  connection contention and synchronization rather than sleep-based
  timing. Keep the regression at the owning interface. Do not add public
  test hooks or source-absence tests.
- Run `cargo test --locked -p muzik-store -p muzik-runner` and
  `mise run check`.

**Commit:** `fix(jobs): make queue admission atomic across processes`

### Phase 2: Atomic checked watchlist writes

- Extend `Repository` in `crates/muzik-store/src/watchlist.rs` with a coherent
  document-and-revision read and a conditional transaction write. Reuse
  `read_document`, normalization, and `write_changes`.
- Replace the external revision-lock-save sequence in
  `WatchlistCheck::check`. Keep reconciliation outside the transaction and
  preserve retry, busy, generation, and event behavior.
- Extend `crates/muzik-store/tests/watchlist.rs` to cover a concurrent source
  edit, a concurrent stage change, a stale write conflict, a successful
  retry, an unchanged document, and transaction rollback on a write error.
  Use separate connections to the same temporary database.
- Keep `crates/muzik-store/tests/watchlist_jobs.rs` coverage for the combined
  waiting stage and question write. Preserve application event coverage in
  `crates/muzik-runner/src/app.rs` and pause/resume coverage in `runner.rs`.
- Reproduce a stale overwrite against the baseline and confirm that the
  repository interface preserves the other writer's committed changes.
  Tests must check stored results, not the use of a mutex or SQL text.
- Run `cargo test --locked -p muzik-store -p muzik-runner` and
  `mise run check`.

**Commit:** `fix(watchlist): make checked writes atomic across processes`

### Phase 3: Device sync owns its execution conditions

- Make execution state in `Prepared` private in
  `crates/muzik-sync/src/run.rs`. Store its target and options there. Retain
  only the read access needed by the CLI. Update `apply` to consume that
  state without accepting a second target.
- Centralize the capacity decision. Use it for preview and before apply.
  Refresh the target, needed-byte, stale-file, and available-space checks
  before the first file change.
- Adapt `apps/cli/src/sync.rs` to the preview and execution interface. Keep
  dry-run output, progress, failure exit behavior, and record warnings.
- Adapt `crates/muzik-sync/tests/run.rs` to the real interface. Test refusal
  before deletion or transfer when space is insufficient or the target is
  gone. Test a stale file removed between preview and apply.
- Use temporary files and the existing SQLite and probe adapters. If a
  controlled capacity value is needed, keep that setup private inside the
  sync module's tests and call the same execution operation as production.
  Do not add a public dependency solely for tests.
- Preserve existing collision, encoding-change, unreadable-track, copy,
  and record-error coverage in `tests/run.rs` and `tests/sync.rs`.
- Run `cargo test --locked -p muzik-sync -p muzik-cli` and
  `mise run check`.

**Commit:** `refactor(sync): own execution checks in the prepared sync`

## Risks & Tradeoffs

- SQLite serializes writers. Keep transactions short and preserve the
  existing five-second busy timeout. Return storage errors without an
  unbounded retry loop.
- A process mutex can hide a concurrency defect in tests. Use independent
  connections and check committed results. A second process is needed when
  a test would otherwise share the same global mutex.
- Waiting and running rows can coexist during a pause. A broad unique
  constraint would reject valid work. Preserve this lifecycle explicitly.
- Generation and database revision have different purposes. Preserve both
  controls so a safe database write does not permit an old GUI result.
- Consuming `Prepared` changes the Rust interface. Update every in-repository
  caller in the same phase. Preserve the CLI behavior apart from the
  intended refusal when execution conditions have changed.
- Sync checks cannot prevent later filesystem changes or reserve capacity.
  Preserve existing transfer errors and partial-file handling.

## Open Questions

- None require a product decision before implementation. Method names and
  private helpers can follow existing naming when each phase starts.
- If a race cannot be reproduced with a reliable regression, record that
  limit and revise the verification step before declaring the fix tested.
- If implementation needs a schema change, update this plan with migration,
  data repair, and rollback steps before making that change.
