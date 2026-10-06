//! Read Spotify track metadata into the existing version 1 export format.

use super::{utc, Client, Result};
use chrono::{DateTime, Utc};
use rspotify_model::{
    AlbumId, FullAlbum, FullPlaylist, FullTrack, Id, Image, PlayableItem, PlaylistId, PlaylistItem,
    SavedTrack, SimplifiedArtist, SimplifiedTrack,
};
use serde_json::{json, Map, Value};
use std::path::Path;
use url::Url;

#[derive(Debug, PartialEq, Eq)]
enum Reference {
    Liked,
    Playlist(String),
    Album(String),
}

struct Track<'a> {
    name: &'a str,
    artists: &'a [SimplifiedArtist],
    id: Option<String>,
    url: Option<&'a String>,
    duration_ms: i64,
    disc_number: i32,
    track_number: u32,
    isrc: Option<&'a String>,
    is_local: bool,
}

struct Album<'a> {
    name: &'a str,
    release_date: Option<&'a str>,
    images: &'a [Image],
    artists: &'a [SimplifiedArtist],
}

pub fn load_playlist_document(config_path: &Path, token_path: &Path, uri: &str) -> Result<Value> {
    let reference = parse_reference(uri)?;
    let mut spotify = Client::connect(config_path, token_path)?;
    let mut entries = Vec::new();
    let mut push = |entry: Option<Map<String, Value>>| {
        if let Some(mut entry) = entry {
            entry.insert("index".into(), json!(entries.len() + 1));
            entries.push(Value::Object(entry));
        }
    };
    let (id, title, snapshot) = match reference {
        Reference::Liked => {
            for saved in spotify.pages::<SavedTrack>("me/tracks?limit=50")? {
                push(full_entry(&saved.track, Some(saved.added_at)));
            }
            ("liked".to_owned(), "Liked Songs".to_owned(), None)
        }
        Reference::Playlist(id) => {
            let playlist = PlaylistId::from_id(id.as_str())?;
            let details: FullPlaylist = spotify.get(&format!("playlists/{}", playlist.id()))?;
            let items = format!("playlists/{}/items?limit=50", playlist.id());
            for item in spotify.pages::<PlaylistItem>(&items)? {
                if let Some(PlayableItem::Track(track)) = &item.item {
                    push(full_entry(track, item.added_at));
                }
            }
            let title = Some(details.name).filter(|name| !name.is_empty());
            let snapshot = Some(details.snapshot_id).filter(|snapshot| !snapshot.is_empty());
            (id.clone(), title.unwrap_or(id), snapshot)
        }
        Reference::Album(id) => {
            let album_id = AlbumId::from_id(id.as_str())?;
            let album: FullAlbum = spotify.get(&format!("albums/{}", album_id.id()))?;
            let fields = Album {
                name: &album.name,
                release_date: Some(&album.release_date),
                images: &album.images,
                artists: &album.artists,
            };
            let tracks = format!("albums/{}/tracks?limit=50", album_id.id());
            for track in spotify.pages::<SimplifiedTrack>(&tracks)? {
                push(entry(&simple_track(&track), &fields, None));
            }
            let title = Some(album.name.clone()).filter(|name| !name.is_empty());
            (id.clone(), title.unwrap_or(id), None)
        }
    };
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

fn parse_reference(uri: &str) -> Result<Reference> {
    let input = uri.trim();
    let unreadable = || format!("muzik cannot read the Spotify reference {uri}");
    let reference = if matches!(
        input.to_ascii_lowercase().as_str(),
        "liked" | "liked songs" | "spotify:liked"
    ) {
        Reference::Liked
    } else if let Some(id) = input.strip_prefix("spotify:playlist:") {
        Reference::Playlist(id.to_owned())
    } else if let Some(id) = input.strip_prefix("spotify:album:") {
        Reference::Album(id.to_owned())
    } else {
        let link = Url::parse(input).map_err(|_| unreadable())?;
        if link.scheme() != "https" || link.host_str() != Some("open.spotify.com") {
            return Err(unreadable().into());
        }
        let path: Vec<_> = link.path_segments().into_iter().flatten().collect();
        let path = match path.as_slice() {
            [locale, rest @ ..] if locale.starts_with("intl-") => rest,
            path => path,
        };
        match path {
            ["collection", "tracks"] => Reference::Liked,
            ["playlist", id] => Reference::Playlist((*id).to_owned()),
            ["album", id] => Reference::Album((*id).to_owned()),
            _ => return Err(unreadable().into()),
        }
    };
    if let Reference::Playlist(id) | Reference::Album(id) = &reference {
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err("Spotify ID must contain only letters and numbers".into());
        }
    }
    Ok(reference)
}

fn full_entry(track: &FullTrack, added_at: Option<DateTime<Utc>>) -> Option<Map<String, Value>> {
    let album = Album {
        name: &track.album.name,
        release_date: track.album.release_date.as_deref(),
        images: &track.album.images,
        artists: &track.album.artists,
    };
    let fields = Track {
        name: &track.name,
        artists: &track.artists,
        id: track.id.as_ref().map(|id| id.id().to_owned()),
        url: track.external_urls.get("spotify"),
        duration_ms: track.duration.num_milliseconds(),
        disc_number: track.disc_number,
        track_number: track.track_number,
        isrc: track.external_ids.get("isrc"),
        is_local: track.is_local,
    };
    entry(&fields, &album, added_at)
}

