//! Transactional writes to the existing beets schema.

use crate::{Entity, Error, Fields, Library, Value};
use rusqlite::{params, params_from_iter, Connection, OpenFlags, OptionalExtension, Transaction};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct LibraryWrite<'a> {
    transaction: Transaction<'a>,
}

impl Library {
    /// Open a library for writes, creating a beets-compatible database if needed.
    pub fn open_or_create(path: &Path) -> Result<Self, Error> {
        if path.exists() {
            return Self::open_read_write(path);
        }
        let parent = path
            .parent()
            .ok_or_else(|| Error::InvalidPath(path.to_path_buf()))?;
        std::fs::create_dir_all(parent)?;
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        crate::register_functions(&connection)?;
        connection.execute_batch("BEGIN IMMEDIATE")?;
        connection.execute_batch(include_str!("schema.sql"))?;
        connection.execute_batch("COMMIT")?;
        Ok(Self {
            connection,
            path: path.to_path_buf(),
            writable: true,
        })
    }

    /// Open an existing database for writes. A backup is made before the first write.
    pub fn open_read_write(path: &Path) -> Result<Self, Error> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        crate::register_functions(&connection)?;
        Ok(Self {
            connection,
            path: path.to_path_buf(),
            writable: true,
        })
    }

    /// Run all writes in one transaction. An error rolls back every write.
    pub fn transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut LibraryWrite<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        self.ensure_backup()?;
        let transaction = self.connection.transaction()?;
        let mut writer = LibraryWrite { transaction };
        let value = operation(&mut writer)?;
        writer.transaction.commit()?;
        Ok(value)
    }

    pub fn insert_album(&mut self, fields: &Fields, attributes: &Fields) -> Result<i64, Error> {
        self.transaction(|writer| writer.insert_album(fields, attributes))
    }

    pub fn insert_item(&mut self, fields: &Fields, attributes: &Fields) -> Result<i64, Error> {
        self.transaction(|writer| writer.insert_item(fields, attributes))
    }

    pub fn update_album(
        &mut self,
        id: i64,
        fields: &Fields,
        attributes: &Fields,
    ) -> Result<(), Error> {
        self.transaction(|writer| writer.update_album(id, fields, attributes))
    }

    pub fn update_item(
        &mut self,
        id: i64,
        fields: &Fields,
        attributes: &Fields,
    ) -> Result<(), Error> {
        self.transaction(|writer| writer.update_item(id, fields, attributes))
    }

    pub fn remove_album(&mut self, id: i64) -> Result<(), Error> {
        self.transaction(|writer| writer.remove_album(id))
    }

    pub fn remove_item(&mut self, id: i64) -> Result<(), Error> {
        self.transaction(|writer| writer.remove_item(id))
    }

    /// Remove missing item records after checking the fraction. Files stay in place.
    pub fn prune_missing_items(
        &mut self,
        directory: &Path,
        safety_fraction: f64,
    ) -> Result<usize, Error> {
        if !safety_fraction.is_finite() || !(0.0..=1.0).contains(&safety_fraction) {
            return Err(Error::InvalidSafetyFraction(safety_fraction));
        }
        let items = self.items()?;
        let missing: Vec<i64> = items
            .iter()
            .filter_map(|item| {
                let path = item.field("path")?;
                let bytes = match path {
                    Value::Blob(bytes) => bytes.clone(),
                    Value::Text(text) => text.as_bytes().to_vec(),
                    _ => return Some(item.id),
                };
                let path = bytes_to_path(&bytes);
                let path = if path.is_absolute() {
                    path
                } else {
                    directory.join(path)
                };
                (!path.exists()).then_some(item.id)
            })
            .collect();
        if !items.is_empty() && (missing.len() as f64) > (items.len() as f64) * safety_fraction {
            return Err(Error::PruneAborted {
                missing: missing.len(),
                total: items.len(),
            });
        }
        if missing.is_empty() {
            return Ok(0);
        }
        self.transaction(|writer| {
            for id in &missing {
                writer.remove_item(*id)?;
            }
            Ok(missing.len())
        })
    }

    fn ensure_backup(&self) -> Result<(), Error> {
        let filename = self
            .path
            .file_name()
            .ok_or_else(|| Error::InvalidPath(self.path.clone()))?;
        let mut backup_name = filename.to_os_string();
        backup_name.push(".native-backup");
        let backup = self.path.with_file_name(backup_name);
        if !backup.exists() {
            self.connection
                .execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])?;
            tracing::info!(path = %backup.display(), "created beets library backup");
        }
        Ok(())
    }
}

