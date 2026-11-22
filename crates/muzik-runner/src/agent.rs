use crate::Result;
use muzik_agent::{Outcome, TracklistRequest};
use muzik_core::DecisionKind;
use muzik_core::chapters::{self, Chapter};
use serde_json::Value;

pub trait Chooser: Send + Sync {
    /// # Errors
    /// Returns an error when the agent cannot make a decision.
    fn choose(&self, kind: DecisionKind, payload: &Value, model: &str) -> Result<Outcome>;
}

/// Ask the agent for the track list in the top comments of a video, and keep it only when the
/// start times rise and fit the video.
///
/// # Errors
/// Returns an error when the agent cannot run or gives no valid answer.
pub fn comment_tracklist(
    model: &str,
    video_title: &str,
    duration: Option<f64>,
    comments: &[String],
) -> std::result::Result<Vec<Chapter>, String> {
    let entries = muzik_agent::read_tracklist(
        &TracklistRequest {
            video_title,
            duration_seconds: duration,
            comments,
        },
        model,
    )?;
    Ok(chapters::tracklist_chapters(
        entries
            .into_iter()
            .map(|entry| (entry.start_seconds, entry.title)),
        duration,
    ))
}

pub struct Codex;

impl Chooser for Codex {
    fn choose(&self, kind: DecisionKind, payload: &Value, model: &str) -> Result<Outcome> {
        Ok(muzik_agent::decide(kind, payload, model)?)
    }
}
