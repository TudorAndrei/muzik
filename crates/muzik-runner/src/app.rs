//! The queue and watchlist operations that the desktop app uses.

use crate::agent::Chooser;
use crate::events::AppEvent;
use crate::library_tasks::{LibraryRequest, LibraryTask};
use crate::queue::{EnqueueError, Jobs, JobsSnapshot, QuestionVersion, job_id, parse_job_id};
use crate::runner::{Options, Prompt, Runner, Sink};
use crate::settings::Settings;
use crate::{gates, watchlist};
use muzik_core::paths::Paths;
use muzik_store::jobs::CancelRequest;
use muzik_store::watchlist::{
    ItemAction, ItemId, LocalCheck, LocalChecks, Playlist, Repository, playlist_files,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

type Decisions = Arc<Mutex<HashMap<String, Sender<Value>>>>;
pub type Busy = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnerMode {
    Run,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistFile {
    On,
    Off,
}

impl From<bool> for PlaylistFile {
    fn from(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    Disk,
    Memory,
}

pub struct AppOptions {
    pub paths: Paths,
    pub workers: usize,
    pub runner: RunnerMode,
    pub storage: Storage,
    pub chooser: Option<Arc<dyn Chooser>>,
    pub sink: Sink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEdit {
    pub changed: bool,
    pub files_error: Option<String>,
}

pub struct App {
    jobs: Arc<Jobs>,
    runner: Option<Runner>,
    sink: Sink,
    decisions: Decisions,
    checks: LocalChecks,
    paths: Paths,
}

impl App {
    /// # Errors
    /// Returns an error when the job queue cannot be opened or the runner cannot start.
    pub fn start(options: AppOptions) -> crate::Result<Self> {
        let jobs = Arc::new(match options.storage {
            Storage::Memory => Jobs::in_memory(&options.paths)?,
            Storage::Disk => Jobs::open(&options.paths)?,
        });
        let decisions: Decisions = Arc::new(Mutex::new(HashMap::new()));
        let generation = Arc::new(AtomicU64::new(0));
        let wants_runner = options.runner == RunnerMode::Run;
        let runner = if wants_runner {
            let sink = Arc::clone(&options.sink);
            let pending = Arc::clone(&decisions);
            let asked = AtomicU64::new(0);
            Runner::start(
                Arc::clone(&jobs),
                Options {
                    workers: options.workers,
                    sink: Arc::clone(&options.sink),
                    ask: Arc::new(move |prompt| {
                        let number = asked.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
                        ask(&sink, &pending, &prompt, number)
                    }),
                    chooser: options.chooser,
                    generation: Arc::clone(&generation),
                },
            )?
        } else {
            None
        };
        if wants_runner && runner.is_none() {
            (options.sink)(AppEvent::RemoteRunner {
                message: "Another muzik process runs the queue. New jobs go into its queue.".into(),
            });
        }
        Ok(Self {
            jobs,
            runner,
            sink: options.sink,
            decisions,
            checks: LocalChecks::new(generation),
            paths: options.paths,
        })
    }

    #[must_use]
    pub const fn paths(&self) -> &Paths {
        &self.paths
    }

    #[must_use]
    pub fn repository(&self) -> Repository {
        Repository::open(&self.paths)
    }

    #[must_use]
    pub fn jobs(&self) -> JobsSnapshot {
        JobsSnapshot {
            queue: self.jobs.snapshot(),
            gates: gates::snapshot(),
            runner: self.runner.is_some(),
        }
    }

    /// # Errors
    /// Returns an error when the job cannot be added to the queue.
    pub fn start_workflow(&self, params: &Value) -> Result<String, EnqueueError> {
        self.queued(self.jobs.workflow(params))
    }

    /// # Errors
    /// Returns an error when the job cannot be added to the queue.
    pub fn start_library_task(
        &self,
        task: LibraryTask,
        dry_run: bool,
    ) -> Result<String, EnqueueError> {
        self.queued(self.jobs.library(&LibraryRequest {
            dry_run,
            ..LibraryRequest::new(task)
        }))
    }

    /// # Errors
    /// Returns an error when the job cannot be added to the queue.
    pub fn refresh(&self, source: Option<(&str, &str)>) -> Result<String, EnqueueError> {
        let params = match source {
            Some((id, title)) => json!({"playlist_id": id, "playlist_title": title}),
            None => json!({}),
        };
        self.queued(self.jobs.refresh(&params))
    }

    /// # Errors
    /// Returns an error when the job cannot be added to the queue.
    pub fn run_item(
        &self,
        id: &ItemId,
        title: &str,
        action: ItemAction,
    ) -> Result<String, EnqueueError> {
        let mut params = json!({"title": title, "action": action});
        id.write(&mut params)?;
        self.queued(self.jobs.item(&params))
    }

    /// # Errors
    /// Returns an error when the answer cannot be saved in the job queue.
    pub fn answer_version(
        &self,
        id: i64,
        version: QuestionVersion,
        value: &Value,
    ) -> crate::Result<crate::Answered> {
        let answered = self.jobs.answer_version(id, version, value)?;
        self.changed();
        Ok(answered)
    }

    /// # Errors
    /// Returns an error when the job queue cannot record the cancel request.
    pub fn cancel(&self, job: &str) -> crate::Result<bool> {
        let Some(number) = job
            .starts_with("queue-")
            .then(|| parse_job_id(job))
            .flatten()
        else {
            return Ok(false);
        };
        if self
            .runner
            .as_ref()
            .is_some_and(|runner| runner.cancel(number))
        {
            return Ok(true);
        }
        Ok(match self.jobs.cancel(number)? {
            CancelRequest::Removed => {
                self.changed();
                true
            }
            CancelRequest::Requested => true,
            CancelRequest::NotOpen => false,
        })
    }

    #[must_use]
    pub fn reply(&self, decision_id: &str, value: Value) -> bool {
        self.decisions
            .lock()
            .remove(decision_id)
            .is_some_and(|reply| reply.send(value).is_ok())
    }

    /// # Errors
    /// Returns an error when the URL is not a valid source or the watchlist cannot be saved.
    pub fn add_source(&self, url: &str) -> crate::Result<Playlist> {
        self.edit(|repository| Ok(repository.add(url)?))
    }

    /// # Errors
    /// Returns an error when the watchlist cannot be saved.
    pub fn rename_source(&self, playlist_id: &str, title: &str) -> crate::Result<SourceEdit> {
        let renamed = self.edit(|repository| Ok(repository.rename(playlist_id, title)?))?;
        Ok(self.followed(renamed))
    }

    /// # Errors
    /// Returns an error when the watchlist cannot be saved.
    pub fn remove_source(&self, playlist_id: &str) -> crate::Result<SourceEdit> {
        let removed = self.edit(|repository| Ok(repository.remove(playlist_id)?))?;
        Ok(self.followed(removed))
    }

    /// # Errors
    /// Returns an error when the watchlist cannot be saved.
    pub fn set_source_playlist(
        &self,
        playlist_id: &str,
        file: PlaylistFile,
    ) -> crate::Result<SourceEdit> {
        let on = file == PlaylistFile::On;
        let changed = self.edit(|repository| Ok(repository.set_playlist_file(playlist_id, on)?))?;
        Ok(self.followed(changed))
    }

    /// # Errors
    /// Returns an error when the settings, the watchlist, the music library, or a playlist file cannot be read or written.
    pub fn write_playlists(&self) -> crate::Result<playlist_files::Report> {
        watchlist::write_playlists(&Settings::resolve(&self.paths, &json!({}))?)
    }

    fn followed(&self, changed: bool) -> SourceEdit {
        SourceEdit {
            changed,
            files_error: self.follow_playlists(),
        }
    }

    fn follow_playlists(&self) -> Option<String> {
        let enabled = Settings::resolve(&self.paths, &json!({}))
            .is_ok_and(|settings| settings.playlists.is_some());
        if !enabled {
            return None;
        }
        let error = self.write_playlists().err()?;
        let message = format!("The playlist files were not updated: {error}");
        (self.sink)(AppEvent::WatchlistError {
            message: message.clone(),
        });
        Some(message)
    }

    /// # Errors
    /// Returns an error when the settings or the saved watchlist cannot be read.
    pub fn load_watchlist(
        &self,
        busy: Busy,
    ) -> crate::Result<(muzik_store::watchlist::WatchlistView, WatchlistCheck)> {
        let (saved, check) = self.checks.prepare::<crate::Error>(self.repository(), || {
            if let Err(error) = watchlist::ensure_sources(&self.paths) {
                (self.sink)(AppEvent::WatchlistError {
                    message: error.to_string(),
                });
            }
            let settings = Settings::resolve(&self.paths, &json!({}))?;
            Ok(settings.reconcile().into())
        })?;
        let check = WatchlistCheck {
            check,
            jobs: Arc::clone(&self.jobs),
            sink: Arc::clone(&self.sink),
            busy,
        };
        Ok((saved, check))
    }

    fn edit<T>(&self, change: impl FnOnce(&Repository) -> crate::Result<T>) -> crate::Result<T> {
        self.checks.edit(|| change(&self.repository()))
    }

    fn queued(&self, queued: Result<i64, EnqueueError>) -> Result<String, EnqueueError> {
        let id = job_id(queued?);
        self.changed();
        Ok(id)
    }

    fn changed(&self) {
        match &self.runner {
            Some(runner) => {
                runner.publish();
                runner.wake();
            }
            None => (self.sink)(AppEvent::JobsUpdated(self.jobs.snapshot())),
        }
    }
}

/// Adapts the local check result to desktop events.
pub struct WatchlistCheck {
    check: LocalCheck,
    jobs: Arc<Jobs>,
    sink: Sink,
    busy: Busy,
}

impl WatchlistCheck {
    pub fn run(self) {
        self.check.run(
            || (self.busy)() || self.jobs.has_running(),
            |result| {
                (self.sink)(match result {
                    Ok(visible) => AppEvent::WatchlistUpdated(visible),
                    Err(error) => AppEvent::WatchlistError {
                        message: error.to_string(),
                    },
                });
            },
        );
    }
}

fn ask(
    sink: &Sink,
    decisions: &Decisions,
    prompt: &Prompt<'_>,
    number: u64,
) -> Result<Value, String> {
    let decision_id = format!("{}-decision-{number}", prompt.job_id);
    let (reply, receiver) = mpsc::channel();
    decisions.lock().insert(decision_id.clone(), reply);
    sink(AppEvent::DecisionRequest {
        job_id: prompt.job_id.to_owned(),
        decision_id: decision_id.clone(),
        kind: prompt.kind,
        payload: prompt.payload.clone(),
    });
    let answer = loop {
        if prompt.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            break Err("import cancelled".to_owned());
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(value) => break Ok(value),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                break Err("The decision is not pending.".to_owned());
            }
        }
    };
    decisions.lock().remove(&decision_id);
    answer
}

#[cfg(test)]
mod tests {
    use super::{App, AppOptions, RunnerMode, Storage};
    use crate::events::AppEvent;
    use muzik_core::paths::Paths;
    use muzik_store::watchlist::{ItemAction, ItemId};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    fn app(root: &std::path::Path) -> Result<(App, Receiver<AppEvent>), String> {
        let (sender, receiver) = mpsc::channel();
        let app = App::start(AppOptions {
            paths: Paths::under(root),
            workers: 2,
            runner: RunnerMode::Off,
            storage: Storage::Memory,
            chooser: None,
            sink: Arc::new(move |event| {
                let _ = sender.send(event);
            }),
        })?;
        Ok((app, receiver))
    }

    #[test]
    fn a_watchlist_load_sends_saved_cards_and_then_the_checked_ones() {
        let dir = tempfile::tempdir().unwrap();
        let (app, events) = app(dir.path()).unwrap();
        app.add_source("https://www.youtube.com/playlist?list=PLnative123")
            .unwrap();
        let (saved, check) = app.load_watchlist(Arc::new(|| false)).unwrap();
        assert_eq!(saved.playlists[0].playlist_id, "PLnative123");
        check.run();
        let checked = loop {
            match events.recv_timeout(Duration::from_secs(5)).unwrap() {
                AppEvent::WatchlistUpdated(watchlist) => break watchlist,
                AppEvent::WatchlistError { message } => panic!("{message}"),
                _ => {}
            }
        };
        assert_eq!(checked.playlists[0].playlist_id, "PLnative123");
    }

    #[test]
    fn an_item_has_one_open_job_and_a_cancel_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _) = app(dir.path()).unwrap();
        let id = ItemId::new("PL1", Some("abcdefghijk"));
        let job = app.run_item(&id, "Song", ItemAction::Run).unwrap();
        assert!(app.run_item(&id, "Song", ItemAction::Run).is_err());
        let listed = app.jobs();
        assert_eq!(listed.queue.open[0].job_id, job);
        assert_eq!(
            listed.queue.open[0].item.as_deref(),
            Some(r#"["PL1","abcdefghijk"]"#)
        );
        assert!(!listed.runner);
        assert!(app.cancel(&job).unwrap());
        assert_eq!(app.jobs().queue, crate::QueueSnapshot::default());
        assert!(!app.cancel("queue-999").unwrap());
        assert!(!app.reply("missing", json!("as_is")));
    }
}
