mod bridge;

use bridge::Bridge;
use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Workflow,
    Watchlist,
    Library,
    Settings,
    Spotify,
}

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
struct Switch {
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

struct PendingAction {
    title: String,
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
    (
        "audio_source",
        "Audio source",
        &["youtube", "soulseek", "auto"],
    ),
    (
        "metadata_source",
        "Metadata",
        &["auto", "youtube", "musicbrainz", "none"],
    ),
    ("prefer", "Prefer", &["lossless", "best", "mp3", "flac"]),
    ("fallback", "Fallback", &["youtube", "none"]),
    ("quality_policy", "Quality policy", &["off", "ask", "auto"]),
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
    fields: Vec<Field>,
    choices: Vec<Choice>,
    switches: Vec<Switch>,
    watch_url: Entity<InputState>,
    playlist_name: Entity<InputState>,
    spotify_client_id: Entity<InputState>,
    chapter_rows: Vec<ChapterRow>,
    confirmation: Option<PendingAction>,
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
}

impl Muzik {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fields = [
            ("raw", "URL or path", ""),
            ("output", "Downloads", ""),
            ("splits", "Splits", ""),
            ("config", "Beets config", ""),
            ("jobs", "Jobs", "0"),
            ("min_bitrate", "Min bitrate", "256"),
        ]
        .into_iter()
        .map(|(key, label, default)| Field {
            key,
            label,
            state: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(label)
                    .default_value(default)
            }),
        })
        .collect();
        let choices: Vec<Choice> = CHOICES
            .iter()
            .map(|(key, label, values)| Choice {
                key,
                label,
                values,
                selected: 0,
                state: cx.new(|cx| {
                    SelectState::new(values.to_vec(), Some(IndexPath::default()), window, cx)
                }),
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
        let mut this = Self {
            page: Page::Workflow,
            fields,
            choices,
            switches: SWITCHES
                .iter()
                .map(|(key, label, enabled)| Switch {
                    key,
                    label,
                    enabled: *enabled,
                })
                .collect(),
            watch_url: cx.new(|cx| InputState::new(window, cx).placeholder("Playlist URL")),
            playlist_name: cx.new(|cx| InputState::new(window, cx).placeholder("Playlist name")),
            spotify_client_id: cx
                .new(|cx| InputState::new(window, cx).placeholder("Spotify client ID")),
            chapter_rows: Vec::new(),
            confirmation: None,
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
        };
        match Bridge::start() {
            Ok(bridge) => {
                this.bridge = Some(bridge);
                this.send("hello", json!({}));
            }
            Err(error) => this.status = error,
        }
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
                    "library.scan" | "services.check" | "spotify.status" | "spotify.playlists"
                ) {
                    self.latest_reads.insert(command.into(), id.clone());
                }
                self.pending.insert(id, command.into());
                self.status = format!("{command} requested");
            }
            Some(Err(error)) => self.status = error,
            None => self.status = "Python service is not available".into(),
        }
    }

    fn launcher_params(&self, cx: &App) -> Value {
        let mut params = Map::new();
        for field in &self.fields {
            let value = field.state.read(cx).value().to_string();
            if field.key == "jobs" || field.key == "min_bitrate" {
                if let Ok(number) = value.parse::<u64>() {
                    params.insert(field.key.into(), json!(number));
                }
            } else {
                let value = if value.is_empty() && (field.key == "output" || field.key == "splits")
                {
                    self.defaults[field.key].as_str().unwrap_or("").to_string()
                } else {
                    value
                };
                params.insert(field.key.into(), json!(value));
            }
        }
        for choice in &self.choices {
            params.insert(choice.key.into(), json!(choice.values[choice.selected]));
        }
        for switch in &self.switches {
            params.insert(switch.key.into(), json!(switch.enabled));
        }
        Value::Object(params)
    }

    fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        self.error = None;
        match page {
            Page::Watchlist => self.send("watchlist.load", self.launcher_params(cx)),
            Page::Library => self.scan_library(cx),
            Page::Settings => self.send("services.check", json!({})),
            Page::Spotify => {
                self.send("watchlist.load", self.launcher_params(cx));
                self.send("spotify.status", json!({}));
            }
            Page::Workflow => {}
        }
        cx.notify();
    }

    fn scan_library(&mut self, cx: &App) {
        let output = self
            .fields
            .iter()
            .find(|f| f.key == "output")
            .map(|f| f.state.read(cx).value().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| self.defaults["output"].as_str().unwrap_or("").to_string());
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
            "message" | "beets_log" => {
                let message = describe(&payload["message"]);
                self.job_status = message.clone();
                message
            }
            "error" | "beets_error" => {
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
            "beets_task" => {
                self.set_activity_section(activity_section(
                    "Beets matches",
                    &payload["task"]["matches"],
                    beets_match_summary,
                ));
                let task = &payload["task"];
                format!(
                    "Beets: {} · {}",
                    task["current_artist"].as_str().unwrap_or("Unknown artist"),
                    task["current_album"].as_str().unwrap_or("Unknown album")
                )
            }
            "beets_import_started" => "Beets import started".into(),
            "beets_import_finished" => "Beets import finished".into(),
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
        title: String,
        command: &'static str,
        params: Value,
        cx: &mut Context<Self>,
    ) {
        self.confirmation = Some(PendingAction {
            title,
            command,
            params,
        });
        cx.notify();
    }

    fn confirmation_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(action) = &self.confirmation else {
            return div().into_any_element();
        };
        div()
            .flex()
            .items_center()
            .gap_3()
            .p_3()
            .bg(rgb(0xffedd5))
            .child(format!("Confirm: {}", action.title))
            .child(
                Button::new("confirm-action")
                    .danger()
                    .label("Confirm")
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(action) = view.confirmation.take() {
                            if action.command == "watchlist.action" {
                                view.start_job(action.command, action.params, cx);
                            } else {
                                view.send(action.command, action.params);
                                cx.notify();
                            }
                        }
                    })),
            )
            .child(
                Button::new("cancel-action")
                    .label("Cancel")
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.confirmation = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
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
                    "library.scan" | "services.check" | "spotify.status" | "spotify.playlists"
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
                    if self.job_kind.as_deref() == Some(command.as_str()) {
                        self.job_status = self.status.clone();
                        self.job_kind = None;
                    }
                    return;
                }
                let result = &message["result"];
                match command.as_str() {
                    "hello" => {
                        self.status = "Python service ready".into();
                        self.defaults = result["defaults"].clone();
                        for field in &self.fields {
                            if matches!(field.key, "output" | "splits") {
                                if let Some(value) = self.defaults[field.key].as_str() {
                                    field.state.update(_cx, |state, cx| {
                                        state.set_value(value.to_string(), window, cx)
                                    });
                                }
                            }
                        }
                    }
                    "watchlist.load" | "watchlist.add" | "watchlist.remove"
                    | "watchlist.rename" => {
                        self.replace_watchlist(result["watchlist"].clone(), window, _cx);
                        self.cache_visible_thumbnails(_cx);
                        if command != "watchlist.load" {
                            self.send("watchlist.load", self.launcher_params(_cx));
                        }
                        self.status = "Watchlist ready".into();
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
                        self.send("watchlist.load", self.launcher_params(_cx));
                        if self.job_kind.as_deref() == Some("spotify.login") {
                            self.send("spotify.status", json!({}));
                            if event == "job.completed" {
                                self.send("spotify.playlists", json!({}));
                            }
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
                    .unwrap_or("Python service stopped")
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
            .filter(|item| {
                self.filter == 0
                    || item["summary"]
                        .as_str()
                        .or_else(|| item["status"].as_str())
                        .map(|state| state.eq_ignore_ascii_case(FILTERS[self.filter]))
                        .unwrap_or(false)
            })
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
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_6()
            .py_3()
            .border_b_1()
            .border_color(rgb(0xe2e8f0))
            .bg(rgb(0xffffff))
            .child(
                div()
                    .text_xl()
                    .font_semibold()
                    .text_color(rgb(0x17212f))
                    .mr_8()
                    .child("muzik"),
            );
        for (page, label) in [
            (Page::Workflow, "Workflow"),
            (Page::Watchlist, "Watchlist"),
            (Page::Library, "Library"),
            (Page::Settings, "Settings"),
            (Page::Spotify, "Spotify"),
        ] {
            let button = Button::new(label).label(label);
            row = row.child(
                if self.page == page {
                    button.primary()
                } else {
                    button
                }
                .on_click(cx.listener(move |view, _, _, cx| view.set_page(page, cx))),
            );
        }
        row.into_any_element()
    }

    fn workflow(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut source = div().v_flex().gap_3();
        let mut destinations = div().v_flex().gap_3();
        let mut tuning = div().flex().flex_wrap().gap_4();
        for (index, field) in self.fields.iter().enumerate() {
            let mut row = div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().child(Input::new(&field.state)));
            if matches!(field.key, "raw" | "output" | "splits" | "config") {
                row = row.child(Button::new(("pick-path", index)).label("Choose…").on_click(
                    cx.listener(move |view, _, window, cx| view.pick_path(index, window, cx)),
                ));
            }
            let field_view = div()
                .v_flex()
                .gap_1()
                .child(div().text_sm().font_semibold().child(field.label))
                .child(row);
            match field.key {
                "raw" => source = source.child(field_view),
                "jobs" | "min_bitrate" => {
                    tuning = tuning.child(div().w(px(180.)).child(field_view))
                }
                _ => destinations = destinations.child(field_view),
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
        let mut switches = div().flex().flex_wrap().gap_3();
        for (index, switch) in self.switches.iter().enumerate() {
            switches = switches.child(
                div().w(px(210.)).child(
                    Checkbox::new(("switch", index))
                        .label(switch.label)
                        .checked(switch.enabled)
                        .on_change(cx.listener(move |view, checked, _, cx| {
                            view.switches[index].enabled = *checked;
                            cx.notify();
                        })),
                ),
            );
        }
        let run = Button::new("run")
            .primary()
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
            .gap_5()
            .w_full()
            .max_w(px(860.))
            .py_8()
            .px_6()
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(div().text_2xl().font_semibold().child("Workflow"))
                    .child(
                        div()
                            .text_color(rgb(0x64748b))
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
            .child(
                GroupBox::new()
                    .id("workflow-destinations")
                    .title("DESTINATIONS")
                    .outline()
                    .child(destinations),
            )
            .child(
                GroupBox::new()
                    .id("workflow-quality")
                    .title("SOURCES AND QUALITY")
                    .outline()
                    .child(choices),
            )
            .child(
                GroupBox::new()
                    .id("workflow-processing")
                    .title("PROCESSING")
                    .outline()
                    .child(tuning)
                    .child(switches),
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
        let mut panel = div()
            .v_flex()
            .gap_4()
            .p_6()
            .w(px(320.))
            .h_full()
            .overflow_y_scrollbar()
            .border_l_1()
            .border_color(rgb(0xe2e8f0))
            .bg(rgb(0xf8fafc))
            .child(div().text_lg().font_semibold().child("Activity"))
            .child(
                GroupBox::new().id("activity-status").outline().child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(div().text_sm().text_color(rgb(0x64748b)).child("STATUS"))
                        .child(div().font_semibold().child(self.job_status.clone()))
                        .child(div().text_sm().child(self.progress.clone()))
                        .when(
                            self.job_kind.is_some() || !self.progress_state.description.is_empty(),
                            |this| {
                                this.child(
                                    Progress::new("activity-progress")
                                        .value(self.progress_state.percentage())
                                        .loading(
                                            self.progress_state.total.is_none()
                                                && self.job_kind.is_some(),
                                        )
                                        .accessibility_label("Workflow progress"),
                                )
                            },
                        ),
                ),
            );
        if let Some(id) = &self.job_id {
            let id = id.clone();
            panel = panel.child(Button::new("cancel-job").danger().label("Cancel").on_click(
                cx.listener(move |view, _, _, cx| {
                    view.send("job.cancel", json!({"job_id":id}));
                    cx.notify();
                }),
            ));
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
                        .text_color(rgb(0x475569))
                        .child(row.clone()),
                );
            }
            if section.count > section.rows.len() {
                summary = summary.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x64748b))
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
            .gap_2()
            .min_h(px(120.))
            .max_h(px(220.))
            .overflow_y_scrollbar();
        if self.logs.is_empty() {
            log = log.child(
                div()
                    .text_sm()
                    .text_color(rgb(0x64748b))
                    .child("Job updates will appear here."),
            );
        }
        for (index, line) in self.logs.iter().rev().take(100).enumerate() {
            log = log.child(div().id(("log", index)).text_sm().child(line.clone()));
        }
        panel
            .child(div().text_sm().font_semibold().child("Recent events"))
            .child(log)
            .into_any_element()
    }

    fn watchlist(&self, cx: &mut Context<Self>) -> AnyElement {
        let playlists = self.watchlist["playlists"]
            .as_array()
            .or_else(|| self.watchlist.as_array());
        let mut rail = div()
            .v_flex()
            .gap_3()
            .p_5()
            .w(px(260.))
            .h_full()
            .border_r_1()
            .border_color(rgb(0xe2e8f0))
            .bg(rgb(0xffffff))
            .child(div().text_lg().font_semibold().child("Sources"));
        if let Some(playlists) = playlists {
            for (index, playlist) in playlists.iter().enumerate() {
                let title = playlist["title"]
                    .as_str()
                    .or_else(|| playlist["playlist_id"].as_str())
                    .unwrap_or("Playlist");
                let rename_title = playlist["title"].as_str().unwrap_or("").to_string();
                let detail = format!(
                    "{} · {} items · {}",
                    playlist["kind"].as_str().unwrap_or("source"),
                    playlist["items"].as_array().map_or(0, Vec::len),
                    playlist["last_checked_at"]
                        .as_str()
                        .unwrap_or("Not checked")
                );
                let mut entry = div()
                    .v_flex()
                    .gap_1()
                    .child(
                        Button::new(("playlist", index))
                            .label(title.to_string())
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.selected_playlist = index;
                                view.watch_page = 0;
                                view.playlist_name.update(cx, |state, cx| {
                                    state.set_value(rename_title.clone(), window, cx)
                                });
                                view.cache_visible_thumbnails(cx);
                                cx.notify();
                            })),
                    )
                    .child(div().text_sm().child(detail));
                if let Some(error) = playlist["last_error"].as_str() {
                    entry = entry.child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xb91c1c))
                            .child(error.to_string()),
                    );
                }
                rail = rail.child(entry);
            }
        }
        rail = rail.child(Input::new(&self.watch_url)).child(
            Button::new("add-playlist")
                .primary()
                .label("Add playlist")
                .on_click(cx.listener(|view, _, _, cx| {
                    let url = view.watch_url.read(cx).value().to_string();
                    if !url.trim().is_empty() {
                        view.send("watchlist.add", json!({"url":url}));
                        cx.notify();
                    }
                })),
        );
        let mut content = div()
            .v_flex()
            .gap_3()
            .p_6()
            .flex_1()
            .max_w(px(1060.))
            .overflow_y_scrollbar()
            .child(div().text_2xl().font_semibold().child("Watchlist"));
        if playlists.is_none_or(Vec::is_empty) {
            let loading = self
                .pending
                .values()
                .any(|command| command == "watchlist.load");
            content = content.child(GroupBox::new().id("watchlist-empty").outline().child(
                if loading {
                    "Loading sources…"
                } else {
                    "Add a YouTube or Spotify playlist link to start."
                },
            ));
        }
        let refresh = self.launcher_params(cx);
        content = content.child(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("watch-refresh")
                        .primary()
                        .label("Refresh")
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.start_job("watchlist.refresh", refresh.clone(), cx)
                        })),
                )
                .child(
                    Button::new("watch-reload")
                        .label("Reload")
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.send("watchlist.load", json!({}));
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("cache-thumbnails")
                        .label("Load thumbnails")
                        .on_click(cx.listener(|view, _, _, cx| {
                            let video_ids = view.visible_thumbnail_ids();
                            if video_ids.is_empty() {
                                view.status = "No missing thumbnails on this page".into();
                                cx.notify();
                            } else {
                                for video_id in &video_ids {
                                    view.thumbnail_attempted.insert(video_id.clone());
                                }
                                view.send("thumbnails.cache", json!({"video_ids":video_ids}));
                                cx.notify();
                            }
                        })),
                ),
        );
        if let Some(playlists) = playlists {
            if let Some(playlist) = playlists.get(self.selected_playlist) {
                let id = playlist["id"]
                    .as_str()
                    .or_else(|| playlist["playlist_id"].as_str())
                    .unwrap_or("")
                    .to_string();
                let title = playlist["title"]
                    .as_str()
                    .or_else(|| playlist["playlist_id"].as_str())
                    .unwrap_or("Playlist");
                let remove_title = title.to_string();
                let source_url = playlist["url"].as_str().unwrap_or("").to_string();
                let open_url = source_url.clone();
                content = content
                    .child(div().text_lg().font_semibold().child(title.to_string()))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("open-source").label("Open source").on_click(
                                    cx.listener(move |_, _, _, cx| cx.open_url(&open_url)),
                                ),
                            )
                            .child(
                                Button::new("copy-source")
                                    .label("Copy source link")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            source_url.clone(),
                                        ))
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(Input::new(&self.playlist_name))
                            .child(Button::new("rename-playlist").label("Rename").on_click(
                                cx.listener({
                                    let id = id.clone();
                                    move |view, _, _, cx| {
                                        let title = view.playlist_name.read(cx).value().to_string();
                                        if !title.trim().is_empty() {
                                            view.send(
                                                "watchlist.rename",
                                                json!({"playlist_id":id,"title":title}),
                                            );
                                            cx.notify();
                                        }
                                    }
                                }),
                            )),
                    )
                    .child(
                        Button::new("remove-playlist")
                            .danger()
                            .label("Remove playlist")
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.request_action(
                                    format!("Remove {remove_title} from the watchlist?"),
                                    "watchlist.remove",
                                    json!({"playlist_id":id}),
                                    cx,
                                );
                            })),
                    );
                let items = playlist["items"].as_array();
                let mut filters = div().flex().flex_wrap().gap_1();
                for (index, label) in FILTERS.iter().enumerate() {
                    let button =
                        Button::new(("filter", index))
                            .label(*label)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.filter = index;
                                view.watch_page = 0;
                                view.cache_visible_thumbnails(cx);
                                cx.notify();
                            }));
                    filters = filters.child(if self.filter == index {
                        button.primary()
                    } else {
                        button
                    });
                }
                content = content.child(filters);
                if let Some(items) = items {
                    let filtered: Vec<(usize, &Value)> = items
                        .iter()
                        .enumerate()
                        .filter(|(_, item)| {
                            self.filter == 0
                                || item["summary"]
                                    .as_str()
                                    .or_else(|| item["status"].as_str())
                                    .map(|state| state.eq_ignore_ascii_case(FILTERS[self.filter]))
                                    .unwrap_or(false)
                        })
                        .collect();
                    let page_count = filtered.len().div_ceil(WATCH_PAGE_SIZE).max(1);
                    let current = self.watch_page.min(page_count - 1);
                    if filtered.is_empty() {
                        let message = if playlist["kind"] == "spotify" && items.is_empty() {
                            "This Spotify source has no tracks. Refresh it to read track names. Set Audio source to Soulseek in Workflow to get audio."
                                .to_string()
                        } else if items.is_empty() {
                            if playlist["last_checked_at"].is_null() {
                                "This playlist has not been checked. Select Refresh to read it."
                                    .to_string()
                            } else {
                                "This playlist has no videos. Refresh it to check again."
                                    .to_string()
                            }
                        } else {
                            format!(
                                "No items have the {} status. Select All to see every item.",
                                FILTERS[self.filter]
                            )
                        };
                        content = content.child(
                            GroupBox::new()
                                .id("watchlist-items-empty")
                                .outline()
                                .child(message),
                        );
                    }
                    for (_, item) in filtered
                        .into_iter()
                        .skip(current * WATCH_PAGE_SIZE)
                        .take(WATCH_PAGE_SIZE)
                    {
                        content = content.child(self.watch_item(item, playlist, cx));
                    }
                    content = content.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("previous")
                                    .label("Previous")
                                    .disabled(current == 0)
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.watch_page = view.watch_page.saturating_sub(1);
                                        view.cache_visible_thumbnails(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(format!("Page {} of {}", current + 1, page_count))
                            .child(
                                Button::new("next")
                                    .label("Next")
                                    .disabled(current + 1 >= page_count)
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.watch_page = (view.watch_page + 1).min(page_count - 1);
                                        view.cache_visible_thumbnails(cx);
                                        cx.notify();
                                    })),
                            ),
                    );
                }
            }
        }
        div()
            .flex()
            .size_full()
            .child(rail)
            .child(content)
            .into_any_element()
    }

    fn watch_item(&self, item: &Value, playlist: &Value, cx: &mut Context<Self>) -> AnyElement {
        let title = item["title"].as_str().unwrap_or("Untitled");
        let position = item["position"].as_u64().unwrap_or(0) as usize;
        let video_id = item["video_id"]
            .as_str()
            .or_else(|| item["id"].as_str())
            .unwrap_or("")
            .to_string();
        let playlist_id = playlist["id"]
            .as_str()
            .or_else(|| playlist["playlist_id"].as_str())
            .unwrap_or("")
            .to_string();
        let mut card = div()
            .v_flex()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(rgb(0xcbd5e1))
            .rounded_md()
            .child(div().font_semibold().child(title.to_string()))
            .child(format!(
                "Status: {}",
                item["summary"]
                    .as_str()
                    .or_else(|| item["status"].as_str())
                    .unwrap_or("Pending")
            ));
        let source_label = if item["kind"] == "spotify" {
            "Spotify ID"
        } else {
            "YouTube ID"
        };
        let source_id = item["video_id"].as_str().unwrap_or("Unavailable");
        card = card.child(
            div()
                .text_sm()
                .text_color(rgb(0x64748b))
                .child(format!("{source_label}: {source_id}")),
        );
        if let Some(error) = item["last_error"].as_str() {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(rgb(0xb91c1c))
                    .child(error.to_string()),
            );
        }
        if let Some(url) = item["video_url"].as_str() {
            let url = url.to_string();
            card = card.child(
                Button::new(("open-item", position))
                    .label("Open item link")
                    .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
            );
        }
        if let Some(path) = item["thumbnail_path"].as_str() {
            card = card.child(
                img(PathBuf::from(path))
                    .w(px(180.))
                    .h(px(100.))
                    .object_fit(ObjectFit::Cover),
            );
        }
        let mut stages = div().flex().gap_2();
        for stage in ["download", "quality", "parse", "split", "organize"] {
            let status = item["stages"][stage]["status"]
                .as_str()
                .unwrap_or("Not started");
            stages = stages.child(div().text_sm().child(format!("{stage}: {status}")));
        }
        card = card.child(stages);
        if let (Some(action), Some(label)) = (
            item["primary_action"]["action"].as_str(),
            item["primary_action"]["label"].as_str(),
        ) {
            let enabled = item["actions"][action]["enabled"].as_bool().unwrap_or(true);
            let mut params = self
                .launcher_params(cx)
                .as_object()
                .cloned()
                .unwrap_or_default();
            params.insert("playlist_id".into(), json!(playlist_id));
            params.insert("position".into(), json!(position));
            params.insert("video_id".into(), json!(video_id));
            params.insert("action".into(), json!(action));
            card = card.child(
                Button::new(("primary-action", position))
                    .primary()
                    .label(label.to_string())
                    .disabled(!enabled)
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.start_job("watchlist.action", Value::Object(params.clone()), cx);
                    })),
            );
        }
        let mut actions = div().flex().flex_wrap().gap_1();
        for (action, label) in ITEM_ACTIONS {
            let action = *action;
            if matches!(action, "run" | "retry") {
                continue;
            }
            let label = *label;
            let item_title = title.to_string();
            let availability = &item["actions"][action];
            let enabled = availability["enabled"].as_bool().unwrap_or(true);
            let params = self.launcher_params(cx);
            let mut params = params.as_object().cloned().unwrap_or_default();
            params.insert("playlist_id".into(), json!(playlist_id));
            params.insert("position".into(), json!(position));
            params.insert("video_id".into(), json!(video_id));
            params.insert("action".into(), json!(action));
            actions = actions.child(
                Button::new((
                    "item-action",
                    position * ITEM_ACTIONS.len()
                        + ITEM_ACTIONS
                            .iter()
                            .position(|(name, _)| *name == action)
                            .unwrap_or(0),
                ))
                .label(label)
                .disabled(!enabled)
                .on_click(cx.listener(move |view, _, _, cx| {
                    let params = Value::Object(params.clone());
                    if matches!(
                        action,
                        "download_again"
                            | "parse_again"
                            | "split_again"
                            | "organize_again"
                            | "run_all_again"
                    ) {
                        view.request_action(
                            format!("{label} for {item_title}?"),
                            "watchlist.action",
                            params,
                            cx,
                        );
                    } else {
                        view.start_job("watchlist.action", params, cx);
                    }
                })),
            );
            if !enabled {
                if let Some(reason) = availability["reason"].as_str() {
                    actions = actions.child(div().text_sm().child(format!("{label}: {reason}")));
                }
            }
        }
        card.child(actions).into_any_element()
    }

    fn library(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut page = div()
            .v_flex()
            .gap_4()
            .p_8()
            .w_full()
            .max_w(px(960.))
            .child(div().text_2xl().font_semibold().child("Downloaded audio"))
            .child(
                Button::new("library-refresh")
                    .label("Refresh")
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.scan_library(cx);
                        cx.notify();
                    })),
            );
        let items = self.library["items"]
            .as_array()
            .or_else(|| self.library.as_array());
        if let Some(items) = items {
            page = page
                .child(self.library["output"].as_str().unwrap_or("").to_string())
                .child(format!(
                    "{} files, {}",
                    items.len(),
                    self.library["total_size"].as_str().unwrap_or("0 B")
                ));
            for (index, item) in items.iter().enumerate() {
                let title = item["title"].as_str().unwrap_or("Audio file");
                let detail = format!(
                    "{}  •  {}  •  {}  •  {}",
                    item["ext"].as_str().unwrap_or(""),
                    item["size_label"].as_str().unwrap_or(""),
                    item["modified"].as_str().unwrap_or(""),
                    item["youtube_id"].as_str().unwrap_or("No YouTube ID")
                );
                page = page.child(
                    div()
                        .id(("library-item", index))
                        .v_flex()
                        .p_2()
                        .border_b_1()
                        .border_color(rgb(0xe2e8f0))
                        .child(title.to_string())
                        .child(detail),
                );
            }
        } else {
            page = page.child("No downloads found.");
        }
        div()
            .flex()
            .justify_center()
            .flex_1()
            .overflow_y_scrollbar()
            .child(page)
            .into_any_element()
    }

    fn settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut page = div()
            .v_flex()
            .gap_4()
            .p_8()
            .w_full()
            .max_w(px(960.))
            .child(
                div()
                    .text_2xl()
                    .font_semibold()
                    .child("Service availability"),
            )
            .child(
                Button::new("service-refresh")
                    .label("Re-check")
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.send("services.check", json!({}));
                        cx.notify();
                    })),
            );
        let services = self.services["services"]
            .as_array()
            .or_else(|| self.services.as_array());
        if let Some(services) = services {
            for (index, service) in services.iter().enumerate() {
                page = page.child(
                    div()
                        .id(("service", index))
                        .flex()
                        .gap_4()
                        .p_2()
                        .border_b_1()
                        .border_color(rgb(0xe2e8f0))
                        .child(service["name"].as_str().unwrap_or("Service").to_string())
                        .child(match service["available"].as_bool() {
                            Some(true) => "Available",
                            Some(false) => "Unavailable",
                            None => "Not configured",
                        })
                        .child(service["detail"].as_str().unwrap_or("").to_string()),
                );
            }
        }
        div()
            .flex()
            .justify_center()
            .flex_1()
            .overflow_y_scrollbar()
            .child(page)
            .into_any_element()
    }

    fn spotify(&self, cx: &mut Context<Self>) -> AnyElement {
        let saved_ids: HashSet<&str> = self.watchlist["playlists"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|playlist| playlist["playlist_id"].as_str())
            .collect();
        let liked_saved = saved_ids.contains("spotify:liked");
        let mut page = div()
            .v_flex()
            .gap_4()
            .p_8()
            .w_full()
            .max_w(px(960.))
            .child(div().text_2xl().font_semibold().child("Spotify"))
            .child("Set a Spotify application client ID, then connect your account.")
            .child(
                Button::new("spotify-dashboard")
                    .label("Open Spotify dashboard")
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.open_url("https://developer.spotify.com/dashboard")
                    })),
            )
            .child("Redirect URI:")
            .child(
                self.spotify["redirect_uri"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            )
            .child(
                Button::new("copy-redirect")
                    .label("Copy redirect URI")
                    .on_click(cx.listener(|view, _, _, cx| {
                        let uri = view.spotify["redirect_uri"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        cx.write_to_clipboard(ClipboardItem::new_string(uri));
                    })),
            )
            .child(Input::new(&self.spotify_client_id))
            .child(
                Button::new("spotify-save")
                    .label("Save client ID")
                    .on_click(cx.listener(|view, _, _, cx| {
                        let client_id = view.spotify_client_id.read(cx).value().to_string();
                        view.send("spotify.set_client_id", json!({"client_id":client_id}));
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("spotify-connect")
                            .primary()
                            .label("Connect")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.start_job("spotify.login", json!({}), cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("spotify-disconnect")
                            .label("Disconnect")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.send("spotify.logout", json!({}));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("spotify-reload")
                            .label("Reload playlists")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.send("spotify.playlists", json!({}));
                                cx.notify();
                            })),
                    ),
            );
        page = page.child(format!(
            "Account: {}",
            self.spotify["account_name"]
                .as_str()
                .unwrap_or("Not connected")
        ));
        if let Some(error) = self.spotify["error"].as_str() {
            page = page.child(div().text_color(rgb(0xb91c1c)).child(error.to_string()));
        }
        page = page.child(
            Button::new("spotify-liked")
                .primary()
                .label(if liked_saved {
                    "Liked Songs saved"
                } else {
                    "Add Liked Songs to watchlist"
                })
                .disabled(self.spotify["connected"] != true || liked_saved)
                .on_click(cx.listener(|view, _, _, cx| {
                    view.send("watchlist.add", json!({"url":"liked"}));
                    cx.notify();
                })),
        );
        if let Some(playlists) = self.spotify["playlists"].as_array() {
            for (index, playlist) in playlists.iter().enumerate() {
                let name = playlist["name"].as_str().unwrap_or("Playlist");
                let uri = playlist["uri"].as_str().unwrap_or("").to_string();
                if uri == "spotify:liked" {
                    continue;
                }
                let saved = saved_ids.contains(uri.as_str());
                let detail = format!(
                    "{} · {} tracks",
                    playlist["owner"].as_str().unwrap_or("Spotify"),
                    playlist["total"].as_u64().unwrap_or(0)
                );
                page = page.child(
                    div()
                        .id(("spotify-playlist", index))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().v_flex().child(name.to_string()).child(detail))
                        .child(
                            Button::new(("spotify-add", index))
                                .label(if saved { "Saved" } else { "Add to watchlist" })
                                .disabled(saved)
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.send("watchlist.add", json!({"url":uri}));
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        div()
            .flex()
            .justify_center()
            .flex_1()
            .overflow_y_scrollbar()
            .child(page)
            .into_any_element()
    }
}

impl Render for Muzik {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.page {
            Page::Workflow => self.workflow(cx),
            Page::Watchlist => self.watchlist(cx),
            Page::Library => self.library(cx),
            Page::Settings => self.settings(cx),
            Page::Spotify => self.spotify(cx),
        };
        let body =
            if self.page != Page::Workflow && (self.job_id.is_some() || self.decision.is_some()) {
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
            .bg(rgb(0xf8fafc))
            .child(self.header(cx))
            .child(body)
            .child(self.confirmation_view(cx))
            .child(
                div()
                    .p_2()
                    .bg(if self.error.is_some() {
                        rgb(0x991b1b)
                    } else {
                        rgb(0x18202a)
                    })
                    .text_color(rgb(0xffffff))
                    .text_sm()
                    .child(self.error.clone().unwrap_or_else(|| self.status.clone())),
            )
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

fn beets_match_summary(candidate: &Value) -> String {
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
            .filter(|item| {
                filter == 0
                    || item["summary"]
                        .as_str()
                        .is_some_and(|status| status.eq_ignore_ascii_case(FILTERS[filter]))
            })
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
        "beets_match" | "beets_duplicate" => {
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
            if decision["kind"] == "beets_duplicate" {
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
        "beets_match" => {
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
        "beets_duplicate" => ["skip", "keep_all", "remove_old", "merge"]
            .into_iter()
            .map(|value| (value.replace('_', " "), json!(value)))
            .collect(),
        _ => Vec::new(),
    }
}

fn main() {
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
        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let view = cx.new(|cx| Muzik::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("open main window");
        })
        .detach();
    });
}

fn check_backend() -> Result<(), String> {
    let mut bridge = Bridge::start()?;
    let id = bridge.send("hello", json!({}))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        for message in bridge.drain() {
            if message["type"] == "transport.closed" {
                return Err("Python service closed before hello".into());
            }
            if message["type"] == "response" && message["id"] == id {
                if message["ok"] == true && message["result"]["protocol_version"] == 1 {
                    println!("Python service ready (protocol 1)");
                    return Ok(());
                }
                return Err(format!(
                    "Python service rejected hello: {}",
                    describe(&message)
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err("Python service did not answer hello within 5 seconds".into())
}

#[cfg(test)]
mod tests {
    use super::{
        activity_section, candidate_summary, decision_choices, decision_details,
        merge_thumbnail_paths, ActivityProgress,
    };
    use serde_json::json;
    use std::collections::HashSet;

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
    fn beets_duplicate_review_shows_existing_file_and_reply_options() {
        let decision = json!({
            "kind": "beets_duplicate",
            "payload": {
                "task": {"current_artist": "Artist", "current_album": "Album", "paths": ["new.flac"]},
                "duplicates": [{"artist": "Artist", "album": "Album", "path": "old.flac"}]
            }
        });
        let details = decision_details(&decision);
        assert!(details.iter().any(|line| line.contains("old.flac")));
        assert!(details.iter().any(|line| line.contains("new.flac")));
        assert_eq!(decision_choices(&decision).len(), 4);
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
}
