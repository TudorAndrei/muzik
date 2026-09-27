use std::fs;
use std::io;

use crate::paths;

const IMPORT_DEFAULTS: &str = "import:\n  move: yes\n  duplicate_action: skip\n  none_rec_action: asis\nmatch:\n  strong_rec_thresh: 0.10\n  medium_rec_thresh: 0.20\n";

pub fn run() -> io::Result<()> {
    for (name, path) in [
        ("Downloads", paths::download_dir()),
        ("Bandcamp", paths::data_dir().join("bandcamp")),
        ("Soulseek", paths::data_dir().join("soulseek")),
        ("Splits", paths::data_dir().join("splits")),
        ("Cache", paths::cache_dir()),
        ("Config", paths::config_dir()),
    ] {
        fs::create_dir_all(&path)?;
        println!("{name}: {}", path.display());
    }

    let library_path = muzik_core::default_config_path();
    if let Some(parent) = library_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let current = match fs::read_to_string(&library_path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let updated = with_import_defaults(&current);
    if current != updated {
        fs::write(&library_path, updated)?;
        println!("Library config: {}", library_path.display());
    }
    Ok(())
}

fn with_import_defaults(current: &str) -> String {
    if !current.lines().any(|line| line.trim() == "import:") {
        return format!("{}\n{}", current.trim_end_matches('\n'), IMPORT_DEFAULTS);
    }
    let mut missing = Vec::new();
    for (key, line) in [
        ("move:", "  move: yes"),
        ("duplicate_action:", "  duplicate_action: skip"),
        ("none_rec_action:", "  none_rec_action: asis"),
    ] {
        if !current.lines().any(|entry| entry.trim().starts_with(key)) {
            missing.push(line);
        }
    }
    if missing.is_empty() {
        return current.to_owned();
    }
    let mut result = String::new();
    for line in current.lines() {
        result.push_str(line);
        result.push('\n');
        if line.trim() == "import:" {
            for entry in &missing {
                result.push_str(entry);
                result.push('\n');
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::with_import_defaults;

    #[test]
    fn init_keeps_existing_beets_settings() {
        let config = "directory: /music\nimport:\n  copy: no\n";
        let updated = with_import_defaults(config);
        assert!(updated.contains("directory: /music\n"));
        assert!(updated.contains("  copy: no\n"));
        assert!(updated.contains("  move: yes\n"));
        assert!(updated.contains("  duplicate_action: skip\n"));
    }
}
