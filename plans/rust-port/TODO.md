# TODO: Port the beets functionality of muzik to Rust

## Milestone 0: Foundation

### Phase 1: Workspace and binding crate rename

- [x] Add `rust/Cargo.toml` workspace; move lock to `rust/Cargo.lock`; keep
  `soulseek-rs-lib` rev `a62bab1e6a505362109b8303aa528af03403eeae`
- [x] Move Soulseek logic to `rust/crates/muzik-soulseek`
- [x] Move PyO3 code to `rust/crates/muzik-py`, module `muzik._native`
- [x] Update `pyproject.toml`, `muzik._seakarr` imports, notices, `mise.toml`,
  `DISTRIBUTION.md`
- [x] `maturin develop` and `maturin sdist` build
- [x] `tests/test_seakarr_bridge.py` passes
- [x] Commit: `refactor(native): split Soulseek bridge into muzik-soulseek and muzik-py`

### Phase 2: Core types, config loading, license gate

- [x] `muzik-core` types and `BeetsConfig` layered load with `serde-saphyr`
- [x] Apache-2.0 `LICENSE`, `NOTICE`, `rust/deny.toml`; `cargo deny check licenses` in `mise run check`
- [x] `get_native_settings()` in `muzik/config.py`, all switches `beets`
- [x] Config fixture test passes
- [x] Commit: `feat(core): add muzik-core types and beets config loading`

## Milestone 1: Matching (`muzik-match`)

### Phase 3: String distance

- [x] `scripts/beets_fixtures/match.py` writes `string_dist.json`
- [x] Port `string_dist`; fixture test with explicit allow list
- [x] Commit: `feat(match): port beets string distance`

### Phase 4: Track and album distance

- [x] Port `Distance`, `MatchConfig`, `track_distance`, album `distance`
- [x] Fixtures: preferred media/countries, `original_year`, length grace/max,
  VA rules, missing and unmatched tracks
- [x] Commit: `feat(match): port track and album distance scoring`

### Phase 5: Assignment and ranking

- [ ] Vendor `lsap` with MIT notice
- [ ] Port `assign_items`, `_recommendation`, candidate sort as `rank_albums`
- [ ] Fixtures: more files than tracks, more tracks than files, multi-disc, ties
- [ ] Commit: `feat(match): add track assignment and candidate ranking`

### Phase 6: Binding and shadow mode

- [ ] `rank_album_candidates` in `muzik-py`
- [ ] `muzik/core/matching.py` task conversion
- [ ] Shadow comparison in `MuzikImportSession.choose_match`
- [ ] Tests: divergence warning, no divergence, Rust error does not stop import
- [ ] Commit: `feat(import): compare native album ranking in shadow mode`

### Phase 7: Native ranking

- [ ] `task_view` uses native order and distances at `native`
- [ ] `resolve_choice` returns the beets candidate object
- [ ] Tests for `AgentBeetsDecisions` and `NonInteractiveBeetsDecisions`
- [ ] Commit: `feat(import): rank beets candidates with the native matcher`

## Milestone 2: Metadata (`muzik-metadata`)

### Phase 8: MusicBrainz client

- [x] `musicbrainz_rs` sync wrapper with `governor` and `backon`
- [x] Release search and lookup mapped to `ReleaseCandidate`
- [x] Recorded-response fixtures compared with beets `AlbumInfo`
- [x] Commit: `feat(metadata): add rate-limited MusicBrainz release client`

### Phase 9: Replace musicbrainzngs

- [ ] `muzik/core/musicbrainz.py` uses the native client behind `native.metadata`
- [ ] `tests/test_musicbrainz_lookup.py` passes in both modes
- [ ] Commit: `feat(metadata): look up chapter tracklists with the native client`

## Milestone 3: Tags (`muzik-tags`)

### Phase 10: Tag read/write

- [ ] mediafile field table on `lofty`, custom keys per format
- [ ] Cross fixtures (mediafile ↔ Rust) for MP3, FLAC, M4A, Opus, OGG
- [ ] Commit: `feat(tags): read and write beets-compatible tags with lofty`

