use std::collections::BTreeMap;

use chrono::{Datelike, Local};
use muzik_core::BeetsConfig;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumString};
use thiserror::Error;

use crate::Recommendation;
use crate::string_dist;
use crate::string_distance::count_to_f64;

const VA_ARTISTS: &[&str] = &["", "various artists", "various", "va", "unknown"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum DistanceKey {
    DataSource,
    Artist,
    Album,
    Media,
    Mediums,
    Year,
    Country,
    Label,
    #[strum(serialize = "catalognum")]
    CatalogNumber,
    #[strum(serialize = "albumdisambig")]
    Disambiguation,
    AlbumId,
    Tracks,
    MissingTracks,
    UnmatchedTracks,
    TrackTitle,
    TrackArtist,
    TrackIndex,
    TrackLength,
    TrackId,
    Medium,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum AlbumField {
    Album,
    Artist,
    AlbumId,
    Media,
    Mediums,
    Year,
    OriginalYear,
    Country,
    Label,
    #[strum(serialize = "catalognum")]
    CatalogNumber,
    #[strum(serialize = "albumdisambig")]
    Disambiguation,
    DataSource,
    Tracks,
    #[strum(default)]
    Other(String),
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("match config has no numeric distance weight for {0}")]
    MissingWeight(DistanceKey),
    #[error("match config has no valid {0}")]
    InvalidConfig(&'static str),
    #[error("invalid preferred regex: {0}")]
    Regex(#[from] regex::Error),
    #[error("distance penalty for {key} is outside 0..=1: {value}")]
    InvalidPenalty { key: DistanceKey, value: f64 },
    #[error("album matching needs at least one local item")]
    NoItems,
    #[error("item or track index is outside the supplied album")]
    InvalidPair,
    #[error("track assignment failed: {0}")]
    Assignment(#[from] lsap::LSAPError),
}

/// The fields in a local beets Item that affect matching.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MatchItem {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default, alias = "albumartist")]
    pub album_artist: String,
    #[serde(default)]
    pub length: f64,
    #[serde(default)]
    pub track: u32,
    #[serde(default)]
    pub disc: u32,
    #[serde(default, alias = "disctotal")]
    pub disc_total: u32,
    #[serde(default)]
    pub year: i32,
    #[serde(default)]
    pub media: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub label: String,
    #[serde(default, alias = "catalognum")]
    pub catalog_number: String,
    #[serde(default, alias = "albumdisambig")]
    pub album_disambiguation: String,
    #[serde(default, alias = "mb_albumid")]
    pub album_id: String,
    #[serde(default, alias = "mb_trackid")]
    pub track_id: String,
    #[serde(default)]
    pub data_source: Option<String>,
    #[serde(default, alias = "comp")]
    pub compilation: bool,
}

/// The fields in a beets `TrackInfo` that affect matching.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MatchTrack {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub length: Option<f64>,
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default)]
    pub medium: Option<u32>,
    #[serde(default)]
    pub medium_index: Option<u32>,
    #[serde(default)]
    pub track_id: Option<String>,
    #[serde(default)]
    pub data_source: Option<String>,
}

/// The fields in a beets `AlbumInfo` that affect matching.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MatchAlbum {
    #[serde(default, alias = "album")]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album_id: Option<String>,
    #[serde(default)]
    pub tracks: Vec<MatchTrack>,
    #[serde(default, alias = "va")]
    pub various_artists: bool,
    #[serde(default)]
    pub media: Option<String>,
    #[serde(default)]
    pub mediums: Option<u32>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub original_year: Option<i32>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, alias = "catalognum")]
    pub catalog_number: Option<String>,
    #[serde(default, alias = "albumdisambig")]
    pub disambiguation: Option<String>,
    #[serde(default)]
    pub data_source: Option<String>,
}

