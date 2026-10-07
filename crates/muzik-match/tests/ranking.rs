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
    expected: ExpectedRanking,
}

#[derive(Deserialize)]
struct ExpectedRanking {
    ids: Vec<Option<String>>,
    scores: Vec<f64>,
    recommendation: Recommendation,
}

#[test]
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
