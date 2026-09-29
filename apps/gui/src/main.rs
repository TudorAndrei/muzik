mod bridge;
mod local_workflow;
mod native;
mod native_watchlist;
mod pages;
mod remote_workflow;
mod services;
mod style;
mod thumbnails;
mod watchlist;
mod watchlist_view;

use bridge::Bridge;
use gpui_kit::component::button::*;
use gpui_kit::component::description_list::DescriptionList;
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants};
use gpui_kit::component::input::{Input, InputState, NumberInput};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::theme::Theme;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use muzik_core::{AudioFallback, AudioSource, MetadataSource, QualityPolicy};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Workflow,
    Watchlist,
    Library,
    Settings,
    Spotify,
}

const PAGES: [(Page, &str); 5] = [
    (Page::Workflow, "Workflow"),
    (Page::Watchlist, "Watchlist"),
    (Page::Library, "Library"),
    (Page::Settings, "Settings"),
    (Page::Spotify, "Spotify"),
];

struct Field {
    key: &'static str,
    label: &'static str,
    state: Entity<InputState>,
}
struct Choice {
    key: &'static str,
    label: &'static str,
    values: &'static [&'static str],
    selected: usize,
    state: Entity<SelectState<Vec<&'static str>>>,
}
struct ConfigSwitch {
    key: &'static str,
    label: &'static str,
    enabled: bool,
}

struct ChapterRow {
    index: Entity<InputState>,
    start: Entity<InputState>,
    end: Entity<InputState>,
    title: Entity<InputState>,
}

#[derive(Clone)]
struct PendingAction {
    title: String,
    description: &'static str,
    confirm: String,
    destructive: bool,
    command: &'static str,
    params: Value,
}

#[derive(Default)]
struct ActivityProgress {
    description: String,
    task_id: String,
    completed: f64,
    total: Option<f64>,
}

impl ActivityProgress {
    fn percentage(&self) -> f32 {
        self.total.filter(|total| *total > 0.).map_or(0., |total| {
            (self.completed / total * 100.).clamp(0., 100.) as f32
        })
    }
}

struct ActivitySection {
    title: &'static str,
    count: usize,
    rows: Vec<String>,
}

const CHOICES: &[(&str, &str, &[&str])] = &[
    ("audio_source", "Audio source", AudioSource::CHOICES),
    ("metadata_source", "Metadata", MetadataSource::CHOICES),
    (
        "prefer",
        "Prefer",
        muzik_core::config_choices::PREFERRED_AUDIO_CHOICES,
    ),
    ("fallback", "Fallback", AudioFallback::CHOICES),
    ("quality_policy", "Quality policy", QualityPolicy::CHOICES),
];
const SWITCHES: &[(&str, &str, bool)] = &[
    ("review", "Review chapters", false),
    ("no_split", "No split", false),
    ("no_organize", "No organize", false),
    ("import_", "Import", false),
    ("tag_only", "Tag only", false),
    ("dry_run", "Dry run", false),
    ("keep_source", "Keep source", false),
    ("force", "Force", false),
    ("interactive", "Interactive", true),
];
const FILTERS: &[&str] = &[
    "All",
    "Pending",
    "Processing",
    "Failed",
    "Processed",
    "Unavailable",
];
const WATCH_PAGE_SIZE: usize = 8;
const REPLACE_WARNING: &str =
    "This replaces the files from this stage. Later stages can become stale.";
const ITEM_ACTIONS: &[(&str, &str)] = &[
    ("run", "Run"),
    ("retry", "Retry"),
    ("download_again", "Download again"),
    ("check_quality_again", "Check quality again"),
    ("parse_again", "Parse again"),
    ("split_again", "Split again"),
    ("organize_again", "Organize again"),
    ("run_all_again", "Run all again"),
];

struct Muzik {
    page: Page,
    raw: Entity<InputState>,
    config_view: Option<Entity<ConfigView>>,
    watch_url: Entity<InputState>,
    playlist_name: Entity<InputState>,
    spotify_client_id: Entity<InputState>,
    chapter_rows: Vec<ChapterRow>,
    logo: Arc<Image>,
    bridge: Option<Bridge>,
    pending: HashMap<String, String>,
    latest_reads: HashMap<String, String>,
    status: String,
    error: Option<String>,
    job_id: Option<String>,
    job_kind: Option<String>,
    completed_jobs: HashSet<String>,
    job_status: String,
    progress: String,
    progress_state: ActivityProgress,
    activity_sections: Vec<ActivitySection>,
    logs: Vec<String>,
    decision: Option<Value>,
    watchlist: Value,
    selected_playlist: usize,
    filter: usize,
    watch_page: usize,
    thumbnail_attempted: HashSet<String>,
    library: Value,
    services: Value,
    spotify: Value,
    defaults: Value,
    config_status: Rc<RefCell<String>>,
}

