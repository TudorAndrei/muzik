use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(ReleaseId);
string_id!(RecordingId);

/// Metadata for one track of a candidate release.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct TrackCandidate {
    pub recording_id: Option<RecordingId>,
    pub release_track_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub length_seconds: Option<f64>,
    pub index: u32,
    pub medium: u32,
    pub medium_index: u32,
}

/// Metadata for one possible album match.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ReleaseCandidate {
    pub id: ReleaseId,
    pub title: String,
    pub artist: String,
    pub tracks: Vec<TrackCandidate>,
    pub release_group_id: Option<String>,
    pub year: Option<i32>,
    pub country: Option<String>,
    pub media: Option<String>,
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub disambiguation: Option<String>,
    pub is_various_artists: bool,
}