/// Values from the layered beets `match:` config.
#[derive(Clone, Debug)]
pub struct MatchConfig {
    pub distance_weights: BTreeMap<DistanceKey, f64>,
    pub preferred_media: Vec<String>,
    pub preferred_countries: Vec<String>,
    pub prefer_original_year: bool,
    pub track_length_grace: f64,
    pub track_length_max: f64,
    pub strong_rec_thresh: f64,
    pub medium_rec_thresh: f64,
    pub rec_gap_thresh: f64,
    pub max_rec: BTreeMap<DistanceKey, Recommendation>,
    pub ignored: Vec<DistanceKey>,
    pub required: Vec<AlbumField>,
    pub metadata_source_count: usize,
    pub data_source_penalties: BTreeMap<String, f64>,
    pub current_year: i32,
}

impl MatchConfig {
    /// # Errors
    /// Returns an error when a required `match:` setting is missing or not valid.
    pub fn from_beets(config: &BeetsConfig) -> Result<Self, Error> {
        let at = |keys: &[&str]| config.get(keys);
        let weights = at(&["match", "distance_weights"])
            .and_then(|v| v.as_object())
            .ok_or(Error::InvalidConfig("match.distance_weights"))?;
        let distance_weights = weights
            .iter()
            .filter_map(|(key, value)| Some((key.parse().ok()?, value)))
            .map(|(key, value)| {
                value
                    .as_f64()
                    .map(|weight| (key, weight))
                    .ok_or(Error::InvalidConfig("match.distance_weights"))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let strings = |path: &[&str]| -> Vec<String> {
            match at(path) {
                Some(serde_json::Value::Array(values)) => values
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
                Some(serde_json::Value::String(value)) => {
                    value.split_whitespace().map(str::to_owned).collect()
                }
                _ => Vec::new(),
            }
        };
        let number = |key| {
            at(&["match", key])
                .and_then(serde_json::Value::as_f64)
                .ok_or(Error::InvalidConfig("match numeric setting"))
        };
        let max_rec = at(&["match", "max_rec"])
            .and_then(|v| v.as_object())
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_str()?)))
            .map(|(key, limit)| {
                limit
                    .parse()
                    .map(|limit| (key, limit))
                    .map_err(|_| Error::InvalidConfig("match.max_rec"))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        Ok(Self {
            distance_weights,
            preferred_media: strings(&["match", "preferred", "media"]),
            preferred_countries: strings(&["match", "preferred", "countries"]),
            prefer_original_year: at(&["match", "preferred", "original_year"])
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            track_length_grace: number("track_length_grace")?,
            track_length_max: number("track_length_max")?,
            strong_rec_thresh: number("strong_rec_thresh")?,
            medium_rec_thresh: number("medium_rec_thresh")?,
            rec_gap_thresh: number("rec_gap_thresh")?,
            max_rec,
            ignored: strings(&["match", "ignored"])
                .iter()
                .filter_map(|key| key.parse().ok())
                .collect(),
            required: strings(&["match", "required"])
                .iter()
                .filter_map(|field| field.parse().ok())
                .collect(),
            metadata_source_count: 1,
            data_source_penalties: BTreeMap::new(),
            current_year: Local::now().year(),
        })
    }

    fn source_penalty(&self, before: Option<&str>, after: Option<&str>) -> Option<f64> {
        if before == after
            || (before.unwrap_or_default().is_empty() && self.metadata_source_count <= 1)
        {
            return None;
        }
        Some(
            after
                .and_then(|name| self.data_source_penalties.get(name).copied())
                .unwrap_or(0.5),
        )
    }
}

/// Weighted penalties in insertion order, as in beets.
#[derive(Clone, Debug, Default)]
pub struct Distance {
    penalties: Vec<(DistanceKey, Vec<f64>)>,
    pub tracks: Vec<Self>,
}

impl Distance {
    pub(crate) fn add(&mut self, key: DistanceKey, value: f64) -> Result<(), Error> {
        if !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidPenalty { key, value });
        }
        if let Some((_, values)) = self.penalties.iter_mut().find(|(name, _)| *name == key) {
            values.push(value);
        } else {
            self.penalties.push((key, vec![value]));
        }
        Ok(())
    }

    pub(crate) fn add_string(
        &mut self,
        key: DistanceKey,
        left: Option<&str>,
        right: Option<&str>,
    ) -> Result<(), Error> {
        self.add(key, string_dist(left, right))
    }

    pub(crate) fn add_equality(&mut self, key: DistanceKey, equal: bool) -> Result<(), Error> {
        self.add(key, if equal { 0.0 } else { 1.0 })
    }

    pub(crate) fn add_number(
        &mut self,
        key: DistanceKey,
        left: u32,
        right: u32,
    ) -> Result<(), Error> {
        let difference = left.abs_diff(right);
        for _ in 0..difference.max(1) {
            self.add(key, if difference == 0 { 0.0 } else { 1.0 })?;
        }
        Ok(())
    }

    pub(crate) fn add_ratio(
        &mut self,
        key: DistanceKey,
        numerator: f64,
        denominator: f64,
    ) -> Result<(), Error> {
        let value = if denominator == 0.0 {
            0.0
        } else {
            numerator.min(denominator).max(0.0) / denominator
        };
        self.add(key, value)
    }

    pub(crate) fn raw_distance(&self, config: &MatchConfig) -> Result<f64, Error> {
        let mut total = 0.0;
        for (key, values) in &self.penalties {
            let weight = config
                .distance_weights
                .get(key)
                .ok_or(Error::MissingWeight(*key))?;
            total += values.iter().sum::<f64>() * weight;
        }
        Ok(total)
    }

    pub(crate) fn max_distance(&self, config: &MatchConfig) -> Result<f64, Error> {
        let mut total = 0.0;
        for (key, values) in &self.penalties {
            let weight = config
                .distance_weights
                .get(key)
                .ok_or(Error::MissingWeight(*key))?;
            total += count_to_f64(values.len()) * weight;
        }
        Ok(total)
    }

    /// # Errors
    /// Returns an error when the config has no weight for a penalty key.
    pub fn score(&self, config: &MatchConfig) -> Result<f64, Error> {
        let max = self.max_distance(config)?;
        if max == 0.0 {
            Ok(0.0)
        } else {
            Ok(self.raw_distance(config)? / max)
        }
    }

    pub(crate) fn weighted_penalty(
        &self,
        key: DistanceKey,
        config: &MatchConfig,
    ) -> Result<f64, Error> {
        let max = self.max_distance(config)?;
        if max == 0.0 {
            return Ok(0.0);
        }
        let values = self
            .penalties
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, values)| values)
            .ok_or(Error::MissingWeight(key))?;
        let weight = config
            .distance_weights
            .get(&key)
            .ok_or(Error::MissingWeight(key))?;
        Ok(values.iter().sum::<f64>() * weight / max)
    }

    pub(crate) fn active_keys(&self, config: &MatchConfig) -> Result<Vec<DistanceKey>, Error> {
        let mut result = Vec::new();
        for &(key, _) in &self.penalties {
            if self.weighted_penalty(key, config)? != 0.0 {
                result.push(key);
            }
        }
        Ok(result)
    }

    #[must_use]
    pub fn penalties(&self) -> &[(DistanceKey, Vec<f64>)] {
        &self.penalties
    }
}

