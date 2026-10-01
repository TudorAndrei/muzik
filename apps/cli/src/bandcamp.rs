use std::fs;
use std::sync::atomic::AtomicBool;

use muzik_core::bandcamp;

use crate::{Bandcamp, paths};

pub fn download(args: &Bandcamp) -> Result<(), String> {
    let login = match &args.cookies {
        Some(file) => {
            let text = fs::read_to_string(file)
                .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
            bandcamp::Login::save(args.user.as_deref().unwrap_or(""), &text)?
        }
        None => bandcamp::Login::load().ok_or(
            "Save the Bandcamp login first: give --cookies <file>, or use Settings in the app.",
        )?,
    };
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| paths::data_dir().join("bandcamp"));
    let purchases = bandcamp::collection(&login)?;
    println!(
        "{} purchase(s) in the collection of {}",
        purchases.len(),
        login.user
    );
    let cancelled = AtomicBool::new(false);
    for purchase in &purchases {
        let label = purchase.label();
        let folder = output.join(folder_name(&label));
        if !args.force && !bandcamp::audio_files(&folder).is_empty() {
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
            &args.format,
            &folder,
            &cancelled,
            &mut |_, _| {},
        )?;
        println!("  {} file(s) in {}", files.len(), folder.display());
    }
    Ok(())
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
    use super::folder_name;

    #[test]
    fn a_purchase_label_becomes_one_safe_folder_name() {
        assert_eq!(folder_name("Band - Album"), "Band - Album");
        assert_eq!(folder_name("AC/DC: Live\n"), "AC_DC_ Live_");
        assert_eq!(folder_name(" .. "), "Bandcamp purchase");
    }
}