impl Muzik {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_bridge(window, cx, true)
    }

    fn new_with_bridge(window: &mut Window, cx: &mut Context<Self>, start_bridge: bool) -> Self {
        let mut this = Self {
            page: Page::Workflow,
            raw: cx.new(|cx| InputState::new(window, cx).placeholder("URL or path")),
            config_view: None,
            watch_url: cx.new(|cx| InputState::new(window, cx).placeholder("Playlist URL")),
            playlist_name: cx.new(|cx| InputState::new(window, cx).placeholder("Playlist name")),
            spotify_client_id: cx
                .new(|cx| InputState::new(window, cx).placeholder("Spotify client ID")),
            chapter_rows: Vec::new(),
            logo: style::logo(),
            bridge: None,
            pending: HashMap::new(),
            latest_reads: HashMap::new(),
            status: String::new(),
            error: None,
            job_id: None,
            job_kind: None,
            completed_jobs: HashSet::new(),
            job_status: "Ready".into(),
            progress: String::new(),
            progress_state: ActivityProgress::default(),
            activity_sections: Vec::new(),
            logs: Vec::new(),
            decision: None,
            watchlist: Value::Null,
            selected_playlist: 0,
            filter: 0,
            watch_page: 0,
            thumbnail_attempted: HashSet::new(),
            library: Value::Null,
            services: Value::Null,
            spotify: Value::Null,
            defaults: Value::Null,
            config_status: Rc::new(RefCell::new(String::new())),
        };
        if start_bridge {
            match Bridge::start() {
                Ok(bridge) => {
                    this.bridge = Some(bridge);
                    this.send("hello", json!({}));
                }
                Err(error) => this.status = error,
            }
        }
        cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        })
        .detach();
        cx.spawn_in(window, async move |weak, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(80))
                .await;
            if weak
                .update_in(cx, |view, window, cx| view.drain(window, cx))
                .is_err()
            {
                break;
            }
        })
        .detach();
        this
    }

    fn send(&mut self, command: &str, params: Value) {
        if matches!(
            command,
            "watchlist.add" | "watchlist.rename" | "watchlist.remove"
        ) && (self.job_kind.is_some() || self.job_id.is_some())
        {
            self.status = "A job is already active".into();
            self.error = Some(self.status.clone());
            return;
        }
        if matches!(
            command,
            "spotify.set_client_id" | "spotify.logout" | "spotify.login"
        ) {
            self.latest_reads.remove("spotify.status");
            self.latest_reads.remove("spotify.playlists");
            if let Some(spotify) = self.spotify.as_object_mut() {
                spotify.remove("playlists");
            }
        } else if command == "spotify.status" {
            self.latest_reads.remove("spotify.playlists");
        }
        match self
            .bridge
            .as_mut()
            .map(|bridge| bridge.send(command, params))
        {
            Some(Ok(id)) => {
                if matches!(
                    command,
                    "library.scan"
                        | "services.check"
                        | "spotify.status"
                        | "spotify.playlists"
                        | "watchlist.load"
                ) {
                    self.latest_reads.insert(command.into(), id.clone());
                }
                self.pending.insert(id, command.into());
                self.status = format!("{command} requested");
            }
            Some(Err(error)) => self.status = error,
            None => self.status = "Backend is not available".into(),
        }
    }

    fn launcher_params(&self, cx: &App) -> Value {
        let mut params = self.defaults.as_object().cloned().unwrap_or_default();
        params.insert("raw".into(), json!(self.raw.read(cx).value().to_string()));
        Value::Object(params)
    }

    fn apply_defaults(&mut self, defaults: Value, cx: &mut Context<Self>) {
        self.defaults = defaults;
        cx.notify();
    }

    fn open_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.defaults.is_object() {
            self.status = "Config is loading".into();
            cx.notify();
            return;
        }
        if self.config_view.is_none() {
            let main = cx.entity();
            let defaults = self.defaults.clone();
            let status = self.config_status.clone();
            self.config_view =
                Some(cx.new(|cx| ConfigView::new(main, defaults, status, window, cx)));
        }
        self.page = Page::Settings;
        self.error = None;
        self.send("services.check", json!({}));
        cx.notify();
    }

    fn set_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if page == Page::Settings {
            self.open_config(window, cx);
            return;
        }
        self.page = page;
        self.error = None;
        match page {
            Page::Watchlist => self.send("watchlist.load", self.launcher_params(cx)),
            Page::Library => self.scan_library(cx),
            Page::Spotify => {
                self.send("watchlist.load", self.launcher_params(cx));
                self.send("spotify.status", json!({}));
            }
            Page::Workflow | Page::Settings => {}
        }
        cx.notify();
    }

    fn scan_library(&mut self, _cx: &App) {
        let output = self.defaults["output"].as_str().unwrap_or("").to_string();
        self.send("library.scan", json!({"output":output}));
    }

    fn start_job(&mut self, command: &str, params: Value, cx: &mut Context<Self>) {
        if self.job_kind.is_some() || self.job_id.is_some() {
            self.status = "A job is already active".into();
            cx.notify();
            return;
        }
        self.error = None;
        self.job_status = "Starting".into();
        self.job_kind = Some(command.to_string());
        self.progress.clear();
        self.progress_state = ActivityProgress::default();
        self.activity_sections.clear();
        self.logs.clear();
        self.decision = None;
        self.send(command, params);
        cx.notify();
    }

    fn record_job_event(&mut self, kind: &str, payload: &Value) {
        let line = match kind {
            "progress_started" => {
                let progress = &mut self.progress_state;
                progress.task_id = describe(&payload["task_id"]);
                progress.description = describe(&payload["description"]);
                progress.completed = 0.;
                progress.total = payload["total"].as_f64().filter(|total| *total > 0.);
                self.job_status = progress.description.clone();
                progress.description.clone()
            }
            "progress_advanced" => {
                let progress = &mut self.progress_state;
                if payload["task_id"].as_str() != Some(progress.task_id.as_str()) {
                    return;
                }
                if let Some(total) = payload["total"].as_f64().filter(|total| *total > 0.) {
                    progress.total = Some(total);
                }
                progress.completed = payload["completed"]
                    .as_f64()
                    .unwrap_or(progress.completed + payload["advance"].as_f64().unwrap_or(1.));
                String::new()
            }
            "progress_finished" => {
                let progress = &mut self.progress_state;
                if payload["task_id"].as_str() != Some(progress.task_id.as_str()) {
                    return;
                }
                if let Some(total) = progress.total {
                    progress.completed = total;
                }
                format!("{} finished", progress.description)
            }
            "step_started" => {
                self.job_status = describe(&payload["name"]);
                format!("Started {}", self.job_status)
            }
            "step_finished" => {
                let progress = &mut self.progress_state;
                if progress.total.is_some() {
                    progress.completed += 1.;
                }
                format!(
                    "{} {}",
                    describe(&payload["name"]),
                    if payload["success"] == false {
                        "failed"
                    } else {
                        "finished"
                    }
                )
            }
            "message" | "log" => {
                let message = describe(&payload["message"]);
                self.job_status = message.clone();
                message
            }
            "error" => {
                let message = describe(&payload["message"]);
                self.error = Some(message.clone());
                format!("Error: {message}")
            }
            "candidates_found" => {
                self.set_activity_section(activity_section(
                    "Source candidates",
                    &payload["candidates"],
                    candidate_summary,
                ));
                format!(
                    "{} {} candidates found",
                    self.activity_sections
                        .iter()
                        .find(|section| section.title == "Source candidates")
                        .map_or(0, |section| section.count),
                    payload["source"].as_str().unwrap_or("source")
                )
            }
            "chapter_review_requested" => {
                self.set_activity_section(activity_section(
                    "Chapters",
                    &payload["chapters"],
                    chapter_summary,
                ));
                format!("Chapter review: {}", describe(&payload["source"]))
            }
            "task" => {
                self.set_activity_section(activity_section(
                    "Album matches",
                    &payload["task"]["matches"],
                    import_match_summary,
                ));
                let task = &payload["task"];
                let message = format!(
                    "Import: {} · {}",
                    task["current_artist"].as_str().unwrap_or("Unknown artist"),
                    task["current_album"].as_str().unwrap_or("Unknown album")
                );
                self.job_status = message.clone();
                message
            }
            "import_started" => {
                self.job_status = "Import started".into();
                self.job_status.clone()
            }
            "import_finished" => {
                self.job_status = if payload["success"] == false {
                    "Import failed".into()
                } else {
                    "Import finished".into()
                };
                self.job_status.clone()
            }
            _ => kind.replace('_', " "),
        };
        self.progress = match self.progress_state.total {
            Some(total) => format!("{:.0} / {:.0}", self.progress_state.completed, total),
            None if !self.progress_state.description.is_empty() => {
                format!("{:.0} complete", self.progress_state.completed)
            }
            None => String::new(),
        };
        if !line.is_empty() {
            self.logs.push(short_text(&line, 180));
        }
    }

    fn set_activity_section(&mut self, section: ActivitySection) {
        if let Some(existing) = self
            .activity_sections
            .iter_mut()
            .find(|existing| existing.title == section.title)
        {
            *existing = section;
        } else {
            self.activity_sections.push(section);
        }
    }

    fn request_action(
        &mut self,
        action: PendingAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let action = action.clone();
            dialog
                .title(action.title.clone())
                .description(action.description)
                .show_cancel(true)
                .cancel_text("Cancel")
                .ok_text(action.confirm.clone())
                .ok_variant(if action.destructive {
                    ButtonVariant::Danger
                } else {
                    ButtonVariant::Primary
                })
                .on_ok(move |_, _, cx| {
                    let action = action.clone();
                    let _ = view.update(cx, |view, cx| view.run_action(action, cx));
                    true
                })
        });
    }

    fn run_action(&mut self, action: PendingAction, cx: &mut Context<Self>) {
        if action.command == "watchlist.action" {
            self.start_job(action.command, action.params, cx);
        } else {
            self.send(action.command, action.params);
            cx.notify();
        }
    }

    fn pick_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.into_iter().next() {
                    let value = path.to_string_lossy().into_owned();
                    let _ = view.update_in(cx, |view, window, cx| {
                        view.raw
                            .update(cx, |state, cx| state.set_value(value, window, cx));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn drain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let messages = self.bridge.as_ref().map(Bridge::drain).unwrap_or_default();
        if messages.is_empty() {
            return;
        }
        for message in messages {
            self.message(message, window, cx);
        }
        cx.notify();
    }

    fn message(&mut self, message: Value, window: &mut Window, _cx: &mut Context<Self>) {
        match message["type"].as_str().unwrap_or_default() {
            "response" => {
                let id = message["id"].as_str().unwrap_or_default();
                let command = self.pending.remove(id).unwrap_or_default();
                if matches!(
                    command.as_str(),
                    "library.scan"
                        | "services.check"
                        | "spotify.status"
                        | "spotify.playlists"
                        | "watchlist.load"
                ) {
                    if self.latest_reads.get(&command).map(String::as_str) != Some(id) {
                        return;
                    }
                    self.latest_reads.remove(&command);
                }
                if !message["ok"].as_bool().unwrap_or(false) {
                    self.status = message["error"]["message"]
                        .as_str()
                        .unwrap_or("Request failed")
                        .into();
                    self.error = Some(self.status.clone());
                    if command == "config.save" {
                        *self.config_status.borrow_mut() = self.status.clone();
                    }
                    if self.job_kind.as_deref() == Some(command.as_str()) {
                        self.job_status = self.status.clone();
                        self.job_kind = None;
                    }
                    return;
                }
                let result = &message["result"];
                match command.as_str() {
                    "hello" | "config.get" | "config.save" => {
                        self.status = "Backend ready".into();
                        if command == "config.save" {
                            self.status = "Config saved".into();
                            self.error = None;
                            *self.config_status.borrow_mut() = self.status.clone();
                        }
                        self.apply_defaults(result["defaults"].clone(), _cx);
                    }
                    "watchlist.load" => {
                        self.replace_watchlist(result["watchlist"].clone(), window, _cx);
                        self.cache_visible_thumbnails(_cx);
                        self.status = "Watchlist ready".into();
                    }
                    "watchlist.add" | "watchlist.remove" | "watchlist.rename" => {
                        self.send("watchlist.load", self.launcher_params(_cx));
                    }
                    "library.scan" => {
                        self.library = result.clone();
                        self.status = "Library ready".into();
                    }
                    "services.check" => {
                        self.services = result.clone();
                        self.status = "Services checked".into();
                    }
                    "spotify.status"
                    | "spotify.set_client_id"
                    | "spotify.logout"
                    | "spotify.playlists" => {
                        if command == "spotify.status" {
                            self.spotify = result.clone();
                            if self.spotify_client_id.read(_cx).value().is_empty() {
                                if let Some(client_id) = result["client_id"].as_str() {
                                    self.spotify_client_id.update(_cx, |state, cx| {
                                        state.set_value(client_id.to_string(), window, cx)
                                    });
                                }
                            }
                            if self.spotify["connected"] == true {
                                self.send("spotify.playlists", json!({}));
                            }
                        } else if command == "spotify.playlists" {
                            if self.spotify["connected"] == true {
                                self.spotify["playlists"] = result["playlists"].clone();
                            }
                        } else {
                            self.send("spotify.status", json!({}));
                        }
                        self.status = "Spotify ready".into();
                    }
                    "workflow.start" | "watchlist.refresh" | "watchlist.action"
                    | "spotify.login" => {
                        if let Some(id) = result["job_id"].as_str() {
                            if !self.completed_jobs.remove(id) {
                                self.job_id = Some(id.to_string());
                                self.job_status = "Working".into();
                            }
                        }
                    }
                    _ => {
                        self.status = format!("{command} complete");
                    }
                }
            }
            "event" => {
                let event = message["event"].as_str().unwrap_or_default();
                let data = &message["data"];
                match event {
                    "watchlist.updated" => {
                        self.replace_watchlist(data["watchlist"].clone(), window, _cx);
                        self.cache_visible_thumbnails(_cx);
                        self.status = "Watchlist updated".into();
                    }
                    "thumbnails.updated" => self.merge_thumbnail_results(data),
                    "watchlist.error" => {
                        self.error = Some(
                            data["message"]
                                .as_str()
                                .unwrap_or("Watchlist check failed")
                                .into(),
                        );
                    }
                    "job.event" => {
                        let kind = data["event"].as_str().unwrap_or("Update");
                        let payload = &data["data"];
                        self.record_job_event(kind, payload);
                        if self.logs.len() > 300 {
                            self.logs.drain(..100);
                        }
                    }
                    "decision.request" => {
                        self.chapter_rows.clear();
                        if data["kind"] == "chapter_edit" {
                            if let Some(chapters) = data["payload"]["chapters"].as_array() {
                                for chapter in chapters {
                                    let mut make = |key: &'static str| {
                                        let value = describe(&chapter[key]);
                                        _cx.new(|cx| {
                                            InputState::new(window, cx)
                                                .placeholder(key)
                                                .default_value(value)
                                        })
                                    };
                                    self.chapter_rows.push(ChapterRow {
                                        index: make("index"),
                                        start: make("start"),
                                        end: make("end"),
                                        title: make("title"),
                                    });
                                }
                            }
                        }
                        self.decision = Some(data.clone());
                        self.job_status = "Decision needed".into();
                    }
                    "job.completed" | "job.failed" | "job.cancelled" => {
                        if self.pending.values().any(|command| {
                            matches!(
                                command.as_str(),
                                "workflow.start"
                                    | "watchlist.refresh"
                                    | "watchlist.action"
                                    | "spotify.login"
                            )
                        }) {
                            if let Some(id) = data["job_id"].as_str() {
                                self.completed_jobs.insert(id.to_string());
                            }
                        }
                        self.job_id = None;
                        self.job_status = event.trim_start_matches("job.").into();
                        let failure = if event == "job.failed" {
                            Some(
                                data["error"]["message"]
                                    .as_str()
                                    .unwrap_or("Job failed")
                                    .to_string(),
                            )
                        } else {
                            None
                        };
                        self.logs.push(format!("Job {}", self.job_status));
                        let job = job_label(self.job_kind.as_deref());
                        let note = match (&failure, event) {
                            (Some(failure), _) => {
                                Notification::error(failure.clone()).title(format!("{job} failed"))
                            }
                            (None, "job.cancelled") => {
                                Notification::warning(format!("{job} cancelled"))
                            }
                            (None, _) => Notification::success(format!("{job} finished")),
                        };
                        window.push_notification(note, _cx);
                        if self.job_kind.as_deref() == Some("spotify.login") {
                            self.send("spotify.status", json!({}));
                            if event == "job.completed" {
                                self.send("spotify.playlists", json!({}));
                            }
                        } else {
                            self.send("watchlist.load", self.launcher_params(_cx));
                        }
                        self.job_kind = None;
                        if let Some(failure) = failure {
                            self.error = Some(failure);
                        }
                    }
                    _ => self.logs.push(format!("{event}: {}", describe(data))),
                }
            }
            "transport.error" | "transport.closed" => {
                self.status = message["message"]
                    .as_str()
                    .unwrap_or("Rust service stopped")
                    .into();
            }
            _ => {}
        }
    }

    fn replace_watchlist(&mut self, incoming: Value, window: &mut Window, cx: &mut Context<Self>) {
        let old_id = self.watchlist["playlists"][self.selected_playlist]["playlist_id"]
            .as_str()
            .map(str::to_owned);
        let old_title = self.watchlist["playlists"][self.selected_playlist]["title"]
            .as_str()
            .unwrap_or("");
        let playlists = incoming["playlists"].as_array();
        let count = playlists.map_or(0, Vec::len);
        let selected = old_id
            .as_deref()
            .and_then(|id| {
                playlists?
                    .iter()
                    .position(|playlist| playlist["playlist_id"] == id)
            })
            .unwrap_or_else(|| self.selected_playlist.min(count.saturating_sub(1)));
        let page_count = playlists
            .and_then(|all| all.get(selected))
            .map(|playlist| watch_page_count(playlist, self.filter))
            .unwrap_or(1);
        self.selected_playlist = selected;
        self.watch_page = self.watch_page.min(page_count - 1);
        let name = playlists
            .and_then(|all| all.get(selected))
            .and_then(|playlist| playlist["title"].as_str())
            .unwrap_or("");
        let selected_id = playlists
            .and_then(|all| all.get(selected))
            .and_then(|playlist| playlist["playlist_id"].as_str());
        if old_id.as_deref() != selected_id || old_title != name {
            self.playlist_name.update(cx, |state, cx| {
                state.set_value(name.to_string(), window, cx)
            });
        }
        self.watchlist = incoming;
    }

    fn visible_thumbnail_ids(&self) -> Vec<String> {
        self.watchlist["playlists"][self.selected_playlist]["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| watchlist_view::matches_filter(item, self.filter))
            .skip(self.watch_page * WATCH_PAGE_SIZE)
            .take(WATCH_PAGE_SIZE)
            .filter(|item| item["thumbnail_path"].is_null())
            .filter(|item| item["thumbnail_url"].is_string())
            .filter_map(|item| item["video_id"].as_str().map(str::to_owned))
            .collect()
    }

    fn cache_visible_thumbnails(&mut self, cx: &mut Context<Self>) {
        if self.page != Page::Watchlist {
            return;
        }
        let video_ids: Vec<String> = self
            .visible_thumbnail_ids()
            .into_iter()
            .filter(|id| !self.thumbnail_attempted.contains(id))
            .collect();
        if video_ids.is_empty() {
            return;
        }
        self.thumbnail_attempted.extend(video_ids.iter().cloned());
        self.send("thumbnails.cache", json!({"video_ids":video_ids}));
        cx.notify();
    }

    fn merge_thumbnail_results(&mut self, data: &Value) {
        let visible: HashSet<String> = self.visible_thumbnail_ids().into_iter().collect();
        merge_thumbnail_paths(&mut self.watchlist, &visible, data);
    }

    fn reply(&mut self, value: Value, cx: &mut Context<Self>) {
        if let Some(decision) = self.decision.take() {
            self.chapter_rows.clear();
            self.send(
                "decision.reply",
                json!({"decision_id":decision["decision_id"],"value":value}),
            );
            self.job_status = "Working".into();
            cx.notify();
        }
    }

    fn submit_chapters(&mut self, cx: &mut Context<Self>) {
        let mut chapters = Vec::new();
        for (row_number, row) in self.chapter_rows.iter().enumerate() {
            let index = row.index.read(cx).value().parse::<u64>();
            let start = row.start.read(cx).value().parse::<u64>();
            let end_text = row.end.read(cx).value().to_string();
            let end = if end_text.trim().is_empty() {
                Ok(None)
            } else {
                end_text.parse::<u64>().map(Some)
            };
            let title = row.title.read(cx).value().to_string();
            match (index, start, end) {
                (Ok(index), Ok(start), Ok(end))
                    if index > 0
                        && end.is_none_or(|end| end > start)
                        && !title.trim().is_empty() =>
                {
                    chapters.push(json!({"index":index,"start":start,"end":end,"title":title}));
                }
                _ => {
                    self.status = format!(
                        "Check chapter {}: index, start, end, and title",
                        row_number + 1
                    );
                    cx.notify();
                    return;
                }
            }
        }
        self.reply(Value::Array(chapters), cx);
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = PAGES
            .iter()
            .position(|(page, _)| *page == self.page)
            .unwrap_or(0);
        div()
            .flex()
            .items_center()
            .gap_8()
            .px_6()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(img(self.logo.clone()).size(px(24.)))
                    .child(div().text_lg().font_bold().child("muzik")),
            )
            .child(
                TabBar::new("pages")
                    .segmented()
                    .selected_index(selected)
                    .children(PAGES.iter().map(|(_, label)| Tab::new().label(*label)))
                    .on_click(cx.listener(|view, index: &usize, window, cx| {
                        if let Some((page, _)) = PAGES.get(*index) {
                            view.set_page(*page, window, cx);
                        }
                    })),
            )
            .into_any_element()
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let (text, color) = match &self.error {
            Some(error) => (error.clone(), cx.theme().danger),
            None if self.status.is_empty() => ("Ready".to_string(), cx.theme().muted_foreground),
            None => (self.status.clone(), cx.theme().muted_foreground),
        };
        let output = self.defaults["output"].as_str().unwrap_or("").to_string();
        StatusBar::new()
            .left(
                div()
                    .text_xs()
                    .text_color(color)
                    .child(short_text(&text, 160)),
            )
            .right(style::mono(output, cx).text_color(cx.theme().muted_foreground))
            .into_any_element()
    }

    fn workflow(&self, cx: &mut Context<Self>) -> AnyElement {
        let source =
            div()
                .v_flex()
                .gap_1()
                .child(div().text_sm().font_semibold().child("URL or path"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(&self.raw)))
                        .child(Button::new("pick-source").label("Choose…").on_click(
                            cx.listener(|view, _, window, cx| view.pick_source(window, cx)),
                        )),
                );
        let run = Button::new("run")
            .primary()
            .icon(IconName::Play)
            .label("Run workflow")
            .on_click(cx.listener(|view, _, _, cx| {
                let params = view.launcher_params(cx);
                if params["raw"].as_str().unwrap_or("").trim().is_empty() {
                    view.status = "Enter a URL or local path".into();
                    cx.notify();
                    return;
                }
                view.start_job("workflow.start", params, cx);
            }));
        let form = div()
            .v_flex()
            .gap_6()
            .w_full()
            .max_w(px(720.))
            .p_6()
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(style::page_title("Workflow"))
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child("Download, split, and organize audio."),
                    ),
            )
            .child(
                GroupBox::new()
                    .id("workflow-source")
                    .title("SOURCE")
                    .outline()
                    .child(source),
            )
            .child(div().flex().justify_end().child(run));
        div()
            .flex()
            .flex_row()
            .size_full()
            .child(
                div()
                    .flex()
                    .justify_center()
                    .flex_1()
                    .overflow_y_scrollbar()
                    .child(form),
            )
            .child(self.job_panel(cx))
            .into_any_element()
    }

    fn job_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let show_progress = self.job_kind.is_some() || !self.progress_state.description.is_empty();
        let mut panel = div()
            .v_flex()
            .gap_4()
            .p_4()
            .w(px(320.))
            .h_full()
            .flex_none()
            .overflow_y_scrollbar()
            .border_l_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar)
            .child(style::section_title("Activity"))
            .child(
                div()
                    .id("activity-status")
                    .v_flex()
                    .gap_2()
                    .child(style::meta(job_label(self.job_kind.as_deref()), cx))
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(self.job_status.clone()),
                    )
                    .when(show_progress, |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2p5()
                                .child(
                                    div().flex_1().child(
                                        Progress::new("activity-progress")
                                            .value(self.progress_state.percentage())
                                            .loading(
                                                self.progress_state.total.is_none()
                                                    && self.job_kind.is_some(),
                                            )
                                            .accessibility_label("Workflow progress"),
                                    ),
                                )
                                .child(style::mono(self.progress.clone(), cx)),
                        )
                    }),
            );
        if let Some(id) = &self.job_id {
            let id = id.clone();
            let job = job_label(self.job_kind.as_deref());
            panel = panel.child(
                div().flex().child(
                    Button::new("cancel-job")
                        .danger()
                        .small()
                        .label("Cancel job")
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.request_action(
                                PendingAction {
                                    title: format!("Cancel the {}?", job.to_lowercase()),
                                    description: "The job stops at a safe point. Finished files and saved state stay.",
                                    confirm: "Cancel job".into(),
                                    destructive: true,
                                    command: "job.cancel",
                                    params: json!({"job_id":id}),
                                },
                                window,
                                cx,
                            );
                        })),
                ),
            );
        }
        if let Some(decision) = &self.decision {
            let mut review = div()
                .v_flex()
                .gap_2()
                .max_h(px(440.))
                .overflow_y_scrollbar()
                .child(div().font_semibold().child(format!(
                    "Choose: {}",
                    decision["kind"].as_str().unwrap_or("decision")
                )));
            for detail in decision_details(decision) {
                review = review.child(div().text_sm().child(detail));
            }
            for (index, (label, value)) in decision_choices(decision).into_iter().enumerate() {
                review =
                    review.child(Button::new(("decision", index)).label(label).on_click(
                        cx.listener(move |view, _, _, cx| view.reply(value.clone(), cx)),
                    ));
            }
            if decision["kind"] == "chapter_edit" {
                let mut rows = div().v_flex().gap_2().max_h(px(300.));
                for (index, chapter) in self.chapter_rows.iter().enumerate() {
                    rows = rows.child(
                        div()
                            .id(("chapter", index))
                            .v_flex()
                            .gap_1()
                            .child(format!("Chapter {}", index + 1))
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child(Input::new(&chapter.index))
                                    .child(Input::new(&chapter.start))
                                    .child(Input::new(&chapter.end)),
                            )
                            .child(Input::new(&chapter.title)),
                    );
                }
                review = review
                    .child(div().text_sm().child(
                        "Index, start seconds, end seconds, title. Leave the last end blank.",
                    ))
                    .child(rows.overflow_y_scrollbar())
                    .child(
                        Button::new("apply-chapters")
                            .primary()
                            .label("Apply chapter edits")
                            .on_click(cx.listener(|view, _, _, cx| view.submit_chapters(cx))),
                    );
            }
            panel = panel.child(review);
        }
        for (index, section) in self.activity_sections.iter().enumerate() {
            let mut summary = div().v_flex().gap_1().child(
                div()
                    .text_sm()
                    .font_semibold()
                    .child(format!("{} ({})", section.title, section.count)),
            );
            if section.rows.is_empty() {
                summary = summary.child(div().text_sm().child("No results"));
            }
            for (row_index, row) in section.rows.iter().enumerate() {
                summary = summary.child(
                    div()
                        .id(("activity-row", index * 10 + row_index))
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(row.clone()),
                );
            }
            if section.count > section.rows.len() {
                summary = summary.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{} more", section.count - section.rows.len())),
                );
            }
            panel = panel.child(
                GroupBox::new()
                    .id(("activity-section", index))
                    .outline()
                    .child(summary),
            );
        }
        let mut log = div()
            .v_flex()
            .gap_1()
            .min_h(px(120.))
            .max_h(px(220.))
            .overflow_y_scrollbar();
        if self.logs.is_empty() {
            log = log.child(style::meta("Job updates will appear here.", cx));
        }
        for (index, line) in self.logs.iter().rev().take(100).enumerate() {
            log = log.child(style::mono(line.clone(), cx).id(("log", index)));
        }
        panel
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(style::overline("RECENT EVENTS", cx))
                    .child(log),
            )
            .into_any_element()
    }
}

