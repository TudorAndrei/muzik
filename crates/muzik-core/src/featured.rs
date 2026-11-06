//! Parse featured performer credits shared by import and track splitting.

use regex::Regex;
use std::sync::LazyLock;

static FEATURED: LazyLock<Regex> = LazyLock::new(|| {
    literal(
        r"(?i)\s*(?:[\(\[]\s*(?:feat|ft|featuring)\.?\s+([^\)\]]+?)\s*[\)\]]|(?:feat|ft|featuring)\.?\s+(.+)$)",
    )
});
static SEPARATOR: LazyLock<Regex> = LazyLock::new(|| literal(r"(?i)\s*(?:,|&|/|\bx\b|\band\b)\s*"));

#[expect(
    clippy::unwrap_used,
    reason = "patterns are fixed literals exercised by caller behavior tests"
)]
fn literal(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

#[must_use]
pub fn parse(title: &str) -> (String, Vec<String>) {
    let unchanged = || (title.trim().to_owned(), Vec::new());
    let Some(capture) = FEATURED.captures(title) else {
        return unchanged();
    };
    let Some(names) = capture.get(1).or_else(|| capture.get(2)) else {
        return unchanged();
    };
    let featured = SEPARATOR
        .split(names.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    let clean = FEATURED.replacen(title, 1, "");
    let clean = if clean.trim().is_empty() {
        title.trim()
    } else {
        clean.trim()
    };
    (clean.to_owned(), featured)
}
