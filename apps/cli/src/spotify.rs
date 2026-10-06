use anyhow::{Context, bail};
use muzik_core::app_config;
use muzik_core::paths::Paths;
use muzik_spotify as spotify;
use muzik_store::watchlist;
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub fn set_client_id(client_id: &str) -> anyhow::Result<()> {
    spotify::set_client_id(&app_config::path(), client_id)?;
    println!("Spotify client ID saved.");
    Ok(())
}

pub fn logout() -> anyhow::Result<()> {
    if spotify::clear_tokens(&Paths::user().spotify_token())? {
        println!("Spotify tokens removed.");
    } else {
        println!("No Spotify tokens were saved.");
    }
    Ok(())
}

pub fn status() -> anyhow::Result<()> {
    let status = spotify::status(&app_config::path(), &Paths::user().spotify_token())?;
    let client_id = status
        .get("client_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("not set");
    println!("Client ID: {client_id}");
    println!(
        "Redirect URI: {}",
        status
            .get("redirect_uri")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    );
    if status.get("connected").and_then(serde_json::Value::as_bool) == Some(true) {
        println!(
            "Connection: connected as {}",
            status
                .get("account_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
        );
        Ok(())
    } else if let Some(error) = status.get("error").and_then(serde_json::Value::as_str) {
        bail!("Connection: {error}")
    } else {
        println!("Connection: not connected");
        Ok(())
    }
}

pub fn playlists() -> anyhow::Result<()> {
    for playlist in spotify::list_playlists(&app_config::path(), &Paths::user().spotify_token())? {
        let total = playlist
            .total
            .map_or_else(|| "?".to_owned(), |total| total.to_string());
        println!("{}\t{}\t{}", playlist.name, total, playlist.uri);
    }
    Ok(())
}

pub fn export(uri: &str, output: Option<&Path>) -> anyhow::Result<()> {
    let document =
        spotify::load_playlist_document(&app_config::path(), &Paths::user().spotify_token(), uri)?;
    let mut bytes = serde_json::to_vec_pretty(&document)?;
    bytes.push(b'\n');
    if let Some(path) = output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        std::fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))?;
    } else {
        print!("{}", String::from_utf8(bytes)?);
    }
    Ok(())
}

pub fn watch(reference: &str) -> anyhow::Result<()> {
    let source = watchlist::parse_source(reference)?;
    if source.kind != watchlist::SourceKind::Spotify {
        bail!("enter a Spotify playlist or album link, or liked");
    }
    let repository = watchlist::Repository::open(&Paths::user());
    let playlist = repository.add(reference)?;
    let name = playlist.title.as_deref().unwrap_or(&playlist.playlist_id);
    println!("Added {name}. Run `muzik watchlist refresh` to sync it.");
    Ok(())
}

pub fn login(port: Option<u16>) -> anyhow::Result<()> {
    let config = app_config::path();
    if let Some(port) = port {
        if port == 0 {
            bail!("port must be from 1 to 65535");
        }
        app_config::save_section_string(&config, "spotify", "redirect_port", &port.to_string())?;
    }
    let mut settings = spotify::settings(&config)?;
    if let Some(port) = port {
        settings.redirect_port = port;
    }
    println!(
        "Your Spotify application must have this redirect URI: {}",
        settings.redirect_uri()
    );
    println!("Opening the browser for Spotify login...");
    let name = spotify::login(
        &config,
        &Paths::user().spotify_token(),
        port,
        &AtomicBool::new(false),
    )?;
    println!("Connected as {name}.");
    Ok(())
}
