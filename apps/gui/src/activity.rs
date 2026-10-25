//! Desktop activity state. Queue snapshots and live events enter through this module.
use muzik_core::{JobEvent, Task as JobTask};
use muzik_runner::{AppEvent, choices};
use muzik_store::jobs::{Kind as JobKind, Status as JobStatus};
use muzik_store::watchlist::ItemId;
use serde_json::{Value, json};
use std::collections::HashSet;
use strum_macros::{Display, EnumString};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplyTarget {
    Live(String),
    Queue(i64),
}

#[derive(Clone)]
pub struct Decision {
    target: ReplyTarget,
    job_id: String,
    pub title: Option<String>,
    pub item: Option<String>,
    pub question: Value,
}
impl Decision {
    fn saved(job: &Value) -> Option<Self> {
        let id = job["id"].as_i64()?;
        Some(Self {
            target: ReplyTarget::Queue(id),
            job_id: muzik_runner::job_id(id),
            title: job["title"].as_str().map(str::to_owned),
            item: job["item"].as_str().map(str::to_owned),
            question: json!({"kind":job["kind"],"payload":job["payload"]}),
        })
    }

    pub const fn queue_job(&self) -> Option<i64> {
        match self.target {
            ReplyTarget::Queue(id) => Some(id),
            ReplyTarget::Live(_) => None,
        }
    }

    pub const fn target(&self) -> &ReplyTarget {
        &self.target
    }
}
pub struct Finished {
    pub kind: RunKind,
    pub title: String,
    pub ending: Ending,
    pub failure: Option<String>,
}
#[derive(Default)]
pub struct Activity {
    runs: Vec<Run>,
    finished_runs: HashSet<String>,
    queued_items: HashSet<ItemId>,
    sections: Vec<ActivitySection>,
    logs: Vec<String>,
    decision: Option<Decision>,
    requests: Vec<Decision>,
    waiting: Vec<Decision>,
}
#[derive(Default)]
pub struct ActivityProgress {
    pub description: String,
    pub task: Option<JobTask>,
    pub completed: f64,
    pub total: Option<f64>,
}

impl ActivityProgress {
    pub fn percentage(&self) -> f32 {
        self.total.filter(|total| *total > 0.).map_or(0., |total| {
            f64_to_f32((self.completed / total * 100.).clamp(0., 100.))
        })
    }
}

#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "std has no From<u64> for f64; progress counts tolerate rounding"
)]
pub const fn u64_to_f64(value: u64) -> f64 {
    value as f64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum Ending {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum RunKind {
    Workflow,
    Refresh,
    Item,
    SpotifyLogin,
    #[default]
    #[strum(disabled)]
    Unknown,
}

impl From<JobKind> for RunKind {
    fn from(kind: JobKind) -> Self {
        match kind {
            JobKind::Workflow => Self::Workflow,
            JobKind::Refresh => Self::Refresh,
            JobKind::Item => Self::Item,
        }
    }
}

impl RunKind {
    fn parse(value: &Value) -> Self {
        value
            .as_str()
            .and_then(|kind| kind.parse().ok())
            .unwrap_or_default()
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Workflow => "Workflow",
            Self::Refresh => "Watchlist check",
            Self::Item => "Item",
            Self::SpotifyLogin => "Spotify connection",
            Self::Unknown => "Job",
        }
    }
}

pub struct Run {
    pub id: String,
    pub kind: RunKind,
    pub title: String,
    pub status: String,
    pub queued: bool,
    pub progress: ActivityProgress,
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

    pub fn progress_text(&self) -> String {
        self.progress.total.map_or_else(String::new, |total| {
            format!("{:.0} / {:.0}", self.progress.completed, total)
        })
    }
}

pub struct ActivitySection {
    pub title: &'static str,
    pub count: usize,
    pub rows: Vec<String>,
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "std has no From<f64> for f32; display values tolerate rounding"
)]
const fn f64_to_f32(value: f64) -> f32 {
    value as f32
}

