//! Spotify playlist references shared by the CLI and desktop app.

use super::{Client, Result};
use rspotify_model::{Id, SimplifiedPlaylist};
use serde::Serialize;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlaylistRef {
    pub uri: String,
    pub name: String,
    pub owner: String,
    pub total: Option<u64>,
    pub image_url: Option<String>,
}

/// # Errors
/// Returns an error when muzik is not connected to Spotify or a Spotify request fails.
pub fn list_playlists(config_path: &Path, token_path: &Path) -> Result<Vec<PlaylistRef>> {
    let mut spotify = Client::connect(config_path, token_path)?;
    let mut playlists = vec![PlaylistRef {
        uri: "spotify:liked".into(),
        name: "Liked Songs".into(),
        owner: "you".into(),
        total: None,
        image_url: None,
    }];
    let saved = spotify.pages::<SimplifiedPlaylist>("me/playlists?limit=50")?;
    playlists.extend(saved.into_iter().map(PlaylistRef::from));
    Ok(playlists)
}

impl From<SimplifiedPlaylist> for PlaylistRef {
    fn from(playlist: SimplifiedPlaylist) -> Self {
        let id = playlist.id.id().to_owned();
        Self {
            uri: format!("spotify:playlist:{id}"),
            name: Some(playlist.name)
                .filter(|name| !name.is_empty())
                .unwrap_or(id),
            owner: playlist
                .owner
                .display_name
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| playlist.owner.id.id().to_owned()),
            total: Some(u64::from(playlist.items.total)),
            image_url: playlist.images.into_iter().next().map(|image| image.url),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PlaylistRef;
    use serde_json::json;

    #[test]
    fn a_playlist_reference_keeps_its_name_owner_total_and_image() {
        let playlist: rspotify_model::SimplifiedPlaylist = serde_json::from_value(json!({
            "collaborative": false,
            "external_urls": {},
            "href": "https://api.spotify.com/v1/playlists/one",
            "id": "one",
            "images": [{"url": "https://example.test/art", "height": null, "width": null}],
            "name": "",
            "owner": {"external_urls": {}, "href": "https://api.spotify.com/v1/users/owner", "id": "owner"},
            "public": true,
            "snapshot_id": "snap",
            "items": {"href": "https://api.spotify.com/v1/playlists/one/items", "total": 3}
        }))
        .unwrap();
        let reference = PlaylistRef::from(playlist);
        assert_eq!(reference.uri, "spotify:playlist:one");
        assert_eq!(reference.name, "one");
        assert_eq!(reference.owner, "owner");
        assert_eq!(reference.total, Some(3));
        assert_eq!(
            reference.image_url.as_deref(),
            Some("https://example.test/art")
        );
    }
}
