//! Run with `cargo bench -p muzik-match --bench assignment --locked --offline`.
use std::hint::black_box;
use std::time::Instant;

use muzik_core::BeetsConfig;
use muzik_match::{MatchConfig, MatchItem, MatchTrack, assign_items};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    assignments: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    items: Vec<MatchItem>,
    tracks: Vec<MatchTrack>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture: Fixture = serde_json::from_str(include_str!("../tests/fixtures/ranking.json"))?;
    let config = MatchConfig::from_beets(&BeetsConfig::from_layers("", serde_json::json!({}))?)?;
    let mut cases = fixture.assignments;
    for (items, tracks) in [
        (0, 0),
        (0, 65),
        (65, 0),
        (1, 1),
        (12, 12),
        (64, 65),
        (65, 64),
    ] {
        cases.push(Case {
            name: format!("generated {items}x{tracks}"),
            items: (0..items).map(|_| MatchItem::default()).collect(),
            tracks: (0..tracks).map(|_| MatchTrack::default()).collect(),
        });
    }
    for case in cases {
        // Fixed iterations and fresh assignment state on every call.
        let iterations = if case.items.len().max(case.tracks.len()) > 60 {
            100
        } else {
            2_000
        };
        for _ in 0..10 {
            black_box(assign_items(&case.items, &case.tracks, &config)?);
        }
        let mut samples = Vec::new();
        for _ in 0..7 {
            let start = Instant::now();
            for _ in 0..iterations {
                black_box(assign_items(
                    black_box(&case.items),
                    black_box(&case.tracks),
                    black_box(&config),
                )?);
            }
            samples.push(
                start
                    .elapsed()
                    .as_nanos()
                    .checked_div(iterations)
                    .ok_or("zero benchmark iterations")?,
            );
        }
        samples.sort_unstable();
        println!("{}: ns/op {samples:?}", case.name);
    }
    Ok(())
}
