mod activity;
mod backend;
mod library;
mod pages;
mod requests;
mod settings;
mod style;
mod thumbnails;
mod watch_table;
mod watchlist_view;

use activity::{
    Activity, ActivitySection, Ending, Finished, ReplyTarget, Run, RunKind, job_label, short_text,
    u64_to_f64,
};
use async_channel::Receiver;
use backend::{Backend, ItemRequest};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::description_list::DescriptionList;
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::table::{TableEvent, TableState};
use gpui_kit::component::theme::Theme;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, Axis, Div, Entity, Image, KeyBinding, Menu, MenuItem,
    ObjectFit, PathPromptOptions, Pixels, SharedString, Stateful, Task, WeakEntity, Window,
    WindowOptions, actions, div, img, px,
};
use muzik_core::app_config::GuiDefaults;
use muzik_core::{
    AudioFallback, AudioSource, DecisionKind, DuplicatePolicy, MetadataSource, QualityPolicy,
};
use muzik_runner::AppEvent;
use muzik_runner::choices::{self, Choice as DecisionChoice};
use muzik_runner::gates::Gate;
use muzik_store::watchlist::{
    ItemAction, ItemId, ItemView, PlaylistView, SourceKind, Summary, WatchlistView,
};
use requests::{Command, PendingAction, Read};
use serde_json::{Value, json};
use settings::ConfigView;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

actions!(
    muzik,
    [
        #[derive(Eq)]
        Quit,
        #[derive(Eq)]
        OpenSelected,
        #[derive(Eq)]
        FocusNext,
        #[derive(Eq)]
        FocusPrev
    ]
);

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

struct ChapterRow {
    index: Entity<InputState>,
    start: Entity<InputState>,
    end: Entity<InputState>,
    title: Entity<InputState>,
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "std has no TryFrom<f64> for integers; `as` saturates and maps NaN to zero"
)]
const fn f64_to_u64(value: f64) -> u64 {
    value as u64
}

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

const fn action_label(action: ItemAction) -> &'static str {
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
    window: AnyWindowHandle,
    backend: Option<Arc<Backend>>,
    events: Option<Task<()>>,
    reads: HashMap<Read, u64>,
    read_serial: u64,
    status: String,
    error: Option<String>,
    activity: Activity,
    gates: Value,
    reload_watchlist: bool,
    watchlist: WatchlistView,
    selected_playlist: usize,
    filter: usize,
    watch_table: Entity<TableState<watch_table::WatchTable>>,
    sheet_item: Option<String>,
    thumbnail_attempted: HashSet<String>,
    library: Option<library::LibraryInfo>,
    library_table: Entity<TableState<library::LibraryTable>>,
    library_search: Entity<InputState>,
    services: Value,
    spotify: Value,
    defaults: Option<GuiDefaults>,
    config_status: Rc<RefCell<String>>,
}

