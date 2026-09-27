use muzik_core::{app_config, paths};
use muzik_soulseek::session::{Session, SessionSettings, setting};

pub fn check() -> Result<(), String> {
    let config = app_config::load(&app_config::path()).map_err(|error| error.to_string())?;
    let settings = SessionSettings::configured(&config).ok_or(
        "Set MUZIK_SOULSEEK_USERNAME and MUZIK_SOULSEEK_PASSWORD, or save them with 'muzik config set-soulseek'.",
    )?;
    let username = settings.username.clone();
    let host = settings
        .server_host
        .clone()
        .unwrap_or_else(|| "server.slsknet.org".into());
    let port = settings.server_port.unwrap_or(2416);
    let download_dir = setting(&config, "MUZIK_SOULSEEK_DOWNLOAD_DIR", "download_dir")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| paths::data_dir().join("soulseek"));
    let session =
        Session::connect(settings).map_err(|error| format!("Soulseek check failed: {error}"))?;
    session.close();
    println!("Soulseek reachable");
    println!("  Username: {username}");
    println!("  Server: {host}:{port}");
    println!("  Download dir: {}", download_dir.display());
    Ok(())
}
