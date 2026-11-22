use crate::agent::{Chooser, Codex};
use crate::app::WatchlistCheck;
use crate::library_info::{self, LibraryInfo};
use crate::library_tasks::LibraryTask;
use crate::thumbnails::{self, ThumbnailUpdate};
use crate::{App, AppEvent, AppOptions, Error, Result, Sink, setup};
use muzik_bandcamp as bandcamp;
use muzik_core::app_config::{self, GuiDefaults};
use muzik_core::paths::Paths;
use muzik_import::beets;
use muzik_spotify as spotify;
use muzik_store::watchlist::{ItemAction, ItemId, Playlist, WatchlistView};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const WORKERS: usize = 5;

#[derive(Clone, Debug)]
pub struct ItemRequest {
    pub id: ItemId,
    pub title: String,
    pub action: ItemAction,
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct SpotifyStatus {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub redirect_uri: String,
    #[serde(default)]
    pub connected: bool,
    pub account_name: Option<String>,
    pub error: Option<String>,
    #[serde(skip)]
    pub playlists: Option<Vec<spotify::PlaylistRef>>,
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
    #[must_use]
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    #[must_use]
    pub fn run(self) -> AppEvent {
        let job_id = self.job_id;
        let event = match spotify::login(&self.config, &self.token, None, &self.cancel) {
            Ok(name) => AppEvent::JobCompleted {
                job_id,
                result: json!({"account_name": name}),
            },
            Err(spotify::Error::Cancelled) => AppEvent::JobCancelled { job_id },
            Err(error) => AppEvent::JobFailed {
                job_id,
                message: error.to_string(),
            },
        };
        *self.slot.lock() = None;
        event
    }
}

pub struct Service {
    app: App,
    login: LoginSlot,
    thumbnails: Mutex<HashSet<String>>,
    logins: AtomicU64,
    writes: Mutex<()>,
}

fn required<'a>(key: &str, value: &'a str) -> Result<&'a str> {
    Some(value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Message(format!("{key} must be a non-empty string.")))
}

impl Service {
    /// # Errors
    /// Returns an error when legacy data folders cannot be migrated.
    pub fn user_paths() -> Result<Paths> {
        let paths = Paths::user();
        muzik_core::paths::migrate_legacy(&paths)?;
        Ok(paths)
    }

    /// # Errors
    /// Returns an error when the job queue cannot be opened or the runner cannot start.
    pub fn start(paths: Paths, sink: Sink) -> Result<Self> {
        Self::with(paths, true, sink)
    }

    pub(crate) fn with(paths: Paths, run: bool, sink: Sink) -> Result<Self> {
        let app = App::start(AppOptions {
            paths,
            workers: WORKERS,
            run,
            in_memory: cfg!(test),
            chooser: (!cfg!(test)).then(|| -> Arc<dyn Chooser> { Arc::new(Codex) }),
            sink,
        })?;
        Ok(Self {
            app,
            login: Arc::new(Mutex::new(None)),
            thumbnails: Mutex::new(HashSet::new()),
            logins: AtomicU64::new(0),
            writes: Mutex::new(()),
        })
    }

    #[must_use]
    pub const fn paths(&self) -> &Paths {
        self.app.paths()
    }

    #[must_use]
    pub fn defaults(&self) -> GuiDefaults {
        app_config::load_gui_defaults(self.paths())
    }

    /// # Errors
    /// Returns an error when the defaults cannot be saved or the playlist files cannot be written.
    pub fn save_defaults(&self, defaults: &GuiDefaults) -> Result<GuiDefaults> {
        let params = serde_json::to_value(defaults)?;
        let _write = self.writes.lock();
        let before = app_config::load_gui_defaults(self.paths());
        let saved = app_config::save_gui_defaults(self.paths(), &params)?;
        if (before.playlists, before.playlist_format) != (saved.playlists, saved.playlist_format) {
            self.app.write_playlists()?;
        }
        Ok(saved)
    }

    /// # Errors
    /// Returns an error when the playlist ID is empty or the source cannot be saved.
    pub fn set_source_playlist(&self, playlist_id: &str, on: bool) -> Result<bool> {
        self.app
            .set_source_playlist(required("playlist_id", playlist_id)?, on)
    }

