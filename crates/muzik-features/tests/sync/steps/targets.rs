use crate::support::StepResult;
use crate::world::SyncWorld;
use cucumber::{given, then, when};
use muzik_core::SyncPreset;
use muzik_sync::Target;
use std::fs;

fn save(world: &mut SyncWorld, name: &str, preset: &str, bitrate: u32) -> StepResult {
    let target = Target {
        path: world.root().join(name),
        preset: preset.parse::<SyncPreset>()?,
        bitrate: Some(bitrate),
        covers: false,
    };
    world.save_result = Some(target.save(&world.config(), name));
    world.saved = Some(target);
    Ok(())
}

#[given(expr = "a(n) {string} sync target")]
fn sync_target(world: &mut SyncWorld, preset: String) -> StepResult {
    world.target.preset = preset.parse()?;
    Ok(())
}

#[given(expr = "a(n) {string} sync target with a bitrate of {int} kbps")]
fn sync_target_with_bitrate(world: &mut SyncWorld, preset: String, bitrate: u32) -> StepResult {
    world.target.preset = preset.parse()?;
    world.target.bitrate = Some(bitrate);
    Ok(())
}

#[given(expr = "the target bitrate is {int} kbps")]
const fn target_bitrate(world: &mut SyncWorld, bitrate: u32) {
    world.target.bitrate = Some(bitrate);
}

#[given(expr = "a config file with the Spotify client id {string}")]
fn config_file(world: &mut SyncWorld, id: String) -> StepResult {
    fs::write(world.config(), format!("spotify:\n  client_id: {id}\n"))?;
    Ok(())
}

#[given(
    expr = "muzik saved the {string} target for the {string} preset at {int} kbps without covers"
)]
fn saved_target(world: &mut SyncWorld, name: String, preset: String, bitrate: u32) -> StepResult {
    save(world, &name, &preset, bitrate)?;
    if let Some(Err(error)) = world.save_result.take() {
        anyhow::bail!("the save failed: {error}");
    }
    Ok(())
}

#[when(
    expr = "muzik saves the {string} target for the {string} preset at {int} kbps without covers"
)]
fn saves_target(world: &mut SyncWorld, name: String, preset: String, bitrate: u32) -> StepResult {
    save(world, &name, &preset, bitrate)
}

#[when(expr = "muzik loads the {string} target")]
fn loads_target(world: &mut SyncWorld, name: String) -> StepResult {
    let config = muzik_core::app_config::load(&world.config())?;
    world.loaded = Some(Target::load(&config, &name));
    Ok(())
}

#[then(expr = "the {string} target loads with the same settings")]
fn target_loads(world: &mut SyncWorld, name: String) -> StepResult {
    if let Some(Err(error)) = &world.save_result {
        anyhow::bail!("the save failed: {error}");
    }
    let config = muzik_core::app_config::load(&world.config())?;
    let loaded = Target::load(&config, &name)?;
    anyhow::ensure!(
        world.saved.as_ref() == Some(&loaded),
        "saved {:?} but loaded {loaded:?}",
        world.saved
    );
    Ok(())
}

#[then(expr = "the Spotify client id is still {string}")]
fn spotify_client_id(world: &mut SyncWorld, id: String) -> StepResult {
    let config = muzik_core::app_config::load(&world.config())?;
    let found = config
        .get("spotify")
        .and_then(|spotify| spotify.get("client_id"))
        .and_then(serde_json::Value::as_str);
    anyhow::ensure!(found == Some(id.as_str()), "the client id is {found:?}");
    Ok(())
}

#[then(expr = "the load fails with a message that names {string}")]
fn load_fails(world: &mut SyncWorld, name: String) -> StepResult {
    match &world.loaded {
        Some(Err(error)) => {
            anyhow::ensure!(error.to_string().contains(&name), "the message is {error}");
        }
        other => anyhow::bail!("the load result is {other:?}"),
    }
    Ok(())
}

#[then("the save fails")]
fn save_fails(world: &mut SyncWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.save_result, Some(Err(_))),
        "the save result is {:?}",
        world.save_result
    );
    Ok(())
}
