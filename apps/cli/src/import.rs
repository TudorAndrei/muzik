//! CLI presentation for the shared Beets import service.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::bail;
use muzik_core::paths::Paths;
use muzik_core::{JobEvent, Severity};
use muzik_import::apply::{AlbumDecision, MatchDecision};
use muzik_import::beets::{self, ImportRequest, Placement, SyncOutcome};
use muzik_import::decide::{ImportPolicy, NeverAsk, decide_album};
use muzik_runner::Settings;
use muzik_runner::library_tasks::{self, AfterImport};
use serde_json::json;

use crate::Import;

enum Target<'a> {
    Audio(&'a Path),
    Library(&'a str),
}

impl<'a> Target<'a> {
    fn of(args: &'a Import) -> anyhow::Result<Self> {
        match (&args.directory, &args.library) {
            (None, None) => bail!("give an audio path or --library QUERY"),
            (Some(_), Some(_)) => bail!("choose an audio path or --library QUERY"),
            (Some(path), None) => Ok(Self::Audio(path)),
            (None, Some(query)) => Ok(Self::Library(query)),
        }
    }
}

fn placement(args: &Import) -> anyhow::Result<Placement> {
    match (args.copy, args.link) {
        (true, true) => bail!("--link and --copy cannot be used together"),
        (true, false) => Ok(Placement::Copy),
        (false, true) => Ok(Placement::Link),
        (false, false) => Ok(Placement::Move),
    }
}

pub fn run(args: &Import) -> anyhow::Result<()> {
    let source = match Target::of(args)? {
        Target::Audio(source) => source,
        Target::Library(query) => return sync(query, args),
    };
    let preview = beets::plan_import(ImportRequest {
        source: source.to_path_buf(),
        config_path: args.config.clone(),
        placement: placement(args)?,
        nowrite: args.nowrite,
        dry_run: args.dry_run,
        force: false,
        no_prune: args.no_prune,
    })?;
    let policy = ImportPolicy {
        interactive: false,
        force: false,
        duplicates: args.duplicates,
    };
    let decisions = preview
        .plan
        .albums
        .iter()
        .map(|album| {
            let title = album
                .items
                .first()
                .map_or("", |item| item.match_item.album.as_str());
            println!("Album: {} ({title})", album.source_dir.display());
            if args.quiet {
                Ok(AlbumDecision {
                    choice: MatchDecision::Skip,
                    duplicate: None,
                })
            } else {
                decide_album(album, policy, &mut NeverAsk)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let library = preview.paths.clone();
    let result = beets::apply_import(preview, &decisions)?;
    if result.apply.already_in_library > 0 {
        println!(
            "{} album(s) are already in the library and were not imported again",
            result.apply.already_in_library
        );
    }
    for path in &result.apply.destinations {
        println!("{}", path.display());
    }
    println!(
        "{} album(s), {} item(s), {} skipped",
        result
            .planned_albums
            .saturating_sub(result.apply.skipped_albums),
        result.apply.destinations.len(),
        result
            .apply
            .skipped_albums
            .saturating_add(result.apply.skipped_incremental)
    );
    if !result.apply.cleanup_failed.is_empty()
        || !result.apply.source_cleanup_failed.is_empty()
        || !result.apply.history_failed.is_empty()
    {
        eprintln!("Warning: some source files or history entries could not be cleaned up");
    }
    if result.pruned_items > 0 {
        println!("Pruned {} missing library item(s)", result.pruned_items);
    }
    if let Some(error) = result.prune_error {
        eprintln!("Warning: skipped library prune: {error}");
    }
    if !args.dry_run {
        library_tasks::after_import(
            after_import(&Paths::user(), args),
            &library,
            &result.apply.destinations,
            &AtomicBool::new(false),
            &mut print_event,
        );
    }
    Ok(())
}

fn after_import(paths: &Paths, args: &Import) -> AfterImport {
    match Settings::resolve(paths, &json!({})) {
        Ok(settings) => AfterImport {
            replaygain: settings.after_import.replaygain && !args.nowrite,
            covers: settings.after_import.covers,
        },
        Err(error) => {
            eprintln!("Warning: skipped the tasks after the import: {error}");
            AfterImport::default()
        }
    }
}

fn print_event(event: JobEvent) {
    match event {
        JobEvent::Message {
            message,
            severity: Severity::Info,
        } => println!("{message}"),
        JobEvent::Message { message, .. } => eprintln!("Warning: {message}"),
        JobEvent::ProgressStarted { description, .. } => println!("{description}"),
        _ => {}
    }
}

fn sync(query: &str, args: &Import) -> anyhow::Result<()> {
    match beets::sync_library(query, args.config.as_deref(), args.dry_run, args.nowrite)? {
        SyncOutcome::Preview { albums, items } => {
            println!("Sync preview: {albums} albums and {items} items selected");
        }
        SyncOutcome::Updated(result) => {
            println!(
                "Updated {} album(s), {} item(s)",
                result.albums_updated, result.items_updated
            );
        }
    }
    Ok(())
}
