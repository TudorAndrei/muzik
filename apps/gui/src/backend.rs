use crate::thumbnails;
use async_channel::Receiver;
use bytesize::ByteSize;
use chrono::{DateTime, Local};
use muzik_bandcamp as bandcamp;
use muzik_core::app_config::{self, GuiDefaults};
use muzik_core::downloads::scan;
use muzik_core::paths::Paths;
use muzik_runner::agent::{Chooser, Codex};
use muzik_runner::app::WatchlistCheck;
use muzik_runner::{setup, App, AppEvent, AppOptions, EnqueueError};
use muzik_spotify as spotify;
use muzik_store::watchlist::{ItemAction, ItemId, Playlist};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

const WORKERS: usize = 5;

#[derive(Clone, Debug)]
pub struct ItemRequest {
    pub id: ItemId,
    pub title: String,
    pub action: ItemAction,
}

pub struct SoulseekForm {
    pub username: String,
    pub password: String,
    pub server_host: String,
    pub server_port: String,
}

struct ActiveLogin {
    job_id: String,
    cancel: Arc<AtomicBool>,
}

type LoginSlot = Arc<Mutex<Option<ActiveLogin>>>;

pub struct SpotifyLogin {
    job_id: String,
    cancel: Arc<AtomicBool>,
    config: PathBuf,
    token: PathBuf,
    slot: LoginSlot,
}

impl SpotifyLogin {
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn run(self) -> AppEvent {
        let job_id = self.job_id;
        let event = match spotify::login(&self.config, &self.token, None, &self.cancel) {
            Ok(name) => AppEvent::JobCompleted {
                job_id,
                result: json!({"account_name": name}),
            },
            Err(message) if message == "cancelled" => AppEvent::JobCancelled { job_id },
            Err(message) => AppEvent::JobFailed { job_id, message },
        };
        *self.slot.lock() = None;
        event
    }
}

pub struct Backend {
    app: App,
    login: LoginSlot,
    thumbnails: Mutex<HashSet<String>>,
    logins: AtomicU64,
    writes: Mutex<()>,
}

fn required<'a>(key: &str, value: &'a str) -> Result<&'a str, String> {
    Some(value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{key} must be a non-empty string."))
}

fn queued(result: Result<String, EnqueueError>) -> Result<String, String> {
    result.map_err(|error| error.to_string())
}

impl Backend {
    pub fn start() -> Result<(Self, Receiver<AppEvent>), String> {
        let paths = Paths::user();
        muzik_core::paths::migrate_legacy(&paths).map_err(|error| error.to_string())?;
        Self::with(paths, true)
    }

    fn with(paths: Paths, run: bool) -> Result<(Self, Receiver<AppEvent>), String> {
        let (sender, events) = async_channel::unbounded();
        let app = App::start(AppOptions {
            paths,
            workers: WORKERS,
            run,
            in_memory: cfg!(test),
            chooser: (!cfg!(test)).then(|| Arc::new(Codex) as Arc<dyn Chooser>),
            sink: Arc::new(move |event| {
                let _ = sender.try_send(event);
            }),
        })?;
        let backend = Self {
            app,
            login: Arc::new(Mutex::new(None)),
            thumbnails: Mutex::new(HashSet::new()),
            logins: AtomicU64::new(0),
            writes: Mutex::new(()),
        };
        Ok((backend, events))
    }

    fn paths(&self) -> &Paths {
        self.app.paths()
    }

    pub fn defaults(&self) -> GuiDefaults {
        app_config::load_gui_defaults(self.paths())
    }

    pub fn save_defaults(&self, defaults: &GuiDefaults) -> Result<GuiDefaults, String> {
        let params = serde_json::to_value(defaults).map_err(|error| error.to_string())?;
        let _write = self.writes.lock();
        Ok(app_config::save_gui_defaults(self.paths(), &params)?)
    }

    pub fn jobs(&self) -> Value {
        self.app.jobs()
    }

    pub fn start_workflow(&self, params: &Value) -> Result<String, String> {
        queued(self.app.start_workflow(params))
    }

    pub fn refresh(&self, source: Option<(&str, &str)>) -> Result<String, String> {
        queued(self.app.refresh(source))
    }

    pub fn run_item(&self, item: &ItemRequest) -> Result<String, String> {
        queued(self.app.run_item(&item.id, &item.title, item.action))
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), String> {
        let job_id = required("job_id", job_id)?;
        if self.app.cancel(job_id)? {
            return Ok(());
        }
        if let Some(active) = self
            .login
            .lock()
            .as_ref()
            .filter(|active| active.job_id == job_id)
        {
            active.cancel.store(true, Ordering::Relaxed);
            return Ok(());
        }
        Err("The job is not active.".into())
    }

