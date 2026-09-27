# Plan: Port the beets functionality of muzik to Rust

## Goal

Replace every part of beets that muzik uses with native Rust library crates,
then remove `beets` from `pyproject.toml`. The parts are album matching,
MusicBrainz lookup, tag read/write, the library database with queries, path
formats, file operations, and the import pipeline. The crates are reusable
Apache-2.0 libraries, not only muzik internals. The migration is a strangler:
each component runs in shadow mode next to beets, then replaces it, one switch
at a time.

## Approach

### Host and seam

Python stays the workflow host. It still runs `yt-dlp`, Playwright (Bandcamp),
pydantic-ai (agent decisions), and the JSON-lines service for the GPUI app
(`muzik/native_gui/server.py`, `rust/gpui_app/src/bridge.rs`). All beets
functionality moves to Rust, and Python calls it through one PyO3 extension.

The pasted design's `muzik-beets` subprocess adapter is not built. A per-component
`beets`/`shadow`/`native` switch in Python gives the same fallback with less
code, and it disappears with beets in Milestone 6. The Rust crates have no Python
dependency, so the GPUI app or a future Rust host can link them directly.

### Rust ecosystem conventions

- **Binding crate:** `rust/seakarr_bridge` becomes `rust/crates/muzik-py`, a thin
  PyO3 crate. The Python module becomes `muzik._native`. This follows the
  pydantic-core (`_pydantic_core`) and polars (`polars-python`) pattern: logic in
  plain crates, bindings in one `-py` crate, private module name with `_`.
- **Soulseek logic** moves out of the binding crate into `muzik-soulseek`.
- Package names use kebab-case with the `muzik-` prefix.
- Each crate has one `thiserror` error enum and emits `tracing` events. No crate
  installs a subscriber.
- All APIs are blocking. `soulseek-rs-lib` is synchronous, and PyO3 0.29
  `Python::detach` releases the GIL around long calls. No tokio.
- All new crates: `license = "Apache-2.0"`, `publish = false`, a `LICENSE`
  file, and a `NOTICE` file with the MIT notices of ported code (beets,
  mediafile, lsap).

### Workspace layout

```text
rust/
├── Cargo.toml / Cargo.lock / deny.toml
├── crates/
│   ├── muzik-core/       domain types, beets config loading
│   ├── muzik-match/      distance, assignment, ranking      (beets.autotag)
│   ├── muzik-metadata/   MusicBrainz client, candidates     (beets musicbrainz plugin)
│   ├── muzik-tags/       tag read/write, probe, cover art   (mediafile, embedart, fetchart)
│   ├── muzik-library/    SQLite items/albums, queries       (beets.library, dbcore)
│   ├── muzik-import/     templates, file ops, plan/apply    (beets.importer, mbsync)
│   ├── muzik-soulseek/   moved from seakarr_bridge
│   └── muzik-py/         PyO3 bindings → muzik._native
└── gpui_app/             outside the workspace (own lock, pinned gpui-kit)
```

### Crate selection

Researched on 2026-09-27 with crates.io and GitHub data, and three local tests.

| Need | Crate | License | Notes |
|---|---|---|---|
| Transliteration | `deunicode` 1.6 | BSD-3 | Same result as Python `unidecode` on 21/23 test strings after beets normalization. Python Unidecode is GPL: do not copy its tables. |
| Levenshtein | `strsim` 0.11 | MIT | Same result as `jellyfish` on ASCII input. |
| Assignment | `lsap` 1.0 (vendored) | MIT | Port of scipy's solver. `f64`, rectangular. Same minimum cost as `lap.lapjv` on 300 random matrices. About 250 lines, not released since 2023: vendor it into `muzik-match`. |
| Regex | `regex` 1.13; `fancy-regex` 0.19 | MIT/Apache, MIT | `fancy-regex` only for user regexes (`field::re` queries, `replace:` rules), which follow Python `re` syntax. |
| Tags, probe | `lofty` 0.25 | MIT/Apache | All formats muzik uses. Custom keys go through `Id3v2Tag::insert_user_text`, MP4 freeform atoms, and `VorbisComments`. `FileProperties` gives duration, bit rate, sample rate, bit depth. |
| MusicBrainz | `musicbrainz_rs` 0.14, `sync` feature | MIT | Active. Release lookup with recordings, media, artist credits, labels. Choose MIT for its `api_bindium` dependency. |
| HTTP, limits | `ureq` 3.4, `governor` 0.10, `backon` 1.6 | MIT/Apache, MIT, Apache | Blocking. MusicBrainz needs 1 request per second. |
| SQLite | `rusqlite` 0.40 (`bundled`, `functions`), `rusqlite_migration` 2.6 | MIT, Apache | `functions` registers the `regexp`/`unidecode`/`bytelower` SQL functions that beets uses. |
| Parsing | `winnow` 1.0, `shlex` 2.0 | MIT, MIT/Apache | Hand-written query and template parsers. No crate parses beets templates. |
| File ops | `reflink-copy` 0.1, `trash` 5.2, `std::fs` | MIT/Apache, MIT | Cross-device move: `rename`, on `ErrorKind::CrossesDevices` copy then delete. |
| YAML | `serde-saphyr` 1.3 | MIT/Apache | Only candidate that reads YAML 1.1 `yes`/`no` as booleans (beets `config_default.yaml` uses them). Layers merge as `serde_json::Value` trees. |
| Images | `image` 0.25, `fast_image_resize` 6.1 | MIT/Apache | For cover art resize. |
| Errors, logs | `thiserror` 2, `tracing` 0.1 | MIT/Apache, MIT | |

