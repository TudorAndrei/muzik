use crate::support::StepResult;
use crate::world::ImportWorld;
use cucumber::{then, when};
use muzik_import::plan::{AlbumPlan, ImportMode, ImportPlanner, PlanItem, PlanOptions};
use muzik_match::MatchConfig;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy)]
enum Mode {
    Album,
    Singletons,
}

fn plan(world: &mut ImportWorld, paths: &[PathBuf], mode: Mode) -> StepResult {
    world.open_library()?;
    let Some(library) = world.library.as_ref() else {
        anyhow::bail!("the library is not open");
    };
    let match_config = MatchConfig::from_beets(&world.config)?;
    let planner = ImportPlanner {
        provider: &world.catalog,
        library,
        match_config: &match_config,
        search_limit: 5,
    };
    let options = PlanOptions {
        autotag: world.autotag,
        history: world.history.clone(),
        incremental_skip_later: world.skip_later,
    };
    let cancelled = &world.cancelled;
    let outcome = match mode {
        Mode::Singletons => planner.plan_singletons(paths),
        Mode::Album if world.catalog.cancel_on_search.is_some() => planner
            .plan_with_options_and_cancel(paths, ImportMode::Album, options, &|| {
                cancelled.load(Ordering::SeqCst)
            }),
        Mode::Album if !world.autotag || world.history.is_some() => {
            planner.plan_with_options(paths, ImportMode::Album, options)
        }
        Mode::Album => planner.plan(paths),
    };
    world.plan = Some(outcome);
    Ok(())
}

fn album(world: &ImportWorld) -> anyhow::Result<&AlbumPlan> {
    world
        .current_plan()?
        .albums
        .first()
        .ok_or_else(|| anyhow::anyhow!("the plan has no albums"))
}

fn song(world: &ImportWorld) -> anyhow::Result<&PlanItem> {
    album(world)?
        .items
        .first()
        .ok_or_else(|| anyhow::anyhow!("the planned album has no songs"))
}

fn best_candidate(world: &ImportWorld) -> anyhow::Result<&muzik_import::plan::PlannedCandidate> {
    album(world)?
        .candidates
        .first()
        .ok_or_else(|| anyhow::anyhow!("the planned album has no candidates"))
}

#[when("muzik plans the import of the songs")]
fn plan_songs(world: &mut ImportWorld) -> StepResult {
    let paths = world.sources.clone();
    plan(world, &paths, Mode::Album)
}

#[when("muzik plans the import of the songs as singletons")]
fn plan_singletons(world: &mut ImportWorld) -> StepResult {
    let paths = world.sources.clone();
    plan(world, &paths, Mode::Singletons)
}

#[when("muzik plans the import of the incoming folder")]
fn plan_folder(world: &mut ImportWorld) -> StepResult {
    let paths = vec![world.incoming.clone()];
    plan(world, &paths, Mode::Album)
}

#[when("muzik plans the import of the incoming folder and its song")]
fn plan_folder_and_song(world: &mut ImportWorld) -> StepResult {
    let paths: Vec<PathBuf> = std::iter::once(world.incoming.clone())
        .chain(world.sources.iter().cloned())
        .collect();
    plan(world, &paths, Mode::Album)
}

#[when("muzik plans the import of the copy")]
fn plan_copy(world: &mut ImportWorld) -> StepResult {
    let copy = world
        .copy
        .clone()
        .ok_or_else(|| anyhow::anyhow!("there is no copy of the song"))?;
    plan(world, &[copy], Mode::Album)
}

#[then(expr = "the plan has {int} album(s)")]
fn plan_has_albums(world: &mut ImportWorld, count: usize) -> StepResult {
    let albums = world.current_plan()?.albums.len();
    anyhow::ensure!(albums == count, "the plan has {albums} albums");
    Ok(())
}

#[then("the plan is cancelled")]
fn plan_is_cancelled(world: &mut ImportWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.plan, Some(Err(muzik_import::Error::Cancelled))),
        "the plan is {:?}",
        world.plan
    );
    Ok(())
}

#[then(expr = "the plan skipped {int} folder(s) imported before")]
fn plan_skipped_folders(world: &mut ImportWorld, count: usize) -> StepResult {
    let skipped = world.current_plan()?.skipped_incremental;
    anyhow::ensure!(skipped == count, "the plan skipped {skipped} folders");
    Ok(())
}

#[then("MusicBrainz was not asked")]
fn musicbrainz_not_asked(world: &mut ImportWorld) -> StepResult {
    anyhow::ensure!(!world.catalog.requested(), "MusicBrainz was asked");
    Ok(())
}