struct ConfigView {
    main: WeakEntity<Muzik>,
    fields: Vec<Field>,
    choices: Vec<Choice>,
    switches: Vec<ConfigSwitch>,
    status: Rc<RefCell<String>>,
}

impl ConfigView {
    fn new(
        main: Entity<Muzik>,
        defaults: Value,
        status: Rc<RefCell<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&main, |_, _, cx| cx.notify()).detach();
        let fields = [
            ("output", "Downloads"),
            ("splits", "Splits"),
            ("config", "Beets config"),
            ("jobs", "Jobs"),
            ("min_bitrate", "Min bitrate"),
        ]
        .into_iter()
        .map(|(key, label)| {
            let value = match &defaults[key] {
                Value::String(value) => value.clone(),
                Value::Number(value) => value.to_string(),
                _ => String::new(),
            };
            Field {
                key,
                label,
                state: cx.new(|cx| {
                    let state = InputState::new(window, cx)
                        .placeholder(label)
                        .default_value(value);
                    match key {
                        "jobs" => state.step(1.).min(0.),
                        "min_bitrate" => state.step(32.).min(0.),
                        _ => state,
                    }
                }),
            }
        })
        .collect();
        let choices: Vec<Choice> = CHOICES
            .iter()
            .map(|(key, label, values)| {
                let selected = values
                    .iter()
                    .position(|value| Some(*value) == defaults[*key].as_str())
                    .unwrap_or(0);
                Choice {
                    key,
                    label,
                    values,
                    selected,
                    state: cx.new(|cx| {
                        SelectState::new(
                            values.to_vec(),
                            Some(IndexPath::new(selected)),
                            window,
                            cx,
                        )
                    }),
                }
            })
            .collect();
        for (index, choice) in choices.iter().enumerate() {
            cx.subscribe_in(&choice.state, window, move |view, _, event, _, cx| {
                let SelectEvent::Confirm(value) = event;
                if let Some(value) = value {
                    if let Some(selected) = view.choices[index]
                        .values
                        .iter()
                        .position(|item| item == value)
                    {
                        view.choices[index].selected = selected;
                        cx.notify();
                    }
                }
            })
            .detach();
        }
        let switches = SWITCHES
            .iter()
            .map(|(key, label, initial)| ConfigSwitch {
                key,
                label,
                enabled: defaults[*key].as_bool().unwrap_or(*initial),
            })
            .collect();
        Self {
            main: main.downgrade(),
            fields,
            choices,
            switches,
            status,
        }
    }

    fn pick_path(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let directories = matches!(self.fields[index].key, "output" | "splits");
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: !directories,
            directories,
            multiple: false,
            prompt: Some("Select".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.into_iter().next() {
                    let value = path.to_string_lossy().into_owned();
                    let _ = view.update_in(cx, |view, window, cx| {
                        view.fields[index]
                            .state
                            .update(cx, |state, cx| state.set_value(value, window, cx));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let mut params = Map::new();
        for field in &self.fields {
            let value = field.state.read(cx).value().to_string();
            if matches!(field.key, "jobs" | "min_bitrate") {
                let Ok(number) = value.parse::<u64>() else {
                    *self.status.borrow_mut() = format!("Enter a number for {}", field.label);
                    cx.notify();
                    return;
                };
                params.insert(field.key.into(), json!(number));
            } else {
                params.insert(field.key.into(), json!(value));
            }
        }
        for choice in &self.choices {
            params.insert(choice.key.into(), json!(choice.values[choice.selected]));
        }
        for switch in &self.switches {
            params.insert(switch.key.into(), json!(switch.enabled));
        }
        if let Some(main) = self.main.upgrade() {
            main.update(cx, |main, cx| {
                main.error = None;
                main.send("config.save", Value::Object(params));
                cx.notify();
            });
        }
        *self.status.borrow_mut() = "Saving config".into();
        cx.notify();
    }
}

impl Render for ConfigView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut destinations = div().v_flex().gap_3();
        let mut tuning = div().flex().flex_wrap().gap_4();
        for (index, field) in self.fields.iter().enumerate() {
            let numeric = matches!(field.key, "jobs" | "min_bitrate");
            let control = if numeric {
                NumberInput::new(&field.state).into_any_element()
            } else {
                Input::new(&field.state).into_any_element()
            };
            let mut row = div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().child(control));
            if matches!(field.key, "output" | "splits" | "config") {
                row = row.child(
                    Button::new(("config-pick", index))
                        .icon(IconName::FolderOpen)
                        .label("Choose…")
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.pick_path(index, window, cx)
                        })),
                );
            }
            let field_view = div()
                .v_flex()
                .gap_1()
                .child(div().text_sm().font_semibold().child(field.label))
                .child(row);
            if numeric {
                tuning = tuning.child(div().w(px(180.)).child(field_view));
            } else {
                destinations = destinations.child(field_view);
            }
        }
        let mut choices = div().flex().flex_wrap().gap_4();
        for choice in &self.choices {
            choices = choices.child(
                div()
                    .v_flex()
                    .gap_1()
                    .w(px(240.))
                    .child(div().text_sm().font_semibold().child(choice.label))
                    .child(Select::new(&choice.state).w_full()),
            );
        }
        let mut switches = div().flex().flex_wrap().gap_x_6().gap_y_3();
        for (index, switch) in self.switches.iter().enumerate() {
            switches = switches.child(
                div().w(px(200.)).child(
                    Switch::new(("config-switch", index))
                        .label(switch.label)
                        .checked(switch.enabled)
                        .on_click(cx.listener(move |view, checked: &bool, _, cx| {
                            view.switches[index].enabled = *checked;
                            cx.notify();
                        })),
                ),
            );
        }
        let status = self.status.borrow().clone();
        div()
            .v_flex()
            .size_full()
            .bg(cx.theme().muted)
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .p_6()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .child(style::page_title("Settings"))
                    .child(style::meta(
                        "Workflow uses these settings for each run. Services shows the tools muzik can use.",
                        cx,
                    )),
            )
            .child(
                div().flex_1().overflow_y_scrollbar().child(
                    div()
                        .v_flex()
                        .gap_6()
                        .p_6()
                        .max_w(px(720.))
                        .child(
                            GroupBox::new()
                                .id("config-destinations")
                                .title("DESTINATIONS")
                                .outline()
                                .child(destinations),
                        )
                        .child(
                            GroupBox::new()
                                .id("config-quality")
                                .title("SOURCES AND QUALITY")
                                .outline()
                                .child(choices),
                        )
                        .child(
                            GroupBox::new()
                                .id("config-processing")
                                .title("PROCESSING")
                                .outline()
                                .child(tuning)
                                .child(switches),
                        )
                        .child(Muzik::services_section(self.main.clone(), cx)),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .p_4()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .child(div().text_sm().child(status))
                    .child(
                        Button::new("save-config")
                            .primary()
                            .label("Save config")
                            .on_click(cx.listener(|view, _, _, cx| view.save(cx))),
                    ),
            )
    }
}

