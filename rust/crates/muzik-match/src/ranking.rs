use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::distance::{
    album_distance, track_distance, Distance, Error, MatchAlbum, MatchConfig, MatchItem, MatchTrack,
};
use crate::lsap;

/// Track mapping and items left after minimum-cost assignment.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Assignment {
    pub pairs: Vec<(usize, usize)>,
    pub extra_items: Vec<usize>,
    pub extra_tracks: Vec<usize>,
}

pub fn assign_items(
    items: &[MatchItem],
    tracks: &[MatchTrack],
    config: &MatchConfig,
) -> Result<Assignment, Error> {
    let mut costs = Vec::with_capacity(items.len() * tracks.len());
    for item in items {
        for track in tracks {
            costs.push(track_distance(item, track, false, config)?.score(config)?);
        }
    }
    let (rows, columns) =
        lsap::solve(items.len(), tracks.len(), &costs, false).map_err(Error::Assignment)?;
    let mut pairs: Vec<_> = rows.into_iter().zip(columns).collect();
    // beets builds its mapping by track order, even when LAP returns row order.
    pairs.sort_by_key(|pair| pair.1);
    let used_items: HashSet<_> = pairs.iter().map(|pair| pair.0).collect();
    let used_tracks: HashSet<_> = pairs.iter().map(|pair| pair.1).collect();
    let mut extra_items: Vec<_> = (0..items.len())
        .filter(|index| !used_items.contains(index))
        .collect();
    extra_items.sort_by(|&left, &right| {
        let left_item = &items[left];
        let right_item = &items[right];
        (left_item.disc, left_item.track, &left_item.title).cmp(&(
            right_item.disc,
            right_item.track,
            &right_item.title,
        ))
    });
    let mut extra_tracks: Vec<_> = (0..tracks.len())
        .filter(|index| !used_tracks.contains(index))
        .collect();
    extra_tracks.sort_by(|&left, &right| {
        (tracks[left].index, &tracks[left].title).cmp(&(tracks[right].index, &tracks[right].title))
    });
    tracing::debug!(
        assigned = pairs.len(),
        extra_items = extra_items.len(),
        extra_tracks = extra_tracks.len(),
        "assigned tracks"
    );
    Ok(Assignment {
        pairs,
        extra_items,
        extra_tracks,
    })
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Recommendation {
    None,
    Low,
    Medium,
    Strong,
}

#[derive(Clone, Debug)]
pub struct RankedAlbum {
    pub input_index: usize,
    pub album: MatchAlbum,
    pub assignment: Assignment,
    pub distance: Distance,
}

#[derive(Clone, Debug)]
pub struct Ranking {
    pub candidates: Vec<RankedAlbum>,
    pub recommendation: Recommendation,
}

fn required_field_present(album: &MatchAlbum, field: &str) -> bool {
    match field {
        "album" => !album.title.is_empty(),
        "artist" => !album.artist.is_empty(),
        "album_id" => album.album_id.is_some(),
        "media" => album.media.is_some(),
        "mediums" => album.mediums.is_some(),
        "year" => album.year.is_some(),
        "original_year" => album.original_year.is_some(),
        "country" => album.country.is_some(),
        "label" => album.label.is_some(),
        "catalognum" => album.catalog_number.is_some(),
        "albumdisambig" => album.disambiguation.is_some(),
        "data_source" => album.data_source.is_some(),
        "tracks" => true,
        _ => false,
    }
}

fn recommendation(
    candidates: &[RankedAlbum],
    config: &MatchConfig,
) -> Result<Recommendation, Error> {
    let Some(best) = candidates.first() else {
        return Ok(Recommendation::None);
    };
    let score = best.distance.score(config)?;
    let mut recommendation = if score < config.strong_rec_thresh {
        Recommendation::Strong
    } else if score <= config.medium_rec_thresh {
        Recommendation::Medium
    } else if candidates.len() == 1
        || candidates[1].distance.score(config)? - score >= config.rec_gap_thresh
    {
        Recommendation::Low
    } else {
        return Ok(Recommendation::None);
    };
    let mut keys = best.distance.active_keys(config)?;
    for track in &best.distance.tracks {
        keys.extend(track.active_keys(config)?);
    }
    for key in keys {
        if let Some(limit) = config.max_rec.get(&key) {
            let limit = match limit.as_str() {
                "strong" => Recommendation::Strong,
                "medium" => Recommendation::Medium,
                "low" => Recommendation::Low,
                "none" => Recommendation::None,
                _ => return Err(Error::InvalidConfig("match.max_rec")),
            };
            recommendation = recommendation.min(limit);
        }
    }
    Ok(recommendation)
}

/// Rank a fixed set of candidate releases with beets' filters and thresholds.
pub fn rank_albums(
    items: &[MatchItem],
    candidates: &[MatchAlbum],
    config: &MatchConfig,
) -> Result<Ranking, Error> {
    let mut ranked = Vec::new();
    let mut seen = HashSet::new();
    for (input_index, album) in candidates.iter().enumerate() {
        if album.tracks.is_empty() {
            continue;
        }
        let identifier = (album.data_source.clone(), album.album_id.clone());
        if album.album_id.is_some() && !seen.insert(identifier) {
            continue;
        }
        if config
            .required
            .iter()
            .any(|field| !required_field_present(album, field))
        {
            continue;
        }
        let assignment = assign_items(items, &album.tracks, config)?;
        let distance = album_distance(items, album, &assignment.pairs, config)?;
        let active = distance.active_keys(config)?;
        if config.ignored.iter().any(|field| active.contains(field)) {
            continue;
        }
        ranked.push(RankedAlbum {
            input_index,
            album: album.clone(),
            assignment,
            distance,
        });
    }
    ranked.sort_by(|left, right| {
        let left = left.distance.score(config).expect("distance was scored");
        let right = right.distance.score(config).expect("distance was scored");
        left.total_cmp(&right)
    });
    let recommendation = recommendation(&ranked, config)?;
    tracing::debug!(
        candidates = ranked.len(),
        ?recommendation,
        "ranked album candidates"
    );
    Ok(Ranking {
        candidates: ranked,
        recommendation,
    })
}
