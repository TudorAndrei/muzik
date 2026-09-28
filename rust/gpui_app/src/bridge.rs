//! JSON-lines link for commands still served by the Python workflow process.
use crate::{native, thumbnails, watchlist};
use muzik_core::{app_config, spotify, watchlist::Repository};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct Bridge {
    input: Option<Sender<Value>>,
    output: Receiver<Value>,
    native_output: Sender<Value>,
    thumbnail_pending: Arc<Mutex<HashSet<String>>>,
    login: Arc<Mutex<Option<NativeLogin>>>,
    python_job: Arc<Mutex<PythonJob>>,
    watchlist_generation: Arc<AtomicU64>,
    watchlist_gate: Arc<Mutex<()>>,
    next_id: u64,
}

struct NativeLogin {
    job_id: String,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct PythonJob {
    pending: HashSet<String>,
    running: HashSet<String>,
    completed_before_response: HashSet<String>,
}

impl PythonJob {
    fn active(&self) -> bool {
        !self.pending.is_empty() || !self.running.is_empty()
    }
}

impl Bridge {
    pub fn start() -> Result<Self, String> {
        let (events, output) = mpsc::channel::<Value>();
        Ok(Self {
            input: None,
            output,
            native_output: events,
            thumbnail_pending: Arc::new(Mutex::new(HashSet::new())),
            login: Arc::new(Mutex::new(None)),
            python_job: Arc::new(Mutex::new(PythonJob::default())),
            watchlist_generation: Arc::new(AtomicU64::new(0)),
            watchlist_gate: Arc::new(Mutex::new(())),
            next_id: 1,
        })
    }

