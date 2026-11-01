use muzik_core::chapters::Chapter;
use muzik_workflow::playlist::SpotifyTrack;
use muzik_workflow::{
    ChapterReview, QualityCheckedAudio, SplitProgress, SplitTask, WorkflowOperations,
    WorkflowOptions,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Default)]
pub struct FakeOperations {
    pub downloads: Vec<String>,
    pub downloaded_files: Vec<PathBuf>,
    pub failing_video: Option<String>,
    pub playlist_ids: Vec<String>,
    pub playlist_listed: bool,
    pub soulseek_ready: bool,
    pub soulseek_files: Vec<PathBuf>,
    pub soulseek_queries: Vec<String>,
    pub spotify_files: Vec<PathBuf>,
    pub spotify_tracks: Vec<String>,
    pub cancel_during_soulseek: Option<Arc<AtomicBool>>,
    pub review_decision: Option<ChapterReview>,
    pub reviewed: Vec<PathBuf>,
    pub quality_result: Option<QualityCheckedAudio>,
    pub quality_calls: usize,
    pub real_split_cache: Option<PathBuf>,
    pub split_sources: Vec<PathBuf>,
    pub split_chapters: Vec<Vec<Chapter>>,
    pub organized: Vec<PathBuf>,
}

impl FakeOperations {
    pub fn downloaded_videos(&self) -> Vec<&str> {
        self.downloads
            .iter()
            .filter_map(|url| url.split("v=").last())
            .collect()
    }
}

impl WorkflowOperations for FakeOperations {
    fn download_youtube(
        &mut self,
        url: &str,
        output: &Path,
        _force: bool,
    ) -> Result<Vec<PathBuf>, String> {
        self.downloads.push(url.to_owned());
        let id = url.split("v=").last().unwrap_or_default();
        if self.failing_video.as_deref() == Some(id) {
            return Err("video unavailable".into());
        }
        if !self.downloaded_files.is_empty() {
            return Ok(self.downloaded_files.clone());
        }
        fs::create_dir_all(output).map_err(|error| error.to_string())?;
        let file = output.join(format!("track [{id}].mp3"));
        fs::write(&file, b"audio").map_err(|error| error.to_string())?;
        Ok(vec![file])
    }

    fn acquire_soulseek(&mut self, query: &str) -> Result<Vec<PathBuf>, String> {
        self.soulseek_queries.push(query.to_owned());
        if let Some(cancelled) = &self.cancel_during_soulseek {
            cancelled.store(true, Ordering::SeqCst);
            return Err("Soulseek cancelled".into());
        }
        Ok(self.soulseek_files.clone())
    }

    fn youtube_playlist_video_ids(&mut self, _url: &str) -> Result<Vec<String>, String> {
        self.playlist_listed = true;
        Ok(self.playlist_ids.clone())
    }

    fn acquire_spotify_track(&mut self, track: &SpotifyTrack) -> Result<Vec<PathBuf>, String> {
        self.spotify_tracks.push(track.title.clone());
        Ok(self.spotify_files.clone())
    }

    fn soulseek_ready(&self) -> bool {
        self.soulseek_ready
    }

    fn check_quality(
        &mut self,
        audio_files: &[PathBuf],
        _options: &WorkflowOptions,
        _cancelled: &AtomicBool,
    ) -> Result<QualityCheckedAudio, String> {
        self.quality_calls = self.quality_calls.saturating_add(1);
        Ok(self
            .quality_result
            .clone()
            .unwrap_or_else(|| QualityCheckedAudio {
                audio_files: audio_files.to_vec(),
                pre_split_dirs: Vec::new(),
            }))
    }

    fn review_chapters(
        &mut self,
        source: &Path,
        _chapters: &[Chapter],
        _cancelled: &AtomicBool,
    ) -> Result<ChapterReview, String> {
        self.reviewed.push(source.to_path_buf());
        Ok(self
            .review_decision
            .clone()
            .unwrap_or(ChapterReview::Accept))
    }

    fn organize(&mut self, target: &Path, _options: &WorkflowOptions) -> Result<(), String> {
        self.organized.push(target.to_path_buf());
        Ok(())
    }

    fn split(&mut self, task: &SplitTask, options: &WorkflowOptions) -> Result<(), String> {
        self.split_sources.push(task.source.clone());
        self.split_chapters.push(task.chapters.clone());
        let Some(cache) = &self.real_split_cache else {
            return fs::create_dir_all(&task.output).map_err(|error| error.to_string());
        };
        muzik_media::splitter::split_audio(
            &task.source,
            &task.chapters,
            &task.output,
            &muzik_media::splitter::SplitOptions {
                force: options.force,
                keep_source: true,
                cache_dir: Some(cache.clone()),
                ..Default::default()
            },
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn split_with_cancel(
        &mut self,
        task: &SplitTask,
        options: &WorkflowOptions,
        _cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(SplitProgress),
    ) -> Result<(), String> {
        self.split(task, options)?;
        on_progress(SplitProgress {
            completed: 1,
            total: task.chapters.len(),
            chapter_index: task
                .chapters
                .first()
                .ok_or("the task has no chapters")?
                .index,
        });
        Ok(())
    }
}
