//! Ordered playlist acquisition with a durable checkpoint after each item.

use crate::{
    AudioProcessingPlan, AudioProcessingResult, AudioSource, Error, WorkflowEvent,
    WorkflowOperations, WorkflowOptions, WorkflowRequest, check_cancelled, find_audio_inputs,
    process_audio_plan_with_events,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
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

/// Run an ordered YouTube playlist. A failed item does not stop later items.
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
        operations
            .youtube_playlist_video_ids(url)
            .map_err(Error::Operation)?
    };
    if ids.is_empty() && !options.dry_run {
        return Err(Error::Operation(
            "playlist contains no available videos".into(),
        ));
    }
    let mut checkpoint = Checkpoint::load(&checkpoint_path(request, "youtube", playlist_id))?;
    let mut result = empty_result();
    for id in ids {
        check_cancelled(cancelled)?;
        let video_url = format!("https://www.youtube.com/watch?v={id}");
        let item = (|| -> Result<(), Error> {
            if !options.force && checkpoint.is_complete(&id, options.no_organize) {
                return Ok(());
            }
            let files = if !options.force {
                let saved = checkpoint.cached_files(&id)?;
                if saved.is_empty() {
                    find_audio_by_id(&request.output, &id)?
                } else {
                    saved
                }
            } else {
                Vec::new()
            };
            let files = if files.is_empty() {
                on_event(WorkflowEvent::AcquisitionStarted);
                let acquired = if options.audio_source == AudioSource::Soulseek {
                    match operations.acquire_soulseek(&video_url) {
                        Ok(files) if !files.is_empty() => files,
                        Ok(_) | Err(_) if options.fallback == crate::AudioFallback::Youtube => {
                            operations
                                .download_youtube(&video_url, &request.output, options.force)
                                .map_err(Error::Operation)?
                        }
                        Ok(files) => files,
                        Err(error) => return Err(Error::Operation(error)),
                    }
                } else {
                    operations
                        .download_youtube(&video_url, &request.output, options.force)
                        .map_err(Error::Operation)?
                };
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
    if !options.dry_run
        && (options.audio_source == AudioSource::Youtube
            || options.audio_source == AudioSource::Auto && !operations.soulseek_ready())
    {
        return Err(Error::Operation(
            "Spotify exports need Soulseek audio. Select Soulseek or Auto with Soulseek ready."
                .into(),
        ));
    }
    let mut checkpoint =
        Checkpoint::load(&checkpoint_path(request, "spotify", &playlist.source_id))?;
    let mut result = empty_result();
    let mut occurrences = HashMap::<String, usize>::new();
    for track in playlist.tracks {
        check_cancelled(cancelled)?;
        let occurrence = occurrences.entry(track.source_id.clone()).or_default();
        let id = format!("{}#{}", track.source_id, *occurrence);
        *occurrence += 1;
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
            files = operations
                .acquire_spotify_track(&track)
                .map_err(Error::Operation)?;
            on_event(WorkflowEvent::AcquisitionCompleted {
                files: files.clone(),
            });
        }
        let files = find_audio_inputs(&files)?;
        if files.is_empty() {
            return Err(Error::Operation(format!(
                "no Soulseek audio was acquired for {}",
                track.title
            )));
        }
        checkpoint.save_files(&id, &files)?;
        let processed = process_item(request, options, operations, cancelled, &files, on_event)?;
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

fn empty_result() -> PlaylistRunResult {
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

fn find_audio_by_id(directory: &Path, id: &str) -> Result<Vec<PathBuf>, Error> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    Ok(find_audio_inputs(&[directory.to_path_buf()])?
        .into_iter()
        .filter(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.contains(&format!("[{id}]")))
        })
        .collect())
}

fn checkpoint_path(request: &WorkflowRequest, source: &str, id: &str) -> PathBuf {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    request
        .output
        .join(".playlist-state")
        .join(format!("{source}_{hash:016x}.json"))
}

struct Checkpoint {
    path: PathBuf,
    entries: serde_json::Map<String, Value>,
}

impl Checkpoint {
    fn load(path: &Path) -> Result<Self, Error> {
        let entries = if path.exists() {
            let data: Value = serde_json::from_slice(&fs::read(path)?)
                .map_err(|error| Error::Operation(format!("invalid playlist state: {error}")))?;
            data.get("entries")
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| Error::Operation("playlist state has no entries object".into()))?
        } else {
            serde_json::Map::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            entries,
        })
    }

    fn is_complete(&self, id: &str, no_organize: bool) -> bool {
        let status = self
            .entries
            .get(id)
            .and_then(|entry| entry.get("status"))
            .and_then(Value::as_str);
        status == Some("complete") || no_organize && status == Some("processed")
    }

    fn cached_files(&self, id: &str) -> Result<Vec<PathBuf>, Error> {
        let files = self
            .entries
            .get(id)
            .and_then(|entry| entry.get("files"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        find_audio_inputs(&files)
    }

    fn save_files(&mut self, id: &str, files: &[PathBuf]) -> Result<(), Error> {
        self.entries.insert(
            id.to_owned(),
            json!({"status": "downloaded", "files": files}),
        );
        self.save()
    }

    fn complete(&mut self, id: &str, no_organize: bool) -> Result<(), Error> {
        if let Some(entry) = self.entries.get_mut(id) {
            entry["status"] = json!(if no_organize { "processed" } else { "complete" });
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
            serde_json::to_vec(&json!({"version": 1, "entries": self.entries}))
                .map_err(|error| Error::Operation(error.to_string()))?,
        )?;
        fs::rename(temp, &self.path)?;
        Ok(())
    }
}

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
    let value: Value =
        serde_json::from_slice(data).map_err(|error| Error::Operation(error.to_string()))?;
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
            None => index + 1,
        };
        let source_id = entry
            .get("source_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("spotify:local:{position}"));
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
    let mut reader =
        csv::Reader::from_path(path).map_err(|error| Error::Operation(error.to_string()))?;
    let headers = reader
        .headers()
        .map_err(|error| Error::Operation(error.to_string()))?
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
        let row = row.map_err(|error| Error::Operation(error.to_string()))?;
        let field = |name: &str| {
            headers
                .iter()
                .position(|header| header == name)
                .and_then(|position| row.get(position))
                .map(str::trim)
                .unwrap_or("")
        };
        if !field("episode").is_empty() || field("type").eq_ignore_ascii_case("episode") {
            return Err(Error::Operation(format!(
                "Spotify CSV row {} is an episode",
                index + 2
            )));
        }
        let title = field("track_name");
        let artist = field("artist_name");
        if title.is_empty() || artist.is_empty() {
            return Err(Error::Operation(format!(
                "Spotify CSV row {} needs track_name and artist_name",
                index + 2
            )));
        }
        let position = if field("position").is_empty() {
            index + 1
        } else {
            field("position").parse::<usize>().map_err(|_| {
                Error::Operation(format!("invalid position in Spotify CSV row {}", index + 2))
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
