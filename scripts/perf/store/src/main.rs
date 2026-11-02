use muzik_core::{PreferredAudio, audio::Codec};
use muzik_soulseek::{
    ranking::{self, RankedCandidate},
    types::{Candidate, FileEntry},
};
use muzik_store::jobs::{Kind, Status, Store};
use serde_json::{Value, json};
use std::{error::Error, fs, hint::black_box, path::Path, time::Instant};
pub mod types {
    pub use muzik_soulseek::types::*;
}
mod ranking_candidate;
mod snapshot;
mod upgrade_candidate;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn projected_snapshot(c: &rusqlite::Connection) -> Value {
    let open=(||->rusqlite::Result<Vec<Value>>{
        let mut s=c.prepare("SELECT id,title,kind,status,item_key,queue,json_valid(params) AND (question IS NULL OR json_valid(question)) AND (answer IS NULL OR json_valid(answer)) FROM jobs WHERE status IN ('queued','running') ORDER BY id")?;
        s.query_map([],|r|{let kind:Kind=r.get(2)?;let status:Status=r.get(3)?; let _:muzik_store::jobs::Queue=r.get(5)?; if !r.get::<_,bool>(6)? {return Err(rusqlite::Error::InvalidQuery);}
            Ok(json!({"job_id":format!("queue-{}",r.get::<_,i64>(0)?),"title":r.get::<_,String>(1)?,"kind":kind.as_ref(),"status":status.as_ref(),"item":(kind==Kind::Item).then_some(r.get::<_,String>(4)?)}))})?.collect()
    })().unwrap_or_default();
    let waiting=(||->rusqlite::Result<Vec<Value>>{
        let mut s=c.prepare("SELECT id,title,question,item_key,queue,kind,json_valid(params) AND (question IS NULL OR json_valid(question)) AND (answer IS NULL OR json_valid(answer)) FROM jobs WHERE status='waiting' ORDER BY id")?;
        s.query_map([],|r|{let _:muzik_store::jobs::Queue=r.get(4)?; let _:Kind=r.get(5)?; if !r.get::<_,bool>(6)? {return Err(rusqlite::Error::InvalidQuery);} let q=r.get::<_,Option<Value>>(2)?.unwrap_or(Value::Null);
            Ok(json!({"id":r.get::<_,i64>(0)?,"title":r.get::<_,String>(1)?,"kind":q.get("kind"),"payload":q.get("payload"),"item":r.get::<_,String>(3)?}))})?.collect()
    })().unwrap_or_default();
    json!({"open":open,"waiting":waiting})
}
fn seed_jobs(rows: &[Value], scale: usize) -> Result<rusqlite::Connection> {
    let c = muzik_store::db::open_in_memory()?;
    for (i, row) in rows.iter().cycle().take(scale).enumerate() {
        let params = json!({"track":row,"source":"generated-state"});
        let question = (i % 3 == 2)
            .then(|| json!({"kind":"choose","payload":{"track":row},"extra":"preserve"}));
        c.execute("INSERT INTO jobs(id,queue,kind,item_key,title,status,params,question,answer,error,created_at,updated_at) VALUES(?1,'item','item',?2,?3,?4,?5,?6,?7,?8,0,0)",
            rusqlite::params![i as i64,format!("sample-{i}"),text(row,"title"),["queued","running","waiting"][i%3],params,question,json!({"saved":row}),"generated error"])?;
    }
    Ok(c)
}
fn text<'a>(row: &'a Value, key: &str) -> &'a str {
    row.get(key).and_then(Value::as_str).unwrap_or("")
}
fn number(row: &Value, key: &str) -> u32 {
    row.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}
