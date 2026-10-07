//! CLI presentation for the shared Beets organization service.

use anyhow::bail;
use muzik_import::beets;

use crate::{Import, Organize, import};

pub fn run(args: &Organize) -> anyhow::Result<()> {
    if !args.directory.exists() {
        bail!("Directory not found: {}", args.directory.display());
    }
    if args.tag_only {
        let count =
            beets::write_library_tags(&args.directory, args.config.as_deref(), args.dry_run)?;
        if args.dry_run {
            println!(
                "Tag preview: {count} library items under {}.",
                args.directory.display()
            );
        } else {
            println!("Organization complete. Tagged {count} library item(s).");
        }
        return Ok(());
    }

    import::run(&Import {
        directory: Some(args.directory.clone()),
        library: None,
        copy: false,
        link: false,
        nowrite: false,
        quiet: false,
        dry_run: args.dry_run,
        no_prune: false,
        duplicates: muzik_core::DuplicatePolicy::default(),
        config: args.config.clone(),
    })
}
