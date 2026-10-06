//! Read Spotify track metadata into the existing version 1 export format.

use super::{get_json_optional, load_tokens, settings};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::Path;
use url::Url;

const API: &str = "https://api.spotify.com/v1";

pub fn load_playlist_document(
    config_path: &Path,
    token_path: &Path,
    uri: &str,
) -> Result<Value, String> {
    let settings = settings(config_path)?;
    let mut tokens = load_tokens(token_path)
        .ok_or("muzik is not connected to Spotify. Run 'muzik spotify login'.")?;
    read_document(uri, |url| {
        get_json_optional(&settings, token_path, &mut tokens, url)
    })
}

fn read_document(
    uri: &str,
    mut fetch: impl FnMut(&str) -> Result<Option<Value>, String>,
) -> Result<Value, String> {
    let input = uri.trim();
    let normalized = input.to_ascii_lowercase();
    let (kind, id) = if matches!(
        normalized.as_str(),
        "liked" | "liked songs" | "spotify:liked"
    ) {
        ("liked", "liked".to_owned())
    } else if let Some(id) = input.strip_prefix("spotify:playlist:") {
        ("playlist", id.to_owned())
    } else if let Some(id) = input.strip_prefix("spotify:album:") {
        ("album", id.to_owned())
    } else if let Ok(link) = Url::parse(input) {
        if link.scheme() != "https" || link.host_str() != Some("open.spotify.com") {
            return Err(format!("muzik cannot read the Spotify reference {uri}"));
        }
        let path: Vec<_> = link.path_segments().into_iter().flatten().collect();
        match path.as_slice() {
            ["collection", "tracks"] => ("liked", "liked".to_owned()),
            ["playlist", id] => ("playlist", (*id).to_owned()),
            ["album", id] => ("album", (*id).to_owned()),
            [locale, "playlist", id] if locale.starts_with("intl-") => {
                ("playlist", (*id).to_owned())
            }
            [locale, "album", id] if locale.starts_with("intl-") => ("album", (*id).to_owned()),
            _ => return Err(format!("muzik cannot read the Spotify reference {uri}")),
        }
    } else {
        return Err(format!("muzik cannot read the Spotify reference {uri}"));
    };
    if kind != "liked" && (id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric())) {
        return Err("Spotify ID must contain only letters and numbers".into());
    }
    let (title, snapshot, raw_items, album, image) = match kind {
        "liked" => (
            "Liked Songs".to_owned(),
            None,
            pages(
                &mut fetch,
                &[
                    format!("{API}/me/tracks?limit=50"),
                    format!("{API}/me/library?type=track&limit=50"),
                ],
            )?,
            None,
            None,
        ),
        "playlist" => {
            let details = required(&mut fetch, &format!("{API}/playlists/{id}"))?;
            let title = details
                .get("name")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .unwrap_or(id.as_str())
                .to_owned();
            let snapshot = details
                .get("snapshot_id")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned);
            let items = pages(
                &mut fetch,
                &[
                    format!("{API}/playlists/{id}/items?limit=50"),
                    format!("{API}/playlists/{id}/tracks?limit=50"),
                ],
            )?;
            (title, snapshot, items, None, None)
        }
        "album" => {
            let details = required(&mut fetch, &format!("{API}/albums/{id}"))?;
            let title = details
                .get("name")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .unwrap_or(id.as_str())
                .to_owned();
            let image = first_image(details.get("images"));
            let items = pages(&mut fetch, &[format!("{API}/albums/{id}/tracks?limit=50")])?;
            (title, None, items, Some(details), image)
        }
        _ => return Err("unsupported Spotify reference".into()),
    };
    let mut entries = Vec::new();
    for raw in &raw_items {
        let track = if kind == "album" {
            raw
        } else {
            raw.get("item")
                .filter(|item| item.is_object())
                .or_else(|| raw.get("track"))
                .unwrap_or(&Value::Null)
        };
        if let Some(entry) = track_entry(
            track,
            album.as_ref(),
            image.as_deref(),
            raw.get("added_at"),
            entries.len() + 1,
        ) {
            entries.push(entry);
        }
    }
    let mut document = json!({
        "version": 1,
        "source": "spotify",
        "type": "playlist",
        "id": id,
        "title": title,
        "entries": entries,
    });
    if let Some(snapshot) = snapshot {
        document["snapshot_id"] = json!(snapshot);
    }
    Ok(document)
}