fn file(row: &Value, i: usize) -> FileEntry {
    let parent = if i % 9 == 0 { "" } else { "sample" };
    FileEntry {
        name: format!(
            "{parent}{}{} {} {}.flac",
            if parent.is_empty() { "" } else { "/" },
            text(row, "artist"),
            text(row, "album"),
            text(row, "title")
        ),
        size: 1000,
        bitrate_kbps: Some(900),
        duration_seconds: row
            .get("length")
            .and_then(Value::as_f64)
            .map(|v| v.max(0.0).round() as u32),
        vbr: None,
        sample_rate_hz: Some(44100),
        bit_depth: Some(16),
    }
}
fn peers(rows: &[Value], n: usize) -> Vec<Candidate> {
    (0..n)
        .map(|i| Candidate {
            username: format!("peer-{}", i % 7),
            slots: (i % 3) as u8,
            speed: 100000,
            files: rows
                .iter()
                .cycle()
                .skip(i)
                .take(1 + i % 11)
                .enumerate()
                .map(|(j, r)| file(r, i + j))
                .collect(),
        })
        .collect()
}
fn ranked_json(x: &[RankedCandidate]) -> Value {
    Value::Array(
        x.iter()
            .map(|r| json!({"candidate":r.candidate,"score":r.score.to_bits()}))
            .collect(),
    )
}
fn selected_json(x: Option<(Candidate, f64)>) -> Value {
    x.map_or(
        Value::Null,
        |(c, s)| json!({"candidate":c,"score":s.to_bits()}),
    )
}
fn flagged(row: &Value) -> muzik_workflow::upgrade::FlaggedTrack {
    muzik_workflow::upgrade::FlaggedTrack {
        artist: text(row, "artist").into(),
        sort_artist: text(row, "albumartist").into(),
        album: text(row, "album").into(),
        title: text(row, "title").into(),
        duration: row.get("length").and_then(Value::as_f64),
        quality: muzik_media::quality::MeasuredQuality {
            format: Codec::Mp3,
            lossless: false,
            bitrate_kbps: Some(128),
            sample_rate: Some(44100),
            bit_depth: None,
            channels: Some(2),
            size: None,
        },
    }
}
fn watchlist(rows: &[Value]) -> Value {
    let items:Vec<_>=rows.iter().enumerate().map(|(i,row)|{
        let status=["not_started","running","waiting","failed","complete","stale"][i%6];
        json!({"position":i+1,"title":text(row,"title"),"kind":(["youtube","spotify","bandcamp"][i%3]),"video_id":format!("sample{i:05}"),"video_url":"https://example.test/generated","track":row,"stages":{"download":{"status":status},"quality":{"status":status},"parse":{"status":status},"split":{"status":status},"organize":{"status":status}}})
    }).collect();
    json!({"version":3,"playlists":[{"playlist_id":"generated","url":"https://example.test/generated","items":items}]})
}
fn check(rows: &[Value], scratch: &Path) -> Result<()> {
    for n in [0, 1, 20, 200] {
        let raw = peers(rows, n);
        for query in [
            "",
            "the",
            "Music flac",
            &format!("{} {}", text(&rows[0], "artist"), text(&rows[0], "title")),
        ] {
            let b = ranking::rank(raw.clone(), query, PreferredAudio::Lossless, 20);
            let c = ranking_candidate::rank(raw.clone(), query, PreferredAudio::Lossless, 20);
            assert_eq!(ranked_json(&b), ranked_json(&c), "ranking {n}");
        }
        let base = Store::from_connection(seed_jobs(rows, n)?);
        let candidate = seed_jobs(rows, n)?;
        assert_eq!(
            snapshot::baseline_snapshot(&base),
            projected_snapshot(&candidate),
            "snapshot {n}"
        );
    }
    // Corrupt persisted fields and missing tables must retain snapshot fallback behavior.
    for status in ["queued", "waiting"] {
        for column in ["params", "question", "answer", "kind", "queue"] {
            let b = seed_jobs(rows, 3)?;
            let c = seed_jobs(rows, 3)?;
            let query = format!("UPDATE jobs SET {column}='invalid' WHERE status=?1");
            b.execute(&query, [status])?;
            c.execute(&query, [status])?;
            assert_eq!(
                snapshot::baseline_snapshot(&Store::from_connection(b)),
                projected_snapshot(&c),
                "snapshot error {status}/{column}"
            );
        }
    }
    // Report a candidate contract failure without stopping unrelated experiments.
    for status in ["queued", "waiting"] {
        let b = seed_jobs(rows, 3)?;
        let c = seed_jobs(rows, 3)?;
        let query = "UPDATE jobs SET params='1e9999' WHERE status=?1";
        b.execute(query, [status])?;
        c.execute(query, [status])?;
        let expected = snapshot::baseline_snapshot(&Store::from_connection(b));
        let actual = projected_snapshot(&c);
        println!(
            "{}",
            json!({"candidate":"job_projection","mode":"boundary","case":format!("{status}_overflowing_json_number"),"correctness_equal":expected==actual,"states":"generated corruption control"})
        );
    }
    let b = seed_jobs(rows, 0)?;
    let c = seed_jobs(rows, 0)?;
    b.execute("DROP TABLE jobs", [])?;
    c.execute("DROP TABLE jobs", [])?;
    assert_eq!(
        snapshot::baseline_snapshot(&Store::from_connection(b)),
        projected_snapshot(&c)
    );
    // Exhaustive five-stage state priority. These are generated states, not a Music workload.
    let raw = watchlist(&rows[..1]);
    let mut bi: muzik_store::watchlist::WatchItem =
        serde_json::from_value(raw["playlists"][0]["items"][0].clone())?;
    let mut ci: muzik_store_dod::watchlist::WatchItem =
        serde_json::from_value(raw["playlists"][0]["items"][0].clone())?;
    let statuses = [
        "not_started",
        "running",
        "waiting",
        "complete",
        "failed",
        "skipped",
        "stale",
    ];
    for mut code in 0..7usize.pow(5) {
        for (bs, cs) in muzik_store::watchlist::Stage::ALL
            .iter()
            .zip(muzik_store_dod::watchlist::Stage::ALL)
        {
            let status = statuses[code % 7];
            code /= 7;
            bi.stage_mut(*bs).status = status.parse()?;
            ci.stage_mut(*cs).status = status.parse()?;
        }
        assert_eq!(
            serde_json::to_value(muzik_store::watchlist::Summary::of(&bi))?,
            serde_json::to_value(muzik_store_dod::watchlist::Summary::of(&ci))?
        );
    }
    bi.unavailable = Some(true);
    ci.unavailable = Some(true);
    assert_eq!(
        serde_json::to_value(muzik_store::watchlist::Summary::of(&bi))?,
        serde_json::to_value(muzik_store_dod::watchlist::Summary::of(&ci))?
    );
    // Generated peers use sampled text and duration. Additional controls check exact boundaries and equal winners.
    for row in rows.iter().take(80) {
        let mut track = flagged(row);
        let mut raw = peers(std::slice::from_ref(row), 8);
        raw.push(Candidate {
            username: "".into(),
            slots: 1,
            speed: 100,
            files: vec![file(row, 1)],
        });
        raw.push(Candidate {
            username: "equal-a".into(),
            slots: 1,
            speed: 100,
            files: vec![file(row, 1)],
        });
        raw.push(Candidate {
            username: "equal-b".into(),
            slots: 1,
            speed: 100,
            files: vec![file(row, 1)],
        });
        if let Some(last) = raw.last_mut() {
            last.files[0].duration_seconds = Some(100);
        }
        let ranked: Vec<_> = raw
            .into_iter()
            .map(|candidate| RankedCandidate {
                candidate,
                score: 1.0,
            })
            .collect();
        for duration in [None, Some(90.0), Some(89.999), Some(110.0), Some(110.001)] {
            track.duration = duration;
            for prefer in [PreferredAudio::Lossless, PreferredAudio::Mp3_320] {
                let b = muzik_workflow::upgrade::select_upgrade(&track, &ranked, prefer);
                let c = upgrade_candidate::select_upgrade(&track, &ranked, prefer);
                assert_eq!(selected_json(b), selected_json(c), "upgrade");
            }
        }
    }
    // Ensure that the upgrade controls include an accepted match and the last equal winner.
    let positive = rows
        .iter()
        .find(|r| {
            !upgrade_candidate::tokens(text(r, "artist")).is_empty()
                && !upgrade_candidate::tokens(text(r, "title")).is_empty()
        })
        .ok_or("no usable sampled identity")?;
    let mut track = flagged(positive);
    track.duration = None;
    let same = file(positive, 1);
    let tied: Vec<_> = ["equal-first", "equal-last"]
        .into_iter()
        .map(|name| RankedCandidate {
            candidate: Candidate {
                username: name.into(),
                slots: 1,
                speed: 100,
                files: vec![same.clone()],
            },
            score: 1.0,
        })
        .collect();
    let original = muzik_workflow::upgrade::select_upgrade(&track, &tied, PreferredAudio::Lossless)
        .ok_or("positive upgrade control rejected")?;
    assert_eq!(original.0.username, "equal-last");
    assert_eq!(
        selected_json(Some(original)),
        selected_json(upgrade_candidate::select_upgrade(
            &track,
            &tied,
            PreferredAudio::Lossless
        ))
    );
    for n in [0, 1, 20, 200] {
        let raw = watchlist(&rows[..rows.len().min(n)]);
        let b: muzik_store::watchlist::Watchlist = serde_json::from_value(raw.clone())?;
        let c: muzik_store_dod::watchlist::Watchlist = serde_json::from_value(raw)?;
        let bv = serde_json::to_value(muzik_store::watchlist::view(&b, scratch, scratch))?;
        let cv = serde_json::to_value(muzik_store_dod::watchlist::view(&c, scratch, scratch))?;
        assert_eq!(bv, cv, "watchlist {n}");
        if let Some(first) = bv.pointer("/playlists/0/items/0") {
            for actions in [
                json!({}),
                json!({"run":{"enabled":true,"reason":null}}),
                json!({"unknown_action":{"enabled":true}}),
                json!({"run":{"enabled":false,"reason":"why"},"retry":{"enabled":true}}),
            ] {
                let mut sparse = first.clone();
                sparse["actions"] = actions;
                let b = serde_json::from_value::<muzik_store::watchlist::ItemView>(sparse.clone());
                let c = serde_json::from_value::<muzik_store_dod::watchlist::ItemView>(sparse);
                assert_eq!(b.is_ok(), c.is_ok(), "sparse parse");
                if let (Ok(b), Ok(c)) = (b, c) {
                    assert_eq!(serde_json::to_value(b)?, serde_json::to_value(c)?);
                }
            }
        }
    }
    println!(
        "{}",
        json!({"mode":"check","status":"valid_cases_pass_boundary_results_reported","source":"Music metadata; generated peers, jobs, watchlist states"})
    );
    Ok(())
}
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct CountAllocator;
static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
#[global_allocator]
static ALLOCATOR: CountAllocator = CountAllocator;
unsafe impl GlobalAlloc for CountAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(size, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, layout, size) }
    }
}
fn allocations<T>(f: &mut impl FnMut() -> T, iterations: usize) -> (f64, f64) {
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    for _ in 0..iterations {
        black_box(f());
    }
    COUNTING.store(false, Ordering::Relaxed);
    (
        ALLOCS.load(Ordering::Relaxed) as f64 / iterations as f64,
        BYTES.load(Ordering::Relaxed) as f64 / iterations as f64,
    )
}

