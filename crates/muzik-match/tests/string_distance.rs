use serde::Deserialize;

use muzik_match::string_dist;

#[derive(Deserialize)]
struct Fixture {
    beets_version: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    left: Option<String>,
    right: Option<String>,
    distance: f64,
}

#[test]
#[expect(clippy::float_cmp, reason = "beets parity needs exact float equality")]
fn matches_beets_string_distance() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/string_distance.json"))
        .expect("valid fixture JSON");
    assert_eq!(fixture.beets_version, "2.13.1");
    let mut differences = Vec::new();
    for case in fixture.cases {
        let actual = string_dist(case.left.as_deref(), case.right.as_deref());
        if actual != case.distance {
            differences.push(format!(
                "left={:?}, right={:?}: actual={actual}, beets={}",
                case.left, case.right, case.distance
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
