use muzik_core::BeetsConfig;
use serde_json::Value;

#[test]
fn config_matches_beets_fixture() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/config.json")).expect("valid fixture");
    let config = BeetsConfig::from_layers(
        fixture["user_yaml"].as_str().expect("user yaml"),
        fixture["overrides"].clone(),
    )
    .expect("valid beets config");
    for (path, expected) in fixture["values"].as_object().expect("fixture values") {
        let keys: Vec<_> = path.split('.').collect();
        assert_eq!(config.get(&keys), Some(expected), "{path}");
    }
}

#[test]
fn missing_user_file_keeps_defaults() {
    let path = std::env::temp_dir().join(format!("muzik-missing-config-{}", std::process::id()));
    let config = BeetsConfig::load(&path, serde_json::json!({})).expect("defaults load");
    assert_eq!(config.get(&["import", "move"]), Some(&Value::Bool(false)));
}

#[test]
fn configured_paths_use_the_config_directory_and_keep_absolute_paths() {
    let directory = tempfile::tempdir().unwrap();
    let absolute = directory.path().join("music");
    let config = BeetsConfig::from_layers(
        "",
        serde_json::json!({
            "library": "state/library.db", "directory": absolute, "statefile": 3,
        }),
    )
    .unwrap();
    let file = directory.path().join("config.yaml");
    assert_eq!(
        config.configured_path(&file, "library").unwrap(),
        directory.path().join("state/library.db")
    );
    assert_eq!(
        config.configured_path(&file, "directory").unwrap(),
        absolute
    );
    assert!(config.configured_path(&file, "statefile").is_err());
    assert!(config.configured_path(&file, "missing").is_err());
}