impl Activity {
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }
    pub const fn queued_items(&self) -> &HashSet<ItemId> {
        &self.queued_items
    }
    pub fn sections(&self) -> &[ActivitySection] {
        &self.sections
    }
    pub fn logs(&self) -> &[String] {
        &self.logs
    }
    pub fn waiting(&self) -> &[Decision] {
        &self.waiting
    }
    pub const fn decision(&self) -> Option<&Decision> {
        self.decision.as_ref()
    }

    pub fn start_login(&mut self, id: &str) {
        if let Some(run) = self.run_mut(id) {
            run.kind = RunKind::SpotifyLogin;
            run.title = "Spotify connection".into();
            run.status = "Waiting for the browser".into();
        }
    }
    pub fn open_waiting(&mut self, id: i64) {
        if let Some(choice) = self
            .waiting
            .iter()
            .find(|choice| choice.target == ReplyTarget::Queue(id))
        {
            self.decision = Some(choice.clone());
        }
    }
    pub fn defer(&mut self) {
        if self
            .decision
            .as_ref()
            .is_some_and(|choice| matches!(choice.target, ReplyTarget::Queue(_)))
        {
            self.decision = self.requests.first().cloned();
        }
    }
    pub fn reply(&mut self) -> Option<ReplyTarget> {
        let choice = self.decision.take()?;
        self.requests
            .retain(|request| request.target != choice.target);
        self.waiting
            .retain(|waiting| waiting.target != choice.target);
        if matches!(choice.target, ReplyTarget::Live(_)) {
            self.set_status(&choice.job_id, "Working");
        }
        self.decision = self.requests.first().cloned();
        Some(choice.target)
    }
    pub fn apply(&mut self, event: &AppEvent) -> Option<Finished> {
        if let AppEvent::JobStarted { job_id, .. } = event {
            self.finished_runs.remove(job_id);
        }
        if event
            .job_id()
            .is_some_and(|id| self.finished_runs.contains(id))
        {
            return None;
        }
        match event {
            AppEvent::JobsUpdated(data) => self.apply_jobs(data),
            AppEvent::JobStarted {
                job_id,
                title,
                kind,
            } => {
                if let Some(run) = self.run_mut(job_id) {
                    run.kind = (*kind).into();
                    run.title.clone_from(title);
                    run.status = "Starting".into();
                }
            }
            AppEvent::JobEvent { job_id, event, .. } => self.record_job_event(job_id, event),
            AppEvent::DecisionRequest {
                job_id,
                decision_id,
                kind,
                payload,
            } => {
                let target = ReplyTarget::Live(decision_id.clone());
                if !self.requests.iter().any(|choice| choice.target == target) {
                    self.requests.push(Decision {
                        target,
                        job_id: job_id.clone(),
                        title: None,
                        item: None,
                        question: json!({"kind":kind,"payload":payload}),
                    });
                }
                self.set_status(job_id, "Decision needed");
                if self.decision.is_none() {
                    self.decision = self.requests.first().cloned();
                }
            }
            AppEvent::JobCompleted { job_id, .. } => {
                return Some(self.finish(job_id, Ending::Completed, None));
            }
            AppEvent::JobCancelled { job_id } => {
                return Some(self.finish(job_id, Ending::Cancelled, None));
            }
            AppEvent::JobFailed { job_id, message } => {
                return Some(self.finish(job_id, Ending::Failed, Some(message.clone())));
            }
            _ => {}
        }
        None
    }
    fn finish(&mut self, id: &str, ending: Ending, failure: Option<String>) -> Finished {
        self.finished_runs.insert(id.to_owned());
        let run = self
            .runs
            .iter()
            .position(|run| run.id == id)
            .map(|index| self.runs.remove(index));
        let kind = run.as_ref().map_or(RunKind::Unknown, |run| run.kind);
        let title = run.map_or_else(|| "Job".to_owned(), |run| run.title);
        self.requests.retain(|choice| choice.job_id != id);
        self.waiting.retain(|choice| choice.job_id != id);
        if self
            .decision
            .as_ref()
            .is_some_and(|choice| choice.job_id == id)
        {
            self.decision = self.requests.first().cloned();
        }
        self.log(short_text(
            &format!("{} {ending}", job_label(kind, &title)),
            180,
        ));
        Finished {
            kind,
            title,
            ending,
            failure,
        }
    }
    fn log(&mut self, line: String) {
        self.logs.push(line);
        if self.logs.len() > 300 {
            self.logs.drain(..100);
        }
    }
    pub fn has_run(&self, kind: RunKind) -> bool {
        self.runs.iter().any(|run| run.kind == kind)
    }

    fn run_mut(&mut self, id: &str) -> Option<&mut Run> {
        if self.finished_runs.contains(id) {
            return None;
        }
        let run = match self.runs.iter().position(|run| run.id == id) {
            Some(index) => self.runs.get_mut(index)?,
            None => self.runs.push_mut(Run::new(id, RunKind::Unknown, "Job")),
        };
        run.queued = false;
        Some(run)
    }

    fn set_status(&mut self, id: &str, status: impl Into<String>) {
        if let Some(run) = self.run_mut(id) {
            run.status = status.into();
        }
    }

    pub fn apply_jobs(&mut self, snapshot: &Value) {
        self.waiting = snapshot["waiting"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Decision::saved)
            .collect();
        if self.decision.as_ref().is_some_and(|choice| {
            matches!(choice.target, ReplyTarget::Queue(_))
                && !self
                    .waiting
                    .iter()
                    .any(|waiting| waiting.target == choice.target)
        }) {
            self.decision = self.requests.first().cloned();
        }
        let open: &[Value] = snapshot["open"].as_array().map_or(&[], Vec::as_slice);
        self.queued_items = open
            .iter()
            .filter_map(|job| job["item"].as_str()?.parse().ok())
            .collect();
        let listed: HashSet<&str> = open
            .iter()
            .filter_map(|job| job["job_id"].as_str())
            .collect();
        self.runs
            .retain(|run| !run.queued || listed.contains(run.id.as_str()));
        for job in open {
            let Some(id) = job["job_id"]
                .as_str()
                .filter(|id| !id.is_empty() && !self.finished_runs.contains(*id))
            else {
                continue;
            };
            let queued = job["status"]
                .as_str()
                .and_then(|status| status.parse::<JobStatus>().ok())
                == Some(JobStatus::Queued);
            let kind = RunKind::parse(&job["kind"]);
            let title = job["title"].as_str().unwrap_or("Job");
            if let Some(run) = self.runs.iter_mut().find(|run| run.id == id) {
                run.kind = kind;
                run.title = title.into();
                if !queued && run.queued {
                    run.queued = false;
                    run.status = "Starting".into();
                }
            } else {
                let mut run = Run::new(id, kind, title);
                if !queued {
                    run.queued = false;
                    run.status = "Starting".into();
                }
                self.runs.push(run);
            }
        }
        self.runs.sort_by_key(|run| run.queued);
    }

    fn record_job_event(&mut self, job_id: &str, event: &JobEvent) {
        let line = match event {
            JobEvent::ProgressStarted {
                task,
                description,
                total,
            } => {
                let Some(run) = self.run_mut(job_id) else {
                    return;
                };
                let progress = &mut run.progress;
                progress.task = Some(*task);
                progress.description.clone_from(description);
                progress.completed = 0.;
                progress.total = total.filter(|total| *total > 0).map(u64_to_f64);
                run.status = progress.description.clone();
                run.status.clone()
            }
            JobEvent::ProgressAdvanced {
                task,
                completed,
                total,
            } => {
                let Some(run) = self.run_mut(job_id) else {
                    return;
                };
                let progress = &mut run.progress;
                if progress.task != Some(*task) {
                    return;
                }
                if let Some(total) = total.filter(|total| *total > 0) {
                    progress.total = Some(u64_to_f64(total));
                }
                progress.completed = completed.map_or(progress.completed + 1., u64_to_f64);
                String::new()
            }
            JobEvent::ProgressFinished { task, .. } => {
                let Some(run) = self.run_mut(job_id) else {
                    return;
                };
                let progress = &mut run.progress;
                if progress.task != Some(*task) {
                    return;
                }
                if let Some(total) = progress.total {
                    progress.completed = total;
                }
                format!("{} finished", progress.description)
            }
            JobEvent::StepStarted(step) => {
                self.set_status(job_id, step.to_string());
                format!("Started {step}")
            }
            JobEvent::StepFinished(step) => {
                if let Some(run) = self.run_mut(job_id)
                    && run.progress.total.is_some()
                {
                    run.progress.completed += 1.;
                }
                format!("{step} finished")
            }
            JobEvent::Message { message, .. } => {
                self.set_status(job_id, message.clone());
                message.clone()
            }
            JobEvent::CandidatesFound { source, candidates } => {
                self.set_activity_section(activity_section(
                    "Source candidates",
                    candidates,
                    candidate_summary,
                ));
                format!(
                    "{} {source} candidates found",
                    self.sections
                        .iter()
                        .find(|section| section.title == "Source candidates")
                        .map_or(0, |section| section.count),
                )
            }
            JobEvent::ItemWaiting { title, question } => {
                let kind = choices::title(choices::kind(question));
                self.set_status(job_id, format!("Waiting for you: {kind}"));
                format!("{title} waits for you: {kind}")
            }
            JobEvent::AgentDecided {
                label,
                confidence,
                reason,
            } => {
                self.set_status(job_id, format!("Chose {label}"));
                format!("Chose {label} ({:.0}%): {reason}", confidence * 100.0)
            }
            JobEvent::WatchlistSaved => return,
        };
        if !line.is_empty() {
            self.log(short_text(&shorten_paths(&line), 180));
        }
    }

    fn set_activity_section(&mut self, section: ActivitySection) {
        if let Some(existing) = self
            .sections
            .iter_mut()
            .find(|existing| existing.title == section.title)
        {
            *existing = section;
        } else {
            self.sections.push(section);
        }
    }
}
fn shorten_paths(line: &str) -> String {
    let Some((head, tail)) = line
        .find('/')
        .and_then(|start| line.split_at_checked(start))
        .filter(|(head, _)| head.is_empty() || head.ends_with(' '))
    else {
        return line.to_owned();
    };
    let path = std::path::Path::new(tail);
    if path.components().count() < 3 {
        return line.to_owned();
    }
    path.file_name().map_or_else(
        || line.to_owned(),
        |name| format!("{head}{}", name.to_string_lossy()),
    )
}