impl Render for Muzik {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.page {
            Page::Workflow => self.workflow(cx),
            Page::Settings => div()
                .flex_1()
                .child(
                    self.config_view
                        .as_ref()
                        .expect("config view exists")
                        .clone(),
                )
                .into_any_element(),
            Page::Watchlist => self.watchlist(cx),
            Page::Library => self.library(cx),
            Page::Spotify => self.spotify(cx),
        };
        let body = if !matches!(self.page, Page::Workflow | Page::Settings)
            && (self.job_id.is_some() || self.decision.is_some())
        {
            div()
                .flex()
                .flex_1()
                .child(body)
                .child(self.job_panel(cx))
                .into_any_element()
        } else {
            body
        };
        div()
            .v_flex()
            .size_full()
            .bg(cx.theme().muted)
            .child(self.header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(body),
            )
            .when(self.page != Page::Settings, |this| {
                this.child(self.status_bar(cx))
            })
    }
}

fn short_text(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let text: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{text}…")
    } else {
        text
    }
}

fn activity_section(
    title: &'static str,
    items: &Value,
    summary: fn(&Value) -> String,
) -> ActivitySection {
    let items = items.as_array();
    ActivitySection {
        title,
        count: items.map_or(0, Vec::len),
        rows: items
            .into_iter()
            .flatten()
            .take(4)
            .map(|item| short_text(&summary(item), 120))
            .collect(),
    }
}

