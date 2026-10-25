//! Ordered playlist acquisition with a durable checkpoint after each item.

use crate::{
    AudioProcessingPlan, AudioProcessingResult, AudioSource, Error, WorkflowEvent,
    WorkflowOperations, WorkflowOptions, WorkflowRequest, check_cancelled, find_audio_inputs,
    process_audio_plan_with_events,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotifyTrack {
    pub source_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub position: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SpotifyTags {
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub track: Option<u64>,
    pub disc: Option<u64>,
    pub date: Option<String>,
}

/// # Errors
/// Returns an error when the tags cannot be written to the file.
pub fn write_spotify_tags(path: &Path, tags: &SpotifyTags) -> Result<(), Error> {
    let mut data = muzik_tags::TagData::default();
    let mut set = |name: &str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            data.fields.insert(name.into(), value);
        }
    };
    set("title", Some(tags.title.clone()));
    set("artist", Some(tags.artists.join(", ")));
    set("albumartist", tags.artists.first().cloned());
    set("album", tags.album.clone());
    set("track", tags.track.map(|track| track.to_string()));
    set("disc", tags.disc.map(|disc| disc.to_string()));
    set("date", tags.date.clone());
    Ok(muzik_tags::write(path, &data)?)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotifyPlaylist {
    pub source_id: String,
    pub title: String,
    pub tracks: Vec<SpotifyTrack>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaylistItemResult {
    pub id: String,
    pub completed: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaylistRunResult {
    pub processing: AudioProcessingResult,
    pub items: Vec<PlaylistItemResult>,
}

/// Run an ordered `YouTube` playlist. A failed item does not stop later items.
///
/// # Errors
/// Returns an error when the run is cancelled, the playlist is empty or cannot be listed, or the
/// checkpoint cannot be read.
pub fn run_youtube_playlist<O: WorkflowOperations>(
    request: &WorkflowRequest,
    options: &WorkflowOptions,
    operations: &mut O,
    cancelled: &AtomicBool,
    playlist_id: &str,
    url: &str,
    on_event: &mut dyn FnMut(WorkflowEvent),
) -> Result<PlaylistRunResult, Error> {
    check_cancelled(cancelled)?;
    let ids = if options.dry_run {
        Vec::new()
    } else {
        operations.youtube_playlist_video_ids(url)?
    };
    if ids.is_empty() && !options.dry_run {
        return Err(Error::Operation(
            "playlist contains no available videos".into(),
        ));
    }
    let mut checkpoint = Checkpoint::load(&checkpoint_path(
        request,
        PlaylistSource::Youtube,
        playlist_id,
    ))?;
    let mut result = empty_result();
    for id in ids {
        check_cancelled(cancelled)?;
        let video_url = format!("https://www.youtube.com/watch?v={id}");
        let item = (|| -> Result<(), Error> {
            if !options.force && checkpoint.is_complete(&id, options.no_organize) {
                return Ok(());
            }
            let files = if options.force {
                Vec::new()
            } else {
                let saved = checkpoint.cached_files(&id)?;
                if saved.is_empty() {
                    crate::find_audio_by_youtube_id(&request.output, &id)?
                } else {
                    saved
                }
            };
            let files = if files.is_empty() {
                on_event(WorkflowEvent::AcquisitionStarted);
                let acquired = crate::acquire_audio(
                    operations,
                    &video_url,
                    request,
                    options,
                    cancelled,
                    options.audio_source == AudioSource::Soulseek,
                )?;
                on_event(WorkflowEvent::AcquisitionCompleted {
                    files: acquired.clone(),
                });
                acquired
            } else {
                files
            };
            let files = find_audio_inputs(&files)?;
            if files.is_empty() {
                return Err(Error::NoAudio);
            }
            checkpoint.save_files(&id, &files)?;
            let processed =
                process_item(request, options, operations, cancelled, &files, on_event)?;
            merge(&mut result.processing, processed);
            checkpoint.complete(&id, options.no_organize)?;
            Ok(())
        })();
        match item {
            Ok(()) => result.items.push(PlaylistItemResult {
                id,
                completed: true,
                error: None,
            }),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(error) => result.items.push(PlaylistItemResult {
                id,
                completed: false,
                error: Some(error.to_string()),
            }),
        }
    }
    check_cancelled(cancelled)?;
    on_event(WorkflowEvent::Completed);
    Ok(result)
}

/// Read a Spotify metadata export and acquire each track in file order.
///
/// # Errors
/// Returns an error when the run is cancelled, the export or checkpoint is invalid, or a track
/// cannot be acquired, tagged, or processed.
pub fn run_spotify_export<O: WorkflowOperations>(
    request: &WorkflowRequest,
    options: &WorkflowOptions,
    operations: &mut O,
    cancelled: &AtomicBool,
    path: &Path,
    on_event: &mut dyn FnMut(WorkflowEvent),
) -> Result<PlaylistRunResult, Error> {
    check_cancelled(cancelled)?;
    let playlist = load_spotify_export(path)?;
    let mut checkpoint = Checkpoint::load(&checkpoint_path(
        request,
        PlaylistSource::Spotify,
        &playlist.source_id,
    ))?;
    let mut result = empty_result();
    let mut occurrences = HashMap::<String, usize>::new();
    for track in playlist.tracks {
        check_cancelled(cancelled)?;
        let occurrence = occurrences.entry(track.source_id.clone()).or_default();
        let id = format!("{}#{}", track.source_id, *occurrence);
        *occurrence = occurrence.saturating_add(1);
        if options.dry_run || !options.force && checkpoint.is_complete(&id, options.no_organize) {
            result.items.push(PlaylistItemResult {
                id,
                completed: true,
                error: None,
            });
            continue;
        }
        let mut files = if options.force {
            Vec::new()
        } else {
            checkpoint.cached_files(&id)?
        };
        if files.is_empty() {
            on_event(WorkflowEvent::AcquisitionStarted);
            files = operations.acquire_spotify_track(&track)?;
            on_event(WorkflowEvent::AcquisitionCompleted {
                files: files.clone(),
            });
        }
        let files = find_audio_inputs(&files)?;
        if files.is_empty() {
            return Err(Error::Operation(format!(
                "no audio was acquired for {}",
                track.title
            )));
        }
        checkpoint.save_files(&id, &files)?;
        let tags = SpotifyTags {
            title: track.title.clone(),
            artists: vec![track.artist.clone()],
            album: track.album.clone(),
            ..SpotifyTags::default()
        };
        for file in &files {
            write_spotify_tags(file, &tags)?;
        }
        let tagged = WorkflowOptions {
            interactive: false,
            ..options.clone()
        };
        let processed = process_item(request, &tagged, operations, cancelled, &files, on_event)?;
        merge(&mut result.processing, processed);
        checkpoint.complete(&id, options.no_organize)?;
        result.items.push(PlaylistItemResult {
            id,
            completed: true,
            error: None,
        });
    }
    check_cancelled(cancelled)?;
    on_event(WorkflowEvent::Completed);
    Ok(result)
}

fn process_item<O: WorkflowOperations>(
    request: &WorkflowRequest,
    options: &WorkflowOptions,
    operations: &mut O,
    cancelled: &AtomicBool,
    files: &[PathBuf],
    on_event: &mut dyn FnMut(WorkflowEvent),
) -> Result<AudioProcessingResult, Error> {
    process_audio_plan_with_events(
        files,
        &[],
        &request.splits,
        options,
        operations,
        cancelled,
        &mut |event| {
            if !matches!(event, WorkflowEvent::Completed) {
                on_event(event);
            }
        },
    )
}

const fn empty_result() -> PlaylistRunResult {
    PlaylistRunResult {
        processing: AudioProcessingResult {
            plan: AudioProcessingPlan {
                albums: Vec::new(),
                singles: Vec::new(),
                pre_split_dirs: Vec::new(),
            },
            split_dirs: Vec::new(),
            organize_targets: Vec::new(),
        },
        items: Vec::new(),
    }
}

fn merge(into: &mut AudioProcessingResult, from: AudioProcessingResult) {
    into.plan.albums.extend(from.plan.albums);
    into.plan.singles.extend(from.plan.singles);
    into.plan.pre_split_dirs.extend(from.plan.pre_split_dirs);
    into.split_dirs.extend(from.split_dirs);
    into.organize_targets.extend(from.organize_targets);
}

#[derive(Clone, Copy)]
enum PlaylistSource {
    Youtube,
    Spotify,
}

impl PlaylistSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Youtube => "youtube",
            Self::Spotify => "spotify",
        }
    }
}

