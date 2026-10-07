use muzik_runner::setup::{self, SoulseekAccount};
use muzik_soulseek::session;
use std::fs;

#[test]
fn the_soulseek_password_lives_in_the_keychain() {
    keyring_core::set_default_store(keyring_core::mock::Store::new().unwrap());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.yaml");
    fs::write(
        &path,
        "soulseek:\n  username: listener\n  password: secret\n",
    )
    .unwrap();

    setup::move_soulseek_password(&path).unwrap();
    assert!(!fs::read_to_string(&path).unwrap().contains("secret"));
    assert_eq!(session::saved_password().as_deref(), Some("secret"));
    assert_eq!(
        setup::soulseek_account(&path).unwrap()["has_password"],
        true
    );

    setup::save_soulseek_account(
        &path,
        &SoulseekAccount {
            username: Some("renamed"),
            password: Some("changed"),
            server_host: None,
            server_port: None,
        },
    )
    .unwrap();
    let saved = fs::read_to_string(&path).unwrap();
    assert!(saved.contains("renamed"));
    assert!(!saved.contains("changed"));
    assert_eq!(session::saved_password().as_deref(), Some("changed"));
}