fn candidate_summary(candidate: &Value) -> String {
    let title = candidate["title"].as_str().unwrap_or("Unknown title");
    let format = candidate["quality"]["format"]
        .as_str()
        .unwrap_or("Unknown format");
    let user = candidate["user"].as_str().unwrap_or("Unknown user");
    format!("{title} · {format} · {user}")
}

fn chapter_summary(chapter: &Value) -> String {
    let index = chapter["index"].as_u64().unwrap_or(0);
    let start = chapter["start"].as_u64().unwrap_or(0);
    let title = chapter["title"].as_str().unwrap_or("Untitled");
    format!("{index}. {title} · {start} s")
}

fn import_match_summary(candidate: &Value) -> String {
    let artist = candidate["artist"].as_str().unwrap_or("Unknown artist");
    let album = candidate["album"]
        .as_str()
        .or_else(|| candidate["title"].as_str())
        .unwrap_or("Unknown album");
    match candidate["distance"].as_f64() {
        Some(distance) => format!("{artist} — {album} · difference {distance:.3}"),
        None => format!("{artist} — {album}"),
    }
}

fn job_label(kind: Option<&str>) -> &'static str {
    match kind {
        Some("workflow.start") => "Workflow",
        Some("watchlist.refresh") => "Watchlist refresh",
        Some("watchlist.action") => "Item command",
        Some("spotify.login") => "Spotify connection",
        _ => "Job",
    }
}

