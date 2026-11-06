//! Move featured performer credits from a track title to its artist.

#[must_use]
pub fn clean(title: &str, artist: &str) -> (String, String) {
    let (clean, featured) = muzik_core::featured::parse(title);
    if featured.is_empty() {
        return (title.trim().to_owned(), artist.to_owned());
    }
    let lower = artist.to_ascii_lowercase();
    let artist = if lower.contains("feat") || lower.contains("ft.") {
        artist.to_owned()
    } else {
        format!("{artist} feat. {}", featured.join(", "))
            .trim()
            .to_owned()
    };
    (clean, artist)
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
