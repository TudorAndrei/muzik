use crate::support::StepResult;
use crate::world::ImportWorld;
use cucumber::given;
use muzik_core::BeetsConfig;
use muzik_import::files::Placement;
use muzik_library::Library;

#[given(expr = "the beets replace rule changes {string} to {string}")]
fn replace_rule(world: &mut ImportWorld, from: String, to: String) -> StepResult {
    world.config = BeetsConfig::from_layers("", serde_json::json!({ "replace": { from: to } }))?;
    Ok(())
}

#[given(expr = "the beets config has the default path {string}")]
fn config_default_path(world: &mut ImportWorld, path: String) -> StepResult {
    world.config = BeetsConfig::from_layers(
        &format!("paths:\n  default: {path}\n"),
        serde_json::json!({}),
    )?;
    Ok(())
}

#[given(expr = "the import uses the default path {string}")]
fn default_path(world: &mut ImportWorld, path: String) {
    world.default_path = Some(path);
}

#[given(expr = "the import uses the singleton path {string}")]
fn singleton_path(world: &mut ImportWorld, path: String) {
    world.singleton_path = Some(path);
}

#[given("muzik moves files into the library")]
const fn moves_files(world: &mut ImportWorld) {
    world.placement = Some(Placement::Move);
}

#[given("muzik copies files into the library")]
const fn copies_files(world: &mut ImportWorld) {
    world.placement = Some(Placement::Copy);
}

#[given("dry run is on")]
const fn dry_run_on(world: &mut ImportWorld) {
    world.dry_run = true;
}

#[given("dry run is off")]
const fn dry_run_off(world: &mut ImportWorld) {
    world.dry_run = false;
}

#[given("autotag is off")]
const fn autotag_off(world: &mut ImportWorld) {
    world.autotag = false;
}

#[given("an empty beets library")]
fn empty_library(world: &mut ImportWorld) -> StepResult {
    world.library = Some(Library::empty()?);
    Ok(())
}