fn replaces_files(action: &str) -> bool {
    matches!(
        action,
        "download_again" | "parse_again" | "split_again" | "organize_again" | "run_all_again"
    )
}

fn describe(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

fn watch_page_count(playlist: &Value, filter: usize) -> usize {
    let matches = playlist["items"].as_array().map_or(0, |items| {
        items
            .iter()
            .filter(|item| watchlist_view::matches_filter(item, filter))
            .count()
    });
    matches.div_ceil(WATCH_PAGE_SIZE).max(1)
}

fn merge_thumbnail_paths(watchlist: &mut Value, visible: &HashSet<String>, data: &Value) {
    let Some(results) = data["thumbnails"].as_array() else {
        return;
    };
    let Some(playlists) = watchlist["playlists"].as_array_mut() else {
        return;
    };
    for result in results {
        let (Some(video_id), Some(path)) = (result["video_id"].as_str(), result["path"].as_str())
        else {
            continue;
        };
        if !visible.contains(video_id) {
            continue;
        }
        for playlist in playlists.iter_mut() {
            if let Some(items) = playlist["items"].as_array_mut() {
                for item in items {
                    if item["video_id"] == video_id {
                        item["thumbnail_path"] = json!(path);
                    }
                }
            }
        }
    }
}

fn decision_details(decision: &Value) -> Vec<String> {
    let payload = &decision["payload"];
    match decision["kind"].as_str().unwrap_or("") {
        "soulseek_candidate" => payload["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, candidate)| {
                format!(
                    "{}. {} · score {:.0} · {} · {} · {} files · {}",
                    index + 1,
                    candidate["title"].as_str().unwrap_or("Candidate"),
                    candidate["score"].as_f64().unwrap_or(0.0),
                    candidate["user"].as_str().unwrap_or("Unknown user"),
                    candidate["quality"]["format"]
                        .as_str()
                        .unwrap_or("Unknown format"),
                    candidate["files"].as_array().map_or(0, Vec::len),
                    candidate["path"]
                        .as_str()
                        .or_else(|| candidate["source_id"].as_str())
                        .unwrap_or("")
                )
            })
            .collect(),
        "chapter_review" | "chapter_edit" => {
            let mut details = Vec::new();
            if let Some(source) = payload["source"].as_str() {
                details.push(format!("Source: {source}"));
            }
            if let Some(chapters) = payload["chapters"].as_array() {
                for chapter in chapters {
                    details.push(format!(
                        "{}. {} s to {} s · {}",
                        chapter["index"].as_u64().unwrap_or(0),
                        chapter["start"].as_u64().unwrap_or(0),
                        chapter["end"]
                            .as_u64()
                            .map_or_else(|| "end".to_string(), |value| value.to_string()),
                        chapter["title"].as_str().unwrap_or("Untitled")
                    ));
                }
            }
            details
        }
        "quality_replacement" => vec![
            format!(
                "Current file: {}",
                payload["current"].as_str().unwrap_or("")
            ),
            format!(
                "Candidate: {} · {} · {} kbps",
                payload["candidate"]["title"]
                    .as_str()
                    .unwrap_or("Audio file"),
                payload["candidate"]["quality"]["format"]
                    .as_str()
                    .unwrap_or("Unknown format"),
                payload["candidate"]["quality"]["bitrate"]
                    .as_u64()
                    .map_or_else(|| "?".to_string(), |value| value.to_string())
            ),
        ],
        "import_match" | "import_duplicate" => {
            let task = &payload["task"];
            let mut details = vec![format!(
                "Current tags: {} · {} · {}",
                task["current_artist"].as_str().unwrap_or("Unknown artist"),
                task["current_album"].as_str().unwrap_or("Unknown album"),
                task["current_year"].as_str().unwrap_or("Unknown year")
            )];
            if let Some(paths) = task["paths"].as_array() {
                details.extend(
                    paths
                        .iter()
                        .filter_map(|path| path.as_str().map(str::to_owned)),
                );
            }
            if decision["kind"] == "import_duplicate" {
                if let Some(duplicates) = payload["duplicates"].as_array() {
                    details.extend(duplicates.iter().map(|duplicate| {
                        format!(
                            "Existing: {} · {} · {}",
                            duplicate["artist"].as_str().unwrap_or("Unknown artist"),
                            duplicate["album"].as_str().unwrap_or("Unknown album"),
                            duplicate["path"].as_str().unwrap_or("No path")
                        )
                    }));
                }
            }
            details
        }
        _ => Vec::new(),
    }
}