    #[must_use]
    pub fn jobs(&self) -> Value {
        self.app.jobs()
    }

    /// # Errors
    /// Returns an error when the request is invalid or cannot be queued.
    pub fn start_workflow(&self, params: &Value) -> Result<String> {
        Ok(self.app.start_workflow(params)?)
    }

    /// # Errors
    /// Returns an error when the task cannot be queued.
    pub fn start_library_task(&self, task: LibraryTask) -> Result<String> {
        Ok(self.app.start_library_task(task)?)
    }

    /// # Errors
    /// Returns an error when the refresh cannot be queued.
    pub fn refresh(&self, source: Option<(&str, &str)>) -> Result<String> {
        Ok(self.app.refresh(source)?)
    }

    /// # Errors
    /// Returns an error when the item cannot be queued.
    pub fn run_item(&self, item: &ItemRequest) -> Result<String> {
        Ok(self.app.run_item(&item.id, &item.title, item.action)?)
    }

    /// # Errors
    /// Returns an error when the job ID is empty, the job is not active, or it cannot be stopped.
    pub fn cancel(&self, job_id: &str) -> Result<()> {
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

    /// # Errors
    /// Returns an error when the decision ID is empty or the decision is not pending.
    pub fn reply(&self, decision_id: &str, value: Value) -> Result<()> {
        let decision_id = required("decision_id", decision_id)?;
        if self.app.reply(decision_id, value) {
            Ok(())
        } else {
            Err("The decision is not pending.".into())
        }
    }

    /// # Errors
    /// Returns an error when the answer cannot be saved.
    pub fn answer(&self, id: i64, value: &Value) -> Result<bool> {
        self.app.answer(id, value)
    }

    /// # Errors
    /// Returns an error when the answer cannot be saved.
    pub fn answer_version(&self, id: i64, version: i64, value: &Value) -> Result<crate::Answered> {
        self.app.answer_version(id, version, value)
    }

    /// # Errors
    /// Returns an error when the saved watchlist cannot be read.
    pub fn load_watchlist(&self) -> Result<(WatchlistView, WatchlistCheck)> {
        let login = Arc::clone(&self.login);
        self.app
            .load_watchlist(Arc::new(move || login.lock().is_some()))
    }

    /// # Errors
    /// Returns an error when the URL is empty or the source cannot be added.
    pub fn add_source(&self, url: &str) -> Result<Playlist> {
        self.app.add_source(required("url", url)?)
    }

    /// # Errors
    /// Returns an error when an argument is empty or the source cannot be renamed.
    pub fn rename_source(&self, playlist_id: &str, title: &str) -> Result<bool> {
        self.app.rename_source(
            required("playlist_id", playlist_id)?,
            required("title", title)?,
        )
    }

    /// # Errors
    /// Returns an error when the playlist ID is empty or the source cannot be removed.
    pub fn remove_source(&self, playlist_id: &str) -> Result<bool> {
        self.app
            .remove_source(required("playlist_id", playlist_id)?)
    }

    pub fn cache_thumbnails(&self, ids: Vec<String>) -> Option<Vec<ThumbnailUpdate>> {
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
        {
            let mut pending = self.thumbnails.lock();
            for id in &fresh {
                pending.remove(id);
            }
        }
        Some(data)
    }

    /// # Errors
    /// Returns an error when the beets config or the library database cannot be read.
    pub fn library(&self, config: Option<&Path>) -> Result<LibraryInfo> {
        let config = config.map_or_else(|| self.paths().beets.clone(), Path::to_path_buf);
        let (_, paths) = beets::load_paths(Some(&config), json!({}))?;
        library_info::read(&paths.library, &paths.directory)
    }

    #[must_use]
    pub fn services(&self) -> Value {
        json!({"services": setup::check_services(self.paths())})
    }

    /// # Errors
    /// Returns an error when the config file cannot be read.
    pub fn soulseek_account(&self) -> Result<Value> {
        setup::soulseek_account(&self.paths().config_file())
    }

    /// # Errors
    /// Returns an error when the form is invalid or the account cannot be saved.
    pub fn save_soulseek(&self, form: &SoulseekForm) -> Result<Value> {
        let path = self.paths().config_file();
        let username = required("username", &form.username)?;
        let server_port = form
            .server_port
            .trim()
            .parse()
            .map_err(|_| Error::from("Enter a server port from 1 to 65535."))?;
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

    #[must_use]
    pub fn bandcamp(&self) -> Value {
        bandcamp::status(self.paths())
    }

    /// # Errors
    /// Returns an error when the login cannot be saved.
    pub fn save_bandcamp(&self, user: &str, cookies: &str) -> Result<Value> {
        let _write = self.writes.lock();
        bandcamp::Login::save(self.paths(), Some(user), cookies)?;
        crate::watchlist::ensure_sources(self.paths())?;
        Ok(bandcamp::status(self.paths()))
    }

    /// # Errors
    /// Returns an error when the login cannot be removed.
    pub fn logout_bandcamp(&self) -> Result<Value> {
        let _write = self.writes.lock();
        bandcamp::Login::clear(self.paths())?;
        Ok(bandcamp::status(self.paths()))
    }

    /// # Errors
    /// Returns an error when the Spotify settings cannot be read.
    pub fn spotify_status(&self) -> Result<SpotifyStatus> {
        Ok(serde_json::from_value(spotify::status(
            &self.paths().config_file(),
            &self.paths().spotify_token(),
        )?)?)
    }

    /// # Errors
    /// Returns an error when the playlists cannot be fetched.
    pub fn spotify_playlists(&self) -> Result<Vec<spotify::PlaylistRef>> {
        Ok(spotify::list_playlists(
            &self.paths().config_file(),
            &self.paths().spotify_token(),
        )?)
    }

    /// # Errors
    /// Returns an error when the client ID cannot be saved.
    pub fn set_spotify_client_id(&self, client_id: &str) -> Result<String> {
        let _write = self.writes.lock();
        Ok(spotify::set_client_id(
            &self.paths().config_file(),
            client_id,
        )?)
    }

    /// # Errors
    /// Returns an error when the saved tokens cannot be removed.
    pub fn spotify_logout(&self) -> Result<bool> {
        let _write = self.writes.lock();
        Ok(spotify::clear_tokens(&self.paths().spotify_token())?)
    }

    /// # Errors
    /// Returns an error when a Spotify login is already active.
    pub fn spotify_login(&self) -> Result<SpotifyLogin> {
        let mut slot = self.login.lock();
        if slot.is_some() {
            return Err("A Spotify login is already active.".into());
        }
        let number = self.logins.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let job_id = format!("spotify-login-{number}");
        let cancel = Arc::new(AtomicBool::new(false));
        *slot = Some(ActiveLogin {
            job_id: job_id.clone(),
            cancel: Arc::clone(&cancel),
        });
        drop(slot);
        Ok(SpotifyLogin {
            job_id,
            cancel,
            config: self.paths().config_file(),
            token: self.paths().spotify_token(),
            slot: Arc::clone(&self.login),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ActiveLogin, ItemRequest, Service};
    use crate::AppEvent;
    use muzik_core::paths::Paths;
    use muzik_store::watchlist::{ItemAction, ItemId};
    use serde_json::json;
    use std::collections::{HashSet, VecDeque};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

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
            if let Some(index) = self.skipped.iter().position(&wanted)
                && let Some(event) = self.skipped.remove(index)
            {
                return Ok(event);
            }
            let deadline = Instant::now()
                .checked_add(timeout)
                .ok_or("timeout is too long")?;
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
    ) -> Result<(tempfile::TempDir, Service, Events), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let (sender, receiver) = mpsc::channel();
        let service = Service::with(
            Paths::under(state.path()),
            run,
            Arc::new(move |event| {
                let _ = sender.send(event);
            }),
        )?;
        let events = Events {
            receiver,
            skipped: VecDeque::new(),
        };
        Ok((state, service, events))
    }

    fn fixture_import(
        dir: &std::path::Path,
    ) -> Result<(PathBuf, PathBuf, PathBuf), Box<dyn std::error::Error>> {
        let audio = dir.join("track.flac");
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../muzik-tags/tests/fixtures/blank.flac"),
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
    fn invalid_requests_are_rejected() {
        let (_state, backend, _) = started(true).unwrap();
        assert!(backend.start_workflow(&json!({"raw":"  "})).is_err());
        assert!(backend.reply("", json!("as_is")).is_err());
        assert!(backend.reply("missing", json!("as_is")).is_err());
        assert!(backend.cancel("").is_err());
        assert!(backend.cancel("queue-999").is_err());
    }

    #[test]
    fn local_workflow_runs_as_a_queue_job() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("track.flac");
        fs::write(&audio, b"audio").unwrap();
        let (_state, backend, mut events) = started(true).unwrap();
        let job = backend
            .start_workflow(&json!({"raw":audio,"no_organize":true,"no_split":true,"dry_run":true}))
            .unwrap();
        assert!(job.starts_with("queue-"));
        events
            .job(&job, |event| matches!(event, AppEvent::JobStarted { .. }))
            .unwrap();
        let AppEvent::JobCompleted { result, .. } = events
            .job(&job, |event| matches!(event, AppEvent::JobCompleted { .. }))
            .unwrap()
        else {
            panic!("the job did not complete");
        };
        assert_eq!(result["singles"], 1);
        assert!(audio.exists());
    }

    #[test]
    fn two_workflow_runs_share_the_import_gate() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let (audio_one, database_one, config_one) = fixture_import(first.path()).unwrap();
        let (audio_two, database_two, config_two) = fixture_import(second.path()).unwrap();
        let (_state, backend, mut events) = started(true).unwrap();
        let mut jobs = Vec::new();
        for (audio, config) in [(&audio_one, &config_one), (&audio_two, &config_two)] {
            jobs.push(
                backend
                    .start_workflow(
                        &json!({"raw":audio,"config":config,"no_split":true,"interactive":true}),
                    )
                    .unwrap(),
            );
        }
        let mut asked = HashSet::new();
        let mut done = 0;
        while done < 2 {
            match events.next(Duration::from_secs(20), |_| true).unwrap() {
                AppEvent::DecisionRequest {
                    job_id,
                    decision_id,
                    kind,
                    ..
                } => {
                    assert_eq!(kind, muzik_core::DecisionKind::ImportMatch);
                    asked.insert(job_id);
                    backend.reply(&decision_id, json!("as_is")).unwrap();
                }
                AppEvent::JobCompleted { job_id, .. } if jobs.contains(&job_id) => done += 1,
                AppEvent::JobFailed { message, .. } => {
                    panic!("job failed: {message}");
                }
                _ => {}
            }
        }
        assert_eq!(asked.len(), 2);
        assert!(database_one.exists() && database_two.exists());
        assert!(!audio_one.exists() && !audio_two.exists());
    }

    #[test]
    fn cancel_ends_a_pending_local_import_decision() {
        let dir = tempfile::tempdir().unwrap();
        let (audio, database, config) = fixture_import(dir.path()).unwrap();
        let (_state, backend, mut events) = started(true).unwrap();
        let job = backend
            .start_workflow(&json!({"raw":audio,"config":config,"no_split":true}))
            .unwrap();
        let AppEvent::DecisionRequest { decision_id, .. } = events
            .job(&job, |event| {
                matches!(event, AppEvent::DecisionRequest { .. })
            })
            .unwrap()
        else {
            panic!("no decision request");
        };
        backend.cancel(&job).unwrap();
        events
            .job(&job, |event| matches!(event, AppEvent::JobCancelled { .. }))
            .unwrap();
        assert!(audio.exists());
        assert!(!database.exists());
        assert!(backend.reply(&decision_id, json!("as_is")).is_err());
    }

    #[test]
    fn cancel_removes_a_queued_item_job() {
        let (_state, backend, _) = started(false).unwrap();
        let item = ItemRequest {
            id: ItemId::new("PL1", Some("abcdefghijk")),
            title: "Song".into(),
            action: ItemAction::Run,
        };
        let job = backend.run_item(&item).unwrap();
        assert!(backend.run_item(&item).is_err());
        let listed = backend.jobs();
        assert_eq!(listed["open"][0]["job_id"], job);
        assert_eq!(listed["runner"], false);
        backend.cancel(&job).unwrap();
        assert_eq!(backend.jobs()["open"], json!([]));
    }

    #[test]
    fn thumbnails_already_in_progress_are_not_fetched_again() {
        let (_state, backend, _) = started(true).unwrap();
        assert!(backend.cache_thumbnails(Vec::new()).is_none());
        backend.thumbnails.lock().insert("abcdefghijk".into());
        assert!(
            backend
                .cache_thumbnails(vec!["abcdefghijk".into(), "abcdefghijk".into()])
                .is_none()
        );
    }

    #[test]
    fn spotify_login_uses_its_own_slot() {
        let (_state, backend, _) = started(true).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        *backend.login.lock() = Some(ActiveLogin {
            job_id: "spotify-login-test".into(),
            cancel: Arc::clone(&cancel),
        });
        assert!(backend.spotify_login().is_err());
        backend.cancel("spotify-login-test").unwrap();
        assert!(cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn watchlist_load_returns_saved_cards_before_local_check() {
        let (_state, backend, mut events) = started(true).unwrap();
        let playlist = backend
            .add_source("https://www.youtube.com/playlist?list=PLnative123")
            .unwrap();
        assert_eq!(playlist.playlist_id, "PLnative123");
        let (saved, check) = backend.load_watchlist().unwrap();
        assert_eq!(saved.playlists[0].playlist_id, "PLnative123");
        check.run();
        let AppEvent::WatchlistUpdated(checked) = events
            .next(Duration::from_secs(5), |event| {
                matches!(event, AppEvent::WatchlistUpdated(_))
            })
            .unwrap()
        else {
            panic!("no checked watchlist");
        };
        assert_eq!(checked.playlists[0].playlist_id, "PLnative123");
    }

    #[test]
    fn watchlist_edits_go_through_the_app() {
        let (_state, backend, _) = started(true).unwrap();
        backend
            .add_source("https://www.youtube.com/playlist?list=PL123")
            .unwrap();
        assert!(backend.rename_source("PL123", "  New name  ").unwrap());
        assert!(backend.rename_source("PL123", "").is_err());
        assert!(backend.remove_source("PL123").unwrap());
    }

    #[test]
    fn playlist_files_follow_the_setting_and_the_source_switch() {
        let (_state, backend, _) = started(false).unwrap();
        let beets = backend.paths().beets.clone();
        fs::create_dir_all(beets.parent().unwrap()).unwrap();
        fs::write(&beets, "directory: music\nlibrary: library.db\n").unwrap();
        let file = beets.parent().unwrap().join("music/Playlists/Mix.m3u8");
        backend
            .add_source("https://www.youtube.com/playlist?list=PL123")
            .unwrap();
        backend.rename_source("PL123", "Mix").unwrap();
        assert!(!file.exists());
        let mut defaults = backend.defaults();
        defaults.playlists = true;
        backend.save_defaults(&defaults).unwrap();
        assert!(file.is_file());
        assert!(backend.set_source_playlist("PL123", false).unwrap());
        assert!(!file.exists());
        assert!(backend.set_source_playlist("PL123", true).unwrap());
        assert!(file.is_file());
        defaults.playlists = false;
        backend.save_defaults(&defaults).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn spotify_status_reads_the_saved_client_id_and_redirect() {
        let (_state, backend, _) = started(false).unwrap();
        let config = backend.paths().config_file();
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            &config,
            "spotify:\n  client_id: saved\n  redirect_port: '9123'\n",
        )
        .unwrap();
        let status = backend.spotify_status().unwrap();
        assert_eq!(status.client_id, "saved");
        assert_eq!(status.redirect_uri, "http://127.0.0.1:9123/callback");
        assert!(!status.connected);
        assert!(status.account_name.is_none() && status.error.is_none());
    }
}