### Phase 11: Probe and cover art

- [ ] Probe replaces `ffprobe` in `muzik/core/audio.py` and `muzik/core/quality.py`
- [ ] Filesystem cover search and embed
- [ ] `tag_only_with_beet` replaced behind `native.tags`
- [ ] Commit: `feat(tags): probe audio and embed cover art natively`

## Milestone 4: Library (`muzik-library`)

### Phase 12: Read the beets database

- [x] Item and album models with flexible attributes
- [x] `regexp`, `unidecode`, `bytelower` SQL functions
- [x] Commit: `feat(library): read beets library items and albums`

### Phase 13: Query language

- [ ] Parser for fields, regex, ranges, negation, quotes, OR groups, sort
- [ ] Fixtures against beets `parse_query_string` and matched IDs
- [ ] Commit: `feat(library): parse and run beets queries`

### Phase 14: Replace library reads

- [ ] `lookup.py`, `watchlist.py`, `commands/soulseek.py` behind `native.library`
- [ ] Commit: `feat(library): serve library lookups from the native reader`

### Phase 15: Library writes

- [ ] Add, update, remove in transactions
- [ ] Port `prune_missing_items` with the safety fraction
- [ ] Commit: `feat(library): write items and albums to the beets database`

## Milestone 5: Import (`muzik-import`)

### Phase 16: Path formats

- [x] Template parser and functions including `%aunique`
- [x] `replace:` sanitizer
- [x] Fixtures against beets `Item.destination()` for the user's `paths:`
- [x] Commit: `feat(import): render beets path formats`

### Phase 17: File operations

- [x] Move, copy, link, hardlink, reflink, cross-device move, empty-folder prune
- [x] Commit: `feat(import): add file operations for library placement`

### Phase 18: Import plan and apply

- [ ] `plan` groups albums, gets candidates, ranks, finds duplicates
- [ ] `apply` writes tags, moves files, embeds art, stores items with
  `muzik_source_id`, applies the `ftclean` rule
- [ ] Python decisions drive `apply`
- [ ] Port `mbsync` as `sync(query)`
- [ ] Commit: `feat(import): plan and apply imports without beets`

### Phase 19: Switch the importer

- [ ] Shadow compares destinations and releases with a beets dry run
- [ ] `import_paths` uses the Rust importer at `native`
- [ ] Commit: `feat(import): run imports through the native pipeline`

## Milestone 6: Remove beets

### Phase 20: Remove the beets dependency

- [ ] All `native:` defaults `native`
- [ ] Remove `beets`, `musicbrainzngs`, `muzik/core/beets/`, `muzik/beets_plugins/`
- [ ] `uv lock` updated; `uv pip check` passes
- [ ] Commit: `refactor(import)!: remove the beets dependency`

## Verification

- [ ] `mise run check` passes after each phase (Python, `rust/` workspace,
  `rust/gpui_app`, `cargo deny`)
- [ ] `cargo tree` for `muzik-core`, `muzik-match`, `muzik-metadata`,
  `muzik-tags`, `muzik-library`, `muzik-import` shows no `pyo3`
- [ ] Each fixture file records the beets version it came from
- [ ] Every crate `Cargo.toml` has `publish = false`
- [ ] With every switch at `beets`, the app behaves as before each phase
- [ ] Shadow run on `~/Music/.library.db` (after a backup): no divergence for
  match, metadata, library, and import
- [ ] Manual smoke test: YouTube album download → split → native import lands
  at the same path, with the same tags, as a beets import
- [ ] `beet ls` still reads the library after native writes
- [ ] Edge cases: empty candidate list, one-track album, track with no length,
  non-Latin titles, VA release, cross-device move, duplicate album
- [ ] No regressions in `tests/test_beets_*.py`, `tests/test_agent_decisions.py`,
  `tests/test_watchlist.py`, `tests/test_seakarr_bridge.py`

## Review

- [ ] Code reviewed
- [ ] PLAN.md updated if approach changed during implementation
- [ ] All phase commits are clean and describe their intent
- [ ] TODO.md items all checked off
