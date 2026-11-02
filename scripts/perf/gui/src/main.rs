#![allow(dead_code)]
mod alloc;
#[global_allocator]
static ALLOCATOR: alloc::Counter = alloc::Counter;
use muzik_store::watchlist::{
    Availability, ItemAction, ItemId, ItemView, PlaylistView, PrimaryAction, SourceKind, Summary,
    WatchItem,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    hint::black_box,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColumnSort {
    Default,
    Ascending,
    Descending,
}
#[derive(Clone)]
struct Operation {
    column: Option<usize>,
    descending: bool,
    filter: String,
}
#[derive(Debug, PartialEq, Eq)]
struct Projection {
    path: String,
    title: String,
    length: u64,
    format: String,
}
fn project(a: &base::AlbumRow, length: f64, format: String) -> Projection {
    Projection {
        path: a.path.clone(),
        title: a.title.clone(),
        length: length.to_bits(),
        format,
    }
}
mod base {
    include!("library_baseline.rs");
}
mod watch_base {
    include!("watch_baseline.rs");
}
mod read_enum {
    include!("read_enum.rs");
}
use read_enum::Read;
mod watchlist_view {
    use super::*;
    pub fn matches_filter(item: &ItemView, filter: Option<Summary>) -> bool {
        filter.map_or_else(
            || item.summary != Summary::Unavailable,
            |wanted| item.summary == wanted,
        )
    }
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}
fn number(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}
fn integer(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}
fn sample_albums(tracks: &[Value]) -> Vec<Arc<base::AlbumRow>> {
    let mut grouped: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for (i, t) in tracks.iter().enumerate() {
        let k = t
            .get("album_id")
            .filter(|v| !v.is_null())
            .map(Value::to_string)
            .unwrap_or_else(|| format!("singleton-{i}"));
        grouped.entry(k).or_default().push(t);
    }
    let mut out = Vec::new();
    for (_key, ts) in grouped {
        let t = ts[0];
        let mut rows: Vec<_> = ts
            .iter()
            .map(|v| base::Track {
                disc: integer(v, "disc"),
                number: Some(integer(v, "track")).filter(|v| *v > 0),
                title: text(v, "title"),
                artist: text(v, "artist"),
                length: number(v, "length"),
                format: text(v, "format").to_uppercase(),
                path: Some(PathBuf::from(text(v, "path"))),
            })
            .collect();
        rows.sort_by_key(|t| (t.disc, t.number));
        let path = rows
            .iter()
            .find_map(|t| t.path.as_deref()?.parent())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        out.push(Arc::new(base::AlbumRow {
            artist: text(t, "albumartist"),
            title: text(t, "album"),
            year: Some(integer(t, "year")).filter(|v| *v > 0),
            genre: text(t, "genre"),
            label: text(t, "label"),
            added: number(t, "added"),
            path,
            tracks: rows,
        }));
    }
    out.sort_by(|a, b| b.added.total_cmp(&a.added));
    out
}

struct SummaryRow {
    album: Arc<base::AlbumRow>,
    length: f64,
    format: String,
}
fn summary_run(albums: &[Arc<base::AlbumRow>], ops: &[Operation]) -> Vec<Vec<Projection>> {
    // Summary construction is INCLUDED once per operation sequence, not free.
    let summaries: Vec<_> = albums
        .iter()
        .map(|a| {
            Arc::new(SummaryRow {
                album: a.clone(),
                length: base::raw_length(a),
                format: base::raw_format(a),
            })
        })
        .collect();
    let mut out = Vec::new();
    let mut rows = summaries.clone();
    black_box(&rows);
    for op in ops {
        let filter = op.filter.trim().to_lowercase();
        rows = summaries
            .iter()
            .filter(|a| filter.is_empty() || base::raw_matches(&a.album, &filter))
            .cloned()
            .collect();
        match op.column {
            Some(0) => rows.sort_by_cached_key(|a| {
                (
                    a.album.artist.to_lowercase(),
                    a.album.year,
                    a.album.title.to_lowercase(),
                )
            }),
            Some(1) => rows.sort_by_cached_key(|a| a.album.title.to_lowercase()),
            Some(2) => rows.sort_by_key(|a| a.album.year),
            Some(3) => rows.sort_by_key(|a| a.album.tracks.len()),
            Some(4) => rows.sort_by(|a, b| a.length.total_cmp(&b.length)),
            Some(5) => rows.sort_by_cached_key(|a| a.format.clone()),
            Some(6) => rows.sort_by(|a, b| a.album.added.total_cmp(&b.album.added)),
            _ => {}
        }
        if op.descending && op.column.is_some() {
            rows.reverse();
        }
        out.push(
            rows.iter()
                .map(|a| project(&a.album, a.length, a.format.clone()))
                .collect(),
        );
    }
    out
}
impl AsRef<base::AlbumRow> for base::AlbumRow {
    fn as_ref(&self) -> &Self {
        self
    }
}
fn index_run<T: AsRef<base::AlbumRow>>(albums: &[T], ops: &[Operation]) -> Vec<Vec<Projection>> {
    let mut out = Vec::new();
    let mut rows: Vec<usize> = (0..albums.len()).collect();
    black_box(&rows);
    for op in ops {
        let filter = op.filter.trim().to_lowercase();
        rows = albums
            .iter()
            .enumerate()
            .filter(|(_, a)| filter.is_empty() || base::raw_matches(a.as_ref(), &filter))
            .map(|(i, _)| i)
            .collect();
        match op.column {
            Some(0) => rows.sort_by_cached_key(|&i| {
                let a = albums[i].as_ref();
                (a.artist.to_lowercase(), a.year, a.title.to_lowercase())
            }),
            Some(1) => rows.sort_by_cached_key(|&i| albums[i].as_ref().title.to_lowercase()),
            Some(2) => rows.sort_by_key(|&i| albums[i].as_ref().year),
            Some(3) => rows.sort_by_key(|&i| albums[i].as_ref().tracks.len()),
            Some(4) => rows.sort_by(|&a, &b| {
                base::raw_length(albums[a].as_ref())
                    .total_cmp(&base::raw_length(albums[b].as_ref()))
            }),
            Some(5) => rows.sort_by_cached_key(|&i| base::raw_format(albums[i].as_ref())),
            Some(6) => rows.sort_by(|&a, &b| {
                albums[a]
                    .as_ref()
                    .added
                    .total_cmp(&albums[b].as_ref().added)
            }),
            _ => {}
        }
        if op.descending && op.column.is_some() {
            rows.reverse();
        }
        out.push(
            rows.into_iter()
                .map(|i| {
                    let a = albums[i].as_ref();
                    project(a, base::raw_length(a), base::raw_format(a))
                })
                .collect(),
        );
    }
    out
}
fn operations(albums: &[Arc<base::AlbumRow>]) -> Vec<Operation> {
    let artist = albums
        .first()
        .map(|a| a.artist.to_lowercase())
        .unwrap_or_default();
    let mut out = Vec::new();
    for column in [
        None,
        Some(4),
        Some(5),
        Some(0),
        Some(1),
        Some(2),
        Some(3),
        Some(6),
    ] {
        for descending in [false, true] {
            out.push(Operation {
                column,
                descending,
                filter: String::new(),
            });
        }
    }
    for filter in [
        artist,
        "__generated_no_match_filter__".to_owned(),
        "  ".to_owned(),
    ] {
        out.push(Operation {
            column: Some(0),
            descending: false,
            filter,
        });
    }
    out
}

fn playlist(tracks: &[Value]) -> Arc<PlaylistView> {
    let items: Vec<ItemView> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut item = WatchItem::new((i + 1) as u64, &text(t, "title"), SourceKind::Youtube);
            // Lifecycle, source IDs, queue flags and actions below are GENERATED.
            // Only the descriptive track payload comes from the real metadata sample.
            item.video_id = Some(format!("generated-{i}"));
            item.video_url = Some(format!("https://example.invalid/generated/{i}"));
            item.track = Some(t.clone());
            let summary = Summary::ALL[i % Summary::ALL.len()];
            let actions = ItemAction::ALL
                .iter()
                .copied()
                .map(|a| {
                    (
                        a,
                        Availability {
                            enabled: i % 3 != 0,
                            reason: if i % 3 == 0 {
                                Some("Generated disabled reason".into())
                            } else {
                                None
                            },
                        },
                    )
                })
                .collect();
            ItemView {
                item,
                thumbnail_path: None,
                summary,
                primary_action: Some(PrimaryAction {
                    action: ItemAction::Run,
                    label: "Run".into(),
                }),
                actions,
            }
        })
        .collect();
    Arc::new(serde_json::from_value(json!({"playlist_id":"generated-playlist","url":"https://example.invalid/playlist","items":items})).unwrap())
}
#[derive(Clone)]
struct IndexedRow {
    index: usize,
    key: ItemId,
    queued: bool,
}
struct IndexedTable {
    snapshot: Arc<PlaylistView>,
    generation: u64,
    rows: Vec<IndexedRow>,
    filter: Option<Summary>,
    column: usize,
    descending: bool,
}
impl IndexedTable {
    fn new(
        snapshot: Arc<PlaylistView>,
        filter: Option<Summary>,
        queued: &HashSet<ItemId>,
        column: usize,
        descending: bool,
        generation: u64,
    ) -> Self {
        let mut rows: Vec<_> = snapshot
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| watchlist_view::matches_filter(item, filter))
            .map(|(index, item)| {
                let key = ItemId::of(&snapshot.playlist_id, &item.item);
                let queued = queued.contains(&key);
                IndexedRow { index, key, queued }
            })
            .collect();
        match column {
            1 => rows.sort_by_cached_key(|r| snapshot.items[r.index].item.title.to_lowercase()),
            3 => rows.sort_by(|a, b| {
                snapshot.items[a.index]
                    .summary
                    .as_ref()
                    .cmp(snapshot.items[b.index].summary.as_ref())
                    .then(a.key.position.cmp(&b.key.position))
            }),
            _ => rows.sort_by_key(|r| r.key.position),
        }
        if descending {
            rows.reverse();
        }
        Self {
            snapshot,
            generation,
            rows,
            filter,
            column,
            descending,
        }
    }
    fn update_queue(&mut self, q: &HashSet<ItemId>) {
        for r in &mut self.rows {
            r.queued = q.contains(&r.key);
        }
    }
    fn replace(&mut self, snapshot: Arc<PlaylistView>, q: &HashSet<ItemId>) {
        *self = Self::new(
            snapshot,
            self.filter,
            q,
            self.column,
            self.descending,
            self.generation + 1,
        );
    }
    fn resolve(&self, generation: u64, index: usize) -> Option<&ItemView> {
        (generation == self.generation)
            .then(|| self.snapshot.items.get(index))
            .flatten()
    }
}
fn queue(p: &PlaylistView, phase: usize) -> HashSet<ItemId> {
    p.items
        .iter()
        .enumerate()
        .filter(|(i, _)| (i + phase) % 3 == 0)
        .map(|(_, i)| ItemId::of(&p.playlist_id, &i.item))
        .collect()
}
fn equal_watch(b: &watch_base::Table, c: &IndexedTable) {
    assert_eq!(b.rows.len(), c.rows.len());
    for (br, cr) in b.rows.iter().zip(&c.rows) {
        assert_eq!(br.key, cr.key);
        assert_eq!(br.queued, cr.queued);
        assert_eq!(br.item, c.snapshot.items[cr.index]);
    }
}
fn watch_sequence_baseline(p: &Arc<PlaylistView>, queues: &[HashSet<ItemId>]) -> watch_base::Table {
    let mut t = watch_base::Table::from_playlist(p, None, &queues[0], 1, false);
    for q in &queues[1..] {
        t = watch_base::Table::from_playlist(p, None, q, 1, false);
    }
    t
}
fn watch_sequence_candidate(p: &Arc<PlaylistView>, queues: &[HashSet<ItemId>]) -> IndexedTable {
    let mut t = IndexedTable::new(p.clone(), None, &queues[0], 1, false, 0);
    for q in &queues[1..] {
        t.update_queue(q);
    }
    t
}

