//! Fetch watchlist images into the existing user cache.

use muzik_core::{thumbnails as cache, watchlist::Repository};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub fn validate_ids(params: &Value) -> Result<Vec<String>, String> {
    let ids = params
        .get("video_ids")
        .and_then(Value::as_array)
        .ok_or("video_ids must be a list of at most 16 IDs")?;
    if ids.len() > 16 || ids.iter().any(|id| !id.is_string()) {
        return Err("video_ids must be a list of at most 16 IDs".into());
    }
    Ok(ids
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect())
}

pub fn cache_requested(ids: &[String], repository: &Repository, cache_dir: &Path) -> Value {
    let updates = match repository.load() {
        Ok(watchlist) => {
            let mut urls = BTreeMap::new();
            if let Some(playlists) = watchlist["playlists"].as_array() {
                for item in playlists
                    .iter()
                    .flat_map(|playlist| playlist["items"].as_array().into_iter().flatten())
                {
                    if let Some(id) = item["video_id"].as_str() {
                        urls.insert(
                            id.to_owned(),
                            item["thumbnail_url"].as_str().map(str::to_owned),
                        );
                    }
                }
            }
            ids.iter()
                .map(|id| {
                    let result = match urls.get(id) {
                        None => Err("The item is no longer in the watchlist.".to_owned()),
                        Some(url) => cache_item(id, url.as_deref(), cache_dir).map(Some),
                    };
                    update(id, result)
                })
                .collect::<Vec<_>>()
        }
        Err(error) => ids
            .iter()
            .map(|id| update(id, Err(error.clone())))
            .collect(),
    };
    json!({"thumbnails": updates})
}

fn cache_item(
    id: &str,
    source_url: Option<&str>,
    cache_dir: &Path,
) -> Result<std::path::PathBuf, String> {
    if !cache::valid_id(id) {
        return Err("Invalid thumbnail ID.".into());
    }
    if let Some(path) = cache::cached_path(id, cache_dir) {
        return Ok(path);
    }
    let url = source_url.ok_or("The item has no thumbnail URL.")?;
    fetch_and_save(id, url, cache_dir)
}

fn update(id: &str, result: Result<Option<std::path::PathBuf>, String>) -> Value {
    match result {
        Ok(path) => json!({"video_id": id, "path": path, "error": null}),
        Err(error) => json!({"video_id": id, "path": null, "error": error}),
    }
}

fn fetch_and_save(
    id: &str,
    source_url: &str,
    cache_dir: &Path,
) -> Result<std::path::PathBuf, String> {
    let mut url = url::Url::parse(source_url).map_err(|error| error.to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Thumbnail URL must use HTTP or HTTPS.".into());
    }
    if matches!(url.host_str(), Some("i.ytimg.com" | "img.youtube.com")) {
        url = url::Url::parse(&format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
            .map_err(|error| error.to_string())?;
    }
    let mut response = ureq::get(url.as_str())
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .call()
        .map_err(|error| error.to_string())?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let bytes = response
        .body_mut()
        .read_to_vec()
        .map_err(|error| error.to_string())?;
    cache::save(id, &content_type, &bytes, cache_dir)
}

#[cfg(test)]
mod tests {
    use super::{cache_requested, validate_ids, Repository};
    use serde_json::json;
    use std::fs;

    #[test]
    fn reads_an_existing_image_from_the_saved_watchlist() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let watchlist = Repository::new(dir.path().join("muzik.db"));
        let cache = dir.path().join("cache");
        fs::create_dir(&cache)?;
        fs::write(cache.join("yt_thumbnail_abcdefghijk.jpg"), b"saved image")?;
        watchlist.save(json!({
            "version": 3,
            "playlists": [{
                "playlist_id": "PL1",
                "url": "https://www.youtube.com/playlist?list=PL1",
                "items": [{"position": 1, "title": "Song", "video_id": "abcdefghijk", "thumbnail_url": "https://i.ytimg.com/vi/abcdefghijk/default.jpg"}]
            }]
        }))?;
        let ids = validate_ids(&json!({"video_ids": ["abcdefghijk", "abcdefghijk"]}))?;
        let result = cache_requested(&ids, &watchlist, &cache);
        assert_eq!(result["thumbnails"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            result["thumbnails"][0]["path"],
            json!(cache.join("yt_thumbnail_abcdefghijk.jpg"))
        );
        assert!(result["thumbnails"][0]["error"].is_null());
        Ok(())
    }
}
