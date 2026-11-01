use crate::operations::FakeOperations;
use cucumber::World;
use muzik_workflow::playlist::{PlaylistRunResult, SpotifyPlaylist};
use muzik_workflow::{
    AudioProcessingPlan, AudioProcessingResult, Error, WorkflowEvent, WorkflowInput,
    WorkflowOptions, WorkflowRequest,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;

#[derive(Debug, World)]
#[world(init = Self::new)]
pub struct WorkflowWorld {
    pub dir: TempDir,
    pub source: PathBuf,
    pub export: PathBuf,
    pub album: PathBuf,
    pub operations: FakeOperations,
    pub options: WorkflowOptions,
    pub cancelled: Arc<AtomicBool>,
    pub cancel_when_plan_ready: bool,
    pub protected: Vec<(PathBuf, Vec<u8>)>,
    pub events: Vec<WorkflowEvent>,
    pub outcome: Option<Result<AudioProcessingResult, Error>>,
    pub plan: Option<AudioProcessingPlan>,
    pub playlist: Option<PlaylistRunResult>,
    pub input: Option<WorkflowInput>,
    pub discovered: Option<Result<Vec<PathBuf>, Error>>,
    pub spotify: Option<Result<SpotifyPlaylist, Error>>,
}

impl WorkflowWorld {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let input = dir.path().join("input");
        std::fs::create_dir(&input)?;
        Ok(Self {
            source: input.join("source.flac"),
            export: dir.path().join("spotify.json"),
            album: input.join("album"),
            dir,
            operations: FakeOperations::default(),
            options: WorkflowOptions::default(),
            cancelled: Arc::new(AtomicBool::new(false)),
            cancel_when_plan_ready: false,
            protected: Vec::new(),
            events: Vec::new(),
            outcome: None,
            plan: None,
            playlist: None,
            input: None,
            discovered: None,
            spotify: None,
        })
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn input_dir(&self) -> PathBuf {
        self.root().join("input")
    }

    pub fn request(&self, raw: &str) -> WorkflowRequest {
        WorkflowRequest {
            raw: raw.to_owned(),
            output: self.root().join("downloads"),
            splits: self.root().join("splits"),
        }
    }

    pub fn result(&self) -> anyhow::Result<&AudioProcessingResult> {
        match &self.outcome {
            Some(Ok(result)) => Ok(result),
            Some(Err(error)) => anyhow::bail!("the workflow failed: {error}"),
            None => anyhow::bail!("the workflow did not run"),
        }
    }

    pub fn current_plan(&self) -> anyhow::Result<&AudioProcessingPlan> {
        if let Some(plan) = &self.plan {
            return Ok(plan);
        }
        if let Some(playlist) = &self.playlist {
            return Ok(&playlist.processing.plan);
        }
        Ok(&self.result()?.plan)
    }
}
