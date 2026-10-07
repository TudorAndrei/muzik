//! Move featured performer credits from a track title to its artist.

use std::sync::LazyLock;

use regex::Regex;

static FEATURED: LazyLock<Regex> = LazyLock::new(|| {
    literal(
        r"(?ix)\s*(?:[\(\[]\s*(?:feat|ft|featuring)\.?\s+([^\)\]]+?)\s*[\)\]]|(?:feat|ft|featuring)\.?\s+(.+)$)",
    )
});
static SPLIT: LazyLock<Regex> = LazyLock::new(|| literal(r"(?i)\s*(?:,|&|/|\bx\b|\band\b)\s*"));

#[expect(
    clippy::unwrap_used,
    reason = "the patterns are string literals that the unit tests compile"
)]
fn literal(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

pub fn clean(title: &str, artist: &str) -> (String, String) {
    let unchanged = || (title.trim().to_owned(), artist.to_owned());
    let Some(found) = FEATURED.captures(title) else {
        return unchanged();
    };
    let whole = found.get_match();
    let Some(names) = found.get(1).or_else(|| found.get(2)) else {
        return unchanged();
    };
    let featured: Vec<_> = SPLIT
        .split(names.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if featured.is_empty() {
        return unchanged();
    }
    let (Some(before), Some(after)) = (title.get(..whole.start()), title.get(whole.end()..)) else {
        return unchanged();
    };
    let clean = format!("{before}{after}");
    let title = if clean.trim().is_empty() {
        title.trim().to_owned()
    } else {
        clean.trim().to_owned()
    };
    let lower = artist.to_ascii_lowercase();
    let artist = if lower.contains("feat") || lower.contains("ft.") {
        artist.to_owned()
    } else {
        format!("{artist} feat. {}", featured.join(", "))
            .trim()
            .to_owned()
    };
    (title, artist)
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn cleans_bracketed_and_trailing_credits() {
        assert_eq!(
            clean("Song (feat. A & B)", "Artist"),
            ("Song".to_owned(), "Artist feat. A, B".to_owned())
        );
        assert_eq!(
            clean("Song ft. Guest", "Artist feat. Someone"),
            ("Song".to_owned(), "Artist feat. Someone".to_owned())
        );
    }
}
