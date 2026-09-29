mod bridge;
mod native;
mod pages;
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
use gpui_kit::component::theme::Theme;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use muzik_core::watchlist::{ItemAction, SourceKind, Summary};
use muzik_core::{
    AudioFallback, AudioSource, DecisionKind, DuplicatePolicy, MetadataSource, QualityPolicy,
};
use muzik_jobs::Status as JobStatus;
use muzik_runner::choices::{self, Choice as DecisionChoice};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use strum_macros::{Display, EnumString};

actions!(muzik, [Quit]);

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
enum Ending {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, EnumString)]
#[strum(serialize_all = "snake_case")]
enum RunKind {
    Workflow,
    Refresh,
    Item,
    SpotifyLogin,
    #[default]
    #[strum(disabled)]
    Unknown,
}

impl RunKind {
    fn parse(value: &Value) -> Self {
        value
            .as_str()
            .and_then(|kind| kind.parse().ok())
            .unwrap_or_default()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Workflow => "Workflow",
            Self::Refresh => "Watchlist check",
            Self::Item => "Item",
            Self::SpotifyLogin => "Spotify connection",
            Self::Unknown => "Job",
        }
    }
}

struct Run {
    id: String,
    kind: RunKind,
    title: String,
    status: String,
    queued: bool,
    progress: ActivityProgress,
}

impl Run {
    fn new(id: &str, kind: RunKind, title: &str) -> Self {
        Self {
            id: id.into(),
            kind,
            title: title.into(),
            status: "Queued".into(),
            queued: true,
            progress: ActivityProgress::default(),
        }
    }

    fn progress_text(&self) -> String {
        match self.progress.total {
            Some(total) => format!("{:.0} / {:.0}", self.progress.completed, total),
            None => String::new(),
        }
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
    (
        "duplicates",
        "Album already in library",
        DuplicatePolicy::CHOICES,
    ),
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
    ("auto_decide", "Choose automatically", true),
];
const WATCH_PAGE_SIZE: usize = 8;
const REPLACE_WARNING: &str =
    "This replaces the files from this stage. Later stages can become stale.";

fn filter_summary(filter: usize) -> Option<Summary> {
    filter
        .checked_sub(1)
        .and_then(|index| Summary::ALL.get(index).copied())
}

fn filter_label(filter: usize) -> &'static str {
    filter_summary(filter).map_or("All", Into::into)
}

