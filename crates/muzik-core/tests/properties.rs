use muzik_core::featured::parse;
use proptest::prelude::*;

fn credited_title() -> impl Strategy<Value = String> {
    "[A-Za-z ]{1,8}( [\\(\\[](feat|ft|featuring)\\.? [A-Za-z&,/ ]{1,8}[\\)\\]]){0,2}( (feat|ft)\\. [A-Za-z&]{1,6})?"
}

proptest! {
    #[test]
    fn parsed_titles_and_names_are_trimmed(title in credited_title()) {
        let (clean, featured) = parse(&title);
        prop_assert_eq!(clean.trim(), clean.as_str());
        for name in featured {
            prop_assert!(!name.is_empty());
            prop_assert_eq!(name.trim(), name.as_str());
        }
    }

    #[test]
    fn titles_without_a_credit_word_stay_unchanged(title in "[A-Za-z ()\\[\\]&,.]{0,20}") {
        prop_assume!(!title.to_ascii_lowercase().contains("ft") && !title.to_ascii_lowercase().contains("feat"));
        prop_assert_eq!(parse(&title), (title.trim().to_owned(), Vec::new()));
    }
}