impl Muzik {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_backend(window, cx, true)
    }

    fn new_with_backend(window: &mut Window, cx: &mut Context<Self>, start_backend: bool) -> Self {
        let view = cx.entity().downgrade();
        let watch_table = cx.new(|cx| {
            TableState::new(watch_table::WatchTable::new(view), window, cx)
                .col_movable(false)
                .row_selectable(true)
        });
        cx.subscribe_in(&watch_table, window, |view, table, event, window, cx| {
            if let TableEvent::DoubleClickedRow(row) = event
                && let Some(key) = table.read(cx).delegate().key(*row)
            {
                view.open_item_sheet(key, window, cx);
            }
        })
        .detach();
        let library_table = cx.new(|cx| {
            TableState::new(library::LibraryTable::new(), window, cx)
                .col_movable(false)
                .row_selectable(true)
        });
        cx.subscribe_in(&library_table, window, |_, table, event, window, cx| {
            if let TableEvent::DoubleClickedRow(row) = event
                && let Some(album) = table.read(cx).delegate().album(*row)
            {
                Self::open_album_sheet(album, window, cx);
            }
        })
        .detach();
        let library_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search artist, album, genre, label, or year")
        });
        cx.subscribe(&library_search, |view, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                view.filter_library(cx);
                cx.notify();
            }
        })
        .detach();
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
            window: window.window_handle(),
            backend: None,
            events: None,
            reads: HashMap::new(),
            read_serial: 0,
            status: String::new(),
            error: None,
            activity: Activity::default(),
            gates: Value::Null,
            reload_watchlist: false,
            watchlist: WatchlistView::default(),
            selected_playlist: 0,
            filter: 0,
            watch_table,
            sheet_item: None,
            thumbnail_attempted: HashSet::new(),
            library: None,
            library_table,
            library_search,
            services: Value::Null,
            spotify: Value::Null,
            defaults: None,
            config_status: Rc::new(RefCell::new(String::new())),
        };
        if start_backend {
            match Backend::start() {
                Ok((backend, events)) => {
                    this.backend = Some(Arc::new(backend));
                    this.events = Some(Self::listen(events, window, cx));
                    this.hello(cx);
                    this.load_jobs(cx);
                }
                Err(error) => this.status = error.to_string(),
            }
        }
        cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        })
        .detach();
        this
    }

    fn listen(events: Receiver<AppEvent>, window: &Window, cx: &Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |view, cx| {
            while let Ok(event) = events.recv().await {
                let mut batch = vec![event];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                if view
                    .update_in(cx, |view, window, cx| view.app_events(batch, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    fn launcher_params(&self, cx: &App) -> Value {
        let mut params = match serde_json::to_value(&self.defaults) {
            Ok(Value::Object(params)) => params,
            _ => serde_json::Map::new(),
        };
        params.insert("raw".into(), json!(self.raw.read(cx).value().to_string()));
        Value::Object(params)
    }

    fn apply_defaults(&mut self, defaults: GuiDefaults, cx: &mut Context<Self>) {
        self.defaults = Some(defaults);
        cx.notify();
    }

    fn open_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(defaults) = self.defaults.clone() else {
            self.status = "Config is loading".into();
            cx.notify();
            return;
        };
        if self.config_view.is_none() {
            let main = cx.entity();
            let status = self.config_status.clone();
            self.config_view =
                Some(cx.new(|cx| ConfigView::new(&main, defaults, status, window, cx)));
            self.load_accounts(cx);
        }
        self.page = Page::Settings;
        self.error = None;
        self.check_services(cx);
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
            Page::Watchlist => self.load_watchlist(cx),
            Page::Library => self.load_library(cx),
            Page::Spotify => {
                self.load_watchlist(cx);
                self.spotify_status(cx);
            }
            Page::Workflow | Page::Settings => {}
        }
        cx.notify();
    }

    fn request_action(action: PendingAction, window: &mut Window, cx: &mut Context<Self>) {
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

    fn pick_source(window: &Window, cx: &Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                let value = path.to_string_lossy().into_owned();
                let _ = view.update_in(cx, |view, window, cx| {
                    view.raw
                        .update(cx, |state, cx| state.set_value(value, window, cx));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn app_events(&mut self, events: Vec<AppEvent>, window: &mut Window, cx: &mut Context<Self>) {
        for event in events {
            self.app_event(event, window, cx);
        }
        if std::mem::take(&mut self.reload_watchlist) {
            self.load_watchlist(cx);
        }
        cx.notify();
    }

    fn app_event(&mut self, event: AppEvent, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.activity.decision().cloned();
        let finished = self.activity.apply(&event);
        if previous.as_ref() != self.activity.decision() {
            self.prepare_decision(window, cx);
        }
        if let Some(finished) = finished {
            self.job_finished(finished, window, cx);
        }
        match event {
            AppEvent::WatchlistUpdated(watchlist) => {
                self.replace_watchlist(watchlist, window, cx);
                self.cache_visible_thumbnails(cx);
                self.status = "Watchlist updated".into();
            }
            AppEvent::WatchlistError(message) => self.error = Some(message),
            AppEvent::WatchlistSaved => self.reload_watchlist = true,
            AppEvent::JobsUpdated(_) => {
                self.sync_watch_table(cx);
            }
            AppEvent::QueuesUpdated(data) => self.gates = data,
            AppEvent::RemoteRunner(message) => self.status = message,
            AppEvent::JobStarted { .. }
            | AppEvent::JobEvent { .. }
            | AppEvent::DecisionRequest { .. }
            | AppEvent::JobCompleted { .. }
            | AppEvent::JobCancelled { .. }
            | AppEvent::JobFailed { .. } => {}
        }
    }

    fn job_finished(&mut self, finished: Finished, window: &mut Window, cx: &mut Context<Self>) {
        let Finished {
            kind,
            title,
            ending,
            failure,
        } = finished;
        let job = job_label(kind, &title);
        let note = match (ending, &failure) {
            (Ending::Failed, failure) => Some(
                Notification::error(failure.clone().unwrap_or_default())
                    .title(format!("{job} failed")),
            ),
            (Ending::Cancelled, _) => Some(Notification::warning(format!("{job} cancelled"))),
            (Ending::Completed, _) if kind != RunKind::Item => {
                Some(Notification::success(format!("{job} finished")))
            }
            (Ending::Completed, _) => None,
        };
        if let Some(note) = note {
            window.push_notification(note, cx);
        }
        if kind == RunKind::SpotifyLogin {
            self.spotify_status(cx);
            if ending == Ending::Completed {
                self.spotify_playlists(cx);
            }
        } else {
            self.reload_watchlist = true;
        }
        if let Some(failure) = failure {
            self.error = Some(failure);
        }
    }

    fn replace_watchlist(
        &mut self,
        incoming: WatchlistView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old = self.watchlist.playlists.get(self.selected_playlist);
        let old_id = old.map(|playlist| playlist.playlist_id.as_str());
        let old_title = old
            .and_then(|playlist| playlist.title.as_deref())
            .unwrap_or("");
        let playlists = &incoming.playlists;
        let selected = old_id
            .and_then(|id| {
                playlists
                    .iter()
                    .position(|playlist| playlist.playlist_id == id)
            })
            .unwrap_or_else(|| {
                self.selected_playlist
                    .min(playlists.len().saturating_sub(1))
            });
        self.selected_playlist = selected;
        let playlist = playlists.get(selected);
        let name = playlist
            .and_then(|playlist| playlist.title.as_deref())
            .unwrap_or("");
        if old_id != playlist.map(|playlist| playlist.playlist_id.as_str()) || old_title != name {
            self.playlist_name.update(cx, |state, cx| {
                state.set_value(name.to_string(), window, cx);
            });
        }
        self.watchlist = incoming;
        self.sync_watch_table(cx);
    }

    fn sync_watch_table(&self, cx: &mut Context<Self>) {
        let rows = self
            .watchlist
            .playlists
            .get(self.selected_playlist)
            .map(|playlist| watch_table::rows(playlist, self.filter, self.activity.queued_items()))
            .unwrap_or_default();
        self.watch_table.update(cx, |state, cx| {
            state.delegate_mut().set_rows(rows);
            cx.notify();
        });
    }

    fn visible_thumbnail_ids(&self) -> Vec<String> {
        let Some(open) = self.sheet_item.as_deref() else {
            return Vec::new();
        };
        self.watchlist
            .playlists
            .get(self.selected_playlist)
            .into_iter()
            .flat_map(|playlist| &playlist.items)
            .filter(|item| item.item.video_id.as_deref() == Some(open))
            .filter(|item| item.thumbnail_path.is_none() && item.item.thumbnail_url.is_some())
            .filter_map(|item| item.item.video_id.clone())
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
        self.cache_thumbnails(video_ids, cx);
        cx.notify();
    }

    fn merge_thumbnail_results(&mut self, data: &Value) {
        let visible: HashSet<String> = self.visible_thumbnail_ids().into_iter().collect();
        merge_thumbnail_paths(&mut self.watchlist, &visible, data);
    }

    fn prepare_decision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chapter_rows.clear();
        let Some(data) = self.activity.decision() else {
            return;
        };
        if choices::kind(data) == Some(DecisionKind::ChapterEdit)
            && let Some(chapters) = data.pointer("/payload/chapters").and_then(Value::as_array)
        {
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

    fn open_waiting(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.activity.open_waiting(id);
        self.prepare_decision(window, cx);
        cx.notify();
    }

    fn reply(&mut self, value: Value, window: &mut Window, cx: &mut Context<Self>) {
        match self.activity.reply() {
            Some(ReplyTarget::Live(id)) => self.send_reply(id, value, cx),
            Some(ReplyTarget::Queue(id)) => self.send_answer(Some(id), value, cx),
            None => return,
        }
        self.prepare_decision(window, cx);
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
                        row_number.saturating_add(1)
                    );
                    cx.notify();
                    return;
                }
            }
        }
        self.reply(Value::Array(chapters), window, cx);
    }

    fn header(&self, cx: &Context<Self>) -> AnyElement {
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

    fn status_bar(&self, cx: &Context<Self>) -> AnyElement {
        let (text, color) = match &self.error {
            Some(error) => (error.clone(), cx.theme().danger),
            None if self.status.is_empty() => ("Ready".to_string(), cx.theme().muted_foreground),
            None => (self.status.clone(), cx.theme().muted_foreground),
        };
        let output = self
            .defaults
            .as_ref()
            .map(|defaults| defaults.output.display().to_string())
            .unwrap_or_default();
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

    fn workflow(&self, cx: &Context<Self>) -> AnyElement {
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
                            cx.listener(|_, _, window, cx| Self::pick_source(window, cx)),
                        )),
                );
        let run = Button::new("run")
            .primary()
            .icon(IconName::Play)
            .label("Run workflow")
            .on_click(cx.listener(|view, _, _, cx| {
                let params = view.launcher_params(cx);
                if params
                    .get("raw")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    view.status = "Enter a URL or local path".into();
                    cx.notify();
                    return;
                }
                view.start_workflow(params, cx);
            }));
        let form = pages::page_frame()
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
        pages::page_scroll(form)
    }

    fn lanes(&self, cx: &App) -> Div {
        let mut lanes = div().v_flex().gap_1p5();
        for (key, label, gate) in [
            ("download", "Download", Gate::Download),
            ("process", "Process", Gate::Process),
            ("import", "Import", Gate::Import),
        ] {
            let lane = self.gates.get(key).unwrap_or(&Value::Null);
            let active: Vec<&str> = lane["active"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let waiting = lane["waiting"].as_array().map_or(0, Vec::len);
            let limit = gate.limit();
            let counts = if waiting > 0 {
                format!("{}/{limit} · {waiting} waiting", active.len())
            } else {
                format!("{}/{limit}", active.len())
            };
            let slots = limit.max(active.len());
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
                    .children((0..slots).map(|slot| {
                        style::meta(active.get(slot).copied().unwrap_or("Idle"), cx)
                            .truncate()
                            .when(slot >= active.len(), |this| this.opacity(0.5))
                    })),
            );
        }
        lanes
    }

    fn run_row(index: usize, run: &Run, cx: &Context<Self>) -> AnyElement {
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
                            .truncate()
                            .child(run.title.clone()),
                    )
                    .child(
                        Button::new(("cancel-run", index))
                            .ghost()
                            .xsmall()
                            .label(if queued { "Remove" } else { "Cancel" })
                            .on_click(cx.listener(move |view, _, window, cx| {
                                if queued {
                                    view.cancel_job(id.clone(), cx);
                                    cx.notify();
                                    return;
                                }
                                Self::request_action(
                                    PendingAction {
                                        title: format!("Cancel {label}?"),
                                        description: "The job stops at a safe point. Finished files and saved state stay.",
                                        confirm: "Cancel job".into(),
                                        destructive: true,
                                        command: Command::Cancel(id.clone()),
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .child(style::meta(format!("{} · {}", run.kind.label(), run.status), cx).truncate())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(!show_progress, Styled::invisible)
                    .child(
                        div().flex_1().child(
                            Progress::new(("run-progress", index))
                                .value(run.progress.percentage())
                                .loading(show_progress && run.progress.total.is_none())
                                .accessibility_label("Job progress"),
                        ),
                    )
                    .child(style::mono(run.progress_text(), cx)),
            )
            .into_any_element()
    }

    fn activity_summary(&self, running: usize) -> String {
        let queued = self.activity.runs().len().saturating_sub(running);
        let mut parts = vec![format!("{running} running")];
        if queued > 0 {
            parts.push(format!("{queued} queued"));
        }
        if !self.activity.waiting().is_empty() {
            parts.push(format!("{} need you", self.activity.waiting().len()));
        }
        parts.join(" · ")
    }

    fn job_panel(&self, width: Pixels, cx: &Context<Self>) -> AnyElement {
        let running = self
            .activity
            .runs()
            .iter()
            .filter(|run| !run.queued)
            .count();
        let summary = self.activity_summary(running);
        let header = div()
            .v_flex()
            .gap_4()
            .p_4()
            .flex_none()
            .border_b_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                div()
                    .id("activity-status")
                    .v_flex()
                    .gap_1()
                    .child(style::section_title("Activity"))
                    .child(style::meta(summary, cx).truncate()),
            )
            .child(self.lanes(cx));
        let mut panel = div().v_flex().gap_4().p_4();
        if let Some(decision) = self.activity.decision() {
            panel = panel.child(self.decision_review(decision, cx));
        }
        if !self.activity.waiting().is_empty() {
            panel = panel.child(self.waiting_inbox(cx));
        }
        panel = if self.activity.runs().is_empty() {
            panel.child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(style::overline("JOBS", cx))
                    .child(style::meta("No active jobs", cx)),
            )
        } else {
            panel.child(self.jobs_list(running, cx))
        };
        for (index, section) in self.activity.sections().iter().enumerate() {
            panel = panel.child(activity_section_box(index, section, cx));
        }
        div()
            .v_flex()
            .w(width)
            .h_full()
            .flex_none()
            .border_l_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar)
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(panel.child(self.recent_events(cx))),
            )
            .into_any_element()
    }

    fn decision_review(&self, decision: &Value, cx: &Context<Self>) -> Stateful<Div> {
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
        if let Some(note) =
            choices::agent_note(decision.pointer("/payload/agent").unwrap_or(&Value::Null))
        {
            review = review.child(div().text_xs().text_color(cx.theme().warning).child(note));
        }
        review = review.child(decision_buttons(decision, cx));
        let details = choices::details(decision);
        if !details.is_empty() {
            review = review.child(decision_details(&details, cx));
        }
        if kind == Some(DecisionKind::ChapterEdit) {
            review =
                review
                    .child(div().text_sm().child(
                        "Index, start seconds, end seconds, title. Leave the last end blank.",
                    ))
                    .child(self.chapter_rows_view().overflow_y_scrollbar())
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
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.activity.defer();
                            view.prepare_decision(window, cx);
                            cx.notify();
                        })),
                ),
            );
        }
        review
    }

    fn chapter_rows_view(&self) -> Div {
        let mut rows = div().v_flex().gap_2().max_h(px(300.));
        for (index, chapter) in self.chapter_rows.iter().enumerate() {
            let heading = format!("Chapter {}", index.saturating_add(1));
            let field = |state: &Entity<InputState>, name: &str| {
                Input::new(state).aria_label(format!("{heading} {name}"))
            };
            rows = rows.child(
                div()
                    .id(("chapter", index))
                    .v_flex()
                    .gap_1()
                    .child(heading.clone())
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(field(&chapter.index, "index"))
                            .child(field(&chapter.start, "start seconds"))
                            .child(field(&chapter.end, "end seconds")),
                    )
                    .child(field(&chapter.title, "title")),
            );
        }
        rows
    }

    fn waiting_inbox(&self, cx: &Context<Self>) -> Div {
        let open = self
            .activity
            .decision()
            .map_or(Value::Null, |decision| decision["queue_job"].clone());
        let mut inbox = div().v_flex().gap_1p5().child(style::overline(
            format!("NEEDS YOU ({})", self.activity.waiting().len()),
            cx,
        ));
        for (index, waiting) in self.activity.waiting().iter().enumerate() {
            let waiting = &waiting.data;
            let kind = choices::kind(waiting);
            let highlighted = waiting["queue_job"] == open;
            let title = short_text(waiting["title"].as_str().unwrap_or("Item"), 60);
            let id = waiting["queue_job"].as_i64();
            inbox = inbox.child(
                Button::new(("waiting", index))
                    .outline()
                    .selected(highlighted)
                    .accessibility_label(format!("{title}, {}", choices::title(kind)))
                    .w_full()
                    .h_auto()
                    .p_2()
                    .when(highlighted, |this| this.border_color(cx.theme().warning))
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .whitespace_normal()
                            .child(div().text_sm().font_semibold().child(title))
                            .child(style::meta(choices::title(kind), cx)),
                    )
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if let Some(id) = id {
                            view.open_waiting(id, window, cx);
                        }
                    })),
            );
        }
        inbox
    }

    fn jobs_list(&self, running: usize, cx: &Context<Self>) -> Div {
        let mut list = div().v_flex().gap_1p5().child(style::overline("JOBS", cx));
        let limit = running.saturating_add(5);
        let shown = self
            .activity
            .runs()
            .iter()
            .enumerate()
            .filter(|(index, run)| !run.queued || *index < limit);
        for (index, run) in shown {
            list = list.child(Self::run_row(index, run, cx));
        }
        let hidden = self.activity.runs().len().saturating_sub(limit);
        if hidden > 0 {
            list = list.child(style::meta(format!("{hidden} more in the queue"), cx));
        }
        list
    }

    fn recent_events(&self, cx: &App) -> Div {
        let mut log = div().v_flex().gap_1p5();
        if self.activity.logs().is_empty() {
            log = log.child(style::meta("Job updates will appear here.", cx));
        }
        for (index, line) in self.activity.logs().iter().rev().take(50).enumerate() {
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
        div()
            .v_flex()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(style::overline("RECENT EVENTS", cx))
            .child(log)
    }
}

