# Isolated candidate experiments

This template is materialized under `.tmp` by the root preparation script.

This directory reads the repo's sample JSON. It does not read media bytes or
write to Music. Peer results, job rows, and watchlist states are generated from
the actual sampled metadata. They are not measured search or queue workloads.

The baseline ranking, upgrade selection, and watchlist paths call production
crates. snapshot.rs is an exact extraction of runner queue.rs snapshot, with
only the function name changed and imports/job_id supplied. The candidate
watchlist crate is a temporary copy. Its action map has a fixed array of eight
optional values, preserving sparse JSON map reads. Its Summary uses a fold.

Snapshot projection checks unused JSON validity in SQLite and validates queue
and kind enums to retain corruption fallback behavior without building JSON
DOMs for unused fields. This SQL validation is included in the timed path.

Run from this directory with Rust 1.98.1. The main agent must run the commands:

```text
cargo run --release --offline -- --check /absolute/path/to/tracks.json
cargo run --release --offline -- --bench /absolute/path/to/tracks.json
```

The workspace release profile uses opt-level z, LTO, one codegen unit, and
symbol stripping. Resolve a local lockfile before adding --locked. Use the
same Cargo target directory as the other experiments if needed for reuse.

--check compares exact serialized outputs, float score bits, winner and order.
It checks empty batches, sparse action maps, unknown action keys, corrupt job
JSON/enums, missing jobs table, equal winners, missing/empty parents, duration
boundaries, and all 7^5 stage-state combinations. It does not test concurrent
writers, network services, or real media processing. Do not treat a passing
check as proof of all arbitrary JSON parse behavior: SQLite json_valid and
serde_json can differ for extreme numeric literals. The overflowing-number controls confirm a projection contract failure. This candidate version is rejected.

--bench also runs --check. It prints one JSON object per line. Each candidate
has generated workloads of 20, 200, and 1000 records. Seven samples alternate
baseline and candidate order. The result includes per-operation ns, allocation
calls, and requested allocation bytes. Alloc/realloc calls use a temporary
System allocator wrapper. Counts are collected outside timing. Returned output
is dropped inside both paths. Input clones in ranking are included on both
sides. Counters are single-threaded and report total allocation traffic, not
peak retained memory.

The watchlist benchmark uses an empty directory under this experiment to
avoid reading actual Music media files. This isolates view construction and
includes equal empty filesystem probes. It is not a full watchlist-load test.

Source changes belong only in this directory. prepare.py can refresh copied
candidate modules from the current repository. It does not write repo files.
