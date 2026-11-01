use crate::support::StepResult;
use crate::world::{Applied, Preparation, SyncWorld, probe};
use cucumber::{given, then, when};
use muzik_store::Connection;
use muzik_sync::{Error, Options, Selection};
use std::sync::atomic::{AtomicUsize, Ordering};

#[given("muzik cannot save sync records")]
const fn cannot_save_records(world: &mut SyncWorld) {
    world.records_fail = true;
}

#[when(regex = r"^muzik prepares the sync with delete (on|off)$")]
fn prepares_sync(world: &mut SyncWorld, delete: String) -> StepResult {
    let selection = Selection {
        tracks: world.tracks.clone(),
        covers: world.covers.clone(),
    };
    let unreadable = &world.unreadable;
    let prepared = muzik_sync::prepare(
        &world.target,
        &world.library(),
        &selection,
        world.records()?,
        Options {
            delete: delete == "on",
            jobs: 1,
        },
        &|path| Ok(probe(unreadable, path)),
    )?;
    world.destinations = prepared
        .plan()
        .pending
        .iter()
        .map(|transfer| transfer.destination.clone())
        .collect();
    world.prepared = Some(Preparation(prepared));
    Ok(())
}

#[when("muzik applies the sync")]
fn applies_sync(world: &mut SyncWorld) -> StepResult {
    let Some(Preparation(prepared)) = world.prepared.take() else {
        anyhow::bail!("muzik did not prepare the sync");
    };
    let connection = if world.records_fail {
        Connection::open_in_memory()?
    } else {
        world
            .records
            .take()
            .ok_or_else(|| anyhow::anyhow!("the sync records are closed"))?
    };
    let calls = AtomicUsize::new(0);
    let transfer_failures = AtomicUsize::new(0);
    let record_failures = AtomicUsize::new(0);
    let result = muzik_sync::apply(prepared, connection, &|done| {
        calls.fetch_add(1, Ordering::Relaxed);
        if done.result.is_err() {
            transfer_failures.fetch_add(1, Ordering::Relaxed);
        }
        if done.record_error.is_some() {
            record_failures.fetch_add(1, Ordering::Relaxed);
        }
    });
    world.applied = Some(result.map(|report| Applied {
        total: report.total,
        failed: report.failed,
        unrecorded: report.unrecorded,
        calls: calls.into_inner(),
        transfer_failures: transfer_failures.into_inner(),
        record_failures: record_failures.into_inner(),
    }));
    Ok(())
}

#[then("the delete is blocked")]
fn delete_is_blocked(world: &mut SyncWorld) -> StepResult {
    let prepared = world.preparation()?;
    anyhow::ensure!(
        !prepared.delete() && prepared.delete_blocked(),
        "delete is {} and blocked is {}",
        prepared.delete(),
        prepared.delete_blocked()
    );
    Ok(())
}

#[then("the delete is allowed")]
fn delete_is_allowed(world: &mut SyncWorld) -> StepResult {
    let prepared = world.preparation()?;
    anyhow::ensure!(
        prepared.delete() && !prepared.delete_blocked(),
        "delete is {} and blocked is {}",
        prepared.delete(),
        prepared.delete_blocked()
    );
    Ok(())
}

#[then("no old files are set for removal")]
fn no_old_files_for_removal(world: &mut SyncWorld) -> StepResult {
    let stale = world.preparation()?.stale();
    anyhow::ensure!(stale.is_empty(), "the old files are {stale:?}");
    Ok(())
}

#[then(expr = "the old file {string} is set for removal")]
fn old_file_for_removal(world: &mut SyncWorld, name: String) -> StepResult {
    let expected = world.card().join(name);
    let stale = world.preparation()?.stale();
    anyhow::ensure!(
        stale == std::slice::from_ref(&expected),
        "the old files are {stale:?}"
    );
    Ok(())
}

#[then(expr = "the delete frees {int} bytes")]
fn delete_frees(world: &mut SyncWorld, bytes: u64) -> StepResult {
    let freed = world.preparation()?.freed();
    anyhow::ensure!(freed == bytes, "the delete frees {freed} bytes");
    Ok(())
}

#[then(expr = "the {int} planned files are on the device")]
fn planned_files_on_device(world: &mut SyncWorld, count: usize) -> StepResult {
    anyhow::ensure!(
        world.destinations.len() == count,
        "the plan has {} files",
        world.destinations.len()
    );
    for destination in &world.destinations {
        anyhow::ensure!(
            destination.is_file(),
            "{} is missing",
            destination.display()
        );
    }
    Ok(())
}

#[then(expr = "the sync reports {int} transfers and {int} failures")]
fn sync_reports(world: &mut SyncWorld, total: usize, failed: usize) -> StepResult {
    let report = world.report()?;
    anyhow::ensure!(
        report.total == total && report.total == world.destinations.len(),
        "the sync reports {} transfers for {} planned files",
        report.total,
        world.destinations.len()
    );
    anyhow::ensure!(
        report.failed == failed && report.transfer_failures == failed,
        "the sync reports {} failures and {} failed transfers",
        report.failed,
        report.transfer_failures
    );
    Ok(())
}

#[then("each transfer is reported and recorded")]
fn each_transfer_recorded(world: &mut SyncWorld) -> StepResult {
    let report = world.report()?;
    anyhow::ensure!(
        report.calls == world.destinations.len(),
        "{} of {} transfers are reported",
        report.calls,
        world.destinations.len()
    );
    anyhow::ensure!(
        report.transfer_failures == 0 && report.record_failures == 0,
        "{report:?}"
    );
    Ok(())
}

#[then(expr = "{int} transfers are not recorded")]
fn transfers_not_recorded(world: &mut SyncWorld, count: usize) -> StepResult {
    let report = world.report()?;
    anyhow::ensure!(
        report.unrecorded == count
            && report.record_failures == count
            && count == world.destinations.len(),
        "{report:?}"
    );
    Ok(())
}

#[then("the sync fails because the device is missing")]
fn sync_fails_device_missing(world: &mut SyncWorld) -> StepResult {
    let card = world.card();
    anyhow::ensure!(
        matches!(&world.applied, Some(Err(Error::TargetMissing(path))) if *path == card),
        "the sync result is {:?}",
        world.applied
    );
    Ok(())
}
