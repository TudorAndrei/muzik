use muzik_runner::setup::{self, SoulseekAccount};
use muzik_soulseek::session;
use std::fs;

#[test]
fn the_soulseek_password_lives_in_the_keychain() -> Result<(), Box<dyn std::error::Error>> {
    keyring_core::set_default_store(keyring_core::mock::Store::new()?);
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("config.yaml");
    fs::write(
        &path,
        "soulseek:\n  username: listener\n  password: secret\n",
    )?;

    setup::move_soulseek_password(&path)?;
    assert!(!fs::read_to_string(&path)?.contains("secret"));
    assert_eq!(session::saved_password().as_deref(), Some("secret"));
    assert_eq!(setup::soulseek_account(&path)?["has_password"], true);

    setup::save_soulseek_account(
        &path,
        &SoulseekAccount {
            username: Some("renamed"),
            password: Some("changed"),
            server_host: None,
            server_port: None,
        },
    )?;
    let saved = fs::read_to_string(&path)?;
    assert!(saved.contains("renamed"));
    assert!(!saved.contains("changed"));
    assert_eq!(session::saved_password().as_deref(), Some("changed"));
    Ok(())
}
