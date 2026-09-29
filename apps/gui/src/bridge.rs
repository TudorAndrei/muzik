//! JSON protocol for the Rust desktop application.
use crate::{local_workflow, native, native_watchlist, remote_workflow, thumbnails, watchlist};
use muzik_core::{app_config, spotify, watchlist::Repository};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct Bridge {
    output: Receiver<Value>,
    native_output: Sender<Value>,
    thumbnail_pending: Arc<Mutex<HashSet<String>>>,
    login: Arc<Mutex<Option<NativeLogin>>>,
    local_job: Arc<Mutex<Option<NativeLogin>>>,
    local_decision: Arc<Mutex<Option<NativeDecision>>>,
    watchlist_generation: Arc<AtomicU64>,
    watchlist_gate: Arc<Mutex<()>>,
    next_id: u64,
}

struct NativeLogin {
    job_id: String,
    cancel: Arc<AtomicBool>,
}

struct NativeDecision {
    id: String,
    reply: Sender<Value>,
}

enum NativeWorkflowRequest {
    Local(local_workflow::LocalRequest),
    Remote(remote_workflow::RemoteRequest),
    WatchlistRefresh(Value),
    WatchlistAction(Value),
}

impl Bridge {
    pub fn start() -> Result<Self, String> {
        let (events, output) = mpsc::channel::<Value>();
        Ok(Self {
            output,
            native_output: events,
            thumbnail_pending: Arc::new(Mutex::new(HashSet::new())),
            login: Arc::new(Mutex::new(None)),
            local_job: Arc::new(Mutex::new(None)),
            local_decision: Arc::new(Mutex::new(None)),
            watchlist_generation: Arc::new(AtomicU64::new(0)),
            watchlist_gate: Arc::new(Mutex::new(())),
            next_id: 1,
        })
    }

