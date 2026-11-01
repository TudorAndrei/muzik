use crate::support::StepResult;
use crate::world::WorkflowWorld;
use cucumber::gherkin::Step;
use cucumber::{given, then};
use muzik_core::chapters::{Chapter, Sidecar};
use muzik_workflow::{ChapterReview, MetadataSource, QualityCheckedAudio};
use std::fmt::Write as _;
use std::fs;

fn one_chapter(title: String) -> Vec<Chapter> {
    vec![Chapter {
        index: 1,
        start: 0,
        end: None,
        title,
    }]
}

#[given(expr = "a local file {string}")]
fn a_local_file(world: &mut WorkflowWorld, name: String) -> StepResult {
    world.source = world.input_dir().join(name);
    fs::write(&world.source, b"audio")?;
    Ok(())
}

#[given(expr = "a local album {string} with {int} chapter(s)")]
fn a_local_album(world: &mut WorkflowWorld, name: String, chapters: u32) -> StepResult {
    world.source = world.input_dir().join(name);
    fs::write(&world.source, b"audio")?;
    let list = (0..chapters).fold(String::new(), |mut list, minute| {
        let _ = writeln!(list, "{minute}:00 Chapter {}", minute.saturating_add(1));
        list
    });
    fs::write(Sidecar::Chapters.path(&world.source), list)?;
    Ok(())
}

#[given(expr = "a local album {string} with the YouTube description:")]
fn a_local_album_with_description(
    world: &mut WorkflowWorld,
    name: String,
    step: &Step,
) -> StepResult {
    let description = step
        .docstring
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("the step has no description"))?;
    world.source = world.input_dir().join(name);
    fs::write(&world.source, b"audio")?;
    fs::write(
        Sidecar::Info.path(&world.source),
        serde_json::json!({ "description": description.trim() }).to_string(),
    )?;
    Ok(())
}

#[given("the metadata source is YouTube")]
const fn metadata_source_is_youtube(world: &mut WorkflowWorld) {
    world.options.metadata_source = MetadataSource::Youtube;
}

#[given("the reviewer rejects the chapters")]
fn reviewer_rejects(world: &mut WorkflowWorld) {
    world.options.review = true;
    world.operations.review_decision = Some(ChapterReview::Reject);
}

#[given(expr = "the reviewer replaces the chapters with one chapter {string}")]
fn reviewer_edits(world: &mut WorkflowWorld, title: String) {
    world.options.review = true;
    world.operations.review_decision = Some(ChapterReview::Edit(one_chapter(title)));
}

#[given("the split folder already has other files")]
fn split_folder_has_other_files(world: &mut WorkflowWorld) -> StepResult {
    let other_album = world.root().join("splits/other-album");
    fs::create_dir_all(&other_album)?;
    let files = [
        (
            world.root().join("unrelated.txt"),
            b"keep this file".to_vec(),
        ),
        (other_album.join("track.flac"), b"keep this track".to_vec()),
    ];
    for (path, content) in &files {
        fs::write(path, content)?;
    }
    world.protected.extend(files);
    Ok(())
}

#[given("force is on, with organize and metadata off")]
const fn force_without_organize_or_metadata(world: &mut WorkflowWorld) {
    world.options.force = true;
    world.options.no_organize = true;
    world.options.metadata_source = MetadataSource::None;
}

#[given("the real splitter is used")]
fn real_splitter(world: &mut WorkflowWorld) {
    world.operations.real_split_cache = Some(world.root().join("cache"));
}

#[given("the quality check replaces the source with an album folder")]
fn quality_check_replaces_source(world: &mut WorkflowWorld) -> StepResult {
    world.album = world.input_dir().join("replacement-album");
    fs::create_dir(&world.album)?;
    world.operations.quality_result = Some(QualityCheckedAudio {
        audio_files: Vec::new(),
        pre_split_dirs: vec![world.album.clone()],
    });
    Ok(())
}

#[then(expr = "the split folder {string} is organized")]
fn split_folder_is_organized(world: &mut WorkflowWorld, name: String) -> StepResult {
    let expected = world.root().join("splits").join(name);
    anyhow::ensure!(
        world.operations.organized == [expected],
        "organized {:?}",
        world.operations.organized
    );
    Ok(())
}

#[then(expr = "chapter {int} of the album is {string}")]
fn chapter_of_album_is(world: &mut WorkflowWorld, number: usize, title: String) -> StepResult {
    let chapter = world
        .result()?
        .plan
        .albums
        .first()
        .and_then(|album| album.chapters.get(number.saturating_sub(1)))
        .ok_or_else(|| anyhow::anyhow!("the album has no chapter {number}"))?;
    anyhow::ensure!(chapter.title == title, "chapter {number} is {chapter:?}");
    Ok(())
}

#[then("the reviewer saw the source")]
fn reviewer_saw_source(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.reviewed == [world.source.clone()],
        "reviewed {:?}",
        world.operations.reviewed
    );
    Ok(())
}

#[then(expr = "the album has only the chapter {string}")]
fn album_has_only_chapter(world: &mut WorkflowWorld, title: String) -> StepResult {
    let albums = &world.result()?.plan.albums;
    let chapters = albums.first().map(|album| &album.chapters);
    anyhow::ensure!(chapters == Some(&one_chapter(title)), "albums {albums:?}");
    Ok(())
}

#[then(expr = "the split used only the chapter {string}")]
fn split_used_only_chapter(world: &mut WorkflowWorld, title: String) -> StepResult {
    anyhow::ensure!(
        world.operations.split_chapters == [one_chapter(title)],
        "split chapters {:?}",
        world.operations.split_chapters
    );
    Ok(())
}

#[then("the other files did not change")]
fn other_files_did_not_change(world: &mut WorkflowWorld) -> StepResult {
    for (path, content) in &world.protected {
        anyhow::ensure!(&fs::read(path)? == content, "{} changed", path.display());
    }
    Ok(())
}

#[then("the source and its chapter file still exist")]
fn source_and_chapters_exist(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(world.source.exists(), "the source is gone");
    anyhow::ensure!(
        Sidecar::Chapters.path(&world.source).exists(),
        "the chapter file is gone"
    );
    Ok(())
}

#[then("the source still exists")]
fn source_still_exists(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(world.source.exists(), "the source is gone");
    Ok(())
}

#[then("the quality check ran once")]
fn quality_check_ran_once(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.quality_calls == 1,
        "the quality check ran {} times",
        world.operations.quality_calls
    );
    Ok(())
}

#[then("the replacement album folder is organized")]
fn replacement_is_organized(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.organized == [world.album.clone()],
        "organized {:?}",
        world.operations.organized
    );
    Ok(())
}

#[then("the replacement album folder is the split result")]
fn replacement_is_split_result(world: &mut WorkflowWorld) -> StepResult {
    let split_dirs = &world.result()?.split_dirs;
    anyhow::ensure!(
        split_dirs == std::slice::from_ref(&world.album),
        "split folders {split_dirs:?}"
    );
    Ok(())
}
