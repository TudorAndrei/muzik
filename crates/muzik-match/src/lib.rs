//! Music matching primitives compatible with beets.

mod distance;
mod ranking;
mod string_distance;

pub use distance::{
    album_distance, track_distance, AlbumField, Distance, DistanceKey, Error, MatchAlbum,
    MatchConfig, MatchItem, MatchTrack,
};
pub use ranking::{assign_items, rank_albums, Assignment, RankedAlbum, Ranking, Recommendation};
pub use string_distance::string_dist;