fn decision_buttons(decision: &Value, cx: &Context<Muzik>) -> Div {
    let suggested = choices::suggestion(decision);
    let mut buttons = div().v_flex().gap_1p5();
    for (index, option) in choices::choices(decision).into_iter().enumerate() {
        let value = option.value.clone();
        let reply = cx.listener(move |view, _, window, cx| view.reply(value.clone(), window, cx));
        let highlight = suggested == Some(index);
        if option.score.is_none() && option.meta.is_empty() {
            let button = Button::new(("decision", index))
                .label(option.label)
                .w_full()
                .on_click(reply);
            buttons = buttons.child(if highlight { button.primary() } else { button });
        } else {
            buttons = buttons.child(decision_row(index, &option, highlight, cx).on_click(reply));
        }
    }
    buttons
}

fn decision_details(details: &[String], cx: &App) -> Div {
    let mut list = div()
        .v_flex()
        .gap_1()
        .max_h(px(160.))
        .overflow_y_scrollbar();
    for detail in details.iter().take(6) {
        list = list.child(style::meta(short_detail(detail), cx));
    }
    if details.len() > 6 {
        list = list.child(style::meta(
            format!("{} more", details.len().saturating_sub(6)),
            cx,
        ));
    }
    div()
        .v_flex()
        .gap_1()
        .pt_2()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(style::overline("DETAILS", cx))
        .child(list)
}

