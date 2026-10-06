//! Spotify playlist references shared by the CLI and desktop app.

use super::{get_json, load_tokens, settings};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use url::Url;

const PLAYLISTS_URL: &str = "https://api.spotify.com/v1/me/playlists?limit=50";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlaylistRef {
    pub uri: String,
    pub name: String,
    pub owner: String,
    pub total: Option<u64>,
    pub image_url: Option<String>,
}

pub fn list_playlists(config_path: &Path, token_path: &Path) -> Result<Vec<PlaylistRef>, String> {
    let settings = settings(config_path)?;
    let mut tokens = load_tokens(token_path)
        .ok_or("muzik is not connected to Spotify. Run 'muzik spotify login'.")?;
    collect_playlists(|url| get_json(&settings, token_path, &mut tokens, url))
}

fn collect_playlists(
    mut fetch: impl FnMut(&str) -> Result<Value, String>,
) -> Result<Vec<PlaylistRef>, String> {
    let mut playlists = vec![PlaylistRef {
        uri: "spotify:liked".into(),
        name: "Liked Songs".into(),
        owner: "you".into(),
        total: None,
        image_url: None,
    }];
    let mut next = Some(PLAYLISTS_URL.to_owned());
    let mut seen = HashSet::new();
    while let Some(url) = next {
        let parsed =
            Url::parse(&url).map_err(|error| format!("invalid Spotify page URL: {error}"))?;
        if parsed.scheme() != "https" || parsed.host_str() != Some("api.spotify.com") {
            return Err("Spotify returned a playlist page outside its API".into());
        }
        if !seen.insert(url.clone()) {
            return Err("Spotify returned a repeated playlist page".into());
        }
        let page = fetch(&url)?;
        if let Some(items) = page.get("items").and_then(Value::as_array) {
            for raw in items {
                let Some(id) = raw
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                else {
                    continue;
                };
                let name = raw
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .unwrap_or(id);
                let owner = raw
                    .get("owner")
                    .and_then(|owner| {
                        owner
                            .get("display_name")
                            .and_then(Value::as_str)
                            .filter(|name| !name.is_empty())
                            .or_else(|| owner.get("id").and_then(Value::as_str))
                    })
                    .unwrap_or("");
                let total = raw
                    .get("items")
                    .and_then(|items| items.get("total"))
                    .and_then(Value::as_u64)
                    .or_else(|| {
                        raw.get("tracks")
                            .and_then(|tracks| tracks.get("total"))
                            .and_then(Value::as_u64)
                    });
                let image_url = raw
                    .get("images")
                    .and_then(Value::as_array)
                    .and_then(|images| images.first())
                    .and_then(|image| image.get("url"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                playlists.push(PlaylistRef {
                    uri: format!("spotify:playlist:{id}"),
                    name: name.to_owned(),
                    owner: owner.to_owned(),
                    total,
                    image_url,
                });
            }
        }
        next = page
            .get("next")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .map(str::to_owned);
    }
    Ok(playlists)
}

#[cfg(test)]
mod tests {
    use super::collect_playlists;
    use serde_json::json;

    #[test]
    fn reads_all_pages_and_legacy_totals() -> Result<(), String> {
        let playlists = collect_playlists(|url| {
            if url.ends_with("limit=50") {
                Ok(
                    json!({"items": [{"id": "one", "name": "First", "owner": {"display_name": "Alex"}, "items": {"total": 3}, "images": [{"url": "https://example.test/art"}]}], "next": "https://api.spotify.com/v1/me/playlists?offset=50"}),
                )
            } else {
                Ok(
                    json!({"items": [{"id": "two", "owner": {"id": "owner"}, "tracks": {"total": 4}}], "next": null}),
                )
            }
        })?;
        assert_eq!(playlists.len(), 3);
        assert_eq!(
            playlists.first().map(|item| item.uri.as_str()),
            Some("spotify:liked")
        );
        assert_eq!(playlists.get(1).map(|item| item.total), Some(Some(3)));
        assert_eq!(
            playlists.get(1).map(|item| item.owner.as_str()),
            Some("Alex")
        );
        assert_eq!(playlists.get(2).map(|item| item.name.as_str()), Some("two"));
        assert_eq!(playlists.get(2).map(|item| item.total), Some(Some(4)));
        Ok(())
    }

    #[test]
    fn rejects_a_page_that_could_receive_the_access_token() {
        let result =
            collect_playlists(|_| Ok(json!({"items": [], "next": "https://example.test/steal"})));
        assert!(result.is_err());
    }
}
