use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::Context;
use muzik_bandcamp as bandcamp;
use muzik_core::paths::Paths;

use crate::{Bandcamp, paths};

pub fn download(args: &Bandcamp) -> anyhow::Result<()> {
    let login = match &args.cookies {
        Some(file) => {
            let text = fs::read_to_string(file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            bandcamp::Login::save(&Paths::user(), args.user.as_deref(), &text)?
        }
        None => bandcamp::Login::load(&Paths::user()).context(
            "Save the Bandcamp login first: give --cookies <file>, or use Settings in the app.",
        )?,
    };
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| paths::data_dir().join("bandcamp"));
    let purchases = bandcamp::collection(&login)?;
    let folders = purchase_folders(&output, &purchases)?;
    println!("{} purchase(s) in the collection", purchases.len());
    let cancelled = AtomicBool::new(false);
    for (purchase, folder) in purchases.iter().zip(&folders) {
        let label = purchase.label();
        if !args.force && !bandcamp::audio_files(folder).is_empty() {
            println!("Already downloaded: {label}");
            continue;
        }
        if args.dry_run {
            println!("Would download: {label}");
            continue;
        }
        println!("Downloading: {label}");
        let files = bandcamp::download(
            &login,
            &purchase.download_page,
            args.format,
            folder,
            &cancelled,
            &mut |_, _| {},
        )?;
        println!("  {} file(s) in {}", files.len(), folder.display());
    }
    Ok(())
}

fn purchase_folders(
    output: &Path,
    purchases: &[bandcamp::Purchase],
) -> anyhow::Result<Vec<PathBuf>> {
    let mut owners = HashMap::new();
    let mut folders = Vec::new();
    for purchase in purchases {
        let name = folder_name(&purchase.label());
        if let Some(previous) = owners.insert(name.to_lowercase(), &purchase.key) {
            anyhow::bail!(
                "Bandcamp purchases {previous} and {} use the same folder: {name}. Download them to separate folders.",
                purchase.key
            );
        }
        folders.push(output.join(name));
    }
    Ok(folders)
}

fn folder_name(label: &str) -> String {
    let name: String = label
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\' | ':') {
                '_'
            } else {
                character
            }
        })
        .collect();
    let name = name.trim().trim_start_matches('.');
    if name.is_empty() {
        "Bandcamp purchase".into()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{folder_name, purchase_folders};

    #[test]
    fn purchases_need_distinct_download_folders() {
        let make = |key: &str, title: &str| muzik_bandcamp::Purchase {
            key: key.into(),
            artist: "Band".into(),
            title: title.into(),
            single: false,
            download_page: "https://bandcamp.com/download".into(),
            item_url: None,
            art_url: None,
        };
        let output = std::path::Path::new("downloads");
        for (first, other) in [("Album", "Album"), ("Album", "album"), ("A/B", "A:B")] {
            let error =
                purchase_folders(output, &[make("a1", first), make("t2", other)]).unwrap_err();
            assert!(error.to_string().contains("a1 and t2"));
        }
        assert_eq!(
            purchase_folders(output, &[make("a1", "Album"), make("t2", "Single")]).unwrap(),
            [output.join("Band - Album"), output.join("Band - Single")]
        );
    }

    #[test]
    fn a_purchase_label_becomes_one_safe_folder_name() {
        assert_eq!(folder_name("Band - Album"), "Band - Album");
        assert_eq!(folder_name("AC/DC: Live\n"), "AC_DC_ Live_");
        assert_eq!(folder_name(" .. "), "Bandcamp purchase");
    }
}
