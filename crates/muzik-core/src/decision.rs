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

const KEEP_CURRENT_TAGS: &str = "as_is";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Option<String>", into = "Option<String>")]
pub enum MatchAnswer {
    KeepCurrentTags,
    Candidate(String),
    Skip,
}

impl From<Option<String>> for MatchAnswer {
    fn from(answer: Option<String>) -> Self {
        match answer {
            Some(answer) if answer == KEEP_CURRENT_TAGS => Self::KeepCurrentTags,
            Some(id) => Self::Candidate(id),
            None => Self::Skip,
        }
    }
}

impl From<MatchAnswer> for Option<String> {
    fn from(answer: MatchAnswer) -> Self {
        match answer {
            MatchAnswer::KeepCurrentTags => Some(KEEP_CURRENT_TAGS.to_owned()),
            MatchAnswer::Candidate(id) => Some(id),
            MatchAnswer::Skip => None,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::MatchAnswer;
    use serde_json::json;

    #[test]
    fn a_match_answer_keeps_the_stored_json() {
        let stored = json!(["as_is", "release:1", null]);
        let answers: Vec<MatchAnswer> = serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(
            answers,
            [
                MatchAnswer::KeepCurrentTags,
                MatchAnswer::Candidate("release:1".into()),
                MatchAnswer::Skip,
            ]
        );
        assert_eq!(serde_json::to_value(answers).unwrap(), stored);
        assert!(serde_json::from_value::<MatchAnswer>(json!(3)).is_err());
    }
}
