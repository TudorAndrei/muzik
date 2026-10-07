use crate::agent::Chooser;
use crate::events::{AppEvent, Source};
use crate::queue::{Jobs, RunnerClaim, job_id};
use crate::settings::Settings;
use crate::{gates, local_workflow, remote_workflow, watchlist};
use muzik_core::{DecisionKind, JobEvent};
use muzik_store::jobs::{Job, Kind, Queue, RunnerLock, Store};
use muzik_store::watchlist::Stage;
use muzik_store::watchlist::jobs::JobError;
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
pub type Running = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

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

type Outcome = Result<Value, (bool, String)>;

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

    fn running(&self) -> MutexGuard<'_, HashMap<String, Arc<AtomicBool>>> {
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
            thread::spawn(move || work(&shared));
        }
        let watcher = Arc::clone(&shared);
        thread::spawn(move || watch(&watcher));
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
    pub fn cancel(&self, job_id: &str) -> bool {
        self.shared.running().get(job_id).is_some_and(|cancel| {
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

fn work(shared: &Arc<Shared>) {
    while !shared.stop.load(Ordering::SeqCst) {
        let claimed = shared.store().claim_any(&QUEUES).ok().flatten();
        let Some(job) = claimed else {
            let mut idle = shared.idle.lock();
            shared.wake.wait_for(&mut idle, Duration::from_secs(1));
            continue;
        };
        run_job(shared, &job);
    }
}

fn watch(shared: &Arc<Shared>) {
    let mut last = Value::Null;
    while !shared.stop.load(Ordering::SeqCst) {
        let requests = shared.store().cancel_requests().unwrap_or_default();
        {
            let running = shared.running();
            for id in requests {
                if let Some(cancel) = running.get(&job_id(id)) {
                    cancel.store(true, Ordering::SeqCst);
                }
            }
        }
        let snapshot = shared.jobs.snapshot();
        if snapshot != last {
            (shared.sink)(AppEvent::JobsUpdated(snapshot.clone()));
            last = snapshot;
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn run_job(shared: &Arc<Shared>, job: &Job) {
    let id = job_id(job.id);
    let cancel = Arc::new(AtomicBool::new(false));
    shared.running().insert(id.clone(), Arc::clone(&cancel));
    shared.generation.fetch_add(1, Ordering::SeqCst);
    gates::set_label(&job.title);
    (shared.sink)(AppEvent::JobStarted {
        job_id: id.clone(),
        title: job.title.clone(),
        kind: job.kind,
    });
    shared.publish();
    let result = match job.kind {
        Kind::Refresh => run_refresh(shared, job, &id, &cancel),
        Kind::Item => run_item(shared, job, &id, &cancel),
        Kind::Workflow => run_workflow(shared, job, &id, &cancel),
    };
    {
        let store = shared.store();
        let _ = match &result {
            Ok(_) => store.finish(job.id),
            Err((true, _)) if job.question.is_some() => store.reopen(job.id),
            Err((true, _)) => store.cancel(job.id),
            Err((false, message)) => store.fail(job.id, message),
        };
    }
    shared.running().remove(&id);
    shared.generation.fetch_add(1, Ordering::SeqCst);
    (shared.sink)(match result {
        Ok(result) => AppEvent::JobCompleted { job_id: id, result },
        Err((true, _)) => AppEvent::JobCancelled { job_id: id },
        Err((false, message)) => AppEvent::JobFailed {
            job_id: id,
            message,
        },
    });
    shared.publish();
}

fn settings(shared: &Shared, job: &Job) -> Result<Settings, (bool, String)> {
    Settings::resolve(shared.jobs.paths(), &job.params).map_err(|error| (false, error.to_string()))
}

fn run_refresh(shared: &Shared, job: &Job, job_id: &str, cancel: &AtomicBool) -> Outcome {
    let settings = settings(shared, job)?;
    let pending = watchlist::sync(
        &settings,
        job.params.get("playlist_id").and_then(Value::as_str),
        cancel,
        &mut |event| shared.event(job_id, Source::Workflow, event),
    )
    .map_err(|error| job_error(&error))?;
    let queued = shared
        .jobs
        .queue_pending(&job.params, &pending)
        .map_err(|error| (false, error.to_string()))?;
    shared.event(
        job_id,
        Source::Workflow,
        JobEvent::message(format!("Queued {queued} item(s).")),
    );
    shared.wake.notify_all();
    Ok(json!({"pending":pending.len(),"queued":queued}))
}

fn run_item(shared: &Shared, job: &Job, job_id: &str, cancel: &AtomicBool) -> Outcome {
    let settings = settings(shared, job)?;
    let model = settings.agent_model.as_deref();
    let parked = RefCell::new(None);
    let resume = RefCell::new(job.answer.as_ref().and_then(|answer| {
        let kind = answer["kind"].as_str()?.parse::<DecisionKind>().ok()?;
        Some((kind, answer["value"].clone()))
    }));
    let mut workflow_event = |event: JobEvent| {
        if matches!(event, JobEvent::ItemWaiting { .. }) {
            shared.publish();
        }
        shared.event(job_id, Source::Workflow, event);
    };
    let mut import_event = |event: JobEvent| shared.event(job_id, Source::Native, event);
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
        if let Some(value) = ask_agent(shared, job_id, model, kind, &mut payload) {
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
    .map_err(|error| job_error(&error))
}

fn run_workflow(shared: &Shared, job: &Job, job_id: &str, cancel: &AtomicBool) -> Outcome {
    let settings = settings(shared, job)?;
    let model = settings.agent_model.as_deref();
    let mut workflow_event = |event: JobEvent| shared.event(job_id, Source::Workflow, event);
    let mut import_event = |event: JobEvent| shared.event(job_id, Source::Native, event);
    let mut decide = |kind: DecisionKind, mut payload: Value| {
        if let Some(value) = ask_agent(shared, job_id, model, kind, &mut payload) {
            return Ok(value);
        }
        gates::suspended(|| {
            (shared.ask)(Prompt {
                job_id,
                title: &job.title,
                kind,
                payload,
                cancelled: cancel,
            })
        })
    };
    let workflow_failure = |error: muzik_workflow::Error| {
        (
            matches!(error, muzik_workflow::Error::Cancelled),
            error.to_string(),
        )
    };
    if settings.request.raw.is_empty() {
        return Err((false, "Enter a URL or path.".to_owned()));
    }
    let input = classify_input(&settings.request.raw);
    if matches!(input, WorkflowInput::Local(_)) {
        return local_workflow::run(
            &settings,
            cancel,
            &mut workflow_event,
            &mut import_event,
            &mut decide,
        )
        .map_err(workflow_failure);
    }
    remote_workflow::run(
        input,
        &settings,
        cancel,
        &Cell::new(Stage::Download),
        &mut workflow_event,
        &mut import_event,
        &mut decide,
    )
    .map_err(workflow_failure)
}

fn ask_agent(
    shared: &Shared,
    job_id: &str,
    model: Option<&str>,
    kind: DecisionKind,
    payload: &mut Value,
) -> Option<Value> {
    if !muzik_agent::supports(kind) || muzik_agent::options(kind, payload).is_empty() {
        return None;
    }
    let model = model?;
    let chooser = shared.chooser.as_ref()?;
    let message = |event: JobEvent| shared.event(job_id, Source::Agent, event);
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

fn job_error(error: &JobError) -> (bool, String) {
    (matches!(error, JobError::Cancelled), error.to_string())
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
                assert_eq!(result["singles"], 1);
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