/// # Errors
/// Returns an error when a penalty is outside 0..=1.
pub fn track_distance(
    item: &MatchItem,
    track: &MatchTrack,
    include_artist: bool,
    config: &MatchConfig,
) -> Result<Distance, Error> {
    let mut dist = Distance::default();
    if let Some(length) = track.length.filter(|length| *length != 0.0) {
        dist.add_ratio(
            DistanceKey::TrackLength,
            (item.length - length).abs() - config.track_length_grace,
            config.track_length_max,
        )?;
    }
    dist.add_string(
        DistanceKey::TrackTitle,
        Some(&item.title),
        Some(&track.title),
    )?;
    if include_artist
        && track
            .artist
            .as_deref()
            .is_some_and(|artist| !artist.is_empty())
        && !VA_ARTISTS.contains(&item.artist.to_lowercase().as_str())
    {
        dist.add_string(
            DistanceKey::TrackArtist,
            Some(&item.artist),
            track.artist.as_deref(),
        )?;
    }
    if track.index.is_some_and(|index| index != 0) && item.track != 0 {
        dist.add_equality(
            DistanceKey::TrackIndex,
            Some(item.track) == track.index || Some(item.track) == track.medium_index,
        )?;
    }
    if !item.track_id.is_empty() {
        dist.add_equality(
            DistanceKey::TrackId,
            Some(item.track_id.as_str()) == track.track_id.as_deref(),
        )?;
    }
    if track.medium.is_some_and(|medium| medium != 0) && item.disc != 0 {
        dist.add_equality(DistanceKey::Medium, Some(item.disc) == track.medium)?;
    }
    if let Some(penalty) =
        config.source_penalty(item.data_source.as_deref(), track.data_source.as_deref())
    {
        dist.add(DistanceKey::DataSource, penalty)?;
    }
    Ok(dist)
}

