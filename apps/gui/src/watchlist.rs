//! Saved watchlist reads and local checks for the desktop app.

use muzik_core::watchlist;
use muzik_runner::Settings;
use serde_json::Value;

pub fn saved(settings: &Settings, repository: &watchlist::Repository) -> Result<Value, String> {
    view(settings, repository.load()?)
}

pub fn checked(settings: &Settings, repository: &watchlist::Repository) -> Result<Value, String> {
    let mut document = repository.load()?;
    watchlist::reconcile(&mut document, settings.reconcile())?;
    Ok(document)
}

pub fn view(settings: &Settings, document: Value) -> Result<Value, String> {
    watchlist::view(document, &settings.request.output, &settings.paths.cache)
}

#[cfg(test)]
mod tests {
    use muzik_core::paths::Paths;
    use muzik_runner::Settings;
    use serde_json::json;
    use std::fs;

    #[test]
    fn load_uses_the_given_output_and_saved_cards() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let output = dir.path().join("audio");
        fs::create_dir(&output)?;
        let paths = Paths::under(dir.path());
        let repo = muzik_core::watchlist::Repository::open(&paths);
        repo.add("https://www.youtube.com/playlist?list=PL123")
            .map_err(std::io::Error::other)?;
        let settings =
            Settings::resolve(&paths, &json!({"output": output})).map_err(std::io::Error::other)?;
        let saved = super::saved(&settings, &repo).map_err(std::io::Error::other)?;
        assert_eq!(saved["playlists"][0]["playlist_id"], "PL123");
        Ok(())
    }
}
