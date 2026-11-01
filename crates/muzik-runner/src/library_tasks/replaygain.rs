use super::{TaskContext, TaskReport};
use std::path::PathBuf;

#[expect(
    clippy::unnecessary_wraps,
    reason = "the ReplayGain backfill is not written yet"
)]
pub fn run(context: &TaskContext<'_>) -> crate::Result<TaskReport> {
    let _ = context;
    Ok(TaskReport::default())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the ReplayGain import step is not written yet"
)]
pub fn after_import(
    context: &TaskContext<'_>,
    album_dirs: &[PathBuf],
) -> crate::Result<TaskReport> {
    let _ = (context, album_dirs);
    Ok(TaskReport::default())
}
