//! Fetch watchlist images into the existing user cache.

use muzik_core::thumbnails as cache;
use muzik_store::watchlist::Repository;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub struct ThumbnailUpdate {
    pub video_id: String,
    pub result: Result<PathBuf, String>,
}

pub fn cache_requested(
    ids: &[String],
    repository: &Repository,
    cache_dir: &Path,
) -> Vec<ThumbnailUpdate> {
    match repository.load() {
        Ok(watchlist) => {
            let urls: BTreeMap<&str, Option<&str>> = watchlist
                .items()
                .filter_map(|item| Some((item.video_id.as_deref()?, item.thumbnail_url.as_deref())))
                .collect();
            ids.iter()
                .map(|id| ThumbnailUpdate {
                    video_id: id.clone(),
                    result: urls.get(id.as_str()).map_or_else(
                        || Err("The item is no longer in the watchlist.".to_owned()),
                        |url| cache_item(id, *url, cache_dir),
                    ),
                })
                .collect()
        }
        Err(error) => ids
            .iter()
            .map(|id| ThumbnailUpdate {
                video_id: id.clone(),
                result: Err(error.to_string()),
            })
            .collect(),
    }
}

fn cache_item(id: &str, source_url: Option<&str>, cache_dir: &Path) -> Result<PathBuf, String> {
    if !cache::valid_id(id) {
        return Err("Invalid thumbnail ID.".into());
    }
    if let Some(path) = cache::cached_path(id, cache_dir) {
        return Ok(path);
    }
    let url = source_url.ok_or("The item has no thumbnail URL.")?;
    fetch_and_save(id, url, cache_dir)
}

fn fetch_and_save(id: &str, source_url: &str, cache_dir: &Path) -> Result<PathBuf, String> {
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
    Ok(cache::save(id, &content_type, &bytes, cache_dir)?)
}

#[cfg(test)]
mod tests {
    use super::{Repository, ThumbnailUpdate, cache_requested};
    use serde_json::json;
    use std::fs;

    #[test]
    fn reads_an_existing_image_from_the_saved_watchlist() {
        let dir = tempfile::tempdir().unwrap();
        let watchlist = Repository::new(dir.path().join("muzik.db"));
        let cache = dir.path().join("cache");
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("yt_thumbnail_abcdefghijk.jpg"), b"saved image").unwrap();
        watchlist
            .save(
                &muzik_store::watchlist::Watchlist::from_value(json!({
                    "version": 3,
                    "playlists": [{
                        "playlist_id": "PL1",
                        "url": "https://www.youtube.com/playlist?list=PL1",
                        "items": [{"position": 1, "title": "Song", "video_id": "abcdefghijk", "thumbnail_url": "https://i.ytimg.com/vi/abcdefghijk/default.jpg"}]
                    }]
                }))
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            cache_requested(&["abcdefghijk".into()], &watchlist, &cache),
            [ThumbnailUpdate {
                video_id: "abcdefghijk".into(),
                result: Ok(cache.join("yt_thumbnail_abcdefghijk.jpg")),
            }]
        );
    }
}
