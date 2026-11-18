use muzik_store::watchlist::jobs::{JobError, LoadedSource, Operations};
use muzik_store::watchlist::{
    ItemAction, Playlist, Repository, SourceKind, Stage, StageEvent, WatchItem,
};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Default)]
pub enum Processing {
    #[default]
    Complete,
    StopAtSecond,
    CancelAfterStart,
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

#[derive(Debug)]
pub enum Sync {
    Retitle(String),
    Remove,
}

#[derive(Debug)]
pub struct DuringRun {
    pub database: PathBuf,
    pub video: String,
    pub sync: Sync,
}

impl DuringRun {
    fn happen(self) -> Result<(), JobError> {
        Repository::new(self.database).update(|document| {
            for playlist in &mut document.playlists {
                let ours = |item: &WatchItem| item.video_id.as_deref() == Some(self.video.as_str());
                match &self.sync {
                    Sync::Retitle(title) => {
                        for item in playlist.items.iter_mut().filter(|item| ours(item)) {
                            title.clone_into(&mut item.title);
                        }
                    }
                    Sync::Remove => playlist.items.retain(|item| !ours(item)),
                }
            }
            Ok(())
        })?;
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct FakeOperations {
    pub listed: Vec<WatchItem>,
    pub listed_title: Option<String>,
    pub list_per_source: bool,
    pub processing: Processing,
    pub question: Option<Question>,
    pub downloads_first: bool,
    pub during_run: Option<DuringRun>,
    pub calls: Vec<String>,
    pub processed: Vec<String>,
}

pub fn card(position: u64, id: &str, title: &str) -> WatchItem {
    let mut item = WatchItem::new(position, title, SourceKind::Youtube);
    item.video_id = Some(id.into());
    item.video_url = Some(format!("https://www.youtube.com/watch?v={id}"));
    item
}

fn processed(item: &mut WatchItem) -> Result<(), JobError> {
    for stage in [Stage::Download, Stage::Parse, Stage::Organize] {
        let path = item.kept_path(stage);
        item.apply(stage, StageEvent::Complete(path))?;
    }
    for stage in [Stage::Quality, Stage::Split] {
        item.apply(stage, StageEvent::Skip)?;
    }
    Ok(())
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
        item: &mut WatchItem,
        _: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<(), JobError> {
        let id = item
            .video_id
            .clone()
            .ok_or_else(|| JobError::Operation("video ID missing".into()))?;
        self.calls.push(format!("process {id}"));
        self.processed.push(id.clone());
        if self.downloads_first {
            item.apply(
                Stage::Download,
                StageEvent::Complete(Some(PathBuf::from(format!("{id}.opus")))),
            )?;
        }
        if let Some(during) = self.during_run.take() {
            during.happen()?;
        }
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
            Processing::CancelAfterStart => {
                cancelled.store(true, Ordering::SeqCst);
                Err(JobError::Cancelled)
            }
            Processing::Complete | Processing::StopAtSecond => processed(item),
            Processing::Unchanged => Ok(()),
            Processing::Fail(message) => Err(JobError::Operation(message.clone())),
            Processing::FailStage(stage, message) => Err(JobError::Failed {
                stage: *stage,
                message: message.clone(),
            }),
            Processing::DownloadAgain => {
                item.apply(Stage::Download, StageEvent::Complete(None))?;
                item.invalidate(&[Stage::Parse, Stage::Split, Stage::Organize])?;
                Ok(())
            }
        }
    }
}