fn required(
    fetch: &mut impl FnMut(&str) -> Result<Option<Value>, String>,
    url: &str,
) -> Result<Value, String> {
    fetch(url)?.ok_or_else(|| format!("Spotify resource was not found: {url}"))
}

fn pages(
    fetch: &mut impl FnMut(&str) -> Result<Option<Value>, String>,
    choices: &[String],
) -> Result<Vec<Value>, String> {
    let mut first = None;
    for url in choices {
        validate_url(url)?;
        if let Some(page) = fetch(url)? {
            first = Some((url.to_owned(), page));
            break;
        }
    }
    let (mut url, mut page) = first.ok_or("No Spotify endpoint answered")?;
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    loop {
        if !seen.insert(url.clone()) {
            return Err("Spotify returned a repeated page".into());
        }
        if let Some(current) = page.get("items").and_then(Value::as_array) {
            items.extend(current.iter().cloned());
        }
        let Some(next) = page
            .get("next")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        else {
            break;
        };
        validate_url(next)?;
        url = next.to_owned();
        page = required(fetch, &url)?;
    }
    Ok(items)
}

fn validate_url(url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|error| format!("invalid Spotify page URL: {error}"))?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("api.spotify.com") {
        return Err("Spotify returned a page outside its API".into());
    }
    Ok(())
}

fn first_image(images: Option<&Value>) -> Option<String> {
    images?
        .as_array()?
        .iter()
        .find_map(|image| image.get("url")?.as_str().map(str::to_owned))
}

fn track_entry(
    track: &Value,
    album_override: Option<&Value>,
    image_override: Option<&str>,
    added_at: Option<&Value>,
    index: usize,
) -> Option<Value> {
    if track.get("type").and_then(Value::as_str) == Some("episode")
        || track.get("is_local").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let title = track.get("name")?.as_str()?.trim();
    if title.is_empty() {
        return None;
    }
    let album = album_override.or_else(|| track.get("album"));
    let track_artists = artists(track.get("artists"));
    let artists = if track_artists.is_empty() {
        artists(album.and_then(|value| value.get("artists")))
    } else {
        track_artists
    };
    if artists.is_empty() {
        return None;
    }
    let id = track
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    let source_id = track
        .get("uri")
        .and_then(Value::as_str)
        .filter(|uri| uri.starts_with("spotify:track:"))
        .map(str::to_owned)
        .or_else(|| id.map(|id| format!("spotify:track:{id}")));
    let source_url = track
        .get("external_urls")
        .and_then(|urls| urls.get("spotify"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| id.map(|id| format!("https://open.spotify.com/track/{id}")));
    let mut entry = Map::new();
    entry.insert("index".into(), json!(index));
    entry.insert("title".into(), json!(title));
    entry.insert("artists".into(), json!(artists));
    entry.insert("source_id".into(), json!(source_id));
    entry.insert("source_url".into(), json!(source_url));
    copy_str(&mut entry, album, "name", "album");
    copy_str(&mut entry, album, "release_date", "release_date");
    for key in ["duration_ms", "disc_number", "track_number"] {
        if let Some(value) = track.get(key).filter(|value| value.is_number()) {
            entry.insert(key.into(), value.clone());
        }
    }
    copy_str(&mut entry, track.get("external_ids"), "isrc", "isrc");
    if let Some(value) = added_at
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        entry.insert("added_at".into(), json!(value));
    }
    if let Some(image) = image_override
        .map(str::to_owned)
        .or_else(|| first_image(album.and_then(|value| value.get("images"))))
    {
        entry.insert("source_metadata".into(), json!({"image": image}));
    }
    Some(Value::Object(entry))
}