fn plurality<T: Eq + Clone + Default>(items: &[MatchItem], get: impl Fn(&MatchItem) -> T) -> T {
    let mut best = T::default();
    let mut best_count = 0;
    for item in items {
        let value = get(item);
        let count = items.iter().filter(|other| get(other) == value).count();
        if count > best_count {
            best = value;
            best_count = count;
        }
    }
    best
}

fn preferred_match(value: &str, patterns: &[String], media: bool) -> Result<f64, Error> {
    let unit = 1.0 / count_to_f64(patterns.len().max(1));
    for (index, pattern) in patterns.iter().enumerate() {
        let expression = if media {
            format!(r"(\d+x)?({pattern})")
        } else {
            pattern.clone()
        };
        let regex = RegexBuilder::new(&expression)
            .case_insensitive(true)
            .build()?;
        if regex.find(value).is_some_and(|found| found.start() == 0) {
            return Ok(count_to_f64(index) * unit);
        }
    }
    Ok(1.0)
}

/// # Errors
/// Returns an error when there are no items, a pair is outside the album, a weight or preferred regex is not valid, or a penalty is outside 0..=1.
pub fn album_distance(
    items: &[MatchItem],
    album: &MatchAlbum,
    pairs: &[(usize, usize)],
    config: &MatchConfig,
) -> Result<Distance, Error> {
    if items.is_empty() {
        return Err(Error::NoItems);
    }
    let mut dist = Distance::default();
    add_release_distance(&mut dist, items, album, config)?;
    add_year_distance(&mut dist, items, album, config)?;
    add_details_distance(&mut dist, items, album, config)?;
    add_track_distances(&mut dist, items, album, pairs, config)?;
    let data_source = plurality(items, |item| item.data_source.clone());
    if let Some(penalty) =
        config.source_penalty(data_source.as_deref(), album.data_source.as_deref())
    {
        dist.add(DistanceKey::DataSource, penalty)?;
    }
    tracing::debug!(score = dist.score(config)?, "scored album candidate");
    Ok(dist)
}

fn add_release_distance(
    dist: &mut Distance,
    items: &[MatchItem],
    album: &MatchAlbum,
    config: &MatchConfig,
) -> Result<(), Error> {
    let artist = plurality(items, |item| item.artist.clone());
    let album_artist = plurality(items, |item| item.album_artist.clone());
    let artist =
        if !album_artist.is_empty() && items.iter().all(|item| item.album_artist == album_artist) {
            album_artist
        } else {
            artist
        };
    if !album.various_artists {
        dist.add_string(DistanceKey::Artist, Some(&artist), Some(&album.artist))?;
    }
    dist.add_string(
        DistanceKey::Album,
        Some(&plurality(items, |item| item.album.clone())),
        Some(&album.title),
    )?;
    let media = plurality(items, |item| item.media.clone());
    if let Some(album_media) = album.media.as_deref().filter(|value| !value.is_empty()) {
        if !config.preferred_media.is_empty() {
            dist.add(
                DistanceKey::Media,
                preferred_match(album_media, &config.preferred_media, true)?,
            )?;
        } else if !media.is_empty() {
            dist.add_equality(DistanceKey::Media, album_media == media)?;
        }
    }
    let disc_total = plurality(items, |item| item.disc_total);
    if let Some(mediums) = album.mediums.filter(|mediums| *mediums != 0)
        && disc_total != 0
    {
        dist.add_number(DistanceKey::Mediums, disc_total, mediums)?;
    }
    Ok(())
}

