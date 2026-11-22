use std::{collections::HashMap, time::Duration};

use muzik_core::DecisionKind;
use muzik_runner::{AppEvent, choices, job_id, service::Service};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::{Receiver, error::RecvError};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{RouterErrorExt, SeeOther, see_other},
        href, page, query_params, route,
    },
    runtime::{connected, shard},
    view::{View, component, emit, live, view},
};

use crate::{
    auth,
    hub::{Progress, Snapshot},
    state::{backend, events, hub},
};

const JOB_LIMIT: usize = 100;
const DETAIL_LIMIT: usize = 6;
const LOG_LIMIT: usize = 30;
const CHAPTER_LIMIT: usize = 500;
const SETTLE: Duration = Duration::from_millis(150);
const RESYNC: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRow {
    pub id: String,
    pub title: String,
    pub kind: &'static str,
    pub status: String,
    pub percent: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceCard {
    pub index: usize,
    pub label: String,
    pub meta: String,
    pub suggested: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterRow {
    pub index: u64,
    pub start: u64,
    pub end: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionCard {
    pub target: String,
    pub job_id: String,
    pub heading: &'static str,
    pub job: String,
    pub note: String,
    pub details: Vec<String>,
    pub hidden_details: usize,
    pub choices: Vec<ChoiceCard>,
    pub chapters: Vec<ChapterRow>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Board {
    pub runner: bool,
    pub jobs: Vec<JobRow>,
    pub hidden_jobs: usize,
    pub decisions: Vec<DecisionCard>,
    pub log: Vec<String>,
}

#[must_use]
pub const fn kind_label(kind: &str) -> &'static str {
    match kind.as_bytes() {
        b"workflow" => "Workflow",
        b"refresh" => "Watchlist check",
        b"item" => "Item",
        b"library" => "Library",
        _ => "Job",
    }
}

#[must_use]
pub fn truncate(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn job_rows(jobs: &Value, progress: &HashMap<String, Progress>) -> (Vec<JobRow>, usize) {
    let mut rows: Vec<(bool, JobRow)> = jobs["open"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|job| {
            let id = job["job_id"].as_str().filter(|id| !id.is_empty())?;
            let queued = job["status"].as_str() == Some("queued");
            let known = progress.get(id);
            let status = if queued {
                "Queued".to_owned()
            } else {
                known
                    .map(|progress| progress.status.clone())
                    .filter(|status| !status.is_empty())
                    .unwrap_or_else(|| "Starting".to_owned())
            };
            Some((
                queued,
                JobRow {
                    id: id.to_owned(),
                    title: truncate(job["title"].as_str().unwrap_or("Job"), 120),
                    kind: kind_label(job["kind"].as_str().unwrap_or_default()),
                    status: truncate(&status, 120),
                    percent: known.and_then(Progress::percent),
                },
            ))
        })
        .collect();
    rows.sort_by_key(|(queued, _)| *queued);
    let hidden = rows.len().saturating_sub(JOB_LIMIT);
    (
        rows.into_iter()
            .map(|(_, row)| row)
            .take(JOB_LIMIT)
            .collect(),
        hidden,
    )
}

#[must_use]
pub fn decision_card(target: String, job_id: String, job: &str, question: &Value) -> DecisionCard {
    let kind = choices::kind(question);
    let suggested = choices::suggestion(question);
    let mut note = choices::note(question)
        .map(str::to_owned)
        .unwrap_or_default();
    if let Some(agent) = question
        .pointer("/payload/agent")
        .filter(|agent| agent.is_object())
        .and_then(choices::agent_note)
    {
        if !note.is_empty() {
            note.push(' ');
        }
        note.push_str(&agent);
    }
    let mut details = choices::details(question);
    let hidden_details = details.len().saturating_sub(DETAIL_LIMIT);
    details.truncate(DETAIL_LIMIT);
    let chapters = if kind == Some(DecisionKind::ChapterEdit) {
        question
            .pointer("/payload/chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|chapter| ChapterRow {
                index: chapter["index"].as_u64().unwrap_or(0),
                start: chapter["start"].as_u64().unwrap_or(0),
                end: chapter["end"]
                    .as_u64()
                    .map(|end| end.to_string())
                    .unwrap_or_default(),
                title: chapter["title"].as_str().unwrap_or_default().to_owned(),
            })
            .collect()
    } else {
        Vec::new()
    };
    DecisionCard {
        target,
        job_id,
        heading: choices::title(kind),
        job: truncate(job, 100),
        note,
        details: details.iter().map(|line| truncate(line, 160)).collect(),
        hidden_details,
        choices: choices::choices(question)
            .into_iter()
            .enumerate()
            .map(|(index, choice)| ChoiceCard {
                index,
                label: truncate(&choice.label, 120),
                meta: [choice.meta, choice.score.map(|score| format!("{score}%"))]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · "),
                suggested: suggested == Some(index),
            })
            .collect(),
        chapters,
    }
}

#[must_use]
pub fn build_board(jobs: &Value, snapshot: &Snapshot) -> Board {
    let (rows, hidden_jobs) = job_rows(jobs, &snapshot.progress);
    let title_of = |id: &str| {
        jobs["open"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|job| job["job_id"].as_str() == Some(id))
            .and_then(|job| job["title"].as_str())
            .unwrap_or("Job")
            .to_owned()
    };
    let mut decisions: Vec<DecisionCard> = snapshot
        .decisions
        .iter()
        .map(|decision| {
            decision_card(
                format!("live:{}", decision.decision_id),
                decision.job_id.clone(),
                &title_of(&decision.job_id),
                &decision.question,
            )
        })
        .collect();
    decisions.extend(
        jobs["waiting"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|waiting| {
                let id = waiting["id"].as_i64()?;
                let question = json!({"kind": waiting["kind"], "payload": waiting["payload"]});
                Some(decision_card(
                    format!("queue:{id}"),
                    job_id(id),
                    waiting["title"].as_str().unwrap_or("Item"),
                    &question,
                ))
            }),
    );
    Board {
        runner: jobs["runner"].as_bool().unwrap_or(true),
        jobs: rows,
        hidden_jobs,
        decisions,
        log: snapshot
            .log
            .iter()
            .take(LOG_LIMIT)
            .map(|line| truncate(line, 160))
            .collect(),
    }
}

async fn load(cx: &Cx) -> Result<Board> {
    let jobs = backend(cx).run(Service::jobs).await?;
    Ok(build_board(&jobs, &hub(cx).snapshot()))
}

#[query_params(error = bad_request)]
struct NoticeQuery {
    notice: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct NoticeOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn back(cx: &Cx, outcome: Result<String, String>) -> SeeOther {
    let notice = match outcome {
        Ok(text) => NoticeOut {
            notice: Some(truncate(&text, 200)),
            error: None,
        },
        Err(text) => NoticeOut {
            notice: None,
            error: Some(truncate(&text, 200)),
        },
    };
    see_other(href!(page).query(&notice).resolve(cx))
}

#[page("/")]
pub async fn page(cx: &Cx) -> Result<impl View> {
    auth::require(cx).await?;
    let query = query_params::<NoticeQuery>(cx)?;
    let message = query.error.clone().or_else(|| query.notice.clone());
    let failed = query.error.is_some();
    Ok(view! {
        <h1>"Activity"</h1>
        <p class=(if failed { "notice error" } else { "notice" })>(message)</p>
        <form class="launcher" method="post" action=(href!(start_download))>
            <input
                id="launch-raw"
                type="text"
                name="raw"
                placeholder="YouTube, Bandcamp or Spotify URL, local path, or search query"
                autocomplete="off"
                required="true"
            >
            <button class="primary" type="submit">"Download"</button>
        </form>
        panel()
    })
}

#[shard]
pub async fn panel(cx: &Cx) -> Result<impl View> {
    auth::require(cx).await?;
    Ok(live! {
        let mut changes = events(cx).0.subscribe();
        let mut shown: Option<Board> = None;
        loop {
            let current = load(cx).await?;
            let token = if shown.as_ref() == Some(&current) {
                None
            } else {
                shown = Some(current.clone());
                Some(emit! { jobs_board(data: current) }?)
            };
            if !connected(cx) {
                match token {
                    Some(token) => break Ok(token),
                    None => continue,
                }
            }
            if !auth::authenticated(cx).await? {
                break emit! { <p class="empty">"The session has ended. Reload the page to sign in again."</p> };
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

fn relevant(event: &AppEvent) -> bool {
    event.job_id().is_some()
        || matches!(event, AppEvent::JobsUpdated(_) | AppEvent::QueuesUpdated(_))
}

pub async fn settle(changes: &mut Receiver<AppEvent>, relevant: fn(&AppEvent) -> bool) -> bool {
    let wait = async {
        loop {
            match changes.recv().await {
                Ok(event) if !relevant(&event) => {}
                Ok(_) | Err(RecvError::Lagged(_)) => return true,
                Err(RecvError::Closed) => return false,
            }
        }
    };
    match tokio::time::timeout(RESYNC, wait).await {
        Ok(false) => return false,
        Ok(true) => tokio::time::sleep(SETTLE).await,
        Err(_) => {}
    }
    while changes.try_recv().is_ok() {}
    true
}

#[component]
async fn jobs_board(cx: &Cx, data: Board) -> Result<impl View> {
    let quiet = data.jobs.is_empty();
    let runner = data.runner;
    let waiting = data.decisions.len();
    let log_empty = data.log.is_empty();
    Ok(view! {
        <div class="grid">
            <section class="card">
                <h2>"Jobs"</h2>
                <div class="scroll">
                    if !runner {
                        <div class="empty">"The job runner is not running."</div>
                    }
                    if quiet {
                        <div class="empty">"No jobs yet."</div>
                    }
                    for job in data.jobs {
                        <div class="row" id=(format!("job-{}", job.id))>
                            <div class="main">
                                <div class="title">(job.title)</div>
                                <div class="meta">(job.kind) " · " (job.status)</div>
                                <div class="bar-track">
                                    if let Some(percent) = job.percent {
                                        <div class="bar-fill" style=(format!("width:{percent}%"))></div>
                                    }
                                </div>
                            </div>
                            <form method="post" action=(href!(cancel))>
                                <input type="hidden" name="job_id" value=(job.id)>
                                <button class="small" type="submit">"Cancel"</button>
                            </form>
                        </div>
                    }
                    if data.hidden_jobs > 0 {
                        <div class="empty">(data.hidden_jobs) " more in the queue"</div>
                    }
                </div>
            </section>
            <section class="card">
                <h2>"Needs your answer (" (waiting) ")"</h2>
                <div class="scroll">
                    if waiting == 0 {
                        <div class="empty">"Nothing waits for you."</div>
                    }
                    for card in data.decisions {
                        decision_form(card: card)
                    }
                </div>
            </section>
        </div>
        <section class="card" style="margin-top:16px">
            <h2>"Recent events"</h2>
            <div class="scroll short log">
                if log_empty {
                    <div>"Job updates appear here."</div>
                }
                for line in data.log {
                    <div>(line)</div>
                }
            </div>
        </section>
    })
}

#[component]
async fn decision_form(cx: &Cx, card: DecisionCard) -> Result<impl View> {
    let editing = !card.chapters.is_empty();
    let count = card.chapters.len();
    let has_note = !card.note.is_empty();
    Ok(view! {
        <form class="decision" method="post" action=(href!(decide)) id=(format!("decision-{}", card.target))>
            <input type="hidden" name="target" value=(card.target.clone())>
            <h3>(card.heading)</h3>
            <div class="meta">(card.job)</div>
            if has_note {
                <div class="meta">(card.note)</div>
            }
            <div class="details">
                for line in card.details {
                    <div>(line)</div>
                }
                if card.hidden_details > 0 {
                    <div>(card.hidden_details) " more"</div>
                }
            </div>
            if editing {
                <input type="hidden" name="count" value=(count)>
                <div class="chapters">
                    for (position, chapter) in card.chapters.into_iter().enumerate() {
                        <div class="chapter">
                            <input type="hidden" name=(format!("index_{position}")) value=(chapter.index)>
                            <input
                                type="number"
                                min="0"
                                name=(format!("start_{position}"))
                                id=(format!("start-{}-{position}", card.target))
                                value=(chapter.start)
                                aria-label="Start in seconds"
                            >
                            <input
                                type="number"
                                min="0"
                                name=(format!("end_{position}"))
                                id=(format!("end-{}-{position}", card.target))
                                value=(chapter.end)
                                aria-label="End in seconds"
                            >
                            <input
                                type="text"
                                name=(format!("title_{position}"))
                                id=(format!("title-{}-{position}", card.target))
                                value=(chapter.title)
                                aria-label="Title"
                            >
                        </div>
                    }
                    <button class="primary" type="submit" name="chapters" value="save">"Use these chapters"</button>
                </div>
            }
            <div class="choices">
                <button class="small" type="submit" formnovalidate="true" formaction=(href!(cancel)) name="job_id" value=(card.job_id)>
                    "Cancel the job"
                </button>
                for choice in card.choices {
                    <button
                        type="submit"
                        formnovalidate="true"
                        name="choice"
                        value=(choice.index)
                        class=(if choice.suggested { "primary" } else { "" })
                    >
                        (choice.label)
                        if !choice.meta.is_empty() {
                            <span class="meta">" · " (choice.meta)</span>
                        }
                    </button>
                }
            </div>
        </form>
    })
}

#[derive(Deserialize)]
struct StartForm {
    raw: String,
}

#[route(POST "/activity/start")]
pub async fn start_download(cx: &Cx, Form(form): Form<StartForm>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let raw = form.raw.trim().to_owned();
    if raw.is_empty() {
        return Ok(back(
            cx,
            Err("Enter a URL, a local path, or a search query.".into()),
        ));
    }
    let outcome = backend(cx)
        .run(move |service| {
            let mut params = match serde_json::to_value(service.defaults())? {
                Value::Object(params) => params,
                _ => serde_json::Map::new(),
            };
            params.remove("motion");
            params.insert("raw".into(), json!(raw));
            service.start_workflow(&Value::Object(params))
        })
        .await?;
    Ok(back(
        cx,
        outcome
            .map(|id| format!("Queued {id}"))
            .map_err(|error| error.to_string()),
    ))
}

#[derive(Deserialize)]
struct CancelForm {
    job_id: String,
}

#[route(POST "/activity/cancel")]
pub async fn cancel(cx: &Cx, Form(form): Form<CancelForm>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let target = form.job_id;
    let outcome = backend(cx)
        .run({
            let target = target.clone();
            move |service| service.cancel(&target)
        })
        .await?;
    Ok(back(
        cx,
        outcome
            .map(|()| format!("Stopping {target}"))
            .map_err(|error| error.to_string()),
    ))
}

fn chapter_answer(form: &HashMap<String, String>) -> Option<Value> {
    let count: usize = form.get("count")?.parse().ok()?;
    if count > CHAPTER_LIMIT {
        return None;
    }
    let mut chapters = Vec::new();
    for position in 0..count {
        let field = |name: &str| form.get(&format!("{name}_{position}"));
        let index: u64 = field("index")?.trim().parse().ok()?;
        let start: u64 = field("start")?.trim().parse().ok()?;
        let end_text = field("end")?.trim();
        let end: Option<u64> = if end_text.is_empty() {
            None
        } else {
            Some(end_text.parse().ok()?)
        };
        let title = field("title")?.trim();
        if index == 0 || title.is_empty() || end.is_some_and(|end| end <= start) {
            return None;
        }
        chapters.push(json!({"index": index, "start": start, "end": end, "title": title}));
    }
    Some(Value::Array(chapters))
}

#[derive(Debug, PartialEq, Eq)]
enum Target {
    Live(String),
    Queue(i64),
}

fn parse_target(text: &str) -> Option<Target> {
    if let Some(id) = text.strip_prefix("live:") {
        return Some(Target::Live(id.to_owned()));
    }
    text.strip_prefix("queue:")?.parse().ok().map(Target::Queue)
}

async fn question_for(cx: &Cx, target: &Target) -> Result<Option<Value>> {
    match target {
        Target::Live(id) => Ok(hub(cx).decision(id).map(|decision| decision.question)),
        Target::Queue(id) => {
            let id = *id;
            let jobs = backend(cx).run(Service::jobs).await?;
            Ok(jobs
                .get("waiting")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|waiting| waiting["id"].as_i64() == Some(id))
                .map(|waiting| json!({"kind": waiting["kind"], "payload": waiting["payload"]})))
        }
    }
}

#[route(POST "/activity/decision")]
pub async fn decide(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let target = form
        .get("target")
        .and_then(|text| parse_target(text))
        .ok_or_bad_request("The decision is not valid.")?;
    let Some(question) = question_for(cx, &target).await? else {
        return Ok(back(cx, Err("This decision is no longer open.".into())));
    };
    let value = if form.contains_key("chapters") {
        chapter_answer(&form)
    } else {
        form.get("choice")
            .and_then(|text| text.parse::<usize>().ok())
            .and_then(|index| choices::choices(&question).into_iter().nth(index))
            .map(|choice| choice.value)
    }
    .ok_or_bad_request("The answer is not valid.")?;
    let outcome = match target {
        Target::Live(id) => {
            let outcome = backend(cx)
                .run({
                    let id = id.clone();
                    move |service| service.reply(&id, value)
                })
                .await?;
            if outcome.is_ok() {
                hub(cx).resolve(&id);
            }
            outcome
        }
        Target::Queue(id) => {
            backend(cx)
                .run(move |service| {
                    service.answer(id, &value).and_then(|saved| {
                        if saved {
                            Ok(())
                        } else {
                            Err("This decision is no longer open.".into())
                        }
                    })
                })
                .await?
        }
    };
    Ok(back(
        cx,
        outcome
            .map(|()| "Answer sent".to_owned())
            .map_err(|error| error.to_string()),
    ))
}

#[cfg(test)]
mod tests {
    use super::{Target, chapter_answer, parse_target};
    use serde_json::json;
    use std::collections::HashMap;

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn targets_name_a_live_or_a_saved_decision() {
        assert_eq!(parse_target("live:abc"), Some(Target::Live("abc".into())));
        assert_eq!(parse_target("queue:7"), Some(Target::Queue(7)));
        assert_eq!(parse_target("queue:x"), None);
        assert_eq!(parse_target("other:1"), None);
    }

    #[test]
    fn chapter_rows_become_an_answer_only_when_every_row_is_valid() {
        let valid = form(&[
            ("count", "2"),
            ("index_0", "1"),
            ("start_0", "0"),
            ("end_0", "60"),
            ("title_0", "Intro"),
            ("index_1", "2"),
            ("start_1", "60"),
            ("end_1", ""),
            ("title_1", "Song"),
        ]);
        assert_eq!(
            chapter_answer(&valid),
            Some(json!([
                {"index": 1, "start": 0, "end": 60, "title": "Intro"},
                {"index": 2, "start": 60, "end": null, "title": "Song"},
            ]))
        );
        let mut backwards = valid.clone();
        backwards.insert("end_0".into(), "0".into());
        assert_eq!(chapter_answer(&backwards), None);
        let mut untitled = valid;
        untitled.insert("title_1".into(), " ".into());
        assert_eq!(chapter_answer(&untitled), None);
    }
}

#[cfg(test)]
mod board_tests {
    use super::build_board;
    use crate::hub::{Progress, Snapshot};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn the_board_lists_running_jobs_first_and_both_kinds_of_decision() {
        let jobs = json!({
            "open": [
                {"job_id": "queue-2", "title": "Waiting", "kind": "item", "status": "queued", "item": null},
                {"job_id": "queue-1", "title": "Album", "kind": "workflow", "status": "running", "item": null},
            ],
            "waiting": [
                {"id": 5, "title": "Song", "kind": "import_duplicate", "payload": {}, "item": null},
            ],
            "runner": true,
        });
        let snapshot = Snapshot {
            progress: HashMap::from([(
                "queue-1".to_owned(),
                Progress {
                    status: "Download".into(),
                    completed: 1,
                    total: Some(4),
                    ..Progress::default()
                },
            )]),
            ..Snapshot::default()
        };
        let board = build_board(&jobs, &snapshot);
        let ids: Vec<&str> = board.jobs.iter().map(|job| job.id.as_str()).collect();
        assert_eq!(ids, ["queue-1", "queue-2"]);
        assert_eq!(board.jobs[0].status, "Download");
        assert_eq!(board.jobs[0].percent, Some(25));
        assert_eq!(board.jobs[1].status, "Queued");
        assert_eq!(board.decisions.len(), 1);
        assert_eq!(board.decisions[0].target, "queue:5");
        assert_eq!(board.decisions[0].job_id, "queue-5");
        assert_eq!(board.decisions[0].choices.len(), 3);
    }
}
