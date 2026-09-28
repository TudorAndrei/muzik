//! Organize audio through the native importer or refresh tags from beets rows.

use std::fs;
use std::path::PathBuf;

use muzik_core::BeetsConfig;
use muzik_library::{Item, Library, SqlValue};
use muzik_tags::TagData;
use serde_json::json;

use crate::{Import, Organize, import};

pub fn run(args: &Organize) -> Result<(), String> {
    if !args.directory.exists() {
        return Err(format!("Directory not found: {}", args.directory.display()));
    }
    if args.tag_only {
        return write_library_tags(args);
    }

    // The Python command accepts --import, but its default already moves files.
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

fn write_library_tags(args: &Organize) -> Result<(), String> {
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(muzik_core::default_config_path);
    let config = BeetsConfig::load(&config_path, json!({})).map_err(|error| error.to_string())?;
    let db = import::configured_path(&config, &config_path, "library")?;
    let root = import::configured_path(&config, &config_path, "directory")?;
    if !db.exists() {
        return Err(format!("library database does not exist: {}", db.display()));
    }
    let library = Library::open_read_only(&db).map_err(|error| error.to_string())?;
    let requested = args
        .directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let mut count = 0;
    for item in library.items().map_err(|error| error.to_string())? {
        let Some(path) = item.field("path").and_then(stored_path) else {
            continue;
        };
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        let path = path.canonicalize().unwrap_or(path);
        if path != requested && !path.starts_with(&requested) {
            continue;
        }
        count += 1;
        if args.dry_run {
            continue;
        }
        muzik_tags::write(&path, &tags_from_item(&item))
            .map_err(|error| format!("cannot write tags to {}: {error}", path.display()))?;
        if let Some(directory) = path.parent()
            && let Some(cover) = muzik_tags::find_cover(directory)
        {
            let mime = if cover
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
            {
                "image/png"
            } else {
                "image/jpeg"
            };
            let bytes = fs::read(&cover).map_err(|error| error.to_string())?;
            muzik_tags::embed_cover(&path, &bytes, mime)
                .map_err(|error| format!("cannot embed cover in {}: {error}", path.display()))?;
        }
    }
    if count == 0 {
        return Err(format!(
            "No library items match {}",
            args.directory.display()
        ));
    }
    if args.dry_run {
        println!(
            "Tag preview: {count} library items under {}.",
            args.directory.display()
        );
    } else {
        println!("Organization complete. Tagged {count} library item(s).");
    }
    Ok(())
}

fn stored_path(value: &SqlValue) -> Option<PathBuf> {
    match value {
        SqlValue::Blob(bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                Some(std::ffi::OsString::from_vec(bytes.clone()).into())
            }
            #[cfg(not(unix))]
            {
                Some(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
            }
        }
        SqlValue::Text(text) => Some(PathBuf::from(text)),
        _ => None,
    }
}

fn tags_from_item(item: &Item) -> TagData {
    let mut tags = TagData::default();
    for field in muzik_tags::FIELDS {
        if matches!(field.name, "date" | "original_date") {
            continue;
        }
        if let Some(value) = item.field(field.name).and_then(scalar_text)
            && !value.is_empty()
        {
            tags.fields.insert(field.name.to_owned(), value);
        }
    }
    if let Some(value) = item.field("albumdisambig").and_then(scalar_text)
        && !value.is_empty()
    {
        tags.fields.insert("albumdisambig".into(), value);
    }
    for name in ["rg_track_gain", "rg_album_gain"] {
        if let Some(SqlValue::Real(value)) = item.field(name) {
            tags.fields.insert(name.into(), format!("{value:.2} dB"));
        }
    }
    for name in ["rg_track_peak", "rg_album_peak"] {
        if let Some(SqlValue::Real(value)) = item.field(name) {
            tags.fields.insert(name.into(), format!("{value:.6}"));
        }
    }
    for (prefix, target) in [("", "date"), ("original_", "original_date")] {
        let Some(year) = item.field(&format!("{prefix}year")).and_then(scalar_text) else {
            continue;
        };
        let mut date = format!("{year:0>4}");
        if let Some(month) = item.field(&format!("{prefix}month")).and_then(scalar_text) {
            date.push_str(&format!("-{month:0>2}"));
            if let Some(day) = item.field(&format!("{prefix}day")).and_then(scalar_text) {
                date.push_str(&format!("-{day:0>2}"));
            }
        }
        tags.fields.insert(target.to_owned(), date);
    }
    if let Some(comp) = item.field("comp").and_then(scalar_text) {
        tags.fields
            .insert("comp".into(), if comp == "0" { "0" } else { "1" }.into());
    }
    tags
}

fn scalar_text(value: &SqlValue) -> Option<String> {
    match value {
        SqlValue::Text(text) => Some(text.clone()),
        SqlValue::Integer(number) => Some(number.to_string()),
        SqlValue::Real(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use muzik_library::Fields;

    fn fixture() -> (tempfile::TempDir, Organize, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("music");
        fs::create_dir_all(&root).unwrap();
        let audio = root.join("song.mp3");
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../crates/muzik-tags/tests/fixtures/blank.mp3"),
            &audio,
        )
        .unwrap();
        let db = temp.path().join("library.db");
        let mut library = Library::open_or_create(&db).unwrap();
        let mut fields = Fields::new();
        fields.insert(
            "path".into(),
            SqlValue::Blob(audio.as_os_str().as_encoded_bytes().to_vec()),
        );
        fields.insert("title".into(), SqlValue::Text("Library title".into()));
        fields.insert("artist".into(), SqlValue::Text("Library artist".into()));
        library.insert_item(&fields, &Fields::new()).unwrap();
        drop(library);
        let config = temp.path().join("config.yaml");
        fs::write(
            &config,
            format!("library: {}\ndirectory: {}\n", db.display(), root.display()),
        )
        .unwrap();
        let args = Organize {
            directory: root,
            import: false,
            tag_only: true,
            dry_run: false,
            config: Some(config),
        };
        (temp, args, audio)
    }

    #[test]
    fn tag_only_writes_from_existing_beets_row() {
        let (_temp, args, audio) = fixture();
        run(&args).unwrap();
        let tags = muzik_tags::read(&audio, &[]).unwrap();
        assert_eq!(
            tags.fields.get("title").map(String::as_str),
            Some("Library title")
        );
        assert_eq!(
            tags.fields.get("artist").map(String::as_str),
            Some("Library artist")
        );
        assert!(audio.exists());
    }

    #[test]
    fn tag_only_dry_run_keeps_audio_unchanged() {
        let (_temp, mut args, audio) = fixture();
        let before = fs::read(&audio).unwrap();
        args.dry_run = true;
        run(&args).unwrap();
        assert_eq!(fs::read(audio).unwrap(), before);
    }

    #[test]
    fn tag_only_reports_when_no_library_item_matches() {
        let (_temp, mut args, _audio) = fixture();
        let other = args.directory.join("other");
        fs::create_dir(&other).unwrap();
        args.directory = other.clone();
        args.dry_run = true;
        assert_eq!(
            run(&args),
            Err(format!("No library items match {}", other.display()))
        );
    }
}
