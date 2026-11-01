use crate::support::StepResult;
use crate::world::{SyncWorld, audio};
use cucumber::{then, when};
use muzik_sync::{Action, Encoding};

fn expect_action(world: &SyncWorld, expected: &Action) -> StepResult {
    anyhow::ensure!(
        world.action.as_ref() == Some(expected),
        "the action is {:?}",
        world.action
    );
    Ok(())
}

#[when(expr = "muzik checks a {word} track at {int} Hz with {word} bit depth")]
fn checks_track(world: &mut SyncWorld, format: String, rate: u32, depth: String) -> StepResult {
    let bit_depth = if depth == "no" {
        None
    } else {
        Some(depth.parse()?)
    };
    world.action = Some(world.target.action(&audio(&format, rate, bit_depth)));
    Ok(())
}

#[then("the track is copied")]
fn track_is_copied(world: &mut SyncWorld) -> StepResult {
    expect_action(world, &Action::Copy)
}

#[then(expr = "the track is converted to MP3 at {int} kbps")]
fn track_is_mp3(world: &mut SyncWorld, kbps: u32) -> StepResult {
    expect_action(world, &Action::Convert(Encoding::Mp3 { kbps }))
}

#[then(expr = "the track is converted to Opus at {int} kbps")]
fn track_is_opus(world: &mut SyncWorld, kbps: u32) -> StepResult {
    expect_action(world, &Action::Convert(Encoding::Opus { kbps }))
}

#[then(expr = "the track is converted to FLAC at {int} Hz and {int} bit")]
fn track_is_flac(world: &mut SyncWorld, sample_rate: u32, bit_depth: u32) -> StepResult {
    expect_action(
        world,
        &Action::Convert(Encoding::Flac {
            sample_rate: Some(sample_rate),
            bit_depth: Some(bit_depth),
        }),
    )
}