fn artists(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|artist| artist.get("name")?.as_str().map(str::trim))
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn copy_str(entry: &mut Map<String, Value>, source: Option<&Value>, key: &str, target: &str) {
    if let Some(value) = source
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        entry.insert(target.into(), json!(value));
    }
}

#[cfg(test)]
mod tests {
    use super::read_document;
    use serde_json::json;

    #[test]
    fn reads_playlist_with_old_and_new_item_fields() -> Result<(), String> {
        let document = read_document("spotify:playlist:PL1", |url| {
            if url.ends_with("/PL1") {
                Ok(Some(json!({"name": "Road trip", "snapshot_id": "snap-1"})))
            } else if url.contains("/items?") {
                Ok(Some(json!({"items": [
                    {"item": {"id": "t1", "name": "One", "artists": [{"name": "Alex"}], "duration_ms": 120000}},
                    {"track": {"id": "t2", "name": "Two", "artists": [{"name": "Bea"}]}},
                    {"item": {"type": "episode", "name": "Podcast"}}
                ], "next": null})))
            } else {
                Err(format!("unexpected URL: {url}"))
            }
        })?;
        assert_eq!(document["snapshot_id"], "snap-1");
        assert_eq!(document["entries"].as_array().map(Vec::len), Some(2));
        assert_eq!(document["entries"][0]["source_id"], "spotify:track:t1");
        assert_eq!(document["entries"][1]["index"], 2);
        Ok(())
    }

    #[test]
    fn reads_album_and_uses_album_artist_and_image() -> Result<(), String> {
        let document = read_document("spotify:album:AL1", |url| {
            if url.ends_with("/AL1") {
                Ok(Some(
                    json!({"name": "Album", "artists": [{"name": "Alex"}], "images": [{"url": "https://example.test/art"}]}),
                ))
            } else {
                Ok(Some(
                    json!({"items": [{"id": "t1", "name": "One"}], "next": null}),
                ))
            }
        })?;
        assert_eq!(document["entries"][0]["artists"][0], "Alex");
        assert_eq!(
            document["entries"][0]["source_metadata"]["image"],
            "https://example.test/art"
        );
        Ok(())
    }

    #[test]
    fn rejects_cross_site_page_url() {
        let result = read_document("spotify:liked", |_| {
            Ok(Some(
                json!({"items": [], "next": "https://example.test/private"}),
            ))
        });
        assert!(result.is_err());
    }

    #[test]
    fn liked_songs_use_the_old_endpoint_and_follow_pages() -> Result<(), String> {
        let mut requested = Vec::new();
        let document = read_document("liked", |url| {
            requested.push(url.to_owned());
            if url.contains("/me/tracks?") {
                Ok(None)
            } else if url.contains("offset=50") {
                Ok(Some(
                    json!({"items": [{"track": {"id": "t2", "name": "Two", "artists": [{"name": "Alex"}]}}], "next": null}),
                ))
            } else {
                Ok(Some(
                    json!({"items": [{"track": {"id": "t1", "name": "One", "artists": [{"name": "Alex"}]}}], "next": "https://api.spotify.com/v1/me/library?offset=50"}),
                ))
            }
        })?;
        assert_eq!(document["entries"].as_array().map(Vec::len), Some(2));
        assert_eq!(document["entries"][1]["index"], 2);
        assert_eq!(requested.len(), 3);
        Ok(())
    }

    #[test]
    fn accepts_a_spotify_playlist_link() -> Result<(), String> {
        let document = read_document("https://open.spotify.com/playlist/PL1?si=abc", |_| {
            Ok(Some(
                json!({"name": "Road trip", "items": [], "next": null}),
            ))
        })?;
        assert_eq!(document["id"], "PL1");
        Ok(())
    }
}