    pub fn send(&mut self, command: &str, params: Value) -> Result<String, String> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        if command == "spotify.login" {
            return self.start_spotify_login(id, params);
        }
        if command == "decision.reply" {
            let decision_id = params
                .get("decision_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let mut pending = self
                .local_decision
                .lock()
                .map_err(|_| "Local decision state is unavailable")?;
            if decision_id.is_empty() {
                self.native_output.send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":"decision_id must be a non-empty string."}}))
                    .map_err(|_| "Rust backend is not available".to_owned())?;
                return Ok(id);
            }
            if pending
                .as_ref()
                .is_some_and(|decision| decision.id == decision_id)
            {
                let decision = pending
                    .take()
                    .ok_or("Local decision state is unavailable")?;
                let sent = decision
                    .reply
                    .send(params.get("value").cloned().unwrap_or(Value::Null))
                    .is_ok();
                let response = if sent {
                    json!({"id":id,"type":"response","ok":true,"result":{"decision_id":decision_id}})
                } else {
                    json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":"The decision is not pending."}})
                };
                self.native_output
                    .send(response)
                    .map_err(|_| "Rust backend is not available".to_owned())?;
                return Ok(id);
            }
            self.native_output.send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":"The decision is not pending."}}))
                .map_err(|_| "Rust backend is not available".to_owned())?;
            return Ok(id);
        }
        if command == "job.cancel" {
            let job_id = params.get("job_id").and_then(Value::as_str).unwrap_or("");
            let local = self
                .local_job
                .lock()
                .map_err(|_| "Local job state is unavailable")?;
            if let Some(active) = local.as_ref().filter(|active| active.job_id == job_id) {
                active.cancel.store(true, Ordering::SeqCst);
                self.native_output
                    .send(json!({"id":id,"type":"response","ok":true,"result":{"job_id":job_id,"cancel_requested":true}}))
                    .map_err(|_| "Rust backend is not available".to_owned())?;
                return Ok(id);
            }
            drop(local);
            let login = self
                .login
                .lock()
                .map_err(|_| "Spotify login is not available")?;
            if let Some(active) = login.as_ref().filter(|active| active.job_id == job_id) {
                active.cancel.store(true, Ordering::Relaxed);
                self.native_output
                    .send(json!({"id":id,"type":"response","ok":true,"result":{"job_id":job_id,"cancel_requested":true}}))
                    .map_err(|_| "Rust backend is not available".to_owned())?;
                return Ok(id);
            }
            self.native_output
                .send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":"The job is not active."}}))
                .map_err(|_| "Rust backend is not available".to_owned())?;
            return Ok(id);
        }
        if command == "thumbnails.cache" {
            let ids = match thumbnails::validate_ids(&params) {
                Ok(ids) => ids,
                Err(message) => {
                    self.native_output
                        .send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":message}}))
                        .map_err(|_| "Rust backend is not available".to_owned())?;
                    return Ok(id);
                }
            };
            let mut pending = self
                .thumbnail_pending
                .lock()
                .map_err(|_| "thumbnail queue is not available")?;
            let fresh = ids
                .into_iter()
                .filter(|id| pending.insert(id.clone()))
                .collect::<Vec<_>>();
            let response = json!({"id": id, "type": "response", "ok": true, "result": {"queued": fresh.len()}});
            self.native_output
                .send(response)
                .map_err(|_| "Rust backend is not available".to_owned())?;
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
            let generation = self.watchlist_generation.fetch_add(1, Ordering::SeqCst) + 1;
            let response_id = id.clone();
            let sender = self.native_output.clone();
            let login = Arc::clone(&self.login);
            let local = Arc::clone(&self.local_job);
            let latest = Arc::clone(&self.watchlist_generation);
            let gate = Arc::clone(&self.watchlist_gate);
            let repository = Repository::new(Repository::default_path());
            thread::spawn(move || {
                load_watchlist(WatchlistLoad {
                    sender,
                    id: response_id,
                    params,
                    repository,
                    login,
                    local,
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
            let login = self
                .login
                .lock()
                .map_err(|_| "Spotify login state is unavailable")?;
            let local_active = self
                .local_job
                .lock()
                .map_err(|_| "Local job state is unavailable")?
                .is_some();
            let response = if login.is_some() || local_active {
                json!({"id":id,"type":"response","ok":false,"error":{"code":"job_active","message":"A job is already active."}})
            } else {
                native_response(&id, command, &params)
            };
            if response["ok"] == true {
                self.watchlist_generation.fetch_add(1, Ordering::SeqCst);
            }
            self.native_output
                .send(response)
                .map_err(|_| "Rust backend is not available".to_owned())?;
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
                let response = native_response(&id, command, &params);
                self.native_output
                    .send(response)
                    .map_err(|_| "Rust backend is not available".to_string())?;
            }
            return Ok(id);
        }
        if command == "workflow.start" {
            if let Some(local) = local_workflow::supported(&params) {
                return self.start_native_workflow(id, local.map(NativeWorkflowRequest::Local));
            }
            if let Some(remote) = remote_workflow::supported(&params) {
                return self.start_native_workflow(id, remote.map(NativeWorkflowRequest::Remote));
            }
        }
        if let Err(message) = validate_workflow_command(command, &params) {
            self.native_output
                .send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":message}}))
                .map_err(|_| "Rust backend is not available".to_owned())?;
            return Ok(id);
        }
        if matches!(command, "watchlist.refresh" | "watchlist.action")
            && (command != "watchlist.action"
                || matches!(
                    params["action"].as_str(),
                    Some(
                        "run"
                            | "retry"
                            | "download_again"
                            | "run_all_again"
                            | "check_quality_again"
                            | "parse_again"
                            | "split_again"
                            | "organize_again"
                    )
                ))
        {
            self.watchlist_generation.fetch_add(1, Ordering::SeqCst);
            let request = if command == "watchlist.refresh" {
                NativeWorkflowRequest::WatchlistRefresh(params)
            } else {
                NativeWorkflowRequest::WatchlistAction(params)
            };
            return self.start_native_workflow(id, Ok(request));
        }
        self.native_output
            .send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":format!("Unknown command: {command}")}}))
            .map_err(|_| "Rust backend is not available".to_owned())?;
        Ok(id)
    }

    fn start_native_workflow(
        &mut self,
        id: String,
        request: Result<NativeWorkflowRequest, String>,
    ) -> Result<String, String> {
        let request = match request {
            Ok(request) => request,
            Err(message) => {
                self.native_output.send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":message}}))
                    .map_err(|_| "Rust backend is not available".to_owned())?;
                return Ok(id);
            }
        };
        let login = self
            .login
            .lock()
            .map_err(|_| "Spotify login state is unavailable")?;
        let mut state = self
            .local_job
            .lock()
            .map_err(|_| "Local job state is unavailable")?;
        if state.is_some() || login.is_some() {
            self.native_output.send(json!({"id":id,"type":"response","ok":false,"error":{"code":"job_active","message":"A job is already active."}}))
                .map_err(|_| "Rust backend is not available".to_owned())?;
            return Ok(id);
        }
        let job_id = format!("local-workflow-{id}");
        let cancel = Arc::new(AtomicBool::new(false));
        *state = Some(NativeLogin {
            job_id: job_id.clone(),
            cancel: Arc::clone(&cancel),
        });
        self.native_output
            .send(json!({"id":id,"type":"response","ok":true,"result":{"job_id":job_id}}))
            .map_err(|_| "Rust backend is not available".to_owned())?;
        drop(login);
        drop(state);
        let sender = self.native_output.clone();
        let active = Arc::clone(&self.local_job);
        let pending = Arc::clone(&self.local_decision);
        thread::spawn(move || {
            let mut decision_number = 0_usize;
            let mut workflow_event = |event: Value| {
                let _ = sender.send(json!({"type":"event","event":"job.event","data":{"job_id":job_id,"source":"workflow","event":event["event"],"data":event["data"]}}));
            };
            let mut import_event = |event: Value| {
                let _ = sender.send(json!({"type":"event","event":"job.event","data":{"job_id":job_id,"source":"native","event":event["event"],"data":event["data"]}}));
            };
            let mut decide = |kind: &str, mut payload: Value| {
                let job_message = |event: &str, data: Value| {
                    let _ = sender.send(json!({"type":"event","event":"job.event","data":{"job_id":job_id,"source":"agent","event":event,"data":data}}));
                };
                if let Some(model) = agent_model(kind, &payload) {
                    if muzik_agent::strong_match(kind, &payload).is_none() {
                        job_message(
                            "message",
                            json!({"message":format!("Asking {model} to choose.")}),
                        );
                    }
                    match muzik_agent::decide(kind, &payload, &model) {
                        Ok(muzik_agent::Outcome::Decided(choice)) => {
                            job_message(
                                "agent_decided",
                                json!({"kind":kind,"label":choice.label,"confidence":choice.confidence,"reason":choice.reason}),
                            );
                            return Ok(choice.value);
                        }
                        Ok(muzik_agent::Outcome::Unsure {
                            suggestion,
                            confidence,
                            reason,
                        }) => {
                            payload["agent"] = json!({"model":model,"suggestion":suggestion,"confidence":confidence,"reason":reason});
                        }
                        Err(error) => {
                            payload["agent"] = json!({"model":model,"error":error});
                        }
                    }
                }
                decision_number += 1;
                let decision_id = format!("{job_id}-decision-{decision_number}");
                let (reply, receiver) = mpsc::channel();
                {
                    let mut state = pending
                        .lock()
                        .map_err(|_| "Local decision state is unavailable")?;
                    *state = Some(NativeDecision {
                        id: decision_id.clone(),
                        reply,
                    });
                }
                sender.send(json!({"type":"event","event":"decision.request","data":{"job_id":job_id,"decision_id":decision_id,"kind":kind,"payload":payload}}))
                    .map_err(|_| "Rust backend is not available")?;
                let answer = loop {
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
                };
                if let Ok(mut state) = pending.lock() {
                    if state
                        .as_ref()
                        .is_some_and(|decision| decision.id == decision_id)
                    {
                        *state = None;
                    }
                }
                answer
            };
            let result: Result<Value, (bool, String)> = match request {
                NativeWorkflowRequest::Local(local) => local_workflow::run(
                    local,
                    &cancel,
                    &mut workflow_event,
                    &mut import_event,
                    &mut decide,
                )
                .map_err(|error| {
                    (
                        matches!(error, muzik_workflow::Error::Cancelled),
                        error.to_string(),
                    )
                }),
                NativeWorkflowRequest::Remote(remote) => remote_workflow::run(
                    remote,
                    &cancel,
                    &mut workflow_event,
                    &mut import_event,
                    &mut decide,
                )
                .map_err(|error| {
                    (
                        matches!(error, muzik_workflow::Error::Cancelled),
                        error.to_string(),
                    )
                }),
                NativeWorkflowRequest::WatchlistRefresh(params) => native_watchlist::refresh(
                    &params,
                    &cancel,
                    &mut workflow_event,
                    &mut import_event,
                    &mut decide,
                )
                .map_err(|error| {
                    (
                        matches!(error, muzik_core::watchlist::jobs::JobError::Cancelled),
                        error.to_string(),
                    )
                }),
                NativeWorkflowRequest::WatchlistAction(params) => native_watchlist::action(
                    &params,
                    &cancel,
                    &mut workflow_event,
                    &mut import_event,
                    &mut decide,
                )
                .map_err(|error| {
                    (
                        matches!(error, muzik_core::watchlist::jobs::JobError::Cancelled),
                        error.to_string(),
                    )
                }),
            };
            if let Ok(mut state) = active.lock() {
                *state = None;
            }
            let terminal = match result {
                Ok(result) => {
                    json!({"type":"event","event":"job.completed","data":{"job_id":job_id,"result":result}})
                }
                Err((true, _)) => {
                    json!({"type":"event","event":"job.cancelled","data":{"job_id":job_id}})
                }
                Err((false, message)) => {
                    json!({"type":"event","event":"job.failed","data":{"job_id":job_id,"error":{"code":"operation_failed","message":message}}})
                }
            };
            let _ = sender.send(terminal);
        });
        Ok(id)
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
                    self.native_output.send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":"port must be an integer from 1 to 65535."}}))
                        .map_err(|_| "Rust backend is not available".to_owned())?;
                    return Ok(id);
                }
            },
        };
        let mut login = self
            .login
            .lock()
            .map_err(|_| "Spotify login is not available")?;
        if login.is_some()
            || self
                .local_job
                .lock()
                .map_err(|_| "Local job state is unavailable")?
                .is_some()
        {
            self.native_output.send(json!({"id":id,"type":"response","ok":false,"error":{"code":"job_active","message":"A job is already active."}}))
                .map_err(|_| "Rust backend is not available".to_owned())?;
            return Ok(id);
        }
        let config = app_config::path();
        if let Some(port) = port {
            if let Err(message) = app_config::save_section_string(
                &config,
                "spotify",
                "redirect_port",
                &port.to_string(),
            ) {
                self.native_output.send(json!({"id":id,"type":"response","ok":false,"error":{"code":"operation_failed","message":message}}))
                    .map_err(|_| "Rust backend is not available".to_owned())?;
                return Ok(id);
            }
        }
        let job_id = format!("spotify-login-{id}");
        let cancel = Arc::new(AtomicBool::new(false));
        *login = Some(NativeLogin {
            job_id: job_id.clone(),
            cancel: Arc::clone(&cancel),
        });
        self.native_output
            .send(json!({"id":id,"type":"response","ok":true,"result":{"job_id":job_id}}))
            .map_err(|_| "Rust backend is not available".to_owned())?;
        drop(login);
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

fn validate_workflow_command(command: &str, params: &Value) -> Result<(), String> {
    let required = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("{key} must be a non-empty string."))
    };
    match command {
        "workflow.start" => {
            if params
                .get("raw")
                .and_then(Value::as_str)
                .is_none_or(|raw| raw.trim().is_empty())
            {
                return Err("Enter a URL or path.".into());
            }
        }
        "watchlist.action" => {
            required("playlist_id")?;
            if params
                .get("position")
                .is_none_or(|value| value.as_i64().is_none() && value.as_u64().is_none())
            {
                return Err("position must be an integer.".into());
            }
            let action = required("action")?;
            if !matches!(
                action,
                "run"
                    | "retry"
                    | "download_again"
                    | "check_quality_again"
                    | "parse_again"
                    | "split_again"
                    | "organize_again"
                    | "run_all_again"
            ) {
                return Err(format!("'{action}' is not a valid ItemAction"));
            }
        }
        "job.cancel" => {
            required("job_id")?;
        }
        "decision.reply" => {
            required("decision_id")?;
        }
        _ => {}
    }
    Ok(())
}

