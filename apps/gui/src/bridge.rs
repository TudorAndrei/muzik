//! JSON protocol for the Rust desktop application.
use crate::{
    local_workflow, native, native_watchlist, queues, remote_workflow, thumbnails, watchlist,
};
use muzik_core::watchlist::jobs::JobError;
use muzik_core::watchlist::{ItemAction, Stage};
use muzik_core::{app_config, paths, spotify, watchlist::Repository, DecisionKind};
use muzik_jobs::{Job, Kind, NewJob, Queue, Status, Store};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use strum_macros::AsRefStr;

const WORKERS: usize = 5;
const QUEUES: [Queue; 3] = [Queue::Sync, Queue::Workflow, Queue::Item];

#[derive(Clone, Copy, AsRefStr)]
#[strum(serialize_all = "snake_case")]
enum Source {
    Workflow,
    Native,
    Agent,
}

pub struct Bridge {
    output: Receiver<Value>,
    native_output: Sender<Value>,
    thumbnail_pending: Arc<Mutex<HashSet<String>>>,
    login: Arc<Mutex<Option<NativeLogin>>>,
    shared: Arc<Shared>,
    watchlist_gate: Arc<Mutex<()>>,
    next_id: u64,
}

struct NativeLogin {
    job_id: String,
    cancel: Arc<AtomicBool>,
}

struct Shared {
    jobs: Mutex<Store>,
    running: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    decisions: Mutex<HashMap<String, Sender<Value>>>,
    idle: Mutex<()>,
    wake: Condvar,
    stop: AtomicBool,
    generation: Arc<AtomicU64>,
    sender: Sender<Value>,
}

impl Shared {
    fn store(&self) -> MutexGuard<'_, Store> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn send(&self, message: Value) {
        let _ = self.sender.send(message);
    }

    fn event(&self, job_id: &str, source: Source, event: &Value) {
        self.send(json!({"type":"event","event":"job.event","data":{"job_id":job_id,"source":source.as_ref(),"event":event["event"],"data":event["data"]}}));
    }

    fn publish(&self) {
        let snapshot = jobs_snapshot(&self.store());
        self.send(json!({"type":"event","event":"jobs.updated","data":snapshot}));
    }

    fn wake(&self) {
        self.wake.notify_all();
    }

    fn running(&self) -> MutexGuard<'_, HashMap<String, Arc<AtomicBool>>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for cancel in self.shared.running().values() {
            cancel.store(true, Ordering::SeqCst);
        }
        self.shared.wake();
    }
}

impl Bridge {
    pub fn start() -> Result<Self, String> {
        let (events, output) = mpsc::channel::<Value>();
        let shared = Arc::new(Shared {
            jobs: Mutex::new(open_jobs()?),
            running: Arc::new(Mutex::new(HashMap::new())),
            decisions: Mutex::new(HashMap::new()),
            idle: Mutex::new(()),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            generation: Arc::new(AtomicU64::new(0)),
            sender: events.clone(),
        });
        let gates = Mutex::new(events.clone());
        queues::listen(move |snapshot| {
            if let Ok(sender) = gates.lock() {
                let _ =
                    sender.send(json!({"type":"event","event":"queues.updated","data":snapshot}));
            }
        });
        for _ in 0..WORKERS {
            let shared = Arc::clone(&shared);
            thread::spawn(move || work(&shared));
        }
        Ok(Self {
            output,
            native_output: events,
            thumbnail_pending: Arc::new(Mutex::new(HashSet::new())),
            login: Arc::new(Mutex::new(None)),
            shared,
            watchlist_gate: Arc::new(Mutex::new(())),
            next_id: 1,
        })
    }

    fn respond(&self, message: Value) -> Result<(), String> {
        self.native_output
            .send(message)
            .map_err(|_| "Rust backend is not available".to_owned())
    }

    fn reject(&self, id: &str, code: &str, message: impl Into<String>) -> Result<String, String> {
        self.respond(json!({"id":id,"type":"response","ok":false,"error":{"code":code,"message":message.into()}}))?;
        Ok(id.to_owned())
    }

    fn accept(&self, id: &str, result: Value) -> Result<String, String> {
        self.respond(json!({"id":id,"type":"response","ok":true,"result":result}))?;
        Ok(id.to_owned())
    }

