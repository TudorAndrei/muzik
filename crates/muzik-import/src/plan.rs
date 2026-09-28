//! Album groups, MusicBrainz candidates, and duplicate checks before import.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub use crate::Error as ImportError;
use crate::history::IncrementalHistory;
use muzik_core::{ReleaseCandidate, TrackCandidate};
use muzik_library::{Library, SqlValue};
use muzik_match::{
    Assignment, MatchAlbum, MatchConfig, MatchItem, MatchTrack, Recommendation, rank_albums,
};
use muzik_metadata::{MetadataClient, ReleaseSearch, ReleaseSearchHit};
use muzik_tags::TagData;

pub trait ReleaseProvider {
    fn search_releases(
        &self,
        criteria: &ReleaseSearch,
        limit: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error>;
    fn lookup_release(&self, id: &str) -> Result<ReleaseCandidate, muzik_metadata::Error>;
    fn lookup_recording(&self, id: &str) -> Result<TrackCandidate, muzik_metadata::Error>;
}

impl ReleaseProvider for MetadataClient {
    fn search_releases(
        &self,
        criteria: &ReleaseSearch,
        limit: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
        self.search_releases(criteria, limit)
    }

    fn lookup_release(&self, id: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        self.lookup_release(id)
    }

    fn lookup_recording(&self, id: &str) -> Result<TrackCandidate, muzik_metadata::Error> {
        self.lookup_recording(id)
    }
}

#[derive(Clone, Debug)]
pub struct PlanItem {
    pub source: PathBuf,
    pub tags: TagData,
    pub match_item: MatchItem,
    pub source_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PlannedCandidate {
    pub release: ReleaseCandidate,
    pub distance: f64,
    pub assignment: Assignment,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DuplicateReason {
    ReleaseId,
    SourceId,
    ArtistAndAlbum,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Duplicate {
    pub album_id: i64,
    pub reason: DuplicateReason,
}

#[derive(Clone, Debug)]
pub struct AlbumPlan {
    pub kind: ImportMode,
    pub source_dir: PathBuf,
    pub items: Vec<PlanItem>,
    pub candidates: Vec<PlannedCandidate>,
    pub recommendation: Recommendation,
    pub duplicates: Vec<Duplicate>,
}

#[derive(Clone, Debug)]
pub struct ImportPlan {
    pub albums: Vec<AlbumPlan>,
    pub history: Option<IncrementalHistory>,
    pub incremental_skip_later: bool,
    pub skipped_incremental: usize,
}

pub struct PlanOptions {
    pub autotag: bool,
    pub history: Option<IncrementalHistory>,
    pub incremental_skip_later: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            autotag: true,
            history: None,
            incremental_skip_later: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportMode {
    Album,
    Singleton,
}

pub struct ImportPlanner<'a, P: ReleaseProvider> {
    pub provider: &'a P,
    pub library: &'a Library,
    pub match_config: &'a MatchConfig,
    pub search_limit: u8,
}

impl<P: ReleaseProvider> ImportPlanner<'_, P> {
    pub fn plan(&self, paths: &[PathBuf]) -> Result<ImportPlan, ImportError> {
        self.plan_with_options(paths, ImportMode::Album, PlanOptions::default())
    }

    /// Plan each audio file as an item without an album row.
    pub fn plan_singletons(&self, paths: &[PathBuf]) -> Result<ImportPlan, ImportError> {
        self.plan_with_options(paths, ImportMode::Singleton, PlanOptions::default())
    }

    pub fn plan_with_options(
        &self,
        paths: &[PathBuf],
        mode: ImportMode,
        options: PlanOptions,
    ) -> Result<ImportPlan, ImportError> {
        self.plan_with_options_and_cancel(paths, mode, options, &|| false)
    }

    pub fn plan_with_options_and_cancel(
        &self,
        paths: &[PathBuf],
        mode: ImportMode,
        options: PlanOptions,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ImportPlan, ImportError> {
        check_cancelled(cancelled)?;
        let grouped = group_audio_paths(paths, cancelled)?;
        let groups: Vec<_> = match mode {
            ImportMode::Album => grouped.into_iter().collect(),
            ImportMode::Singleton => grouped
                .into_iter()
                .flat_map(|(dir, paths)| {
                    paths.into_iter().map(move |path| (dir.clone(), vec![path]))
                })
                .collect(),
        };
        if groups.is_empty() {
            return Err(ImportError::NoAudio);
        }
        let mut skipped_incremental = 0;
        let library_albums = self.library.albums()?;
        let library_items = self.library.items()?;
        let mut albums = Vec::new();
        for (source_dir, paths) in groups {
            check_cancelled(cancelled)?;
            let history_key = if mode == ImportMode::Singleton {
                paths.clone()
            } else {
                vec![source_dir.clone()]
            };
            if let Some(history) = &options.history
                && history.contains(&history_key)?
            {
                skipped_incremental += 1;
                continue;
            }
            let mut items = Vec::new();
            for source in paths {
                check_cancelled(cancelled)?;
                let tags = muzik_tags::read(&source, &[])?;
                let sidecar = read_sidecar(&source)?;
                let mut match_item = match_item(&tags);
                match_item.length = muzik_tags::probe(&source)?.duration_seconds.unwrap_or(0.0);
                fill_from_sidecar(&mut match_item, sidecar.as_ref(), &source);
                let source_id = sidecar
                    .as_ref()
                    .and_then(|data| data.get("source_id"))
                    .and_then(|value| value.as_str())
                    .map(str::to_owned);
                items.push(PlanItem {
                    source,
                    tags,
                    match_item,
                    source_id,
                });
            }
            let current: Vec<_> = items.iter().map(|item| item.match_item.clone()).collect();
            let title = plurality(&current, |item| item.album.as_str());
            let album_artist = plurality(&current, |item| item.album_artist.as_str());
            let artist = if !album_artist.is_empty() {
                album_artist
            } else {
                plurality(&current, |item| item.artist.as_str())
            };
            let various_artists = current.iter().any(|item| item.compilation)
                || current.iter().any(|item| item.artist != current[0].artist)
                || ["", "various artists", "various", "va", "unknown"]
                    .contains(&artist.to_lowercase().as_str());
            let mut releases = Vec::new();
            if options.autotag && mode == ImportMode::Album && !title.is_empty() {
                let criteria = ReleaseSearch {
                    release: title.to_owned(),
                    artist: (!artist.is_empty()).then(|| artist.to_owned()),
                    various_artists,
                    tracks: Some(items.len() as u32),
                    ..ReleaseSearch::default()
                };
                let hits = match self.provider.search_releases(&criteria, self.search_limit) {
                    Ok(hits) => hits,
                    Err(error) => {
                        tracing::warn!(path = %source_dir.display(), %error, "MusicBrainz search failed; import can continue as-is");
                        Vec::new()
                    }
                };
                check_cancelled(cancelled)?;
                for hit in hits {
                    check_cancelled(cancelled)?;
                    let release = match self.provider.lookup_release(&hit.id.0) {
                        Ok(release) => release,
                        Err(error) => {
                            tracing::warn!(id = %hit.id.0, %error, "MusicBrainz release lookup failed; skip candidate");
                            continue;
                        }
                    };
                    check_cancelled(cancelled)?;
                    if !releases
                        .iter()
                        .any(|known: &ReleaseCandidate| known.id == release.id)
                    {
                        releases.push(release);
                    }
                }
            }
            check_cancelled(cancelled)?;
            let candidates_for_match: Vec<_> = releases.iter().map(match_album).collect();
            let ranked = rank_albums(&current, &candidates_for_match, self.match_config)?;
            let candidates = ranked
                .candidates
                .into_iter()
                .map(|ranked| {
                    Ok(PlannedCandidate {
                        release: releases[ranked.input_index].clone(),
                        distance: ranked.distance.score(self.match_config)?,
                        assignment: ranked.assignment,
                    })
                })
                .collect::<Result<Vec<_>, muzik_match::Error>>()?;
            let duplicates = if mode == ImportMode::Album {
                find_duplicates(&items, &releases, &library_albums, &library_items)
            } else {
                Vec::new()
            };
            tracing::debug!(path = %source_dir.display(), tracks = items.len(), candidates = releases.len(), duplicates = duplicates.len(), "planned album import");
            albums.push(AlbumPlan {
                kind: mode,
                source_dir,
                items,
                candidates,
                recommendation: ranked.recommendation,
                duplicates,
            });
        }
        check_cancelled(cancelled)?;
        Ok(ImportPlan {
            albums,
            history: options.history,
            incremental_skip_later: options.incremental_skip_later,
            skipped_incremental,
        })
    }
}

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<(), ImportError> {
    if cancelled() {
        Err(ImportError::Cancelled)
    } else {
        Ok(())
    }
}

fn group_audio_paths(
    paths: &[PathBuf],
    cancelled: &dyn Fn() -> bool,
) -> Result<BTreeMap<PathBuf, Vec<PathBuf>>, ImportError> {
    fn visit(
        path: &Path,
        found: &mut BTreeSet<PathBuf>,
        visited_dirs: &mut BTreeSet<PathBuf>,
        supplied: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), ImportError> {
        check_cancelled(cancelled)?;
        let metadata = fs::metadata(path)?;
        if metadata.is_dir() {
            if !visited_dirs.insert(path.canonicalize()?) {
                return Ok(());
            }
            for entry in fs::read_dir(path)? {
                visit(&entry?.path(), found, visited_dirs, false, cancelled)?;
            }
        } else if metadata.is_file() && is_audio(path) {
            found.insert(path.canonicalize()?);
        } else if metadata.is_file() && supplied {
            return Err(ImportError::UnsupportedAudio(path.to_owned()));
        }
        Ok(())
    }
    let mut found = BTreeSet::new();
    let mut visited_dirs = BTreeSet::new();
    for path in paths {
        visit(path, &mut found, &mut visited_dirs, true, cancelled)?;
    }
    let mut groups: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for path in found {
        if let Some(parent) = path.parent() {
            groups.entry(parent.to_owned()).or_default().push(path);
        }
    }
    Ok(groups)
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            [
                "mp3", "flac", "m4a", "mp4", "opus", "ogg", "wav", "aiff", "aif", "ape", "wv",
                "aac", "alac", "mpc", "spx",
            ]
            .contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn number(value: Option<&String>) -> u32 {
    value
        .and_then(|value| value.split('/').next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn match_item(tags: &TagData) -> MatchItem {
    let field = |name: &str| tags.fields.get(name).cloned().unwrap_or_default();
    MatchItem {
        title: field("title"),
        artist: field("artist"),
        album: field("album"),
        album_artist: field("albumartist"),
        media: field("media"),
        country: field("country"),
        label: field("label"),
        catalog_number: field("catalognum"),
        album_disambiguation: field("albumdisambig"),
        track: number(tags.fields.get("track")),
        disc: number(tags.fields.get("disc")),
        disc_total: number(tags.fields.get("disctotal")),
        year: field("date")
            .get(0..4)
            .and_then(|year| year.parse().ok())
            .unwrap_or(0),
        track_id: field("mb_trackid"),
        album_id: field("mb_albumid"),
        compilation: matches!(field("comp").as_str(), "1" | "true" | "True"),
        ..MatchItem::default()
    }
}

fn match_album(release: &ReleaseCandidate) -> MatchAlbum {
    MatchAlbum {
        title: release.title.clone(),
        artist: release.artist.clone(),
        album_id: Some(release.id.0.clone()),
        various_artists: release.is_various_artists,
        media: release.media.clone(),
        year: release.year,
        country: release.country.clone(),
        label: release.label.clone(),
        catalog_number: release.catalog_number.clone(),
        disambiguation: release.disambiguation.clone(),
        data_source: Some("MusicBrainz".to_owned()),
        tracks: release
            .tracks
            .iter()
            .map(|track| MatchTrack {
                title: track.title.clone(),
                artist: Some(track.artist.clone()),
                length: track.length_seconds,
                index: Some(track.index),
                medium: Some(track.medium),
                medium_index: Some(track.medium_index),
                track_id: track.recording_id.as_ref().map(|id| id.0.clone()),
                data_source: Some("MusicBrainz".to_owned()),
            })
            .collect(),
        ..MatchAlbum::default()
    }
}

fn plurality<'a>(items: &'a [MatchItem], get: impl Fn(&'a MatchItem) -> &'a str) -> &'a str {
    let mut best = "";
    let mut count = 0;
    for item in items {
        let value = get(item);
        let frequency = items.iter().filter(|other| get(other) == value).count();
        if frequency > count {
            best = value;
            count = frequency;
        }
    }
    best
}

fn read_sidecar(path: &Path) -> Result<Option<serde_json::Value>, ImportError> {
    for candidate in [
        path.with_extension("muzik.json"),
        path.parent().unwrap_or(Path::new(".")).join(".muzik.json"),
    ] {
        let contents = match fs::read_to_string(candidate) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let Ok(data) = serde_json::from_str::<serde_json::Value>(&contents) else {
            continue;
        };
        if data.is_object() {
            return Ok(Some(data));
        }
    }
    Ok(None)
}

fn sidecar_value<'a>(data: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    [
        data.get("resolved").and_then(|resolved| resolved.get(key)),
        data.get("candidate")
            .and_then(|candidate| candidate.get("metadata"))
            .and_then(|metadata| metadata.get(key)),
        data.get(key),
        data.get("candidate")
            .and_then(|candidate| candidate.get(key)),
    ]
    .into_iter()
    .flatten()
    .find(|value| match value {
        serde_json::Value::String(value) => !value.trim().is_empty(),
        serde_json::Value::Number(value) => value.as_i64() != Some(0),
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Null => false,
        _ => true,
    })
}

fn sidecar_field<'a>(data: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    sidecar_value(data, key).and_then(|value| value.as_str())
}

fn fill_from_sidecar(item: &mut MatchItem, sidecar: Option<&serde_json::Value>, path: &Path) {
    if let Some(data) = sidecar {
        if item.title.is_empty() {
            item.title = sidecar_field(data, "title")
                .or_else(|| sidecar_field(data, "track"))
                .unwrap_or_default()
                .to_owned();
        }
        if item.artist.is_empty() {
            item.artist = sidecar_field(data, "artist")
                .or_else(|| sidecar_field(data, "uploader"))
                .unwrap_or_default()
                .to_owned();
        }
        if item.album.is_empty() {
            item.album = sidecar_field(data, "album").unwrap_or_default().to_owned();
        }
        if item.year == 0 {
            item.year = sidecar_value(data, "year")
                .and_then(|year| {
                    year.as_str()
                        .and_then(|year| year.get(0..4))
                        .and_then(|year| year.parse().ok())
                        .or_else(|| year.as_i64().and_then(|year| i32::try_from(year).ok()))
                })
                .unwrap_or(0);
        }
        if item.length == 0.0 {
            item.length = data
                .get("resolved")
                .and_then(|value| value.get("duration"))
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
        }
    }
    if item.title.is_empty() {
        item.title = path
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    if item.album.is_empty() {
        item.album = path
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
}

fn field_text<'a>(fields: &'a muzik_library::Fields, key: &str) -> Option<&'a str> {
    match fields.get(key) {
        Some(SqlValue::Text(value)) => Some(value),
        _ => None,
    }
}

fn find_duplicates(
    items: &[PlanItem],
    releases: &[ReleaseCandidate],
    albums: &[muzik_library::Album],
    library_items: &[muzik_library::Item],
) -> Vec<Duplicate> {
    let ids: BTreeSet<_> = releases
        .iter()
        .map(|release| release.id.0.as_str())
        .chain(
            items
                .iter()
                .map(|item| item.match_item.album_id.as_str())
                .filter(|id| !id.is_empty()),
        )
        .collect();
    let sources: BTreeSet<_> = items
        .iter()
        .filter_map(|item| item.source_id.as_deref())
        .collect();
    let mut result = Vec::new();
    for album in albums {
        let reason = if field_text(&album.fields, "mb_albumid").is_some_and(|id| ids.contains(id)) {
            Some(DuplicateReason::ReleaseId)
        } else if library_items.iter().any(|item| {
            item.album_id() == Some(album.id)
                && item
                    .attribute("muzik_source_id")
                    .is_some_and(|value| match value {
                        SqlValue::Text(id) => sources.contains(id.as_str()),
                        _ => false,
                    })
        }) {
            Some(DuplicateReason::SourceId)
        } else if let Some(first) = items.first() {
            let artist = field_text(&album.fields, "albumartist")
                .or_else(|| field_text(&album.fields, "artist"));
            let title = field_text(&album.fields, "album");
            let local_artist = if first.match_item.album_artist.is_empty() {
                &first.match_item.artist
            } else {
                &first.match_item.album_artist
            };
            (artist.is_some_and(|value| value.eq_ignore_ascii_case(local_artist))
                && title.is_some_and(|value| value.eq_ignore_ascii_case(&first.match_item.album)))
            .then_some(DuplicateReason::ArtistAndAlbum)
        } else {
            None
        };
        if let Some(reason) = reason {
            result.push(Duplicate {
                album_id: album.id,
                reason,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_fills_missing_track_fields_in_source_order() {
        let sidecar = serde_json::json!({
            "source_id": "video-123",
            "title": "top title",
            "resolved": {"title": "resolved title", "album": "Night Lines", "year": 2021},
            "candidate": {"metadata": {"artist": "Mara Vale", "album": "other album"}}
        });
        let mut item = MatchItem::default();
        fill_from_sidecar(
            &mut item,
            Some(&sidecar),
            Path::new("/source/02 untagged.flac"),
        );
        assert_eq!(item.title, "resolved title");
        assert_eq!(item.artist, "Mara Vale");
        assert_eq!(item.album, "Night Lines");
        assert_eq!(item.year, 2021);
    }

    #[test]
    fn supplied_non_audio_file_reports_unsupported_format() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("notes.txt");
        fs::write(&path, "notes").unwrap();
        assert!(matches!(
            group_audio_paths(&[path], &|| false),
            Err(ImportError::UnsupportedAudio(_))
        ));
    }

    #[test]
    fn candidate_metadata_fills_track_when_resolved_is_empty() {
        let sidecar = serde_json::json!({
            "resolved": {"track": ""},
            "candidate": {"metadata": {
                "track": "Second Song", "artist": "Mara Vale",
                "album": "Night Lines", "year": 2020
            }}
        });
        let mut item = MatchItem::default();
        fill_from_sidecar(&mut item, Some(&sidecar), Path::new("/source/02.flac"));
        assert_eq!(item.title, "Second Song");
        assert_eq!(item.artist, "Mara Vale");
        assert_eq!(item.album, "Night Lines");
        assert_eq!(item.year, 2020);
    }
}
