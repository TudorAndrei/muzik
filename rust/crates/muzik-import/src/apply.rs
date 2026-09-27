//! Apply explicit album choices to files, tags, cover art, and the library.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::files::{self, Placement};
use crate::ftclean;
use crate::paths::{AlbumFields, PathFormats, PathKind, PathSanitizer, TemplateContext};
use crate::plan::{AlbumPlan, ImportPlan};
use muzik_core::BeetsConfig;
use muzik_library::{Fields as LibraryFields, Library, SqlValue};
use muzik_tags::TagData;

pub use crate::Error as ApplyError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatchDecision {
    Candidate(usize),
    AsIs,
    Skip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DuplicateDecision {
    /// Keep the existing album and also import the new album.
    Keep,
    /// Remove duplicate rows and send their old files to the trash.
    Replace,
    /// Do not import the new album.
    Skip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AlbumDecision {
    pub choice: MatchDecision,
    pub duplicate: Option<DuplicateDecision>,
}

pub struct ApplyOptions {
    pub library_root: PathBuf,
    pub placement: Placement,
    pub write_tags: bool,
    pub embed_art: bool,
    pub dry_run: bool,
    pub paths: PathFormats,
    pub sanitizer: PathSanitizer,
    pub aunique_keys: Vec<String>,
    pub aunique_disambiguators: Vec<String>,
    pub aunique_bracket: String,
}

impl ApplyOptions {
    pub fn from_beets(config: &BeetsConfig, library_root: PathBuf) -> Result<Self, ApplyError> {
        let text = |path: &[&str], fallback: &str| {
            config
                .get(path)
                .and_then(|value| value.as_str())
                .unwrap_or(fallback)
                .to_owned()
        };
        let flag = |name: &str| {
            config
                .get(&["import", name])
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        };
        let placement = if flag("move") {
            Placement::Move
        } else if flag("link") {
            Placement::Symlink
        } else if flag("hardlink") {
            Placement::Hardlink
        } else if flag("reflink") {
            Placement::Reflink
        } else {
            Placement::Copy
        };
        Ok(Self {
            library_root,
            placement,
            write_tags: flag("write"),
            embed_art: flag("write"),
            dry_run: flag("pretend"),
            paths: PathFormats {
                default: text(&["paths", "default"], "$albumartist/$album/$track $title"),
                compilation: text(&["paths", "comp"], "Compilations/$album/$track $title"),
                singleton: text(&["paths", "singleton"], "Non-Album/$artist/$title"),
            },
            sanitizer: PathSanitizer::new(&[])?,
            aunique_keys: text(&["aunique", "keys"], "albumartist album")
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            aunique_disambiguators: text(&["aunique", "disambiguators"], "year label catalognum")
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            aunique_bracket: text(&["aunique", "bracket"], "[]"),
        })
    }
}

#[derive(Debug, Default)]
pub struct ApplyResult {
    pub album_ids: Vec<i64>,
    pub item_ids: Vec<i64>,
    pub destinations: Vec<PathBuf>,
    pub skipped_albums: usize,
}

struct PreparedItem {
    source: PathBuf,
    destination: PathBuf,
    tags: TagData,
    source_id: Option<String>,
}

struct PreparedAlbum {
    items: Vec<PreparedItem>,
    album_fields: LibraryFields,
    cover: Option<(PathBuf, PathBuf)>,
    replace_ids: Vec<i64>,
    old_paths: Vec<PathBuf>,
}

pub fn apply(
    library: &mut Library,
    plan: &ImportPlan,
    decisions: &[AlbumDecision],
    options: &ApplyOptions,
) -> Result<ApplyResult, ApplyError> {
    if decisions.len() != plan.albums.len() {
        return Err(ApplyError::DecisionCount);
    }
    if matches!(options.placement, Placement::Symlink | Placement::Hardlink)
        && (options.write_tags || options.embed_art)
    {
        return Err(ApplyError::LinkedWrite);
    }
    let mut result = ApplyResult::default();
    let mut reserved = BTreeSet::new();
    for (album, decision) in plan.albums.iter().zip(decisions) {
        if decision.choice == MatchDecision::Skip
            || (!album.duplicates.is_empty() && decision.duplicate == Some(DuplicateDecision::Skip))
        {
            result.skipped_albums += 1;
            continue;
        }
        if !album.duplicates.is_empty() && decision.duplicate.is_none() {
            return Err(ApplyError::DuplicateDecision);
        }
        let prepared = prepare(library, album, *decision, options, &mut reserved)?;
        if options.dry_run {
            result
                .destinations
                .extend(prepared.items.into_iter().map(|item| item.destination));
            continue;
        }
        let mut created = Vec::new();
        let placed = (|| -> Result<(), ApplyError> {
            for item in &prepared.items {
                let mode = if options.placement == Placement::Move {
                    Placement::Copy
                } else {
                    options.placement
                };
                files::place(&item.source, &item.destination, mode)?;
                created.push(item.destination.clone());
                if options.write_tags {
                    muzik_tags::write(&item.destination, &item.tags)?;
                }
            }
            if let Some((source, destination)) = &prepared.cover {
                files::place(source, destination, Placement::Copy)?;
                created.push(destination.clone());
                if options.embed_art {
                    let bytes = fs::read(source)?;
                    let mime = if source
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
                    {
                        "image/png"
                    } else {
                        "image/jpeg"
                    };
                    for item in &prepared.items {
                        muzik_tags::embed_cover(&item.destination, &bytes, mime)?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = placed {
            rollback(&created);
            return Err(error);
        }
        let item_fields = prepared
            .items
            .iter()
            .map(|item| {
                muzik_tags::probe(&item.destination)
                    .map(|properties| item_fields(&item.tags, &item.destination, &properties))
            })
            .collect::<Result<Vec<_>, _>>();
        let item_fields = match item_fields {
            Ok(fields) => fields,
            Err(error) => {
                rollback(&created);
                return Err(error.into());
            }
        };
        let database = library.transaction(|writer| {
            for id in &prepared.replace_ids {
                writer.remove_album(*id)?;
            }
            let album_id = writer.insert_album(&prepared.album_fields, &LibraryFields::new())?;
            let mut item_ids = Vec::new();
            for (item, mut fields) in prepared.items.iter().zip(item_fields) {
                fields.insert("album_id".into(), SqlValue::Integer(album_id));
                let mut attributes = LibraryFields::new();
                if let Some(source_id) = &item.source_id {
                    attributes.insert("muzik_source_id".into(), SqlValue::Text(source_id.clone()));
                }
                item_ids.push(writer.insert_item(&fields, &attributes)?);
            }
            Ok((album_id, item_ids))
        });
        let (album_id, item_ids) = match database {
            Ok(value) => value,
            Err(error) => {
                rollback(&created);
                return Err(error.into());
            }
        };
        if options.placement == Placement::Move {
            for item in &prepared.items {
                fs::remove_file(&item.source)?;
            }
        }
        for path in &prepared.old_paths {
            if path.exists() {
                files::move_to_trash(path)?;
            }
        }
        result.album_ids.push(album_id);
        result.item_ids.extend(item_ids);
        result.destinations.extend(
            created
                .into_iter()
                .filter(|path| prepared.items.iter().any(|item| item.destination == *path)),
        );
    }
    Ok(result)
}

fn prepare(
    library: &Library,
    album: &AlbumPlan,
    decision: AlbumDecision,
    options: &ApplyOptions,
    reserved: &mut BTreeSet<PathBuf>,
) -> Result<PreparedAlbum, ApplyError> {
    let candidate = match decision.choice {
        MatchDecision::Candidate(index) => Some(
            album
                .candidates
                .get(index)
                .ok_or(ApplyError::CandidateIndex { index })?,
        ),
        MatchDecision::AsIs => None,
        MatchDecision::Skip => unreachable!(),
    };
    let replace_ids = if decision.duplicate == Some(DuplicateDecision::Replace) {
        album
            .duplicates
            .iter()
            .map(|entry| entry.album_id)
            .collect()
    } else {
        Vec::new()
    };
    let old_paths = if replace_ids.is_empty() {
        Vec::new()
    } else {
        let mut paths = Vec::new();
        for id in &replace_ids {
            for item in library.items_for_album(*id)? {
                if let Some(mut path) = item.fields.get("path").and_then(sql_path) {
                    if path.is_relative() {
                        path = options.library_root.join(path);
                    }
                    if !path.starts_with(&options.library_root) {
                        return Err(ApplyError::UnsafeReplacePath(path));
                    }
                    paths.push(path);
                }
            }
        }
        paths
    };
    let mut album_fields = LibraryFields::new();
    let mut prepared = Vec::new();
    let albums = library.albums()?;
    let next_id = albums.iter().map(|album| album.id).max().unwrap_or(0) + 1;
    let mut known: Vec<AlbumFields> = albums
        .iter()
        .map(|album| AlbumFields {
            id: album.id,
            fields: album
                .fields
                .iter()
                .filter_map(|(key, value)| match value {
                    SqlValue::Text(value) => Some((key.clone(), value.clone())),
                    SqlValue::Integer(value) => Some((key.clone(), value.to_string())),
                    _ => None,
                })
                .collect(),
        })
        .collect();
    for (index, item) in album.items.iter().enumerate() {
        let mut tags = item.tags.clone();
        for (name, value) in [
            ("title", &item.match_item.title),
            ("artist", &item.match_item.artist),
            ("album", &item.match_item.album),
            ("albumartist", &item.match_item.album_artist),
        ] {
            if !value.is_empty() {
                tags.fields
                    .entry(name.into())
                    .or_insert_with(|| value.clone());
            }
        }
        if tags.fields.get("albumartist").is_none_or(String::is_empty)
            && let Some(artist) = tags.fields.get("artist").cloned()
        {
            tags.fields.insert("albumartist".into(), artist);
        }
        if let Some(candidate) = candidate {
            let release = &candidate.release;
            for (name, value) in [
                ("album", Some(release.title.as_str())),
                ("albumartist", Some(release.artist.as_str())),
                ("mb_albumid", Some(release.id.0.as_str())),
                ("mb_releasegroupid", release.release_group_id.as_deref()),
                ("country", release.country.as_deref()),
                ("media", release.media.as_deref()),
                ("label", release.label.as_deref()),
                ("catalognum", release.catalog_number.as_deref()),
                ("albumdisambig", release.disambiguation.as_deref()),
            ] {
                if let Some(value) = value {
                    tags.fields.insert(name.into(), value.into());
                }
            }
            if let Some(year) = release.year {
                tags.fields.insert("date".into(), year.to_string());
            }
            if release.is_various_artists {
                tags.fields.insert("comp".into(), "1".into());
            }
            if let Some((_, track_index)) = candidate
                .assignment
                .pairs
                .iter()
                .find(|(source, _)| *source == index)
                && let Some(track) = release.tracks.get(*track_index)
            {
                tags.fields.insert("title".into(), track.title.clone());
                tags.fields.insert("artist".into(), track.artist.clone());
                tags.fields
                    .insert("track".into(), track.medium_index.to_string());
                tags.fields.insert("disc".into(), track.medium.to_string());
                if let Some(id) = &track.recording_id {
                    tags.fields.insert("mb_trackid".into(), id.0.clone());
                }
            }
        }
        let title = tags.fields.get("title").cloned().unwrap_or_default();
        let artist = tags.fields.get("artist").cloned().unwrap_or_default();
        let (title, artist) = ftclean::clean(&title, &artist);
        tags.fields.insert("title".into(), title);
        tags.fields.insert("artist".into(), artist);
        let path_fields = path_fields(&tags);
        if index == 0 {
            for key in [
                "album",
                "albumartist",
                "mb_albumid",
                "mb_releasegroupid",
                "year",
                "label",
                "catalognum",
                "country",
                "albumdisambig",
                "comp",
            ] {
                if let Some(value) = path_fields.get(key) {
                    album_fields.insert(key.into(), sql_scalar(key, value));
                }
            }
            known.push(AlbumFields {
                id: next_id,
                fields: path_fields.clone(),
            });
        }
        let kind = if tags.fields.get("comp").is_some_and(|value| value == "1") {
            PathKind::Compilation
        } else {
            PathKind::Album
        };
        let context = TemplateContext {
            fields: path_fields,
            album_id: Some(next_id),
            albums: known.clone(),
            aunique_keys: options.aunique_keys.clone(),
            aunique_disambiguators: options.aunique_disambiguators.clone(),
            aunique_bracket: options.aunique_bracket.clone(),
        };
        let extension = item
            .source
            .extension()
            .map(|value| format!(".{}", value.to_string_lossy()))
            .unwrap_or_default();
        let relative = options
            .paths
            .destination(kind, &context, &extension, &options.sanitizer)?;
        if relative.is_empty()
            || Path::new(&relative).is_absolute()
            || Path::new(&relative)
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(ApplyError::InvalidDestination);
        }
        let destination = options.library_root.join(relative);
        if destination.exists() || destination.symlink_metadata().is_ok() {
            return Err(ApplyError::DestinationExists(destination));
        }
        if !reserved.insert(destination.clone()) {
            return Err(ApplyError::DestinationCollision(destination));
        }
        prepared.push(PreparedItem {
            source: item.source.clone(),
            destination,
            tags,
            source_id: item.source_id.clone(),
        });
    }
    let cover = muzik_tags::find_cover(&album.source_dir).and_then(|source| {
        let parent = prepared.first()?.destination.parent()?;
        let name = source.file_name()?.to_owned();
        Some((source, parent.join(name)))
    });
    if let Some((_, destination)) = &cover {
        if destination.exists() || !reserved.insert(destination.clone()) {
            return Err(ApplyError::DestinationExists(destination.clone()));
        }
        album_fields.insert("artpath".into(), sql_path_value(destination));
    }
    album_fields.insert("added".into(), SqlValue::Real(now()));
    Ok(PreparedAlbum {
        items: prepared,
        album_fields,
        cover,
        replace_ids,
        old_paths,
    })
}

fn path_fields(tags: &TagData) -> BTreeMap<String, String> {
    let mut fields = tags.fields.clone();
    for key in ["track", "disc"] {
        if let Some(value) = fields.get_mut(key)
            && let Ok(number) = value.parse::<u32>()
        {
            *value = format!("{number:02}");
        }
    }
    if let Some(date) = fields.get("date") {
        fields.insert("year".into(), date.chars().take(4).collect());
    }
    fields
}

fn item_fields(
    tags: &TagData,
    path: &Path,
    properties: &muzik_tags::AudioProperties,
) -> LibraryFields {
    let mut fields = LibraryFields::new();
    for key in [
        "title",
        "artist",
        "album",
        "albumartist",
        "track",
        "tracktotal",
        "disc",
        "disctotal",
        "mb_trackid",
        "mb_releasetrackid",
        "mb_albumid",
        "mb_releasegroupid",
        "mb_artistid",
        "mb_albumartistid",
        "mb_workid",
        "label",
        "catalognum",
        "country",
        "media",
        "albumdisambig",
        "comp",
        "rg_track_gain",
        "rg_track_peak",
        "rg_album_gain",
        "rg_album_peak",
    ] {
        if let Some(value) = tags.fields.get(key) {
            fields.insert(key.into(), sql_scalar(key, value));
        }
    }
    if let Some(date) = tags.fields.get("date")
        && let Ok(year) = date.chars().take(4).collect::<String>().parse::<i64>()
    {
        fields.insert("year".into(), SqlValue::Integer(year));
    }
    fields.insert("path".into(), sql_path_value(path));
    fields.insert("format".into(), SqlValue::Text(properties.format.clone()));
    fields.insert("added".into(), SqlValue::Real(now()));
    if let Some(value) = properties.duration_seconds {
        fields.insert("length".into(), SqlValue::Real(value));
    }
    if let Some(value) = properties.bitrate_kbps {
        fields.insert("bitrate".into(), SqlValue::Integer(i64::from(value) * 1000));
    }
    if let Some(value) = properties.sample_rate_hz {
        fields.insert("samplerate".into(), SqlValue::Integer(i64::from(value)));
    }
    if let Some(value) = properties.bit_depth {
        fields.insert("bitdepth".into(), SqlValue::Integer(i64::from(value)));
    }
    if let Some(value) = properties.channels {
        fields.insert("channels".into(), SqlValue::Integer(i64::from(value)));
    }
    fields
}

fn sql_scalar(key: &str, value: &str) -> SqlValue {
    if ["track", "tracktotal", "disc", "disctotal", "year", "comp"].contains(&key) {
        SqlValue::Integer(value.parse().unwrap_or(0))
    } else if [
        "rg_track_gain",
        "rg_track_peak",
        "rg_album_gain",
        "rg_album_peak",
    ]
    .contains(&key)
    {
        SqlValue::Real(value.parse().unwrap_or(0.0))
    } else {
        SqlValue::Text(value.to_owned())
    }
}

fn sql_path_value(path: &Path) -> SqlValue {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        SqlValue::Blob(path.as_os_str().as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        SqlValue::Blob(path.to_string_lossy().as_bytes().to_vec())
    }
}

fn sql_path(value: &SqlValue) -> Option<PathBuf> {
    match value {
        SqlValue::Blob(bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                Some(std::ffi::OsString::from_vec(bytes.clone()).into())
            }
            #[cfg(not(unix))]
            {
                Some(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
            }
        }
        SqlValue::Text(value) => Some(PathBuf::from(value)),
        _ => None,
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

fn rollback(paths: &[PathBuf]) {
    for path in paths.iter().rev() {
        if let Err(error) = fs::remove_file(path) {
            tracing::warn!(path = %path.display(), %error, "cannot remove failed import file");
        }
    }
}
