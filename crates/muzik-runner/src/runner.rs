use crate::agent::Chooser;
use crate::error::JobFailure;
use crate::events::{AppEvent, Source};
use crate::queue::{Jobs, RunnerClaim, job_id};
use crate::settings::Settings;
use crate::{gates, local_workflow, remote_workflow, watchlist};
use muzik_core::{DecisionKind, JobEvent};
use muzik_store::jobs::{Job, Kind, Queue, RunnerLock, Store};
use muzik_store::watchlist::Stage;
use muzik_workflow::{WorkflowInput, classify_input};
use parking_lot::{Condvar, Mutex, MutexGuard};
use serde_json::{Value, json};
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

const QUEUES: [Queue; 3] = [Queue::Sync, Queue::Workflow, Queue::Item];

pub type Sink = Arc<dyn Fn(AppEvent) + Send + Sync>;
pub type Ask = Arc<dyn Fn(Prompt<'_>) -> Result<Value, String> + Send + Sync>;
pub type Running = Arc<Mutex<HashMap<i64, Arc<AtomicBool>>>>;

pub struct Prompt<'a> {
    pub job_id: &'a str,
    pub title: &'a str,
    pub kind: DecisionKind,
    pub payload: Value,
    pub cancelled: &'a AtomicBool,
}

pub struct Options {
    pub workers: usize,
    pub sink: Sink,
    pub ask: Ask,
    pub chooser: Option<Arc<dyn Chooser>>,
    pub generation: Arc<AtomicU64>,
}

type Outcome = Result<Value, JobFailure>;

struct Shared {
    jobs: Arc<Jobs>,
    running: Running,
    idle: Mutex<()>,
    wake: Condvar,
    stop: AtomicBool,
    sink: Sink,
    ask: Ask,
    chooser: Option<Arc<dyn Chooser>>,
    generation: Arc<AtomicU64>,
}

impl Shared {
    fn store(&self) -> MutexGuard<'_, Store> {
        self.jobs.store()
    }

    fn event(&self, job_id: &str, source: Source, event: JobEvent) {
        (self.sink)(match event {
            JobEvent::WatchlistSaved => AppEvent::WatchlistSaved,
            event => AppEvent::JobEvent {
                job_id: job_id.to_owned(),
                source,
                event,
            },
        });
    }

    fn publish(&self) {
        (self.sink)(AppEvent::JobsUpdated(self.jobs.snapshot()));
    }

    fn running(&self) -> MutexGuard<'_, HashMap<i64, Arc<AtomicBool>>> {
        self.running.lock()
    }
}

pub struct Runner {
    shared: Arc<Shared>,
    _lock: Option<RunnerLock>,
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.wake.notify_all();
    }
}

impl Runner {
    /// # Errors
    /// Returns an error when the runner lock or the job queue cannot be prepared.
    pub fn start(jobs: Arc<Jobs>, options: Options) -> crate::Result<Option<Self>> {
        let lock = match jobs.runner_lock()? {
            RunnerClaim::Taken => return Ok(None),
            RunnerClaim::Unlocked => None,
            RunnerClaim::Locked(lock) => Some(lock),
        };
        jobs.import_legacy()?;
        jobs.store().recover()?;
        jobs.release_import_questions()?;
        let shared = Arc::new(Shared {
            jobs,
            running: Arc::new(Mutex::new(HashMap::new())),
            idle: Mutex::new(()),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            sink: options.sink,
            ask: options.ask,
            chooser: options.chooser,
            generation: options.generation,
        });
        let sink = Arc::clone(&shared.sink);
        gates::listen(move |snapshot| sink(AppEvent::QueuesUpdated(snapshot)));
        for _ in 0..options.workers.max(1) {
            let shared = Arc::clone(&shared);
            thread::spawn(move || shared.work());
        }
        let watcher = Arc::clone(&shared);
        thread::spawn(move || watcher.watch());
        Ok(Some(Self {
            shared,
            _lock: lock,
        }))
    }

    #[must_use]
    pub fn running(&self) -> Running {
        Arc::clone(&self.shared.running)
    }

    #[must_use]
    pub fn cancel(&self, id: i64) -> bool {
        self.shared.running().get(&id).is_some_and(|cancel| {
            cancel.store(true, Ordering::SeqCst);
            true
        })
    }

    pub fn wake(&self) {
        self.shared.wake.notify_all();
    }