#[derive(Clone, Copy)]
enum ReadOp {
    Start(Read),
    Finish(Read, u64),
}
fn read_index(r: Read) -> usize {
    match r {
        Read::Watchlist => 0,
        Read::Library => 1,
        Read::Services => 2,
        Read::SpotifyStatus => 3,
        Read::SpotifyPlaylists => 4,
    }
}
fn read_script(count: usize) -> Vec<ReadOp> {
    let kinds = [
        Read::Watchlist,
        Read::Library,
        Read::Services,
        Read::SpotifyStatus,
        Read::SpotifyPlaylists,
    ];
    let mut serial = 0_u64;
    let mut out = Vec::new();
    for i in 0..count.max(1) {
        let r = kinds[i % 5];
        let other = kinds[(i + 1) % 5];
        serial = serial.wrapping_add(1);
        let first = serial;
        out.push(ReadOp::Start(r));
        serial = serial.wrapping_add(1);
        let second = serial;
        out.push(ReadOp::Start(r));
        serial = serial.wrapping_add(1);
        let third = serial;
        out.push(ReadOp::Start(other));
        out.push(ReadOp::Finish(r, first));
        out.push(ReadOp::Finish(other, third));
        out.push(ReadOp::Finish(r, second));
    }
    out
}
fn reads_baseline(ops: &[ReadOp]) -> Vec<(bool, [bool; 5])> {
    let mut reads = HashMap::new();
    let mut serial = 0_u64;
    let kinds = [
        Read::Watchlist,
        Read::Library,
        Read::Services,
        Read::SpotifyStatus,
        Read::SpotifyPlaylists,
    ];
    let mut out = Vec::new();
    for &op in ops {
        let accepted = match op {
            ReadOp::Start(read) => {
                serial = serial.wrapping_add(1);
                reads.insert(read, serial);
                false
            }
            ReadOp::Finish(read, n) => {
                if reads.get(&read) != Some(&n) {
                    false
                } else {
                    reads.remove(&read);
                    true
                }
            }
        };
        out.push((accepted, kinds.map(|r| reads.contains_key(&r))));
    }
    out
}
fn reads_candidate(ops: &[ReadOp]) -> Vec<(bool, [bool; 5])> {
    let mut reads = [None; 5];
    let mut serial = 0_u64;
    let mut out = Vec::new();
    for &op in ops {
        let accepted = match op {
            ReadOp::Start(r) => {
                serial = serial.wrapping_add(1);
                reads[read_index(r)] = Some(serial);
                false
            }
            ReadOp::Finish(r, n) => {
                let slot = &mut reads[read_index(r)];
                if *slot != Some(n) {
                    false
                } else {
                    *slot = None;
                    true
                }
            }
        };
        out.push((accepted, reads.map(|n| n.is_some())));
    }
    out
}
fn summary_index(s: Summary) -> usize {
    match s {
        Summary::Pending => 0,
        Summary::Processing => 1,
        Summary::Waiting => 2,
        Summary::Failed => 3,
        Summary::Processed => 4,
        Summary::Unavailable => 5,
    }
}
fn counts_baseline(items: &[ItemView]) -> String {
    let mut counts = HashMap::<Summary, usize>::new();
    for item in items {
        let count = counts.entry(item.summary).or_default();
        *count = count.saturating_add(1);
    }
    Summary::ALL
        .iter()
        .filter_map(|state| {
            counts
                .get(state)
                .map(|count| format!("{count} {}", state.as_ref().to_lowercase()))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}
fn counts_candidate(items: &[ItemView]) -> String {
    let mut counts = [0_usize; 6];
    for item in items {
        let count = &mut counts[summary_index(item.summary)];
        *count = count.saturating_add(1);
    }
    Summary::ALL
        .iter()
        .filter_map(|&state| {
            let count = counts[summary_index(state)];
            (count != 0).then(|| format!("{count} {}", state.as_ref().to_lowercase()))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn bench<B, C>(
    name: &str,
    size: &str,
    n: usize,
    iters: usize,
    mut before: impl FnMut() -> B,
    mut after: impl FnMut() -> C,
) {
    black_box(before());
    black_box(after());
    for sample in 0..7 {
        for side in if sample % 2 == 0 {
            ["before", "after"]
        } else {
            ["after", "before"]
        } {
            let start = Instant::now();
            if side == "before" {
                for _ in 0..iters {
                    black_box(before());
                }
            } else {
                for _ in 0..iters {
                    black_box(after());
                }
            }
            let ns = start.elapsed().as_nanos();
            // A separate untimed pass counts allocator calls and includes destruction.
            alloc::start();
            if side == "before" {
                for _ in 0..iters {
                    black_box(before());
                }
            } else {
                for _ in 0..iters {
                    black_box(after());
                }
            }
            let (allocations, reallocations, deallocations, bytes) = alloc::stop();
            println!(
                "{}",
                json!({"candidate":name,"size":size,"records":n,"sample":sample,"side":side,"iterations":iters,"elapsed_ns":ns,"allocations_total":allocations,"allocations_per_op":allocations as f64/iters as f64,"reallocations_total":reallocations,"deallocations_total":deallocations,"requested_bytes_total":bytes,"scope":"extracted GUI kernel, no UI latency claim"})
            );
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("--check");
    let path = args.get(2).ok_or(
        "usage: experiment --check|--bench tracks.json [candidate-name] [small|full|both]",
    )?;
    let only = args.get(3).map(String::as_str).unwrap_or("all");
    let wanted_size = args.get(4).map(String::as_str).unwrap_or("both");
    let all: Vec<Value> = serde_json::from_slice(&std::fs::read(path)?)?;
    for (size, tracks) in [
        ("small", &all[..all.len().min(64)]),
        ("full", all.as_slice()),
    ] {
        if wanted_size != "both" && wanted_size != size {
            continue;
        }
        let albums = sample_albums(tracks);
        let ops = operations(&albums);
        let flat: Arc<[base::AlbumRow]> = albums.iter().map(|a| (**a).clone()).collect();
        let p = playlist(tracks);
        let queues: Vec<_> = (0..8).map(|phase| queue(&p, phase)).collect();
        let reads = read_script(tracks.len());
        assert_eq!(base::run(&albums, &ops), summary_run(&albums, &ops));
        assert_eq!(base::run(&albums, &ops), index_run(flat.as_ref(), &ops));
        assert_eq!(reads_baseline(&reads), reads_candidate(&reads));
        assert_eq!(counts_baseline(&p.items), counts_candidate(&p.items));
        assert_eq!(counts_baseline(&[]), counts_candidate(&[]));
        for filter in std::iter::once(None).chain(Summary::ALL.iter().copied().map(Some)) {
            for col in [0, 1, 3] {
                for desc in [false, true] {
                    let b = watch_base::Table::from_playlist(&p, filter, &queues[0], col, desc);
                    let c = IndexedTable::new(p.clone(), filter, &queues[0], col, desc, 0);
                    equal_watch(&b, &c);
                }
            }
        }
        let b = watch_sequence_baseline(&p, &queues);
        let mut c = watch_sequence_candidate(&p, &queues);
        equal_watch(&b, &c);
        let selected_key = c.rows.first().map(|r| r.key.clone());
        assert_eq!(selected_key, b.rows.first().map(|r| r.key.clone()));
        let old_snapshot = c.snapshot.clone();
        let old_generation = c.generation;
        let mut replacement = (*p).clone();
        replacement.items.reverse();
        replacement.items.pop();
        replacement.playlist_id = "generated-replacement".into();
        let replacement = Arc::new(replacement);
        c.replace(replacement.clone(), &HashSet::new());
        assert!(c.resolve(old_generation, 0).is_none());
        assert_eq!(old_snapshot, p);
        let b = watch_base::Table::from_playlist(&replacement, None, &HashSet::new(), 1, false);
        equal_watch(&b, &c);
        if let Some(old_key) = selected_key {
            assert!(!c.rows.iter().any(|r| r.key == old_key));
        }
        // A sheet owns its snapshot. A later library load cannot change it.
        let sheet = flat.clone();
        let old_first = sheet.first().cloned();
        let mut current = flat.clone();
        let mut replacement = current.to_vec();
        replacement.reverse();
        current = replacement.into();
        assert_eq!(sheet.first().cloned(), old_first);
        black_box(current);
        println!(
            "{}",
            json!({"checks":"passed","size":size,"tracks":tracks.len(),"albums":albums.len(),"lifecycle":"generated from real descriptive metadata; queue/source/summary/action values are generated","checks_include":["empty counts","filters and all summary states","sort order","row content equality","snapshot replacement and old-generation rejection","library sheet snapshot lifetime","same-read reversed completions","distinct concurrent reads"]})
        );
        if mode == "--check" {
            continue;
        }
        if mode != "--bench" {
            return Err("unknown mode".into());
        }
        let iters = if size == "small" { 5 } else { 1 };
        let wants = |name: &str| only == "all" || only == name;
        let cold_ops = &ops[..1];
        if wants("library_summaries") {
            bench(
                "library_summaries",
                &format!("{size}_cold1"),
                albums.len(),
                iters,
                || base::run(black_box(&albums), black_box(cold_ops)),
                || summary_run(black_box(&albums), black_box(cold_ops)),
            );
        }
        if wants("library_summaries") {
            bench(
                "library_summaries",
                size,
                albums.len(),
                iters,
                || base::run(black_box(&albums), black_box(&ops)),
                || summary_run(black_box(&albums), black_box(&ops)),
            );
        }
        if wants("library_indices") {
            bench(
                "library_indices",
                size,
                albums.len(),
                iters,
                || base::run(black_box(&albums), black_box(&ops)),
                || index_run(black_box(flat.as_ref()), black_box(&ops)),
            );
        }
        if wants("watch_rows_indices") {
            bench(
                "watch_rows_indices",
                size,
                p.items.len(),
                iters,
                || {
                    watch_base::Table::from_playlist(
                        black_box(&p),
                        None,
                        black_box(&queues[0]),
                        1,
                        false,
                    )
                },
                || {
                    IndexedTable::new(
                        black_box(p.clone()),
                        None,
                        black_box(&queues[0]),
                        1,
                        false,
                        0,
                    )
                },
            );
        }
        if wants("watch_queue_only") {
            bench(
                "watch_queue_only",
                size,
                p.items.len(),
                iters,
                || watch_sequence_baseline(black_box(&p), black_box(&queues)),
                || watch_sequence_candidate(black_box(&p), black_box(&queues)),
            );
        }
        if wants("read_serial_array") {
            bench(
                "read_serial_array",
                size,
                reads.len(),
                iters,
                || reads_baseline(black_box(&reads)),
                || reads_candidate(black_box(&reads)),
            );
        }
        if wants("summary_count_array") {
            bench(
                "summary_count_array",
                size,
                p.items.len(),
                iters,
                || counts_baseline(black_box(&p.items)),
                || counts_candidate(black_box(&p.items)),
            );
        }
    }
    Ok(())
}
