# Track assignment data layout

## Scope and data

This change applies to `muzik-match::assign_items` only.
The input consists of local items and release tracks. The output consists of
matched index pairs and unmatched indices in beets order.
The solver returns unique indices within each input array. It matches all
indices on the smaller side when it succeeds. Errors still pass to the caller.
The public data types and file formats do not change.

ASSUMPTION: Equal item and track counts are common during album imports.
Real library size and frequency data were not available. This affects the
expected use of the path that skips membership checks.

The platform is an arm64 Mac with macOS 27.0.1. Array indices are `usize`.
The cost matrix is already a flat, row-major array. The full operation also
computes string distances and solves the assignment problem.

## Design and cost

Replace the two index hash sets with a shared batch operation that uses a
flat array of `u64` membership words. Each word holds 64 flags. Read item
records only to collect and sort unmatched output. Keep stable output order.
Return an empty output immediately for a fully matched side.
Reject an out-of-range solver index with `Error::InvalidPair` when building
the membership words. The early return relies on the solver index contract.

This removes both membership allocations for equal counts. For unequal
counts, only the larger side needs membership storage: eight bytes per
64 entries, rounded up. This cost excludes output buffers and allocator
metadata. No extra dependency or unsafe code is required.

The simplification pass removed membership work for fully matched inputs.
It kept the existing cost matrix and record layout. The cost loop reads
whole records, so a separate array for each field has no measured basis here.
No index tags or fixed-capacity buffers are needed by this change.

## Measurement contract

Baseline source: `80a1410`, before the data layout change.
Command: `cargo bench -p muzik-match --bench assignment --locked --offline`.
Both binaries use Rust 1.99.1 and the unchanged workspace release profile.
They use the same lockfile, fixtures, iteration counts, and output operation.
Each call creates fresh assignment state. Timing includes output destruction.
The inputs include four existing beets fixture cases and seven generated
boundary cases. Generated records have default field values. They are not a
sample of a user library.

Three runs of each binary used alternating order. Each run has seven timing
samples per input. Values below are nanoseconds per operation. Each entry
shows the median and the full sample range across the three runs.

Limits: This is a shared machine. Some test build work overlapped the timing
runs. The original baseline binary used direct division by the fixed positive
iteration count; the committed harness uses checked division for Clippy.
Both use the same denominator. The benchmark source and the production
layout were built in sequence in one checkout. These limits prevent a firm
latency claim. The first build and its timing samples were discarded.

- more files than tracks: before 6736 [6483, 7192]; after 6547 [6137, 8484].
- more tracks than files: before 7188 [6870, 7551]; after 7066 [6626, 7496].
- multiple discs: before 5196 [4960, 5463]; after 5121 [4802, 7850].
- equal cost tie: before 4677 [4421, 5004]; after 4612 [4362, 4724].
- generated 0x0: before 26 [24, 36]; after 19 [17, 43].
- generated 0x65: before 659 [641, 746]; after 651 [588, 1406].
- generated 65x0: before 744 [735, 891]; after 648 [597, 1049].
- generated 1x1: before 721 [688, 809]; after 670 [603, 766].
- generated 12x12: before 57699 [55802, 73203]; after 58702 [55337, 61058].
- generated 64x65: before 1.63138e+06 [1587774, 1787241]; after 1.65563e+06 [1589262, 1827678].
- generated 65x64: before 1.65828e+06 [1608583, 1912588]; after 1.67038e+06 [1629981, 1782444].

## Result and checks

Keep the compact membership storage and the fully matched early return.
The timing results do not establish a general speed increase. No application
latency or peak memory claim is made. A library workload and an isolated
machine are needed to measure that effect.

The original matching fixtures pass. The added public API test checks pairs,
unmatched output order, empty sides, and indices across 64-bit boundaries.
It passes with both the original and changed implementations.
Matcher tests, Clippy, and the documentation build pass with Rust 1.98.1.
The commit hook also runs workspace Clippy with all targets and features.

No cache, public format, dependency, or matching score changes are included.
The change is limited to transient membership data.
