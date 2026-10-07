use crate::{Album, Error, Fields, Item};
use fancy_regex::Regex;
use rusqlite::types::Value;
use std::cmp::Ordering;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Substring,
    Regexp,
    String,
    Exact,
    Numeric,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Term {
    pub field: Option<String>,
    pub pattern: String,
    pub kind: Kind,
    pub negated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sort {
    pub field: String,
    pub ascending: bool,
}

/// Comma-separated OR groups, each with space-separated AND terms.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Query {
    pub groups: Vec<Vec<Term>>,
    pub sorts: Vec<Sort>,
}

impl Query {
    /// # Errors
    /// Returns an error if the query has an unclosed quote, a bad regex, or a bad numeric range.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let parts = shlex::split(text)
            .ok_or_else(|| Error::InvalidQuery("unclosed quote or escape".into()))?;
        let mut groups = Vec::new();
        let mut group = Vec::new();
        let mut sorts = Vec::new();
        for part in parts {
            let (part, end_group) = part
                .strip_suffix(',')
                .map_or((part.as_str(), false), |stripped| (stripped, true));
            if !part.is_empty() {
                if let Some(sort) = parse_sort(part) {
                    sorts.push(sort);
                } else {
                    group.push(parse_term(part)?);
                }
            }
            if end_group {
                groups.push(std::mem::take(&mut group));
            }
        }
        groups.push(group);
        Ok(Self { groups, sorts })
    }

    #[must_use]
    pub fn matches_item(&self, item: &Item) -> bool {
        self.matches(&item.fields, &item.attributes, ITEM_SEARCH_FIELDS)
    }

    #[must_use]
    pub fn matches_album(&self, album: &Album) -> bool {
        self.matches(&album.fields, &album.attributes, ALBUM_SEARCH_FIELDS)
    }

    pub fn sort_items(&self, items: &mut [Item]) {
        let defaults = [
            Sort::ascending("artist"),
            Sort::ascending("album"),
            Sort::ascending("disc"),
            Sort::ascending("track"),
        ];
        let sorts = if self.sorts.is_empty() {
            &defaults[..]
        } else {
            &self.sorts
        };
        items.sort_by(|left, right| {
            compare_rows(
                &left.fields,
                &left.attributes,
                &right.fields,
                &right.attributes,
                sorts,
            )
        });
    }

    pub fn sort_albums(&self, albums: &mut [Album]) {
        let defaults = [Sort::ascending("albumartist"), Sort::ascending("album")];
        let sorts = if self.sorts.is_empty() {
            &defaults[..]
        } else {
            &self.sorts
        };
        albums.sort_by(|left, right| {
            compare_rows(
                &left.fields,
                &left.attributes,
                &right.fields,
                &right.attributes,
                sorts,
            )
        });
    }

    fn matches(&self, fields: &Fields, attributes: &Fields, search_fields: &[&str]) -> bool {
        self.groups.iter().any(|group| {
            group.iter().all(|term| {
                let found = term.field.as_ref().map_or_else(
                    || {
                        search_fields.iter().any(|name| {
                            value(fields, attributes, name)
                                .is_some_and(|value| term_matches(term, value))
                        })
                    },
                    |name| {
                        value(fields, attributes, name)
                            .is_some_and(|value| term_matches(term, value))
                    },
                );
                found != term.negated
            })
        })
    }
}

impl Sort {
    fn ascending(field: &str) -> Self {
        Self {
            field: field.into(),
            ascending: true,
        }
    }
}

const ITEM_SEARCH_FIELDS: &[&str] = &[
    "artist",
    "title",
    "comments",
    "album",
    "albumartist",
    "genres",
];
const ALBUM_SEARCH_FIELDS: &[&str] = &["album", "albumartist", "genres"];

fn parse_sort(part: &str) -> Option<Sort> {
    if part.contains(':') {
        return None;
    }
    let (field, ascending) = if let Some(field) = part.strip_suffix('+') {
        (field, true)
    } else {
        (part.strip_suffix('-')?, false)
    };
    (!field.is_empty()).then(|| Sort {
        field: field.into(),
        ascending,
    })
}

fn parse_term(part: &str) -> Result<Term, Error> {
    let (negated, part) = part
        .strip_prefix(['-', '^'])
        .map_or((false, part), |rest| (true, rest));
    let (field, pattern) = match part.split_once(':') {
        Some((field, pattern)) if !field.is_empty() => (Some(field.to_lowercase()), pattern),
        _ => (None, part),
    };
    let (kind, pattern) = term_kind(pattern, field.as_deref());
    if kind == Kind::Regexp {
        Regex::new(pattern).map_err(|error| Error::InvalidQuery(error.to_string()))?;
    }
    if kind == Kind::Numeric {
        parse_range(pattern, field.as_deref() == Some("length"))?;
    }
    Ok(Term {
        field,
        pattern: pattern.replace("\\:", ":"),
        kind,
        negated,
    })
}

fn term_kind<'a>(pattern: &'a str, field: Option<&str>) -> (Kind, &'a str) {
    if let Some(pattern) = pattern.strip_prefix(':') {
        return (Kind::Regexp, pattern);
    }
    if let Some(pattern) = pattern.strip_prefix("=~") {
        return (Kind::String, pattern);
    }
    if let Some(pattern) = pattern.strip_prefix('=') {
        return (Kind::Exact, pattern);
    }
    if field.is_some_and(is_numeric_field) {
        (Kind::Numeric, pattern)
    } else {
        (Kind::Substring, pattern)
    }
}