    pub fn reply(&self, decision_id: &str, value: Value) -> Result<(), String> {
        let decision_id = required("decision_id", decision_id)?;
        if self.app.reply(decision_id, value) {
            Ok(())
        } else {
            Err("The decision is not pending.".into())
        }
    }

    pub fn answer(&self, id: i64, value: &Value) -> Result<bool, String> {
        self.app.answer(id, value)
    }

    pub fn load_watchlist(&self) -> Result<(Value, WatchlistCheck), String> {
        let login = Arc::clone(&self.login);
        self.app
            .load_watchlist(Arc::new(move || login.lock().is_some()))
    }

    pub fn add_source(&self, url: &str) -> Result<Playlist, String> {
        self.app.add_source(required("url", url)?)
    }

    pub fn rename_source(&self, playlist_id: &str, title: &str) -> Result<bool, String> {
        self.app.rename_source(
            required("playlist_id", playlist_id)?,
            required("title", title)?,
        )
    }

    pub fn remove_source(&self, playlist_id: &str) -> Result<bool, String> {
        self.app
            .remove_source(required("playlist_id", playlist_id)?)
    }

    pub fn cache_thumbnails(&self, ids: Vec<String>) -> Option<Value> {
        let fresh = {
            let mut pending = self.thumbnails.lock();
            ids.into_iter()
                .filter(|id| pending.insert(id.clone()))
                .collect::<Vec<_>>()
        };
        if fresh.is_empty() {
            return None;
        }
        let data = thumbnails::cache_requested(&fresh, &self.app.repository(), &self.paths().cache);
        let mut pending = self.thumbnails.lock();
        for id in &fresh {
            pending.remove(id);
        }
        Some(data)
    }

    pub fn library_scan(&self, output: &Path) -> Result<Value, String> {
        library_scan(self.paths(), output)
    }

    pub fn services(&self) -> Value {
        json!({"services": setup::check_services(self.paths())})
    }

    pub fn soulseek_account(&self) -> Result<Value, String> {
        setup::soulseek_account(&self.paths().config_file())
    }

    pub fn save_soulseek(&self, form: &SoulseekForm) -> Result<Value, String> {
        let path = self.paths().config_file();
        let username = required("username", &form.username)?;
        let server_port = form
            .server_port
            .trim()
            .parse()
            .map_err(|_| "Enter a server port from 1 to 65535.")?;
        let _write = self.writes.lock();
        setup::save_soulseek_account(
            &path,
            &setup::SoulseekAccount {
                username: Some(username),
                password: Some(&form.password),
                server_host: Some(&form.server_host),
                server_port: Some(server_port),
            },
        )?;
        setup::soulseek_account(&path)
    }

    pub fn bandcamp(&self) -> Value {
        bandcamp::status(self.paths())
    }

    pub fn save_bandcamp(&self, user: &str, cookies: &str) -> Result<Value, String> {
        let _write = self.writes.lock();
        bandcamp::Login::save(self.paths(), user, cookies)?;
        muzik_runner::watchlist::ensure_sources(self.paths())?;
        Ok(bandcamp::status(self.paths()))
    }

    pub fn logout_bandcamp(&self) -> Result<Value, String> {
        let _write = self.writes.lock();
        bandcamp::Login::clear(self.paths())?;
        Ok(bandcamp::status(self.paths()))
    }

    pub fn spotify_status(&self) -> Result<Value, String> {
        spotify::status(&self.paths().config_file(), &self.paths().spotify_token())
    }

    pub fn spotify_playlists(&self) -> Result<Value, String> {
        let playlists =
            spotify::list_playlists(&self.paths().config_file(), &self.paths().spotify_token())?;
        serde_json::to_value(playlists).map_err(|error| error.to_string())
    }

    pub fn set_spotify_client_id(&self, client_id: &str) -> Result<String, String> {
        let _write = self.writes.lock();
        Ok(spotify::set_client_id(
            &self.paths().config_file(),
            client_id,
        )?)
    }

    pub fn spotify_logout(&self) -> Result<bool, String> {
        let _write = self.writes.lock();
        Ok(spotify::clear_tokens(&self.paths().spotify_token())?)
    }

    pub fn spotify_login(&self) -> Result<SpotifyLogin, String> {
        let mut slot = self.login.lock();
        if slot.is_some() {
            return Err("A Spotify login is already active.".into());
        }
        let number = self.logins.fetch_add(1, Ordering::Relaxed) + 1;
        let job_id = format!("spotify-login-{number}");
        let cancel = Arc::new(AtomicBool::new(false));
        *slot = Some(ActiveLogin {
            job_id: job_id.clone(),
            cancel: Arc::clone(&cancel),
        });
        Ok(SpotifyLogin {
            job_id,
            cancel,
            config: self.paths().config_file(),
            token: self.paths().spotify_token(),
            slot: Arc::clone(&self.login),
        })
    }
}

