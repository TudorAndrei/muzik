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

## Music sample experiments, 2026-11-02

### Data and method

The source was the read-only database at `~/Music/.library.db`. A SQLite
backup and metadata export stayed under `.tmp/dod-sample`. The sample has
5,586 tracks in 2,678 albums: 2,365 albums have one track, 34 have 2–4 tracks,
175 have 5–10, 91 have 11–20, 11 have 21–50, and two have more than 50.
The largest album has 120 tracks. The original database hash was unchanged
after the experiments. No audio file was changed.

Sixteen experiments cover all candidates from the survey and related small
variants. Baselines call production paths or extract the relevant source.
Candidates stay in generated private crate copies. The production source,
root dependency lockfile, and release profile did not change.

Rust 1.98.1, arm64 macOS 27.0.1, release `opt-level=z`, LTO, one code generation
unit. Two complete sequential runs each took seven alternating samples per
path. All builds finished before timing. The table shows the median and full
sample range in microseconds, plus median Rust allocation/reallocation calls.
Ratios compare equal work within each workload. They are kernel measurements.

Limits: this is a shared Mac, with warm local database reads. Peer results,
queue states, GUI lifecycle, and read-event sequences were generated from real
track descriptions. No live peer or GUI latency was measured. Allocation
counts exclude SQLite C allocations and do not measure peak retained memory.
The allocator wrappers add a flag read during allocation; gate timing also
includes counter increments. The toolchain reported a symbol-stripping warning,
but all binaries built. Both paths in each experiment use the same binary.

The matching group was measured again after an additional real tagged album
and full-album attribute comparisons were added to its test contract. The
final matching cases contain 1, 9, 99, and 120 tracks. The 99-track case has
real MusicBrainz IDs; the other three selected cases have none. The source
index ratio is 1.16× on that tagged case and 4.10× on the 120-track case
with empty IDs. This difference limits what the larger ratio means.
All 2,678 album-read outputs match, including the real custom attributes.

Twenty-two existing tests passed: ten matching fixture tests, six library
read/query tests, and six gate concurrency tests. Additional comparisons check
full database output, exact score bits, public ranking output, missing weights,
duplicate source indices, GUI order and lifetime, read completion order, sparse
action JSON, all 16,807 five-stage combinations, peer scores, upgrade winners,
and queue errors. The queue projection fails two numeric-JSON boundary cases.

### Decisions

- **Move owned attributes:** Time result is inconclusive: paired ranges cross no change. Ownership transfer removes copies with no public data change.
- **Batch album attribute reads:** Conditional. The largest album improves; the one-track case regresses. Do not batch every album without a size rule.
- **Inline held gates:** Defer. Generated repeated-suspension state shows a small effect; this does not establish a useful app-level gain. Timing includes allocator counter updates.
- **Prepare indexed weights:** Defer. Preparation is included once per matrix. No material benefit is established on this sample.
- **Project queue fields:** Reject this version. Overflowing JSON numbers change the error fallback for queued and waiting jobs. Valid-input timing does not override this failure.
- **Flat library snapshot indices:** Defer. No timing gain is established. Initial snapshot construction is excluded.
- **Album display summaries:** Conditional. Repeated view operations improve, but preparing summaries for one view is slower and allocates more.
- **Five read serial slots:** Useful small change. The event sequence is generated; the kernel ratio is not an app responsiveness claim.
- **Prepared Soulseek query:** Small measured effect, below the 20% timing target. Real peer traffic was not sampled.
- **Source-to-track index array:** Conditional. The 120-track case improves, but one-track cases regress. MBID output is checked; tag file writes are not measured.
- **Stack track penalties:** Defer. Allocation traffic decreases, but time ranges overlap. String processing remains in both paths.
- **Six summary counters:** Useful small change. Counts use generated summary states over real descriptions. Output strings remain equal.
- **Clone the upgrade winner:** Defer for speed. Allocation traffic decreases, but a timing improvement is not established. Download identity checks stay required.
- **Queue-only watchlist updates:** Prioritize with the shared-snapshot change. Eight generated queue updates are included. The initial table is built in both paths.
- **Watchlist row indices:** Prioritize. Output content and order match, and snapshot replacement rejects stale indices. The GPUI event loop is not measured.
- **Fixed actions and stage fold:** Memory candidate. Requested Rust allocation bytes decrease; this is not a peak-memory measurement. Action JSON shape and sparse input behavior are checked.

The strongest next change is one shared watchlist snapshot, row indices, and
queue-only flag updates. These are related changes; their ratios must not be
multiplied. The small read and summary arrays are also supported by these
kernel tests. Large-album changes need a size rule because the real library
is dominated by one-track albums. A further experiment must establish that
rule before a general implementation.

No candidate has been applied to production by this test pass. The scripts
and aggregate numeric results are saved under `scripts/perf/`. Private sample
data stays ignored.

