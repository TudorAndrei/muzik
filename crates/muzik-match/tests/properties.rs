use std::collections::HashSet;

use muzik_core::BeetsConfig;
use muzik_match::{
    Distance, DistanceKey, MatchAlbum, MatchConfig, MatchItem, MatchTrack, album_distance,
    assign_items, rank_albums, string_dist,
};
use proptest::option::of;
use proptest::prelude::*;
use serde_json::json;

fn title() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 &,.()\\[\\]-]{0,24}|(the |a )?[a-z]{1,8}(, the)?( \\(feat\\. [a-z]{1,6}\\))?"
}

fn item() -> impl Strategy<Value = MatchItem> {
    (title(), 0_u32..20, 0.0_f64..600.0).prop_map(|(title, track, length)| MatchItem {
        title,
        track,
        length,
        ..MatchItem::default()
    })
}

fn track() -> impl Strategy<Value = MatchTrack> {
    (
        title(),
        proptest::option::of(1_u32..20),
        proptest::option::of(0.0_f64..600.0),
    )
        .prop_map(|(title, index, length)| MatchTrack {
            title,
            index,
            length,
            ..MatchTrack::default()
        })
}

fn local_item() -> impl Strategy<Value = MatchItem> {
    (
        item(),
        title(),
        title(),
        prop_oneof![0_u32..4, any::<u32>()],
        1950_i32..2030,
    )
        .prop_map(|(item, artist, album, disc_total, year)| MatchItem {
            artist,
            album,
            disc_total,
            year,
            ..item
        })
}

fn album() -> impl Strategy<Value = MatchAlbum> {
    (
        (title(), title(), proptest::collection::vec(track(), 0..8)),
        (of("[a-c]"), of("[xy]"), any::<bool>()),
        (
            of(prop_oneof![1_u32..4, any::<u32>()]),
            of(1950_i32..2030),
            of(1950_i32..2030),
        ),
    )
        .prop_map(
            |(
                (title, artist, tracks),
                (album_id, data_source, various_artists),
                (mediums, year, original_year),
            )| MatchAlbum {
                title,
                artist,
                album_id,
                tracks,
                various_artists,
                mediums,
                year,
                original_year,
                data_source,
                ..MatchAlbum::default()
            },
        )
}

fn default_config() -> Result<MatchConfig, TestCaseError> {
    let beets = BeetsConfig::from_layers("", json!({}))
        .map_err(|error| TestCaseError::fail(error.to_string()))?;
    MatchConfig::from_beets(&beets).map_err(|error| TestCaseError::fail(error.to_string()))
}

fn penalty_count(distance: &Distance, key: DistanceKey) -> usize {
    distance
        .penalties()
        .iter()
        .find(|(name, _)| *name == key)
        .map_or(0, |(_, values)| values.len())
}

fn each_index_once(assigned: impl Iterator<Item = usize>, extra: &[usize], len: usize) -> bool {
    let mut seen: Vec<_> = assigned.chain(extra.iter().copied()).collect();
    seen.sort_unstable();
    seen.into_iter().eq(0..len)
}

proptest! {
    #[test]
    fn string_distance_is_symmetric(left in title(), right in title()) {
        prop_assert_eq!(
            string_dist(Some(&left), Some(&right)).to_bits(),
            string_dist(Some(&right), Some(&left)).to_bits()
        );
    }

    #[test]
    fn a_string_has_no_distance_to_itself(text in title()) {
        prop_assert_eq!(string_dist(Some(&text), Some(&text)).to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn string_distance_is_finite_and_not_negative(left in title(), right in title()) {
        let distance = string_dist(Some(&left), Some(&right));
        prop_assert!(distance.is_finite() && distance >= 0.0, "{distance}");
    }

    #[test]
    fn assignment_uses_each_item_and_track_once(
        items in proptest::collection::vec(item(), 1..8),
        tracks in proptest::collection::vec(track(), 1..8),
    ) {
        let config = default_config()?;
        let assignment = assign_items(&items, &tracks, &config)?;
        prop_assert_eq!(assignment.pairs.len(), items.len().min(tracks.len()));
        prop_assert!(each_index_once(
            assignment.pairs.iter().map(|pair| pair.0),
            &assignment.extra_items,
            items.len(),
        ));
        prop_assert!(each_index_once(
            assignment.pairs.iter().map(|pair| pair.1),
            &assignment.extra_tracks,
            tracks.len(),
        ));
    }

    #[test]
    fn album_distance_counts_each_track_and_scores_within_bounds(
        items in proptest::collection::vec(local_item(), 1..8),
        album in album(),
    ) {
        let config = default_config()?;
        let pairs = assign_items(&items, &album.tracks, &config)?.pairs;
        let distance = album_distance(&items, &album, &pairs, &config)?;
        let score = distance.score(&config)?;
        prop_assert!((0.0..=1.0).contains(&score), "{score}");
        prop_assert_eq!(penalty_count(&distance, DistanceKey::Tracks), pairs.len());
        prop_assert_eq!(
            album.tracks.len().checked_sub(pairs.len()),
            Some(penalty_count(&distance, DistanceKey::MissingTracks))
        );
        prop_assert_eq!(
            items.len().checked_sub(pairs.len()),
            Some(penalty_count(&distance, DistanceKey::UnmatchedTracks))
        );
        prop_assert_eq!(distance.tracks.len(), pairs.len());
    }

    #[test]
    fn ranking_sorts_unique_candidates_by_score(
        items in proptest::collection::vec(local_item(), 1..6),
        albums in proptest::collection::vec(album(), 0..6),
    ) {
        let config = default_config()?;
        let ranking = rank_albums(&items, &albums, &config)?;
        let scores = ranking
            .candidates
            .iter()
            .map(|candidate| candidate.distance.score(&config))
            .collect::<Result<Vec<_>, _>>()?;
        prop_assert!(scores.windows(2).all(|pair| pair[0] <= pair[1]), "{scores:?}");
        let mut indexes = HashSet::new();
        let mut identifiers = HashSet::new();
        for candidate in &ranking.candidates {
            prop_assert!(indexes.insert(candidate.input_index));
            prop_assert!(!albums[candidate.input_index].tracks.is_empty());
            if candidate.album.album_id.is_some() {
                prop_assert!(identifiers.insert((
                    candidate.album.data_source.clone(),
                    candidate.album.album_id.clone(),
                )));
            }
        }
    }
}