fn checkpoint_path(request: &WorkflowRequest, source: PlaylistSource, id: &str) -> PathBuf {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    request
        .output
        .join(".playlist-state")
        .join(format!("{}_{hash:016x}.json", source.as_str()))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ItemStatus {
    Downloaded,
    Processed,
    Complete,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
struct CheckpointEntry {
    #[serde(default)]
    status: ItemStatus,
    #[serde(default)]
    files: Vec<PathBuf>,
}

#[derive(Deserialize)]
struct StoredCheckpoint {
    entries: BTreeMap<String, CheckpointEntry>,
}

#[derive(Serialize)]
struct SavedCheckpoint<'a> {
    version: u32,
    entries: &'a BTreeMap<String, CheckpointEntry>,
}

struct Checkpoint {
    path: PathBuf,
    entries: BTreeMap<String, CheckpointEntry>,
}

impl Checkpoint {
    fn load(path: &Path) -> Result<Self, Error> {
        let entries = if path.exists() {
            serde_json::from_slice::<StoredCheckpoint>(&fs::read(path)?)
                .map_err(|error| Error::Operation(format!("invalid playlist state: {error}")))?
                .entries
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            entries,
        })
    }

    fn is_complete(&self, id: &str, no_organize: bool) -> bool {
        let status = self.entries.get(id).map(|entry| entry.status);
        status == Some(ItemStatus::Complete) || no_organize && status == Some(ItemStatus::Processed)
    }

    fn cached_files(&self, id: &str) -> Result<Vec<PathBuf>, Error> {
        find_audio_inputs(
            self.entries
                .get(id)
                .map_or(&[][..], |entry| entry.files.as_slice()),
        )
    }

    fn save_files(&mut self, id: &str, files: &[PathBuf]) -> Result<(), Error> {
        self.entries.insert(
            id.to_owned(),
            CheckpointEntry {
                status: ItemStatus::Downloaded,
                files: files.to_vec(),
            },
        );
        self.save()
    }

    fn complete(&mut self, id: &str, no_organize: bool) -> Result<(), Error> {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.status = if no_organize {
                ItemStatus::Processed
            } else {
                ItemStatus::Complete
            };
        }
        self.save()
    }

    fn save(&self) -> Result<(), Error> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Operation("state path has no parent".into()))?;
        fs::create_dir_all(parent)?;
        let temp = self.path.with_extension("json.tmp");
        fs::write(
            &temp,
            serde_json::to_vec(&SavedCheckpoint {
                version: 1,
                entries: &self.entries,
            })?,
        )?;
        fs::rename(temp, &self.path)?;
        Ok(())
    }
}

