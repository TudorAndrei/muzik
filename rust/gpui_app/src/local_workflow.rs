//! Local audio workflow adapter for jobs that do not need import decisions.

use muzik_core::{paths, splitter};
use muzik_workflow::{
    classify_input, run_workflow_with_events, SplitProgress, SplitTask, WorkflowEvent,
    WorkflowInput, WorkflowOperations, WorkflowOptions, WorkflowRequest,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub struct LocalRequest {
    request: WorkflowRequest,
    options: WorkflowOptions,
}

/// Return `None` when the Python workflow still owns this mode.
pub fn supported(params: &Value) -> Option<Result<LocalRequest, String>> {
    let raw = params.get("raw")?.as_str()?.trim();
    if !matches!(classify_input(raw), WorkflowInput::Local(_))
        || params.get("no_organize") != Some(&Value::Bool(true))
        || params.get("review") == Some(&Value::Bool(true))
        || params.get("tag_only") == Some(&Value::Bool(true))
    {
        return None;
    }
    Some(parse(raw, params))
}

fn parse(raw: &str, params: &Value) -> Result<LocalRequest, String> {
    let mut options = WorkflowOptions {
        no_organize: true,
        ..WorkflowOptions::default()
    };
    for (key, target) in [
        ("no_split", &mut options.no_split),
        ("dry_run", &mut options.dry_run),
        ("keep_source", &mut options.keep_source),
        ("force", &mut options.force),
    ] {
        if let Some(value) = params.get(key) {
            *target = value
                .as_bool()
                .ok_or_else(|| format!("{key} must be a boolean"))?;
        }
    }
    if let Some(value) = params.get("jobs") {
        options.jobs = value
            .as_u64()
            .and_then(|number| usize::try_from(number).ok())
            .ok_or("jobs must be a non-negative integer")?;
    }
    let request = WorkflowRequest {
        raw: raw.to_owned(),
        output: path(params, "output")?.unwrap_or_else(paths::download_dir),
        splits: path(params, "splits")?.unwrap_or_else(|| paths::data_dir().join("splits")),
    };
    Ok(LocalRequest { request, options })
}

fn path(params: &Value, key: &str) -> Result<Option<PathBuf>, String> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| format!("{key} must be a string"))?;
    if value.trim().is_empty() {
        return Ok(None);
    }
    if let Some(rest) = value.strip_prefix("~/") {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        return Ok(Some(PathBuf::from(home).join(rest)));
    }
    Ok(Some(PathBuf::from(value)))
}

pub fn run(
    local: LocalRequest,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
) -> Result<Value, muzik_workflow::Error> {
    let mut operations = LocalOperations;
    let result = run_workflow_with_events(
        &local.request,
        &local.options,
        &mut operations,
        cancelled,
        &mut |event| on_event(event_record(event)),
    )?;
    Ok(json!({
        "albums": result.plan.albums.len(),
        "singles": result.plan.singles.len(),
        "split_dirs": result.split_dirs,
    }))
}

struct LocalOperations;

impl WorkflowOperations for LocalOperations {
    fn download_youtube(&mut self, _: &str, _: &Path, _: bool) -> Result<Vec<PathBuf>, String> {
        Err("YouTube download is not part of a local audio job".into())
    }

    fn acquire_soulseek(&mut self, _: &str) -> Result<Vec<PathBuf>, String> {
        Err("Soulseek is not part of a local audio job".into())
    }

    fn organize(&mut self, _: &Path, _: &WorkflowOptions) -> Result<(), String> {
        Err("Organization is not part of a local audio job".into())
    }

    fn split(&mut self, task: &SplitTask, options: &WorkflowOptions) -> Result<(), String> {
        self.split_with_cancel(task, options, &AtomicBool::new(false), &mut |_| {})
    }

    fn split_with_cancel(
        &mut self,
        task: &SplitTask,
        options: &WorkflowOptions,
        cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(SplitProgress),
    ) -> Result<(), String> {
        let settings = splitter::SplitOptions {
            jobs: options.jobs,
            keep_source: options.keep_source,
            force: options.force,
            compilation: false,
            cache_dir: None,
        };
        let actual = splitter::split_audio_with_cancel(
            &task.source,
            &task.chapters,
            &task.output,
            &settings,
            cancelled,
            on_progress,
        )
        .map_err(|error| error.to_string())?;
        if actual != task.output {
            return Err(format!(
                "The split cache points to {}, but this workflow needs {}.",
                actual.display(),
                task.output.display()
            ));
        }
        Ok(())
    }
}

fn event_record(event: WorkflowEvent) -> Value {
    let (name, data) = match event {
        WorkflowEvent::InputClassified(_) => ("message", json!({"message":"Reading local audio."})),
        WorkflowEvent::AcquisitionStarted => ("step_started", json!({"name":"read"})),
        WorkflowEvent::AcquisitionCompleted { files } => {
            ("step_finished", json!({"name":"read","files":files}))
        }
        WorkflowEvent::PlanReady { albums, singles } => (
            "message",
            json!({"message":format!("Found {albums} album(s) and {singles} single(s).")}),
        ),
        WorkflowEvent::SplitStarted(task) => (
            "progress_started",
            json!({"task_id":"local-split","description":format!("Splitting {}",task.source.display()),"total":task.chapters.len()}),
        ),
        WorkflowEvent::SplitProgress { progress, .. } => (
            "progress_advanced",
            json!({"task_id":"local-split","completed":progress.completed,"total":progress.total}),
        ),
        WorkflowEvent::SplitCompleted { .. } => (
            "progress_finished",
            json!({"task_id":"local-split","success":true}),
        ),
        WorkflowEvent::OrganizeStarted { .. } | WorkflowEvent::OrganizeCompleted { .. } => {
            ("message", json!({"message":"Organizing audio."}))
        }
        WorkflowEvent::Completed => (
            "message",
            json!({"message":"Local audio workflow complete."}),
        ),
    };
    json!({"event":name,"data":data})
}

#[cfg(test)]
mod tests {
    use super::{run, supported};
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn local_dry_run_reports_plan_without_changing_audio() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let audio = dir.path().join("album.flac");
        fs::write(&audio, b"audio")?;
        fs::write(dir.path().join("album.chapters.txt"), "0:00 First\n")?;
        let splits = dir.path().join("splits");
        let request =
            supported(&json!({"raw":audio,"splits":splits,"no_organize":true,"dry_run":true}))
                .ok_or("local request was not selected")??;
        let mut events = Vec::new();
        let result = run(request, &AtomicBool::new(false), &mut |event| {
            events.push(event)
        })?;
        assert_eq!(result["albums"], 1);
        assert!(events.iter().any(|event| event["event"] == "message"));
        assert!(audio.exists());
        assert!(!splits.exists());
        Ok(())
    }
}
