//! Run library maintenance tasks as jobs on the shared queue.

use std::sync::Arc;

use muzik_core::paths::Paths;
use muzik_runner::library_tasks::{LibraryRequest, LibraryTask};
use muzik_runner::{Jobs, job_id};

use crate::LibraryTaskArgs;

pub fn run(task: LibraryTask, args: &LibraryTaskArgs) -> anyhow::Result<()> {
    run_with(&Paths::user(), task, args)
}

fn run_with(paths: &Paths, task: LibraryTask, args: &LibraryTaskArgs) -> anyhow::Result<()> {
    let jobs = Arc::new(Jobs::open(paths)?);
    let id = jobs.library(&request(task, args)?)?;
    println!("Queued {} as {}.", task.title(), job_id(id));
    if args.queue_only {
        return Ok(());
    }
    crate::jobs::drain(&jobs)
}

fn request(task: LibraryTask, args: &LibraryTaskArgs) -> anyhow::Result<LibraryRequest> {
    Ok(LibraryRequest {
        task,
        dry_run: args.dry_run,
        config: args
            .config
            .as_deref()
            .map(std::path::absolute)
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use super::{request, run_with};
    use crate::LibraryTaskArgs;
    use muzik_core::paths::Paths;
    use muzik_runner::library_tasks::LibraryTask;
    use std::fs;

    #[test]
    fn a_library_command_queues_its_task_and_runs_it() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.yaml");
        fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\n",
                directory.path().join("Music").display(),
                directory.path().join("library.db").display()
            ),
        )
        .unwrap();
        let args = LibraryTaskArgs {
            dry_run: true,
            config: Some(config.clone()),
            queue_only: false,
        };
        let queued = request(LibraryTask::Mbids, &args).unwrap();
        assert!(queued.dry_run);
        assert_eq!(queued.config, Some(config));
        run_with(
            &Paths::under(&directory.path().join("state")),
            LibraryTask::Mbids,
            &args,
        )
        .unwrap();
    }
}
