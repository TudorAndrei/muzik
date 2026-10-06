# TODO: Deepen queue admission, watchlist writes, and device sync

## Phase 1: Atomic queue admission

- [x] Normal second pause: confirm one waiting row and a done running row (`run_job` calls `finish`).
- [x] Cancelled resumed job: confirm whether `park_on` can insert a waiting row before `reopen`; record the result in PLAN.md.
- [x] Add a failing single-connection test: a lower-id waiting row and a queued row; an explicit request reports busy and keeps the waiting row.
- [x] Put the admission check and writes in one immediate transaction in `crates/muzik-store/src/jobs.rs`; admission methods take `&mut self`.
- [x] Preserve explicit-request busy results, waiting-job replacement, and existing-ID results from `Store::enqueue`.
- [x] Replace caller admission sequences in `Jobs::item` and `run_refresh`; count only new refresh jobs.
- [x] Test competing admissions from two connections to one temporary database with a barrier; check that only one open job exists.
- [x] Test independent item keys and preserve existing active jobs without partial cancellation.
- [x] Test waiting replacement and rollback after a failed insertion.
- [x] Add the missing waiting-job case to the existing queue test.
- [x] Preserve pause, answer, resume, cancellation, and legacy import behavior.
- [x] Record that the two-connection race test does not fail reliably on the baseline; the partial-cancellation test is the baseline failure.
- [x] Pass `cargo test --locked -p muzik-store -p muzik-runner` and `mise run check`.
- [x] Commit: `fix(jobs): make queue admission atomic across processes`

## Phase 2: Atomic checked watchlist writes

- [x] Read the watchlist document and revision in one transaction through `Repository`.
- [x] Check the expected revision and write changes in one immediate transaction.
- [x] Return a conflict without changing data; preserve unchanged-document revision behavior.
- [x] Replace the caller write protocol in `WatchlistCheck::check`.
- [x] Preserve the three-attempt limit, busy checks, generation checks, and saved/checked events.
- [x] Run the full busy check before the write; hold `gate` around the conditional write and check only `current()` inside it.
- [x] Do not call `Jobs` while `gate` or a write transaction is held.
- [x] Test stale writes after concurrent source edits and stage changes using separate connections.
- [x] Test a successful retry and rollback after a write error.
- [x] Preserve the combined waiting-stage and question transaction in `watchlist_jobs.rs`.
- [x] Reproduce a stale overwrite against the baseline and verify that committed edits survive.
- [x] Pass `cargo test --locked -p muzik-store -p muzik-runner` and `mise run check`.
- [ ] Commit: `fix(watchlist): make checked writes atomic across processes`

## Phase 3: Device sync owns its execution conditions

- [ ] Keep the plan, target, and execution options private in `Prepared`.
- [ ] Give the CLI read-only preview access and consume the prepared state during apply; `apply` takes no second target or `jobs` value.
- [ ] Centralize the capacity decision for preview and execution in a private function that receives available space.
- [ ] Refresh target existence, needed bytes, remaining stale bytes, and available space before file changes.
- [ ] Preserve the current policy when available space cannot be measured.
- [ ] Adapt `apps/cli/src/sync.rs` and all existing sync test callers.
- [ ] Test missing-target refusal before file changes in `tests/run.rs`.
- [ ] Test insufficient and unknown space in a `#[cfg(test)]` module in `src/run.rs`; move the `fits` test from `tests/run.rs` there.
- [ ] Test a stale file removed between preview and apply.
- [ ] Preserve collision, encoding-change, unreadable-track, copy, and record-error coverage.
- [ ] Pass `cargo test --locked -p muzik-sync -p muzik-cli` and `mise run check`.
- [ ] Commit: `refactor(sync): own execution checks in the prepared sync`

## Verification

- [ ] No behavior change in CLI flags, output, dry runs, progress, item keys, saved formats, or GUI events, except the documented race fixes and execution refusals.
- [ ] Existing database migration tests pass; the schema version and stored formats remain compatible.
- [ ] Reopen a temporary database with job questions, watchlist stages, and sync encoding records; verify that the rows retain their values.
- [ ] Verify queue and watchlist transaction rollback after a controlled write failure.
- [ ] Verify code rollback on a database copy without a schema conversion; stop newer processes first.
- [ ] Smoke-test two `Jobs` instances on the same temporary database: request one item, retain or reject the second request, then cancel it.
- [ ] Smoke-test a local watchlist check with a concurrent source edit; verify the saved edit and current display result.
- [ ] Smoke-test sync with a temporary library and device directory: preview, copy, rerun, and deletion blocked by an unreadable track.
- [ ] Use `.tmp` and `Paths::under` for fixtures; keep real user data outside the test paths.
- [ ] Run `mise run test-scoped` on macOS and the full `mise run check` gate after the final phase.
- [ ] Tests assert current behavior or persistence contracts; no tombstone tests or public test-only hooks were added.

## Review

- [ ] Review each phase and its tests before its commit.
- [ ] Update PLAN.md and TODO.md before changing the phase scope.
- [ ] Mark each commit complete only after that commit succeeds.
- [ ] Verify that all phase commits are scoped conventional commits.
- [ ] Check all implementation and verification items before closing the work.