### Complete measurements

Before and trial values are microseconds per operation: median [minimum, maximum].
A ratio above 1 means less elapsed time for the trial. It is not a confidence
interval or a claim about the whole app. Allocation counts include reallocations.

| Candidate | Workload | Before µs [range] | Trial µs [range] | Ratio | Rust allocations before → trial |
| --- | --- | ---: | ---: | ---: | ---: |
| Move owned attributes | all-albums | 13684.542 [13407.875, 15599.916] | 12244.458 [11971.333, 14114.292] | 1.12× | 146700 → 146432 |
| Move owned attributes | all-items | 78514.938 [76111.958, 82920.959] | 68618.292 [67003.167, 72337.083] | 1.14× | 729301 → 726816 |
| Batch album attribute reads | largest | 1802.375 [1759.638, 1844.250] | 1411.340 [1382.097, 1443.861] | 1.28× | 15246 → 15126 |
| Batch album attribute reads | small | 57.986 [54.264, 68.833] | 62.882 [58.277, 65.166] | 0.92× | 132 → 131 |
| Batch album attribute reads | tagged | 1573.249 [1540.361, 1669.416] | 1236.597 [1185.861, 1294.028] | 1.27× | 13371 → 13272 |
| Batch album attribute reads | typical-multi | 174.715 [167.833, 186.777] | 154.715 [146.055, 160.361] | 1.13× | 1146 → 1137 |
| Inline held gates | depth0_rounds1 | 0.552 [0.528, 0.571] | 0.534 [0.516, 0.553] | 1.03× | 18 → 18 |
| Inline held gates | depth0_rounds100 | 2.122 [1.942, 2.360] | 1.786 [1.775, 1.803] | 1.19× | 18 → 18 |
| Inline held gates | depth1_rounds1 | 0.757 [0.730, 0.799] | 0.711 [0.701, 0.748] | 1.06× | 25 → 23 |
| Inline held gates | depth1_rounds100 | 16.886 [16.697, 17.231] | 15.691 [15.197, 18.875] | 1.08× | 421 → 320 |
| Inline held gates | depth2_rounds1 | 0.897 [0.882, 0.957] | 0.854 [0.836, 1.185] | 1.05× | 28 → 26 |
| Inline held gates | depth2_rounds100 | 27.073 [26.649, 27.708] | 25.097 [24.630, 25.830] | 1.08× | 523 → 422 |
| Inline held gates | depth3_rounds1 | 1.042 [1.016, 1.081] | 1.001 [0.968, 1.044] | 1.04× | 31 → 29 |
| Inline held gates | depth3_rounds100 | 37.268 [36.648, 40.294] | 34.955 [34.437, 35.727] | 1.07× | 625 → 524 |
| Prepare indexed weights | largest | 37969.166 [37031.750, 39163.125] | 37360.208 [36643.416, 38128.083] | 1.02× | 557399 → 557399 |
| Prepare indexed weights | small | 5.783 [5.616, 6.458] | 5.766 [5.641, 8.941] | 1.00× | 52 → 52 |
| Prepare indexed weights | tagged | 16088.791 [15844.250, 16486.334] | 15733.562 [15460.125, 16067.625] | 1.02× | 376041 → 376041 |
| Prepare indexed weights | typical-multi | 164.417 [162.475, 175.450] | 162.204 [159.025, 167.950] | 1.01× | 3234 → 3234 |
| Project queue fields | jobs-1000 | 7084.050 [6825.705, 7341.505] | 3973.438 [3860.090, 4721.209] | 1.78× | 119396 → 55678 |
| Project queue fields | jobs-20 | 142.357 [138.383, 151.072] | 84.035 [81.729, 87.547] | 1.69× | 2289 → 1047 |
| Project queue fields | jobs-200 | 1417.365 [1388.093, 1566.970] | 804.966 [782.180, 928.364] | 1.76× | 24099 → 11183 |
| Flat library snapshot indices | full | 11129.146 [10801.875, 17102.875] | 11401.021 [11008.792, 13206.166] | 0.98× | 341077 → 341066 |
| Flat library snapshot indices | small | 69.000 [67.842, 102.208] | 70.333 [68.933, 100.350] | 0.98× | 2331 → 2327 |
| Album display summaries | full | 10880.500 [10482.166, 11876.334] | 6908.708 [6665.417, 7430.917] | 1.57× | 341077 → 192987 |
| Album display summaries | full_cold1 | 418.041 [397.625, 499.125] | 482.250 [448.333, 501.292] | 0.87× | 16551 → 21897 |
| Album display summaries | small | 69.542 [66.658, 194.333] | 32.929 [31.542, 36.225] | 2.11× | 2331 → 1174 |
| Album display summaries | small_cold1 | 3.258 [3.183, 3.392] | 3.654 [3.542, 3.783] | 0.89× | 120 → 155 |
| Five read serial slots | full | 2959.938 [2881.166, 3137.000] | 171.666 [167.792, 197.167] | 17.24× | 16 → 15 |
| Five read serial slots | small | 34.950 [33.575, 69.867] | 2.321 [2.242, 5.192] | 15.06× | 9 → 8 |
| Prepared Soulseek query | peers-1000 | 7207.436 [7014.164, 7605.510] | 6529.901 [6405.031, 7133.416] | 1.10× | 91329 → 79430 |
| Prepared Soulseek query | peers-20 | 144.002 [141.625, 152.362] | 133.649 [130.400, 137.547] | 1.08× | 1936 → 1709 |
| Prepared Soulseek query | peers-200 | 1339.429 [1304.900, 1354.245] | 1203.015 [1181.435, 1508.141] | 1.11× | 17315 → 14944 |
| Source-to-track index array | largest/reversed | 2.723 [2.695, 3.216] | 0.664 [0.654, 0.995] | 4.10× | 1 → 2 |
| Source-to-track index array | largest/unmatched-and-duplicate | 2.081 [2.066, 2.362] | 0.625 [0.612, 0.800] | 3.33× | 1 → 2 |
| Source-to-track index array | small/reversed | 0.035 [0.033, 0.050] | 0.052 [0.045, 0.070] | 0.67× | 1 → 2 |
| Source-to-track index array | small/unmatched-and-duplicate | 0.037 [0.033, 0.041] | 0.050 [0.045, 0.054] | 0.74× | 1 → 2 |
| Source-to-track index array | tagged/reversed | 12.254 [12.166, 12.875] | 10.583 [10.512, 11.475] | 1.16× | 499 → 500 |
| Source-to-track index array | tagged/unmatched-and-duplicate | 9.425 [9.304, 9.841] | 8.554 [8.458, 8.687] | 1.10× | 401 → 402 |
| Source-to-track index array | typical-multi/reversed | 0.087 [0.079, 0.116] | 0.095 [0.091, 0.270] | 0.92× | 1 → 2 |
| Source-to-track index array | typical-multi/unmatched-and-duplicate | 0.083 [0.075, 0.087] | 0.091 [0.083, 0.104] | 0.91× | 1 → 2 |
| Stack track penalties | largest | 37518.688 [37270.250, 37882.333] | 36938.229 [36641.417, 38289.542] | 1.02× | 557399 → 499799 |
| Stack track penalties | small | 5.804 [5.400, 6.600] | 5.716 [5.375, 6.075] | 1.02× | 52 → 49 |
| Stack track penalties | tagged | 16074.688 [15907.500, 16180.083] | 15201.791 [15070.666, 15460.833] | 1.06× | 376041 → 307434 |
| Stack track penalties | typical-multi | 165.112 [161.625, 169.716] | 161.841 [158.233, 171.683] | 1.02× | 3234 → 2910 |
| Six summary counters | full | 55.250 [54.292, 71.375] | 4.000 [3.833, 4.209] | 13.81× | 23 → 21 |
| Six summary counters | small | 1.350 [1.325, 1.725] | 0.542 [0.517, 0.608] | 2.49× | 23 → 21 |
| Clone the upgrade winner | peers-1000 | 11707.736 [11374.610, 12655.637] | 11803.009 [11302.733, 13784.564] | 0.99× | 200423 → 152467 |
| Clone the upgrade winner | peers-20 | 253.476 [249.545, 344.283] | 261.291 [257.918, 275.193] | 0.97× | 4201 → 3317 |
| Clone the upgrade winner | peers-200 | 2132.131 [2106.133, 2255.620] | 2087.535 [2065.316, 2136.018] | 1.02× | 37575 → 28051 |
| Queue-only watchlist updates | full | 77427.208 [73671.500, 118218.542] | 3585.251 [2697.791, 7586.458] | 21.60× | 1.43923e+06 → 13983 |
| Queue-only watchlist updates | small | 554.562 [492.242, 1198.758] | 26.842 [25.508, 32.092] | 20.66× | 16704 → 168 |
| Watchlist row indices | full | 9422.624 [9017.958, 13658.000] | 1859.500 [1205.375, 2021.625] | 5.07× | 179904 → 13983 |
| Watchlist row indices | small | 61.812 [58.583, 90.317] | 9.679 [8.992, 17.625] | 6.39× | 2088 → 168 |
| Fixed actions and stage fold | items-1000 | 2396.060 [2301.693, 2761.695] | 2233.840 [2118.962, 2849.387] | 1.07× | 44021 → 42021 |
| Fixed actions and stage fold | items-20 | 58.738 [53.941, 75.335] | 54.543 [50.318, 66.833] | 1.08× | 871 → 831 |
| Fixed actions and stage fold | items-200 | 482.252 [462.379, 669.470] | 441.480 [424.266, 781.331] | 1.09× | 8894 → 8494 |
