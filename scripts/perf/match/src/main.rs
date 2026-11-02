use base_library::{Fields, SqlValue};
use serde_json::{Value, json};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{
    collections::BTreeMap,
    hint::black_box,
    path::{Path, PathBuf},
    time::Instant,
};
static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static REQUESTED_BYTES: AtomicUsize = AtomicUsize::new(0);
struct CountingAllocator;
// This wrapper exists only in this private measurement binary.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            REQUESTED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() && COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            REQUESTED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(ptr, layout, size) };
        if !result.is_null() && COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            REQUESTED_BYTES.fetch_add(size, Ordering::Relaxed);
        }
        result
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
fn reset_counts() {
    COUNTING.store(true, Ordering::Relaxed);
    ALLOCS.store(0, Ordering::Relaxed);
    REQUESTED_BYTES.store(0, Ordering::Relaxed);
}
fn counts() -> (usize, usize) {
    COUNTING.store(false, Ordering::Relaxed);
    (
        ALLOCS.load(Ordering::Relaxed),
        REQUESTED_BYTES.load(Ordering::Relaxed),
    )
}
type ResultValue<T> = Result<T, String>;
type ItemRows = Vec<(i64, Fields, Fields)>;

fn run<T: PartialEq + std::fmt::Debug>(
    name: &str,
    workload: &str,
    count: usize,
    iterations: usize,
    bench: bool,
    mut before: impl FnMut() -> ResultValue<T>,
    mut after: impl FnMut() -> ResultValue<T>,
) -> ResultValue<()> {
    // Warm both paths before the untimed allocation passes.
    let _ = black_box(before());
    let _ = black_box(after());
    reset_counts();
    let expected = before();
    let before_counts = counts();
    reset_counts();
    let actual = after();
    let after_counts = counts();
    if expected != actual {
        return Err(format!(
            "{name}/{workload}: outputs differ: {expected:?} != {actual:?}"
        ));
    }
    println!(
        "{}",
        json!({"candidate":name,"workload":workload,"records":count,"mode":"check","correctness_equal":true,"before_allocs":before_counts.0,"after_allocs":after_counts.0,"before_requested_bytes":before_counts.1,"after_requested_bytes":after_counts.1,"outcome":if expected.is_ok(){"ok"}else{"error"}})
    );
    if !bench {
        return Ok(());
    }
    for sample in 0..7 {
        for side in if sample % 2 == 0 {
            ["before", "after"]
        } else {
            ["after", "before"]
        } {
            COUNTING.store(false, Ordering::Relaxed);
            let start = Instant::now();
            for _ in 0..iterations {
                if side == "before" {
                    black_box(before());
                } else {
                    black_box(after());
                }
            }
            let ns = start.elapsed().as_nanos() / iterations as u128;
            let (allocs, requested_bytes) = if side == "before" {
                before_counts
            } else {
                after_counts
            };
            println!(
                "{}",
                json!({"candidate":name,"workload":workload,"records":count,"mode":"bench","side":side,"sample":sample,"iterations":iterations,"sample_ns":ns,"sample_allocs":allocs,"sample_requested_bytes":requested_bytes,"allocation_pass":"untimed-warm","correctness_equal":true})
            );
        }
    }
    Ok(())
}
fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}
fn bits(values: Vec<f64>) -> Vec<u64> {
    values.into_iter().map(f64::to_bits).collect()
}
macro_rules! matrix {
    ($fn:ident,$lib:ident) => {
        fn $fn(
            items: &[$lib::MatchItem],
            tracks: &[$lib::MatchTrack],
            config: &$lib::MatchConfig,
        ) -> ResultValue<Vec<u64>> {
            let mut values = Vec::with_capacity(items.len().saturating_mul(tracks.len()));
            for item in items {
                for track in tracks {
                    values.push(
                        $lib::track_distance(item, track, false, config)
                            .and_then(|x| x.score(config))
                            .map_err(err)?,
                    );
                }
            }
            Ok(bits(values))
        }
    };
}
matrix!(matrix_base, base_match);
fn matrix_weights(
    items: &[trial_match_weights::MatchItem],
    tracks: &[trial_match_weights::MatchTrack],
    config: &trial_match_weights::MatchConfig,
) -> ResultValue<Vec<u64>> {
    trial_match_weights::experimental_matrix(items, tracks, config)
        .map(bits)
        .map_err(err)
}