    pub fn send(&mut self, command: &str, params: Value) -> Result<String, String> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        match command {
            "spotify.login" => return self.start_spotify_login(id, params),
            "decision.reply" => return self.reply(&id, &params),
            "job.cancel" => return self.cancel(&id, &params),
            "jobs.list" => {
                let mut snapshot = jobs_snapshot(&self.shared.store());
                snapshot["gates"] = queues::snapshot();
                return self.accept(&id, snapshot);
            }
            "jobs.answer" => return self.answer(&id, &params),
            "workflow.start" => return self.start_workflow(&id, params),
            "watchlist.refresh" => {
                return self.enqueue(
                    &id,
                    &NewJob {
                        kind: Kind::Refresh,
                        item_key: "refresh",
                        title: "Watchlist check",
                        params: &params,
                    },
                )
            }
            "watchlist.action" => return self.start_item(&id, params),
            _ => {}
        }
        if command == "thumbnails.cache" {
            let ids = match thumbnails::validate_ids(&params) {
                Ok(ids) => ids,
                Err(message) => return self.reject(&id, "invalid_request", message),
            };
            let mut pending = self
                .thumbnail_pending
                .lock()
                .map_err(|_| "thumbnail queue is not available")?;
            let fresh = ids
                .into_iter()
                .filter(|id| pending.insert(id.clone()))
                .collect::<Vec<_>>();
            self.accept(&id, json!({"queued": fresh.len()}))?;
            drop(pending);
            if !fresh.is_empty() {
                let sender = self.native_output.clone();
                let pending = Arc::clone(&self.thumbnail_pending);
                thread::spawn(move || {
                    let (watchlist, cache) = thumbnails::default_paths();
                    let data = thumbnails::cache_requested(&fresh, &watchlist, &cache);
                    let _ = sender
                        .send(json!({"type":"event", "event":"thumbnails.updated", "data":data}));
                    if let Ok(mut pending) = pending.lock() {
                        for id in fresh {
                            pending.remove(&id);
                        }
                    }
                });
            }
            return Ok(id);
        }
        if command == "watchlist.load" {
            let _gate = self
                .watchlist_gate
                .lock()
                .map_err(|_| "Watchlist state is unavailable")?;
            let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
            let response_id = id.clone();
            let sender = self.native_output.clone();
            let login = Arc::clone(&self.login);
            let running = Arc::clone(&self.shared.running);
            let latest = Arc::clone(&self.shared.generation);
            let gate = Arc::clone(&self.watchlist_gate);
            let repository = Repository::new(Repository::default_path());
            thread::spawn(move || {
                load_watchlist(WatchlistLoad {
                    sender,
                    id: response_id,
                    params,
                    repository,
                    login,
                    running,
                    latest,
                    gate,
                    generation,
                });
            });
            return Ok(id);
        }
        if matches!(
            command,
            "watchlist.add" | "watchlist.rename" | "watchlist.remove"
        ) {
            let _gate = self
                .watchlist_gate
                .lock()
                .map_err(|_| "Watchlist state is unavailable")?;
            let response = native_response(&id, command, &params);
            if response["ok"] == true {
                self.shared.generation.fetch_add(1, Ordering::SeqCst);
            }
            self.respond(response)?;
            return Ok(id);
        }
        if native::handles(command) {
            if matches!(
                command,
                "library.scan" | "services.check" | "spotify.status" | "spotify.playlists"
            ) {
                let sender = self.native_output.clone();
                let response_id = id.clone();
                let command = command.to_owned();
                thread::spawn(move || {
                    let response = native_response(&response_id, &command, &params);
                    let _ = sender.send(response);
                });
            } else {
                self.respond(native_response(&id, command, &params))?;
            }
            return Ok(id);
        }
        self.reject(
            &id,
            "invalid_request",
            format!("Unknown command: {command}"),
        )
    }

    fn reply(&self, id: &str, params: &Value) -> Result<String, String> {
        let decision_id = params
            .get("decision_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        if decision_id.is_empty() {
            return self.reject(
                id,
                "invalid_request",
                "decision_id must be a non-empty string.",
            );
        }
        let reply = self
            .shared
            .decisions
            .lock()
            .map_err(|_| "Decision state is unavailable")?
            .remove(decision_id);
        let sent = reply.is_some_and(|reply| {
            reply
                .send(params.get("value").cloned().unwrap_or(Value::Null))
                .is_ok()
        });
        if sent {
            self.accept(id, json!({"decision_id":decision_id}))
        } else {
            self.reject(id, "invalid_request", "The decision is not pending.")
        }
    }

    fn cancel(&self, id: &str, params: &Value) -> Result<String, String> {
        let job_id = params.get("job_id").and_then(Value::as_str).unwrap_or("");
        if job_id.is_empty() {
            return self.reject(id, "invalid_request", "job_id must be a non-empty string.");
        }
        if let Some(cancel) = self.shared.running().get(job_id) {
            cancel.store(true, Ordering::SeqCst);
            return self.accept(id, json!({"job_id":job_id,"cancel_requested":true}));
        }
        let queued = job_id
            .strip_prefix("queue-")
            .and_then(|number| number.parse::<i64>().ok());
        if let Some(number) = queued {
            if self.shared.store().cancel_open(number)? {
                self.accept(id, json!({"job_id":job_id,"cancel_requested":true}))?;
                self.shared
                    .send(json!({"type":"event","event":"job.cancelled","data":{"job_id":job_id}}));
                self.shared.publish();
                return Ok(id.to_owned());
            }
        }
        let login = self
            .login
            .lock()
            .map_err(|_| "Spotify login is not available")?;
        if let Some(active) = login.as_ref().filter(|active| active.job_id == job_id) {
            active.cancel.store(true, Ordering::Relaxed);
            return self.accept(id, json!({"job_id":job_id,"cancel_requested":true}));
        }
        self.reject(id, "invalid_request", "The job is not active.")
    }

    fn answer(&self, id: &str, params: &Value) -> Result<String, String> {
        let (Some(job_id), Some(value)) = (params["id"].as_i64(), params.get("value")) else {
            return self.reject(id, "invalid_request", "id and value are required.");
        };
        let answered = {
            let store = self.shared.store();
            let kind = store
                .get(job_id)?
                .and_then(|job| job.question)
                .map(|question| question["kind"].clone())
                .unwrap_or(Value::Null);
            store.answer(job_id, &json!({"kind":kind,"value":value}))?
        };
        self.accept(id, json!({"answered":answered}))?;
        self.shared.publish();
        self.shared.wake();
        Ok(id.to_owned())
    }

    fn start_workflow(&self, id: &str, params: Value) -> Result<String, String> {
        let raw = params["raw"].as_str().unwrap_or("").trim().to_owned();
        if raw.is_empty() {
            return self.reject(id, "invalid_request", "Enter a URL or path.");
        }
        let checked = local_workflow::supported(&params)
            .map(|request| request.map(drop))
            .or_else(|| remote_workflow::supported(&params).map(|request| request.map(drop)));
        match checked {
            Some(Ok(())) => {}
            Some(Err(message)) => return self.reject(id, "invalid_request", message),
            None => return self.reject(id, "invalid_request", "Enter a URL or path."),
        }
        let key = format!("{raw}#{}", unique());
        self.enqueue(
            id,
            &NewJob {
                kind: Kind::Workflow,
                item_key: &key,
                title: &raw,
                params: &params,
            },
        )
    }

    fn start_item(&self, id: &str, params: Value) -> Result<String, String> {
        if let Err(message) = validate_item(&params) {
            return self.reject(id, "invalid_request", message);
        }
        let key = item_key(&params);
        {
            let store = self.shared.store();
            for open in store.find_open(Kind::Item, &key)? {
                if open.status == Status::Waiting {
                    store.cancel_open(open.id)?;
                } else {
                    drop(store);
                    return self.reject(
                        id,
                        "job_active",
                        "This item already has a job in the queue.",
                    );
                }
            }
        }
        let title = params["title"]
            .as_str()
            .or_else(|| params["video_id"].as_str())
            .unwrap_or("Item")
            .to_owned();
        self.enqueue(
            id,
            &NewJob {
                kind: Kind::Item,
                item_key: &key,
                title: &title,
                params: &params,
            },
        )
    }

    fn enqueue(&self, id: &str, job: &NewJob<'_>) -> Result<String, String> {
        let number = self.shared.store().enqueue(job)?;
        self.accept(id, json!({"job_id":format!("queue-{number}")}))?;
        self.shared.publish();
        self.shared.wake();
        Ok(id.to_owned())
    }

    fn start_spotify_login(&mut self, id: String, params: Value) -> Result<String, String> {
        let port = match params.get("port") {
            None | Some(Value::Null) => None,
            Some(value) => match value
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port > 0)
            {
                Some(port) => Some(port),
                None => {
                    return self.reject(
                        &id,
                        "invalid_request",
                        "port must be an integer from 1 to 65535.",
                    )
                }
            },
        };
        let mut login = self
            .login
            .lock()
            .map_err(|_| "Spotify login is not available")?;
        if login.is_some() {
            drop(login);
            return self.reject(&id, "job_active", "A Spotify login is already active.");
        }
        let config = app_config::path();
        if let Some(port) = port {
            if let Err(message) = app_config::save_section_string(
                &config,
                "spotify",
                "redirect_port",
                &port.to_string(),
            ) {
                drop(login);
                return self.reject(&id, "operation_failed", message);
            }
        }
        let job_id = format!("spotify-login-{id}");
        let cancel = Arc::new(AtomicBool::new(false));
        *login = Some(NativeLogin {
            job_id: job_id.clone(),
            cancel: Arc::clone(&cancel),
        });
        drop(login);
        self.accept(&id, json!({"job_id":job_id}))?;
        let sender = self.native_output.clone();
        let state = Arc::clone(&self.login);
        thread::spawn(move || {
            let result = spotify::login(&config, &spotify::token_path(), port, &cancel);
            let event = match result {
                Ok(name) => {
                    json!({"type":"event","event":"job.completed","data":{"job_id":job_id,"result":{"account_name":name}}})
                }
                Err(message) if message == "cancelled" => {
                    json!({"type":"event","event":"job.cancelled","data":{"job_id":job_id}})
                }
                Err(message) => {
                    json!({"type":"event","event":"job.failed","data":{"job_id":job_id,"error":{"code":"operation_failed","message":message}}})
                }
            };
            let _ = sender.send(event);
            if let Ok(mut active) = state.lock() {
                *active = None;
            }
        });
        Ok(id)
    }

    pub fn drain(&self) -> Vec<Value> {
        self.output.try_iter().collect()
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

type Outcome = Result<Value, (bool, String)>;

fn run_job(shared: &Arc<Shared>, job: Job) {
    let job_id = format!("queue-{}", job.id);
    let cancel = Arc::new(AtomicBool::new(false));
    shared.running().insert(job_id.clone(), Arc::clone(&cancel));
    shared.generation.fetch_add(1, Ordering::SeqCst);
    queues::set_label(&job.title);
    shared.send(json!({"type":"event","event":"job.started","data":{"job_id":job_id,"title":job.title,"kind":job.kind.as_ref()}}));
    shared.publish();
    let result = match job.kind {
        Kind::Refresh => run_refresh(shared, &job, &job_id, &cancel),
        Kind::Item => run_item(shared, &job, &job_id, &cancel),
        Kind::Workflow => run_workflow(shared, &job, &job_id, &cancel),
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
    shared.running().remove(&job_id);
    shared.generation.fetch_add(1, Ordering::SeqCst);
    shared.send(match result {
        Ok(result) => {
            json!({"type":"event","event":"job.completed","data":{"job_id":job_id,"result":result}})
        }
        Err((true, _)) => json!({"type":"event","event":"job.cancelled","data":{"job_id":job_id}}),
        Err((false, message)) => {
            json!({"type":"event","event":"job.failed","data":{"job_id":job_id,"error":{"code":"operation_failed","message":message}}})
        }
    });
    shared.publish();
}

fn run_refresh(shared: &Shared, job: &Job, job_id: &str, cancel: &AtomicBool) -> Outcome {
    let pending = native_watchlist::sync(&job.params, cancel, &mut |event| {
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
    shared.wake();
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
        parked.replace(Some(native_watchlist::Parked { kind, payload }));
        Err("waiting for a choice".to_owned())
    };
    native_watchlist::action(
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
    let mut decision_number = 0_usize;
    let mut workflow_event = |event: Value| shared.event(job_id, Source::Workflow, &event);
    let mut import_event = |event: Value| shared.event(job_id, Source::Native, &event);
    let mut decide = |kind: DecisionKind, mut payload: Value| {
        if let Some(value) = ask_agent(shared, job_id, kind, &mut payload) {
            return Ok(value);
        }
        decision_number += 1;
        let decision_id = format!("{job_id}-decision-{decision_number}");
        let (reply, receiver) = mpsc::channel();
        shared
            .decisions
            .lock()
            .map_err(|_| "Decision state is unavailable")?
            .insert(decision_id.clone(), reply);
        shared.send(json!({"type":"event","event":"decision.request","data":{"job_id":job_id,"decision_id":decision_id,"kind":kind,"payload":payload}}));
        let answer = queues::suspended(|| loop {
            if cancel.load(Ordering::SeqCst) {
                break Err("import cancelled".to_owned());
            }
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(value) => break Ok(value),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    break Err("The decision is not pending.".to_owned())
                }
            }
        });
        if let Ok(mut decisions) = shared.decisions.lock() {
            decisions.remove(&decision_id);
        }
        answer
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

fn job_error(error: JobError) -> (bool, String) {
    (matches!(error, JobError::Cancelled), error.to_string())
}

fn validate_item(params: &Value) -> Result<(), String> {
    params
        .get("playlist_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("playlist_id must be a non-empty string.")?;
    if params
        .get("position")
        .is_none_or(|value| value.as_i64().is_none() && value.as_u64().is_none())
    {
        return Err("position must be an integer.".into());
    }
    let action = params
        .get("action")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("action must be a non-empty string.")?;
    action
        .parse::<ItemAction>()
        .map_err(|_| format!("'{action}' is not a valid ItemAction"))?;
    Ok(())
}

pub(crate) fn item_key(params: &Value) -> String {
    format!(
        "{}:{}:{}",
        params["playlist_id"].as_str().unwrap_or(""),
        params["position"],
        params["video_id"].as_str().unwrap_or("")
    )
}

fn unique() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn open_jobs() -> Result<Store, String> {
    let store = if cfg!(test) {
        Store::open_in_memory()
    } else {
        Store::open(&paths::data_dir().join("jobs.db")).or_else(|_| Store::open_in_memory())
    }?;
    store.recover()?;
    Ok(store)
}

fn jobs_snapshot(store: &Store) -> Value {
    let open: Vec<Value> = store
        .list_open()
        .unwrap_or_default()
        .into_iter()
        .map(|job| {
            json!({"job_id":format!("queue-{}", job.id),"title":job.title,"kind":job.kind.as_ref(),"status":job.status.as_ref(),"item":(job.kind == Kind::Item).then_some(job.item_key)})
        })
        .collect();
    let waiting: Vec<Value> = store
        .list(Status::Waiting)
        .unwrap_or_default()
        .into_iter()
        .map(|job| {
            let question = job.question.unwrap_or(Value::Null);
            json!({"id":job.id,"title":job.title,"kind":question["kind"],"payload":question["payload"],"item":job.item_key})
        })
        .collect();
    json!({"open":open,"waiting":waiting})
}

fn park_item(store: &Store, params: &Value, data: &Value) -> Result<i64, String> {
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

struct WatchlistLoad {
    sender: Sender<Value>,
    id: String,
    params: Value,
    repository: Repository,
    login: Arc<Mutex<Option<NativeLogin>>>,
    running: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    latest: Arc<AtomicU64>,
    gate: Arc<Mutex<()>>,
    generation: u64,
}

impl WatchlistLoad {
    fn busy(&self) -> Result<bool, String> {
        Ok(self
            .login
            .lock()
            .map_err(|_| "Spotify login state is unavailable")?
            .is_some()
            || !self
                .running
                .lock()
                .map_err(|_| "Job state is unavailable")?
                .is_empty()
            || self.generation != self.latest.load(Ordering::SeqCst))
    }
}

fn load_watchlist(load: WatchlistLoad) {
    let options = match watchlist::Options::from_params(&load.params) {
        Ok(options) => options,
        Err(message) => {
            let _ = load.sender.send(json!({"id":load.id,"type":"response","ok":false,"error":{"code":"invalid_request","message":message}}));
            return;
        }
    };
    let saved = match options.saved(&load.repository) {
        Ok(saved) => saved,
        Err(message) => {
            let _ = load.sender.send(json!({"id":load.id,"type":"response","ok":false,"error":{"code":"operation_failed","message":message}}));
            return;
        }
    };
    if load
        .sender
        .send(json!({"id":load.id,"type":"response","ok":true,"result":{"watchlist":saved}}))
        .is_err()
    {
        return;
    }
    if let Err(message) = reconcile_watchlist(&load, &options) {
        if load.generation == load.latest.load(Ordering::SeqCst) {
            let _ = load
                .sender
                .send(json!({"type":"event","event":"watchlist.error","data":{"message":message}}));
        }
    }
}

fn reconcile_watchlist(load: &WatchlistLoad, options: &watchlist::Options) -> Result<(), String> {
    for _ in 0..3 {
        if load.busy()? {
            return Ok(());
        }
        let path = load.repository.path();
        let stamp = watchlist::stamp(path)?;
        let checked = options.checked(&load.repository)?;
        let saved = load.repository.locked(|| -> Result<bool, String> {
            if load.busy()? {
                return Ok(true);
            }
            if watchlist::stamp(path)? != stamp {
                return Ok(false);
            }
            load.repository.save(checked.clone())?;
            Ok(true)
        })?;
        if !saved {
            continue;
        }
        if load.busy()? {
            return Ok(());
        }
        let visible = options.view(checked)?;
        let _gate = load
            .gate
            .lock()
            .map_err(|_| "Watchlist state is unavailable")?;
        if load.generation == load.latest.load(Ordering::SeqCst) {
            let _ = load.sender.send(
                json!({"type":"event","event":"watchlist.updated","data":{"watchlist":visible}}),
            );
        }
        return Ok(());
    }
    Err("The watchlist changed during the local check. Reload it.".into())
}

fn native_response(id: &str, command: &str, params: &Value) -> Value {
    match native::dispatch(command, params) {
        Ok(result) => json!({"id": id, "type":"response", "ok":true, "result":result}),
        Err(message) => {
            let code = if matches!(command, "config.save" | "spotify.set_client_id") {
                "invalid_request"
            } else {
                "operation_failed"
            };
            json!({"id": id, "type":"response", "ok":false, "error":{"code":code, "message":message}})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{load_watchlist, Bridge, NativeLogin, WatchlistLoad};
    use muzik_core::watchlist::Repository;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn response(bridge: &Bridge, id: &str) -> Result<Value, Box<dyn std::error::Error>> {
        loop {
            let message = bridge.output.recv_timeout(Duration::from_secs(5))?;
            if message["type"] == "response" && message["id"] == id {
                return Ok(message);
            }
        }
    }

    fn event(
        bridge: &Bridge,
        wanted: &[&str],
        job_id: &str,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        loop {
            let message = bridge.output.recv_timeout(Duration::from_secs(20))?;
            let name = message["event"].as_str().unwrap_or("");
            if wanted.contains(&name) && message["data"]["job_id"] == job_id {
                return Ok(message);
            }
            if name == "job.failed" && message["data"]["job_id"] == job_id {
                return Err(format!("job failed: {}", message["data"]["error"]["message"]).into());
            }
        }
    }

    fn fixture_import(
        dir: &std::path::Path,
    ) -> Result<(PathBuf, PathBuf, PathBuf), Box<dyn std::error::Error>> {
        let audio = dir.join("track.flac");
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/muzik-tags/tests/fixtures/blank.flac"),
            &audio,
        )?;
        let database = dir.join("library.db");
        let config = dir.join("config.yaml");
        fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                dir.join("Music").display(),
                database.display(),
                dir.join("state.pickle").display()
            ),
        )?;
        Ok((audio, database, config))
    }

    #[test]
    fn library_scan_returns_file_size() -> TestResult {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("Track [dQw4w9WgXcQ].mp3"), b"audio")?;
        let mut bridge = Bridge::start()?;
        let id = bridge.send("library.scan", json!({"output": dir.path()}))?;
        assert_eq!(response(&bridge, &id)?["result"]["total_size"], "5.0 B");
        Ok(())
    }

    #[test]
    fn startup_answers_hello() -> TestResult {
        let mut bridge = Bridge::start()?;
        let id = bridge.send("hello", json!({}))?;
        assert_eq!(response(&bridge, &id)?["result"]["protocol_version"], 1);
        Ok(())
    }

    #[test]
    fn invalid_workflow_requests_use_the_native_protocol_response() -> TestResult {
        let mut bridge = Bridge::start()?;
        for (command, params) in [
            ("workflow.start", json!({"raw":"  "})),
            (
                "watchlist.action",
                json!({"playlist_id":"PL123","position":true,"action":"run"}),
            ),
            (
                "watchlist.action",
                json!({"playlist_id":"PL123","position":1,"action":"unknown"}),
            ),
            ("decision.reply", json!({"decision_id":""})),
            ("job.cancel", json!({"job_id":""})),
        ] {
            let id = bridge.send(command, params)?;
            assert_eq!(response(&bridge, &id)?["error"]["code"], "invalid_request");
        }
        Ok(())
    }

    #[test]
    fn local_workflow_runs_as_a_queue_job() -> TestResult {
        let dir = tempfile::tempdir()?;
        let audio = dir.path().join("track.flac");
        fs::write(&audio, b"audio")?;
        let mut bridge = Bridge::start()?;
        let id = bridge.send(
            "workflow.start",
            json!({
                "raw":audio,"no_organize":true,"no_split":true,"dry_run":true
            }),
        )?;
        let job_id = response(&bridge, &id)?["result"]["job_id"]
            .as_str()
            .ok_or("missing job ID")?
            .to_owned();
        assert!(job_id.starts_with("queue-"));
        event(&bridge, &["job.started"], &job_id)?;
        let done = event(&bridge, &["job.completed"], &job_id)?;
        assert_eq!(done["data"]["result"]["singles"], 1);
        assert!(audio.exists());
        Ok(())
    }

    #[test]
    fn two_workflow_runs_share_the_import_gate() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let (audio_one, database_one, config_one) = fixture_import(first.path())?;
        let (audio_two, database_two, config_two) = fixture_import(second.path())?;
        let mut bridge = Bridge::start()?;
        let mut jobs = Vec::new();
        for (audio, config) in [(&audio_one, &config_one), (&audio_two, &config_two)] {
            let id = bridge.send(
                "workflow.start",
                json!({"raw":audio,"config":config,"no_split":true,"interactive":true}),
            )?;
            jobs.push(
                response(&bridge, &id)?["result"]["job_id"]
                    .as_str()
                    .ok_or("missing job ID")?
                    .to_owned(),
            );
        }
        let mut asked = HashMap::new();
        let mut done = 0;
        while done < 2 {
            let message = bridge.output.recv_timeout(Duration::from_secs(20))?;
            match message["event"].as_str().unwrap_or("") {
                "decision.request" => {
                    let job = message["data"]["job_id"].as_str().unwrap_or("").to_owned();
                    assert_eq!(message["data"]["kind"], "import_match");
                    asked.insert(job, ());
                    let id = bridge.send(
                        "decision.reply",
                        json!({"decision_id":message["data"]["decision_id"],"value":"as_is"}),
                    )?;
                    let _ = id;
                }
                "job.completed" if jobs.iter().any(|job| message["data"]["job_id"] == *job) => {
                    done += 1;
                }
                "job.failed" => {
                    return Err(
                        format!("job failed: {}", message["data"]["error"]["message"]).into(),
                    )
                }
                _ => {}
            }
        }
        assert_eq!(asked.len(), 2);
        assert!(database_one.exists() && database_two.exists());
        assert!(!audio_one.exists() && !audio_two.exists());
        Ok(())
    }

    #[test]
    fn cancel_ends_a_pending_local_import_decision() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (audio, database, config) = fixture_import(dir.path())?;
        let mut bridge = Bridge::start()?;
        let id = bridge.send(
            "workflow.start",
            json!({"raw":audio,"config":config,"no_split":true}),
        )?;
        let job_id = response(&bridge, &id)?["result"]["job_id"]
            .as_str()
            .ok_or("missing job ID")?
            .to_owned();
        let asked = event(&bridge, &["decision.request"], &job_id)?;
        let decision_id = asked["data"]["decision_id"]
            .as_str()
            .ok_or("missing decision ID")?
            .to_owned();
        let id = bridge.send("job.cancel", json!({"job_id":job_id}))?;
        assert_eq!(response(&bridge, &id)?["result"]["cancel_requested"], true);
        event(&bridge, &["job.cancelled"], &job_id)?;
        assert!(audio.exists());
        assert!(!database.exists());
        let reply = bridge.send(
            "decision.reply",
            json!({"decision_id":decision_id,"value":"as_is"}),
        )?;
        assert_eq!(
            response(&bridge, &reply)?["error"]["code"],
            "invalid_request"
        );
        Ok(())
    }

    #[test]
    fn cancel_reaches_a_running_job() -> TestResult {
        let mut bridge = Bridge::start()?;
        let cancel = Arc::new(AtomicBool::new(false));
        bridge
            .shared
            .running()
            .insert("queue-test".into(), Arc::clone(&cancel));
        let id = bridge.send("job.cancel", json!({"job_id":"queue-test"}))?;
        assert_eq!(response(&bridge, &id)?["result"]["cancel_requested"], true);
        assert!(cancel.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn cancel_removes_a_queued_item_job() -> TestResult {
        let mut bridge = Bridge::start()?;
        bridge.shared.stop.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1200));
        let params = json!({"playlist_id":"PL1","position":2,"video_id":"abcdefghijk","action":"run","title":"Song"});
        let id = bridge.send("watchlist.action", params.clone())?;
        let job_id = response(&bridge, &id)?["result"]["job_id"]
            .as_str()
            .ok_or("missing job ID")?
            .to_owned();
        let again = bridge.send("watchlist.action", params)?;
        assert_eq!(response(&bridge, &again)?["error"]["code"], "job_active");
        let id = bridge.send("jobs.list", json!({}))?;
        let listed = response(&bridge, &id)?;
        assert_eq!(listed["result"]["open"][0]["job_id"], job_id);
        assert_eq!(listed["result"]["open"][0]["item"], "PL1:2:abcdefghijk");
        let id = bridge.send("job.cancel", json!({"job_id":job_id}))?;
        assert_eq!(response(&bridge, &id)?["result"]["cancel_requested"], true);
        let id = bridge.send("jobs.list", json!({}))?;
        assert_eq!(response(&bridge, &id)?["result"]["open"], json!([]));
        Ok(())
    }

    #[test]
    fn thumbnail_requests_get_a_native_protocol_response() -> TestResult {
        let mut bridge = Bridge::start()?;
        let id = bridge.send("thumbnails.cache", json!({"video_ids": []}))?;
        assert_eq!(response(&bridge, &id)?["result"]["queued"], 0);
        let id = bridge.send("thumbnails.cache", json!({"video_ids": [42]}))?;
        assert_eq!(response(&bridge, &id)?["error"]["code"], "invalid_request");
        Ok(())
    }

    #[test]
    fn spotify_login_validates_port_and_uses_its_own_slot() -> TestResult {
        let mut bridge = Bridge::start()?;
        let id = bridge.send("spotify.login", json!({"port": 0}))?;
        assert_eq!(response(&bridge, &id)?["error"]["code"], "invalid_request");
        let cancel = Arc::new(AtomicBool::new(false));
        *bridge.login.lock().map_err(|_| "login lock failed")? = Some(NativeLogin {
            job_id: "spotify-login-test".into(),
            cancel: Arc::clone(&cancel),
        });
        let id = bridge.send("job.cancel", json!({"job_id": "spotify-login-test"}))?;
        assert_eq!(response(&bridge, &id)?["result"]["cancel_requested"], true);
        assert!(cancel.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn watchlist_load_sends_saved_cards_before_local_check() -> TestResult {
        let dir = tempfile::tempdir()?;
        let repository = Repository::new(dir.path().join("watchlist.json"));
        repository
            .add("https://www.youtube.com/playlist?list=PLnative123")
            .map_err(std::io::Error::other)?;
        let (sender, receiver) = mpsc::channel();
        let request = WatchlistLoad {
            sender,
            id: "load-1".into(),
            params: json!({"output": dir.path().join("downloads"), "splits": dir.path().join("splits"), "quality_policy":"off", "no_split":false, "no_organize":false}),
            repository,
            login: Arc::new(Mutex::new(None)),
            running: Arc::new(Mutex::new(HashMap::new())),
            latest: Arc::new(AtomicU64::new(1)),
            gate: Arc::new(Mutex::new(())),
            generation: 1,
        };
        load_watchlist(request);
        let saved = receiver.recv()?;
        let checked = receiver.recv()?;
        assert_eq!(saved["id"], "load-1");
        assert_eq!(
            saved["result"]["watchlist"]["playlists"][0]["playlist_id"],
            "PLnative123"
        );
        assert_eq!(checked["event"], "watchlist.updated");
        assert_eq!(
            checked["data"]["watchlist"]["playlists"][0]["playlist_id"],
            "PLnative123"
        );
        Ok(())
    }

    #[test]
    fn unknown_command_has_protocol_error() -> TestResult {
        let mut bridge = Bridge::start()?;
        let id = bridge.send("unknown.command", json!({}))?;
        assert_eq!(response(&bridge, &id)?["error"]["code"], "invalid_request");
        Ok(())
    }

    #[test]
    fn a_parked_choice_waits_for_an_answer_and_then_queues() -> TestResult {
        let mut bridge = Bridge::start()?;
        bridge.shared.stop.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1200));
        let question = json!({"kind":"import_match","payload":{"matches":[]}});
        let job_id = super::park_item(
            &bridge.shared.store(),
            &json!({"output":"/music"}),
            &json!({"playlist_id":"PL1","position":3,"video_id":"abcdefghijk","title":"Album","stage":"organize","question":question}),
        )?;
        let id = bridge.send("jobs.list", json!({}))?;
        assert_eq!(
            response(&bridge, &id)?["result"]["waiting"],
            json!([{"id":job_id,"title":"Album","kind":"import_match","payload":{"matches":[]},"item":"PL1:3:abcdefghijk"}])
        );
        let id = bridge.send("jobs.answer", json!({"id":job_id,"value":"release:1"}))?;
        assert_eq!(response(&bridge, &id)?["result"]["answered"], true);
        let job = bridge
            .shared
            .store()
            .claim(muzik_jobs::Queue::Item)?
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
}