pub fn short_text(value: &str, limit: usize) -> String {
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
    items: &[Value],
    summary: fn(&Value) -> String,
) -> ActivitySection {
    ActivitySection {
        title,
        count: items.len(),
        rows: items
            .iter()
            .take(4)
            .map(|item| short_text(&summary(item), 120))
            .collect(),
    }
}

fn candidate_summary(candidate: &Value) -> String {
    let title = candidate["title"].as_str().unwrap_or("Unknown title");
    let format = candidate
        .pointer("/quality/format")
        .and_then(Value::as_str)
        .unwrap_or("Unknown format");
    let user = candidate["user"].as_str().unwrap_or("Unknown user");
    format!("{title} · {format} · {user}")
}

pub fn job_label(kind: RunKind, title: &str) -> String {
    match kind {
        RunKind::Refresh | RunKind::SpotifyLogin => kind.label().into(),
        _ => format!("{} “{}”", kind.label(), short_text(title, 60)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job_event(event: JobEvent) -> AppEvent {
        AppEvent::JobEvent {
            job_id: "queue-1".into(),
            source: muzik_runner::Source::Native,
            event,
        }
    }

    #[test]
    fn snapshots_and_live_progress_describe_one_run_until_it_finishes() {
        let mut activity = Activity::default();
        let snapshot = json!({"open":[{"job_id":"queue-1", "kind":"item", "title":"Album", "status":"queued", "item":"PL1:1:video"}], "waiting":[]});
        activity.apply_jobs(&snapshot);
        assert!(
            activity
                .queued_items()
                .contains(&ItemId::new("PL1", 1, Some("video")))
        );
        activity.apply(&job_event(JobEvent::ProgressStarted {
            task: JobTask::LocalSplit,
            description: "Split tracks".into(),
            total: Some(10),
        }));
        activity.apply_jobs(&snapshot);
        assert_eq!(activity.runs().len(), 1);
        assert!(!activity.runs()[0].queued);
        assert_eq!(activity.runs()[0].status, "Split tracks");
        activity.apply(&job_event(JobEvent::ProgressAdvanced {
            task: JobTask::LocalSplit,
            completed: Some(12),
            total: None,
        }));
        assert!((activity.runs()[0].progress.percentage() - 100.).abs() < f32::EPSILON);
        activity.apply(&job_event(JobEvent::ProgressAdvanced {
            task: JobTask::WatchlistRefresh,
            completed: Some(2),
            total: Some(20),
        }));
        assert_eq!(activity.runs()[0].progress_text(), "12 / 10");
        let finished = activity
            .apply(&AppEvent::JobCompleted {
                job_id: "queue-1".into(),
                result: Value::Null,
            })
            .unwrap();
        assert_eq!(finished.title, "Album");
        assert_eq!(finished.kind, RunKind::Item);
        assert_eq!(finished.ending, Ending::Completed);
        activity.apply_jobs(&snapshot);
        activity.apply(&job_event(JobEvent::message("Late message")));
        assert!(activity.runs().is_empty());
        activity.apply(&AppEvent::JobStarted {
            job_id: "queue-1".into(),
            title: "Resumed album".into(),
            kind: JobKind::Item,
        });
        assert_eq!(activity.runs()[0].title, "Resumed album");
    }

    fn request(job: &str, decision: &str) -> AppEvent {
        AppEvent::DecisionRequest {
            job_id: job.into(),
            decision_id: decision.into(),
            kind: muzik_core::DecisionKind::ChapterEdit,
            payload: json!({"chapters":[]}),
        }
    }

    #[test]
    fn choices_route_replies_and_advance_after_completion_or_deferral() {
        let mut activity = Activity::default();
        activity.apply(&request("queue-1", "first"));
        activity.apply(&request("queue-2", "second"));
        activity.apply(&request("queue-2", "second"));
        activity.apply_jobs(&json!({"waiting":[{"id":3,"item":"PL1:3:video", "kind":"chapter_edit", "payload":{"chapters":[]}}]}));
        activity.open_waiting(3);
        assert_eq!(activity.decision().unwrap().queue_job(), Some(3));
        assert_eq!(
            activity.decision().unwrap().item.as_deref(),
            Some("PL1:3:video")
        );
        activity.defer();
        assert_eq!(
            activity.decision().unwrap().target,
            ReplyTarget::Live("first".into())
        );
        activity.apply(&AppEvent::JobCancelled {
            job_id: "queue-1".into(),
        });
        assert_eq!(
            activity.decision().unwrap().target,
            ReplyTarget::Live("second".into())
        );
        activity.open_waiting(3);
        assert_eq!(activity.reply(), Some(ReplyTarget::Queue(3)));
        assert!(activity.waiting().is_empty());
        assert_eq!(activity.reply(), Some(ReplyTarget::Live("second".into())));
        assert!(activity.decision().is_none());
        assert_eq!(
            activity
                .runs()
                .iter()
                .find(|run| run.id == "queue-2")
                .unwrap()
                .status,
            "Working"
        );
        activity.apply(&request("queue-1", "late"));
        assert!(activity.decision().is_none());
    }

    #[test]
    fn a_saved_choice_closes_when_the_queue_no_longer_waits_for_it() {
        let mut activity = Activity::default();
        activity.apply_jobs(&json!({"waiting":[{"id":3,"kind":"chapter_edit"}]}));
        activity.open_waiting(3);
        activity.apply(&request("queue-4", "live"));
        activity.apply_jobs(&json!({"waiting":[]}));
        assert_eq!(activity.reply(), Some(ReplyTarget::Live("live".into())));
    }

    #[test]
    fn job_events_update_status_and_bound_candidate_rows() {
        let mut activity = Activity::default();
        activity.apply(&job_event(JobEvent::StepStarted(muzik_core::Step::Import)));
        assert_eq!(activity.runs()[0].status, "import");
        activity.apply(&job_event(JobEvent::CandidatesFound {
            source: muzik_core::CandidateSource::Soulseek,
            candidates: (0..6).map(|index| json!({"title":format!("Album {index}"),"quality":{"format":"FLAC"},"user":"listener"})).collect(),
        }));
        let section = &activity.sections()[0];
        assert_eq!(section.count, 6);
        assert_eq!(section.rows.len(), 4);
        assert_eq!(section.rows[0], "Album 0 · FLAC · listener");
        activity.apply(&job_event(JobEvent::message("Writing tags")));
        assert_eq!(activity.runs()[0].status, "Writing tags");
        assert!(activity.logs().iter().any(|line| line == "Writing tags"));
        assert!(
            activity
                .logs()
                .iter()
                .any(|line| line == "6 soulseek candidates found")
        );
    }

    #[test]
    fn event_lines_show_the_folder_name_instead_of_the_full_path() {
        assert_eq!(
            shorten_paths(
                "Import group 1 of 1: /Users/tudor/Library/Application Support/muzik/splits/GENDEMA - sassy things [Full album] [wiih44Gfi2M]"
            ),
            "Import group 1 of 1: GENDEMA - sassy things [Full album] [wiih44Gfi2M]"
        );
        assert_eq!(shorten_paths("Split 3/12 tracks"), "Split 3/12 tracks");
        assert_eq!(shorten_paths("Started import"), "Started import");
    }
}
