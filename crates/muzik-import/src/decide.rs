//! The match and duplicate rules for one planned album.

use crate::apply::{AlbumDecision, DuplicateDecision, MatchDecision};
use crate::plan::AlbumPlan;
use muzik_core::DuplicatePolicy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportPolicy {
    pub interactive: bool,
    pub force: bool,
    pub duplicates: DuplicatePolicy,
}

pub trait Ask {
    /// # Errors
    /// Returns an error when the user cannot be asked or gives no valid answer.
    fn choose_match(&mut self, album: &AlbumPlan) -> Result<MatchDecision, String>;
    /// # Errors
    /// Returns an error when the user cannot be asked or gives no valid answer.
    fn choose_duplicate(&mut self, album: &AlbumPlan) -> Result<DuplicateDecision, String>;
}

pub struct NeverAsk;

impl Ask for NeverAsk {
    fn choose_match(&mut self, _: &AlbumPlan) -> Result<MatchDecision, String> {
        Ok(MatchDecision::AsIs)
    }

    fn choose_duplicate(&mut self, _: &AlbumPlan) -> Result<DuplicateDecision, String> {
        Ok(DuplicateDecision::Skip)
    }
}

/// # Errors
/// Returns an error when asking for a match or duplicate decision fails.
pub fn decide_album(
    album: &AlbumPlan,
    policy: ImportPolicy,
    ask: &mut dyn Ask,
) -> crate::Result<AlbumDecision> {
    let choice = if policy.interactive {
        ask.choose_match(album)?
    } else {
        MatchDecision::AsIs
    };
    let duplicate = if choice == MatchDecision::Skip {
        None
    } else if album.duplicates.is_empty() {
        (!policy.force
            && matches!(
                policy.duplicates,
                DuplicatePolicy::Skip | DuplicatePolicy::Ask
            ))
        .then_some(DuplicateDecision::Skip)
    } else if policy.force {
        Some(DuplicateDecision::Replace)
    } else {
        Some(match policy.duplicates {
            DuplicatePolicy::Skip => DuplicateDecision::Skip,
            DuplicatePolicy::KeepAll => DuplicateDecision::Keep,
            DuplicatePolicy::RemoveOld => DuplicateDecision::Replace,
            DuplicatePolicy::Ask if !policy.interactive => DuplicateDecision::Skip,
            DuplicatePolicy::Ask => ask.choose_duplicate(album)?,
        })
    };
    Ok(AlbumDecision { choice, duplicate })
}

#[cfg(test)]
mod tests {
    use super::{Ask, ImportPolicy, decide_album};
    use crate::apply::{AlbumDecision, DuplicateDecision, MatchDecision};
    use crate::plan::{AlbumPlan, Duplicate, DuplicateReason, ImportMode};
    use muzik_core::DuplicatePolicy;
    use muzik_match::Recommendation;
    use std::path::PathBuf;

    struct Answers {
        choice: MatchDecision,
        duplicate: DuplicateDecision,
        asked: Vec<&'static str>,
    }

    impl Ask for Answers {
        fn choose_match(&mut self, _: &AlbumPlan) -> Result<MatchDecision, String> {
            self.asked.push("match");
            Ok(self.choice)
        }

        fn choose_duplicate(&mut self, _: &AlbumPlan) -> Result<DuplicateDecision, String> {
            self.asked.push("duplicate");
            Ok(self.duplicate)
        }
    }

    fn album(duplicated: bool) -> AlbumPlan {
        AlbumPlan {
            kind: ImportMode::Album,
            source_dir: PathBuf::from("/incoming/album"),
            items: Vec::new(),
            candidates: Vec::new(),
            recommendation: Recommendation::None,
            duplicates: if duplicated {
                vec![Duplicate {
                    album_id: 7,
                    reason: DuplicateReason::ArtistAndAlbum,
                }]
            } else {
                Vec::new()
            },
        }
    }

    fn decide(
        has_duplicate: bool,
        interactive: bool,
        force: bool,
        duplicates: DuplicatePolicy,
    ) -> Result<(AlbumDecision, Vec<&'static str>), String> {
        let mut answers = Answers {
            choice: MatchDecision::Candidate(0),
            duplicate: DuplicateDecision::Keep,
            asked: Vec::new(),
        };
        let decision = decide_album(
            &album(has_duplicate),
            ImportPolicy {
                interactive,
                force,
                duplicates,
            },
            &mut answers,
        )?;
        Ok((decision, answers.asked))
    }

    #[test]
    fn a_duplicate_follows_the_policy_and_asks_only_when_it_can() {
        let cases = [
            (
                DuplicatePolicy::Skip,
                true,
                Some(DuplicateDecision::Skip),
                vec!["match"],
            ),
            (
                DuplicatePolicy::KeepAll,
                true,
                Some(DuplicateDecision::Keep),
                vec!["match"],
            ),
            (
                DuplicatePolicy::RemoveOld,
                false,
                Some(DuplicateDecision::Replace),
                vec![],
            ),
            (
                DuplicatePolicy::Ask,
                false,
                Some(DuplicateDecision::Skip),
                vec![],
            ),
            (
                DuplicatePolicy::Ask,
                true,
                Some(DuplicateDecision::Keep),
                vec!["match", "duplicate"],
            ),
        ];
        for (policy, interactive, expected, asked) in cases {
            let (decision, questions) = decide(true, interactive, false, policy).unwrap();
            assert_eq!(decision.duplicate, expected, "{policy:?}");
            assert_eq!(questions, asked, "{policy:?}");
        }
    }

    #[test]
    fn force_replaces_a_duplicate_and_imports_a_new_album_again() {
        let (duplicated, _) = decide(true, false, true, DuplicatePolicy::Skip).unwrap();
        assert_eq!(duplicated.duplicate, Some(DuplicateDecision::Replace));
        let (fresh, _) = decide(false, false, true, DuplicatePolicy::Skip).unwrap();
        assert_eq!(fresh.duplicate, None);
    }

    #[test]
    fn an_album_without_duplicates_skips_only_if_it_is_in_the_library() {
        for (policy, expected) in [
            (DuplicatePolicy::Skip, Some(DuplicateDecision::Skip)),
            (DuplicatePolicy::Ask, Some(DuplicateDecision::Skip)),
            (DuplicatePolicy::KeepAll, None),
            (DuplicatePolicy::RemoveOld, None),
        ] {
            let (decision, _) = decide(false, false, false, policy).unwrap();
            assert_eq!(decision.choice, MatchDecision::AsIs);
            assert_eq!(decision.duplicate, expected, "{policy:?}");
        }
    }

    #[test]
    fn a_skipped_match_needs_no_duplicate_decision() {
        let mut answers = Answers {
            choice: MatchDecision::Skip,
            duplicate: DuplicateDecision::Keep,
            asked: Vec::new(),
        };
        let decision = decide_album(
            &album(true),
            ImportPolicy {
                interactive: true,
                force: false,
                duplicates: DuplicatePolicy::Ask,
            },
            &mut answers,
        )
        .unwrap();
        assert_eq!(decision.duplicate, None);
        assert_eq!(answers.asked, ["match"]);
    }
}