fn simple_track(track: &SimplifiedTrack) -> Track<'_> {
    Track {
        name: &track.name,
        artists: &track.artists,
        id: track.id.as_ref().map(|id| id.id().to_owned()),
        url: track.external_urls.get("spotify"),
        duration_ms: track.duration.num_milliseconds(),
        disc_number: track.disc_number,
        track_number: track.track_number,
        isrc: None,
        is_local: track.is_local,
    }
}

fn entry(
    track: &Track<'_>,
    album: &Album<'_>,
    added_at: Option<DateTime<Utc>>,
) -> Option<Map<String, Value>> {
    let title = track.name.trim();
    if track.is_local || title.is_empty() {
        return None;
    }
    let artists = Some(names(track.artists))
        .filter(|names| !names.is_empty())
        .unwrap_or_else(|| names(album.artists));
    if artists.is_empty() {
        return None;
    }
    let mut entry = Map::new();
    entry.insert("title".into(), json!(title));
    entry.insert("artists".into(), json!(artists));
    entry.insert(
        "source_id".into(),
        json!(track.id.as_ref().map(|id| format!("spotify:track:{id}"))),
    );
    entry.insert(
        "source_url".into(),
        json!(track.url.cloned().or_else(|| track
            .id
            .as_ref()
            .map(|id| format!("https://open.spotify.com/track/{id}")))),
    );
    for (key, value) in [
        ("album", Some(album.name)),
        ("release_date", album.release_date),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            entry.insert(key.into(), json!(value));
        }
    }
    entry.insert("duration_ms".into(), json!(track.duration_ms));
    entry.insert("disc_number".into(), json!(track.disc_number));
    entry.insert("track_number".into(), json!(track.track_number));
    if let Some(isrc) = track.isrc.filter(|isrc| !isrc.is_empty()) {
        entry.insert("isrc".into(), json!(isrc));
    }
    if let Some(added_at) = added_at {
        entry.insert("added_at".into(), json!(utc(added_at)));
    }
    if let Some(image) = album.images.first() {
        entry.insert("source_metadata".into(), json!({"image": image.url}));
    }
    Some(entry)
}

fn names(artists: &[SimplifiedArtist]) -> Vec<String> {
    artists
        .iter()
        .map(|artist| artist.name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{entry, parse_reference, Album, Reference, Track};
    use chrono::{TimeZone, Utc};
    use rspotify_model::{Image, SimplifiedArtist};
    use std::collections::HashMap;

    fn artist(name: &str) -> SimplifiedArtist {
        SimplifiedArtist {
            external_urls: HashMap::new(),
            href: None,
            id: None,
            name: name.into(),
        }
    }

    fn track<'a>(name: &'a str, artists: &'a [SimplifiedArtist]) -> Track<'a> {
        Track {
            name,
            artists,
            id: Some("t1".into()),
            url: None,
            duration_ms: 120_000,
            disc_number: 1,
            track_number: 2,
            isrc: None,
            is_local: false,
        }
    }

    #[test]
    fn an_entry_uses_the_album_artist_and_image_when_the_track_has_none(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let album_artists = [artist("Alex")];
        let images = [Image {
            height: None,
            url: "https://example.test/art".into(),
            width: None,
        }];
        let album = Album {
            name: "Album",
            release_date: Some("2020-01-01"),
            images: &images,
            artists: &album_artists,
        };
        let added = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).single();
        let entry = entry(&track("One", &[]), &album, added).ok_or("no entry")?;
        assert_eq!(entry["artists"][0], "Alex");
        assert_eq!(entry["source_id"], "spotify:track:t1");
        assert_eq!(entry["source_url"], "https://open.spotify.com/track/t1");
        assert_eq!(
            entry["source_metadata"]["image"],
            "https://example.test/art"
        );
        assert_eq!(entry["added_at"], "2026-01-02T03:04:05Z");
        assert_eq!(entry["duration_ms"], 120_000);
        Ok(())
    }

    #[test]
    fn local_and_untitled_tracks_have_no_entry() {
        let artists = [artist("Bea")];
        let album = Album {
            name: "",
            release_date: None,
            images: &[],
            artists: &[],
        };
        let mut local = track("One", &artists);
        local.is_local = true;
        assert!(entry(&local, &album, None).is_none());
        assert!(entry(&track("  ", &artists), &album, None).is_none());
        assert!(entry(&track("One", &[]), &album, None).is_none());
    }

    #[test]
    fn references_accept_uris_links_and_liked_songs() -> crate::Result<()> {
        assert_eq!(parse_reference("Liked Songs")?, Reference::Liked);
        assert_eq!(
            parse_reference("https://open.spotify.com/collection/tracks")?,
            Reference::Liked
        );
        assert_eq!(
            parse_reference("https://open.spotify.com/intl-de/playlist/PL1?si=abc")?,
            Reference::Playlist("PL1".into())
        );
        assert_eq!(
            parse_reference("spotify:album:AL1")?,
            Reference::Album("AL1".into())
        );
        assert!(parse_reference("https://example.test/playlist/PL1").is_err());
        assert!(parse_reference("spotify:playlist:../x").is_err());
        Ok(())
    }
}
