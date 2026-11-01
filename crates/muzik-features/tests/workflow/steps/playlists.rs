use crate::support::{StepResult, fixture};
use crate::world::WorkflowWorld;
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use muzik_workflow::AudioSource;
use muzik_workflow::playlist::{
    PlaylistRunResult, load_spotify_export, run_spotify_export, run_youtube_playlist,
};
use std::fs;

const REPEATED_TRACK_EXPORT: &str = r#"{"version":1,"source":"spotify","type":"playlist","id":"p1","title":"List","entries":[{"title":"One","artist":"A","source_id":"spotify:track:same","index":1},{"title":"Two","artist":"B","source_id":"spotify:track:same","index":2}]}"#;

fn list(text: &str) -> Vec<String> {
    text.split(", ").map(str::to_owned).collect()
}

fn run_playlist(world: &mut WorkflowWorld, id: &str) -> anyhow::Result<PlaylistRunResult> {
    let request = world.request("");
    Ok(run_youtube_playlist(
        &request,
        &world.options,
        &mut world.operations,
        &world.cancelled,
        id,
        &format!("https://youtube.com/playlist?list={id}"),
        &mut |_| {},
    )?)
}

fn run_export(world: &mut WorkflowWorld) -> anyhow::Result<PlaylistRunResult> {
    let request = world.request("");
    Ok(run_spotify_export(
        &request,
        &world.options,
        &mut world.operations,
        &world.cancelled,
        &world.export,
        &mut |_| {},
    )?)
}

#[given(expr = "the YouTube playlist has the videos {string}")]
fn playlist_has_videos(world: &mut WorkflowWorld, videos: String) {
    world.operations.playlist_ids = list(&videos);
    world.operations.soulseek_ready = true;
}

#[given(expr = "the video {string} fails to download")]
fn video_fails(world: &mut WorkflowWorld, video: String) {
    world.operations.failing_video = Some(video);
}

#[given(expr = "an earlier run of the YouTube playlist {string} failed at the video {string}")]
fn earlier_playlist_run_failed(world: &mut WorkflowWorld, id: String, video: String) -> StepResult {
    world.operations.failing_video = Some(video);
    run_playlist(world, &id)?;
    world.operations.failing_video = None;
    world.operations.downloads.clear();
    Ok(())
}

#[when(expr = "the workflow runs the YouTube playlist {string}")]
fn workflow_runs_playlist(world: &mut WorkflowWorld, id: String) -> StepResult {
    world.playlist = Some(run_playlist(world, &id)?);
    Ok(())
}

#[then(expr = "the playlist items are {string}")]
fn playlist_items_are(world: &mut WorkflowWorld, states: String) -> StepResult {
    let playlist = world
        .playlist
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no playlist ran"))?;
    let actual = playlist
        .items
        .iter()
        .map(|item| if item.completed { "complete" } else { "failed" })
        .collect::<Vec<_>>()
        .join(", ");
    anyhow::ensure!(actual == states, "the playlist items are {actual}");
    Ok(())
}

#[then(expr = "YouTube downloaded the videos {string}")]
fn youtube_downloaded_videos(world: &mut WorkflowWorld, videos: String) -> StepResult {
    let actual = world.operations.downloaded_videos().join(", ");
    anyhow::ensure!(actual == videos, "YouTube downloaded {actual}");
    Ok(())
}

#[then("the playlist videos were listed")]
fn playlist_was_listed(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.playlist_listed,
        "the playlist was not listed"
    );
    Ok(())
}

#[given("a Spotify JSON export with the same track at positions 1 and 2")]
fn spotify_export_with_repeated_track(world: &mut WorkflowWorld) -> StepResult {
    world.export = world.root().join("spotify.json");
    fs::write(&world.export, REPEATED_TRACK_EXPORT)?;
    Ok(())
}

#[given("Soulseek has the track audio")]
fn soulseek_has_track_audio(world: &mut WorkflowWorld) -> StepResult {
    world.source = world.root().join("song.mp3");
    fs::copy(
        fixture("muzik-tags/tests/fixtures/blank.mp3"),
        &world.source,
    )?;
    world.operations.spotify_files = vec![world.source.clone()];
    world.operations.soulseek_ready = true;
    world.options.audio_source = AudioSource::Soulseek;
    Ok(())
}

#[given("an earlier run of the Spotify export is complete")]
fn earlier_export_run(world: &mut WorkflowWorld) -> StepResult {
    run_export(world)?;
    world.operations.spotify_tracks.clear();
    Ok(())
}

#[when("the workflow runs the Spotify export")]
fn workflow_runs_export(world: &mut WorkflowWorld) -> StepResult {
    world.playlist = Some(run_export(world)?);
    Ok(())
}

#[then(expr = "the playlist item ids are {string}")]
fn playlist_item_ids(world: &mut WorkflowWorld, ids: String) -> StepResult {
    let playlist = world
        .playlist
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no playlist ran"))?;
    let actual = playlist
        .items
        .iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    anyhow::ensure!(actual == ids, "the playlist item ids are {actual}");
    Ok(())
}

#[then(expr = "the track audio has the title {string} and the artist {string}")]
fn track_audio_tags(world: &mut WorkflowWorld, title: String, artist: String) -> StepResult {
    let tags = muzik_tags::read(&world.source, &[])?;
    let field = |name: &str| tags.fields.get(name).map(String::as_str);
    anyhow::ensure!(
        field("title") == Some(title.as_str()) && field("artist") == Some(artist.as_str()),
        "the track tags are {:?}",
        tags.fields
    );
    Ok(())
}

#[then("no Spotify track was acquired")]
fn no_spotify_track_acquired(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.spotify_tracks.is_empty(),
        "acquired {:?}",
        world.operations.spotify_tracks
    );
    Ok(())
}

#[given(expr = "a Spotify export {string} with the content:")]
fn spotify_export_with_content(world: &mut WorkflowWorld, name: String, step: &Step) -> StepResult {
    let content = step
        .docstring
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("the step has no content"))?;
    world.export = world.root().join(name);
    fs::write(&world.export, content.trim_start())?;
    Ok(())
}

#[when("the Spotify export is read")]
fn spotify_export_is_read(world: &mut WorkflowWorld) {
    world.spotify = Some(load_spotify_export(&world.export));
}

#[then(expr = "the first track has the title {string}")]
fn first_track_title(world: &mut WorkflowWorld, title: String) -> StepResult {
    let Some(Ok(playlist)) = &world.spotify else {
        anyhow::bail!("reading the export gave {:?}", world.spotify);
    };
    let first = playlist.tracks.first().map(|track| track.title.as_str());
    anyhow::ensure!(
        first == Some(title.as_str()),
        "the first track is {first:?}"
    );
    Ok(())
}

#[then("the Spotify export is rejected")]
fn spotify_export_rejected(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.spotify, Some(Err(_))),
        "reading the export gave {:?}",
        world.spotify
    );
    Ok(())
}
