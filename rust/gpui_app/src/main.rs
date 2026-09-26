mod bridge;

use bridge::Bridge;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::*;
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
    status: String,
    error: Option<String>,
    job_id: Option<String>,
    job_kind: Option<String>,
    completed_jobs: HashSet<String>,
    job_status: String,
    progress: String,
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
        let mut this = Self {
            page: Page::Workflow,
            fields,
            choices: CHOICES
                .iter()
                .map(|(key, label, values)| Choice {
                    key,
                    label,
                    values,
                    selected: 0,
                })
                .collect(),
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
            status: String::new(),
            error: None,
            job_id: None,
            job_kind: None,
            completed_jobs: HashSet::new(),
            job_status: "Ready".into(),
            progress: String::new(),
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
        match self
            .bridge
            .as_mut()
            .map(|bridge| bridge.send(command, params))
        {
            Some(Ok(id)) => {
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
        self.logs.clear();
        self.decision = None;
        self.send(command, params);
        cx.notify();
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
                        self.replace_watchlist(result["watchlist"].clone());
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
                            let playlists = self.spotify["playlists"].clone();
                            self.spotify = result.clone();
                            if self.spotify_client_id.read(_cx).value().is_empty() {
                                if let Some(client_id) = result["client_id"].as_str() {
                                    self.spotify_client_id.update(_cx, |state, cx| {
                                        state.set_value(client_id.to_string(), window, cx)
                                    });
                                }
                            }
                            if !playlists.is_null() && self.spotify["connected"] == true {
                                self.spotify["playlists"] = playlists;
                            }
                            if self.spotify["connected"] == true
                                && self.spotify["playlists"].is_null()
                            {
                                self.send("spotify.playlists", json!({}));
                            }
                        } else if command == "spotify.playlists" {
                            self.spotify["playlists"] = result["playlists"].clone();
                        } else {
                            self.send("spotify.status", json!({}));
                        }
                        self.status = "Spotify ready".into();
                    }
                    "workflow.start" | "watchlist.refresh" | "watchlist.action"
                    | "spotify.login" | "thumbnails.cache" => {
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
                        self.replace_watchlist(data["watchlist"].clone());
                        self.cache_visible_thumbnails(_cx);
                        self.status = "Watchlist updated".into();
                    }
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
                        self.job_status = kind.into();
                        self.progress = describe(payload);
                        self.logs.push(format!("{kind}: {}", describe(payload)));
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
                                    | "thumbnails.cache"
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
                        self.logs
                            .push(format!("{}: {}", self.job_status, describe(data)));
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

    fn replace_watchlist(&mut self, incoming: Value) {
        let old_id = self.watchlist["playlists"][self.selected_playlist]["playlist_id"]
            .as_str()
            .map(str::to_owned);
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
        if self.page != Page::Watchlist || self.job_kind.is_some() || self.job_id.is_some() {
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
        self.start_job("thumbnails.cache", json!({"video_ids":video_ids}), cx);
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
            .gap_2()
            .p_3()
            .bg(rgb(0x18202a))
            .child(
                div()
                    .text_xl()
                    .font_semibold()
                    .text_color(rgb(0xffffff))
                    .mr_4()
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
                    button.ghost()
                }
                .on_click(cx.listener(move |view, _, _, cx| view.set_page(page, cx))),
            );
        }
        row.into_any_element()
    }

    fn workflow(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut form = div()
            .v_flex()
            .gap_3()
            .p_5()
            .overflow_y_scrollbar()
            .flex_1()
            .child(div().text_2xl().font_semibold().child("Workflow"))
            .child(
                div()
                    .text_color(rgb(0x64748b))
                    .child("Download, split, and organize audio."),
            );
        for (index, field) in self.fields.iter().enumerate() {
            let mut row = div()
                .flex()
                .items_center()
                .gap_2()
                .child(Input::new(&field.state));
            if matches!(field.key, "raw" | "output" | "splits" | "config") {
                row = row.child(Button::new(("pick-path", index)).label("Choose…").on_click(
                    cx.listener(move |view, _, window, cx| view.pick_path(index, window, cx)),
                ));
            }
            form = form.child(div().v_flex().gap_1().child(field.label).child(row));
        }
        form = form.child(
            div()
                .text_lg()
                .font_semibold()
                .mt_3()
                .child("Sources and quality"),
        );
        for (index, choice) in self.choices.iter().enumerate() {
            let label = format!("{}: {}", choice.label, choice.values[choice.selected]);
            form = form.child(
                Button::new(("choice", index))
                    .label(label)
                    .on_click(cx.listener(move |view, _, _, cx| {
                        let choice = &mut view.choices[index];
                        choice.selected = (choice.selected + 1) % choice.values.len();
                        cx.notify();
                    })),
            );
        }
        form = form.child(div().text_lg().font_semibold().mt_3().child("Options"));
        for (index, switch) in self.switches.iter().enumerate() {
            let label = format!(
                "{} {}",
                if switch.enabled { "☑" } else { "☐" },
                switch.label
            );
            form = form.child(
                Button::new(("switch", index))
                    .label(label)
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.switches[index].enabled = !view.switches[index].enabled;
                        cx.notify();
                    })),
            );
        }
        form = form.child(Button::new("run").primary().label("Run workflow").on_click(
            cx.listener(|view, _, _, cx| {
                let params = view.launcher_params(cx);
                if params["raw"].as_str().unwrap_or("").trim().is_empty() {
                    view.status = "Enter a URL or local path".into();
                    cx.notify();
                    return;
                }
                view.start_job("workflow.start", params, cx);
            }),
        ));
        div()
            .flex()
            .flex_row()
            .size_full()
            .child(form)
            .child(self.job_panel(cx))
            .into_any_element()
    }

    fn job_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut panel = div()
            .v_flex()
            .gap_2()
            .p_4()
            .w(px(360.))
            .h_full()
            .bg(rgb(0xf1f5f9))
            .child(div().text_lg().font_semibold().child("Activity"))
            .child(self.job_status.clone())
            .child(self.progress.clone());
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
            panel = panel
                .child(div().font_semibold().child(format!(
                    "Choose: {}",
                    decision["kind"].as_str().unwrap_or("decision")
                )))
                .child(describe(&decision["payload"]));
            for (index, (label, value)) in decision_choices(decision).into_iter().enumerate() {
                panel =
                    panel.child(Button::new(("decision", index)).label(label).on_click(
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
                panel = panel
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
        }
        let mut log = div().v_flex().gap_1().overflow_y_scrollbar().flex_1();
        for (index, line) in self.logs.iter().rev().take(100).enumerate() {
            log = log.child(div().id(("log", index)).text_sm().child(line.clone()));
        }
        panel.child(log).into_any_element()
    }

    fn watchlist(&self, cx: &mut Context<Self>) -> AnyElement {
        let playlists = self.watchlist["playlists"]
            .as_array()
            .or_else(|| self.watchlist.as_array());
        let mut rail = div()
            .v_flex()
            .gap_2()
            .p_4()
            .w(px(240.))
            .h_full()
            .bg(rgb(0xf1f5f9))
            .child(div().text_lg().font_semibold().child("Playlists"));
        if let Some(playlists) = playlists {
            for (index, playlist) in playlists.iter().enumerate() {
                let title = playlist["title"]
                    .as_str()
                    .or_else(|| playlist["name"].as_str())
                    .unwrap_or("Playlist");
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
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.selected_playlist = index;
                                view.watch_page = 0;
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
            .p_4()
            .flex_1()
            .overflow_y_scrollbar()
            .child(div().text_2xl().font_semibold().child("Watchlist"));
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
                                view.start_job(
                                    "thumbnails.cache",
                                    json!({"video_ids":video_ids}),
                                    cx,
                                );
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
                    .or_else(|| playlist["name"].as_str())
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
                content = content.child(
                    Button::new("filter")
                        .label(format!("Filter: {}", FILTERS[self.filter]))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.filter = (view.filter + 1) % FILTERS.len();
                            view.watch_page = 0;
                            view.cache_visible_thumbnails(cx);
                            cx.notify();
                        })),
                );
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
                    for (_, item) in filtered
                        .into_iter()
                        .skip(current * WATCH_PAGE_SIZE)
                        .take(WATCH_PAGE_SIZE)
                    {
                        content = content.child(self.watch_item(item, playlist, cx));
                    }
                    content =
                        content.child(
                            div()
                                .flex()
                                .gap_2()
                                .child(Button::new("previous").label("Previous").on_click(
                                    cx.listener(|view, _, _, cx| {
                                        view.watch_page = view.watch_page.saturating_sub(1);
                                        view.cache_visible_thumbnails(cx);
                                        cx.notify();
                                    }),
                                ))
                                .child(format!("Page {} of {}", current + 1, page_count))
                                .child(Button::new("next").label("Next").on_click(cx.listener(
                                    move |view, _, _, cx| {
                                        view.watch_page = (view.watch_page + 1).min(page_count - 1);
                                        view.cache_visible_thumbnails(cx);
                                        cx.notify();
                                    },
                                ))),
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
            .gap_3()
            .p_5()
            .overflow_y_scrollbar()
            .size_full()
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
        page.into_any_element()
    }

    fn settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut page = div()
            .v_flex()
            .gap_3()
            .p_5()
            .overflow_y_scrollbar()
            .size_full()
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
        page.into_any_element()
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
            .gap_3()
            .p_5()
            .overflow_y_scrollbar()
            .size_full()
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
        page.into_any_element()
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
            .bg(rgb(0xffffff))
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

fn decision_choices(decision: &Value) -> Vec<(String, Value)> {
    let payload = &decision["payload"];
    match decision["kind"].as_str().unwrap_or("") {
        "soulseek_candidate" => payload["candidates"]
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
            .collect(),
        "chapter_review" => ["accept", "edit", "reject"]
            .into_iter()
            .map(|value| (value.to_string(), json!(value)))
            .collect(),
        "chapter_edit" => vec![
            ("Keep these chapters".into(), payload["chapters"].clone()),
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
                                "{} — {}",
                                candidate["artist"].as_str().unwrap_or("Unknown artist"),
                                candidate["album"]
                                    .as_str()
                                    .or_else(|| candidate["title"].as_str())
                                    .unwrap_or("Unknown release")
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
