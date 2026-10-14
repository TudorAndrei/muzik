//! Typed results kept inside the runner until the job event boundary.

use muzik_workflow::{AudioProcessingResult, playlist::PlaylistItemResult};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
pub struct WorkflowResult {
    pub albums: usize,
    pub singles: usize,
    pub split_dirs: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_files: Option<Vec<PathBuf>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failures: Option<Vec<PlaylistFailure>>,
}

#[derive(Serialize)]
pub struct PlaylistFailure {
    id: String,
    error: Option<String>,
}

impl From<PlaylistItemResult> for PlaylistFailure {
    fn from(item: PlaylistItemResult) -> Self {
        Self {
            id: item.id,
            error: item.error,
        }
    }
}

impl From<AudioProcessingResult> for WorkflowResult {
    fn from(result: AudioProcessingResult) -> Self {
        Self {
            albums: result.plan.albums.len(),
            singles: result.plan.singles.len(),
            split_dirs: result.split_dirs,
            audio_files: None,
            items: None,
            failures: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PlaylistFailure, WorkflowResult};
    use muzik_workflow::{AudioProcessingPlan, AudioProcessingResult};
    use serde_json::json;

    #[test]
    fn remote_results_keep_the_job_event_schema() {
        let result = || {
            WorkflowResult::from(AudioProcessingResult {
                plan: AudioProcessingPlan {
                    albums: Vec::new(),
                    singles: vec!["song.flac".into()],
                    pre_split_dirs: Vec::new(),
                },
                split_dirs: Vec::new(),
                organize_targets: Vec::new(),
            })
        };
        let mut track = result();
        track.audio_files = Some(vec!["song.flac".into()]);
        let mut spotify = result();
        spotify.items = Some(1);
        let mut playlist = result();
        playlist.failures = Some(Vec::new());
        let mut failed_playlist = result();
        failed_playlist.failures = Some(vec![PlaylistFailure {
            id: "video".into(),
            error: Some("download failed".into()),
        }]);
        for (result, expected) in [
            (
                track,
                json!({"albums":0,"singles":1,"split_dirs":[],"audio_files":["song.flac"]}),
            ),
            (
                spotify,
                json!({"albums":0,"singles":1,"split_dirs":[],"items":1}),
            ),
            (
                playlist,
                json!({"albums":0,"singles":1,"split_dirs":[],"failures":[]}),
            ),
            (
                failed_playlist,
                json!({"albums":0,"singles":1,"split_dirs":[],"failures":[{"id":"video","error":"download failed"}]}),
            ),
        ] {
            assert_eq!(serde_json::to_value(result).unwrap(), expected);
        }
    }
}
