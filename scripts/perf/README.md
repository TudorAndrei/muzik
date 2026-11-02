# Data-oriented design experiments

These experiments test the candidates from the code survey at `5f4acbe`.
They do not change production source or write to the original music library.
Candidate source is generated under `.tmp/dod-*` from the current source.
The source extractors stop when an expected code pattern is absent.

Use the same Rust toolchain for every build. The recorded run used Rust 1.98.1
on macOS arm64. All groups use the workspace release settings: `opt-level=z`,
LTO, one code generation unit, and symbol stripping.

```sh
python3 scripts/perf/sample.py ~/Music/.library.db
python3 scripts/perf/run.py check --build
python3 scripts/perf/run.py bench --repeat 2
python3 scripts/perf/summarize.py
python3 scripts/perf/chart.py
```

The database copy, track descriptions, generated candidate crates, build
outputs, and raw logs stay under the ignored `.tmp` directory. Do not add the
sample database or metadata files to Git. The sample command opens the source
through SQLite read-only mode and uses the backup API. The copied database is
also marked read-only. No audio writes or network calls are part of this test.

`run.py` builds all binaries before timing any group. It runs the groups in
sequence. Each benchmark takes seven samples with alternating before/after
order. A second complete run uses the reverse group order. Allocation counts
come from separate untimed passes. Benchmark-only allocator wrappers forward
pointers and layouts to the system allocator unchanged. They count Rust
allocation and reallocation calls, not SQLite C allocations or peak memory.
The wrappers read a counter flag during timed allocation calls, which is a
remaining measurement cost. Gate timing uses an always-on counter in both
paths; its result needs that additional limit.

`check` runs comparisons against the current implementation. A completed run
can include a rejected candidate: inspect the `boundary` records. The queue
projection fails its overflowing-JSON-number control. Its valid-input timing
is recorded for comparison, but that version must not be used in production.

The copied matching crates also support the existing public fixture tests:

```sh
CARGO_TARGET_DIR="$PWD/.tmp/dod-target" cargo test --offline \
  --manifest-path .tmp/dod-match/Cargo.toml \
  -p trial-match-stack -p trial-match-weights \
  --test ranking --test distance --test string_distance
CARGO_TARGET_DIR="$PWD/.tmp/dod-target" cargo test --offline \
  --manifest-path .tmp/dod-match/Cargo.toml \
  -p trial-library-move -p trial-library-batch --test read --test query
CARGO_TARGET_DIR="$PWD/.tmp/dod-target" cargo test --offline \
  --manifest-path .tmp/dod-gates/Cargo.toml
```

## Coverage and limits

- Library reads use the copied real database. They compare complete fields,
  attribute values, IDs, and order. Small, median multi-track, and largest
  album cases come from that database. Reads are warm local database reads.
- Match trials use real titles, artists, durations, IDs, and disc/track data.
  They compare exact score bits, assignment output, and missing-weight errors.
  Indexed weights include preparation once per matrix and retain the public
  configuration map. Source-index timing covers the public MBID-fill path;
  it does not measure tag file writes.
- GUI trials extract pure table code. They compare filters, order, content,
  queue flags, stale index rejection, and snapshot lifetime. They do not
  measure window rendering, input latency, or a live GPUI event loop.
- Library summaries include construction once per operation sequence. The
  one-view case and repeated 19-operation case are separate. Library indices
  measure repeated view operations and exclude initial snapshot construction.
- Peer results, job states, queue changes, source IDs, action states, and read
  events are generated from real descriptions. They are not captured live
  service traffic. The summary test checks all 16,807 five-stage combinations.
- Watchlist view construction uses an empty scratch directory. This keeps
  filesystem probes equal and avoids scanning the original music folders.
- Gate trials use extracted production transitions. Both versions pass the
  existing concurrency tests. Inline storage uses ArrayVec in the private
  experiment only; no production dependency is added.

`results/data-oriented.json` contains all 56 measured workloads, 14 samples per
path, allocation counts, paired timing ratios, and failed boundary controls.
Its values are safe to save: no track descriptions or media paths are included.
See the dated section in `perf-log.md` for the decisions. No result establishes
application-wide speed or peak-memory use.