fn library_scan(paths: &Paths, output: &Path) -> Result<Value, String> {
    let output = if output.as_os_str().is_empty() {
        paths.downloads()
    } else {
        output.to_path_buf()
    };
    let items = scan(&output).map_err(|error| error.to_string())?;
    let total = items
        .iter()
        .fold(0_u64, |size, item| size.saturating_add(item.size));
    let items = items
        .into_iter()
        .map(|item| {
            let modified: DateTime<Local> = item.modified_at.into();
            let size_label = ByteSize(item.size).to_string();
            let mut value = serde_json::to_value(item).map_err(|error| error.to_string())?;
            let fields = value
                .as_object_mut()
                .ok_or("invalid audio inventory item")?;
            fields.insert("size_label".into(), json!(size_label));
            fields.insert(
                "modified".into(),
                json!(modified.format("%Y-%m-%d %H:%M").to_string()),
            );
            Ok(value)
        })
        .collect::<Result<Vec<Value>, String>>()?;
    Ok(json!({"output": output, "total_size": ByteSize(total).to_string(), "items": items}))
}

#[cfg(test)]
mod tests {
    use super::{ActiveLogin, Backend, ItemRequest};
    use async_channel::Receiver;
    use muzik_core::paths::Paths;
    use muzik_runner::AppEvent;
    use muzik_store::watchlist::{ItemAction, ItemId};
    use serde_json::json;
    use std::collections::{HashSet, VecDeque};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct Events {
        receiver: Receiver<AppEvent>,
        skipped: VecDeque<AppEvent>,
    }

    impl Events {
        fn next(
            &mut self,
            timeout: Duration,
            wanted: impl Fn(&AppEvent) -> bool,
        ) -> Result<AppEvent, Box<dyn std::error::Error>> {
            if let Some(index) = self.skipped.iter().position(&wanted) {
                if let Some(event) = self.skipped.remove(index) {
                    return Ok(event);
                }
            }
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                match self.receiver.try_recv() {
                    Ok(event) if wanted(&event) => return Ok(event),
                    Ok(event) => self.skipped.push_back(event),
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
            Err("no matching event".into())
        }

        fn job(
            &mut self,
            job_id: &str,
            wanted: impl Fn(&AppEvent) -> bool,
        ) -> Result<AppEvent, Box<dyn std::error::Error>> {
            match self.next(Duration::from_secs(20), |event| {
                event.job_id() == Some(job_id)
                    && (wanted(event) || matches!(event, AppEvent::JobFailed { .. }))
            })? {
                AppEvent::JobFailed { message, .. } => Err(format!("job failed: {message}").into()),
                event => Ok(event),
            }
        }
    }

    fn started(
        run: bool,
    ) -> Result<(tempfile::TempDir, Backend, Events), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let (backend, receiver) = Backend::with(Paths::under(state.path()), run)?;
        let events = Events {
            receiver,
            skipped: VecDeque::new(),
        };
        Ok((state, backend, events))
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
    fn library_scan_reports_existing_audio() -> TestResult {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("Track [dQw4w9WgXcQ].mp3"), b"audio")?;
        let (_state, backend, _) = started(true)?;
        let result = backend.library_scan(dir.path())?;
        assert_eq!(result["total_size"], "5 B");
        assert_eq!(result["items"][0]["title"], "Track");
        assert_eq!(result["items"][0]["youtube_id"], "dQw4w9WgXcQ");
        assert!(result["items"][0]["modified"]
            .as_str()
            .is_some_and(|date| !date.is_empty()));
        Ok(())
    }

    #[test]
    fn invalid_requests_are_rejected() -> TestResult {
        let (_state, backend, _) = started(true)?;
        assert!(backend.start_workflow(&json!({"raw":"  "})).is_err());
        assert!(backend.reply("", json!("as_is")).is_err());
        assert!(backend.reply("missing", json!("as_is")).is_err());
        assert!(backend.cancel("").is_err());
        assert!(backend.cancel("queue-999").is_err());
        Ok(())
    }

