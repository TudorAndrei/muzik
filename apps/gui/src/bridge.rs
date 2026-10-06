//! Request and response transport between the desktop view and the runner `App`.
use crate::{native, thumbnails};
use muzik_core::app_config;
use muzik_core::paths::Paths;
use muzik_runner::agent::{Chooser, Codex};
use muzik_runner::{App, AppEvent, AppOptions, EnqueueError};
use muzik_spotify as spotify;
use muzik_store::watchlist::{ItemAction, ItemId};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

const WORKERS: usize = 5;

pub enum Message {
    Response(Value),
    App(AppEvent),
    Thumbnails(Value),
}

pub struct Bridge {
    output: Receiver<Message>,
    native_output: Sender<Message>,
    thumbnail_pending: Arc<Mutex<HashSet<String>>>,
    login: Arc<Mutex<Option<NativeLogin>>>,
    app: Arc<App>,
    next_id: u64,
}

struct NativeLogin {
    job_id: String,
    cancel: Arc<AtomicBool>,
}

impl Bridge {
    pub fn start() -> Result<Self, String> {
        let paths = Paths::user();
        muzik_core::paths::migrate_legacy(&paths).map_err(|error| error.to_string())?;
        Self::with(paths, true)
    }

    fn with(paths: Paths, run: bool) -> Result<Self, String> {
        let (events, output) = mpsc::channel::<Message>();
        let sink = Mutex::new(events.clone());
        let app = App::start(AppOptions {
            paths,
            workers: WORKERS,
            run,
            in_memory: cfg!(test),
            chooser: (!cfg!(test)).then(|| Arc::new(Codex) as Arc<dyn Chooser>),
            sink: Arc::new(move |event| {
                if let Ok(sender) = sink.lock() {
                    let _ = sender.send(Message::App(event));
                }
            }),
        })?;
        Ok(Self {
            output,
            native_output: events,
            thumbnail_pending: Arc::new(Mutex::new(HashSet::new())),
            login: Arc::new(Mutex::new(None)),
            app: Arc::new(app),
            next_id: 1,
        })
    }

