# GUI data layout experiments

These files are an experiment. They do not change the repository or the music files.
`setup.py` extracts the current production library row, table transform, watch row,
row construction, sort, and Read enum into local modules. No GPUI dependency is
required. The modules include only pure data code and small experiment wrappers.
The read and CLI count kernels are direct copies of their map operations.

No build, correctness run, or timing run was made by the agent that prepared this
harness. The main agent must inspect and run it.

## Run

Use Rust 1.98.1. The local manifest matches the workspace release settings:
opt-level=z, LTO=true, codegen-units=1, strip=symbols. Use offline dependency
resolution. All build and lock files belong inside this experiment directory.

```sh
cd @LAB@
python3 setup.py
cargo build --release --offline
./target/release/muzik-dod-gui-experiment --check @ROOT@/.tmp/dod-sample/tracks.json
./target/release/muzik-dod-gui-experiment --bench @ROOT@/.tmp/dod-sample/tracks.json all both
```

The last two arguments can select one candidate and one size (`small`, `full`, or
`both`). Candidate names:

- `library_summaries`
- `library_indices`
- `watch_rows_indices`
- `watch_queue_only`
- `read_serial_array`
- `summary_count_array`

`--check` runs output comparisons only. `--bench` runs those checks before timing.
It emits seven alternating samples per side, with JSONL output. Allocation calls
are counted in a separate untimed pass after each timed pass. Output destruction
is inside each measured operation. The allocator delegates directly to System;
its unsafe code is local to this experiment. The disabled counter still adds a
single atomic flag load to allocation calls in the timing pass.

The small input is the first 64 sample tracks. The full input uses every sample
track. Their exact track and album counts are emitted in the check records.
No metadata values or music paths are printed in normal output.

## Measurement contracts and limits

These are kernel measurements. They do not measure GPUI rendering latency.

- Library summaries: For each operation sequence, the candidate first constructs
  summaries, then applies the same one-view or 19-operation sequence and emits the same
  row projections as the baseline. Summary setup and destruction are included.
  This is a generated UI operation sequence over real album and track metadata.
  Total length uses the same sum order. Format text uses the same sorted set.
- Library indices: Both representations are prepared before timing. The baseline
  has the production per-album Arc layout; the candidate has a shared contiguous
  snapshot and sorts/filter indices. Full records, track vectors, and projection
  output are equal. This measures repeated view operations. It excludes the cost
  of creating either source representation and cannot prove a library-load or
  total peak-memory gain. The album-sheet lifetime check keeps the old snapshot
  alive across replacement.
- Watch rows: ItemView and ItemId are the real production types. Descriptive
  payloads come from the sample. Source IDs, queue state, availability, summary,
  primary actions, and event order are GENERATED. Candidate and baseline resolve
  exactly equal ItemView records, keys, and queued flags. Row creation compares
  deep copies with snapshot indices. Queue-only updates compare eight baseline
  row rebuilds with one indexed build and seven flag updates. This is not a
  sample of a real saved watchlist.
- Read serials: The five-kind event script is GENERATED. Its length follows the
  sample size, but this is not a measured distribution of GUI read requests.
  Both versions preserve wrapped serial increments and ignore stale completions.
  Checks include reversed completion of one kind and a distinct concurrent kind.
- Summary counts: The real six-state Summary enum and CLI output formatting are
  used. Counts are based on the GENERATED summary states in the watch rows.
  Empty output and all state counts have equal observable text.

The watch snapshot check replaces the snapshot, changes the playlist identity,
removes an item, and reverses the input. Old-generation indices are rejected;
selection is compared by ItemId. Sort and filter checks cover all Summary values,
default filtering, position/title/status order, and both sort directions.

A speed result from these kernels is a hypothesis for a source change. Keep the
original behavior tests and measure end-to-end UI work before a release claim.
