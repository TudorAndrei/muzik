use std::collections::BTreeMap;

use chrono::{Datelike, Local};
use muzik_core::BeetsConfig;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::string_dist;

const VA_ARTISTS: &[&str] = &["", "various artists", "various", "va", "unknown"];

#[derive(Debug, Error)]
pub enum Error {
    #[error("match config has no numeric distance weight for {0}")]
    MissingWeight(String),
    #[error("match config has no valid {0}")]
    InvalidConfig(&'static str),
    #[error("invalid preferred regex: {0}")]
    Regex(#[from] regex::Error),
    #[error("distance penalty for {key} is outside 0..=1: {value}")]
    InvalidPenalty { key: String, value: f64 },
    #[error("album matching needs at least one local item")]
    NoItems,
    #[error("item or track index is outside the supplied album")]
    InvalidPair,
    #[error("track assignment failed: {0}")]
    Assignment(&'static str),
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

/// The fields in a beets TrackInfo that affect matching.
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

/// The fields in a beets AlbumInfo that affect matching.
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
    pub distance_weights: BTreeMap<String, f64>,
    pub preferred_media: Vec<String>,
    pub preferred_countries: Vec<String>,
    pub prefer_original_year: bool,
    pub track_length_grace: f64,
    pub track_length_max: f64,
    pub strong_rec_thresh: f64,
    pub medium_rec_thresh: f64,
    pub rec_gap_thresh: f64,
    pub max_rec: BTreeMap<String, String>,
    pub ignored: Vec<String>,
    pub required: Vec<String>,
    pub metadata_source_count: usize,
    pub data_source_penalties: BTreeMap<String, f64>,
    pub current_year: i32,
}

impl MatchConfig {
    pub fn from_beets(config: &BeetsConfig) -> Result<Self, Error> {
        let at = |keys: &[&str]| config.get(keys);
        let weights = at(&["match", "distance_weights"])
            .and_then(|v| v.as_object())
            .ok_or(Error::InvalidConfig("match.distance_weights"))?;
        let distance_weights = weights
            .iter()
            .map(|(key, value)| {
                value
                    .as_f64()
                    .map(|weight| (key.clone(), weight))
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
                .and_then(|v| v.as_f64())
                .ok_or(Error::InvalidConfig("match numeric setting"))
        };
        let max_rec = at(&["match", "max_rec"])
            .and_then(|v| v.as_object())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            distance_weights,
            preferred_media: strings(&["match", "preferred", "media"]),
            preferred_countries: strings(&["match", "preferred", "countries"]),
            prefer_original_year: at(&["match", "preferred", "original_year"])
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            track_length_grace: number("track_length_grace")?,
            track_length_max: number("track_length_max")?,
            strong_rec_thresh: number("strong_rec_thresh")?,
            medium_rec_thresh: number("medium_rec_thresh")?,
            rec_gap_thresh: number("rec_gap_thresh")?,
            max_rec,
            ignored: strings(&["match", "ignored"]),
            required: strings(&["match", "required"]),
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
    penalties: Vec<(String, Vec<f64>)>,
    pub tracks: Vec<Distance>,
}

impl Distance {
    pub fn add(&mut self, key: &str, value: f64) -> Result<(), Error> {
        if !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidPenalty {
                key: key.to_owned(),
                value,
            });
        }
        if let Some((_, values)) = self.penalties.iter_mut().find(|(name, _)| name == key) {
            values.push(value);
        } else {
            self.penalties.push((key.to_owned(), vec![value]));
        }
        Ok(())
    }

    pub fn add_string(
        &mut self,
        key: &str,
        left: Option<&str>,
        right: Option<&str>,
    ) -> Result<(), Error> {
        self.add(key, string_dist(left, right))
    }

    pub fn add_equality(&mut self, key: &str, equal: bool) -> Result<(), Error> {
        self.add(key, if equal { 0.0 } else { 1.0 })
    }

    pub fn add_number(&mut self, key: &str, left: u32, right: u32) -> Result<(), Error> {
        let difference = left.abs_diff(right);
        for _ in 0..difference.max(1) {
            self.add(key, if difference == 0 { 0.0 } else { 1.0 })?;
        }
        Ok(())
    }

    pub fn add_ratio(&mut self, key: &str, numerator: f64, denominator: f64) -> Result<(), Error> {
        let value = if denominator == 0.0 {
            0.0
        } else {
            numerator.min(denominator).max(0.0) / denominator
        };
        self.add(key, value)
    }

    pub fn raw_distance(&self, config: &MatchConfig) -> Result<f64, Error> {
        let mut total = 0.0;
        for (key, values) in &self.penalties {
            let weight = config
                .distance_weights
                .get(key)
                .ok_or_else(|| Error::MissingWeight(key.clone()))?;
            total += values.iter().sum::<f64>() * weight;
        }
        Ok(total)
    }

    pub fn max_distance(&self, config: &MatchConfig) -> Result<f64, Error> {
        let mut total = 0.0;
        for (key, values) in &self.penalties {
            let weight = config
                .distance_weights
                .get(key)
                .ok_or_else(|| Error::MissingWeight(key.clone()))?;
            total += values.len() as f64 * weight;
        }
        Ok(total)
    }

    pub fn score(&self, config: &MatchConfig) -> Result<f64, Error> {
        let max = self.max_distance(config)?;
        if max == 0.0 {
            Ok(0.0)
        } else {
            Ok(self.raw_distance(config)? / max)
        }
    }

    pub fn weighted_penalty(&self, key: &str, config: &MatchConfig) -> Result<f64, Error> {
        let max = self.max_distance(config)?;
        if max == 0.0 {
            return Ok(0.0);
        }
        let values = self
            .penalties
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, values)| values)
            .ok_or_else(|| Error::MissingWeight(key.to_owned()))?;
        let weight = config
            .distance_weights
            .get(key)
            .ok_or_else(|| Error::MissingWeight(key.to_owned()))?;
        Ok(values.iter().sum::<f64>() * weight / max)
    }

    pub fn active_keys(&self, config: &MatchConfig) -> Result<Vec<String>, Error> {
        let mut result = Vec::new();
        for (key, _) in &self.penalties {
            if self.weighted_penalty(key, config)? != 0.0 {
                result.push(key.clone());
            }
        }
        Ok(result)
    }

    pub fn penalties(&self) -> &[(String, Vec<f64>)] {
        &self.penalties
    }
}

pub fn track_distance(
    item: &MatchItem,
    track: &MatchTrack,
    include_artist: bool,
    config: &MatchConfig,
) -> Result<Distance, Error> {
    let mut dist = Distance::default();
    if let Some(length) = track.length.filter(|length| *length != 0.0) {
        dist.add_ratio(
            "track_length",
            (item.length - length).abs() - config.track_length_grace,
            config.track_length_max,
        )?;
    }
    dist.add_string("track_title", Some(&item.title), Some(&track.title))?;
    if include_artist
        && track
            .artist
            .as_deref()
            .is_some_and(|artist| !artist.is_empty())
        && !VA_ARTISTS.contains(&item.artist.to_lowercase().as_str())
    {
        dist.add_string("track_artist", Some(&item.artist), track.artist.as_deref())?;
    }
    if track.index.is_some_and(|index| index != 0) && item.track != 0 {
        dist.add_equality(
            "track_index",
            Some(item.track) == track.index || Some(item.track) == track.medium_index,
        )?;
    }
    if !item.track_id.is_empty() {
        dist.add_equality(
            "track_id",
            Some(item.track_id.as_str()) == track.track_id.as_deref(),
        )?;
    }
    if track.medium.is_some_and(|medium| medium != 0) && item.disc != 0 {
        dist.add_equality("medium", Some(item.disc) == track.medium)?;
    }
    if let Some(penalty) =
        config.source_penalty(item.data_source.as_deref(), track.data_source.as_deref())
    {
        dist.add("data_source", penalty)?;
    }
    Ok(dist)
}

fn plurality<T: Eq + Clone>(items: &[MatchItem], get: impl Fn(&MatchItem) -> T) -> T {
    let mut best = get(&items[0]);
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
    let unit = 1.0 / patterns.len().max(1) as f64;
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
            return Ok(index as f64 * unit);
        }
    }
    Ok(1.0)
}

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
    let artist = plurality(items, |item| item.artist.clone());
    let album_artist = plurality(items, |item| item.album_artist.clone());
    let artist =
        if !album_artist.is_empty() && items.iter().all(|item| item.album_artist == album_artist) {
            album_artist
        } else {
            artist
        };
    if !album.various_artists {
        dist.add_string("artist", Some(&artist), Some(&album.artist))?;
    }
    dist.add_string(
        "album",
        Some(&plurality(items, |item| item.album.clone())),
        Some(&album.title),
    )?;
    let media = plurality(items, |item| item.media.clone());
    if let Some(album_media) = album.media.as_deref().filter(|value| !value.is_empty()) {
        if !config.preferred_media.is_empty() {
            dist.add(
                "media",
                preferred_match(album_media, &config.preferred_media, true)?,
            )?;
        } else if !media.is_empty() {
            dist.add_equality("media", album_media == media)?;
        }
    }
    let disc_total = plurality(items, |item| item.disc_total);
    if let Some(mediums) = album.mediums.filter(|mediums| *mediums != 0) {
        if disc_total != 0 {
            dist.add_number("mediums", disc_total, mediums)?;
        }
    }
    let year = plurality(items, |item| item.year);
    if let Some(album_year) = album.year.filter(|year| *year != 0) {
        if config.prefer_original_year {
            let original = album
                .original_year
                .filter(|year| *year != 0)
                .unwrap_or(1889);
            dist.add_ratio(
                "year",
                (album_year - original).abs() as f64,
                (config.current_year - original).abs() as f64,
            )?;
        } else if year != 0 {
            if year == album_year || album.original_year == Some(year) {
                dist.add("year", 0.0)?;
            } else if let Some(original) = album.original_year.filter(|year| *year != 0) {
                dist.add_ratio(
                    "year",
                    (year - album_year).abs() as f64,
                    (config.current_year - original).abs() as f64,
                )?;
            } else {
                dist.add("year", 1.0)?;
            }
        }
    }
    let country = plurality(items, |item| item.country.clone());
    if let Some(album_country) = album.country.as_deref().filter(|value| !value.is_empty()) {
        if !config.preferred_countries.is_empty() {
            dist.add(
                "country",
                preferred_match(album_country, &config.preferred_countries, false)?,
            )?;
        } else if !country.is_empty() {
            dist.add_string("country", Some(&country), Some(album_country))?;
        }
    }
    for (key, local, candidate) in [
        (
            "label",
            plurality(items, |item| item.label.clone()),
            album.label.as_deref(),
        ),
        (
            "catalognum",
            plurality(items, |item| item.catalog_number.clone()),
            album.catalog_number.as_deref(),
        ),
        (
            "albumdisambig",
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
            "album_id",
            Some(album_id.as_str()) == album.album_id.as_deref(),
        )?;
    }
    for &(item_index, track_index) in pairs {
        let (Some(item), Some(track)) = (items.get(item_index), album.tracks.get(track_index))
        else {
            return Err(Error::InvalidPair);
        };
        let track_dist = track_distance(item, track, album.various_artists, config)?;
        dist.add("tracks", track_dist.score(config)?)?;
        dist.tracks.push(track_dist);
    }
    for _ in pairs.len()..album.tracks.len() {
        dist.add("missing_tracks", 1.0)?;
    }
    for _ in pairs.len()..items.len() {
        dist.add("unmatched_tracks", 1.0)?;
    }
    let data_source = plurality(items, |item| item.data_source.clone());
    if let Some(penalty) =
        config.source_penalty(data_source.as_deref(), album.data_source.as_deref())
    {
        dist.add("data_source", penalty)?;
    }
    tracing::debug!(score = dist.score(config)?, "scored album candidate");
    Ok(dist)
}
