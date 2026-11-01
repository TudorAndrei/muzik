use super::tables::{cell, records};
use crate::catalog::FakeCatalog;
use crate::support::{self, StepResult};
use crate::world::ImportWorld;
use cucumber::gherkin::Step;
use cucumber::given;
use muzik_core::{RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
use muzik_metadata::ReleaseArtistIds;
use std::collections::BTreeMap;
use std::sync::Arc;

#[given(expr = "MusicBrainz has the release {string} by {string} from {int} with the id {string}:")]
fn release(
    world: &mut ImportWorld,
    title: String,
    artist: String,
    year: i32,
    id: String,
    step: &Step,
) -> StepResult {
    let tracks = records(step)?
        .iter()
        .map(|record| {
            let index = cell(record, "track")?.parse()?;
            Ok(TrackCandidate {
                recording_id: Some(RecordingId(cell(record, "recording")?.to_owned())),
                release_track_id: Some(cell(record, "release track")?.to_owned()),
                title: cell(record, "title")?.to_owned(),
                artist: cell(record, "artist")?.to_owned(),
                length_seconds: record
                    .get("length")
                    .map(|length| length.parse())
                    .transpose()?,
                index,
                medium: 1,
                medium_index: index,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    world.catalog.releases.push(ReleaseCandidate {
        id: ReleaseId(id),
        title,
        artist,
        tracks,
        release_group_id: None,
        year: Some(year),
        country: None,
        media: None,
        label: None,
        catalog_number: None,
        disambiguation: None,
        is_various_artists: false,
    });
    Ok(())
}

#[given(expr = "the release is in the release group {string} on {string}")]
fn release_group(world: &mut ImportWorld, group: String, media: String) -> StepResult {
    let release = world.catalog.last_release_mut()?;
    release.release_group_id = Some(group);
    release.media = Some(media);
    Ok(())
}

#[given(expr = "MusicBrainz has the recorded release {string} by {string}")]
fn recorded_release(world: &mut ImportWorld, title: String, artist: String) -> StepResult {
    let recorded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        support::fixture("muzik-metadata/tests/fixtures/release.json"),
    )?)?;
    let expected = recorded
        .get("expected")
        .ok_or_else(|| anyhow::anyhow!("the recording has no expected release"))?;
    let release: ReleaseCandidate = serde_json::from_value(expected.clone())?;
    anyhow::ensure!(
        release.title == title && release.artist == artist,
        "the recorded release is {:?} by {:?}",
        release.title,
        release.artist
    );
    world.catalog.releases.push(release);
    Ok(())
}

#[given(expr = "MusicBrainz has the recording {string} called {string} by {string}")]
fn recording(world: &mut ImportWorld, id: String, title: String, artist: String) {
    world.catalog.recordings.push(TrackCandidate {
        recording_id: Some(RecordingId(id)),
        release_track_id: None,
        title,
        artist,
        length_seconds: None,
        index: 0,
        medium: 0,
        medium_index: 0,
    });
}

#[given(
    expr = "MusicBrainz credits the artist {string} for the album and for the release track {string}"
)]
fn artist_ids(world: &mut ImportWorld, artist: String, track: String) {
    world.catalog.artist_ids = Some(ReleaseArtistIds {
        album_artist: Some(artist.clone()),
        tracks: BTreeMap::from([(track, artist)]),
    });
}

#[given("MusicBrainz is not reachable")]
fn not_reachable(world: &mut ImportWorld) {
    world.catalog = FakeCatalog::default();
}

#[given("the import is cancelled during the MusicBrainz search")]
fn cancelled_during_search(world: &mut ImportWorld) {
    world.catalog.cancel_on_search = Some(Arc::clone(&world.cancelled));
}