fn decision_choices(decision: &Value) -> Vec<(String, Value)> {
    let payload = &decision["payload"];
    match decision["kind"].as_str().unwrap_or("") {
        "soulseek_candidate" => {
            let mut choices: Vec<(String, Value)> = payload["candidates"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, candidate)| {
                    (
                        format!(
                            "{}: {}",
                            index + 1,
                            candidate["title"]
                                .as_str()
                                .or_else(|| candidate["name"].as_str())
                                .unwrap_or("Candidate")
                        ),
                        json!(index),
                    )
                })
                .collect();
            choices.push(("Skip these candidates".into(), Value::Null));
            choices
        }
        "chapter_review" => ["accept", "edit", "reject"]
            .into_iter()
            .map(|value| (value.to_string(), json!(value)))
            .collect(),
        "chapter_edit" => vec![
            ("Keep original chapters".into(), payload["chapters"].clone()),
            ("Cancel chapter edit".into(), Value::Null),
        ],
        "quality_replacement" => vec![
            ("Replace file".into(), json!(true)),
            ("Keep current file".into(), json!(false)),
        ],
        "import_match" => {
            let mut choices: Vec<(String, Value)> = payload["task"]["matches"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|candidate| {
                    candidate["candidate_id"].as_str().map(|id| {
                        (
                            format!(
                                "{} — {} · distance {}",
                                candidate["artist"].as_str().unwrap_or("Unknown artist"),
                                candidate["album"]
                                    .as_str()
                                    .or_else(|| candidate["title"].as_str())
                                    .unwrap_or("Unknown release"),
                                candidate["distance"]
                                    .as_f64()
                                    .map_or_else(|| "?".to_string(), |value| format!("{value:.3}"))
                            ),
                            json!(id),
                        )
                    })
                })
                .collect();
            choices.push(("Keep current tags".into(), json!("as_is")));
            choices.push(("Skip".into(), Value::Null));
            choices
        }
        "import_duplicate" => ["skip", "keep_all", "remove_old"]
            .into_iter()
            .map(|value| (value.replace('_', " "), json!(value)))
            .collect(),
        _ => Vec::new(),
    }
}

fn main() {
    if cfg!(target_os = "macos") {
        let path = tool_path(
            std::env::var_os("PATH"),
            std::env::var_os("HOME").map(PathBuf::from),
        );
        if let Ok(path) = std::env::join_paths(path) {
            std::env::set_var("PATH", path);
        }
    }
    match std::env::args().nth(1).as_deref() {
        Some("--version") => {
            println!("muzik-gpui {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("--check-backend") => {
            if let Err(error) = check_backend() {
                eprintln!("{error}");
                std::process::exit(1);
            }
            return;
        }
        _ => {}
    }
    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    app.run(|cx| {
        gpui_kit::init(cx);
        style::apply_theme(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Muzik::new(window, cx))
        })
        .expect("open main window");
    });
}