fn agent_model(kind: &str, payload: &Value) -> Option<String> {
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
    local: Arc<Mutex<Option<NativeLogin>>>,
    latest: Arc<AtomicU64>,
    gate: Arc<Mutex<()>>,
    generation: u64,
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
        if load
            .login
            .lock()
            .map_err(|_| "Spotify login state is unavailable")?
            .is_some()
            || load
                .local
                .lock()
                .map_err(|_| "Local job state is unavailable")?
                .is_some()
            || load.generation != load.latest.load(Ordering::SeqCst)
        {
            return Ok(());
        }
        let path = load.repository.path();
        let stamp = watchlist::stamp(path)?;
        let checked = options.checked(&load.repository)?;
        let login = load
            .login
            .lock()
            .map_err(|_| "Spotify login state is unavailable")?;
        let local = load
            .local
            .lock()
            .map_err(|_| "Local job state is unavailable")?;
        if login.is_some()
            || local.is_some()
            || load.generation != load.latest.load(Ordering::SeqCst)
        {
            return Ok(());
        }
        if watchlist::stamp(path)? != stamp {
            continue;
        }
        load.repository.save(checked.clone())?;
        drop(local);
        drop(login);
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
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn library_scan_returns_file_size() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("Track [dQw4w9WgXcQ].mp3"), b"audio")?;
        let mut bridge = Bridge::start()?;
        let id = bridge.send("library.scan", json!({"output": dir.path()}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["total_size"], "5.0 B");
        Ok(())
    }

    #[test]
    fn startup_answers_hello() -> Result<(), Box<dyn std::error::Error>> {
        let mut bridge = Bridge::start()?;
        let id = bridge.send("hello", json!({}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["protocol_version"], 1);
        Ok(())
    }

    #[test]
    fn invalid_workflow_requests_use_the_native_protocol_response(
    ) -> Result<(), Box<dyn std::error::Error>> {
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
            let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
            assert_eq!(response["id"], id);
            assert_eq!(response["error"]["code"], "invalid_request");
        }
        Ok(())
    }

    #[test]
    fn local_workflow_uses_native_job_events() -> Result<(), Box<dyn std::error::Error>> {
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
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["ok"], true);
        let job_id = response["result"]["job_id"]
            .as_str()
            .ok_or("missing job ID")?;
        let mut saw_event = false;
        loop {
            let message = bridge.output.recv_timeout(Duration::from_secs(2))?;
            if message["event"] == "job.event" {
                saw_event = true;
            }
            if message["event"] == "job.completed" {
                assert_eq!(message["data"]["job_id"], job_id);
                assert_eq!(message["data"]["result"]["singles"], 1);
                break;
            }
        }
        assert!(saw_event);
        assert!(audio.exists());
        Ok(())
    }

    #[test]
    fn local_import_uses_native_decision_and_beets_library(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let audio = dir.path().join("track.flac");
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/muzik-tags/tests/fixtures/blank.flac"),
            &audio,
        )?;
        let database = dir.path().join("library.db");
        let root = dir.path().join("Music");
        let config = dir.path().join("config.yaml");
        fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                root.display(),
                database.display(),
                dir.path().join("state.pickle").display()
            ),
        )?;
        let mut bridge = Bridge::start()?;
        let id = bridge.send(
            "workflow.start",
            json!({
                "raw":audio,"config":config,"no_split":true,"interactive":true
            }),
        )?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["ok"], true);
        let job_id = response["result"]["job_id"]
            .as_str()
            .ok_or("missing job ID")?;
        let decision_id = loop {
            let message = bridge.output.recv_timeout(Duration::from_secs(10))?;
            if message["event"] == "decision.request" {
                assert_eq!(message["data"]["job_id"], job_id);
                assert_eq!(message["data"]["kind"], "import_match");
                break message["data"]["decision_id"]
                    .as_str()
                    .ok_or("missing decision ID")?
                    .to_owned();
            }
            if message["event"] == "job.failed" {
                return Err(
                    format!("import failed: {}", message["data"]["error"]["message"]).into(),
                );
            }
        };
        let reply = bridge.send(
            "decision.reply",
            json!({"decision_id":decision_id,"value":"as_is"}),
        )?;
        let mut finished = false;
        for _ in 0..20 {
            let message = bridge.output.recv_timeout(Duration::from_secs(10))?;
            if message["type"] == "response" && message["id"] == reply {
                assert_eq!(message["ok"], true);
            }
            if message["event"] == "job.failed" {
                return Err(
                    format!("import failed: {}", message["data"]["error"]["message"]).into(),
                );
            }
            if message["event"] == "job.completed" {
                finished = true;
                break;
            }
        }
        assert!(finished);
        assert!(database.exists());
        assert!(!audio.exists());
        Ok(())
    }

    #[test]
    fn cancel_ends_a_pending_local_import_decision() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let audio = dir.path().join("track.flac");
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/muzik-tags/tests/fixtures/blank.flac"),
            &audio,
        )?;
        let database = dir.path().join("library.db");
        let config = dir.path().join("config.yaml");
        fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                dir.path().join("Music").display(),
                database.display(),
                dir.path().join("state.pickle").display()
            ),
        )?;
        let mut bridge = Bridge::start()?;
        bridge.send(
            "workflow.start",
            json!({"raw":audio,"config":config,"no_split":true}),
        )?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        let job_id = response["result"]["job_id"]
            .as_str()
            .ok_or("missing job ID")?
            .to_owned();
        let decision_id = loop {
            let message = bridge.output.recv_timeout(Duration::from_secs(10))?;
            if message["event"] == "decision.request" {
                break message["data"]["decision_id"]
                    .as_str()
                    .ok_or("missing decision ID")?
                    .to_owned();
            }
            if message["event"] == "job.failed" {
                return Err(
                    format!("import failed: {}", message["data"]["error"]["message"]).into(),
                );
            }
        };
        bridge.send("job.cancel", json!({"job_id":job_id}))?;
        let mut cancelled = false;
        for _ in 0..5 {
            let message = bridge.output.recv_timeout(Duration::from_secs(2))?;
            if message["event"] == "job.cancelled" {
                cancelled = true;
                break;
            }
        }
        assert!(cancelled);
        assert!(audio.exists());
        assert!(!database.exists());
        let reply = bridge.send(
            "decision.reply",
            json!({"decision_id":decision_id,"value":"as_is"}),
        )?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], reply);
        assert_eq!(response["error"]["code"], "invalid_request");
        Ok(())
    }

    #[test]
    fn cancel_reaches_an_active_local_job() -> Result<(), Box<dyn std::error::Error>> {
        let mut bridge = Bridge::start()?;
        let cancel = Arc::new(AtomicBool::new(false));
        *bridge.local_job.lock().map_err(|_| "local lock failed")? = Some(NativeLogin {
            job_id: "local-workflow-test".into(),
            cancel: Arc::clone(&cancel),
        });
        let id = bridge.send("job.cancel", json!({"job_id":"local-workflow-test"}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["cancel_requested"], true);
        assert!(cancel.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn thumbnail_requests_get_a_native_protocol_response() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut bridge = Bridge::start()?;
        let id = bridge.send("thumbnails.cache", json!({"video_ids": []}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["queued"], 0);

        let id = bridge.send("thumbnails.cache", json!({"video_ids": [42]}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], "invalid_request");
        Ok(())
    }

    #[test]
    fn spotify_login_validates_port_and_uses_the_native_job_slot(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut bridge = Bridge::start()?;
        let id = bridge.send("spotify.login", json!({"port": 0}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], "invalid_request");

        let cancel = Arc::new(AtomicBool::new(false));
        *bridge.login.lock().map_err(|_| "login lock failed")? = Some(NativeLogin {
            job_id: "spotify-login-test".into(),
            cancel: Arc::clone(&cancel),
        });
        let id = bridge.send("job.cancel", json!({"job_id": "spotify-login-test"}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["cancel_requested"], true);
        assert!(cancel.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn watchlist_load_sends_saved_cards_before_local_check(
    ) -> Result<(), Box<dyn std::error::Error>> {
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
            local: Arc::new(Mutex::new(None)),
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
    fn watchlist_edit_waits_for_an_active_workflow() -> Result<(), Box<dyn std::error::Error>> {
        let mut bridge = Bridge::start()?;
        *bridge.local_job.lock().map_err(|_| "job lock failed")? = Some(NativeLogin {
            job_id: "local-workflow-running".into(),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        let id = bridge.send("watchlist.add", json!({"url":"liked"}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], "job_active");
        Ok(())
    }

    #[test]
    fn unknown_command_has_protocol_error() -> Result<(), Box<dyn std::error::Error>> {
        let mut bridge = Bridge::start()?;
        let id = bridge.send("unknown.command", json!({}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], "invalid_request");
        Ok(())
    }
}
