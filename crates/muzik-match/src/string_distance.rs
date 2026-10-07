use std::sync::LazyLock;

use regex::Regex;

// Order matters: each successful reduction changes the baseline for the next.
const SD_END_WORDS: &[&str] = &["the", "a", "an"];
const SD_REPLACE: &[(&str, &str)] = &[("&", "and")];
const SD_PATTERNS: &[(&str, f64)] = &[
    (r"^the ", 0.1),
    (r"[\[\(]?(ep|single)[\]\)]?", 0.0),
    (r"[\[\(]?(featuring|feat|ft)[\. :].+", 0.1),
    (r"\(.*?\)", 0.3),
    (r"\[.*?\]", 0.3),
    (r"(, )?(pt\.|part) .+", 0.2),
];

#[expect(
    clippy::expect_used,
    reason = "the patterns are fixed literals covered by tests"
)]
static PATTERNS: LazyLock<Vec<(Regex, f64)>> = LazyLock::new(|| {
    SD_PATTERNS
        .iter()
        .map(|(pattern, weight)| (Regex::new(pattern).expect("fixed beets pattern"), *weight))
        .collect()
});

fn basic_distance(left: &str, right: &str) -> f64 {
    let normalize = |text: &str| {
        // Python Unidecode drops U+1F3B5; deunicode spells it out.
        // Keep this compatibility override explicit until more Unicode
        // differences are observed in the fixture corpus.
        let text = text.replace('🎵', "");
        deunicode::deunicode(&text)
            .to_ascii_lowercase()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
    };
    let left = normalize(left);
    let right = normalize(right);
    let length = left.len().max(right.len());
    if length == 0 {
        0.0
    } else {
        count_to_f64(strsim::levenshtein(&left, &right)) / count_to_f64(length)
    }
}

#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "std has no From<usize> for f64; beets divides counts as floats"
)]
pub const fn count_to_f64(count: usize) -> f64 {
    count as f64
}

/// Compute beets' normalized string distance, including reduced weights for
/// release descriptors, featured artists, and parenthesized text.
pub fn string_dist(left: Option<&str>, right: Option<&str>) -> f64 {
    let (Some(left), Some(right)) = (left, right) else {
        return if left.is_none() && right.is_none() {
            0.0
        } else {
            1.0
        };
    };

    let mut left = left.to_lowercase();
    let mut right = right.to_lowercase();
    for word in SD_END_WORDS {
        let suffix = format!(", {word}");
        if let Some(prefix) = left.strip_suffix(&suffix) {
            left = format!("{word} {prefix}");
        }
        if let Some(prefix) = right.strip_suffix(&suffix) {
            right = format!("{word} {prefix}");
        }
    }

    for (pattern, replacement) in SD_REPLACE {
        left = left.replace(pattern, replacement);
        right = right.replace(pattern, replacement);
    }

    let mut base_distance = basic_distance(&left, &right);
    let mut penalty = 0.0;
    for (pattern, weight) in PATTERNS.iter() {
        let reduced_left = pattern.replace_all(&left, "").into_owned();
        let reduced_right = pattern.replace_all(&right, "").into_owned();
        if reduced_left == left && reduced_right == right {
            continue;
        }
        let reduced_distance = basic_distance(&reduced_left, &reduced_right);
        let delta = (base_distance - reduced_distance).max(0.0);
        if delta == 0.0 {
            continue;
        }
        left = reduced_left;
        right = reduced_right;
        base_distance = reduced_distance;
        penalty += weight * delta;
    }
    base_distance + penalty
}