    pub fn publish(&self) {
        self.shared.publish();
    }

    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.shared.running().is_empty()
            && self
                .shared
                .store()
                .list_open()
                .is_ok_and(|jobs| jobs.is_empty())
    }

    pub fn wait_until_idle(&self, interrupted: &AtomicBool) {
        loop {
            if interrupted.load(Ordering::SeqCst) {
                self.shared.stop.store(true, Ordering::SeqCst);
                let running = self.shared.running();
                if running.is_empty() {
                    return;
                }
                for cancel in running.values() {
                    cancel.store(true, Ordering::SeqCst);
                }
            } else if self.is_idle() {
                return;
            }
            thread::sleep(Duration::from_millis(300));
        }
    }
}

impl Shared {
    fn work(&self) {
        while !self.stop.load(Ordering::SeqCst) {
            let claimed = self.store().claim_any(&QUEUES).ok().flatten();
            let Some(job) = claimed else {
                let mut idle = self.idle.lock();
                self.wake.wait_for(&mut idle, Duration::from_secs(1));
                continue;
            };
            self.run_job(&job);
        }
    }

    fn watch(&self) {
        let mut last = Value::Null;
        while !self.stop.load(Ordering::SeqCst) {
            let requests = self.store().cancel_requests().unwrap_or_default();
            {
                let running = self.running();
                for id in requests {
                    if let Some(cancel) = running.get(&id) {
                        cancel.store(true, Ordering::SeqCst);
                    }
                }
            }
            let snapshot = self.jobs.snapshot();
            if snapshot != last {
                (self.sink)(AppEvent::JobsUpdated(snapshot.clone()));
                last = snapshot;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }

    fn run_job(&self, job: &Job) {
        let id = job_id(job.id);
        let cancel = Arc::new(AtomicBool::new(false));
        self.running().insert(job.id, Arc::clone(&cancel));
        self.generation.fetch_add(1, Ordering::SeqCst);
        gates::set_label(&job.title);
        (self.sink)(AppEvent::JobStarted {
            job_id: id.clone(),
            title: job.title.clone(),
            kind: job.kind,
        });
        self.publish();
        let result = match job.kind {
            Kind::Refresh => self.run_refresh(job, &id, &cancel),
            Kind::Item => self.run_item(job, &id, &cancel),
            Kind::Workflow => self.run_workflow(job, &id, &cancel),
        };
        {
            let store = self.store();
            let _ = match &result {
                Ok(_) => store.finish(job.id),
                Err(JobFailure::Cancelled) if job.question.is_some() => store.reopen(job.id),
                Err(JobFailure::Cancelled) => store.cancel(job.id),
                Err(JobFailure::Failed(error)) => store.fail(job.id, &error.to_string()),
            };
        }
        self.running().remove(&job.id);
        self.generation.fetch_add(1, Ordering::SeqCst);
        (self.sink)(match result {
            Ok(result) => AppEvent::JobCompleted { job_id: id, result },
            Err(JobFailure::Cancelled) => AppEvent::JobCancelled { job_id: id },
            Err(JobFailure::Failed(error)) => AppEvent::JobFailed {
                job_id: id,
                message: error.to_string(),
            },
        });
        self.publish();
    }

    fn settings(&self, job: &Job) -> crate::Result<Settings> {
        Settings::resolve(self.jobs.paths(), &job.params)
    }

    fn run_refresh(&self, job: &Job, job_id: &str, cancel: &AtomicBool) -> Outcome {
        let settings = self.settings(job)?;
        let pending = watchlist::sync(
            &settings,
            job.params.get("playlist_id").and_then(Value::as_str),
            cancel,
            &mut |event| self.event(job_id, Source::Workflow, event),
        )?;
        let queued = self.jobs.queue_pending(&job.params, &pending)?;
        self.event(
            job_id,
            Source::Workflow,
            JobEvent::message(format!("Queued {queued} item(s).")),
        );
        self.wake.notify_all();
        Ok(json!({"pending":pending.len(),"queued":queued}))
    }

    fn run_item(&self, job: &Job, job_id: &str, cancel: &AtomicBool) -> Outcome {
        let settings = self.settings(job)?;
        let model = settings.agent_model.as_deref();
        let parked = RefCell::new(None);
        let resume = RefCell::new(job.answer.as_ref().and_then(|answer| {
            let kind = answer["kind"].as_str()?.parse::<DecisionKind>().ok()?;
            Some((kind, answer["value"].clone()))
        }));
        let mut workflow_event = |event: JobEvent| {
            if matches!(event, JobEvent::ItemWaiting { .. }) {
                self.publish();
            }
            self.event(job_id, Source::Workflow, event);
        };
        let mut import_event = |event: JobEvent| self.event(job_id, Source::Native, event);
        let mut decide = |kind: DecisionKind, mut payload: Value| {
            let answered = {
                let mut resume = resume.borrow_mut();
                match resume.as_ref() {
                    Some((asked, _)) if *asked == kind => resume.take(),
                    _ => None,
                }
            };
            if let Some((_, value)) = answered {
                return Ok(value);
            }
            if let Some(value) = self.ask_agent(job_id, model, kind, &mut payload) {
                return Ok(value);
            }
            parked.replace(Some(watchlist::Parked { kind, payload }));
            Err("waiting for a choice".to_owned())
        };
        watchlist::action(
            &settings,
            &job.params,
            cancel,
            &mut workflow_event,
            &mut import_event,
            &mut decide,
            &parked,
        )
        .map_err(JobFailure::from)
    }

    fn run_workflow(&self, job: &Job, job_id: &str, cancel: &AtomicBool) -> Outcome {
        let settings = self.settings(job)?;
        let model = settings.agent_model.as_deref();
        let mut workflow_event = |event: JobEvent| self.event(job_id, Source::Workflow, event);
        let mut import_event = |event: JobEvent| self.event(job_id, Source::Native, event);
        let mut decide = |kind: DecisionKind, mut payload: Value| {
            if let Some(value) = self.ask_agent(job_id, model, kind, &mut payload) {
                return Ok(value);
            }
            gates::suspended(|| {
                (self.ask)(Prompt {
                    job_id,
                    title: &job.title,
                    kind,
                    payload,
                    cancelled: cancel,
                })
            })
        };
        if settings.request.raw.is_empty() {
            return Err(crate::Error::from("Enter a URL or path.").into());
        }
        let input = classify_input(&settings.request.raw);
        let result = if matches!(input, WorkflowInput::Local(_)) {
            local_workflow::run(
                &settings,
                cancel,
                &mut workflow_event,
                &mut import_event,
                &mut decide,
            )
        } else {
            remote_workflow::run(
                input,
                &settings,
                cancel,
                &Cell::new(Stage::Download),
                &mut workflow_event,
                &mut import_event,
                &mut decide,
            )
        }?;
        Ok(serde_json::to_value(result)?)
    }

    fn ask_agent(
        &self,
        job_id: &str,
        model: Option<&str>,
        kind: DecisionKind,
        payload: &mut Value,
    ) -> Option<Value> {
        if !muzik_agent::supports(kind) || muzik_agent::options(kind, payload).is_empty() {
            return None;
        }
        let model = model?;
        let chooser = self.chooser.as_ref()?;
        let message = |event: JobEvent| self.event(job_id, Source::Agent, event);
        if muzik_agent::strong_match(kind, payload).is_none() {
            message(JobEvent::message(format!("Asking {model} to choose.")));
        }
        match chooser.choose(kind, payload, model) {
            Ok(muzik_agent::Outcome::Decided(choice)) => {
                message(JobEvent::AgentDecided {
                    label: choice.label,
                    confidence: choice.confidence,
                    reason: choice.reason,
                });
                Some(choice.value)
            }
            Ok(muzik_agent::Outcome::Unsure {
                suggestion,
                confidence,
                reason,
            }) => {
                payload["agent"] = json!({"model":model,"suggestion":suggestion,"confidence":confidence,"reason":reason});
                None
            }
            Err(error) => {
                payload["agent"] = json!({"model":model,"error":error.to_string()});
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Options, Runner};
    use crate::events::AppEvent;
    use crate::queue::Jobs;
    use muzik_core::paths::Paths;
    use muzik_store::watchlist::{Repository, SourceKind, Stage, StageStatus, WatchItem};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn runner(jobs: &Arc<Jobs>) -> Result<(Runner, mpsc::Receiver<AppEvent>), String> {
        let (sender, receiver) = mpsc::channel();
        let runner = Runner::start(
            Arc::clone(jobs),
            Options {
                workers: 2,
                sink: Arc::new(move |message| {
                    let _ = sender.send(message);
                }),
                ask: Arc::new(|_| Err("no answer in tests".into())),
                chooser: None,
                generation: Arc::new(AtomicU64::new(0)),
            },
        )?
        .ok_or("the runner did not start")?;
        Ok((runner, receiver))
    }

    #[test]
    fn an_item_that_needs_a_choice_parks_and_resumes_with_the_answer() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let output = dir.path().join("downloads");
        std::fs::create_dir_all(&output).unwrap();
        let audio = output.join("Song [abcdefghijk].flac");
        std::fs::copy(crate::sources::testing::fixture(), &audio).unwrap();
        let config = crate::sources::testing::library_config(dir.path()).unwrap();
        let repository = Repository::open(&paths);
        repository
            .add("https://www.youtube.com/playlist?list=PL1")
            .unwrap();
        repository
            .update(|document| {
                let mut item = WatchItem::new(1, "Song", SourceKind::Youtube);
                item.video_id = Some("abcdefghijk".into());
                item.video_url = Some("https://www.youtube.com/watch?v=abcdefghijk".into());
                item.complete(Stage::Download, Some(audio.clone()));
                document.playlists[0].items = vec![item];
                Ok(())
            })
            .unwrap();
        let jobs = Arc::new(Jobs::open(&paths).unwrap());
        let (runner, _) = runner(&jobs).unwrap();
        jobs.item(&json!({
            "playlist_id":"PL1","position":1,"video_id":"abcdefghijk","title":"Song",
            "action":"organize_again","output":output,"config":config,"interactive":true
        }))
        .unwrap();
        runner.wake();
        let started = Instant::now();
        let waiting = loop {
            if let Some(job) = jobs.snapshot()["waiting"]
                .as_array()
                .and_then(|waiting| waiting.first().cloned())
            {
                break job;
            }
            assert!(
                started.elapsed() <= Duration::from_secs(20),
                "the item did not park"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(waiting["kind"], "import_match");
        assert_eq!(waiting["item"], "PL1:1:abcdefghijk");
        assert_eq!(
            repository.load().unwrap().playlists[0].items[0].status(Stage::Organize),
            StageStatus::Waiting
        );
        let id = waiting["id"].as_i64().unwrap();
        assert!(jobs.answer(id, &json!("as_is")).unwrap());
        runner.wake();
        runner.wait_until_idle(&AtomicBool::new(false));
        assert_eq!(
            repository.load().unwrap().playlists[0].items[0].status(Stage::Organize),
            StageStatus::Complete
        );
        assert_eq!(
            muzik_library::Library::open_read_only(&dir.path().join("library.db"))
                .unwrap()
                .items()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_workflow_job_runs_and_reports_its_result() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("track.flac");
        std::fs::write(&audio, b"audio").unwrap();
        let jobs = Arc::new(Jobs::in_memory(&Paths::under(dir.path())).unwrap());
        let (runner, receiver) = runner(&jobs).unwrap();
        let id = jobs
            .workflow(&json!({"raw":audio,"no_organize":true,"no_split":true,"dry_run":true}))
            .unwrap();
        runner.wake();
        let job_id = super::job_id(id);
        loop {
            if let AppEvent::JobCompleted {
                job_id: done,
                result,
            } = receiver.recv_timeout(Duration::from_secs(20)).unwrap()
                && done == job_id
            {
                assert_eq!(result, json!({"albums":0,"singles":1,"split_dirs":[]}));
                break;
            }
        }
        runner.wait_until_idle(&std::sync::atomic::AtomicBool::new(false));
        assert!(runner.is_idle());
    }

    #[test]
    fn an_interrupt_stops_the_runner_and_keeps_queued_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("track.flac");
        std::fs::write(&audio, b"audio").unwrap();
        let jobs = Arc::new(Jobs::in_memory(&Paths::under(dir.path())).unwrap());
        let (runner, _receiver) = runner(&jobs).unwrap();
        runner.wait_until_idle(&AtomicBool::new(true));
        let id = jobs
            .workflow(&json!({"raw":audio,"no_organize":true,"no_split":true,"dry_run":true}))
            .unwrap();
        runner.wake();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(
            jobs.store().get(id).unwrap().map(|job| job.status),
            Some(muzik_store::jobs::Status::Queued)
        );
    }
}