Rejected: `any_ascii` (different output for Greek, Cyrillic, CJK),
`pathfinding::kuhn_munkres` (needs `Ord` costs), `lapjv` crate (square only),
`reqwest` (needs a runtime), `sqlx` (async), `diesel` (static schema cannot hold
flexible attributes), `serde_yml` (RUSTSEC-2025-0068), `serde_yaml` (deprecated),
`serde_norway`/`serde_yaml_ng` (YAML 1.2 booleans), `figment` (uses
deprecated `serde_yaml`), onetagger (GPL-3.0, do not copy).

Code with no crate, which we write: template parser and functions, query parser,
`replace:`-driven path sanitizer, and config layer merge.

`rust/deny.toml` (`cargo-deny`) allows MIT, Apache-2.0,
Apache-2.0 WITH LLVM-exception (required by PyO3's `target-lexicon`),
BSD-3-Clause, Unicode-3.0, and Zlib, and fails on GPL and AGPL.
It has crate-specific ISC exceptions for `ring`, `rustls-webpki`, and
`untrusted`, and a CDLA-Permissive-2.0 exception for `webpki-roots`.

### Compatibility with the current beets setup

The user's `~/.config/beets/config.yaml` uses the plugins `musicbrainz`,
`mbsync`, `fetchart` (filesystem source), and `embedart` (auto), the path
formats `default`, `comp`, and `singleton` with `%aunique{}`, custom `match:`
thresholds, and the library `~/Music/.library.db`.

- **Config.** `muzik-core` reads the beets config file itself: beets defaults
  (embedded copy of `config_default.yaml`), then the user file, then muzik
  overrides. The file stays the source of truth, so `beet` still works.
- **Database.** `muzik-library` reads and writes the existing beets schema
  (`items`, `albums`, `item_attributes`, `album_attributes`, `migrations`). It
  does not change the schema while beets is still installed. The `beet` CLI and
  muzik can then use the same library during the whole migration.
- **Flexible fields.** `muzik_source_id` (from `muzik/beets_plugins/muzik_source.py`)
  stays a flexible attribute. The `ftclean` behavior moves into `muzik-import`.

### Parity strategy

Beets is the oracle. `scripts/beets_fixtures/` holds one Python script per
component. Each calls real beets or mediafile code on a fixed corpus and writes
JSON fixtures, with the beets version, to the crate's `tests/fixtures/`. Rust
tests compare with these. Known differences go into explicit allow lists, never
a loose tolerance. Assignment tests compare total cost, because equal-cost ties
can resolve differently.

### Rollout switch

The muzik config (`~/.config/muzik/config.yaml`, loaded by
`load_muzik_config` in `muzik/config.py`) gets a `native:` section. A new
`get_native_settings()` follows the `get_seakarr_settings` pattern:

```yaml
native:
  match: beets      # beets | shadow | native
  metadata: beets
  tags: beets
  library: beets
  import: beets
```

`shadow` runs both paths, uses the beets result, and emits a warning event when
the results differ. A Rust error in `shadow` never stops the workflow. The
default for each switch changes to `native` only after shadow runs on the real
library show no divergence.

### Out of scope

- Moving the workflow host (`muzik/core/workflow/service.py`) to Rust.
- Beets plugins that the user does not enable, and the `beet` CLI commands.
- Singleton (`tag_item`) matching until Milestone 5 needs it.
- Publishing to crates.io. Every crate has `publish = false`. The crates are
  still structured as reusable libraries.
- AcoustID fingerprinting and Discogs lookup.

## Implementation Phases

### Milestone 0: Foundation

#### Phase 1: Workspace and binding crate rename

- Add `rust/Cargo.toml` workspace with `crates/*`. Move the lock file to
  `rust/Cargo.lock`, keep the `soulseek-rs-lib` rev
  `a62bab1e6a505362109b8303aa528af03403eeae`.
- Move `rust/seakarr_bridge/src/{session,job,types,error}.rs` to
  `rust/crates/muzik-soulseek`. Move `lib.rs` PyO3 code to
  `rust/crates/muzik-py`, `[lib] name = "_native"`.
- Update `pyproject.toml` `[tool.maturin]` (`manifest-path`, `module-name =
  "muzik._native"`), every `muzik._seakarr` import, `THIRD_PARTY_NOTICES.md`,
  `mise.toml` `check`/`develop`, and `DISTRIBUTION.md`.
- Check `maturin develop`, `maturin sdist`, and `tests/test_seakarr_bridge.py`.
  **Commit:** `refactor(native): split Soulseek bridge into muzik-soulseek and muzik-py`

#### Phase 2: Core types, config loading, license gate

- Add `muzik-core`: `LocalTrack`, `ReleaseCandidate`, `TrackCandidate`, ID
  newtypes, and `BeetsConfig` (layered load with `serde-saphyr`).
- Add Apache-2.0 `LICENSE` and `NOTICE` files, and `rust/deny.toml`. Run
  `cargo deny check licenses` in `mise run check`.
- Add `get_native_settings()` to `muzik/config.py` with all switches `beets`.
- Fixture: beets `config` values for the user's config shape.
  **Commit:** `feat(core): add muzik-core types and beets config loading`

### Milestone 1: Matching (`muzik-match`)

#### Phase 3: String distance

- Add `scripts/beets_fixtures/match.py` and port `string_dist`
  (`SD_END_WORDS`, `SD_REPLACE`, `SD_PATTERNS`) with fixture tests.
  **Commit:** `feat(match): port beets string distance`

#### Phase 4: Track and album distance

- Port `Distance` with per-key penalties, `MatchConfig` from the `match:`
  config, `track_distance`, and album `distance`.
  **Commit:** `feat(match): port track and album distance scoring`

#### Phase 5: Assignment and ranking

- Vendor `lsap` with its MIT notice. Port `assign_items`, `_recommendation`,
  and the candidate sort as `rank_albums`.
  **Commit:** `feat(match): add track assignment and candidate ranking`

#### Phase 6: Binding and shadow mode

- `muzik-py` exposes `rank_album_candidates`. `muzik/core/matching.py`
  converts a beets task. `MuzikImportSession.choose_match` in
  `muzik/core/beets/importer.py` compares rankings when `native.match: shadow`.
  **Commit:** `feat(import): compare native album ranking in shadow mode`

#### Phase 7: Native ranking

- With `native.match: native`, `task_view` in `muzik/core/beets/views.py` uses
  native order and distances. `resolve_choice` still returns the beets candidate.
  **Commit:** `feat(import): rank beets candidates with the native matcher`

### Milestone 2: Metadata (`muzik-metadata`)

#### Phase 8: MusicBrainz client

- Wrap `musicbrainz_rs` (sync) with `governor` (1 request per second) and
  `backon` retries. Release search with the same query fields as the beets
  `musicbrainz` plugin, and release lookup mapped to `ReleaseCandidate`.
- Fixtures: recorded MusicBrainz JSON responses, compared with the beets
  plugin's `AlbumInfo` for the same responses.
  **Commit:** `feat(metadata): add rate-limited MusicBrainz release client`

#### Phase 9: Replace musicbrainzngs

- Replace `musicbrainzngs` in `muzik/core/musicbrainz.py` (`search_releases`,
  `get_tracklist`) with the native client, behind `native.metadata`. Keep
  `musicbrainzngs` for the `beets` and `shadow` fallback until Milestone 6.
  **Commit:** `feat(metadata): look up chapter tracklists with the native client`

### Milestone 3: Tags (`muzik-tags`)

#### Phase 10: Tag read/write

- Port the mediafile field table (title, artists, album fields, track/disc,
  dates, MusicBrainz IDs, compilation, ReplayGain) onto `lofty`, with custom
  keys through the format-specific APIs.
- Fixtures: files written by mediafile and read by Rust, and the reverse, for
  MP3, FLAC, M4A, Opus, OGG.
  **Commit:** `feat(tags): read and write beets-compatible tags with lofty`

#### Phase 11: Probe and cover art

- `FileProperties` probe to replace `ffprobe` in `muzik/core/audio.py` and the
  measured quality in `muzik/core/quality.py`.
- Cover art: filesystem cover search (as `fetchart` with `sources: filesystem`)
  and embed (as `embedart`).
- Replace `tag_only_with_beet` in `muzik/core/beets/service.py` behind
  `native.tags`.
  **Commit:** `feat(tags): probe audio and embed cover art natively`

### Milestone 4: Library (`muzik-library`)

#### Phase 12: Read the beets database

- `rusqlite` models for items and albums with fixed and flexible attributes,
  with the `regexp`, `unidecode`, `bytelower` SQL functions.
  **Commit:** `feat(library): read beets library items and albums`

#### Phase 13: Query language

- Hand-written parser: `field:value`, `field::regex`, ranges `a..b`, negation
  `^`/`-`, quoted terms, `,` OR groups, sort `field+`/`field-`.
- Fixtures: beets `parse_query_string` results and the matched item IDs on a
  fixture database.
  **Commit:** `feat(library): parse and run beets queries`

#### Phase 14: Replace library reads

- Behind `native.library`, replace `beets.library.Library` use in
  `muzik/core/beets/lookup.py`, `muzik/core/watchlist.py`, and
  `muzik/commands/soulseek.py`.
  **Commit:** `feat(library): serve library lookups from the native reader`

#### Phase 15: Library writes

- Add, update, and remove items and albums in transactions. Port
  `prune_missing_items` with the same safety fraction.
  **Commit:** `feat(library): write items and albums to the beets database`

### Milestone 5: Import (`muzik-import`)

#### Phase 16: Path formats

- Template parser (`$field`, `${field}`, `%func{}`), functions `%if`, `%left`,
  `%right`, `%lower`, `%upper`, `%title`, `%asciify`, `%aunique`, and the
  `replace:` sanitizer. Fixtures: beets `Item.destination()` for the user's
  `paths:` config.
  **Commit:** `feat(import): render beets path formats`

#### Phase 17: File operations

- Move, copy, link, hardlink, reflink, cross-device move, and prune of empty
  folders. Tests on temp folders.
  **Commit:** `feat(import): add file operations for library placement`

#### Phase 18: Import plan and apply

- `plan(paths)` groups album folders, gets candidates (`muzik-metadata`), ranks
  them (`muzik-match`), and finds duplicates. `apply(plan, decisions)` writes
  tags, moves files, embeds art, stores items (with `muzik_source_id`), and
  moves feat. credits to the artist (the `ftclean` rule).
- Python keeps the decisions (`BeetsDecisions`, `AgentBeetsDecisions`): it
  gets the plan, chooses, and calls `apply`.
- Port `mbsync` as `sync(query)`.
  **Commit:** `feat(import): plan and apply imports without beets`

#### Phase 19: Switch the importer

- `native.import: shadow` makes a plan next to a beets dry run and compares
  destinations and chosen releases. `native` sends `import_paths` in
  `muzik/core/beets/importer.py` to the Rust importer.
  **Commit:** `feat(import): run imports through the native pipeline`

### Milestone 6: Remove beets

#### Phase 20: Remove the beets dependency

- Set all `native:` defaults to `native`. Remove `beets`, `musicbrainzngs`, and
  their use: `muzik/core/beets/`, `muzik/beets_plugins/`, `BEETS_CONFIG` in
  `muzik/config.py` (use the path from `muzik-core`), and their tests.
  Keep the config file and database compatible.
  **Commit:** `refactor(import)!: remove the beets dependency`

## Risks & Tradeoffs

- **Size.** This is about 20 phases. Each milestone gives value alone, and
  work can stop after any milestone with a working app.
- **Transliteration.** `deunicode` differs from Python Unidecode on some
  strings. Allow lists record each difference.
- **Assignment ties.** Tests compare total cost, and shadow mode only reports a
  different top candidate or a distance change.
- **Shared database.** Rust and `beet` write the same SQLite file. Mitigation:
  no schema change while beets exists, WAL-safe transactions, and a backup of
  `~/Music/.library.db` before the first native write.
- **Beets drift.** Fixtures record the beets version. A beets upgrade that
  changes behavior shows as a test diff.
- **MusicBrainz candidate search.** The beets plugin's search and its
  `AlbumInfo` mapping have many details. Mitigation: recorded responses as
  fixtures and shadow comparison of candidate IDs.

## Verification record

On 2026-09-27, a read-only comparison with the configured beets database found
1,370 items and 161 albums in both readers. The tested item and album queries
had the same IDs and order. Five album lookups also matched. The database had
no `muzik_source_id` values, so this check did not cover live source ID lookup.

The import shadow run used one temporary FLAC group and a temporary snapshot of
the configured library. It found no unmatched group, release difference, or
destination difference. The real database stayed at 1,370 items, with the same
file size and modification time. The source file hash did not change. A
separate test selected the second of two releases and checked that a release
difference was reported while the destination comparison remained equal.

## Open Questions

None. Decided 2026-09-27: no crate is published to crates.io, and AcoustID and
Discogs are not part of this port.