fn tool_path(current: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = current
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    let extra = [
        Some(PathBuf::from("/opt/homebrew/bin")),
        Some(PathBuf::from("/usr/local/bin")),
        home.map(|home| home.join(".local/share/mise/shims")),
    ];
    for directory in extra.into_iter().flatten() {
        if !paths.contains(&directory) {
            paths.push(directory);
        }
    }
    paths
}

fn check_backend() -> Result<(), String> {
    let mut bridge = Bridge::start()?;
    let id = bridge.send("hello", json!({}))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        for message in bridge.drain() {
            if message["type"] == "transport.closed" {
                return Err("Backend closed before hello".into());
            }
            if message["type"] == "response" && message["id"] == id {
                if message["ok"] == true && message["result"]["protocol_version"] == 1 {
                    println!("Rust backend ready (protocol 1)");
                    return Ok(());
                }
                return Err(format!(
                    "Rust backend rejected hello: {}",
                    describe(&message)
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err("Rust backend did not answer hello within 5 seconds".into())
}

#[cfg(test)]
mod tests {
    use super::{
        activity_section, candidate_summary, decision_choices, decision_details,
        merge_thumbnail_paths, tool_path, ActivityProgress, Muzik,
    };
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, TestAppContext, WindowOptions};
    use serde_json::json;
    use std::collections::HashSet;

    #[gpui_kit::test]
    fn settings_tab_saves_config_without_repeating_workflow_fields(cx: &mut TestAppContext) {
        let (handle, main) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Muzik::new_with_bridge(window, cx, false))
            })
            .unwrap()
        });
        cx.update(|cx| {
            main.update(cx, |view, cx| {
                view.defaults = json!({
                    "output": "/tmp/downloads", "splits": "/tmp/splits", "config": "",
                    "jobs": 0, "min_bitrate": 256,
                });
                view.status = "Changed".into();
                cx.notify();
            });
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("pages").click(3usize, cx);
            window.render_frame(cx);
            assert!(window.try_find("service-refresh").is_some());
            window.click("save-config", cx);
            window.within("pages").click(0usize, cx);
            assert!(window.try_find("run").is_some());
            assert!(window.try_find("save-config").is_none());
        })
        .unwrap();
        let params = main.read_with(cx, |view, cx| view.launcher_params(cx));
        assert_eq!(params["output"], "/tmp/downloads");
        assert_eq!(params["splits"], "/tmp/splits");
    }

    #[test]
    fn soulseek_review_shows_candidate_quality_and_selects_its_index() {
        let decision = json!({
            "kind": "soulseek_candidate",
            "payload": {"candidates": [{
                "title": "Album", "score": 91.0, "user": "listener",
                "quality": {"format": "FLAC"}, "files": [{"name": "track.flac"}],
                "path": "Music/Album"
            }]}
        });
        let details = decision_details(&decision);
        assert!(details[0].contains("91"));
        assert!(details[0].contains("FLAC"));
        assert!(details[0].contains("Music/Album"));
        assert_eq!(decision_choices(&decision)[0].1, json!(0));
        assert_eq!(decision_choices(&decision).last().unwrap().1, json!(null));
    }

    #[test]
    fn import_duplicate_review_shows_existing_file_and_reply_options() {
        let decision = json!({
            "kind": "import_duplicate",
            "payload": {
                "task": {"current_artist": "Artist", "current_album": "Album", "paths": ["new.flac"]},
                "duplicates": [{"artist": "Artist", "album": "Album", "path": "old.flac"}]
            }
        });
        let details = decision_details(&decision);
        assert!(details.iter().any(|line| line.contains("old.flac")));
        assert!(details.iter().any(|line| line.contains("new.flac")));
        assert_eq!(
            decision_choices(&decision)
                .into_iter()
                .map(|(_, value)| value)
                .collect::<Vec<_>>(),
            vec![json!("skip"), json!("keep_all"), json!("remove_old")]
        );
    }

    #[gpui_kit::test]
    fn native_import_events_show_matches_and_progress(cx: &mut TestAppContext) {
        let main = cx.update(|cx| {
            gpui_kit::init(cx);
            let (_, main) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Muzik::new_with_bridge(window, cx, false))
            })
            .unwrap();
            main
        });
        cx.update(|cx| {
            main.update(cx, |view, _cx| {
                view.record_job_event(
                    "import_started",
                    &json!({"paths": ["/music/album"], "dry_run": false}),
                );
                assert_eq!(view.job_status, "Import started");
                view.record_job_event(
                    "task",
                    &json!({"task": {
                        "current_artist": "Artist",
                        "current_album": "Album",
                        "matches": [{
                            "artist": "Artist", "album": "Album", "distance": 0.05
                        }]
                    }}),
                );
                let section = view
                    .activity_sections
                    .iter()
                    .find(|section| section.title == "Album matches")
                    .unwrap();
                assert_eq!(section.count, 1);
                assert!(section.rows[0].contains("Artist"));
                assert!(view.job_status.contains("Album"));
                view.record_job_event("log", &json!({"message": "Writing tags"}));
                assert_eq!(view.job_status, "Writing tags");
                view.record_job_event("import_finished", &json!({"success": true}));
                assert_eq!(view.job_status, "Import finished");
                assert!(view.logs.iter().any(|line| line == "Writing tags"));
            });
        });
    }

    #[test]
    fn thumbnail_event_updates_only_current_visible_cards() {
        let mut watchlist = json!({"playlists":[{"items":[
            {"video_id":"visible", "thumbnail_path":null},
            {"video_id":"other", "thumbnail_path":null}
        ]}]});
        let visible = HashSet::from(["visible".to_string()]);
        let event = json!({"thumbnails":[
            {"video_id":"visible", "path":"/cache/visible.jpg"},
            {"video_id":"other", "path":"/cache/other.jpg"},
            {"video_id":"visible", "path":null}
        ]});
        merge_thumbnail_paths(&mut watchlist, &visible, &event);
        assert_eq!(
            watchlist["playlists"][0]["items"][0]["thumbnail_path"],
            "/cache/visible.jpg"
        );
        assert!(watchlist["playlists"][0]["items"][1]["thumbnail_path"].is_null());
    }

    #[test]
    fn activity_summary_keeps_count_and_bounds_visible_rows() {
        let candidates = json!((0..6)
            .map(|index| json!({
                "title": format!("Album {index}"),
                "quality": {"format": "FLAC"},
                "user": "listener"
            }))
            .collect::<Vec<_>>());
        let section = activity_section("Source candidates", &candidates, candidate_summary);
        assert_eq!(section.count, 6);
        assert_eq!(section.rows.len(), 4);
        assert_eq!(section.rows[0], "Album 0 · FLAC · listener");
    }

    #[test]
    fn activity_progress_clamps_value_for_late_events() {
        let progress = ActivityProgress {
            completed: 12.,
            total: Some(10.),
            ..ActivityProgress::default()
        };
        assert_eq!(progress.percentage(), 100.);
    }

    #[test]
    fn app_path_adds_homebrew_and_mise_tools_once() {
        let path = tool_path(
            Some("/usr/bin:/opt/homebrew/bin".into()),
            Some(std::path::PathBuf::from("/Users/listener")),
        );
        assert_eq!(
            path,
            [
                "/usr/bin",
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/Users/listener/.local/share/mise/shims"
            ]
            .map(std::path::PathBuf::from)
            .to_vec()
        );
    }
}