    fn respond(&self, message: Value) -> Result<(), String> {
        self.native_output
            .send(Message::Response(message))
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
            "spotify.login" => self.start_spotify_login(id, params),
            "decision.reply" => self.reply(&id, &params),
            "job.cancel" => self.cancel(&id, &params),
            "jobs.list" => self.accept(&id, self.app.jobs()),
            "jobs.answer" => self.answer(&id, &params),
            "workflow.start" => {
                let queued = self.app.start_workflow(&params);
                self.queued(&id, queued)
            }
            "watchlist.refresh" => {
                let source = params["playlist_id"]
                    .as_str()
                    .filter(|playlist| !playlist.is_empty())
                    .map(|playlist| {
                        (
                            playlist,
                            params["playlist_title"].as_str().unwrap_or("source"),
                        )
                    });
                let queued = self.app.refresh(source);
                self.queued(&id, queued)
            }
            "watchlist.action" => self.run_item(&id, &params),
            "watchlist.load" => self.load_watchlist(id),
            "watchlist.add" | "watchlist.rename" | "watchlist.remove" => {
                let result = self.edit_watchlist(command, &params);
                match result {
                    Ok(result) => self.accept(&id, result),
                    Err(message) => self.reject(&id, "operation_failed", message),
                }
            }
            "thumbnails.cache" => self.cache_thumbnails(id, &params),
            _ if native::handles(command) => {
                let paths = self.app.paths().clone();
                if matches!(
                    command,
                    "library.scan" | "services.check" | "spotify.status" | "spotify.playlists"
                ) {
                    let sender = self.native_output.clone();
                    let response_id = id.clone();
                    let command = command.to_owned();
                    thread::spawn(move || {
                        let response = native_response(&paths, &response_id, &command, &params);
                        let _ = sender.send(Message::Response(response));
                    });
                } else {
                    self.respond(native_response(&paths, &id, command, &params))?;
                }
                Ok(id)
            }
            _ => self.reject(
                &id,
                "invalid_request",
                format!("Unknown command: {command}"),
            ),
        }
    }

    fn run_item(&self, id: &str, params: &Value) -> Result<String, String> {
        let item = match ItemId::from_params(params) {
            Ok(item) => item,
            Err(message) => return self.reject(id, "invalid_request", message),
        };
        let action = params["action"].as_str().unwrap_or("");
        let Ok(action) = action.parse::<ItemAction>() else {
            return self.reject(
                id,
                "invalid_request",
                format!("'{action}' is not a valid ItemAction"),
            );
        };
        let title = params["title"]
            .as_str()
            .or_else(|| params["video_id"].as_str())
            .unwrap_or("Item");
        let queued = self.app.run_item(&item, title, action);
        self.queued(id, queued)
    }

    fn load_watchlist(&self, id: String) -> Result<String, String> {
        let app = Arc::clone(&self.app);
        let sender = self.native_output.clone();
        let login = Arc::clone(&self.login);
        let response_id = id.clone();
        thread::spawn(move || {
            let busy = Arc::new(move || login.lock().map_or(true, |active| active.is_some()));
            match app.load_watchlist(busy) {
                Ok((saved, check)) => {
                    let response = json!({"id":response_id,"type":"response","ok":true,"result":{"watchlist":saved}});
                    if sender.send(Message::Response(response)).is_ok() {
                        check.run();
                    }
                }
                Err(message) => {
                    let _ = sender.send(Message::Response(json!({"id":response_id,"type":"response","ok":false,"error":{"code":"operation_failed","message":message}})));
                }
            }
        });
        Ok(id)
    }

    fn edit_watchlist(&self, command: &str, params: &Value) -> Result<Value, String> {
        let text = |key: &str| {
            params[key]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("{key} must be a non-empty string."))
        };
        Ok(match command {
            "watchlist.add" => json!({"playlist": self.app.add_source(text("url")?)?}),
            "watchlist.rename" => json!({"renamed": self.app.rename_source(
                text("playlist_id")?,
                text("title")?,
            )?}),
            _ => json!({"removed": self.app.remove_source(text("playlist_id")?)?}),
        })
    }

    fn cache_thumbnails(&self, id: String, params: &Value) -> Result<String, String> {
        let ids = match thumbnails::validate_ids(params) {
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
            let app = Arc::clone(&self.app);
            thread::spawn(move || {
                let data =
                    thumbnails::cache_requested(&fresh, &app.repository(), &app.paths().cache);
                let _ = sender.send(Message::Thumbnails(data));
                if let Ok(mut pending) = pending.lock() {
                    for id in fresh {
                        pending.remove(&id);
                    }
                }
            });
        }
        Ok(id)
    }

    fn queued(&self, id: &str, queued: Result<String, EnqueueError>) -> Result<String, String> {
        match queued {
            Ok(job_id) => self.accept(id, json!({"job_id":job_id})),
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
        let value = params.get("value").cloned().unwrap_or(Value::Null);
        if self.app.reply(decision_id, value) {
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
        if self.app.cancel(job_id)? {
            return self.accept(id, json!({"job_id":job_id,"cancel_requested":true}));
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
        let answered = self.app.answer(job_id, value)?;
        self.accept(id, json!({"answered":answered}))
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
        let config = self.app.paths().config_file();
        let token = self.app.paths().spotify_token();
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
            let event = match spotify::login(&config, &token, port, &cancel) {
                Ok(name) => AppEvent::JobCompleted {
                    job_id,
                    result: json!({"account_name": name}),
                },
                Err(message) if message == "cancelled" => AppEvent::JobCancelled { job_id },
                Err(message) => AppEvent::JobFailed { job_id, message },
            };
            let _ = sender.send(Message::App(event));
            if let Ok(mut active) = state.lock() {
                *active = None;
            }
        });
        Ok(id)
    }

    pub fn drain(&self) -> Vec<Message> {
        self.output.try_iter().collect()
    }
}

