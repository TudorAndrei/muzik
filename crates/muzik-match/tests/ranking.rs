use muzik_core::BeetsConfig;
use muzik_match::{
    Error, MatchAlbum, MatchConfig, MatchItem, MatchTrack, Recommendation, assign_items,
    rank_albums, track_distance,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Fixture {
    beets_version: String,
    assignments: Vec<AssignmentCase>,
    rankings: Vec<RankingCase>,
}

#[derive(Deserialize)]
struct AssignmentCase {
    name: String,
    items: Vec<MatchItem>,
    tracks: Vec<MatchTrack>,
    expected: ExpectedAssignment,
}

#[derive(Deserialize)]
struct ExpectedAssignment {
    pairs: Vec<(usize, usize)>,
    extra_items: Vec<usize>,
    extra_tracks: Vec<usize>,
    total_cost: f64,
}

#[derive(Deserialize)]
struct RankingCase {
    name: String,
    items: Vec<MatchItem>,
    albums: Vec<MatchAlbum>,
    #[serde(default = "no_overrides")]
    config: serde_json::Value,
    expected: ExpectedRanking,
}

fn no_overrides() -> serde_json::Value {
    json!({})
}

#[derive(Deserialize)]
struct ExpectedRanking {
    ids: Vec<Option<String>>,
    scores: Vec<f64>,
    recommendation: Recommendation,
}

#[test]
#[expect(clippy::float_cmp, reason = "beets parity needs exact float equality")]
fn matches_beets_assignment_and_ranking() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/ranking.json")).unwrap();
    assert_eq!(fixture.beets_version, "2.13.1");
    let beets = BeetsConfig::from_layers("", json!({})).unwrap();
    let config = MatchConfig::from_beets(&beets).unwrap();
    for case in fixture.assignments {
        let assignment = assign_items(&case.items, &case.tracks, &config).unwrap();
        assert_eq!(
            assignment.extra_items, case.expected.extra_items,
            "{} extra items",
            case.name
        );
        assert_eq!(
            assignment.extra_tracks, case.expected.extra_tracks,
            "{} extra tracks",
            case.name
        );
        assert_eq!(
            assignment.pairs.len(),
            case.expected.pairs.len(),
            "{} pair count",
            case.name
        );
        let total_cost = assignment
            .pairs
            .iter()
            .map(|&(item, track)| {
                track_distance(&case.items[item], &case.tracks[track], false, &config)
                    .unwrap()
                    .score(&config)
                    .unwrap()
            })
            .sum::<f64>();
        assert_eq!(
            total_cost, case.expected.total_cost,
            "{} total cost",
            case.name
        );
        if case.name != "equal cost tie" {
            assert_eq!(
                assignment.pairs, case.expected.pairs,
                "{} mapping",
                case.name
            );
        }
    }
    for case in fixture.rankings {
        let beets = BeetsConfig::from_layers("", json!({"match": case.config})).unwrap();
        let config = MatchConfig::from_beets(&beets).unwrap();
        let actual = rank_albums(&case.items, &case.albums, &config).unwrap();
        let ids: Vec<_> = actual
            .candidates
            .iter()
            .map(|match_| match_.album.album_id.clone())
            .collect();
        let scores: Vec<_> = actual
            .candidates
            .iter()
            .map(|match_| match_.distance.score(&config).unwrap())
            .collect();
        assert_eq!(ids, case.expected.ids, "{} order", case.name);
        assert_eq!(scores, case.expected.scores, "{} scores", case.name);
        assert_eq!(
            actual.recommendation, case.expected.recommendation,
            "{} recommendation",
            case.name
        );
    }
}

#[test]
fn rejects_an_unknown_max_rec_limit() {
    let beets =
        BeetsConfig::from_layers("", json!({"match": {"max_rec": {"year": "high"}}})).unwrap();
    assert!(matches!(
        MatchConfig::from_beets(&beets),
        Err(Error::InvalidConfig("match.max_rec"))
    ));
}

#[test]
fn assignment_preserves_pairs_and_extra_order_across_word_boundaries() {
    let beets = BeetsConfig::from_layers("", json!({})).unwrap();
    let config = MatchConfig::from_beets(&beets).unwrap();
    for count in [0_u32, 1, 63, 64, 65, 127, 128, 129] {
        let items: Vec<_> = (0..count)
            .map(|index| MatchItem {
                title: format!("Track {index}"),
                track: count - index,
                ..MatchItem::default()
            })
            .collect();
        let tracks: Vec<_> = (0..count)
            .map(|index| MatchTrack {
                title: format!("Track {index}"),
                index: Some(count - index),
                ..MatchTrack::default()
            })
            .collect();
        let all_extras: Vec<_> = (0..items.len()).rev().collect();
        let without_tracks = assign_items(&items, &[], &config).unwrap();
        assert_eq!(without_tracks.extra_items, all_extras);
        assert!(without_tracks.pairs.is_empty());
        let without_items = assign_items(&[], &tracks, &config).unwrap();
        assert_eq!(without_items.extra_tracks, all_extras);
        assert!(without_items.pairs.is_empty());
        let selected: Vec<_> = (0..items.len()).step_by(2).rev().collect();
        let expected_extra: Vec<_> = (0..items.len())
            .filter(|index| index % 2 != 0)
            .rev()
            .collect();
        let selected_tracks: Vec<_> = selected
            .iter()
            .map(|&index| tracks[index].clone())
            .collect();
        let actual = assign_items(&items, &selected_tracks, &config).unwrap();
        let expected_pairs: Vec<_> = selected
            .iter()
            .enumerate()
            .map(|(track, &item)| (item, track))
            .collect();
        assert_eq!(actual.pairs, expected_pairs, "{count} items");
        assert_eq!(actual.extra_items, expected_extra, "{count} items");
        assert!(actual.extra_tracks.is_empty());

        let selected_items: Vec<_> = selected.iter().map(|&index| items[index].clone()).collect();
        let actual = assign_items(&selected_items, &tracks, &config).unwrap();
        let expected_pairs: Vec<_> = selected
            .iter()
            .enumerate()
            .rev()
            .map(|(item, &track)| (item, track))
            .collect();
        assert_eq!(actual.pairs, expected_pairs, "{count} tracks");
        assert_eq!(actual.extra_tracks, expected_extra, "{count} tracks");
        assert!(actual.extra_items.is_empty());
    }
}
