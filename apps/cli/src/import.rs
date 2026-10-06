//! CLI presentation for the shared Beets import service.

use anyhow::{Context, bail};
use muzik_import::apply::{AlbumDecision, MatchDecision};
use muzik_import::beets::{self, ImportRequest, SyncOutcome};
use muzik_import::decide::{ImportPolicy, NeverAsk, decide_album};

use crate::Import;

pub fn run(args: &Import) -> anyhow::Result<()> {
    match (&args.directory, &args.library) {
        (None, None) => bail!("give an audio path or --library QUERY"),
        (Some(_), Some(_)) => bail!("choose an audio path or --library QUERY"),
        (None, Some(query)) => return sync(query, args),
        (Some(_), None) => {}
    }
    let source = args.directory.as_ref().context("audio path is missing")?;
    let preview = beets::plan_import(ImportRequest {
        source: source.clone(),
        config_path: args.config.clone(),
        copy: args.copy,
        link: args.link,
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
                .map(|item| item.match_item.album.as_str())
                .unwrap_or("");
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