fn action_label(action: ItemAction) -> &'static str {
    match action {
        ItemAction::Run => "Run",
        ItemAction::Retry => "Retry",
        ItemAction::DownloadAgain => "Download again",
        ItemAction::CheckQualityAgain => "Check quality again",
        ItemAction::ParseAgain => "Parse again",
        ItemAction::SplitAgain => "Split again",
        ItemAction::OrganizeAgain => "Organize again",
        ItemAction::RunAllAgain => "Run all again",
    }
}

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
    runs: Vec<Run>,
    finished_runs: HashSet<String>,
    gates: Value,
    queued_items: HashSet<String>,
    reload_watchlist: bool,
    activity_sections: Vec<ActivitySection>,
    logs: Vec<String>,
    decision: Option<Value>,
    requests: Vec<Value>,
    waiting: Vec<Value>,
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
            runs: Vec::new(),
            finished_runs: HashSet::new(),
            gates: Value::Null,
            queued_items: HashSet::new(),
            reload_watchlist: false,
            activity_sections: Vec::new(),
            logs: Vec::new(),
            decision: None,
            requests: Vec::new(),
            waiting: Vec::new(),
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
                    this.send("jobs.list", json!({}));
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
            self.send("soulseek.get", json!({}));
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
        self.error = None;
        self.send(command, params);
        cx.notify();
    }

    fn has_run(&self, kind: RunKind) -> bool {
        self.runs.iter().any(|run| run.kind == kind)
    }

    fn run_mut(&mut self, id: &str) -> &mut Run {
        let index = match self.runs.iter().position(|run| run.id == id) {
            Some(index) => index,
            None => {
                self.runs.push(Run::new(id, RunKind::Unknown, "Job"));
                self.runs.len() - 1
            }
        };
        let run = &mut self.runs[index];
        run.queued = false;
        run
    }

    fn set_status(&mut self, id: &str, status: impl Into<String>) {
        self.run_mut(id).status = status.into();
    }

    fn apply_jobs(&mut self, snapshot: &Value) {
        self.waiting = snapshot["waiting"].as_array().cloned().unwrap_or_default();
        let open = snapshot["open"].as_array().cloned().unwrap_or_default();
        self.queued_items = open
            .iter()
            .filter_map(|job| job["item"].as_str().map(str::to_owned))
            .collect();
        let listed: HashSet<&str> = open
            .iter()
            .filter_map(|job| job["job_id"].as_str())
            .collect();
        self.runs
            .retain(|run| !run.queued || listed.contains(run.id.as_str()));
        for job in &open {
            let id = job["job_id"].as_str().unwrap_or("");
            if id.is_empty() || self.finished_runs.contains(id) {
                continue;
            }
            let queued = job["status"]
                .as_str()
                .and_then(|status| status.parse::<JobStatus>().ok())
                == Some(JobStatus::Queued);
            let kind = RunKind::parse(&job["kind"]);
            let title = job["title"].as_str().unwrap_or("Job");
            match self.runs.iter_mut().find(|run| run.id == id) {
                Some(run) => {
                    run.kind = kind;
                    run.title = title.into();
                    if !queued && run.queued {
                        run.queued = false;
                        run.status = "Starting".into();
                    }
                }
                None => {
                    let mut run = Run::new(id, kind, title);
                    if !queued {
                        run.queued = false;
                        run.status = "Starting".into();
                    }
                    self.runs.push(run);
                }
            }
        }
        self.runs.sort_by_key(|run| run.queued);
    }

    fn record_job_event(&mut self, job_id: &str, kind: &str, payload: &Value) {
        let line = match kind {
            "progress_started" => {
                let run = self.run_mut(job_id);
                let progress = &mut run.progress;
                progress.task_id = describe(&payload["task_id"]);
                progress.description = describe(&payload["description"]);
                progress.completed = 0.;
                progress.total = payload["total"].as_f64().filter(|total| *total > 0.);
                run.status = progress.description.clone();
                run.status.clone()
            }
            "progress_advanced" => {
                let progress = &mut self.run_mut(job_id).progress;
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
                let progress = &mut self.run_mut(job_id).progress;
                if payload["task_id"].as_str() != Some(progress.task_id.as_str()) {
                    return;
                }
                if let Some(total) = progress.total {
                    progress.completed = total;
                }
                format!("{} finished", progress.description)
            }
            "step_started" => {
                let name = describe(&payload["name"]);
                self.set_status(job_id, name.clone());
                format!("Started {name}")
            }
            "step_finished" => {
                let progress = &mut self.run_mut(job_id).progress;
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
                self.set_status(job_id, message.clone());
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
                self.set_status(job_id, message.clone());
                message
            }
            "item_waiting" => {
                let title = describe(&payload["title"]);
                let kind = choices::title(choices::kind(&payload["question"]));
                self.set_status(job_id, format!("Waiting for you: {kind}"));
                format!("{title} waits for you: {kind}")
            }
            "agent_decided" => {
                let label = describe(&payload["label"]);
                self.set_status(job_id, format!("Chose {label}"));
                format!(
                    "Chose {label} ({:.0}%): {}",
                    payload["confidence"].as_f64().unwrap_or(0.0) * 100.0,
                    describe(&payload["reason"])
                )
            }
            "import_started" => {
                self.set_status(job_id, "Import started");
                "Import started".into()
            }
            "import_finished" => {
                let status = if payload["success"] == false {
                    "Import failed"
                } else {
                    "Import finished"
                };
                self.set_status(job_id, status);
                status.into()
            }
            _ => kind.replace('_', " "),
        };
        if !line.is_empty() {
            self.logs.push(short_text(&shorten_paths(&line), 180));
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
        if std::mem::take(&mut self.reload_watchlist) {
            self.send("watchlist.load", self.launcher_params(cx));
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
                    if matches!(command.as_str(), "config.save" | "soulseek.save") {
                        *self.config_status.borrow_mut() = self.status.clone();
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
                        if command == "watchlist.add" {
                            self.watch_url
                                .update(_cx, |state, cx| state.set_value("", window, cx));
                        }
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
                    "soulseek.get" | "soulseek.save" => {
                        if let Some(view) = self.config_view.clone() {
                            view.update(_cx, |view, cx| view.set_soulseek(result, window, cx));
                        }
                        if command == "soulseek.save" {
                            *self.config_status.borrow_mut() =
                                "Config and Soulseek account saved".into();
                            self.send("services.check", json!({}));
                        }
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
                    "jobs.list" => {
                        self.apply_jobs(result);
                        self.gates = result["gates"].clone();
                    }
                    "jobs.answer" => self.status = "Answer saved; the item is queued".into(),
                    "spotify.login" => {
                        if let Some(id) = result["job_id"].as_str() {
                            if !self.finished_runs.contains(id) {
                                let mut run =
                                    Run::new(id, RunKind::SpotifyLogin, "Spotify connection");
                                run.queued = false;
                                run.status = "Waiting for the browser".into();
                                self.runs.push(run);
                            }
                        }
                    }
                    "workflow.start" | "watchlist.refresh" | "watchlist.action" => {
                        self.status = "Added to the queue".into();
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
                    "jobs.updated" => self.apply_jobs(data),
                    "queues.updated" => self.gates = data.clone(),
                    "jobs.remote" => {
                        self.status = data["message"]
                            .as_str()
                            .unwrap_or("Another muzik process runs the queue.")
                            .into();
                    }
                    "job.started" => {
                        let id = data["job_id"].as_str().unwrap_or("");
                        let kind = RunKind::parse(&data["kind"]);
                        let title = data["title"].as_str().unwrap_or("Job").to_owned();
                        let run = self.run_mut(id);
                        run.kind = kind;
                        run.title = title;
                        run.status = "Starting".into();
                    }
                    "job.event" => {
                        let kind = data["event"].as_str().unwrap_or("Update");
                        let payload = &data["data"];
                        if kind == "watchlist_saved" {
                            self.reload_watchlist = true;
                            return;
                        }
                        let job_id = data["job_id"].as_str().unwrap_or("").to_owned();
                        self.record_job_event(&job_id, kind, payload);
                        if self.logs.len() > 300 {
                            self.logs.drain(..100);
                        }
                    }
                    "decision.request" => {
                        let job_id = data["job_id"].as_str().unwrap_or("").to_owned();
                        self.set_status(&job_id, "Decision needed");
                        self.requests.push(data.clone());
                        if self.decision.is_none() {
                            self.open_decision(data.clone(), window, _cx);
                        }
                    }
                    "job.completed" | "job.failed" | "job.cancelled" => {
                        let id = data["job_id"].as_str().unwrap_or("").to_owned();
                        self.finished_runs.insert(id.clone());
                        let run = self
                            .runs
                            .iter()
                            .position(|run| run.id == id)
                            .map(|index| self.runs.remove(index));
                        let kind = run.as_ref().map_or(RunKind::Unknown, |run| run.kind);
                        let title = run
                            .as_ref()
                            .map_or_else(|| "Job".to_owned(), |run| run.title.clone());
                        let ending = event
                            .trim_start_matches("job.")
                            .parse::<Ending>()
                            .unwrap_or(Ending::Failed);
                        self.requests
                            .retain(|request| request["job_id"] != id.as_str());
                        if self
                            .decision
                            .as_ref()
                            .is_some_and(|decision| decision["job_id"] == id.as_str())
                        {
                            self.decision = None;
                            self.chapter_rows.clear();
                            if let Some(next) = self.requests.first().cloned() {
                                self.open_decision(next, window, _cx);
                            }
                        }
                        let failure = (ending == Ending::Failed).then(|| {
                            data["error"]["message"]
                                .as_str()
                                .unwrap_or("Job failed")
                                .to_string()
                        });
                        let job = job_label(kind, &title);
                        self.logs.push(short_text(&format!("{job} {ending}"), 180));
                        let note = match (ending, &failure) {
                            (Ending::Failed, failure) => Some(
                                Notification::error(failure.clone().unwrap_or_default())
                                    .title(format!("{job} failed")),
                            ),
                            (Ending::Cancelled, _) => {
                                Some(Notification::warning(format!("{job} cancelled")))
                            }
                            (Ending::Completed, _) if kind != RunKind::Item => {
                                Some(Notification::success(format!("{job} finished")))
                            }
                            (Ending::Completed, _) => None,
                        };
                        if let Some(note) = note {
                            window.push_notification(note, _cx);
                        }
                        if kind == RunKind::SpotifyLogin {
                            self.send("spotify.status", json!({}));
                            if ending == Ending::Completed {
                                self.send("spotify.playlists", json!({}));
                            }
                        } else {
                            self.reload_watchlist = true;
                        }
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

    fn open_decision(&mut self, data: Value, window: &mut Window, cx: &mut Context<Self>) {
        self.chapter_rows.clear();
        if choices::kind(&data) == Some(DecisionKind::ChapterEdit) {
            if let Some(chapters) = data["payload"]["chapters"].as_array() {
                for chapter in chapters {
                    let mut make = |key: &'static str| {
                        let value = describe(&chapter[key]);
                        cx.new(|cx| {
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
        self.decision = Some(data);
    }

    fn open_waiting(&mut self, job: &Value, window: &mut Window, cx: &mut Context<Self>) {
        self.open_decision(
            json!({"queue_job":job["id"],"title":job["title"],"kind":job["kind"],"payload":job["payload"]}),
            window,
            cx,
        );
        cx.notify();
    }

    fn reply(&mut self, value: Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(decision) = self.decision.take() else {
            return;
        };
        self.chapter_rows.clear();
        if decision["queue_job"].is_null() {
            self.requests
                .retain(|request| request["decision_id"] != decision["decision_id"]);
            self.send(
                "decision.reply",
                json!({"decision_id":decision["decision_id"],"value":value}),
            );
            if let Some(job_id) = decision["job_id"].as_str() {
                self.set_status(job_id, "Working");
            }
        } else {
            self.waiting
                .retain(|job| job["id"] != decision["queue_job"]);
            self.send(
                "jobs.answer",
                json!({"id":decision["queue_job"],"value":value}),
            );
        }
        if let Some(next) = self.requests.first().cloned() {
            self.open_decision(next, window, cx);
        }
        cx.notify();
    }

    fn submit_chapters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.reply(Value::Array(chapters), window, cx);
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

    fn lanes(&self, cx: &App) -> Div {
        let mut lanes = div().v_flex().gap_1p5();
        for (key, label) in [
            ("download", "Download"),
            ("process", "Process"),
            ("import", "Import"),
        ] {
            let lane = &self.gates[key];
            let active: Vec<&str> = lane["active"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let waiting = lane["waiting"].as_array().map_or(0, Vec::len);
            let limit = lane["limit"].as_u64().unwrap_or(1);
            let mut counts = format!("{}/{limit}", active.len());
            if waiting > 0 {
                counts.push_str(&format!(" · {waiting} waiting"));
            }
            lanes = lanes.child(
                div()
                    .v_flex()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_sm()
                            .child(label)
                            .child(style::mono(counts, cx)),
                    )
                    .children(
                        active
                            .into_iter()
                            .map(|name| style::meta(short_text(name, 48), cx)),
                    ),
            );
        }
        lanes
    }

    fn run_row(&self, index: usize, run: &Run, cx: &mut Context<Self>) -> AnyElement {
        let id = run.id.clone();
        let label = job_label(run.kind, &run.title);
        let queued = run.queued;
        let show_progress = !queued && (run.progress.total.is_some() || run.kind != RunKind::Item);
        div()
            .id(("run", index))
            .v_flex()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_semibold()
                            .child(short_text(&run.title, 48)),
                    )
                    .child(
                        Button::new(("cancel-run", index))
                            .ghost()
                            .xsmall()
                            .label(if queued { "Remove" } else { "Cancel" })
                            .on_click(cx.listener(move |view, _, window, cx| {
                                if queued {
                                    view.send("job.cancel", json!({"job_id":id}));
                                    cx.notify();
                                    return;
                                }
                                view.request_action(
                                    PendingAction {
                                        title: format!("Cancel {label}?"),
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
            )
            .child(style::meta(
                format!("{} · {}", run.kind.label(), short_text(&run.status, 60)),
                cx,
            ))
            .when(show_progress, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div().flex_1().child(
                                Progress::new(("run-progress", index))
                                    .value(run.progress.percentage())
                                    .loading(run.progress.total.is_none())
                                    .accessibility_label("Job progress"),
                            ),
                        )
                        .child(style::mono(run.progress_text(), cx)),
                )
            })
            .into_any_element()
    }

    fn job_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let running = self.runs.iter().filter(|run| !run.queued).count();
        let queued = self.runs.len() - running;
        let mut summary = format!("{running} running");
        if queued > 0 {
            summary.push_str(&format!(" · {queued} queued"));
        }
        if !self.waiting.is_empty() {
            summary.push_str(&format!(" · {} need you", self.waiting.len()));
        }
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
            .child(
                div()
                    .id("activity-status")
                    .v_flex()
                    .gap_1()
                    .child(style::section_title("Activity"))
                    .child(style::meta(summary, cx)),
            )
            .when(self.gates.is_object(), |this| this.child(self.lanes(cx)));
        if let Some(decision) = &self.decision {
            let kind = choices::kind(decision);
            let mut review = div()
                .id("decision")
                .v_flex()
                .gap_3()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().warning)
                .bg(cx.theme().background)
                .child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(style::overline("DECISION NEEDED", cx))
                        .child(div().text_sm().font_semibold().child(choices::title(kind)))
                        .when_some(decision["title"].as_str(), |this, title| {
                            this.child(style::meta(short_text(title, 80), cx))
                        }),
                );
            if let Some(note) = choices::note(decision) {
                review = review.child(style::meta(note, cx));
            }
            if let Some(note) = choices::agent_note(&decision["payload"]["agent"]) {
                review = review.child(div().text_xs().text_color(cx.theme().warning).child(note));
            }
            let suggested = choices::suggestion(decision);
            let mut buttons = div().v_flex().gap_1p5();
            for (index, option) in choices::choices(decision).into_iter().enumerate() {
                let value = option.value.clone();
                let reply =
                    cx.listener(move |view, _, window, cx| view.reply(value.clone(), window, cx));
                let highlight = suggested == Some(index);
                if option.score.is_none() && option.meta.is_empty() {
                    let button = Button::new(("decision", index))
                        .label(option.label)
                        .w_full()
                        .on_click(reply);
                    buttons = buttons.child(if highlight { button.primary() } else { button });
                } else {
                    buttons =
                        buttons.child(decision_row(index, &option, highlight, cx).on_click(reply));
                }
            }
            review = review.child(buttons);
            let details = choices::details(decision);
            if !details.is_empty() {
                let mut list = div()
                    .v_flex()
                    .gap_1()
                    .max_h(px(160.))
                    .overflow_y_scrollbar();
                for detail in details.iter().take(6) {
                    list = list.child(style::meta(short_detail(detail), cx));
                }
                if details.len() > 6 {
                    list = list.child(style::meta(format!("{} more", details.len() - 6), cx));
                }
                review = review.child(
                    div()
                        .v_flex()
                        .gap_1()
                        .pt_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(style::overline("DETAILS", cx))
                        .child(list),
                );
            }
            if kind == Some(DecisionKind::ChapterEdit) {
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
                            .on_click(
                                cx.listener(|view, _, window, cx| view.submit_chapters(window, cx)),
                            ),
                    );
            }
            if !decision["queue_job"].is_null() {
                review = review.child(
                    div().flex().child(
                        Button::new("decision-later")
                            .ghost()
                            .small()
                            .label("Later")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.decision = None;
                                view.chapter_rows.clear();
                                cx.notify();
                            })),
                    ),
                );
            }
            panel = panel.child(review);
        }
        if !self.waiting.is_empty() {
            let open = self
                .decision
                .as_ref()
                .map_or(Value::Null, |decision| decision["queue_job"].clone());
            let mut inbox = div().v_flex().gap_1p5().child(style::overline(
                format!("NEEDS YOU ({})", self.waiting.len()),
                cx,
            ));
            for (index, job) in self.waiting.iter().enumerate() {
                let kind = choices::kind(job);
                let job = job.clone();
                inbox = inbox.child(
                    div()
                        .id(("waiting", index))
                        .v_flex()
                        .gap_0p5()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(if job["id"] == open {
                            cx.theme().warning
                        } else {
                            cx.theme().border
                        })
                        .bg(cx.theme().background)
                        .cursor_pointer()
                        .hover(|this| this.bg(cx.theme().accent))
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(short_text(job["title"].as_str().unwrap_or("Item"), 60)),
                        )
                        .child(style::meta(choices::title(kind), cx))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.open_waiting(&job, window, cx)
                        })),
                );
            }
            panel = panel.child(inbox);
        }
        if !self.runs.is_empty() {
            let mut list = div().v_flex().gap_1p5().child(style::overline("JOBS", cx));
            let shown = self
                .runs
                .iter()
                .enumerate()
                .filter(|(index, run)| !run.queued || *index < running + 5);
            for (index, run) in shown {
                list = list.child(self.run_row(index, run, cx));
            }
            let hidden = self.runs.len().saturating_sub(running + 5);
            if hidden > 0 {
                list = list.child(style::meta(format!("{hidden} more in the queue"), cx));
            }
            panel = panel.child(list);
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
        let mut log = div().v_flex().gap_1p5();
        if self.logs.is_empty() {
            log = log.child(style::meta("Job updates will appear here.", cx));
        }
        for (index, line) in self.logs.iter().rev().take(50).enumerate() {
            log = log.child(
                div()
                    .id(("log", index))
                    .text_xs()
                    .text_color(if index == 0 {
                        cx.theme().foreground
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(line.clone()),
            );
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
    soulseek: SoulseekFields,
    status: Rc<RefCell<String>>,
}

struct SoulseekFields {
    username: Entity<InputState>,
    password: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    has_password: bool,
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
            ("agent_model", "Model"),
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
        let soulseek = SoulseekFields {
            username: cx.new(|cx| InputState::new(window, cx).placeholder("Username")),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Password")
                    .masked(true)
            }),
            host: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("server.slsknet.org")
                    .default_value("server.slsknet.org")
            }),
            port: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("2416")
                    .default_value("2416")
                    .step(1.)
                    .min(1.)
                    .max(65535.)
            }),
            has_password: false,
        };
        Self {
            main: main.downgrade(),
            fields,
            choices,
            switches,
            soulseek,
            status,
        }
    }

    fn set_soulseek(&mut self, settings: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let text = |key: &str| describe(&settings[key]);
        for (state, value) in [
            (&self.soulseek.username, text("username")),
            (&self.soulseek.host, text("server_host")),
            (&self.soulseek.port, text("server_port")),
        ] {
            if !value.is_empty() {
                state.update(cx, |state, cx| state.set_value(value, window, cx));
            }
        }
        self.soulseek.has_password = settings["has_password"] == true;
        let placeholder = if self.soulseek.has_password {
            "Saved. Type a new one to change it."
        } else {
            "Password"
        };
        self.soulseek.password.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.set_placeholder(placeholder, window, cx);
        });
        cx.notify();
    }

    fn soulseek_params(&self, cx: &App) -> Option<Value> {
        let username = self.soulseek.username.read(cx).value().trim().to_string();
        if username.is_empty() {
            return None;
        }
        Some(json!({
            "username": username,
            "password": self.soulseek.password.read(cx).value().to_string(),
            "server_host": self.soulseek.host.read(cx).value().trim().to_string(),
            "server_port": self.soulseek.port.read(cx).value().trim().to_string(),
        }))
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
        let soulseek = self.soulseek_params(cx);
        if let Some(main) = self.main.upgrade() {
            main.update(cx, |main, cx| {
                main.error = None;
                main.send("config.save", Value::Object(params));
                if let Some(soulseek) = soulseek {
                    main.send("soulseek.save", soulseek);
                }
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
        let mut agent = div().v_flex().gap_3().child(style::meta(
            "Muzik asks this Codex model to pick album matches and Soulseek downloads. It asks you when the model is not sure.",
            cx,
        ));
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
            } else if field.key == "agent_model" {
                agent = agent.child(div().w(px(240.)).child(field_view));
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
            let control = Switch::new(("config-switch", index))
                .label(switch.label)
                .checked(switch.enabled)
                .on_click(cx.listener(move |view, checked: &bool, _, cx| {
                    view.switches[index].enabled = *checked;
                    cx.notify();
                }));
            if switch.key == "auto_decide" {
                agent = agent.child(control);
            } else {
                switches = switches.child(div().w(px(200.)).child(control));
            }
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
                                .id("config-agent")
                                .title("AI DECISIONS")
                                .outline()
                                .child(agent),
                        )
                        .child(
                            GroupBox::new()
                                .id("config-soulseek")
                                .title("SOULSEEK")
                                .outline()
                                .child(style::meta(
                                    "Your Soulseek account. Workflow uses it when Audio source is soulseek or as a fallback.",
                                    cx,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_4()
                                        .child(labeled("Username", 240., Input::new(&self.soulseek.username)))
                                        .child(labeled("Password", 240., Input::new(&self.soulseek.password))),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_4()
                                        .child(labeled("Server", 240., Input::new(&self.soulseek.host)))
                                        .child(labeled("Port", 140., NumberInput::new(&self.soulseek.port))),
                                ),
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
            && (!self.runs.is_empty() || self.decision.is_some() || !self.waiting.is_empty())
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

fn shorten_paths(line: &str) -> String {
    let Some(start) = line
        .find('/')
        .filter(|start| *start == 0 || line[..*start].ends_with(' '))
    else {
        return line.to_owned();
    };
    let path = std::path::Path::new(&line[start..]);
    if path.components().count() < 3 {
        return line.to_owned();
    }
    path.file_name().map_or_else(
        || line.to_owned(),
        |name| format!("{}{}", &line[..start], name.to_string_lossy()),
    )
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

fn labeled(label: &'static str, width: f32, control: impl IntoElement) -> Div {
    div()
        .v_flex()
        .gap_1()
        .w(px(width))
        .child(div().text_sm().font_semibold().child(label))
        .child(control)
}

fn job_label(kind: RunKind, title: &str) -> String {
    match kind {
        RunKind::Refresh | RunKind::SpotifyLogin => kind.label().into(),
        _ => format!("{} “{}”", kind.label(), short_text(title, 60)),
    }
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

fn decision_row(index: usize, option: &DecisionChoice, highlight: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let score_color = match option.score {
        Some(score) if score >= 85 => theme.success,
        Some(score) if score >= 60 => theme.warning,
        _ => theme.muted_foreground,
    };
    let label = div().text_sm().font_semibold().child(option.label.clone());
    let mut row = div()
        .id(("decision", index))
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(if highlight {
            theme.primary
        } else {
            theme.border
        })
        .cursor_pointer()
        .hover(|row| row.bg(theme.accent))
        .child(
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .when(highlight, |column| {
                    column.child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(theme.primary)
                            .child("SUGGESTED"),
                    )
                })
                .child(label)
                .when(!option.meta.is_empty(), |column| {
                    column.child(style::meta(option.meta.clone(), cx))
                }),
        );
    if let Some(score) = option.score {
        row = row.child(
            div()
                .flex_none()
                .text_sm()
                .font_semibold()
                .text_color(score_color)
                .child(format!("{score}%")),
        );
    }
    row
}

fn short_detail(detail: &str) -> String {
    if detail.starts_with('/') {
        std::path::Path::new(detail).file_name().map_or_else(
            || detail.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    } else {
        detail.to_owned()
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
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus([Menu::new("Muzik").items([MenuItem::action("Quit Muzik", Quit)])]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
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
        activity_section, candidate_summary, merge_thumbnail_paths, shorten_paths, tool_path,
        ActivityProgress, Muzik,
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
                let status = |view: &Muzik| {
                    view.runs
                        .iter()
                        .find(|run| run.id == "queue-1")
                        .map(|run| run.status.clone())
                        .unwrap_or_default()
                };
                view.record_job_event(
                    "queue-1",
                    "import_started",
                    &json!({"paths": ["/music/album"], "dry_run": false}),
                );
                assert_eq!(status(view), "Import started");
                view.record_job_event(
                    "queue-1",
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
                assert!(status(view).contains("Album"));
                view.record_job_event("queue-1", "log", &json!({"message": "Writing tags"}));
                assert_eq!(status(view), "Writing tags");
                view.record_job_event("queue-1", "import_finished", &json!({"success": true}));
                assert_eq!(status(view), "Import finished");
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
    fn event_lines_show_the_folder_name_instead_of_the_full_path() {
        assert_eq!(
            shorten_paths("Import group 1 of 1: /Users/tudor/Library/Application Support/muzik/splits/GENDEMA - sassy things [Full album] [wiih44Gfi2M]"),
            "Import group 1 of 1: GENDEMA - sassy things [Full album] [wiih44Gfi2M]"
        );
        assert_eq!(shorten_paths("Split 3/12 tracks"), "Split 3/12 tracks");
        assert_eq!(shorten_paths("Started import"), "Started import");
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
