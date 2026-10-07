use anyhow::{Context, anyhow, bail};
use bytesize::ByteSize;
use muzik_core::app_config;
use muzik_core::paths::Paths;
use muzik_import::beets;
use muzik_library::Library;
use muzik_media::quality;
use muzik_store::db;
use muzik_sync::{self as sync, Action, Done, Encoding, Options, Prepared, Shortfall, Target};
use serde_json::json;
use std::path::Path;

use crate::{SetSyncTarget, Sync};

pub fn set_target(args: &SetSyncTarget) -> anyhow::Result<()> {
    let target = Target {
        path: muzik_core::paths::expand_home(&args.path),
        preset: args.preset,
        bitrate: args.bitrate,
        covers: !args.no_covers,
    };
    target.save(&app_config::path(), &args.name)?;
    println!(
        "Sync target {}: {} ({})",
        args.name.trim(),
        target.path.display(),
        target.preset
    );
    Ok(())
}

pub fn run(args: &Sync) -> anyhow::Result<()> {
    let config = app_config::load(&app_config::path())?;
    let target = Target::load(&config, &args.target)?;
    if !target.path.is_dir() {
        return Err(missing(&target.path));
    }
    let (_, paths) = beets::load_paths(args.config.as_deref(), json!({}))?;
    let library =
        Library::open_read_only(&paths.library).context("Could not open the music library")?;
    let selection = sync::select(
        &library,
        &paths.directory,
        args.query.as_deref().unwrap_or(""),
        target.covers,
    )?;

    println!(
        "Checking {} tracks for {} ({})",
        selection.tracks.len(),
        target.path.display(),
        target.preset
    );
    let connection = db::open(&Paths::user().database())?;
    let prepared = sync::prepare(
        &target,
        &paths.directory,
        &selection,
        &connection,
        Options {
            delete: args.delete,
            jobs: args.jobs,
        },
        &quality::measure,
    )?;
    print_plan(&prepared);
    if let Some(shortfall) = prepared.shortfall() {
        return Err(no_space(shortfall, args.delete));
    }
    if args.dry_run {
        print_dry_run(&prepared);
        return Ok(());
    }
    let report = sync::apply(prepared, connection, &|done| {
        print_done(&done, &target.path);
    })
    .map_err(|error| match error {
        sync::Error::TargetMissing(path) => missing(&path),
        sync::Error::NoSpace(shortfall) => no_space(shortfall, args.delete),
        error => error.into(),
    })?;
    if report.failed > 0 {
        bail!("{} of {} files failed", report.failed, report.written);
    }
    if report.unrecorded > 0 {
        bail!(
            "{} of {} files were written, but muzik could not save their encoding; the next sync converts them again",
            report.unrecorded,
            report.written
        );
    }
    println!("Sync complete: {} files written", report.written);
    Ok(())
}

fn print_plan(prepared: &Prepared) {
    let plan = prepared.plan();
    for source in &plan.outside {
        eprintln!("skip (outside the library folder): {}", source.display());
    }
    for source in &plan.unreadable {
        eprintln!("skip (cannot read audio): {}", source.display());
    }
    for source in &plan.duplicates {
        eprintln!(
            "skip (another track has the same device file name): {}",
            source.display()
        );
    }
    let converts = plan
        .pending
        .iter()
        .filter(|transfer| transfer.action != Action::Copy)
        .count();
    println!(
        "{} up to date, {} to copy, {} to convert, about {} to write",
        plan.fresh,
        plan.pending.len().saturating_sub(converts),
        converts,
        ByteSize(prepared.needed())
    );
    if prepared.delete_blocked() {
        eprintln!(
            "not deleting old files: {} tracks could not be read, so muzik cannot tell which device files are old",
            plan.unreadable.len()
        );
    }
    if prepared.delete() {
        println!(
            "{} files to delete ({})",
            prepared.stale().len(),
            ByteSize(prepared.freed())
        );
    }
}

fn print_dry_run(prepared: &Prepared) {
    for path in prepared.stale() {
        println!("delete\t{}", path.display());
    }
    for transfer in &prepared.plan().pending {
        println!(
            "{}\t{}",
            label(&transfer.action),
            transfer.destination.display()
        );
    }
}

fn print_done(done: &Done<'_>, root: &Path) {
    let name = done
        .transfer
        .destination
        .strip_prefix(root)
        .unwrap_or(&done.transfer.destination)
        .display();
    let (index, total) = (done.index, done.total);
    match done.result {
        Ok(()) => {
            println!("[{index}/{total}] {}\t{name}", label(&done.transfer.action));
            if let Some(error) = &done.record_error {
                eprintln!("cannot record the encoding of {name}: {error}");
            }
        }
        Err(error) => eprintln!("[{index}/{total}] failed\t{name}: {error}"),
    }
}

fn missing(path: &Path) -> anyhow::Error {
    anyhow!(
        "{} does not exist; connect the device or create the folder first",
        path.display()
    )
}

fn no_space(shortfall: Shortfall, delete: bool) -> anyhow::Error {
    anyhow!(
        "not enough space: {} needed, {} available; select fewer tracks with --query{}",
        ByteSize(shortfall.needed),
        ByteSize(shortfall.space),
        if delete {
            ""
        } else {
            " or remove old files with --delete"
        }
    )
}

fn label(action: &Action) -> String {
    match action {
        Action::Copy => "copy".into(),
        Action::Convert(Encoding::Mp3 { kbps }) => format!("mp3 {kbps}k"),
        Action::Convert(Encoding::Opus { kbps }) => format!("opus {kbps}k"),
        Action::Convert(Encoding::Flac { .. }) => "flac".into(),
    }
}
