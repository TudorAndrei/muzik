use muzik_core::{app_config, spotify, watchlist};
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub fn set_client_id(client_id: &str) -> Result<(), String> {
    spotify::set_client_id(&app_config::path(), client_id)?;
    println!("Spotify client ID saved.");
    Ok(())
}

pub fn logout() -> Result<(), String> {
    if spotify::clear_tokens(&spotify::token_path())? {
        println!("Spotify tokens removed.");
    } else {
        println!("No Spotify tokens were saved.");
    }
    Ok(())
}

pub fn status() -> Result<(), String> {
    let status = spotify::status(&app_config::path(), &spotify::token_path())?;
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
        Err(format!("Connection: {error}"))
    } else {
        println!("Connection: not connected");
        Ok(())
    }
}

pub fn playlists() -> Result<(), String> {
    for playlist in spotify::list_playlists(&app_config::path(), &spotify::token_path())? {
        let total = playlist
            .total
            .map_or_else(|| "?".to_owned(), |total| total.to_string());
        println!("{}\t{}\t{}", playlist.name, total, playlist.uri);
    }
    Ok(())
}

pub fn export(uri: &str, output: Option<&Path>) -> Result<(), String> {
    let document =
        spotify::load_playlist_document(&app_config::path(), &spotify::token_path(), uri)?;
    let mut bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if let Some(path) = output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        std::fs::write(path, bytes)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    } else {
        print!(
            "{}",
            String::from_utf8(bytes).map_err(|error| error.to_string())?
        );
    }
    Ok(())
}

pub fn watch(reference: &str) -> Result<(), String> {
    let source = watchlist::parse_source(reference)?;
    if source.get("kind").and_then(serde_json::Value::as_str) != Some("spotify") {
        return Err("enter a Spotify playlist or album link, or liked".into());
    }
    let repository = watchlist::Repository::default();
    let playlist = repository.add(reference)?;
    let name = playlist
        .get("title")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            playlist
                .get("playlist_id")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("playlist");
    println!("Added {name}. Run `muzik watchlist refresh` to sync it.");
    Ok(())
}

pub fn login(port: Option<u16>) -> Result<(), String> {
    let config = app_config::path();
    if let Some(port) = port {
        if port == 0 {
            return Err("port must be from 1 to 65535".into());
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
        &spotify::token_path(),
        port,
        &AtomicBool::new(false),
    )?;
    println!("Connected as {name}.");
    Ok(())
}
