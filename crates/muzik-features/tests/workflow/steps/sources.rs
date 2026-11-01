use crate::support::StepResult;
use crate::world::WorkflowWorld;
use cucumber::{given, then};
use muzik_workflow::{AudioFallback, AudioSource};
use std::fs;
use std::sync::Arc;

#[given(expr = "the download folder has the audio of the video {string}")]
fn download_folder_has_video(world: &mut WorkflowWorld, video: String) -> StepResult {
    let downloads = world.root().join("downloads");
    fs::create_dir_all(&downloads)?;
    world.source = downloads.join(format!("Track [{video}].mp3"));
    fs::write(&world.source, b"audio")?;
    Ok(())
}

#[given("dry run is on")]
const fn dry_run_is_on(world: &mut WorkflowWorld) {
    world.options.dry_run = true;
}

#[given("the audio source is Soulseek")]
const fn audio_source_is_soulseek(world: &mut WorkflowWorld) {
    world.options.audio_source = AudioSource::Soulseek;
}

#[given("the audio source is Soulseek with a YouTube fallback")]
const fn audio_source_is_soulseek_with_fallback(world: &mut WorkflowWorld) {
    world.options.audio_source = AudioSource::Soulseek;
    world.options.fallback = AudioFallback::Youtube;
}

#[given("Soulseek has a result")]
fn soulseek_has_a_result(world: &mut WorkflowWorld) -> StepResult {
    world.source = world.input_dir().join("song.flac");
    fs::write(&world.source, b"audio")?;
    world.operations.soulseek_files = vec![world.source.clone()];
    Ok(())
}

#[given("YouTube has a download")]
fn youtube_has_a_download(world: &mut WorkflowWorld) -> StepResult {
    world.source = world.input_dir().join("song.flac");
    fs::write(&world.source, b"audio")?;
    world.operations.downloaded_files = vec![world.source.clone()];
    Ok(())
}

#[given("the workflow is cancelled during the Soulseek search")]
fn cancelled_during_soulseek(world: &mut WorkflowWorld) {
    world.operations.cancel_during_soulseek = Some(Arc::clone(&world.cancelled));
}

#[then("nothing is downloaded from YouTube")]
fn nothing_is_downloaded(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.downloads.is_empty(),
        "downloaded {:?}",
        world.operations.downloads
    );
    Ok(())
}

#[then("the downloaded audio is a single")]
fn downloaded_audio_is_a_single(world: &mut WorkflowWorld) -> StepResult {
    let singles = &world.result()?.plan.singles;
    anyhow::ensure!(
        singles == std::slice::from_ref(&world.source),
        "singles {singles:?}"
    );
    Ok(())
}

#[then("the downloaded audio is organized")]
fn downloaded_audio_is_organized(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.organized == [world.source.clone()],
        "organized {:?}",
        world.operations.organized
    );
    Ok(())
}

#[then("there are no organize targets")]
fn no_organize_targets(world: &mut WorkflowWorld) -> StepResult {
    let targets = &world.result()?.organize_targets;
    anyhow::ensure!(targets.is_empty(), "organize targets {targets:?}");
    Ok(())
}

#[then("the Soulseek result is the organize target")]
fn soulseek_result_is_the_target(world: &mut WorkflowWorld) -> StepResult {
    let targets = &world.result()?.organize_targets;
    anyhow::ensure!(
        targets == std::slice::from_ref(&world.source),
        "organize targets {targets:?}"
    );
    Ok(())
}

#[then(expr = "Soulseek was searched for {string}")]
fn soulseek_was_searched_for(world: &mut WorkflowWorld, query: String) -> StepResult {
    anyhow::ensure!(
        world.operations.soulseek_queries == [query],
        "Soulseek queries {:?}",
        world.operations.soulseek_queries
    );
    Ok(())
}

#[then(expr = "YouTube downloaded {string}")]
fn youtube_downloaded(world: &mut WorkflowWorld, url: String) -> StepResult {
    anyhow::ensure!(
        world.operations.downloads == [url],
        "downloaded {:?}",
        world.operations.downloads
    );
    Ok(())
}
