use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use muzik_core::sync::{self, Action, Encoding, Target, Transfer};
use muzik_core::{app_config, quality};
use muzik_import::beets;
use muzik_library::{Item, Library};
use serde_json::json;

use crate::soulseek::stored_path;
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
    let items = library
        .query_items(args.query.as_deref().unwrap_or(""))
        .map_err(|error| error.to_string())?;
    let tracks: Vec<PathBuf> = items
        .iter()
        .filter_map(|item| item.field("path").and_then(stored_path))
        .map(|path| absolute(&paths.directory, path))
        .collect();
    let album_ids: BTreeSet<i64> = if target.covers {
        items.iter().filter_map(Item::album_id).collect()
    } else {
        BTreeSet::new()
    };
    let mut covers = Vec::new();
    for id in album_ids {
        let album = library.album(id).map_err(|error| error.to_string())?;
        if let Some(path) = album
            .as_ref()
            .and_then(|album| album.field("artpath"))
            .and_then(stored_path)
            .map(|path| absolute(&paths.directory, path))
            .filter(|path| path.is_file())
        {
            covers.push(path);
        }
    }

    println!(
        "Checking {} tracks for {} ({})",
        tracks.len(),
        target.path.display(),
        target.preset
    );
    let plan = sync::plan(
        &target,
        &paths.directory,
        &tracks,
        &covers,
        args.jobs,
        &quality::measure,
    );
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
    let needed = plan.bytes_needed();
    println!(
        "{} up to date, {} to copy, {} to convert, about {} to write",
        plan.fresh,
        plan.pending.len() - converts,
        converts,
        size(needed)
    );
    let stale = if args.delete {
        sync::stale_files(&target.path, &plan.planned).map_err(|error| error.to_string())?
    } else {
        Vec::new()
    };
    let freed: u64 = stale
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|meta| meta.len())
        .sum();
    if args.delete {
        println!("{} files to delete ({})", stale.len(), size(freed));
    }
    if let Some(available) = sync::available_bytes(&target.path) {
        let space = available.saturating_add(freed);
        if needed > space {
            return Err(format!(
                "not enough space: {} needed, {} available; select fewer tracks with --query{}",
                size(needed),
                size(space),
                if args.delete {
                    ""
                } else {
                    " or remove old files with --delete"
                }
            ));
        }
    }
    if args.dry_run {
        for path in &stale {
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
    for path in &stale {
        fs::remove_file(path).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    if args.delete {
        sync::remove_empty_folders(&target.path).map_err(|error| error.to_string())?;
    }
    let total = plan.pending.len();
    let count = AtomicUsize::new(0);
    let failed = sync::run(
        &plan.pending,
        args.jobs,
        &|transfer: &Transfer, result: &Result<(), String>| {
            let done = count.fetch_add(1, Ordering::Relaxed) + 1;
            let name = transfer
                .destination
                .strip_prefix(&target.path)
                .unwrap_or(&transfer.destination)
                .display();
            match result {
                Ok(()) => println!("[{done}/{total}] {}\t{name}", label(&transfer.action)),
                Err(error) => eprintln!("[{done}/{total}] failed\t{name}: {error}"),
            }
        },
    );
    if failed > 0 {
        return Err(format!("{failed} of {total} files failed"));
    }
    println!("Sync complete: {total} files written");
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

fn absolute(directory: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        directory.join(path)
    }
}