    fn start_python(&mut self) -> Result<(), String> {
        let python = std::env::var("MUZIK_PYTHON").unwrap_or_else(|_| "python3".into());
        let mut child = Command::new(python)
            .args(["-m", "muzik.native_gui"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| format!("Cannot start the Python service: {err}"))?;
        let mut stdin = child.stdin.take().ok_or("Cannot open Python input")?;
        let stdout = child.stdout.take().ok_or("Cannot open Python output")?;
        let (input, commands) = mpsc::channel::<Value>();
        let events = self.native_output.clone();
        let job = Arc::clone(&self.python_job);
        thread::spawn(move || {
            while let Ok(command) = commands.recv() {
                if writeln!(stdin, "{command}").is_err() || stdin.flush().is_err() {
                    break;
                }
            }
        });
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => match serde_json::from_str::<Value>(&line) {
                        Ok(message) => {
                            update_python_job(&job, &message);
                            if events.send(message).is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            let _ = events.send(json!({"type":"transport.error","message":format!("Invalid Python response: {err}")}));
                        }
                    },
                    Err(err) => {
                        let _ = events.send(json!({"type":"transport.error","message":format!("Python output failed: {err}")}));
                        break;
                    }
                }
            }
            update_python_job(&job, &json!({"type":"transport.closed"}));
            let _ = events.send(json!({"type":"transport.closed"}));
            let _ = child.wait();
        });
        self.input = Some(input);
        Ok(())
    }

    pub fn send(&mut self, command: &str, params: Value) -> Result<String, String> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        if command == "spotify.login" {
            return self.start_spotify_login(id, params);
        }
        if command == "job.cancel" {
            let job_id = params.get("job_id").and_then(Value::as_str).unwrap_or("");
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
            if job_id.starts_with("spotify-login-") {
                self.native_output
                    .send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":"The job is not active."}}))
                    .map_err(|_| "Rust backend is not available".to_owned())?;
                return Ok(id);
            }
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
            let job = Arc::clone(&self.python_job);
            let login = Arc::clone(&self.login);
            let latest = Arc::clone(&self.watchlist_generation);
            let gate = Arc::clone(&self.watchlist_gate);
            let repository = Repository::new(Repository::default_path());
            thread::spawn(move || {
                load_watchlist(WatchlistLoad {
                    sender,
                    id: response_id,
                    params,
                    repository,
                    job,
                    login,
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
            let job = self
                .python_job
                .lock()
                .map_err(|_| "Python job state is unavailable")?;
            let login = self
                .login
                .lock()
                .map_err(|_| "Spotify login state is unavailable")?;
            let response = if job.active() || login.is_some() {
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
        if let Err(message) = validate_python_command(command, &params) {
            self.native_output
                .send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":message}}))
                .map_err(|_| "Rust backend is not available".to_owned())?;
            return Ok(id);
        }
        if self.input.is_none() {
            self.start_python()?;
        }
        let starts_job = matches!(
            command,
            "workflow.start" | "watchlist.refresh" | "watchlist.action"
        );
        if starts_job {
            let _gate = self
                .watchlist_gate
                .lock()
                .map_err(|_| "Watchlist state is unavailable")?;
            self.watchlist_generation.fetch_add(1, Ordering::SeqCst);
            let mut job = self
                .python_job
                .lock()
                .map_err(|_| "Python job state is unavailable")?;
            job.pending.insert(id.clone());
        }
        let send_result = self
            .input
            .as_ref()
            .ok_or("Python service is not available")?
            .send(json!({"id":id,"command":command,"params":params}))
            .map_err(|_| "Python service is not available".to_string());
        if send_result.is_err() && starts_job {
            if let Ok(mut job) = self.python_job.lock() {
                job.pending.remove(&id);
            }
        }
        send_result?;
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
        if login.is_some() {
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

fn validate_python_command(command: &str, params: &Value) -> Result<(), String> {
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

struct WatchlistLoad {
    sender: Sender<Value>,
    id: String,
    params: Value,
    repository: Repository,
    job: Arc<Mutex<PythonJob>>,
    login: Arc<Mutex<Option<NativeLogin>>>,
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
            .job
            .lock()
            .map_err(|_| "Python job state is unavailable")?
            .active()
            || load
                .login
                .lock()
                .map_err(|_| "Spotify login state is unavailable")?
                .is_some()
            || load.generation != load.latest.load(Ordering::SeqCst)
        {
            return Ok(());
        }
        let path = load.repository.path();
        let stamp = watchlist::stamp(path)?;
        let checked = options.checked(&load.repository)?;
        let job = load
            .job
            .lock()
            .map_err(|_| "Python job state is unavailable")?;
        if job.active()
            || load
                .login
                .lock()
                .map_err(|_| "Spotify login state is unavailable")?
                .is_some()
            || load.generation != load.latest.load(Ordering::SeqCst)
        {
            return Ok(());
        }
        if watchlist::stamp(path)? != stamp {
            continue;
        }
        load.repository.save(checked.clone())?;
        drop(job);
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

fn update_python_job(state: &Arc<Mutex<PythonJob>>, message: &Value) {
    let Ok(mut job) = state.lock() else {
        return;
    };
    if message["type"] == "response" {
        if let Some(id) = message["id"].as_str() {
            if job.pending.remove(id) && message["ok"] == true {
                if let Some(job_id) = message["result"]["job_id"].as_str() {
                    if !job.completed_before_response.remove(job_id) {
                        job.running.insert(job_id.to_owned());
                    }
                }
            }
        }
    } else if message["type"] == "event"
        && matches!(
            message["event"].as_str(),
            Some("job.completed" | "job.failed" | "job.cancelled")
        )
    {
        if let Some(job_id) = message["data"]["job_id"].as_str() {
            job.running.remove(job_id);
            if !job.pending.is_empty() {
                job.completed_before_response.insert(job_id.to_owned());
            }
        }
    } else if message["type"] == "transport.closed" {
        *job = PythonJob::default();
    }
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
    use super::{load_watchlist, update_python_job, Bridge, NativeLogin, PythonJob, WatchlistLoad};
    use muzik_core::watchlist::Repository;
    use serde_json::{json, Value};
    use std::collections::HashSet;
    use std::fs;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn library_scan_uses_rust_without_sending_a_python_request(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("Track [dQw4w9WgXcQ].mp3"), b"audio")?;
        let (input, python_requests) = mpsc::channel::<Value>();
        let (native_output, output) = mpsc::channel::<Value>();
        let mut bridge = Bridge {
            input: Some(input),
            output,
            native_output,
            thumbnail_pending: Arc::new(Mutex::new(HashSet::new())),
            login: Arc::new(Mutex::new(None)),
            python_job: Arc::new(Mutex::new(PythonJob::default())),
            watchlist_generation: Arc::new(AtomicU64::new(0)),
            watchlist_gate: Arc::new(Mutex::new(())),
            next_id: 1,
        };
        let id = bridge.send("library.scan", json!({"output": dir.path()}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["total_size"], "5.0 B");
        assert!(python_requests.try_recv().is_err());
        Ok(())
    }

    #[test]
    fn startup_answers_hello_without_starting_python() -> Result<(), Box<dyn std::error::Error>> {
        let mut bridge = Bridge::start()?;
        assert!(bridge.input.is_none());
        let id = bridge.send("hello", json!({}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["protocol_version"], 1);
        assert!(bridge.input.is_none());
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
            assert!(bridge.input.is_none());
        }
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
        assert!(bridge.input.is_none());

        let id = bridge.send("thumbnails.cache", json!({"video_ids": [42]}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], "invalid_request");
        assert!(bridge.input.is_none());
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
        assert!(bridge.input.is_none());

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
        assert!(bridge.input.is_none());
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
            job: Arc::new(Mutex::new(PythonJob::default())),
            login: Arc::new(Mutex::new(None)),
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
        bridge
            .python_job
            .lock()
            .map_err(|_| "job lock failed")?
            .running
            .insert("running-job".into());
        let id = bridge.send("watchlist.add", json!({"url":"liked"}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], "job_active");
        Ok(())
    }

    #[test]
    fn a_rejected_start_keeps_the_first_job_active() -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(PythonJob::default()));
        state
            .lock()
            .map_err(|_| "job lock failed")?
            .pending
            .insert("start-1".into());
        update_python_job(
            &state,
            &json!({"type":"response","id":"start-1","ok":true,"result":{"job_id":"job-1"}}),
        );
        state
            .lock()
            .map_err(|_| "job lock failed")?
            .pending
            .insert("start-2".into());
        update_python_job(
            &state,
            &json!({"type":"response","id":"start-2","ok":false,"error":{"code":"job_active"}}),
        );
        assert!(state.lock().map_err(|_| "job lock failed")?.active());
        update_python_job(
            &state,
            &json!({"type":"event","event":"job.completed","data":{"job_id":"job-1"}}),
        );
        assert!(!state.lock().map_err(|_| "job lock failed")?.active());
        state
            .lock()
            .map_err(|_| "job lock failed")?
            .pending
            .insert("start-3".into());
        update_python_job(
            &state,
            &json!({"type":"event","event":"job.completed","data":{"job_id":"job-3"}}),
        );
        update_python_job(
            &state,
            &json!({"type":"response","id":"start-3","ok":true,"result":{"job_id":"job-3"}}),
        );
        assert!(!state.lock().map_err(|_| "job lock failed")?.active());
        Ok(())
    }
}
