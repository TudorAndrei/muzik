//! Music matching primitives compatible with beets.

mod distance;
mod ranking;
mod string_distance;

pub use distance::{
    AlbumField, Distance, DistanceKey, Error, MatchAlbum, MatchConfig, MatchItem, MatchTrack,
    album_distance, track_distance,
};
pub use ranking::{Assignment, RankedAlbum, Ranking, Recommendation, assign_items, rank_albums};
pub use string_distance::string_dist;
