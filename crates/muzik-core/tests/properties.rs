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
    fn titles_without_featured_names_stay_unchanged(
        title in prop_oneof![credited_title(), "[A-Za-z ()\\[\\]&,./]{0,20}"],
    ) {
        let (clean, featured) = parse(&title);
        if featured.is_empty() {
            prop_assert_eq!(clean, title.trim());
        }
    }

    #[test]
    fn a_cleaned_title_has_no_credit_left(title in credited_title()) {
        let (clean, _) = parse(&title);
        if clean != title.trim() {
            let reparsed = parse(&clean);
            prop_assert_eq!(reparsed, (clean, Vec::new()));
        }
    }
}