    #[test]
    fn local_workflow_runs_as_a_queue_job() -> TestResult {
        let dir = tempfile::tempdir()?;
        let audio = dir.path().join("track.flac");
        fs::write(&audio, b"audio")?;
        let (_state, backend, mut events) = started(true)?;
        let job = backend.start_workflow(
            &json!({"raw":audio,"no_organize":true,"no_split":true,"dry_run":true}),
        )?;
        assert!(job.starts_with("queue-"));
        events.job(&job, |event| matches!(event, AppEvent::JobStarted { .. }))?;
        let AppEvent::JobCompleted { result, .. } =
            events.job(&job, |event| matches!(event, AppEvent::JobCompleted { .. }))?
        else {
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
        let (_state, backend, mut events) = started(true)?;
        let mut jobs = Vec::new();
        for (audio, config) in [(&audio_one, &config_one), (&audio_two, &config_two)] {
            jobs.push(backend.start_workflow(
                &json!({"raw":audio,"config":config,"no_split":true,"interactive":true}),
            )?);
        }
        let mut asked = HashSet::new();
        let mut done = 0;
        while done < 2 {
            match events.next(Duration::from_secs(20), |_| true)? {
                AppEvent::DecisionRequest {
                    job_id,
                    decision_id,
                    kind,
                    ..
                } => {
                    assert_eq!(kind, muzik_core::DecisionKind::ImportMatch);
                    asked.insert(job_id);
                    backend.reply(&decision_id, json!("as_is"))?;
                }
                AppEvent::JobCompleted { job_id, .. } if jobs.contains(&job_id) => done += 1,
                AppEvent::JobFailed { message, .. } => {
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
        let (_state, backend, mut events) = started(true)?;
        let job = backend.start_workflow(&json!({"raw":audio,"config":config,"no_split":true}))?;
        let AppEvent::DecisionRequest { decision_id, .. } = events.job(&job, |event| {
            matches!(event, AppEvent::DecisionRequest { .. })
        })?
        else {
            return Err("no decision request".into());
        };
        backend.cancel(&job)?;
        events.job(&job, |event| matches!(event, AppEvent::JobCancelled { .. }))?;
        assert!(audio.exists());
        assert!(!database.exists());
        assert!(backend.reply(&decision_id, json!("as_is")).is_err());
        Ok(())
    }

    #[test]
    fn cancel_removes_a_queued_item_job() -> TestResult {
        let (_state, backend, _) = started(false)?;
        let item = ItemRequest {
            id: ItemId::new("PL1", 2, Some("abcdefghijk")),
            title: "Song".into(),
            action: ItemAction::Run,
        };
        let job = backend.run_item(&item)?;
        assert!(backend.run_item(&item).is_err());
        let listed = backend.jobs();
        assert_eq!(listed["open"][0]["job_id"], job);
        assert_eq!(listed["runner"], false);
        backend.cancel(&job)?;
        assert_eq!(backend.jobs()["open"], json!([]));
        Ok(())
    }

    #[test]
    fn thumbnails_already_in_progress_are_not_fetched_again() -> TestResult {
        let (_state, backend, _) = started(true)?;
        assert!(backend.cache_thumbnails(Vec::new()).is_none());
        backend.thumbnails.lock().insert("abcdefghijk".into());
        assert!(backend
            .cache_thumbnails(vec!["abcdefghijk".into(), "abcdefghijk".into()])
            .is_none());
        Ok(())
    }

    #[test]
    fn spotify_login_uses_its_own_slot() -> TestResult {
        let (_state, backend, _) = started(true)?;
        let cancel = Arc::new(AtomicBool::new(false));
        *backend.login.lock() = Some(ActiveLogin {
            job_id: "spotify-login-test".into(),
            cancel: Arc::clone(&cancel),
        });
        assert!(backend.spotify_login().is_err());
        backend.cancel("spotify-login-test")?;
        assert!(cancel.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn watchlist_load_returns_saved_cards_before_local_check() -> TestResult {
        let (_state, backend, mut events) = started(true)?;
        let playlist = backend.add_source("https://www.youtube.com/playlist?list=PLnative123")?;
        assert_eq!(playlist.playlist_id, "PLnative123");
        let (saved, check) = backend.load_watchlist()?;
        assert_eq!(saved["playlists"][0]["playlist_id"], "PLnative123");
        check.run();
        let AppEvent::WatchlistUpdated(checked) = events.next(Duration::from_secs(5), |event| {
            matches!(event, AppEvent::WatchlistUpdated(_))
        })?
        else {
            return Err("no checked watchlist".into());
        };
        assert_eq!(checked["playlists"][0]["playlist_id"], "PLnative123");
        Ok(())
    }

    #[test]
    fn watchlist_edits_go_through_the_app() -> TestResult {
        let (_state, backend, _) = started(true)?;
        backend.add_source("https://www.youtube.com/playlist?list=PL123")?;
        assert!(backend.rename_source("PL123", "  New name  ")?);
        assert!(backend.rename_source("PL123", "").is_err());
        assert!(backend.remove_source("PL123")?);
        Ok(())
    }
}
