use crate::Result;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn load(connection: &Connection, root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    let mut statement = connection.prepare("SELECT destination, encoding FROM sync_files")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut files = BTreeMap::new();
    for row in rows {
        let (destination, encoding) = row?;
        let destination = PathBuf::from(destination);
        if destination.starts_with(root) {
            files.insert(destination, encoding);
        }
    }
    Ok(files)
}

pub fn save(connection: &Connection, destination: &Path, encoding: Option<&str>) -> Result<()> {
    let destination = destination.to_string_lossy();
    match encoding {
        None => connection.execute(
            "DELETE FROM sync_files WHERE destination = ?1",
            [destination],
        ),
        Some(encoding) => connection.execute(
            "INSERT INTO sync_files (destination, encoding) VALUES (?1, ?2)
             ON CONFLICT (destination) DO UPDATE SET encoding = excluded.encoding",
            (destination, encoding),
        ),
    }
    .map(drop)
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn load_returns_the_saved_text_under_the_root_only() -> Result<(), String> {
        let connection = db::open_in_memory()?;
        save(&connection, Path::new("/device/a.mp3"), Some("{\"mp3\":1}"))?;
        save(&connection, Path::new("/device/a.mp3"), Some("{\"mp3\":2}"))?;
        save(&connection, Path::new("/other/b.mp3"), Some("{\"mp3\":3}"))?;
        let files = load(&connection, Path::new("/device"))?;
        assert_eq!(
            files.into_iter().collect::<Vec<_>>(),
            vec![(PathBuf::from("/device/a.mp3"), "{\"mp3\":2}".to_owned())]
        );
        Ok(())
    }

    #[test]
    fn saving_no_encoding_removes_the_row() -> Result<(), String> {
        let connection = db::open_in_memory()?;
        save(&connection, Path::new("/device/a.mp3"), Some("{}"))?;
        save(&connection, Path::new("/device/a.mp3"), None)?;
        assert!(load(&connection, Path::new("/device"))?.is_empty());
        Ok(())
    }
}
