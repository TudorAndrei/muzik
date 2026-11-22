use std::{collections::HashSet, path::Path, str::FromStr, sync::Arc};

use muzik_core::thumbnails;
use muzik_runner::{
    AppEvent,
    service::{ItemRequest, Service},
};
use muzik_store::watchlist::{
    ItemAction, ItemId, ItemView, PlaylistView, SourceKind, Stage, StageStatus, Summary,
    WatchlistView,
};
use serde::{Deserialize, Serialize};
use topcoat::{
    Result,
    context::Cx,
    router::{
        StatusCode,
        content::Form,
        error::{RouterErrorExt, SeeOther, see_other},
        href, page, path_param, query_params, route,
    },
    runtime::{connected, shard},
    view::{View, component, emit, live, view},
};

use crate::{
    activity::{settle, truncate},
    auth,
    state::{backend, events, hub},
};

const ROW_LIMIT: usize = 300;
const TITLE_LIMIT: usize = 120;
const ERROR_LIMIT: usize = 160;
const NOTICE_LIMIT: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Place {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub filter: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub item: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
}

const fn at(number: usize) -> Option<usize> {
    if number > 1 { Some(number) } else { None }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceCard {
    id: String,
    title: String,
    kind: &'static str,
    count: usize,
    failing: bool,
    selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tab {
    label: &'static str,
    filter: String,
    active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pip {
    tip: String,
    class: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Act {
    name: &'static str,
    label: String,
    enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    position: u64,
    key: String,
    title: String,
    summary: String,
    tone: &'static str,
    error: String,
    pips: Vec<Pip>,
    action: Option<Act>,
    selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CardAction {
    name: &'static str,
    label: String,
    enabled: bool,
    reason: String,
    confirm: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Card {
    key: String,
    title: String,
    id_label: &'static str,
    id: String,
    open_label: &'static str,
    url: Option<String>,
    thumb: Option<String>,
    error: String,
    stages: Vec<(&'static str, &'static str, &'static str)>,
    note: String,
    actions: Vec<CardAction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Detail {
    id: String,
    title: String,
    name: String,
    facts: String,
    url: Option<String>,
    error: Option<String>,
    playlist_file: bool,
    tabs: Vec<Tab>,
    rows: Vec<Row>,
    page: usize,
    pages: usize,
    total: usize,
    empty: Option<String>,
    retry: usize,
    card: Option<Card>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Board {
    place: Place,
    error: Option<String>,
    loaded: bool,
    sources: Vec<SourceCard>,
    detail: Option<Detail>,
    wanted: Option<String>,
}

const fn kind_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Youtube => "YouTube",
        SourceKind::Spotify => "Spotify",
        SourceKind::Bandcamp => "Bandcamp",
    }
}

const fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Download => "Download",
        Stage::Quality => "Quality",
        Stage::Parse => "Parse",
        Stage::Split => "Split",
        Stage::Organize => "Organize",
    }
}

const fn status_word(status: StageStatus) -> &'static str {
    match status {
        StageStatus::NotStarted => "Not started",
        StageStatus::Running => "Running",
        StageStatus::Waiting => "Waiting for you",
        StageStatus::Complete => "Complete",
        StageStatus::Failed => "Failed",
        StageStatus::Skipped => "Skipped",
        StageStatus::Stale => "Stale",
    }
}

const fn status_class(status: StageStatus) -> &'static str {
    match status {
        StageStatus::NotStarted => "none",
        StageStatus::Running => "run",
        StageStatus::Waiting => "wait",
        StageStatus::Complete => "done",
        StageStatus::Failed => "fail",
        StageStatus::Skipped => "skip",
        StageStatus::Stale => "stale",
    }
}

const fn summary_tone(summary: Summary) -> &'static str {
    match summary {
        Summary::Pending => "tag",
        Summary::Processing => "tag info",
        Summary::Waiting => "tag warn",
        Summary::Failed => "tag bad",
        Summary::Processed => "tag ok",
        Summary::Unavailable => "tag mute",
    }
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

fn web_url(url: &str) -> Option<String> {
    let url = url.trim();
    (url.starts_with("https://") || url.starts_with("http://")).then(|| url.to_owned())
}

fn playlist_title(playlist: &PlaylistView) -> &str {
    playlist.title.as_deref().unwrap_or(&playlist.playlist_id)
}

fn matches_filter(item: &ItemView, filter: Option<Summary>) -> bool {
    filter.map_or_else(
        || item.summary != Summary::Unavailable,
        |wanted| item.summary == wanted,
    )
}

fn retryable(item: &ItemView) -> bool {
    item.summary == Summary::Failed
        && item
            .primary_action
            .as_ref()
            .is_some_and(|primary| primary.action == ItemAction::Retry)
        && item
            .actions
            .get(&ItemAction::Retry)
            .is_some_and(|retry| retry.enabled)
}

fn pips(item: &ItemView) -> Vec<Pip> {
    Stage::ALL
        .iter()
        .map(|stage| {
            let status = item.status(*stage);
            Pip {
                tip: format!("{} · {}", stage_label(*stage), status_word(status)),
                class: status_class(status),
            }
        })
        .collect()
}

fn empty_message(playlist: &PlaylistView, filter: Option<Summary>) -> String {
    if playlist.kind == SourceKind::Spotify && playlist.items.is_empty() {
        "This Spotify source has no tracks. Refresh it to read track names. Set Audio source to Soulseek in Settings to get audio.".to_owned()
    } else if playlist.items.is_empty() && playlist.last_checked_at.is_none() {
        "This playlist has not been checked. Select Refresh to read it.".to_owned()
    } else if playlist.items.is_empty() {
        "This playlist has no videos. Refresh it to check again.".to_owned()
    } else {
        format!(
            "No items have the {} status. Select All to see every item.",
            filter.map_or("All", Into::into)
        )
    }
}

fn card(
    playlist: &PlaylistView,
    key: &str,
    queued: &HashSet<ItemId>,
    cache: &Path,
) -> Option<(Card, Option<String>)> {
    let ids = playlist.item_ids();
    let (id, item) = ids
        .iter()
        .zip(&playlist.items)
        .find(|(id, _)| id.key.as_deref() == Some(key))?;
    let (id_label, open_label) = match item.item.kind {
        SourceKind::Spotify => ("Spotify ID", "Open in Spotify"),
        SourceKind::Bandcamp => ("Bandcamp ID", "Open on Bandcamp"),
        SourceKind::Youtube => ("YouTube ID", "Open on YouTube"),
    };
    let open_job = match item.hold.as_ref() {
        Some(hold) if hold.is_waiting() => Some("Answer or cancel the open choice first."),
        Some(_) => Some("This item has a job in the queue."),
        None => queued
            .contains(id)
            .then_some("This item has a job in the queue."),
    };
    let actions = ItemAction::ALL
        .iter()
        .map(|action| {
            let availability = item.actions.get(action);
            let label = item
                .primary_action
                .as_ref()
                .filter(|primary| primary.action == *action)
                .map_or_else(|| action_label(*action).to_owned(), |p| p.label.clone());
            let reason = match (open_job, availability) {
                (Some(_), _) | (None, None) => String::new(),
                (None, Some(availability)) if availability.enabled => String::new(),
                (None, Some(availability)) => availability.reason.clone().unwrap_or_default(),
            };
            CardAction {
                name: (*action).into(),
                label,
                enabled: open_job.is_none() && availability.is_some_and(|a| a.enabled),
                reason,
                confirm: action.replaces_files(),
            }
        })
        .collect();
    let video_id = item.item.video_id.clone().filter(|id| !id.is_empty());
    let cached = video_id
        .as_deref()
        .filter(|id| thumbnails::cached_path(id, cache).is_some())
        .map(str::to_owned);
    let wanted = video_id
        .clone()
        .filter(|id| cached.is_none() && thumbnails::valid_id(id))
        .filter(|_| item.item.thumbnail_url.is_some());
    Some((
        Card {
            key: id.key.clone().unwrap_or_default(),
            title: truncate(&item.item.title, TITLE_LIMIT),
            id_label,
            id: video_id.unwrap_or_default(),
            open_label,
            url: item.item.video_url.as_deref().and_then(web_url),
            thumb: cached,
            error: truncate(item.item.last_error.as_deref().unwrap_or_default(), 300),
            stages: Stage::ALL
                .iter()
                .map(|stage| {
                    let status = item.status(*stage);
                    (
                        stage_label(*stage),
                        status_word(status),
                        status_class(status),
                    )
                })
                .collect(),
            note: open_job.unwrap_or_default().to_owned(),
            actions,
        },
        wanted,
    ))
}

fn detail(
    playlist: &PlaylistView,
    place: &Place,
    filter: Option<Summary>,
    queued: &HashSet<ItemId>,
    cache: &Path,
) -> (Detail, Option<String>) {
    let ids = playlist.item_ids();
    let shown: Vec<(&ItemId, &ItemView)> = ids
        .iter()
        .zip(&playlist.items)
        .filter(|(_, item)| matches_filter(item, filter))
        .collect();
    let selected = Some(place.item.as_str()).filter(|key| !key.is_empty());
    let pages = shown.len().div_ceil(ROW_LIMIT).max(1);
    let number = place.page.unwrap_or(1).clamp(1, pages);
    let rows: Vec<Row> = shown
        .iter()
        .skip(number.saturating_sub(1).saturating_mul(ROW_LIMIT))
        .take(ROW_LIMIT)
        .map(|(id, item)| {
            let busy = queued.contains(*id);
            let (summary, tone) = if busy {
                ("In the queue".to_owned(), "tag info")
            } else {
                (item.summary.to_string(), summary_tone(item.summary))
            };
            let action = item.primary_action.as_ref().map(|primary| Act {
                name: primary.action.into(),
                label: primary.label.clone(),
                enabled: !busy
                    && item
                        .actions
                        .get(&primary.action)
                        .is_some_and(|availability| availability.enabled),
            });
            Row {
                position: item.item.position,
                key: id.key.clone().unwrap_or_default(),
                title: truncate(&item.item.title, TITLE_LIMIT),
                summary,
                tone,
                error: truncate(
                    item.item.last_error.as_deref().unwrap_or_default(),
                    ERROR_LIMIT,
                ),
                pips: pips(item),
                action,
                selected: selected.is_some() && selected == id.key.as_deref(),
            }
        })
        .collect();
    let mut seen = HashSet::new();
    let retry = ids
        .iter()
        .zip(&playlist.items)
        .filter(|(id, item)| retryable(item) && !queued.contains(*id))
        .filter(|(id, _)| seen.insert((*id).clone()))
        .count();
    let tabs = std::iter::once(None)
        .chain(Summary::ALL.iter().copied().map(Some))
        .map(|tab| Tab {
            label: tab.map_or("All", Into::into),
            filter: tab.map_or(String::new(), |summary| {
                <&'static str>::from(summary).to_owned()
            }),
            active: tab == filter,
        })
        .collect();
    let (card, wanted) = selected
        .and_then(|key| card(playlist, key, queued, cache))
        .unzip();
    (
        Detail {
            id: playlist.playlist_id.clone(),
            title: truncate(playlist_title(playlist), TITLE_LIMIT),
            name: playlist_title(playlist).to_owned(),
            facts: format!(
                "{} · {} items · {}",
                kind_name(playlist.kind),
                playlist.items.len(),
                playlist
                    .last_checked_at
                    .as_deref()
                    .map_or_else(|| "not checked".to_owned(), |at| format!("checked {at}"))
            ),
            url: web_url(&playlist.url),
            error: playlist
                .last_error
                .as_deref()
                .map(|error| truncate(error, 300)),
            playlist_file: playlist.playlist_file,
            tabs,
            page: number,
            pages,
            total: shown.len(),
            empty: shown.is_empty().then(|| empty_message(playlist, filter)),
            rows,
            retry,
            card,
        },
        wanted.flatten(),
    )
}

fn has_item(playlist: &PlaylistView, key: &str) -> bool {
    !key.is_empty()
        && playlist
            .item_ids()
            .iter()
            .any(|id| id.key.as_deref() == Some(key))
}

fn build_board(
    view: Option<&WatchlistView>,
    error: Option<String>,
    queued: &HashSet<ItemId>,
    cache: &Path,
    requested: &Place,
) -> Board {
    let playlists = view.map_or(&[][..], |view| view.playlists.as_slice());
    let chosen = playlists
        .iter()
        .position(|playlist| playlist.playlist_id == requested.source)
        .or_else(|| (!playlists.is_empty()).then_some(0));
    let filter = Summary::from_str(&requested.filter).ok();
    let sources = playlists
        .iter()
        .enumerate()
        .map(|(index, playlist)| SourceCard {
            id: playlist.playlist_id.clone(),
            title: truncate(playlist_title(playlist), TITLE_LIMIT),
            kind: kind_name(playlist.kind),
            count: playlist.items.len(),
            failing: playlist.last_error.is_some(),
            selected: chosen == Some(index),
        })
        .collect();
    let source = chosen.and_then(|index| playlists.get(index));
    let asked = source.filter(|playlist| playlist.playlist_id == requested.source);
    let mut place = Place {
        source: source
            .map(|playlist| playlist.playlist_id.clone())
            .unwrap_or_default(),
        filter: filter
            .map(|summary| <&'static str>::from(summary).to_owned())
            .unwrap_or_default(),
        item: asked
            .filter(|playlist| has_item(playlist, &requested.item))
            .map(|_| requested.item.clone())
            .unwrap_or_default(),
        page: asked.and(requested.page),
    };
    let (detail, wanted) = source
        .map(|playlist| detail(playlist, &place, filter, queued, cache))
        .unzip();
    if let Some(detail) = &detail {
        place.page = at(detail.page);
    }
    Board {
        place,
        error: error.map(|message| truncate(&message, 300)),
        loaded: view.is_some(),
        sources,
        detail,
        wanted: wanted.flatten(),
    }
}

/// # Errors
/// Returns an error when the blocking task fails to finish.
pub async fn reload(cx: &Cx) -> Result<()> {
    let shared = hub(cx);
    let _ = shared.take_watchlist_stale();
    match backend(cx).run(Service::load_watchlist).await? {
        Ok((view, check)) => {
            shared.store_watchlist(view);
            drop(tokio::task::spawn_blocking(move || check.run()));
        }
        Err(error) => shared.fail_watchlist(error.to_string()),
    }
    let _ = events(cx).0.send(AppEvent::WatchlistSaved);
    Ok(())
}

async fn current(cx: &Cx) -> Result<Option<Arc<WatchlistView>>> {
    if hub(cx).take_watchlist_stale() {
        reload(cx).await?;
    }
    Ok(hub(cx).watchlist())
}

async fn load(cx: &Cx, place: &Place) -> Result<Board> {
    let view = current(cx).await?;
    let (jobs, cache) = backend(cx)
        .run(|service| (service.jobs(), service.paths().cache.clone()))
        .await?;
    let queued: HashSet<ItemId> = jobs
        .get("open")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|job| job["item"].as_str()?.parse().ok())
        .collect();
    let board = build_board(
        view.as_deref(),
        hub(cx).watchlist_error(),
        &queued,
        &cache,
        place,
    );
    if let Some(id) = board.wanted.clone().filter(|id| hub(cx).try_thumbnail(id)) {
        let sender = events(cx).0.clone();
        let service = Arc::clone(&backend(cx).0);
        drop(tokio::task::spawn_blocking(move || {
            if service.cache_thumbnails(vec![id]).is_some() {
                let _ = sender.send(AppEvent::WatchlistSaved);
            }
        }));
    }
    Ok(board)
}

const fn relevant(event: &AppEvent) -> bool {
    matches!(
        event,
        AppEvent::WatchlistUpdated(_)
            | AppEvent::WatchlistSaved
            | AppEvent::WatchlistError(_)
            | AppEvent::JobsUpdated(_)
            | AppEvent::JobCompleted { .. }
            | AppEvent::JobFailed { .. }
            | AppEvent::JobCancelled { .. }
    )
}

#[query_params(error = bad_request)]
struct WatchQuery {
    source: Option<String>,
    filter: Option<String>,
    item: Option<String>,
    page: Option<usize>,
    notice: Option<String>,
    error: Option<String>,
}

#[query_params(error = bad_request)]
struct PlaceQuery {
    source: Option<String>,
    filter: Option<String>,
    item: Option<String>,
    page: Option<usize>,
}

#[derive(Serialize)]
struct Outcome {
    #[serde(flatten)]
    place: Place,
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn place_of(cx: &Cx) -> Result<Place> {
    let query = query_params::<PlaceQuery>(cx)?;
    Ok(Place {
        source: query.source.clone().unwrap_or_default(),
        filter: query.filter.clone().unwrap_or_default(),
        item: query.item.clone().unwrap_or_default(),
        page: query.page,
    })
}

fn back(cx: &Cx, place: Place, outcome: Result<String, String>) -> SeeOther {
    let (notice, error) = match outcome {
        Ok(text) => (Some(truncate(&text, NOTICE_LIMIT)), None),
        Err(text) => (None, Some(truncate(&text, NOTICE_LIMIT))),
    };
    see_other(
        href!(page)
            .query(&Outcome {
                place,
                notice,
                error,
            })
            .resolve(cx),
    )
}

#[page("/watchlist")]
pub async fn page(cx: &Cx) -> Result<impl View> {
    auth::require(cx).await?;
    let query = query_params::<WatchQuery>(cx)?;
    let message = query.error.clone().or_else(|| query.notice.clone());
    let failed = query.error.is_some();
    let source = query.source.clone().unwrap_or_default();
    let filter = query.filter.clone().unwrap_or_default();
    let item = query.item.clone().unwrap_or_default();
    let number = query.page.unwrap_or_default();
    let place = Place {
        source: source.clone(),
        filter: filter.clone(),
        item: item.clone(),
        page: query.page,
    };
    let to_add = href!(add).query(&place).resolve(cx);
    let refresh_all = href!(refresh).query(&place).resolve(cx);
    Ok(view! {
        <h1>"Watchlist"</h1>
        <p class=(if failed { "notice error" } else { "notice" })>(message)</p>
        <form class="launcher" method="post" action=(to_add)>
            <input
                id="watch-url"
                type="text"
                name="url"
                placeholder="YouTube or Spotify playlist or album link, or liked"
                autocomplete="off"
                required="true"
            >
            <button class="primary" type="submit">"Add playlist"</button>
            <button type="submit" formaction=(refresh_all) formnovalidate="true">"Refresh all"</button>
        </form>
        live_board(source: source, filter: filter, item: item, number: number)
    })
}

#[shard]
pub async fn live_board(
    cx: &Cx,
    source: String,
    filter: String,
    item: String,
    number: usize,
) -> Result<impl View> {
    auth::require(cx).await?;
    let place = Place {
        source,
        filter,
        item,
        page: at(number),
    };
    Ok(live! {
        let mut changes = events(cx).0.subscribe();
        let mut shown: Option<Board> = None;
        loop {
            if !auth::authenticated(cx).await? {
                break emit! { <p class="empty">"The session has ended. Reload the page to sign in again."</p> };
            }
            let current = load(cx, &place).await?;
            let token = if shown.as_ref() == Some(&current) {
                None
            } else {
                shown = Some(current.clone());
                Some(emit! { watch_board(data: current) }?)
            };
            if !connected(cx) {
                match token {
                    Some(token) => break Ok(token),
                    None => continue,
                }
            }
            if !settle(&mut changes, relevant).await {
                break if let Some(token) = token {
                    Ok(token)
                } else {
                    emit! { <p class="empty">"The server is stopping."</p> }
                };
            }
        }
    })
}

fn go(cx: &Cx, source: &str, filter: &str, item: &str, number: usize) -> String {
    href!(page)
        .query(&Place {
            source: source.to_owned(),
            filter: filter.to_owned(),
            item: item.to_owned(),
            page: at(number),
        })
        .resolve(cx)
}

type Linked = (
    Detail,
    Vec<(Tab, String)>,
    Vec<(Row, String)>,
    Option<String>,
    Option<String>,
);

fn linked(cx: &Cx, detail: Detail, place: &Place) -> Linked {
    let tabs = detail
        .tabs
        .iter()
        .cloned()
        .map(|tab| {
            let link = go(cx, &detail.id, &tab.filter, "", 0);
            (tab, link)
        })
        .collect();
    let rows = detail
        .rows
        .iter()
        .cloned()
        .map(|row| {
            let link = go(cx, &detail.id, &place.filter, &row.key, detail.page);
            (row, link)
        })
        .collect();
    let around = |number: usize| go(cx, &detail.id, &place.filter, &place.item, number);
    let previous = (detail.page > 1).then(|| around(detail.page.saturating_sub(1)));
    let following = (detail.page < detail.pages).then(|| around(detail.page.saturating_add(1)));
    (detail, tabs, rows, previous, following)
}

#[component]
async fn watch_board(cx: &Cx, data: Board) -> Result<impl View> {
    let place = data.place.clone();
    let sources: Vec<(SourceCard, String)> = data
        .sources
        .into_iter()
        .map(|source| {
            let link = go(cx, &source.id, "", "", 0);
            (source, link)
        })
        .collect();
    let loaded = data.loaded;
    let error = data.error.clone().unwrap_or_default();
    let run = href!(run_item).query(&place).resolve(cx);
    let main = data.detail.map(|detail| linked(cx, detail, &place));
    Ok(view! {
        <p class="notice error">(error)</p>
        <div class="watch">
            source_list(sources: sources, loaded: loaded)
            <section class="card">
                if let Some((detail, tabs, rows, previous, following)) = main {
                    <div class="src-head">
                        <div class="title">(detail.title.clone())</div>
                        <div class="meta">(detail.facts.clone())</div>
                    </div>
                    tools_bar(
                        id: detail.id.clone(),
                        title: detail.name.clone(),
                        url: detail.url.clone().unwrap_or_default(),
                        retry: detail.retry,
                        file_on: detail.playlist_file,
                        place: place.clone(),
                    )
                    <p class="notice error">(detail.error.clone().unwrap_or_default())</p>
                    <div class="tabs">
                        for (tab, link) in tabs {
                            <a class=(if tab.active { "tab current" } else { "tab" }) href=(link)>(tab.label)</a>
                        }
                    </div>
                    item_list(
                        id: detail.id.clone(),
                        empty: detail.empty.clone().unwrap_or_default(),
                        number: detail.page,
                        pages: detail.pages,
                        total: detail.total,
                        previous: previous,
                        following: following,
                        rows: rows,
                        place: place.clone(),
                    )
                    <div class="scroll card-detail">
                        if let Some(card) = detail.card.clone() {
                            item_card(card: card, source: detail.id.clone(), run: run.clone())
                        } else {
                            <div class="empty">"Select a title to see its details and commands."</div>
                        }
                    </div>
                } else {
                    <div class="empty">"Select a source."</div>
                }
            </section>
        </div>
    })
}

#[component]
async fn source_list(
    cx: &Cx,
    sources: Vec<(SourceCard, String)>,
    loaded: bool,
) -> Result<impl View> {
    let none = sources.is_empty();
    Ok(view! {
        <aside class="card">
            <h2>"Sources"</h2>
            <div class="scroll rail">
                if none {
                    if loaded {
                        <div class="empty">"No playlists yet. Paste a YouTube or Spotify playlist link above, then add it."</div>
                    } else {
                        <div class="empty">"The watchlist is not available."</div>
                    }
                }
                for (source, link) in sources {
                    <a
                        class=(if source.selected { "source current" } else { "source" })
                        href=(link)
                        id=(format!("source-{}", source.id))
                    >
                        <span class="title">(source.title)</span>
                        <span class="meta">
                            (source.kind) " · " (source.count) " items"
                            if source.failing {
                                " · error"
                            }
                        </span>
                    </a>
                }
            </div>
        </aside>
    })
}

#[component]
async fn tools_bar(
    cx: &Cx,
    id: String,
    title: String,
    url: String,
    retry: usize,
    file_on: bool,
    place: Place,
) -> Result<impl View> {
    let to_rename = href!(rename).query(&place).resolve(cx);
    let to_remove = href!(remove).query(&place).resolve(cx);
    let to_file = href!(playlist_file).query(&place).resolve(cx);
    let refresh_one = href!(refresh).query(&place).resolve(cx);
    let to_retry = href!(retry_failed).query(&place).resolve(cx);
    let has_url = !url.is_empty();
    Ok(view! {
        <div class="tools">
            <form class="inline" method="post" action=(refresh_one)>
                <input type="hidden" name="playlist_id" value=(id.clone())>
                <input type="hidden" name="title" value=(title.clone())>
                <button class="small" type="submit">"Refresh"</button>
            </form>
            if has_url {
                <a class="button small" href=(url) target="_blank" rel="noopener noreferrer">"Open"</a>
            }
            if retry > 0 {
                <form class="inline" method="post" action=(to_retry)>
                    <input type="hidden" name="playlist_id" value=(id.clone())>
                    <button class="small" type="submit">"Retry " (retry) " failed"</button>
                </form>
            }
            <form class="inline" method="post" action=(to_file)>
                <input type="hidden" name="playlist_id" value=(id.clone())>
                <input type="hidden" name="on" value=(if file_on { "false" } else { "true" })>
                <button
                    class="small"
                    type="submit"
                    title="Write a playlist file for this source. Turn on playlist files in Settings."
                >(if file_on { "Playlist file: on" } else { "Playlist file: off" })</button>
            </form>
            <form class="inline renamer" method="post" action=(to_rename)>
                <input type="hidden" name="playlist_id" value=(id.clone())>
                <input
                    type="text"
                    name="title"
                    value=(title)
                    aria-label="Playlist name"
                    id="rename-title"
                    required="true"
                >
                <button class="small" type="submit">"Rename"</button>
            </form>
            <form
                class="inline"
                method="post"
                action=(to_remove)
                onsubmit="return confirm('Remove this playlist from the watchlist?')"
            >
                <input type="hidden" name="playlist_id" value=(id)>
                <button class="small danger" type="submit">"Remove"</button>
            </form>
        </div>
    })
}

#[component]
async fn item_list(
    cx: &Cx,
    id: String,
    empty: String,
    number: usize,
    pages: usize,
    total: usize,
    previous: Option<String>,
    following: Option<String>,
    rows: Vec<(Row, String)>,
    place: Place,
) -> Result<impl View> {
    let run = href!(run_item).query(&place).resolve(cx);
    let has_empty = !empty.is_empty();
    let several = pages > 1;
    Ok(view! {
        <div class="scroll items">
            if has_empty {
                <div class="empty">(empty)</div>
            }
            for (row, link) in rows {
                <div class=(if row.selected { "irow current" } else { "irow" }) id=(format!("item-{}", row.position))>
                    <span class="num">(row.position)</span>
                    <div class="main">
                        <a class="title" href=(link)>(row.title.clone())</a>
                        <div class="meta">(row.error.clone())</div>
                    </div>
                    <span class="pips">
                        for pip in row.pips.clone() {
                            <span class=(format!("pip {}", pip.class)) title=(pip.tip)></span>
                        }
                    </span>
                    <span class=(row.tone)>(row.summary.clone())</span>
                    <span class="act">
                        if let Some(action) = row.action.clone() {
                            <form class="inline" method="post" action=(run.clone())>
                                <input type="hidden" name="playlist_id" value=(id.clone())>
                                <input type="hidden" name="key" value=(row.key.clone())>
                                <input type="hidden" name="action" value=(action.name)>
                                <button class="small" type="submit" disabled=(!action.enabled)>(action.label)</button>
                            </form>
                        }
                    </span>
                </div>
            }
            if several {
                <div class="pager">
                    <span class="meta">(total) " items · page " (number) " of " (pages)</span>
                    if let Some(link) = previous {
                        <a class="button small" href=(link)>"Previous"</a>
                    }
                    if let Some(link) = following {
                        <a class="button small" href=(link)>"Next"</a>
                    }
                </div>
            }
        </div>
    })
}

#[component]
async fn item_card(cx: &Cx, card: Card, source: String, run: String) -> Result<impl View> {
    let has_thumb = card.thumb.is_some();
    let thumb = card
        .thumb
        .clone()
        .map(|id| href!(thumbnail, thumbnail_id: id.as_str()).resolve(cx))
        .unwrap_or_default();
    Ok(view! {
        <div class="ic">
            <div class="ic-thumb">
                if has_thumb {
                    <img src=(thumb) alt="" loading="lazy">
                }
            </div>
            <div class="ic-body">
                <div class="title">(card.title.clone())</div>
                <div class="meta">(card.id_label) ": " (card.id.clone())</div>
                if let Some(url) = card.url.clone() {
                    <a href=(url) target="_blank" rel="noopener noreferrer">(card.open_label)</a>
                }
                <div class="meta error">(card.error.clone())</div>
                <div class="stages">
                    for (label, word, class) in card.stages.clone() {
                        <span class="stage">
                            <span class=(format!("pip {class}")) title=(word)></span>
                            (label) " " <span class="meta">(word)</span>
                        </span>
                    }
                </div>
                <div class="meta">(card.note.clone())</div>
            </div>
            <div class="ic-actions">
                for action in card.actions.clone() {
                    <form
                        class="inline"
                        method="post"
                        action=(run.clone())
                        if action.confirm {
                            onsubmit="return confirm('This replaces the files from this stage. Later stages can become stale.')"
                        }
                    >
                        <input type="hidden" name="playlist_id" value=(source.clone())>
                        <input type="hidden" name="key" value=(card.key.clone())>
                        <input type="hidden" name="action" value=(action.name)>
                        <button class="small" type="submit" disabled=(!action.enabled) title=(action.reason.clone())>(action.label)</button>
                    </form>
                }
            </div>
        </div>
    })
}

#[derive(Deserialize)]
struct AddForm {
    url: String,
}

#[route(POST "/watchlist/add")]
pub async fn add(cx: &Cx, Form(form): Form<AddForm>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let mut place = place_of(cx)?;
    let url = form.url.trim().to_owned();
    if url.is_empty() {
        return Ok(back(cx, place, Err("Enter a playlist link.".into())));
    }
    let outcome = backend(cx)
        .run(move |service| service.add_source(&url))
        .await?;
    match outcome {
        Ok(added) => {
            reload(cx).await?;
            place.source = added.playlist_id;
            place.item.clear();
            Ok(back(cx, place, Ok("Playlist added".into())))
        }
        Err(error) => Ok(back(cx, place, Err(error.to_string()))),
    }
}

#[derive(Deserialize)]
struct SourceForm {
    playlist_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    on: String,
}

#[route(POST "/watchlist/rename")]
pub async fn rename(cx: &Cx, Form(form): Form<SourceForm>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let place = place_of(cx)?;
    let outcome = backend(cx)
        .run(move |service| service.rename_source(&form.playlist_id, &form.title))
        .await?;
    Ok(match outcome {
        Ok(true) => {
            reload(cx).await?;
            back(cx, place, Ok("Playlist renamed".into()))
        }
        Ok(false) => back(
            cx,
            place,
            Err("The playlist is not in the watchlist.".into()),
        ),
        Err(error) => back(cx, place, Err(error.to_string())),
    })
}

#[route(POST "/watchlist/remove")]
pub async fn remove(cx: &Cx, Form(form): Form<SourceForm>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let mut place = place_of(cx)?;
    let removed = form.playlist_id.clone();
    let outcome = backend(cx)
        .run(move |service| service.remove_source(&form.playlist_id))
        .await?;
    Ok(match outcome {
        Ok(true) => {
            reload(cx).await?;
            if place.source == removed {
                place.source.clear();
                place.item.clear();
            }
            back(cx, place, Ok("Playlist removed".into()))
        }
        Ok(false) => back(
            cx,
            place,
            Err("The playlist is not in the watchlist.".into()),
        ),
        Err(error) => back(cx, place, Err(error.to_string())),
    })
}

#[route(POST "/watchlist/playlist-file")]
pub async fn playlist_file(cx: &Cx, Form(form): Form<SourceForm>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let place = place_of(cx)?;
    let on = form.on == "true";
    let outcome = backend(cx)
        .run(move |service| service.set_source_playlist(&form.playlist_id, on))
        .await?;
    Ok(match outcome {
        Ok(true) => {
            reload(cx).await?;
            back(
                cx,
                place,
                Ok(if on {
                    "Playlist file turned on".into()
                } else {
                    "Playlist file turned off".into()
                }),
            )
        }
        Ok(false) => back(
            cx,
            place,
            Err("The playlist is not in the watchlist.".into()),
        ),
        Err(error) => back(cx, place, Err(error.to_string())),
    })
}

#[derive(Deserialize)]
struct RefreshForm {
    #[serde(default)]
    playlist_id: String,
    #[serde(default)]
    title: String,
}

#[route(POST "/watchlist/refresh")]
pub async fn refresh(cx: &Cx, Form(form): Form<RefreshForm>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let place = place_of(cx)?;
    let outcome = backend(cx)
        .run(move |service| {
            let id = form.playlist_id.trim().to_owned();
            if id.is_empty() {
                service.refresh(None)
            } else {
                let title = if form.title.trim().is_empty() {
                    id.clone()
                } else {
                    form.title.trim().to_owned()
                };
                service.refresh(Some((&id, &title)))
            }
        })
        .await?;
    Ok(back(
        cx,
        place,
        outcome
            .map(|id| format!("Queued {id}"))
            .map_err(|error| error.to_string()),
    ))
}

fn find_item(view: &WatchlistView, playlist_id: &str, key: &str) -> Option<(ItemId, String)> {
    let playlist = view
        .playlists
        .iter()
        .find(|playlist| playlist.playlist_id == playlist_id)?;
    let wanted = ItemId::new(playlist_id, Some(key));
    playlist
        .item_ids()
        .into_iter()
        .zip(&playlist.items)
        .find(|(id, _)| *id == wanted)
        .map(|(id, item)| (id, item.item.title.clone()))
}

#[derive(Deserialize)]
struct ItemForm {
    playlist_id: String,
    #[serde(default)]
    key: String,
    action: String,
}

#[route(POST "/watchlist/item")]
pub async fn run_item(cx: &Cx, Form(form): Form<ItemForm>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let place = place_of(cx)?;
    let action = ItemAction::from_str(&form.action)
        .ok()
        .ok_or_bad_request("The action is not valid.")?;
    let Some(view) = current(cx).await? else {
        return Ok(back(
            cx,
            place,
            Err("The watchlist is not available.".into()),
        ));
    };
    let Some((id, title)) = find_item(&view, &form.playlist_id, &form.key) else {
        return Ok(back(
            cx,
            place,
            Err("The item is not in the watchlist.".into()),
        ));
    };
    let outcome = backend(cx)
        .run(move |service| service.run_item(&ItemRequest { id, title, action }))
        .await?;
    Ok(back(
        cx,
        place,
        outcome
            .map(|id| format!("Queued {id}"))
            .map_err(|error| error.to_string()),
    ))
}

#[route(POST "/watchlist/retry-failed")]
pub async fn retry_failed(cx: &Cx, Form(form): Form<SourceForm>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let place = place_of(cx)?;
    let Some(view) = current(cx).await? else {
        return Ok(back(
            cx,
            place,
            Err("The watchlist is not available.".into()),
        ));
    };
    let Some(playlist) = view
        .playlists
        .iter()
        .find(|playlist| playlist.playlist_id == form.playlist_id)
    else {
        return Ok(back(
            cx,
            place,
            Err("The playlist is not in the watchlist.".into()),
        ));
    };
    let mut seen = HashSet::new();
    let requests: Vec<ItemRequest> = playlist
        .item_ids()
        .into_iter()
        .zip(&playlist.items)
        .filter(|(_, item)| retryable(item))
        .filter(|(id, _)| seen.insert(id.clone()))
        .map(|(id, item)| ItemRequest {
            id,
            title: item.item.title.clone(),
            action: ItemAction::Retry,
        })
        .collect();
    let outcome = backend(cx)
        .run(move |service| {
            let mut queued = 0_usize;
            for request in &requests {
                match service.run_item(request) {
                    Ok(_) => queued = queued.saturating_add(1),
                    Err(error) if queued == 0 => return Err(error.to_string()),
                    Err(_) => {}
                }
            }
            Ok(queued)
        })
        .await?;
    Ok(back(
        cx,
        place,
        outcome.map(|queued| format!("Queued {queued} items")),
    ))
}

path_param!(thumbnail_id);

#[route(GET "/watchlist/thumbnail/{thumbnail_id}")]
pub async fn thumbnail(
    cx: &Cx,
) -> Result<(StatusCode, [(&'static str, &'static str); 3], Vec<u8>)> {
    auth::require(cx).await?;
    let id = thumbnail_id(cx).to_owned();
    thumbnails::valid_id(&id).then_some(()).ok_or_not_found()?;
    let found = backend(cx)
        .run(move |service| {
            let path = thumbnails::cached_path(&id, &service.paths().cache)?;
            let png = path.extension().is_some_and(|extension| extension == "png");
            Some((png, std::fs::read(path).ok()?))
        })
        .await?
        .ok_or_not_found()?;
    let (png, bytes) = found;
    Ok((
        StatusCode::OK,
        [
            ("content-type", if png { "image/png" } else { "image/jpeg" }),
            ("cache-control", "private, max-age=3600"),
            ("x-content-type-options", "nosniff"),
        ],
        bytes,
    ))
}
