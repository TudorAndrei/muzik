use muzik_core::app_config;
use muzik_core::paths::Paths;
use muzik_import::beets;
use muzik_library::Library;
use muzik_media::quality;
use muzik_store::db;
use muzik_sync::{self as sync, Action, Encoding, Options, Target};
use serde_json::json;

use crate::{SetSyncTarget, Sync};

pub fn set_target(args: &SetSyncTarget) -> Result<(), String> {
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

pub fn run(args: &Sync) -> Result<(), String> {
    let config = app_config::load(&app_config::path())?;
    let target = Target::load(&config, &args.target)?;
    if !target.path.is_dir() {
        return Err(format!(
            "{} does not exist; connect the device or create the folder first",
            target.path.display()
        ));
    }
    let (_, paths) = beets::load_paths(args.config.as_deref(), json!({}))?;
    let library = Library::open_read_only(&paths.library)
        .map_err(|error| format!("Could not open the music library: {error}"))?;
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
    let plan = &prepared.plan;
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
        plan.pending.len() - converts,
        converts,
        size(prepared.needed)
    );
    if prepared.delete_blocked {
        eprintln!(
            "not deleting old files: {} tracks could not be read, so muzik cannot tell which device files are old",
            plan.unreadable.len()
        );
    }
    if prepared.delete {
        println!(
            "{} files to delete ({})",
            prepared.stale.len(),
            size(prepared.freed)
        );
    }
    if let (false, Some(space)) = (prepared.fits(), prepared.space()) {
        return Err(format!(
            "not enough space: {} needed, {} available; select fewer tracks with --query{}",
            size(prepared.needed),
            size(space),
            if args.delete {
                ""
            } else {
                " or remove old files with --delete"
            }
        ));
    }
    if args.dry_run {
        for path in &prepared.stale {
            println!("delete\t{}", path.display());
        }
        for transfer in &plan.pending {
            println!(
                "{}\t{}",
                label(&transfer.action),
                transfer.destination.display()
            );
        }
        return Ok(());
    }
    let report = sync::apply(&prepared, &target, connection, args.jobs, &|done| {
        let name = done
            .transfer
            .destination
            .strip_prefix(&target.path)
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
    })?;
    if report.failed > 0 {
        return Err(format!(
            "{} of {} files failed",
            report.failed, report.written
        ));
    }
    println!("Sync complete: {} files written", report.written);
    Ok(())
}

fn label(action: &Action) -> String {
    match action {
        Action::Copy => "copy".into(),
        Action::Convert(Encoding::Mp3 { kbps }) => format!("mp3 {kbps}k"),
        Action::Convert(Encoding::Opus { kbps }) => format!("opus {kbps}k"),
        Action::Convert(Encoding::Flac { .. }) => "flac".into(),
    }
}

fn size(bytes: u64) -> String {
    let megabytes = bytes / 1_000_000;
    if megabytes >= 1_000 {
        format!("{}.{} GB", megabytes / 1_000, megabytes % 1_000 / 100)
    } else {
        format!("{megabytes} MB")
    }
}
