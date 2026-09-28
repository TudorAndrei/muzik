//! CLI presentation for the shared Beets import service.

use muzik_import::apply::{AlbumDecision, DuplicateDecision, MatchDecision};
use muzik_import::beets::{self, ImportRequest, SyncOutcome};

use crate::Import;

pub fn run(args: &Import) -> Result<(), String> {
    match (&args.directory, &args.library) {
        (None, None) => return Err("give an audio path or --library QUERY".into()),
        (Some(_), Some(_)) => return Err("choose an audio path or --library QUERY".into()),
        (None, Some(query)) => return sync(query, args),
        (Some(_), None) => {}
    }
    let source = args.directory.as_ref().ok_or("audio path is missing")?;
    let preview = beets::plan_import(ImportRequest {
        source: source.clone(),
        config_path: args.config.clone(),
        copy: args.copy,
        link: args.link,
        nowrite: args.nowrite,
        dry_run: args.dry_run,
        no_prune: args.no_prune,
    })?;
    let decisions = preview
        .plan
        .albums
        .iter()
        .map(|album| {
            let title = album
                .items
                .first()
                .map(|item| item.match_item.album.as_str())
                .unwrap_or("");
            println!("Album: {} ({title})", album.source_dir.display());
            AlbumDecision {
                choice: if args.quiet {
                    MatchDecision::Skip
                } else {
                    MatchDecision::AsIs
                },
                duplicate: (!album.duplicates.is_empty()).then_some(DuplicateDecision::Skip),
            }
        })
        .collect::<Vec<_>>();
    let result = beets::apply_import(preview, &decisions)?;
    for path in &result.apply.destinations {
        println!("{}", path.display());
    }
    println!(
        "{} album(s), {} item(s), {} skipped",
        result
            .planned_albums
            .saturating_sub(result.apply.skipped_albums),
        result.apply.destinations.len(),
        result.apply.skipped_albums + result.apply.skipped_incremental
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
    Ok(())
}

fn sync(query: &str, args: &Import) -> Result<(), String> {
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
