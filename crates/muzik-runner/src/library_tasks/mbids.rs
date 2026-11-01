use super::{TaskContext, TaskReport};

#[expect(
    clippy::unnecessary_wraps,
    reason = "the MusicBrainz ID backfill is not written yet"
)]
pub fn run(context: &TaskContext<'_>) -> crate::Result<TaskReport> {
    let _ = context;
    Ok(TaskReport::default())
}