fn activity_section_box(index: usize, section: &ActivitySection, cx: &App) -> GroupBox {
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
                .id((
                    "activity-row",
                    index.saturating_mul(10).saturating_add(row_index),
                ))
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
                .child(format!(
                    "{} more",
                    section.count.saturating_sub(section.rows.len())
                )),
        );
    }
    GroupBox::new()
        .id(("activity-section", index))
        .outline()
        .child(summary)
}

impl Render for Muzik {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.page {
            Page::Workflow => self.workflow(cx),
            Page::Settings => div()
                .flex_1()
                .min_w_0()
                .children(self.config_view.clone())
                .into_any_element(),
            Page::Watchlist => self.watchlist(cx),
            Page::Library => self.library(cx),
            Page::Spotify => self.spotify(cx),
        };
        let body = div()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(body)
            .child(self.job_panel(
                px((window.bounds().size.width.as_f32() - 600.).clamp(200., 420.)),
                cx,
            ));
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
            .child(self.status_bar(cx))
    }
}

fn describe(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

fn merge_thumbnail_paths(watchlist: &mut WatchlistView, visible: &HashSet<String>, data: &Value) {
    let Some(results) = data["thumbnails"].as_array() else {
        return;
    };
    let playlists = &mut watchlist.playlists;
    for result in results {
        let (Some(video_id), Some(path)) = (result["video_id"].as_str(), result["path"].as_str())
        else {
            continue;
        };
        if !visible.contains(video_id) {
            continue;
        }
        for playlist in playlists.iter_mut() {
            for item in &mut playlist.items {
                if item.item.video_id.as_deref() == Some(video_id) {
                    item.thumbnail_path = Some(path.to_owned());
                }
            }
        }
    }
}

fn decision_row(index: usize, option: &DecisionChoice, highlight: bool, cx: &App) -> Button {
    let theme = cx.theme();
    let score_color = match option.score {
        Some(score) if score >= 85 => theme.success,
        Some(score) if score >= 60 => theme.warning,
        _ => theme.muted_foreground,
    };
    let name = [
        Some(option.label.clone()),
        highlight.then(|| "suggested".to_string()),
        (!option.meta.is_empty()).then(|| option.meta.clone()),
        option.score.map(|score| format!("{score}%")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    let label = div().text_sm().font_semibold().child(option.label.clone());
    let mut row = Button::new(("decision", index))
        .outline()
        .accessibility_label(name)
        .w_full()
        .h_auto()
        .gap_3()
        .px_3()
        .py_2()
        .when(highlight, |row| row.border_color(theme.primary))
        .child(
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .whitespace_normal()
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

fn main() -> std::process::ExitCode {
    if cfg!(target_os = "macos") {
        let path = tool_path(
            std::env::var_os("PATH"),
            std::env::var_os("HOME").map(PathBuf::from),
        );
        if let Ok(path) = std::env::join_paths(path) {
            unsafe { std::env::set_var("PATH", path) };
        }
    }
    match std::env::args().nth(1).as_deref() {
        Some("--version") => {
            println!("muzik-gpui {}", env!("CARGO_PKG_VERSION"));
            return std::process::ExitCode::SUCCESS;
        }
        Some("--check-backend") => {
            if let Err(error) = check_backend() {
                eprintln!("{error}");
                return std::process::ExitCode::FAILURE;
            }
            return std::process::ExitCode::SUCCESS;
        }
        _ => {}
    }
    let logs = muzik_core::paths::Paths::user().logs();
    if std::fs::create_dir_all(&logs).is_ok()
        && let Ok(appender) = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("muzik")
            .filename_suffix("log")
            .max_log_files(7)
            .build(logs)
    {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("MUZIK_LOG")
                    .unwrap_or_else(|_| "info".into()),
            )
            .with_writer(appender)
            .with_ansi(false)
            .init();
    }
    if keyring::Entry::store_status().is_ok()
        && let Err(error) = muzik_runner::setup::move_soulseek_password(
            &muzik_core::paths::Paths::user().config_file(),
        )
    {
        tracing::warn!(%error, "the Soulseek password did not move to the keychain");
    }
    let app = gpui_kit::application().with_assets(style::AppAssets);
    app.run(|cx| {
        gpui_kit::init(cx);
        style::apply_theme(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        bind_keys(cx);
        cx.set_menus([Menu::new("Muzik").items([MenuItem::action("Quit Muzik", Quit)])]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if let Err(error) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Muzik::new(window, cx))
        }) {
            tracing::error!(%error, "the main window did not open");
            eprintln!("open main window: {error}");
            cx.quit();
        }
    });
    std::process::ExitCode::SUCCESS
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("enter", OpenSelected, Some("DataTable")),
        KeyBinding::new("tab", FocusNext, Some("DataTable")),
        KeyBinding::new("shift-tab", FocusPrev, Some("DataTable")),
    ]);
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

fn check_backend() -> anyhow::Result<()> {
    Backend::start()?;
    println!("Rust backend ready");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Muzik, merge_thumbnail_paths, tool_path};
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, Focusable as _, TestAppContext, WindowOptions};
    use muzik_core::app_config::GuiDefaults;
    use serde_json::json;
    use std::collections::HashSet;

    #[gpui_kit::test]
    fn the_keyboard_opens_the_selected_album_and_leaves_the_table(cx: &mut TestAppContext) {
        let (handle, main) = cx.update(|cx| {
            gpui_kit::init(cx);
            super::bind_keys(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Muzik::new_with_backend(window, cx, false))
            })
            .unwrap()
        });
        cx.update(|cx| {
            main.update(cx, |view, cx| {
                view.apply_library(
                    super::library::LibraryInfo {
                        albums: vec![std::sync::Arc::new(super::library::AlbumRow {
                            artist: "Phoniks".into(),
                            title: "Down To Earth".into(),
                            ..Default::default()
                        })],
                        ..Default::default()
                    },
                    cx,
                );
            });
        });
        let table = cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("pages").click(2usize, cx);
            window.render_frame(cx);
            let table = main.read(cx).library_table.clone();
            table.update(cx, |state, cx| state.set_selected_row(0, cx));
            let focus = table.focus_handle(cx);
            focus.focus(window, cx);
            window.render_frame(cx);
            focus
        });
        let table = table.unwrap();
        cx.simulate_keystrokes(handle, "enter");
        cx.update_window(handle, |_, window, cx| {
            assert!(window.has_active_sheet(cx));
            window.close_sheet(cx);
            table.focus(window, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.simulate_keystrokes(handle, "tab");
        cx.update_window(handle, |_, window, cx| {
            assert!(!table.contains_focused(window, cx));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn settings_tab_saves_config_without_repeating_workflow_fields(cx: &mut TestAppContext) {
        let (handle, main) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Muzik::new_with_backend(window, cx, false))
            })
            .unwrap()
        });
        cx.update(|cx| {
            main.update(cx, |view, cx| {
                view.defaults = Some(GuiDefaults {
                    output: "/tmp/downloads".into(),
                    splits: "/tmp/splits".into(),
                    min_bitrate: 256,
                    ..GuiDefaults::default()
                });
                view.status = "Changed".into();
                cx.notify();
            });
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("pages").click(3usize, cx);
            window.render_frame(cx);
            window.within("settings-sidebar").click("0-2", cx);
            window.render_frame(cx);
            assert!(window.try_find("service-refresh").is_some());
            window.click("save-config", cx);
            window.within("pages").click(0usize, cx);
            assert!(window.try_find("run").is_some());
            assert!(window.try_find("save-config").is_none());
        })
        .unwrap();
        let params = main.read_with(cx, Muzik::launcher_params);
        assert_eq!(params["output"], "/tmp/downloads");
        assert_eq!(params["splits"], "/tmp/splits");
    }

    #[gpui_kit::test]
    fn chapter_inputs_follow_the_active_choice_and_keep_edits_during_progress(
        cx: &mut TestAppContext,
    ) {
        let (handle, main) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Muzik::new_with_backend(window, cx, false))
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            main.update(cx, |view, cx| {
                view.app_event(muzik_runner::AppEvent::DecisionRequest {
                    job_id: "queue-1".into(), decision_id: "live".into(),
                    kind: muzik_core::DecisionKind::ChapterEdit,
                    payload: json!({"chapters":[{"index":1,"start":0,"title":"Original"}]}),
                }, window, cx);
                view.chapter_rows[0].title.update(cx, |state, cx| state.set_value("Edited", window, cx));
                view.app_event(muzik_runner::AppEvent::JobEvent {
                    job_id: "queue-1".into(), source: muzik_runner::Source::Native,
                    event: muzik_core::JobEvent::message("Still working"),
                }, window, cx);
                assert_eq!(view.chapter_rows[0].title.read(cx).value().as_str(), "Edited");
                view.app_event(muzik_runner::AppEvent::JobsUpdated(json!({"waiting":[{
                    "id":2,"kind":"chapter_edit", "payload":{"chapters":[{"index":1,"start":0,"title":"Saved"}]}
                }]})), window, cx);
                view.open_waiting(2, window, cx);
                assert_eq!(view.chapter_rows[0].title.read(cx).value().as_str(), "Saved");
                view.app_event(muzik_runner::AppEvent::JobsUpdated(json!({"waiting":[]})), window, cx);
                assert_eq!(view.chapter_rows[0].title.read(cx).value().as_str(), "Original");
                view.app_event(muzik_runner::AppEvent::JobCancelled { job_id: "queue-1".into() }, window, cx);
                assert!(view.chapter_rows.is_empty());
            });
            window.render_frame(cx);
            assert!(window.try_find("decision").is_none());
        }).unwrap();
    }

    #[test]
    fn thumbnail_event_updates_only_current_visible_cards() {
        let mut watchlist: super::WatchlistView = serde_json::from_value(json!({"playlists":[{"playlist_id":"PL1","url":"https://www.youtube.com/playlist?list=PL1","items":[
            {"position":1,"title":"Visible","summary":"Pending","actions":{},"video_id":"visible", "thumbnail_path":null},
            {"position":2,"title":"Other","summary":"Pending","actions":{},"video_id":"other", "thumbnail_path":null}
        ]}]})).unwrap();
        let visible = HashSet::from(["visible".to_string()]);
        let event = json!({"thumbnails":[
            {"video_id":"visible", "path":"/cache/visible.jpg"},
            {"video_id":"other", "path":"/cache/other.jpg"},
            {"video_id":"visible", "path":null}
        ]});
        merge_thumbnail_paths(&mut watchlist, &visible, &event);
        assert_eq!(
            watchlist.playlists[0].items[0].thumbnail_path.as_deref(),
            Some("/cache/visible.jpg")
        );
        assert!(watchlist.playlists[0].items[1].thumbnail_path.is_none());
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
