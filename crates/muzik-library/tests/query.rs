use muzik_library::query::Query;
use serde_json::Value;

#[test]
fn parsed_queries_match_beets() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/query.json")).unwrap();
    assert_eq!(fixture["beets_version"], "2.13.1");
    for case in fixture["cases"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let query = Query::parse(text).unwrap();
        let expected_groups = case["groups"].as_array().unwrap();
        assert_eq!(query.groups.len(), expected_groups.len(), "{text}");
        for (group, expected_terms) in query.groups.iter().zip(expected_groups) {
            let expected_terms = expected_terms.as_array().unwrap();
            assert_eq!(group.len(), expected_terms.len(), "{text}");
            for (term, expected) in group.iter().zip(expected_terms) {
                assert_eq!(term.field(), expected["field"].as_str(), "{text}");
                assert_eq!(
                    term.pattern(),
                    expected["pattern"].as_str().unwrap(),
                    "{text}"
                );
                assert_eq!(format!("{:?}", term.kind()), expected["kind"], "{text}");
                assert_eq!(
                    term.negated(),
                    expected["negated"].as_bool().unwrap(),
                    "{text}"
                );
            }
        }
        let expected_sorts = case["sorts"].as_array().unwrap();
        assert_eq!(query.sorts.len(), expected_sorts.len(), "{text}");
        for (sort, expected) in query.sorts.iter().zip(expected_sorts) {
            assert_eq!(sort.field, expected["field"].as_str().unwrap(), "{text}");
            assert_eq!(
                sort.ascending,
                expected["ascending"].as_bool().unwrap(),
                "{text}"
            );
        }
    }
}

#[test]
fn query_with_non_ascii_final_character_is_a_search_term() {
    let query = Query::parse("Beyoncé").unwrap();
    assert_eq!(query.groups[0][0].pattern(), "Beyoncé");
    assert!(query.sorts.is_empty());

    let sorted = Query::parse("Beyoncé+").unwrap();
    assert_eq!(sorted.sorts[0].field, "Beyoncé");
}
