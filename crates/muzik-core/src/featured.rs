//! Parse featured performer credits shared by import and track splitting.

use regex::Regex;
use std::sync::LazyLock;

static FEATURED: LazyLock<Regex> = LazyLock::new(|| {
    literal(
        r"(?i)\s*(?:[\(\[]\s*(?:feat|ft|featuring)\.?\s+([^\)\]]+?)\s*[\)\]]|(?:feat|ft|featuring)\.?\s+([^\(\[]+))",
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
    let found = capture.get_match();
    let before = title.get(..found.start()).unwrap_or_default();
    let after = title.get(found.end()..).unwrap_or_default();
    let clean = if capture.get(2).is_some() {
        format!("{} {}", before.trim_end(), after.trim_start())
    } else {
        format!("{before}{after}")
    };
    let clean = if clean.trim().is_empty() {
        title.trim()
    } else {
        clean.trim()
    };
    (clean.to_owned(), featured)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn unbracketed_credits_keep_the_version_in_the_title() {
        for title in ["Song ft. A (Remix)", "Song featuring A [Live]"] {
            let version = if title.ends_with("(Remix)") {
                "Song (Remix)"
            } else {
                "Song [Live]"
            };
            assert_eq!(parse(title), (version.to_owned(), vec!["A".to_owned()]));
        }
        assert_eq!(
            parse("Song (feat. A) (Remix)"),
            ("Song (Remix)".to_owned(), vec!["A".to_owned()])
        );
    }
}