fn native_response(paths: &Paths, id: &str, command: &str, params: &Value) -> Value {
    match native::dispatch(paths, command, params) {
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
    use super::{Bridge, Message, NativeLogin};
    use muzik_core::paths::Paths;
    use muzik_runner::AppEvent;
    use serde_json::{json, Value};
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    thread_local! {
        static SKIPPED: RefCell<VecDeque<Message>> = const { RefCell::new(VecDeque::new()) };
    }

    fn next_matching(
        bridge: &Bridge,
        timeout: Duration,
        wanted: impl Fn(&Message) -> bool,
    ) -> Result<Message, Box<dyn std::error::Error>> {
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

    fn started() -> Result<(tempfile::TempDir, Bridge), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let bridge = Bridge::with(Paths::under(state.path()), true)?;
        Ok((state, bridge))
    }

    fn response(bridge: &Bridge, id: &str) -> Result<Value, Box<dyn std::error::Error>> {
        match next_matching(
            bridge,
            Duration::from_secs(5),
            |message| matches!(message, Message::Response(value) if value["id"] == id),
        )? {
            Message::Response(value) => Ok(value),
            _ => Err("not a response".into()),
        }
    }

    fn job_event(
        bridge: &Bridge,
        job_id: &str,
        wanted: impl Fn(&AppEvent) -> bool,
    ) -> Result<AppEvent, Box<dyn std::error::Error>> {
        match next_matching(bridge, Duration::from_secs(20), |message| {
            matches!(message, Message::App(event) if event.job_id() == Some(job_id)
                && (wanted(event) || matches!(event, AppEvent::JobFailed { .. })))
        })? {
            Message::App(AppEvent::JobFailed { message, .. }) => {
                Err(format!("job failed: {message}").into())
            }
            Message::App(event) => Ok(event),
            _ => Err("not an app event".into()),
        }
    }

    fn job_id(value: &Value) -> Result<String, Box<dyn std::error::Error>> {
        Ok(value["result"]["job_id"]
            .as_str()
            .ok_or("missing job ID")?
            .to_owned())
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
        let (_state, mut bridge) = started()?;
        let id = bridge.send("library.scan", json!({"output": dir.path()}))?;
        assert_eq!(response(&bridge, &id)?["result"]["total_size"], "5.0 B");
        Ok(())
    }

    #[test]
    fn startup_answers_hello() -> TestResult {
        let (_state, mut bridge) = started()?;
        let id = bridge.send("hello", json!({}))?;
        assert_eq!(response(&bridge, &id)?["result"]["protocol_version"], 1);
        Ok(())
    }

    #[test]
    fn invalid_workflow_requests_use_the_native_protocol_response() -> TestResult {
        let (_state, mut bridge) = started()?;
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
        let (_state, mut bridge) = started()?;
        let id = bridge.send(
            "workflow.start",
            json!({"raw":audio,"no_organize":true,"no_split":true,"dry_run":true}),
        )?;
        let job = job_id(&response(&bridge, &id)?)?;
        assert!(job.starts_with("queue-"));
        job_event(&bridge, &job, |event| {
            matches!(event, AppEvent::JobStarted { .. })
        })?;
        let done = job_event(&bridge, &job, |event| {
            matches!(event, AppEvent::JobCompleted { .. })
        })?;
        let AppEvent::JobCompleted { result, .. } = done else {
            return Err("the job did not complete".into());
        };
        assert_eq!(result["singles"], 1);
        assert!(audio.exists());
        Ok(())
    }

    #[test]
    fn two_workflow_runs_share_the_import_gate() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let (audio_one, database_one, config_one) = fixture_import(first.path())?;
        let (audio_two, database_two, config_two) = fixture_import(second.path())?;
        let (_state, mut bridge) = started()?;
        let mut jobs = Vec::new();
        for (audio, config) in [(&audio_one, &config_one), (&audio_two, &config_two)] {
            let id = bridge.send(
                "workflow.start",
                json!({"raw":audio,"config":config,"no_split":true,"interactive":true}),
            )?;
            jobs.push(job_id(&response(&bridge, &id)?)?);
        }
        let mut asked = HashMap::new();
        let mut done = 0;
        while done < 2 {
            match next_matching(&bridge, Duration::from_secs(20), |_| true)? {
                Message::App(AppEvent::DecisionRequest {
                    job_id,
                    decision_id,
                    kind,
                    ..
                }) => {
                    assert_eq!(kind, muzik_core::DecisionKind::ImportMatch);
                    asked.insert(job_id, ());
                    bridge.send(
                        "decision.reply",
                        json!({"decision_id":decision_id,"value":"as_is"}),
                    )?;
                }
                Message::App(AppEvent::JobCompleted { job_id, .. }) if jobs.contains(&job_id) => {
                    done += 1;
                }
                Message::App(AppEvent::JobFailed { message, .. }) => {
                    return Err(format!("job failed: {message}").into())
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
        let (_state, mut bridge) = started()?;
        let id = bridge.send(
            "workflow.start",
            json!({"raw":audio,"config":config,"no_split":true}),
        )?;
        let job = job_id(&response(&bridge, &id)?)?;
        let AppEvent::DecisionRequest { decision_id, .. } = job_event(&bridge, &job, |event| {
            matches!(event, AppEvent::DecisionRequest { .. })
        })?
        else {
            return Err("no decision request".into());
        };
        let id = bridge.send("job.cancel", json!({"job_id":job}))?;
        assert_eq!(response(&bridge, &id)?["result"]["cancel_requested"], true);
        job_event(&bridge, &job, |event| {
            matches!(event, AppEvent::JobCancelled { .. })
        })?;
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
        let dir = tempfile::tempdir()?;
        let mut bridge = Bridge::with(Paths::under(dir.path()), false)?;
        let params = json!({"playlist_id":"PL1","position":2,"video_id":"abcdefghijk","action":"run","title":"Song"});
        let id = bridge.send("watchlist.action", params.clone())?;
        let job = job_id(&response(&bridge, &id)?)?;
        let again = bridge.send("watchlist.action", params)?;
        assert_eq!(response(&bridge, &again)?["error"]["code"], "job_active");
        let id = bridge.send("jobs.list", json!({}))?;
        let listed = response(&bridge, &id)?;
        assert_eq!(listed["result"]["open"][0]["job_id"], job);
        assert_eq!(listed["result"]["runner"], false);
        let id = bridge.send("job.cancel", json!({"job_id":job}))?;
        assert_eq!(response(&bridge, &id)?["result"]["cancel_requested"], true);
        let id = bridge.send("jobs.list", json!({}))?;
        assert_eq!(response(&bridge, &id)?["result"]["open"], json!([]));
        Ok(())
    }

    #[test]
    fn thumbnail_requests_get_a_native_protocol_response() -> TestResult {
        let (_state, mut bridge) = started()?;
        let id = bridge.send("thumbnails.cache", json!({"video_ids": []}))?;
        assert_eq!(response(&bridge, &id)?["result"]["queued"], 0);
        let id = bridge.send("thumbnails.cache", json!({"video_ids": [42]}))?;
        assert_eq!(response(&bridge, &id)?["error"]["code"], "invalid_request");
        Ok(())
    }

    #[test]
    fn spotify_login_validates_port_and_uses_its_own_slot() -> TestResult {
        let (_state, mut bridge) = started()?;
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
        let (_state, mut bridge) = started()?;
        let id = bridge.send(
            "watchlist.add",
            json!({"url":"https://www.youtube.com/playlist?list=PLnative123"}),
        )?;
        assert_eq!(
            response(&bridge, &id)?["result"]["playlist"]["playlist_id"],
            "PLnative123"
        );
        let id = bridge.send("watchlist.load", json!({}))?;
        let saved = response(&bridge, &id)?;
        assert_eq!(
            saved["result"]["watchlist"]["playlists"][0]["playlist_id"],
            "PLnative123"
        );
        let Message::App(AppEvent::WatchlistUpdated(checked)) =
            next_matching(&bridge, Duration::from_secs(5), |message| {
                matches!(message, Message::App(AppEvent::WatchlistUpdated(_)))
            })?
        else {
            return Err("no checked watchlist".into());
        };
        assert_eq!(checked["playlists"][0]["playlist_id"], "PLnative123");
        Ok(())
    }

    #[test]
    fn watchlist_edits_go_through_the_app() -> TestResult {
        let (_state, mut bridge) = started()?;
        let id = bridge.send(
            "watchlist.add",
            json!({"url": "https://www.youtube.com/playlist?list=PL123"}),
        )?;
        assert_eq!(
            response(&bridge, &id)?["result"]["playlist"]["playlist_id"],
            "PL123"
        );
        let id = bridge.send(
            "watchlist.rename",
            json!({"playlist_id": "PL123", "title": "  New name  "}),
        )?;
        assert_eq!(response(&bridge, &id)?["result"]["renamed"], true);
        let id = bridge.send("watchlist.rename", json!({"playlist_id": "PL123"}))?;
        assert_eq!(response(&bridge, &id)?["error"]["code"], "operation_failed");
        let id = bridge.send("watchlist.remove", json!({"playlist_id": "PL123"}))?;
        assert_eq!(response(&bridge, &id)?["result"]["removed"], true);
        Ok(())
    }

    #[test]
    fn unknown_command_has_protocol_error() -> TestResult {
        let (_state, mut bridge) = started()?;
        let id = bridge.send("unknown.command", json!({}))?;
        assert_eq!(response(&bridge, &id)?["error"]["code"], "invalid_request");
        Ok(())
    }
}
