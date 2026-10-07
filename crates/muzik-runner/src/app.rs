//! The queue and watchlist operations that the desktop app uses.

use crate::agent::Chooser;
use crate::events::AppEvent;
use crate::queue::{EnqueueError, Jobs, job_id, parse_job_id};
use crate::runner::{Options, Prompt, Runner, Sink};
use crate::settings::Settings;
use crate::{gates, watchlist};
use muzik_core::paths::Paths;
use muzik_store::jobs::CancelRequest;
use muzik_store::watchlist::{
    self as saved, CheckedWrite, ItemAction, ItemId, Playlist, Repository,
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

pub struct AppOptions {
    pub paths: Paths,
    pub workers: usize,
    pub run: bool,
    pub in_memory: bool,
    pub chooser: Option<Arc<dyn Chooser>>,
    pub sink: Sink,
}

pub struct App {
    jobs: Arc<Jobs>,
    runner: Option<Runner>,
    sink: Sink,
    decisions: Decisions,
    generation: Arc<AtomicU64>,
    gate: Arc<Mutex<()>>,
    paths: Paths,
}

impl App {
    /// # Errors
    /// Returns an error when the job queue cannot be opened or the runner cannot start.
    pub fn start(options: AppOptions) -> crate::Result<Self> {
        let jobs = Arc::new(if options.in_memory {
            Jobs::in_memory(&options.paths)?
        } else {
            Jobs::open(&options.paths)?
        });
        let decisions: Decisions = Arc::new(Mutex::new(HashMap::new()));
        let generation = Arc::new(AtomicU64::new(0));
        let runner = if options.run {
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
        if options.run && runner.is_none() {
            (options.sink)(AppEvent::RemoteRunner(
                "Another muzik process runs the queue. New jobs go into its queue.".into(),
            ));
        }
        Ok(Self {
            jobs,
            runner,
            sink: options.sink,
            decisions,
            generation,
            gate: Arc::new(Mutex::new(())),
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
    pub fn jobs(&self) -> Value {
        let mut snapshot = self.jobs.snapshot();
        if let Some(fields) = snapshot.as_object_mut() {
            fields.insert("gates".into(), gates::snapshot());
            fields.insert("runner".into(), json!(self.runner.is_some()));
        }
        snapshot
    }

    /// # Errors
    /// Returns an error when the job cannot be added to the queue.
    pub fn start_workflow(&self, params: &Value) -> Result<String, EnqueueError> {
        self.queued(self.jobs.workflow(params))
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
        id.write(&mut params);
        self.queued(self.jobs.item(&params))
    }

    /// # Errors
    /// Returns an error when the answer cannot be saved in the job queue.
    pub fn answer(&self, id: i64, value: &Value) -> crate::Result<bool> {
        let answered = self.jobs.answer(id, value)?;
        self.changed();
        Ok(answered)
    }

    /// # Errors
    /// Returns an error when the job queue cannot record the cancel request.
    pub fn cancel(&self, job: &str) -> crate::Result<bool> {
        if self
            .runner
            .as_ref()
            .is_some_and(|runner| runner.cancel(job))
        {
            return Ok(true);
        }
        let Some(number) = job
            .starts_with("queue-")
            .then(|| parse_job_id(job))
            .flatten()
        else {
            return Ok(false);
        };
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
    pub fn rename_source(&self, playlist_id: &str, title: &str) -> crate::Result<bool> {
        self.edit(|repository| Ok(repository.rename(playlist_id, title)?))
    }

    /// # Errors
    /// Returns an error when the watchlist cannot be saved.
    pub fn remove_source(&self, playlist_id: &str) -> crate::Result<bool> {
        self.edit(|repository| Ok(repository.remove(playlist_id)?))
    }

    /// # Errors
    /// Returns an error when the settings or the saved watchlist cannot be read.
    pub fn load_watchlist(&self, busy: Busy) -> crate::Result<(Value, WatchlistCheck)> {
        let generation = {
            let _gate = self.gate.lock();
            self.generation
                .fetch_add(1, Ordering::SeqCst)
                .wrapping_add(1)
        };
        let repository = self.repository();
        if let Err(error) = watchlist::ensure_sources(&self.paths) {
            (self.sink)(AppEvent::WatchlistError(error.to_string()));
        }
        let settings = Settings::resolve(&self.paths, &json!({}))?;
        let saved = saved::view(
            &repository.load()?,
            &settings.request.output,
            &settings.paths.cache,
        )?;
        let check = WatchlistCheck {
            repository,
            settings,
            jobs: Arc::clone(&self.jobs),
            latest: Arc::clone(&self.generation),
            gate: Arc::clone(&self.gate),
            sink: Arc::clone(&self.sink),
            busy,
            generation,
        };
        Ok((saved, check))
    }

    fn edit<T>(&self, change: impl FnOnce(&Repository) -> crate::Result<T>) -> crate::Result<T> {
        let _gate = self.gate.lock();
        let result = change(&self.repository())?;
        self.generation.fetch_add(1, Ordering::SeqCst);
        Ok(result)
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

pub struct WatchlistCheck {
    repository: Repository,
    settings: Settings,
    jobs: Arc<Jobs>,
    latest: Arc<AtomicU64>,
    gate: Arc<Mutex<()>>,
    sink: Sink,
    busy: Busy,
    generation: u64,
}

impl WatchlistCheck {
    pub fn run(self) {
        if let Err(error) = self.check()
            && self.current()
        {
            (self.sink)(AppEvent::WatchlistError(error.to_string()));
        }
    }

    fn current(&self) -> bool {
        self.generation == self.latest.load(Ordering::SeqCst)
    }

    fn busy(&self) -> bool {
        (self.busy)() || self.jobs.has_running() || !self.current()
    }

    fn check(&self) -> crate::Result<()> {
        let options = self.settings.reconcile();
        saved::import_cache(&self.repository, options)?;
        for _ in 0..3 {
            if self.busy() {
                return Ok(());
            }
            let (mut checked, revision) = self.repository.load_revision()?;
            saved::reconcile(&mut checked, options)?;
            if self.busy() {
                return Ok(());
            }
            let written = {
                let _gate = self.gate.lock();
                if !self.current() {
                    return Ok(());
                }
                self.repository.save_at(revision, &checked)?
            };
            if written == CheckedWrite::Conflict {
                continue;
            }
            if self.busy() {
                return Ok(());
            }
            let visible = saved::view(
                &checked,
                &self.settings.request.output,
                &self.settings.paths.cache,
            )?;
            let _gate = self.gate.lock();
            if self.current() {
                (self.sink)(AppEvent::WatchlistUpdated(visible));
            }
            return Ok(());
        }
        Err("The watchlist changed during the local check. Reload it.".into())
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
    use super::{App, AppOptions};
    use crate::events::AppEvent;
    use muzik_core::paths::Paths;
    use muzik_store::watchlist::{ItemAction, ItemId};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    fn app(root: &std::path::Path, run: bool) -> Result<(App, Receiver<AppEvent>), String> {
        let (sender, receiver) = mpsc::channel();
        let app = App::start(AppOptions {
            paths: Paths::under(root),
            workers: 2,
            run,
            in_memory: true,
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
        let (app, events) = app(dir.path(), false).unwrap();
        app.add_source("https://www.youtube.com/playlist?list=PLnative123")
            .unwrap();
        let (saved, check) = app.load_watchlist(Arc::new(|| false)).unwrap();
        assert_eq!(saved["playlists"][0]["playlist_id"], "PLnative123");
        check.run();
        let checked = loop {
            match events.recv_timeout(Duration::from_secs(5)).unwrap() {
                AppEvent::WatchlistUpdated(watchlist) => break watchlist,
                AppEvent::WatchlistError(message) => panic!("{message}"),
                _ => {}
            }
        };
        assert_eq!(checked["playlists"][0]["playlist_id"], "PLnative123");
    }

    #[test]
    fn an_item_has_one_open_job_and_a_cancel_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _) = app(dir.path(), false).unwrap();
        let id = ItemId::new("PL1", 2, Some("abcdefghijk"));
        let job = app.run_item(&id, "Song", ItemAction::Run).unwrap();
        assert!(app.run_item(&id, "Song", ItemAction::Run).is_err());
        assert_eq!(app.jobs()["open"][0]["job_id"], job);
        assert_eq!(app.jobs()["open"][0]["item"], "PL1:2:abcdefghijk");
        assert_eq!(app.jobs()["runner"], false);
        assert!(app.cancel(&job).unwrap());
        assert_eq!(app.jobs()["open"], json!([]));
        assert!(!app.cancel("queue-999").unwrap());
        assert!(!app.reply("missing", json!("as_is")));
    }
}
