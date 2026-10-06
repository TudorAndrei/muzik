//! Read the fixed and flexible fields in a beets SQLite library.

mod functions;
pub mod query;
mod values;
mod write;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row};
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use thiserror::Error;

pub use functions::register_functions;
pub use rusqlite::types::Value as SqlValue;
pub use values::{path_from_sql, path_to_sql, scalar_text};
pub use write::LibraryWrite;
use SqlValue as Value;

#[derive(Debug, Error)]
pub enum Error {
    #[error("SQLite library error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("library file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("beets {table} row has no id")]
    MissingId { table: &'static str },
    #[error("invalid beets query: {0}")]
    InvalidQuery(String),
    #[error("library is read only")]
    ReadOnly,
    #[error("invalid {table} field: {field}")]
    InvalidField { table: &'static str, field: String },
    #[error("beets {table} row {id} does not exist")]
    MissingRow { table: &'static str, id: i64 },
    #[error("invalid prune safety fraction: {0}")]
    InvalidSafetyFraction(f64),
    #[error("prune aborted: {missing}/{total} items are missing")]
    PruneAborted { missing: usize, total: usize },
    #[error("library path has no parent: {0}")]
    InvalidPath(PathBuf),
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
    path: PathBuf,
    writable: bool,
}

impl Library {
    /// Make a read-only empty library for planning before the first import.
    pub fn empty() -> Result<Self, Error> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(include_str!("schema.sql"))?;
        register_functions(&connection)?;
        Ok(Self {
            connection,
            path: PathBuf::from(":memory:"),
            writable: false,
        })
    }

    /// Open an existing beets database without changing its schema.
    pub fn open_read_only(path: &Path) -> Result<Self, Error> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        register_functions(&connection)?;
        tracing::debug!(path = %path.display(), "opened beets library");
        Ok(Self {
            connection,
            path: path.to_path_buf(),
            writable: false,
        })
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
                    attributes: self.attributes(Entity::Item, id)?,
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
                    attributes: self.attributes(Entity::Album, id)?,
                })
            })
            .transpose()
    }

    pub fn items(&self) -> Result<Vec<Item>, Error> {
        let rows = self.rows(Entity::Item)?;
        let attributes = self.all_attributes(Entity::Item)?;
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
        let rows = self.rows(Entity::Album)?;
        let attributes = self.all_attributes(Entity::Album)?;
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
            let id = row_id(&fields, Entity::Item.table())?;
            items.push(Item {
                id,
                fields,
                attributes: self.attributes(Entity::Item, id)?,
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

    fn rows(&self, entity: Entity) -> Result<Vec<(i64, Fields)>, Error> {
        let mut statement = self
            .connection
            .prepare(&format!("SELECT * FROM {} ORDER BY id", entity.table()))?;
        let rows = statement.query_map([], row_fields)?;
        rows.map(|row| {
            let fields = row?;
            let id = row_id(&fields, entity.table())?;
            Ok((id, fields))
        })
        .collect()
    }

    fn attributes(&self, entity: Entity, id: i64) -> Result<Fields, Error> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT key, value FROM {} WHERE entity_id = ?1",
            entity.attribute_table()
        ))?;
        let rows = statement.query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Value>(1)?))
        })?;
        rows.collect::<Result<Fields, _>>().map_err(Error::from)
    }

    fn all_attributes(&self, entity: Entity) -> Result<BTreeMap<i64, Fields>, Error> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT entity_id, key, value FROM {}",
            entity.attribute_table()
        ))?;
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

#[derive(Clone, Copy)]
enum Entity {
    Item,
    Album,
}

impl Entity {
    fn table(self) -> &'static str {
        match self {
            Self::Item => "items",
            Self::Album => "albums",
        }
    }

    fn attribute_table(self) -> &'static str {
        match self {
            Self::Item => "item_attributes",
            Self::Album => "album_attributes",
        }
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