fn add_year_distance(
    dist: &mut Distance,
    items: &[MatchItem],
    album: &MatchAlbum,
    config: &MatchConfig,
) -> Result<(), Error> {
    let year = plurality(items, |item| item.year);
    if let Some(album_year) = album.year.filter(|year| *year != 0) {
        if config.prefer_original_year {
            let original = album
                .original_year
                .filter(|year| *year != 0)
                .unwrap_or(1889);
            dist.add_ratio(
                DistanceKey::Year,
                (f64::from(album_year) - f64::from(original)).abs(),
                (f64::from(config.current_year) - f64::from(original)).abs(),
            )?;
        } else if year != 0 {
            if year == album_year || album.original_year == Some(year) {
                dist.add(DistanceKey::Year, 0.0)?;
            } else if let Some(original) = album.original_year.filter(|year| *year != 0) {
                dist.add_ratio(
                    DistanceKey::Year,
                    (f64::from(year) - f64::from(album_year)).abs(),
                    (f64::from(config.current_year) - f64::from(original)).abs(),
                )?;
            } else {
                dist.add(DistanceKey::Year, 1.0)?;
            }
        }
    }
    Ok(())
}

fn add_details_distance(
    dist: &mut Distance,
    items: &[MatchItem],
    album: &MatchAlbum,
    config: &MatchConfig,
) -> Result<(), Error> {
    let country = plurality(items, |item| item.country.clone());
    if let Some(album_country) = album.country.as_deref().filter(|value| !value.is_empty()) {
        if !config.preferred_countries.is_empty() {
            dist.add(
                DistanceKey::Country,
                preferred_match(album_country, &config.preferred_countries, false)?,
            )?;
        } else if !country.is_empty() {
            dist.add_string(DistanceKey::Country, Some(&country), Some(album_country))?;
        }
    }
    for (key, local, candidate) in [
        (
            DistanceKey::Label,
            plurality(items, |item| item.label.clone()),
            album.label.as_deref(),
        ),
        (
            DistanceKey::CatalogNumber,
            plurality(items, |item| item.catalog_number.clone()),
            album.catalog_number.as_deref(),
        ),
        (
            DistanceKey::Disambiguation,
            plurality(items, |item| item.album_disambiguation.clone()),
            album.disambiguation.as_deref(),
        ),
    ] {
        if !local.is_empty() && candidate.is_some_and(|value| !value.is_empty()) {
            dist.add_string(key, Some(&local), candidate)?;
        }
    }
    let album_id = plurality(items, |item| item.album_id.clone());
    if !album_id.is_empty() {
        dist.add_equality(
            DistanceKey::AlbumId,
            Some(album_id.as_str()) == album.album_id.as_deref(),
        )?;
    }
    Ok(())
}

fn add_track_distances(
    dist: &mut Distance,
    items: &[MatchItem],
    album: &MatchAlbum,
    pairs: &[(usize, usize)],
    config: &MatchConfig,
) -> Result<(), Error> {
    for &(item_index, track_index) in pairs {
        let (Some(item), Some(track)) = (items.get(item_index), album.tracks.get(track_index))
        else {
            return Err(Error::InvalidPair);
        };
        let track_dist = track_distance(item, track, album.various_artists, config)?;
        dist.add(DistanceKey::Tracks, track_dist.score(config)?)?;
        dist.tracks.push(track_dist);
    }
    for _ in pairs.len()..album.tracks.len() {
        dist.add(DistanceKey::MissingTracks, 1.0)?;
    }
    for _ in pairs.len()..items.len() {
        dist.add(DistanceKey::UnmatchedTracks, 1.0)?;
    }
    Ok(())
}