#[then("the planned album comes from the incoming folder")]
fn album_from_incoming(world: &mut ImportWorld) -> StepResult {
    let source_dir = &album(world)?.source_dir;
    anyhow::ensure!(
        *source_dir == world.incoming.canonicalize()?,
        "the planned album comes from {}",
        source_dir.display()
    );
    Ok(())
}

#[then(expr = "the planned album has {int} song(s)")]
fn album_has_songs(world: &mut ImportWorld, count: usize) -> StepResult {
    let songs = album(world)?.items.len();
    anyhow::ensure!(songs == count, "the planned album has {songs} songs");
    Ok(())
}

#[then("the planned album is a singleton")]
fn album_is_singleton(world: &mut ImportWorld) -> StepResult {
    let kind = album(world)?.kind;
    anyhow::ensure!(
        kind == ImportMode::Singleton,
        "the planned album is {kind:?}"
    );
    Ok(())
}

#[then("the planned album is an album")]
fn album_is_album(world: &mut ImportWorld) -> StepResult {
    let kind = album(world)?.kind;
    anyhow::ensure!(kind == ImportMode::Album, "the planned album is {kind:?}");
    Ok(())
}

#[then(expr = "the planned album has {int} candidate(s)")]
fn album_has_candidates(world: &mut ImportWorld, count: usize) -> StepResult {
    let candidates = album(world)?.candidates.len();
    anyhow::ensure!(
        candidates == count,
        "the planned album has {candidates} candidates"
    );
    Ok(())
}

#[then("the planned album has no candidates")]
fn album_has_no_candidates(world: &mut ImportWorld) -> StepResult {
    album_has_candidates(world, 0)
}

#[then(expr = "the planned album has {int} duplicate(s)")]
fn album_has_duplicates(world: &mut ImportWorld, count: usize) -> StepResult {
    let duplicates = &album(world)?.duplicates;
    anyhow::ensure!(
        duplicates.len() == count,
        "the planned album has the duplicates {duplicates:?}"
    );
    Ok(())
}

#[then("the planned song is the incoming song")]
fn song_is_incoming(world: &mut ImportWorld) -> StepResult {
    let planned = &song(world)?.source;
    anyhow::ensure!(
        *planned == world.source()?.canonicalize()?,
        "the planned song is {}",
        planned.display()
    );
    Ok(())
}

#[then(expr = "the planned song comes from the source {string}")]
fn song_source(world: &mut ImportWorld, source_id: String) -> StepResult {
    let planned = song(world)?.source_id.as_deref();
    anyhow::ensure!(
        planned == Some(source_id.as_str()),
        "the planned song comes from {planned:?}"
    );
    Ok(())
}

#[then("the planned song has the length of its audio file")]
fn song_length(world: &mut ImportWorld) -> StepResult {
    let duration = muzik_tags::probe(world.source()?)?
        .duration_seconds
        .ok_or_else(|| anyhow::anyhow!("the audio file has no duration"))?;
    let length = song(world)?.match_item.length;
    anyhow::ensure!(
        length.to_bits() == duration.to_bits(),
        "the planned song is {length} seconds long, the file {duration}"
    );
    Ok(())
}

#[then(expr = "the planned song has the artist {string}")]
fn song_artist(world: &mut ImportWorld, artist: String) -> StepResult {
    let planned = &song(world)?.match_item.artist;
    anyhow::ensure!(*planned == artist, "the planned song is by {planned:?}");
    Ok(())
}

#[then(expr = "the best candidate is the release {string}")]
fn best_candidate_is(world: &mut ImportWorld, id: String) -> StepResult {
    let release = &best_candidate(world)?.release.id.0;
    anyhow::ensure!(*release == id, "the best candidate is {release:?}");
    Ok(())
}

#[then("the best candidate pairs the song with the first track")]
fn best_candidate_pairs_first(world: &mut ImportWorld) -> StepResult {
    let pairs = &best_candidate(world)?.assignment.pairs;
    anyhow::ensure!(*pairs == [(0, 0)], "the best candidate pairs {pairs:?}");
    Ok(())
}

#[then(expr = "the best candidate pairs {int} songs with tracks")]
fn best_candidate_pairs(world: &mut ImportWorld, count: usize) -> StepResult {
    let pairs = best_candidate(world)?.assignment.pairs.len();
    anyhow::ensure!(pairs == count, "the best candidate pairs {pairs} songs");
    Ok(())
}
