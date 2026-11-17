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

#[derive(Clone, Debug)]
pub struct Term {
    field: Option<String>,
    pattern: String,
    kind: Kind,
    negated: bool,
    matcher: Matcher,
}

#[derive(Clone, Debug)]
enum Matcher {
    Substring(String),
    Regexp(Option<Regex>),
    String(String),
    Exact(String),
    Numeric(Option<(Option<f64>, Option<f64>)>),
}

impl Term {
    #[must_use]
    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.kind
    }
    #[must_use]
    pub const fn negated(&self) -> bool {
        self.negated
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sort {
    pub field: String,
    pub ascending: bool,
}

/// Comma-separated OR groups, each with space-separated AND terms.
#[derive(Clone, Debug)]
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
    let pattern = pattern.replace("\\:", ":");
    // Validate the input above, then prepare the exact normalized pattern used
    // by beets matching. An invalid normalized pattern remains a non-match.
    let matcher = match kind {
        Kind::Regexp => Matcher::Regexp(Regex::new(&pattern.nfc().collect::<String>()).ok()),
        Kind::Numeric => {
            Matcher::Numeric(parse_range(&pattern, field.as_deref() == Some("length")).ok())
        }
        Kind::Exact => Matcher::Exact(pattern.clone()),
        Kind::String => Matcher::String(pattern.to_lowercase()),
        Kind::Substring => Matcher::Substring(pattern.to_lowercase()),
    };
    Ok(Term {
        field,
        pattern,
        kind,
        negated,
        matcher,
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
    match &term.matcher {
        Matcher::Numeric(Some((min, max))) => {
            let number = match value {
                Value::Integer(number) => integer_to_f64(*number),
                Value::Real(number) => *number,
                Value::Text(number) => match number.parse::<f64>() {
                    Ok(number) => number,
                    Err(_) => return false,
                },
                _ => return false,
            };
            min.is_none_or(|min| number >= min) && max.is_none_or(|max| number <= max)
        }
        Matcher::Regexp(Some(regex)) => {
            let normalized: String = value_text(value).nfc().collect();
            regex.is_match(&normalized).unwrap_or(false)
        }
        Matcher::Numeric(None) | Matcher::Regexp(None) => false,
        Matcher::Exact(pattern) => value_text(value) == *pattern,
        Matcher::String(pattern) => value_text(value).to_lowercase() == *pattern,
        Matcher::Substring(pattern) => value_text(value).to_lowercase().contains(pattern),
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
    match (sort_key(left), sort_key(right)) {
        (SortKey::Integer(left), SortKey::Integer(right)) => left.cmp(&right),
        (SortKey::Real(left), SortKey::Real(right)) => {
            left.partial_cmp(&right).unwrap_or(Ordering::Equal)
        }
        (SortKey::Integer(left), SortKey::Real(right)) => compare_integer_real(left, right),
        (SortKey::Real(left), SortKey::Integer(right)) => {
            compare_integer_real(right, left).reverse()
        }
        (SortKey::Text(left), SortKey::Text(right)) => left
            .iter()
            .map(u8::to_ascii_lowercase)
            .cmp(right.iter().map(u8::to_ascii_lowercase)),
        (left, right) => left.rank().cmp(&right.rank()),
    }
}

enum SortKey<'a> {
    Null,
    Integer(i64),
    Real(f64),
    Text(&'a [u8]),
}

impl SortKey<'_> {
    const fn rank(&self) -> u8 {
        match self {
            Self::Null => 0,
            Self::Integer(_) | Self::Real(_) => 1,
            Self::Text(_) => 2,
        }
    }
}

fn sort_key(value: Option<&Value>) -> SortKey<'_> {
    match value {
        None | Some(Value::Null) => SortKey::Null,
        Some(Value::Real(real)) if real.is_nan() => SortKey::Null,
        Some(Value::Integer(integer)) => SortKey::Integer(*integer),
        Some(Value::Real(real)) => SortKey::Real(*real),
        Some(Value::Text(text)) => SortKey::Text(text.as_bytes()),
        Some(Value::Blob(bytes)) => SortKey::Text(bytes),
    }
}

const INTEGER_LIMIT: f64 = 9_223_372_036_854_775_808.0;

fn compare_integer_real(integer: i64, real: f64) -> Ordering {
    if real >= INTEGER_LIMIT {
        return Ordering::Less;
    }
    if real < -INTEGER_LIMIT {
        return Ordering::Greater;
    }
    integer
        .cmp(&whole_real_to_integer(real.trunc()))
        .then_with(|| {
            0.0_f64
                .partial_cmp(&real.fract())
                .unwrap_or(Ordering::Equal)
        })
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "callers pass a whole number inside the i64 range, which converts exactly"
)]
const fn whole_real_to_integer(real: f64) -> i64 {
    real as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn value() -> impl Strategy<Value = Option<Value>> {
        prop_oneof![
            Just(None),
            Just(Some(Value::Null)),
            (-3_i64..3).prop_map(|number| Some(Value::Integer(number))),
            any::<i64>().prop_map(|number| Some(Value::Integer(number))),
            (-3_i32..3).prop_map(|number| Some(Value::Real(f64::from(number)))),
            (-3.0_f64..3.0).prop_map(|number| Some(Value::Real(number))),
            any::<f64>().prop_map(|number| Some(Value::Real(number))),
            prop_oneof![
                Just(INTEGER_LIMIT),
                Just(-INTEGER_LIMIT),
                Just(9_223_372_036_854_774_784.0),
                Just(-0.0),
            ]
            .prop_map(|number| Some(Value::Real(number))),
            "[a-cA-C0-9]{0,3}".prop_map(|text| Some(Value::Text(text))),
            proptest::collection::vec(prop_oneof![Just(b'a'), Just(b'B'), Just(0xff)], 0..3)
                .prop_map(|bytes| Some(Value::Blob(bytes))),
        ]
    }

    fn item(id: i64, year: Option<Value>) -> Item {
        Item {
            id,
            fields: year
                .map(|year| Fields::from([("year".to_owned(), year)]))
                .unwrap_or_default(),
            attributes: Fields::new(),
        }
    }

    proptest! {
        #[test]
        fn sort_order_is_antisymmetric(left in value(), right in value()) {
            prop_assert_eq!(
                compare_values(left.as_ref(), right.as_ref()),
                compare_values(right.as_ref(), left.as_ref()).reverse()
            );
        }

        #[test]
        fn sort_order_is_transitive(first in value(), second in value(), third in value()) {
            let (first, second, third) = (first.as_ref(), second.as_ref(), third.as_ref());
            let first_second = compare_values(first, second);
            let second_third = compare_values(second, third);
            if first_second != Ordering::Greater && second_third != Ordering::Greater {
                prop_assert_ne!(compare_values(first, third), Ordering::Greater);
            }
            if first_second == Ordering::Equal && second_third == Ordering::Equal {
                prop_assert_eq!(compare_values(first, third), Ordering::Equal);
            }
        }

        #[test]
        fn items_sort_like_the_beets_sql_order(years in proptest::collection::vec(value(), 0..12)) {
            let connection = rusqlite::Connection::open_in_memory()?;
            connection.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, year)", [])?;
            let mut items = Vec::new();
            for (id, year) in (1_i64..).zip(years) {
                connection.execute(
                    "INSERT INTO items (id, year) VALUES (?1, ?2)",
                    (id, year.clone().unwrap_or(Value::Null)),
                )?;
                items.push(item(id, year));
            }
            let mut statement = connection.prepare(
                "SELECT id FROM items ORDER BY (CASE \
                 WHEN TYPEOF(year)='text' THEN LOWER(year) \
                 WHEN TYPEOF(year)='blob' THEN LOWER(year) \
                 ELSE year END) ASC, id",
            )?;
            let beets: Vec<i64> = statement
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            Query::parse("year+")?.sort_items(&mut items);
            let muzik: Vec<i64> = items.iter().map(|item| item.id).collect();
            prop_assert_eq!(muzik, beets);
        }
    }

    #[test]
    fn numbers_sort_before_text_and_compare_exactly_across_types() {
        let order = |left: Value, right: Value| compare_values(Some(&left), Some(&right));
        assert_eq!(
            order(Value::Integer(10), Value::Text("2".into())),
            Ordering::Less
        );
        assert_eq!(order(Value::Real(2.5), Value::Integer(3)), Ordering::Less);
        assert_eq!(
            order(Value::Integer(-2), Value::Real(-2.5)),
            Ordering::Greater
        );
        assert_eq!(
            order(Value::Integer(i64::MAX), Value::Real(INTEGER_LIMIT)),
            Ordering::Less
        );
        assert_eq!(
            order(
                Value::Integer(9_223_372_036_854_774_785),
                Value::Real(9_223_372_036_854_774_784.0)
            ),
            Ordering::Greater
        );
        assert_eq!(
            order(Value::Text("abc".into()), Value::Blob(b"ABX".to_vec())),
            Ordering::Less
        );
    }
}