fn measure<T>(f: &mut impl FnMut() -> T, iterations: usize) -> u128 {
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(f());
    }
    start.elapsed().as_nanos() / iterations as u128
}
fn bench<T, U>(
    label: &str,
    workload: &str,
    iterations: usize,
    mut before: impl FnMut() -> T,
    mut after: impl FnMut() -> U,
) {
    for _ in 0..3 {
        black_box(before());
        black_box(after());
    }
    let (before_alloc_count, before_alloc_bytes) = allocations(&mut before, iterations);
    let (after_alloc_count, after_alloc_bytes) = allocations(&mut after, iterations);
    for sample in 0..7 {
        let (b, c) = if sample % 2 == 0 {
            (
                measure(&mut before, iterations),
                measure(&mut after, iterations),
            )
        } else {
            let c = measure(&mut after, iterations);
            (measure(&mut before, iterations), c)
        };
        println!(
            "{}",
            json!({"candidate":label,"workload":workload,"sample":sample,"iterations":iterations,"baseline_ns":b,"candidate_ns":c,"before_allocs":before_alloc_count,"after_allocs":after_alloc_count,"before_alloc_bytes":before_alloc_bytes,"after_alloc_bytes":after_alloc_bytes,"states":"generated from Music metadata"})
        );
    }
}
fn benchmarks(rows: &[Value], scratch: &Path) -> Result<()> {
    for n in [20, 200, 1000] {
        let b = Store::from_connection(seed_jobs(rows, n)?);
        let c = seed_jobs(rows, n)?;
        bench(
            "job_projection",
            &format!("jobs-{n}"),
            50,
            || snapshot::baseline_snapshot(&b),
            || projected_snapshot(&c),
        );
    }
    for n in [20, 200, 1000] {
        let input = peers(rows, n);
        let query = format!("{} {}", text(&rows[0], "artist"), text(&rows[0], "title"));
        bench(
            "soulseek_prepared_query_one_pass",
            &format!("peers-{n}"),
            20,
            || ranking::rank(input.clone(), &query, PreferredAudio::Lossless, 20),
            || ranking_candidate::rank(input.clone(), &query, PreferredAudio::Lossless, 20),
        );
    }
    for n in [20, 200, 1000] {
        let raw = watchlist(&rows[..rows.len().min(n)]);
        let b: muzik_store::watchlist::Watchlist = serde_json::from_value(raw.clone())?;
        let c: muzik_store_dod::watchlist::Watchlist = serde_json::from_value(raw)?;
        bench(
            "watchlist_fixed_actions_summary",
            &format!("items-{n}"),
            20,
            || muzik_store::watchlist::view(&b, scratch, scratch),
            || muzik_store_dod::watchlist::view(&c, scratch, scratch),
        );
    }
    for n in [20, 200, 1000] {
        let raw = peers(rows, n);
        let ranked: Vec<_> = raw
            .into_iter()
            .map(|candidate| RankedCandidate {
                candidate,
                score: 1.0,
            })
            .collect();
        let track = flagged(&rows[0]);
        bench(
            "upgrade_index_winner",
            &format!("peers-{n}"),
            20,
            || muzik_workflow::upgrade::select_upgrade(&track, &ranked, PreferredAudio::Lossless),
            || upgrade_candidate::select_upgrade(&track, &ranked, PreferredAudio::Lossless),
        );
    }
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("--check");
    let sample = args
        .get(2)
        .map(String::as_str)
        .unwrap_or("@ROOT@/.tmp/dod-sample/tracks.json");
    let rows: Vec<Value> = serde_json::from_slice(&fs::read(sample)?)?;
    if rows.is_empty() {
        return Err("sample is empty".into());
    }
    let scratch = Path::new("@LAB@/empty");
    fs::create_dir_all(scratch)?;
    match mode {
        "--check" => check(&rows, scratch),
        "--bench" => {
            check(&rows, scratch)?;
            benchmarks(&rows, scratch)
        }
        _ => Err("use --check or --bench [tracks.json]".into()),
    }
}
