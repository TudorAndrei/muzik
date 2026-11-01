use muzik_store::watchlist::jobs::{JobError, LoadedSource, Operations};
use muzik_store::watchlist::{ItemAction, Playlist, SourceKind, Stage, StageStatus, WatchItem};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Default)]
pub enum Processing {
    #[default]
    Complete,
    StopAtSecond,
    Unchanged,
    Fail(String),
    FailStage(Stage, String),
    DownloadAgain,
}

#[derive(Debug)]
pub struct Question {
    pub video: String,
    pub stage: Stage,
    pub value: Value,
}

#[derive(Debug, Default)]
pub struct FakeOperations {
    pub listed: Vec<WatchItem>,
    pub listed_title: Option<String>,
    pub list_per_source: bool,
    pub processing: Processing,
    pub question: Option<Question>,
    pub calls: Vec<String>,
    pub processed: Vec<String>,
}

pub fn card(position: u64, id: &str, title: &str) -> WatchItem {
    let mut item = WatchItem::new(position, title, SourceKind::Youtube);
    item.video_id = Some(id.into());
    item.video_url = Some(format!("https://www.youtube.com/watch?v={id}"));
    item
}

fn processed(item: &WatchItem) -> WatchItem {
    let mut updated = item.clone();
    for stage in [Stage::Download, Stage::Parse, Stage::Organize] {
        updated.set(stage, StageStatus::Complete);
    }
    for stage in [Stage::Quality, Stage::Split] {
        updated.set(stage, StageStatus::Skipped);
    }
    updated
}

impl Operations for FakeOperations {
    fn load(&mut self, playlist: &Playlist) -> Result<LoadedSource, JobError> {
        let id = &playlist.playlist_id;
        self.calls.push(format!("load {id}"));
        if self.list_per_source {
            let video = format!("video_{id}");
            return Ok(LoadedSource {
                title: None,
                items: vec![card(1, &video, &video)],
            });
        }
        Ok(LoadedSource {
            title: self.listed_title.clone(),
            items: self.listed.clone(),
        })
    }

    fn process(
        &mut self,
        _: &Playlist,
        item: &WatchItem,
        _: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        let id = item
            .video_id
            .clone()
            .ok_or_else(|| JobError::Operation("video ID missing".into()))?;
        self.calls.push(format!("process {id}"));
        self.processed.push(id.clone());
        if let Some(question) = self.question.as_ref().filter(|asked| asked.video == id) {
            return Err(JobError::Waiting {
                stage: question.stage,
                question: question.value.clone(),
            });
        }
        match &self.processing {
            Processing::StopAtSecond if self.processed.len() == 2 => {
                cancelled.store(true, Ordering::SeqCst);
                Err(JobError::Cancelled)
            }
            Processing::Complete | Processing::StopAtSecond => Ok(processed(item)),
            Processing::Unchanged => Ok(item.clone()),
            Processing::Fail(message) => Err(JobError::Operation(message.clone())),
            Processing::FailStage(stage, message) => Err(JobError::Failed {
                stage: *stage,
                message: message.clone(),
            }),
            Processing::DownloadAgain => {
                let mut item = item.clone();
                item.set(Stage::Download, StageStatus::Complete);
                item.invalidate(&[Stage::Parse, Stage::Split, Stage::Organize]);
                Ok(item)
            }
        }
    }
}
