//! JSON protocol for the Rust desktop application.
use crate::{native, thumbnails, watchlist};
use muzik_core::{app_config, spotify, watchlist::Repository};
use muzik_jobs::CancelRequest;
use muzik_runner::{gates, parse_job_id, EnqueueError, Jobs, Options, Prompt, Runner};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const WORKERS: usize = 5;

type Decisions = Arc<Mutex<HashMap<String, Sender<Value>>>>;

pub struct Bridge {
    output: Receiver<Value>,
    native_output: Sender<Value>,
    thumbnail_pending: Arc<Mutex<HashSet<String>>>,
    login: Arc<Mutex<Option<NativeLogin>>>,
    jobs: Arc<Jobs>,
    runner: Option<Runner>,
    decisions: Decisions,
    generation: Arc<AtomicU64>,
    watchlist_gate: Arc<Mutex<()>>,
    next_id: u64,
}

struct NativeLogin {
    job_id: String,
    cancel: Arc<AtomicBool>,
}

impl Bridge {
    pub fn start() -> Result<Self, String> {
        Self::with_runner(true)
    }

    fn with_runner(run: bool) -> Result<Self, String> {
        let (events, output) = mpsc::channel::<Value>();
        let jobs = Arc::new(if cfg!(test) {
            Jobs::in_memory()?
        } else {
            Jobs::open()?
        });
        let decisions: Decisions = Arc::new(Mutex::new(HashMap::new()));
        let generation = Arc::new(AtomicU64::new(0));
        let runner = if run {
            let sink = Mutex::new(events.clone());
            let asker = Mutex::new(events.clone());
            let pending = Arc::clone(&decisions);
            let asked = AtomicU64::new(0);
            Runner::start(
                Arc::clone(&jobs),
                Options {
                    workers: WORKERS,
                    sink: Arc::new(move |message| {
                        if let Ok(sender) = sink.lock() {
                            let _ = sender.send(message);
                        }
                    }),
                    ask: Arc::new(move |prompt| {
                        let number = asked.fetch_add(1, Ordering::SeqCst) + 1;
                        ask(&asker, &pending, &prompt, number)
                    }),
                    generation: Arc::clone(&generation),
                },
            )?
        } else {
            None
        };
        if run && runner.is_none() {
            let _ = events.send(json!({"type":"event","event":"jobs.remote","data":{"message":"Another muzik process runs the queue. New jobs go into its queue."}}));
        }
        Ok(Self {
            output,
            native_output: events,
            thumbnail_pending: Arc::new(Mutex::new(HashSet::new())),
            login: Arc::new(Mutex::new(None)),
            jobs,
            runner,
            decisions,
            generation,
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

    fn changed(&self) {
        match &self.runner {
            Some(runner) => {
                runner.publish();
                runner.wake();
            }
            None => {
                let _ = self.native_output.send(
                    json!({"type":"event","event":"jobs.updated","data":self.jobs.snapshot()}),
                );
            }
        }
    }

    pub fn send(&mut self, command: &str, params: Value) -> Result<String, String> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        match command {
            "spotify.login" => return self.start_spotify_login(id, params),
            "decision.reply" => return self.reply(&id, &params),
            "job.cancel" => return self.cancel(&id, &params),
            "jobs.list" => {
                let mut snapshot = self.jobs.snapshot();
                snapshot["gates"] = gates::snapshot();
                snapshot["runner"] = json!(self.runner.is_some());
                return self.accept(&id, snapshot);
            }
            "jobs.answer" => return self.answer(&id, &params),
            "workflow.start" => {
                let queued = self.jobs.workflow(&params);
                return self.queued(&id, queued);
            }
            "watchlist.refresh" => {
                let queued = self.jobs.refresh(&params);
                return self.queued(&id, queued);
            }
            "watchlist.action" => {
                let queued = self.jobs.item(&params);
                return self.queued(&id, queued);
            }
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
                    let data = thumbnails::cache_requested(
                        &fresh,
                        &Repository::default(),
                        &muzik_core::paths::cache_dir(),
                    );
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
            let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            let response_id = id.clone();
            let sender = self.native_output.clone();
            let login = Arc::clone(&self.login);
            let jobs = Arc::clone(&self.jobs);
            let latest = Arc::clone(&self.generation);
            let gate = Arc::clone(&self.watchlist_gate);
            let repository = Repository::default();
            thread::spawn(move || {
                load_watchlist(WatchlistLoad {
                    sender,
                    id: response_id,
                    params,
                    repository,
                    login,
                    jobs,
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
                self.generation.fetch_add(1, Ordering::SeqCst);
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

    fn queued(&self, id: &str, queued: Result<i64, EnqueueError>) -> Result<String, String> {
        match queued {
            Ok(number) => {
                self.accept(id, json!({"job_id":muzik_runner::job_id(number)}))?;
                self.changed();
                Ok(id.to_owned())
            }
            Err(EnqueueError::Invalid(message)) => self.reject(id, "invalid_request", message),
            Err(EnqueueError::Busy(message)) => self.reject(id, "job_active", message),
            Err(EnqueueError::Store(message)) => self.reject(id, "operation_failed", message),
        }
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
        if self
            .runner
            .as_ref()
            .is_some_and(|runner| runner.cancel(job_id))
        {
            return self.accept(id, json!({"job_id":job_id,"cancel_requested":true}));
        }
        if let Some(number) = job_id
            .starts_with("queue-")
            .then(|| parse_job_id(job_id))
            .flatten()
        {
            match self.jobs.cancel(number)? {
                CancelRequest::Removed => {
                    self.accept(id, json!({"job_id":job_id,"cancel_requested":true}))?;
                    self.respond(
                        json!({"type":"event","event":"job.cancelled","data":{"job_id":job_id}}),
                    )?;
                    self.changed();
                    return Ok(id.to_owned());
                }
                CancelRequest::Requested => {
                    return self.accept(id, json!({"job_id":job_id,"cancel_requested":true}));
                }
                CancelRequest::NotOpen => {}
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
        let answered = self.jobs.answer(job_id, value)?;
        self.accept(id, json!({"answered":answered}))?;
        self.changed();
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

fn ask(
    sender: &Mutex<Sender<Value>>,
    decisions: &Decisions,
    prompt: &Prompt<'_>,
    number: u64,
) -> Result<Value, String> {
    let decision_id = format!("{}-decision-{number}", prompt.job_id);
    let (reply, receiver) = mpsc::channel();
    decisions
        .lock()
        .map_err(|_| "Decision state is unavailable")?
        .insert(decision_id.clone(), reply);
    sender
        .lock()
        .map_err(|_| "Rust backend is not available")?
        .send(json!({"type":"event","event":"decision.request","data":{"job_id":prompt.job_id,"decision_id":decision_id,"kind":prompt.kind,"payload":prompt.payload}}))
        .map_err(|_| "Rust backend is not available")?;
    let answer = loop {
        if prompt.cancelled.load(Ordering::SeqCst) {
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
    if let Ok(mut decisions) = decisions.lock() {
        decisions.remove(&decision_id);
    }
    answer
}

struct WatchlistLoad {
    sender: Sender<Value>,
    id: String,
    params: Value,
    repository: Repository,
    login: Arc<Mutex<Option<NativeLogin>>>,
    jobs: Arc<Jobs>,
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
            || self.jobs.has_running()
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
    if let Some(login) = muzik_core::bandcamp::Login::load() {
        let source = muzik_core::watchlist::bandcamp_source(&login.user);
        if let Err(message) = load.repository.ensure(&source) {
            let _ = load
                .sender
                .send(json!({"type":"event","event":"watchlist.error","data":{"message":message}}));
        }
    }
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
        let revision = load.repository.revision()?;
        let checked = options.checked(&load.repository)?;
        let saved = load.repository.locked(|| -> Result<bool, String> {
            if load.busy()? {
                return Ok(true);
            }
            if load.repository.revision()? != revision {
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
    use muzik_runner::Jobs;
    use serde_json::{json, Value};
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    thread_local! {
        static SKIPPED: RefCell<VecDeque<Value>> = const { RefCell::new(VecDeque::new()) };
    }

    fn next_matching(
        bridge: &Bridge,
        timeout: Duration,
        wanted: impl Fn(&Value) -> bool,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let earlier = SKIPPED.with(|skipped| {
            let mut skipped = skipped.borrow_mut();
            let index = skipped.iter().position(&wanted)?;
            skipped.remove(index)
        });
        if let Some(message) = earlier {
            return Ok(message);
        }
        loop {
            let message = bridge.output.recv_timeout(timeout)?;
            if wanted(&message) {
                return Ok(message);
            }
            SKIPPED.with(|skipped| skipped.borrow_mut().push_back(message));
        }
    }

    fn response(bridge: &Bridge, id: &str) -> Result<Value, Box<dyn std::error::Error>> {
        next_matching(bridge, Duration::from_secs(5), |message| {
            message["type"] == "response" && message["id"] == id
        })
    }

    fn event(
        bridge: &Bridge,
        wanted: &[&str],
        job_id: &str,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let message = next_matching(bridge, Duration::from_secs(20), |message| {
            let name = message["event"].as_str().unwrap_or("");
            (wanted.contains(&name) || name == "job.failed") && message["data"]["job_id"] == job_id
        })?;
        if message["event"] == "job.failed" && !wanted.contains(&"job.failed") {
            return Err(format!("job failed: {}", message["data"]["error"]["message"]).into());
        }
        Ok(message)
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
            let message = next_matching(&bridge, Duration::from_secs(20), |_| true)?;
            match message["event"].as_str().unwrap_or("") {
                "decision.request" => {
                    let job = message["data"]["job_id"].as_str().unwrap_or("").to_owned();
                    assert_eq!(message["data"]["kind"], "import_match");
                    asked.insert(job, ());
                    bridge.send(
                        "decision.reply",
                        json!({"decision_id":message["data"]["decision_id"],"value":"as_is"}),
                    )?;
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
    fn cancel_removes_a_queued_item_job() -> TestResult {
        let mut bridge = Bridge::with_runner(false)?;
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
        assert_eq!(listed["result"]["runner"], false);
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
        let repository = Repository::new(dir.path().join("muzik.db"));
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
            jobs: Arc::new(Jobs::in_memory()?),
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
}
