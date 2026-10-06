//! Run the workflow command as a job on the shared queue.

use std::path::Path;
use std::sync::Arc;

use muzik_core::paths::Paths;
use muzik_runner::{Jobs, job_id};
use muzik_workflow::{WorkflowInput, classify_input};
use serde_json::{Map, Value, json};

use crate::Workflow;

pub fn run(args: &Workflow) -> anyhow::Result<()> {
    run_with(&Paths::user(), args)
}

fn run_with(paths: &Paths, args: &Workflow) -> anyhow::Result<()> {
    let jobs = Arc::new(Jobs::open(paths)?);
    let id = jobs.workflow(&params(args)?)?;
    println!("Queued {} as {}.", args.raw, job_id(id));
    crate::jobs::drain(&jobs)
}

fn params(args: &Workflow) -> anyhow::Result<Value> {
    let absolute =
        |path: &Path| std::path::absolute(path).map(|path| path.to_string_lossy().into_owned());
    let raw = if matches!(classify_input(&args.raw), WorkflowInput::Local(_)) {
        absolute(Path::new(&args.raw))?
    } else {
        args.raw.clone()
    };
    let mut params = Map::new();
    for (key, path) in [
        ("output", &args.output),
        ("splits", &args.splits),
        ("config", &args.config),
    ] {
        if let Some(path) = path {
            params.insert(key.into(), json!(absolute(path)?));
        }
    }
    params.extend(
        json!({
            "raw": raw,
            "review": args.review,
            "no_split": args.no_split,
            "no_organize": args.no_organize,
            "tag_only": args.tag_only,
            "dry_run": args.dry_run,
            "jobs": args.jobs,
            "keep_source": args.keep_source,
            "force": args.force,
            "compilation": args.compilation,
            "audio_source": args.audio_source,
            "metadata_source": args.metadata_source,
            "quality_policy": args.quality_policy,
            "min_bitrate": args.min_bitrate,
            "prefer": args.prefer.to_string(),
            "fallback": args.fallback,
            "interactive": !args.no_interactive,
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    );
    Ok(Value::Object(params))
}

#[cfg(test)]
mod tests {
    use super::run_with;
    use crate::Workflow;
    use muzik_core::paths::Paths;
    use std::fs;

    #[test]
    fn local_dry_run_keeps_source_and_does_not_create_split_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let audio = directory.path().join("album.flac");
        fs::write(&audio, b"audio")?;
        fs::write(directory.path().join("album.chapters.txt"), "0:00 First\n")?;
        let splits = directory.path().join("splits");
        run_with(
            &Paths::under(&directory.path().join("state")),
            &Workflow {
                raw: audio.to_string_lossy().into_owned(),
                output: None,
                splits: Some(splits.clone()),
                review: false,
                no_split: false,
                no_organize: false,
                import: false,
                tag_only: false,
                dry_run: true,
                jobs: 0,
                config: None,
                keep_source: false,
                force: false,
                compilation: false,
                audio_source: muzik_core::AudioSource::default(),
                metadata_source: muzik_core::MetadataSource::default(),
                quality_policy: muzik_core::QualityPolicy::default(),
                min_bitrate: 256,
                prefer: muzik_core::PreferredAudio::default(),
                fallback: muzik_core::AudioFallback::default(),
                no_interactive: true,
                queue: false,
            },
        )?;
        assert!(audio.exists());
        assert!(!splits.exists());
        Ok(())
    }
}
