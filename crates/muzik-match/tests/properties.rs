use muzik_core::BeetsConfig;
use muzik_match::{MatchConfig, MatchItem, MatchTrack, assign_items, string_dist};
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
        let config = MatchConfig::from_beets(&BeetsConfig::from_layers("", json!({}))?)?;
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
}