/// # Errors
/// Returns an error when the file cannot be read, is not JSON or CSV, or has invalid tracks.
pub fn load_spotify_export(path: &Path) -> Result<SpotifyPlaylist, Error> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => parse_spotify_json(&fs::read(path)?),
        Some("csv") => parse_spotify_csv(path),
        _ => Err(Error::Operation(
            "Spotify export must be JSON or CSV".into(),
        )),
    }
}

fn parse_spotify_json(data: &[u8]) -> Result<SpotifyPlaylist, Error> {
    let value: Value = serde_json::from_slice(data)?;
    if value.get("version").and_then(Value::as_u64) != Some(1)
        || value.get("source").and_then(Value::as_str) != Some("spotify")
        || value.get("type").and_then(Value::as_str) != Some("playlist")
    {
        return Err(Error::Operation(
            "Spotify JSON requires version 1, source spotify, and type playlist".into(),
        ));
    }
    let source_id = required(&value, "id")?.to_owned();
    let title = required(&value, "title")?.to_owned();
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Operation("Spotify JSON has no entries array".into()))?;
    let mut tracks = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        if entry.get("type").and_then(Value::as_str) == Some("episode") {
            return Err(Error::Operation(
                "Spotify episodes are not supported".into(),
            ));
        }
        let title = required(entry, "title")?.to_owned();
        let artist = entry
            .get("artist")
            .and_then(Value::as_str)
            .or_else(|| {
                entry
                    .get("artists")
                    .and_then(Value::as_array)
                    .and_then(|artists| artists.first())
                    .and_then(Value::as_str)
            })
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::Operation(format!("Spotify track {title} has no artist")))?
            .to_owned();
        let position = match entry.get("index").or_else(|| entry.get("position")) {
            Some(value) => value
                .as_u64()
                .and_then(|number| usize::try_from(number).ok())
                .ok_or_else(|| Error::Operation("Spotify track position is invalid".into()))?,
            None => index.saturating_add(1),
        };
        let source_id = entry
            .get("source_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| format!("spotify:local:{position}"), str::to_owned);
        tracks.push(SpotifyTrack {
            source_id,
            title,
            artist,
            album: entry
                .get("album")
                .and_then(Value::as_str)
                .map(str::to_owned),
            position,
        });
    }
    validate_tracks(&tracks)?;
    Ok(SpotifyPlaylist {
        source_id,
        title,
        tracks,
    })
}