macro_rules! rank_snapshot {
 ($fn:ident,$lib:ident)=>{
 fn $fn(raw:&Value)->ResultValue<Value>{
  let items:Vec<$lib::MatchItem>=serde_json::from_value(raw.clone()).map_err(err)?;
  let tracks:Vec<$lib::MatchTrack>=items.iter().map(|x|$lib::MatchTrack {
   title:x.title.clone(),artist:Some(x.artist.clone()),length:Some(x.length),index:Some(x.track),medium:Some(x.disc),medium_index:Some(x.track),track_id:Some(x.track_id.clone()),data_source:x.data_source.clone()
  }).collect();
  let first=items.first().ok_or("empty ranking sample")?;
  let album=$lib::MatchAlbum {title:first.album.clone(),artist:first.album_artist.clone(),album_id:Some(first.album_id.clone()),tracks,..$lib::MatchAlbum::default()};
  let cfg=muzik_core::BeetsConfig::from_layers("",json!({})).map_err(err)?;
  let config=$lib::MatchConfig::from_beets(&cfg).map_err(err)?;
  let result=$lib::rank_albums(&items,&[album],&config).map_err(err)?;
  let mut candidates=Vec::new();
  for candidate in result.candidates {
   let penalty_values:Vec<_>=candidate.distance.penalties().iter().map(|(key,values)|(format!("{key:?}"),values.iter().map(|x|x.to_bits()).collect::<Vec<_>>())).collect();
   candidates.push(json!({"input_index":candidate.input_index,"pairs":candidate.assignment.pairs,"extra_items":candidate.assignment.extra_items,"extra_tracks":candidate.assignment.extra_tracks,"score_bits":candidate.distance.score(&config).map_err(err)?.to_bits(),"penalties":penalty_values}));
  }
  Ok(json!({"recommendation":format!("{:?}",result.recommendation),"candidates":candidates}))
 }
 }
}
rank_snapshot!(rank_base, base_match);
rank_snapshot!(rank_stack, trial_match_stack);
rank_snapshot!(rank_weights, trial_match_weights);
fn matching(name: &str, raw: &Value, bench: bool) -> ResultValue<()> {
    let values = raw.as_array().ok_or("match records must be an array")?;
    let track_values:Vec<_>=values.iter().map(|x|json!({
        "title":x["title"],"artist":x["artist"],"length":x["length"],"index":x["track"],
        "medium":x["disc"],"medium_index":x["track"],"track_id":x["track_id"],"data_source":x["data_source"]
    })).collect();
    let track_raw = Value::Array(track_values);
    let items: Vec<base_match::MatchItem> = serde_json::from_value(raw.clone()).map_err(err)?;
    let tracks: Vec<base_match::MatchTrack> =
        serde_json::from_value(track_raw.clone()).map_err(err)?;
    let stack_items: Vec<trial_match_stack::MatchItem> =
        serde_json::from_value(raw.clone()).map_err(err)?;
    let stack_tracks: Vec<trial_match_stack::MatchTrack> =
        serde_json::from_value(track_raw.clone()).map_err(err)?;
    let weight_items: Vec<trial_match_weights::MatchItem> =
        serde_json::from_value(raw.clone()).map_err(err)?;
    let weight_tracks: Vec<trial_match_weights::MatchTrack> =
        serde_json::from_value(track_raw).map_err(err)?;
    let cfg = muzik_core::BeetsConfig::from_layers("", json!({})).map_err(err)?;
    let mut base_cfg = base_match::MatchConfig::from_beets(&cfg).map_err(err)?;
    let mut stack_cfg = trial_match_stack::MatchConfig::from_beets(&cfg).map_err(err)?;
    let mut weight_cfg = trial_match_weights::MatchConfig::from_beets(&cfg).map_err(err)?;
    let count = items.len();
    let iterations = if count > 30 { 1 } else { 5 };
    run(
        "stack-track-score",
        name,
        count,
        iterations,
        bench,
        || matrix_base(black_box(&items), black_box(&tracks), black_box(&base_cfg)),
        || {
            trial_match_stack::experimental_matrix(
                black_box(&stack_items),
                black_box(&stack_tracks),
                black_box(&stack_cfg),
            )
            .map(bits)
            .map_err(err)
        },
    )?;
    run(
        "indexed-weights",
        name,
        count,
        iterations,
        bench,
        || matrix_base(black_box(&items), black_box(&tracks), black_box(&base_cfg)),
        || {
            matrix_weights(
                black_box(&weight_items),
                black_box(&weight_tracks),
                black_box(&weight_cfg),
            )
        },
    )?;
    // Full public rank path checks include assignment, recommendation, and float bits.
    run(
        "stack-track-score",
        &format!("{name}/public-rank"),
        count,
        1,
        false,
        || rank_base(raw),
        || rank_stack(raw),
    )?;
    run(
        "indexed-weights",
        &format!("{name}/public-rank"),
        count,
        1,
        false,
        || rank_base(raw),
        || rank_weights(raw),
    )?;
    // Error control: a used weight is absent. No metadata is fabricated.
    base_cfg
        .distance_weights
        .remove(&base_match::DistanceKey::TrackTitle);
    stack_cfg
        .distance_weights
        .remove(&trial_match_stack::DistanceKey::TrackTitle);
    weight_cfg
        .distance_weights
        .remove(&trial_match_weights::DistanceKey::TrackTitle);
    run(
        "stack-track-score",
        &format!("{name}/missing-weight"),
        count,
        1,
        false,
        || matrix_base(&items, &tracks, &base_cfg),
        || {
            trial_match_stack::experimental_matrix(&stack_items, &stack_tracks, &stack_cfg)
                .map(bits)
                .map_err(err)
        },
    )?;
    run(
        "indexed-weights",
        &format!("{name}/missing-weight"),
        count,
        1,
        false,
        || matrix_base(&items, &tracks, &base_cfg),
        || matrix_weights(&weight_items, &weight_tracks, &weight_cfg),
    )?;
    Ok(())
}
fn library(root: &Path, bench: bool, selected: &[(String, i64, Vec<usize>)]) -> ResultValue<()> {
    let path = root.join("library.db");
    let base = base_library::Library::open_read_only(&path).map_err(err)?;
    let moved = trial_library_move::Library::open_read_only(&path).map_err(err)?;
    let batch = trial_library_batch::Library::open_read_only(&path).map_err(err)?;
    let all_count = base.items().map_err(err)?.len();
    run(
        "attribute-ownership",
        "all-items",
        all_count,
        1,
        bench,
        || {
            base.items()
                .map(|xs| {
                    xs.into_iter()
                        .map(|x| (x.id, x.fields, x.attributes))
                        .collect::<ItemRows>()
                })
                .map_err(err)
        },
        || {
            moved
                .items()
                .map(|xs| {
                    xs.into_iter()
                        .map(|x| (x.id, x.fields, x.attributes))
                        .collect::<ItemRows>()
                })
                .map_err(err)
        },
    )?;
    let count = base.albums().map_err(err)?.len();
    run(
        "attribute-ownership",
        "all-albums",
        count,
        1,
        bench,
        || {
            base.albums()
                .map(|xs| {
                    xs.into_iter()
                        .map(|x| (x.id, x.fields, x.attributes))
                        .collect::<ItemRows>()
                })
                .map_err(err)
        },
        || {
            moved
                .albums()
                .map(|xs| {
                    xs.into_iter()
                        .map(|x| (x.id, x.fields, x.attributes))
                        .collect::<ItemRows>()
                })
                .map_err(err)
        },
    )?;
    for (name, id, indices) in selected {
        run(
            "batch-album-attributes",
            name,
            indices.len(),
            3,
            bench,
            || {
                base.items_for_album(*id)
                    .map(|xs| {
                        xs.into_iter()
                            .map(|x| (x.id, x.fields, x.attributes))
                            .collect::<ItemRows>()
                    })
                    .map_err(err)
            },
            || {
                batch
                    .items_for_album(*id)
                    .map(|xs| {
                        xs.into_iter()
                            .map(|x| (x.id, x.fields, x.attributes))
                            .collect::<ItemRows>()
                    })
                    .map_err(err)
            },
        )?;
    }
    // Check every real album, including all custom attributes, outside timing.
    let all_ids: Vec<_> = base
        .albums()
        .map_err(err)?
        .into_iter()
        .map(|a| a.id)
        .collect();
    run(
        "batch-album-attributes",
        "all-albums-output",
        all_count,
        1,
        false,
        || {
            all_ids
                .iter()
                .map(|id| {
                    base.items_for_album(*id)
                        .map(|xs| {
                            xs.into_iter()
                                .map(|x| (x.id, x.fields, x.attributes))
                                .collect::<ItemRows>()
                        })
                        .map_err(err)
                })
                .collect::<ResultValue<Vec<_>>>()
        },
        || {
            all_ids
                .iter()
                .map(|id| {
                    batch
                        .items_for_album(*id)
                        .map(|xs| {
                            xs.into_iter()
                                .map(|x| (x.id, x.fields, x.attributes))
                                .collect::<ItemRows>()
                        })
                        .map_err(err)
                })
                .collect::<ResultValue<Vec<_>>>()
        },
    )?;
    // Missing album is a current public reader contract.
    let mut missing = i64::MAX;
    while base.albums().map_err(err)?.iter().any(|x| x.id == missing) {
        missing -= 1;
    }
    run(
        "batch-album-attributes",
        "missing-album",
        0,
        1,
        false,
        || {
            base.items_for_album(missing)
                .map(|xs| {
                    xs.into_iter()
                        .map(|x| (x.id, x.fields, x.attributes))
                        .collect::<ItemRows>()
                })
                .map_err(err)
        },
        || {
            batch
                .items_for_album(missing)
                .map(|xs| {
                    xs.into_iter()
                        .map(|x| (x.id, x.fields, x.attributes))
                        .collect::<ItemRows>()
                })
                .map_err(err)
        },
    )?;
    Ok(())
}
fn source_indices(name: &str, raw: &Value, bench: bool) -> ResultValue<()> {
    let items: Vec<base_match::MatchItem> = serde_json::from_value(raw.clone()).map_err(err)?;
    let first = items.first().ok_or("empty selected group")?;
    let tracks: Vec<_> = items
        .iter()
        .map(|x| muzik_core::TrackCandidate {
            recording_id: (!x.track_id.is_empty())
                .then(|| muzik_core::RecordingId(x.track_id.clone())),
            release_track_id: None,
            title: x.title.clone(),
            artist: x.artist.clone(),
            length_seconds: Some(x.length),
            index: x.track,
            medium: x.disc,
            medium_index: x.track,
        })
        .collect();
    let release = muzik_core::ReleaseCandidate {
        id: muzik_core::ReleaseId(first.album_id.clone()),
        title: first.album.clone(),
        artist: first.album_artist.clone(),
        tracks,
        release_group_id: None,
        year: Some(first.year),
        country: Some(first.country.clone()),
        media: Some(first.media.clone()),
        label: Some(first.label.clone()),
        catalog_number: Some(first.catalog_number.clone()),
        disambiguation: Some(first.album_disambiguation.clone()),
        is_various_artists: first.compilation,
    };
    let album = base_import::mbid_fill::MissingAlbum {
        album_id: 0,
        artist: first.artist.clone(),
        album: first.album.clone(),
        year: Some(first.year.into()),
        items: items
            .iter()
            .enumerate()
            .map(|(id, x)| base_import::mbid_fill::MissingItem {
                id: id as i64,
                path: PathBuf::new(),
                match_item: x.clone(),
            })
            .collect(),
    };
    let trial_album = trial_import_index::mbid_fill::MissingAlbum {
        album_id: album.album_id,
        artist: album.artist.clone(),
        album: album.album.clone(),
        year: album.year,
        items: album
            .items
            .iter()
            .map(|x| trial_import_index::mbid_fill::MissingItem {
                id: x.id,
                path: x.path.clone(),
                match_item: x.match_item.clone(),
            })
            .collect(),
    };
    let ids = muzik_metadata::ReleaseArtistIds::default();
    let count = items.len();
    for variant in ["reversed", "unmatched-and-duplicate", "invalid-first"] {
        let mut pairs: Vec<_> = (0..count).rev().map(|x| (x, x)).collect();
        if variant == "unmatched-and-duplicate" {
            pairs.retain(|(x, _)| x % 2 == 0);
            pairs.push((0, count)); // First pair must win, even when a later pair is invalid.
            pairs.push((count, 0)); // Source outside the input must be ignored.
        }
        if variant == "invalid-first" {
            pairs.insert(0, (0, count));
        }
        let base_candidate = base_import::plan::PlannedCandidate {
            release: release.clone(),
            distance: 0.0,
            assignment: base_match::Assignment {
                pairs: pairs.clone(),
                extra_items: vec![],
                extra_tracks: vec![],
            },
        };
        let trial_candidate = trial_import_index::plan::PlannedCandidate {
            release: release.clone(),
            distance: 0.0,
            assignment: base_match::Assignment {
                pairs,
                extra_items: vec![],
                extra_tracks: vec![],
            },
        };
        let workload = format!("{name}/{variant}");
        run(
            "source-index-array",
            &workload,
            count,
            10,
            bench && variant != "invalid-first",
            || {
                let x = base_import::mbid_fill::mbid_fields(
                    black_box(&album),
                    black_box(&base_candidate),
                    black_box(&ids),
                );
                Ok((x.album, x.items))
            },
            || {
                let x = trial_import_index::mbid_fill::mbid_fields(
                    black_box(&trial_album),
                    black_box(&trial_candidate),
                    black_box(&ids),
                );
                Ok((x.album, x.items))
            },
        )?;
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let bench = args.iter().any(|x| x == "--bench");
    let sample = args
        .iter()
        .position(|x| x == "--sample")
        .and_then(|i| args.get(i + 1))
        .ok_or("use --check or --bench --sample PATH")?;
    let root = Path::new(sample);
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(root.join("tracks.json"))?)?;
    let matches: Vec<Value> =
        serde_json::from_slice(&std::fs::read(root.join("match_items.json"))?)?;
    if rows.len() != matches.len() {
        return Err("sample arrays have different lengths".into());
    }
    let mut groups: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        if let Some(id) = row.get("album_id").and_then(Value::as_i64) {
            groups.entry(id).or_default().push(index);
        }
    }
    let mut sorted: Vec<_> = groups.into_iter().collect();
    sorted.sort_by_key(|(id, xs)| (xs.len(), *id));
    if sorted.is_empty() {
        return Err("sample contains no album groups".into());
    }
    let multi: Vec<_> = sorted.iter().filter(|(_, xs)| xs.len() > 1).collect();
    let typical = multi.get(multi.len() / 2).copied().unwrap_or(&sorted[0]);
    let picked = [
        ("small", &sorted[0]),
        ("typical-multi", typical),
        ("largest", sorted.last().unwrap()),
    ];
    let mut selected: Vec<_> = picked
        .iter()
        .map(|(name, (id, xs))| ((*name).to_string(), *id, xs.clone()))
        .collect();
    if let Some((id, indices)) = sorted.iter().rev().find(|(_, indices)| {
        indices.iter().any(|&i| {
            matches[i]["track_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        })
    }) {
        selected.push(("tagged".to_owned(), *id, indices.clone()));
    }
    println!(
        "{}",
        json!({"mode":"sample","tracks":rows.len(),"album_groups":sorted.len(),"selected":selected.iter().map(|(n,_,xs)|json!({"name":n,"tracks":xs.len()})).collect::<Vec<_>>() })
    );
    library(root, bench, &selected)?;
    for (name, _, indices) in &selected {
        let data = Value::Array(indices.iter().map(|&i| matches[i].clone()).collect());
        matching(name, &data, bench)?;
        source_indices(name, &data, bench)?;
    }
    Ok(())
}
