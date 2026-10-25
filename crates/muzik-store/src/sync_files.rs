use crate::Result;
use rusqlite::Connection;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// # Errors
/// Returns an error if the sync records cannot be read.
pub fn read<E: DeserializeOwned>(
    connection: &Connection,
    root: &Path,
) -> Result<BTreeMap<PathBuf, E>> {
    Ok(load(connection, root)?
        .into_iter()
        .filter_map(|(destination, text)| Some((destination, serde_json::from_str(&text).ok()?)))
        .collect())
}

/// # Errors
/// Returns an error if the encoding cannot be serialized or the sync record cannot be written or removed.
pub fn write<E: Serialize>(
    connection: &Connection,
    destination: &Path,
    encoding: Option<&E>,
) -> Result<()> {
    let text = encoding.map(serde_json::to_string).transpose()?;
    save(connection, destination, text.as_deref())
}

/// # Errors
/// Returns an error if the sync records cannot be read.
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

/// # Errors
/// Returns an error if the sync record cannot be written or removed.
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
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Encoding {
        mp3: u32,
    }

    #[test]
    fn read_returns_the_saved_encodings_under_the_root_only() {
        let connection = db::open_in_memory().unwrap();
        write(
            &connection,
            Path::new("/device/a.mp3"),
            Some(&Encoding { mp3: 1 }),
        )
        .unwrap();
        write(
            &connection,
            Path::new("/device/a.mp3"),
            Some(&Encoding { mp3: 2 }),
        )
        .unwrap();
        write(
            &connection,
            Path::new("/other/b.mp3"),
            Some(&Encoding { mp3: 3 }),
        )
        .unwrap();
        save(&connection, Path::new("/device/c.mp3"), Some("not json")).unwrap();
        let files = read::<Encoding>(&connection, Path::new("/device")).unwrap();
        assert_eq!(
            files.into_iter().collect::<Vec<_>>(),
            vec![(PathBuf::from("/device/a.mp3"), Encoding { mp3: 2 })]
        );
        assert_eq!(
            load(&connection, Path::new("/device")).unwrap()[Path::new("/device/a.mp3")],
            "{\"mp3\":2}"
        );
    }

    #[test]
    fn writing_no_encoding_removes_the_row() {
        let connection = db::open_in_memory().unwrap();
        write(
            &connection,
            Path::new("/device/a.mp3"),
            Some(&Encoding { mp3: 1 }),
        )
        .unwrap();
        write::<Encoding>(&connection, Path::new("/device/a.mp3"), None).unwrap();
        assert!(load(&connection, Path::new("/device")).unwrap().is_empty());
    }
}
