//! Run the workflow command as a job on the shared queue.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use muzik_core::paths::Paths;
use muzik_core::{AudioFallback, AudioSource, MetadataSource, PreferredAudio, QualityPolicy};
use muzik_runner::{Jobs, job_id};
use muzik_workflow::{WorkflowInput, classify_input};
use serde::Serialize;
use serde_json::Value;

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

#[derive(Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent job parameter"
)]
struct WorkflowParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    splits: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<String>,
    raw: String,
    review: bool,
    no_split: bool,
    no_organize: bool,
    tag_only: bool,
    dry_run: bool,
    jobs: usize,
    keep_source: bool,
    force: bool,
    compilation: bool,
    audio_source: AudioSource,
    metadata_source: MetadataSource,
    quality_policy: QualityPolicy,
    min_bitrate: u32,
    prefer: PreferredAudio,
    fallback: AudioFallback,
    interactive: bool,
}

fn params(args: &Workflow) -> anyhow::Result<Value> {
    let absolute =
        |path: &Path| std::path::absolute(path).map(|path| path.to_string_lossy().into_owned());
    let optional = |path: Option<&PathBuf>| path.map(|path| absolute(path)).transpose();
    let raw = if matches!(classify_input(&args.raw), WorkflowInput::Local(_)) {
        absolute(Path::new(&args.raw))?
    } else {
        args.raw.clone()
    };
    Ok(serde_json::to_value(WorkflowParams {
        output: optional(args.output.as_ref())?,
        splits: optional(args.splits.as_ref())?,
        config: optional(args.config.as_ref())?,
        raw,
        review: args.review,
        no_split: args.no_split,
        no_organize: args.no_organize,
        tag_only: args.tag_only,
        dry_run: args.dry_run,
        jobs: args.jobs,
        keep_source: args.keep_source,
        force: args.force,
        compilation: args.compilation,
        audio_source: args.audio_source,
        metadata_source: args.metadata_source,
        quality_policy: args.quality_policy,
        min_bitrate: args.min_bitrate,
        prefer: args.prefer,
        fallback: args.fallback,
        interactive: !args.no_interactive,
    })?)
}

#[cfg(test)]
mod tests {
    use super::{params, run_with};
    use crate::Workflow;
    use muzik_core::paths::Paths;
    use serde_json::json;
    use std::fs;

    #[test]
    fn params_keep_the_job_keys_and_choice_text() {
        let params = params(&Workflow {
            raw: "https://youtu.be/abcdefghijk".into(),
            output: Some("/music/out".into()),
            splits: None,
            review: true,
            no_split: false,
            no_organize: true,
            import: false,
            tag_only: false,
            dry_run: false,
            jobs: 2,
            config: None,
            keep_source: true,
            force: false,
            compilation: false,
            audio_source: muzik_core::AudioSource::Soulseek,
            metadata_source: muzik_core::MetadataSource::default(),
            quality_policy: muzik_core::QualityPolicy::Ask,
            min_bitrate: 256,
            prefer: muzik_core::PreferredAudio::Mp3_320,
            fallback: muzik_core::AudioFallback::default(),
            no_interactive: false,
            queue: false,
        })
        .unwrap();
        assert_eq!(
            params,
            json!({
                "output": "/music/out",
                "raw": "https://youtu.be/abcdefghijk",
                "review": true,
                "no_split": false,
                "no_organize": true,
                "tag_only": false,
                "dry_run": false,
                "jobs": 2,
                "keep_source": true,
                "force": false,
                "compilation": false,
                "audio_source": "soulseek",
                "metadata_source": muzik_core::MetadataSource::default().as_str(),
                "quality_policy": "ask",
                "min_bitrate": 256,
                "prefer": "mp3-320",
                "fallback": muzik_core::AudioFallback::default().as_str(),
                "interactive": true,
            })
        );
    }

    #[test]
    fn local_dry_run_keeps_source_and_does_not_create_split_files() {
        let directory = tempfile::tempdir().unwrap();
        let audio = directory.path().join("album.flac");
        fs::write(&audio, b"audio").unwrap();
        fs::write(directory.path().join("album.chapters.txt"), "0:00 First\n").unwrap();
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
        )
        .unwrap();
        assert!(audio.exists());
        assert!(!splits.exists());
    }
}
