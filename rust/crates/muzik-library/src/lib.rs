//! Read the fixed and flexible fields in a beets SQLite library.

mod functions;
pub mod query;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

pub use functions::register_functions;
pub use rusqlite::types::Value as SqlValue;
use SqlValue as Value;

#[derive(Debug, Error)]
pub enum Error {
    #[error("SQLite library error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("beets {table} row has no id")]
    MissingId { table: &'static str },
    #[error("invalid beets query: {0}")]
    InvalidQuery(String),
}

pub type Fields = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: i64,
    pub fields: Fields,
    pub attributes: Fields,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    pub id: i64,
    pub fields: Fields,
    pub attributes: Fields,
}

impl Item {
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    pub fn attribute(&self, name: &str) -> Option<&Value> {
        self.attributes.get(name)
    }

    pub fn album_id(&self) -> Option<i64> {
        match self.field("album_id") {
            Some(Value::Integer(id)) => Some(*id),
            _ => None,
        }
    }
}

impl Album {
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    pub fn attribute(&self, name: &str) -> Option<&Value> {
        self.attributes.get(name)
    }
}

pub struct Library {
    connection: Connection,
}

impl Library {
    /// Open an existing beets database without changing its schema.
    pub fn open_read_only(path: &Path) -> Result<Self, Error> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        register_functions(&connection)?;
        tracing::debug!(path = %path.display(), "opened beets library");
        Ok(Self { connection })
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn item(&self, id: i64) -> Result<Option<Item>, Error> {
        let fields = self
            .connection
            .query_row("SELECT * FROM items WHERE id = ?1", [id], row_fields)
            .optional()?;
        fields
            .map(|fields| {
                Ok(Item {
                    id,
                    fields,
                    attributes: self.attributes("item_attributes", id)?,
                })
            })
            .transpose()
    }

    pub fn album(&self, id: i64) -> Result<Option<Album>, Error> {
        let fields = self
            .connection
            .query_row("SELECT * FROM albums WHERE id = ?1", [id], row_fields)
            .optional()?;
        fields
            .map(|fields| {
                Ok(Album {
                    id,
                    fields,
                    attributes: self.attributes("album_attributes", id)?,
                })
            })
            .transpose()
    }

    pub fn items(&self) -> Result<Vec<Item>, Error> {
        let rows = self.rows("SELECT * FROM items ORDER BY id", "items")?;
        let attributes = self.all_attributes("item_attributes")?;
        Ok(rows
            .into_iter()
            .map(|(id, fields)| Item {
                id,
                fields,
                attributes: attributes.get(&id).cloned().unwrap_or_default(),
            })
            .collect())
    }

    pub fn albums(&self) -> Result<Vec<Album>, Error> {
        let rows = self.rows("SELECT * FROM albums ORDER BY id", "albums")?;
        let attributes = self.all_attributes("album_attributes")?;
        Ok(rows
            .into_iter()
            .map(|(id, fields)| Album {
                id,
                fields,
                attributes: attributes.get(&id).cloned().unwrap_or_default(),
            })
            .collect())
    }

    pub fn items_for_album(&self, album_id: i64) -> Result<Vec<Item>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT * FROM items WHERE album_id = ?1 ORDER BY id")?;
        let rows = statement.query_map([album_id], row_fields)?;
        let mut items = Vec::new();
        for row in rows {
            let fields = row?;
            let id = row_id(&fields, "items")?;
            items.push(Item {
                id,
                fields,
                attributes: self.attributes("item_attributes", id)?,
            });
        }
        Ok(items)
    }

    pub fn query_items(&self, query_text: &str) -> Result<Vec<Item>, Error> {
        let query = query::Query::parse(query_text)?;
        let mut items = self.items()?;
        items.retain(|item| query.matches_item(item));
        query.sort_items(&mut items);
        Ok(items)
    }

    pub fn query_albums(&self, query_text: &str) -> Result<Vec<Album>, Error> {
        let query = query::Query::parse(query_text)?;
        let mut albums = self.albums()?;
        albums.retain(|album| query.matches_album(album));
        query.sort_albums(&mut albums);
        Ok(albums)
    }

    fn rows(&self, query: &str, table: &'static str) -> Result<Vec<(i64, Fields)>, Error> {
        let mut statement = self.connection.prepare(query)?;
        let rows = statement.query_map([], row_fields)?;
        rows.map(|row| {
            let fields = row?;
            let id = row_id(&fields, table)?;
            Ok((id, fields))
        })
        .collect()
    }

    fn attributes(&self, table: &'static str, id: i64) -> Result<Fields, Error> {
        let query = match table {
            "item_attributes" => "SELECT key, value FROM item_attributes WHERE entity_id = ?1",
            _ => "SELECT key, value FROM album_attributes WHERE entity_id = ?1",
        };
        let mut statement = self.connection.prepare(query)?;
        let rows = statement.query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Value>(1)?))
        })?;
        rows.collect::<Result<Fields, _>>().map_err(Error::from)
    }

    fn all_attributes(&self, table: &'static str) -> Result<BTreeMap<i64, Fields>, Error> {
        let query = match table {
            "item_attributes" => "SELECT entity_id, key, value FROM item_attributes",
            _ => "SELECT entity_id, key, value FROM album_attributes",
        };
        let mut statement = self.connection.prepare(query)?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Value>(2)?,
            ))
        })?;
        let mut attributes: BTreeMap<i64, Fields> = BTreeMap::new();
        for row in rows {
            let (id, key, value) = row?;
            attributes.entry(id).or_default().insert(key, value);
        }
        Ok(attributes)
    }
}

fn row_id(fields: &Fields, table: &'static str) -> Result<i64, Error> {
    match fields.get("id") {
        Some(Value::Integer(id)) => Ok(*id),
        _ => Err(Error::MissingId { table }),
    }
}

fn row_fields(row: &Row<'_>) -> rusqlite::Result<Fields> {
    let mut fields = Fields::new();
    for (index, column) in row.as_ref().column_names().iter().enumerate() {
        let value = match row.get_ref(index)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(value) => Value::Integer(value),
            ValueRef::Real(value) => Value::Real(value),
            ValueRef::Text(value) => Value::Text(String::from_utf8_lossy(value).into_owned()),
            ValueRef::Blob(value) => Value::Blob(value.to_vec()),
        };
        fields.insert((*column).to_string(), value);
    }
    Ok(fields)
}
