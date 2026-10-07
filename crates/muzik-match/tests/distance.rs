use muzik_core::BeetsConfig;
use muzik_match::{
    Distance, MatchAlbum, MatchConfig, MatchItem, MatchTrack, album_distance, track_distance,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Fixture {
    beets_version: String,
    current_year: i32,
    tracks: Vec<TrackCase>,
    albums: Vec<AlbumCase>,
}

#[derive(Deserialize)]
struct Expected {
    score: f64,
    penalties: Value,
    #[serde(default)]
    tracks: Vec<Expected>,
}

#[derive(Deserialize)]
struct TrackCase {
    name: String,
    item: MatchItem,
    track: MatchTrack,
    include_artist: bool,
    expected: Expected,
}

#[derive(Deserialize)]
struct AlbumCase {
    name: String,
    items: Vec<MatchItem>,
    album: MatchAlbum,
    pairs: Vec<(usize, usize)>,
    #[serde(default)]
    preferred: Value,
    expected: Expected,
}

fn compare(name: &str, actual: &Distance, expected: &Expected, config: &MatchConfig) {
    assert_eq!(
        actual.score(config).ok(),
        Some(expected.score),
        "{name} score"
    );
    let penalties: Value = actual
        .penalties()
        .iter()
        .map(|(key, values)| (key.to_string(), json!(values)))
        .collect::<serde_json::Map<_, _>>()
        .into();
    assert_eq!(penalties, expected.penalties, "{name} penalties");
    assert_eq!(
        actual.tracks.len(),
        expected.tracks.len(),
        "{name} track count"
    );
    for (index, (actual, expected)) in actual.tracks.iter().zip(&expected.tracks).enumerate() {
        compare(&format!("{name} track {index}"), actual, expected, config);
    }
}

#[test]
fn matches_beets_track_and_album_distance() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/distance.json")).unwrap();
    assert_eq!(fixture.beets_version, "2.13.1");
    let defaults = BeetsConfig::from_layers("", json!({})).unwrap();
    let mut config = MatchConfig::from_beets(&defaults).unwrap();
    config.current_year = fixture.current_year;
    for case in fixture.tracks {
        let actual = track_distance(&case.item, &case.track, case.include_artist, &config).unwrap();
        compare(&case.name, &actual, &case.expected, &config);
    }
    for case in fixture.albums {
        let beets =
            BeetsConfig::from_layers("", json!({"match": {"preferred": case.preferred}})).unwrap();
        let mut config = MatchConfig::from_beets(&beets).unwrap();
        config.current_year = fixture.current_year;
        let actual = album_distance(&case.items, &case.album, &case.pairs, &config).unwrap();
        compare(&case.name, &actual, &case.expected, &config);
    }
}