impl LibraryWrite<'_> {
    pub fn insert_album(&mut self, fields: &Fields, attributes: &Fields) -> Result<i64, Error> {
        insert(&self.transaction, Entity::Album, fields, attributes)
    }

    pub fn insert_item(&mut self, fields: &Fields, attributes: &Fields) -> Result<i64, Error> {
        insert(&self.transaction, Entity::Item, fields, attributes)
    }

    pub fn update_album(
        &mut self,
        id: i64,
        fields: &Fields,
        attributes: &Fields,
    ) -> Result<(), Error> {
        update(&self.transaction, Entity::Album, id, fields, attributes)
    }

    pub fn update_item(
        &mut self,
        id: i64,
        fields: &Fields,
        attributes: &Fields,
    ) -> Result<(), Error> {
        update(&self.transaction, Entity::Item, id, fields, attributes)
    }

    pub fn remove_album(&mut self, id: i64) -> Result<(), Error> {
        let mut statement = self
            .transaction
            .prepare("SELECT id FROM items WHERE album_id = ?1")?;
        let item_ids = statement
            .query_map([id], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for item_id in item_ids {
            remove(&self.transaction, Entity::Item, item_id)?;
        }
        remove(&self.transaction, Entity::Album, id)
    }

    pub fn remove_item(&mut self, id: i64) -> Result<(), Error> {
        let album_id: Option<i64> = self
            .transaction
            .query_row("SELECT album_id FROM items WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?
            .flatten();
        remove(&self.transaction, Entity::Item, id)?;
        if let Some(album_id) = album_id {
            let remaining: i64 = self.transaction.query_row(
                "SELECT count(*) FROM items WHERE album_id = ?1",
                [album_id],
                |row| row.get(0),
            )?;
            if remaining == 0 {
                remove(&self.transaction, Entity::Album, album_id)?;
            }
        }
        Ok(())
    }
}

fn columns(connection: &Connection, table: &'static str) -> Result<BTreeSet<String>, Error> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?;
    names.collect::<Result<_, _>>().map_err(Error::from)
}

fn validate(connection: &Connection, table: &'static str, fields: &Fields) -> Result<(), Error> {
    let columns = columns(connection, table)?;
    for field in fields.keys() {
        if field == "id" || !columns.contains(field) {
            return Err(Error::InvalidField {
                table,
                field: field.clone(),
            });
        }
    }
    Ok(())
}

fn insert(
    connection: &Connection,
    entity: Entity,
    fields: &Fields,
    attributes: &Fields,
) -> Result<i64, Error> {
    let table = entity.table();
    validate(connection, table, fields)?;
    if fields.is_empty() {
        connection.execute(&format!("INSERT INTO {table} DEFAULT VALUES"), [])?;
    } else {
        let names = fields
            .keys()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let placeholders = vec!["?"; fields.len()].join(", ");
        let sql = format!("INSERT INTO {table} ({names}) VALUES ({placeholders})");
        connection.execute(&sql, params_from_iter(fields.values()))?;
    }
    let id = connection.last_insert_rowid();
    put_attributes(connection, entity, id, attributes)?;
    Ok(id)
}

fn update(
    connection: &Connection,
    entity: Entity,
    id: i64,
    fields: &Fields,
    attributes: &Fields,
) -> Result<(), Error> {
    let table = entity.table();
    validate(connection, table, fields)?;
    let exists: bool = connection.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = ?1)"),
        [id],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(Error::MissingRow { table, id });
    }
    if !fields.is_empty() {
        let assignments = fields
            .keys()
            .map(|name| format!("\"{name}\" = ?"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("UPDATE {table} SET {assignments} WHERE id = ?");
        let mut values: Vec<&Value> = fields.values().collect();
        let id_value = Value::Integer(id);
        values.push(&id_value);
        connection.execute(&sql, params_from_iter(values))?;
    }
    put_attributes(connection, entity, id, attributes)
}

fn remove(connection: &Connection, entity: Entity, id: i64) -> Result<(), Error> {
    let table = entity.table();
    let removed = connection.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [id])?;
    if removed == 0 {
        return Err(Error::MissingRow { table, id });
    }
    connection.execute(
        &format!(
            "DELETE FROM {} WHERE entity_id = ?1",
            entity.attribute_table()
        ),
        [id],
    )?;
    Ok(())
}

fn put_attributes(
    connection: &Connection,
    entity: Entity,
    id: i64,
    attributes: &Fields,
) -> Result<(), Error> {
    let table = entity.attribute_table();
    for (key, value) in attributes {
        connection.execute(
            &format!("INSERT INTO {table} (entity_id, key, value) VALUES (?1, ?2, ?3) ON CONFLICT(entity_id, key) DO UPDATE SET value = excluded.value"),
            params![id, key, value],
        )?;
    }
    Ok(())
}

#[cfg(unix)]
fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}
