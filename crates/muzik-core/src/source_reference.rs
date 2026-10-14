//! Source references shared by registration and provider readers.

use crate::{Error, Result};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpotifyReference {
    Liked,
    Playlist(String),
    Album(String),
}

impl SpotifyReference {
    /// # Errors
    /// Returns an error for unsupported references or invalid IDs.
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        let invalid =
            || Error::from("Expected a Spotify playlist, album, or Liked Songs reference");
        let reference = if matches!(
            input.to_ascii_lowercase().trim_end_matches('/'),
            "liked" | "liked songs" | "spotify:liked"
        ) {
            Self::Liked
        } else if let Some(id) = input.strip_prefix("spotify:playlist:") {
            Self::Playlist(id.to_owned())
        } else if let Some(id) = input.strip_prefix("spotify:album:") {
            Self::Album(id.to_owned())
        } else {
            let link = Url::parse(input).map_err(|_| invalid())?;
            if link.scheme() != "https" || link.host_str() != Some("open.spotify.com") {
                return Err(invalid());
            }
            let path: Vec<_> = link
                .path()
                .trim_end_matches('/')
                .split('/')
                .skip(1)
                .collect();
            let path = match path.as_slice() {
                [locale, rest @ ..] if locale.starts_with("intl-") => rest,
                path => path,
            };
            match path {
                ["collection", "tracks"] => Self::Liked,
                ["playlist", id] => Self::Playlist((*id).to_owned()),
                ["album", id] => Self::Album((*id).to_owned()),
                _ => return Err(invalid()),
            }
        };
        if let Self::Playlist(id) | Self::Album(id) = &reference
            && (id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        {
            return Err(invalid());
        }
        Ok(reference)
    }
}

/// # Errors
/// Returns an error unless the URL belongs to `YouTube` and contains a playlist ID.
pub fn youtube_playlist(input: &str) -> Result<String> {
    let invalid = || Error::from("Expected a YouTube playlist URL");
    let url = Url::parse(input.trim()).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(
            url.host_str(),
            Some(
                "youtube.com"
                    | "www.youtube.com"
                    | "m.youtube.com"
                    | "music.youtube.com"
                    | "youtu.be"
            )
        )
    {
        return Err(invalid());
    }
    let id = url
        .query_pairs()
        .find_map(|(key, value)| (key == "list").then(|| value.into_owned()))
        .ok_or_else(invalid)?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid());
    }
    Ok(id)
}
