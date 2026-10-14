//! Conversion between release metadata, mediafile tags, and beets fields.

use muzik_core::{ReleaseCandidate, TrackCandidate};
use muzik_library::{Fields, Item, SqlValue, scalar_text};
use muzik_tags::TagData;
use std::collections::BTreeMap;

pub fn release_tags(
    release: &ReleaseCandidate,
    track: Option<&TrackCandidate>,
) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
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
            fields.insert(name.into(), value.into());
        }
    }
    if let Some(year) = release.year {
        fields.insert("date".into(), year.to_string());
    }
    if let Some(track) = track {
        fields.insert("title".into(), track.title.clone());
        fields.insert("artist".into(), track.artist.clone());
        fields.insert("track".into(), track.medium_index.to_string());
        fields.insert("disc".into(), track.medium.to_string());
        if let Some(id) = &track.recording_id {
            fields.insert("mb_trackid".into(), id.0.clone());
        }
        if let Some(id) = &track.release_track_id {
            fields.insert("mb_releasetrackid".into(), id.clone());
        }
    }
    fields
}

pub fn apply_sync_fields(tags: &mut TagData, fields: &Fields) {
    for (name, value) in fields {
        let tag_name = if name == "year" { "date" } else { name };
        if let SqlValue::Text(value) = value {
            tags.fields.insert(tag_name.into(), value.clone());
        } else if name == "year" {
            if let SqlValue::Integer(value) = value {
                tags.fields.insert("date".into(), value.to_string());
            }
        } else if ["track", "disc"].contains(&name.as_str())
            && let SqlValue::Integer(value) = value
        {
            tags.fields.insert(name.clone(), value.to_string());
        }
    }
}

pub fn sql_scalar(key: &str, value: &str) -> SqlValue {
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
        SqlValue::Real(
            value
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .parse()
                .unwrap_or(0.0),
        )
    } else {
        SqlValue::Text(value.to_owned())
    }
}

pub fn insert_dates(fields: &mut Fields, tags: &TagData) {
    for (tag_name, prefix) in [("date", ""), ("original_date", "original_")] {
        let Some(value) = tags.fields.get(tag_name) else {
            continue;
        };
        for (part, number) in ["year", "month", "day"].into_iter().zip(value.split('-')) {
            if let Ok(number) = number.parse::<i64>() {
                fields.insert(format!("{prefix}{part}"), SqlValue::Integer(number));
            }
        }
    }
}

pub fn tags_from_item(item: &Item) -> TagData {
    let mut tags = TagData::default();
    for field in muzik_tags::FIELDS {
        if matches!(field.name, "date" | "original_date") {
            continue;
        }
        if let Some(value) = item.field(field.name).and_then(scalar_text)
            && !value.is_empty()
        {
            tags.fields.insert(field.name.to_owned(), value);
        }
    }
    if let Some(value) = item.field("albumdisambig").and_then(scalar_text)
        && !value.is_empty()
    {
        tags.fields.insert("albumdisambig".into(), value);
    }
    for name in ["rg_track_gain", "rg_album_gain"] {
        if let Some(SqlValue::Real(value)) = item.field(name) {
            tags.fields.insert(name.into(), format!("{value:.2} dB"));
        }
    }
    for name in ["rg_track_peak", "rg_album_peak"] {
        if let Some(SqlValue::Real(value)) = item.field(name) {
            tags.fields.insert(name.into(), format!("{value:.6}"));
        }
    }
    for (prefix, target) in [("", "date"), ("original_", "original_date")] {
        let Some(year) = item.field(&format!("{prefix}year")).and_then(scalar_text) else {
            continue;
        };
        let month = item.field(&format!("{prefix}month")).and_then(scalar_text);
        let day = month
            .as_ref()
            .and_then(|_| item.field(&format!("{prefix}day")).and_then(scalar_text));
        let date = match (month, day) {
            (Some(month), Some(day)) => format!("{year:0>4}-{month:0>2}-{day:0>2}"),
            (Some(month), None) => format!("{year:0>4}-{month:0>2}"),
            (None, _) => format!("{year:0>4}"),
        };
        tags.fields.insert(target.to_owned(), date);
    }
    if let Some(comp) = item.field("comp").and_then(scalar_text) {
        tags.fields
            .insert("comp".into(), if comp == "0" { "0" } else { "1" }.into());
    }
    tags
}
