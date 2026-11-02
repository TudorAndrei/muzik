# Private matching experiments

No source files or sample files in the repository are changed.
`setup.py` creates five crate copies in this directory. It copies the source
from the current checkout. It changes only the trial code described below.
It does not build or run the binary.

Prepare:

```sh
python3 @LAB@/setup.py @ROOT@
```

Build and run (the main agent runs these commands):

```sh
RUSTUP_TOOLCHAIN=1.98.1 cargo build --release --offline --manifest-path @LAB@/Cargo.toml
@LAB@/target/release/muzik-dod-match-experiments --check --sample @ROOT@/.tmp/dod-sample
@LAB@/target/release/muzik-dod-match-experiments --bench --sample @ROOT@/.tmp/dod-sample
```

The initial build resolves the private package names into the copied lockfile.
Use `--locked` for later builds after the lockfile has been resolved.
The profile uses opt-level=z, LTO, one codegen unit, and symbol stripping.
Do not run benchmarks in parallel with other builds or benchmarks.

The input files are `library.db`, `tracks.json`, and `match_items.json`.
The two JSON arrays must have the same order. `tracks.json` has numeric
`album_id`; `match_items.json` contains MatchItem records. The binary selects
the smallest album, the median album among albums with more than one item,
the largest album, and a tagged album with real track IDs. It reports the sizes without printing private fields.
These are selected input cases, not a workload-weighted application result.

The five trials are:

- `attribute-ownership`: real Library::items and Library::albums against the
  same production bodies with map removal instead of attribute cloning.
- `batch-album-attributes`: real Library::items_for_album against the same
  body with one restricted attribute query for the selected album.
- `stack-track-score`: real track_distance().score() for all matrix cells
  against an exact copy of the track penalty body with seven inline slots.
  The trial keeps the two score passes and their float order. Compare output
  float bits and error text. The string-distance code is unchanged. The copied public rank path uses
  the inline matrix builder, so public ranking checks exercise this change.
- `indexed-weights`: real track_distance().score() against an internal
  twenty-slot weight array prepared once per cost matrix. Preparation is
  included in timing. The public MatchConfig map remains unchanged. The
  public ranking path uses the prepared matrix builder. max_rec is unchanged.
- `source-index-array`: real mbid_fields against an exact copy with only the
  source-track lookup changed. It compares the full album and item field
  maps. It covers reversed pairs and sparse pairs, a duplicate source, an
  invalid release-track index, and an invalid source index. First pair wins.

Check mode compares exact full outputs. Matrix results compare f64::to_bits.
Full public rank checks compare assignment pairs, extra indices, recommendations,
penalty float bits, and total score float bits. A missing TrackTitle weight
supplies an error control for both score trials.
The trials do not claim equivalence for all possible configurations. Further
checks are needed for unusual nonfinite weights and future penalty fields.
No original media writes, tag writes, or network calls occur. Database reads use the private copy.

Each benchmark case runs seven samples, in alternating before/after order.
Both sides use the same iteration count and produce the same output. The
binary emits one JSON object per line with sample_ns per operation.
Allocation counts include alloc, alloc_zeroed, and realloc. Requested bytes
are cumulative allocator request sizes, not peak memory or retained memory.
The counting System allocator uses unsafe code only in this private binary.
COUNTING is enabled only for separate untimed allocation passes after both
paths are warmed. It is disabled during timing. Per-sample allocation fields
reuse those untimed per-operation counts. The allocator still reads the
COUNTING flag on each request during timing; it does not update counters.
The --check command emits the same warm allocation counts.

Limits: The three album cases contain local metadata on both matrix axes;
they are not sampled remote candidate releases. The source-index trial
measures the complete public MBID-field transform, not file writing. Most
source albums may have one track; do not infer application speed from the
largest album. SQLite runs use the system file cache after the first reads.
Malformed SQLite table schemas are not tested. Preserve public configuration
and error compatibility before moving a trial into production.

The indexed-weights timing measures reuse of configuration storage prepared
once before the matrix call, as MatchConfig is prepared in production. It
does not include parsing or cold preparation. The cost of a separate
prepared array created per rank call is not measured by this trial. A
production change that uses that design needs an additional preparation
benchmark. The public rank checks include configuration preparation for
correctness, but are not timed.

All album-read outputs are compared outside timing, including real custom attributes. The tagged case uses real MusicBrainz IDs. Invalid-first duplicate mappings are checked outside timing.