fn parse_spotify_csv(path: &Path) -> Result<SpotifyPlaylist, Error> {
    let mut reader = csv::Reader::from_path(path)?;
    let headers = reader
        .headers()?
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    if !headers.iter().any(|value| value == "track_name")
        || !headers.iter().any(|value| value == "artist_name")
    {
        return Err(Error::Operation(
            "Spotify CSV needs track_name and artist_name columns".into(),
        ));
    }
    let mut tracks = Vec::new();
    for (index, row) in reader.records().enumerate() {
        let row = row?;
        let field = |name: &str| {
            headers
                .iter()
                .position(|header| header == name)
                .and_then(|position| row.get(position))
                .map_or("", str::trim)
        };
        if !field("episode").is_empty() || field("type").eq_ignore_ascii_case("episode") {
            return Err(Error::Operation(format!(
                "Spotify CSV row {} is an episode",
                index.saturating_add(2)
            )));
        }
        let title = field("track_name");
        let artist = field("artist_name");
        if title.is_empty() || artist.is_empty() {
            return Err(Error::Operation(format!(
                "Spotify CSV row {} needs track_name and artist_name",
                index.saturating_add(2)
            )));
        }
        let position = if field("position").is_empty() {
            index.saturating_add(1)
        } else {
            field("position").parse::<usize>().map_err(|_| {
                Error::Operation(format!(
                    "invalid position in Spotify CSV row {}",
                    index.saturating_add(2)
                ))
            })?
        };
        let track_id = field("spotify_track_id");
        let uri = field("spotify_track_uri");
        let source_id = if !track_id.is_empty() {
            format!("spotify:track:{track_id}")
        } else if !uri.is_empty() {
            uri.to_owned()
        } else {
            format!("spotify:local:{position}")
        };
        tracks.push(SpotifyTrack {
            source_id,
            title: title.to_owned(),
            artist: artist.to_owned(),
            album: (!field("album_name").is_empty()).then(|| field("album_name").to_owned()),
            position,
        });
    }
    validate_tracks(&tracks)?;
    let source_id = path.canonicalize()?.to_string_lossy().into_owned();
    Ok(SpotifyPlaylist {
        source_id,
        title: "Spotify CSV import".into(),
        tracks,
    })
}

fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Operation(format!("Spotify export requires {key}")))
}

fn validate_tracks(tracks: &[SpotifyTrack]) -> Result<(), Error> {
    if tracks.is_empty() {
        return Err(Error::Operation(
            "Spotify export contains no music tracks".into(),
        ));
    }
    let mut seen = HashSet::new();
    if tracks
        .iter()
        .any(|track| track.position == 0 || !seen.insert(track.position))
    {
        return Err(Error::Operation(
            "Spotify track positions must be positive and unique".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checkpoint_keeps_the_stored_format() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("youtube_0.json");
        let song = temp.path().join("song.opus");
        fs::write(&song, b"audio").unwrap();
        fs::write(
            &path,
            json!({"version": 1, "entries": {
                "a": {"status": "complete", "files": []},
                "b": {"status": "processed", "files": []},
            }})
            .to_string(),
        )
        .unwrap();
        let mut checkpoint = Checkpoint::load(&path).unwrap();
        assert!(checkpoint.is_complete("a", false));
        assert!(!checkpoint.is_complete("b", false));
        assert!(checkpoint.is_complete("b", true));
        checkpoint
            .save_files("c", std::slice::from_ref(&song))
            .unwrap();
        assert_eq!(
            checkpoint.cached_files("c").unwrap(),
            std::slice::from_ref(&song)
        );
        let stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            stored,
            json!({"version": 1, "entries": {
                "a": {"status": "complete", "files": []},
                "b": {"status": "processed", "files": []},
                "c": {"status": "downloaded", "files": [song]},
            }})
        );
        checkpoint.complete("c", true).unwrap();
        let stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["entries"]["c"]["status"], "processed");
    }
}
