//! Parse featured performer credits shared by import and track splitting.

use regex::Regex;
use std::ops::Range;
use std::sync::LazyLock;

static FEATURED: LazyLock<Regex> = LazyLock::new(|| {
    literal(
        r"(?i)\s*(?:[\(\[]\s*\b(?:feat|ft|featuring)\b\.?\s+([^\)\]]+?)\s*[\)\]]|\b(?:feat|ft|featuring)\b\.?\s+([^\(\[]+))",
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

struct Credit {
    range: Range<usize>,
    names: Vec<String>,
    bracketed: bool,
}

fn next_credit(text: &str) -> Option<Credit> {
    FEATURED.captures_iter(text).find_map(|capture| {
        let (names, bracketed) = match capture.get(1) {
            Some(names) => (names, true),
            None => (capture.get(2)?, false),
        };
        let names: Vec<_> = SEPARATOR
            .split(names.as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        (!names.is_empty()).then(|| Credit {
            range: capture.get_match().range(),
            names,
            bracketed,
        })
    })
}

#[must_use]
pub fn parse(title: &str) -> (String, Vec<String>) {
    let mut clean = title.trim().to_owned();
    let mut featured = Vec::new();
    while let Some(credit) = next_credit(&clean) {
        for name in credit.names {
            if !featured.contains(&name) {
                featured.push(name);
            }
        }
        let before = clean.get(..credit.range.start).unwrap_or_default();
        let after = clean.get(credit.range.end..).unwrap_or_default();
        let joined = if credit.bracketed {
            format!("{before}{after}")
        } else {
            format!("{} {}", before.trim_end(), after.trim_start())
        };
        joined.trim().clone_into(&mut clean);
    }
    if clean.is_empty() {
        return (title.trim().to_owned(), featured);
    }
    (clean, featured)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn credit_keywords_inside_words_leave_titles_unchanged() {
        for title in ["Left Behind", "Soft Rain", "Gift of Love", "Defeat Me"] {
            assert_eq!(parse(title), (title.to_owned(), Vec::new()));
        }
    }

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

    #[test]
    fn every_credit_moves_out_of_the_title() {
        assert_eq!(
            parse("Song (feat. A) [ft. B & A] feat. C"),
            (
                "Song".to_owned(),
                vec!["A".to_owned(), "B".to_owned(), "C".to_owned()]
            )
        );
    }
}
