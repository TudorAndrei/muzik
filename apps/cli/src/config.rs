use std::env;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

use crate::{SetSoulseek, paths};
use muzik_core::paths::expand_home;
use muzik_soulseek::session::{DEFAULT_SERVER_HOST, DEFAULT_SERVER_PORT};

pub fn show(path: Option<&Path>) -> io::Result<()> {
    let library_path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(muzik_core::default_config_path);
    let library = read_yaml(&library_path)?;
    println!("Library config: {}", library_path.display());
    println!("  exists: {}", library_path.exists());
    println!(
        "  directory: {}",
        library.get("directory").map(value_text).unwrap_or_default()
    );
    println!(
        "  database: {}",
        library.get("library").map(value_text).unwrap_or_default()
    );
    if library_path.exists() {
        println!("{}", fs::read_to_string(&library_path)?);
    }

    let muzik_path = paths::config_dir().join("config.yaml");
    let muzik = read_yaml(&muzik_path)?;
    let soulseek = muzik.get("soulseek");
    println!("Muzik config: {}", muzik_path.display());
    println!(
        "  Soulseek username: {}",
        soulseek
            .and_then(|settings| settings.get("username"))
            .map(value_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "not set".to_owned())
    );
    println!(
        "  Soulseek password: {}",
        if soulseek
            .and_then(|settings| settings.get("password"))
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            "set"
        } else {
            "not set"
        }
    );
    Ok(())
}

pub fn set_library(directory: &Path, db: Option<&Path>, path: Option<&Path>) -> io::Result<()> {
    let library_path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(muzik_core::default_config_path);
    let directory = expand_home(directory);
    fs::create_dir_all(&directory)?;
    let directory = fs::canonicalize(directory)?;
    let db = db
        .map(expand_home)
        .unwrap_or_else(|| directory.join(".library.db"));
    let db = std::path::absolute(db)?;

    let mut data = read_yaml(&library_path)?;
    let object = data
        .as_object_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "library config is not a map"))?;
    object.insert("directory".to_owned(), json!(directory));
    object.insert("library".to_owned(), json!(db));
    object.entry("paths").or_insert_with(|| {
        json!({
            "default": "$albumartist/$album%aunique{}/$track $title",
            "singleton": "Non-Album/$artist/$title",
            "comp": "Compilations/$album%aunique{}/$track $title"
        })
    });
    object
        .entry("import")
        .or_insert_with(|| json!({"write": true, "copy": true}));
    write_yaml(&library_path, &data)?;
    println!("Music library config: {}", library_path.display());
    println!("  directory: {}", directory.display());
    println!("  library db: {}", db.display());
    Ok(())
}

pub fn set_soulseek(args: &SetSoulseek) -> io::Result<()> {
    let path = paths::config_dir().join("config.yaml");
    let mut data = read_yaml(&path)?;
    let object = data
        .as_object_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "muzik config is not a map"))?;
    let settings = object
        .entry("soulseek")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "soulseek config is not a map")
        })?;
    if let Some(username) = &args.username {
        settings.insert("username".to_owned(), json!(username));
    }
    if let Some(password) = &args.password {
        settings.insert("password".to_owned(), json!(password));
    }
    settings.insert(
        "server_host".to_owned(),
        json!(args.server_host.as_deref().unwrap_or(DEFAULT_SERVER_HOST)),
    );
    settings.insert(
        "server_port".to_owned(),
        json!(args.server_port.unwrap_or(DEFAULT_SERVER_PORT)),
    );
    let downloads = args
        .download_dir
        .clone()
        .unwrap_or_else(|| paths::data_dir().join("soulseek"));
    let downloads = expand_home(&downloads);
    settings.insert("download_dir".to_owned(), json!(downloads));
    write_yaml(&path, &data)?;
    fs::create_dir_all(&downloads)?;
    println!("Soulseek config saved: {}", path.display());
    println!("  downloads: {}", downloads.display());
    Ok(())
}

pub fn edit(path: Option<&Path>) -> io::Result<()> {
    let path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(muzik_core::default_config_path);
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &path,
            "# Music library configuration (beets-compatible format)\n\
             directory: ~/music\n\
             library: ~/music/.library.db\n",
        )?;
    }
    let editor = env::var("EDITOR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env::var("VISUAL").ok())
        .unwrap_or_else(|| "vi".to_owned());
    let mut words = editor.split_whitespace();
    let executable = words
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "EDITOR is empty"))?;
    let status = Command::new(executable).args(words).arg(&path).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "editor exited with status {status}"
        )))
    }
}

fn read_yaml(path: &Path) -> io::Result<Value> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(error),
    };
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_saphyr::from_str(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_yaml(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_saphyr::to_string(value).map_err(io::Error::other)?;
    fs::write(path, text)
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;

    use super::set_library;

    #[test]
    fn set_library_keeps_beets_config_fields() {
        let temp = tempfile::tempdir().expect("create test directory");
        let config = temp.path().join("config.yaml");
        let library = temp.path().join("Music");
        fs::write(&config, "plugins:\n  - fetchart\nimport:\n  move: true\n")
            .expect("write config");

        set_library(&library, None, Some(&config)).expect("set library");

        let text = fs::read_to_string(config).expect("read config");
        let data: Value = serde_saphyr::from_str(&text).expect("parse config");
        assert_eq!(data["plugins"][0], "fetchart");
        assert_eq!(data["import"]["move"], true);
        let library = fs::canonicalize(library).expect("resolve library path");
        assert_eq!(data["directory"].as_str(), library.to_str());
        assert_eq!(
            data["library"].as_str(),
            library.join(".library.db").to_str()
        );
    }
}