fn is_numeric_field(field: &str) -> bool {
    matches!(
        field,
        "id" | "album_id"
            | "added"
            | "mtime"
            | "length"
            | "track"
            | "tracktotal"
            | "disc"
            | "disctotal"
            | "year"
            | "month"
            | "day"
            | "original_year"
            | "original_month"
            | "original_day"
            | "bitrate"
            | "samplerate"
            | "bitdepth"
            | "channels"
    )
}

fn value<'a>(fields: &'a Fields, attributes: &'a Fields, name: &str) -> Option<&'a Value> {
    fields.get(name).or_else(|| attributes.get(name))
}

fn term_matches(term: &Term, value: &Value) -> bool {
    match term.kind {
        Kind::Numeric => {
            let number = match value {
                Value::Integer(number) => integer_to_f64(*number),
                Value::Real(number) => *number,
                Value::Text(number) => match number.parse::<f64>() {
                    Ok(number) => number,
                    Err(_) => return false,
                },
                _ => return false,
            };
            let Ok((min, max)) =
                parse_range(&term.pattern, term.field.as_deref() == Some("length"))
            else {
                return false;
            };
            min.is_none_or(|min| number >= min) && max.is_none_or(|max| number <= max)
        }
        Kind::Regexp => {
            let normalized: String = value_text(value).nfc().collect();
            let pattern: String = term.pattern.nfc().collect();
            Regex::new(&pattern).is_ok_and(|regex| regex.is_match(&normalized).unwrap_or(false))
        }
        Kind::Exact => value_text(value) == term.pattern,
        Kind::String => value_text(value).to_lowercase() == term.pattern.to_lowercase(),
        Kind::Substring => value_text(value)
            .to_lowercase()
            .contains(&term.pattern.to_lowercase()),
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Integer(value) => value.to_string(),
        Value::Real(value) => value.to_string(),
        Value::Text(value) => value.clone(),
        Value::Blob(value) => String::from_utf8_lossy(value).into_owned(),
    }
}

fn parse_range(pattern: &str, duration: bool) -> Result<(Option<f64>, Option<f64>), Error> {
    let number = |text: &str| -> Result<Option<f64>, Error> {
        if text.is_empty() {
            Ok(None)
        } else {
            let parsed = if duration {
                parse_duration(text).or_else(|| text.parse::<f64>().ok())
            } else {
                text.parse::<f64>().ok()
            };
            parsed
                .map(Some)
                .ok_or_else(|| Error::InvalidQuery(format!("invalid number: {text}")))
        }
    };
    if let Some((start, end)) = pattern.split_once("..") {
        Ok((number(start)?, number(end)?))
    } else {
        let point = number(pattern)?;
        Ok((point, point))
    }
}

fn parse_duration(text: &str) -> Option<f64> {
    let (minutes, seconds) = text.split_once(':')?;
    if minutes.is_empty()
        || !minutes.bytes().all(|byte| byte.is_ascii_digit())
        || seconds.len() != 2
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let seconds = seconds.parse::<u32>().ok()?;
    if seconds >= 60 {
        return None;
    }
    Some(f64::from(minutes.parse::<u32>().ok()?).mul_add(60.0, f64::from(seconds)))
}

#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "std has no From<i64> for f64; numeric queries compare as floats like beets"
)]
const fn integer_to_f64(number: i64) -> f64 {
    number as f64
}

fn compare_rows(
    left_fields: &Fields,
    left_attributes: &Fields,
    right_fields: &Fields,
    right_attributes: &Fields,
    sorts: &[Sort],
) -> Ordering {
    for sort in sorts {
        let left = sort_value(left_fields, left_attributes, &sort.field);
        let right = sort_value(right_fields, right_attributes, &sort.field);
        let order = compare_values(left, right);
        if order != Ordering::Equal {
            return if sort.ascending {
                order
            } else {
                order.reverse()
            };
        }
    }
    Ordering::Equal
}

fn sort_value<'a>(fields: &'a Fields, attributes: &'a Fields, field: &str) -> Option<&'a Value> {
    if field == "artist" || field == "albumartist" {
        let sort_field = format!("{field}_sort");
        if let Some(value @ Value::Text(text)) = fields.get(&sort_field)
            && !text.is_empty()
        {
            return Some(value);
        }
    }
    value(fields, attributes, field)
}

fn compare_values(left: Option<&Value>, right: Option<&Value>) -> Ordering {
    match (left, right) {
        (None | Some(Value::Null), None | Some(Value::Null)) => Ordering::Equal,
        (None | Some(Value::Null), Some(_)) => Ordering::Less,
        (Some(_), None | Some(Value::Null)) => Ordering::Greater,
        (Some(Value::Integer(left)), Some(Value::Integer(right))) => left.cmp(right),
        (Some(Value::Real(left)), Some(Value::Real(right))) => left.total_cmp(right),
        (Some(left), Some(right)) => value_text(left)
            .to_ascii_lowercase()
            .cmp(&value_text(right).to_ascii_lowercase()),
    }
}
