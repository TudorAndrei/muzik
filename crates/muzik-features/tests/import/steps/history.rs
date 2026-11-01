use crate::support::StepResult;
use crate::world::ImportWorld;
use cucumber::{given, then};
use muzik_import::history::IncrementalHistory;

fn has_incoming(world: &ImportWorld) -> anyhow::Result<bool> {
    let history = world
        .history
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("incremental import is off"))?;
    Ok(history.contains(&[world.incoming.canonicalize()?])?)
}

#[given("incremental import is on with an empty history")]
fn empty_history(world: &mut ImportWorld) -> StepResult {
    world.history = Some(IncrementalHistory::open_or_seed(&world.statefile, &[])?);
    Ok(())
}

#[given("the incremental history already has the incoming folder")]
fn seeded_history(world: &mut ImportWorld) -> StepResult {
    let seed = vec![world.incoming.canonicalize()?];
    world.history = Some(IncrementalHistory::open_or_seed(&world.statefile, &[seed])?);
    Ok(())
}

#[given(regex = r"^incremental skip later is (on|off)$")]
fn skip_later(world: &mut ImportWorld, setting: String) {
    world.skip_later = setting == "on";
}

#[then("no history update failed")]
fn no_history_failure(world: &mut ImportWorld) -> StepResult {
    let failed = &world.result()?.history_failed;
    anyhow::ensure!(
        failed.is_empty(),
        "these history updates failed: {failed:?}"
    );
    Ok(())
}

#[then("the history has the incoming folder")]
fn history_has_incoming(world: &mut ImportWorld) -> StepResult {
    anyhow::ensure!(has_incoming(world)?, "the history does not have the folder");
    Ok(())
}

#[then("the history does not have the incoming folder")]
fn history_lacks_incoming(world: &mut ImportWorld) -> StepResult {
    anyhow::ensure!(!has_incoming(world)?, "the history has the folder");
    Ok(())
}

#[then("the history file is written")]
fn history_file_written(world: &mut ImportWorld) -> StepResult {
    anyhow::ensure!(
        IncrementalHistory::path_for_statefile(&world.statefile).exists(),
        "the history file is missing"
    );
    Ok(())
}

#[then("the history file is not written")]
fn history_file_not_written(world: &mut ImportWorld) -> StepResult {
    anyhow::ensure!(
        !IncrementalHistory::path_for_statefile(&world.statefile).exists(),
        "the history file exists"
    );
    Ok(())
}
