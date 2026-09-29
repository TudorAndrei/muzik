use crate::watchlist::Stage;
use serde::{Deserialize, Serialize};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr};

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum DecisionKind {
    ImportMatch,
    ImportDuplicate,
    ChapterReview,
    ChapterEdit,
    QualityReplacement,
    SoulseekCandidate,
}

pub const KEEP_CURRENT_TAGS: &str = "as_is";

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ChapterAnswer {
    Accept,
    Reject,
    Edit,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum DuplicateAnswer {
    Skip,
    KeepAll,
    RemoveOld,
}

impl DecisionKind {
    pub fn stage(self) -> Stage {
        match self {
            Self::ImportMatch | Self::ImportDuplicate => Stage::Organize,
            Self::ChapterReview | Self::ChapterEdit => Stage::Parse,
            Self::QualityReplacement => Stage::Quality,
            Self::SoulseekCandidate => Stage::Download,
        }
    }
}
