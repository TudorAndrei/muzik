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
