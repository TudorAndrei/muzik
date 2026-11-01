use crate::support::StepResult;
use crate::world::WorkflowWorld;
use cucumber::{given, then, when};
use muzik_workflow::{WorkflowInput, classify_input, find_audio_inputs};
use std::fs;
use std::os::unix::fs::symlink;

#[given("a Spotify CSV export")]
fn a_spotify_csv_export(world: &mut WorkflowWorld) -> StepResult {
    world.export = world.root().join("spotify.csv");
    fs::write(&world.export, "Track Name,Artist Name\n")?;
    Ok(())
}

#[when("the workflow classifies the export")]
fn workflow_classifies_the_export(world: &mut WorkflowWorld) {
    world.input = Some(classify_input(&world.export.to_string_lossy()));
}

#[when(expr = "the workflow classifies {string}")]
fn workflow_classifies(world: &mut WorkflowWorld, raw: String) {
    world.input = Some(classify_input(&raw));
}

#[then("the input is a Spotify export")]
fn input_is_spotify_export(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.input == Some(WorkflowInput::SpotifyExport(world.export.clone())),
        "the input is {:?}",
        world.input
    );
    Ok(())
}

#[then(expr = "the input is the YouTube playlist {string}")]
fn input_is_playlist(world: &mut WorkflowWorld, id: String) -> StepResult {
    anyhow::ensure!(
        matches!(&world.input, Some(WorkflowInput::YoutubePlaylist { playlist_id, .. }) if *playlist_id == id),
        "the input is {:?}",
        world.input
    );
    Ok(())
}

#[then(expr = "the input is the YouTube video {string}")]
fn input_is_video(world: &mut WorkflowWorld, id: String) -> StepResult {
    anyhow::ensure!(
        matches!(&world.input, Some(WorkflowInput::YoutubeVideo { video_id, .. }) if *video_id == id),
        "the input is {:?}",
        world.input
    );
    Ok(())
}

#[given("an album folder with one track, one text file, and a broken link")]
fn album_folder(world: &mut WorkflowWorld) -> StepResult {
    world.album = world.root().join("album");
    fs::create_dir(&world.album)?;
    world.source = world.album.join("track.flac");
    fs::write(&world.source, b"audio")?;
    fs::write(world.album.join("notes.txt"), b"notes")?;
    symlink(
        world.root().join("missing"),
        world.album.join("missing.flac"),
    )?;
    Ok(())
}

#[given("a link to the album folder")]
fn link_to_album(world: &mut WorkflowWorld) -> StepResult {
    symlink(&world.album, world.root().join("alias"))?;
    Ok(())
}

#[given("the album folder contains a link to itself")]
fn album_links_to_itself(world: &mut WorkflowWorld) -> StepResult {
    symlink(&world.album, world.album.join("loop"))?;
    Ok(())
}

#[when("the workflow finds the audio in the album folder, its link, and a missing path")]
fn find_audio_with_link(world: &mut WorkflowWorld) {
    world.discovered = Some(find_audio_inputs(&[
        world.album.clone(),
        world.root().join("alias"),
        world.root().join("missing"),
    ]));
}

#[when("the workflow finds the audio in the album folder")]
fn find_audio(world: &mut WorkflowWorld) {
    world.discovered = Some(find_audio_inputs(std::slice::from_ref(&world.album)));
}

#[then("it finds only the one track")]
fn finds_only_the_track(world: &mut WorkflowWorld) -> StepResult {
    let Some(Ok(files)) = &world.discovered else {
        anyhow::bail!("audio discovery gave {:?}", world.discovered);
    };
    let [file] = files.as_slice() else {
        anyhow::bail!("audio discovery found {files:?}");
    };
    anyhow::ensure!(
        fs::canonicalize(file)? == fs::canonicalize(&world.source)?,
        "audio discovery found {}",
        file.display()
    );
    Ok(())
}

#[then("audio discovery fails")]
fn audio_discovery_fails(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.discovered, Some(Err(_))),
        "audio discovery gave {:?}",
        world.discovered
    );
    Ok(())
}
