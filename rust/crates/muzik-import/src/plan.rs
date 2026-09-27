//! Album groups, MusicBrainz candidates, and duplicate checks before import.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use muzik_core::ReleaseCandidate;
use muzik_library::{Library, SqlValue};
use muzik_match::{
    Assignment, MatchAlbum, MatchConfig, MatchItem, MatchTrack, Recommendation, rank_albums,
};
use muzik_metadata::{MetadataClient, ReleaseSearch, ReleaseSearchHit};
use muzik_tags::TagData;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("cannot read import path: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot read audio tags: {0}")]
    Tags(#[from] muzik_tags::TagsError),
    #[error("cannot read library: {0}")]
    Library(#[from] muzik_library::Error),
    #[error("MusicBrainz request failed: {0}")]
    Metadata(#[from] muzik_metadata::Error),
    #[error("cannot score album: {0}")]
    Match(#[from] muzik_match::Error),
    #[error("no audio files were found")]
    NoAudio,
}

pub trait ReleaseProvider {
    fn search_releases(
        &self,
        criteria: &ReleaseSearch,
        limit: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error>;
    fn lookup_release(&self, id: &str) -> Result<ReleaseCandidate, muzik_metadata::Error>;
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
    pub source_dir: PathBuf,
    pub items: Vec<PlanItem>,
    pub candidates: Vec<PlannedCandidate>,
    pub recommendation: Recommendation,
    pub duplicates: Vec<Duplicate>,
}

#[derive(Clone, Debug)]
pub struct ImportPlan {
    pub albums: Vec<AlbumPlan>,
}

pub struct ImportPlanner<'a, P: ReleaseProvider> {
    pub provider: &'a P,
    pub library: &'a Library,
    pub match_config: &'a MatchConfig,
    pub search_limit: u8,
}

impl<P: ReleaseProvider> ImportPlanner<'_, P> {
    pub fn plan(&self, paths: &[PathBuf]) -> Result<ImportPlan, ImportError> {
        let groups = group_audio_paths(paths)?;
        if groups.is_empty() {
            return Err(ImportError::NoAudio);
        }
        let library_albums = self.library.albums()?;
        let library_items = self.library.items()?;
        let mut albums = Vec::new();
        for (source_dir, paths) in groups {
            let mut items = Vec::new();
            for source in paths {
                let tags = muzik_tags::read(&source, &[])?;
                let match_item = match_item(&tags);
                let source_id = read_source_id(&source)?;
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
            if !title.is_empty() {
                let criteria = ReleaseSearch {
                    release: title.to_owned(),
                    artist: (!artist.is_empty()).then(|| artist.to_owned()),
                    various_artists,
                    tracks: Some(items.len() as u32),
                    ..ReleaseSearch::default()
                };
                let hits = self
                    .provider
                    .search_releases(&criteria, self.search_limit)?;
                for hit in hits {
                    let release = self.provider.lookup_release(&hit.id.0)?;
                    if !releases
                        .iter()
                        .any(|known: &ReleaseCandidate| known.id == release.id)
                    {
                        releases.push(release);
                    }
                }
            }
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
            let duplicates = find_duplicates(&items, &releases, &library_albums, &library_items);
            tracing::debug!(path = %source_dir.display(), tracks = items.len(), candidates = releases.len(), duplicates = duplicates.len(), "planned album import");
            albums.push(AlbumPlan {
                source_dir,
                items,
                candidates,
                recommendation: ranked.recommendation,
                duplicates,
            });
        }
        Ok(ImportPlan { albums })
    }
}

fn group_audio_paths(paths: &[PathBuf]) -> Result<BTreeMap<PathBuf, Vec<PathBuf>>, std::io::Error> {
    fn visit(path: &Path, found: &mut BTreeSet<PathBuf>) -> Result<(), std::io::Error> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                visit(&entry?.path(), found)?;
            }
        } else if metadata.is_file() && is_audio(path) {
            found.insert(path.canonicalize()?);
        }
        Ok(())
    }
    let mut found = BTreeSet::new();
    for path in paths {
        visit(path, &mut found)?;
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
            ["mp3", "flac", "m4a", "mp4", "opus", "ogg"]
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

fn read_source_id(path: &Path) -> Result<Option<String>, ImportError> {
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
        if let Some(value) = data.get("source_id").and_then(|value| value.as_str()) {
            return Ok(Some(value.to_owned()));
        }
    }
    Ok(None)
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
