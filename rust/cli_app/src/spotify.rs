use muzik_core::{app_config, spotify};

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
