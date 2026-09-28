//! Move featured performer credits from a track title to its artist.

use std::sync::LazyLock;

use regex::Regex;

static FEATURED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?ix)\s*(?:[\(\[]\s*(?:feat|ft|featuring)\.?\s+([^\)\]]+?)\s*[\)\]]|(?:feat|ft|featuring)\.?\s+(.+)$)").unwrap()
});
static SPLIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*(?:,|&|/|\bx\b|\band\b)\s*").unwrap());

pub fn clean(title: &str, artist: &str) -> (String, String) {
    let Some(found) = FEATURED.captures(title) else {
        return (title.trim().to_owned(), artist.to_owned());
    };
    let whole = found.get(0).unwrap();
    let names = found.get(1).or_else(|| found.get(2)).unwrap().as_str();
    let featured: Vec<_> = SPLIT
        .split(names)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if featured.is_empty() {
        return (title.trim().to_owned(), artist.to_owned());
    }
    let clean = format!("{}{}", &title[..whole.start()], &title[whole.end()..]);
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
