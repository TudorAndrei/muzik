use crate::queue::{item_key, job_id, Jobs};
use crate::{gates, local_workflow, remote_workflow, watchlist};
use muzik_core::watchlist::jobs::JobError;
use muzik_core::watchlist::{ItemAction, Stage};
use muzik_core::{app_config, DecisionKind};
use muzik_jobs::{Job, Kind, NewJob, Queue, RunnerLock, Store};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;
use strum_macros::AsRefStr;

const QUEUES: [Queue; 3] = [Queue::Sync, Queue::Workflow, Queue::Item];

pub type Sink = Arc<dyn Fn(Value) + Send + Sync>;
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
    pub generation: Arc<AtomicU64>,
}

#[derive(Clone, Copy, AsRefStr)]
#[strum(serialize_all = "snake_case")]
enum Source {
    Workflow,
    Native,
    Agent,
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
    generation: Arc<AtomicU64>,
}

impl Shared {
    fn store(&self) -> MutexGuard<'_, Store> {
        self.jobs.store()
    }

    fn event(&self, job_id: &str, source: Source, event: &Value) {
        (self.sink)(
            json!({"type":"event","event":"job.event","data":{"job_id":job_id,"source":source.as_ref(),"event":event["event"],"data":event["data"]}}),
        );
    }

    fn publish(&self) {
        let snapshot = self.jobs.snapshot();
        (self.sink)(json!({"type":"event","event":"jobs.updated","data":snapshot}));
    }

    fn running(&self) -> MutexGuard<'_, HashMap<String, Arc<AtomicBool>>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
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
    pub fn start(jobs: Arc<Jobs>, options: Options) -> Result<Option<Self>, String> {
        let Some(lock) = jobs.runner_lock()? else {
            return Ok(None);
        };
        jobs.store().recover()?;
        jobs.release_spotify_questions()?;
        let shared = Arc::new(Shared {
            jobs,
            running: Arc::new(Mutex::new(HashMap::new())),
            idle: Mutex::new(()),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            sink: options.sink,
            ask: options.ask,
            generation: options.generation,
        });
        let sink = Arc::clone(&shared.sink);
        gates::listen(move |snapshot| {
            sink(json!({"type":"event","event":"queues.updated","data":snapshot}));
        });
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

    pub fn running(&self) -> Running {
        Arc::clone(&self.shared.running)
    }

    pub fn cancel(&self, job_id: &str) -> bool {
        match self.shared.running().get(job_id) {
            Some(cancel) => {
                cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    pub fn wake(&self) {
        self.shared.wake.notify_all();
    }

    pub fn publish(&self) {
        self.shared.publish();
    }

    pub fn is_idle(&self) -> bool {
        self.shared.running().is_empty()
            && self
                .shared
                .store()
                .list_open()
                .is_ok_and(|jobs| jobs.is_empty())
    }

    pub fn wait_until_idle(&self, interrupted: &AtomicBool) {
        while !self.is_idle() {
            if interrupted.load(Ordering::SeqCst) {
                for cancel in self.shared.running().values() {
                    cancel.store(true, Ordering::SeqCst);
                }
            }
            thread::sleep(Duration::from_millis(300));
        }
    }

    #[cfg(test)]
    pub(crate) fn stop_workers(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.wake.notify_all();
    }
}

fn work(shared: &Arc<Shared>) {
    while !shared.stop.load(Ordering::SeqCst) {
        let claimed = shared.store().claim_any(&QUEUES).ok().flatten();
        let Some(job) = claimed else {
            let idle = shared.idle.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = shared.wake.wait_timeout(idle, Duration::from_secs(1));
            continue;
        };
        run_job(shared, job);
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
            (shared.sink)(json!({"type":"event","event":"jobs.updated","data":snapshot}));
            last = snapshot;
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn run_job(shared: &Arc<Shared>, job: Job) {
    let id = job_id(job.id);
    let cancel = Arc::new(AtomicBool::new(false));
    shared.running().insert(id.clone(), Arc::clone(&cancel));
    shared.generation.fetch_add(1, Ordering::SeqCst);
    gates::set_label(&job.title);
    (shared.sink)(
        json!({"type":"event","event":"job.started","data":{"job_id":id,"title":job.title,"kind":job.kind.as_ref()}}),
    );
    shared.publish();
    let result = match job.kind {
        Kind::Refresh => run_refresh(shared, &job, &id, &cancel),
        Kind::Item => run_item(shared, &job, &id, &cancel),
        Kind::Workflow => run_workflow(shared, &job, &id, &cancel),
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
        Ok(result) => {
            json!({"type":"event","event":"job.completed","data":{"job_id":id,"result":result}})
        }
        Err((true, _)) => json!({"type":"event","event":"job.cancelled","data":{"job_id":id}}),
        Err((false, message)) => {
            json!({"type":"event","event":"job.failed","data":{"job_id":id,"error":{"code":"operation_failed","message":message}}})
        }
    });
    shared.publish();
}

fn run_refresh(shared: &Shared, job: &Job, job_id: &str, cancel: &AtomicBool) -> Outcome {
    let pending = watchlist::sync(&job.params, cancel, &mut |event| {
        shared.event(job_id, Source::Workflow, &event);
    })
    .map_err(job_error)?;
    let mut queued = 0;
    {
        let store = shared.store();
        for item in &pending {
            let mut params = job.params.clone();
            params["playlist_id"] = json!(item.playlist_id);
            params["position"] = json!(item.position);
            params["video_id"] = json!(item.video_id);
            params["title"] = json!(item.title);
            params["action"] = json!(ItemAction::Run);
            let key = item_key(&params);
            if store
                .find_open(Kind::Item, &key)
                .map_err(|error| (false, error))?
                .is_empty()
            {
                store
                    .enqueue(&NewJob {
                        kind: Kind::Item,
                        item_key: &key,
                        title: &item.title,
                        params: &params,
                    })
                    .map_err(|error| (false, error))?;
                queued += 1;
            }
        }
    }
    shared.event(
        job_id,
        Source::Workflow,
        &json!({"event":"message","data":{"message":format!("Queued {queued} item(s).")}}),
    );
    shared.wake.notify_all();
    Ok(json!({"pending":pending.len(),"queued":queued}))
}

fn run_item(shared: &Shared, job: &Job, job_id: &str, cancel: &AtomicBool) -> Outcome {
    let parked = RefCell::new(None);
    let resume = RefCell::new(job.answer.as_ref().and_then(|answer| {
        let kind = answer["kind"].as_str()?.parse::<DecisionKind>().ok()?;
        Some((kind, answer["value"].clone()))
    }));
    let mut workflow_event = |event: Value| {
        if event["event"] == "item_waiting" {
            let _ = park_item(&shared.store(), &job.params, &event["data"]);
            shared.publish();
        }
        shared.event(job_id, Source::Workflow, &event);
    };
    let mut import_event = |event: Value| shared.event(job_id, Source::Native, &event);
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
        if let Some(value) = ask_agent(shared, job_id, kind, &mut payload) {
            return Ok(value);
        }
        parked.replace(Some(watchlist::Parked { kind, payload }));
        Err("waiting for a choice".to_owned())
    };
    watchlist::action(
        &job.params,
        cancel,
        &mut workflow_event,
        &mut import_event,
        &mut decide,
        &parked,
    )
    .map_err(job_error)
}

fn run_workflow(shared: &Shared, job: &Job, job_id: &str, cancel: &AtomicBool) -> Outcome {
    let mut workflow_event = |event: Value| shared.event(job_id, Source::Workflow, &event);
    let mut import_event = |event: Value| shared.event(job_id, Source::Native, &event);
    let mut decide = |kind: DecisionKind, mut payload: Value| {
        if let Some(value) = ask_agent(shared, job_id, kind, &mut payload) {
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
    if let Some(local) = local_workflow::supported(&job.params) {
        let local = local.map_err(|message| (false, message))?;
        return local_workflow::run(
            local,
            cancel,
            &mut workflow_event,
            &mut import_event,
            &mut decide,
        )
        .map_err(workflow_failure);
    }
    let remote = remote_workflow::supported(&job.params)
        .ok_or((false, "Enter a URL or path.".to_owned()))?
        .map_err(|message| (false, message))?;
    remote_workflow::run(
        remote,
        cancel,
        &mut workflow_event,
        &mut import_event,
        &mut decide,
    )
    .map_err(workflow_failure)
}

fn ask_agent(
    shared: &Shared,
    job_id: &str,
    kind: DecisionKind,
    payload: &mut Value,
) -> Option<Value> {
    let model = agent_model(kind, payload)?;
    let message = |event: &str, data: Value| {
        shared.event(job_id, Source::Agent, &json!({"event":event,"data":data}));
    };
    if muzik_agent::strong_match(kind, payload).is_none() {
        message(
            "message",
            json!({"message":format!("Asking {model} to choose.")}),
        );
    }
    match muzik_agent::decide(kind, payload, &model) {
        Ok(muzik_agent::Outcome::Decided(choice)) => {
            message(
                "agent_decided",
                json!({"kind":kind,"label":choice.label,"confidence":choice.confidence,"reason":choice.reason}),
            );
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
            payload["agent"] = json!({"model":model,"error":error});
            None
        }
    }
}

fn agent_model(kind: DecisionKind, payload: &Value) -> Option<String> {
    if !muzik_agent::supports(kind) || muzik_agent::options(kind, payload).is_empty() {
        return None;
    }
    let settings = app_config::load_gui_defaults(&app_config::path()).ok()?;
    if settings["auto_decide"] != true {
        return None;
    }
    let model = settings["agent_model"]
        .as_str()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(muzik_agent::DEFAULT_MODEL);
    Some(model.to_owned())
}

fn job_error(error: JobError) -> (bool, String) {
    (matches!(error, JobError::Cancelled), error.to_string())
}

pub(crate) fn park_item(store: &Store, params: &Value, data: &Value) -> Result<i64, String> {
    let stage = data["stage"]
        .as_str()
        .and_then(|stage| stage.parse::<Stage>().ok())
        .unwrap_or(Stage::Download);
    let mut params = params.clone();
    params["playlist_id"] = data["playlist_id"].clone();
    params["position"] = data["position"].clone();
    params["video_id"] = json!(data["video_id"].as_str().unwrap_or(""));
    params["action"] = json!(stage.resume_action());
    let key = item_key(&params);
    let title = data["title"]
        .as_str()
        .or_else(|| params["title"].as_str())
        .unwrap_or("Item")
        .to_owned();
    store.park(
        &NewJob {
            kind: Kind::Item,
            item_key: &key,
            title: &title,
            params: &params,
        },
        &data["question"],
    )
}

#[cfg(test)]
mod tests {
    use super::{park_item, Options, Runner};
    use crate::queue::Jobs;
    use muzik_jobs::Queue;
    use serde_json::{json, Value};
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn runner(jobs: &Arc<Jobs>) -> Result<(Runner, mpsc::Receiver<Value>), String> {
        let (sender, receiver) = mpsc::channel();
        let sender = Mutex::new(sender);
        let runner = Runner::start(
            Arc::clone(jobs),
            Options {
                workers: 2,
                sink: Arc::new(move |message| {
                    if let Ok(sender) = sender.lock() {
                        let _ = sender.send(message);
                    }
                }),
                ask: Arc::new(|_| Err("no answer in tests".into())),
                generation: Arc::new(AtomicU64::new(0)),
            },
        )?
        .ok_or("the runner did not start")?;
        Ok((runner, receiver))
    }

    #[test]
    fn a_parked_choice_waits_for_an_answer_and_then_queues() -> Result<(), String> {
        let jobs = Arc::new(Jobs::in_memory()?);
        let (runner, _) = runner(&jobs)?;
        runner.stop_workers();
        std::thread::sleep(Duration::from_millis(1200));
        let question = json!({"kind":"import_match","payload":{"matches":[]}});
        let id = park_item(
            &jobs.store(),
            &json!({"output":"/music"}),
            &json!({"playlist_id":"PL1","position":3,"video_id":"abcdefghijk","title":"Album","stage":"organize","question":question}),
        )?;
        assert_eq!(
            jobs.snapshot()["waiting"],
            json!([{"id":id,"title":"Album","kind":"import_match","payload":{"matches":[]},"item":"PL1:3:abcdefghijk"}])
        );
        assert!(jobs.answer(id, &json!("release:1"))?);
        let job = jobs
            .store()
            .claim(Queue::Item)?
            .ok_or("job is not queued")?;
        assert_eq!(
            job.answer,
            Some(json!({"kind":"import_match","value":"release:1"}))
        );
        assert_eq!(
            job.params,
            json!({"output":"/music","playlist_id":"PL1","position":3,"video_id":"abcdefghijk","action":"organize_again"})
        );
        Ok(())
    }

    #[test]
    fn a_workflow_job_runs_and_reports_its_result() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let audio = dir.path().join("track.flac");
        std::fs::write(&audio, b"audio")?;
        let jobs = Arc::new(Jobs::in_memory()?);
        let (runner, receiver) = runner(&jobs)?;
        let id =
            jobs.workflow(&json!({"raw":audio,"no_organize":true,"no_split":true,"dry_run":true}))?;
        runner.wake();
        let job_id = super::job_id(id);
        loop {
            let message = receiver.recv_timeout(Duration::from_secs(20))?;
            if message["event"] == "job.completed" && message["data"]["job_id"] == job_id {
                assert_eq!(message["data"]["result"]["singles"], 1);
                break;
            }
        }
        runner.wait_until_idle(&std::sync::atomic::AtomicBool::new(false));
        assert!(runner.is_idle());
        Ok(())
    }
}
