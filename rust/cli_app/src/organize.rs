//! CLI presentation for the shared Beets organization service.

use muzik_import::beets;

use crate::{Import, Organize, import};

pub fn run(args: &Organize) -> Result<(), String> {
    if !args.directory.exists() {
        return Err(format!("Directory not found: {}", args.directory.display()));
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

    // Import moves files by default, as in the existing Beets CLI command.
    let _ = args.import;
    import::run(&Import {
        directory: Some(args.directory.clone()),
        library: None,
        copy: false,
        link: false,
        nowrite: false,
        quiet: false,
        dry_run: args.dry_run,
        no_prune: false,
        config: args.config.clone(),
    })
}
